//! The Assistant's composer for a native shell: the Auto chip's model, what `@` picks as context
//! and what `/` runs as a skill. The agent API's `GET` and `PUT /agent/model` and `GET /agent/context` read strictly,
//! the picks an ask carries, the skills and the menus' rules, as the kit's `agent-composer.ts`. Nexus GPUI draws it
//! (`desktop-gpui/src/pages/agent.rs`).
//!
//! An older API answers 404 for each route: it reads as [`Read::Off`], and the shell draws no chip and an empty `@` menu.
//! A bad row of a list is left out. The agent API reads each pick again from the caller's own data, so a pick never
//! reaches data the person cannot see.

use serde_json::{Value, json};

/// `GET` reads the saved model and the models the policy allows; `PUT` with `{ "model" }` saves one.
pub const MODEL_PATH: &str = "/agent/model";
/// `GET` lists what `@` may pick.
pub const CONTEXT_PATH: &str = "/agent/context";
/// The composer's placeholder.
pub const PLACEHOLDER: &str = "Hand off anything, / for skills, @ for context";
/// The most picks one ask carries.
pub const MAX_CONTEXT: usize = 8;
/// A pick's id: 1 to this many printable characters.
const ID_CHARS: usize = 128;
/// A menu label is cut to this many characters.
const LABEL_CHARS: usize = 80;

/// How a read of one of the routes ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Read<T> {
    Ok(T),
    /// The API has no such route (404).
    Off,
    Failed,
}

fn read_of<T>(status: u16, body: Option<&Value>, read: fn(&Value) -> Option<T>) -> Read<T> {
    if status == 404 {
        return Read::Off;
    }
    if !(200..300).contains(&status) {
        return Read::Failed;
    }
    match body.and_then(read) {
        Some(value) => Read::Ok(value),
        None => Read::Failed,
    }
}

/// One chat model the person's policy allows.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub maker: String,
}

/// The saved model (`auto`, `fast`, `best` or a model id) and the models to pick from.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelView {
    pub model: String,
    pub models: Vec<ModelOption>,
    pub models_error: Option<String>,
}

/// The picks every person has, before the models their policy allows: id, label and hint.
pub const MODEL_PICKS: [(&str, &str, &str); 3] = [
    ("auto", "Auto", "Nexus picks the model for each question"),
    ("fast", "Fast", "The quickest model your policy allows"),
    ("best", "Best", "The strongest model your policy allows"),
];

/// A model id as the agent API takes it.
pub fn model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-'))
}

fn model_option_of(value: &Value) -> Option<ModelOption> {
    let id = value.get("id")?.as_str().filter(|id| model_id(id))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty());
    Some(ModelOption {
        id: id.into(),
        name: text("name").unwrap_or(id).into(),
        maker: text("maker")
            .map(String::from)
            .unwrap_or_else(|| id.split('/').next().unwrap_or("other").into()),
    })
}

fn model_view_of(value: &Value) -> Option<ModelView> {
    let model = value.get("model")?.as_str().filter(|model| model_id(model))?;
    let models = value.get("models")?.as_array()?;
    Some(ModelView {
        model: model.into(),
        models: models.iter().filter_map(model_option_of).collect(),
        models_error: value
            .get("modelsError")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .map(String::from),
    })
}

/// `GET` or `PUT /agent/model`'s answer, from its status and body.
pub fn model_read(status: u16, body: Option<&Value>) -> Read<ModelView> {
    read_of(status, body, model_view_of)
}

/// The chip's word for a saved model: Auto, Fast, Best, or the model's name when the list has it, else its id's last part.
pub fn chip_label(model: &str, models: &[ModelOption]) -> String {
    if let Some((_, label, _)) = MODEL_PICKS.iter().find(|(id, _, _)| *id == model) {
        return (*label).into();
    }
    models
        .iter()
        .find(|option| option.id == model)
        .map(|option| option.name.clone())
        .unwrap_or_else(|| model.rsplit('/').next().unwrap_or(model).into())
}

/// `PUT /agent/model`'s body; None for an id the API would refuse.
pub fn model_body(model: &str) -> Option<String> {
    model_id(model).then(|| json!({ "model": model }).to_string())
}

/// What `@` may pick, in the menu's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextKind {
    Connector,
    Errand,
    Project,
    Person,
    Note,
    Conversation,
}

impl ContextKind {
    pub const ALL: [ContextKind; 6] = [
        ContextKind::Connector,
        ContextKind::Errand,
        ContextKind::Project,
        ContextKind::Person,
        ContextKind::Note,
        ContextKind::Conversation,
    ];

