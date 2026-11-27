use super::*;

#[test]
fn missing_browsers_do_not_report_access_failures() {
    let home = std::env::temp_dir().join(format!("nexus-cookie-missing-{}", uuid::Uuid::new_v4()));
    let found = discover(&home);
    assert!(found.sources.is_empty());
    assert!(found.warnings.is_empty());
    assert_eq!(
        found.select(Some("chrome:Default")).err().as_deref(),
        Some("Choose a browser profile.")
    );
}

#[test]
#[cfg(unix)]
fn permission_denied_includes_eperm_and_eacces_and_survives_selection() {
    let mut found = Discovery::default();
    // macOS privacy protection returns EPERM, while Unix file permissions usually return EACCES.
    for code in [1, 13] {
        found.warn("Chrome", io::Error::from_raw_os_error(code));
    }
    assert_eq!(found.warnings.len(), 1, "repeat failures must not repeat the warning");
    let warning = &found.warnings[0];
    assert!(warning.contains("Chrome profiles"));
    assert!(warning.contains("sign in directly"));
    if cfg!(target_os = "macos") {
        assert!(warning.contains("macOS blocked access"));
        assert!(warning.contains("System Settings → Privacy & Security"));
    }
    let warning = warning.clone();
    assert_eq!(found.select(Some("chrome:Default")).err(), Some(warning));
}

#[test]
fn discovery_warnings_do_not_expose_filesystem_paths() {
    let mut found = Discovery::default();
    found.warn(
        "Firefox",
        io::Error::other("read failed at /private/profile/cookies.sqlite"),
    );
    assert_eq!(found.warnings.len(), 1);
    assert!(found.warnings[0].contains("could not read Firefox profiles"));
    assert!(!found.warnings[0].contains("/private"));
}

#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_failed_browser_does_not_hide_other_profiles_and_retry_recovers() {
    let home = std::env::temp_dir().join(format!("nexus-cookie-partial-{}", uuid::Uuid::new_v4()));
    let chrome = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Google/Chrome")
    } else {
        home.join(".config/google-chrome")
    };
    let firefox = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Firefox")
    } else {
        home.join(".mozilla/firefox")
    };
    std::fs::create_dir_all(chrome.parent().expect("parent")).expect("parent");
    // A non-directory root forces a real read_dir failure without depending on the test runner's privileges.
    std::fs::write(&chrome, b"").expect("unreadable root");
    std::fs::create_dir_all(firefox.join("Profiles/test")).expect("profile");
    std::fs::write(firefox.join("Profiles/test/cookies.sqlite"), b"").expect("cookies");
    std::fs::write(
        firefox.join("profiles.ini"),
        "[Profile0]\nName=Test\nIsRelative=1\nPath=Profiles/test\n",
    )
    .expect("profiles.ini");

    let found = discover(&home);
    assert_eq!(found.warnings.len(), 1);
    assert!(found.warnings[0].contains("Chrome"));
    assert_eq!(found.sources.len(), 1);
    assert_eq!(
        found
            .select(Some("firefox:Profiles/test"))
            .expect("usable Firefox")
            .name,
        "Firefox · Test"
    );

    std::fs::remove_file(&chrome).expect("remove failed root");
    std::fs::create_dir_all(chrome.join("Default/Network")).expect("chrome profile");
    std::fs::write(chrome.join("Default/Network/Cookies"), b"").expect("cookies");
    let retry = discover(&home);
    assert!(retry.warnings.is_empty());
    assert_eq!(retry.sources.len(), 2);
    assert_eq!(retry.sources[0].id, "chrome:Default");
    std::fs::remove_dir_all(home).expect("clean up");
}

