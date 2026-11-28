//! Where the app keeps its shared files: the data folder for the identifier `xyz.nexus.desktop`, which an older
//! Nexus install also uses, so both show the same Code sessions. Its `settings.json` is read and written in
//! `desktop_settings.rs`.

use std::path::PathBuf;

use serde_json::Value;

const APP_ID: &str = "xyz.nexus.desktop";

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|dir| !dir.is_empty()).map(PathBuf::from)
}

/// The person's own folder of `kind` (data, config or cache), as `resolve` finds it on this OS.
#[cfg(not(test))]
pub(crate) fn person_dir(
    kind: impl AsRef<std::path::Path>,
    resolve: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let _ = kind;
    resolve()
}

/// In a test, `kind` under [`test_root`]: no test reads or changes the person's own files.
#[cfg(test)]
pub(crate) fn person_dir(
    kind: impl AsRef<std::path::Path>,
    resolve: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let _ = resolve;
    Some(test_root().join(kind))
}

/// A test process's stand-in for the person's folders: one folder per process under the build's `target/`.
#[cfg(test)]
pub(crate) fn test_root() -> PathBuf {
    PathBuf::from(env!("OUT_DIR"))
        .join("test-folders")
        .join(std::process::id().to_string())
}

/// The app's data folder: the Code sessions' event log (`workbench.sqlite3`), plan accounts and attachments.
pub fn data_dir() -> Option<PathBuf> {
    person_dir(std::path::Path::new("data").join(APP_ID), || {
        nexus_workbench::accounts::desktop_data_dir(home().as_deref(), &env_dir)
    })
}

/// This build's own data folder, apart from the shared one: its crash file (`crash.rs`), so neither app takes the
/// other's crash.
pub fn own_data_dir() -> Option<PathBuf> {
    data_dir().and_then(|dir| dir.parent().map(|base| base.join("nexus-desktop-gpui")))
}

/// This build's own cache folder (the web engine's profile): nothing there is shared.
pub fn cache_dir() -> PathBuf {
    let base = person_dir("cache", || {
        if cfg!(target_os = "macos") {
            home().map(|home| home.join("Library/Caches"))
        } else if cfg!(windows) {
            env_dir("LOCALAPPDATA")
        } else {
            env_dir("XDG_CACHE_HOME").or_else(|| home().map(|home| home.join(".cache")))
        }
    });
    base.unwrap_or_else(std::env::temp_dir).join("nexus-desktop-gpui")
}

/// This build's own settings, apart from the Nexus app's `settings.json` (`desktop_settings.rs`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppSettings {
    /// `system`, `light` or `dark`.
    pub theme: Option<String>,
}

/// Where this build keeps its own settings: `nexus-desktop-gpui/settings.json` in the config folder.
pub(crate) fn app_settings_file() -> Option<PathBuf> {
    let base = person_dir("config", || {
        if cfg!(target_os = "macos") {
            home().map(|home| home.join("Library/Application Support"))
        } else if cfg!(windows) {
            env_dir("APPDATA")
        } else {
            env_dir("XDG_CONFIG_HOME").or_else(|| home().map(|home| home.join(".config")))
        }
    });
    base.map(|base| base.join("nexus-desktop-gpui").join("settings.json"))
}

/// Where this build keeps its prompt stash (`workspace/stash.rs`), beside its settings. The Nexus app keeps its own in
/// its webview's localStorage, which this build cannot read.
pub fn prompt_stash_file() -> Option<PathBuf> {
    app_settings_file().map(|file| file.with_file_name("prompt-stash.json"))
}

/// Where this build keeps when the person last saw each Code session (`workspace/seen.rs`), beside its settings.
pub fn seen_file() -> Option<PathBuf> {
    app_settings_file().map(|file| file.with_file_name("seen.json"))
}

/// Only Nexus session identifiers and credential fingerprints, never agent transcripts or credentials.
pub fn update_recovery_file() -> Option<PathBuf> {
    app_settings_file().map(|file| file.with_file_name("update-recovery.json"))
}

