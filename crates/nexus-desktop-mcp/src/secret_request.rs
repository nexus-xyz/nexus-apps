//! An agent asks the person for a secret in a private card, instead of having it pasted into the chat. The
//! app's side (the cache folder, the commands and [`Shell`]'s implementation) is the shell's.
//!
//! The built-in `nexus` MCP server's `request_secret` tool ([`TOOL_NAME`], src/mcp.rs) reaches [`answer`] over the
//! session's loopback socket. The app keeps the request in memory and shows it to the windows
//! as a card: a password field with Provide and Decline. The call waits
//! for the answer, at most [`WAIT`].
//!
//! The value goes from the card to the app over IPC, and from the app to one file: mode 0600, a random name, in the
//! session's own folder (mode 0700) under the app's cache folder, never the shared /tmp. The agent gets the file's path
//! only; it reads the value in its own shell (`"$(cat <path>)"`, a `--from-file` option), so the value enters neither
//! the model's context nor its transcript. The value is never in the session's event log, the tool result, a
//! notification, an event to the windows, or a log line. The file is deleted when the agent's run ends, [`TTL`] after
//! it was written, or at the app's next start, whichever comes first.
//!
//! Only `delivery: "file"`: an environment variable cannot reach a running agent, and a reference the agent hands to
//! a resolving tool would still need that tool to run the command for it. Values are not saved to the keychain.
//! Remote sessions do not get the tool: their agent runs on the host, where this app writes no file. The sessions of the
//! background service (src/service.rs) do: the service runs on this computer and passes the call to the app
//! ([`crate::service_relay`]), which writes the file as for its own sessions. A side chat is a session of its own: its card shows in its tab, and its file goes in its folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;

use nexus_desktop_core::notify::Notifier;
use nexus_workbench::Workbench;
use nexus_workbench::event::ThreadSummary;
use tokio::sync::oneshot;

/// What `request_secret` needs from the app that runs it: where the requests are kept, the event sink for the open
/// requests, the workbench (the session's title), and a runtime for the files' expiry. It shows the notification as its
/// [`Notifier`].
pub trait Shell: Notifier + Clone + Send + Sync + 'static {
    /// The requests, once the app keeps them; None before.
    fn secrets(&self) -> Option<&Secrets>;
    /// The event sink: the open requests changed.
    fn emit(&self, open: &[SecretRequest]);
    /// The workbench, when it is open.
    fn workbench(&self) -> Option<Workbench>;
    /// This computer's sessions, for a request's session and its title. An app whose sessions run in its background
    /// service reads them its own way; the rest use the workbench's.
    fn threads(&self) -> Option<Vec<ThreadSummary>> {
        self.workbench()?.threads().ok()
    }
    /// Runs a task until it ends.
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>);
}

/// The tool's name in the built-in server (`nexus_workbench`'s).
pub use nexus_workbench::app_tools::SECRET_TOOL_NAME as TOOL_NAME;
/// How long a written file lives at most.
pub use nexus_workbench::app_tools::SECRET_TTL as TTL;
/// What the built-in server's relayed `request_secret` calls say in `tools`, so the socket hands them here.
pub use nexus_workbench::mcp_servers::SECRET_TOOLS as TOOLS;
/// How long a call waits for the person. A card still open then closes, and the call says so.
pub use nexus_workbench::mcp_servers::SECRET_WAIT as WAIT;
/// The app's cache folder's subfolder that holds one folder per session.
pub const FOLDER: &str = "secret-files";
use nexus_workbench::app_tools::{SECRET_MAX_NAME as MAX_NAME, SECRET_MAX_REASON as MAX_REASON};
const MAX_VALUE: usize = 64 * 1024;
/// Open requests at once, across sessions.
const MAX_OPEN: usize = 4;

/// An open request, as the windows see it. Never the value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequest {
    pub id: String,
    pub thread_id: String,
    pub name: String,
    pub reason: String,
}

/// What the person did with a card.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Provided(PathBuf),
    Declined,
}

struct Pending {
    request: SecretRequest,
    run: String,
    reply: oneshot::Sender<Outcome>,
}

