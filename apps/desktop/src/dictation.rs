//! Dictation, as the Nexus app's (`dictation.ts`): the microphone is read at its own rate, mixed to mono,
//! brought down to 16 kHz 16-bit PCM and streamed in ~100ms chunks over a WebSocket to the speech-to-text address
//! Gateway hands out (`POST /v1/chat/voice-token`). The service answers with `Turn` messages: a finished turn is
//! kept, an unfinished one is the live tail. Stopping sends `Terminate` and waits a moment for the last words.
//!
//! The microphone runs on a thread of its own (cpal's stream is not `Send`); the socket on the Gateway's runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// What the speech-to-text service takes.
const RATE: u32 = 16_000;
/// About 100ms of audio per frame sent.
const CHUNK: usize = 1_600;
/// How long a stop waits for the last words.
const FINISH_WAIT: Duration = Duration::from_millis(2_500);

/// What a dictation tells the composer, in order: listening, levels and text while it listens, then finishing and
/// ended (or failed).
#[derive(Debug, Clone, PartialEq)]
pub enum DictationEvent {
    Listening,
    /// The chosen microphone (`wanted`, its name) is not connected: dictation uses the system default (`using`, its
    /// name when the system says) instead.
    FellBack {
        wanted: String,
        using: Option<String>,
    },
    /// The microphone's loudness, 0 to 1, about ten times a second.
    Level(f32),
    /// All the words so far: the finished turns, then the live one.
    Text(String),
    Finishing,
    Ended,
    Failed(Problem),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Denied,
    NoMicrophone,
    /// Gateway has no speech-to-text service (503 `voice_not_configured`).
    Unavailable,
    Failed,
}

impl Problem {
    /// The line under the composer (`dictation.ts`'s messages).
    pub fn message(self) -> &'static str {
        match self {
            Problem::Denied => "Nexus can't use the microphone.",
            Problem::NoMicrophone => "No microphone found.",
            Problem::Unavailable => "Dictation isn't available here.",
            Problem::Failed => "Dictation stopped. Try again.",
        }
    }
}

/// The handle of a running dictation: stop keeps the last words, cancel ends it now.
pub struct Dictation {
    stop: Option<oneshot::Sender<()>>,
    cancelled: Arc<AtomicBool>,
}

impl Dictation {
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.stop();
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Where to stream, from Gateway's voice token.
pub struct Socket {
    pub url: String,
}

impl Socket {
    pub fn from_json(token: &Value) -> Option<Socket> {
        let url = token.get("url").and_then(Value::as_str)?.to_string();
        Some(Socket { url })
    }
}

/// Start a dictation: `socket` resolves to where to stream (or why not), and every event goes to `emit`.
pub fn start(
    runtime: &tokio::runtime::Handle,
    socket: impl Future<Output = Result<Socket, Problem>> + Send + 'static,
    emit: impl Fn(DictationEvent) + Send + Sync + 'static,
) -> Dictation {
    let (stop_tx, stop_rx) = oneshot::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    runtime.spawn(async move {
        let emit = Arc::new(emit);
        let outcome = run(socket, stop_rx, flag.clone(), emit.clone()).await;
        match outcome {
            Ok(()) => emit(DictationEvent::Ended),
            Err(_) if flag.load(Ordering::SeqCst) => emit(DictationEvent::Ended),
            Err(problem) => emit(DictationEvent::Failed(problem)),
        }
    });
    Dictation {
        stop: Some(stop_tx),
        cancelled,
    }
}

async fn run(
    socket: impl Future<Output = Result<Socket, Problem>>,
    mut stop: oneshot::Receiver<()>,
    cancelled: Arc<AtomicBool>,
    emit: Arc<dyn Fn(DictationEvent) + Send + Sync>,
) -> Result<(), Problem> {
    // The microphone opens while the token is fetched, as the Nexus app does, so the first words are not lost.
    let (audio_tx, mut audio) = mpsc::unbounded_channel::<Vec<i16>>();
    let (opened_tx, opened) = oneshot::channel();
    let mic_running = Arc::new(AtomicBool::new(true));
    let mic_flag = mic_running.clone();
    let to_composer = emit.clone();
    std::thread::Builder::new()
        .name("nexus-dictation".into())
        .spawn(move || {
            let choice = crate::paths::dictation_microphone();
            capture(choice, audio_tx, opened_tx, mic_flag, to_composer)
        })
        .map_err(|_| Problem::Failed)?;
    let mic = StopOnDrop(mic_running);
    let socket = socket.await?;
    opened.await.map_err(|_| Problem::Failed)??;
    let (stream, _) = tokio_tungstenite::connect_async(socket.url.as_str())
        .await
        .map_err(|_| Problem::Failed)?;
    let (mut sink, mut incoming) = stream.split();
    emit(DictationEvent::Listening);

    let mut done = String::new();
    let mut live = String::new();
    let words = |done: &str, live: &str| {
        [done, live]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Listening: audio out, turns in, until stopped.
    loop {
        tokio::select! {
            _ = &mut stop => break,
            chunk = audio.recv() => {
                let Some(chunk) = chunk else { break };
                let bytes: Vec<u8> = chunk.iter().flat_map(|sample| sample.to_le_bytes()).collect();
                sink.send(Message::Binary(bytes.into())).await.map_err(|_| Problem::Failed)?;
            }
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if read_turn(&text, &mut done, &mut live) {
                        emit(DictationEvent::Text(words(&done, &live)));
                    }
                    if is_termination(&text) { return Ok(()) }
                }
                Some(Ok(_)) => {}
                // The service closed while listening: a failure, not an end.
                _ => return Err(Problem::Failed),
            },
        }
    }
    // The microphone closes at once; only the last words are still on their way.
    drop(mic);
    if cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Finishing: no more audio; wait for the last formatted turn.
    emit(DictationEvent::Finishing);
    let _ = sink
        .send(Message::Text(json!({ "type": "Terminate" }).to_string().into()))
        .await;
    let _ = tokio::time::timeout(FINISH_WAIT, async {
        while let Some(Ok(message)) = incoming.next().await {
            if let Message::Text(text) = message {
                if read_turn(&text, &mut done, &mut live) {
                    emit(DictationEvent::Text(words(&done, &live)));
                }
                if is_termination(&text) {
                    break;
                }
            }
        }
    })
    .await;
    Ok(())
}