    pub fn word(self) -> &'static str {
        match self {
            ContextKind::Connector => "connector",
            ContextKind::Errand => "errand",
            ContextKind::Project => "project",
            ContextKind::Person => "person",
            ContextKind::Note => "note",
            ContextKind::Conversation => "conversation",
        }
    }

    fn of(word: &str) -> Option<ContextKind> {
        ContextKind::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// Its group heading in the menu.
    pub fn group(self) -> &'static str {
        match self {
            ContextKind::Connector => "Accounts",
            ContextKind::Errand => "Errands",
            ContextKind::Project => "Projects",
            ContextKind::Person => "People",
            ContextKind::Note => "Notes",
            ContextKind::Conversation => "Conversations",
        }
    }
}

/// One row of the `@` menu: the pick, its label and a short detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub kind: ContextKind,
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

/// A label cut to [`LABEL_CHARS`], on one line.
pub fn label(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= LABEL_CHARS {
        return line;
    }
    let cut: String = line.chars().take(LABEL_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

fn context_id(id: &str) -> bool {
    !id.trim().is_empty() && id.chars().count() <= ID_CHARS && !id.chars().any(char::is_control)
}

fn context_item_of(value: &Value) -> Option<ContextItem> {
    let kind = ContextKind::of(value.get("kind")?.as_str()?)?;
    let id = value.get("id")?.as_str().filter(|id| context_id(id))?;
    let name = value.get("label")?.as_str().filter(|name| !name.trim().is_empty())?;
    Some(ContextItem {
        kind,
        id: id.into(),
        label: label(name),
        detail: value
            .get("detail")
            .and_then(Value::as_str)
            .filter(|detail| !detail.trim().is_empty())
            .map(label),
    })
}

/// `GET /agent/context`'s answer: the rows, and the parts that could not be read, by kind.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextView {
    pub items: Vec<ContextItem>,
    pub errors: Vec<(ContextKind, String)>,
}

fn context_view_of(value: &Value) -> Option<ContextView> {
    let items = value.get("items")?.as_array()?;
    let errors = value
        .get("errors")
        .and_then(Value::as_object)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|(kind, error)| Some((ContextKind::of(kind)?, error.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(ContextView {
        items: items.iter().filter_map(context_item_of).collect(),
        errors,
    })
}

/// `GET /agent/context`'s answer, from its status and body.
pub fn context_read(status: u16, body: Option<&Value>) -> Read<ContextView> {
    read_of(status, body, context_view_of)
}

/// One built-in skill: what `/` lists, and whether it reads the words after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skill {
    pub id: &'static str,
    pub hint: &'static str,
    pub takes_text: bool,
}

/// The Assistant's skills, each mapped to a command it already obeys (the agent API reads `skill`).
pub const SKILLS: [Skill; 7] = [
    Skill {
        id: "brief",
        hint: "Today's brief",
        takes_text: false,
    },
    Skill {
        id: "errand",
        hint: "Start an errand",
        takes_text: true,
    },
    Skill {
        id: "watch",
        hint: "Tell me when something changes",
        takes_text: true,
    },
    Skill {
        id: "remember",
        hint: "Save a note",
        takes_text: true,
    },
    Skill {
        id: "forget",
        hint: "Forget the last note",
        takes_text: false,
    },
    Skill {
        id: "web",
        hint: "Search the web",
        takes_text: true,
    },
    Skill {
        id: "handoff",
        hint: "Hand off to a session",
        takes_text: true,
    },
];

/// The skills whose id starts with `query`, any case.
pub fn filter_skills(query: &str) -> Vec<Skill> {
    let query = query.to_lowercase();
    SKILLS
        .into_iter()
        .filter(|skill| skill.id.starts_with(&query))
        .collect()
}

/// The rows whose label (or detail) starts a word with `query`, any case; all of them for no query.
pub fn filter_context(items: &[ContextItem], query: &str) -> Vec<ContextItem> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return items.to_vec();
    }
    let starts = |text: &str| {
        let text = text.to_lowercase();
        text.starts_with(&query)
            || text
                .split(|c: char| c.is_whitespace() || matches!(c, '/' | '.' | '_' | '-'))
                .any(|word| word.starts_with(&query))
    };
    items
        .iter()
        .filter(|item| starts(&item.label) || item.detail.as_deref().is_some_and(starts))
        .cloned()
        .collect()
}

/// ↑ and ↓ in a menu of `size` rows: it wraps.
pub fn move_active(active: usize, size: usize, down: bool) -> usize {
    if size == 0 {
        0
    } else if down {
        (active + 1) % size
    } else {
        (active + size - 1) % size
    }
}

/// The menu the text before the caret opens: `/` at the start of the message, `@` at the start or after white space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub skill: bool,
    /// What was typed after the sign, to filter the menu.
    pub query: String,
    /// Where the sign is, in bytes.
    pub start: usize,
}

