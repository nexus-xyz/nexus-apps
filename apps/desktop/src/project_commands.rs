//! Project commands where a session runs them (the Nexus app's `project-scripts.ts`): the Code top bar's ▶ menu and the
//! search palette's Project commands. Each runs in a new terminal of the session's folder, named after it. The lists
//! are kept here by project once read, so a render reads them without waiting. Settings › Worktrees edits them
//! (`pages/project_editor.rs`), and what it saves shows here at once. The rules (`unavailable`, `read`) are
//! `nexus_workbench::scripts`'s, shared with the Nexus app. A server's project keeps its commands in its host's store,
//! so they are read there (`wb_project_scripts`).

use std::collections::{HashMap, HashSet};

use gpui_kit::{App, Global, Keystroke};
use nexus_workbench::scripts::{self, ProjectScript};

/// Each project's commands as last read, and the projects being read.
#[derive(Default)]
struct Lists {
    lists: HashMap<String, Vec<ProjectScript>>,
    loading: HashSet<String>,
}

impl Global for Lists {}

/// The project's commands as last read; none until then.
pub fn of(project: &str, cx: &App) -> Vec<ProjectScript> {
    cx.try_global::<Lists>()
        .and_then(|lists| lists.lists.get(project).cloned())
        .unwrap_or_default()
}

/// Read the project's commands once, off the UI thread; every window draws again when they arrive.
pub fn load(project: &str, cx: &mut App) {
    let known = cx
        .try_global::<Lists>()
        .is_some_and(|lists| lists.lists.contains_key(project));
    if !known {
        reload(project, cx);
    }
}

