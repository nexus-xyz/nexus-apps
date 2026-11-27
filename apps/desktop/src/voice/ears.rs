//! The call's ears (the web kit's) on dictation's speech path (`dictation.rs`): Gateway trades its AssemblyAI
//! key for a short-lived token (`POST /v1/chat/voice-token`), and the app streams 16 kHz PCM to AssemblyAI's socket
//! directly. Gateway never carries audio. The microphone is dictation's, the one chosen in Settings, and it opens only
//! when the call starts: macOS asks for it then, the first time.
//!
//! Unlike dictation, a call outlives one socket. A token's session lasts five minutes, so when AssemblyAI ends it
//! (`Termination`) the ears open a new one with a new token, and the microphone stays open. A socket that drops is
//! opened again too; after `MAX_DROPS` drops in a row with nothing heard, the connection counts as lost.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dictation::{DictationEvent, Problem};

/// Drops in a row, with no message between them, before the ears give up. Gateway allows three tokens a minute.
pub const MAX_DROPS: u32 = 2;

/// What the ears tell the call while the microphone is open.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// The microphone and the speech service are open.
    Opened,
    /// They could not open.
    Failed(Problem),
    /// The words of the turn being spoken, as they come.
    Words(String),
    /// A turn ended: what the person said, with punctuation.
    Turn(String),
    /// The microphone's level, 0 to 1, about ten times a second.
    Level(f32),
    /// The connection broke and did not come back.
    Lost,
}

/// Open ears: their events, and what closes the microphone and the socket when dropped.
pub struct Hearing {
    pub events: UnboundedReceiver<Heard>,
    pub _stop: Option<Box<dyn Send>>,
}

impl Hearing {
    /// Ears that cannot hear: this app has no speech service.
    pub fn unavailable() -> Hearing {
        let (out, events) = unbounded();
        send(&out, Heard::Failed(Problem::Unavailable));
        Hearing { events, _stop: None }
    }
}

fn send(out: &UnboundedSender<Heard>, heard: Heard) {
    // A call that closed its ears no longer listens: nothing to tell.
    out.unbounded_send(heard).ok();
}

/// Where to stream: a new token's socket address each time, or why there is none.
pub type Token = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<String, Problem>> + Send>> + Send + Sync>;

/// What one AssemblyAI message means for the call.
#[derive(Debug, PartialEq)]
enum Said {
    Words(String),
    Turn { order: i64, said: String },
    Termination,
    Other,
}

fn read(text: &str) -> Said {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return Said::Other;
    };
    match message.get("type").and_then(Value::as_str) {
        Some("Termination") => Said::Termination,
        Some("Turn") => {
            let said = message
                .get("transcript")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let flag = |name: &str| message.get(name).and_then(Value::as_bool).unwrap_or(false);
            if flag("end_of_turn") && flag("turn_is_formatted") {
                let order = message.get("turn_order").and_then(Value::as_i64).unwrap_or(-1);
                Said::Turn { order, said }
            } else {
                Said::Words(said)
            }
        }
        _ => Said::Other,
    }
}

/// How one socket ended.
enum Ended {
    /// The call closed the ears.
    Stopped,
    /// AssemblyAI ended the session: a new token goes on.
    Terminated,
    /// The socket broke; `heard` says whether a message came first.
    Dropped { heard: bool },
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(token: &Token) -> Result<Socket, Problem> {
    let url = token().await?;
    let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .map_err(|_| Problem::Failed)?;
    Ok(socket)
}

/// One socket's life: audio out, turns in.
async fn session(
    socket: Socket,
    audio: &mut mpsc::UnboundedReceiver<Vec<i16>>,
    stop: &mut oneshot::Receiver<()>,
    out: &UnboundedSender<Heard>,
) -> Ended {
    let (mut sink, mut incoming) = socket.split();
    // Each socket counts its turns from zero: a formatted turn is passed on once.
    let mut last = -1;
    let mut heard = false;
    loop {
        tokio::select! {
            _ = &mut *stop => return Ended::Stopped,
            chunk = audio.recv() => {
                // The microphone ended: nothing more to hear.
                let Some(chunk) = chunk else { return Ended::Stopped };
                let bytes: Vec<u8> = chunk.iter().flat_map(|sample| sample.to_le_bytes()).collect();
                if sink.send(WsMessage::Binary(bytes.into())).await.is_err() {
                    return Ended::Dropped { heard };
                }
            }
            message = incoming.next() => match message {
                Some(Ok(WsMessage::Text(text))) => {
                    heard = true;
                    match read(&text) {
                        Said::Termination => return Ended::Terminated,
                        Said::Turn { order, said } => {
                            let order = if order < 0 { last + 1 } else { order };
                            if order > last {
                                last = order;
                                if !said.is_empty() {
                                    send(out, Heard::Turn(said));
                                }
                            }
                        }
                        Said::Words(words) => send(out, Heard::Words(words)),
                        Said::Other => {}
                    }
                }
                Some(Ok(_)) => {}
                _ => return Ended::Dropped { heard },
            },
        }
        if out.is_closed() {
            return Ended::Stopped;
        }
    }
}

/// What closes the ears when the call drops them: the socket's loop and the microphone's thread.
struct Stop {
    _socket: oneshot::Sender<()>,
    microphone: Arc<AtomicBool>,
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.microphone.store(false, Ordering::SeqCst);
    }
}