/// The menu the text before byte `caret` opens, or None. Either ends at white space, so a finished word closes it.
pub fn trigger(text: &str, caret: usize) -> Option<Trigger> {
    let caret = caret.min(text.len());
    let before = text.get(..caret)?;
    let at = before.rfind('@');
    if let Some(at) = at {
        let query = &before[at + 1..];
        let lead_ok = at == 0 || before[..at].chars().next_back().is_some_and(char::is_whitespace);
        if lead_ok && query.chars().count() <= 40 && !query.chars().any(|c| c.is_whitespace() || c == '@') {
            return Some(Trigger {
                skill: false,
                query: query.into(),
                start: at,
            });
        }
    }
    let query = before.strip_prefix('/')?;
    (query.chars().count() <= 32 && query.chars().all(|c| c.is_ascii_alphabetic())).then(|| Trigger {
        skill: true,
        query: query.into(),
        start: 0,
    })
}

/// The text with the sign and what was typed after it taken out, and the caret where they were.
pub fn without_trigger(text: &str, trigger: &Trigger, caret: usize) -> (String, usize) {
    let caret = caret.min(text.len());
    let before = &text[..trigger.start];
    let after = &text[caret..];
    let joined = if before.ends_with(' ') && after.starts_with(' ') {
        format!("{before}{}", &after[1..])
    } else {
        format!("{before}{after}")
    };
    (joined, before.len())
}

/// What one send carries: the words, the picks and the skill.
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub message: String,
    pub context: Vec<(ContextKind, String)>,
    pub skill: Option<&'static str>,
}

/// The send's wire from the box. A skill that needs words and has none is not sendable (None); one that needs none sends
/// its name, which the thread shows.
pub fn wire_of(text: &str, picks: &[ContextItem], skill: Option<Skill>) -> Option<Wire> {
    let words = text.trim();
    if skill.is_some_and(|skill| skill.takes_text && words.is_empty()) {
        return None;
    }
    let message = if words.is_empty() {
        format!("/{}", skill?.id)
    } else {
        words.into()
    };
    Some(Wire {
        message,
        context: picks
            .iter()
            .take(MAX_CONTEXT)
            .map(|pick| (pick.kind, pick.id.clone()))
            .collect(),
        skill: skill.map(|skill| skill.id),
    })
}

/// The thread's line for a send: the skill, the picks as `@label`, then the words.
pub fn display_of(text: &str, picks: &[ContextItem], skill: Option<Skill>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(skill) = skill {
        parts.push(format!("/{}", skill.id));
    }
    parts.extend(picks.iter().map(|pick| format!("@{}", pick.label)));
    let words = text.trim();
    if !words.is_empty() {
        parts.push(words.into());
    }
    parts.join(" ")
}

/// The body of an ask with picks and a skill: streamed, in the conversation when there is one.
pub fn ask_body(
    message: &str,
    conversation: Option<&str>,
    context: &[(ContextKind, String)],
    skill: Option<&str>,
) -> String {
    ask_body_on(message, conversation, context, skill, None)
}