#[test]
fn failed_cookie_metadata_is_reported_and_the_legacy_location_is_still_checked() {
    let profile = std::env::temp_dir().join(format!("nexus-cookie-metadata-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&profile).expect("profile");
    std::fs::write(profile.join("Network"), b"").expect("invalid network directory");
    std::fs::write(profile.join("Cookies"), b"").expect("legacy cookies");
    let mut found = Discovery::default();
    let database = cookie_database(
        [profile.join("Network/Cookies"), profile.join("Cookies")],
        &mut |error| found.warn("Chrome", error),
    );
    assert_eq!(database, Some(profile.join("Cookies")));
    assert_eq!(found.warnings.len(), 1);
    std::fs::remove_dir_all(profile).expect("clean up");
}

#[test]
fn unreadable_firefox_profile_index_is_reported() {
    let root = std::env::temp_dir().join(format!("nexus-cookie-firefox-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("profiles.ini")).expect("unreadable index");
    let mut found = Discovery::default();
    let profiles = firefox_profiles(&root, &mut |error| found.warn("Firefox", error));
    assert!(profiles.is_empty());
    assert_eq!(found.warnings.len(), 1);
    assert!(found.warnings[0].contains("Firefox"));
    std::fs::remove_dir_all(root).expect("clean up");
}

#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn finds_chromium_and_firefox_profiles_under_home() {
    let home = std::env::temp_dir().join(format!("nexus-cookie-home-{}", uuid::Uuid::new_v4()));
    let chrome = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Google/Chrome")
    } else {
        home.join(".config/google-chrome")
    };
    for (profile, file) in [
        ("Default", "Network/Cookies"),
        ("Profile 2", "Cookies"),
        ("System Profile", "Cookies"),
    ] {
        let path = chrome.join(profile).join(file);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("profile");
        std::fs::write(path, b"").expect("cookies");
    }
    std::fs::write(
        chrome.join("Local State"),
        r#"{"profile":{"info_cache":{"Default":{"name":"Work"}}}}"#,
    )
    .expect("local state");
    let firefox = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Firefox")
    } else {
        home.join(".mozilla/firefox")
    };
    std::fs::create_dir_all(firefox.join("Profiles/abc.default")).expect("firefox profile");
    std::fs::write(firefox.join("Profiles/abc.default/cookies.sqlite"), b"").expect("cookies");
    std::fs::write(
        firefox.join("profiles.ini"),
        "[General]\nStartWithLastProfile=1\n\n[Profile0]\nName=default\nIsRelative=1\nPath=Profiles/abc.default\n\n[Profile1]\nName=gone\nPath=Profiles/missing\n",
    )
    .expect("profiles.ini");
    let found = discover(&home);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    let listed: Vec<(&str, &str)> = found
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source.name.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("chrome:Default", "Chrome · Work"),
            ("chrome:Profile 2", "Chrome · Profile 2"),
            ("firefox:Profiles/abc.default", "Firefox · default"),
        ]
    );
    assert!(found.sources[0].database.ends_with("Default/Network/Cookies"));
    std::fs::remove_dir_all(home).expect("clean up");
}

/// Firefox 147 and later keep profiles in `~/.config/mozilla/firefox`. Before this, discovery looked only in
/// `~/.mozilla/firefox` and found no Firefox profile on a current install.
#[test]
#[cfg(target_os = "linux")]
fn finds_firefox_profiles_in_the_xdg_folder_and_lists_a_linked_one_once() {
    let home = std::env::temp_dir().join(format!("nexus-cookie-xdg-{}", uuid::Uuid::new_v4()));
    let xdg = home.join(".config/mozilla/firefox");
    std::fs::create_dir_all(xdg.join("a1.default-release")).expect("profile");
    std::fs::write(xdg.join("a1.default-release/cookies.sqlite"), b"").expect("cookies");
    std::fs::write(
        xdg.join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=a1.default-release\n",
    )
    .expect("profiles.ini");
    // The old folder: the same profile through a link, and another profile with its own folder name.
    let legacy = home.join(".mozilla/firefox");
    std::fs::create_dir_all(legacy.join("b2.old")).expect("old profile");
    std::fs::write(legacy.join("b2.old/cookies.sqlite"), b"").expect("cookies");
    std::os::unix::fs::symlink(xdg.join("a1.default-release"), legacy.join("a1.default-release")).expect("link");
    std::fs::write(
        legacy.join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=a1.default-release\n\n[Profile1]\nName=old\nIsRelative=1\nPath=b2.old\n",
    )
    .expect("profiles.ini");
    let found = discover(&home);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    let listed: Vec<(&str, &str)> = found
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source.name.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("firefox:a1.default-release", "Firefox · default-release"),
            ("firefox:b2.old", "Firefox · old"),
        ]
    );
    std::fs::remove_dir_all(home).expect("clean up");
}

