//! The hands-free call, without GPUI: the web kit's `createVoiceCall`, rule for rule. The microphone stays
//! open; each turn the person speaks is asked as a normal message in the conversation, and Nexus says the answer. The
//! person can talk over Nexus, mute the microphone, type, or end the call.
//!
//! The web's engine calls its ears, voice and timers itself. This one says what to do as [`Command`]s, and the pane
//! (`pane.rs`) does it and reports back: the ears opened or failed, a turn was heard, the agent answered, the voice
//! finished, a timer fired. Each answer, word and timer carries the turn or the timer it belongs to, so a late one
//! changes nothing, as the web's turn counter makes sure.

use nexus_desktop_core::agent::Message;

use crate::dictation::Problem;

use super::spoken::is_echo;

/// The call's eight states, and the two that end or weaken it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VoiceState {
    Ready,
    Listening,
    Thinking,
    Speaking,
    Interrupted,
    NeedsYou,
    CantHear,
    Muted,
    PoorConnection,
    Ended,
}

impl VoiceState {
    /// The one word under the mark.
    pub fn word(self) -> &'static str {
        match self {
            VoiceState::Ready => "Ready",
            VoiceState::Listening => "Listening",
            VoiceState::Thinking => "Thinking",
            VoiceState::Speaking => "Speaking",
            VoiceState::Interrupted => "Interrupted",
            VoiceState::NeedsYou => "Needs you",
            VoiceState::CantHear => "Can't hear you",
            VoiceState::Muted => "Muted",
            VoiceState::PoorConnection => "Poor connection",
            VoiceState::Ended => "Ended",
        }
    }
}

/// Interrupted, the mark settles for 600 ms, then it listens.
pub const YIELD_MS: u64 = 600;
/// The level each spoken word pulses to.
pub const WORD_LEVEL: f32 = 0.75;
/// A level under this is digital silence: a microphone that is off or muted, not a quiet room.
pub const SILENCE_FLOOR: f32 = 0.002;
/// Digital silence this long while listening reads as can't hear you.
pub const CANT_HEAR_MS: i64 = 6_000;
/// After a lost connection, it tries again this often while the call lasts.
pub const RETRY_MS: u64 = 5_000;

/// The pane adds its offer, "Switch to text?".
pub const POOR_CONNECTION: &str = "Connection is weak.";
pub const SILENT: &str = "I can't hear you. Check your microphone, or click Type.";
pub const UNANSWERED_TEXT: &str = "Nexus did not answer. Say it again, or click Type.";
/// The system refused the microphone: the pane adds "Open System Settings ›" and "Type instead".
pub const DENIED: &str = "Nexus can't use the microphone.";

/// Why the ears cannot hear: the line under the captions.
fn deaf(problem: Problem) -> &'static str {
    match problem {
        Problem::Denied => DENIED,
        Problem::NoMicrophone => "No microphone found. Click Type to go on.",
        Problem::Unavailable | Problem::Failed => "Voice isn't available here. Click Type.",
    }
}

/// What the agent's reply gives the call: the words to say and the reply's messages, or why there are none.
#[derive(Clone, Debug, PartialEq)]
pub enum VoiceReply {
    Said {
        say: String,
        needs_you: bool,
        messages: Vec<Message>,
    },
    Problem {
        text: String,
        offline: bool,
    },
}

impl VoiceReply {
    pub fn unanswered() -> VoiceReply {
        VoiceReply::Problem {
            text: UNANSWERED_TEXT.into(),
            offline: false,
        }
    }
}

/// What the call asks the pane to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Open the microphone and the speech service; report back with `opening`.
    Open { opening: u64 },
    /// Close the microphone and the connection. An open still running ends without opening.
    Close,
    /// Ask the words as a normal turn of the conversation; answer with `turn`.
    Ask { turn: u64, words: String },
    /// Say the text; its words and its end come back with `turn`.
    Speak { turn: u64, text: String },
    /// Stop the voice at once.
    Hush,
    /// Fire `timer` after `ms`.
    Later { timer: Timer, ms: u64 },
}

/// A timer the call set. A newer one of the same kind, or the end of the call, makes an older one fire for nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    Yield(u64),
    Retry(u64),
}

/// Who said a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    You,
    Nexus,
}

/// One side of the call, as the transcript keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceTurn {
    pub who: Who,
    pub text: String,
    /// Epoch ms.
    pub at: i64,
}

