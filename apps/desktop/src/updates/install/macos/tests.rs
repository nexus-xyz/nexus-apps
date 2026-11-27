use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn app(root: &Path, name: &str, version: &str, marker: &str) -> PathBuf {
    let app = root.join(name);
    let contents = app.join("Contents");
    fs::create_dir_all(contents.join("MacOS")).unwrap();
    fs::create_dir_all(contents.join("Resources")).unwrap();
    fs::copy("/usr/bin/true", contents.join("MacOS").join(BIN)).unwrap();
    fs::write(contents.join("Resources/marker"), marker).unwrap();
    // CEF's framework uses symlinks: ditto must keep them, not copy their targets into ordinary files.
    symlink("marker", contents.join("Resources/current")).unwrap();
    fs::write(
        contents.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>xyz.nexus.desktop-gpui</string>
<key>CFBundleExecutable</key><string>{BIN}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
        ),
    )
    .unwrap();
    run(
        Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app),
        "sign the fixture",
    )
    .unwrap();
    app
}

fn image(root: &Path, source: &Path) -> Vec<u8> {
    let image = root.join("release.dmg");
    run(
        Command::new("/usr/bin/hdiutil")
            .args(["create", "-quiet", "-fs", "HFS+", "-format", "UDZO", "-srcfolder"])
            .arg(source)
            .arg(&image),
        "create the fixture disk image",
    )
    .unwrap();
    fs::read(image).unwrap()
}

fn marker(app: &Path) -> String {
    fs::read_to_string(app.join("Contents/Resources/marker")).unwrap()
}

#[test]
fn a_disk_image_replaces_a_renamed_bundle_and_relaunches_from_its_installed_path() {
    let root = tempfile::tempdir().unwrap();
    let current = app(root.path(), "My Nexus GPUI.app", "0.1.2", "old");
    let source = root.path().join("source");
    let new = app(&source, APPS[0], "9.9.9", "new");
    let exe = current.join("Contents/MacOS").join(BIN);
    assert!(super::super::packaged(&exe));
    let bytes = image(root.path(), &source);
    let relaunch = install(&exe, &bytes, "9.9.9").unwrap();
    assert_eq!(
        relaunch,
        Relaunch {
            program: exe,
            args: Vec::new(),
            restarted: true
        }
    );
    assert_eq!(marker(&current), "new");
    assert_eq!(
        fs::read_link(current.join("Contents/Resources/current")).unwrap(),
        Path::new("marker")
    );
    assert_eq!(
        fs::metadata(&current).unwrap().permissions().mode() & 0o777,
        fs::metadata(new).unwrap().permissions().mode() & 0o777
    );
    validate(&current, "9.9.9").unwrap();
    relaunch.start().unwrap();
}

#[test]
fn an_update_names_its_bundle_nexus_and_older_images_name_it_nexus_gpui() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(app_in(root.path()), None);
    let legacy = root.path().join("Nexus GPUI.app");
    fs::create_dir(&legacy).unwrap();
    assert_eq!(app_in(root.path()), Some(legacy));
    let current = root.path().join("Nexus.app");
    fs::create_dir(&current).unwrap();
    assert_eq!(app_in(root.path()), Some(current.clone()));
    // A link is never the bundle, so an image cannot point the update elsewhere.
    fs::remove_dir(&current).unwrap();
    symlink("/Applications", &current).unwrap();
    assert_eq!(app_in(root.path()), Some(root.path().join("Nexus GPUI.app")));
}

#[test]
fn invalid_images_and_wrong_versions_leave_the_current_app_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let current = app(root.path(), APPS[0], "0.1.2", "old");
    let exe = current.join("Contents/MacOS").join(BIN);
    assert!(install(&exe, b"broken disk image", "9.9.9").is_err());
    assert_eq!(marker(&current), "old");
    let source = root.path().join("source");
    app(&source, APPS[0], "9.9.8", "new");
    let bytes = image(root.path(), &source);
    assert!(
        install(&exe, &bytes, "9.9.9")
            .unwrap_err()
            .contains("CFBundleShortVersionString")
    );
    assert_eq!(marker(&current), "old");
    validate(&current, "0.1.2").unwrap();
}

#[test]
fn invalid_signatures_and_other_apps_are_refused_before_replacement() {
    let root = tempfile::tempdir().unwrap();
    let new = app(root.path(), APPS[0], "9.9.9", "new");
    validate(&new, "9.9.9").unwrap();
    fs::write(new.join("Contents/Resources/marker"), "tampered").unwrap();
    assert!(validate(&new, "9.9.9").unwrap_err().contains("verify the updated app"));
    run(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Set :CFBundleIdentifier xyz.other.app"])
            .arg(new.join("Contents/Info.plist")),
        "change the identity",
    )
    .unwrap();
    assert!(validate(&new, "9.9.9").unwrap_err().contains("CFBundleIdentifier"));
}

#[test]
fn a_failed_move_restores_the_previous_bundle() {
    let root = tempfile::tempdir().unwrap();
    let current = app(root.path(), APPS[0], "0.1.2", "old");
    let missing = root.path().join("missing.app");
    let backup = root.path().join("previous.app");
    assert!(move_bundle(&missing, &current, &backup).is_err());
    assert_eq!(marker(&current), "old");
    assert!(!backup.exists());
}

#[test]
fn privileged_moves_quote_paths_and_restore_the_old_app_on_failure() {
    let root = tempfile::Builder::new()
        .prefix("nexus ' $() `test` ")
        .tempdir()
        .unwrap();
    let current = app(root.path(), "Nexus ' \" GPUI.app", "0.1.2", "old");
    let backup = root.path().join("backup.app");
    let staged = root.path().join("missing.app");
    let command = privileged(&staged, &current, &backup);
    assert!(
        !Command::new("/bin/sh")
            .args(["-c", &command])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(marker(&current), "old");
    assert!(!backup.exists());
    app(root.path(), "missing.app", "9.9.9", "new");
    // Parse the actual AppleScript string literal without requesting administrator access in the test.
    let script = format!("do shell script {}", applescript(&command));
    run(
        Command::new("/usr/bin/osascript").args(["-e", &script]),
        "run the quoted install",
    )
    .unwrap();
    assert_eq!(marker(&current), "new");
    assert_eq!(marker(&backup), "old");
}

#[test]
fn a_relaunch_sets_the_restart_handoff_environment() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("launch");
    let report = root.path().join("report");
    fs::write(&script, "#!/bin/sh\nprintf '%s' \"$NEXUS_RESTARTED\" > \"$1\"\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    Relaunch {
        program: script,
        args: vec![report.to_str().unwrap().into()],
        restarted: true,
    }
    .start()
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !fs::read_to_string(&report).is_ok_and(|value| value == "1") {
        assert!(
            std::time::Instant::now() < deadline,
            "the relaunched copy did not get NEXUS_RESTARTED"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn development_binaries_and_helper_bundles_cannot_be_update_targets() {
    assert!(bundle(Path::new("/tmp/target/release/nexus-desktop-gpui")).is_none());
    assert!(
        bundle(Path::new(
            "/Applications/Nexus.app/Contents/Frameworks/helper.app/Contents/MacOS/helper"
        ))
        .is_none()
    );
    assert_eq!(
        bundle(Path::new("/Applications/Renamed.app/Contents/MacOS/nexus-desktop-gpui")),
        Some(Path::new("/Applications/Renamed.app"))
    );
}
