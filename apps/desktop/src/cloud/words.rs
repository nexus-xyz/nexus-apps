//! The Nexus app's words and rules for its cloud screens (`workbench/cloud-state.ts`, `cloud-ui.tsx`, `archive.ts`):
//! how a cloud session's machine reads, its one line while it comes up and the queue's line, the setup's checks, the
//! repository picker's order, Settings › Cloud's lines, what ending a session says, and the session's Cloud panel
//! (`cloud-actions.tsx`).

use nexus_remote::machine::Status;
use nexus_workbench::cloud_agent::cloud_agent;
use nexus_workbench::event::{SessionAgent, Via};

use super::{CloudInfo, CloudProgress, CloudRepo, CloudState, CloudStep};
use crate::machine_view::{Action, StatusView, Tone, status_view};
use crate::machines::CloudView;

const DAY: i64 = 86_400_000;

/// How a cloud machine reads in the sidebar, the pickers and Settings: by its provider state first (asleep, waking),
/// then by its connection once it runs (`cloudView`).
pub fn cloud_view(info: &CloudInfo, status: &Status, now: i64) -> StatusView {
    let view = |label, tone, detail: Option<String>, action| StatusView {
        label,
        tone,
        detail,
        action,
    };
    if resizing(info) {
        return view(
            "Restarting on a bigger machine",
            Tone::Busy,
            Some("It ran out of memory. The session continues once it is back.".into()),
            None,
        );
    }
    match info.state {
        CloudState::Queued => view(
            "Waiting for a machine",
            Tone::Busy,
            Some(queue_line(info.queued_until, now)),
            None,
        ),
        CloudState::Creating => view("Creating…", Tone::Busy, None, None),
        CloudState::Starting => view("Waking…", Tone::Busy, Some("Waking takes a few seconds.".into()), None),
        CloudState::Stopping => view("Going to sleep…", Tone::Busy, None, None),
        // Asleep reads as a quiet mark only: the person never has to think about it. Sending a message wakes it.
        CloudState::Sleeping => view("Asleep", Tone::Off, None, None),
        CloudState::Failed => view(
            "Failed",
            Tone::Warn,
            Some(
                info.error
                    .clone()
                    .unwrap_or_else(|| "The cloud provider says this machine broke.".into()),
            ),
            Some(Action::Retry),
        ),
        CloudState::Gone => view(
            "Deleted",
            Tone::Off,
            Some(
                "It no longer exists at the cloud provider: its disk was deleted after 14 days asleep, or someone deleted it.".into(),
            ),
            None,
        ),
        CloudState::Ready => match status {
            Status::Connected { update: None, .. } => view("Ready", Tone::Ok, None, None),
            Status::Closed => view("Offline", Tone::Off, None, Some(Action::Retry)),
            other => status_view(other),
        },
    }
}

/// Whether Gateway is restarting it on a bigger size, after it ran out of memory.
pub fn resizing(info: &CloudInfo) -> bool {
    info.stop_reason.as_deref() == Some("resize")
        && !matches!(info.state, CloudState::Ready | CloudState::Failed | CloudState::Gone)
}

/// "Waiting for a machine (about 3 min)": the line a queued session shows. Without an estimate, no time.
pub fn queue_line(queued_until: Option<i64>, now: i64) -> String {
    match queued_until {
        None => "Waiting for a machine".into(),
        Some(until) => {
            let minutes = ((until - now) as f64 / 60_000.0).ceil().max(1.0) as i64;
            format!("Waiting for a machine (about {minutes} min)")
        }
    }
}

/// Whether the machine is asleep or going to sleep. Opening one of its sessions does not wake it; typing in its
/// composer or sending a message does (`workspace/cloud_view.rs`).
pub fn is_asleep(info: &CloudInfo) -> bool {
    matches!(info.state, CloudState::Sleeping | CloudState::Stopping)
}

