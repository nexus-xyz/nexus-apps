//! The Workbench's entries: what its overview lists, what "+" offers, and what an empty Workbench shows. One list, so
//! the three never drift: the same tools, in the strip's default order, under the same names, then the actions.
//!
//! - The overview (`overview.rs`) lists every entry.
//! - "+" lists every entry that is not a tab yet.
//! - An empty Workbench (`empty.rs`) lists every entry, as a session's nothing-open state does.
//!
//! A tool entry opens its tool, and starts it when it has nothing yet. Side chat opens the session's side chat, or
//! starts one. New terminal adds a shell: it shows once a terminal is open, since before that the Terminal entry
//! starts the first one.

use super::*;

/// One thing the Workbench can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    /// A tab: a tool or the side chat.
    Tab(Tab),
    /// Another shell in the folder's terminals.
    NewTerminal,
}

/// The entries, in the strip's default order: each tool that `available` says can open here, Side chat when
/// `side_chat`, then New terminal while `terminal_open`.
pub(super) fn entries(available: impl Fn(Tool) -> bool, side_chat: bool, terminal_open: bool) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Tool::ALL
        .into_iter()
        .filter(|tool| available(*tool))
        .map(|tool| Entry::Tab(Tab::Tool(tool)))
        .collect();
    if side_chat {
        entries.push(Entry::Tab(Tab::SideChat));
    }
    if terminal_open && available(Tool::Terminal) {
        entries.push(Entry::NewTerminal);
    }
    entries
}

/// What "+" offers: the entries that are not among `tabs`.
pub(super) fn offered(entries: Vec<Entry>, tabs: &[Tab]) -> Vec<Entry> {
    entries
        .into_iter()
        .filter(|entry| match entry {
            Entry::Tab(tab) => !tabs.contains(tab),
            Entry::NewTerminal => true,
        })
        .collect()
}

impl Entry {
    /// Its name in selectors and tests.
    pub(super) fn slug(self) -> &'static str {
        match self {
            Entry::Tab(Tab::Tool(Tool::Changes)) => "changes",
            Entry::Tab(Tab::Tool(Tool::Files)) => "files",
            Entry::Tab(Tab::Tool(Tool::Terminal)) => "terminal",
            Entry::Tab(Tab::Tool(Tool::Browser)) => "browser",
            Entry::Tab(Tab::Tool(Tool::Artifacts)) => "artifacts",
            Entry::Tab(Tab::Tool(Tool::Agents)) => "agents",
            Entry::Tab(Tab::SideChat) => "side-chat",
            Entry::NewTerminal => "new-terminal",
        }
    }

    pub(super) fn icon(self) -> Lucide {
        match self {
            Entry::Tab(Tab::Tool(tool)) => tool.icon(),
            Entry::Tab(Tab::SideChat) => Lucide::MessageSquare,
            // Not the terminal's icon: beside the Terminal row, a second one would read as a second terminal tool.
            Entry::NewTerminal => Lucide::Plus,
        }
    }

    /// Whether it is an action rather than a tab: actions follow a hairline.
    pub(super) fn action(self) -> bool {
        self == Entry::NewTerminal
    }
}

impl Workspace {
    /// The entries for the screen on show.
    pub(super) fn workbench_entries(&self) -> Vec<Entry> {
        entries(
            |tool| self.can_pick_tool(tool),
            self.side_chat_offered(),
            self.side.has_terminal(),
        )
    }

    /// A started Code session can open a side chat, unless this build retired them.
    fn side_chat_offered(&self) -> bool {
        matches!(&self.view, View::Code(view) if !view.is_draft()) && !self.side.side_chat_retired
    }