/// Open the ears on `runtime`: the microphone at once, then the socket with `token`'s address.
pub fn hear(runtime: &tokio::runtime::Handle, token: Token) -> Hearing {
    let (out, events) = unbounded();
    let (stop_tx, mut stop) = oneshot::channel();
    let microphone = Arc::new(AtomicBool::new(true));
    let running = microphone.clone();
    runtime.spawn(async move {
        // The microphone opens while the token is fetched, as dictation's does, so the first words are not lost.
        let (audio_tx, mut audio) = mpsc::unbounded_channel::<Vec<i16>>();
        let (opened_tx, opened) = oneshot::channel();
        let levels = out.clone();
        let emit: Arc<dyn Fn(DictationEvent) + Send + Sync> = Arc::new(move |event| {
            if let DictationEvent::Level(value) = event {
                send(&levels, Heard::Level(value));
            }
        });
        let thread = std::thread::Builder::new().name("nexus-voice".into()).spawn(move || {
            let choice = crate::paths::dictation_microphone();
            crate::dictation::capture(choice, audio_tx, opened_tx, running, emit)
        });
        if thread.is_err() {
            return send(&out, Heard::Failed(Problem::Failed));
        }
        let first = connect(&token).await;
        match opened.await {
            Ok(Ok(())) => {}
            Ok(Err(problem)) => return send(&out, Heard::Failed(problem)),
            Err(_) => return send(&out, Heard::Failed(Problem::Failed)),
        }
        let mut socket = match first {
            Ok(socket) => socket,
            Err(problem) => return send(&out, Heard::Failed(problem)),
        };
        send(&out, Heard::Opened);
        let mut drops = 0;
        loop {
            drops = match session(socket, &mut audio, &mut stop, &out).await {
                Ended::Stopped => return,
                Ended::Terminated | Ended::Dropped { heard: true } => 0,
                Ended::Dropped { heard: false } => drops + 1,
            };
            if drops > MAX_DROPS {
                return send(&out, Heard::Lost);
            }
            socket = tokio::select! {
                _ = &mut stop => return,
                next = connect(&token) => match next {
                    Ok(socket) => socket,
                    Err(_) => return send(&out, Heard::Lost),
                },
            };
            // What the microphone heard while the socket was down is old now.
            while audio.try_recv().is_ok() {}
        }
    });
    Hearing {
        events,
        _stop: Some(Box::new(Stop {
            _socket: stop_tx,
            microphone,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_formatted_turn_is_a_turn_and_the_rest_are_live_words() {
        let turn = |transcript: &str, finished: bool, order: i64| {
            serde_json::json!({
                "type": "Turn", "transcript": transcript, "end_of_turn": finished,
                "turn_is_formatted": finished, "turn_order": order,
            })
            .to_string()
        };
        assert_eq!(read(&turn("find a gi", false, 0)), Said::Words("find a gi".into()));
        assert_eq!(
            read(&turn(" Find a gift. ", true, 0)),
            Said::Turn {
                order: 0,
                said: "Find a gift.".into()
            }
        );
        assert_eq!(read(r#"{"type":"Termination"}"#), Said::Termination);
        assert_eq!(read(r#"{"type":"Begin"}"#), Said::Other);
        assert_eq!(read("not json"), Said::Other);
    }

    #[test]
    fn ears_without_a_speech_service_say_so() {
        let mut hearing = Hearing::unavailable();
        assert_eq!(
            hearing.events.try_recv().ok(),
            Some(Heard::Failed(Problem::Unavailable))
        );
    }
}
