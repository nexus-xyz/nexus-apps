//! What the call says and keeps, as the web kit words it: an answer's Markdown as sentences to say, the
//! echo filter, and the end-of-call card.

use markdown::mdast::Node;
use nexus_desktop_core::agent::Message;

use super::call::{VoiceRecord, VoiceReply, VoiceTurn, Who};

/// The inline text of `nodes`: a link by its label, an image by its alt text, code and maths as written.
fn inline(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text(text) => text.value.clone(),
            Node::InlineCode(code) => code.value.clone(),
            Node::InlineMath(math) => math.value.clone(),
            Node::Image(image) => image.alt.clone(),
            Node::ImageReference(image) => image.alt.clone(),
            Node::Break(_) => " ".into(),
            Node::Strong(_) | Node::Emphasis(_) | Node::Delete(_) | Node::Link(_) | Node::LinkReference(_) => {
                inline(node.children().map(Vec::as_slice).unwrap_or_default())
            }
            // A citation's mark, raw HTML and the rest are not read out.
            _ => String::new(),
        })
        .collect()
}

/// A sentence ends with its mark: one is added when it has none.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.ends_with(['.', '!', '?', ':', ';', '…']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// A list item's sentences: its paragraphs, then its own list.
fn list(items: &[Node]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            Node::ListItem(item) => Some(blocks(&item.children)),
            _ => None,
        })
        .flatten()
        .collect()
}

fn blocks(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .flat_map(|node| match node {
            Node::Paragraph(block) => vec![sentence(&inline(&block.children))],
            Node::Heading(block) => vec![sentence(&inline(&block.children))],
            Node::Blockquote(block) => blocks(&block.children),
            Node::List(block) => list(&block.children),
            Node::Code(_) => vec!["The code is in the thread.".into()],
            Node::Math(_) => vec!["The formula is in the thread.".into()],
            Node::Table(_) => vec!["The table is in the thread.".into()],
            _ => Vec::new(),
        })
        .filter(|part| !part.is_empty())
        .collect()
}

/// An answer's Markdown as words to say: prose and list items as sentences, a link by its label. Code, maths and tables
/// are not read out; the voice says where they are instead.
pub fn spoken_text(source: &str) -> String {
    let options = markdown::ParseOptions {
        constructs: markdown::Constructs {
            math_flow: true,
            math_text: true,
            ..markdown::Constructs::gfm()
        },
        ..markdown::ParseOptions::gfm()
    };
    match markdown::to_mdast(source, &options) {
        Ok(Node::Root(root)) => blocks(&root.children).join(" "),
        // Plain Markdown never fails to parse; if it does, the words as written.
        _ => source.trim().to_string(),
    }
}

/// The words Nexus says for a reply: its answers, a refusal's reason, and, for a write it asks for, that it waits for
/// the person's approval in the thread. Voice never approves a write.
pub fn spoken_reply(messages: Vec<Message>) -> VoiceReply {
    let mut parts = Vec::new();
    let mut needs_you = false;
    for message in &messages {
        match message {
            Message::Answer { text, .. } => parts.push(spoken_text(text)),
            Message::Refused { detail, .. } => parts.push(detail.clone()),
            Message::PendingAction { write, .. } => {
                needs_you = true;
                parts.push(format!(
                    "{} That needs your approval in the thread.",
                    sentence(&write.summary)
                ));
            }
            _ => {}
        }
    }
    let say = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if say.is_empty() && !needs_you {
        return VoiceReply::unanswered();
    }
    VoiceReply::Said {
        say,
        needs_you,
        messages,
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|word| !word.is_empty())
        .map(String::from)
        .collect()
}

/// Whether words heard while Nexus speaks are only Nexus's own voice coming back through the microphone: most of them
/// are in what it says. Two words or fewer count as echo too, so a cough or an "mm" does not stop Nexus.
pub fn is_echo(heard: &str, saying: &str) -> bool {
    let said = tokens(heard);
    if said.len() <= 2 {
        return true;
    }
    let voice: std::collections::HashSet<String> = tokens(saying).into_iter().collect();
    let ours = said.iter().filter(|word| voice.contains(*word)).count();
    ours as f32 / said.len() as f32 >= 0.6
}

/// The end-of-call card: how long, what Nexus did, and the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSummary {
    /// "4 min", or "40 s" for a call under a minute.
    pub length: String,
    /// What Nexus did or asked for, one line each; on a call that only asked, what was asked.
    pub done: Vec<String>,
    /// Whether `done` names things Nexus did or asked for, not only questions.
    pub acted: bool,
    /// The line under the mark once the call ends (Ended): "4 min · 2 things to check".
    pub line: String,
    pub turns: Vec<VoiceTurn>,
    /// Epoch ms.
    pub started_at: i64,
}

/// How long a call took, as the card says it.
pub fn call_length(ms: i64) -> String {
    let seconds = ((ms as f64) / 1000.).round().max(0.) as i64;
    if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{} min", ((seconds as f64) / 60.).round() as i64)
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let head: String = text.chars().take(max - 1).collect();
        format!("{}…", head.trim_end())
    } else {
        text.to_string()
    }
}

