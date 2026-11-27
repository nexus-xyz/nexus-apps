//! Send feedback, as the Nexus app's (Nexus UI's `FeedbackProvider` in `components/nexus-ui/feedback.tsx`):
//! the tray's Send Feedback… and, on macOS, Help › Send Feedback… open the dialog (`workspace/feedback_dialog.rs`).
//! It posts to `POST /v1/diagnostics/feedback`, which hands it to the Nexus team with the person's
//! name, email and organization. The body is Nexus UI's: the kind, the message (trimmed, at most [`MAX_CHARS`]), the
//! screen as its route's shape ([`screen_of`]), and this build's version, OS and OS version. The Gateway call carries
//! `x-nexus-client: desktop/<version>` (`gateway.rs`). This module holds the words and the body; the dialog sends.

use serde_json::{Value, json};

/// The longest message, in characters (`MAX_FEEDBACK_CHARS`, the Gateway's bound).
pub const MAX_CHARS: usize = 4_000;
/// The counter shows once the message nears its bound.
pub const COUNT_FROM: usize = 3_500;
const MAX_SCREEN_CHARS: usize = 120;
const MAX_VERSION_CHARS: usize = 40;

/// What the feedback is about: Nexus UI's three choices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Bug,
    Idea,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Bug, Kind::Idea, Kind::Other];

    /// The API's category.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Bug => "bug",
            Kind::Idea => "idea",
            Kind::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Bug => "Bug",
            Kind::Idea => "Idea",
            Kind::Other => "Other",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Kind::Bug => "What happened, and what did you expect?",
            Kind::Idea => "What would make Nexus better for you?",
            Kind::Other => "What's on your mind?",
        }
    }
}

pub const TITLE: &str = "Send feedback";
pub const NOTE: &str = "Tell the Nexus team what broke, what you would like, or anything else.";
pub const HINT: &str = "The team sees your name, email and organization, the app version, and the page you are on.";
pub const SENT_TITLE: &str = "Thanks, it is sent";

/// The note after a send, naming the address the team writes back to when the app knows it.
pub fn sent_note(email: Option<&str>) -> String {
    match email.filter(|email| !email.is_empty()) {
        Some(email) => format!("The Nexus team reads every message. If we need more, we will write to {email}."),
        None => "The Nexus team reads every message.".into(),
    }
}

/// The counter under the text, `3,500 of 4,000`, once the message has [`COUNT_FROM`] characters; else none.
pub fn count(chars: usize) -> Option<String> {
    (chars >= COUNT_FROM).then(|| format!("{} of {}", thousands(chars), thousands(MAX_CHARS)))
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `text` cut to [`MAX_CHARS`] characters, as the text box's `maxLength`. `None` when it already fits.
pub fn bounded(text: &str) -> Option<String> {
    (text.chars().count() > MAX_CHARS).then(|| text.chars().take(MAX_CHARS).collect())
}

/// The words for a failed send: the Gateway's 503 is a channel no one set up; anything else is worth another try.
/// `what` names the channel (`Feedback and crash reports`, `Crash reports`), as Nexus UI's `failure`.
pub fn failure(error: &str, what: &str) -> String {
    if error.contains("not configured") {
        format!("{what} {UNSET}")
    } else {
        "It did not send. Nothing is lost: try again.".into()
    }
}

/// How [`failure`] ends for a channel no one set up on this Gateway.
const UNSET: &str = "go nowhere on this Gateway yet.";

/// Whether `problem` ([`failure`]'s words) says no one set the channel up: nobody in the app can fix it, so the dialog
/// says it quietly, not in red.
pub fn unset(problem: &str) -> bool {
    problem.ends_with(UNSET)
}

/// A part of the address kept as it is: a word or words joined by hyphens (`sessions`, `acme-robotics`).
fn route_word(part: &str) -> bool {
    part.len() <= 32
        && !part.is_empty()
        && part
            .split('-')
            .all(|word| !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()))
}

/// The screen as its route's shape (`/acme/sessions/:id`), as Nexus UI's `screenOf`: `/app` is left out, and every
/// part that is not a plain word, such as an id, is `:id`. Never a chat's title, which is content.
pub fn screen_of(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let path = match path.strip_prefix("/app") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => path,
    };
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| if route_word(part) { part } else { ":id" })
        .collect();
    format!("/{}", parts.join("/")).chars().take(MAX_SCREEN_CHARS).collect()
}

/// This build's version and build, as the Nexus app splits `0.21.0+abc1234`.
pub fn app_version() -> (String, Option<String>) {
    let mut parts = crate::channel::VERSION.splitn(2, '+');
    let version = crate::redact::plain(parts.next().unwrap_or_default(), MAX_VERSION_CHARS);
    let build = parts
        .next()
        .map(|build| crate::redact::plain(build, MAX_VERSION_CHARS))
        .filter(|build| !build.is_empty());
    (if version.is_empty() { "dev".into() } else { version }, build)
}

