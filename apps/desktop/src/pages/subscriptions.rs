//! The Claude and ChatGPT plan accounts on this computer: what is left of each plan,
//! as Claude Code and Codex report it, and the actions on them. Settings › Agents and plans draws them
//! (`agents/accounts.rs`). The data is local, not Gateway's: each login is probed with `nexus_workbench::plans`,
//! with the agent started without a prompt, so no model call is made.
//!
//! Extra accounts are folders (`<data dir>/accounts/<agent>/<id>`), the same ones the Nexus app uses.
//! Sign in and Add account run the vendor's own login in Nexus (`sign_in.rs`). Remove signs the account out and deletes
//! its folder, after a second press. While it runs the row says "Removing…"; when it ends the row folds away and a
//! note under the card says what changed, or the row shows why it stayed. Use a reset spends one of the free resets
//! Codex banks on a login, also after a second press.
//!
//! The store owner saves the active account and the switching choice of each agent, then updates its running
//! workbench. "Having trouble?" offers the vendor's login in a terminal, with the shared login scripts.

pub(crate) mod account_controls;
pub(crate) mod login;
#[cfg(test)]
mod removal_tests;
pub(crate) mod sign_in;
#[cfg(test)]
mod sign_in_tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use futures::future::LocalBoxFuture;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::accounts;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::harness::{PlanDepth, PlanProbe, harness};
use nexus_workbench::plans::{self, Limit, PlanUsage};
use nexus_workbench::process::{self, Spawn};

use super::{DeviceCtx, Load};

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// At or under this much left, a meter turns strong: the plan is nearly out.
pub(crate) const LOW_LEFT: i64 = 15;
/// How often the page probes again while it is open, as the web page does.
const REFRESH: Duration = Duration::from_secs(60);

/// How soon an open page shows a running session's reported limits.
const BOOK_TICK: Duration = Duration::from_secs(1);

/// Whose plan an agent runs on.
pub(crate) fn vendor(agent: SessionAgent) -> &'static str {
    agent.coding().map_or("Nexus", |agent| agent.entry().identity.plan)
}

/// Who pays a session on the person's own plan (`Your Google AI plan`), as its facts and the agent switch name it.
pub(crate) fn plan_payer(agent: SessionAgent) -> String {
    agent
        .configured_provider()
        .map_or_else(|| format!("Your {} plan", vendor(agent)), String::from)
}

/// What to pick in the agent's sign-in so the login bills the plan.
pub(crate) fn sign_in_hint(agent: SessionAgent) -> &'static str {
    crate::agents::ui(agent).sign_in_hint()
}

/// The plan's name: Claude Code names its plans (`Claude Team`), Codex only the tier (`plus`), and agy none.
pub(crate) fn plan_name(plan: &PlanUsage) -> String {
    crate::agents::ui(plan.agent).plan_name(plan.plan.as_deref())
}

/// A Codex tier as a person says it: `plus` is Plus, `self_serve_business_prolite` is Business.
pub(crate) fn tier_name(tier: &str) -> String {
    let lower = tier.to_lowercase();
    if lower.contains("business") {
        return "Business".into();
    }
    lower
        .split('_')
        .filter(|word| !word.is_empty() && !matches!(*word, "self" | "serve"))
        .map(|word| {
            let mut chars = word.chars();
            let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            format!("{first}{}", chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The limit as the design names it: the reader says `Session` and `Weekly`.
pub(crate) fn limit_label(label: &str) -> String {
    if label == "Session" {
        return "This session".into();
    }
    match label.strip_prefix("Weekly") {
        Some(rest) if rest.is_empty() || rest.starts_with(' ') => format!("This week{rest}"),
        _ => label.to_string(),
    }
}

/// A limit's window alone: "This week" for both "Weekly" and "Weekly · Fable".
pub(crate) fn window_label(label: &str) -> String {
    let label = limit_label(label);
    match label.split_once(" · ") {
        Some((window, _)) => window.to_string(),
        None => label,
    }
}

/// Whole percent used, from 0 to 100.
pub(crate) fn used_pct(limit: &Limit) -> i64 {
    limit.used_percent.round().clamp(0., 100.) as i64
}

/// How long until `at`: `42 min`, `3 h 13 min`, `1 d 13 h`.
pub(crate) fn resets_in(at: i64, now: i64) -> String {
    let minutes = ((at - now) as f64 / 60_000.).ceil().max(1.) as i64;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return if minutes % 60 > 0 {
            format!("{hours} h {} min", minutes % 60)
        } else {
            format!("{hours} h")
        };
    }
    if hours % 24 > 0 {
        format!("{} d {} h", hours / 24, hours % 24)
    } else {
        format!("{} d", hours / 24)
    }
}

/// The name a login shows under.
pub(crate) fn login_name(plan: &PlanUsage) -> String {
    match (&plan.email, &plan.account) {
        (Some(email), _) => email.clone(),
        (None, Some(_)) => format!("{} account", vendor(plan.agent)),
        (None, None) => format!("Your {} login", vendor(plan.agent)),
    }
}

/// One agent's section: whether it is installed, and the logins to show.
#[derive(Debug, PartialEq)]
pub(crate) struct Group<'a> {
    pub agent: SessionAgent,
    /// Extra accounts, and the agent's own login when signed in.
    pub accounts: usize,
    pub rows: Vec<&'a PlanUsage>,
}

/// Whether this page reads `agent`'s plan: it has a section here.
pub(crate) fn reads_plan(agent: SessionAgent) -> bool {
    crate::agents::ui(agent).reads_plan()
}

/// The sections, as the web page builds them. An installed agent always has an entry for its own login; that login,
/// never signed in, shows only while the agent has no other account, as one to connect.
pub(crate) fn groups(plans: &[PlanUsage]) -> Vec<Group<'_>> {
    crate::agents::plan_agents()
        .iter()
        .filter(|agent| plans.iter().any(|plan| plan.agent == **agent && plan.account.is_none()))
        .map(|agent| {
            let shown = |plan: &&PlanUsage| plan.agent == *agent && (plan.signed_in || plan.account.is_some());
            let accounts = plans.iter().filter(shown).count();
            let rows = plans
                .iter()
                .filter(|plan| plan.agent == *agent && (plan.signed_in || plan.account.is_some() || accounts == 0))
                .collect();
            Group {
                agent: *agent,
                accounts,
                rows,
            }
        })
        .collect()
}

/// The sign-in command for a login: the agent's own, with the account's folder in front for an extra account of an
/// agent that reads one.
fn login_command(agent: SessionAgent, dir: Option<&Path>) -> String {
    let login = agent
        .coding()
        .map(|client| client.entry().sign_in.login)
        .unwrap_or("nexus login");
    match dir.zip(accounts::account_variable(agent)) {
        None => login.to_string(),
        Some((dir, variable)) if cfg!(windows) => format!("$env:{variable}='{}'; {login}", dir.display()),
        Some((dir, variable)) => format!(
            "{variable}='{}' {login}",
            dir.display().to_string().replace('\'', r"'\''")
        ),
    }
}

// ---------------------------------------------------------------- probing

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn accounts_root() -> Option<PathBuf> {
    crate::paths::data_dir().map(|dir| dir.join("accounts"))
}

/// This app's environment, with a `PATH` that also has where agents are usually installed.
fn agent_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let home = Some(home()).filter(|home| !home.as_os_str().is_empty());
    env.insert(
        "PATH".into(),
        process::agent_path(
            std::env::var("PATH").ok().as_deref(),
            home.as_deref(),
            &crate::code_run::shell(),
        ),
    );
    env
}

