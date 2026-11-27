//! What the chat shows, read from Gateway's JSON: the session list, a session's messages and the models.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    /// Days since 1970-01-01 (UTC) of its last activity, for the sidebar's groups.
    pub day: Option<i64>,
    /// Its last activity in milliseconds since 1970, for the row's age.
    pub at_ms: Option<i64>,
}

impl SessionSummary {
    /// One entry of `GET /v1/chat/sessions`'s `sessions`.
    pub fn from_json(session: &Value) -> Option<SessionSummary> {
        let id = text(session, "id").filter(|id| !id.is_empty())?;
        let title = text(session, "title")
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Untitled".into());
        let updated = text(session, "updatedAt");
        let day = updated.as_deref().and_then(epoch_day);
        let at_ms = updated.as_deref().and_then(epoch_ms);
        Some(SessionSummary { id, title, day, at_ms })
    }
}

/// Today, in days since 1970-01-01 (UTC).
pub fn today() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| (elapsed.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

/// The day of an RFC 3339 time (`2026-11-05T14:03:00Z`), in days since 1970-01-01. The date is read as UTC: a session
/// near midnight may land in the next or previous group, which is fine for a list's headings.
pub fn epoch_day(at: &str) -> Option<i64> {
    let date = at.get(..10)?;
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// An RFC 3339 time in milliseconds since 1970, read as UTC to the second (`2026-11-05T14:03:00Z`).
pub fn epoch_ms(at: &str) -> Option<i64> {
    let day = epoch_day(at)?;
    let clock = at.get(11..19).unwrap_or("00:00:00");
    let mut parts = clock.split(':').map(|part| part.parse::<i64>().ok());
    let (hours, minutes, seconds) = (parts.next()??, parts.next()??, parts.next()??);
    Some(((day * 86_400) + hours * 3_600 + minutes * 60 + seconds) * 1_000)
}

/// A chat model of the catalog (`GET /v1/models?status=all`), as the model menu lists it (`model-menu.tsx`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
    /// Its maker (`owned_by`): the menu's rail.
    pub maker: String,
    /// The row's quiet line: coming soon, whose default it is, or the catalog's short note.
    pub hint: Option<String>,
    /// Supported and allowed: pickable.
    pub usable: bool,
    /// Supported, but the organization's policy turns it off: only counted, in the menu's foot.
    pub off: bool,
    /// The catalog's listed output price per million tokens: the menu marks what the free start pays for.
    pub output_usd: Option<f64>,
}

impl ModelChoice {
    /// Whether the free start pays for it at `cap` per million output tokens: the catalog
    /// lists the first host's price, never below the cheapest Gateway measures, so a marked model is never refused.
    pub fn free_start_pays(&self, cap: f64) -> bool {
        self.output_usd.is_some_and(|output| output <= cap)
    }
}

/// Auto: Nexus picks the model (`nexus/auto`), the composer's default.
pub const AUTO: &str = "nexus/auto";

impl ModelChoice {
    /// One model of the catalog, if it is offered in chat.
    pub fn from_json(model: &Value) -> Option<ModelChoice> {
        let chat = model
            .get("surfaces")
            .and_then(Value::as_array)
            .is_some_and(|surfaces| surfaces.iter().any(|surface| surface.as_str() == Some("chat")));
        let status = model.get("status").and_then(Value::as_str);
        // Coming-soon models are listed whatever their surfaces, as the Nexus app's composer does.
        if !chat && status != Some("coming_soon") {
            return None;
        }
        let id = text(model, "id").filter(|id| !id.is_empty())?;
        let supported = status == Some("supported");
        let allowed = model.get("allowed").and_then(Value::as_bool).unwrap_or(true);
        let name = text(model, "name")
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        let maker = text(model, "owned_by")
            .filter(|maker| !maker.is_empty())
            .unwrap_or_else(|| provider_of(&id));
        let default_for = model
            .get("default_for")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(Value::as_str)
            // An agent's id reads as its name; `chat` and any other kind as they come.
            .map(|first| {
                nexus_client::agent_entries::Agent::parse(first)
                    .map_or(first, |agent| agent.entry().identity.name)
                    .to_string()
            });
        let hint = if !supported {
            Some("Coming soon".to_string())
        } else if let Some(first) = default_for {
            Some(format!("Default for {first}"))
        } else {
            text(model, "note").filter(|note| !note.is_empty() && note.chars().count() <= 60)
        };
        Some(ModelChoice {
            id,
            name,
            maker,
            hint,
            usable: supported && allowed,
            off: supported && !allowed,
            output_usd: model["pricing"]["output"].as_f64(),
        })
    }
}

/// The provider of a `provider/model` id, or of a bare name from a known family (`model-mark.ts`'s `providerOf`).
pub fn provider_of(model: &str) -> String {
    let id = model.to_lowercase();
    if let Some((provider, _)) = id.split_once('/')
        && !provider.is_empty()
    {
        return provider.to_string();
    }
    let families = [
        ("claude", "anthropic"),
        ("gpt", "openai"),
        ("codex", "openai"),
        ("gemini", "google"),
        ("grok", "xai"),
        ("deepseek", "deepseek"),
        ("llama", "meta"),
        ("mistral", "mistral"),
        ("qwen", "alibaba"),
    ];
    families
        .iter()
        .find(|(family, _)| id.starts_with(family))
        .map_or(id.clone(), |(_, provider)| provider.to_string())
}

/// A maker's name on the rail and in search (`makerName`).
pub fn maker_name(maker: &str) -> String {
    match maker {
        "anthropic" => "Anthropic".into(),
        "openai" => "OpenAI".into(),
        "google" => "Google".into(),
        "xai" => "xAI".into(),
        "deepseek" => "DeepSeek".into(),
        "meta" => "Meta".into(),
        "mistral" => "Mistral".into(),
        "alibaba" => "Alibaba".into(),
        "qwen" => "Qwen".into(),
        "nvidia" => "NVIDIA".into(),
        "zai" | "z-ai" => "Z.ai".into(),
        "zhipu" => "Zhipu".into(),
        "nexus" => "Nexus".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        }
    }
}

