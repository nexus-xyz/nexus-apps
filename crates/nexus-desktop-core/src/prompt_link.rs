//! Prompt links for desktop drafts.
//! A link fills a draft and never sends it.

pub use crate::links::{MAX_LINK, SCHEME, accepted};
use nexus_workbench::event::SessionAgent;

/// The most characters a link's prompt keeps.
pub const MAX_PROMPT: usize = 8_000;
const MAX_MODEL: usize = 128;

/// A repository, wherever it is written: lower case `host` and `path` without `.git`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptLink {
    pub prompt: String,
    pub repo: Option<RepoRef>,
    pub agent: Option<SessionAgent>,
    pub model: Option<String>,
    /// What the link asked for that could not be used, or what was taken out, in words for the person.
    pub notes: Vec<String>,
}

/// Characters that show nothing and can hide instructions from the person reading,
/// and control characters other than the tab and the line break. Emoji joiners stay between two pictures.
fn invisible(c: char) -> bool {
    let code = c as u32;
    matches!(code,
        0x00..=0x08 | 0x0B..=0x1F | 0x7F..=0x9F
        | 0xAD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB
        | 0xE000..=0xF8FF | 0xE0000..=0xE007F | 0xE0100..=0xE01EF | 0xF0000..=0x10FFFF)
}

/// Roughly `\p{Extended_Pictographic}`: enough to keep a joined emoji whole.
fn pictograph(c: char) -> bool {
    matches!(c as u32, 0x2190..=0x2BFF | 0x1F000..=0x1FAFF | 0xA9 | 0xAE | 0x203C | 0x2049)
}

/// The prompt as the draft gets it: line breaks made `\n`, invisible characters out (`hidden`), trimmed, at most
/// MAX_PROMPT characters (`cut`).
pub fn clean_prompt(raw: &str) -> (String, bool, bool) {
    let text = raw.replace("\r\n", "\n").replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let chars: Vec<char> = text.chars().collect();
    let mut hidden = false;
    let mut kept = String::with_capacity(text.len());
    for (at, &c) in chars.iter().enumerate() {
        let before = at.checked_sub(1).map(|i| chars[i]);
        let emoji = match c {
            '\u{FE0F}' => before.is_some_and(pictograph),
            '\u{200D}' => {
                before.is_some_and(|b| pictograph(b) || b == '\u{FE0F}')
                    && chars.get(at + 1).is_some_and(|&a| pictograph(a))
            }
            _ => false,
        };
        if invisible(c) && c != '\t' && c != '\n' && !emoji {
            hidden = true;
        } else {
            kept.push(c);
        }
    }
    let kept = kept.trim();
    let cut = kept.chars().count() > MAX_PROMPT;
    let text = if cut {
        kept.chars().take(MAX_PROMPT).collect::<String>().trim_end().to_string()
    } else {
        kept.to_string()
    };
    (text, hidden, cut)
}

fn segment_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// A repository as a link or a remote writes it: `owner/name` (GitHub), a URL, or `git@host:owner/name.git`.
pub fn repo_ref(text: &str) -> Option<RepoRef> {
    let value = text.trim();
    if value.is_empty() || value.len() > 300 || value.starts_with(['/', '.', '~', '\\']) {
        return None;
    }
    let (host, path) = if let Some((user_host, path)) = value
        .split_once(':')
        .filter(|(left, right)| left.contains('@') && !left.contains('/') && !right.starts_with('/'))
    {
        (user_host.rsplit('@').next()?.to_string(), path.to_string())
    } else if value.contains("://") {
        let url = url::Url::parse(value).ok()?;
        if !["https", "http", "ssh", "git", "git+ssh", "ssh+git"].contains(&url.scheme()) {
            return None;
        }
        (url.host_str()?.to_string(), url.path().to_string())
    } else {
        let mut parts: Vec<&str> = value.split('/').collect();
        let host = if parts[0].contains('.') {
            parts.remove(0).to_string()
        } else {
            "github.com".to_string()
        };
        (host, parts.join("/"))
    };
    let mut host = host.to_lowercase();
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .or_else(|| path.strip_suffix(".GIT"))
        .unwrap_or(path);
    let mut segments: Vec<&str> = path.split('/').collect();
    if host == "github.com" || host == "www.github.com" {
        host = "github.com".to_string();
        segments.truncate(2);
    }
    let host_ok = host
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    if !host_ok || host.is_empty() || segments.len() < 2 || !segments.iter().all(|s| segment_ok(s)) {
        return None;
    }
    Some(RepoRef {
        host,
        path: segments.join("/").to_lowercase(),
    })
}

/// At most 60 visible characters of something a link said, for a note.
fn short(text: &str) -> String {
    let clean = clean_prompt(text).0.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > 60 {
        format!("{}…", clean.chars().take(60).collect::<String>())
    } else {
        clean
    }
}

