//! Fade of perceived brightness toward a target: ambient changes ramp at
//! a constant rate, faster up than down; user changes ease in
//! exponentially, so a stream of slider values blends into one motion.

use std::time::Duration;

/// Ambient-driven brightening, in p per second.
pub const AUTO_BRIGHTEN_RATE: f64 = 0.25;
/// Ambient-driven darkening, in p per second.
pub const AUTO_DARKEN_RATE: f64 = 0.10;
/// Time constant of the user easing.
pub const USER_TAU: Duration = Duration::from_millis(60);
/// A user easing this close to its target lands on it.
const USER_SNAP: f64 = 0.001;

/// How the fade moves toward its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Constant rate: [`AUTO_BRIGHTEN_RATE`] up, [`AUTO_DARKEN_RATE`] down.
    Auto,
    /// Exponential approach with time constant [`USER_TAU`].
    User,
}

/// Moves a perceived level toward a target.
#[derive(Clone, Debug)]
pub struct Fade {
    current: f64,
    target: f64,
    motion: Motion,
}

impl Fade {
    /// A fade resting at `current`.
    pub fn new(current: f64) -> Fade {
        Fade {
            current,
            target: current,
            motion: Motion::Auto,
        }
    }

    /// Heads toward `target` with `motion` from wherever the fade is now.
    /// A non-finite target is ignored.
    pub fn retarget(&mut self, target: f64, motion: Motion) {
        if !target.is_finite() {
            return;
        }
        self.target = target;
        self.motion = motion;
    }

    /// Advances by `dt` and returns the new current level.
    pub fn step(&mut self, dt: Duration) -> f64 {
        let diff = self.target - self.current;
        let dt = dt.as_secs_f64();
        match self.motion {
            Motion::Auto => {
                let rate = if diff > 0.0 {
                    AUTO_BRIGHTEN_RATE
                } else {
                    AUTO_DARKEN_RATE
                };
                let max = rate * dt;
                if diff.abs() <= max {
                    self.current = self.target;
                } else {
                    self.current += max.copysign(diff);
                }
            }
            Motion::User => {
                self.current += diff * (1.0 - (-dt / USER_TAU.as_secs_f64()).exp());
                if (self.target - self.current).abs() < USER_SNAP {
                    self.current = self.target;
                }
            }
        }
        self.current
    }

    /// Whether the fade has reached its target.
    pub fn done(&self) -> bool {
        self.current == self.target
    }

    pub fn current(&self) -> f64 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn auto_brightens_at_its_rate_and_lands() {
        let mut f = Fade::new(0.2);
        f.retarget(0.8, Motion::Auto);
        let p = f.step(MS(1000));
        assert!((p - (0.2 + AUTO_BRIGHTEN_RATE)).abs() < 1e-9, "{p}");
        assert!(!f.done());
        assert_eq!(f.step(MS(5000)), 0.8);
        assert!(f.done());
    }

    #[test]
    fn auto_darkens_at_its_rate_and_lands() {
        let mut f = Fade::new(0.8);
        f.retarget(0.2, Motion::Auto);
        let p = f.step(MS(1000));
        assert!((p - (0.8 - AUTO_DARKEN_RATE)).abs() < 1e-9, "{p}");
        assert_eq!(f.step(MS(10_000)), 0.2);
        assert!(f.done());
    }

    #[test]
    fn user_eases_toward_target_and_snaps() {
        let mut f = Fade::new(0.2);
        f.retarget(0.6, Motion::User);
        // Over one time constant, in frames, about 63 % of the gap.
        let mut p = 0.0;
        for _ in 0..6 {
            p = f.step(USER_TAU / 6);
        }
        let covered = (p - 0.2) / 0.4;
        assert!(
            (covered - (1.0 - (-1.0f64).exp())).abs() < 1e-9,
            "{covered}"
        );
        let mut frames = 0;
        while !f.done() {
            f.step(MS(16));
            frames += 1;
            assert!(frames < 100, "never converged: {}", f.current());
        }
        assert_eq!(f.current(), 0.6);
    }

    #[test]
    fn user_does_not_move_in_zero_time() {
        let mut f = Fade::new(0.2);
        f.retarget(0.6, Motion::User);
        assert_eq!(f.step(Duration::ZERO), 0.2);
    }

    #[test]
    fn auto_retarget_mid_fade_reverses_from_current() {
        let mut f = Fade::new(0.5);
        f.retarget(1.0, Motion::Auto);
        f.step(MS(400)); // 0.6
        f.retarget(0.0, Motion::Auto);
        assert!((f.step(MS(1000)) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn retarget_switches_motion_without_jumping() {
        let mut f = Fade::new(0.5);
        f.retarget(1.0, Motion::Auto);
        let at = f.step(MS(400));
        f.retarget(0.2, Motion::User);
        assert_eq!(f.current(), at);
        let p = f.step(USER_TAU);
        let want = at + (0.2 - at) * (1.0 - (-1.0f64).exp());
        assert!((p - want).abs() < 1e-9, "{p} vs {want}");
        f.retarget(0.9, Motion::Auto);
        assert_eq!(f.current(), p);
        assert!((f.step(MS(100)) - (p + 0.1 * AUTO_BRIGHTEN_RATE)).abs() < 1e-9);
    }

    #[test]
    fn non_finite_target_is_ignored() {
        let mut f = Fade::new(0.5);
        f.retarget(f64::NAN, Motion::User);
        f.retarget(f64::INFINITY, Motion::Auto);
        assert!(f.done());
        assert_eq!(f.step(MS(100)), 0.5);
    }
}