/// Models with a newer pickable version of the same line (`olderModels`): they fold under "Older models". A line is
/// the id with its first version number taken out: `claude-opus-4-5` and `claude-opus-5-5`.
pub fn older_models(models: &[ModelChoice]) -> std::collections::HashSet<String> {
    let lines: Vec<(&str, bool)> = models.iter().map(|model| (model.id.as_str(), model.usable)).collect();
    older_ids(&lines)
}

/// [`older_models`] over ids and whether each is usable, for lists of another shape (the Models catalog).
pub fn older_ids(models: &[(&str, bool)]) -> std::collections::HashSet<String> {
    let version = |id: &str| -> (String, Vec<u64>) {
        let start = id.find(|c: char| c.is_ascii_digit());
        let Some(start) = start else {
            return (id.to_string(), Vec::new());
        };
        let rest = &id[start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let numbers = rest[..end].split('.').filter_map(|part| part.parse().ok()).collect();
        (format!("{}#{}", &id[..start], &rest[end..]), numbers)
    };
    let newer = |a: &[u64], b: &[u64]| {
        for index in 0..a.len().max(b.len()) {
            let (x, y) = (a.get(index).copied().unwrap_or(0), b.get(index).copied().unwrap_or(0));
            if x != y {
                return x > y;
            }
        }
        false
    };
    models
        .iter()
        .filter(|(id, _)| {
            let (line, own) = version(id);
            models.iter().filter(|(_, usable)| *usable).any(|(other_id, _)| {
                let (other_line, other) = version(other_id);
                other_line == line && newer(&other, &own)
            })
        })
        .map(|(id, _)| id.to_string())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// The page the answer wrote or edited (`write_page`, `edit_page`); never its source.
    pub page: Option<PageBlock>,
    /// The quiet line's facts, for an answer; each is `None` when Gateway did not send it.
    pub meta: Meta,
    /// The web pages the answer's searches read (`sources`), listed under it.
    pub web_sources: Vec<nexus_desktop_core::chat::WebSource>,
    /// Gateway's id, rewind mark and attachments, for Rewind and Edit on a question.
    pub stored: Stored,
}

/// What Gateway keeps of a message beyond its text (`ChatSessionMessage`): its id, whether a rewind took it back
/// (`rewound`), and a question's attachments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stored {
    pub id: Option<String>,
    pub rewound: bool,
    pub attachments: Vec<nexus_desktop_core::chat_rewind::AttachmentRef>,
}

