//! Nexus UI's docs, read as the Markdown it serves for agents: the index at `/llms.txt`, by section, and each
//! page at `/docs/<path>.md`. A page is named by its path under `/docs`: `cli/chat` for `/docs/cli/chat`, empty for
//! `/docs` itself. The view is `workspace/docs_view.rs`.

/// A section of the docs' navigation, as `/llms.txt` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub pages: Vec<DocLink>,
}

/// A page in the navigation.
#[derive(Debug, Clone, PartialEq)]
pub struct DocLink {
    pub title: String,
    pub path: String,
}

/// The sections of `/llms.txt`: each `## Section` heading and its `- [Title](/docs/x.md): description` lines. The
/// blog and anything outside `/docs` are left out.
pub fn sections(index: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in index.lines() {
        let line = line.trim();
        if let Some(title) = line.strip_prefix("## ") {
            sections.push(Section {
                title: title.trim().to_string(),
                pages: Vec::new(),
            });
            continue;
        }
        let Some(rest) = line.strip_prefix("- [") else {
            continue;
        };
        let Some((title, rest)) = rest.split_once("](") else {
            continue;
        };
        let Some((href, _)) = rest.split_once(')') else {
            continue;
        };
        if let (Some(section), Some(path)) = (sections.last_mut(), page_path(href)) {
            section.pages.push(DocLink {
                title: title.to_string(),
                path,
            });
        }
    }
    sections.retain(|section| !section.pages.is_empty());
    sections
}

/// The page a link leads to, when it is a docs page: `/docs/cli/chat.md`, `/docs/cli/chat#flags` and the full
/// address all name `cli/chat`. `None` for anything else.
pub fn page_path(href: &str) -> Option<String> {
    let href = href.split(['#', '?']).next().unwrap_or("");
    let path = match href.find("://") {
        Some(scheme) => {
            let after = &href[scheme + 3..];
            &after[after.find('/')?..]
        }
        None => href,
    };
    let path = path.strip_suffix(".md").unwrap_or(path);
    let rest = path.strip_prefix("/docs")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    Some(rest.strip_prefix('/')?.trim_end_matches('/').to_string())
}

/// Where a page's Markdown is: `/docs.md` for the first page, `/docs/<path>.md` for the rest.
pub fn markdown_path(path: &str) -> String {
    if path.is_empty() {
        "/docs.md".into()
    } else {
        format!("/docs/{path}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_lists_the_docs_by_section() {
        let index = "# Nexus\n\n> About.\n\n## Start\n\n- [Overview](/docs.md): What it is.\n- [Quickstart](/docs/quickstart.md)\n\n## Blog\n\n- [A post](/blog/a.md): Words.\n";
        let sections = sections(index);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Start");
        assert_eq!(sections[0].pages[0].path, "");
        assert_eq!(sections[0].pages[1].path, "quickstart");
    }

    #[test]
    fn docs_links_name_their_page() {
        assert_eq!(page_path("/docs/cli/chat#flags").as_deref(), Some("cli/chat"));
        assert_eq!(
            page_path("https://nexus.example/docs/api/keys.md").as_deref(),
            Some("api/keys")
        );
        assert_eq!(page_path("/docs").as_deref(), Some(""));
        assert_eq!(page_path("/docsy"), None);
        assert_eq!(page_path("/app/acme/keys"), None);
        assert_eq!(markdown_path("cli/chat"), "/docs/cli/chat.md");
    }
}
