//! A cloud machine's line to Gateway. On a machine with `cloud.json` and its Nexus key
//! (`credentials`), the host dials out, so the desktop app reaches it with no SSH and no open port:
//!
//! 1. `GET /v1/cloud/machines/{machineId}/host`, upgraded, with `Authorization: Bearer <the machine's token>`: the
//!    control connection. Gateway keeps one per machine. It sends `{ "type": "open", "stream": "<id>" }` each time an
//!    app connects; the host sends only pings.
//! 2. For each `open`, `GET /v1/cloud/machines/{machineId}/host/streams/{id}` with the same token. Gateway joins it to
//!    the app's connection and passes every frame through as it is. The host serves it as one more client, in this
//!    process ([`crate::server::serve_socket`], [`Peer::Relayed`]): the hello and its protocol as always, but not the
//!    token, since Gateway checked the person.
//!
//! Every connection pings every 20 seconds (Cloudflare closes one idle for 100). A lost control connection is dialled
//! again after 1 second, doubled up to 30, plus up to a quarter more at random, so a fleet back from a Gateway restart
//! does not dial at once. A refused token (401 or 403) or a missing route (404) waits 5 minutes. No frame's content is ever logged.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use nexus_remote::relay::{self, DialError, PING, SILENCE, Socket};
use serde::Deserialize;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

use crate::cloud::CloudFile;
use crate::files::Credentials;
use crate::server::{Peer, Server, WRITE_STALL, WriteDeadline, serve_socket};

/// Where the line's address and token are. Read before each dial: Gateway may write them after the host started.
#[derive(Debug, Clone)]
pub struct Sources {
    pub cloud: PathBuf,
    pub credentials: PathBuf,
}

/// The waits between dials.
#[derive(Debug, Clone, Copy)]
pub struct Waits {
    pub first: Duration,
    pub max: Duration,
    /// A control connection that lasted this long starts the waits over.
    pub healthy: Duration,
    /// After Gateway refused the token.
    pub refused: Duration,
    /// While the machine has no `cloud.json` or no Nexus key.
    pub idle: Duration,
}

impl Default for Waits {
    fn default() -> Waits {
        Waits {
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
            healthy: Duration::from_secs(60),
            refused: Duration::from_secs(5 * 60),
            idle: Duration::from_secs(2),
        }
    }
}

/// What Gateway sends on the control connection.
#[derive(Debug, Deserialize)]
struct Control {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    stream: Option<String>,
}

/// A stream id is Gateway's own: letters, digits, `-`, `_` and `.`, so it is safe in a path.
fn valid_stream(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The machine's Gateway, id and token, when it is a cloud machine with its Nexus key.
fn target(sources: &Sources) -> Option<(String, String, String)> {
    let file = CloudFile::load(&sources.cloud)?;
    let credentials = Credentials::load(&sources.credentials)?;
    let gateway = file
        .gateway_url
        .filter(|url| !url.trim().is_empty())
        .unwrap_or(credentials.gateway);
    Some((gateway, file.machine_id, credentials.token))
}

/// The headers of every dial: the machine's token, and who is dialling.
fn headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("authorization".into(), format!("Bearer {token}")),
        ("x-nexus-client".into(), format!("desktop-host/{}", crate::VERSION)),
        ("user-agent".into(), format!("nexus-host/{}", crate::VERSION)),
    ]
}

/// `wait` plus up to a quarter more, from the clock's nanoseconds.
fn jitter(wait: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| now.subsec_nanos())
        .unwrap_or(0);
    wait + wait.mul_f64(f64::from(nanos % 1000) / 4000.0)
}

/// Waits `wait`, or less when the host stops. True when it stopped.
async fn rest(wait: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    let stopped = tokio::select! {
        _ = tokio::time::sleep(wait) => false,
        _ = stopped(stop) => true,
    };
    stopped || *stop.borrow()
}

/// Done when `stop` turns true (or its sender is gone).
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

async fn dial(url: &str, token: &str) -> Result<Socket, DialError> {
    relay::dial_over(url, &headers(token), |tcp| {
        Box::new(WriteDeadline::new(tcp, WRITE_STALL))
    })
    .await
}

/// Keeps the line up until `stop` turns true. A machine that is not a cloud machine looks again every minute.
pub async fn run(server: Arc<Server>, sources: Sources, stop: watch::Receiver<bool>) {
    run_with(server, sources, stop, Waits::default()).await
}

