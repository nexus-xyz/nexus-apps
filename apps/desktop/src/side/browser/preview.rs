//! The browser's dev-server checks and its screenshot. The local addresses the session printed are its servers: each
//! is checked every few seconds (`nexus_browser::preview::probe`), the first that answers opens until
//! the person chooses an address, and a server that does not answer says so. A screenshot of the page goes into the
//! message as `preview.png`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nexus_browser::MAX_TABS;
use nexus_browser::preview::Probe;
use serde_json::{Value, json};

use crate::attachments::{Incoming, Source};

/// How often the servers are checked, as the Nexus app's.
pub const PROBE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a screenshot may take, and how large it may be, as the Nexus app's.
pub const CAPTURE_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
pub const CAPTURE_BYTES: usize = 10 * 1024 * 1024;
/// The screenshot's file name in the message.
pub const CAPTURE_NAME: &str = "preview.png";

/// Whether an address is a dev server on this computer. Websites never widen the server list.
pub fn is_local(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|url| matches!(url.host_str(), Some("localhost" | "127.0.0.1")))
}

/// The address without its query and fragment, for what is shown again later. An address that does not parse stays.
pub fn without_query(address: &str) -> String {
    match url::Url::parse(address) {
        Ok(mut url) => {
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => address.to_string(),
    }
}

/// The addresses to check: the tabs', then the session's servers, each once, at most [`MAX_TABS`].
pub fn addresses(tabs: &[String], found: &[String]) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for address in tabs.iter().chain(found) {
        if !address.is_empty() && !all.contains(address) {
            all.push(address.clone());
        }
    }
    all.truncate(MAX_TABS);
    all
}

/// The servers row: the first address of each origin the session printed.
pub fn servers(found: &[String]) -> Vec<String> {
    let mut origins: Vec<String> = Vec::new();
    let mut servers = Vec::new();
    for address in found {
        let Ok(url) = url::Url::parse(address) else { continue };
        let origin = url.origin().ascii_serialization();
        if !origins.contains(&origin) {
            origins.push(origin);
            servers.push(address.clone());
        }
    }
    servers
}

/// A server's button: its port, `:3000`.
pub fn port_label(address: &str) -> String {
    let Ok(url) = url::Url::parse(address) else {
        return address.to_string();
    };
    let port = url.port().unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    format!(":{port}")
}

/// The answer for `address`, once one came.
pub fn probe_of<'a>(probes: &'a [Probe], address: &str) -> Option<&'a Probe> {
    probes.iter().find(|probe| probe.url == address)
}

/// What the browser says under its bar for the page on screen: a local server that does not answer. Websites handle
/// their own redirects and errors.
pub fn frame_problem(probes: &[Probe], address: &str) -> Option<String> {
    probe_of(probes, address)
        .filter(|probe| !probe.reachable)
        .and_then(|probe| probe.error.clone())
}

/// A way out of a local server that does not answer, as the Nexus app's `wb-preview-recovery` buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// "Try again": check and load the address again.
    TryAgain,
    /// "Open :5173": another of the session's servers that answers.
    Open(String),
    /// "Ask the agent to start it": the text for the message.
    AskAgent(String),
}

impl Recovery {
    pub fn label(&self) -> String {
        match self {
            Recovery::TryAgain => "Try again".into(),
            Recovery::Open(address) => format!("Open {}", port_label(address)),
            Recovery::AskAgent(_) => "Ask the agent to start it".into(),
        }
    }
}

/// The recovery buttons for `address` when its check says nothing answers there: Try again, the first other address
/// that answers, and, when an agent drives the browser, a message that asks it to start the server. Empty otherwise.
pub fn recovery(probes: &[Probe], address: &str, agent_control: bool) -> Vec<Recovery> {
    if frame_problem(probes, address).is_none() {
        return Vec::new();
    }
    let mut all = vec![Recovery::TryAgain];
    if let Some(other) = probes
        .iter()
        .find(|probe| probe.url != address && probe.reachable && probe.error.is_none() && probe.frame_error.is_none())
    {
        all.push(Recovery::Open(other.url.clone()));
    }
    if agent_control && let Ok(url) = url::Url::parse(address) {
        all.push(Recovery::AskAgent(format!(
            "Start the dev server at {} and check that it answers.",
            url.origin().ascii_serialization()
        )));
    }
    all
}

