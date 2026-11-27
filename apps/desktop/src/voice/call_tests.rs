//! The hands-free call on a fake clock, as the web kit's tests drive the web's: the commands stand in for
//! the ears, the voice and the timers.

use super::*;
use crate::voice::spoken::{answer, spoken_reply, write};

const T0: i64 = 1_000;

/// A call and the commands it gave, kept in order.
struct Harness {
    call: Call,
    log: Vec<Command>,
    clock: i64,
}

impl Harness {
    fn new() -> Harness {
        Harness {
            call: Call::new(T0),
            log: Vec::new(),
            clock: T0,
        }
    }

    /// The commands since the last look.
    fn run(&mut self) -> Vec<Command> {
        let commands = self.call.take_commands();
        self.log.extend(commands.iter().cloned());
        commands
    }

    fn last(&self) -> Option<&Command> {
        self.log.last()
    }

    fn asked(&self) -> Vec<String> {
        self.log
            .iter()
            .filter_map(|command| match command {
                Command::Ask { words, .. } => Some(words.clone()),
                _ => None,
            })
            .collect()
    }

    /// The last ask's turn.
    fn ask_turn(&self) -> u64 {
        self.log
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Ask { turn, .. } => Some(*turn),
                _ => None,
            })
            .expect("an ask")
    }

    fn speak_turn(&self) -> u64 {
        self.log
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Speak { turn, .. } => Some(*turn),
                _ => None,
            })
            .expect("speech")
    }

    fn opening(&self) -> u64 {
        self.log
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Open { opening } => Some(*opening),
                _ => None,
            })
            .expect("an open")
    }

    /// The timer set last for `ms`.
    fn timer(&self, ms: u64) -> Timer {
        self.log
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Later { timer, ms: at } if *at == ms => Some(*timer),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {ms} ms timer"))
    }

    fn fire(&mut self, ms: u64) {
        let timer = self.timer(ms);
        self.call.fired(timer, self.clock);
        self.run();
    }

    /// The ears open as asked.
    fn open(&mut self) {
        self.run();
        let opening = self.opening();
        self.call.ears_opened(opening, self.clock);
        self.run();
    }

    fn turn(&mut self, said: &str) {
        self.call.heard_turn(said, self.clock);
        self.run();
    }

    fn reply(&mut self, reply: VoiceReply) {
        let turn = self.ask_turn();
        self.call.answered(turn, reply, self.clock);
        self.run();
    }

    fn state(&self) -> VoiceState {
        self.call.state()
    }
}

/// A call that has opened its ears and listens.
fn listening() -> Harness {
    let mut h = Harness::new();
    h.call.start();
    h.open();
    h
}

/// A call speaking an answer to "Book the Uber.".
fn speaking() -> Harness {
    let mut h = listening();
    h.turn("Book the Uber.");
    h.reply(spoken_reply(vec![answer(
        "The Uber is about sixty four dollars, so it needs Face ID.",
    )]));
    assert_eq!(h.state(), VoiceState::Speaking);
    h
}