/// Whether to show the step: something is being created or woken, or stopped on an error.
pub fn shows_steps(progress: Option<&CloudProgress>) -> bool {
    progress.is_some_and(|progress| progress.error.is_some() || progress.step != CloudStep::Ready)
}

/// A cloud session's machine on its way (`CloudMachineProgress`): its title, whether its step shows, and why it
/// failed. None once the machine runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coming {
    pub title: String,
    pub steps: bool,
    pub failed: Option<String>,
}

pub fn coming(machine: &CloudView, now: i64) -> Option<Coming> {
    let info = &machine.info;
    let progress = machine.progress.as_ref();
    let queued = info.state == CloudState::Queued;
    let restarting = resizing(info);
    let asleep = is_asleep(info);
    let failed = progress.is_some_and(|progress| progress.error.is_some()) || info.state == CloudState::Failed;
    // A sleeping machine is not on its way until something wakes it: its wake's steps show then.
    if !queued && !restarting && !failed && !shows_steps(progress) {
        return None;
    }
    let title = if queued {
        queue_line(info.queued_until, now)
    } else if restarting {
        "Restarting on a bigger machine".into()
    } else if asleep {
        "Waking the machine".into()
    } else if failed {
        "The machine did not start".into()
    } else {
        STARTING.into()
    };
    let error = progress
        .and_then(|progress| progress.error.clone())
        .or_else(|| cloud_view(info, &machine.status, now).detail);
    Some(Coming {
        title,
        steps: !queued && !restarting && shows_steps(progress),
        failed: failed.then(|| error.unwrap_or_default()),
    })
}

/// How a step reads in the one-line status of a session whose machine comes up: "Starting a machine · Connecting".
pub fn step_line(step: CloudStep) -> &'static str {
    match step {
        CloudStep::Create => "Creating",
        CloudStep::Start => "Starting",
        CloudStep::Ssh => "Connecting",
        CloudStep::Setup => "Preparing",
        CloudStep::Install => "Installing Nexus",
        CloudStep::Connect => "Starting Nexus",
        CloudStep::Ready => "Ready",
    }
}

/// The one line above a cloud session's composer while its machine comes up, in place of the steps' card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartLine {
    /// On its way: what it does now.
    Busy(String),
    /// Stopped: why. Try again and End follow it.
    Failed(String),
}

/// The line while it starts.
const STARTING: &str = "Starting a machine";

/// The line once the machine runs and the session is being made there.
const CLONING: &str = "Cloning the repository";

/// The line of a machine on its way (`coming`), in short: the queue's line, a wake or a restart, the step it is at,
/// or why it failed. None once it runs.
pub fn machine_line(machine: &CloudView, now: i64) -> Option<StartLine> {
    let coming = coming(machine, now)?;
    if let Some(failed) = coming.failed {
        return Some(StartLine::Failed(if failed.is_empty() { coming.title } else { failed }));
    }
    let step = machine.progress.as_ref().filter(|_| coming.steps);
    Some(StartLine::Busy(match step {
        Some(progress) => format!("{} · {}", coming.title, step_line(progress.step)),
        None => coming.title,
    }))
}

/// The line of a new cloud session while it starts: why its start failed (`failed`), else its machine's line; before
/// Gateway answered with a machine, that one is on its way, and once it runs, the repository is cloned, with git's
/// progress (`cloning`: its stage and percent) once the machine said it.
pub fn start_line(
    machine: Option<&CloudView>,
    failed: Option<&str>,
    cloning: Option<&(String, Option<u8>)>,
    now: i64,
) -> StartLine {
    if let Some(failed) = failed {
        return StartLine::Failed(failed.to_string());
    }
    match machine {
        None => StartLine::Busy(STARTING.into()),
        Some(machine) => machine_line(machine, now).unwrap_or_else(|| StartLine::Busy(clone_line(cloning))),
    }
}

/// The stage a cloud machine's host says while it takes its saved copy of the repository instead of a clone:
/// "Starting from a saved copy of acme/web".
const SAVED_COPY: &str = "Starting from a saved copy of ";