/// Read a `Turn` message into the finished text and the live tail; whether it changed them.
fn read_turn(text: &str, done: &mut String, live: &mut String) -> bool {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    if message.get("type").and_then(Value::as_str) != Some("Turn") {
        return false;
    }
    let transcript = message.get("transcript").and_then(Value::as_str).unwrap_or("").trim();
    let flag = |name: &str| message.get(name).and_then(Value::as_bool).unwrap_or(false);
    // A turn is finished only once it is both ended and formatted; before that it is the live tail.
    if flag("end_of_turn") && flag("turn_is_formatted") {
        if !transcript.is_empty() {
            if !done.is_empty() {
                done.push(' ');
            }
            done.push_str(transcript);
        }
        live.clear();
    } else {
        *live = transcript.to_string();
    }
    true
}

fn is_termination(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .is_some_and(|message| message.get("type").and_then(Value::as_str) == Some("Termination"))
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The chosen microphone, found and ready to open.
enum Chosen {
    /// A sound server's source, opened through PipeWire's ALSA device (`microphones.rs` says why).
    #[cfg(target_os = "linux")]
    PipeWire(String),
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Cpal(cpal::Device),
}

#[cfg(target_os = "linux")]
fn chosen(id: &str) -> Option<Chosen> {
    Some(Chosen::PipeWire(id.to_string()))
}

#[cfg(not(target_os = "linux"))]
fn chosen(id: &str) -> Option<Chosen> {
    cpal::default_host()
        .input_devices()
        .ok()?
        .find(|device| device.name().is_ok_and(|name| name == id))
        .map(Chosen::Cpal)
}

/// Read the chosen microphone until `running` goes false (dictation, and voice mode's ears in `voice/ears.rs`): mono, 16 kHz, 16-bit, in ~100ms chunks. With no choice, or
/// when the chosen one is not connected, the system default; the composer hears about the second
/// ([`DictationEvent::FellBack`]).
pub(crate) fn capture(
    choice: Option<crate::microphones::Choice>,
    audio: mpsc::UnboundedSender<Vec<i16>>,
    opened: oneshot::Sender<Result<(), Problem>>,
    running: Arc<AtomicBool>,
    emit: Arc<dyn Fn(DictationEvent) + Send + Sync>,
) {
    let level = {
        let emit = emit.clone();
        move |rms| emit(DictationEvent::Level(rms))
    };
    let fell_back = |wanted: String, using: Option<String>| emit(DictationEvent::FellBack { wanted, using });
    let device = choice.and_then(|choice| {
        let microphones = crate::microphones::list();
        let found = microphones.find(&choice.id).and_then(|_| chosen(&choice.id));
        if found.is_none() {
            fell_back(choice.name.clone(), microphones.default);
        }
        found.map(|device| (device, choice.name))
    });
    match device {
        #[cfg(target_os = "linux")]
        Some((Chosen::PipeWire(node), name)) => match open_pipewire(&node) {
            Ok((pcm, rate)) => capture_pipewire(&pcm, rate, audio, opened, running, level),
            // No PipeWire (a PulseAudio-only system): the default, and say so.
            Err(_) => {
                fell_back(name, None);
                capture_default(audio, opened, running, level);
            }
        },
        Some((Chosen::Cpal(device), _)) => capture_cpal(device, audio, opened, running, level),
        None => capture_default(audio, opened, running, level),
    }
}

fn capture_default(
    audio: mpsc::UnboundedSender<Vec<i16>>,
    opened: oneshot::Sender<Result<(), Problem>>,
    running: Arc<AtomicBool>,
    level: impl Fn(f32) + Send + 'static,
) {
    match cpal::default_host().default_input_device() {
        Some(device) => capture_cpal(device, audio, opened, running, level),
        None => {
            let _ = opened.send(Err(Problem::NoMicrophone));
        }
    }
}

/// PipeWire's ALSA device on the source named `node`, as mono 16-bit at 16 kHz when it can (PipeWire converts), and
/// the rate it gives.
#[cfg(target_os = "linux")]
fn open_pipewire(node: &str) -> Result<(alsa::PCM, u32), alsa::Error> {
    use alsa::pcm::{Access, Format, HwParams};
    let name = format!("pipewire:NODE=\"{}\"", node.replace('"', ""));
    let pcm = alsa::PCM::new(&name, alsa::Direction::Capture, false)?;
    {
        let params = HwParams::any(&pcm)?;
        params.set_access(Access::RWInterleaved)?;
        params.set_format(Format::s16())?;
        params.set_channels(1)?;
        params.set_rate(RATE, alsa::ValueOr::Nearest)?;
        pcm.hw_params(&params)?;
    }
    let rate = pcm.hw_params_current()?.get_rate()?;
    Ok((pcm, rate))
}

#[cfg(target_os = "linux")]
fn capture_pipewire(
    pcm: &alsa::PCM,
    rate: u32,
    audio: mpsc::UnboundedSender<Vec<i16>>,
    opened: oneshot::Sender<Result<(), Problem>>,
    running: Arc<AtomicBool>,
    level: impl Fn(f32) + Send + 'static,
) {
    let Ok(io) = pcm.io_i16() else {
        let _ = opened.send(Err(Problem::Failed));
        return;
    };
    let mut resampler = Resampler::new(rate, audio, level);
    let _ = opened.send(Ok(()));
    let mut buffer = vec![0i16; CHUNK];
    while running.load(Ordering::SeqCst) {
        match io.readi(&mut buffer) {
            Ok(frames) => resampler.push(buffer[..frames].iter().map(|&sample| sample as f32 / i16::MAX as f32)),
            // An overrun after a stall is recovered; anything else ends the capture, which ends the dictation.
            Err(error) => {
                if pcm.try_recover(error, true).is_err() {
                    break;
                }
            }
        }
    }
}

fn capture_cpal(
    device: cpal::Device,
    audio: mpsc::UnboundedSender<Vec<i16>>,
    opened: oneshot::Sender<Result<(), Problem>>,
    running: Arc<AtomicBool>,
    level: impl Fn(f32) + Send + 'static,
) {
    let Ok(config) = device.default_input_config() else {
        let _ = opened.send(Err(Problem::Denied));
        return;
    };
    let channels = config.channels() as usize;
    let mut resampler = Resampler::new(config.sample_rate().0, audio, level);
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let error = |_| {};
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &_| resampler.push(data.chunks(channels).map(mix)),
            error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &_| {
                resampler.push(
                    data.chunks(channels)
                        .map(|frame| mix_with(frame, |sample| sample as f32 / i16::MAX as f32)),
                )
            },
            error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &_| {
                resampler.push(
                    data.chunks(channels)
                        .map(|frame| mix_with(frame, |sample| (sample as f32 - 32_768.) / 32_768.)),
                )
            },
            error,
            None,
        ),
        _ => {
            let _ = opened.send(Err(Problem::Failed));
            return;
        }
    };
    let Ok(stream) = stream else {
        let _ = opened.send(Err(Problem::Denied));
        return;
    };
    if stream.play().is_err() {
        let _ = opened.send(Err(Problem::Denied));
        return;
    }
    let _ = opened.send(Ok(()));
    while running.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
}