/// What `POST /v1/diagnostics/feedback` takes, as Nexus UI's `FeedbackForm` sends it.
pub fn body(kind: Kind, message: &str, screen: Option<String>, os_version: &str) -> Value {
    let (version, build) = app_version();
    json!({
        "category": kind.id(),
        "message": message.trim().chars().take(MAX_CHARS).collect::<String>(),
        "screen": screen,
        "appVersion": version,
        "appBuild": build,
        "platform": crate::crash::platform(),
        "osVersion": os_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_is_the_routes_shape_and_never_its_ids() {
        assert_eq!(
            screen_of("/app/acme-robotics/sessions/ses_01JX9"),
            "/acme-robotics/sessions/:id"
        );
        assert_eq!(screen_of("/app"), "/");
        assert_eq!(screen_of("/app/org_123/library?tab=mine#top"), "/:id/library");
        assert_eq!(screen_of("/docs"), "/docs");
        assert_eq!(screen_of("/apple/x"), "/apple/x");
        assert_eq!(screen_of(&format!("/{}", "a".repeat(33))), "/:id");
        assert!(screen_of(&"/a".repeat(100)).chars().count() <= MAX_SCREEN_CHARS);
    }

    #[test]
    fn the_body_is_nexus_uis() {
        let sent = body(
            Kind::Idea,
            "  More shortcuts, please  ",
            Some("/acme/library".into()),
            "Fedora Linux 44",
        );
        assert_eq!(sent["category"], "idea");
        assert_eq!(sent["message"], "More shortcuts, please");
        assert_eq!(sent["screen"], "/acme/library");
        assert_eq!(sent["osVersion"], "Fedora Linux 44");
        assert_eq!(sent["platform"], crate::crash::platform());
        assert_eq!(sent["appVersion"], crate::channel::VERSION.split('+').next().unwrap());
        assert!(sent.get("crash").is_none() && sent.get("answer").is_none());
        let long = body(Kind::Bug, &"x".repeat(MAX_CHARS + 10), None, "x");
        assert_eq!(long["message"].as_str().unwrap().chars().count(), MAX_CHARS);
        assert_eq!(long["screen"], Value::Null);
    }

    #[test]
    fn the_counter_and_the_bound_follow_nexus_ui() {
        assert_eq!(count(3_499), None);
        assert_eq!(count(3_500).as_deref(), Some("3,500 of 4,000"));
        assert_eq!(bounded("short"), None);
        assert_eq!(bounded(&"é".repeat(MAX_CHARS + 1)).unwrap().chars().count(), MAX_CHARS);
        assert_eq!(thousands(12), "12");
    }

    #[test]
    fn the_words_are_nexus_uis() {
        assert_eq!(Kind::ALL.map(Kind::id), ["bug", "idea", "other"]);
        assert_eq!(Kind::Bug.placeholder(), "What happened, and what did you expect?");
        assert_eq!(
            sent_note(Some("ana@acme.com")),
            "The Nexus team reads every message. If we need more, we will write to ana@acme.com."
        );
        assert_eq!(sent_note(None), "The Nexus team reads every message.");
        assert_eq!(
            failure(
                "feedback is not configured on this Gateway",
                "Feedback and crash reports"
            ),
            "Feedback and crash reports go nowhere on this Gateway yet."
        );
        assert!(unset(&failure(
            "feedback is not configured on this Gateway",
            "Crash reports"
        )));
        assert!(failure("Nexus Gateway did not answer in time", "x").contains("try again"));
        assert!(!unset(&failure("Nexus Gateway did not answer in time", "x")));
    }

    /// The whole send through this build's Gateway client, against a fake Gateway on loopback: the path, the client
    /// header and the body, read off the wire. Nothing reaches a real Gateway.
    #[test]
    fn feedback_reaches_the_gateway_as_nexus_ui_sends_it() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(String::from)
                        })
                        .and_then(|n| n.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 13\r\nconnection: close\r\n\r\n{\"sent\":true}")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        nexus_client::install_tls();
        let client = nexus_client::Client::new(crate::gateway::client_options(&url, Some("key".into())));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sent = body(
            Kind::Bug,
            "The tray menu closed",
            Some(screen_of("/app/acme")),
            "Fedora Linux 44",
        );
        let answer = runtime
            .block_on(client.post("/v1/diagnostics/feedback", sent.clone()))
            .unwrap();
        assert_eq!(answer["sent"], true);
        let request = server.join().unwrap();
        let (head, payload) = request.split_once("\r\n\r\n").unwrap();
        let head = head.to_ascii_lowercase();
        assert!(head.starts_with("post /v1/diagnostics/feedback http/1.1\r\n"), "{head}");
        let version = crate::channel::VERSION.to_ascii_lowercase();
        assert!(
            head.contains(&format!("x-nexus-client: desktop/{version}\r\n")),
            "{head}"
        );
        let payload: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(payload, sent);
        assert_eq!(payload["screen"], "/acme");
    }
}
