//! The measured send for a measure run (`crate::measure`): once the window
//! is interactive, make a chat session, send one message through the workspace's own path, note when its first token
//! reaches the UI, send a second on the warm connection, and write the marks. When measure seeded a session and a page,
//! it then switches to that session and back, as a sidebar click does, and opens the page, as the Library's Open does.
//! Without them it deletes the measured session; with `NEXUS_DESKTOP_MEASURE_PAGES` set, it then opens those pages one
//! by one.

use std::time::{Duration, Instant};

use gpui_kit::*;
use nexus_desktop_core::metrics;
use serde_json::json;

use super::Workspace;
use crate::gateway::Verb;
use crate::model::{PageBlock, SessionSummary};
use crate::pages::fetch;

/// The two measured prompts.
const PROMPT: &str = "Say hello in five words.";
const PROMPT_2: &str = "And again, in three.";

/// Where the measured send is. `seen` is whether the turn's first token came. `Switching` opens `to`; `other` is the
/// seeded session the next stage opens.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) enum MeasureStep {
    #[default]
    Waiting,
    Creating,
    First {
        turn: u64,
        seen: bool,
    },
    Second {
        turn: u64,
        seen: bool,
    },
    Switching {
        to: String,
        other: String,
        stage: Stage,
    },
    Done,
}

/// The measured switches: open the first seeded session untimed, switch to the second (never opened in this launch,
/// so read from the Gateway), then back to the first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Stage {
    Prime,
    Cold,
    Back,
}

impl Workspace {
    /// After the account or the sessions arrive: with both, the window is interactive, and the measured send starts.
    pub(super) fn measure_ready(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !crate::measure::on()
            || self.measure != MeasureStep::Waiting
            || self.account.is_none()
            || !self.sessions_loaded
        {
            return;
        }
        crate::measure::mark(metrics::INTERACTIVE);
        self.measure = MeasureStep::Creating;
        let body = json!({ "title": crate::model::title_from(PROMPT) });
        let request = account_gateway.request(Verb::Post, "/v1/chat/sessions".into(), Some(body));
        fetch(cx, request, |this, made, cx| {
            let id = made.map(|made| crate::model::text(&made, "id").unwrap_or_default());
            match id {
                Ok(id) if !id.is_empty() => {
                    this.current = Some(id);
                    crate::measure::mark(metrics::SEND);
                    this.ask(PROMPT.into(), Default::default(), cx);
                    this.measure = MeasureStep::First {
                        turn: this.turn,
                        seen: false,
                    };
                }
                other => {
                    log::warn!("measure: no session: {other:?}");
                    this.measure = MeasureStep::Done;
                    crate::measure::finish();
                }
            }
        });
    }

    /// A delta of `turn` reached the UI: the first one is the first token.
    pub(super) fn measure_delta(&mut self, turn: u64) {
        match &mut self.measure {
            MeasureStep::First { turn: asked, seen } if *asked == turn && !*seen => {
                *seen = true;
                crate::measure::mark(metrics::FIRST_TOKEN);
            }
            MeasureStep::Second { turn: asked, seen } if *asked == turn && !*seen => {
                *seen = true;
                crate::measure::mark(metrics::FIRST_TOKEN_2);
            }
            _ => {}
        }
    }

    /// `turn` is answered, or failed: send the second message, or end the measured send.
    pub(super) fn measure_answered(&mut self, turn: u64, success: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !success
            && matches!(self.measure, MeasureStep::First { turn: asked, .. } | MeasureStep::Second { turn: asked, .. } if asked == turn)
        {
            self.measure = MeasureStep::Done;
            crate::measure::mark("measure-failed");
            return self.measure_cleanup(cx);
        }
        match self.measure {
            MeasureStep::First { turn: asked, .. } if asked == turn => {
                crate::measure::mark(metrics::ANSWER_DONE);
                crate::measure::mark(metrics::SEND_2);
                self.ask(PROMPT_2.into(), Default::default(), cx);
                self.measure = MeasureStep::Second {
                    turn: self.turn,
                    seen: false,
                };
            }
            MeasureStep::Second { turn: asked, .. } if asked == turn => {
                self.measure = MeasureStep::Done;
                let Some(id) = self.current.clone() else {
                    return crate::measure::finish();
                };
                if let Some((first, second)) = crate::measure::seeded_sessions() {
                    // Open the first seeded session untimed, so both apps time the switches from a session's view.
                    self.measure_switch(Stage::Prime, first, second, window, cx);
                    return;
                }
                let path = format!("/v1/chat/sessions/{}", nexus_client::url::enc(&id));
                let request = account_gateway.request(Verb::Delete, path, None);
                fetch(cx, request, |this, _, cx| this.measure_pages(cx));
            }
            _ => {}
        }
    }

