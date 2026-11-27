//! Observe fresh primary-agent page versions and offer them to Follow / Manual navigation.
//! Restored history and linked pages do not navigate. Manual still records new versions as background activity.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace};
use crate::code::Block;
use crate::palette::Palette;

/// The version a Code card opens: the latest. Chat's cards name versions from 1, so 0 never stands for one of theirs.
const LATEST: u32 = 0;

/// The card's quiet line, as the Nexus app's.
const NOTE: &str = "Private to your organization’s Gateway until you share it.";

/// Which page to open as the session writes it, from its blocks as they are read (`auto-page.ts`'s `AutoPage`).
pub(super) struct AutoPage {
    /// A session made here: every page it writes is new. Otherwise the first read is the baseline.
    fresh: bool,
    ready: bool,
    /// The last event read: only pages written after it count.
    after: i64,
}

impl AutoPage {
    pub(super) fn new(fresh: bool) -> AutoPage {
        AutoPage {
            fresh,
            ready: false,
            after: 0,
        }
    }

    /// The page to open now, from the session's blocks read up to event `last`; None when nothing new is to open.
    pub(super) fn observe(&mut self, blocks: &[Block], last: i64) -> Option<String> {
        if !self.ready {
            self.ready = true;
            if !self.fresh {
                self.after = last;
                return None;
            }
        }
        let after = self.after;
        self.after = self.after.max(last);
        let page_id = blocks.iter().rev().find_map(|block| match block {
            Block::Page {
                page_id,
                seq,
                written: true,
                ..
            } if *seq > after => Some(page_id.clone()),
            _ => None,
        })?;
        Some(page_id)
    }
}

impl Workspace {
    /// Show page `id` at its latest version in the side pane's Artifacts tab, as a card's Open does. `auto`: the app
    /// opens it on its own as the agent writes it, not the person.
    pub(super) fn open_code_page(&mut self, id: &str, auto: bool, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if auto {
            if let Some(session) = self.navigation_id() {
                let revision = self.navigation(&session).revision;
                self.side.pending_pages.insert(id.into(), (session, revision));
            }
        } else {
            self.reveal_manual_workbench();
            self.side.pending_pages.remove(id);
        }

        let path = match self.account.as_ref().map(|account| account.org.clone()) {
            Some(org) => format!("/app/{org}/pages/{id}"),
            None => format!("/app?next=/pages/{id}"),
        };
        // The engine starts while Gateway answers with the page's address.
        crate::web::warm();
        self.code_page_seen = Some((id.to_string(), self.newest_page_write(id)));
        self.page_opening = Some((id.to_string(), LATEST));
        self.page_opening_auto = auto;
        account_gateway.frame_page(id.to_string(), path);
        cx.notify();
    }

    /// The event of the open Code session's newest write of page `id`; 0 when it wrote none.
    fn newest_page_write(&self, id: &str) -> i64 {
        let View::Code(view) = &self.view else {
            return 0;
        };
        newest_write(&view.blocks, id)
    }

    /// Whether the Artifacts tab holds page `id` as the session last wrote it. A later version is not loaded yet.
    fn code_page_loaded(&self, id: &str) -> bool {
        self.page_block_loaded(&(id.to_string(), LATEST))
            && self
                .code_page_seen
                .as_ref()
                .is_some_and(|(seen, at)| seen == id && *at >= self.newest_page_write(id))
    }

    /// A page's card in a Code session, block `ix`: its title, and Open.
    pub(super) fn render_code_page(
        &self,
        ix: usize,
        page_id: &str,
        title: Option<&str>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = (page_id.to_string(), LATEST);
        let open = self.code_page_loaded(page_id) && self.side.showing(super::side_pane::Tool::Artifacts);
        let opening = self.page_opening.as_ref() == Some(&key);
        let title = SharedString::from(title.unwrap_or("Nexus page").to_string());
        let id = page_id.to_string();
        Self::page_card(
            ("code-page-open", ix),
            Lucide::AppWindow,
            (title, NOTE.into()),
            (open, opening),
            palette,
            |hovered, _, cx| crate::web::warm_on_hover(*hovered, cx),
            cx.listener(move |this, _, window, cx| this.open_code_card(&id, window, cx)),
        )
    }

    /// A Code page card's Open: show the Artifacts tab when it holds the page as last written, else load the latest.
    pub(super) fn open_code_card(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_page_loaded(id) {
            self.pick_tool(super::side_pane::Tool::Artifacts, window, cx);
        } else {
            self.open_code_page(id, false, cx);
        }
    }

    /// Open the page the open Code session just wrote, if [`AutoPage`] says so. Called after each read.
    pub(super) fn auto_open_code_page(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        if !view.loaded {
            return;
        }
        if let Some(id) = view.auto_page.observe(&view.blocks, view.last_seq) {
            self.offer_workbench(super::side_pane::navigation::Target::Page(id), true, cx);
        }
    }
}