/// The call once it ends: what was said, and every message of the agent's replies, for the thread.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceRecord {
    pub started_at: i64,
    pub ended_at: i64,
    pub turns: Vec<VoiceTurn>,
    pub messages: Vec<Message>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trouble {
    Lost,
    Deaf,
    Silent,
}

/// The call. Times are epoch ms, from the caller's clock.
#[derive(Debug)]
pub struct Call {
    state: VoiceState,
    muted: bool,
    needs_you: bool,
    ended: bool,
    ears_open: bool,
    trouble: Option<Trouble>,
    // Each answer and each end moves `turn` on, so a late answer, word or end of an older turn changes nothing. Each
    // opening or closing of the ears moves `opening` on.
    turn: u64,
    opening: u64,
    saying: String,
    pending: Option<String>,
    /// The turn whose ask is in flight.
    asking: Option<u64>,
    /// The call ended while that turn's ask was in flight: its answer is waited for, unspoken, for the record.
    late: Option<u64>,
    last_sound: i64,
    timers: u64,
    yield_timer: Option<(u64, u64)>,
    retry_timer: Option<u64>,
    started_at: i64,
    ended_at: Option<i64>,
    turns: Vec<VoiceTurn>,
    messages: Vec<Message>,
    commands: Vec<Command>,
    /// The person's words: live while they talk, then the last thing they said.
    pub heard: String,
    /// The last thing Nexus said.
    pub said: String,
    pub problem: Option<String>,
    /// The voice's level, 0 to 1: the microphone's while listening, a pulse per word while speaking.
    pub level: f32,
    /// The level the call set last, until the pane takes it: the mark's own pulse fades between them.
    level_event: Option<f32>,
}

impl Call {
    pub fn new(now: i64) -> Call {
        Call {
            state: VoiceState::Ready,
            muted: false,
            needs_you: false,
            ended: false,
            ears_open: false,
            trouble: None,
            turn: 0,
            opening: 0,
            saying: String::new(),
            pending: None,
            asking: None,
            late: None,
            last_sound: now,
            timers: 0,
            yield_timer: None,
            retry_timer: None,
            started_at: now,
            ended_at: None,
            turns: Vec::new(),
            messages: Vec::new(),
            commands: Vec::new(),
            heard: String::new(),
            said: String::new(),
            problem: None,
            level: 0.,
            level_event: None,
        }
    }

    pub fn state(&self) -> VoiceState {
        self.state
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    /// The level the call set since the last look, if it set one.
    pub fn take_level(&mut self) -> Option<f32> {
        self.level_event.take()
    }

    fn emit_level(&mut self, value: f32) {
        self.level = value;
        self.level_event = Some(value);
    }

    /// What to do now, in order.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    fn set(&mut self, next: VoiceState) {
        if next == self.state {
            return;
        }
        self.state = next;
        if next != VoiceState::Listening && next != VoiceState::Speaking {
            self.emit_level(0.);
        }
    }

    /// Where the call rests between turns.
    fn rest(&self) -> VoiceState {
        if self.muted {
            return VoiceState::Muted;
        }
        match self.trouble {
            Some(Trouble::Lost) => VoiceState::PoorConnection,
            Some(_) => VoiceState::CantHear,
            None if !self.ears_open => VoiceState::Ready,
            None if self.needs_you => VoiceState::NeedsYou,
            None => VoiceState::Listening,
        }
    }

    /// Rests, unless a turn is under way: thinking, speaking and the yield settle by themselves.
    fn settle(&mut self) {
        if !matches!(
            self.state,
            VoiceState::Thinking | VoiceState::Speaking | VoiceState::Interrupted
        ) {
            self.set(self.rest());
        }
    }

    fn timer(&mut self) -> u64 {
        self.timers += 1;
        self.timers
    }

    fn schedule_retry(&mut self) {
        let id = self.timer();
        self.retry_timer = Some(id);
        self.commands.push(Command::Later {
            timer: Timer::Retry(id),
            ms: RETRY_MS,
        });
    }

    fn close_ears(&mut self) {
        self.opening += 1;
        self.ears_open = false;
        self.commands.push(Command::Close);
    }

    fn lose(&mut self) {
        self.close_ears();
        self.trouble = Some(Trouble::Lost);
        self.problem = Some(POOR_CONNECTION.into());
        self.settle();
        self.schedule_retry();
    }

