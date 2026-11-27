//! The session's Browser opens on its own when the agent starts a local dev server, as the Nexus app's
//! (`auto-preview.ts`). A server command (`npm run dev`, `vite`, `uvicorn`…) whose output carries a
//! local address, or a readiness line with one ("Local: http://localhost:5173"), is an offer. The Browser opens once
//! per turn, after the server answers. A conversation restored when it opens offers nothing: only output that arrives
//! while it is open counts. A turn in which the person picked another tab or hid the pane opens nothing more.
//! "Open the browser when the agent starts a server" (Settings › App, `openBrowserOnServer`) turns it off in both apps.

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use nexus_workbench::event::WbEvent;
use regex::Regex;
use serde_json::Value;

/// How long a new server has to answer before the offer lapses, and how often it is asked, as the Nexus app's.
pub const READY_WITHIN: Duration = Duration::from_secs(20);
pub const ASK_EVERY: Duration = Duration::from_millis(750);
/// The most one check may take; the probe has its own 3-second deadline.
const CHECK_AT_MOST: Duration = Duration::from_secs(4);

/// The tools that run commands (`summarize`'s `command` kind in Nexus UI's `session-timeline/tools.ts`).
const COMMAND_TOOLS: [&str; 4] = ["Bash", "BashOutput", "KillShell", "KillBash"];

static CONTENT_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[;&|\n])\s*(?:\S*/)?(?:cat|head|tail|sed|rg|grep|find|ls|less|more)\b").expect("valid")
});
static SERVER_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(?:(?:npm|pnpm|yarn|bun)\s+(?:run\s+)?(?:dev|start|serve|preview)|(?:next|astro|nuxt)\s+(?:dev|start)",
        r"|(?:http-server|uvicorn)|python3?\s+-m\s+(?:http\.server|uvicorn)|flask\s+run|rails\s+(?:server|s))\b",
        // `vite` alone, before its flags, or with a server verb (the Nexus app's lookahead, without one).
        r"|\bvite(?:\s+(?:dev|serve|preview)\b|\s*(?:--|$))",
    ))
    .expect("valid")
});
static READY_OUTPUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:local\s*:|(?:now\s+)?listening\s+(?:at|on)|server\s+ready|ready\s+(?:at|on)|running\s+(?:at|on)",
        r"|(?:live|available)\s+at|preview(?:\s+is)?\s+(?:at|on))[^\n]*https?://",
    ))
    .expect("valid")
});
static WRAPPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:\S*/)?(?:bash|zsh|sh|fish)\s+-[a-z]*c\s+(['"])([\s\S]*)$"#).expect("valid"));
/// Terminal colour codes: dev servers colour their banner, and Vite even colours the port apart from the host.
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("valid"));
static LOCAL_HOST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bhttps?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\])").expect("valid"));
static LOCAL_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(?::\d{1,5})?(?:[/?#][^\s"'`<>()\[\]{}]*)?"#).expect("valid"));

/// What a command tool was asked to run: a server, or a read of a file's content (a README's banner is not a server).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Command {
    server: bool,
    content: bool,
}

fn command_source(input: &Value) -> Command {
    let mut command = input
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| input.get("cmd").and_then(Value::as_str))
        .unwrap_or("");
    // Codex can report the login shell around its command. Classification does not run or rewrite that text.
    if let Some(wrapped) = WRAPPED.captures(command) {
        let quote = &wrapped[1];
        let rest = wrapped.get(2).map_or("", |rest| rest.as_str()).trim_end();
        if let Some(inner) = rest.strip_suffix(quote) {
            command = inner;
        }
    }
    let server = SERVER_COMMAND.is_match(command);
    Command {
        server,
        content: !server && CONTENT_COMMAND.is_match(command),
    }
}

/// An address the way the Nexus app's `previewUrl` writes it: listening addresses and IPv6 loopback become `localhost`.
fn preview_url(text: &str) -> Option<String> {
    let mut url = url::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if matches!(url.host_str(), Some("0.0.0.0" | "[::]" | "[::1]")) {
        url.set_host(Some("localhost")).ok()?;
    }
    Some(url.to_string())
}

