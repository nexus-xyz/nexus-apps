//! A code block's corner in an answer: its language, then Copy, as Nexus UI's code block head
//! (`ui/src/components/nexus-ui/code-block.tsx`), where "Copied" holds 2 s. In a Code session on this computer, a
//! finished shell block also has Run, as the Nexus app's (`terminal-command.tsx`): it runs the
//! command in a new terminal of the Workbench, named after the command.
//!
//! The corner sits on the block's own fill with no fill of its own. gpui-kit wraps the corner in a box
//! filled with its theme's accent, so the corner is a zero-size anchor and its row hangs from it: the kit's box has no
//! area to paint. Over a see-through window the stacked fills read as a lighter capsule around the language and Copy.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::text::CodeBlock;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::motion::{self, Tween, TweenExt as _};
use crate::palette::Palette;

/// How long "Copied" shows, as Nexus UI's `COPIED_MS`.
const COPIED_FOR: Duration = Duration::from_millis(2000);

/// The fence languages Run offers on, as the Nexus app's `SHELLS`.
const SHELLS: [&str; 9] = ["sh", "bash", "shell", "zsh", "fish", "powershell", "ps1", "cmd", "bat"];

/// The longest command Run offers on, in UTF-16 units as the Nexus app counts (`code.length <= 32000`).
const MAX_COMMAND: usize = 32_000;

/// Runs a command in the session's terminal. The Code session gives one; every other answer has none.
pub(crate) type RunCommand = Arc<dyn Fn(SharedString, &mut Window, &mut App) + Send + Sync>;

thread_local! {
    /// The code copied last, and when: its block says "Copied" for a moment.
    static COPIED: RefCell<Option<(SharedString, Instant)>> = const { RefCell::new(None) };
}

/// Whether Run shows on a block of `code` fenced as `language`: a shell, with a command that isn't blank or huge.
pub(crate) fn runnable(language: &str, code: &str) -> bool {
    SHELLS.contains(&language.to_lowercase().as_str())
        && !code.trim().is_empty()
        && code.encode_utf16().count() <= MAX_COMMAND
}

fn copied(code: &SharedString) -> bool {
    COPIED.with(|copied| {
        copied
            .borrow()
            .as_ref()
            .is_some_and(|(what, at)| what == code && at.elapsed() < COPIED_FOR)
    })
}

/// Copy `code`, show "Copied" on its block, and take it away after [`COPIED_FOR`].
fn copy(code: SharedString, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
    COPIED.with(|copied| *copied.borrow_mut() = Some((code, Instant::now())));
    // A refresh draws cached views again too, as the transcript's pane is.
    window.refresh();
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = cx.update(|window, _| window.refresh());
        })
        .detach();
}

/// A button's height and corner: Copy is a square of this side, Run a text button this tall.
const BUTTON: f32 = 24.;
const RADIUS: f32 = 8.;
/// The icons' size, centred in the button's square.
const ICON: f32 = 14.;
/// How small an icon starts as it swaps in, and ends as it swaps out.
const SWAP_FROM: f32 = 0.6;

/// The check settling in: brand v5's `easing.settle`, `cubic-bezier(0, 0, 0.2, 1)`.
fn settle(t: f32) -> f32 {
    motion::cubic_bezier(0., 0., 0.2, 1., t)
}

/// Copy to Copied in `motion::HOVER` on the settle curve, and back in `motion::UI_EXIT`.
fn swap(done: bool) -> Tween {
    if done {
        Tween::new(motion::HOVER, settle)
    } else {
        Tween::new(motion::UI_EXIT, motion::standard)
    }
}

/// Run: a 24 text button, `play` 14 and "Run" 12, shared by chat and Code.
fn run_button(palette: Palette) -> Stateful<Div> {
    h_flex()
        .id("code-run")
        .relative()
        .h(px(BUTTON))
        .pl(px(4.))
        .pr(px(8.))
        .gap(px(4.))
        .rounded(px(RADIUS))
        .text_size(px(12.))
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.foreground))
        .child(motion::hover_fill("code-run-fill", palette.hover, RADIUS))
        .child(Icon::new(Lucide::Play).size(px(ICON)).text_color(palette.icon))
        .child("Run")
}

/// An icon button of the corner: a 24 square, the 14 icon centred, the hover fill fading under the pointer, and its
/// name as a tooltip. An answer's math uses it for "Show LaTeX" and Retry (`rich_text`).
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: Lucide,
    tip: &'static str,
    palette: Palette,
) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(BUTTON))
        .rounded(px(RADIUS))
        .cursor_pointer()
        .child(motion::hover_fill(
            ElementId::Name(format!("{id}-fill").into()),
            palette.hover,
            RADIUS,
        ))
        .child(Icon::new(icon).size(px(ICON)).text_color(palette.icon))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
}

