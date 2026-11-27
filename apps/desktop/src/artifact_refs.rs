//! Artifacts referenced in a message: a page, a shared page or a shared chat from the Library, added to the composer
//! from its "+" menu or a Library row's "Add to a session…". Plain data, no UI, so the tests below check it directly.
//!
//! A reference travels as a short block at the end of the message's text ([`with_block`]): its title, kind, id,
//! version and link, so any agent can read it. The app reads each source with its own sign-in and sends it with the
//! message as a file: a chat gets a data file (`attachments::chat_body`), and a Code session's agent a file on its
//! computer (`attachments::Draft::take_artifacts`). The `nexus` CLI may be signed in to another Gateway, so the
//! block never relies on it: it names this app's Gateway for a source that could not be attached.
//! [`split`] reads the block back, so a sent or reloaded message shows its chips instead of the block.

use serde::{Deserialize, Serialize};

use crate::library::{Kind, Open, Row};

/// At most this many artifacts in one message.
pub const MAX: usize = 8;

/// The block's first line. [`split`] looks for it, so it never changes.
const HEADER: &str = "Referenced Nexus artifacts:";
const SEPARATOR: &str = " · ";

/// Who reads the block, which sets its second line: how that agent gets each source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader<'a> {
    /// A chat: no terminal, so only the data files the message carries.
    Chat,
    /// A Code session's agent, with a terminal. The Gateway is this app's, where the artifacts are.
    Terminal { gateway: &'a str },
}

impl Reader<'_> {
    /// The block's second line. It says only what the app does: a chat's sources are data files, and a Code session's
    /// are listed under "Attached files". The CLI line names this app's Gateway, as the CLI's saved one may differ.
    fn hint(self) -> String {
        match self {
            Reader::Chat => {
                "(Nexus attached each source it could read as a file. One it could not read has its link only.)".into()
            }
            Reader::Terminal { gateway } => format!(
                "(Nexus attached each source it could read as a file: see Attached files. For one it could not attach, \
                 run `NEXUS_GATEWAY={gateway} nexus pages get <page> <version> --source`, or \
                 `NEXUS_GATEWAY={gateway} nexus artifacts show <id>` for a share or a chat: Nexus for desktop reads \
                 that Gateway.)"
            ),
        }
    }
}

/// What an artifact is, for the agent: one of your pages, a page shared with you, or a shared chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "lowercase")]
pub enum Target {
    Page(String),
    Share(String),
    Chat(String),
}