/// Every local address in `text`, in order, each once (the Nexus app's `localUrls`). Trailing sentence punctuation is
/// not part of the address, and neither is a longer host (`localhost.example.com`).
pub fn local_urls(text: &str) -> Vec<String> {
    let text = ANSI.replace_all(text, "");
    let mut found: Vec<String> = Vec::new();
    for host in LOCAL_HOST.find_iter(&text) {
        let rest = &text[host.end()..];
        let mut next = rest.chars();
        let longer = match next.next() {
            Some(c) if c.is_alphanumeric() || c == '_' || c == '-' => true,
            Some('.') => next.next().is_some_and(|c| c.is_alphanumeric() || c == '_'),
            _ => false,
        };
        if longer {
            continue;
        }
        let tail = LOCAL_TAIL.find(rest).map_or(0, |tail| tail.end());
        let address = text[host.start()..host.end() + tail].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if let Some(url) = preview_url(address)
            && !found.contains(&url)
        {
            found.push(url);
        }
    }
    found
}

/// What the log says for the Browser to open: each turn's start, and each server address, by its event's number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Turn(i64),
    Server { seq: i64, url: String },
}

/// The [`Signal`]s in a session's events. A sub-agent's message, a failed result, a file or web tool's output, and a
/// shell's read of a file are never a server; a server command's output, or a readiness line, is.
pub fn signals<'a>(events: impl IntoIterator<Item = (i64, &'a WbEvent)>) -> Vec<Signal> {
    let mut commands: HashMap<&str, Command> = HashMap::new();
    let mut found = Vec::new();
    for (seq, event) in events {
        let text = match event {
            WbEvent::TurnStart { .. } => {
                found.push(Signal::Turn(seq));
                continue;
            }
            WbEvent::Tool { id, name, input, .. } => {
                if COMMAND_TOOLS.contains(&name.as_str()) {
                    commands.insert(id, command_source(input));
                }
                continue;
            }
            WbEvent::Text { text, parent: None, .. } if READY_OUTPUT.is_match(text) => text,
            WbEvent::ToolResult {
                id,
                output,
                is_error: false,
                ..
            } => match commands.get(id.as_str()) {
                Some(command) if !command.content && (command.server || READY_OUTPUT.is_match(output)) => output,
                _ => continue,
            },
            _ => continue,
        };
        if let Some(url) = local_urls(text).pop() {
            found.push(Signal::Server { seq, url });
        }
    }
    found
}

/// An address to open, if its server answers while the offer is still current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub url: String,
    pub turn: i64,
    version: u64,
}

/// One session's automatic Browser, while its view is open (the Nexus app's `AutoPreview`).
#[derive(Debug)]
pub struct AutoPreview {
    /// A session this build just started: its output before the first read counts.
    fresh: bool,
    ready: bool,
    /// The last event already seen.
    after: i64,
    /// The running turn's start event.
    turn: i64,
    version: u64,
    /// The turn the Browser already opened in, or the person chose the pane in.
    suppressed: Option<i64>,
    pending: Option<Offer>,
}

impl AutoPreview {
    pub fn new(fresh: bool) -> AutoPreview {
        AutoPreview {
            fresh,
            ready: false,
            after: 0,
            turn: 0,
            version: 0,
            suppressed: None,
            pending: None,
        }
    }