impl Stored {
    pub fn from_message(message: &Value) -> Stored {
        Stored {
            id: text(message, "id").filter(|id| !id.is_empty()),
            rewound: message.get("rewound").and_then(Value::as_bool).unwrap_or(false),
            attachments: nexus_desktop_core::chat_rewind::AttachmentRef::list(message),
        }
    }
}

/// What Gateway tells about an answer, for the quiet line under it (Nexus UI `chat/message-meta.ts`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    /// The model that ran, with its provider (`anthropic/claude-sonnet-5.5`).
    pub model: Option<String>,
    /// Nexus chose the model (the request was `nexus/auto`).
    pub auto: bool,
    pub cost_usd: Option<f64>,
    pub tokens: Option<u64>,
    /// When the message was created, as Gateway wrote it (RFC 3339).
    pub created_at: Option<String>,
}

impl Meta {
    pub fn from_message(message: &Value) -> Meta {
        let model = text(message, "model").filter(|model| !model.is_empty());
        let usage = message.get("usage");
        let count = |field: &str| usage.and_then(|usage| usage.get(field)).and_then(Value::as_u64);
        Meta {
            auto: model.is_some() && text(message, "requestedModel").as_deref() == Some("nexus/auto"),
            model,
            cost_usd: message.get("costUsd").and_then(Value::as_f64),
            tokens: match (count("promptTokens"), count("completionTokens")) {
                (None, None) => None,
                (prompt, completion) => Some(prompt.unwrap_or(0) + completion.unwrap_or(0)),
            },
            created_at: text(message, "createdAt").filter(|at| !at.is_empty()),
        }
    }

    /// The line's parts in Nexus UI's order, each only when known: "Auto → claude-sonnet-5.5 · $0.18 · 41k tokens".
    pub fn parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some(model) = &self.model {
            let name = model_name(model);
            parts.push(if self.auto {
                format!("Auto → {name}")
            } else {
                name.to_string()
            });
        }
        if let Some(cost) = self.cost_usd {
            parts.push(usd(cost));
        }
        if let Some(tokens) = self.tokens.filter(|tokens| *tokens > 0) {
            parts.push(format!("{} tokens", compact(tokens)));
        }
        parts
    }

    pub fn line(&self) -> Option<String> {
        Some(self.parts().join(" · ")).filter(|line| !line.is_empty())
    }

    /// The hover detail, as Nexus UI's `exactTime`: "Nov 6, 2026, 3:42 PM -03".
    pub fn exact_time(&self) -> Option<String> {
        let at = chrono::DateTime::parse_from_rfc3339(self.created_at.as_deref()?).ok()?;
        Some(
            at.with_timezone(&chrono::Local)
                .format("%b %-d, %Y, %-I:%M %p %Z")
                .to_string(),
        )
    }
}

/// A model's name without its provider (`modelName`).
pub fn model_name(model: &str) -> &str {
    let name = model.rsplit('/').next().unwrap_or(model);
    if name.is_empty() { model } else { name }
}

/// 1.2k, 34k, 1.5M: a token count at a glance (`compact`).
pub fn compact(n: u64) -> String {
    let trim = |value: String| value.strip_suffix(".0").map(String::from).unwrap_or(value);
    match n {
        0..=999 => n.to_string(),
        1000..=999_999 => {
            let k = n as f64 / 1000.;
            format!("{}k", trim(format!("{k:.*}", usize::from(n < 10_000))))
        }
        _ => format!("{}M", trim(format!("{:.1}", n as f64 / 1_000_000.))),
    }
}