#[test]
fn it_opens_the_microphone_only_on_start_and_listens() {
    let mut h = Harness::new();
    assert!(h.run().is_empty());
    h.call.start();
    assert_eq!(h.run(), [Command::Open { opening: 1 }]);
    h.call.ears_opened(1, T0);
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn it_shows_the_live_words_asks_the_turn_says_the_answer_and_listens_again() {
    let mut h = listening();
    h.call.heard_level(0.4, h.clock);
    assert_eq!(h.call.level, 0.4);
    h.call.heard_words("find a gift");
    assert_eq!(h.call.heard, "find a gift");
    h.turn("Find a gift for Ana.");
    assert_eq!(h.asked(), ["Find a gift for Ana."]);
    assert_eq!(h.state(), VoiceState::Thinking);
    assert_eq!(h.call.level, 0.);
    h.reply(spoken_reply(vec![answer("On it. New errand: **Gift for Ana**.")]));
    assert_eq!(h.state(), VoiceState::Speaking);
    assert!(h.log.contains(&Command::Speak {
        turn: h.speak_turn(),
        text: "On it. New errand: Gift for Ana.".into()
    }));
    assert_eq!(h.call.said, "On it. New errand: Gift for Ana.");
    let turn = h.speak_turn();
    h.call.word(turn);
    assert_eq!(h.call.level, WORD_LEVEL);
    h.call.spoken(turn);
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn it_rests_on_needs_you_after_a_write_that_waits_and_listens_once_the_person_speaks() {
    let mut h = listening();
    h.turn("Tell them about Friday.");
    h.reply(spoken_reply(vec![write("Text Ana, Dev and Sam")]));
    let turn = h.speak_turn();
    h.call.spoken(turn);
    assert_eq!(h.state(), VoiceState::NeedsYou);
    h.call.heard_words("and book");
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn it_ignores_words_while_it_thinks_so_one_turn_is_asked_at_a_time() {
    let mut h = listening();
    h.turn("First.");
    h.turn("Second.");
    assert_eq!(h.asked(), ["First."]);
}

#[test]
fn it_says_why_when_the_agent_does_not_answer_and_keeps_listening() {
    let mut h = listening();
    h.turn("Hello?");
    h.reply(VoiceReply::unanswered());
    assert_eq!(h.state(), VoiceState::Listening);
    assert_eq!(h.call.problem.as_deref(), Some(UNANSWERED_TEXT));
}

#[test]
fn a_late_answer_of_an_older_turn_changes_nothing() {
    let mut h = speaking();
    let old = h.speak_turn();
    h.call.interrupt();
    h.run();
    h.call.answered(old, spoken_reply(vec![answer("Stale.")]), h.clock);
    h.call.spoken(old);
    assert_eq!(h.state(), VoiceState::Interrupted);
}

#[test]
fn talking_over_it_stops_at_once_yields_for_600_ms_then_listens() {
    let mut h = speaking();
    h.call.heard_words("wait no stop please");
    h.run();
    assert_eq!(h.log[h.log.len() - 2], Command::Hush);
    assert_eq!(h.state(), VoiceState::Interrupted);
    // The voice's end of the turn it stopped changes nothing.
    let turn = h.speak_turn();
    h.call.spoken(turn);
    assert_eq!(h.state(), VoiceState::Interrupted);
    h.fire(YIELD_MS);
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn it_does_not_stop_for_its_own_voice_coming_back_through_the_microphone() {
    let mut h = speaking();
    h.call.heard_words("the uber is about sixty four");
    h.turn("The Uber is about sixty four dollars.");
    assert_eq!(h.state(), VoiceState::Speaking);
    assert_eq!(h.asked(), ["Book the Uber."]);
}

#[test]
fn it_asks_the_turn_that_interrupted_it_once_the_yield_ends() {
    let mut h = speaking();
    h.turn("Actually make it Friday instead.");
    assert_eq!(h.state(), VoiceState::Interrupted);
    h.fire(YIELD_MS);
    assert_eq!(
        h.asked().last().map(String::as_str),
        Some("Actually make it Friday instead.")
    );
}

#[test]
fn it_stops_for_interrupt_and_a_typed_turn_is_asked() {
    let mut h = speaking();
    h.call.interrupt();
    h.run();
    assert_eq!(h.state(), VoiceState::Interrupted);
    h.fire(YIELD_MS);
    h.call.type_words("Thanks", h.clock);
    h.run();
    assert_eq!(h.asked().last().map(String::as_str), Some("Thanks"));
}

#[test]
fn a_typed_turn_stops_nexus_even_when_it_matches_what_it_says() {
    let mut h = speaking();
    h.call.type_words("about sixty four dollars", h.clock);
    h.run();
    assert_eq!(h.state(), VoiceState::Interrupted);
    h.fire(YIELD_MS);
    assert_eq!(h.asked().last().map(String::as_str), Some("about sixty four dollars"));
}

#[test]
fn mute_closes_the_microphone_lets_nexus_finish_then_rests_muted_and_unmute_opens_it_again() {
    let mut h = listening();
    h.turn("Book it.");
    h.reply(spoken_reply(vec![answer("Booked.")]));
    h.call.toggle_mute(h.clock);
    h.run();
    assert!(h.call.muted());
    assert_eq!(h.last(), Some(&Command::Close));
    assert_eq!(h.state(), VoiceState::Speaking);
    h.call.heard_words("ignored now because muted");
    assert_eq!(h.state(), VoiceState::Speaking);
    let turn = h.speak_turn();
    h.call.spoken(turn);
    assert_eq!(h.state(), VoiceState::Muted);
    h.call.toggle_mute(h.clock);
    h.run();
    assert!(matches!(h.last(), Some(Command::Open { .. })));
    h.open();
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn a_refused_microphone_cant_hear_and_says_why() {
    let mut h = Harness::new();
    h.call.start();
    h.run();
    h.call.ears_failed(h.opening(), Problem::Denied);
    assert_eq!(h.state(), VoiceState::CantHear);
    assert!(h.call.problem.as_deref().unwrap_or_default().contains("microphone"));
}

#[test]
fn no_speech_service_says_voice_isnt_available() {
    let mut h = Harness::new();
    h.call.start();
    h.run();
    h.call.ears_failed(h.opening(), Problem::Unavailable);
    assert_eq!(h.state(), VoiceState::CantHear);
    assert_eq!(
        h.call.problem.as_deref(),
        Some("Voice isn't available here. Click Type.")
    );
}

#[test]
fn digital_silence_reads_as_cant_hear_and_recovers_when_sound_comes_back() {
    let mut h = listening();
    h.call.heard_level(0., h.clock);
    h.clock += CANT_HEAR_MS + 1;
    h.call.heard_level(0., h.clock);
    assert_eq!(h.state(), VoiceState::CantHear);
    assert_eq!(h.call.problem.as_deref(), Some(SILENT));
    h.call.heard_level(0.1, h.clock);
    assert_eq!(h.state(), VoiceState::Listening);
    assert_eq!(h.call.problem, None);
}

#[test]
fn a_lost_connection_offers_text_and_tries_again() {
    let mut h = listening();
    h.call.heard_lost();
    h.run();
    assert_eq!(h.state(), VoiceState::PoorConnection);
    assert_eq!(h.call.problem.as_deref(), Some(POOR_CONNECTION));
    h.fire(RETRY_MS);
    assert!(matches!(h.last(), Some(Command::Open { .. })));
    h.open();
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn a_failed_first_open_is_a_poor_connection() {
    let mut h = Harness::new();
    h.call.start();
    h.run();
    h.call.ears_failed(h.opening(), Problem::Failed);
    h.run();
    assert_eq!(h.state(), VoiceState::PoorConnection);
    h.timer(RETRY_MS);
}

#[test]
fn an_ask_that_fails_offline_offers_text() {
    let mut h = listening();
    h.turn("Hello.");
    h.reply(VoiceReply::Problem {
        text: "offline".into(),
        offline: true,
    });
    assert_eq!(h.state(), VoiceState::PoorConnection);
    h.call.retry();
    assert_eq!(h.state(), VoiceState::Listening);
}

#[test]
fn ending_stops_everything_rests_on_ended_and_keeps_the_transcript_and_the_messages() {
    let pending = write("Text Ana");
    let mut h = listening();
    h.turn("Tell Ana about Friday.");
    h.reply(spoken_reply(vec![answer("Here's the text. Send it?"), pending.clone()]));
    h.clock += 240_000;
    h.call.end(h.clock);
    h.run();
    assert_eq!(h.last(), Some(&Command::Hush));
    assert!(h.log.contains(&Command::Close));
    assert_eq!(h.state(), VoiceState::Ended);
    let record = h.call.record().expect("a record");
    let who: Vec<Who> = record.turns.iter().map(|turn| turn.who).collect();
    assert_eq!(who, [Who::You, Who::Nexus]);
    assert!(record.messages.contains(&pending));
    assert_eq!(record.ended_at - record.started_at, 240_000);
    h.turn("Too late.");
    assert_eq!(h.asked().len(), 1);
}

#[test]
fn ending_waits_for_an_answer_still_coming_unspoken_so_it_lands_in_the_thread() {
    let mut h = listening();
    h.turn("What's happening with dinner?");
    let turn = h.ask_turn();
    h.call.end(h.clock);
    h.run();
    assert_eq!(h.call.record(), None, "the answer is still coming");
    h.call.answered(
        turn,
        spoken_reply(vec![answer("Booked: Nopa, Friday at 7:30.")]),
        h.clock,
    );
    h.run();
    let record = h.call.record().expect("a record");
    assert_eq!(
        record.turns.last().map(|turn| turn.text.as_str()),
        Some("Booked: Nopa, Friday at 7:30.")
    );
    assert!(!h.log.iter().any(|command| matches!(command, Command::Speak { .. })));
}

#[test]
fn it_ends_once() {
    let mut h = listening();
    h.call.end(h.clock);
    let first = h.call.record();
    h.run();
    h.call.end(h.clock + 5_000);
    assert!(h.run().is_empty());
    assert_eq!(h.call.record(), first);
}

#[test]
fn every_state_has_one_word() {
    assert_eq!(VoiceState::CantHear.word(), "Can't hear you");
    assert_eq!(VoiceState::PoorConnection.word(), "Poor connection");
    assert_eq!(VoiceState::NeedsYou.word(), "Needs you");
}