    fn listen(&mut self) {
        self.opening += 1;
        self.commands.push(Command::Open { opening: self.opening });
    }

    /// Opens the microphone (its permission is asked then, and only then) and listens.
    pub fn start(&mut self) {
        if !self.ended {
            self.listen();
        }
    }

    /// The ears of `opening` hear.
    pub fn ears_opened(&mut self, opening: u64, now: i64) {
        if opening != self.opening || self.ended {
            return;
        }
        self.ears_open = true;
        self.trouble = None;
        self.last_sound = now;
        self.problem = None;
        self.settle();
    }

    /// The ears of `opening` could not open: a refused or missing microphone, no speech service, or no connection.
    pub fn ears_failed(&mut self, opening: u64, problem: Problem) {
        if opening != self.opening || self.ended {
            return;
        }
        self.ears_open = false;
        if problem == Problem::Failed {
            return self.lose();
        }
        self.trouble = Some(Trouble::Deaf);
        self.problem = Some(deaf(problem).into());
        self.settle();
    }

    /// Tries the connection or the microphone again.
    pub fn retry(&mut self) {
        if self.ended || self.muted {
            return;
        }
        if self.ears_open {
            self.trouble = None;
            self.problem = None;
            self.settle();
            return;
        }
        self.listen();
    }

    fn answer(&mut self, said: String, now: i64) {
        self.turn += 1;
        self.needs_you = false;
        self.turns.push(VoiceTurn {
            who: Who::You,
            text: said.clone(),
            at: now,
        });
        self.heard = said.clone();
        self.said.clear();
        self.problem = None;
        self.set(VoiceState::Thinking);
        self.asking = Some(self.turn);
        self.commands.push(Command::Ask {
            turn: self.turn,
            words: said,
        });
    }

    fn keep(&mut self, say: &str, messages: Vec<Message>, now: i64) {
        self.messages.extend(messages);
        if !say.is_empty() {
            self.turns.push(VoiceTurn {
                who: Who::Nexus,
                text: say.to_string(),
                at: now,
            });
        }
    }

    /// The agent answered `turn`'s ask.
    pub fn answered(&mut self, turn: u64, reply: VoiceReply, now: i64) {
        if self.asking == Some(turn) {
            self.asking = None;
        }
        if self.ended {
            if self.late == Some(turn) {
                self.late = None;
                if let VoiceReply::Said { say, messages, .. } = reply {
                    self.keep(&say, messages, now);
                }
                self.ended_at = Some(now);
            }
            return;
        }
        if turn != self.turn {
            return;
        }
        match reply {
            VoiceReply::Problem { text, offline } => {
                if offline {
                    self.trouble = Some(Trouble::Lost);
                    self.schedule_retry();
                }
                self.problem = Some(if offline { POOR_CONNECTION.into() } else { text });
                self.set(self.rest());
            }
            VoiceReply::Said {
                say,
                needs_you,
                messages,
            } => {
                self.keep(&say, messages, now);
                self.needs_you = needs_you;
                self.said = say.clone();
                if say.is_empty() {
                    return self.set(self.rest());
                }
                self.saying = say.clone();
                self.set(VoiceState::Speaking);
                self.commands.push(Command::Speak { turn, text: say });
            }
        }
    }

    /// The voice said a word of `turn`'s answer.
    pub fn word(&mut self, turn: u64) {
        if turn == self.turn && self.state == VoiceState::Speaking {
            self.emit_level(WORD_LEVEL);
        }
    }

    /// The voice finished `turn`'s answer, or stopped, or failed.
    pub fn spoken(&mut self, turn: u64) {
        if turn != self.turn || self.state != VoiceState::Speaking {
            return;
        }
        self.saying.clear();
        self.set(self.rest());
    }

    /// Talks over Nexus: it stops at once, settles for 600 ms, and listens.
    pub fn interrupt(&mut self) {
        if self.ended || self.state != VoiceState::Speaking {
            return;
        }
        self.turn += 1;
        self.commands.push(Command::Hush);
        self.saying.clear();
        self.set(VoiceState::Interrupted);
        let id = self.timer();
        self.yield_timer = Some((id, self.turn));
        self.commands.push(Command::Later {
            timer: Timer::Yield(id),
            ms: YIELD_MS,
        });
    }

