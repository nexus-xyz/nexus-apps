use super::*;

// ------------------------------------------------------------------ How it reads

/// Gateway names connectors by id, so the display names live here (`connectorName`).
pub(super) fn connector_name(id: &str) -> String {
    let known = match id {
        "asana" => "Asana",
        "bitbucket" => "Bitbucket",
        "cloudflare" => "Cloudflare",
        "confluence" => "Confluence",
        "datadog" => "Datadog",
        "dropbox" => "Dropbox",
        "figma" => "Figma",
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "gmail" => "Gmail",
        "google-calendar" => "Google Calendar",
        "google-drive" => "Google Drive",
        "hubspot" => "HubSpot",
        "intercom" => "Intercom",
        "jira" => "Jira",
        "linear" => "Linear",
        "notion" => "Notion",
        "okta" => "Okta",
        "pagerduty" => "PagerDuty",
        "postgres" => "Postgres",
        "quickbooks" => "QuickBooks",
        "salesforce" => "Salesforce",
        "sentry" => "Sentry",
        "shopify" => "Shopify",
        "slack" => "Slack",
        "snowflake" => "Snowflake",
        "stripe" => "Stripe",
        "teams" => "Microsoft Teams",
        "zendesk" => "Zendesk",
        "zoom" => "Zoom",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Past this, the line ends at its last whole clause.
pub(super) const LINE_MAX: usize = 56;

/// The row's one line: what the connector reads, without the catalogue's lead-in (`shortLine`).
pub(in crate::pages) fn short_line(description: &str) -> String {
    let mut text = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(colon) = text.find(": ")
        && colon > 0
        && colon <= 100
    {
        text = text[colon + 2..].to_string();
    }
    let bytes = text.as_bytes();
    let end = (0..bytes.len())
        .find(|&at| matches!(bytes[at], b'.' | b';') && (at + 1 == bytes.len() || bytes[at + 1].is_ascii_whitespace()));
    if let Some(end) = end
        && end > 0
    {
        text.truncate(end);
    }
    if text.get(..4).is_some_and(|lead| lead.eq_ignore_ascii_case("its ")) {
        text = text[4..].to_string();
    }
    if text.len() > LINE_MAX
        && let Some(clause) = text
            .match_indices(", ")
            .map(|(at, _)| at)
            .filter(|&at| at > 0 && at <= LINE_MAX)
            .last()
    {
        text.truncate(clause);
    }
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The three answers to "Who can use it", in the design's words: `disabled`, `available` and
/// `enabled_for_all`.
pub(super) const LEVELS: [(&str, &str); 3] = [
    ("disabled", "Off"),
    ("available", "Their own account"),
    ("enabled_for_all", "Everyone"),
];

pub(super) fn level_label(level: &str) -> &'static str {
    LEVELS
        .iter()
        .find(|(id, _)| *id == level)
        .map(|(_, label)| *label)
        .unwrap_or("Off")
}

/// What an answer means, in one sentence under the choice. `org` is the organization's name.
pub(super) fn level_consequence(availability: &Availability, org: &str) -> String {
    let what = match availability.level.as_str() {
        "available" => "Each person connects their own account, and agents use only that.".to_string(),
        "enabled_for_all" => format!(
            "Everyone can use it. People without their own account, and runs for no person, use {org}'s account."
        ),
        _ => "Nobody can use it. Accounts stay connected, and no call uses them.".to_string(),
    };
    if availability.default {
        format!("Nobody chose yet, so this is Nexus's default. {what}")
    } else {
        what
    }
}

/// How a status reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    On,
    Some,
    Off,
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Status {
    pub(super) label: String,
    pub(super) tone: Tone,
}

pub(super) fn status(label: impl Into<String>, tone: Tone) -> Status {
    Status {
        label: label.into(),
        tone,
    }
}

pub(super) fn teams_text(teams: &[String]) -> String {
    match teams {
        [one] => format!("the {one} team"),
        [one, two] => format!("{one} and {two}"),
        _ => format!("{} teams", teams.len()),
    }
}

/// Who may use it, narrowed to its teams (`availabilityStatus`).
pub(super) fn availability_status(availability: &Availability) -> Status {
    let teams = &availability.teams;
    match availability.level.as_str() {
        "available" if teams.is_empty() => status("People connect their own", Tone::On),
        "available" => {
            let who = teams_text(teams);
            let label = if teams.len() == 1 {
                format!("{who} connects its own")
            } else {
                format!("{who} connect their own")
            };
            let mut chars = label.chars();
            let label: String = chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default();
            status(label, Tone::Some)
        }
        "enabled_for_all" if teams.is_empty() => status("On for everyone", Tone::On),
        "enabled_for_all" => status(format!("On for {}", teams_text(teams)), Tone::Some),
        _ => status(level_label("disabled"), Tone::Off),
    }
}

/// Whose account it runs with, from the reader's side (`accountStatus`).
pub(super) fn account_status(
    connector: &Connector,
    mine: &[&Connection],
    org: &[&Connection],
    rights: Rights,
    chose: bool,
) -> Option<Status> {
    let takes = |scope: &str| connector.connects.iter().any(|item| item == scope);
    if rights.own && takes("user") {
        if mine.iter().any(|item| item.needs_reconnect) {
            return Some(status("Your account needs reconnecting", Tone::Warn));
        }
        return Some(match (mine.len(), connector.usable) {
            (count, _) if count > 1 => status(format!("You have {count} accounts"), Tone::On),
            (1, _) => status("You're connected", Tone::On),
            (_, Access::Unavailable) => status("Not available to you", Tone::Off),
            (_, Access::Unknown) => status("Couldn't check your access", Tone::Off),
            _ => status("You're not connected", Tone::Off),
        });
    }
    if takes("org") {
        if org.iter().any(|item| item.needs_reconnect) {
            return Some(status("Organization account needs reconnecting", Tone::Warn));
        }
        if !chose && connector.usable == Access::Unavailable {
            return Some(status("Not available to you", Tone::Off));
        }
        return Some(if org.is_empty() {
            status("No organization account", Tone::Off)
        } else {
            status("Organization account connected", Tone::On)
        });
    }
    None
}

/// The groups, in the web page's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    On,
    Off,
    Yours,
    Unavailable,
    Unknown,
    Preview,
}

pub(super) const GROUPS: [(Group, &str, &str); 6] = [
    (Group::On, "On", ""),
    (
        Group::Off,
        "Off",
        "Nobody can use these until an owner or admin turns them on.",
    ),
    (Group::Yours, "Available to you", ""),
    (
        Group::Unavailable,
        "Not available to you",
        "Ask an owner or admin to turn one on for you.",
    ),
    (
        Group::Unknown,
        "Access could not be checked",
        "Refresh to check which of these tools your organization allows you to use.",
    ),
    (
        Group::Preview,
        "In preview",
        "This Gateway does not offer these yet. Their accounts are kept, and no call uses them.",
    ),
];

/// One connector as the page draws it (`ConnectorCard`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Card {
    pub(super) connector: Connector,
    pub(super) name: String,
    pub(super) line: String,
    pub(super) mine: Vec<Connection>,
    pub(super) org: Vec<Connection>,
    pub(super) availability: Option<Availability>,
    pub(super) statuses: Vec<Status>,
}