/// Until the person chooses an address, the first of the session's servers that answers opens, or the first server
/// when nothing is open yet. `None` when the page on screen stays.
pub fn auto_open(typed: bool, found: &[String], probes: &[Probe], current: &str) -> Option<String> {
    if typed {
        return None;
    }
    let answering = found
        .iter()
        .find(|address| probe_of(probes, address).is_some_and(|probe| probe.reachable && probe.error.is_none()));
    match answering {
        Some(address) if address != current => Some(address.clone()),
        Some(_) => None,
        None if current.is_empty() => found.first().cloned(),
        None => None,
    }
}

/// The DevTools message that asks the page for a screenshot of what it shows.
pub fn capture_message(id: u64) -> String {
    json!({ "id": id, "method": "Page.captureScreenshot", "params": { "format": "png" } }).to_string()
}

/// The answer to [`capture_message`] `id`: the screenshot as an image for the message, or why not. `None` for any
/// other message.
pub fn capture_answer(id: u64, message: &str) -> Option<Result<Incoming, String>> {
    let message: Value = serde_json::from_str(message).ok()?;
    if message.get("id")?.as_u64()? != id {
        return None;
    }
    let Some(data) = message
        .get("result")
        .and_then(|result| result.get("data"))
        .and_then(Value::as_str)
    else {
        return Some(Err("The preview could not be captured.".into()));
    };
    let Ok(bytes) = STANDARD.decode(data) else {
        return Some(Err("The screenshot could not be decoded.".into()));
    };
    if bytes.is_empty() {
        return Some(Err("The preview returned no image.".into()));
    }
    if bytes.len() > CAPTURE_BYTES {
        return Some(Err("The screenshot is too large.".into()));
    }
    Some(Ok(Incoming {
        name: CAPTURE_NAME.into(),
        media_type: Some("image/png".into()),
        size: bytes.len() as u64,
        source: Source::Bytes(std::sync::Arc::new(bytes)),
    }))
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn probe(url: &str, reachable: bool, error: Option<&str>) -> Probe {
        Probe {
            url: url.into(),
            reachable,
            frame_error: None,
            error: error.map(Into::into),
        }
    }

    #[test]
    fn only_this_computers_servers_are_local() {
        assert!(is_local("http://localhost:3000/"));
        assert!(is_local("http://127.0.0.1:5173/app"));
        assert!(!is_local("https://localhost.example.com/"));
        assert!(!is_local("https://example.com/"));
        assert!(!is_local(""));
    }

    #[test]
    fn what_is_shown_again_has_no_query_or_fragment() {
        assert_eq!(
            without_query("http://localhost:3000/cb?code=abc#x"),
            "http://localhost:3000/cb"
        );
        assert_eq!(without_query("http://localhost:3000/app"), "http://localhost:3000/app");
        assert_eq!(without_query("not an address"), "not an address");
    }

    #[test]
    fn the_checked_addresses_are_the_tabs_then_the_servers_each_once() {
        let tabs = vec!["http://localhost:3000/".to_string(), String::new()];
        let found: Vec<String> = (0..10).map(|n| format!("http://localhost:{}/", 3000 + n)).collect();
        let all = addresses(&tabs, &found);
        assert_eq!(all.len(), MAX_TABS);
        assert_eq!(all[0], "http://localhost:3000/");
        assert_eq!(all[1], "http://localhost:3001/");
    }

    #[test]
    fn a_server_is_named_once_by_its_port() {
        let found = vec![
            "http://localhost:3000/a".to_string(),
            "http://localhost:3000/b".to_string(),
            "http://localhost:5173/".to_string(),
        ];
        assert_eq!(servers(&found), ["http://localhost:3000/a", "http://localhost:5173/"]);
        assert_eq!(port_label("http://localhost:5173/"), ":5173");
        assert_eq!(port_label("https://localhost/"), ":443");
        assert_eq!(port_label("http://localhost/"), ":80");
    }

    #[test]
    fn a_server_that_does_not_answer_says_so() {
        let probes = vec![
            probe(
                "http://localhost:3000/",
                false,
                Some("Nothing answers at this address."),
            ),
            probe("http://localhost:4000/", true, Some("The server answered HTTP 503.")),
        ];
        assert_eq!(
            frame_problem(&probes, "http://localhost:3000/").as_deref(),
            Some("Nothing answers at this address.")
        );
        assert_eq!(frame_problem(&probes, "http://localhost:4000/"), None);
        assert_eq!(frame_problem(&probes, "https://example.com/"), None);
    }

    #[test]
    fn a_server_that_does_not_answer_offers_its_recovery() {
        let down = "http://localhost:3000/app";
        let probes = vec![
            probe(down, false, Some("Nothing answers at this address.")),
            probe("http://localhost:4000/", true, Some("The server answered HTTP 503.")),
            probe("http://localhost:5173/", true, None),
            probe("http://localhost:6006/", true, None),
        ];
        let ask = Recovery::AskAgent("Start the dev server at http://localhost:3000 and check that it answers.".into());
        assert_eq!(
            recovery(&probes, down, true),
            [
                Recovery::TryAgain,
                Recovery::Open("http://localhost:5173/".into()),
                ask.clone()
            ]
        );
        assert_eq!(
            recovery(&probes, down, false),
            [Recovery::TryAgain, Recovery::Open("http://localhost:5173/".into())],
            "no agent to ask"
        );
        assert_eq!(recovery(&probes[..1], down, true), [Recovery::TryAgain, ask.clone()]);
        assert_eq!(recovery(&probes, "http://localhost:5173/", true), [], "it answers");
        assert_eq!(recovery(&probes, "https://example.com/", true), [], "not checked");
        assert_eq!(Recovery::Open("http://localhost:5173/".into()).label(), "Open :5173");
        assert_eq!(ask.label(), "Ask the agent to start it");
    }

    #[test]
    fn the_first_answering_server_opens_until_the_person_chooses() {
        let found = vec![
            "http://localhost:3000/".to_string(),
            "http://localhost:5173/".to_string(),
        ];
        let probes = vec![
            probe(
                "http://localhost:3000/",
                false,
                Some("Nothing answers at this address."),
            ),
            probe("http://localhost:5173/", true, None),
        ];
        assert_eq!(
            auto_open(false, &found, &probes, "http://localhost:3000/").as_deref(),
            Some("http://localhost:5173/")
        );
        assert_eq!(auto_open(false, &found, &probes, "http://localhost:5173/"), None);
        assert_eq!(auto_open(true, &found, &probes, ""), None, "the person chose");
        assert_eq!(
            auto_open(false, &found, &[], "").as_deref(),
            Some("http://localhost:3000/"),
            "nothing open yet: the first server"
        );
        assert_eq!(auto_open(false, &found, &[], "https://example.com/"), None);
    }

    #[test]
    fn a_screenshot_becomes_preview_png() {
        let png = STANDARD.encode(b"\x89PNG fake");
        let answer = format!(r#"{{"id":9,"result":{{"data":"{png}"}}}}"#);
        let image = capture_answer(9, &answer).expect("its answer").expect("an image");
        assert_eq!(image.name, "preview.png");
        assert_eq!(image.media_type.as_deref(), Some("image/png"));
        assert_eq!(image.size, 9);
        assert!(capture_answer(8, &answer).is_none(), "another message");
        assert_eq!(
            capture_answer(9, r#"{"id":9,"error":{"message":"no"}}"#).map(|result| result.err()),
            Some(Some("The preview could not be captured.".into()))
        );
        let large = STANDARD.encode(vec![0_u8; CAPTURE_BYTES + 1]);
        let answer = format!(r#"{{"id":9,"result":{{"data":"{large}"}}}}"#);
        assert_eq!(
            capture_answer(9, &answer).map(|result| result.err()),
            Some(Some("The screenshot is too large.".into()))
        );
        let message: Value = serde_json::from_str(&capture_message(9)).expect("JSON");
        assert_eq!(message["method"], "Page.captureScreenshot");
        assert_eq!(message["params"]["format"], "png");
    }
}