    /// A timer the call set fired.
    pub fn fired(&mut self, timer: Timer, now: i64) {
        match timer {
            Timer::Yield(id) => {
                let Some((current, mine)) = self.yield_timer else {
                    return;
                };
                if current != id {
                    return;
                }
                self.yield_timer = None;
                if mine != self.turn || self.ended {
                    return;
                }
                match self.pending.take() {
                    Some(next) => self.answer(next, now),
                    None => self.set(self.rest()),
                }
            }
            Timer::Retry(id) => {
                if self.retry_timer != Some(id) {
                    return;
                }
                self.retry_timer = None;
                if !self.ended && !self.muted && self.trouble == Some(Trouble::Lost) {
                    self.retry();
                }
            }
        }
    }

    /// A turn the person said or typed: asked now, or after the yield when Nexus is speaking.
    fn take(&mut self, said: &str, typed: bool, now: i64) {
        let words = said.trim();
        if words.is_empty() || self.ended {
            return;
        }
        match self.state {
            VoiceState::Speaking => {
                if !typed && is_echo(words, &self.saying) {
                    return;
                }
                self.pending = Some(words.to_string());
                self.interrupt();
            }
            VoiceState::Interrupted => self.pending = Some(words.to_string()),
            VoiceState::Thinking => {}
            _ => self.answer(words.to_string(), now),
        }
    }

    /// Words the person typed in the call, asked as a turn.
    pub fn type_words(&mut self, words: &str, now: i64) {
        self.take(words, true, now);
    }

    /// The words of the turn being spoken, as they come.
    pub fn heard_words(&mut self, live: &str) {
        if self.ended || self.muted {
            return;
        }
        match self.state {
            VoiceState::Speaking => {
                if !is_echo(live, &self.saying) {
                    self.interrupt();
                }
            }
            VoiceState::Listening | VoiceState::NeedsYou | VoiceState::Interrupted => {
                if live.trim().is_empty() {
                    return;
                }
                if self.state == VoiceState::NeedsYou {
                    self.set(VoiceState::Listening);
                }
                self.heard = live.to_string();
            }
            _ => {}
        }
    }

    /// A turn ended: what the person said, with punctuation.
    pub fn heard_turn(&mut self, said: &str, now: i64) {
        if !self.muted {
            self.take(said, false, now);
        }
    }

    /// The microphone's level, 0 to 1, about ten times a second.
    pub fn heard_level(&mut self, value: f32, now: i64) {
        if self.ended || self.muted {
            return;
        }
        if value > SILENCE_FLOOR {
            self.last_sound = now;
            if self.trouble == Some(Trouble::Silent) {
                self.trouble = None;
                self.problem = None;
                self.settle();
            }
        } else if self.trouble.is_none()
            && now - self.last_sound > CANT_HEAR_MS
            && matches!(self.state, VoiceState::Listening | VoiceState::NeedsYou)
        {
            self.trouble = Some(Trouble::Silent);
            self.problem = Some(SILENT.into());
            self.settle();
        }
        if self.state == VoiceState::Listening {
            self.emit_level(value);
        }
    }

    /// The connection to the speech service broke and did not come back.
    pub fn heard_lost(&mut self) {
        if !self.ended && !self.muted && self.ears_open {
            self.lose();
        }
    }

    /// Mutes or unmutes the microphone. Muted, Nexus still finishes what it is saying.
    pub fn toggle_mute(&mut self, now: i64) {
        if self.ended {
            return;
        }
        self.muted = !self.muted;
        if self.muted {
            self.close_ears();
            self.retry_timer = None;
            self.trouble = None;
            self.problem = None;
            self.settle();
            return;
        }
        self.last_sound = now;
        self.settle();
        self.listen();
    }

    /// Ends the call. An answer still coming is waited for, unspoken, so it lands in the thread ([`Call::record`]).
    pub fn end(&mut self, now: i64) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.turn += 1;
        self.yield_timer = None;
        self.retry_timer = None;
        self.close_ears();
        if self.state == VoiceState::Speaking {
            self.commands.push(Command::Hush);
        }
        self.late = if self.state == VoiceState::Thinking {
            self.asking
        } else {
            None
        };
        if self.late.is_none() {
            self.ended_at = Some(now);
        }
        self.set(VoiceState::Ended);
    }

    /// The call's record, once it ended and no answer is still coming.
    pub fn record(&self) -> Option<VoiceRecord> {
        Some(VoiceRecord {
            started_at: self.started_at,
            ended_at: self.ended_at?,
            turns: self.turns.clone(),
            messages: self.messages.clone(),
        })
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
