use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-daily-active-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn record(day: i64, minutes: u32, sent: bool) -> Record {
    Record {
        day,
        minutes,
        modes: BTreeSet::new(),
        sent,
    }
}

#[test]
fn a_day_goes_once_after_five_minutes_or_late_within_a_week() {
    let today = 20_000;
    assert!(!due(&record(today, 0, false), today), "not used yet");
    assert!(!due(&record(today, SEND_AFTER_MINUTES - 1, false), today), "too soon");
    assert!(due(&record(today, SEND_AFTER_MINUTES, false), today));
    assert!(!due(&record(today, 60, true), today), "once a day");
    assert!(
        due(&record(today - 1, 1, false), today),
        "closed early yesterday: sent late"
    );
    assert!(
        due(&record(today - LATE_DAYS, 1, false), today),
        "the Gateway's oldest day"
    );
    assert!(
        !due(&record(today - LATE_DAYS - 1, 1, false), today),
        "the Gateway would refuse it"
    );
    assert!(!due(&record(today - 1, 0, false), today), "a day the app never ran");
}

#[test]
fn the_event_holds_the_privacy_notices_fields_and_no_others() {
    let mut day = record(20_416, 5, false);
    day.modes.insert("own_subscription".into());
    day.modes.insert("cloud".into());
    let id = "3f1c9a2e-5b7d-4e1a-9c0f-2d8b6a4e7f13";
    let sent = body(id, &day, "0.42.0+abc1234", "macos", Some("15.1.2"), Some("arm64")).expect("body");
    assert_eq!(
        sent,
        json!({
            "install_id": id, "day": "2025-11-24", "app_version": "0.42.0+abc1234", "channel": "stable",
            "os": "macos", "os_version": "15.1", "arch": "arm64", "modes": ["cloud", "own_subscription"],
        })
    );
    let bare = body(
        id,
        &record(20_416, 5, false),
        "0.2.0-internal.4",
        "linux",
        None,
        Some("x86_64"),
    )
    .expect("body");
    assert!(bare.get("os_version").is_none(), "no OS version is no field");
    assert_eq!(bare["channel"], "internal");
    assert!(
        body(id, &day, "0.42.0", "other", None, Some("arm64")).is_none(),
        "an OS outside the list"
    );
    assert!(
        body(id, &day, "0.42.0", "macos", None, None).is_none(),
        "a CPU outside the list"
    );
}

#[test]
fn values_are_mapped_into_the_contracts_lists() {
    assert_eq!(arch("aarch64"), Some("arm64"));
    assert_eq!(arch("x86_64"), Some("x86_64"));
    assert_eq!(arch("riscv64"), None);
    assert_eq!(major_minor("10.0.22631").as_deref(), Some("10.0"));
    assert_eq!(major_minor("24.04").as_deref(), Some("24.04"));
    assert_eq!(major_minor("15").as_deref(), Some("15"));
    assert_eq!(major_minor("rolling"), None);
    assert_eq!(major_minor("12345.1"), None);
    assert_eq!(channel("0.42.0"), "stable");
    assert_eq!(channel("0.42.0-beta.2+abc"), "beta");
    assert_eq!(channel("0.2.0-internal.4"), "internal");
    assert_eq!(channel("0.42.0-rc.1"), "other");
}

#[test]
fn the_install_id_is_made_once_kept_and_gone_with_its_day_when_forgotten() {
    let dir = temp_dir("id");
    assert_eq!(install_id(&dir), None);
    let id = ensure_install_id(&dir).expect("made");
    assert!(valid_id(&id), "{id}");
    assert_eq!(
        ensure_install_id(&dir).as_deref(),
        Some(id.as_str()),
        "kept across runs"
    );
    save(&dir, &record(20_000, 3, false));
    assert_eq!(forget(&dir).as_deref(), Some(id.as_str()), "shown one last time");
    assert_eq!(install_id(&dir), None);
    assert!(load(&dir).is_none(), "the unsent day goes too");
    let again = ensure_install_id(&dir).expect("made");
    assert_ne!(again, id, "turning usage data on again makes a new ID");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edited_id_file_is_not_sent() {
    let dir = temp_dir("edited");
    std::fs::create_dir_all(&dir).expect("dir");
    for bad in [
        "ada@example.com",
        "3F1C9A2E-5B7D-4E1A-9C0F-2D8B6A4E7F13",
        "3f1c9a2e-5b7d-1e1a-9c0f-2d8b6a4e7f13",
    ] {
        std::fs::write(dir.join(ID_FILE), bad).expect("write");
        assert_eq!(install_id(&dir), None, "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn production_is_the_default_gateway_only() {
    let on = |url: &str| on_production(&Client::new(ClientOptions::new(url)));
    assert!(on(DEFAULT_GATEWAY));
    assert!(on(&format!("{DEFAULT_GATEWAY}/")));
    assert!(!on("https://gateway.example.com"));
    assert!(!on("http://localhost:3000"));
}

#[test]
fn the_flag_is_off_unless_set_to_on() {
    for off in [
        None,
        Some(""),
        Some("off"),
        Some("0"),
        Some("false"),
        Some("yes"),
        Some("staff"),
    ] {
        assert!(!flag_on(off), "{off:?}");
    }
    for on in ["on", "1", "true", " ON ", "True"] {
        assert!(flag_on(Some(on)), "{on}");
    }
}