pub fn automatic_updates() -> bool {
    stored_setting("automaticUpdates")
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

pub fn save_automatic_updates(enabled: bool) -> Result<(), String> {
    try_update_app_settings(|stored| {
        stored.insert("automaticUpdates".into(), Value::Bool(enabled));
    })
}

/// Where this build records that the person has read the dictation privacy note (`workspace/mic.rs`), beside its
/// settings. The note shows once per device, as Nexus UI does with `nexus.dictation.notice`.
fn dictation_notice_file() -> Option<PathBuf> {
    app_settings_file().map(|file| file.with_file_name("dictation-notice"))
}

/// Whether the dictation privacy note was already shown on this device.
pub fn dictation_notice_told() -> bool {
    dictation_notice_file().is_some_and(|file| file.exists())
}

/// Records that the dictation privacy note was shown alongside a dictation that listened. A failed write only means
/// the note shows once more.
pub fn mark_dictation_notice_told() {
    if let Some(file) = dictation_notice_file() {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(file, "").ok();
    }
}

/// The microphone dictation uses, as the composer's microphone menu saved it on this computer; None for the system
/// default.
pub fn dictation_microphone() -> Option<crate::microphones::Choice> {
    let value = stored_setting("dictationMicrophone")?;
    Some(crate::microphones::Choice {
        id: value.get("id")?.as_str()?.to_string(),
        name: value.get("name")?.as_str()?.to_string(),
    })
}

/// Save the microphone dictation uses; None goes back to the system default.
pub fn save_dictation_microphone(choice: Option<&crate::microphones::Choice>) -> Result<(), String> {
    try_update_app_settings(|stored| match choice {
        Some(choice) => {
            stored.insert(
                "dictationMicrophone".into(),
                serde_json::json!({ "id": choice.id, "name": choice.name }),
            );
        }
        None => {
            stored.remove("dictationMicrophone");
        }
    })
}

/// Root for private, identity-scoped composer stores. Resolving it performs no I/O.
pub fn composer_drafts_root() -> Option<PathBuf> {
    app_settings_file().and_then(|file| file.parent().map(|dir| dir.join("composer-drafts")))
}

pub fn app_settings() -> AppSettings {
    let Some(text) = app_settings_file().and_then(|file| std::fs::read_to_string(file).ok()) else {
        return AppSettings::default();
    };
    let settings: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    AppSettings {
        theme: settings.get("theme").and_then(Value::as_str).map(String::from),
    }
}

/// Write the settings, keeping fields this version does not know. A failure is logged and answered: the choice still
/// holds until the app quits, and Settings says it was not kept.
pub fn save_app_settings(settings: &AppSettings) -> Result<(), String> {
    try_update_app_settings(|stored| match &settings.theme {
        Some(theme) => {
            stored.insert("theme".into(), Value::String(theme.clone()));
        }
        None => {
            stored.remove("theme");
        }
    })
}

/// The window's transparency (`frost.rs`): `opaque`, `frost`, `glass` or `custom`; None when never chosen.
pub fn transparency() -> Option<String> {
    stored_setting("transparency").and_then(|value| value.as_str().map(String::from))
}

/// The Fine-tune values (`frost::Frost`): see-through and blur, each 0 to 100. A `custom` choice reads them.
pub fn transparency_values() -> (Option<u64>, Option<u64>) {
    let read = |key: &str| stored_setting(key).and_then(|value| value.as_u64());
    (read("transparencySeeThrough"), read("transparencyBlur"))
}

pub fn save_transparency(level: &str, see_through: u8, blur: u8) -> Result<(), String> {
    try_update_app_settings(|stored| {
        stored.insert("transparency".into(), Value::String(level.into()));
        stored.insert("transparencySeeThrough".into(), Value::from(see_through));
        stored.insert("transparencyBlur".into(), Value::from(blur));
    })
}

/// The Gateway picked at sign-in on an internal build (`gatewayUrl`); the Nexus app's own is read after it.
pub fn app_gateway_url() -> Option<String> {
    app_settings_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings.get("gatewayUrl")?.as_str().map(String::from))
        .filter(|url| !url.trim().is_empty())
}

