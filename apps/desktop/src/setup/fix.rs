//! What a setup line's button does: open the exact row, do the step, or ask the people who can.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::{App, SharedString, Window};
use nexus_workbench::event::SessionAgent;

use crate::pages::PageId;

/// The sign-in a fix starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// Nexus itself: the browser sign-in this window shows when signed out.
    Nexus,
    /// A coding agent's plan: Settings › Agents and plans, with the agent's browser sign-in started.
    Agent(SessionAgent),
    /// A remote server's Nexus sign-in: Settings › Remote servers, with its sign-in started.
    Server(String),
}

/// A command the person runs in the Workbench terminal, where they see it and answer its prompts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalStep {
    pub command: String,
    /// The folder it runs in; `None` is the Workbench's own folder (the session's project).
    pub cwd: Option<PathBuf>,
    /// The terminal's tab name; `None` names it after the command.
    pub name: Option<String>,
}

impl TerminalStep {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            name: None,
        }
    }
}

/// What blocks a member that only the organization's owners can change (`POST /v1/notifications/ask-owner`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillingBlock {
    UsageLimit,
    CreditsOut,
    FreeStartUsed,
    PaymentFailed,
    PastDue,
    Billing,
}

impl BillingBlock {
    /// The `block` Gateway reads.
    pub fn wire(self) -> &'static str {
        match self {
            BillingBlock::UsageLimit => "usage_limit",
            BillingBlock::CreditsOut => "credits_out",
            BillingBlock::FreeStartUsed => "free_start_used",
            BillingBlock::PaymentFailed => "payment_failed",
            BillingBlock::PastDue => "past_due",
            BillingBlock::Billing => "billing",
        }
    }
}

/// A question to the people who can fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    /// The permission the fix needs, which names who is asked (`access::who_can`).
    pub needs: Option<&'static str>,
    /// What the person sends: copied to the clipboard when Gateway has no request for it.
    pub message: String,
    /// A billing block goes to the owners' inboxes through Gateway; anything else is copied.
    pub billing: Option<BillingBlock>,
    /// Up to two people who can, by name; empty names "an admin" or "an owner". The line fills it.
    pub who: Vec<String>,
}

impl Ask {
    pub fn new(needs: Option<&'static str>, message: impl Into<String>) -> Self {
        Self {
            needs,
            message: message.into(),
            billing: None,
            who: Vec::new(),
        }
    }

    pub fn billing(mut self, block: BillingBlock) -> Self {
        self.billing = Some(block);
        self
    }
}

/// A step a call site runs itself ([`Fix::Do`]).
pub type Step = Rc<dyn Fn(&mut Window, &mut App)>;

/// One fix. A line resolves it for the person (`access::decide`): who lacks `needs` gets [`Fix::Ask`] instead.
#[derive(Clone)]
pub enum Fix {
    /// Open a page of the app on the row `focus`, a `SettingsFields::field` anchor the page registers. Settings opened
    /// this way show "Back to session".
    Open {
        page: PageId,
        focus: Option<&'static str>,
        needs: Option<&'static str>,
    },
    SignIn(SignIn),
    /// Run a command in the Workbench terminal and watch its exit (`workspace/side_pane/fix_terminal.rs`).
    RunInTerminal(TerminalStep),
    /// This window takes the Code sessions over from the app that runs them (`Workspace::take_over`).
    TakeOver,
    Ask(Ask),
    /// A guide on the web, when nobody in the app can fix it.
    Guide(SharedString),
    /// A step only the call site knows (pick another payer, run on this computer). `needs` as in [`Fix::Open`].
    Do {
        run: Step,
        needs: Option<&'static str>,
    },
}

impl Fix {
    pub fn open(page: PageId) -> Self {
        Fix::Open {
            page,
            focus: None,
            needs: None,
        }
    }

    /// Open `page` on its row `focus`.
    pub fn open_at(page: PageId, focus: &'static str) -> Self {
        Fix::Open {
            page,
            focus: Some(focus),
            needs: None,
        }
    }

    pub fn run(command: impl Into<String>) -> Self {
        Fix::RunInTerminal(TerminalStep::new(command))
    }

    pub fn act(run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Fix::Do {
            run: Rc::new(run),
            needs: None,
        }
    }

    /// The same fix, for people who hold `permission` only.
    pub fn needs(mut self, permission: &'static str) -> Self {
        match &mut self {
            Fix::Open { needs, .. } | Fix::Do { needs, .. } => *needs = Some(permission),
            Fix::Ask(ask) => ask.needs = Some(permission),
            _ => {}
        }
        self
    }

    /// The permission it needs, if any.
    pub fn required(&self) -> Option<&'static str> {
        match self {
            Fix::Open { needs, .. } | Fix::Do { needs, .. } => *needs,
            _ => None,
        }
    }
}

impl std::fmt::Debug for Fix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fix::Open { page, focus, needs } => f
                .debug_struct("Open")
                .field("page", page)
                .field("focus", focus)
                .field("needs", needs)
                .finish(),
            Fix::SignIn(target) => f.debug_tuple("SignIn").field(target).finish(),
            Fix::RunInTerminal(step) => f.debug_tuple("RunInTerminal").field(step).finish(),
            Fix::TakeOver => f.write_str("TakeOver"),
            Fix::Ask(ask) => f.debug_tuple("Ask").field(ask).finish(),
            Fix::Guide(url) => f.debug_tuple("Guide").field(url).finish(),
            Fix::Do { needs, .. } => f.debug_struct("Do").field("needs", needs).finish(),
        }
    }
}
