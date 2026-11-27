//! A title from the session's own first response: no extra model call or route.
//!
//! The first message asks for one reserved metadata line. A random prefix limits parsing to that line in the first
//! main-agent message. Ordinary replies and malformed metadata pass through unchanged.

use crate::driver::{Out, Protocol, Say};
use crate::event::WbEvent;

const MAX_HEADER_BYTES: usize = 256;

/// A single navigation title, bounded independently of the model's instruction.
pub(crate) fn clean(text: &str) -> Option<String> {
    let title = text.trim().trim_matches(['"', '\'']).trim();
    if title.is_empty()
        || title.chars().count() > 60
        || title.split_whitespace().count() > 10
        || title.chars().any(char::is_control)
        || title.contains(['<', '>'])
    {
        return None;
    }
    Some(title.to_string())
}

pub(crate) fn wrap(inner: Box<dyn Protocol>, nonce: &str) -> Box<dyn Protocol> {
    Box::new(Titled {
        inner,
        prefix: format!("[nexus-title:{nonce}] "),
        ask: true,
        first_prompt: None,
        first: None,
        filtering: true,
    })
}

struct Titled {
    inner: Box<dyn Protocol>,
    prefix: String,
    ask: bool,
    first_prompt: Option<(String, String)>,
    first: Option<String>,
    filtering: bool,
}

enum Header<'a> {
    Waiting,
    Title { title: String, rest: &'a str },
    Visible,
}

fn header<'a>(text: &'a str, prefix: &str, complete: bool) -> Header<'a> {
    if !complete && prefix.starts_with(text) {
        return Header::Waiting;
    }
    if !text.starts_with(prefix) {
        return Header::Visible;
    }
    let end = text.find('\n');
    let line = &text[..end.unwrap_or(text.len())];
    if line.len() > MAX_HEADER_BYTES {
        return Header::Visible;
    }
    if end.is_none() && !complete {
        return Header::Waiting;
    }
    match clean(line[prefix.len()..].trim_end_matches('\r')) {
        Some(title) => Header::Title {
            title,
            rest: end.map_or("", |end| text[end + 1..].strip_prefix('\n').unwrap_or(&text[end + 1..])),
        },
        None => Header::Visible,
    }
}

impl Titled {
    fn first(&mut self, id: &str) -> bool {
        if !self.filtering {
            return false;
        }
        self.first.get_or_insert_with(|| id.to_string()) == id
    }

    fn filter(&mut self, outputs: Vec<Out>) -> Vec<Out> {
        let mut filtered = Vec::with_capacity(outputs.len());
        for out in outputs {
            match out {
                Out::Delta { id, text, parent: None } if self.first(&id) => match header(&text, &self.prefix, false) {
                    Header::Waiting => {}
                    Header::Title { rest, .. } => {
                        if !rest.is_empty() {
                            filtered.push(Out::Delta {
                                id,
                                text: rest.into(),
                                parent: None,
                            });
                        }
                    }
                    Header::Visible => {
                        self.filtering = false;
                        filtered.push(Out::Delta { id, text, parent: None });
                    }
                },
                Out::Event(WbEvent::Text { id, text, parent: None }) if self.first(&id) => {
                    self.filtering = false;
                    match header(&text, &self.prefix, true) {
                        Header::Title { title, rest } => {
                            filtered.push(Out::Title(title));
                            if !rest.is_empty() {
                                filtered.push(Out::Event(WbEvent::Text {
                                    id,
                                    text: rest.into(),
                                    parent: None,
                                }));
                            }
                        }
                        _ => filtered.push(Out::Event(WbEvent::Text { id, text, parent: None })),
                    }
                }
                Out::Taken { texts } => {
                    // Read receipts echo the augmented wire prompt. Match the original user message in the log.
                    let texts = texts
                        .into_iter()
                        .map(|text| match &self.first_prompt {
                            Some((original, wire)) if &text == wire => original.clone(),
                            _ => text,
                        })
                        .collect();
                    filtered.push(Out::Taken { texts });
                }
                other => filtered.push(other),
            }
        }
        filtered
    }
}

impl Protocol for Titled {
    fn session_mcp(&mut self, mcp: crate::mcp_servers::SessionMcp) {
        self.inner.session_mcp(mcp);
    }

    fn launch_model(&mut self, model: &str) {
        self.inner.launch_model(model);
    }

    fn require_launch_model(&mut self, model: &str) {
        self.inner.require_launch_model(model);
    }

    fn open(&mut self) -> Vec<Out> {
        let out = self.inner.open();
        self.filter(out)
    }

