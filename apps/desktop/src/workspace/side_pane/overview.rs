//! The Workbench's overview: while the Workbench is closed in a started session or a saved chat, a column of its tools
//! in the main area's top right, each a click away. A click opens the Workbench on that tool, as its shortcut does.
//!
//! It moves with the pane. When the Workbench opens, the rows fade and slide right, top row first, before the pane
//! covers them. When it closes, the rows come back once the pane has mostly left. Under reduced motion, both swap in
//! one frame (`TweenExt::with_tween`).
//!
//! It keeps out of the conversation's text: beside a gutter wide enough, each row shows its icon and name; beside a
//! narrower one, its icon only; beside none, it hides. Either way it sits left of the minimap's rail (`minimap.rs`).
//! A row's tooltip names it with its shortcut, so a row keeps no room for a hint and its name is never cut.
//!
//! Its rows are the Workbench's entries (`entries.rs`), as "+" offers them: the tools under their tabs' names, with
//! their tabs' unread dot and Agents count, then New terminal under a hairline.

use super::entries::Entry;
use super::*;
use crate::motion::Tween;
use std::time::Instant;

/// The named column fits the Background tasks label, icon, gap and padding.
const WIDTH: f32 = 158.;
/// The column's width with icons only: a row's padding and its icon.
const ICONS_WIDTH: f32 = 35.;
/// From the main area's right edge: past the minimap's rail (12 in and 24 wide), with 8 of air.
const RIGHT: f32 = 44.;
/// From the main area's top.
const TOP: f32 = 12.;
/// The least room between the conversation's text and the column.
const AIR: f32 = 12.;
/// A row's height.
const ROW: f32 = 30.;
/// How far a row slides as it leaves or comes back.
const TRAVEL: f32 = 12.;
/// The most rows the column holds: the head, six tools, Side chat, the hairline and New terminal. The timings count
/// them all, so a row's moment does not depend on how many show.
const ROWS: usize = 10;
/// Opening: each row leaves over `motion::UI_EXIT`, the next one 12 ms later.
const OUT_MS: f32 = 120.;
const OUT_STAGGER_MS: f32 = 12.;
/// Closing: the rows wait while the pane leaves (`motion::PANEL`, 280 ms), then come back over `motion::UI_LONG`,
/// 22 ms apart.
const IN_DELAY_MS: f32 = 200.;
const IN_MS: f32 = 200.;
const IN_STAGGER_MS: f32 = 22.;

/// The whole move, from the first row's start to the last row's end, in ms.
fn total_ms(showing: bool) -> f32 {
    let rest = (ROWS - 1) as f32;
    if showing {
        IN_DELAY_MS + IN_STAGGER_MS * rest + IN_MS
    } else {
        OUT_STAGGER_MS * rest + OUT_MS
    }
}

/// Row `index`'s level, from 0 (gone) to 1 (shown), when the whole column is `group` of the way through its move
/// (0 gone, 1 shown), toward shown (`showing`) or toward gone.
fn row_level(group: f32, index: usize, showing: bool) -> f32 {
    let total = total_ms(showing);
    if showing {
        let ms = group * total - IN_DELAY_MS - IN_STAGGER_MS * index as f32;
        crate::motion::ease_out((ms / IN_MS).clamp(0., 1.))
    } else {
        let ms = (1. - group) * total - OUT_STAGGER_MS * index as f32;
        1. - crate::motion::standard((ms / OUT_MS).clamp(0., 1.))
    }
}

/// How the column draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Each row's icon and name, under a head.
    Names,
    /// Each row's icon only, named by its tooltip.
    Icons,
}

impl Layout {
    fn width(self) -> f32 {
        match self {
            Layout::Names => WIDTH,
            Layout::Icons => ICONS_WIDTH,
        }
    }
}

/// How the column fits a main area `room` px wide, with the conversation's text `column` px wide in its middle: the
/// widest layout whose rows stay clear of the text, or None. Code blocks and tables may grow wider than the text
/// (`chat_width.rs`); they are rare at the top right, and holding the whole gutter for them hid the column in the
/// wider presets.
fn layout(room: f32, column: f32) -> Option<Layout> {
    let gutter = (room - column) / 2.;
    [Layout::Names, Layout::Icons]
        .into_iter()
        .find(|layout| gutter >= layout.width() + RIGHT + AIR)
}

