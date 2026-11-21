//! Ambient light smoothing, the "worth moving" dead band, and the sensor's
//! integration-time range with hysteresis.

use std::time::Duration;

use crate::model::LUX_FLOOR;

/// Exponential moving average of ambient lux in log10 space, with a faster
/// time constant for brightening than for darkening.
#[derive(Clone, Debug)]
pub struct Smoother {
    rise: Duration,
    fall: Duration,
    /// Smoothed `log10(lux)`, `None` until the first sample.
    log: Option<f64>,
}

impl Smoother {
    /// `rise` is the time constant used when the sample is brighter than the
    /// current value, `fall` when it is darker.
    pub fn new(rise: Duration, fall: Duration) -> Smoother {
        Smoother {
            rise,
            fall,
            log: None,
        }
    }

    /// Feeds one lux sample taken `dt` after the previous one and returns the
    /// smoothed lux. The first sample is taken as-is; non-finite samples
    /// count as `LUX_FLOOR`.
    pub fn push(&mut self, lux: f64, dt: Duration) -> f64 {
        let lux = if lux.is_finite() { lux } else { LUX_FLOOR };
        let x = lux.max(LUX_FLOOR).log10();
        let next = match self.log {
            None => x,
            Some(cur) => {
                let tau = if x > cur { self.rise } else { self.fall };
                let alpha = if tau.is_zero() {
                    1.0
                } else {
                    1.0 - (-dt.as_secs_f64() / tau.as_secs_f64()).exp()
                };
                cur + alpha * (x - cur)
            }
        };
        self.log = Some(next);
        10f64.powf(next)
    }

    /// The current smoothed lux, if any sample has been pushed.
    pub fn value(&self) -> Option<f64> {
        self.log.map(|x| 10f64.powf(x))
    }

    /// Forgets the history; the next sample is taken as-is.
    pub fn reset(&mut self) {
        self.log = None;
    }
}

/// Whether a change from `current` to `target` (perceived) is big enough to
/// act on.
pub fn worth_moving(current: f64, target: f64) -> bool {
    (target - current).abs() >= 0.02
}

/// Light sensor integration time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// 0.1 s integration, for bright light.
    Fast,
    /// 0.8 s integration, for dim light.
    Sensitive,
}

impl Range {
    /// The value written to the sensor's `integration_time` sysfs attribute.
    pub fn sysfs(self) -> &'static str {
        match self {
            Range::Fast => "0.1",
            Range::Sensitive => "0.8",
        }
    }
}

/// Picks the sensor range from smoothed lux, with hysteresis between 8 and 12.
#[derive(Clone, Debug)]
pub struct Ranger {
    current: Range,
}

impl Ranger {
    pub fn new(start: Range) -> Ranger {
        Ranger { current: start }
    }

    /// Returns the new range when it should switch, else `None`.
    pub fn next(&mut self, smoothed_lux: f64) -> Option<Range> {
        let want = match self.current {
            Range::Sensitive if smoothed_lux > 12.0 => Range::Fast,
            Range::Fast if smoothed_lux < 8.0 => Range::Sensitive,
            _ => return None,
        };
        self.current = want;
        Some(want)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn secs(x: f64) -> Duration {
        Duration::from_secs_f64(x)
    }

    #[test]
    fn first_sample_is_taken() {
        let mut s = Smoother::new(secs(2.0), secs(8.0));
        assert_eq!(s.push(10.0, secs(1.0)), 10.0);
    }

    #[test]
    fn brightening_is_faster_than_darkening() {
        let mut up = Smoother::new(secs(2.0), secs(8.0));
        up.push(1.0, secs(1.0));
        let mut down = Smoother::new(secs(2.0), secs(8.0));
        down.push(100.0, secs(1.0));
        let u = up.push(100.0, secs(1.0)).log10(); // moved from 0 toward 2
        let d = 2.0 - down.push(1.0, secs(1.0)).log10(); // moved from 2 toward 0
        assert!(u > d * 2.0);
    }

    #[test]
    fn zero_lux_is_floored() {
        let mut s = Smoother::new(secs(2.0), secs(8.0));
        assert_eq!(s.push(0.0, secs(1.0)), 0.01);
    }

    #[test]
    fn dead_band() {
        assert!(!worth_moving(0.50, 0.515));
        assert!(worth_moving(0.50, 0.52));
    }

    #[test]
    fn ranger_has_hysteresis() {
        let mut r = Ranger::new(Range::Sensitive);
        assert_eq!(r.next(10.0), None);
        assert_eq!(r.next(13.0), Some(Range::Fast));
        assert_eq!(r.next(10.0), None);
        assert_eq!(r.next(7.0), Some(Range::Sensitive));
    }
}
