//! Discover for a native shell: the static words of the kit's `agent-discover.ts` (the ideas, what each
//! connector unlocks, What's new) and the live parts of Nexus UI's `lib/agent-discover.ts` (Connect more, the autonomy
//! ladder, the safe next steps). Nexus GPUI draws it (`desktop-gpui/src/pages/agent/discover.rs`).
//!
//! Discover grows with the agent. A new capability or connector adds an idea to the kit and a line to its news; copy
//! both here in the same change, so every app says the same.

use crate::agent_approvals::{ApprovalsView, dollars, rule_text};
use crate::agent_cards::connector_name;

/// One idea under "Try these": a short title, and the line Nexus gets as the person's message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Idea {
    pub id: &'static str,
    pub title: &'static str,
    pub line: &'static str,
}

/// The kit's `DISCOVER_IDEAS`: each a line the agent answers today.
pub const IDEAS: [Idea; 8] = [
    Idea {
        id: "prep-meeting",
        title: "Prep a meeting",
        line: "What is on my calendar tomorrow?",
    },
    Idea {
        id: "tidy-inbox",
        title: "Tidy my inbox",
        line: "Summarize my unread Gmail threads",
    },
    Idea {
        id: "draft-reply",
        title: "Draft a reply",
        line: "Draft a polite reply that moves our Thursday meeting to Friday",
    },
    Idea {
        id: "my-issues",
        title: "Catch up on work",
        line: "List my open Linear issues",
    },
    Idea {
        id: "my-pulls",
        title: "Check my reviews",
        line: "List my open pull requests on GitHub",
    },
    Idea {
        id: "find-doc",
        title: "Find a doc",
        line: "Find the Notion page about onboarding",
    },
    Idea {
        id: "my-spend",
        title: "Check my AI spend",
        line: "How much did I spend on AI last week?",
    },
    Idea {
        id: "what-it-knows",
        title: "Ask what it knows",
        line: "What do you remember about me?",
    },
];

/// What connecting each account unlocks (the kit's `CONNECTOR_UNLOCKS`); another reads its catalog line.
pub fn unlocks_of(connector: &str, fallback: &str) -> String {
    let known = match connector {
        "asana" => "List and track your open tasks",
        "confluence" => "Find pages and answer from them",
        "github" => "Your pull requests, issues and reviews",
        "gitlab" => "Your merge requests and issues",
        "gmail" => "Summarize your mail and draft replies",
        "google-calendar" => "Read your day and prep your meetings",
        "google-drive" => "Find docs and answer from them",
        "hubspot" => "Read your deals and contacts",
        "jira" => "List and track your open issues",
        "linear" => "List and track your open issues",
        "notion" => "Find pages and answer from them",
        "salesforce" => "Read your accounts and opportunities",
        "slack" => "Read your channels and threads",
        "zendesk" => "List your open tickets",
        _ => fallback,
    };
    known.to_string()
}

/// A view of the Assistant a line of What's new opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Chat,
    Errands,
    Memory,
    Approvals,
}

/// One line of What's new: shown with a dot until the person has seen it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct News {
    pub id: &'static str,
    pub title: &'static str,
    pub line: &'static str,
    pub opens: View,
}

/// The kit's `DISCOVER_NEWS`, newest first. Keep each id: the dot reads them.
pub const NEWS: [News; 4] = [
    News {
        id: "2026-11-brief",
        title: "Your morning brief",
        line: "Today's thread starts with what changed overnight.",
        opens: View::Chat,
    },
    News {
        id: "2026-11-hand-off",
        title: "Hand off from ⌘K",
        line: "Type anything in ⌘K, then press ⌘J. Nexus takes it from there.",
        opens: View::Chat,
    },
    News {
        id: "2026-11-every-surface",
        title: "One conversation, every surface",
        line: "Start in Slack, go on here. The list beside the chat has them all.",
        opens: View::Chat,
    },
    News {
        id: "2026-11-memory",
        title: "Correct what it knows",
        line: "Add a note, edit one, or pause learning. It forgets at once.",
        opens: View::Memory,
    },
];

/// The ids of the news the person has not seen, in order.
pub fn unseen_news(seen: &[String]) -> Vec<&'static str> {
    NEWS.iter()
        .filter(|item| !seen.iter().any(|id| id == item.id))
        .map(|item| item.id)
        .collect()
}

/// The kinds of thing to tell Nexus, with an example of each (Teach it about you).
pub const TEACH: [(&str, &str); 4] = [
    ("People", "Ana is my manager at Acme."),
    ("Preferences", "Aisle seats. No shellfish."),
    ("Places", "My pharmacy is the Walgreens on Divisadero."),
    ("Routines", "Gym on Tuesday and Thursday at 7."),
];

/// The most connectors Connect more lists.
pub const CONNECT_MORE_MAX: usize = 6;

/// A connector the person may connect as themselves: its id and its catalog line.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub connector: String,
    pub line: String,
}

/// One row of Connect more.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectRow {
    pub connector: String,
    pub name: String,
    pub unlocks: String,
    pub connected: bool,
}