/// [`agent_env`] without what would bill something other than the person's plan.
fn plan_env(agent: SessionAgent) -> HashMap<String, String> {
    let mut env = agent_env();
    for name in agent
        .coding()
        .map_or(&[][..], |client| client.entry().billing.plan_overrides)
    {
        env.remove(*name);
    }
    env
}

/// The agent's profile, whose settings are shared with each new extra account.
fn agent_home(agent: SessionAgent) -> PathBuf {
    accounts::profile_dir(agent, &home(), &|name| std::env::var(name).ok())
}

fn find(agent: SessionAgent) -> Option<PathBuf> {
    let client = agent.coding()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| home());
    nexus_client::agents::find_agent(client, &agent_env(), cfg!(windows), &cwd)
        .ok()
        .flatten()
}

/// Run async work on its own thread and runtime, off the UI thread.
pub(crate) fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, String>> {
    let (send, receive) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        send.send(work()).ok();
    });
    async move {
        receive
            .await
            .map_err(|_| "The check stopped before it answered.".to_string())?
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
}

/// Under the Claude accounts: Make active moves new sessions, and an open session only once its own account is at its
/// limits (the workbench's `rotate`).
pub(crate) const ACTIVE_NOTE: &str = "New Claude sessions start on the Active account. Open sessions keep their \
                                      account until it reaches its limits, then continue on the Active account.";

/// One check's readings so far. A check sends one before any login answers, then one each time a login answers.
pub(crate) struct Progress {
    /// Every login the check probes, in the order the page lists them.
    logins: Vec<(SessionAgent, Option<String>)>,
    /// The logins read so far, in that order.
    read: Vec<PlanUsage>,
}

impl Progress {
    /// What to show now: each login's new reading, else its reading in `before`. A login this check does not probe,
    /// such as a removed account, is dropped.
    pub(crate) fn over(&self, before: &[PlanUsage]) -> Vec<PlanUsage> {
        let same = |plan: &PlanUsage, (agent, account): &(SessionAgent, Option<String>)| {
            plan.agent == *agent && plan.account == *account
        };
        self.logins
            .iter()
            .filter_map(|login| self.read.iter().chain(before).find(|plan| same(plan, login)).cloned())
            .collect()
    }

    /// [`Progress::over`] for check number `check`, with the logins removed since: `removed` holds each one's key
    /// ([`login_key`]) and the number of the last check started before it went. A check that started before the
    /// removal cannot show the login again: the login keeps only its reading in `before`, which is the reading it
    /// folds away with. A later check that reads the login shows it again (it was signed in again) and clears its
    /// mark; one that does not list it at all clears the mark too.
    pub(crate) fn over_removed(
        &self,
        before: &[PlanUsage],
        removed: &mut HashMap<String, u64>,
        check: u64,
    ) -> Vec<PlanUsage> {
        let same = |plan: &PlanUsage, (agent, account): &(SessionAgent, Option<String>)| {
            plan.agent == *agent && plan.account == *account
        };
        let key = |(agent, account): &(SessionAgent, Option<String>)| login_key(*agent, account.as_deref());
        removed.retain(|login, at| {
            check <= *at
                || self
                    .logins
                    .iter()
                    .any(|each| key(each) == *login && !self.read.iter().any(|plan| same(plan, each)))
        });
        self.logins
            .iter()
            .filter_map(|login| {
                let gone = removed.contains_key(&key(login));
                let fresh = self.read.iter().filter(|_| !gone);
                fresh.chain(before).find(|plan| same(plan, login)).cloned()
            })
            .collect()
    }

    /// The readings this check got so far: only these are new.
    pub(crate) fn fresh(&self) -> &[PlanUsage] {
        &self.read
    }

    /// The agents with a login that has not answered yet.
    pub(crate) fn waiting(&self) -> Vec<SessionAgent> {
        crate::agents::plan_agents()
            .into_iter()
            .filter(|agent| {
                self.logins.iter().any(|(each, account)| {
                    each == agent
                        && !self
                            .read
                            .iter()
                            .any(|plan| plan.agent == *each && plan.account == *account)
                })
            })
            .collect()
    }
}

/// Each installed agent's own login, then each extra account, probed all at once. `progress` gets the readings each
/// time a login answers, so a fast agent shows while a slow one still runs. The active Claude account is the
/// choice in `settings.json`, while its folder is there.
pub(crate) fn probe_each(progress: futures::channel::mpsc::UnboundedSender<Progress>) -> Result<(), String> {
    use futures::StreamExt;
    let root = accounts_root();
    let mut logins = vec![];
    let mut probes = vec![];
    for agent in crate::agents::plan_agents() {
        let Some(program) = find(agent) else { continue };
        if let Some(root) = root.as_deref() {
            adopt_profiles(root, agent);
        }
        let env = plan_env(agent);
        let ids = root
            .as_deref()
            .map(|root| accounts::list(root, agent))
            .unwrap_or_default();
        for account in std::iter::once(None).chain(ids.into_iter().map(Some)) {
            let dir = match (&account, &root) {
                (Some(id), Some(root)) => accounts::dir(root, agent, id).ok(),
                _ => None,
            };
            let probe = PlanProbe {
                program: program.clone(),
                env: env.clone(),
                account: account.clone(),
                dir,
                cwd: None,
            };
            if let Some(read) = harness(agent).read_plan(probe, PlanDepth::Limits) {
                logins.push((agent, account));
                probes.push(read);
            }
        }
    }
    let runtime = runtime()?;
    let mut slots: Vec<Option<PlanUsage>> = vec![None; probes.len()];
    let send = |slots: &[Option<PlanUsage>]| {
        let mut read: Vec<PlanUsage> = slots.iter().flatten().cloned().collect();
        // Read each time: Make active during the check changes the active account, and the page has marked the
        // readings it keeps with it.
        let settings = crate::desktop_settings::load();
        note_chosen(&settings);
        mark_active(&mut read, &settings, root.as_deref());
        // The page may have closed; the probes still end on their own.
        let _ = progress.unbounded_send(Progress {
            logins: logins.clone(),
            read,
        });
    };
    send(&slots);
    let mut running: futures::stream::FuturesUnordered<_> = probes
        .into_iter()
        .enumerate()
        .map(|(index, probe)| async move { (index, probe.await) })
        .collect();
    runtime.block_on(async {
        while let Some((index, plan)) = running.next().await {
            slots[index] = Some(plan);
            send(&slots);
        }
    });
    Ok(())
}