impl Card {
    pub(super) fn group(&self) -> Group {
        if !self.connector.offered {
            Group::Preview
        } else if let Some(availability) = &self.availability {
            if availability.level == "disabled" {
                Group::Off
            } else {
                Group::On
            }
        } else {
            match self.connector.usable {
                Access::Usable => Group::Yours,
                Access::Unavailable => Group::Unavailable,
                Access::Unknown => Group::Unknown,
            }
        }
    }

    pub(super) fn rank(&self) -> u8 {
        if self.statuses.iter().any(|item| item.tone == Tone::Warn) {
            0
        } else if !self.mine.is_empty() || !self.org.is_empty() {
            1
        } else {
            2
        }
    }
}

pub(super) fn card(
    connector: &Connector,
    connections: &[Connection],
    availability: Option<&Availability>,
    rights: Rights,
) -> Card {
    let held: Vec<&Connection> = connections
        .iter()
        .filter(|item| item.connector == connector.name)
        .collect();
    let mine: Vec<&Connection> = held.iter().copied().filter(|item| item.scope == "user").collect();
    let org: Vec<&Connection> = held.iter().copied().filter(|item| item.scope == "org").collect();
    let mut statuses = Vec::new();
    if connector.offered {
        let level = availability.map(availability_status);
        let account = account_status(connector, &mine, &org, rights, availability.is_some());
        let both_off = level.as_ref().is_some_and(|item| item.tone == Tone::Off)
            && account.as_ref().is_some_and(|item| item.tone == Tone::Off);
        statuses.extend(level);
        if !both_off {
            statuses.extend(account);
        }
        if statuses.is_empty() {
            statuses.push(match connector.usable {
                Access::Usable => status("Available through Nexus", Tone::On),
                Access::Unavailable => status("Not available to you", Tone::Off),
                Access::Unknown => status("Couldn't check your access", Tone::Off),
            });
        }
    }
    Card {
        name: connector_name(&connector.name),
        line: short_line(&connector.description),
        mine: mine.into_iter().cloned().collect(),
        org: org.into_iter().cloned().collect(),
        availability: availability.cloned(),
        statuses,
        connector: connector.clone(),
    }
}