pub fn save_app_gateway_url(url: &str) {
    update_app_settings(|stored| {
        stored.insert("gatewayUrl".into(), Value::String(url.to_string()));
    });
}

/// A Gateway chosen from Local keeps its saved credential parked across app launches.
pub fn gateway_restore_suppressed(url: &str) -> bool {
    stored_setting("signedOutGateway")
        .and_then(|value| value.as_str().map(|saved| saved == url))
        .unwrap_or(false)
}

pub fn set_gateway_restore_suppressed(url: &str, suppressed: bool) -> Result<(), String> {
    try_update_app_settings(|settings| record_gateway_restore_policy(settings, url, suppressed))
}

fn record_gateway_restore_policy(settings: &mut serde_json::Map<String, Value>, url: &str, suppressed: bool) {
    if suppressed {
        settings.insert("signedOutGateway".into(), Value::String(url.into()));
    } else if settings.get("signedOutGateway").and_then(Value::as_str) == Some(url) {
        settings.remove("signedOutGateway");
    }
}

/// The device's subscription setup hint, independent of organization or session text.
pub fn setup_line() -> nexus_desktop_core::setup_line::State {
    let stored = app_settings_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    nexus_desktop_core::setup_line::State::read(&stored["setupLine"])
}

pub fn save_setup_line(state: nexus_desktop_core::setup_line::State) {
    update_app_settings(|stored| {
        stored.insert("setupLine".into(), serde_json::to_value(state).unwrap_or(Value::Null));
    });
}

/// The choices remembered for new Code sessions in `org` (`newSession.<org>`), as the Nexus app keeps them per
/// organization (`session-defaults.ts`): the models and their efforts are an organization's policy.
pub fn new_session_defaults(org: &str) -> Value {
    app_settings_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings.get("newSession")?.get(org).cloned())
        .unwrap_or(Value::Null)
}

/// Remember `defaults` as `org`'s new-session choices.
pub fn save_new_session_defaults(org: &str, defaults: Value) {
    update_app_settings(|stored| {
        let orgs = stored
            .entry("newSession")
            .or_insert_with(|| Value::Object(Default::default()));
        if !orgs.is_object() {
            *orgs = Value::Object(Default::default());
        }
        if let Some(orgs) = orgs.as_object_mut() {
            orgs.insert(org.to_string(), defaults);
        }
    });
}

/// The left sidebar's width, as the person last left it (`sidebarWidth`).
pub fn sidebar_width() -> Option<f32> {
    stored_setting("sidebarWidth")
        .and_then(|width| width.as_f64())
        .map(|width| width as f32)
}

pub fn save_sidebar_width(width: f32) {
    update_app_settings(|stored| {
        stored.insert("sidebarWidth".into(), Value::from(width.round() as i64));
    });
}

/// The system's last light or dark the app saw (`lastAppearance`: `dark` or `light`), for `appearance.rs`.
pub fn last_appearance_dark() -> Option<bool> {
    match stored_setting("lastAppearance")?.as_str()? {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    }
}

pub fn save_last_appearance_dark(dark: bool) {
    update_app_settings(|stored| {
        stored.insert(
            "lastAppearance".into(),
            Value::from(if dark { "dark" } else { "light" }),
        );
    });
}

/// Whether the sidebar's Recent group is folded to its newest sessions (`recentFolded`). Open by default.
pub fn recent_folded() -> bool {
    stored_setting("recentFolded")
        .and_then(|folded| folded.as_bool())
        .unwrap_or(false)
}

pub fn save_recent_folded(folded: bool) {
    update_app_settings(|stored| {
        stored.insert("recentFolded".into(), Value::Bool(folded));
    });
}