    pub(super) fn entry_refusal(&self, entry: Entry) -> Option<&'static str> {
        match entry {
            Entry::Tab(Tab::SideChat) => self.session_account_control_reason(),
            _ => None,
        }
    }

    /// Its name, as its tab says it: the terminal tool is Commands while the session shares its commands.
    pub(super) fn entry_label(&self, entry: Entry) -> &'static str {
        match entry {
            Entry::Tab(Tab::Tool(Tool::Terminal)) if self.side.shows_commands() => "Commands",
            Entry::Tab(Tab::Tool(tool)) => tool.label(),
            Entry::Tab(Tab::SideChat) => "Side chat",
            Entry::NewTerminal => "New terminal",
        }
    }

    /// Its keys as the person has them, if it has any.
    pub(super) fn entry_keys(entry: Entry) -> Option<String> {
        match entry {
            Entry::Tab(Tab::Tool(tool)) => crate::key_bindings::keystroke(tool.shortcut_id()),
            Entry::Tab(Tab::SideChat) => None,
            Entry::NewTerminal => Some(shortcut_label::NEW_TERMINAL.into()),
        }
    }

    /// Its tooltip: its name, with its keys when it has any.
    pub(super) fn entry_tooltip(label: &str, entry: Entry) -> Tooltip {
        match Self::entry_keys(entry) {
            Some(keys) => shortcut_label::tooltip(label, &keys),
            None => Tooltip::new(label.to_string()),
        }
    }

    /// Open `entry`, as its tab, its key or its row does.
    pub(super) fn open_entry(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry_refusal(entry).is_some() {
            return;
        }
        self.side.plus_menu = false;
        match entry {
            Entry::Tab(Tab::Tool(tool)) => self.pick_tool(tool, window, cx),
            Entry::Tab(Tab::SideChat) if self.open_side_chat().is_some() || self.waiting_side_chat().is_some() => {
                self.reveal_manual_workbench();
                self.side.open = true;
                self.side.connection = None;
                self.side.side_chat_visible = true;
                self.sync_side_chat(window, cx);
                if let Some(chat) = self.open_side_chat() {
                    chat.update(cx, |chat, cx| chat.focus_input(window, cx));
                }
                cx.notify();
            }
            // A new side chat opens the Workbench on it once Nexus has made it (`side_chat/creation.rs`).
            Entry::Tab(Tab::SideChat) => self.create_side_chat(None, window, cx),
            Entry::NewTerminal => self.new_side_terminal(window, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn tool(tool: Tool) -> Entry {
        Entry::Tab(Tab::Tool(tool))
    }

    #[test]
    fn a_session_lists_its_tools_in_the_strips_order_then_side_chat_then_new_terminal() {
        let session = |tool: Tool| tool != Tool::Artifacts;
        assert_eq!(
            entries(session, true, true),
            vec![
                tool(Tool::Changes),
                tool(Tool::Files),
                tool(Tool::Browser),
                tool(Tool::Terminal),
                tool(Tool::Agents),
                Entry::Tab(Tab::SideChat),
                Entry::NewTerminal,
            ]
        );
    }

    #[test]
    fn new_terminal_waits_for_a_terminal_since_the_terminal_entry_starts_the_first() {
        let home = |tool: Tool| matches!(tool, Tool::Browser | Tool::Terminal);
        assert_eq!(
            entries(home, false, false),
            vec![tool(Tool::Browser), tool(Tool::Terminal)]
        );
        assert_eq!(
            entries(home, false, true),
            vec![tool(Tool::Browser), tool(Tool::Terminal), Entry::NewTerminal]
        );
    }

    #[test]
    fn plus_offers_what_is_not_a_tab_and_always_the_actions() {
        let home = |tool: Tool| matches!(tool, Tool::Browser | Tool::Terminal);
        let tabs = [Tab::Tool(Tool::Terminal)];
        assert_eq!(
            offered(entries(home, false, true), &tabs),
            vec![tool(Tool::Browser), Entry::NewTerminal]
        );
        let session = |tool: Tool| tool != Tool::Artifacts;
        let tabs = [
            Tab::Tool(Tool::Changes),
            Tab::Tool(Tool::Files),
            Tab::Tool(Tool::Terminal),
            Tab::Tool(Tool::Agents),
        ];
        assert_eq!(
            offered(entries(session, true, true), &tabs),
            vec![tool(Tool::Browser), Entry::Tab(Tab::SideChat), Entry::NewTerminal]
        );
    }

    #[test]
    fn every_entry_has_its_own_slug_and_new_terminal_is_the_only_action() {
        let all = entries(|_| true, true, true);
        let mut slugs: Vec<_> = all.iter().map(|entry| entry.slug()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), all.len());
        assert_eq!(
            all.iter().filter(|entry| entry.action()).collect::<Vec<_>>(),
            vec![&Entry::NewTerminal]
        );
    }
}
