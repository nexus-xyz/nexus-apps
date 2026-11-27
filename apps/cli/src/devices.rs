//! `nexus devices`: this computer as a trusted device for end-to-end encrypted session sync, as Settings ›
//! Devices pairs the desktop app and a browser (the key directory ADR's I2 to I4). `pair` registers this computer's
//! key (`device_key.rs`) and shows its 16-character code and a QR code; the person approves it from a device that is
//! already trusted, which compares the code itself. Gateway is not trusted for that check. Then the person compares the
//! approver's code the other way (threat model S1): Gateway names the approver, so this computer locks no
//! content key until the person confirms it. `list` and `revoke` work on the person's devices; a revocation is signed
//! with this computer's key. `org-key` pins the organization's recovery key by its fingerprint (ADR I7, threat model
//! S12): under `user_and_org`, this computer locks sessions for that key only once the person confirms it.

use std::time::{Duration, Instant};

use nexus_client::e2e::{self, DeviceKey};
use nexus_workbench::session_sync::KeyStore;
use nexus_workbench::session_sync::approver::{self, Approval, Approver};
use nexus_workbench::session_sync::org_key::{self, OrgKeyState, Shown};
use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Result, usage};
use crate::json::{a, js_string, s};
use crate::style::{Style, ago, col, list, pad_end, rows_of};

pub const DEVICES_COMMANDS: &[(&str, &str)] = &[
    (
        "list [--json]",
        "Your devices for encrypted session sync, oldest first: this computer, each one's state and pairing code",
    ),
    (
        "pair [--name N] [--new] [--no-wait] [--approver-code C] [--json]",
        "Make this computer a trusted device: it shows a code and a QR code, and you approve it from a trusted device (Settings › Devices in Nexus UI or the desktop app). Then confirm the approver's code: sync starts only after that",
    ),
    (
        "revoke <device-id> [--yes]",
        "Revoke a device with this computer's key: it gets no key for anything new. Revoking this computer also deletes its key",
    ),
    (
        "org-key [--fingerprint F] [--vault-public-key FILE] [--json]",
        "Your org's recovery key: compare its fingerprint with the one computed from your vault, then confirm it here. Your synced sessions are also locked for it when your org chooses so",
    ),
    (
        "control [on|off] [--json]",
        "Phone control for this computer's `nexus claude` and `nexus codex` sessions: off until you turn it on. With no setting, says where it stands",
    ),
];

/// What a pairing QR code starts with, then the device id and its whole fingerprint (`kit/src/pairing.ts`).
pub const PAIRING_QR_PREFIX: &str = "nexus-pair:v1:";
/// How long a pending device can be approved: Gateway's `PAIRING_TTL_MS`.
const PAIRING_TTL: Duration = Duration::from_secs(15 * 60);
const POLL: Duration = Duration::from_secs(2);

pub fn devices_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus devices <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        DEVICES_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 46)))),
    );
    lines.push(String::new());
    lines.push(style.dim(&format!(
        "A trusted computer syncs its `nexus claude` and `nexus codex` sessions when {}=1. Nexus never sees their content.",
        crate::session_log::SWITCH
    )));
    lines
}

/// The code people compare: the fingerprint's first 10 bytes in base32 (ADR I3), as four groups of four.
pub fn grouped(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn key32(text: &str) -> Option<[u8; 32]> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .ok()?
        .try_into()
        .ok()
}

/// A listed device's code, from the keys Gateway lists.
fn code_of(device: &Value) -> Option<String> {
    let encryption = key32(s(device, "encryptionKey")?)?;
    let signing = key32(s(device, "signingKey")?)?;
    Some(e2e::short_code(&e2e::fingerprint(&encryption, Some(&signing))))
}

/// Whether Gateway lists `device` with this key's public halves.
fn is_mine(device: &Value, key: &DeviceKey) -> bool {
    s(device, "encryptionKey") == Some(e2e::b64u(&key.encryption_key()).as_str())
        && s(device, "signingKey") == Some(e2e::b64u(&key.signing_key()).as_str())
}