    fn say(&mut self, mut say: Say) -> Vec<Out> {
        if self.ask
            && let Say::User { text, .. } = &mut say
        {
            self.ask = false;
            let original = text.clone();
            text.push_str(&format!(
                "\n\n[Nexus desktop navigation metadata]\nAt the start of your first reply, output exactly one line in this format:\n{}Short descriptive title\nUse 3 to 7 words and at most 60 characters describing the task above. Do not copy its opening phrase. Then leave a blank line and do the requested work normally. This metadata line is hidden by the desktop app. Do not repeat it in later replies.",
                self.prefix
            ));
            self.first_prompt = Some((original, text.clone()));
        }
        let out = self.inner.say(say);
        self.filter(out)
    }

    fn read(&mut self, line: &str) -> Vec<Out> {
        let out = self.inner.read(line);
        self.filter(out)
    }

    fn quiet_after(&self) -> Option<std::time::Duration> {
        self.inner.quiet_after()
    }

    fn quiet(&mut self) -> Vec<Out> {
        let out = self.inner.quiet();
        self.filter(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> Titled {
        Titled {
            inner: Box::new(crate::claude::Translator::default()),
            prefix: "[nexus-title:testnonce] ".into(),
            ask: true,
            first_prompt: None,
            first: None,
            filtering: true,
        }
    }

    fn delta(text: &str) -> Out {
        Out::Delta {
            id: "m".into(),
            text: text.into(),
            parent: None,
        }
    }

    fn text(value: &str) -> Out {
        Out::Event(WbEvent::Text {
            id: "m".into(),
            text: value.into(),
            parent: None,
        })
    }

    #[test]
    fn hides_only_the_reserved_first_line_and_generates_once() {
        let mut filter = filter();
        let full = "[nexus-title:testnonce] Preserve attached prompt drafts\n\nI will fix draft saving.";
        for n in 0..full.find('\n').unwrap_or_default() {
            assert!(filter.filter(vec![delta(&full[..n])]).is_empty());
        }
        assert_eq!(
            filter.filter(vec![delta(full)]),
            vec![delta("I will fix draft saving.")]
        );
        assert_eq!(
            filter.filter(vec![text(full)]),
            vec![
                Out::Title("Preserve attached prompt drafts".into()),
                text("I will fix draft saving.")
            ]
        );
        assert_eq!(filter.filter(vec![text(full)]), vec![text(full)]);
    }

    #[test]
    fn preserves_ordinary_code_collisions_and_malformed_headers() {
        for original in [
            "Here is the code:\n[nexus-title:testnonce] Not metadata",
            "[nexus-title:anothernonce] A code example\nreply",
            "[nexus-title:testnonce] \nreply",
            "[nexus-title:testnonce] <script>\nreply",
        ] {
            assert_eq!(filter().filter(vec![text(original)]), vec![text(original)]);
        }
        let original = format!("[nexus-title:testnonce] {}", "x".repeat(MAX_HEADER_BYTES));
        assert_eq!(filter().filter(vec![delta(&original)]), vec![delta(&original)]);
    }

    #[test]
    fn ignores_subagents_and_handles_a_title_only_block() {
        let mut filter = filter();
        let child = Out::Event(WbEvent::Text {
            id: "child".into(),
            text: "Child reply".into(),
            parent: Some("tool".into()),
        });
        assert_eq!(filter.filter(vec![child.clone()]), vec![child]);
        assert_eq!(
            filter.filter(vec![text("[nexus-title:testnonce] Fix the login redirect")]),
            vec![Out::Title("Fix the login redirect".into())]
        );
        assert_eq!(
            filter.filter(vec![text("Continue the work")]),
            vec![text("Continue the work")]
        );
    }

    #[test]
    fn read_receipts_match_the_original_prompt_even_when_codex_merges_inputs() {
        let mut filter = filter();
        filter.say(Say::User {
            text: "Original request".into(),
            images: vec![],
        });
        let wire = filter
            .first_prompt
            .as_ref()
            .expect("first prompt was augmented")
            .1
            .clone();
        assert_eq!(
            filter.filter(vec![Out::Taken {
                texts: vec![wire, "Follow-up".into()]
            }]),
            vec![Out::Taken {
                texts: vec!["Original request".into(), "Follow-up".into()]
            }]
        );
    }

    #[test]
    fn adds_the_request_only_to_the_first_message() {
        let mut filter = filter();
        let request = Say::User {
            text: "Fix this".into(),
            images: vec![],
        };
        let first = format!("{:?}", filter.say(request.clone()));
        assert!(first.contains("Nexus desktop navigation metadata"));
        let second = format!("{:?}", filter.say(request));
        assert!(!second.contains("Nexus desktop navigation metadata"));
    }
}