#[test]
#[cfg(target_os = "linux")]
fn two_profiles_with_one_folder_name_keep_two_ids() {
    let home = std::env::temp_dir().join(format!("nexus-cookie-twins-{}", uuid::Uuid::new_v4()));
    for root in [".config/mozilla/firefox", ".mozilla/firefox"] {
        let root = home.join(root);
        std::fs::create_dir_all(root.join("p.default")).expect("profile");
        std::fs::write(root.join("p.default/cookies.sqlite"), root.to_string_lossy().as_bytes()).expect("cookies");
        std::fs::write(
            root.join("profiles.ini"),
            "[Profile0]\nName=default\nIsRelative=1\nPath=p.default\n",
        )
        .expect("profiles.ini");
    }
    let found = discover(&home);
    let ids: Vec<&str> = found.sources.iter().map(|source| source.id.as_str()).collect();
    assert_eq!(ids, ["firefox:p.default", "firefox-legacy:p.default"]);
    std::fs::remove_dir_all(home).expect("clean up");
}

/// KDE Wallet holds Chromium's key as folder `Chrome Keys`, entry `Chrome Safe Storage`, and KDE's Secret Service
/// shows them as `server` and `user`. Before this, only libsecret's `application` was searched, so on KDE every
/// encrypted cookie was skipped.
#[test]
fn the_key_is_looked_for_by_libsecrets_name_then_by_kde_wallets() {
    let labels: Vec<_> = CHROMIUM
        .iter()
        .map(|browser| {
            let [libsecret, kwallet] = super::chromium::key_searches(browser);
            (libsecret["application"], kwallet["server"], kwallet["user"])
        })
        .collect();
    assert_eq!(
        labels,
        [
            ("chrome", "Chrome Keys", "Chrome Safe Storage"),
            ("chromium", "Chromium Keys", "Chromium Safe Storage"),
            ("brave", "Brave Keys", "Brave Safe Storage"),
            ("msedge", "Microsoft Edge Keys", "Microsoft Edge Safe Storage"),
        ]
    );
}

/// A person who moved their config folder (`XDG_CONFIG_HOME`) keeps Chrome and Firefox there, not in `~/.config`.
#[test]
#[cfg(target_os = "linux")]
fn a_moved_config_folder_is_where_chrome_and_firefox_are_found() {
    let home = std::env::temp_dir().join(format!("nexus-cookie-config-{}", uuid::Uuid::new_v4()));
    let config = home.join("dotfiles/config");
    std::fs::create_dir_all(config.join("google-chrome/Default")).expect("chrome");
    std::fs::write(config.join("google-chrome/Default/Cookies"), b"").expect("cookies");
    let firefox = config.join("mozilla/firefox");
    std::fs::create_dir_all(firefox.join("x.default")).expect("firefox");
    std::fs::write(firefox.join("x.default/cookies.sqlite"), b"").expect("cookies");
    std::fs::write(
        firefox.join("profiles.ini"),
        "[Profile0]\nName=default\nIsRelative=1\nPath=x.default\n",
    )
    .expect("profiles.ini");
    let ids = |found: Discovery| -> Vec<String> { found.sources.into_iter().map(|source| source.id).collect() };
    assert_eq!(
        ids(discover_with_config(&home, &config)),
        ["chrome:Default", "firefox:x.default"]
    );
    assert!(ids(discover(&home)).is_empty(), "nothing is in ~/.config");
    std::fs::remove_dir_all(home).expect("clean up");
}