/// `trusted`, `pending`, `expired` (pending past 15 minutes) or `revoked`, as `kit/src/pairing.ts` says.
fn status(device: &Value) -> String {
    let state = js_string(device.get("state"));
    if state != "pending" {
        return state;
    }
    let created = s(device, "createdAt")
        .and_then(|at| time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok());
    let waited = created.map(|at| time::OffsetDateTime::now_utc() - at);
    match waited {
        Some(waited) if waited.whole_seconds() > PAIRING_TTL.as_secs() as i64 => "expired".into(),
        _ => state,
    }
}

/// The QR code's text: `nexus-pair:v1:<device id>:<fingerprint, base64url>`.
pub fn qr_text(device_id: &str, key: &DeviceKey) -> String {
    format!("{PAIRING_QR_PREFIX}{device_id}:{}", e2e::b64u(&key.fingerprint()))
}

/// The QR code in half blocks ([`nexus_client::qr::lines`]).
pub use nexus_client::qr::lines as qr_lines;

pub fn devices_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("list");
    match action {
        "help" => {
            if cmd.json {
                return Ok(cmd.print(&json!({
                    "usage": "nexus devices <command>",
                    "commands": ["list", "pair", "revoke", "org-key", "control"],
                })));
            }
            cmd.lines(devices_help(cmd.style));
            Ok(0)
        }
        "list" | "ls" => list_devices(cmd),
        "pair" => pair(cmd),
        "revoke" => revoke(args.get(1).map(String::as_str), cmd),
        "org-key" => org_key_command(cmd),
        "control" => crate::phone_control::control_command(args, cmd),
        other => usage(
            format!("Unknown devices command: {other}."),
            Some("Run `nexus devices help`."),
        ),
    }
}

fn devices(cmd: &Cmd) -> Result<Vec<Value>> {
    let answer = cmd.core.get("/v1/devices")?;
    Ok(a(&answer, "devices").to_vec())
}