/// [`run`] with its own waits (the tests' short ones).
pub async fn run_with(server: Arc<Server>, sources: Sources, mut stop: watch::Receiver<bool>, waits: Waits) {
    let mut delay = waits.first;
    loop {
        if *stop.borrow() {
            return;
        }
        let Some((gateway, machine, token)) = target(&sources) else {
            if rest(waits.idle, &mut stop).await {
                return;
            }
            continue;
        };
        let base = format!("/v1/cloud/machines/{machine}/host");
        let url = match relay::ws_url(&gateway, &base) {
            Ok(url) => url,
            Err(error) => {
                eprintln!("nexus-host: relay: {error}");
                if rest(waits.idle, &mut stop).await {
                    return;
                }
                continue;
            }
        };
        let wait = match dial(&url, &token).await {
            Ok(control) => {
                let started = Instant::now();
                let held = control_loop(control, &server, &url, &token, &mut stop).await;
                if held == Ended::Stopped {
                    return;
                }
                if started.elapsed() >= waits.healthy {
                    delay = waits.first;
                }
                jitter(delay)
            }
            // A 404 is a Gateway that does not have the routes yet: as slow as a refusal.
            Err(error) if error.refused() || error.status == Some(404) => {
                eprintln!(
                    "nexus-host: relay: Gateway refused the machine's token or lacks the route ({}); trying again in {} s",
                    error.status.unwrap_or(0),
                    waits.refused.as_secs()
                );
                delay = waits.first;
                waits.refused
            }
            Err(error) => {
                eprintln!("nexus-host: relay: {error}");
                jitter(delay)
            }
        };
        if rest(wait, &mut stop).await {
            return;
        }
        delay = (delay * 2).min(waits.max);
    }
}

#[derive(Debug, PartialEq)]
enum Ended {
    Lost,
    Stopped,
}

/// The control connection, until it is lost or the host stops: pings, and a stream for each `open`.
async fn control_loop(
    control: Socket,
    server: &Arc<Server>,
    url: &str,
    token: &str,
    stop: &mut watch::Receiver<bool>,
) -> Ended {
    let (mut sink, mut source) = control.split();
    let streams_stop = stop.clone();
    let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
    let silence = tokio::time::sleep(SILENCE);
    tokio::pin!(silence);
    loop {
        tokio::select! {
            _ = stopped(stop) => {
                let _ = sink.send(Message::Close(None)).await;
                return Ended::Stopped;
            }
            _ = pings.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Ended::Lost;
                }
            }
            _ = &mut silence => {
                eprintln!("nexus-host: relay: Gateway went quiet; dialling again");
                return Ended::Lost;
            }
            message = source.next() => {
                silence.as_mut().reset(tokio::time::Instant::now() + SILENCE);
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(control) = serde_json::from_str::<Control>(text.as_str()) else {
                            continue;
                        };
                        match control.stream {
                            Some(id) if control.kind == "open" && valid_stream(&id) => {
                                let stream_url = format!("{url}/streams/{id}");
                                tokio::spawn(serve_stream(server.clone(), stream_url, token.to_string(), streams_stop.clone()));
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ended::Lost,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// One app's connection: dialled back to Gateway, then served as a relayed client.
async fn serve_stream(server: Arc<Server>, url: String, token: String, stop: watch::Receiver<bool>) {
    match dial(&url, &token).await {
        Ok(stream) => serve_socket(stream, server, stop, Peer::Relayed).await,
        Err(error) => eprintln!("nexus-host: relay stream: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_gateways_own_stream_ids_are_dialled() {
        assert!(valid_stream("st_01HZX-abc.9"));
        assert!(!valid_stream(""));
        assert!(!valid_stream("../../x"));
        assert!(!valid_stream("a/b"));
        assert!(!valid_stream("a?b"));
        assert!(!valid_stream(&"a".repeat(129)));
    }

    #[test]
    fn jitter_adds_at_most_a_quarter() {
        let wait = Duration::from_secs(8);
        let got = jitter(wait);
        assert!(got >= wait && got <= wait + Duration::from_secs(2));
    }

    #[test]
    fn reads_the_machine_and_its_key() {
        let dir = std::env::temp_dir().join(format!("nexus-relay-target-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sources = Sources {
            cloud: dir.join("cloud.json"),
            credentials: dir.join("credentials"),
        };
        assert!(target(&sources).is_none());
        let _ = std::fs::write(&sources.cloud, r#"{"machineId":"m_1","gatewayUrl":""}"#);
        assert!(target(&sources).is_none(), "no key yet");
        let credentials = Credentials {
            gateway: "https://gw.example".into(),
            token: "nx_machine".into(),
            org: None,
            label: "box".into(),
        };
        credentials.save(&sources.credentials).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            target(&sources),
            Some(("https://gw.example".into(), "m_1".into(), "nx_machine".into()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