/// Lists the agent's logins made outside Nexus (`~/.claude-work` beside `~/.claude`) as linked accounts, so a person
/// with a work and a personal login sees both and picks one. Listing chooses nothing, and a profile the
/// person removed stays removed.
fn adopt_profiles(root: &Path, agent: SessionAgent) {
    for profile in accounts::discover(agent, &home(), &agent_home(agent), &[]) {
        if let Err(error) = accounts::adopt(root, agent, &profile) {
            eprintln!("nexus-desktop-gpui: could not list {}: {error}", profile.display());
        }
    }
}

/// The agents whose plan account the person chose, as the last probe read the settings: New session asks the others to
/// choose when they have more than one login. Kept here so drawing never reads the settings file.
static CHOSEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Keeps the settings' [`Settings::chosen_accounts`](nexus_desktop_core::settings::Settings::chosen_accounts), and the
/// accounts the organizations other than the window's use.
pub(crate) fn note_chosen(settings: &nexus_desktop_core::settings::Settings) {
    if let Ok(mut chosen) = CHOSEN.lock() {
        chosen.clone_from(&settings.chosen_accounts);
    }
    let org = current_org();
    if let Ok(mut claimed) = CLAIMED.lock() {
        *claimed = settings
            .org_accounts
            .iter()
            .filter(|(other, _)| org.as_deref() != Some(other.as_str()))
            .flat_map(|(_, picks)| picks.iter().map(|(agent, pick)| (agent.clone(), pick.clone())))
            .collect();
    }
}

/// The window's organization (its key), whose agent accounts the settings hold now.
static ORG: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The accounts other organizations use, as (agent, account or [`OWN_LOGIN`](nexus_desktop_core::settings::OWN_LOGIN)).
static CLAIMED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

fn current_org() -> Option<String> {
    ORG.lock().ok().and_then(|org| org.clone())
}

/// Whether another organization uses `account` of `agent` (None: its own login): the window's organization does not
/// take it over without asking.
pub(crate) fn claimed_elsewhere(agent: SessionAgent, account: &Option<String>) -> bool {
    let pick = account.as_deref().unwrap_or(nexus_desktop_core::settings::OWN_LOGIN);
    CLAIMED.lock().is_ok_and(|claimed| {
        claimed
            .iter()
            .any(|(each, used)| each == agent.as_str() && used == pick)
    })
}

/// The window left its organization (sign-out, or before a switch): no organization holds the settings' picks until
/// [`enter_org`] runs again.
pub(crate) fn leave_org() {
    if let Ok(mut current) = ORG.lock() {
        *current = None;
    }
}

/// The window entered the organization `org`: new sessions start on the agent accounts it chose, and an agent it chose
/// none for is asked for on New session.
pub(crate) fn enter_org(
    org: String,
) -> impl std::future::Future<Output = Result<nexus_desktop_core::settings::Settings, String>> {
    if let Ok(mut current) = ORG.lock() {
        *current = Some(org.clone());
    }
    off_thread(move || {
        crate::desktop_settings::update(move |settings| {
            settings.enter_org(
                &org,
                &[
                    ("claude", |settings, account| settings.claude_account = account),
                    ("codex", |settings, account| settings.codex_account = account),
                ],
            );
            Ok(())
        })
        .map(|(settings, ())| {
            note_chosen(&settings);
            settings
        })
    })
}

/// New session's "Use <email>": new sessions of `agent` start on `account` (None: its own login), the person's choice,
/// and stay there: automatic switching goes off for the agent, so Nexus never moves them to another login.
pub(crate) fn choose(
    agent: SessionAgent,
    account: Option<String>,
) -> impl std::future::Future<Output = Result<nexus_desktop_core::settings::Settings, String>> {
    off_thread(move || {
        crate::desktop_settings::update(move |settings| {
            let root = accounts_root().ok_or("This computer has no app data folder.")?;
            account_controls::select(settings, &root, agent, account)?;
            crate::agents::ui(agent).set_switches_accounts(settings, false);
            Ok(())
        })
        .map(|(settings, ())| {
            note_chosen(&settings);
            settings
        })
    })
}

/// Whether the person chose `agent`'s plan account themselves.
pub(crate) fn chosen_by_person(agent: SessionAgent) -> bool {
    CHOSEN
        .lock()
        .is_ok_and(|chosen| chosen.iter().any(|name| name == agent.as_str()))
}

/// Every login's reading once all have answered: [`probe_each`] without its progress. The workbench's `rotate` reads
/// it (`code_run.rs`).
pub(crate) fn probe_all() -> Result<Vec<PlanUsage>, String> {
    let (send, progress) = futures::channel::mpsc::unbounded();
    probe_each(send)?;
    // `probe_each` dropped its sender, so the stream ends after the last reading it sent.
    Ok(futures::executor::block_on_stream(progress)
        .last()
        .map(|read| read.over(&[]))
        .unwrap_or_default())
}

/// The account new sessions of `agent` start on, as Settings saved it, while its folder is there.
pub(crate) fn chosen(settings: &nexus_desktop_core::settings::Settings, agent: SessionAgent) -> Option<String> {
    crate::agents::ui(agent).chosen_account(settings)
}

/// Whether sessions of `agent` take the account with the most left near a limit. On by default for Claude only.
pub(crate) fn switches(settings: &nexus_desktop_core::settings::Settings, agent: SessionAgent) -> bool {
    crate::agents::ui(agent).switches_accounts(settings)
}