fn trim_end_marks(text: &str) -> &str {
    text.trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

/// The card for a call that ended. A write the agent asked for waits in the thread; a refusal says what did not happen;
/// a call that only asked says what was asked. `None`: nothing was said, so nothing lands in the thread.
pub fn summarize(record: &VoiceRecord) -> Option<VoiceSummary> {
    if record.turns.is_empty() {
        return None;
    }
    let mut done = Vec::new();
    for message in &record.messages {
        match message {
            Message::PendingAction { write, .. } => {
                done.push(format!("{}: waits for your approval", trim_end_marks(&write.summary)))
            }
            Message::Refused { detail, .. } => done.push(format!("Not done: {}", clip(trim_end_marks(detail), 80))),
            _ => {}
        }
    }
    let asked: Vec<String> = record
        .turns
        .iter()
        .filter(|turn| turn.who == Who::You)
        .map(|turn| format!("Asked: {}", clip(&turn.text, 70)))
        .collect();
    let length = call_length(record.ended_at - record.started_at);
    let count = if done.is_empty() {
        format!(
            "{} {}",
            asked.len(),
            if asked.len() == 1 { "question" } else { "questions" }
        )
    } else {
        format!(
            "{} {} to check",
            done.len(),
            if done.len() == 1 { "thing" } else { "things" }
        )
    };
    let acted = !done.is_empty();
    let lines = if acted {
        done
    } else {
        asked.into_iter().take(3).collect()
    };
    Some(VoiceSummary {
        line: format!("{length} · {count}"),
        length,
        done: lines,
        acted,
        turns: record.turns.clone(),
        started_at: record.started_at,
    })
}

#[cfg(test)]
pub(crate) use tests::{answer, write};

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_desktop_core::agent::Write;

    pub(crate) fn answer(text: &str) -> Message {
        serde_json::from_value(serde_json::json!({ "kind": "answer", "text": text, "model": "m" })).expect("an answer")
    }

    pub(crate) fn write(summary: &str) -> Message {
        Message::PendingAction {
            write: Write {
                connector: "gmail".into(),
                operation: "send".into(),
                summary: summary.into(),
                effect: "write".into(),
                arguments: serde_json::Value::Null,
            },
            approve_url: "https://nexus.example/approve".into(),
            detail: String::new(),
            card: "c1".into(),
            asks: None,
            expires_at: None,
            in_chat: None,
            edit: None,
            edited: false,
        }
    }

    fn refused(detail: &str) -> Message {
        Message::Refused {
            detail: detail.into(),
            billing_url: None,
        }
    }

    #[test]
    fn markdown_is_read_as_sentences_and_says_where_code_and_tables_are() {
        assert_eq!(
            spoken_text("**Booked.** See [the place](https://x.example).\n\n- Ana said yes\n- Sam has not"),
            "Booked. See the place. Ana said yes. Sam has not."
        );
        assert_eq!(
            spoken_text("Run this:\n\n```sh\nls\n```"),
            "Run this: The code is in the thread."
        );
        assert_eq!(spoken_text("| a |\n| - |\n| 1 |"), "The table is in the thread.");
    }

    #[test]
    fn a_write_waits_for_approval_in_the_thread_and_is_never_approved() {
        let VoiceReply::Said { say, needs_you, .. } = spoken_reply(vec![write("Text Ana, Dev and Sam")]) else {
            panic!("a reply")
        };
        assert!(needs_you);
        assert_eq!(say, "Text Ana, Dev and Sam. That needs your approval in the thread.");
        assert_eq!(spoken_reply(vec![]), VoiceReply::unanswered());
    }

    #[test]
    fn echo_is_told_from_the_person_talking_over_nexus() {
        assert!(is_echo("mm", "anything"));
        assert!(is_echo(
            "about sixty four dollars",
            "The Uber is about sixty four dollars"
        ));
        assert!(!is_echo("no make it Friday", "The Uber is about sixty four dollars"));
    }

    fn turn(who: Who, text: &str) -> VoiceTurn {
        VoiceTurn {
            who,
            text: text.into(),
            at: 1_000,
        }
    }

    #[test]
    fn the_card_lists_what_waits_for_the_person_and_the_calls_length() {
        let summary = summarize(&VoiceRecord {
            started_at: 1_000,
            ended_at: 1_000 + 4 * 60_000,
            turns: vec![
                turn(Who::You, "Tell Ana about Friday."),
                turn(Who::Nexus, "Here's the text. Send it?"),
            ],
            messages: vec![
                write("Send the dinner text to Ana."),
                refused("Booking needs Face ID on your phone."),
            ],
        })
        .expect("a card");
        assert_eq!(summary.length, "4 min");
        assert_eq!(
            summary.done,
            [
                "Send the dinner text to Ana: waits for your approval",
                "Not done: Booking needs Face ID on your phone"
            ]
        );
        assert_eq!(summary.line, "4 min · 2 things to check");
        assert!(summary.acted);
        assert_eq!(summary.turns.len(), 2);
    }

    #[test]
    fn a_call_that_only_asked_says_what_was_asked() {
        let summary = summarize(&VoiceRecord {
            started_at: 1_000,
            ended_at: 41_000,
            turns: vec![turn(Who::You, "What's happening with dinner?")],
            messages: vec![answer("Booked.")],
        })
        .expect("a card");
        assert_eq!(summary.length, "40 s");
        assert!(!summary.acted);
        assert_eq!(summary.done, ["Asked: What's happening with dinner?"]);
        assert_eq!(summary.line, "40 s · 1 question");
    }

    #[test]
    fn a_call_where_nothing_was_said_lands_nothing() {
        let record = VoiceRecord {
            started_at: 1_000,
            ended_at: 6_000,
            turns: vec![],
            messages: vec![],
        };
        assert_eq!(summarize(&record), None);
    }
}