/// The stage a cloud machine's own setup says while the repository's `.agents/setup` runs.
pub const RUNNING_SETUP: &str = "Running setup";

/// "Cloning the repository · Receiving objects · 43%", the stage alone without a percentage, the title before either.
/// A saved copy's stage and the setup's are the line alone, so the person sees which path the session took.
fn clone_line(cloning: Option<&(String, Option<u8>)>) -> String {
    match cloning {
        None => CLONING.into(),
        Some((stage, _)) if stage.starts_with(SAVED_COPY) || stage == RUNNING_SETUP => stage.clone(),
        Some((stage, None)) => format!("{CLONING} · {stage}"),
        Some((stage, Some(percent))) => format!("{CLONING} · {stage} · {percent}%"),
    }
}

/// Under the first message of a session whose machine is not ready yet.
pub const SENDS_WHEN_READY: &str = "Sends when the machine is ready";

/// A setup token lasts a year (`claude setup-token`); the app warns from 11 months on.
const TOKEN_DAYS: i64 = 365;
const TOKEN_WARN_DAYS: i64 = 335;

/// When the saved Claude token needs replacing: None while it is fine.
pub fn token_warning(saved_at: Option<i64>, now: i64) -> Option<String> {
    let age = (now - saved_at?).div_euclid(DAY);
    if age < TOKEN_WARN_DAYS {
        return None;
    }
    let left = TOKEN_DAYS - age;
    Some(if left > 0 {
        format!(
            "Expires in {left} day{}, make a new one.",
            if left == 1 { "" } else { "s" }
        )
    } else {
        "Expired, make a new one.".into()
    })
}

/// The repositories matching `query` (in `owner/name`), most recently pushed first (`pickRepos`).
pub fn pick_repos(repos: &[CloudRepo], query: &str) -> Vec<CloudRepo> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut picked: Vec<CloudRepo> = repos
        .iter()
        .filter(|repo| {
            let name = repo.name.to_lowercase();
            words.iter().all(|word| name.contains(word))
        })
        .cloned()
        .collect();
    picked.sort_by(|a, b| {
        b.pushed_at
            .unwrap_or(-1)
            .cmp(&a.pushed_at.unwrap_or(-1))
            .then_with(|| a.name.cmp(&b.name))
    });
    picked
}

/// Settings › Cloud's steps: the sign-ins every new machine gets from this computer.
pub use nexus_workbench::cloud_agent::Check;

/// The steps a cloud session with `agent` needs (`checksFor`): its own file under nexus-workbench's `cloud_agent` says.
pub fn checks_for(agent: SessionAgent) -> &'static [Check] {
    cloud_agent(agent).checks()
}

/// The steps still to do for `agent`, from this computer's account.
pub fn missing(agent: SessionAgent, claude: bool, github: bool) -> Vec<Check> {
    checks_for(agent)
        .iter()
        .copied()
        .filter(|check| match check {
            Check::Claude => !claude,
            Check::Github => !github,
        })
        .collect()
}

/// True when `agent` paid by `via` can't run on a cloud machine (`cloudPlanRefused`).
pub fn plan_refused(agent: SessionAgent, via: Via) -> bool {
    plan_refused_line(agent, via).is_some()
}

/// Why a new cloud session with `agent` paid by `via` can't start; `None` when it can ([`plan_refused`]). Each
/// agent's own file under nexus-workbench's `cloud_agent` says.
pub fn plan_refused_line(agent: SessionAgent, via: Via) -> Option<&'static str> {
    cloud_agent(agent).refused(via)
}

/// Why the organization can't start a cloud machine: Gateway's 403 `cloud_not_enabled`.
const NOT_ENABLED: &str = "Cloud machines aren't turned on for this organization yet.";

/// Why the person's role can't use cloud machines: any other 403.
const ROLE: &str = "Your role in this organization can't use cloud machines. Ask an admin if you need one.";

