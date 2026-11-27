//! Setup that blocks an action: a missing key, plans off, an agent not installed, a sign-in, a budget, a role. One
//! part says what is missing and opens the fix (`SetupLine`), who can't fix it asks who can (`access`), and the
//! blocked action resumes when the fix lands (`state::on_done`).
//! Each blocker is one line of the same shape.
//!
//! A call site builds a [`SetupLine`] each frame with its [`Fix`]es and draws it with [`line::slot`] and its view's
//! [`Fixer`]: `Workspace::fixer` in the workspace, `Ctx::fixer` in a page.

// The call sites land one by one. Drop this once the first one does.
#![allow(dead_code, unused_imports)]

pub mod access;
pub mod fix;
pub mod line;
pub mod locked;
pub mod plans_off;
pub mod quiet;
pub mod state;

use std::rc::Rc;

use gpui_kit::{App, SharedString, Window};

pub use access::{Access, OrgAccess};
pub use fix::{Ask, BillingBlock, Fix, SignIn, TerminalStep};
pub use line::{SetupLine, Tone, slot};
pub use locked::{Locked, role_notice};
pub use state::{Run, done, forget, on_done};

/// The blocked action a line stands for, named by its call site: `"start:no-key"`, `"pr:gh-auth"`.
pub type FixKey = SharedString;

type RunFix = Rc<dyn Fn(FixKey, Fix, &mut Window, &mut App)>;

/// Runs a line's fix in the window that drew it, for the organization it is about.
#[derive(Clone)]
pub struct Fixer {
    org: Option<String>,
    run: RunFix,
}

impl Fixer {
    pub fn new(org: Option<String>, run: impl Fn(FixKey, Fix, &mut Window, &mut App) + 'static) -> Self {
        Self { org, run: Rc::new(run) }
    }

    /// A fixer that does nothing: for a page's tests.
    #[cfg(test)]
    pub fn inert() -> Self {
        Self::new(None, |_, _, _, _| {})
    }

    pub fn run(&self, key: FixKey, fix: Fix, window: &mut Window, cx: &mut App) {
        (self.run)(key, fix, window, cx);
    }

    /// The same fixer, with `hook` run first on each press: where a call site registers what resumes ([`on_done`]) as
    /// the person presses the fix.
    pub fn before(&self, hook: impl Fn(&FixKey, &Fix, &mut Window, &mut App) + 'static) -> Self {
        let run = self.run.clone();
        Self {
            org: self.org.clone(),
            run: Rc::new(move |key, fix, window, cx| {
                hook(&key, &fix, window, cx);
                run(key, fix, window, cx);
            }),
        }
    }

    /// The person's access in the organization, empty before it is known.
    pub fn access(&self, cx: &App) -> OrgAccess {
        self.org.as_deref().map(|org| Access::of(org, cx)).unwrap_or_default()
    }
}
