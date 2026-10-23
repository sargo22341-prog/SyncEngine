//! Tiny deterministic tweening used for every transition. Values are pure
//! functions of time; the UI subscribes to frames only while one is running.

use std::time::{Duration, Instant};

/// Easing curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ease {
    OutCubic,
    InOutCubic,
    /// Slight overshoot, for « pop » appearances.
    OutBack,
}

impl Ease {
    pub(crate) fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::OutCubic => 1.0 - (1.0 - t).powi(3),
            Self::InOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::OutBack => {
                let c1 = 1.701_58_f32;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
        }
    }
}

/// Animated scalar going from its current value to a target.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    ease: Ease,
}

impl Tween {
    pub(crate) fn new(value: f32, duration_ms: u64, ease: Ease) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
            duration: Duration::from_millis(duration_ms),
            ease,
        }
    }

    /// Interpolated value at `now`.
    pub(crate) fn at(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let total = self.duration.as_secs_f32();
        if total <= 0.0 {
            return self.to;
        }
        let k = self.ease.apply(elapsed / total);
        self.from + (self.to - self.from) * k
    }

    /// Retargets smoothly from the value currently displayed.
    pub(crate) fn go(&mut self, to: f32, now: Instant) {
        if (self.to - to).abs() < f32::EPSILON && !self.running(now) {
            return;
        }
        self.from = self.at(now);
        self.to = to;
        self.start = now;
    }

    /// Restarts from `from` toward `to` (entrance animations).
    pub(crate) fn replay(&mut self, from: f32, to: f32, now: Instant) {
        self.from = from;
        self.to = to;
        self.start = now;
    }

    pub(crate) fn target(&self) -> f32 {
        self.to
    }

    pub(crate) fn running(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) < self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_curves_start_at_zero_and_end_at_one() {
        for ease in [Ease::OutCubic, Ease::InOutCubic, Ease::OutBack] {
            assert!(ease.apply(0.0).abs() < 1e-5, "{ease:?}");
            assert!((ease.apply(1.0) - 1.0).abs() < 1e-5, "{ease:?}");
        }
        assert!(Ease::OutBack.apply(0.8) > 1.0, "overshoot expected");
    }

    #[test]
    fn tween_reaches_target_and_retargets_from_current_value() {
        let start = Instant::now();
        let mut tween = Tween::new(0.0, 200, Ease::OutCubic);
        tween.go(1.0, start);
        assert!(tween.running(start));
        let mid = tween.at(start + Duration::from_millis(100));
        assert!(mid > 0.0 && mid < 1.0);
        assert!((tween.at(start + Duration::from_millis(400)) - 1.0).abs() < 1e-6);
        let later = start + Duration::from_millis(100);
        tween.go(0.0, later);
        assert!(
            (tween.at(later) - mid).abs() < 1e-5,
            "no jump when retargeting"
        );
        assert!(!tween.running(later + Duration::from_millis(300)));
    }
}