/// The groups in order with their cards, empty ones left out (`shownConnectors` and `groupCards`).
pub(super) fn group_cards(
    connectors: &[Connector],
    connections: &[Connection],
    availability: Option<&[Availability]>,
    rights: Rights,
) -> Vec<(Group, Vec<Card>)> {
    let cards: Vec<Card> = connectors
        .iter()
        .filter(|connector| {
            rights.manage || connector.offered || connections.iter().any(|item| item.connector == connector.name)
        })
        .map(|connector| {
            let chosen = availability.and_then(|list| list.iter().find(|item| item.name == connector.name));
            card(connector, connections, chosen, rights)
        })
        .collect();
    GROUPS
        .iter()
        .filter_map(|(group, _, _)| {
            let said = match group {
                Group::Off => Some(level_label("disabled")),
                Group::Unavailable => Some("Not available to you"),
                _ => None,
            };
            let mut members: Vec<Card> = cards
                .iter()
                .filter(|item| item.group() == *group)
                .cloned()
                .map(|mut item| {
                    item.statuses.retain(|status| Some(status.label.as_str()) != said);
                    item
                })
                .collect();
            members.sort_by(|a, b| {
                a.rank()
                    .cmp(&b.rank())
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            (!members.is_empty()).then_some((*group, members))
        })
        .collect()
}

impl Group {
    /// Off and In preview start folded: Off can hold twenty connectors nobody may use, and preview ones make no calls.
    /// The list a person acts on stays on screen.
    pub(super) fn folds(self) -> bool {
        matches!(self, Group::Off | Group::Preview)
    }
}

/// A row's one status, in a few words: the card's `statuses` read for the row, where the open row says the rest.
/// None when the group's heading already says it, or when the row shows Connect instead (`quick_connect`).
pub(super) fn row_status(card: &Card, rights: Rights) -> Option<Status> {
    let connector = &card.connector;
    if !connector.offered {
        return None;
    }
    if card.mine.iter().chain(&card.org).any(|item| item.needs_reconnect) {
        return Some(status("Reconnect needed", Tone::Warn));
    }
    match card.mine.len() {
        0 => {}
        1 => return Some(status("Connected", Tone::On)),
        count => return Some(status(format!("{count} accounts"), Tone::On)),
    }
    if let Some(availability) = &card.availability {
        let level = availability_status(availability);
        return (level.tone != Tone::Off).then_some(level);
    }
    if !card.org.is_empty() {
        return Some(status("Organization account", Tone::On));
    }
    match connector.usable {
        Access::Unavailable => None,
        Access::Unknown => Some(status("Couldn't check access", Tone::Off)),
        Access::Usable if connector.connects.is_empty() => Some(status("Through Nexus", Tone::On)),
        Access::Usable if quick_connect(card, rights) => None,
        Access::Usable if rights.own && connector.connects.iter().any(|item| item == "user") => {
            Some(status("Not connected", Tone::Off))
        }
        Access::Usable => None,
    }
}

/// Whether a row offers Connect itself: a connector the person may use, has no account at, and connects in the browser
/// (OAuth). A connector with OAuth connects this way even when it also keeps a key form; the key form needs the open row.
pub(super) fn quick_connect(card: &Card, rights: Rights) -> bool {
    let connector = &card.connector;
    rights.own
        && connector.offered
        && connector.usable == Access::Usable
        && connector.connects.iter().any(|item| item == "user")
        && card.mine.is_empty()
        && connector.oauth
}

/// Whether a card matches the filter: its display name, its id or its line, any case. An empty filter matches all.
pub(super) fn matches_filter(card: &Card, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || [&card.name, &card.connector.name, &card.line]
            .iter()
            .any(|text| text.to_lowercase().contains(&query))
}

/// A tone's dot, as Outside access colours its own: mint for working, amber for a step to take (never red, which is
/// for errors), grey for off.
pub(super) fn tone_color(tone: Tone, palette: Palette) -> Hsla {
    match tone {
        Tone::On => Palette::success(),
        Tone::Some => palette.muted,
        Tone::Off => palette.icon,
        Tone::Warn => palette.waiting(),
    }
}

/// A path segment, percent-encoded as `encodeURIComponent` does.
pub(super) fn segment(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// A refusal in plain words: a missing role reads as the admins-only line.
pub(super) fn plain(error: &str) -> String {
    if error.starts_with("only the org's") || error.contains("role_required") || error.contains("HTTP 403") {
        ADMINS_ONLY.to_string()
    } else {
        error.to_string()
    }
}

/// `web, data` as team slugs. None means the whole organization.
pub(super) fn team_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|team| !team.is_empty())
        .map(String::from)
        .collect()
}
