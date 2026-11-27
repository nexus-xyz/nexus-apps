//! Which Nexus pages a session published, for a shell that is not a webview (Nexus GPUI). A page comes from the
//! agent's answer, as the viewer link `nexus pages publish` prints on its `View it:` line (`<Nexus UI>/app/<org>/pages/<page-id>`, or `/app?next=/pages/<page-id>` before an
//! organization is saved), or from a call of the built-in Nexus MCP server's `createPage` or `writePageVersion`,
//! whose answer names the page. Pure, so the rules are tested on their own.

use serde_json::Value;

/// A page a card shows: its id, and the title known before Gateway answers (a link's text, a tool's answer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRef {
    pub page_id: String,
    pub label: Option<String>,
}

/// The tools that publish a page: the Nexus MCP server's, as Claude Code and Codex name them.
pub const PAGE_TOOLS: [&str; 2] = ["mcp__nexus__createPage", "mcp__nexus__writePageVersion"];

/// A page's id: `pag_` and the contract's id characters (`openapi.json`, `GET /v1/pages/{id}`).
fn is_page_id(id: &str) -> bool {
    id.len() > 4
        && id.starts_with("pag_")
        && id[4..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The page a link opens, or None for any other link. Only Nexus UI's page viewer counts, at any Gateway's address.
pub fn page_link(href: &str) -> Option<String> {
    let url = url::Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let path = url.path();
    let segments: Vec<&str> = path.strip_prefix('/')?.split('/').collect();
    let own = match segments.as_slice() {
        ["app", org, "pages", id] | ["app", org, "pages", id, ""] if !org.is_empty() && !id.is_empty() => Some(*id),
        _ => None,
    };
    let id = match own {
        Some(id) => decode(id)?,
        // `(^|/)app/?$`: the CLI's link through the active organization.
        None if path.ends_with("/app") || path.ends_with("/app/") => {
            let next = url
                .query_pairs()
                .find_map(|(key, value)| (key == "next").then(|| value.into_owned()))?;
            let id = next.strip_prefix("/pages/")?;
            if id.is_empty() || id.contains(['/', '?', '#']) {
                return None;
            }
            decode(id)?
        }
        None => return None,
    };
    is_page_id(&id).then_some(id)
}

/// The pages an answer links to, in order, each once, with a Markdown link's text as its label.
pub fn page_links_in(text: &str) -> Vec<PageRef> {
    let mut labels: Vec<(String, String)> = Vec::new();
    for (label, href) in markdown_links(text) {
        let name = label.trim();
        let Some(id) = page_link(href) else { continue };
        if !name.is_empty() && !starts_with_scheme(name) && !labels.iter().any(|(seen, _)| *seen == id) {
            labels.push((id, name.to_string()));
        }
    }
    let mut found: Vec<PageRef> = Vec::new();
    for link in urls_in(text) {
        let Some(page_id) = page_link(link.trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_'])) else {
            continue;
        };
        let label = labels
            .iter()
            .find(|(id, _)| *id == page_id)
            .map(|(_, name)| name.clone());
        match found.iter_mut().find(|page| page.page_id == page_id) {
            Some(seen) => {
                if seen.label.is_none() {
                    seen.label = label;
                }
            }
            None => found.push(PageRef { page_id, label }),
        }
    }
    found
}

/// The page a finished call published, or None: a failed call publishes nothing.
pub fn written_page(tool: &str, output: &str, is_error: bool) -> Option<PageRef> {
    if is_error {
        return None;
    }
    page_in_answer(tool, output)
}

/// The page a page tool's answer names (`{ page: { id, title }, version }`), or None for any other tool.
pub fn page_in_answer(tool: &str, output: &str) -> Option<PageRef> {
    if !PAGE_TOOLS.contains(&tool) {
        return None;
    }
    let page = answer(output)?.get("page")?.clone();
    let id = page.get("id")?.as_str()?;
    if !is_page_id(id) {
        return None;
    }
    let label = page
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string);
    Some(PageRef {
        page_id: id.to_string(),
        label,
    })
}

/// The pages a run of calls (tool, output, failed) published, in order, each once: a later version of the same page
/// keeps one card, with its latest title.
pub fn pages_written<'a>(calls: impl IntoIterator<Item = (&'a str, &'a str, bool)>) -> Vec<PageRef> {
    let mut found: Vec<PageRef> = Vec::new();
    for (tool, output, is_error) in calls {
        let Some(page) = written_page(tool, output, is_error) else {
            continue;
        };
        match found.iter_mut().find(|item| item.page_id == page.page_id) {
            Some(item) => *item = page,
            None => found.push(page),
        }
    }
    found
}

