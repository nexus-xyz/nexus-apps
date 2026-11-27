//! Tabs that come and go. A tab that joins a strip that already has tabs grows in from nothing over
//! [`crate::motion::UI_LONG`]. A closed tab is drawn once more, narrowing to nothing over [`crate::motion::UI_EXIT`].
//! Both fade, so the neighbours slide aside instead of jumping. Under reduced motion both happen in one frame: the
//! caller draws neither.

use super::*;
use std::time::Instant;

/// A closed tab, drawn while it leaves.
#[derive(Clone)]
pub(super) struct Leaving {
    scope: String,
    pub(super) index: usize,
    label: &'static str,
    icon: Lucide,
    since: Instant,
}

/// Which tabs of which strip move now. Read and written while the strip draws, as its order is (`ordered_tabs`).
#[derive(Default)]
pub(super) struct TabMotion {
    joined: RefCell<HashMap<(String, Tab), Instant>>,
    leaving: RefCell<Option<Leaving>>,
}

impl TabMotion {
    /// `tab` joined `scope`'s strip.
    pub(super) fn join(&self, scope: &str, tab: Tab) {
        self.joined
            .borrow_mut()
            .insert((scope.to_string(), tab), Instant::now());
    }

    /// The tab at `index` of `scope`'s strip closed.
    pub(super) fn leave(&self, scope: &str, index: usize, label: &'static str, icon: Lucide) {
        self.joined.borrow_mut().retain(|(joined, _), _| joined != scope);
        *self.leaving.borrow_mut() = Some(Leaving {
            scope: scope.to_string(),
            index,
            label,
            icon,
            since: Instant::now(),
        });
    }

    /// When `tab` joined `scope`'s strip, while it still grows.
    pub(super) fn joining(&self, scope: &str, tab: Tab) -> Option<Instant> {
        let mut joined = self.joined.borrow_mut();
        joined.retain(|_, since| since.elapsed() < crate::motion::UI_LONG);
        joined.get(&(scope.to_string(), tab)).copied()
    }

    /// The tab that leaves `scope`'s strip, while it still narrows.
    pub(super) fn leaving(&self, scope: &str) -> Option<Leaving> {
        let mut leaving = self.leaving.borrow_mut();
        if leaving
            .as_ref()
            .is_some_and(|tab| tab.since.elapsed() >= crate::motion::UI_EXIT)
        {
            *leaving = None;
        }
        leaving.clone().filter(|tab| tab.scope == scope)
    }
}

/// About how wide a tab named `label` is: its padding and icon, about 7px a letter at 12.5px, and a ×. The strip's
/// compact rule (`render_side_pane`) counts the same way.
fn width(label: &str) -> f32 {
    36. + 7. * label.len() as f32 + 20.
}

/// `tab`, growing into the strip since it joined at `since`. The frame is never narrower than the tab once grown.
pub(super) fn grow(tab: AnyElement, key: usize, label: &str, since: Instant) -> AnyElement {
    let width = width(label);
    div()
        .flex_none()
        .overflow_hidden()
        .child(tab)
        .with_animation(
            SharedString::from(format!("side-tab-join-{key}-{since:?}")),
            Animation::new(crate::motion::UI_LONG).with_easing(crate::motion::ease_out),
            move |frame, t| {
                if t < 1. {
                    frame.max_w(px(width * t)).opacity(t)
                } else {
                    frame
                }
            },
        )
        .into_any_element()
}

/// The closed tab, narrowing to nothing and fading. It takes no clicks.
pub(super) fn shrink(leaving: &Leaving, palette: Palette) -> AnyElement {
    let width = width(leaving.label);
    h_flex()
        .flex_none()
        .h(px(28.))
        .pl(px(8.))
        .gap(px(6.))
        .rounded(px(7.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(12.5))
        .text_color(palette.muted)
        .child(Icon::new(leaving.icon).size(px(14.)))
        .child(leaving.label)
        .with_animation(
            SharedString::from(format!("side-tab-leave-{:?}", leaving.since)),
            Animation::new(crate::motion::UI_EXIT).with_easing(crate::motion::standard),
            move |ghost, t| ghost.w(px(width * (1. - t))).opacity(1. - t),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_closed_tab_leaves_only_its_own_strip_and_ends_the_joins_there() {
        let motion = TabMotion::default();
        motion.join("a", Tab::Tool(Tool::Browser));
        motion.join("b", Tab::Tool(Tool::Browser));
        motion.leave("a", 1, "Terminal", Lucide::SquareTerminal);
        assert_eq!(motion.leaving("a").map(|tab| tab.index), Some(1));
        assert!(motion.leaving("b").is_none());
        assert!(motion.joining("a", Tab::Tool(Tool::Browser)).is_none());
        assert!(motion.joining("b", Tab::Tool(Tool::Browser)).is_some());
    }

    #[test]
    fn the_moves_end() {
        let motion = TabMotion::default();
        let long_ago = Instant::now() - crate::motion::UI_LONG * 2;
        motion
            .joined
            .borrow_mut()
            .insert(("a".into(), Tab::Tool(Tool::Browser)), long_ago);
        assert!(motion.joining("a", Tab::Tool(Tool::Browser)).is_none());
        motion.leave("a", 0, "Browser", Lucide::Globe);
        motion.leaving.borrow_mut().as_mut().unwrap().since = long_ago;
        assert!(motion.leaving("a").is_none());
        assert!(motion.leaving.borrow().is_none(), "an ended move keeps no record");
    }
}