    /// After the measured send, the page journey: open each page `NEXUS_DESKTOP_MEASURE_PAGES` lists as its card's
    /// click does, one at a time, then end the measure. A page's marks are `page-<n>-<event>`, from 1
    /// (`crate::measure::page_mark`); the next page opens once this one is usable, or after 15 s, and its dwell.
    fn measure_pages(&mut self, cx: &mut Context<Self>) {
        let pages = crate::measure::pages();
        if pages.is_empty() {
            return crate::measure::finish();
        }
        let (hover, dwell) = (crate::measure::page_hover(), crate::measure::page_dwell());
        cx.spawn(async move |this, cx| {
            for (index, (id, version)) in pages.into_iter().enumerate() {
                let step = index + 1;
                crate::measure::set_page_step(step);
                if hover > 0 {
                    crate::measure::page_mark("hover");
                    let (gateway, id) = (this.update(cx, |this, _| this.account_gateway()), id.clone());
                    if let Ok(Some(gateway)) = gateway {
                        crate::web::warm();
                        gateway.prefetch_frame_page_version(id, version);
                    }
                    cx.background_executor().timer(Duration::from_millis(hover)).await;
                }
                let block = PageBlock {
                    title: format!("{id} v{version}"),
                    page_id: id,
                    version,
                    format: "html".into(),
                };
                let opened = this.update(cx, |this, cx| {
                    crate::measure::page_mark("click");
                    this.open_page_block(&block, false, cx);
                });
                if opened.is_err() {
                    break;
                }
                let usable = format!("page-{step}-usable");
                let give_up = Instant::now() + Duration::from_secs(15);
                while !crate::measure::marked(&usable) && Instant::now() < give_up {
                    cx.background_executor().timer(Duration::from_millis(10)).await;
                }
                cx.background_executor().timer(Duration::from_millis(dwell)).await;
            }
            crate::measure::set_page_step(0);
            crate::measure::finish();
        })
        .detach();
    }

    fn measure_cleanup(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(id) = self.current.clone() else {
            return crate::measure::finish();
        };
        let path = format!("/v1/chat/sessions/{}", nexus_client::url::enc(&id));
        let request = account_gateway.request(Verb::Delete, path, None);
        fetch(cx, request, |_, _, _| crate::measure::finish());
    }

    /// Open `to` as a sidebar click does; `other` is the seeded session the next stage opens.
    fn measure_switch(&mut self, stage: Stage, to: String, other: String, window: &mut Window, cx: &mut Context<Self>) {
        match stage {
            Stage::Prime => {}
            Stage::Cold => crate::measure::mark(metrics::SWITCH),
            Stage::Back => crate::measure::mark(metrics::SWITCH_BACK),
        }
        let session = self.measure_summary(to.clone());
        self.measure = MeasureStep::Switching { to, other, stage };
        self.open(&session, window, cx);
    }

    /// A session's messages are in `entries` (`Event::Opened`): when it is the one the measured switch opened, mark
    /// the frame after the next drawn one, as the first frame is marked, then take the next step.
    pub(super) fn measure_opened(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let MeasureStep::Switching { to, stage, .. } = &self.measure else {
            return;
        };
        if to != id {
            return;
        }
        let done = match stage {
            Stage::Prime => None,
            Stage::Cold => Some(metrics::SWITCH_DONE),
            Stage::Back => Some(metrics::SWITCH_BACK_DONE),
        };
        let this = cx.entity().downgrade();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                if let Some(done) = done {
                    crate::measure::mark(done);
                }
                let _ = this.update(cx, |this, cx| this.measure_switched(window, cx));
            })
        });
    }

    /// A switch is on screen: take the next one (the cold switch to the second session, then back to the first), then
    /// open the seeded page beside the first, then end.
    fn measure_switched(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let MeasureStep::Switching { to, other, stage } = std::mem::take(&mut self.measure) else {
            return;
        };
        match stage {
            Stage::Prime => return self.measure_switch(Stage::Cold, other, to, window, cx),
            Stage::Cold => return self.measure_switch(Stage::Back, other, to, window, cx),
            Stage::Back => {}
        }
        self.measure = MeasureStep::Done;
        let Some(page) = crate::measure::seeded_page() else {
            return crate::measure::finish();
        };
        crate::measure::mark(metrics::PAGE_OPEN);
        crate::measure::arm_page();
        let reference = crate::artifact_refs::ArtifactRef {
            title: "Measured page".into(),
            kind: "Doc".into(),
            target: crate::artifact_refs::Target::Page(page),
            version: None,
            link: None,
        };
        self.open_artifact_ref(&reference, cx);
    }

    /// The sidebar's row for `id`, as a click passes it; a bare one when the list does not have it.
    fn measure_summary(&self, id: String) -> SessionSummary {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
            .unwrap_or(SessionSummary {
                id,
                title: String::new(),
                day: None,
                at_ms: None,
            })
    }
}
