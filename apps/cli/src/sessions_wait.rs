use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::args::Args;
use crate::error::{CliError, Result, hint_text, to_cli_error, usage};
use crate::safe_text::safe_text;
use crate::sessions::{CLIENT, Desktop, ends_wait, session_of};
use crate::style::Style;
use crate::{Context, Io};

pub(crate) const USAGE: &str = "wait <id|name|title> [--timeout S]";
pub(crate) const COMMAND: &str = "nexus sessions wait <id|name|title> [--timeout S]";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1800);
const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq)]
enum Outcome {
    Finished,
    Failed,
    Stopped,
    Unreadable(CliError),
    Approval,
    TimedOut,
}

impl Outcome {
    fn report(self, io: &dyn Io, style: &Style, id: &str, timeout: Duration) -> i32 {
        let id = safe_text(&id.replace(['\r', '\n'], " "));
        let (code, reason) = match self {
            Self::Finished => (0, "finished".to_string()),
            Self::Failed => (1, "failed".to_string()),
            Self::Stopped => (1, "was stopped".to_string()),
            Self::Unreadable(error) => {
                let message = safe_text(&error.message);
                io.err(&format!("session {id} could not be read: {message}"));
                if let Some(hint) = error.hint {
                    io.err(&hint_text(style, &hint));
                }
                return 2;
            }
            Self::Approval => (3, "needs approval in the app".to_string()),
            Self::TimedOut => (124, format!("did not finish in {} seconds", timeout.as_secs())),
        };
        let line = format!("session {id} {reason}");
        if code == 0 {
            io.out(&line);
        } else {
            io.err(&line);
        }
        code
    }
}

#[derive(Debug, PartialEq)]
enum Poll {
    Waiting,
    Busy,
    Exit(Outcome),
}

fn classify(shown: &Value, was_busy: bool) -> Option<Poll> {
    use Outcome::{Approval, Failed, Finished, Stopped};
    Some(match shown["status"].as_str()? {
        "starting" | "running" => Poll::Busy,
        "approval" => Poll::Exit(Approval),
        "error" => Poll::Exit(Failed),
        "interrupted" => Poll::Exit(Stopped),
        "idle" => match shown["turns"].as_array()?.last() {
            None if was_busy => Poll::Exit(Finished),
            None => Poll::Waiting,
            Some(turn) => match turn["status"].as_str()? {
                "completed" => Poll::Exit(Finished),
                "error" => Poll::Exit(Failed),
                "interrupted" => Poll::Exit(Stopped),
                "approval" => Poll::Exit(Approval),
                "running" => Poll::Waiting,
                _ => return None,
            },
        },
        _ => return None,
    })
}

fn wait(
    timeout: Duration,
    interval: Duration,
    mut read: impl FnMut(bool) -> std::result::Result<Value, CliError>,
    mut sleep: impl FnMut(Duration),
    elapsed: impl Fn() -> Duration,
) -> Outcome {
    let mut was_busy = false;
    let mut failed_reads = 0;
    let mut final_done = false;
    loop {
        final_done |= elapsed() >= timeout;
        let poll = read(final_done).and_then(|shown| {
            classify(&shown, was_busy).ok_or_else(|| CliError::new("Nexus for desktop's answer did not read."))
        });
        match poll {
            Ok(Poll::Exit(outcome)) => return outcome,
            Ok(next) => {
                was_busy |= next == Poll::Busy;
                failed_reads = 0;
            }
            Err(error) => {
                failed_reads += 1;
                if ends_wait(&error) || failed_reads == 3 {
                    return Outcome::Unreadable(error);
                }
            }
        }
        let remaining = timeout.saturating_sub(elapsed());
        if remaining.is_zero() {
            if final_done {
                return Outcome::TimedOut;
            }
            continue;
        }
        sleep(interval.min(remaining));
    }
}

fn timeout_of(args: &Args) -> Result<Duration> {
    let value = args.flag("timeout");
    if value.is_none() && !args.has("timeout") {
        return Ok(DEFAULT_TIMEOUT);
    }
    if let Some(seconds) = value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|&seconds| seconds > 0)
    {
        return Ok(Duration::from_secs(seconds));
    }
    usage("Give --timeout a positive whole number of seconds.", Some(COMMAND))
}

pub(crate) fn command(ctx: &Context, style: &Style, word: String, args: &Args) -> Result<i32> {
    let timeout = timeout_of(args)?;
    let start = Instant::now();
    let mut desktop = match Desktop::new(ctx, CLIENT) {
        Ok(desktop) => desktop.with_wait_limit(start, timeout),
        Err(error) => {
            let outcome = Outcome::Unreadable(to_cli_error(&error, ""));
            return Ok(outcome.report(ctx.io.as_ref(), style, &word, timeout));
        }
    };
    let mut session = None;
    let outcome = wait(
        timeout,
        ctx.poll_ms.map_or(POLL_INTERVAL, Duration::from_millis),
        |final_read| {
            if final_read {
                desktop.start_final_read();
            }
            let mut read = || -> Result<Value> {
                if session.is_none() {
                    session = Some(session_of(&desktop, &word)?);
                }
                let shown = desktop.call("show_session", json!({"session_id": session, "turns": 1}))?;
                if let Some(id) = shown["id"].as_str() {
                    session = Some(id.to_string());
                }
                Ok(shown)
            };
            read().map_err(|error| to_cli_error(&error, ""))
        },
        std::thread::sleep,
        || start.elapsed(),
    );
    Ok(outcome.report(ctx.io.as_ref(), style, session.as_deref().unwrap_or(&word), timeout))
}

#[cfg(test)]
#[path = "sessions_wait_tests.rs"]
mod tests;