struct Written {
    path: PathBuf,
    run: String,
    at: Instant,
}

/// The open requests and the files not deleted yet, in memory only.
#[derive(Default)]
pub struct Secrets {
    pending: Mutex<HashMap<String, Pending>>,
    written: Mutex<Vec<Written>>,
}

impl Secrets {
    /// Opens a request for the run `run` of the session `thread`. One at a time per session.
    fn ask(
        &self,
        thread: &str,
        run: &str,
        name: &str,
        reason: &str,
    ) -> Result<(SecretRequest, oneshot::Receiver<Outcome>), String> {
        let mut pending = self.pending.lock().map_err(|_| UNAVAILABLE)?;
        if pending.values().any(|open| open.request.thread_id == thread) {
            return Err("This session already waits for a secret. Ask for one at a time.".into());
        }
        // Each waiting call holds one of the app socket's few connections.
        if pending.len() >= MAX_OPEN {
            return Err("Too many sessions wait for a secret. Try again when one is answered.".into());
        }
        let request = SecretRequest {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id: thread.into(),
            name: name.into(),
            reason: reason.into(),
        };
        let (reply, answer) = oneshot::channel();
        pending.insert(
            request.id.clone(),
            Pending {
                request: request.clone(),
                run: run.into(),
                reply,
            },
        );
        Ok((request, answer))
    }

    /// The open requests, one per session at most.
    pub fn open(&self) -> Vec<SecretRequest> {
        let Ok(pending) = self.pending.lock() else {
            return Vec::new();
        };
        let mut open: Vec<_> = pending.values().map(|open| open.request.clone()).collect();
        open.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
        open
    }

    /// The person's answer to the request `id`: the value, written to a file under `base`, or None to decline.
    pub fn settle(&self, base: &Path, id: &str, value: Option<&str>) -> Result<(), String> {
        let value = value.map(|value| value.trim_end_matches(['\r', '\n']));
        if value.is_some_and(str::is_empty) {
            return Err("Enter the secret, or decline.".into());
        }
        if value.is_some_and(|value| value.len() > MAX_VALUE) {
            return Err("The secret is longer than 64 KB.".into());
        }
        // Taken out first: a second answer to the same card finds nothing.
        let open = self
            .pending
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .remove(id)
            .ok_or("The agent no longer waits for this secret.")?;
        let Some(value) = value else {
            let _ = open.reply.send(Outcome::Declined);
            return Ok(());
        };
        let path = write(base, &open.request.thread_id, value)?;
        // Kept before the agent gets the path, so the run's end deletes it from then on.
        if let Ok(mut written) = self.written.lock() {
            written.push(Written {
                path: path.clone(),
                run: open.run,
                at: Instant::now(),
            });
        }
        if open.reply.send(Outcome::Provided(path.clone())).is_err() {
            // The call ended in the meantime: nobody gets the path.
            self.remove_where(|written| written.path == path);
            return Err("The agent stopped waiting for this secret.".into());
        }
        Ok(())
    }

    /// Closes the request `id` without an answer. True when it was still open.
    fn withdraw(&self, id: &str) -> bool {
        self.pending
            .lock()
            .is_ok_and(|mut pending| pending.remove(id).is_some())
    }

    /// The run `run` ended: its open requests close, and its files go.
    fn end_run(&self, run: &str) -> bool {
        let closed = self.pending.lock().is_ok_and(|mut pending| {
            let before = pending.len();
            pending.retain(|_, open| open.run != run);
            pending.len() != before
        });
        self.remove_where(|written| written.run == run);
        closed
    }

    /// Deletes the files written [`TTL`] or longer before `now`.
    fn expire(&self, now: Instant) {
        self.remove_where(|written| now.saturating_duration_since(written.at) >= TTL);
    }

    fn remove_where(&self, gone: impl Fn(&Written) -> bool) {
        let Ok(mut written) = self.written.lock() else {
            return;
        };
        written.retain(|file| {
            if gone(file) {
                remove(&file.path);
                false
            } else {
                true
            }
        });
    }
}

