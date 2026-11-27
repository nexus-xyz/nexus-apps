//! Whether the workbench follows the agent: the words its button's tooltip says, the action it offers (Pause, or
//! Follow agent), and its mark. Clicking inside the workbench pauses it on its own, so Manual is mostly a state the
//! person falls into; the button is the way back.
use super::Target;
use crate::motion::{self, Tween, TweenExt as _};
use crate::palette::Palette;
use gpui_kit::prelude::*;
use gpui_kit::{IntoElement, div, px, rgb};

/// The live mark's green (`--nx-live`, brand v5).
const LIVE: u32 = 0x30D158;

/// What the line says: its headline, the detail after it, and its action's label.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Status {
    pub headline: &'static str,
    pub detail: Option<String>,
    pub action: &'static str,
}

/// The line's words for a mode, the count of updates the person has not seen, and the newest item.
pub(super) fn status(manual: bool, unread: usize, latest: Option<&Target>) -> Status {
    let label = latest.map(Target::label);
    if !manual {
        return Status {
            headline: "Following agent",
            detail: label,
            action: "Pause",
        };
    }
    let detail = match (unread, label) {
        (0, label) => label,
        (count, Some(label)) => Some(format!("{count} new · {label}")),
        (count, None) => Some(format!("{count} new")),
    };
    Status {
        headline: "Manual",
        detail,
        action: "Follow agent",
    }
}

/// The mode's mark: a filled green dot while following, a hollow ring in Manual. The fill and the ring cross-fade over
/// `UI` when following starts and `UI_EXIT` when it stops; the mode a session loads with sits still.
pub(super) fn mark(manual: bool, palette: Palette) -> impl IntoElement {
    let live = gpui_kit::Hsla::from(rgb(LIVE));
    let ring = palette.icon;
    let tween = Tween::new(if manual { motion::UI_EXIT } else { motion::UI }, motion::ease_out);
    div().flex_none().size(px(7.)).rounded_full().border_1().with_tween(
        "workbench-follow-mark",
        if manual { 0. } else { 1. },
        tween,
        move |mark, t| mark.bg(live.opacity(t)).border_color(ring.opacity(1. - t)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_names_what_it_shows_and_offers_pause() {
        let page = Target::Page("pag_1".into());
        assert_eq!(
            status(false, 0, Some(&page)),
            Status {
                headline: "Following agent",
                detail: Some(page.label()),
                action: "Pause",
            }
        );
        assert_eq!(status(false, 3, None).detail, None);
    }

    #[test]
    fn manual_counts_what_it_missed_and_offers_the_way_back() {
        let changes = Target::Changes("src/a.rs".into());
        let missed = status(true, 2, Some(&changes));
        assert_eq!(missed.headline, "Manual");
        assert_eq!(missed.action, "Follow agent");
        assert_eq!(missed.detail, Some(format!("2 new · {}", changes.label())));
        assert_eq!(status(true, 0, Some(&changes)).detail, Some(changes.label()));
        assert_eq!(status(true, 1, None).detail.as_deref(), Some("1 new"));
        assert_eq!(status(true, 0, None).detail, None);
    }
}
