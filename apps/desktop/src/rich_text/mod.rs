//! Shared rich Markdown in the existing native web host.
mod assets;
mod corner;
mod math;
#[cfg(test)]
mod view_tests;
use crate::motion;
use crate::palette::Palette;
use crate::web::{WebEvent, WebView};
use base64::Engine as _;
use corner::{Action, Place};
use gpui_kit::component::text::{MarkdownExtensions, MarkdownNode, TextViewState, TextViewStyle, markdown_ast};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use motion::TweenExt as _;
use std::{
    collections::{HashMap, VecDeque},
    sync::OnceLock,
    time::Instant,
};
struct Block {
    source: String,
    diagram: bool,
    code: String,
}
/// Re-parsing the final Markdown block preserves its renderer; each window keeps at most 32 recent diagrams.
#[derive(Default)]
struct RichViews(HashMap<WindowId, VecDeque<(String, Entity<RichBlock>)>>);
impl Global for RichViews {}

pub fn window_closed(cx: &mut App, id: WindowId) {
    if cx.has_global::<RichViews>() {
        cx.global_mut::<RichViews>().0.remove(&id);
    }
}

fn complete_diagram(source: &str, language: &str) -> bool {
    matches!(language, "mermaid" | "mmd") && complete_fence(source)
}
fn complete_fence(source: &str) -> bool {
    source
        .lines()
        .next()
        .and_then(|line| line.trim_start().strip_prefix("```"))
        .is_some_and(|rest| !rest.starts_with('`'))
        && source.lines().count() >= 3
        && source.lines().last().is_some_and(|line| {
            let line = line.trim();
            line.len() >= 3 && line.chars().all(|c| c == '`')
        })
}
pub fn extensions() -> MarkdownExtensions {
    static EXT: OnceLock<MarkdownExtensions> = OnceLock::new();
    EXT.get_or_init(|| {
        MarkdownExtensions::default()
            .block_parser(|node, cx| {
                let source = cx.node_source(node)?;
                let diagram = matches!(node, markdown_ast::Node::Code(code) if complete_diagram(source, code.lang.as_deref().unwrap_or_default()));
                let math = match node {
                    markdown_ast::Node::Code(code) => code.lang.as_deref() == Some("math") && complete_fence(source),
                    markdown_ast::Node::Math(_) | markdown_ast::Node::Paragraph(_) => math::contains_math(source),
                    _ => false,
                };
                if !diagram && !math {return None;}
                Some(
                    MarkdownNode::new(
                        "nexus-rich",
                        Block {
                            source: source.into(),
                            diagram,
                            code: match node {
                                markdown_ast::Node::Code(code) if diagram => code.value.clone(),
                                _ => source.to_owned(),
                            },
                        },
                    )
                    .text(source.to_owned())
                    .markdown(source.to_owned())
                    .accessibility_label(if diagram {"Mermaid diagram"} else {"Mathematical text"}),
                )
            })
            .block_renderer("nexus-rich", |node, window, cx| {
                let data = node.data::<Block>().expect("rich block payload");
                let id = window.window_handle().window_id();
                let cached = {
                    let entries = cx.default_global::<RichViews>().0.entry(id).or_default();
                    entries
                        .iter()
                        .position(|(source, _)| source == &data.source)
                        .and_then(|index| entries.remove(index))
                };
                let (source, view) = cached.unwrap_or_else(|| {
                    (
                        data.source.clone(),
                        cx.new(|cx| RichBlock::new(data.source.clone(), data.code.clone(), data.diagram, window, cx)),
                    )
                });
                let entries = cx.default_global::<RichViews>().0.entry(id).or_default();
                entries.push_back((source, view.clone()));
                while entries.len() > 32 {
                    entries.pop_front();
                }
                div().w_full().child(view)
            })
    })
    .clone()
}
fn status(title: &str) -> Option<(f32, bool)> {
    let mut fields = title.strip_prefix("nx-rich:")?.split(':');
    let height: f32 = fields.next()?.parse().ok()?;
    if !height.is_finite() || height < 0. {
        return None;
    }
    let failed = match fields.next()? {
        "ready" | "pending" => false,
        "failed" => true,
        _ => return None,
    };
    if fields.next().is_some() {
        return None;
    }
    Some((height.clamp(36., 2400.), failed))
}
fn source_block(source: &str) -> String {
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}text\n{source}\n{fence}")
}
/// One rich block of an answer: math or a diagram, drawn by the web host on the transcript's own surface.
///
/// The page has no background (`WebView::clear_with_startup`), so it reads as the text around it. Until it drew, the
/// block shows its words natively in the muted ink. Under the pointer, a corner over its top right offers the code
/// and Copy (`corner.rs`); the code shows in a well like a code block's, with the corner in its head.
struct RichBlock {
    source: String,
    diagram: bool,
    /// What Copy copies and the code view shows: a diagram's code, or the paragraph's Markdown with its math.
    code: SharedString,
    /// The code view's text, selectable.
    fallback: Entity<TextViewState>,
    /// The words shown while the page draws.
    placeholder: SharedString,
    web: Option<Entity<WebView>>,
    height: f32,
    /// The page drew: it shows in place of the placeholder.
    shown: bool,
    show_code: bool,
    failed: bool,
    /// The pointer is over the block, or over its floating corner.
    hovered: bool,
    corner_hovered: bool,
    /// When the view last changed (drawn, code shown or hidden), and how many times: the new view fades in once.
    changed: Option<Instant>,
    switches: u64,
    _events: Option<Subscription>,
    _deadline: Task<()>,
}
impl RichBlock {
    fn new(source: String, code: String, diagram: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fallback = cx.new(|cx| TextViewState::markdown(&source_block(&code), cx));
        let placeholder = if diagram {
            "Drawing the diagram…".into()
        } else {
            math::placeholder(&source).into()
        };
        let mut result = Self {
            source,
            code: code.into(),
            diagram,
            fallback,
            placeholder,
            web: None,
            height: 36.,
            shown: false,
            show_code: false,
            failed: false,
            hovered: false,
            corner_hovered: false,
            changed: None,
            switches: 0,
            _events: None,
            _deadline: Task::ready(()),
        };
        result.load(window, cx);
        result
    }
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(origin) = assets::origin() else {
            self.failed = true;
            return;
        };
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let palette = palette(cx, dark);
        let data = serde_json::json!({
            "source": self.source,
            "dark": palette.dark,
            "colors": page_colors(palette),
        })
        .to_string();
        if data.len() > 512 * 1024 {
            self.failed = true;
            return;
        }
        let url = format!("{origin}#{}", base64::engine::general_purpose::STANDARD.encode(data));
        let web = cx.new(|cx| {
            WebView::clear_with_startup(
                &url,
                crate::web::Startup {
                    source: "",
                    binding: None,
                },
                window,
                cx,
            )
        });
        self.failed = false;
        self.shown = false;
        self._events = Some(cx.subscribe(&web, |this, web, event: &WebEvent, cx| match event {
            WebEvent::TitleChanged(title) => {
                if let Some(encoded) = title.strip_prefix("nx-link:")
                    && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded)
                    && let Ok(link) = String::from_utf8(bytes)
                {
                    crate::external_link::open(&link, cx);
                }
                if let Some((height, failed)) = status(title) {
                    this.height = height;
                    if failed && !this.failed {
                        this.switched(cx);
                    }
                    this.failed = failed;
                    if !title.ends_with(":pending") {
                        this._deadline = Task::ready(());
                        // A page that drew but keeps loading still shows once it says it is done.
                        web.update(cx, |web, cx| web.reveal_if_drawn(cx));
                    }
                    cx.notify();
                }
            }
            WebEvent::Shown => {
                this.shown = true;
                this.switched(cx);
                cx.notify();
            }
            _ => {}
        }));
        self.web = Some(web);
        self._deadline = cx.spawn(async |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
            if this
                .update(cx, |this, cx| {
                    if !this.shown {
                        this.failed = true;
                        this.switched(cx);
                        cx.notify();
                    }
                })
                .is_err()
            {
                // The block is gone: nothing waits on it.
            }
        });
    }
    /// The view changed: the new one fades in.
    fn switched(&mut self, cx: &App) {
        self.changed = Some(cx.background_executor().now());
        self.switches += 1;
    }
    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.show_code = !self.show_code;
        // The corner of the view left behind is not under the pointer any more.
        self.corner_hovered = false;
        self.switched(cx);
        cx.notify();
    }
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_code = false;
        self.load(window, cx);
        self.switched(cx);
        cx.notify();
    }
    /// The middle button's click, on this block.
    fn on_action(&self, retry: bool, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, window, cx| {
            if this
                .update(
                    cx,
                    |this, cx| if retry { this.retry(window, cx) } else { this.toggle(cx) },
                )
                .is_err()
            {
                // The block closed under the click: nothing to switch.
            }
        }
    }
    /// The drawing, on the transcript's surface: the hover wash under it and the floating corner over its top right.
    fn drawing(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let floating = self.hovered || self.corner_hovered;
        let corner = corner::corner(
            Place::Float,
            self.diagram,
            Action::Toggle { showing_code: false },
            self.code.clone(),
            palette,
            self.on_action(false, cx),
        )
        .when(floating, |corner| {
            corner.on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.corner_hovered != *hovered {
                    this.corner_hovered = *hovered;
                    cx.notify();
                }
            }))
        })
        .with_tween(
            "rich-corner-fade",
            if floating { 1. } else { 0. },
            corner::fade(floating),
            // Gone at rest: an invisible corner takes no clicks.
            |corner, shown| {
                if shown <= 0. {
                    corner.invisible()
                } else {
                    corner.opacity(shown)
                }
            },
        );
        let page = self.web.clone();
        div()
            .id("rich-area")
            .relative()
            .w_full()
            .debug_selector(|| "rich-area".into())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.hovered != *hovered {
                    this.hovered = *hovered;
                    cx.notify();
                }
            }))
            // The wash reaches past the words, as a row's does.
            .child(
                div()
                    .absolute()
                    .top(px(-4.))
                    .bottom(px(-4.))
                    .left(px(-8.))
                    .right(px(-8.))
                    .child(motion::hover_fill("rich-wash", palette.hover, corner::RADIUS)),
            )
            .when(!self.shown, |area| {
                // The page lays out and draws under the words, unseen (veiled), at the column's width.
                area.child(
                    div()
                        .w_full()
                        .text_color(palette.muted)
                        .debug_selector(|| "rich-placeholder".into())
                        .child(self.placeholder.clone()),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(self.height))
                        .children(page.clone()),
                )
            })
            .when(self.shown, |area| {
                area.child(div().w_full().h(px(self.height)).children(page))
            })
            .child(corner)
            .into_any_element()
    }
    /// The code, in a well like a code block's, with the corner in its head; a failure says so under it.
    fn code_view(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let action = if self.failed {
            Action::Retry
        } else {
            Action::Toggle { showing_code: true }
        };
        // The well holds the code plainly: the text view's own code block draws no fill, frame or padding of its own.
        let plain = StyleRefinement::default()
            .p_0()
            .border_0()
            .bg(gpui_kit::transparent_black())
            .font_family("Geist Mono")
            .text_size(px(13.))
            .line_height(px(20.));
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(
                div()
                    .relative()
                    .w_full()
                    .px(px(12.))
                    .pt(px(32.))
                    .pb(px(12.))
                    .rounded(px(10.))
                    .bg(palette.sidebar)
                    .debug_selector(|| "rich-code".into())
                    .child(
                        crate::external_link::text_view(&self.fallback)
                            .selectable(true)
                            .style(TextViewStyle::default().code_block(plain)),
                    )
                    .child(corner::corner(
                        Place::Head,
                        self.diagram,
                        action,
                        self.code.clone(),
                        palette,
                        self.on_action(self.failed, cx),
                    )),
            )
            .when(self.failed, |column| {
                column.child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child(if self.diagram {
                            "Could not draw the diagram."
                        } else {
                            "Could not draw the math."
                        }),
                )
            })
            .into_any_element()
    }
}
/// The colors the transcript draws with now; the block's own theme before the window set them.
fn palette(cx: &App, dark: bool) -> Palette {
    cx.try_global::<crate::pages::Theme>()
        .map(|theme| theme.0)
        .unwrap_or_else(|| Palette::solid_for(dark))
}
/// The page's ink from the app's palette (`--nx-*` in `rich-text/style.css`), as `#rrggbbaa`.
fn page_colors(palette: Palette) -> serde_json::Value {
    let hex = |color: Hsla| {
        let rgba = Rgba::from(color);
        let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            byte(rgba.r),
            byte(rgba.g),
            byte(rgba.b),
            byte(rgba.a)
        )
    };
    serde_json::json!({
        "fg-strong": hex(palette.foreground),
        "fg-muted": hex(palette.muted),
        "line": hex(palette.border),
        "bg-subtle": hex(palette.sidebar),
    })
}
impl Render for RichBlock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx, false);
        let body = if self.show_code || self.failed {
            self.code_view(palette, cx)
        } else {
            self.drawing(palette, cx)
        };
        // The view that just came fades in; one drawn again later (scrolled back into the list) does not.
        let now = cx.background_executor().now();
        let fresh = self.changed.is_some_and(|at| now.duration_since(at) < motion::UI);
        let body = div().w_full().child(body);
        let id = ElementId::Integer(self.switches);
        if fresh {
            motion::fade_in(body, id).into_any_element()
        } else {
            // Keep the buttons' element IDs when the fade ends, including during a click.
            body.id(id).into_any_element()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{complete_diagram, source_block, status};
    #[test]
    fn incomplete_or_other_code_stays_native() {
        assert!(complete_diagram("```mermaid\ngraph TD; A-->B\n```", "mermaid"));
        assert!(!complete_diagram("~~~mmd\ngraph TD; A-->B\n~~~", "mmd"));
        assert!(!complete_diagram("```mermaid\ngraph TD; A-->B", "mermaid"));
        assert!(!complete_diagram("```rust\nlet a=1;\n```", "rust"));
    }
    #[test]
    fn source_fallback_preserves_delimiters_and_embedded_fences() {
        assert_eq!(source_block("$$x$$"), "```text\n$$x$$\n```");
        assert!(source_block("```math\nx\n```").starts_with("````text\n```math"));
    }
    #[test]
    fn page_messages_are_bounded() {
        assert_eq!(status("nx-rich:99:ready"), Some((99., false)));
        assert_eq!(status("nx-rich:99999:failed"), Some((2400., true)));
        for title in [
            "nx-rich:NaN:ready",
            "nx-rich:-1:ready",
            "nx-rich:10:oops",
            "nx-rich:10:ready:extra",
        ] {
            assert_eq!(status(title), None);
        }
    }
}