const UNAVAILABLE: &str = "Secret requests are unavailable.";

#[cfg(test)]
use nexus_workbench::secret_files::session_dir;
use nexus_workbench::secret_files::{provided_text, remove, write};

/// The tool's definition, for the server's `tools/list` (`nexus_workbench`'s).
pub use nexus_workbench::app_tools::secret_tool_definition as tool_definition;

/// The tool's arguments: the name and the reason, checked.
fn arguments(args: &Value) -> Result<(String, String), String> {
    let text = |key: &str, max: usize| -> Result<String, String> {
        let value = args[key].as_str().map(str::trim).unwrap_or_default();
        if value.is_empty() {
            return Err(format!("Give the secret's {key}."));
        }
        if value.chars().count() > max {
            return Err(format!("The {key} is longer than {max} characters."));
        }
        if value.chars().any(char::is_control) {
            return Err(format!("The {key} has a control character."));
        }
        Ok(value.to_string())
    };
    match args["delivery"].as_str() {
        None | Some("file") => {}
        Some(other) => {
            return Err(format!(
                "Delivery \"{other}\" is not available. Use \"file\": you get the path of a file that holds the value."
            ));
        }
    }
    Ok((text("name", MAX_NAME)?, text("reason", MAX_REASON)?))
}

/// Sends the open requests to the event sink.
pub fn emit(shell: &impl Shell) {
    if let Some(secrets) = shell.secrets() {
        shell.emit(&secrets.open());
    }
}

/// The run `run` ended: its open cards close, and its files go.
pub fn end_run(shell: &impl Shell, run: &str) {
    if let Some(secrets) = shell.secrets()
        && secrets.end_run(run)
    {
        emit(shell);
    }
}

/// Answers a relayed `request_secret` call of the session `thread`'s run `run`: waits for the card's answer.
pub async fn answer(shell: &impl Shell, thread: &str, run: &str, message: &Value) -> Value {
    let done = ask(shell, thread, run, &message["params"]["arguments"])
        .await
        .map(|(name, path)| provided_text(&name, &path));
    reply(message, done)
}

/// [`answer`] for a session on a person's own server: the card opens here as for any session, but the
/// agent reads the file there. So the value goes back in the answer ([`nexus_workbench::secret_files::VALUE_FIELD`]),
/// over the SSH connection the call came on, and the server's host writes the file and tells the agent its path. The
/// file written here is deleted at once.
pub async fn answer_remote(shell: &impl Shell, thread: &str, run: &str, message: &Value) -> Value {
    let (name, path) = match ask(shell, thread, run, &message["params"]["arguments"]).await {
        Ok(provided) => provided,
        Err(text) => return reply(message, Err(text)),
    };
    let value = std::fs::read_to_string(&path);
    remove(&path);
    match value {
        Ok(value) => {
            let mut answer = reply(message, Ok(String::new()));
            answer[nexus_workbench::secret_files::VALUE_FIELD] = json!({"name": name, "value": value});
            answer
        }
        Err(error) => reply(
            message,
            Err(format!("Nexus could not read the secret's file: {}", error.kind())),
        ),
    }
}

