//! The Assistant's setup lines for its sign-in problems (board P4): an ended Nexus sign-in gets Sign in, and the
//! message that was not sent goes again after the sign-in; an app with no agent service, or a sign-in Nexus refuses,
//! is a quiet line. Other problems stay the red notice: they failed.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::prelude::*;

use super::AgentPage;
use crate::agent::Sent;
use crate::setup::{self, SetupLine, Tone};
use crate::workspace::nexus_sign_in::{self as nexus, ASSISTANT_KEY};
use nexus_desktop_core::agent::{self as rules, Problem};

/// Which sign-in problem the pane has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// 401: the sign-in ended. Sign in, and the message goes again.
    Ended,
    /// No agent service for this app: nobody here can fix it.
    Unavailable,
    /// 403: Nexus refused this sign-in.
    Refused,
}

/// The kind of `problem`, by the core's own titles (`nexus_desktop_core::agent::problem`), so it may reword them.
fn kind(problem: &Problem) -> Option<Kind> {
    let title = |status, code| rules::problem(status, code, None).title;
    if problem.title == title(401, None) {
        Some(Kind::Ended)
    } else if problem.title == title(503, Some("agents_api_unconfigured")) {
        Some(Kind::Unavailable)
    } else if problem.title == title(403, None) {
        Some(Kind::Refused)
    } else {
        None
    }
}

/// The line for `problem`, when it is a sign-in problem. `unsent`: the person's message waits to go again.
pub(super) fn line(problem: &Problem, unsent: bool) -> Option<SetupLine> {
    Some(match kind(problem)? {
        Kind::Ended => nexus::line(ASSISTANT_KEY, unsent.then_some(nexus::WAITS)),
        Kind::Unavailable => SetupLine::new("assistant:unavailable", Lucide::Info, format!("{}.", problem.title))
            .why(problem.detail.clone())
            .tone(Tone::Info),
        Kind::Refused => SetupLine::new("assistant:refused", Lucide::Info, format!("{}.", problem.title))
            .why(problem.detail.clone())
            .tone(Tone::Info),
    })
}

impl AgentPage {
    /// The pane's problem is an ended sign-in: after the next one, the message that was not sent goes again.
    pub(super) fn wait_for_sign_in(&mut self, cx: &mut Context<Self>) {
        let ended = self.pane.problem.as_ref().and_then(kind) == Some(Kind::Ended);
        if !ended || setup::state::waiting(&ASSISTANT_KEY.into(), cx) {
            return;
        }
        let page = cx.entity().downgrade();
        setup::on_done(ASSISTANT_KEY, cx, move |window, cx| {
            if let Err(error) = page.update(cx, |page, cx| page.retry(window, cx)) {
                eprintln!("nexus-desktop-gpui: ask again after sign-in: {error}");
            }
        });
    }

    /// The person's last message was not sent.
    pub(super) fn unsent(&self) -> bool {
        self.pane.sent == Some(Sent::NotSent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ended_sign_in_offers_sign_in_and_says_the_message_waits() {
        let ended = rules::problem(401, None, None);
        assert_eq!(kind(&ended), Some(Kind::Ended));
        let line = line(&ended, true).expect("a line");
        assert_eq!(line.key().as_ref(), ASSISTANT_KEY);
    }

    #[test]
    fn no_agent_service_and_a_refusal_are_quiet_and_other_problems_stay_failures() {
        assert_eq!(
            kind(&rules::problem(503, Some("agents_api_unconfigured"), None)),
            Some(Kind::Unavailable)
        );
        assert_eq!(
            kind(&rules::problem(403, None, Some("No access."))),
            Some(Kind::Refused)
        );
        for status in [429, 502, 503, 504] {
            assert_eq!(kind(&rules::problem(status, None, None)), None, "{status} failed");
        }
    }
}