/// A cost as the Nexus app writes it (`usd`): whole cents with two places, small amounts with four or six.
pub fn usd(value: f64) -> String {
    let abs = value.abs();
    if abs == 0. {
        return "$0.00".into();
    }
    if abs >= 100. || (value * 100. - (value * 100.).round()).abs() < 1e-9 {
        let fixed = format!("{abs:.2}");
        let (whole, cents) = fixed.split_once('.').unwrap_or((&fixed, "00"));
        let mut grouped = String::new();
        for (at, digit) in whole.chars().enumerate() {
            if at > 0 && (whole.len() - at) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        return format!("{}${grouped}.{cents}", if value < 0. { "-" } else { "" });
    }
    if abs >= 0.01 {
        format!("${value:.4}")
    } else {
        format!("${value:.6}")
    }
}

/// A page attached to a chat answer (`ChatPageBlock`): which page, which version, its title and format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBlock {
    pub page_id: String,
    pub version: u32,
    pub title: String,
    pub format: String,
}

impl PageBlock {
    /// A message's `page`, when it has one with an id.
    pub fn from_message(message: &Value) -> Option<PageBlock> {
        let page = message.get("page")?;
        Some(PageBlock {
            page_id: text(page, "pageId").filter(|id| !id.is_empty())?,
            version: page.get("version").and_then(Value::as_u64).unwrap_or(1) as u32,
            title: text(page, "title")
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "Untitled".into()),
            format: text(page, "format").unwrap_or_default(),
        })
    }

    /// The card's quiet line, as Nexus UI's `pageFacts`: the version and the format.
    pub fn facts(&self) -> String {
        let format = match self.format.as_str() {
            "md" => "Markdown",
            "jsx" => "React",
            _ => "HTML",
        };
        format!("Version {} · {format}", self.version)
    }
}

impl Message {
    /// One entry of `GET /v1/chat/sessions/{id}`'s `messages`; system and tool messages are not shown.
    pub fn from_json(message: &Value) -> Option<Message> {
        let role = match message.get("role").and_then(Value::as_str)? {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => return None,
        };
        Some(Message {
            role,
            content: content(message),
            page: PageBlock::from_message(message),
            meta: Meta::from_message(message),
            web_sources: nexus_desktop_core::chat::web_sources(message),
            stored: Stored::from_message(message),
        })
    }
}

/// The meta for each of a chat's local messages (their roles, oldest first), read from Gateway's copy of the chat. The
/// two lists end at the same message, so they are matched from the end; a role that differs stops the match, and a
/// message that already has meta (or is a question) gets `None`.
pub fn meta_for(local: &[(Role, bool)], stored: &[Message]) -> Vec<Option<Meta>> {
    let mut found = vec![None; local.len()];
    for (slot, (message, (role, has_meta))) in found.iter_mut().rev().zip(stored.iter().rev().zip(local.iter().rev())) {
        if message.role != *role {
            break;
        }
        if *role == Role::Assistant && !has_meta && message.meta != Meta::default() {
            *slot = Some(message.meta.clone());
        }
    }
    found
}

/// A message's text as stored by Gateway, followed by any historical source labels.
pub fn content(message: &Value) -> String {
    let text = match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    text + &source_labels(message)
}

fn source_labels(message: &Value) -> String {
    let Some(citations) = message
        .get("citations")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
    else {
        return String::new();
    };
    let mut out = String::from("\n\nSources:\n");
    for citation in citations {
        let part = |name: &str| {
            citation
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        };
        let title: Vec<&str> = ["repo", "page", "section"].into_iter().filter_map(part).collect();
        let n = citation.get("n").and_then(Value::as_u64).unwrap_or(0);
        let title = title.join(" › ");
        out.push_str(&format!("\n- [{n}] {title}"));
    }
    out
}

/// A new session's title: the prompt's first line, at most 60 characters (as the CLI titles it).
pub fn title_from(prompt: &str) -> String {
    prompt.lines().next().unwrap_or("").trim().chars().take(60).collect()
}

