//! The Workbench with no tab: with no session, its toggle opens it on nothing, rather than start a shell. It lists the
//! entries it can open (`entries.rs`), each with its keys as "+" shows them. A click opens that tool, as its key
//! does.

use super::entries::Entry;
use super::*;

impl Workspace {
    pub(super) fn render_workbench_empty(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<_> = self
            .workbench_entries()
            .into_iter()
            .map(|entry| self.render_empty_row(entry, palette, cx))
            .collect();
        div()
            .id("workbench-empty")
            .debug_selector(|| "workbench-empty".into())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(340.))
                    .max_w_full()
                    .px(px(12.))
                    .gap(px(2.))
                    .child(
                        div()
                            .px(px(12.))
                            .pb(px(12.))
                            .text_size(px(15.))
                            .text_color(palette.foreground)
                            .child("Open a tool"),
                    )
                    .children(rows),
            )
            .into_any_element()
    }

    /// What `entry` opens here, under its name: where a shell starts, and what the browser shows.
    fn empty_caption(&self, entry: Entry) -> Option<String> {
        match entry {
            Entry::Tab(Tab::Tool(Tool::Browser)) => Some("Web pages and local servers".into()),
            Entry::Tab(Tab::Tool(Tool::Terminal)) if self.side.server.is_some() => {
                Some("A shell on the session's server".into())
            }
            Entry::Tab(Tab::Tool(Tool::Terminal)) if self.side.dir == home_dir() => {
                Some("A shell in your home folder".into())
            }
            Entry::Tab(Tab::Tool(Tool::Terminal)) => self
                .side
                .dir
                .file_name()
                .map(|name| format!("A shell in {}", name.to_string_lossy())),
            _ => None,
        }
    }

    fn render_empty_row(&self, entry: Entry, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let slug = entry.slug();
        let refusal = self.entry_refusal(entry);
        let caption = refusal.map(str::to_string).or_else(|| self.empty_caption(entry));
        h_flex()
            .id(SharedString::from(format!("workbench-empty-{slug}")))
            .debug_selector(move || format!("workbench-empty-{slug}"))
            .relative()
            .h(px(if caption.is_some() { 48. } else { 36. }))
            .pl(px(12.))
            .pr(px(8.))
            .gap(px(12.))
            .rounded(px(8.))
            .text_size(px(13.))
            .text_color(palette.foreground)
            .when(refusal.is_none(), |row| {
                row.cursor_pointer()
                    .child(crate::motion::hover_fill("hover", palette.hover, 8.))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_entry(entry, window, cx)))
            })
            .when(refusal.is_some(), |row| row.text_color(palette.faint))
            .child(Icon::new(entry.icon()).size(px(15.)).text_color(palette.icon))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(self.entry_label(entry))
                    .when_some(caption, |label, caption| {
                        label.child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(caption),
                        )
                    }),
            )
            .when_some(Self::entry_keys(entry), |row, keys| row.child(key_hint(&keys, palette)))
    }
}
