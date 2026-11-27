//! Each coding agent's checks and launch on this machine, one file per agent. [`LocalAgentT`] is this crate's domain
//! trait, as Gateway's agent trait is. Each of nexus-agents' agent types implements it, and [`local`]
//! reaches it with `nexus_agents::of!`. Facts the `AgentEntry` holds stay there: the executable, the variables and
//! the status command's arguments.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use nexus_agents::Agent;
use nexus_providers::{Credential, ProviderError};

use crate::Login;
use crate::launch::{CredentialEnv, LaunchOptions};

mod antigravity;
mod claude;
mod codex;
mod cursor;
mod grok;
mod opencode;

pub use antigravity::{antigravity_login, antigravity_settings};
pub use claude::{
    CLAUDE_NO_CONVERSATION, CLAUDE_NO_USAGE_ENV, claude_plan, claude_project_name, is_session_id, read_claude_account,
    read_claude_status,
};
pub use codex::{CODEX_KEY_ENV, codex_conversation, read_codex_status};
pub use opencode::{OPENCODE_KEY_ENV, OPENCODE_SERVER_MODEL, opencode_server_provider};

/// How far a file's timestamp may lag the run's start and still count as written after it. A file's time comes from the
/// kernel's coarse clock, which can read a few milliseconds behind `SystemTime::now()`.
pub const FILE_CLOCK_SLACK: Duration = Duration::from_millis(100);

/// A model-catalog override prepared without running a model turn.
pub type CatalogFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<std::path::PathBuf>> + Send + 'a>>;

pub type ModelFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<crate::models::Model>>> + Send + 'a>>;

/// The trait every coding agent's type implements in this crate (`src/agents/<agent>.rs`): what that agent does on
/// this machine. It is not the [`LocalAgent`](crate::LocalAgent) status, which says what this machine has of an
/// agent.
pub trait LocalAgentT: Sync {
    /// Prepares a Gateway model-catalog override, or returns None when none is needed.
    fn model_catalog<'a>(
        &self,
        program: &'a std::path::Path,
        env: &'a HashMap<String, String>,
        dir: &'a std::path::Path,
    ) -> CatalogFuture<'a>;

    /// Queries the installed agent using the caller’s environment when supplied.
    fn models<'a>(
        &self,
        program: &'a std::path::Path,
        path: &'a str,
        version: Option<&'a str>,
        env: Option<&'a HashMap<String, String>>,
    ) -> ModelFuture<'a>;

    /// The sign-in its own files under the home folder `home` settle. A settled sign-in skips its check. `None` when
    /// only its check can tell.
    fn settled_login(&self, home: Option<&str>) -> Option<Login>;

    /// What its sign-in check printed, read: how it is signed in, and the plan's name when it gives one. The caller
    /// has already read the entry's sign-in prompt as signed out.
    fn read_status(&self, stdout: &str, stderr: &str) -> (Login, Option<String>);

    /// The account its sign-in check names (an email), read from its stdout; `None` when its check names none.
    fn read_account(&self, stdout: &str) -> Option<String>;

    /// Its arguments and full environment for one run on `options.route`. The credential's part comes from
    /// [`LocalAgentT::credential_env`]; the model, search and catalog flags are its own.
    fn launch(&self, options: LaunchOptions) -> (Vec<String>, HashMap<String, String>);

    /// Builds the same launch with a credential prepared by the provider.
    fn launch_with_credential(
        &self,
        options: LaunchOptions,
        credential: Credential,
    ) -> (Vec<String>, HashMap<String, String>);

    /// The variables and arguments `credential` gives this agent: its key, base URL, token and usage sink, and the
    /// variables that would compete with them. `ProviderError::NotOffered` for a credential this agent does not take.
    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError>;

    /// The arguments that set `name` to `value` in the command shells it starts; `None` when those shells inherit its
    /// environment, so the variable already reaches them.
    fn shell_variable(&self, name: &str, value: &str) -> Option<[String; 2]>;

    /// Whether it also takes its model as `-m x` and as a `-c model=x` override.
    fn takes_short_model_flags(&self) -> bool;

    /// A policy id (`anthropic/claude-fable-5.1`) as the agent names the model (`claude-fable-5-1`).
    fn provider_model_id(&self, id: &str) -> String;

    /// Whether it takes Nexus's instructions for a run (the pages, the commit trailer) through
    /// [`LocalAgentT::with_instructions`].
    fn takes_instructions(&self) -> bool;

    /// Its arguments `args` with `text` added to its instructions for this run. An agent that takes none gets `args`
    /// unchanged.
    fn with_instructions(&self, args: &[String], text: &str) -> Vec<String>;

    /// Whether it starts on a screen recording (`--recording`). The launch refuses the flag for an agent that does not.
    fn takes_recording(&self) -> bool;

    /// Whether its launcher takes a prompt as its first argument and starts a conversation on it (`nexus claude --
    /// "<prompt>"`), as `nexus <product> apply … --agent` starts one.
    fn takes_prompt_argument(&self) -> bool;

    /// Its arguments `args` with a screen recording: `prompt` says where the recording is and how to read it, and
    /// `sheet` is the recording's contact sheet, which may not exist. An agent that takes none gets `args` unchanged.
    fn with_recording(&self, args: &[String], prompt: &str, sheet: &Path) -> Vec<String>;

    /// The profile's native memory for the repository at `repo`.
    fn memory(&self, home: &Path, repo: &Path) -> std::io::Result<Vec<nexus_agent_memory::NativeEntry>>;

    /// Writes an explicit export into this profile for the selected checkout.
    fn write_memory(&self, home: &Path, repo: &Path, entries: &[nexus_agent_memory::Entry]) -> std::io::Result<()>;

    /// Whether it writes each conversation as a JSON-lines transcript that [`LocalAgentT::transcript`] finds. `false`:
    /// its runs keep no session log.
    fn keeps_transcript(&self) -> bool;

    /// The newest conversation it wrote to in `cwd` since `since`, read from its transcripts under its profile folder
    /// `home`. Each agent's file says which conversations count. `None` when none qualifies, or when it keeps no
    /// transcript.
    fn newest_conversation(&self, home: &Path, cwd: &Path, since: SystemTime) -> Option<String>;

    /// The transcript of its conversation `id`, run in `cwd`, under its profile folder `home`. `None` until it has
    /// written one, or when it keeps no transcript.
    fn transcript(&self, home: &Path, cwd: &Path, id: &str) -> Option<PathBuf>;
}

/// `agent`'s [`LocalAgentT`].
pub fn local(agent: Agent) -> &'static dyn LocalAgentT {
    nexus_agents::of!(agent, dyn LocalAgentT)
}

/// The profile variable naming an extra account's folder, when the agent has one, for an `AgentLogin` credential.
fn account_dir_env<'c>(agent: Agent, credential: &'c Credential) -> Option<(&'static str, std::borrow::Cow<'c, str>)> {
    let Credential::AgentLogin {
        account_dir: Some(dir), ..
    } = credential
    else {
        return None;
    };
    Some((agent.entry().profile.var?, dir.to_string_lossy()))
}

/// The credential a launch's route names, which every agent takes: Gateway's session or the agent's own login. A
/// failure here is a defect in the agent's `credential_env`, which the launch goldens pin for each route.
fn route_env<'c>(agent: Agent, credential: &'c Credential) -> CredentialEnv<'c> {
    local(agent)
        .credential_env(credential)
        .unwrap_or_else(|error| panic!("{agent} refused its route's credential: {error}"))
}