fn mix(frame: &[f32]) -> f32 {
    frame.iter().sum::<f32>() / frame.len().max(1) as f32
}

fn mix_with<T: Copy>(frame: &[T], to_f32: impl Fn(T) -> f32) -> f32 {
    frame.iter().map(|&sample| to_f32(sample)).sum::<f32>() / frame.len().max(1) as f32
}

/// Brings a mono stream down to 16 kHz by averaging each output sample's span of input (`downsample()` in the Nexus
/// app), and sends it in chunks with their loudness.
struct Resampler<L> {
    ratio: f64,
    /// Input samples still owed to the current output sample.
    position: f64,
    sum: f32,
    count: u32,
    chunk: Vec<i16>,
    audio: mpsc::UnboundedSender<Vec<i16>>,
    level: L,
}

impl<L: Fn(f32)> Resampler<L> {
    fn new(rate: u32, audio: mpsc::UnboundedSender<Vec<i16>>, level: L) -> Self {
        Resampler {
            ratio: rate as f64 / RATE as f64,
            position: 0.,
            sum: 0.,
            count: 0,
            chunk: Vec::with_capacity(CHUNK),
            audio,
            level,
        }
    }

    fn push(&mut self, samples: impl Iterator<Item = f32>) {
        for sample in samples {
            self.sum += sample;
            self.count += 1;
            self.position += 1.;
            if self.position >= self.ratio {
                self.position -= self.ratio;
                let value = (self.sum / self.count as f32).clamp(-1., 1.);
                self.chunk.push((value * i16::MAX as f32) as i16);
                self.sum = 0.;
                self.count = 0;
                if self.chunk.len() >= CHUNK {
                    let chunk = std::mem::replace(&mut self.chunk, Vec::with_capacity(CHUNK));
                    (self.level)(loudness(&chunk));
                    let _ = self.audio.send(chunk);
                }
            }
        }
    }
}