impl ConnectRow {
    /// The row's line: "Unlocks: …", or what it already does.
    pub fn line(&self) -> String {
        if !self.connected {
            return format!("Unlocks: {}", self.unlocks);
        }
        let mut chars = self.unlocks.chars();
        let lowered: String = chars
            .next()
            .map(|first| first.to_lowercase().chain(chars).collect())
            .unwrap_or_default();
        format!("Already connected: {lowered}")
    }
}

/// Connect more: the offers, the unconnected first, in the catalog's order. `mine`: the connectors the person holds
/// a connection of their own to.
pub fn connect_rows(offers: &[Offer], mine: &[String]) -> Vec<ConnectRow> {
    let rows: Vec<ConnectRow> = offers
        .iter()
        .map(|offer| ConnectRow {
            connector: offer.connector.clone(),
            name: connector_name(&offer.connector),
            unlocks: unlocks_of(&offer.connector, &offer.line),
            connected: mine.contains(&offer.connector),
        })
        .collect();
    let (open, held): (Vec<_>, Vec<_>) = rows.into_iter().partition(|row| !row.connected);
    open.into_iter().chain(held).take(CONNECT_MORE_MAX).collect()
}

/// One rung of the autonomy ladder.
#[derive(Debug, Clone, PartialEq)]
pub struct Rung {
    pub title: &'static str,
    pub tag: &'static str,
    pub line: String,
}

/// The ladder, top to bottom: what always asks, what asks first, and what Nexus does freely.
pub fn autonomy_ladder(view: Option<&ApprovalsView>) -> Vec<Rung> {
    let line = view.map_or_else(|| "your line".to_string(), |view| dollars(&view.spend_limit_usd));
    vec![
        Rung {
            title: "Always asks",
            tag: "Fixed",
            line: format!("Spending over {line}, passwords, sharing outside, deleting. You confirm it in Nexus."),
        },
        Rung {
            title: "Asks first",
            tag: "Where most things are",
            line: format!("Sending, booking, spending up to {line}, invites."),
        },
        Rung {
            title: "Does freely",
            tag: "Safe to leave on",
            line: "Reading, drafting, reminders, tracking.".into(),
        },
    ]
}

/// The safe next steps: each kind that asks first and has no "Always allow" rule yet, in words.
pub fn safe_next_steps(view: &ApprovalsView) -> Vec<String> {
    ["send", "rsvp", "share-inside", "book", "spend"]
        .into_iter()
        .filter(|kind| !view.rules.iter().any(|rule| rule.kind == *kind))
        .map(|kind| rule_text(kind, None).0)
        .collect()
}

/// The approvals summary in the section's row: "Asks first", or "Every write asks" with the tiers off.
pub fn autonomy_summary(view: Option<&ApprovalsView>) -> String {
    match view {
        None => "Asks first".into(),
        Some(view) if !view.tiers_on => "Every write asks".into(),
        Some(view) if !view.rules.is_empty() => format!("{} always allowed", view.rules.len()),
        Some(_) => "Asks first".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_approvals::approvals_view_of;
    use serde_json::json;

    #[test]
    fn connect_more_puts_the_unconnected_first_with_what_each_unlocks() {
        let offers = [
            Offer {
                connector: "github".into(),
                line: "Repos".into(),
            },
            Offer {
                connector: "acme-crm".into(),
                line: "Your accounts".into(),
            },
            Offer {
                connector: "linear".into(),
                line: "Issues".into(),
            },
        ];
        let rows = connect_rows(&offers, &["github".into()]);
        assert_eq!(
            rows.iter().map(|row| row.connector.as_str()).collect::<Vec<_>>(),
            ["acme-crm", "linear", "github"]
        );
        assert_eq!(rows[0].line(), "Unlocks: Your accounts");
        assert_eq!(rows[1].line(), "Unlocks: List and track your open issues");
        assert_eq!(
            rows[2].line(),
            "Already connected: your pull requests, issues and reviews"
        );
        assert_eq!(rows[2].name, "GitHub");
    }

    #[test]
    fn the_ladder_names_the_persons_line_and_the_safe_next_steps() {
        let view = approvals_view_of(&json!({
            "tiers": "on", "spendLimitUsd": "50.00", "ownLine": false, "defaultSpendLimitUsd": "50.00",
            "maxSpendLimitUsd": "200.00", "rules": [{ "id": "allow_0123456789abcdef01234567", "kind": "send", "since": "x" }]
        }))
        .expect("reads");
        let ladder = autonomy_ladder(Some(&view));
        assert_eq!(
            ladder[0].line,
            "Spending over $50, passwords, sharing outside, deleting. You confirm it in Nexus."
        );
        assert_eq!(
            autonomy_ladder(None)[1].line,
            "Sending, booking, spending up to your line, invites."
        );
        assert_eq!(safe_next_steps(&view)[0], "Accept or decline invites");
        assert_eq!(autonomy_summary(Some(&view)), "1 always allowed");
        assert_eq!(autonomy_summary(None), "Asks first");
    }

    #[test]
    fn whats_new_keeps_a_dot_until_seen() {
        assert_eq!(unseen_news(&[]).len(), NEWS.len());
        assert_eq!(unseen_news(&["2026-11-brief".into()])[0], "2026-11-hand-off");
        assert_eq!(IDEAS.len(), 8);
    }
}