impl Target {
    pub fn id(&self) -> &str {
        match self {
            Self::Page(id) | Self::Share(id) | Self::Chat(id) => id,
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Self::Page(_) => "page",
            Self::Share(_) => "share",
            Self::Chat(_) => "chat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub title: String,
    /// "Doc", "Site", "Sheet", "Design" or "Chat" ([`Kind::name`]).
    pub kind: String,
    pub target: Target,
    /// The version it was added at; `None` is the latest.
    pub version: Option<u32>,
    /// The reader's address, when the artifact has one.
    pub link: Option<String>,
}

impl ArtifactRef {
    /// A Library row as a reference. Another organization's share and a revoked link have none: no agent here can
    /// read them.
    pub fn from_row(row: &Row, site: &str) -> Option<ArtifactRef> {
        if row.revoked {
            return None;
        }
        let target = match &row.open {
            Open::Page(id) => Target::Page(id.clone()),
            Open::SharedPage(id) => Target::Share(id.clone()),
            Open::Chat(id) => Target::Chat(id.clone()),
            Open::Browser(_) => return None,
        };
        let link = row
            .link
            .as_deref()
            .and_then(|link| crate::library::reader_url(site, link));
        Some(ArtifactRef {
            title: one_line(&row.title),
            kind: row.artifact_kind().name().to_string(),
            target,
            version: version_number(&row.version),
            link,
        })
    }

    pub fn kind(&self) -> Kind {
        match self.kind.as_str() {
            "Doc" => Kind::Doc,
            "Sheet" => Kind::Sheet,
            "Design" | "Board" => Kind::Board,
            "Chat" => Kind::Chat,
            _ => Kind::Site,
        }
    }

    /// "v4", or "latest".
    pub fn version_label(&self) -> String {
        self.version
            .map_or_else(|| "latest".into(), |version| format!("v{version}"))
    }

    /// Two references to the same artifact are one chip.
    pub fn same(&self, other: &ArtifactRef) -> bool {
        self.target == other.target
    }

    /// Gateway's address of its source, as text: a page's version, or a share's Markdown.
    pub fn source_path(&self) -> String {
        let enc = nexus_client::url::enc;
        match &self.target {
            Target::Page(id) => {
                let version = self
                    .version
                    .map_or_else(|| "latest".into(), |version| version.to_string());
                format!("/v1/pages/{}/versions/{version}/source", enc(id))
            }
            Target::Share(id) | Target::Chat(id) => format!("/v1/artifacts/{}.md", enc(id)),
        }
    }

    /// The data file a chat gets: Markdown for a doc or a chat, plain text for any other source.
    pub fn source_file(&self) -> String {
        let extension = match self.kind() {
            Kind::Doc | Kind::Chat => "md",
            _ => "txt",
        };
        let stem: String = self
            .title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || " -_".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let stem = stem.trim();
        format!(
            "{} ({}).{extension}",
            if stem.is_empty() { "Artifact" } else { stem },
            self.target.id()
        )
    }

    fn line(&self) -> String {
        [
            format!("- {}", self.title),
            self.kind.clone(),
            format!("{} {}", self.target.word(), self.target.id()),
            self.version_label(),
            self.link.clone().unwrap_or_else(|| "-".into()),
        ]
        .join(SEPARATOR)
    }

    fn parse(line: &str) -> Option<ArtifactRef> {
        let line = line.strip_prefix("- ")?;
        let mut parts = line.rsplitn(5, SEPARATOR);
        let link = parts.next()?;
        let version = parts.next()?;
        let (word, id) = parts.next()?.split_once(' ')?;
        let kind = parts.next()?;
        let title = parts.next()?;
        let target = match word {
            "page" => Target::Page(id.into()),
            "share" => Target::Share(id.into()),
            "chat" => Target::Chat(id.into()),
            _ => return None,
        };
        let version = match version {
            "latest" => None,
            version => Some(version.strip_prefix('v')?.parse().ok()?),
        };
        (!id.is_empty() && !title.is_empty()).then(|| ArtifactRef {
            title: title.into(),
            kind: kind.into(),
            target,
            version,
            link: (link != "-").then(|| link.into()),
        })
    }
}

/// "v3" as 3; "latest", or a chat's "v3 · model", as what it is.
fn version_number(version: &str) -> Option<u32> {
    version.strip_prefix('v')?.parse().ok()
}

/// A title on one line, without the block's separator.
fn one_line(title: &str) -> String {
    let title = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(SEPARATOR.trim(), "-");
    if title.is_empty() { "Untitled".into() } else { title }
}

/// The message's text with its references at the end, as `reader` reads it.
pub fn with_block(text: &str, refs: &[ArtifactRef], reader: Reader) -> String {
    if refs.is_empty() {
        return text.to_string();
    }
    let block = std::iter::once(HEADER.to_string())
        .chain(std::iter::once(reader.hint()))
        .chain(refs.iter().map(ArtifactRef::line))
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        block
    } else {
        format!("{}\n\n{block}", text.trim_end())
    }
}

/// Where the block starts in `text`, when it has one that [`split`] reads.
pub fn block_start(text: &str) -> Option<usize> {
    if split(text).1.is_empty() {
        return None;
    }
    if text.starts_with(HEADER) {
        Some(0)
    } else {
        text.rfind(&format!("\n\n{HEADER}"))
    }
}

/// A message's own text and its references. A text without a whole block is all text. The block ends at its first
/// blank line: what follows, such as a Code session's "Attached files" list, stays text after the message's own.
pub fn split(text: &str) -> (String, Vec<ArtifactRef>) {
    let (body, block) = if let Some(block) = text.strip_prefix(HEADER) {
        ("", block)
    } else if let Some(at) = text.rfind(&format!("\n\n{HEADER}")) {
        (&text[..at], &text[at + 2 + HEADER.len()..])
    } else {
        return (text.to_string(), Vec::new());
    };
    // The header's own line ends first; then the hint, if any.
    let mut lines = block.lines().skip(1).peekable();
    if lines.peek().is_some_and(|line| line.starts_with('(')) {
        lines.next();
    }
    let mut refs = Vec::new();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        match ArtifactRef::parse(line) {
            Some(reference) => refs.push(reference),
            None => return (text.to_string(), Vec::new()),
        }
    }
    if refs.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let tail = lines.collect::<Vec<_>>().join("\n");
    let tail = tail.trim();
    let body = match (body.is_empty(), tail.is_empty()) {
        (_, true) => body.to_string(),
        (true, false) => tail.to_string(),
        (false, false) => format!("{body}\n\n{tail}"),
    };
    (body, refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(title: &str, id: &str, version: Option<u32>) -> ArtifactRef {
        ArtifactRef {
            title: title.into(),
            kind: "Doc".into(),
            target: Target::Page(id.into()),
            version,
            link: Some(format!("https://nexus.example/p/{id}")),
        }
    }

    #[test]
    fn a_block_reads_back_as_the_same_references() {
        let refs = vec![
            page("Q4 rollout plan", "pag_1", Some(4)),
            ArtifactRef {
                title: "Pricing review".into(),
                kind: "Chat".into(),
                target: Target::Chat("art_2".into()),
                version: None,
                link: None,
            },
        ];
        let text = with_block("Check the plan", &refs, Reader::Chat);
        assert!(text.starts_with("Check the plan\n\nReferenced Nexus artifacts:\n("));
        assert!(text.contains("- Q4 rollout plan · Doc · page pag_1 · v4 · https://nexus.example/p/pag_1"));
        assert_eq!(split(&text), ("Check the plan".to_string(), refs));
    }

    #[test]
    fn a_message_of_references_only_has_no_text() {
        let refs = vec![page("Plan", "pag_1", None)];
        let text = with_block("  ", &refs, Reader::Chat);
        assert!(text.starts_with(HEADER));
        assert_eq!(split(&text), (String::new(), refs));
    }

    #[test]
    fn a_title_with_the_separator_still_reads_back() {
        let mut one = page("a · b", "pag_1", Some(1));
        one.title = one_line(&one.title);
        let text = with_block("x", std::slice::from_ref(&one), Reader::Chat);
        assert_eq!(split(&text).1, vec![one]);
    }

    #[test]
    fn text_that_only_looks_like_a_block_stays_text() {
        for text in [
            "Hello",
            "Hi\n\nReferenced Nexus artifacts:\n(hint)\n- not a reference",
            "Hi\n\nReferenced Nexus artifacts:\n",
        ] {
            assert_eq!(split(text), (text.to_string(), Vec::new()));
        }
    }

    #[test]
    fn the_hint_promises_only_what_the_app_does() {
        // A chat has no terminal: its hint names no command.
        let chat = with_block("x", &[page("P", "pag_1", Some(1))], Reader::Chat);
        assert!(!chat.contains("nexus pages get"), "{chat}");
        assert!(chat.contains("attached each source it could read as a file"));
        // A Code session's agent runs the CLI against this app's Gateway, not the CLI's saved one.
        let gateway = "https://api.nexus.inc";
        let code = with_block("x", &[page("P", "pag_1", Some(1))], Reader::Terminal { gateway });
        assert!(code.contains("see Attached files"), "{code}");
        assert!(code.contains("`NEXUS_GATEWAY=https://api.nexus.inc nexus pages get <page> <version> --source`"));
        assert!(
            !code.contains("A chat gets"),
            "a Code session's agent is not told it is a chat"
        );
    }

    #[test]
    fn a_code_sessions_attached_files_list_stays_after_the_chips() {
        // `nexus_workbench::attachments::prepare` adds the files' paths after the block.
        let refs = vec![page("Plan", "pag_1", Some(1))];
        let sent = format!(
            "{}\n\nAttached files (on this session's computer):\n- \"Plan (pag_1).txt\": \"/tmp/a/1-Plan (pag_1).txt\"",
            with_block(
                "Read it",
                &refs,
                Reader::Terminal {
                    gateway: "https://g.example"
                }
            )
        );
        let (text, read) = split(&sent);
        assert_eq!(read, refs);
        assert!(text.starts_with("Read it\n\nAttached files"), "{text}");
        let only = format!(
            "{}\n\nAttached files (on this session's computer):\n- x",
            with_block("", &refs, Reader::Chat)
        );
        assert!(split(&only).0.starts_with("Attached files"));
    }

    #[test]
    fn sources_name_the_version_or_the_share() {
        assert_eq!(
            page("P", "pag_1", Some(3)).source_path(),
            "/v1/pages/pag_1/versions/3/source"
        );
        assert_eq!(
            page("P", "pag_1", None).source_path(),
            "/v1/pages/pag_1/versions/latest/source"
        );
        let chat = ArtifactRef {
            target: Target::Chat("art_9".into()),
            kind: "Chat".into(),
            ..page("Notes/1", "x", None)
        };
        assert_eq!(chat.source_path(), "/v1/artifacts/art_9.md");
        assert_eq!(chat.source_file(), "Notes-1 (art_9).md");
    }

    #[test]
    fn versions_read_only_as_numbers() {
        assert_eq!(version_number("v12"), Some(12));
        assert_eq!(version_number("latest"), None);
        assert_eq!(version_number("v3 · claude"), None);
    }
}