/// Reads a `nexus://new?…` link. Anything else is refused with a sentence for the person.
pub fn read(text: &str) -> Result<PromptLink, String> {
    let url = url::Url::parse(text.trim()).map_err(|_| "This link could not be read.".to_string())?;
    if url.scheme() != SCHEME {
        return Err("This isn't a Nexus link.".into());
    }
    let place = format!("{}{}", url.host_str().unwrap_or(""), url.path());
    if !place.trim_matches('/').eq_ignore_ascii_case("new") {
        return Err("Nexus for desktop opens new-session links only: nexus://new?prompt=…".into());
    }
    let get = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let mut notes = Vec::new();
    let (prompt, hidden, cut) = clean_prompt(&get("prompt").unwrap_or_default());
    if hidden {
        notes.push("Characters nobody can see were taken out of the prompt.".to_string());
    }
    if cut {
        notes.push(format!(
            "The prompt was longer than {MAX_PROMPT} characters and was cut."
        ));
    }
    let repo_asked = get("repo");
    let repo = repo_asked.as_deref().and_then(repo_ref);
    if let (Some(asked), None) = (&repo_asked, &repo) {
        notes.push(format!(
            "The link's repository, \"{}\", could not be read.",
            short(asked)
        ));
    }
    let agent_asked = get("agent").or_else(|| get("provider"));
    let agent = agent_asked.as_deref().map(str::to_lowercase).and_then(|asked| {
        [SessionAgent::Claude, SessionAgent::Codex]
            .into_iter()
            .find(|agent| agent.as_str() == asked)
    });
    if let (Some(asked), None) = (&agent_asked, agent) {
        notes.push(format!(
            "The link's agent, \"{}\", isn't one Nexus runs. Your usual agent is used.",
            short(asked)
        ));
    }
    let model_asked = get("model");
    let model = model_asked.clone().filter(|model| {
        model.len() <= MAX_MODEL
            && model.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '@' | '[' | ']' | '-'))
    });
    if model_asked.is_some() && model.is_none() {
        notes.push("The link's model could not be read. Your usual model is used.".to_string());
    }
    Ok(PromptLink {
        prompt,
        repo,
        agent,
        model,
        notes,
    })
}

/// The link a draft carries, and whether its prompt reached the character cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub link: String,
    pub cut: bool,
}

/// Write a cleaned draft with its first usable remote. Spaces use `%20`; literal plus signs use `%2B`.
pub fn write<'a>(
    text: &str,
    remotes: impl IntoIterator<Item = &'a str>,
    agent: SessionAgent,
    model: Option<&str>,
) -> Written {
    let (prompt, _, cut) = clean_prompt(text);
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if !prompt.is_empty() {
        query.append_pair("prompt", &prompt);
    }
    if let Some(repo) = remotes.into_iter().find_map(repo_ref) {
        let repo = if repo.host == "github.com" {
            repo.path
        } else {
            format!("{}/{}", repo.host, repo.path)
        };
        query.append_pair("repo", &repo);
    }
    query.append_pair("agent", agent.as_str());
    if let Some(model) = model.filter(|model| !model.is_empty()) {
        query.append_pair("model", model);
    }
    Written {
        link: format!("nexus://new?{}", query.finish().replace('+', "%20")),
        cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_link_reads_back_its_prompt_repo_agent_and_model() {
        let written = write(
            "Fix a+b & the\nbuild",
            ["/local/mirror", "git@github.com:Acme/App.git"],
            SessionAgent::Codex,
            Some("gpt-5.5"),
        );
        assert!(!written.cut);
        assert_eq!(
            written.link,
            "nexus://new?prompt=Fix%20a%2Bb%20%26%20the%0Abuild&repo=acme%2Fapp&agent=codex&model=gpt-5.5"
        );
        let read = read(&written.link).expect("valid prompt-link fixture");
        assert_eq!(read.prompt, "Fix a+b & the\nbuild");
        assert_eq!(read.agent, Some(SessionAgent::Codex));
        assert_eq!(read.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(read.repo.expect("valid prompt-link fixture").path, "acme/app");
    }

    #[test]
    fn writer_cleans_and_caps_unicode_prompts_and_omits_absent_fields() {
        assert_eq!(
            write("Hi", [], SessionAgent::Claude, None).link,
            "nexus://new?prompt=Hi&agent=claude"
        );
        assert_eq!(
            write(" \u{200b}Hi\r\nthere ", [], SessionAgent::Claude, None).link,
            "nexus://new?prompt=Hi%0Athere&agent=claude"
        );
        let written = write(&"😀".repeat(MAX_PROMPT + 1), [], SessionAgent::Claude, None);
        assert!(written.cut);
        assert_eq!(
            read(&written.link)
                .expect("valid prompt-link fixture")
                .prompt
                .chars()
                .count(),
            MAX_PROMPT
        );
    }

    #[test]
    fn reads_a_new_link() {
        let link = read("nexus://new?prompt=Fix%20the%20build&repo=Acme/nexus&agent=codex&model=gpt-5.5")
            .expect("valid prompt-link fixture");
        assert_eq!(link.prompt, "Fix the build");
        assert_eq!(
            link.repo,
            Some(RepoRef {
                host: "github.com".into(),
                path: "acme/nexus".into()
            })
        );
        assert_eq!(link.agent, Some(SessionAgent::Codex));
        assert_eq!(link.model.as_deref(), Some("gpt-5.5"));
        assert!(link.notes.is_empty());
        assert!(read("nexus:///new?prompt=a").is_ok());
        assert!(read("nexus://a/key").is_err());
        assert!(read("https://new").is_err());
    }

    #[test]
    fn takes_out_hidden_characters_but_keeps_emoji() {
        let (text, hidden, _) = clean_prompt("do\u{200B}this \u{1F469}\u{200D}\u{1F4BB}");
        assert_eq!(text, "dothis \u{1F469}\u{200D}\u{1F4BB}");
        assert!(hidden);
    }

    #[test]
    fn reads_repositories_however_written() {
        let github = Some(RepoRef {
            host: "github.com".into(),
            path: "o/n".into(),
        });
        assert_eq!(repo_ref("git@github.com:o/n.git"), github);
        assert_eq!(repo_ref("https://github.com/o/n/pull/7"), github);
        assert_eq!(repo_ref("o/n"), github);
        assert_eq!(repo_ref("gitlab.com/g/s/n").map(|r| r.path), Some("g/s/n".into()));
        assert_eq!(repo_ref("/home/me/repo"), None);
        assert_eq!(repo_ref("name"), None);
    }
}