/// The Code session side pane's width, as the person last left it (`sidePaneWidth`).
pub fn side_pane_width() -> Option<f32> {
    app_settings_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings.get("sidePaneWidth")?.as_f64())
        .map(|width| width as f32)
}

pub fn save_side_pane_width(width: f32) {
    update_app_settings(|stored| {
        stored.insert("sidePaneWidth".into(), Value::from(width.round() as i64));
    });
}

/// Whether crash reports go without asking (`errorReports`, Settings, Appearance): Nexus UI's "Send crash and error
/// reports", kept per computer as the Nexus app keeps it. Off by default: a desktop crash report carries the person's
/// name, email and organization, and the open build is opt-in (PRIVACY.md, "Crash reports"). Nexus UI on
/// the web keeps its own default.
pub fn error_reports() -> bool {
    reports_on(stored_setting("errorReports"))
}

fn reports_on(stored: Option<Value>) -> bool {
    stored.and_then(|on| on.as_bool()).unwrap_or(false)
}

pub fn save_error_reports(on: bool) -> Result<(), String> {
    try_update_app_settings(|stored| {
        stored.insert("errorReports".into(), Value::Bool(on));
    })
}

/// The person's "Send usage data" choice (`usageData`, Settings, Privacy), kept per computer. None until they choose.
pub fn usage_data() -> Option<bool> {
    app_settings_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings.get("usageData")?.as_bool())
}

/// Saves the choice and applies it. On: a new install ID. Off: the ID and the day not sent are deleted, and the ID
/// comes back to be shown one last time (PRIVACY.md, "The install ID").
pub fn save_usage_data(on: bool) -> Result<Option<String>, String> {
    try_update_app_settings(|stored| {
        stored.insert("usageData".into(), Value::Bool(on));
    })?;
    apply_usage_data();
    let Some(dir) = own_data_dir() else {
        return Ok(None);
    };
    // Off first, then forgotten: the loop stops before the files go. On: always a fresh ID.
    let forgotten = nexus_desktop_core::daily_active::forget(&dir);
    // The install ID exists only for daily_active, so it is made only while that is on.
    if on && nexus_desktop_core::usage::allowed() && nexus_desktop_core::daily_active::enabled() {
        nexus_desktop_core::daily_active::ensure_install_id(&dir);
        return Ok(None);
    }
    Ok(forgotten)
}

/// Whether to ask the first-start question: `NEXUS_DESKTOP_DAILY_ACTIVE` on, a release build, no choice saved, and no
/// environment variable that turns usage data off (PRIVACY.md: either variable wins, and the app does not ask).
pub fn asks_usage_data() -> bool {
    nexus_desktop_core::daily_active::enabled()
        && crate::channel::RELEASE_BUILD
        && usage_data().is_none()
        && nexus_desktop_core::usage::decide(Some(true), |name| std::env::var(name).ok(), true)
}

/// This install's ID while usage data is on (Settings, Privacy).
pub fn install_id() -> Option<String> {
    own_data_dir().and_then(|dir| nexus_desktop_core::daily_active::install_id(&dir))
}

/// Sets `nexus_desktop_core::usage` from the choice, the environment and the kind of build: at start and on a change.
pub fn apply_usage_data() {
    use nexus_desktop_core::usage;
    usage::set(usage::decide(
        usage_data(),
        |name| std::env::var(name).ok(),
        crate::channel::RELEASE_BUILD,
    ));
}

/// The new session's background (`newSessionBackground`, `workspace/backdrop.rs`), kept per computer.
pub fn new_session_background() -> Option<Value> {
    stored_setting("newSessionBackground")
}

pub fn save_new_session_background(choice: Value) {
    update_app_settings(|stored| {
        stored.insert("newSessionBackground".into(), choice);
    });
}

