//! Read adapters for the existing Skills API. Authority comes from scoped lists, never owner labels.
use serde::Deserialize;
use serde_json::Value;

pub const MAX_BYTES: usize = 1024 * 1024;
pub const SKELETON: &str = "---\nname: my-skill\ndescription: Say what this skill does and when an agent should use it.\n---\n\n# my-skill\n\nWrite the instructions here.\n";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub latest_version: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: u64,
    pub scope: String,
    pub team_id: Option<String>,
    pub approved_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub version: u64,
    pub name: String,
    pub created_at: String,
    pub bytes: usize,
    pub content_sha256: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Detail {
    #[serde(flatten)]
    pub skill: Skill,
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub skill_id: String,
    pub version: u64,
    pub content: String,
}

#[derive(Deserialize)]
struct List<T> {
    skills: Vec<T>,
}

pub fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("Nexus returned an invalid Skills answer: {error}"))
}

pub fn own(value: Value) -> Result<Vec<Skill>, String> {
    Ok(parse::<List<Skill>>(value)?.skills)
}

pub fn library(value: Value) -> Result<Vec<LibrarySkill>, String> {
    Ok(parse::<List<LibrarySkill>>(value)?.skills)
}

/// A successful own-list membership is positive authority. A library match always vetoes it.
/// Failed or pending library discovery does not turn an unknown id into a personal skill.
pub fn writable(id: &str, own: Option<&[Skill]>, library: Option<&[LibrarySkill]>) -> bool {
    own.is_some_and(|rows| rows.iter().any(|skill| skill.id == id))
        && !library.is_some_and(|rows| rows.iter().any(|skill| skill.id == id))
}

pub fn path(id: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
    format!("/v1/skills/{encoded}")
}

pub fn validate(text: &str, initial: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        Err("Write the SKILL.md first.".into())
    } else if text == initial {
        Err("Change the text first.".into())
    } else if text.len() > MAX_BYTES {
        Err("A SKILL.md is at most 1 MiB of UTF-8 text.".into())
    } else {
        Ok(())
    }
}

/// Keep all-CRLF input as CRLF, as the shared editor does. Mixed line ends cannot roundtrip safely.
pub fn edited(typed: &str, initial: &str) -> Result<String, String> {
    let crlf = initial.contains("\r\n") && !initial.replace("\r\n", "").contains(['\r', '\n']);
    if initial.contains('\r') && !crlf {
        return Err(
            "This file has mixed line endings. Export it and normalize its line endings before editing.".into(),
        );
    }
    let typed = typed.replace("\r\n", "\n").replace('\r', "\n");
    let text = if crlf { typed.replace('\n', "\r\n") } else { typed };
    // The original byte-order mark is part of the stored file.
    Ok(if initial.starts_with('\u{feff}') && !text.starts_with('\u{feff}') {
        format!("\u{feff}{text}")
    } else {
        text
    })
}

pub fn decode(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > MAX_BYTES {
        return Err("A SKILL.md is at most 1 MiB of UTF-8 text.".into());
    }
    String::from_utf8(bytes).map_err(|_| "This file is not UTF-8. Save it as UTF-8 and pick it again.".into())
}

pub fn problem(error: nexus_client::Error) -> String {
    let extra = match &error {
        nexus_client::Error::Core(error) => match error.code.as_deref() {
            Some("skill_version_conflict") => {
                " Copy your draft, cancel to reload the latest version, then edit it again."
            }
            Some("duplicate_name") => " Change the name in the front matter, or open your existing skill.",
            Some("invalid_frontmatter" | "missing_name" | "missing_description") => {
                " Start with --- on its own line, name: and description:, then another --- line."
            }
            Some("invalid_name") => " Use 1–64 lowercase letters, digits and single hyphens for the name.",
            Some("skill_limit") => " Delete a skill before creating another.",
            Some("skill_version_limit") => " Save this text as a new skill.",
            Some("skill_not_found" | "skill_version_not_found") => {
                " It may have been deleted or access may have changed."
            }
            _ => "",
        },
        _ => "",
    };
    format!("{error}{extra}")
}

/// Make hidden instructions visible in library descriptions, matching the shared screen's warning intent.
pub fn visible(text: &str) -> String {
    static HIDDEN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let hidden = HIDDEN.get_or_init(|| {
        regex::Regex::new(
            r"[\p{Cc}\p{Cf}\p{Zs}\p{Zl}\p{Zp}\u034f\u115f\u1160\u17b4\u17b5\u2800\u3164\uffa0\x{e0100}-\x{e01ef}]",
        )
        .expect("constant hidden-character pattern")
    });
    let mut chars = text.chars().peekable();
    let mut shown = String::new();
    while let Some(c) = chars.next() {
        if matches!(c, ' ' | '\t' | '\n') || c == '\r' && chars.peek() == Some(&'\n') {
            shown.push(c);
        } else if hidden.is_match(c.encode_utf8(&mut [0; 4])) {
            shown.push_str(&format!("[U+{:04X}]", c as u32));
        } else {
            shown.push(c);
        }
    }
    shown
}
