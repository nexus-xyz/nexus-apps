//! Command tray presence uses the design system's UI entrance and exit tokens.
use gpui_kit::*;
use std::time::{Duration, Instant};

const EXIT: Duration = Duration::from_millis(120); // --nx-dur-ui-exit

#[derive(Default)]
pub struct Motion {
    shown: bool,
    step: Option<(Instant, f32)>,
}

impl Motion {
    fn duration(&self) -> Duration {
        if self.shown { crate::motion::UI } else { EXIT }
    }

    fn opacity(&self, now: Instant) -> f32 {
        let Some((at, from)) = self.step else {
            return f32::from(self.shown);
        };
        let time = now.saturating_duration_since(at).as_secs_f32() / self.duration().as_secs_f32();
        let progress = if self.shown {
            crate::motion::ease_out(time)
        } else {
            crate::motion::cubic_bezier(0.4, 0., 1., 1., time)
        };
        from + (f32::from(self.shown) - from) * progress
    }

    fn update(&mut self, shown: bool, now: Instant, reduced: bool) -> f32 {
        if reduced {
            self.shown = shown;
            self.step = None;
        } else if self.shown != shown {
            let from = self.opacity(now);
            self.shown = shown;
            self.step = Some((now, from));
        }
        if self
            .step
            .is_some_and(|(at, _)| now.saturating_duration_since(at) >= self.duration())
        {
            self.step = None;
        }
        self.opacity(now)
    }

    pub fn visible(&self) -> bool {
        self.shown || self.step.is_some_and(|(at, _)| at.elapsed() < self.duration())
    }

    pub fn sample<T: 'static>(&mut self, shown: bool, cx: &mut Context<T>) -> f32 {
        let opacity = self.update(shown, Instant::now(), cx.reduce_motion());
        // Each timer redraws the owner, including the final frame that unmounts the tray.
        if self.step.is_some() {
            cx.spawn(async move |owner, cx| {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let _ = owner.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        opacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn exit_stays_mounted_then_releases_the_tray() {
        let now = Instant::now();
        let mut motion = Motion::default();
        assert_eq!(motion.update(true, now, false), 0.);
        assert_eq!(motion.update(true, now + crate::motion::UI, false), 1.);
        let closing = now + crate::motion::UI;
        assert_eq!(motion.update(false, closing, false), 1.);
        assert!(motion.visible());
        assert_eq!(motion.update(false, closing + EXIT, false), 0.);
        assert!(!motion.visible());
    }

    #[test]
    fn reversal_keeps_current_opacity_and_reduced_motion_finishes_immediately() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.update(true, now, false);
        let midway = now + crate::motion::UI / 2;
        let opacity = motion.opacity(midway);
        assert_eq!(motion.update(false, midway, false), opacity);
        assert_eq!(motion.update(true, midway, true), 1.);
        assert_eq!(motion.update(false, midway, true), 0.);
        assert!(!motion.visible());
    }
}