/// Copy's icon in its own 24 square: the copy glyph, or the check while "Copied" shows. One shrinks out as the other
/// settles in, both centred in the same square, so the check lands where the glyph was.
fn copy_glyph(done: bool, palette: Palette) -> impl IntoElement {
    let layer = |icon: Lucide, shown: f32, color: Hsla| {
        let size = ICON * (SWAP_FROM + (1. - SWAP_FROM) * shown);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .opacity(shown)
            .child(Icon::new(icon).size(px(size)).text_color(color))
    };
    div()
        .relative()
        .flex_none()
        .size(px(BUTTON))
        .debug_selector(|| "code-copy-glyph".into())
        .with_tween(
            "code-copy-swap",
            if done { 1. } else { 0. },
            swap(done),
            move |square, t| {
                square
                    .when(t < 1., |square| square.child(layer(Lucide::Copy, 1. - t, palette.icon)))
                    .when(t > 0., |square| {
                        square.child(layer(Lucide::Check, t, palette.foreground))
                    })
            },
        )
}

/// Copy: the glyph's square, and "Copied" growing in to its left for [`COPIED_FOR`]. Error messages use it too
/// (`code_view.rs`, `transcript.rs`).
pub(crate) fn copy_button(code: SharedString, palette: Palette) -> impl IntoElement {
    let done = copied(&code);
    let said = done.then(|| {
        div()
            .pl(px(8.))
            .text_size(px(12.))
            .line_height(px(BUTTON))
            .text_color(palette.foreground)
            .debug_selector(|| "code-copied".into())
            .child("Copied")
            .into_any_element()
    });
    h_flex()
        .id("code-copy")
        .relative()
        .h(px(BUTTON))
        .rounded(px(RADIUS))
        .cursor_pointer()
        .child(motion::hover_fill("code-copy-fill", palette.hover, RADIUS))
        .child(motion::reveal_width("code-copied-word", done, said))
        .child(copy_glyph(done, palette))
        .tooltip(move |window, cx| Tooltip::new(if done { "Copied" } else { "Copy" }).build(window, cx))
        .debug_selector(|| "code-copy".into())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            copy(code.clone(), window, cx);
        })
}

/// The block's head, at its top right: the language in Geist Mono 12, faint, then Run (when `run` is given and the
/// block is a shell command), then Copy, all on one 24 line with no fill behind them.
pub(crate) fn actions(block: &CodeBlock, palette: Palette, run: Option<&RunCommand>) -> AnyElement {
    let code = block.code();
    let language = block.lang().filter(|language| !language.is_empty());
    let run = run
        .filter(|_| language.as_ref().is_some_and(|language| runnable(language, &code)))
        .cloned();
    let command = code.clone();
    let row = h_flex()
        .absolute()
        // gpui-kit sets the anchor 8 in from the block's top and right: the row then sits 4 in, centred on the
        // block's 32 head.
        .top(px(-4.))
        .right(px(-4.))
        .h(px(BUTTON))
        .gap(px(4.))
        .font_family(".SystemUIFont")
        .debug_selector(|| "code-actions".into())
        .children(language.map(|language| {
            div()
                .pr(px(4.))
                .font_family("Geist Mono")
                .text_size(px(12.))
                .line_height(px(BUTTON))
                .text_color(palette.faint)
                .debug_selector(|| "code-language".into())
                .child(language)
        }))
        .children(run.map(|run| {
            run_button(palette)
                .debug_selector(|| "code-run".into())
                .tooltip(|window, cx| Tooltip::new("Run command in Terminal").build(window, cx))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    run(command.clone(), window, cx);
                })
        }))
        .child(copy_button(code, palette));
    // A zero-size anchor: gpui-kit's box around it takes this size, so its accent fill has no area to paint.
    div()
        .relative()
        .size_0()
        .debug_selector(|| "code-anchor".into())
        .child(row)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{MAX_COMMAND, SHELLS, runnable};

    #[test]
    fn run_offers_on_the_nexus_apps_shells_only() {
        for language in SHELLS {
            assert!(runnable(language, "echo hello"), "{language}");
        }
        assert!(runnable("Bash", "ls"), "the fence's case does not matter");
        for language in ["rust", "python", "console", "", "text"] {
            assert!(!runnable(language, "echo hello"), "{language}");
        }
    }

    #[test]
    fn run_does_not_offer_on_a_blank_or_huge_command() {
        assert!(!runnable("bash", "  \n\t"));
        assert!(runnable("bash", &"a".repeat(MAX_COMMAND)));
        assert!(!runnable("bash", &"a".repeat(MAX_COMMAND + 1)));
    }
}