impl Workspace {
    /// Whether the Workbench can open on `tool` on this screen (`pick_tool`).
    pub(super) fn can_pick_tool(&self, tool: Tool) -> bool {
        let available = match tool {
            Tool::Changes | Tool::Files => self.side.has_folder(),
            Tool::Artifacts => self.page.is_some(),
            Tool::Agents => matches!(self.view, View::Code(_)),
            Tool::Terminal | Tool::Browser => true,
        };
        available && (!self.side.home.home || matches!(tool, Tool::Browser | Tool::Terminal))
    }

    /// The overview, in the main area: while the Workbench is closed, and while its rows leave as it opens.
    pub(in crate::workspace) fn render_workbench_overview(
        &self,
        palette: Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // When the Workbench last opened or closed. Read on every frame, so a change made on another screen is seen.
        let now = Instant::now();
        let open = self.side.open;
        let since = match self.side.overview.get() {
            Some((seen, since)) if seen == open => since,
            Some(_) => Some(now),
            None => None,
        };
        self.side.overview.set(Some((open, since)));
        let showing = !open;
        let moving =
            since.is_some_and(|since| now.saturating_duration_since(since).as_secs_f32() * 1000. < total_ms(showing));
        if open && !moving {
            return None;
        }
        // The find bar takes the same corner (`find_bar.rs`).
        if !matches!(self.side_scope(), Scope::Session(_) | Scope::Chat(_)) || self.find.open {
            return None;
        }
        let mut room = f32::from(window.viewport_size().width);
        if self.sidebar_open {
            room -= self.sidebar_room();
        }
        let layout = layout(room, self.chat_widths().column)?;
        let entries = self.workbench_entries();
        if entries.is_empty() {
            return None;
        }
        let head = (layout == Layout::Names).then(|| {
            div()
                .id("workbench-overview-head")
                .relative()
                .px(px(10.))
                .pb(px(4.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .whitespace_nowrap()
                .child("Workbench")
        });
        let mut rows: Vec<Stateful<Div>> = head.into_iter().collect();
        for entry in entries {
            // The actions follow the tools under a hairline, as in "+".
            if entry.action() && rows.len() > usize::from(layout == Layout::Names) {
                rows.push(
                    div()
                        .id("workbench-overview-rule")
                        .py(px(4.))
                        .px(px(10.))
                        .child(div().h(px(1.)).bg(palette.border)),
                );
            }
            rows.push(self.render_overview_row(entry, layout, showing, palette, cx));
        }
        let tween = Tween::new(Duration::from_secs_f32(total_ms(showing) / 1000.), |t| t);
        // A column that comes back as the Workbench closes starts gone. Anywhere else it is drawn at rest.
        let tween = if showing && moving { tween.from(0.) } else { tween };
        Some(
            v_flex()
                .id("workbench-overview")
                .absolute()
                .top(px(TOP))
                .right(px(RIGHT))
                .w(px(layout.width()))
                .with_tween(
                    "workbench-overview-rows",
                    if showing { 1. } else { 0. },
                    tween,
                    move |column, group| {
                        column.children(rows.into_iter().enumerate().map(|(index, row)| {
                            let level = row_level(group, index, showing);
                            row.opacity(level).left(px(TRAVEL * (1. - level)))
                        }))
                    },
                )
                .into_any_element(),
        )
    }

    /// One entry: its icon, and its name in [`Layout::Names`], with its tab's unread dot and Agents count. Its tooltip
    /// names it with its shortcut, as the Workbench's tabs do. Icons only, the dot sits on the icon and the count
    /// moves to the tooltip. A leaving row takes no clicks.
    fn render_overview_row(
        &self,
        entry: Entry,
        layout: Layout,
        live: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let slug = entry.slug();
        let label = self.entry_label(entry);
        let refusal = self.entry_refusal(entry);
        let dot = matches!(entry, Entry::Tab(Tab::Tool(tool)) if self.side_tab_dot(tool, cx));
        let agents = entry == Entry::Tab(Tab::Tool(Tool::Agents));
        let tooltip: SharedString = match self.agents_live_count().filter(|_| agents && layout == Layout::Icons) {
            Some(count) => format!("{label}, {count} live").into(),
            None => label.into(),
        };
        let group: SharedString = format!("workbench-overview-{slug}").into();
        h_flex()
            .id(SharedString::from(format!("workbench-overview-row-{slug}")))
            .debug_selector(move || format!("workbench-overview-{slug}"))
            .group(group.clone())
            .relative()
            .h(px(ROW))
            .px(px(10.))
            .gap(px(10.))
            .rounded(px(8.))
            .text_size(px(13.))
            .text_color(palette.muted)
            .when(live && refusal.is_none(), |row| {
                row.cursor_pointer()
                    .child(crate::motion::hover_fill("hover", palette.hover, 8.))
                    .hover(|style| style.text_color(palette.foreground))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_entry(entry, window, cx)))
                    .tooltip(move |window, cx| Self::entry_tooltip(&tooltip, entry).build(window, cx))
            })
            .when_some(refusal, |row, reason| {
                row.text_color(palette.faint)
                    .tooltip(move |window, cx| Tooltip::new(reason).build(window, cx))
            })
            .child(
                div()
                    .flex_none()
                    .relative()
                    .text_color(palette.icon)
                    .group_hover(group, |style| style.text_color(palette.foreground))
                    .child(Icon::new(entry.icon()).size(px(15.)))
                    .when(dot && layout == Layout::Icons, |icon| {
                        icon.child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-3.))
                                .size(px(5.))
                                .rounded_full()
                                .bg(palette.muted),
                        )
                    }),
            )
            .when(layout == Layout::Names, |row| {
                row.child(div().flex_none().whitespace_nowrap().child(label))
                    .when(dot, |row| {
                        row.child(div().flex_none().size(px(5.)).rounded_full().bg(palette.muted))
                    })
                    .when(agents, |row| row.children(self.agents_tab_badge(palette)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn rows_rest_at_either_end() {
        for index in 0..ROWS {
            assert_eq!(row_level(1., index, true), 1.);
            assert_eq!(row_level(0., index, true), 0.);
            assert_eq!(row_level(0., index, false), 0.);
            assert_eq!(row_level(1., index, false), 1.);
        }
    }

    #[test]
    fn rows_come_back_only_after_the_pane_has_mostly_left() {
        let group = (IN_DELAY_MS - 1.) / total_ms(true);
        assert!((0..ROWS).all(|index| row_level(group, index, true) == 0.));
    }

    #[test]
    fn the_top_row_moves_first_both_ways() {
        let coming = 0.5;
        assert!(row_level(coming, 0, true) > row_level(coming, ROWS - 1, true));
        let leaving = 0.5;
        assert!(row_level(leaving, 0, false) < row_level(leaving, ROWS - 1, false));
    }

    #[test]
    fn rows_leave_before_the_pane_has_opened() {
        assert!(total_ms(false) < crate::motion::PANEL.as_secs_f32() * 1000.);
    }

    #[test]
    fn the_column_names_its_tools_beside_a_wide_gutter() {
        // Standard's 720 px lane in a 1440 px window less a 260 px sidebar.
        assert_eq!(layout(1180., 720.), Some(Layout::Names));
        // Wide needs a 214 px gutter for names. With a 260 px sidebar, a 1920 px window meets that limit.
        assert_eq!(layout(1919. - 260., 1232.), Some(Layout::Icons));
        assert_eq!(layout(1920. - 260., 1232.), Some(Layout::Names));
    }

    #[test]
    fn a_narrower_gutter_keeps_the_icons() {
        assert_eq!(layout(1000., 720.), Some(Layout::Icons));
    }

    #[test]
    fn the_column_hides_beside_no_gutter() {
        // The Workbench open beside Standard's lane, and Fit window's column, which fills the room less 48.
        assert_eq!(layout(1440. - 260. - 480., 720.), None);
        assert_eq!(layout(1660., 1660. - 48.), None);
    }

    #[test]
    fn every_name_fits_its_row() {
        // A row's padding, icon and gap (45 px), then the name at about 7 px a letter at 13 px.
        let longest = [
            Tool::ALL.map(Tool::label).as_slice(),
            &["Commands", "Side chat", "New terminal", "Workbench"],
        ]
        .concat()
        .into_iter()
        .map(str::len)
        .max()
        .unwrap_or_default();
        assert!(45. + 7. * longest as f32 <= WIDTH);
    }
}
