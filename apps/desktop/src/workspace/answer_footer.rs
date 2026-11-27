//! The transcript parts Code and chat share: the quiet line under an answer (how it ended, and the model's
//! friendly name and the cost on hover), and the 24px icon buttons that sit in transcript footers and message
//! actions. One look in both: chat and Code call these rather than drawing their own.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::glyph;
use crate::palette::Palette;

/// Icon buttons inside transcript blocks, footers and message actions: 24 square, a 14 icon.
pub(super) const HOVER_BUTTON: f32 = 24.;

/// A 24px icon button with its tooltip: faint icon, radius 6, the hover fill under the pointer.
pub(super) fn hover_button(
    id: impl Into<ElementId>,
    icon: Lucide,
    tip: impl Into<SharedString>,
    palette: Palette,
) -> Stateful<Div> {
    let tip = tip.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(tip.clone())
        .size(px(HOVER_BUTTON))
        .flex_none()
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.7))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(glyph(icon, palette.faint))
}

/// The cost as the footer says it: two places ("$0.16"), "<$0.01" under a cent.
pub(super) fn cost(usd: f64) -> String {
    if usd > 0. && usd < 0.005 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

/// Whose mark a model id carries: Claude's ids are Anthropic's, GPT and Codex ids OpenAI's, Gemini's Google's.
pub(super) fn maker_of(model: &str) -> Option<&'static str> {
    let model = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    if model.starts_with("claude") || ["opus", "sonnet", "haiku"].iter().any(|name| model.starts_with(name)) {
        Some("anthropic")
    } else if model.starts_with("gpt") || model.contains("codex") || model.starts_with("o3") || model.starts_with("o4")
    {
        Some("openai")
    } else if model.starts_with("gemini") {
        Some("google")
    } else {
        None
    }
}

/// The footer's words: "Opus 5.5 · $0.16", then " · Stopped" (or another ending) when the turn didn't finish.
/// The cost drops when nothing was billed.
pub(super) fn words(model: Option<&str>, usd: Option<f64>, ended: Option<&str>) -> String {
    [
        model.map(str::to_string).filter(|model| !model.is_empty()),
        usd.filter(|usd| *usd > 0.).map(cost),
        ended.map(str::to_string),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// The quiet line under an answer, 12/16 faint. A turn that stopped or failed says so, `tip` (model, cost, time,
/// tokens) on hover, `failed` in the danger ink. One that ended well has no words: its `tip` shows in their place
/// while the answer is `hovered`, and keeps its place hidden, so nothing moves. No maker's mark: the composer's model
/// button carries it.
pub(super) fn line(
    id: impl Into<ElementId>,
    words: String,
    tip: Option<SharedString>,
    failed: bool,
    hovered: bool,
    palette: Palette,
) -> Stateful<Div> {
    let quiet = words.is_empty();
    let (words, tip): (SharedString, _) = if quiet {
        (tip.unwrap_or_default(), None)
    } else {
        (words.into(), tip)
    };
    h_flex()
        .id(id)
        .min_w(px(0.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(if failed { palette.destructive } else { palette.faint })
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .when(quiet && !hovered, |words| words.invisible())
                .child(words),
        )
        .when_some(tip, |line, tip| {
            line.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        })
}

#[cfg(test)]
mod tests {
    use super::{cost, maker_of, words};

    #[test]
    fn the_footer_reads_model_then_cost_then_how_it_ended() {
        assert_eq!(words(Some("Opus 5.5"), Some(0.16), None), "Opus 5.5 · $0.16");
        assert_eq!(words(Some("Opus 5.5"), Some(0.), Some("Stopped")), "Opus 5.5 · Stopped");
        assert_eq!(words(None, None, Some("Failed")), "Failed");
        assert_eq!(cost(0.004), "<$0.01");
        assert_eq!(cost(1.0), "$1.00");
    }

    #[test]
    fn a_model_id_names_its_maker() {
        assert_eq!(maker_of("claude-opus-5-5"), Some("anthropic"));
        assert_eq!(maker_of("anthropic/claude-sonnet-4-5"), Some("anthropic"));
        assert_eq!(maker_of("gpt-5.1-codex"), Some("openai"));
        assert_eq!(maker_of("mystery"), None);
    }
}