    /// The transcript read again: its signals and its last event. The first read of a restored session is the
    /// baseline and offers nothing.
    pub fn observe(&mut self, signals: &[Signal], last: i64) -> Option<Offer> {
        if !self.ready {
            self.ready = true;
            if !self.fresh {
                for signal in signals {
                    if let Signal::Turn(seq) = signal {
                        self.turn = *seq;
                        self.version += 1;
                    }
                }
                self.after = last;
                return None;
            }
        }
        let mut offered: Option<(String, i64)> = None;
        for signal in signals {
            match signal {
                Signal::Turn(seq) if *seq > self.after => {
                    self.turn = *seq;
                    self.version += 1;
                }
                Signal::Server { seq, url } if *seq > self.after => offered = Some((url.clone(), self.turn)),
                _ => {}
            }
        }
        self.after = self.after.max(last);
        let (url, turn) = offered?;
        if turn != self.turn || self.suppressed == Some(self.turn) {
            return None;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| self.current(pending) && pending.url == url)
        {
            return None;
        }
        self.version += 1;
        let offer = Offer {
            url,
            turn,
            version: self.version,
        };
        self.pending = Some(offer.clone());
        Some(offer)
    }

    /// The person picked another tab, hid the pane, or left the session: nothing more opens in this turn.
    pub fn dismiss(&mut self) {
        if self.ready {
            self.suppressed = Some(self.turn);
        }
        self.pending = None;
        self.version += 1;
    }

    pub fn current(&self, offer: &Offer) -> bool {
        offer.version == self.version && offer.turn == self.turn && self.suppressed != Some(self.turn)
    }

    /// The server did not answer in time. A later result can offer the address again.
    pub fn release(&mut self, offer: &Offer) {
        if self.current(offer) {
            self.pending = None;
            self.version += 1;
        }
    }

    /// The server answered: open it, unless a newer turn or the person's choice came first. Once per turn.
    pub fn accept(&mut self, offer: &Offer) -> bool {
        if !self.current(offer) {
            return false;
        }
        self.pending = None;
        self.suppressed = Some(self.turn);
        true
    }
}