/// The event of the newest write of page `id` in `blocks`; 0 when none wrote it.
fn newest_write(blocks: &[Block], id: &str) -> i64 {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Page {
                page_id,
                seq,
                written: true,
                ..
            } if page_id == id => Some(*seq),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{AutoPage, Block, LATEST, newest_write};

    /// A card opens the version the agent wrote last: Open after a newer write loads it again, instead of showing the
    /// Artifacts tab with the version it already holds.
    #[gpui_kit::test]
    fn a_card_loads_a_newer_version_of_an_open_page(cx: &mut gpui_kit::TestAppContext) {
        use super::super::code_view::CodeView;
        use super::super::side_pane::Tool;
        use super::super::{View, Workspace, machine_source::MachineSource};

        cx.update(gpui_kit::init);
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(gateway, events, arrivals, false, MachineSource::isolated(), window, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.auth = crate::workspace::Auth::SignedIn;
            let mut view = CodeView::new("session".into(), window, cx);
            view.blocks = vec![page("pag_1", 1, 3)];
            workspace.view = View::Code(view);

            // The person opens version 1, and its frame arrives.
            workspace.open_code_card("pag_1", window, cx);
            assert_eq!(
                workspace.page_opening,
                Some(("pag_1".to_string(), LATEST)),
                "version 1 loads"
            );
            let chat = crate::gateway::ChatText {
                title: "Page".into(),
                markdown: "# Page".into(),
                path: "/pages/pag_1".into(),
            };
            workspace.page_opening = None;
            workspace.open_chat_text(chat, cx);
            workspace.page.as_mut().expect("the page").block = Some(("pag_1".to_string(), LATEST));
            workspace.pick_tool(Tool::Artifacts, window, cx);
            assert!(workspace.code_page_loaded("pag_1"), "version 1 is on screen");

            // Open again: the tab already holds it, so nothing loads.
            workspace.open_code_card("pag_1", window, cx);
            assert_eq!(workspace.page_opening, None, "the same version does not load again");

            // The agent writes version 2. The card now loads it.
            let View::Code(view) = &mut workspace.view else {
                unreachable!()
            };
            view.blocks.push(page("pag_1", 2, 9));
            assert!(!workspace.code_page_loaded("pag_1"), "the tab holds an older version");
            workspace.open_code_card("pag_1", window, cx);
            assert_eq!(
                workspace.page_opening,
                Some(("pag_1".to_string(), LATEST)),
                "version 2 loads"
            );
        });
    }

    fn page(id: &str, turn: i64, seq: i64) -> Block {
        Block::Page {
            page_id: id.into(),
            title: None,
            turn,
            seq,
            written: true,
        }
    }

    #[test]
    fn a_session_opened_from_the_list_opens_only_the_pages_written_after() {
        let mut auto = AutoPage::new(false);
        let restored = vec![page("pag_1", 1, 3)];
        assert_eq!(auto.observe(&restored, 5), None);
        assert_eq!(auto.observe(&restored, 5), None);
        let grown = vec![page("pag_1", 1, 3), page("pag_2", 6, 8)];
        assert_eq!(auto.observe(&grown, 8).as_deref(), Some("pag_2"));
    }

    #[test]
    fn a_later_write_of_the_same_page_is_its_newest() {
        let mut linked = page("pag_1", 0, 0);
        if let Block::Page { written, .. } = &mut linked {
            *written = false;
        }
        let blocks = vec![page("pag_1", 1, 3), page("pag_2", 2, 5), page("pag_1", 3, 9), linked];
        assert_eq!(newest_write(&blocks, "pag_1"), 9);
        assert_eq!(newest_write(&blocks, "pag_2"), 5);
        assert_eq!(newest_write(&blocks, "pag_3"), 0);
    }

    #[test]
    fn a_new_session_opens_its_first_page() {
        let mut auto = AutoPage::new(true);
        assert_eq!(auto.observe(&[page("pag_1", 1, 3)], 3).as_deref(), Some("pag_1"));
    }

    #[test]
    fn a_link_in_an_answer_opens_nothing() {
        let mut auto = AutoPage::new(true);
        let linked = Block::Page {
            page_id: "pag_1".into(),
            title: None,
            turn: 0,
            seq: 0,
            written: false,
        };
        assert_eq!(auto.observe(&[linked], 4), None);
    }

    #[test]
    fn each_fresh_page_version_is_an_activity_even_in_the_same_turn() {
        let mut auto = AutoPage::new(true);
        assert_eq!(auto.observe(&[page("pag_1", 1, 3)], 3).as_deref(), Some("pag_1"));
        // A fresh version is offered; the navigation mode decides whether it opens.
        assert_eq!(auto.observe(&[page("pag_1", 1, 5)], 5).as_deref(), Some("pag_1"));
        assert_eq!(auto.observe(&[page("pag_1", 1, 5)], 5), None);
        // The next turn writes it again: it opens.
        let next = vec![page("pag_1", 1, 5), page("pag_1", 7, 9)];
        assert_eq!(auto.observe(&next, 9).as_deref(), Some("pag_1"));
    }

    #[test]
    fn two_distinct_pages_can_open_in_one_turn_without_a_person_dismissal() {
        let mut auto = AutoPage::new(true);
        assert_eq!(auto.observe(&[page("pag_1", 1, 3)], 3).as_deref(), Some("pag_1"));
        let more = vec![page("pag_1", 1, 3), page("pag_2", 1, 6)];
        assert_eq!(auto.observe(&more, 6).as_deref(), Some("pag_2"));
    }

    #[test]
    fn manual_records_fresh_page_activity_across_turns_without_opening_it() {
        use crate::workspace::side_pane::navigation::{Navigation, Target};
        let mut auto = AutoPage::new(true);
        let mut navigation = Navigation::new(true);
        for block in [page("pag_1", 1, 3), page("pag_2", 1, 6), page("pag_3", 8, 10)] {
            let Block::Page { seq, .. } = block else { unreachable!() };
            let id = auto.observe(&[block], seq).unwrap();
            navigation.offer(Target::Page(id), true, 0);
            assert!(navigation.manual);
            assert!(
                navigation
                    .take(
                        false,
                        true,
                        std::time::Instant::now() + std::time::Duration::from_secs(1)
                    )
                    .is_none()
            );
        }
        assert_eq!(navigation.latest, Some(Target::Page("pag_3".into())));
    }
}