/// The link on the CLI's `View it:` line, which `nexus pages publish` and `update` print (`^\s*View it:\s*(\S+)`,
/// multiline).
pub fn view_it_link(output: &str) -> Option<&str> {
    let mut starts = std::iter::once(0).chain(output.match_indices(['\n', '\r']).map(|(ix, _)| ix + 1));
    starts.find_map(|start| {
        let rest = output[start..].trim_start().strip_prefix("View it:")?.trim_start();
        let link = rest.split(char::is_whitespace).next().unwrap_or("");
        (!link.is_empty()).then_some(link)
    })
}

/// The page a tool's answer says it wrote: a page tool's answer, else the page on a `View it:` line.
pub fn page_in_result(tool: &str, output: &str) -> Option<PageRef> {
    page_in_answer(tool, output).or_else(|| {
        let link = view_it_link(output)?;
        page_links_in(link).into_iter().next()
    })
}

/// The tool's answer as a JSON object: the text itself, or the text of its first block when the agent kept the
/// protocol's content blocks.
fn answer(output: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(output.trim()).ok()? {
        Value::Array(blocks) => {
            let text = blocks.iter().find_map(|block| block.get("text")?.as_str())?;
            match serde_json::from_str::<Value>(text.trim()).ok()? {
                Value::Object(map) => Some(map),
                _ => None,
            }
        }
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn starts_with_scheme(text: &str) -> bool {
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The length of the `http://` or `https://` (any case) `text` starts with, or None.
fn scheme_len(text: &str) -> Option<usize> {
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    if lower.starts_with("https://") {
        Some(8)
    } else if lower.starts_with("http://") {
        Some(7)
    } else {
        None
    }
}

/// The web addresses in `text` (`\bhttps?://[^\s<>()[\]{}"'`]+`, any case).
fn urls_in(text: &str) -> Vec<&str> {
    let stop = |c: char| c.is_whitespace() || "<>()[]{}\"'`".contains(c);
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find(['h', 'H']) {
        let start = at + offset;
        let boundary = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let Some(scheme) = scheme_len(&text[start..]).filter(|_| boundary) else {
            at = start + 1;
            continue;
        };
        let rest = &text[start + scheme..];
        let len = rest.find(stop).unwrap_or(rest.len());
        if len == 0 {
            at = start + 1;
            continue;
        }
        found.push(&text[start..start + scheme + len]);
        at = start + scheme + len;
    }
    found
}

/// The Markdown links in `text`, as (label, address) (`\[([^\]\n]+)\]\((https?://[^\s)]+)\)`, any case).
fn markdown_links(text: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find('[') {
        let open = at + offset;
        at = open + 1;
        let after = &text[open + 1..];
        let Some(close) = after.find([']', '\n']) else { continue };
        if close == 0 || !after[close..].starts_with("](") {
            continue;
        }
        let target = &after[close + 2..];
        let Some(scheme) = scheme_len(target) else { continue };
        let len = target[scheme..]
            .find(|c: char| c.is_whitespace() || c == ')')
            .unwrap_or(target.len() - scheme);
        if len == 0 || !target[scheme + len..].starts_with(')') {
            continue;
        }
        found.push((&after[..close], &target[..scheme + len]));
        at = open + 1 + close + 2 + scheme + len + 1;
    }
    found
}

/// `decodeURIComponent`: None for a malformed escape or bytes that are not UTF-8.
fn decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%' {
            let hex = segment.get(ix + 1..ix + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            ix += 3;
        } else {
            out.push(bytes[ix]);
            ix += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn written(id: &str, title: &str) -> String {
        serde_json::to_string_pretty(
            &json!({ "page": { "id": id, "title": title, "format": "html", "latestVersion": 1 }, "version": { "version": 1 } }),
        )
        .unwrap_or_default()
    }

    fn page(id: &str, label: Option<&str>) -> PageRef {
        PageRef {
            page_id: id.into(),
            label: label.map(str::to_string),
        }
    }

    #[test]
    fn a_page_link_is_the_viewer_link_nexus_pages_publish_prints_at_any_gateway() {
        assert_eq!(
            page_link("https://nexus.example.com/app/acme/pages/pag_01JX9").as_deref(),
            Some("pag_01JX9")
        );
        assert_eq!(
            page_link("http://localhost:3000/app/acme/pages/pag_1/").as_deref(),
            Some("pag_1")
        );
        assert_eq!(
            page_link("https://gw.example.com/app/acme%20co/pages/pag_1?v=3").as_deref(),
            Some("pag_1")
        );
        // Before an organization is saved, the CLI links through the active one.
        assert_eq!(
            page_link("https://gw.example.com/app?next=%2Fpages%2Fpag_2").as_deref(),
            Some("pag_2")
        );
    }

    #[test]
    fn a_page_link_is_no_other_link() {
        for href in [
            "https://example.com/",
            "https://docs.example.com/app/acme/pages/about",
            "https://gw.example.com/app/acme/pages",
            "https://gw.example.com/app/acme/pages/pag_1/versions",
            "https://gw.example.com/app/acme/artifacts/art_1",
            "https://gw.example.com/app?next=%2Fspend",
            "https://user:pw@gw.example.com/app/acme/pages/pag_1",
            "ftp://gw.example.com/app/acme/pages/pag_1",
            "not a link",
        ] {
            assert_eq!(page_link(href), None, "{href}");
        }
    }

    #[test]
    fn an_answer_links_the_view_it_page_once_with_a_markdown_links_text_as_its_title() {
        let text = [
            "Published the chart.",
            "→ View it: https://gw.example.com/app/acme/pages/pag_1.",
            "Open [Q3 spend by team](https://gw.example.com/app/acme/pages/pag_1) or the [other one](https://gw.example.com/app/acme/pages/pag_2).",
        ]
        .join("\n");
        assert_eq!(
            page_links_in(&text),
            vec![
                page("pag_1", Some("Q3 spend by team")),
                page("pag_2", Some("other one"))
            ]
        );
    }

    #[test]
    fn an_answer_whose_links_are_not_pages_links_none() {
        assert_eq!(
            page_links_in(
                "See https://example.com/docs and http://localhost:5173/ and [the PR](https://github.com/o/r/pull/1)."
            ),
            vec![]
        );
        assert_eq!(page_links_in("No links here."), vec![]);
    }

    #[test]
    fn a_bare_link_is_not_a_title() {
        assert_eq!(
            page_links_in("[https://gw.example.com/app/acme/pages/pag_1](https://gw.example.com/app/acme/pages/pag_1)"),
            vec![page("pag_1", None)]
        );
    }

    #[test]
    fn create_page_and_write_page_version_answers_name_the_page_from_claude_code_or_codex() {
        assert_eq!(
            written_page("mcp__nexus__createPage", &written("pag_1", "Q3 spend"), false),
            Some(page("pag_1", Some("Q3 spend")))
        );
        assert_eq!(
            written_page("mcp__nexus__writePageVersion", &written("pag_1", "Q3 spend, v2"), false),
            Some(page("pag_1", Some("Q3 spend, v2")))
        );
        // An answer kept as the protocol's content blocks.
        let blocks = json!([{ "type": "text", "text": written("pag_3", "Plan") }]).to_string();
        assert_eq!(
            written_page("mcp__nexus__createPage", &blocks, false),
            Some(page("pag_3", Some("Plan")))
        );
    }

    #[test]
    fn a_failed_call_another_tool_or_another_server_names_no_page() {
        assert_eq!(
            written_page("mcp__nexus__createPage", &written("pag_1", "x"), true),
            None
        );
        assert_eq!(
            written_page("mcp__nexus__createPage", "Gateway refused: 403 forbidden", false),
            None
        );
        assert_eq!(
            written_page("mcp__nexus__listTeams", &written("pag_1", "x"), false),
            None
        );
        assert_eq!(
            written_page("mcp__other__createPage", &written("pag_1", "x"), false),
            None
        );
        assert_eq!(
            written_page("Bash", "→ View it: https://gw.example.com/app/acme/pages/pag_1", false),
            None
        );
        assert_eq!(
            written_page("mcp__nexus__createPage", &written("../etc", "x"), false),
            None
        );
    }

    #[test]
    fn a_run_keeps_one_card_per_page_with_its_latest_title() {
        let (draft, last, other) = (
            written("pag_1", "Draft"),
            written("pag_1", "Final"),
            written("pag_2", "Other"),
        );
        let calls = [
            ("mcp__nexus__createPage", draft.as_str(), false),
            ("Bash", "ok", false),
            ("mcp__nexus__writePageVersion", last.as_str(), false),
            ("mcp__nexus__createPage", other.as_str(), false),
        ];
        assert_eq!(
            pages_written(calls),
            vec![page("pag_1", Some("Final")), page("pag_2", Some("Other"))]
        );
    }

    #[test]
    fn the_cli_view_it_line_names_the_page_it_wrote() {
        let printed = "Published.\n  View it: https://gw.example.com/app/acme/pages/pag_7.\nDone";
        assert_eq!(
            view_it_link(printed),
            Some("https://gw.example.com/app/acme/pages/pag_7.")
        );
        assert_eq!(page_in_result("Bash", printed), Some(page("pag_7", None)));
        // Only at a line's start, as `^\s*View it:`.
        assert_eq!(
            view_it_link("→ View it: https://gw.example.com/app/acme/pages/pag_7"),
            None
        );
        assert_eq!(page_in_result("Bash", "View it: https://example.com/"), None);
        // A page tool's answer comes first.
        assert_eq!(
            page_in_result("mcp__nexus__createPage", &written("pag_1", "Plan")),
            Some(page("pag_1", Some("Plan")))
        );
    }
}