/// Whether `url` answers within `within`, asking one check at a time, every `every` (the Nexus app's `waitForPreview`).
pub async fn wait_ready<F, Fut>(mut check: F, url: &str, within: Duration, every: Duration) -> bool
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = nexus_browser::preview::Probe>,
{
    let deadline = Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let answered = tokio::time::timeout(remaining.min(CHECK_AT_MOST), check(url.to_string()))
            .await
            .is_ok_and(|probe| probe.url == url && probe.reachable && probe.error.is_none());
        if answered {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        tokio::time::sleep(every.min(remaining)).await;
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use serde_json::json;

    use super::*;

    const URL: &str = "http://localhost:5173/";

    fn turn() -> WbEvent {
        WbEvent::TurnStart { turn_id: "t".into() }
    }

    fn text(address: &str) -> WbEvent {
        WbEvent::Text {
            id: "a".into(),
            text: format!("Ready at {address}"),
            parent: None,
        }
    }

    fn tool(name: &str, command: &str) -> WbEvent {
        WbEvent::Tool {
            id: "tool".into(),
            name: name.into(),
            input: json!({ "command": command }),
            parent: None,
        }
    }

    fn result(output: &str, is_error: bool) -> WbEvent {
        serde_json::from_value(json!({
            "kind": "toolResult", "id": "tool", "output": output, "isError": is_error, "truncated": false
        }))
        .expect("a tool result")
    }

    fn read(events: &[(i64, WbEvent)]) -> (Vec<Signal>, i64) {
        let signals = signals(events.iter().map(|(seq, event)| (*seq, event)));
        (signals, events.last().map_or(0, |(seq, _)| *seq))
    }

    fn observe(preview: &mut AutoPreview, events: &[(i64, WbEvent)]) -> Option<Offer> {
        let (signals, last) = read(events);
        preview.observe(&signals, last)
    }

    #[test]
    fn a_restored_conversation_is_the_baseline_and_only_later_output_offers() {
        let mut preview = AutoPreview::new(false);
        let mut events = vec![(1, turn()), (2, text(URL))];
        assert_eq!(observe(&mut preview, &events), None);
        events.push((3, text(URL)));
        let offer = observe(&mut preview, &events).expect("an offer");
        assert_eq!(offer.url, URL);
        assert!(preview.accept(&offer));
    }

    #[test]
    fn a_session_started_here_keeps_its_first_output() {
        let mut preview = AutoPreview::new(true);
        let offer = observe(&mut preview, &[(1, turn()), (2, text(URL))]).expect("an offer");
        assert!(preview.accept(&offer));
    }

    #[test]
    fn conversation_and_public_links_offer_nothing() {
        let mut preview = AutoPreview::new(true);
        assert_eq!(
            observe(&mut preview, &[(1, turn()), (2, text("https://example.com/"))]),
            None
        );
        let explained = WbEvent::Text {
            id: "e".into(),
            text: format!("The README suggests {URL}. No server has started."),
            parent: None,
        };
        assert_eq!(observe(&mut preview, &[(1, turn()), (3, explained)]), None);
        let child = WbEvent::Text {
            id: "c".into(),
            text: format!("Local: {URL}"),
            parent: Some("subagent".into()),
        };
        assert_eq!(
            observe(&mut preview, &[(1, turn()), (4, child)]),
            None,
            "a sub-agent's message"
        );
    }

    #[test]
    fn only_successful_shell_output_offers() {
        for (name, is_error, expected) in [
            ("Bash", false, Some(URL)),
            ("Read", false, None),
            ("Write", false, None),
            ("Grep", false, None),
            ("WebFetch", false, None),
            ("mcp__slack__read_thread", false, None),
            ("Bash", true, None),
        ] {
            let mut preview = AutoPreview::new(true);
            let events = [
                (1, turn()),
                (2, tool(name, "npm run dev")),
                (3, result(&format!("Local: {URL}"), is_error)),
            ];
            let offer = observe(&mut preview, &events).map(|offer| offer.url);
            assert_eq!(offer.as_deref(), expected, "{name}, error={is_error}");
        }
    }

    #[test]
    fn a_shell_started_before_the_baseline_offers_when_its_new_result_arrives() {
        let mut preview = AutoPreview::new(false);
        let mut events = vec![(1, turn()), (2, tool("Bash", "npm run dev"))];
        assert_eq!(observe(&mut preview, &events), None);
        events.push((3, result(URL, false)));
        assert_eq!(
            observe(&mut preview, &events).map(|offer| offer.url).as_deref(),
            Some(URL)
        );
    }

    #[test]
    fn shell_reads_of_a_file_are_not_a_server() {
        for command in [
            "cat README.md",
            "head README.md",
            "rg localhost README.md",
            "cd /app && cat README.md",
            "/bin/zsh -lc 'cat README.md'",
        ] {
            let mut preview = AutoPreview::new(true);
            let events = [
                (1, turn()),
                (2, tool("Bash", command)),
                (3, result(&format!("Example banner:\nLocal: {URL}"), false)),
            ];
            assert_eq!(observe(&mut preview, &events), None, "{command}");
        }
    }

    #[test]
    fn server_commands_offer_their_output() {
        for command in [
            "npm run dev",
            "pnpm dev",
            "vite",
            "vite --port 5173",
            "npx vite preview",
            "uvicorn app:app",
            "python3 -m http.server",
            "bash -lc \"npm run dev\"",
        ] {
            let mut preview = AutoPreview::new(true);
            let events = [(1, turn()), (2, tool("Bash", command)), (3, result(URL, false))];
            assert!(observe(&mut preview, &events).is_some(), "{command}");
        }
        assert!(!SERVER_COMMAND.is_match("vitest run"));
    }

    #[test]
    fn a_custom_servers_readiness_line_offers_but_another_address_does_not() {
        let mut preview = AutoPreview::new(true);
        let mut events = vec![
            (1, turn()),
            (2, tool("Bash", "python custom.py")),
            (3, result(&format!("An example address: {URL}"), false)),
        ];
        assert_eq!(observe(&mut preview, &events), None);
        events.push((4, result(&format!("Now listening on {URL}"), false)));
        assert_eq!(
            observe(&mut preview, &events).map(|offer| offer.url).as_deref(),
            Some(URL)
        );
    }

    #[test]
    fn the_browser_opens_once_per_turn_even_after_it_closes() {
        let mut preview = AutoPreview::new(true);
        let mut events = vec![(1, turn()), (2, text(URL))];
        let offer = observe(&mut preview, &events).expect("an offer");
        assert!(preview.accept(&offer));
        preview.dismiss();
        events.push((3, text(URL)));
        assert_eq!(observe(&mut preview, &events), None);
        events.extend([(4, turn()), (5, text(URL))]);
        assert_eq!(observe(&mut preview, &events).map(|offer| offer.turn), Some(4));
    }

    #[test]
    fn a_pane_choice_cancels_the_check_and_the_rest_of_the_turn() {
        let mut preview = AutoPreview::new(true);
        let mut events = vec![(1, turn()), (2, text(URL))];
        let pending = observe(&mut preview, &events).expect("an offer");
        preview.dismiss();
        assert!(!preview.accept(&pending));
        events.push((3, text(URL)));
        assert_eq!(observe(&mut preview, &events), None);
    }

    #[test]
    fn a_newer_address_or_turn_replaces_an_older_check() {
        let mut preview = AutoPreview::new(true);
        let mut events = vec![(1, turn()), (2, text(URL))];
        let first = observe(&mut preview, &events).expect("an offer");
        events.push((3, text("http://localhost:5174/")));
        let newer = observe(&mut preview, &events).expect("a newer offer");
        assert!(!preview.accept(&first));
        events.push((4, turn()));
        assert_eq!(observe(&mut preview, &events), None);
        assert!(!preview.accept(&newer));
    }

    #[test]
    fn an_address_that_did_not_answer_can_be_offered_again() {
        let mut preview = AutoPreview::new(true);
        let mut events = vec![(1, turn()), (2, text(URL))];
        let first = observe(&mut preview, &events).expect("an offer");
        preview.release(&first);
        events.push((3, text(URL)));
        let second = observe(&mut preview, &events).expect("offered again");
        assert!(second.version > first.version);
        assert!(preview.accept(&second));
    }

    #[test]
    fn the_same_address_again_keeps_the_running_check() {
        let mut preview = AutoPreview::new(true);
        let mut events = vec![(1, turn()), (2, text(URL))];
        let pending = observe(&mut preview, &events).expect("an offer");
        events.push((3, text(URL)));
        assert_eq!(observe(&mut preview, &events), None);
        assert!(preview.current(&pending));
    }

    #[test]
    fn local_addresses_read_as_the_nexus_app_reads_them() {
        assert_eq!(
            local_urls("\x1b[32mLocal:\x1b[0m http://localhost:\x1b[1m5173\x1b[22m/ and http://0.0.0.0:8000."),
            ["http://localhost:5173/", "http://localhost:8000/"]
        );
        assert_eq!(
            local_urls("http://[::1]:3000/app?x=1, done"),
            ["http://localhost:3000/app?x=1"]
        );
        assert!(local_urls("http://localhost.example.com and http://localhostx").is_empty());
        assert_eq!(local_urls("HTTP://127.0.0.1:8080"), ["http://127.0.0.1:8080/"]);
    }

    fn probe(url: String, reachable: bool) -> nexus_browser::preview::Probe {
        nexus_browser::preview::Probe {
            url,
            reachable,
            frame_error: None,
            error: None,
        }
    }

    #[test]
    fn a_server_that_answers_late_opens_and_one_that_never_answers_lapses() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a runtime");
        let mut asked = 0;
        let late = runtime.block_on(wait_ready(
            |url| {
                asked += 1;
                let reachable = asked >= 3;
                async move { probe(url, reachable) }
            },
            URL,
            Duration::from_secs(5),
            Duration::from_millis(5),
        ));
        assert!(late);
        assert_eq!(asked, 3);
        let never = runtime.block_on(wait_ready(
            |url| async move { probe(url, false) },
            URL,
            Duration::from_millis(40),
            Duration::from_millis(5),
        ));
        assert!(!never);
    }
}
