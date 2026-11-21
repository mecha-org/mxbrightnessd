//! Brightness model: the ambient curve, perceived ↔ raw mapping, and the
//! user's levels (auto offset, manual level, minimum).
//!
//! All levels are perceived brightness `p ∈ [0, 1]`; raw values only exist
//! at the sysfs boundary.

/// Lux below this is treated as this before taking log10.
pub const LUX_FLOOR: f64 = 0.01;

/// Ambient lux → perceived brightness, piecewise linear in log10(lux) and
/// flat outside its first and last points.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// `(log10(max(lux, LUX_FLOOR)), p)`, strictly increasing in the first field.
    points: Vec<(f64, f64)>,
}

fn log_lux(lux: f64) -> f64 {
    lux.max(LUX_FLOOR).log10()
}

impl Curve {
    /// Builds a curve from `(lux, p)` pairs: at least one, lux > 0 and
    /// strictly increasing, p in [0, 1].
    pub fn new(points: Vec<(f64, f64)>) -> Result<Curve, String> {
        if points.is_empty() {
            return Err("curve needs at least one point".into());
        }
        let mut out: Vec<(f64, f64)> = Vec::with_capacity(points.len());
        for (lux, p) in points {
            if !(lux.is_finite() && lux > 0.0) {
                return Err(format!("curve lux {lux} must be finite and > 0"));
            }
            if !(0.0..=1.0).contains(&p) {
                return Err(format!("curve level {p} at {lux} lux must be in [0, 1]"));
            }
            let x = log_lux(lux);
            if let Some(&(prev, _)) = out.last()
                && x <= prev
            {
                return Err(format!(
                    "curve lux {lux} must be greater than the previous point (and above {LUX_FLOOR})"
                ));
            }
            out.push((x, p));
        }
        Ok(Curve { points: out })
    }

    /// Perceived brightness for `lux`.
    pub fn at(&self, lux: f64) -> f64 {
        let x = log_lux(lux);
        let first = self.points[0];
        let last = self.points[self.points.len() - 1];
        if x <= first.0 {
            return first.1;
        }
        if x >= last.0 {
            return last.1;
        }
        let i = self.points.partition_point(|&(px, _)| px <= x);
        let (x0, p0) = self.points[i - 1];
        let (x1, p1) = self.points[i];
        p0 + (p1 - p0) * (x - x0) / (x1 - x0)
    }
}

impl Default for Curve {
    fn default() -> Self {
        Curve::new(vec![
            (0.05, 0.10),
            (0.3, 0.30),
            (3.0, 0.50),
            (30.0, 0.75),
            (300.0, 1.0),
        ])
        .expect("default curve is valid")
    }
}

/// Perceived `p` → raw backlight value: `round(max · p^gamma)` clamped to
/// `[1, max]`. Never 0, so the panel is never switched off by a level.
pub fn to_raw(p: f64, gamma: f64, max: u32) -> u32 {
    let raw = (max as f64 * p.clamp(0.0, 1.0).powf(gamma)).round();
    // NaN casts to 0, which the clamp lifts to 1.
    (raw as u32).clamp(1, max.max(1))
}

/// Raw backlight value → perceived `p`: `(raw / max)^(1/gamma)`.
pub fn from_raw(raw: u32, gamma: f64, max: u32) -> f64 {
    if max == 0 {
        return 0.0;
    }
    (raw.min(max) as f64 / max as f64).powf(1.0 / gamma)
}

/// The user's brightness state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// Follow ambient light.
    pub auto: bool,
    /// Added to the curve in auto mode, in [-1, 1].
    pub offset: f64,
    /// Level used when not in auto mode (or no lux is known).
    pub manual: f64,
    /// Floor for every target.
    pub minimum: f64,
}

impl Default for Levels {
    fn default() -> Self {
        Levels {
            auto: true,
            offset: 0.0,
            manual: 0.5,
            minimum: 0.05,
        }
    }
}

impl Levels {
    /// The level to show: the curve plus offset in auto mode with a lux
    /// reading, otherwise the manual level; always within `[minimum, 1]`.
    pub fn target(&self, curve: &Curve, lux: Option<f64>) -> f64 {
        let p = match (self.auto, lux) {
            (true, Some(lux)) => curve.at(lux) + self.offset,
            _ => self.manual,
        };
        p.clamp(self.minimum, 1.0)
    }

    /// Records a user-chosen level. In auto mode with a lux reading it is
    /// stored as an offset from the curve (so it follows the light);
    /// otherwise as the manual level. Returns the clamped level.
    pub fn user_set(&mut self, p: f64, curve: &Curve, lux: Option<f64>) -> f64 {
        let p = p.clamp(self.minimum, 1.0);
        match (self.auto, lux) {
            (true, Some(lux)) => self.offset = (p - curve.at(lux)).clamp(-1.0, 1.0),
            _ => self.manual = p,
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_hits_points_and_is_flat_outside() {
        let c = Curve::default();
        assert!((c.at(0.3) - 0.30).abs() < 1e-9);
        assert!((c.at(300.0) - 1.0).abs() < 1e-9);
        assert!((c.at(1e6) - 1.0).abs() < 1e-9);
        assert!((c.at(0.0) - 0.10).abs() < 1e-9); // below floor → first point
    }

    #[test]
    fn curve_is_linear_in_log_lux() {
        let c = Curve::default(); // midpoint of 0.3..3 in log space is ~0.9487
        assert!((c.at(0.9486833) - 0.40).abs() < 1e-6);
    }

    #[test]
    fn curve_rejects_unsorted_or_out_of_range() {
        assert!(Curve::new(vec![(1.0, 0.5), (0.5, 0.6)]).is_err());
        assert!(Curve::new(vec![(1.0, 1.5)]).is_err());
        assert!(Curve::new(vec![]).is_err());
    }

    #[test]
    fn raw_mapping_round_trips() {
        assert_eq!(to_raw(1.0, 2.2, 255), 255);
        assert_eq!(to_raw(0.5, 2.2, 255), 55);
        assert!((from_raw(55, 2.2, 255) - 0.5).abs() < 0.01);
    }

    #[test]
    fn raw_never_zero() {
        assert_eq!(to_raw(0.0, 2.2, 255), 1);
        assert_eq!(to_raw(-1.0, 2.2, 255), 1);
    }

    #[test]
    fn minimum_floors_auto_and_manual() {
        let c = Curve::default();
        let l = Levels {
            auto: true,
            offset: -1.0,
            manual: 0.0,
            minimum: 0.05,
        };
        assert_eq!(l.target(&c, Some(0.3)), 0.05);
        let m = Levels { auto: false, ..l };
        assert_eq!(m.target(&c, None), 0.05);
    }

    #[test]
    fn user_set_in_auto_stores_offset_that_follows_light() {
        let c = Curve::default();
        let mut l = Levels::default();
        assert_eq!(l.user_set(0.40, &c, Some(0.3)), 0.40);
        assert!((l.offset - 0.10).abs() < 1e-9);
        assert!((l.target(&c, Some(3.0)) - 0.60).abs() < 1e-9);
    }

    #[test]
    fn user_set_without_lux_sets_manual() {
        let c = Curve::default();
        let mut l = Levels {
            auto: false,
            ..Levels::default()
        };
        assert_eq!(l.user_set(0.01, &c, None), 0.05);
        assert_eq!(l.manual, 0.05);
    }
}
