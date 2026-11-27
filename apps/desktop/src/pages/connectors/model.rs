use super::*;

// ------------------------------------------------------------------ What Gateway sends

/// What the reader may do, from `GET /v1/tenants/me`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Rights {
    /// `policy.manage`: an owner or admin.
    pub(super) manage: bool,
    /// `connections.manage_own`: may connect their own accounts.
    pub(super) own: bool,
}

pub(super) fn parse_rights(me: &Value) -> Rights {
    let has = |name: &str| {
        me.get("permissions")
            .and_then(Value::as_array)
            .is_some_and(|list| list.iter().any(|item| item.as_str() == Some(name)))
    };
    Rights {
        manage: has("policy.manage"),
        own: has("connections.manage_own"),
    }
}

/// Whether a member may connect (`memberAccess`): absent is yes, `null` is unknown, `false` is no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Access {
    Usable,
    Unavailable,
    Unknown,
}

/// One input of a key connector's form (`formFields`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Field {
    pub(super) name: String,
    pub(super) label: String,
    pub(super) description: Option<String>,
    pub(super) placeholder: Option<String>,
    pub(super) required: bool,
    pub(super) secret: bool,
}

/// One connector of `GET /v1/connectors`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Connector {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) operations: Vec<(String, String)>,
    /// Absent reads as offered: an older Gateway offered every one.
    pub(super) offered: bool,
    pub(super) usable: Access,
    /// `user` and `org`: whose accounts it runs with. Empty when it has its own setup.
    pub(super) connects: Vec<String>,
    /// It connects with a key form.
    pub(super) keyed: bool,
    pub(super) fields: Vec<Field>,
    pub(super) oauth: bool,
}

/// One account of `GET /v1/connections`. The secret never comes back.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Connection {
    pub(super) connector: String,
    /// `user` or `org`.
    pub(super) scope: String,
    pub(super) account: String,
    pub(super) name: String,
    pub(super) identity: Option<String>,
    pub(super) needs_reconnect: bool,
    pub(super) suspended: bool,
    pub(super) last4: Option<String>,
    pub(super) created_at: String,
}

/// Who may use a connector, from `GET /v1/policies/connectors` (owners and admins only).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Availability {
    pub(super) name: String,
    /// `disabled`, `available` or `enabled_for_all`.
    pub(super) level: String,
    /// Team slugs. Empty means the whole organization.
    pub(super) teams: Vec<String>,
    /// Nobody chose yet.
    pub(super) default: bool,
}

pub(super) fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(String::from)
}

pub(super) fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

pub(super) fn list<'a>(answer: &'a Value, key: &str) -> &'a [Value] {
    answer
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(super) fn parse_fields(form: &Value) -> Vec<Field> {
    let required = strings(form, "required");
    let Some(properties) = form.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, property)| {
            let spaced = name.replace('_', " ");
            let mut chars = spaced.chars();
            let label = chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default();
            Field {
                name: name.clone(),
                label,
                description: string(property, "description"),
                placeholder: property
                    .get("examples")
                    .and_then(Value::as_array)
                    .and_then(|examples| examples.first())
                    .and_then(Value::as_str)
                    .map(String::from),
                required: required.iter().any(|item| item == name),
                secret: property.get("x-nexus-secret").and_then(Value::as_bool) == Some(true),
            }
        })
        .collect()
}

pub(super) fn parse_connectors(answer: &Value) -> Vec<Connector> {
    list(answer, "connectors")
        .iter()
        .filter_map(|item| {
            let name = string(item, "name")?;
            let form = item.get("form").filter(|form| form.is_object());
            Some(Connector {
                name,
                description: string(item, "description").unwrap_or_default(),
                operations: list(item, "operations")
                    .iter()
                    .filter_map(|operation| {
                        Some((
                            string(operation, "name")?,
                            string(operation, "summary").unwrap_or_default(),
                        ))
                    })
                    .collect(),
                offered: item.get("offered").and_then(Value::as_bool) != Some(false),
                usable: match item.get("usable") {
                    None | Some(Value::Bool(true)) => Access::Usable,
                    Some(Value::Bool(false)) => Access::Unavailable,
                    Some(_) => Access::Unknown,
                },
                connects: strings(item, "connects"),
                keyed: form.is_some(),
                fields: form.map(parse_fields).unwrap_or_default(),
                oauth: item.get("oauth").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect()
}

pub(super) fn parse_connection(item: &Value) -> Option<Connection> {
    let account = string(item, "account")?;
    Some(Connection {
        connector: string(item, "connector")?,
        scope: if string(item, "scope").as_deref() == Some("org") {
            "org".into()
        } else {
            "user".into()
        },
        name: string(item, "name").unwrap_or_else(|| account.clone()),
        account,
        identity: string(item, "identity"),
        needs_reconnect: string(item, "status").as_deref() == Some("needs_reconnect"),
        suspended: item.get("suspended").and_then(Value::as_bool) == Some(true),
        last4: string(item, "last4"),
        created_at: string(item, "createdAt").unwrap_or_default(),
    })
}

pub(super) fn parse_connections(answer: &Value) -> Vec<Connection> {
    list(answer, "connections")
        .iter()
        .filter_map(parse_connection)
        .collect()
}

pub(super) fn parse_one_availability(item: &Value) -> Option<Availability> {
    let level = string(item, "availability")?;
    if !matches!(level.as_str(), "disabled" | "available" | "enabled_for_all") {
        return None;
    }
    Some(Availability {
        name: string(item, "name")?,
        level,
        teams: strings(item, "teams"),
        default: item.get("default").and_then(Value::as_bool) == Some(true),
    })
}

pub(super) fn parse_availability(answer: &Value) -> Vec<Availability> {
    list(answer, "connectors")
        .iter()
        .filter_map(parse_one_availability)
        .collect()
}

/// One thing a connector lets agents do, from `GET /v1/connectors/{name}`: the list omits each operation's effect.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Ability {
    /// It changes something at the service (`effect: mutate`).
    pub(super) changes: bool,
    pub(super) text: String,
}

pub(super) fn parse_abilities(answer: &Value) -> Vec<Ability> {
    list(answer, "operations")
        .iter()
        .filter_map(|operation| {
            let name = string(operation, "name")?;
            Some(Ability {
                changes: string(operation, "effect").as_deref() == Some("mutate"),
                text: string(operation, "summary")
                    .map(|summary| plain_summary(&summary))
                    .unwrap_or_else(|| name.replace('_', " ")),
            })
        })
        .collect()
}

/// An operation's summary without the catalogue's lead-in: "Write (needs a write grant): resolve a thread" reads
/// "Resolve a thread", because the Change heading over it already says so.
pub(super) fn plain_summary(summary: &str) -> String {
    let text = summary.trim();
    let text = match text.strip_prefix("Write") {
        Some(rest) => rest
            .split_once("): ")
            .or_else(|| rest.split_once(": "))
            .map_or(text, |(_, rest)| rest),
        None => text,
    };
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Chat's master switch, from `GET /v1/policies/chat-writes`.
pub(super) fn parse_chat(answer: &Value) -> Option<bool> {
    answer.get("enabled").and_then(Value::as_bool)
}
