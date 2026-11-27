//! Settings › Connections' groups: the connectors as one person sees their own accounts. The organization's rules
//! group the same connectors by who can use them ([`group_cards`]); this page groups them by what the person does next.

use super::*;

/// A group of Settings › Connections, in the page's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shelf {
    /// The person holds an account: a row to reconnect sorts first.
    Connected,
    /// They may connect one and have not.
    Ready,
    /// The organization's setup serves them: nothing to connect.
    Shared,
    Unavailable,
    Unknown,
    /// Not offered by this Gateway, with an account the person still holds.
    Preview,
}

pub(super) const SHELVES: [(Shelf, &str, &str); 6] = [
    (Shelf::Connected, "Connected", ""),
    (Shelf::Ready, "Not connected yet", ""),
    (
        Shelf::Shared,
        "Through your organization",
        "Agents use your organization's setup for these. You have nothing to connect.",
    ),
    (
        Shelf::Unavailable,
        "Not available to you",
        // The page draws a lock, this reason and Ask in place of the footnote (`turned_off`).
        "",
    ),
    (
        Shelf::Unknown,
        "Access could not be checked",
        "Refresh to check which of these your organization lets you use.",
    ),
    (
        Shelf::Preview,
        "In preview",
        "This Gateway does not offer these yet. Your accounts are kept, and no call uses them.",
    ),
];

impl Shelf {
    /// The groups a person rarely acts on start folded, so the accounts they hold and can connect stay on screen.
    pub(super) fn folds(self) -> bool {
        matches!(self, Shelf::Shared | Shelf::Unavailable | Shelf::Preview)
    }

    pub(super) fn title(self) -> &'static str {
        SHELVES
            .iter()
            .find(|(id, _, _)| *id == self)
            .map_or("", |(_, title, _)| title)
    }

    pub(super) fn footnote(self) -> &'static str {
        SHELVES
            .iter()
            .find(|(id, _, _)| *id == self)
            .map_or("", |(_, _, note)| note)
    }
}

/// Where a connector sits on Settings › Connections, or None when it is not the person's to see. Access the
/// organization refused or could not check wins over a saved account, as on the organization's page.
pub(super) fn shelf(card: &Card, rights: Rights) -> Option<Shelf> {
    let connector = &card.connector;
    if !connector.offered {
        return (!card.mine.is_empty()).then_some(Shelf::Preview);
    }
    let connects_own = rights.own && connector.connects.iter().any(|scope| scope == "user");
    Some(match connector.usable {
        Access::Unavailable => Shelf::Unavailable,
        Access::Unknown => Shelf::Unknown,
        Access::Usable if !card.mine.is_empty() => Shelf::Connected,
        Access::Usable if connects_own => Shelf::Ready,
        Access::Usable => Shelf::Shared,
    })
}

/// The groups in order with their cards, empty ones left out. A card here never carries the organization's choice:
/// this page says what the person may do, not who chose it.
pub(super) fn mine_shelves(
    connectors: &[Connector],
    connections: &[Connection],
    rights: Rights,
) -> Vec<(Shelf, Vec<Card>)> {
    let mine: Vec<Connection> = connections
        .iter()
        .filter(|item| item.scope == "user")
        .cloned()
        .collect();
    let placed: Vec<(Shelf, Card)> = connectors
        .iter()
        .map(|connector| card(connector, &mine, None, rights))
        .filter_map(|card| Some((shelf(&card, rights)?, card)))
        .collect();
    SHELVES
        .iter()
        .filter_map(|(shelf, _, _)| {
            let mut members: Vec<Card> = placed
                .iter()
                .filter(|(at, _)| at == shelf)
                .map(|(_, card)| card.clone())
                .collect();
            members.sort_by(|a, b| {
                a.rank()
                    .cmp(&b.rank())
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            (!members.is_empty()).then_some((*shelf, members))
        })
        .collect()
}

/// A row's one status on the organization's page: who can use it, or the organization's account to reconnect. None
/// when the Off heading already says it.
pub(super) fn org_status(card: &Card) -> Option<Status> {
    if !card.connector.offered {
        return None;
    }
    if card.org.iter().any(|item| item.needs_reconnect) {
        return Some(status("Organization account needs reconnecting", Tone::Warn));
    }
    let level = availability_status(card.availability.as_ref()?);
    (level.tone != Tone::Off).then_some(level)
}

/// How the abilities read folded: "11 to read, 13 to change", or only reads.
pub(super) fn abilities_summary(abilities: &[Ability]) -> String {
    let changes = abilities.iter().filter(|item| item.changes).count();
    let reads = abilities.len() - changes;
    match (reads, changes) {
        (reads, 0) => format!("{reads} to read. It changes nothing."),
        (0, changes) => format!("{changes} to change."),
        (reads, changes) => format!("{reads} to read, {changes} to change."),
    }
}