/// Marks each plan that new sessions of its agent start on.
fn mark_active(plans: &mut [PlanUsage], settings: &nexus_desktop_core::settings::Settings, root: Option<&Path>) {
    for agent in crate::agents::plan_agents() {
        let active =
            chosen(settings, agent).filter(|id| root.is_some_and(|root| accounts::dir(root, agent, id).is_ok()));
        for plan in plans.iter_mut().filter(|plan| plan.agent == agent) {
            plan.active = plan.account == active;
        }
    }
}

/// The folder of a login: None for the agent's own login.
fn login_dir(plan: &PlanUsage) -> Result<Option<PathBuf>, String> {
    let Some(id) = &plan.account else { return Ok(None) };
    let root = accounts_root().ok_or("This computer has no app data folder.")?;
    accounts::dir(&root, plan.agent, id).map(Some)
}

/// The signed-in login new sessions move to when their account goes: the first other one the page lists.
fn replacement<'a>(plans: &'a [PlanUsage], agent: SessionAgent, removed: &Option<String>) -> Option<&'a PlanUsage> {
    plans
        .iter()
        .find(|plan| plan.agent == agent && plan.signed_in && &plan.account != removed)
}

/// When new sessions of `agent` start on the `removed` login, start them on `replacement` instead (None: the agent's
/// own login). Answers whether it changed the choice.
fn move_off(
    settings: &mut nexus_desktop_core::settings::Settings,
    root: Option<&Path>,
    agent: SessionAgent,
    removed: &Option<String>,
    replacement: Option<String>,
) -> Result<bool, String> {
    if chosen(settings, agent) != *removed {
        return Ok(false);
    }
    let root = root.ok_or("This computer has no app data folder.")?;
    account_controls::select(settings, root, agent, replacement)?;
    Ok(true)
}

/// What a removal changed besides the login.
pub(crate) struct Removed {
    /// The settings saved when new sessions started on the removed login and moved to another; the error when that
    /// save failed. None when they started elsewhere, or when this build may not write the settings.
    pub settings: Option<Result<nexus_desktop_core::settings::Settings, String>>,
}

/// Remove the login, then, in the same operation, move new sessions off it when they started on it. A failed settings
/// save does not undo the removal: it is reported with it.
fn remove_login(agent: SessionAgent, id: Option<String>, replacement: Option<String>) -> Result<Removed, String> {
    remove_account(agent, id.clone())?;
    if !crate::desktop_settings::writable() {
        return Ok(Removed { settings: None });
    }
    let root = accounts_root();
    let saved =
        crate::desktop_settings::update(|settings| move_off(settings, root.as_deref(), agent, &id, replacement));
    Ok(Removed {
        settings: match saved {
            Ok((_, false)) => None,
            Ok((settings, true)) => Some(Ok(settings)),
            Err(error) => Some(Err(error)),
        },
    })
}

/// Sign out the original login without deleting the agent's configuration folder.
fn sign_out_original(agent: SessionAgent) -> Result<(), String> {
    let args = crate::agents::ui(agent)
        .sign_out_args()
        .ok_or("This agent has no sign-out command.")?;
    let program = find(agent).ok_or("The agent is not installed on this computer.")?;
    let logout = Spawn {
        program,
        args: args.iter().map(|arg| arg.to_string()).collect(),
        env: plan_env(agent),
        cwd: home(),
    };
    finish_sign_out(&logout)
}

fn finish_sign_out(logout: &Spawn) -> Result<(), String> {
    runtime()?.block_on(async {
        let mut child = process::spawn(logout).map_err(|error| format!("Could not start sign-out: {error}"))?;
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .map_err(|_| "Sign-out timed out. Try again.".to_string())?
            .map_err(|error| format!("Could not finish sign-out: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("The agent could not sign out. Try again.".into())
        }
    })
}

/// Sign the account out of its agent (its stored login goes too), then delete its folder.
fn remove_account(agent: SessionAgent, id: Option<String>) -> Result<(), String> {
    let Some(id) = id else { return sign_out_original(agent) };
    let root = accounts_root().ok_or("This computer has no app data folder.")?;
    // A linked profile is the person's own: Remove forgets it here and never signs it out.
    if accounts::linked(&root, agent, &id).is_some() {
        return accounts::remove(&root, agent, &id);
    }
    let dir = accounts::dir(&root, agent, &id)?;
    // A removal's account id comes from a plan reading, and only Claude Code's and Codex's carry one; both have a
    // sign-out. An agent without one keeps no login to sign out here.
    if let (Some(program), Some(args)) = (find(agent), crate::agents::ui(agent).sign_out_args()) {
        let mut env = plan_env(agent);
        if let Some(variable) = accounts::account_variable(agent) {
            env.insert(variable.into(), dir.to_string_lossy().into_owned());
        }
        let logout = Spawn {
            program,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env,
            cwd: dir,
        };
        // A sign-out that fails or hangs does not keep the folder.
        runtime()?.block_on(async {
            match process::spawn(&logout) {
                Ok(mut child) => {
                    if tokio::time::timeout(Duration::from_secs(10), child.wait())
                        .await
                        .is_err()
                    {
                        eprintln!("nexus-desktop-gpui: signing {id} out did not end; removing it anyway");
                    }
                }
                Err(error) => eprintln!("nexus-desktop-gpui: could not sign {id} out: {error}; removing it anyway"),
            }
        });
    }
    accounts::remove(&root, agent, &id)
}

// ---------------------------------------------------------------- the state

/// Probe every login, sending each [`Progress`]; the future ends when the check does.
type Probe = Rc<dyn Fn(UnboundedSender<Progress>) -> LocalBoxFuture<'static, Result<(), String>>>;
/// Remove a login, moving new sessions to the given account ([`remove_login`]).
type Remove =
    Rc<dyn Fn(SessionAgent, Option<String>, Option<String>) -> LocalBoxFuture<'static, Result<Removed, String>>>;
/// Tells the app the settings changed (`DeviceCtx::settings_saved`).
type SettingsSaved = Rc<dyn Fn(&nexus_desktop_core::settings::Settings, &mut App)>;

/// The work `Plans` hands off the UI thread. Tests give it their own, so no agent runs and no file changes.
#[derive(Clone)]
pub(crate) struct Backend {
    probe: Probe,
    remove: Remove,
    /// The sign-in's work ([`sign_in::Ops`]).
    sign_in: sign_in::Ops,
}

impl Backend {
    /// This computer's agents and settings.
    fn local() -> Backend {
        Backend {
            probe: Rc::new(|send| Box::pin(off_thread(move || probe_each(send)))),
            remove: Rc::new(|agent, id, replacement| {
                Box::pin(off_thread(move || remove_login(agent, id, replacement)))
            }),
            sign_in: sign_in::Ops::local(),
        }
    }
}

#[cfg(test)]
struct FixtureBackend(Backend);

#[cfg(test)]
impl gpui::Global for FixtureBackend {}

#[cfg(test)]
pub(crate) fn fixture_backend(cx: &mut App) {
    cx.set_global(FixtureBackend(Backend {
        probe: Rc::new(|_| Box::pin(async { Ok(()) })),
        remove: Rc::new(|_, _, _| panic!("unexpected login removal")),
        sign_in: sign_in::Ops::inert(),
    }));
}

/// The removal that runs. One runs at a time: it keeps [`Plans::busy`] set.
pub(crate) struct Removal {
    pub agent: SessionAgent,
    pub account: Option<String>,
    /// Which run this is: an answer for another run changes nothing.
    op: u64,
    /// The login's name as the row showed it.
    name: String,
    /// The name of the login new sessions move to, when they started on this one.
    next: Option<String>,
}

impl Removal {
    pub(crate) fn key(&self) -> String {
        login_key(self.agent, self.account.as_deref())
    }
}

/// The agent's name in "Signed … out of Claude".
fn agent_name(agent: SessionAgent) -> &'static str {
    crate::agents::ui(agent).short_name()
}