pub fn text(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_old_answer_keeps_its_source_labels() {
        let answer = serde_json::json!({
            "role": "assistant",
            "content": "In `auth.rs` [1].",
            "citations": [{ "n": 1, "repo": "acme/api", "page": "Auth", "section": "Tokens", "slug": "auth", "path": "auth#tokens", "runId": "r" }],
        });
        assert_eq!(
            content(&answer),
            "In `auth.rs` [1].\n\nSources:\n\n- [1] acme/api › Auth › Tokens"
        );
        assert_eq!(
            content(&serde_json::json!({ "content": "Plain.", "citations": [] })),
            "Plain."
        );
    }

    #[test]
    fn meta_line_follows_the_nexus_app() {
        let message = json!({
            "role": "assistant", "content": "hi", "model": "anthropic/claude-sonnet-5.5",
            "requestedModel": "nexus/auto", "costUsd": 0.01,
            "usage": { "promptTokens": 1000, "completionTokens": 234 },
        });
        let meta = Message::from_json(&message).unwrap().meta;
        assert_eq!(
            meta.line().as_deref(),
            Some("Auto → claude-sonnet-5.5 · $0.01 · 1.2k tokens")
        );
        let named = json!({ "role": "assistant", "model": "anthropic/claude-sonnet-5.5", "requestedModel": "anthropic/claude-sonnet-5.5" });
        assert_eq!(Meta::from_message(&named).line().as_deref(), Some("claude-sonnet-5.5"));
        assert_eq!(Meta::from_message(&json!({ "role": "assistant" })).line(), None);
    }

    #[test]
    fn fills_a_streamed_answers_meta_from_the_stored_chat() {
        let answer = |model: &str| Message {
            role: Role::Assistant,
            content: "a".into(),
            page: None,
            web_sources: Vec::new(),
            meta: Meta {
                model: Some(model.into()),
                cost_usd: Some(0.5),
                ..Meta::default()
            },
            stored: Stored::default(),
        };
        let question = Message {
            role: Role::User,
            content: "q".into(),
            page: None,
            web_sources: Vec::new(),
            meta: Meta::default(),
            stored: Stored::default(),
        };
        let stored = vec![question.clone(), answer("old"), question, answer("new")];
        // Both answers streamed or loaded without meta: only what is empty is filled, and from the end.
        let local = [
            (Role::User, false),
            (Role::Assistant, true),
            (Role::User, false),
            (Role::Assistant, false),
        ];
        let filled = meta_for(&local, &stored);
        assert_eq!(filled[3].as_ref().and_then(|meta| meta.model.as_deref()), Some("new"));
        assert!(filled[0].is_none() && filled[1].is_none() && filled[2].is_none());
        // A new question asked meanwhile: the roles no longer line up, so nothing is filled.
        let ahead = [
            (Role::User, false),
            (Role::Assistant, false),
            (Role::User, false),
            (Role::Assistant, false),
            (Role::User, false),
        ];
        assert!(meta_for(&ahead, &stored).iter().all(Option::is_none));
        // Gateway has no facts yet: nothing to fill.
        let bare = vec![Message {
            meta: Meta::default(),
            ..answer("x")
        }];
        assert!(meta_for(&[(Role::Assistant, false)], &bare)[0].is_none());
    }

    #[test]
    fn formats_cost_and_tokens() {
        assert_eq!(usd(0.), "$0.00");
        assert_eq!(usd(0.18), "$0.18");
        assert_eq!(usd(0.0123), "$0.0123");
        assert_eq!(usd(0.000123), "$0.000123");
        assert_eq!(usd(1234.5), "$1,234.50");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1234), "1.2k");
        assert_eq!(compact(10_000), "10k");
        assert_eq!(compact(41_000), "41k");
        assert_eq!(compact(1_500_000), "1.5M");
        assert_eq!(compact(2_000_000), "2M");
    }

    #[test]
    fn reads_sessions_and_skips_ones_without_an_id() {
        let named = json!({ "id": "s1", "title": "  Plan the launch " });
        let untitled = json!({ "id": "s2", "title": "" });
        assert_eq!(
            SessionSummary::from_json(&named),
            Some(SessionSummary {
                id: "s1".into(),
                title: "Plan the launch".into(),
                day: None,
                at_ms: None,
            })
        );
        assert_eq!(
            SessionSummary::from_json(&untitled).map(|s| s.title),
            Some("Untitled".into())
        );
        assert_eq!(SessionSummary::from_json(&json!({ "title": "x" })), None);
    }

    #[test]
    fn reads_user_and_assistant_messages_only() {
        let user = json!({ "role": "user", "content": "hi" });
        let parts = json!({ "role": "assistant", "content": [{ "type": "text", "text": "a" }, { "text": "b" }] });
        assert_eq!(Message::from_json(&user).map(|m| m.role), Some(Role::User));
        assert_eq!(Message::from_json(&parts).map(|m| m.content), Some("a\nb".into()));
        assert_eq!(Message::from_json(&json!({ "role": "system", "content": "x" })), None);
    }

    #[test]
    fn reads_an_answers_page() {
        let answer = json!({
            "role": "assistant",
            "content": "Done.",
            "page": { "pageId": "pag_1", "version": 2, "title": "Plan", "format": "md" },
        });
        let page = Message::from_json(&answer).and_then(|m| m.page).unwrap();
        assert_eq!(page.page_id, "pag_1");
        assert_eq!(page.facts(), "Version 2 · Markdown");
        let bare = json!({ "role": "assistant", "content": "x", "page": { "pageId": "" } });
        assert_eq!(Message::from_json(&bare).and_then(|m| m.page), None);
    }

    #[test]
    fn reads_days_and_times() {
        assert_eq!(epoch_day("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_day("2000-03-01T12:00:00Z"), Some(11_017));
        assert_eq!(epoch_day("2026-11-05"), Some(20_762));
        assert_eq!(epoch_day("not a date"), None);
        assert_eq!(epoch_ms("1970-01-02T00:01:02Z"), Some(86_462_000));
    }

    #[test]
    fn older_versions_fold_and_makers_read() {
        let model = |id: &str| ModelChoice {
            id: id.into(),
            name: id.into(),
            maker: provider_of(id),
            hint: None,
            usable: true,
            off: false,
            output_usd: None,
        };
        let models = [
            model("anthropic/claude-opus-5-5"),
            model("anthropic/claude-opus-4-5"),
            model("openai/gpt-5"),
        ];
        let older = older_models(&models);
        assert!(older.contains("anthropic/claude-opus-4-5"));
        assert_eq!(older.len(), 1);
        assert_eq!(provider_of("claude-haiku-5-5"), "anthropic");
        assert_eq!(maker_name("openai"), "OpenAI");
        assert_eq!(maker_name("acme"), "Acme");
    }

    #[test]
    fn reads_chat_models_as_usable_off_or_coming_soon() {
        let usable = json!({ "id": "a/x", "name": "X", "surfaces": ["chat"], "status": "supported", "allowed": true });
        let blocked = json!({ "id": "a/y", "surfaces": ["chat"], "status": "supported", "allowed": false });
        let soon = json!({ "id": "a/z", "surfaces": ["chat"], "status": "coming_soon" });
        let read = |model: &Value| ModelChoice::from_json(model).expect("a chat model");
        assert!(read(&usable).usable);
        assert!(read(&blocked).off && !read(&blocked).usable);
        assert_eq!(read(&soon).hint.as_deref(), Some("Coming soon"));
        assert!(!read(&soon).usable && !read(&soon).off);
        assert_eq!(
            ModelChoice::from_json(&json!({ "id": "a/w", "surfaces": ["api"] })),
            None
        );
    }

    #[test]
    fn titles_by_the_first_line() {
        assert_eq!(title_from("  Fix the login\nmore"), "Fix the login");
        assert_eq!(title_from(&"a".repeat(80)).len(), 60);
    }
}