/// [`ask_body`] from a named surface (`desktop` for the Mac chat bar), as `agent::ask_body_on` sends it.
pub fn ask_body_on(
    message: &str,
    conversation: Option<&str>,
    context: &[(ContextKind, String)],
    skill: Option<&str>,
    surface: Option<&str>,
) -> String {
    let mut body = json!({ "message": message, "stream": true });
    if let Some(surface) = surface.filter(|surface| !surface.is_empty()) {
        body["surface"] = json!(surface);
    }
    if let Some(conversation) = conversation.filter(|id| !id.is_empty()) {
        body["conversation"] = json!(conversation);
    }
    if !context.is_empty() {
        body["context"] = context
            .iter()
            .take(MAX_CONTEXT)
            .map(|(kind, id)| json!({ "kind": kind.word(), "id": id }))
            .collect::<Vec<_>>()
            .into();
    }
    if let Some(skill) = skill {
        body["skill"] = json!(skill);
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ContextKind, id: &str, label: &str) -> ContextItem {
        ContextItem {
            kind,
            id: id.into(),
            label: label.into(),
            detail: None,
        }
    }

    #[test]
    fn reads_the_model_view_strictly_and_drops_a_bad_row() {
        let body =
            json!({ "model": "auto", "models": [{ "id": "a/b", "name": "B", "maker": "a" }, { "id": "bad id" }, 3] });
        let Read::Ok(view) = model_read(200, Some(&body)) else {
            panic!("not read")
        };
        assert_eq!(view.model, "auto");
        assert_eq!(
            view.models,
            vec![ModelOption {
                id: "a/b".into(),
                name: "B".into(),
                maker: "a".into()
            }]
        );
        assert_eq!(model_read(404, None), Read::Off);
        assert_eq!(model_read(500, Some(&body)), Read::Failed);
        assert_eq!(
            model_read(200, Some(&json!({ "model": "bad id", "models": [] }))),
            Read::Failed
        );
    }

    #[test]
    fn names_the_chip_and_builds_the_save_body() {
        let models = vec![ModelOption {
            id: "a/b".into(),
            name: "B 1".into(),
            maker: "a".into(),
        }];
        assert_eq!(chip_label("auto", &[]), "Auto");
        assert_eq!(chip_label("a/b", &models), "B 1");
        assert_eq!(chip_label("x/y", &[]), "y");
        assert_eq!(model_body("fast").as_deref(), Some(r#"{"model":"fast"}"#));
        assert_eq!(model_body("a b"), None);
    }

    #[test]
    fn reads_the_context_list_and_its_errors() {
        let body = json!({ "items": [{ "kind": "connector", "id": "gmail", "label": "Gmail", "detail": "maya@acme.com" }, { "kind": "x" }], "errors": { "errand": "down", "nope": "x" } });
        let Read::Ok(view) = context_read(200, Some(&body)) else {
            panic!("not read")
        };
        assert_eq!(view.items.len(), 1);
        assert_eq!(view.items[0].detail.as_deref(), Some("maya@acme.com"));
        assert_eq!(view.errors, vec![(ContextKind::Errand, "down".to_string())]);
    }

    #[test]
    fn filters_by_the_start_of_a_word() {
        let items = vec![
            item(ContextKind::Connector, "gmail", "Gmail"),
            item(ContextKind::Errand, "i1", "Dinner with Ana"),
        ];
        let ids = |rows: Vec<ContextItem>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
        assert_eq!(ids(filter_context(&items, "an")), vec!["i1"]);
        assert_eq!(filter_context(&items, "").len(), 2);
        assert_eq!(
            filter_skills("re").iter().map(|skill| skill.id).collect::<Vec<_>>(),
            vec!["remember"]
        );
    }

    #[test]
    fn opens_the_menu_the_caret_is_in() {
        assert_eq!(
            trigger("/br", 3),
            Some(Trigger {
                skill: true,
                query: "br".into(),
                start: 0
            })
        );
        assert_eq!(trigger("see /br", 7), None);
        assert_eq!(
            trigger("ask @gm", 7),
            Some(Trigger {
                skill: false,
                query: "gm".into(),
                start: 4
            })
        );
        assert_eq!(trigger("a@gm", 4), None);
        assert_eq!(trigger("ask @gm done", 12), None);
        assert_eq!(
            trigger("@", 1),
            Some(Trigger {
                skill: false,
                query: String::new(),
                start: 0
            })
        );
    }

    #[test]
    fn takes_the_trigger_out_and_keeps_one_space() {
        let text = "plan @gm dinner";
        let found = trigger(text, 8).expect("an @ trigger");
        assert_eq!(without_trigger(text, &found, 8), ("plan dinner".to_string(), 5));
        let slash = trigger("/br", 3).expect("a / trigger");
        assert_eq!(without_trigger("/br", &slash, 3), (String::new(), 0));
    }

    #[test]
    fn wraps_the_active_row() {
        assert_eq!(move_active(0, 3, false), 2);
        assert_eq!(move_active(2, 3, true), 0);
        assert_eq!(move_active(0, 0, true), 0);
    }

    #[test]
    fn builds_the_wire_and_the_thread_line() {
        let gmail = item(ContextKind::Connector, "gmail", "Gmail");
        let web = SKILLS[5];
        let brief = SKILLS[0];
        let wire = wire_of(" plan it ", std::slice::from_ref(&gmail), None).expect("a wire");
        assert_eq!(wire.message, "plan it");
        assert_eq!(wire.context, vec![(ContextKind::Connector, "gmail".to_string())]);
        assert_eq!(
            display_of(" plan it ", std::slice::from_ref(&gmail), None),
            "@Gmail plan it"
        );
        assert_eq!(wire_of("", &[], Some(web)), None);
        assert_eq!(
            wire_of("", &[], Some(brief)).expect("a skill-only wire").message,
            "/brief"
        );
        assert_eq!(wire_of("  ", &[], None), None);
        assert_eq!(
            wire_of("cheap flights", &[], Some(web))
                .expect("a wire with a skill")
                .skill,
            Some("web")
        );
    }

    #[test]
    fn writes_an_ask_with_its_picks_and_skill() {
        let body = ask_body(
            "plan it",
            Some("c1"),
            &[(ContextKind::Note, "n1".into())],
            Some("errand"),
        );
        let value: Value = serde_json::from_str(&body).expect("json");
        assert_eq!(value["context"], json!([{ "kind": "note", "id": "n1" }]));
        assert_eq!(value["skill"], "errand");
        assert_eq!(value["conversation"], "c1");
        let plain: Value = serde_json::from_str(&ask_body("hi", None, &[], None)).expect("json");
        assert!(plain.get("context").is_none() && plain.get("skill").is_none());
    }
}