#[cfg(test)]
mod view_tests {
    use std::sync::{Arc, Mutex};

    use gpui_kit::component::text::{TextView, TextViewState};
    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render, SharedString, Styled,
        TestAppContext, VisualTestContext, Window, WindowAppearance, div, px,
    };

    use super::RunCommand;
    use crate::palette::{Palette, ThemeChoice};

    struct Answer {
        text: Entity<TextViewState>,
        run: Option<RunCommand>,
    }

    impl Render for Answer {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark);
            div().w(px(640.)).child(crate::workspace::transcript::prose_running(
                TextView::new(&self.text),
                palette,
                self.run.clone(),
            ))
        }
    }

    fn answer<'a>(source: &str, run: Option<RunCommand>, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
        cx.update(gpui_kit::init);
        let source = source.to_string();
        let (_, cx) = cx.add_window_view(|_, cx| Answer {
            text: cx.new(|cx| TextViewState::markdown(&source, cx)),
            run,
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn copy_puts_the_blocks_code_on_the_clipboard(cx: &mut TestAppContext) {
        let cx = answer("Try this:\n\n```rust\nfn main() {}\n```\n", None, cx);
        assert!(cx.debug_bounds("code-run").is_none(), "Rust is not a shell");
        click(cx, "code-copy");
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("fn main() {}"));
        assert!(super::copied(&"fn main() {}".into()), "its Copy says Copied");
    }

    #[gpui_kit::test]
    fn run_hands_a_shell_blocks_command_to_the_session(cx: &mut TestAppContext) {
        let ran: Arc<Mutex<Vec<SharedString>>> = Arc::default();
        let seen = ran.clone();
        let run: RunCommand = Arc::new(move |command, _, _| seen.lock().unwrap().push(command));
        let cx = answer("```bash\nnpm test\n```\n", Some(run), cx);
        click(cx, "code-run");
        assert_eq!(*ran.lock().unwrap(), vec![SharedString::from("npm test")]);
    }

    #[gpui_kit::test]
    fn the_corner_leaves_gpui_kits_box_no_area_to_fill(cx: &mut TestAppContext) {
        let cx = answer("```markdown\n## Install\n```\n", None, cx);
        let anchor = cx.debug_bounds("code-anchor").expect("the corner is drawn");
        assert_eq!(anchor.size.width, px(0.), "a box sized to the anchor paints nothing");
        assert_eq!(anchor.size.height, px(0.));
        let row = cx.debug_bounds("code-actions").unwrap();
        assert_eq!(
            row.right(),
            anchor.right() + px(4.),
            "the row sits 4 in from the block's right"
        );
        assert_eq!(row.top(), anchor.top() - px(4.), "and centred on the block's 32 head");
    }

    #[gpui_kit::test]
    fn copy_is_a_24_square_with_its_icon_centred_on_the_languages_line(cx: &mut TestAppContext) {
        let cx = answer("```markdown\n## Install\n```\n", None, cx);
        let copy = cx.debug_bounds("code-copy").unwrap();
        assert_eq!((copy.size.width, copy.size.height), (px(24.), px(24.)));
        assert_eq!(
            cx.debug_bounds("code-copy-glyph").unwrap(),
            copy,
            "the icon's square is the button"
        );
        let language = cx.debug_bounds("code-language").unwrap();
        assert_eq!(language.center().y, copy.center().y, "one centre line");
        assert!(cx.debug_bounds("code-copied").is_none());
    }

    #[gpui_kit::test]
    fn copied_grows_in_beside_a_check_that_stays_where_the_glyph_was(cx: &mut TestAppContext) {
        let cx = answer("```rust\nfn main() {}\n```\n", None, cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        let before = cx.debug_bounds("code-copy-glyph").unwrap();
        click(cx, "code-copy");
        let copy = cx.debug_bounds("code-copy").unwrap();
        let word = cx.debug_bounds("code-copied").expect("Copied shows");
        assert!(word.right() <= before.left(), "the word grows to the glyph's left");
        assert!(copy.size.width > px(24.));
        assert_eq!(
            cx.debug_bounds("code-copy-glyph").unwrap(),
            before,
            "the check lands where the glyph was"
        );
    }

    #[gpui_kit::test]
    fn run_is_not_offered_outside_a_code_session(cx: &mut TestAppContext) {
        let cx = answer("```bash\nnpm test\n```\n", None, cx);
        assert!(cx.debug_bounds("code-copy").is_some());
        assert!(cx.debug_bounds("code-run").is_none());
    }
}
