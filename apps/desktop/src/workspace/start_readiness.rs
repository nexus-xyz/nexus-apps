//! One bounded, organization-scoped readiness snapshot for start blockers and send gates.
use super::{Auth, View, Workspace};
use gpui_kit::*;
use nexus_client::local_agents::LocalAgent;
use nexus_desktop_core::session_readiness::Budget;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(60);

#[derive(Clone, Default, PartialEq, Eq)]
enum ReadinessScope {
    #[default]
    None,
    Local,
    Account(String),
}

#[derive(Default)]
pub(super) struct Readiness {
    scope: ReadinessScope,
    pub budget: Budget,
    pub local: Option<Result<Vec<LocalAgent>, String>>,
    /// The account of the Claude login that reads signed out but still names one: it was lost.
    pub lost_claude: Option<String>,
    pub checked: Option<Instant>,
    pub local_checked: Option<Instant>,
    pub checking: Option<Task<()>>,
    pub local_checking: Option<Task<()>>,
}

impl Workspace {
    pub(super) fn probe_readiness(&mut self, cx: &mut Context<Self>) {
        // UI tests inject snapshots; no real vendor commands or network run from deterministic tests.
        if cfg!(test) {
            return;
        }
        let local_only = self.is_local();
        let scope = if local_only {
            ReadinessScope::Local
        } else if matches!(self.auth, Auth::SignedIn)
            && let Some(account) = &self.account
        {
            ReadinessScope::Account(format!("{}:{}", self.gateway.url(), account.org))
        } else {
            return;
        };
        if self.readiness.scope != scope {
            self.readiness = Readiness {
                scope: scope.clone(),
                ..Readiness::default()
            };
        }
        if matches!(self.view, View::NewCode)
            && self.readiness.local_checking.is_none()
            && self.readiness.local_checked.is_none_or(|at| at.elapsed() >= REFRESH)
        {
            self.readiness.local_checked = Some(Instant::now());
            let local = crate::code_run::read_local_logins();
            let local_scope = scope.clone();
            self.readiness.local_checking = Some(cx.spawn(async move |this, cx| {
                let local = local
                    .await
                    .unwrap_or_else(|_| Err("The local sign-in check was lost.".into()));
                let _ = this.update(cx, |this, cx| {
                    if this.readiness.scope != local_scope {
                        return;
                    }
                    this.readiness.local_checking = None;
                    let (local, lost) = match local {
                        Ok((agents, lost)) => (Ok(agents), lost),
                        Err(error) => (Err(error), None),
                    };
                    this.readiness.local = Some(local);
                    this.readiness.lost_claude = lost;
                    cx.notify();
                });
            }));
        }
        if local_only
            || self.readiness.checking.is_some()
            || self.readiness.checked.is_some_and(|at| at.elapsed() < REFRESH)
        {
            return;
        }
        self.readiness.checked = Some(Instant::now());
        let Some(account) = self.account_gateway() else { return };
        let spend = account.start_budget_snapshot();
        let deadline = cx.background_executor().timer(Duration::from_secs(25));
        self.readiness.checking = Some(cx.spawn(async move |this, cx| {
            let spend = match futures::future::select(spend, Box::pin(deadline)).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err("The budget check timed out.".into()),
            };
            let _ = this.update(cx, |this, cx| {
                if this.readiness.scope != scope {
                    return;
                }
                this.readiness.checking = None;
                if let Ok(spend) = spend {
                    this.readiness.budget = Budget::read(&spend);
                }
                // A raised budget sends the message held for it (`limit_line.rs`).
                this.limit_check(cx);
                cx.notify();
            });
        }));
    }

    pub(super) fn recheck_readiness(&mut self, cx: &mut Context<Self>) {
        self.readiness.checked = None;
        self.readiness.local_checked = None;
        self.probe_readiness(cx);
        self.refresh_harness(cx);
    }

    pub(super) fn budget_line(&self) -> Option<String> {
        let gateway = match &self.view {
            View::Code(_) => self
                .code_session()
                .is_some_and(|session| session.via == nexus_workbench::event::Via::Nexus),
            View::NewCode => self.code_form.effective_via() == nexus_workbench::event::Via::Nexus,
            View::Chat => true,
            _ => false,
        };
        self.budget_line_for(gateway)
    }

    /// The used-up budget's line for a send that pays through Gateway (`gateway`), or None when nothing blocks it. A
    /// side chat asks for its own payer (`side_chat/budget.rs`).
    pub(super) fn budget_line_for(&self, gateway: bool) -> Option<String> {
        let account = self.account.as_ref()?;
        if self.readiness.scope != ReadinessScope::Account(format!("{}:{}", self.gateway.url(), account.org)) {
            return None;
        }
        if !self.readiness.budget.blocks_send(gateway) {
            return None;
        }
        self.readiness.budget.line(
            self.account.as_ref().map_or("", |account| account.org_name.as_str()),
            self.budget_day().as_deref(),
        )
    }

    /// The day the used-up budget resets ("Dec 1"), when Gateway said it.
    pub(super) fn budget_day(&self) -> Option<String> {
        match &self.readiness.budget {
            Budget::Exhausted { resets_at: Some(at) } => crate::limit_stop::resume_day(at),
            _ => None,
        }
    }

    /// Tests: the readiness snapshot reads as this account's.
    #[cfg(test)]
    pub(super) fn readiness_for_this_account(&mut self) {
        if let Some(account) = &self.account {
            self.readiness.scope = ReadinessScope::Account(format!("{}:{}", self.gateway.url(), account.org));
        }
    }

    /// The readiness snapshot is this account's.
    pub(super) fn readiness_current(&self) -> bool {
        self.account.as_ref().is_some_and(|account| {
            self.readiness.scope == ReadinessScope::Account(format!("{}:{}", self.gateway.url(), account.org))
        })
    }
}