fn list_devices(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let key = crate::device_key::stored(cmd.ctx, &cmd.config.url)?;
    let listed = devices(cmd)?;
    let mine = key
        .as_ref()
        .and_then(|key| listed.iter().find(|device| is_mine(device, key)))
        .and_then(|device| s(device, "id"))
        .map(String::from);
    if cmd.json {
        let devices: Vec<Value> = listed
            .iter()
            .map(|device| {
                let mut device = device.clone();
                device["status"] = json!(status(&device));
                device["code"] = json!(code_of(&device));
                device
            })
            .collect();
        return Ok(cmd.print(&json!({ "devices": devices, "thisDevice": mine })));
    }
    if listed.is_empty() {
        cmd.out(format!(
            "{} No devices yet. Run {} to make this computer the first.",
            style.info,
            style.cyan("nexus devices pair")
        ));
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = listed
        .iter()
        .map(|device| {
            let id = js_string(device.get("id"));
            let name = if mine.as_deref() == Some(id.as_str()) {
                format!("{} {}", js_string(device.get("name")), style.dim("(this computer)"))
            } else {
                js_string(device.get("name"))
            };
            vec![
                name,
                id,
                status(device),
                code_of(device).map(|code| grouped(&code)).unwrap_or_default(),
                ago(s(device, "createdAt").unwrap_or("")),
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[col("Device"), col("Id"), col("State"), col("Code"), col("Added")],
        rows_of(rows),
        None,
    ));
    if mine.is_none() {
        cmd.out(style.dim("This computer is not paired: nexus devices pair"));
    }
    Ok(0)
}

fn register(cmd: &Cmd, key: &DeviceKey) -> Result<Value> {
    let name = cmd.flag_set_str("name").unwrap_or_else(|| match &cmd.ctx.hostname {
        Some(host) => format!("nexus CLI on {host}"),
        None => "nexus CLI".to_string(),
    });
    cmd.core.post(
        "/v1/devices",
        json!({
            "name": name,
            "encryptionKey": e2e::b64u(&key.encryption_key()),
            "signingKey": e2e::b64u(&key.signing_key()),
        }),
    )
}

/// This computer's entry in Gateway's list, registered now when it has none, is revoked or waited too long.
fn mine_or_register(cmd: &Cmd, gateway: &str) -> Result<(DeviceKey, Value)> {
    if cmd.has("new") {
        crate::device_key::forget(cmd.ctx, gateway)?;
    }
    let (mut key, _) = crate::device_key::load_or_create(cmd.ctx, gateway)?;
    let listed = devices(cmd)?;
    match listed.iter().find(|device| is_mine(device, &key)) {
        Some(device) if status(device) == "trusted" || status(device) == "pending" => Ok((key, device.clone())),
        Some(device) if status(device) == "revoked" => {
            // A revocation is final: this key is done, so pair a new one.
            crate::device_key::forget(cmd.ctx, gateway)?;
            key = crate::device_key::load_or_create(cmd.ctx, gateway)?.0;
            let device = register(cmd, &key)?;
            Ok((key, device))
        }
        _ => {
            let device = register(cmd, &key)?;
            Ok((key, device))
        }
    }
}

fn pair(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let gateway = cmd.config.url.clone();
    let (key, device) = mine_or_register(cmd, &gateway)?;
    let id = js_string(device.get("id"));
    let code = e2e::short_code(&key.fingerprint());
    let qr = qr_text(&id, &key);
    let summary = |device: &Value| {
        json!({
            "device": device,
            "status": status(device),
            "code": code,
            "qr": qr,
            "keyIn": crate::device_key::kept_in(cmd.ctx, &gateway),
        })
    };
    if status(&device) == "trusted" {
        if !cmd.json {
            cmd.out(format!(
                "{} This computer is a trusted device: {} {} {}",
                style.ok,
                js_string(device.get("name")),
                style.bullet,
                grouped(&code)
            ));
        }
        return confirm_approver(cmd, &key, summary(&device));
    }
    if !cmd.json {
        cmd.out(
            "Approve this computer from a device you already trust: Nexus UI or the desktop app, Settings › Devices.",
        );
        cmd.out(format!(
            "Check that it shows this code: {}",
            style.bold(&grouped(&code))
        ));
        let fits = style.color && cmd.ctx.columns.is_some_and(|columns| columns >= 50);
        if let Some(lines) = qr_lines(&qr).filter(|_| fits) {
            cmd.out(style.dim("Or scan this with the Nexus app:"));
            cmd.lines(lines);
        }
    }
    if cmd.has("no-wait") {
        if cmd.json {
            return Ok(cmd.print(&summary(&device)));
        }
        cmd.out(style.dim("Run `nexus devices pair` again to check, or `nexus devices` to see it."));
        return Ok(0);
    }
    if !cmd.json {
        cmd.out(style.dim("Waiting for the approval (Ctrl-C stops waiting; the code works for 15 minutes)…"));
    }
    let started = Instant::now();
    let poll = cmd.ctx.poll_ms.map_or(POLL, Duration::from_millis);
    loop {
        std::thread::sleep(poll);
        let listed = devices(cmd)?;
        let now = listed.iter().find(|device| is_mine(device, &key)).cloned();
        let state = now.as_ref().map(status);
        match state.as_deref() {
            Some("trusted") => {
                let device = now.unwrap_or_default();
                if !cmd.json {
                    cmd.out(format!(
                        "{} Approved. This computer is a trusted device ({}).",
                        style.ok,
                        js_string(device.get("name"))
                    ));
                }
                return confirm_approver(cmd, &key, summary(&device));
            }
            Some("pending") if started.elapsed() < PAIRING_TTL => continue,
            Some("revoked") => {
                crate::device_key::forget(cmd.ctx, &gateway)?;
                return Err(CliError::hint(
                    "This computer was revoked instead of approved.",
                    "Run `nexus devices pair` to start again with a new key.",
                )
                .into());
            }
            _ => {
                return Err(CliError::hint(
                    "The pairing code expired before anyone approved it.",
                    "Run `nexus devices pair` to show a new one.",
                )
                .into());
            }
        }
    }
}

/// Whether the code a person typed is `code`: case, spaces and hyphens do not count.
fn same_code(typed: &str, code: &str) -> bool {
    let typed: String = typed.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    typed.to_uppercase() == code
}

/// What to do when the approver's code does not match: revoke this computer from a device the person trusts.
fn revoke_hint(shown: &Approver, summary: &Value) -> String {
    let id = js_string(summary.pointer("/device/id"));
    format!(
        "If {} shows another code, or you do not know it, revoke this computer from a device you trust: Settings › Devices, or `nexus devices revoke {id}` there.",
        shown.name
    )
}

/// After approval (threat model S1). Gateway names this computer's approver, and Gateway is not trusted for
/// that, so the person compares the approver's code with the approver's own screen. Only a yes lets this computer lock
/// keys for it. `--approver-code` answers without a prompt. No answer, or no, records nothing and exits 1.
fn confirm_approver(cmd: &Cmd, key: &DeviceKey, mut summary: Value) -> Result<i32> {
    let style = cmd.style;
    let (shown, confirmed) = ask_approver(cmd, key, &summary)?;
    if cmd.json {
        summary["approver"] = json!(
            shown
                .as_ref()
                .map(|shown| json!({ "id": shown.id, "name": shown.name, "code": shown.code }))
        );
        summary["confirmed"] = json!(confirmed);
        cmd.print(&summary);
        return Ok(if confirmed { 0 } else { 1 });
    }
    match (&shown, confirmed) {
        (Some(shown), false) => {
            let hint = if cmd.ctx.interactive {
                revoke_hint(shown, &summary)
            } else {
                format!(
                    "Run `nexus devices pair --approver-code <the code {} shows>`. {}",
                    shown.name,
                    revoke_hint(shown, &summary)
                )
            };
            return Err(CliError::hint(
                "Nothing was recorded: this computer syncs nothing until you confirm its approver.",
                hint,
            )
            .into());
        }
        (Some(shown), true) => cmd.out(format!(
            "{} Its approver, {} {} {}, is confirmed on this computer.",
            style.ok,
            shown.name,
            style.bullet,
            grouped(&shown.code)
        )),
        (None, _) => {}
    }
    cmd.out(style.dim(&format!(
        "Its `nexus claude` and `nexus codex` sessions sync when {}=1.",
        crate::session_log::SWITCH
    )));
    Ok(0)
}

/// This computer's approver and whether the person confirmed it, asked now when it is not. None for a first device: it
/// has no approver (I4), so nothing is asked and no key store is opened.
fn ask_approver(cmd: &Cmd, key: &DeviceKey, summary: &Value) -> Result<(Option<Approver>, bool)> {
    let style = cmd.style;
    let listed = devices(cmd)?;
    let failed = |error: approver::ConfirmError| CliError::new(error.message());
    if approver::approver_of(&listed, key).map_err(failed)?.is_none() {
        return Ok((None, true));
    }
    let whoami = cmd.core.get("/v1/auth/whoami")?;
    let Some(person) = s(&whoami, "userId").filter(|user| !user.is_empty()) else {
        return Err(CliError::hint(
            "Session sync needs a person's sign-in, and this one is not.",
            "Run `nexus login` as yourself.",
        )
        .into());
    };
    let owner = approver::owner(cmd.core.base(), person);
    let store = KeyStore::open(&crate::session_log::keys_path(cmd.ctx)).map_err(CliError::new)?;
    Ok(
        match approver::approval(&store, &owner, &listed, key).map_err(failed)? {
            Approval::Root => (None, true),
            Approval::Confirmed(shown) => (Some(shown), true),
            Approval::Unconfirmed(shown) => {
                if !cmd.json {
                    cmd.out(format!(
                        "Nexus says {} approved it. On {}, open Settings › Devices: its fingerprint must be {}.",
                        style.bold(&shown.name),
                        shown.name,
                        style.bold(&grouped(&shown.code))
                    ));
                }
                let yes = match cmd.flag_set_str("approver-code") {
                    Some(typed) if same_code(&typed, &shown.code) => true,
                    Some(_) => {
                        return Err(CliError::hint(
                            format!("That is not {}'s code. Nothing was recorded.", shown.name),
                            revoke_hint(&shown, summary),
                        )
                        .into());
                    }
                    None if cmd.ctx.interactive && !cmd.json => {
                        let question = format!("Does {} show this code? [y/N] ", shown.name);
                        if !cmd.ctx.io.write(&question) {
                            cmd.out(&question);
                        }
                        let answer = cmd.ctx.io.read_line().unwrap_or_default().trim().to_lowercase();
                        answer == "y" || answer == "yes"
                    }
                    None => false,
                };
                if yes {
                    approver::confirm(&store, &owner, &listed, key, &shown.id).map_err(failed)?;
                }
                (Some(shown), yes)
            }
        },
    )
}

fn shown_json(shown: &Shown) -> Value {
    json!({ "id": shown.id, "fingerprint": shown.fingerprint })
}

/// Computes the fingerprint from an offline public-key export. Reads nothing from Gateway.
fn vault_fingerprint(cmd: &Cmd, path: &str) -> Result<i32> {
    let bytes =
        std::fs::read(path).map_err(|_| CliError::new("The public-key export from your vault could not be read."))?;
    let public = vault_public_key(&bytes).ok_or_else(|| {
        CliError::new("Export only the 32-byte X25519 public key from your vault, as raw bytes or base64url.")
    })?;
    let fingerprint = e2e::org_key_fingerprint(&public);
    if cmd.json {
        return Ok(cmd.print(&json!({ "source": "vault", "fingerprint": fingerprint })));
    }
    cmd.lines(vec![
        "Recovery-key fingerprint computed from your vault public-key export:".into(),
        org_key::grouped(&fingerprint),
        "Share this fingerprint with members through a trusted channel.".into(),
    ]);
    Ok(0)
}

fn vault_public_key(bytes: &[u8]) -> Option<[u8; 32]> {
    use base64::Engine as _;
    if let Ok(raw) = <[u8; 32]>::try_from(bytes) {
        return Some(raw);
    }
    let encoded = std::str::from_utf8(bytes).ok()?.trim();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?
        .try_into()
        .ok()
}

/// `nexus devices org-key` (ADR I7). Gateway serves the org's recovery key, and Gateway is not trusted for
/// it: whoever registers it reads every session locked for it. So the person compares the fingerprint this computer
/// computes with the one computed from your vault. Only a yes, or `--fingerprint` with the same one, records
/// the pin. A key that changed since the pin is a warning, and nothing is locked for it until the person confirms again.
/// Exits 1 while the key is not confirmed, as `pair` does while the approver is not.
fn org_key_command(cmd: &Cmd) -> Result<i32> {
    if cmd.has("vault-public-key") || cmd.flag("vault-public-key").is_some() {
        let Some(path) = cmd.flag_set_str("vault-public-key") else {
            return usage("Give --vault-public-key the public-key export from your vault.", None);
        };
        if cmd.has("fingerprint") || cmd.flag("fingerprint").is_some() {
            return usage("Use --vault-public-key or --fingerprint, not both.", None);
        }
        return vault_fingerprint(cmd, &path);
    }
    let style = cmd.style;
    let whoami = cmd.core.get("/v1/auth/whoami")?;
    let (Some(person), Some(tenant)) = (
        s(&whoami, "userId").filter(|user| !user.is_empty()),
        s(&whoami, "tenantId").filter(|tenant| !tenant.is_empty()),
    ) else {
        return Err(CliError::hint(
            "Session sync needs a person's sign-in to an organization, and this one is not.",
            "Run `nexus login` as yourself.",
        )
        .into());
    };
    let answer = cmd.core.get("/v1/orgs/recovery-key")?;
    let owner = approver::owner(cmd.core.base(), person);
    let store = KeyStore::open(&crate::session_log::keys_path(cmd.ctx)).map_err(CliError::new)?;
    let failed = |error: org_key::ConfirmError| CliError::new(error.message());
    let state = org_key::state(&store, &owner, tenant, &answer).map_err(failed)?;
    let typed = cmd.flag_set_str("fingerprint");
    let (served, confirmed_before) = match &state {
        OrgKeyState::None if typed.is_some() => return Err(CliError::new(org_key::copy::NO_KEY).into()),
        OrgKeyState::None => {
            if cmd.json {
                return Ok(cmd.print(&json!({ "key": null, "state": "none", "confirmed": false })));
            }
            cmd.lines(org_key_lines(style, &state));
            return Ok(0);
        }
        // A fingerprint given for a confirmed key is checked again: a script that pins a key learns when it changed.
        OrgKeyState::Confirmed(shown) if typed.is_none() => {
            if cmd.json {
                return Ok(cmd.print(&json!({ "key": shown_json(shown), "state": "confirmed", "confirmed": true })));
            }
            cmd.lines(org_key_lines(style, &state));
            return Ok(0);
        }
        OrgKeyState::Confirmed(served) | OrgKeyState::Unconfirmed(served) => (served, None),
        OrgKeyState::Changed { confirmed, served } => (served, Some(confirmed)),
    };
    if !cmd.json && typed.is_none() {
        cmd.lines(org_key_lines(style, &state));
    }
    let yes = match typed {
        Some(typed) => {
            org_key::confirm(&store, &owner, tenant, &answer, &typed).map_err(failed)?;
            true
        }
        None if cmd.ctx.interactive && !cmd.json => {
            let question = "Does it match the fingerprint computed from your vault? [y/N] ";
            if !cmd.ctx.io.write(question) {
                cmd.out(question);
            }
            let answer_line = cmd.ctx.io.read_line().unwrap_or_default().trim().to_lowercase();
            let yes = answer_line == "y" || answer_line == "yes";
            if yes {
                org_key::confirm(&store, &owner, tenant, &answer, &served.fingerprint).map_err(failed)?;
            }
            yes
        }
        None => false,
    };
    if cmd.json {
        cmd.print(&json!({
            "key": shown_json(served),
            "state": if yes { "confirmed" } else if confirmed_before.is_some() { "changed" } else { "unconfirmed" },
            "confirmed": yes,
            "confirmedKey": confirmed_before.map(shown_json),
        }));
        // As `nexus devices pair --json` for an unconfirmed approver: a script must act, so it is not a success.
        return Ok(if yes { 0 } else { 1 });
    }
    if !yes {
        return Err(CliError::hint("Nothing was recorded.", ORG_KEY_NEXT).into());
    }
    cmd.lines(org_key_lines(style, &OrgKeyState::Confirmed(served.clone())));
    Ok(0)
}

/// What to run when the key is not confirmed and no terminal can ask.
const ORG_KEY_NEXT: &str = "Run `nexus devices org-key --fingerprint <fingerprint>` with the fingerprint computed from your vault. Ask your admin to run `nexus devices org-key --vault-public-key <public-key-file>` offline.";

/// A fingerprint as two rows of eight groups of four, so it fits a terminal and reads aloud in pieces.
fn fingerprint_rows(fingerprint: &str) -> Vec<String> {
    let groups: Vec<String> = org_key::grouped(fingerprint).split(' ').map(String::from).collect();
    groups.chunks(8).map(|row| row.join(" ")).collect()
}

/// The human output of `nexus devices org-key` for `state`: a status line in the words every shell shares
/// (`org_key::copy`), the fingerprint and the key, then what it is, where to compare it, and what to do next.
fn org_key_lines(style: &Style, state: &OrgKeyState) -> Vec<String> {
    use org_key::copy;
    let (glyph, line, shown, next) = match state {
        OrgKeyState::None => (
            &style.info,
            copy::NO_KEY,
            None,
            Some("Nothing to do until your org's admin registers one."),
        ),
        OrgKeyState::Confirmed(shown) => (
            &style.ok,
            copy::CONFIRMED,
            Some(shown),
            Some("Nothing to do. If your org replaces the key, run `nexus devices org-key` again."),
        ),
        OrgKeyState::Unconfirmed(shown) => (&style.info, copy::UNCONFIRMED, Some(shown), Some(copy::COMPARE)),
        OrgKeyState::Changed { served, .. } => (&style.warn, copy::CHANGED, Some(served), Some(copy::COMPARE)),
    };
    let mut lines = vec![format!("{glyph} {line}")];
    if let Some(shown) = shown {
        lines.push(String::new());
        for (at, row) in fingerprint_rows(&shown.fingerprint).iter().enumerate() {
            let label = if at == 0 { "Fingerprint" } else { "" };
            lines.push(format!("  {}{}", pad_end(label, 13), style.bold(row)));
        }
        lines.push(format!("  {}{}", pad_end("Key", 13), shown.id));
        lines.push(String::new());
        lines.push(copy::ABOUT.to_string());
    }
    if let Some(next) = next {
        lines.push(style.dim(next));
    }
    lines
}

fn revoke(target: Option<&str>, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(target) = target.filter(|id| !id.is_empty()) else {
        return usage(
            "Name the device to revoke: nexus devices revoke <device-id>.",
            Some("`nexus devices` lists them."),
        );
    };
    let gateway = cmd.config.url.clone();
    let Some(key) = crate::device_key::stored(cmd.ctx, &gateway)? else {
        return Err(CliError::hint(
            "This computer has no device key, so it cannot sign a revocation.",
            "Revoke it from a trusted device (Settings › Devices), or pair this computer first.",
        )
        .into());
    };
    let listed = devices(cmd)?;
    let Some(me) = listed.iter().find(|device| is_mine(device, &key)) else {
        return Err(CliError::hint(
            "This computer is not one of your devices on this Gateway.",
            "Run `nexus devices pair` first.",
        )
        .into());
    };
    let Some(device) = listed.iter().find(|device| s(device, "id") == Some(target)) else {
        return Err(CliError::hint(
            format!("You have no device {}.", style.bold(target)),
            "`nexus devices` lists them.",
        )
        .into());
    };
    let me_id = js_string(me.get("id"));
    let itself = me_id == target;
    if !itself && status(me) != "trusted" {
        return Err(CliError::hint(
            "Only a trusted device can revoke another, and this computer is not trusted yet.",
            "Run `nexus devices pair`, or revoke it from a trusted device.",
        )
        .into());
    }
    if status(device) == "revoked" {
        cmd.out(format!("{} {target} is already revoked.", style.info));
        return Ok(0);
    }
    if !cmd.has("yes") {
        let name = js_string(device.get("name"));
        if !cmd.ctx.interactive || cmd.json {
            return usage(
                format!("Revoking {name} is final: it gets no key for anything new."),
                Some(&format!("Run `nexus devices revoke {target} --yes` to go ahead.")),
            );
        }
        let question = format!("Revoke {name} ({target})? It is final. [y/N] ");
        if !cmd.ctx.io.write(&question) {
            cmd.out(&question);
        }
        let answer = cmd.ctx.io.read_line().unwrap_or_default().trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            cmd.out(style.dim("Nothing changed."));
            return Ok(1);
        }
    }
    let message = e2e::revocation_message(target).map_err(|error| CliError::new(error.to_string()))?;
    let revoked = cmd.core.post(
        &format!("/v1/devices/{}/revoke", enc(target)),
        json!({ "revokerDeviceId": me_id, "signature": e2e::b64u(&key.sign(&message)) }),
    )?;
    if itself {
        crate::device_key::forget(cmd.ctx, &gateway)?;
    }
    if cmd.json {
        return Ok(cmd.print(&json!({ "device": revoked, "forgotKey": itself })));
    }
    cmd.out(format!(
        "{} Revoked {}{}",
        style.ok,
        js_string(device.get("name")),
        if itself {
            ", and deleted this computer's key."
        } else {
            "."
        }
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_exports_compute_the_protocol_fingerprint_without_gateway_input() {
        let raw = [0u8; 32];
        assert_eq!(vault_public_key(&raw), Some(raw));
        assert_eq!(vault_public_key(format!("{}\n", e2e::b64u(&raw)).as_bytes()), Some(raw));
        assert_eq!(vault_public_key(b"{\"fingerprint\":\"gateway-supplied\"}"), None);
        assert_eq!(vault_public_key(&[0u8; 31]), None);
        assert_eq!(
            e2e::org_key_fingerprint(&raw),
            "e10e33546b772652b9faffb62b970e29735472a6eba7a68fa7257edb7b8f8dce"
        );
    }

    #[test]
    fn the_code_and_qr_text_are_what_trusted_devices_compare() {
        let key = DeviceKey::generate();
        let code = e2e::short_code(&key.fingerprint());
        assert_eq!(code.len(), 16);
        assert_eq!(grouped(&code).len(), 19);
        let listed = json!({
            "encryptionKey": e2e::b64u(&key.encryption_key()),
            "signingKey": e2e::b64u(&key.signing_key()),
        });
        assert_eq!(code_of(&listed).as_deref(), Some(code.as_str()));
        assert!(is_mine(&listed, &key));
        assert!(!is_mine(&listed, &DeviceKey::generate()));
        let text = qr_text("dev_01", &key);
        assert_eq!(text, format!("nexus-pair:v1:dev_01:{}", e2e::b64u(&key.fingerprint())));
        let lines = qr_lines(&text).expect("a QR code");
        // Half blocks: about half as many lines as modules, each line as wide as the code and its quiet zone.
        let width = lines[0].chars().filter(|c| matches!(c, '█' | '▀' | '▄' | ' ')).count();
        assert!(width >= 25 + 4, "{width}");
        assert_eq!(lines.len(), width.div_ceil(2));
    }

    #[test]
    fn a_pending_device_expires_after_fifteen_minutes() {
        let at = |minutes: i64| {
            (time::OffsetDateTime::now_utc() - time::Duration::minutes(minutes))
                .format(&time::format_description::well_known::Rfc3339)
                .expect("time")
        };
        assert_eq!(status(&json!({ "state": "pending", "createdAt": at(1) })), "pending");
        assert_eq!(status(&json!({ "state": "pending", "createdAt": at(16) })), "expired");
        assert_eq!(status(&json!({ "state": "trusted", "createdAt": at(60) })), "trusted");
        assert_eq!(status(&json!({ "state": "revoked" })), "revoked");
    }

    #[test]
    fn the_org_key_reads_in_the_shared_words_with_its_fingerprint_in_two_rows_and_a_next_step() {
        use nexus_workbench::session_sync::org_key::copy;
        let style = Style::new(false);
        let print = e2e::org_key_fingerprint(&[0; 32]);
        let shown = Shown {
            id: "ork_one".into(),
            fingerprint: print.clone(),
        };
        let rows = fingerprint_rows(&print);
        assert_eq!(
            rows,
            [
                "e10e 3354 6b77 2652 b9fa ffb6 2b97 0e29",
                "7354 72a6 eba7 a68f a725 7edb 7b8f 8dce"
            ]
        );
        let unconfirmed = org_key_lines(&style, &OrgKeyState::Unconfirmed(shown.clone()));
        assert_eq!(unconfirmed[0], format!("{} {}", style.info, copy::UNCONFIRMED));
        assert!(unconfirmed.contains(&format!("  Fingerprint  {}", rows[0])));
        assert!(unconfirmed.contains(&format!("               {}", rows[1])));
        assert!(unconfirmed.contains(&"  Key          ork_one".to_string()));
        assert_eq!(unconfirmed.last().map(String::as_str), Some(copy::COMPARE));
        let changed = org_key_lines(
            &style,
            &OrgKeyState::Changed {
                confirmed: shown.clone(),
                served: shown.clone(),
            },
        );
        assert_eq!(changed[0], format!("{} {}", style.warn, copy::CHANGED));
        let confirmed = org_key_lines(&style, &OrgKeyState::Confirmed(shown));
        assert_eq!(confirmed[0], format!("{} {}", style.ok, copy::CONFIRMED));
        assert!(confirmed.last().is_some_and(|line| line.starts_with("Nothing to do")));
        let none = org_key_lines(&style, &OrgKeyState::None);
        assert_eq!(none.len(), 2);
        assert_eq!(none[0], format!("{} {}", style.info, copy::NO_KEY));
        assert!(ORG_KEY_NEXT.contains("--fingerprint") && ORG_KEY_NEXT.contains("from your vault"));
    }

    #[test]
    fn a_typed_approver_code_matches_in_any_case_and_grouping() {
        let code = e2e::short_code(&DeviceKey::generate().fingerprint());
        assert!(same_code(&code, &code));
        assert!(same_code(&grouped(&code).to_lowercase(), &code));
        assert!(same_code(&grouped(&code).replace(' ', "-"), &code));
        assert!(!same_code(&code[..15], &code));
        assert!(!same_code(
            &e2e::short_code(&DeviceKey::generate().fingerprint()),
            &code
        ));
    }
}