fn reply(message: &Value, done: Result<String, String>) -> Value {
    let (text, failed) = match done {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    json!({"jsonrpc": "2.0", "id": message["id"], "result": {"content": [{"type": "text", "text": text}], "isError": failed}})
}

/// The person's answer to the session's request: the secret's name and the file holding it, or why there is none.
async fn ask(shell: &impl Shell, thread: &str, run: &str, args: &Value) -> Result<(String, PathBuf), String> {
    let (name, reason) = arguments(args)?;
    let title = shell
        .threads()
        .and_then(|threads| threads.into_iter().find(|t| t.id == thread))
        .map(|t| t.title)
        .ok_or("This session is not one of this computer's sessions. Remote sessions can't ask for a secret.")?;
    let secrets = shell.secrets().ok_or(UNAVAILABLE)?;
    let (request, answer) = secrets.ask(thread, run, &name, &reason)?;
    emit(shell);
    shell.show(None, thread, &title, &format!("Asks for a secret: {name}"));
    let outcome = tokio::time::timeout(WAIT, answer).await;
    // A card still open (the wait ended): it closes. One answered was closed by its answer.
    if secrets.withdraw(&request.id) {
        emit(shell);
    }
    match outcome {
        Ok(Ok(Outcome::Provided(path))) => {
            let later = shell.clone();
            shell.spawn(Box::pin(async move {
                tokio::time::sleep(TTL).await;
                if let Some(secrets) = later.secrets() {
                    secrets.expire(Instant::now());
                }
            }));
            Ok((name, path))
        }
        Ok(Ok(Outcome::Declined)) => Err(format!(
            "The person declined to provide {name}. Go on without it, or ask them what to do."
        )),
        Ok(Err(_)) => Err("The request closed: the agent run ended.".into()),
        Err(_) => Err(format!(
            "The person did not answer within {} minutes, so the card closed. Ask again when they are back.",
            WAIT.as_secs() / 60
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUE: &str = "sk-live-7f3a9c-very-secret";

    /// A folder of its own, deleted with the value.
    struct Dir(PathBuf);
    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn base() -> Dir {
        let dir = std::env::temp_dir().join(format!("nexus-secret-request-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        Dir(dir)
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn a_provided_secret_is_a_0600_file_in_a_0700_session_folder_and_the_result_has_only_its_path() {
        let dir = base();
        let base = dir.path().join(FOLDER);
        let secrets = Secrets::default();
        let (request, mut answer) = secrets
            .ask("thread-1", "run-1", "OpenAI API key", "To run the evals.")
            .expect("ask");
        // The windows' payload: the name and the reason, never the value.
        let shown = serde_json::to_string(&secrets.open()).expect("json");
        assert!(shown.contains("OpenAI API key") && !shown.contains(VALUE));
        secrets
            .settle(&base, &request.id, Some(&format!("{VALUE}\n")))
            .expect("settle");
        let Ok(Outcome::Provided(path)) = answer.try_recv() else {
            panic!("no path");
        };
        assert_eq!(std::fs::read_to_string(&path).expect("read"), VALUE);
        assert!(path.starts_with(base.join("thread-1")));
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().expect("dir")), 0o700);
            assert_eq!(mode(&base), 0o700);
        }
        let text = provided_text("OpenAI API key", &path);
        assert!(text.contains(&path.display().to_string()));
        assert!(!text.contains(VALUE));
        assert!(!path.display().to_string().contains(VALUE));
        let call = json!({"content": [{"type": "text", "text": text}]}).to_string();
        assert!(!call.contains(VALUE));
        assert!(secrets.open().is_empty());
    }

    #[test]
    fn the_file_goes_when_the_run_ends_and_another_runs_file_stays() {
        let dir = base();
        let secrets = Secrets::default();
        let mut paths = Vec::new();
        for (thread, run) in [("a", "run-a"), ("b", "run-b")] {
            let (request, mut answer) = secrets.ask(thread, run, "Token", "Why.").expect("ask");
            secrets.settle(dir.path(), &request.id, Some(VALUE)).expect("settle");
            let Ok(Outcome::Provided(path)) = answer.try_recv() else {
                panic!("no path");
            };
            paths.push(path);
        }
        secrets.end_run("run-a");
        assert!(!paths[0].exists());
        // Its empty session folder goes too.
        assert!(!paths[0].parent().expect("dir").exists());
        assert!(paths[1].exists());
    }

    #[test]
    fn the_file_goes_after_the_ttl() {
        let dir = base();
        let secrets = Secrets::default();
        let (request, mut answer) = secrets.ask("a", "run", "Token", "Why.").expect("ask");
        secrets.settle(dir.path(), &request.id, Some(VALUE)).expect("settle");
        let Ok(Outcome::Provided(path)) = answer.try_recv() else {
            panic!("no path");
        };
        secrets.expire(Instant::now());
        assert!(path.exists());
        secrets.expire(Instant::now() + TTL);
        assert!(!path.exists());
    }

    #[test]
    fn a_decline_writes_nothing_and_a_card_takes_one_answer() {
        let dir = base();
        let secrets = Secrets::default();
        let (request, mut answer) = secrets.ask("a", "run", "Token", "Why.").expect("ask");
        secrets.settle(dir.path(), &request.id, None).expect("decline");
        assert_eq!(answer.try_recv(), Ok(Outcome::Declined));
        assert!(!dir.path().join("a").exists());
        assert!(secrets.settle(dir.path(), &request.id, Some(VALUE)).is_err());
        assert!(!dir.path().join("a").exists());
    }

    #[test]
    fn an_answer_after_the_call_ended_deletes_its_file_and_says_so_without_the_value() {
        let dir = base();
        let secrets = Secrets::default();
        let (request, answer) = secrets.ask("a", "run", "Token", "Why.").expect("ask");
        drop(answer);
        let error = secrets
            .settle(dir.path(), &request.id, Some(VALUE))
            .expect_err("nobody waits");
        assert!(!error.contains(VALUE));
        assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 0);
    }

    #[test]
    fn a_run_that_ends_closes_its_open_card() {
        let secrets = Secrets::default();
        let (_, mut answer) = secrets.ask("a", "run", "Token", "Why.").expect("ask");
        assert!(secrets.end_run("run"));
        assert!(secrets.open().is_empty());
        assert!(answer.try_recv().is_err());
    }

    #[test]
    fn one_open_request_per_session() {
        let secrets = Secrets::default();
        let _first = secrets.ask("a", "run", "Token", "Why.").expect("ask");
        assert!(secrets.ask("a", "run", "Other", "Why.").is_err());
        assert!(secrets.ask("b", "run", "Other", "Why.").is_ok());
    }

    #[test]
    fn an_empty_or_oversized_value_is_refused_and_the_card_stays_open() {
        let dir = base();
        let secrets = Secrets::default();
        let (request, _answer) = secrets.ask("a", "run", "Token", "Why.").expect("ask");
        assert!(secrets.settle(dir.path(), &request.id, Some("\n")).is_err());
        assert!(
            secrets
                .settle(dir.path(), &request.id, Some(&"x".repeat(MAX_VALUE + 1)))
                .is_err()
        );
        assert_eq!(secrets.open().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_session_folder_is_refused() {
        let dir = base();
        let elsewhere = base();
        std::fs::create_dir_all(dir.path()).expect("base");
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("a")).expect("link");
        let error = write(dir.path(), "a", VALUE).expect_err("refused");
        assert!(!error.contains(VALUE));
        assert_eq!(std::fs::read_dir(elsewhere.path()).expect("dir").count(), 0);
    }

    #[test]
    fn checks_the_arguments() {
        assert_eq!(
            arguments(&json!({"name": " Token ", "reason": "Why."})),
            Ok(("Token".into(), "Why.".into()))
        );
        assert!(arguments(&json!({"name": "Token", "reason": "Why.", "delivery": "file"})).is_ok());
        assert!(arguments(&json!({"name": "Token", "reason": "Why.", "delivery": "env"})).is_err());
        assert!(arguments(&json!({"reason": "Why."})).is_err());
        assert!(arguments(&json!({"name": "Token"})).is_err());
        assert!(arguments(&json!({"name": "a\u{1b}[2J", "reason": "Why."})).is_err());
        assert!(arguments(&json!({"name": "x".repeat(MAX_NAME + 1), "reason": "Why."})).is_err());
        assert!(session_dir(Path::new("/b"), "../../etc").ends_with("etc"));
        assert_eq!(session_dir(Path::new("/b"), "../.."), Path::new("/b/session"));
    }

    #[test]
    fn the_definition_names_the_tool_and_offers_only_file_delivery() {
        let definition = tool_definition();
        assert_eq!(definition["name"], TOOL_NAME);
        assert_eq!(
            definition["inputSchema"]["properties"]["delivery"]["enum"],
            json!(["file"])
        );
        assert_eq!(definition["inputSchema"]["required"], json!(["name", "reason"]));
    }
}