/// Read them again, as when the ▶ menu opens: the Nexus app may have changed them. One read at a time per project.
pub fn reload(project: &str, cx: &mut App) {
    let lists = cx.default_global::<Lists>();
    if !lists.loading.insert(project.to_string()) {
        return;
    }
    let project = project.to_string();
    // A server's project keeps its commands in its host's store: read there, through its relay.
    let read: futures::future::LocalBoxFuture<'static, Option<Vec<ProjectScript>>> =
        match on_server(&crate::machines::get(), &project) {
            Some(reply) => Box::pin(async move { reply.await.ok() }),
            None => {
                let project = project.clone();
                Box::pin(cx.background_executor().spawn(async move { read(&project) }))
            }
        };
    cx.spawn(async move |cx| {
        let list = read.await;
        cx.update(|cx| {
            let lists = cx.default_global::<Lists>();
            lists.loading.remove(&project);
            // A store that does not open, or a host that does not answer, keeps what was read before, or none: read
            // once, so a render asks no more.
            match list {
                Some(list) => {
                    lists.lists.insert(project, list);
                }
                None => {
                    lists.lists.entry(project).or_default();
                }
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// What Settings saved: the top bar and the palette follow at once.
pub fn saved(project: &str, list: Vec<ProjectScript>, cx: &mut App) {
    cx.default_global::<Lists>().lists.insert(project.to_string(), list);
    cx.refresh_windows();
}

/// The read of a server project's commands from its host (`wb_project_scripts`); None for a project on this computer.
fn on_server(
    machines: &crate::machines::Machines,
    project: &str,
) -> Option<crate::pages::project_host::Reply<Vec<ProjectScript>>> {
    let machine = machines.machine_of(project)?;
    Some(crate::pages::project_host::on_server(
        machines,
        &machine,
        "wb_project_scripts",
        serde_json::json!({ "projectId": project }),
    ))
}

/// The project's commands from the store on this computer; None when it does not open.
fn read(project: &str) -> Option<Vec<ProjectScript>> {
    let store = crate::local_store::open().ok().flatten()?;
    let saved = store.project_scripts(project).ok()?;
    Some(scripts::read(saved.as_deref()))
}

/// A written key (`Mod+Alt+Shift+KeyR`): Alt, Shift and the key's name. None for one this build can't read.
fn parse(text: &str) -> Option<(bool, bool, &str)> {
    let mut parts: Vec<&str> = text.split('+').collect();
    let code = parts.pop()?;
    if parts.first() != Some(&"Mod") {
        return None;
    }
    let name = code.trim_start_matches("Key").trim_start_matches("Digit");
    Some((parts.contains(&"Alt"), parts.contains(&"Shift"), name))
}

/// The key's caps in order: Mod, then Alt and Shift, then the key.
fn caps(text: &str, mac: bool) -> Option<Vec<&str>> {
    let (alt, shift, name) = parse(text)?;
    let (ctrl, alt_name, shift_name) = if mac {
        ("⌘", "⌥", "⇧")
    } else {
        ("Ctrl", "Alt", "Shift")
    };
    let mut caps = vec![ctrl];
    caps.extend(alt.then_some(alt_name));
    caps.extend(shift.then_some(shift_name));
    caps.push(name);
    Some(caps)
}

/// How a written key reads: `Ctrl+Alt+R`, `⌘⌥R` on a Mac (the Nexus app's `keyLabel`). Empty for one this build can't
/// read.
pub fn key_label(text: &str, mac: bool) -> String {
    caps(text, mac).map_or_else(String::new, |caps| caps.join(if mac { "" } else { "+" }))
}

/// The key as the palette's keycaps, spaced (`Ctrl Alt R`, `⌘ ⌥ R`), as the Nexus app's `keyCaps`. None for no key,
/// or one this build can't read.
pub fn key_caps(text: Option<&str>, mac: bool) -> Option<String> {
    caps(text?, mac).map(|caps| caps.join(" "))
}

/// A written key the Nexus app accepts (`script-keys.ts`'s `parseKey`): Mod, then Alt or Shift or both, then a
/// letter, a digit or F1 to F12. Alt, Shift and the key's place (`KeyD`, `Digit1`, `F5`).
fn parse_strict(text: &str) -> Option<(bool, bool, &str)> {
    let mut parts: Vec<&str> = text.trim().split('+').collect();
    let code = parts.pop()?;
    if parts.first() != Some(&"Mod") {
        return None;
    }
    let rest = &parts[1..];
    let (alt, shift) = (rest.contains(&"Alt"), rest.contains(&"Shift"));
    if rest.len() != usize::from(alt) + usize::from(shift) || !(alt || shift) {
        return None;
    }
    let one = |tail: &str, ok: fn(&u8) -> bool| tail.len() == 1 && ok(&tail.as_bytes()[0]);
    let valid = code
        .strip_prefix("Key")
        .is_some_and(|tail| one(tail, u8::is_ascii_uppercase))
        || code
            .strip_prefix("Digit")
            .is_some_and(|tail| one(tail, u8::is_ascii_digit))
        || code
            .strip_prefix('F')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=12).contains(&n));
    valid.then_some((alt, shift, code))
}

/// The command whose own key `keystroke` is, from the project's list (`script-keys.ts`'s `scriptFor`), with whether
/// that key has Alt. It matches the key's place, not what it types, so ⇧1 (`!`) and ⌥D (`∂`) still match.
/// `mac`: Mod is ⌘ and Ctrl is not held, else Ctrl and ⌘ is not held.
pub fn command_for<'a>(
    list: &'a [ProjectScript],
    keystroke: &Keystroke,
    mac: bool,
) -> Option<(&'a ProjectScript, bool)> {
    let modifiers = keystroke.modifiers;
    let mod_key = if mac {
        modifiers.platform && !modifiers.control
    } else {
        modifiers.control && !modifiers.platform
    };
    if !mod_key {
        return None;
    }
    let place = crate::key_bindings::key_like(keystroke).code?;
    list.iter().find_map(|script| {
        let (alt, shift, code) = parse_strict(script.shortcut.as_deref()?)?;
        (modifiers.alt == alt && modifiers.shift == shift && place == code).then_some((script, alt))
    })
}

/// Whether a command's key runs while the cursor is in a text box (`worksWhileTyping`). Off a Mac, Ctrl+Alt is AltGr
/// on Windows and many Linux layouts, so the text box keeps it. ⌘⌥ on a Mac types nothing.
pub fn runs_while_typing(alt: bool, mac: bool) -> bool {
    mac || !alt
}

#[cfg(test)]
mod tests {
    use super::{command_for, key_caps, key_label, runs_while_typing};
    use gpui_kit::{Keystroke, Modifiers};
    use nexus_workbench::scripts::ProjectScript;

    fn script(id: &str, shortcut: Option<&str>) -> ProjectScript {
        ProjectScript {
            id: id.into(),
            name: id.into(),
            command: format!("run {id}"),
            shortcut: shortcut.map(String::from),
        }
    }

    fn press(key: &str, platform: bool, control: bool, alt: bool, shift: bool) -> Keystroke {
        Keystroke {
            key: key.into(),
            key_char: None,
            modifiers: Modifiers {
                platform,
                control,
                alt,
                shift,
                function: false,
            },
        }
    }

    #[test]
    fn a_key_press_finds_its_command_as_the_nexus_app_does() {
        let list = [
            script("dev", Some("Mod+Alt+KeyD")),
            script("test", Some("Mod+Shift+KeyR")),
            script("build", Some("Mod+Alt+Shift+F5")),
            script("one", Some("Mod+Shift+Digit1")),
            script("none", None),
            script("bad", Some("Mod+KeyB")),
        ];
        let hit = |keystroke: Keystroke, mac: bool| command_for(&list, &keystroke, mac).map(|(s, _)| s.id.clone());
        assert_eq!(hit(press("d", true, false, true, false), true).as_deref(), Some("dev"));
        assert_eq!(
            hit(press("r", false, true, false, true), false).as_deref(),
            Some("test")
        );
        assert_eq!(
            hit(press("f5", true, false, true, true), true).as_deref(),
            Some("build")
        );
        // Extra Shift, the other platform's Mod, both Mods, or Mod alone: nothing.
        assert_eq!(hit(press("d", true, false, true, true), true), None);
        assert_eq!(hit(press("d", false, true, true, false), true), None);
        assert_eq!(hit(press("d", true, true, true, false), true), None);
        assert_eq!(hit(press("b", true, false, false, false), true), None);
        assert_eq!(hit(press("e", true, false, true, false), true), None);
    }

    /// A server's project reads its commands from its host; a project here does not ask any server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_server_projects_commands_are_read_on_its_host() {
        use crate::machines::tests::{Fake, machines, settings, until};
        let fake = Fake::new(nexus_workbench::api::PROTOCOL);
        let machines = machines(&fake);
        machines.sync(&settings(&[("m1", "me@box")]));
        until("the overview", || !machines.projects().is_empty()).await;
        let list = vec![script("dev", Some("Mod+Alt+KeyD")), script("test", None)];
        if let Ok(mut host) = fake.host.lock() {
            host.scripts = serde_json::json!(list);
        }
        let read = super::on_server(&machines, "p1").expect("p1 is the server's");
        assert_eq!(read.await, Ok(list));
        assert!(super::on_server(&machines, "p_here").is_none());
    }

    #[test]
    fn ctrl_alt_stays_with_a_text_box_off_a_mac() {
        assert!(!runs_while_typing(true, false));
        assert!(runs_while_typing(true, true));
        assert!(runs_while_typing(false, false));
    }

    #[test]
    fn a_key_reads_as_the_nexus_app_shows_it() {
        assert_eq!(key_label("Mod+Alt+KeyR", false), "Ctrl+Alt+R");
        assert_eq!(key_label("Mod+Shift+Digit1", true), "⌘⇧1");
        assert_eq!(key_label("Alt+KeyR", false), "");
        assert_eq!(key_caps(Some("Mod+Alt+KeyD"), false).as_deref(), Some("Ctrl Alt D"));
        assert_eq!(key_caps(Some("Mod+Alt+Shift+F5"), true).as_deref(), Some("⌘ ⌥ ⇧ F5"));
        assert_eq!(key_caps(Some("Alt+KeyR"), false), None);
        assert_eq!(key_caps(None, false), None);
    }
}