/// The original login once it has signed out, as the next check reads it.
fn signed_out(plan: &PlanUsage) -> PlanUsage {
    PlanUsage {
        email: None,
        plan: None,
        signed_in: false,
        active: false,
        limits: vec![],
        resets: None,
        error: None,
        ..plan.clone()
    }
}

/// The plan accounts on this computer and what is being done to them. It draws nothing: the Agents and plans page
/// reads it and calls its actions.
pub(crate) struct Plans {
    settings_saved: SettingsSaved,
    backend: Backend,
    pub plans: Load<Vec<PlanUsage>>,
    pub checking: bool,
    /// The number of the last check started.
    check_seq: u64,
    /// A check was asked for while one ran: it runs once that one ends.
    recheck: bool,
    /// The agents whose logins the running check has not read yet.
    pub waiting: Vec<SessionAgent>,
    pub busy: bool,
    pub problem: Option<String>,
    /// The sign-in Nexus runs, or its card's last state ([`sign_in`]).
    pub sign_in: Option<sign_in::SignIn>,
    /// The number of the last sign-in run started.
    sign_in_seq: u64,
    /// The account whose Remove was pressed once.
    pub confirm_remove: Option<(SessionAgent, Option<String>)>,
    /// The login ([`fold_key`]) whose Use a reset was pressed once.
    pub confirm_reset: Option<String>,
    /// What the last reset did, and on which login.
    pub reset_note: Option<(String, String)>,
    /// The attempt of a reset that got no answer, by login. Trying again sends the same one, so Codex cannot spend a
    /// second reset for it.
    reset_attempts: HashMap<String, String>,
    /// What was copied last, so its button says so.
    pub copied: Option<String>,
    /// Rows the person opened (true) or closed (false); the rest follow [`starts_open`].
    folds: HashMap<String, bool>,
    /// The removal that runs.
    pub removal: Option<Removal>,
    /// The number of the last removal started.
    removal_seq: u64,
    /// Why a login could not be removed, by [`login_key`]: shown under its row.
    pub removal_errors: HashMap<String, String>,
    /// The logins removed, by [`login_key`], with the number of the last check started before each went. See
    /// [`Progress::over_removed`]. Its row folds away, then goes.
    pub removed: HashMap<String, u64>,
    /// What the last removal did, under its agent's card. The next action on the card clears it.
    pub removed_note: Option<(SessionAgent, String)>,
}

impl Plans {
    /// Probes the plans now and every minute while the page is open.
    pub(crate) fn new(ctx: &DeviceCtx, cx: &mut App) -> Entity<Plans> {
        cx.new(|cx| {
            let backend = Backend::local();
            #[cfg(test)]
            let backend = cx
                .try_global::<FixtureBackend>()
                .map(|fixture| fixture.0.clone())
                .unwrap_or(backend);
            let mut plans = Plans::with(ctx.settings_saved.clone(), backend);
            plans.check(cx);
            plans.refresh_loop(cx);
            plans
        })
    }

    /// Nothing read yet, and nothing running.
    fn with(settings_saved: SettingsSaved, backend: Backend) -> Plans {
        Plans {
            settings_saved,
            backend,
            plans: Load::Loading,
            checking: false,
            check_seq: 0,
            recheck: false,
            waiting: Vec::new(),
            busy: false,
            problem: None,
            sign_in: None,
            sign_in_seq: 0,
            confirm_remove: None,
            confirm_reset: None,
            reset_note: None,
            reset_attempts: HashMap::new(),
            copied: None,
            folds: HashMap::new(),
            removal: None,
            removal_seq: 0,
            removal_errors: HashMap::new(),
            removed: HashMap::new(),
            removed_note: None,
        }
    }

