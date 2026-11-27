//! A Claude Code session whose own saved conversation can't be reopened goes on from Nexus's copy.
//!
//! Every message after a session's first starts Claude Code with `--resume <id>` (or forks a source's conversation).
//! Claude Code reads that conversation from its own transcript file, which can be missing or unreadable: another
//! profile folder (`CLAUDE_CONFIG_DIR`) than the one that wrote it, a moved working folder, its 30-day cleanup, a
//! crash before the file was written, a truncated file, a rewind point the file no longer has. It then exits before it
//! says hello (`init`), and every later message would fail the same way.
//!
//! So a resume that exits before `init` restarts the conversation as a refused rewind does
//! ([`crate::Workbench::restart_rewound_conversation`]): a new native id, seeded with the text Nexus logged. The
//! message is sent once more, to that fresh conversation, with a notice in the log. The fresh start is not a resume,
//! so it is never retried: if it fails too, the error shows as it does today.

use futures::future::BoxFuture;

use crate::driver::Say;
use crate::event::{SessionAgent, ThreadStatus, WbEvent};
use crate::store::ThreadRecord;
use crate::{Result, Workbench};

/// What the log says when the session went on from Nexus's copy.
pub const UNRESUMED: &str =
    "Claude Code couldn't reopen its saved copy of this conversation, so it continued from Nexus's copy.";

/// What the agent is told first in the fresh conversation. The files are as the last turn left them.
const REOPENED: &str = "Your saved copy of this conversation could not be opened, so this is a new one. \
     Continue from Nexus's transcript of it below.";

/// The message a resume launch was started with, kept until the agent says hello: sent once more to a fresh
/// conversation when the resume fails.
pub(crate) struct Resend {
    say: Say,
    seq: i64,
    person: bool,
}

impl Resend {
    /// The launch's message, when the launch resumes a conversation Claude Code may no longer have. `say` is the
    /// message as the person sent it, before notes for the resumed conversation were put in front of it.
    pub(crate) fn keep(agent: SessionAgent, resumes: bool, say: Option<Say>, seq: i64, person: bool) -> Option<Resend> {
        let say = say.filter(|_| resumes && agent == SessionAgent::Claude)?;
        matches!(say, Say::User { .. } | Say::Steer { .. }).then_some(Resend { say, seq, person })
    }

    /// The message's number (its `User` event).
    pub(crate) fn seq(&self) -> i64 {
        self.seq
    }

    /// The message for the fresh conversation: what was said before it first, as a fork from an earlier turn has it.
    fn seeded(self, seed: Option<String>) -> (Say, i64, bool) {
        let Resend { mut say, seq, person } = self;
        if let (Some(seed), Say::User { text, .. } | Say::Steer { text, .. }) = (seed, &mut say) {
            *text = format!("{seed}{text}");
        }
        (say, seq, person)
    }
}

/// Whether a run that ended is a resume the agent could not open: it ended on its own before it said hello, and no
/// cause was shown for it already (a sign-in, say, which a fresh conversation would not fix).
pub(crate) fn unresumed(resend: bool, crashed: bool, initialized: bool, spawned: bool, explained: bool) -> bool {
    resend && crashed && spawned && !initialized && !explained
}

impl Workbench {
    /// Starts the session's conversation again from Nexus's log, up to the message numbered `seq`, and says so. Run
    /// while the failed run is still registered, so a message sent meanwhile already finds the fresh conversation.
    pub(crate) fn restart_unresumed(&self, thread_id: &str, seq: i64) -> Result<Option<String>> {
        let mut record: ThreadRecord = self.record(thread_id)?;
        let events = self.reader().events_after(thread_id, 0)?;
        record.summary.status = ThreadStatus::Idle;
        let seed = self.restart_conversation(&mut record, &events, seq, REOPENED)?;
        self.append(thread_id, WbEvent::Notice { text: UNRESUMED.into() })?;
        Ok(seed)
    }

    /// Sends the failed launch's message to the fresh conversation. Boxed: it starts a run from the end of one.
    pub(crate) fn resend_unresumed(
        &self,
        thread_id: String,
        resend: Resend,
        seed: Option<String>,
    ) -> BoxFuture<'static, ()> {
        let this = self.clone();
        Box::pin(async move {
            let (say, seq, person) = resend.seeded(seed);
            let started = match this.record(&thread_id) {
                Ok(record) => this.start(record, say, seq, person, None).await,
                Err(error) => Err(error),
            };
            if let Err(text) = started {
                eprintln!("nexus-workbench: {thread_id}: {text}");
                let shown = this.append(&thread_id, WbEvent::Error { text, detail: None });
                let status = this.change(&thread_id, |record| record.summary.status = ThreadStatus::Error);
                if let Err(error) = shown.and(status) {
                    eprintln!("nexus-workbench: {thread_id}: {error}");
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(text: &str) -> Say {
        Say::Steer {
            text: text.into(),
            images: vec![],
            person: true,
        }
    }

    #[test]
    fn keeps_only_a_claude_resumes_message() {
        assert!(Resend::keep(SessionAgent::Claude, true, Some(say("hi")), 3, true).is_some());
        assert!(Resend::keep(SessionAgent::Claude, false, Some(say("hi")), 3, true).is_none());
        assert!(Resend::keep(SessionAgent::Codex, true, Some(say("hi")), 3, true).is_none());
        assert!(Resend::keep(SessionAgent::Claude, true, None, 3, true).is_none());
    }

    #[test]
    fn the_resent_message_carries_the_conversation_first() {
        let resend = Resend::keep(SessionAgent::Claude, true, Some(say("next")), 7, true);
        let (said, seq, person) = resend
            .map(|resend| resend.seeded(Some("<conversation/>\n\n".into())))
            .unwrap_or_else(|| panic!("kept"));
        assert!(matches!(said, Say::Steer { text, .. } if text == "<conversation/>\n\nnext"));
        assert_eq!((seq, person), (7, true));
    }

    #[test]
    fn only_a_resume_that_ended_by_itself_before_hello_is_restarted() {
        assert!(unresumed(true, true, false, true, false));
        // Started before: Claude Code had the conversation.
        assert!(!unresumed(true, true, true, true, false));
        // Stopped, or never spawned.
        assert!(!unresumed(true, false, false, true, false));
        assert!(!unresumed(true, true, false, false, false));
        // A cause already shown (a sign-in) is not fixed by a fresh conversation.
        assert!(!unresumed(true, true, false, true, true));
        // Not a resume: the fresh start itself, which is never retried.
        assert!(!unresumed(false, true, false, true, false));
    }
}