/// What a 403 from Gateway says, by its error code.
pub fn refusal(code: &str) -> &'static str {
    match code {
        "cloud_not_enabled" => NOT_ENABLED,
        _ => ROLE,
    }
}

/// The one line Settings › Cloud shows instead of the checklist when cloud machines are unavailable.
pub fn unavailable_line(reason: Option<&str>) -> &'static str {
    match reason {
        Some("not_enabled" | "cloud_not_enabled") => NOT_ENABLED,
        Some("not_allowed") => ROLE,
        Some("signed_out") => "Sign in to Nexus to use a cloud machine.",
        Some("degraded") => {
            "Cloud machines are having trouble right now. Nexus keeps trying, and this goes away when they're back."
        }
        _ => "Cloud machines aren't available on this Nexus yet.",
    }
}

/// The person's cloud sessions by where their machines stand: running (or on the way), and asleep.
pub fn session_counts(machines: &[CloudView]) -> (usize, usize) {
    machines
        .iter()
        .filter(|machine| !matches!(machine.info.state, CloudState::Gone | CloudState::Failed))
        .fold((0, 0), |(running, asleep), machine| {
            if is_asleep(&machine.info) && !resizing(&machine.info) {
                (running, asleep + 1)
            } else {
                (running + 1, asleep)
            }
        })
}

/// "2 sessions running, 1 asleep".
pub fn counts_line((running, asleep): (usize, usize)) -> String {
    format!(
        "{running} session{} running, {asleep} asleep",
        if running == 1 { "" } else { "s" }
    )
}

/// A new cloud session's title: its first message's first line, cut at 80 characters.
pub fn session_title(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return "Cloud session".into();
    }
    if line.chars().count() <= 80 {
        return line.to_string();
    }
    let cut: String = line.chars().take(79).collect();
    format!("{}…", cut.trim_end())
}

/// What ending a cloud session does: archive or delete. Both delete its machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Archive,
    Delete,
}

/// The one question before a machine goes with commits that are on no remote (`unpushedText`, for one session).
pub fn unpushed_text(title: &str, unpushed: &[String], end: End) -> String {
    let doing = match end {
        End::Archive => "Archiving deletes its machine and them",
        End::Delete => "Deleting it deletes its machine and them",
    };
    format!(
        "\u{201c}{title}\u{201d} has commits that aren\u{2019}t on GitHub: {}. {doing}.",
        unpushed.join("; ")
    )
}

/// The question's action: "Archive anyway" or "Delete anyway".
pub fn anyway(end: End) -> &'static str {
    match end {
        End::Archive => "Archive anyway",
        End::Delete => "Delete anyway",
    }
}

/// What the notice says once the cloud session called `title` is gone (`endedText`).
pub fn ended_text(title: &str, end: End) -> String {
    match end {
        End::Archive => format!("Archived \u{201c}{title}\u{201d}"),
        End::Delete => format!("Deleted \u{201c}{title}\u{201d}"),
    }
}

/// The Code top bar's Cloud button's tip (`CloudActions`).
pub fn panel_tip(label: &str) -> String {
    format!("Cloud · {label}. Preview, share and watch")
}

/// The Cloud panel's title while its machine does not run: "Cloud is asleep".
pub fn panel_waiting(label: &str) -> String {
    format!("Cloud is {}", label.to_lowercase())
}

/// The Cloud panel's line under that title.
pub const PANEL_WAITING_NOTE: &str = "Its ports and desktop show once it is running.";

/// The Cloud panel while it asks the machine for its ports.
pub const PANEL_CHECKING: &str = "Checking the machine…";

/// The Cloud panel when nothing listens on a port.
pub const PANEL_NO_PORTS: &str = "Nothing listens on a port yet. Ask the agent to start the dev server.";

/// The Cloud panel's line after a share link for `port` was made: "Link copied." while the copy is fresh.
pub fn shared_line(port: u16, copied: bool) -> String {
    let copied = if copied { "Link copied. " } else { "" };
    format!("{copied}Anyone with the link can open port {port}.")
}