    /// Read every login again. Asked while a check runs, it runs once more when that one ends.
    pub(crate) fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            self.recheck = true;
            return;
        }
        self.checking = true;
        self.check_seq += 1;
        let number = self.check_seq;
        let probe = self.backend.probe.clone();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let (send, mut progress) = futures::channel::mpsc::unbounded();
            let done = probe(send);
            while let Some(read) = progress.next().await {
                let open = this.update(cx, |page, cx| {
                    let before = match &page.plans {
                        Load::Ready(plans) => plans.as_slice(),
                        _ => &[],
                    };
                    let mut plans = read.over_removed(before, &mut page.removed, number);
                    // A new account's row stays while it signs in, even before a check lists its folder.
                    if let Some(key) = page.hidden()
                        && !plans.iter().any(|plan| fold_key(plan) == key)
                        && let Some(row) = before.iter().find(|plan| fold_key(plan) == key)
                    {
                        plans.push(row.clone());
                    }
                    page.waiting = read.waiting();
                    // An older or limitless reading does not replace newer limits (`plan_book.rs`).
                    crate::plan_book::learn(read.fresh());
                    page.plans = Load::Ready(crate::plan_book::shown(&plans));
                    cx.notify();
                });
                if open.is_err() {
                    return;
                }
            }
            let done = done.await;
            this.update(cx, |page, cx| {
                page.checking = false;
                page.waiting.clear();
                match done {
                    Ok(()) => {}
                    // A failed check keeps the plans read.
                    Err(error) if !matches!(page.plans, Load::Ready(_)) => page.plans = Load::Failed(error.into()),
                    Err(error) => page.problem = Some(error),
                }
                if std::mem::take(&mut page.recheck) {
                    page.check(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Probe again every minute while the page is open, and show a running session's reported limits within a
    /// second (`plan_book.rs`). The loops end with the page.
    fn refresh_loop(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::wakeups::pace(REFRESH, cx)).await;
                if this.update(cx, |page, cx| page.check(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let mut seen = crate::plan_book::changes();
            loop {
                cx.background_executor()
                    .timer(crate::wakeups::pace(BOOK_TICK, cx))
                    .await;
                let changes = crate::plan_book::changes();
                if this.upgrade().is_none() {
                    break;
                }
                if changes == seen {
                    continue;
                }
                seen = changes;
                let open = this.update(cx, |page, cx| {
                    if let Load::Ready(plans) = &mut page.plans {
                        *plans = crate::plan_book::shown(plans);
                        cx.notify();
                    }
                });
                if open.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// New sessions of `agent` start on `account` (None: the agent's own login).
    pub(crate) fn use_account(&mut self, agent: SessionAgent, account: Option<String>, cx: &mut Context<Self>) {
        self.save_controls(
            move |settings| {
                let root = accounts_root().ok_or("This computer has no app data folder.")?;
                account_controls::select(settings, &root, agent, account)
            },
            cx,
        );
    }

    /// Whether sessions of `agent` take the account with the most left near a limit.
    pub(crate) fn rotate(&mut self, agent: SessionAgent, on: bool, cx: &mut Context<Self>) {
        self.save_controls(
            move |settings| {
                crate::agents::ui(agent).set_switches_accounts(settings, on);
                Ok(())
            },
            cx,
        );
    }

    /// Serialize changes and keep settings I/O off the UI thread.
    fn save_controls(
        &mut self,
        change: impl FnOnce(&mut nexus_desktop_core::settings::Settings) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.problem = None;
        self.removed_note = None;
        cx.spawn(async move |this, cx| {
            let saved =
                off_thread(move || crate::desktop_settings::update(change).map(|(settings, ())| settings)).await;
            let _ = this.update(cx, |page, cx| {
                page.busy = false;
                match saved {
                    Ok(settings) => {
                        (page.settings_saved)(&settings, cx);
                        if let Load::Ready(plans) = &mut page.plans {
                            mark_active(plans, &settings, accounts_root().as_deref());
                        }
                    }
                    Err(error) => page.problem = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Remove a login, on the second press. A press while something runs does nothing.
    pub(crate) fn remove(&mut self, agent: SessionAgent, id: Option<String>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let target = (agent, id.clone());
        if self.confirm_remove.as_ref() != Some(&target) {
            self.confirm_remove = Some(target);
            self.removed_note = None;
            cx.notify();
            return;
        }
        self.start_removal(agent, id, cx);
    }

    /// Try a failed removal again. It was confirmed once, so it starts at once.
    pub(crate) fn retry_remove(&mut self, agent: SessionAgent, id: Option<String>, cx: &mut Context<Self>) {
        if !self.busy {
            self.start_removal(agent, id, cx);
        }
    }

    /// Hide why a login could not be removed.
    pub(crate) fn dismiss_removal_error(&mut self, agent: SessionAgent, id: Option<&str>, cx: &mut Context<Self>) {
        self.removal_errors.remove(&login_key(agent, id));
        cx.notify();
    }

    /// The row says "Removing…" until the removal answers ([`Plans::finish_removal`]).
    fn start_removal(&mut self, agent: SessionAgent, id: Option<String>, cx: &mut Context<Self>) {
        let key = login_key(agent, id.as_deref());
        self.confirm_remove = None;
        self.removed_note = None;
        self.removal_errors.remove(&key);
        self.removal_seq += 1;
        let op = self.removal_seq;
        let plans = match &self.plans {
            Load::Ready(plans) => plans.as_slice(),
            _ => &[],
        };
        let name = plans
            .iter()
            .find(|plan| fold_key(plan) == key)
            .map(login_name)
            .unwrap_or_else(|| format!("This {} account", vendor(agent)));
        let next = replacement(plans, agent, &id);
        let (next_account, next) = (next.and_then(|plan| plan.account.clone()), next.map(login_name));
        self.removal = Some(Removal {
            agent,
            account: id.clone(),
            op,
            name,
            next,
        });
        self.busy = true;
        let removing = (self.backend.remove)(agent, id, next_account);
        cx.spawn(async move |this, cx| {
            let removed = removing.await;
            this.update(cx, |page, cx| page.finish_removal(op, removed, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    /// The removal `op` answered. Removed: its row folds away, the card says what changed, and the page reads again.
    /// Not removed: the row stays as it was and says why. An answer for another run changes nothing.
    fn finish_removal(&mut self, op: u64, removed: Result<Removed, String>, cx: &mut Context<Self>) {
        let Some(removal) = self.removal.take_if(|removal| removal.op == op) else {
            return;
        };
        self.busy = false;
        let key = removal.key();
        match removed {
            Ok(removed) => {
                let mut note = match removal.account {
                    Some(_) => format!("Removed {} from this computer.", removal.name),
                    None => {
                        let agent = agent_name(removal.agent);
                        format!(
                            "Signed {} out of {agent} on this computer. Your {agent} settings stay.",
                            removal.name
                        )
                    }
                };
                match removed.settings {
                    Some(Ok(settings)) => {
                        (self.settings_saved)(&settings, cx);
                        if let Load::Ready(plans) = &mut self.plans {
                            mark_active(plans, &settings, accounts_root().as_deref());
                        }
                        if let Some(next) = &removal.next {
                            note.push_str(&format!(" New sessions start on {next}."));
                        }
                    }
                    Some(Err(error)) => note.push_str(&format!(
                        " Nexus could not move new sessions to another account: {error}"
                    )),
                    None => {}
                }
                self.removed.insert(key.clone(), self.check_seq);
                self.removed_note = Some((removal.agent, note));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(crate::motion::COLLAPSE).await;
                    this.update(cx, |page, cx| page.drop_removed(&key, cx)).ok();
                })
                .detach();
                self.check(cx);
            }
            Err(error) => {
                self.removal_errors
                    .insert(key, format!("{} is still on this computer. {error}", removal.name));
            }
        }
        cx.notify();
    }

    /// A removed row has folded away: an extra account goes, and the agent's own login reads as signed out until the
    /// next check reads it.
    fn drop_removed(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.removed.contains_key(key) {
            return;
        }
        if let Load::Ready(plans) = &mut self.plans {
            plans.retain(|plan| fold_key(plan) != key || plan.account.is_none());
            for plan in plans.iter_mut().filter(|plan| fold_key(plan) == key) {
                *plan = signed_out(plan);
            }
        }
        cx.notify();
    }

    /// Uses one of the resets the agent banks on the login (Codex), on the second press: its limits clear now. Then
    /// reads again.
    pub(crate) fn use_reset(&mut self, plan: &PlanUsage, cx: &mut Context<Self>) {
        let key = fold_key(plan);
        if !crate::agents::ui(plan.agent).banks_resets() || self.busy {
            return;
        }
        if self.confirm_reset.as_ref() != Some(&key) {
            self.confirm_reset = Some(key);
            self.reset_note = None;
            self.removed_note = None;
            cx.notify();
            return;
        }
        self.confirm_reset = None;
        let dir = match login_dir(plan) {
            Ok(dir) => dir,
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
                return;
            }
        };
        let attempt = self
            .reset_attempts
            .entry(key.clone())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.busy = true;
        self.problem = None;
        let login = (plan.agent, plan.account.clone());
        cx.spawn(async move |this, cx| {
            let used = off_thread(move || {
                let program = find(SessionAgent::Codex).ok_or("Codex is not on this computer.")?;
                runtime()?.block_on(plans::use_codex_reset(
                    program,
                    plan_env(SessionAgent::Codex),
                    dir,
                    &attempt,
                ))
            })
            .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match used {
                    Ok(note) => {
                        page.reset_attempts.remove(&key);
                        // The reset cleared the login's limits in their window: what was known no longer holds.
                        crate::plan_book::forget(login.0, login.1);
                        page.reset_note = Some((key, note.into()));
                    }
                    // The attempt stays, so pressing again retries it.
                    Err(error) => page.problem = Some(error),
                }
                page.check(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn is_open(&self, plan: &PlanUsage) -> bool {
        let confirming = self.confirm_remove.as_ref() == Some(&(plan.agent, plan.account.clone()));
        confirming
            || self
                .folds
                .get(&fold_key(plan))
                .copied()
                .unwrap_or_else(|| starts_open(plan))
    }

    pub(crate) fn toggle(&mut self, plan: &PlanUsage, cx: &mut Context<Self>) {
        let open = self.is_open(plan);
        self.folds.insert(fold_key(plan), !open);
        cx.notify();
    }

    pub(crate) fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.copied = Some(text);
        cx.notify();
    }
}

/// Whether a row has anything to fold: a signed-in login's meters, or why it couldn't be read.
pub(crate) fn folds(plan: &PlanUsage) -> bool {
    plan.signed_in || plan.error.is_some()
}

/// A row's key in [`Plans::folds`]: the agent, and the account or the agent's own login.
pub(crate) fn fold_key(plan: &PlanUsage) -> String {
    login_key(plan.agent, plan.account.as_deref())
}

/// A login's key: the agent, and the account or `own` for the agent's own login. Rows key their state and their
/// element ids on it, so a row keeps both when the rows above it go.
pub(crate) fn login_key(agent: SessionAgent, account: Option<&str>) -> String {
    format!("{}:{}", agent.as_str(), account.unwrap_or("own"))
}

/// A row starts open when it needs the person: not signed in, a failed read, or a limit nearly out.
pub(crate) fn starts_open(plan: &PlanUsage) -> bool {
    !plan.signed_in || plan.error.is_some() || tightest(plan).is_some_and(|limit| 100 - used_pct(limit) <= LOW_LEFT)
}

/// The limit with the least left: what a folded row shows.
pub(crate) fn tightest(plan: &PlanUsage) -> Option<&Limit> {
    plan.limits.iter().max_by_key(|limit| used_pct(limit))
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_NOTE, Limit, PlanUsage, Progress, SessionAgent, folds, groups, limit_label, login_command, plan_name,
        plan_payer, plans, resets_in, starts_open, tightest, used_pct, window_label,
    };

    /// Each agent on its own plan names that plan: Antigravity's is Google AI, not Claude's.
    #[test]
    fn each_plan_session_names_its_own_plan() {
        assert_eq!(plan_payer(SessionAgent::Claude), "Your Claude plan");
        assert_eq!(plan_payer(SessionAgent::Codex), "Your ChatGPT plan");
        assert_eq!(plan_payer(SessionAgent::Antigravity), "Your Google AI plan");
    }

    fn sample() -> Vec<PlanUsage> {
        let init: serde_json::Value =
            serde_json::from_str(r#"{"account": {"email": "ana@acme.dev", "subscriptionType": "Claude Max"}}"#)
                .expect("init");
        let usage: serde_json::Value = serde_json::from_str(
            r#"{"rate_limits_available": true, "rate_limits": {
                "five_hour": {"utilization": 87.6, "resets_at": "2026-11-05T15:00:00Z"},
                "seven_day": {"utilization": 12.0, "resets_at": "2026-11-09T09:00:00Z"},
                "limits": [{"kind": "weekly_scoped", "percent": 40, "resets_at": "2026-11-09T09:00:00Z",
                            "scope": {"model": {"display_name": "Fable"}}}]
            }}"#,
        )
        .expect("usage");
        let own = plans::read_claude(None, &init, Some(&usage));
        let extra = plans::read_claude(Some("a_1792000000000".into()), &serde_json::json!({}), None);
        let codex_answer: serde_json::Value = serde_json::from_str(r#"{"account": null}"#).expect("codex");
        let codex = plans::read_codex(None, &codex_answer, None);
        vec![own, extra, codex]
    }

    #[cfg(unix)]
    #[test]
    fn original_sign_out_reports_failure_without_deleting_configuration() {
        let file = crate::desktop_settings::TempFile::new("original-logout");
        let directory = file.path().with_extension("config");
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.toml");
        std::fs::write(&config, "keep this configuration").unwrap();
        for (code, succeeds) in [(0, true), (1, false)] {
            let request = nexus_workbench::process::Spawn {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), format!("exit {code}")],
                env: std::collections::HashMap::new(),
                cwd: directory.clone(),
            };
            assert_eq!(super::finish_sign_out(&request).is_ok(), succeeds);
            assert_eq!(std::fs::read_to_string(&config).unwrap(), "keep this configuration");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn removing_the_active_login_can_choose_a_remaining_signed_in_account() {
        let mut plans = sample();
        plans[1].signed_in = true;
        let account = |removed: &Option<String>, agent: SessionAgent, plans: &[PlanUsage]| {
            super::replacement(plans, agent, removed).and_then(|plan| plan.account.clone())
        };
        assert_eq!(account(&None, SessionAgent::Claude, &plans), plans[1].account);
        assert_eq!(account(&plans[1].account, SessionAgent::Claude, &plans), None);
        plans[1].signed_in = false;
        assert!(super::replacement(&plans, SessionAgent::Claude, &None).is_none());
        assert!(super::replacement(&plans, SessionAgent::Codex, &None).is_none());
    }

    #[test]
    fn reads_plans_into_sections() {
        let plans = sample();
        assert_eq!(plans[0].limits.len(), 3);
        let groups = groups(&plans);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].agent, SessionAgent::Claude);
        assert_eq!(groups[0].accounts, 2, "the signed-in own login and the extra account");
        assert_eq!(groups[0].rows.len(), 2);
        // Codex is installed but never signed in: its own login shows, to connect.
        assert_eq!(groups[1].accounts, 0);
        assert_eq!(groups[1].rows.len(), 1);
        assert_eq!(plan_name(&plans[0]), "Claude Max");
        let google = PlanUsage {
            agent: SessionAgent::Antigravity,
            plan: Some("Google AI".into()),
            ..plans[0].clone()
        };
        assert_eq!(plan_name(&google), "Google AI");
        assert_eq!(used_pct(&plans[0].limits[0]), 88);
        assert_eq!(limit_label(&plans[0].limits[0].label), "This session");
        assert_eq!(limit_label(&plans[0].limits[2].label), "This week · Fable");
        // A row's summary names the window only; the open row names the rest.
        assert_eq!(window_label(&plans[0].limits[2].label), "This week");
        assert_eq!(window_label("Weekly · Claude and GPT"), "This week");
        assert_eq!(window_label(&plans[0].limits[0].label), "This session");
    }

    #[test]
    fn shows_each_login_as_it_answers() {
        let old = sample();
        let fresh = PlanUsage {
            plan: Some("plus".into()),
            signed_in: true,
            ..old[2].clone()
        };
        let progress = Progress {
            logins: old.iter().map(|plan| (plan.agent, plan.account.clone())).collect(),
            read: vec![fresh.clone()],
        };
        // The first check shows what answered; Claude's two logins still run.
        assert_eq!(progress.over(&[]), std::slice::from_ref(&fresh));
        assert_eq!(progress.waiting(), [SessionAgent::Claude]);
        // A later check keeps each last reading in its place until its login answers.
        assert_eq!(progress.over(&old), [old[0].clone(), old[1].clone(), fresh]);
        // A login the check no longer probes, such as a removed account, goes.
        let removed = Progress {
            logins: vec![(SessionAgent::Claude, None)],
            read: vec![],
        };
        assert_eq!(removed.over(&old), [old[0].clone()]);
    }

    #[test]
    fn login_keeps_its_limits_while_its_agent_cannot_report_them() {
        let old = sample();
        assert!(!old[0].limits.is_empty());
        crate::plan_book::read(&old);
        let mid_turn = PlanUsage {
            limits: vec![],
            unavailable: true,
            ..old[0].clone()
        };
        let progress = Progress {
            logins: vec![(old[0].agent, old[0].account.clone())],
            read: vec![mid_turn],
        };
        // The check shows the mid-turn reading; the plan book adds the login's last limits to it.
        crate::plan_book::learn(progress.fresh());
        assert_eq!(
            crate::plan_book::shown(&progress.over(&old)),
            [old[0].clone()].map(|plan| PlanUsage {
                unavailable: true,
                ..plan
            })
        );
    }

    #[test]
    fn used_reset_shows_the_cleared_limits_while_the_old_row_waits() {
        let old = sample();
        let login = (old[0].agent, old[0].account.clone());
        crate::plan_book::learn(&old);
        crate::plan_book::forget(login.0, login.1.clone());
        // The check's first progress, before the login answers, shows the old row: it is not a reading.
        let first = Progress {
            logins: vec![login.clone()],
            read: vec![],
        };
        crate::plan_book::learn(first.fresh());
        assert_eq!(crate::plan_book::shown(&first.over(&old)), [old[0].clone()]);
        // The login answers with its limits cleared, in the same window.
        let cleared = PlanUsage {
            limits: old[0]
                .limits
                .iter()
                .map(|limit| Limit {
                    used_percent: 0.,
                    ..limit.clone()
                })
                .collect(),
            ..old[0].clone()
        };
        let answered = Progress {
            logins: vec![login],
            read: vec![cleared.clone()],
        };
        crate::plan_book::learn(answered.fresh());
        assert_eq!(crate::plan_book::shown(&answered.over(&old)), [cleared]);
    }

    #[test]
    fn says_when_limits_reset_and_how_to_sign_in() {
        let now = 1_000_000_000_000;
        assert_eq!(resets_in(now + 42 * 60_000, now), "42 min");
        assert_eq!(resets_in(now + (3 * 60 + 13) * 60_000, now), "3 h 13 min");
        assert_eq!(resets_in(now + 37 * 3_600_000, now), "1 d 13 h");
        assert_eq!(resets_in(now - 5, now), "1 min");
        assert_eq!(login_command(SessionAgent::Codex, None), "codex login");
        if !cfg!(windows) {
            let dir = std::path::Path::new("/data/accounts/claude/a_1");
            assert_eq!(
                login_command(SessionAgent::Claude, Some(dir)),
                "CLAUDE_CONFIG_DIR='/data/accounts/claude/a_1' claude auth login"
            );
        }
        // agy reads no account variable, so a folder adds nothing in front of its login.
        let dir = std::path::Path::new("/data/accounts/antigravity/a_1");
        assert_eq!(login_command(SessionAgent::Antigravity, Some(dir)), "agy");
    }

    #[test]
    fn says_when_open_sessions_move_to_the_active_account() {
        assert!(ACTIVE_NOTE.starts_with("New Claude sessions start on the Active account."));
        assert!(ACTIVE_NOTE.contains("Open sessions keep their account until it reaches its limits"));
    }

    #[test]
    fn folds_signed_in_rows_and_opens_those_nearly_out() {
        let mut plans = sample();
        assert!(folds(&plans[0]));
        assert!(!folds(&plans[1]), "a signed-out account has nothing to fold");
        // 88% of the session is used: 12% left is under the line, so the row starts open.
        assert_eq!(used_pct(tightest(&plans[0]).expect("limits")), 88);
        assert!(starts_open(&plans[0]));
        plans[0].limits.retain(|limit| limit.used_percent < 50.);
        assert!(!starts_open(&plans[0]));
    }
}