/// Whether this profile has chosen to open the workspace without an account.
pub fn local_workspace_remembered() -> bool {
    stored_setting("localWorkspace")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub fn remember_local_workspace() {
    update_app_settings(|settings| {
        settings.insert("localWorkspace".into(), Value::Bool(true));
    });
}

/// First-run onboarding state saved on this computer.
pub fn onboarding() -> Option<Value> {
    stored_setting("onboarding")
}

pub fn save_onboarding(state: Value) {
    update_app_settings(|stored| {
        stored.insert("onboarding".into(), state);
    });
}

pub fn first_connection() -> Option<Value> {
    stored_setting("firstConnection")
}

pub fn save_first_connection(state: Value) {
    update_app_settings(|stored| {
        stored.insert("firstConnection".into(), state);
    });
}

/// Settings › Appearance's text size (`zoom`): 1 is 100%. The Nexus app keeps its zoom per computer too.
pub fn zoom() -> f32 {
    stored_setting("zoom")
        .and_then(|value| value.as_f64())
        .map(|zoom| zoom as f32)
        .unwrap_or(1.)
}

/// Keep the zoom; 100% removes the key, as the Nexus app does.
pub fn save_zoom(zoom: f32) {
    update_app_settings(|stored| {
        if zoom == 1. {
            stored.remove("zoom");
        } else {
            stored.insert("zoom".into(), Value::from((zoom as f64 * 100.).round() / 100.));
        }
    });
}

/// Settings › Appearance's "Reduce motion" (`reduceMotion`). Off by default.
pub fn reduce_motion() -> bool {
    stored_setting("reduceMotion")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub fn save_reduce_motion(on: bool) -> Result<(), String> {
    try_update_app_settings(|stored| {
        if on {
            stored.insert("reduceMotion".into(), Value::Bool(true));
        } else {
            stored.remove("reduceMotion");
        }
    })
}

/// Settings › Appearance's "Assistant sounds" (`assistantSounds`): the soft sounds when the person sends and when Nexus
/// replies (`sounds.rs`). On by default; off keeps `false`.
pub fn assistant_sounds() -> bool {
    stored_setting("assistantSounds")
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

pub fn save_assistant_sounds(on: bool) -> Result<(), String> {
    try_update_app_settings(|stored| {
        if on {
            stored.remove("assistantSounds");
        } else {
            stored.insert("assistantSounds".into(), Value::Bool(false));
        }
    })
}

/// Where the clones of each machine go: `cloneParent.<machine>`, `""` for this computer. The Nexus app keeps the
/// same map in its window's local storage (`clone.ts`, `nexus.wb.cloneParent`), which no other process reads.
const CLONE_PARENT: &str = "cloneParent";

/// The folder the last clone on `machine` (None: this computer) went under, when one was remembered.
pub fn clone_parent(machine: Option<&str>) -> Option<String> {
    saved_parent(stored_setting(CLONE_PARENT).as_ref(), machine)
}

/// Remember `parent` as where `machine`'s next clone goes (`clone.ts` `saveParent`).
pub fn save_clone_parent(machine: Option<&str>, parent: &str) {
    update_app_settings(|stored| remember_parent(stored, machine, parent));
}

fn saved_parent(parents: Option<&Value>, machine: Option<&str>) -> Option<String> {
    parents?
        .get(machine.unwrap_or_default())?
        .as_str()
        .map(str::trim)
        .filter(|parent| !parent.is_empty())
        .map(str::to_string)
}

fn remember_parent(stored: &mut serde_json::Map<String, Value>, machine: Option<&str>, parent: &str) {
    let parents = stored
        .entry(CLONE_PARENT)
        .or_insert_with(|| Value::Object(Default::default()));
    if !parents.is_object() {
        *parents = Value::Object(Default::default());
    }
    if let Some(parents) = parents.as_object_mut() {
        parents.insert(
            machine.unwrap_or_default().to_string(),
            Value::String(parent.trim().to_string()),
        );
    }
}

/// The app window's last place, for the next launch (`lastPlace`, `workspace/last_place.rs`). The Nexus app keeps its
/// own in its window's local storage (`nexus.desktop.lastPath`), which no other process reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastPlace {
    /// Nexus UI's path: `/app/<org>/<page>`.
    pub path: String,
    /// A chat's title, shown while it opens.
    pub title: Option<String>,
    /// A Code session on a server or a cloud machine, which may be read only after the launch.
    pub machine: bool,
}

const LAST_PLACE: &str = "lastPlace";

pub fn last_place() -> Option<LastPlace> {
    read_last_place(stored_setting(LAST_PLACE).as_ref())
}

pub fn save_last_place(place: &LastPlace) {
    if last_place().as_ref() == Some(place) {
        return;
    }
    update_app_settings(|stored| {
        stored.insert(LAST_PLACE.into(), last_place_value(place));
    });
}

/// Signed out: the next launch opens the home.
pub fn forget_last_place() {
    if stored_setting(LAST_PLACE).is_none() {
        return;
    }
    update_app_settings(|stored| {
        stored.remove(LAST_PLACE);
    });
}

fn read_last_place(value: Option<&Value>) -> Option<LastPlace> {
    let value = value?;
    let path = value.get("path")?.as_str().filter(|path| !path.is_empty())?;
    Some(LastPlace {
        path: path.to_string(),
        title: value.get("title").and_then(Value::as_str).map(str::to_string),
        machine: value.get("machine").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn last_place_value(place: &LastPlace) -> Value {
    let mut value = serde_json::Map::new();
    value.insert("path".into(), Value::String(place.path.clone()));
    if let Some(title) = &place.title {
        value.insert("title".into(), Value::String(title.clone()));
    }
    if place.machine {
        value.insert("machine".into(), Value::Bool(true));
    }
    Value::Object(value)
}

/// Pending diff comments, scoped to Gateway, organization, and person.
pub fn diff_comment_drafts(
    scope: &str,
) -> std::collections::HashMap<String, Vec<nexus_workbench::diff_comment::DiffComment>> {
    read_diff_comment_drafts(stored_setting("diffCommentDrafts").as_ref(), scope)
}

fn read_diff_comment_drafts(
    value: Option<&Value>,
    scope: &str,
) -> std::collections::HashMap<String, Vec<nexus_workbench::diff_comment::DiffComment>> {
    value
        .and_then(|value| value.get(scope))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(thread, value)| {
            let comments: Vec<nexus_workbench::diff_comment::DiffComment> = value
                .as_array()?
                .iter()
                .filter_map(|value| serde_json::from_value(value.clone()).ok())
                .collect();
            (!comments.is_empty()).then(|| (thread.clone(), comments))
        })
        .collect()
}

pub fn save_diff_comment_drafts(
    scope: &str,
    drafts: &std::collections::HashMap<String, Vec<nexus_workbench::diff_comment::DiffComment>>,
) {
    update_app_settings(|stored| {
        let scopes = stored
            .entry("diffCommentDrafts")
            .or_insert_with(|| Value::Object(Default::default()));
        if !scopes.is_object() {
            *scopes = Value::Object(Default::default());
        }
        if let Some(scopes) = scopes.as_object_mut() {
            if drafts.is_empty() {
                scopes.remove(scope);
            } else if let Ok(value) = serde_json::to_value(drafts) {
                scopes.insert(scope.to_string(), value);
            }
        }
    });
}

fn stored_setting(key: &str) -> Option<Value> {
    app_settings_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings.get(key).cloned())
}

/// Read this build's settings file, change it, and write it back, keeping fields this version does not know.
fn update_app_settings(change: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    // Logged by the try: these settings are kept quietly.
    let _ = try_update_app_settings(change);
}

/// [`update_app_settings`], answering why a write failed, for a setting whose row says so.
fn try_update_app_settings(change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> Result<(), String> {
    let Some(file) = app_settings_file() else {
        return Err("This computer has no config folder.".into());
    };
    update_settings_at(&file, change)
}

/// Read the settings file at `file`, change it and write it back. A failed write leaves the file as it was.
fn update_settings_at(
    file: &std::path::Path,
    change: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Result<(), String> {
    let mut stored: serde_json::Map<String, Value> = std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    change(&mut stored);
    // Replace only after the full new value reaches disk. A failed write must not truncate the saved value.
    let written = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        nexus_desktop_core::atomic_file::write_atomic(file, &serde_json::to_vec_pretty(&Value::Object(stored))?)
    })();
    written.map_err(|error| {
        log::warn!("settings not saved to {}: {error}", file.display());
        format!("Couldn't save it: {error}.")
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{LastPlace, last_place_value, read_last_place, remember_parent, saved_parent, update_settings_at};

    #[test]
    fn local_gateway_restore_policy_is_saved_until_sign_in_to_that_gateway() {
        let file = super::test_root().join("local-gateway-restore-test.json");
        let parked = "http://localhost:44221";
        let other = "http://localhost:44222";
        let read =
            || -> Value { serde_json::from_slice(&std::fs::read(&file).expect("saved settings")).expect("JSON") };
        update_settings_at(&file, |settings| {
            super::record_gateway_restore_policy(settings, parked, true)
        })
        .unwrap();
        update_settings_at(&file, |settings| {
            super::record_gateway_restore_policy(settings, other, false)
        })
        .unwrap();
        assert_eq!(read()["signedOutGateway"].as_str(), Some(parked));
        update_settings_at(&file, |settings| {
            super::record_gateway_restore_policy(settings, parked, false)
        })
        .unwrap();
        assert!(read().get("signedOutGateway").is_none());
    }

    /// A test never reaches the person's own folders: each of them resolves under the test root.
    #[test]
    fn every_folder_of_the_person_resolves_under_the_test_root() {
        let root = super::test_root();
        let folders = [
            ("data_dir", super::data_dir()),
            ("own_data_dir", super::own_data_dir()),
            ("cache_dir", Some(super::cache_dir())),
            ("app_settings_file", super::app_settings_file()),
            ("prompt_stash_file", super::prompt_stash_file()),
            ("seen_file", super::seen_file()),
            ("dictation_notice_file", super::dictation_notice_file()),
            ("composer_drafts_root", super::composer_drafts_root()),
            ("desktop_settings::file", crate::desktop_settings::file()),
        ];
        for (name, folder) in folders {
            let folder = folder.unwrap_or_else(|| panic!("{name} resolves to nothing"));
            assert!(
                folder.starts_with(&root),
                "{name} resolves outside the test root {}: {}",
                root.display(),
                folder.display()
            );
        }
    }

    /// Crash reports are off for a person who never chose, and an explicit On holds.
    #[test]
    fn crash_reports_are_off_until_turned_on() {
        assert!(!super::reports_on(None), "off by default");
        assert!(super::reports_on(Some(json!(true))), "an explicit On stays on");
        assert!(!super::reports_on(Some(json!(false))));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_answers_why_and_keeps_the_saved_file() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        update_settings_at(&file, |stored| {
            stored.insert("reduceMotion".into(), Value::Bool(true));
        })
        .unwrap();
        let saved = std::fs::read_to_string(&file).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = update_settings_at(&file, |stored| {
            stored.insert("reduceMotion".into(), Value::Bool(false));
        });
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = result.unwrap_err();
        assert!(error.starts_with("Couldn't save it:"), "{error}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), saved);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn each_machine_remembers_where_its_last_clone_went() {
        let mut stored = Map::new();
        assert_eq!(saved_parent(stored.get("cloneParent"), None), None);
        remember_parent(&mut stored, None, " ~/code ");
        remember_parent(&mut stored, Some("m1"), "/srv/work");
        assert_eq!(saved_parent(stored.get("cloneParent"), None).as_deref(), Some("~/code"));
        assert_eq!(
            saved_parent(stored.get("cloneParent"), Some("m1")).as_deref(),
            Some("/srv/work")
        );
        assert_eq!(
            saved_parent(stored.get("cloneParent"), Some("m2")),
            None,
            "another server starts at home"
        );
        // The next clone there replaces it; the other machine's stays.
        remember_parent(&mut stored, Some("m1"), "/srv/other");
        assert_eq!(stored["cloneParent"], json!({ "": "~/code", "m1": "/srv/other" }));
    }

    #[test]
    fn a_broken_or_blank_entry_reads_as_none() {
        let mut stored = Map::new();
        stored.insert("cloneParent".into(), Value::String("not a map".into()));
        assert_eq!(saved_parent(stored.get("cloneParent"), None), None);
        remember_parent(&mut stored, None, "~/src");
        assert_eq!(stored["cloneParent"], json!({ "": "~/src" }));
        assert_eq!(saved_parent(Some(&json!({ "": "  " })), None), None);
    }

    #[test]
    fn the_last_place_round_trips_and_a_broken_one_reads_as_none() {
        let place = LastPlace {
            path: "/app/acme/sessions/c_1".into(),
            title: Some("Fix the login".into()),
            machine: false,
        };
        let value = last_place_value(&place);
        assert_eq!(
            value,
            json!({ "path": "/app/acme/sessions/c_1", "title": "Fix the login" })
        );
        assert_eq!(read_last_place(Some(&value)), Some(place));
        let code = LastPlace {
            path: "/app/acme/code/t_1".into(),
            title: None,
            machine: true,
        };
        assert_eq!(read_last_place(Some(&last_place_value(&code))), Some(code));
        assert_eq!(read_last_place(None), None);
        assert_eq!(read_last_place(Some(&json!({ "path": "" }))), None);
        assert_eq!(read_last_place(Some(&json!("/app/acme"))), None);
    }
}

#[cfg(test)]
mod diff_comment_tests {
    use super::read_diff_comment_drafts;
    use serde_json::json;

    #[test]
    fn drafts_stay_in_their_account_scope_and_bad_entries_do_not_hide_valid_comments() {
        let comment = json!({"id":"c", "path":"src/a.rs", "range":"line 1", "quote":"+x", "text":"Why?"});
        let value = json!({"alice": {"thread": [comment.clone(), {"text": 3}], "bad": false},
            "bob": {"other": [comment]}});
        let alice = read_diff_comment_drafts(Some(&value), "alice");
        assert_eq!(alice.len(), 1);
        assert_eq!(alice["thread"].len(), 1);
        assert_eq!(alice["thread"][0].quote, "+x");
        assert!(!alice.contains_key("other"));
        assert!(read_diff_comment_drafts(Some(&value), "charlie").is_empty());
        assert!(read_diff_comment_drafts(None, "alice").is_empty());
    }
}

/// Follow mode belongs to the session, across windows. It does not change execution permissions.
static WORKBENCH_MODES: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<String, bool>>> =
    std::sync::LazyLock::new(|| {
        let settings = app_settings_file()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let modes = settings
            .and_then(|value| {
                value["workbenchModes"]
                    .as_object()
                    .map(|values| values.iter().map(|(id, mode)| (id.clone(), mode == "manual")).collect())
            })
            .unwrap_or_default();
        std::sync::RwLock::new(modes)
    });
pub fn workbench_manual(id: &str) -> bool {
    WORKBENCH_MODES
        .read()
        .ok()
        .is_some_and(|modes| modes.get(id).copied().unwrap_or(false))
}
pub fn save_workbench_manual(id: &str, manual: bool) {
    if let Ok(mut modes) = WORKBENCH_MODES.write() {
        modes.insert(id.into(), manual);
    }
    update_app_settings(|stored| {
        let modes = stored.entry("workbenchModes").or_insert_with(|| serde_json::json!({}));
        if !modes.is_object() {
            *modes = serde_json::json!({});
        }
        modes[id] = Value::from(if manual { "manual" } else { "follow" });
    });
}
