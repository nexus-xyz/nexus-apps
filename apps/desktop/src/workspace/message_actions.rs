//! Message actions fade in and out, then release their space beside the timestamp.
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, Instant};

const ENTER: Duration = crate::motion::UI;
const EXIT: Duration = Duration::from_millis(120); // --nx-dur-ui-exit

#[derive(Default)]
pub(super) struct Motion {
    shown: bool,
    epoch: usize,
    step: Option<Frame>,
}

#[derive(Clone, Copy)]
pub(super) struct Frame {
    from: f32,
    shown: bool,
    at: Instant,
    epoch: usize,
}

impl Frame {
    fn duration(self) -> Duration {
        if self.shown { ENTER } else { EXIT }
    }

    fn opacity(self, now: Instant) -> f32 {
        let time = now.saturating_duration_since(self.at).as_secs_f32() / self.duration().as_secs_f32();
        let progress = if self.shown {
            crate::motion::ease_out(time)
        } else {
            crate::motion::cubic_bezier(0.4, 0., 1., 1., time)
        };
        self.from + (f32::from(self.shown) - self.from) * progress
    }
}

impl Motion {
    /// Keep an interrupted fade's current opacity. Reduced motion reaches the target immediately.
    pub(super) fn update(&mut self, shown: bool, now: Instant, reduced: bool) -> Option<Frame> {
        if reduced {
            self.shown = shown;
            self.step = None;
        } else if shown != self.shown {
            let from = self.step.map_or(f32::from(self.shown), |step| step.opacity(now));
            self.shown = shown;
            self.epoch += 1;
            self.step = Some(Frame {
                from,
                shown,
                at: now,
                epoch: self.epoch,
            });
        }
        if self
            .step
            .is_some_and(|step| now.saturating_duration_since(step.at) >= step.duration())
        {
            self.step = None;
        }
        self.step.or_else(|| {
            self.shown.then_some(Frame {
                from: 1.,
                shown: true,
                at: now - ENTER,
                epoch: self.epoch,
            })
        })
    }
}

pub(super) fn render(seq: i64, actions: Div, count: usize, has_time: bool, frame: Option<Frame>) -> AnyElement {
    let Some(frame) = frame else {
        return div().into_any_element();
    };
    let width = 24. * count as f32 + 2. * count.saturating_sub(1) as f32;
    let gap = if has_time { 8. } else { 0. };
    let style = move |row: Stateful<Div>, opacity: f32| {
        // Layout snaps only at the fade boundaries. Animate opacity, as the design system requires.
        row.w(px(if opacity > 0. { width } else { 0. }))
            .ml(px(if opacity > 0. { gap } else { 0. }))
            .opacity(opacity)
            .when(opacity <= 0., |row| row.invisible())
    };
    let row = h_flex()
        .id(("message-actions", seq as u64))
        .h(px(24.))
        .flex_none()
        .overflow_hidden()
        .child(actions.w(px(width)).flex_none());
    if frame.at.elapsed() >= frame.duration() {
        return style(row, f32::from(frame.shown)).into_any_element();
    }
    row.with_animation(
        ("message-actions-fade", frame.epoch),
        Animation::new(frame.duration()),
        // Read elapsed time so unrelated transcript redraws cannot restart a fade.
        move |row, _| style(row, frame.opacity(Instant::now())),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn hidden_actions_release_space_after_the_exit() {
        let now = Instant::now();
        let mut motion = Motion::default();
        assert!(motion.update(false, now, false).is_none());
        assert_eq!(motion.update(true, now, false).unwrap().opacity(now), 0.);
        let visible = now + ENTER;
        assert_eq!(motion.update(true, visible, false).unwrap().opacity(visible), 1.);
        assert!(motion.update(false, visible, false).is_some());
        assert!(
            motion
                .update(false, visible + EXIT - Duration::from_millis(1), false)
                .is_some()
        );
        assert!(motion.update(false, visible + EXIT, false).is_none());
    }

    #[test]
    fn reversing_a_fade_preserves_opacity() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let entering = motion.update(true, now, false).unwrap();
        let midway = now + ENTER / 2;
        let opacity = entering.opacity(midway);
        let exiting = motion.update(false, midway, false).unwrap();
        assert!((exiting.opacity(midway) - opacity).abs() < 0.001);
        let reenter = midway + EXIT / 2;
        assert!(
            (motion.update(true, reenter, false).unwrap().opacity(reenter) - exiting.opacity(reenter)).abs() < 0.001
        );
    }

    #[test]
    fn reduced_motion_cancels_a_fade_and_collapses_at_once() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.update(true, now, false);
        assert_eq!(motion.update(true, now, true).unwrap().opacity(now), 1.);
        assert!(motion.update(false, now, true).is_none());
    }
}