/// The chunk's RMS, scaled as the Nexus app's meter (`min(1, rms × 4)`).
fn loudness(chunk: &[i16]) -> f32 {
    let squares: f32 = chunk
        .iter()
        .map(|&sample| {
            let value = sample as f32 / i16::MAX as f32;
            value * value
        })
        .sum();
    ((squares / chunk.len().max(1) as f32).sqrt() * 4.).min(1.)
}

/// Put dictated words into the text around the caret, with a space on either side where one is needed
/// (`compose()` in `dictation.ts`).
pub fn compose(before: &str, words: &str, after: &str) -> String {
    if words.is_empty() {
        return format!("{before}{after}");
    }
    let gap_before = if before.is_empty() || before.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let gap_after = if after.is_empty() || after.starts_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    format!("{before}{gap_before}{words}{gap_after}{after}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_keep_finished_text_and_replace_the_live_tail() {
        let (mut done, mut live) = (String::new(), String::new());
        let turn = |transcript: &str, finished: bool| {
            json!({ "type": "Turn", "transcript": transcript, "end_of_turn": finished, "turn_is_formatted": finished })
                .to_string()
        };
        assert!(read_turn(&turn("hello wor", false), &mut done, &mut live));
        assert_eq!((done.as_str(), live.as_str()), ("", "hello wor"));
        read_turn(&turn("Hello world.", true), &mut done, &mut live);
        read_turn(&turn("and more", false), &mut done, &mut live);
        assert_eq!((done.as_str(), live.as_str()), ("Hello world.", "and more"));
        assert!(!read_turn(r#"{"type":"Begin"}"#, &mut done, &mut live));
        assert!(is_termination(r#"{"type":"Termination"}"#));
    }

    #[test]
    fn dictated_words_get_spaces_where_needed() {
        assert_eq!(compose("Fix", "the bug", "now"), "Fix the bug now");
        assert_eq!(compose("Fix ", "the bug", " now"), "Fix the bug now");
        assert_eq!(compose("", "Hello", ""), "Hello");
    }

    #[test]
    fn resampling_halves_a_32k_stream() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut resampler = Resampler::new(32_000, tx, |_| {});
        resampler.push(std::iter::repeat_n(0.5, CHUNK * 2));
        let chunk = rx.try_recv().expect("a chunk");
        assert_eq!(chunk.len(), CHUNK);
        assert!((chunk[0] as f32 / i16::MAX as f32 - 0.5).abs() < 0.01);
    }
}
