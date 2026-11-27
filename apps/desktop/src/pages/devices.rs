//! Devices (`ui/src/components/nexus-ui/settings/you/devices.tsx`): the devices that can read the person's
//! synced sessions, and pairing a new one. This computer's key is the keychain entry the Nexus app also uses
//! (service `xyz.nexus.desktop`, account `<gateway>#e2e-device`), so both apps are one device. The key is made the
//! first time the page opens.
//!
//! - With no row in the directory, this device registers (`POST /v1/devices`). The person's first device is trusted at
//!   once; a later one waits as pending and shows its 16-character code until a trusted device approves it.
//! - A trusted device finds a pending one by its code (or a pairing QR code's text), checks the fingerprint itself,
//!   and signs the approval (`POST /v1/devices/{id}/approve`).
//! - Every device is listed with who approved it, checked here (`verifyApprovals` in `@nexus/kit/pairing`). A trusted
//!   device revokes any of them with its signature (`POST /v1/devices/{id}/revoke`), after a second press.
//! - A pending device shows its code and draws no QR code. The page's only QR code connects a phone (Link another
//!   device), and a trusted phone that scans it approves this computer too.
//! - Once approved, this device shows its approver's code, and the person confirms it before this computer syncs
//!   (`devices/approver.rs`).
//! - A trusted device shows the org's recovery key by the fingerprint it computes, and the person confirms it before
//!   this computer locks sessions for it (`devices/org_key.rs`).
//! - When Gateway vouches for the person's devices (`trust: gateway`), a computer registers itself at sign-in
//!   (`devices/sign_in.rs`), and the page shows no codes or approvals (`devices/gateway_trust.rs`).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::e2e::{self, DEVICE_SECRET_LEN, DeviceKey};
use serde_json::{Value, json};

use super::{Ctx, Load, Verb, fetch, palette, ui};
use crate::motion;
use crate::palette::Palette;

mod approver;
mod gateway_trust;
mod gateway_view;
mod org_key;
mod remote_computers;
mod sign_in;
mod trust_pin;
mod view;
use gateway_trust::{DeviceTrust, Parts, read_device_trust};
pub(crate) use sign_in::register_this_computer;
pub(super) use view::build_locked;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// How long a pending device can be approved after it registers: Gateway's `PAIRING_TTL_MS`.
const PAIRING_TTL_MS: i64 = 15 * 60 * 1000;
/// What a pairing QR code's text starts with; then the device id and its fingerprint, base64url.
const PAIRING_QR_PREFIX: &str = "nexus-pair:v1:";
/// The phone invitation's QR code. Its link is long, so its modules are small: a large side keeps them readable.
const PHONE_QR_SIDE: f32 = 300.;
const PHONE_QR_BORDER: i32 = 4;
/// How long the fingerprint's copy button shows its check.
const COPIED: Duration = Duration::from_millis(1500);
/// How often a pending device asks whether it was approved.
const POLL: Duration = Duration::from_secs(3);
/// How often the open page reads Phone control again.
const CONTROL_POLL: Duration = Duration::from_secs(15);

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Device name");
            state.set_value(crate::computer_name::desktop_label(), window, cx);
            state
        });
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("XXXX XXXX XXXX XXXX"));
        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this: &mut DevicesPage, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.register(None, cx);
                }
            }),
            cx.subscribe_in(&code, window, |this: &mut DevicesPage, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.find(cx);
                }
            }),
            cx.observe(&code, |_, _, cx| cx.notify()),
        ];
        // Made now, shown when the mode allows it: Gateway names the mode after the page opens.
        let remote_computers = Some(cx.new(|cx| remote_computers::RemoteComputers::new(ctx.clone(), window, cx)));
        let mut page = DevicesPage {
            ctx,
            remote_computers,
            remote_preview: nexus_workbench::computer::remote_pairing_preview(),
            trust: DeviceTrust::default(),
            offered: DeviceTrust::default(),
            settling: false,
            gateway_waits: None,
            auto_registered: false,
            key: Load::Loading,
            devices: Load::Loading,
            name,
            code,
            found: None,
            busy: false,
            outcome: None,
            approve_outcome: None,
            confirm_revoke: None,
            copied_at: None,
            polling: false,
            phone_control: Load::Loading,
            control_busy: false,
            control_watched: false,
            link_invitation: None,
            link_candidate: None,
            link_busy: false,
            link_polling: false,
            link_outcome: None,
            approver: Load::Loading,
            org_key: Load::Loading,
            _subscriptions: subscriptions,
        };
        page.load_key(cx);
        page.load(cx);
        page
    });
    // The Code callback reads Workspace. Wait until its page-construction update has ended.
    let control = view.clone();
    cx.defer(move |cx| control.update(cx, |page, cx| page.load_control(cx)));
    Some(view.into())
}

/// A device in the key directory (`E2eDevice` in `kit/src/api.ts`).
#[derive(Debug, Clone, PartialEq)]
struct Device {
    id: String,
    name: String,
    encryption_key: String,
    signing_key: String,
    /// `pending`, `trusted` or `revoked`.
    state: String,
    approved_by: Option<String>,
    approval_signature: Option<String>,
    created_at: String,
    approved_at: Option<String>,
    revoked_at: Option<String>,
}

fn read_devices(value: &Value) -> Vec<Device> {
    let opt = |device: &Value, key: &str| {
        device
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(String::from)
    };
    value
        .get("devices")
        .and_then(Value::as_array)
        .map(|devices| {
            devices
                .iter()
                .filter_map(|device| {
                    Some(Device {
                        id: device.get("id")?.as_str()?.to_string(),
                        name: ui::text(device, "name").to_string(),
                        encryption_key: device.get("encryptionKey")?.as_str()?.to_string(),
                        signing_key: device.get("signingKey")?.as_str()?.to_string(),
                        state: device.get("state")?.as_str()?.to_string(),
                        approved_by: opt(device, "approvedBy"),
                        approval_signature: opt(device, "approvalSignature"),
                        created_at: ui::text(device, "createdAt").to_string(),
                        approved_at: opt(device, "approvedAt"),
                        revoked_at: opt(device, "revokedAt"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Trusted,
    Pending,
    /// Still pending at Gateway, but past the pairing window.
    Expired,
    Revoked,
}

fn status(device: &Device, now: i64) -> Status {
    match device.state.as_str() {
        "trusted" => Status::Trusted,
        "revoked" => Status::Revoked,
        _ => match iso_ms(&device.created_at) {
            Some(at) if now - at >= PAIRING_TTL_MS => Status::Expired,
            _ => Status::Pending,
        },
    }
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Trusted => "Trusted",
        Status::Pending => "Waiting for approval",
        Status::Expired => "Pairing expired",
        Status::Revoked => "Revoked",
    }
}

/// Phone control needs encrypted session sync and a confirmed local device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhoneConnection {
    Managed,
    Setup,
    Pending,
    Expired,
    Revoked,
    /// Trusted, but the person has not confirmed its approver's code: the runner locks nothing until then.
    Approver,
    Checking,
    Unavailable,
    Ready,
}

impl PhoneConnection {
    /// `approved`: whether this device's approver is confirmed, true for a root; None while it is read.
    fn read(local_cache: bool, device: Option<Status>, approved: Option<bool>, available: Option<bool>) -> Self {
        if !local_cache {
            return Self::Managed;
        }
        match device {
            None => Self::Setup,
            Some(Status::Pending) => Self::Pending,
            Some(Status::Expired) => Self::Expired,
            Some(Status::Revoked) => Self::Revoked,
            Some(Status::Trusted) => match (approved, available) {
                (Some(false), _) => Self::Approver,
                (None, _) | (_, None) => Self::Checking,
                (_, Some(false)) => Self::Unavailable,
                (Some(true), Some(true)) => Self::Ready,
            },
        }
    }

    /// An invitation can set up a computer or finish the approval of the phone that scanned it.
    fn can_invite(self) -> bool {
        matches!(self, Self::Setup | Self::Pending | Self::Approver | Self::Ready)
    }

    fn message(self) -> &'static str {
        match self {
            Self::Managed => "Your organization's settings prevent encrypted session sync on this computer.",
            Self::Setup => "Set up this computer under This device before connecting your phone.",
            Self::Pending => "Approve this computer from a trusted device before connecting your phone.",
            Self::Expired => "Pairing expired. Set up this computer again under This device.",
            Self::Revoked => "This computer was revoked. Set it up again under This device.",
            Self::Approver => {
                "Confirm this computer's approver under This device before connecting your phone. Sessions sync after \
                 you confirm."
            }
            Self::Checking => "Checking encrypted session sync…",
            Self::Unavailable => {
                "Encrypted session sync is unavailable. Check your session sync settings, then try again."
            }
            Self::Ready => "",
        }
    }

    /// `message`, for the mode: in gateway mode this computer sets itself up, and no device approves it.
    fn message_for(self, trust: DeviceTrust) -> &'static str {
        match (trust, self) {
            (DeviceTrust::Gateway, Self::Setup | Self::Pending) => {
                "This computer is connecting to your account. Remote control is ready when it is set up."
            }
            (DeviceTrust::Gateway, Self::Expired) => {
                "This computer's setup expired. Set it up again under This device."
            }
            (DeviceTrust::Gateway, Self::Revoked) => "This computer was revoked. Make a new key under This device.",
            _ => self.message(),
        }
    }
}

/// How a device came to be trusted, checked here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    /// Trusted with no approval: a first device.
    Root,
    Approved,
    /// The signature does not verify, or its approver is not listed.
    Unverified,
}

fn key32(text: &str) -> Option<[u8; 32]> {
    b64u_decode(text)?.try_into().ok()
}

/// A device's fingerprint from the keys Gateway lists; None for keys that are not 32 bytes.
fn device_fingerprint(device: &Device) -> Option<[u8; 32]> {
    Some(e2e::fingerprint(
        &key32(&device.encryption_key)?,
        Some(&key32(&device.signing_key)?),
    ))
}

/// The 16-character code people compare; empty for keys that are not 32 bytes.
fn device_code(device: &Device) -> String {
    device_fingerprint(device)
        .map(|fp| e2e::short_code(&fp))
        .unwrap_or_default()
}

/// `verifyApprovals`: pending devices, and revoked ones never approved, are not in it.
fn trust(device: &Device, devices: &[Device]) -> Option<Trust> {
    if device.state == "pending" || (device.state == "revoked" && device.approved_at.is_none()) {
        return None;
    }
    let (Some(approver), Some(signature)) = (&device.approved_by, &device.approval_signature) else {
        return Some(Trust::Root);
    };
    let verified = (|| {
        let approver = devices.iter().find(|each| each.id == *approver)?;
        let approver_key = key32(&approver.signing_key)?;
        let message = e2e::approval_message(
            &device.id,
            &key32(&device.encryption_key)?,
            &key32(&device.signing_key)?,
        )
        .ok()?;
        Some(e2e::verify(&approver_key, &message, &b64u_decode(signature)?))
    })();
    Some(if verified == Some(true) {
        Trust::Approved
    } else {
        Trust::Unverified
    })
}

/// This device's newest row: the one with its keys.
fn this_device<'a>(devices: &'a [Device], encryption_key: &str, signing_key: &str) -> Option<&'a Device> {
    devices
        .iter()
        .filter(|device| device.encryption_key == encryption_key && device.signing_key == signing_key)
        .max_by_key(|device| iso_ms(&device.created_at).unwrap_or(0))
}

/// What the person typed or pasted: a short code, or a QR code's device id and whole fingerprint.
#[derive(Debug, Clone, PartialEq)]
struct PairingInput {
    code: String,
    device_id: Option<String>,
    fingerprint: Option<[u8; 32]>,
}

const NOT_A_CODE: &str = "That is not a pairing code. It has 16 letters and digits, shown on the new device.";

fn read_pairing_input(text: &str) -> Result<PairingInput, String> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix(PAIRING_QR_PREFIX) {
        let mut parts = rest.split(':');
        let (Some(id), Some(encoded)) = (parts.next(), parts.next()) else {
            return Err(NOT_A_CODE.into());
        };
        let valid_id = !id.is_empty()
            && id.len() <= 128
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        let fingerprint: Option<[u8; 32]> = b64u_decode(encoded).and_then(|bytes| bytes.try_into().ok());
        return match (valid_id, fingerprint) {
            (true, Some(fingerprint)) => Ok(PairingInput {
                code: e2e::short_code(&fingerprint),
                device_id: Some(id.to_string()),
                fingerprint: Some(fingerprint),
            }),
            _ => Err(NOT_A_CODE.into()),
        };
    }
    let code: String = text
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    let base32 = code.len() == 16 && code.chars().all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c));
    if !base32 {
        return Err(NOT_A_CODE.into());
    }
    Ok(PairingInput {
        code,
        device_id: None,
        fingerprint: None,
    })
}

/// Whether the device's keys have this input's fingerprint: the whole one from a QR code, or the code's.
fn matches_input(device: &Device, input: &PairingInput) -> bool {
    if input.device_id.as_ref().is_some_and(|id| *id != device.id) {
        return false;
    }
    let Some(fingerprint) = device_fingerprint(device) else {
        return false;
    };
    match &input.fingerprint {
        Some(whole) => *whole == fingerprint,
        None => e2e::short_code(&fingerprint) == input.code,
    }
}

/// The pending device the person means (`findPending`), checked here and not by Gateway.
fn find_pending(devices: &[Device], input: &PairingInput, now: i64) -> Result<Device, String> {
    let mut live: Vec<&Device> = Vec::new();
    let mut expired = false;
    for device in devices {
        let status = status(device, now);
        if !matches!(status, Status::Pending | Status::Expired) || !matches_input(device, input) {
            continue;
        }
        if status == Status::Expired {
            expired = true;
        } else {
            live.push(device);
        }
    }
    if live.is_empty() {
        return Err(if expired {
            "That device waited more than 15 minutes. Start the pairing again on it."
        } else {
            "No device is waiting with that code. Check the code on the new device."
        }
        .into());
    }
    let first = live[0];
    if live
        .iter()
        .any(|device| device.encryption_key != first.encryption_key || device.signing_key != first.signing_key)
    {
        return Err("Two devices have that code. Revoke the one you do not know, then pair again.".into());
    }
    Ok(live
        .into_iter()
        .max_by_key(|device| iso_ms(&device.created_at).unwrap_or(0))
        .cloned()
        .expect("one at least"))
}

/// The approval of `device`, signed only when its keys have `code` (`sign_approval` in `device_key.rs`).
fn sign_approval(key: &DeviceKey, device: &Device, code: &str) -> Result<String, String> {
    const MISMATCH: &str = "That device's keys do not match the code. Do not approve it.";
    let encryption_key = key32(&device.encryption_key).ok_or(MISMATCH)?;
    let signing_key = key32(&device.signing_key).ok_or(MISMATCH)?;
    if e2e::short_code(&e2e::fingerprint(&encryption_key, Some(&signing_key))) != code {
        return Err(MISMATCH.into());
    }
    let message = e2e::approval_message(&device.id, &encryption_key, &signing_key).map_err(|e| e.to_string())?;
    Ok(e2e::b64u(&key.sign(&message)))
}

/// The QR modules with medium error correction and a light border.
fn qr_modules(text: &str, border: i32) -> Option<Vec<Vec<bool>>> {
    use qrcodegen::{QrCode, QrCodeEcc};
    let code = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let range = -border..code.size() + border;
    Some(
        range
            .clone()
            .map(|y| range.clone().map(|x| code.get_module(x, y)).collect())
            .collect(),
    )
}

/// A QR code: dark modules on white whatever the theme, so every camera reads it. Each module is a whole number of
/// pixels, centred in the square, so no seam shows between two dark modules.
fn render_qr(modules: Arc<Vec<Vec<bool>>>, side: f32) -> Div {
    let count = modules.len().max(1) as f32;
    let module = (side / count).floor().max(1.);
    let inset = (side - module * count) / 2.;
    div()
        .flex_none()
        .size(px(side))
        .rounded(px(8.))
        .overflow_hidden()
        .bg(gpui::white())
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let origin = bounds.origin + point(px(inset), px(inset));
                    for (y, row) in modules.iter().enumerate() {
                        // One quad per run of dark modules in a row.
                        let mut x = 0;
                        while x < row.len() {
                            if !row[x] {
                                x += 1;
                                continue;
                            }
                            let start = x;
                            while x < row.len() && row[x] {
                                x += 1;
                            }
                            window.paint_quad(fill(
                                Bounds::new(
                                    origin + point(px(module * start as f32), px(module * y as f32)),
                                    size(px(module * (x - start) as f32), px(module)),
                                ),
                                gpui::black(),
                            ));
                        }
                    }
                },
            )
            .size_full(),
        )
}

/// The code in four groups of four.
fn format_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Milliseconds since 1970 from an RFC 3339 time (`2026-11-05T12:00:00.123Z`, or with an offset).
fn iso_ms(at: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| at.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let mut rest = at.get(19..)?;
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: String = fraction.chars().take_while(char::is_ascii_digit).collect();
        millis = format!("{digits:0<3}").get(..3)?.parse::<i64>().ok()?;
        rest = &fraction[digits.len()..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let hours = rest.get(1..3)?.parse::<i64>().ok()?;
            let minutes = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (hours * 60 + minutes) * 60_000
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + hour * 3600 + minute * 60 + second) * 1000 + millis) - offset)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Base64url without padding (padding is accepted too), as Gateway sends keys and signatures.
fn b64u_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0u32, 0u32);
    for byte in text.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- the keychain, as `device_key.rs`

/// The stored key, or a new one made and stored now, and whether it is new. A key the keychain would not give, or an
/// entry that is not a key, reads as an error, never as no key: making a new one would orphan the device the person
/// paired. One call runs at a time: the page and the sign-in's registration can both ask, and must not make two keys.
fn load_or_create(gateway: &str) -> Result<(DeviceKey, bool), String> {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match crate::keychain::device_key(gateway)? {
        Some(text) => {
            let bytes = b64u_decode(text.trim()).unwrap_or_default();
            let secret: &[u8; DEVICE_SECRET_LEN] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| "The keychain holds a device key Nexus cannot read.".to_string())?;
            Ok((DeviceKey::from_secret_bytes(secret), false))
        }
        None => {
            let key = DeviceKey::generate();
            crate::keychain::save_device_key(gateway, &e2e::b64u(&key.secret_bytes()[..]))
                .map_err(|error| format!("Nexus did not save this computer's device key: {error}"))?;
            Ok((key, true))
        }
    }
}

/// Delete the key, so the next load makes a new one.
fn forget(gateway: &str) -> Result<(), String> {
    crate::keychain::forget_device_key(gateway)
        .map_err(|error| format!("The keychain did not delete the device key: {error}"))
}

/// Run blocking keychain work off the UI thread.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, String>> {
    let (send, receive) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        send.send(work()).ok();
    });
    async move { receive.await.map_err(|_| "The keychain did not answer.".to_string())? }
}

// ---------------------------------------------------------------- the page

/// This computer's key, and its public half as Gateway lists it.
#[derive(Clone)]
struct Key {
    key: Arc<DeviceKey>,
    encryption_key: String,
    signing_key: String,
}

impl Key {
    fn new(key: DeviceKey) -> Key {
        Key {
            encryption_key: e2e::b64u(&key.encryption_key()),
            signing_key: e2e::b64u(&key.signing_key()),
            key: Arc::new(key),
        }
    }
}

/// An outcome line: the text, and whether it is an error.
type Outcome = Option<(String, bool)>;

/// The part of the page an outcome answers. Its line shows under that part, where the person pressed the button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Setting up this device, its new key, and confirming its approver.
    ThisDevice,
    /// Confirming the org's recovery key (`org_key.rs`).
    OrgKey,
    /// Revoking a device in All devices.
    AllDevices,
}

/// A fingerprint the page lets the person copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Copied {
    /// This device's.
    Device,
    /// The org's recovery key's (`org_key.rs`).
    OrgKey,
    PhoneLink,
}

struct PhoneLink {
    invitation: nexus_workbench::computer::Invitation,
    link: String,
    qr: Arc<Vec<Vec<bool>>>,
}

impl PhoneLink {
    fn new(invitation: nexus_workbench::computer::Invitation) -> Result<Self, String> {
        let bytes = serde_json::to_vec(&invitation).map_err(|error| error.to_string())?;
        let base = std::env::var("NEXUS_PHONE_APP_URL").unwrap_or_else(|_| "xyz.nexus.app://connect".into());
        let address = base.split('#').next().unwrap_or(&base);
        let link = format!("{address}#invite={}", e2e::b64u(&bytes));
        let qr = Arc::new(qr_modules(&link, PHONE_QR_BORDER).ok_or("The connection link is too long for a QR code.")?);
        Ok(Self { invitation, link, qr })
    }
}

struct DevicesPage {
    ctx: Ctx,
    /// Other computers. It shows only when [`Parts::remote_computers`] says so: read it with `remote_computers()`.
    remote_computers: Option<Entity<remote_computers::RemoteComputers>>,
    /// Whether the remote pairing preview is on.
    remote_preview: bool,
    /// The trust this computer follows: Gateway's own only when the pin allows it (`trust_pin.rs`).
    trust: DeviceTrust,
    /// The trust Gateway named at the last directory read.
    offered: DeviceTrust,
    /// Whether the pin is being read for Gateway's trust.
    settling: bool,
    /// The key store owner, while Gateway offers its own trust and this computer waits for the person to opt in.
    gateway_waits: Option<String>,
    /// Whether the page registered this computer itself in gateway mode. It tries once: a refusal shows Try again.
    auto_registered: bool,
    key: Load<Key>,
    devices: Load<Vec<Device>>,
    name: Entity<InputState>,
    code: Entity<InputState>,
    /// The pending device found by the code, waiting for Approve.
    found: Option<(Device, PairingInput)>,
    busy: bool,
    /// The last action's outcome, and the part of the page it answers.
    outcome: Option<(Part, String, bool)>,
    approve_outcome: Outcome,
    /// The device whose Revoke was pressed once.
    confirm_revoke: Option<String>,
    /// When the fingerprint was last copied, while its button shows a check.
    /// Which fingerprint was copied last, and when: its button shows a check for [`COPIED`].
    copied_at: Option<(Copied, Instant)>,
    polling: bool,
    phone_control: Load<nexus_workbench::session_sync::control::Settings>,
    control_busy: bool,
    /// Whether the loop that reads Phone control again runs.
    control_watched: bool,
    link_invitation: Option<PhoneLink>,
    link_candidate: Option<nexus_workbench::computer::LinkStatus>,
    link_busy: bool,
    link_polling: bool,
    link_outcome: Outcome,
    /// Whether the person confirmed this device's approver by its code (`approver.rs`).
    approver: approver::ApproverCheck,
    /// Whether the person confirmed the org's recovery key by its fingerprint on this computer (`org_key.rs`).
    org_key: org_key::OrgKeyCheck,
    _subscriptions: Vec<Subscription>,
}

impl DevicesPage {
    fn phone_connection(&self) -> PhoneConnection {
        let available = match &self.phone_control {
            Load::Loading => None,
            Load::Ready(settings) => Some(settings.available),
            Load::Failed(_) => Some(false),
        };
        // In gateway mode session sync trusts Gateway's list: no approver waits for the person's confirmation.
        let approved = match self.trust {
            DeviceTrust::Gateway => Some(true),
            DeviceTrust::Approvals => approver::approved(&self.approver),
        };
        PhoneConnection::read(
            crate::managed::local_cache(crate::managed::policy()),
            self.me().map(|device| status(device, now_ms())),
            approved,
            available,
        )
    }

    /// The parts of the page that show in this mode.
    fn parts(&self) -> Parts {
        gateway_trust::parts(self.trust, self.remote_preview)
    }

    /// Other computers, while this mode shows them.
    fn remote_computers(&self) -> Option<&Entity<remote_computers::RemoteComputers>> {
        self.remote_computers.as_ref().filter(|_| self.parts().remote_computers)
    }

    /// Take a directory Gateway served: its devices and its trust mode. Approvals applies at once; Gateway's own trust
    /// applies once the pin allows it (`settle_trust`).
    fn take_directory(&mut self, value: &Value) -> Vec<Device> {
        self.offered = read_device_trust(value);
        if self.offered == DeviceTrust::Approvals {
            self.trust = DeviceTrust::Approvals;
            self.gateway_waits = None;
        }
        read_devices(value)
    }

    /// In gateway mode the page registers this computer itself when Gateway does not list it, or lists it pending from
    /// approvals (Gateway trusts the same key now), so it does not depend on the sign-in's registration. It tries once;
    /// a refusal shows under This device with Try again.
    fn register_if_gateway(&mut self, cx: &mut Context<Self>) {
        if self.trust != DeviceTrust::Gateway || self.auto_registered || self.busy {
            return;
        }
        let listed = self.me().is_some_and(|me| me.state != "pending");
        if !matches!((&self.key, &self.devices), (Load::Ready(_), Load::Ready(_))) || listed {
            return;
        }
        if !crate::managed::local_cache(crate::managed::policy()) {
            return;
        }
        self.auto_registered = true;
        self.register(Some(crate::computer_name::desktop_label()), cx);
    }

    fn connect_phone(&mut self, cx: &mut Context<Self>) {
        if self.link_busy || !self.phone_connection().can_invite() {
            return;
        }
        let Some(commands) = (self.ctx.code)(cx).projects else {
            self.link_outcome = Some((
                "Show a connection code in the Nexus app that owns Code sessions.".into(),
                true,
            ));
            cx.notify();
            return;
        };
        self.link_busy = true;
        self.link_outcome = None;
        let request = commands.phone_invitation_begin();
        cx.spawn(async move |this, cx| {
            let answer = request
                .await
                .unwrap_or_else(|_| Err("The computer did not answer.".into()))
                .and_then(PhoneLink::new);
            this.update(cx, |page, cx| {
                page.link_busy = false;
                match answer {
                    Ok(i) => {
                        page.link_invitation = Some(i);
                        page.load(cx);
                        page.poll_phone_link(cx);
                    }
                    Err(e) => page.link_outcome = Some((e, true)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn cancel_phone_link(&mut self, cx: &mut Context<Self>) {
        self.link_candidate = None;
        if let Some(i) = self.link_invitation.take()
            && let Some(commands) = (self.ctx.code)(cx).projects
        {
            let request = commands.phone_invitation_cancel(i.invitation.invite_id);
            cx.spawn(async move |_, _| {
                let _ = request.await;
            })
            .detach();
        }
        self.link_outcome = None;
        cx.notify();
    }
    fn poll_phone_link(&mut self, cx: &mut Context<Self>) {
        if self.link_polling {
            return;
        }
        let Some(i) = &self.link_invitation else {
            return;
        };
        let id = i.invitation.invite_id.clone();
        let Some(commands) = (self.ctx.code)(cx).projects else {
            return;
        };
        let request = commands.phone_invitation_poll(id.clone());
        self.link_polling = true;
        cx.spawn(async move |this, cx| {
            let answer = request
                .await
                .unwrap_or_else(|_| Err("The computer did not answer.".into()));
            let active = this
                .update(cx, |page, cx| {
                    page.link_polling = false;
                    if page
                        .link_invitation
                        .as_ref()
                        .is_none_or(|i| i.invitation.invite_id != id)
                    {
                        // A new code replaced this one while this read was out; read for the new one next.
                        return page.link_invitation.is_some();
                    }
                    match answer {
                        Ok(s) if s.state == "connected" => page.phone_connected(s.name, cx),
                        Ok(s) if s.state == "expired" || s.state == "cancelled" => {
                            page.link_invitation = None;
                            page.link_candidate = None;
                            page.link_outcome = Some((
                                if s.state == "expired" {
                                    "This connection code expired. Show a new connection code."
                                } else {
                                    "Connection cancelled."
                                }
                                .into(),
                                false,
                            ));
                        }
                        Ok(s) if s.state == "approving" => {
                            if page.link_candidate.is_none() {
                                page.link_outcome = None;
                            }
                            page.link_candidate = Some(s);
                        }
                        Ok(_) => {
                            page.link_candidate = None;
                            page.link_outcome = None;
                        }
                        Err(e) => page.link_outcome = Some((e, true)),
                    };
                    cx.notify();
                    page.link_invitation.is_some()
                })
                .unwrap_or(false);
            if active {
                cx.background_executor().timer(POLL).await;
                this.update(cx, |page, cx| page.poll_phone_link(cx)).ok();
            }
        })
        .detach();
    }
    fn approve_phone_link(&mut self, cx: &mut Context<Self>) {
        if self.link_busy
            || matches!(
                self.phone_connection(),
                PhoneConnection::Managed | PhoneConnection::Expired | PhoneConnection::Revoked
            )
        {
            return;
        }
        let (Some(invite), Some(candidate)) = (&self.link_invitation, &self.link_candidate) else {
            return;
        };
        let (Some(device), Some(code)) = (&candidate.device_id, &candidate.code) else {
            return;
        };
        let Some(commands) = (self.ctx.code)(cx).projects else {
            return;
        };
        let invite_id = invite.invitation.invite_id.clone();
        let request = commands.phone_invitation_approve(invite_id.clone(), device.clone(), code.clone());
        self.link_busy = true;
        self.link_outcome = None;
        cx.spawn(async move |this, cx| {
            let answer = request
                .await
                .unwrap_or_else(|_| Err("The computer did not answer.".into()));
            this.update(cx, |page, cx| {
                page.link_busy = false;
                if page
                    .link_invitation
                    .as_ref()
                    .is_none_or(|i| i.invitation.invite_id != invite_id)
                {
                    cx.notify();
                    return;
                }

                match answer {
                    Ok(status) if status.state == "connected" => page.phone_connected(status.name, cx),
                    Ok(_) => {
                        page.link_outcome = Some(("The invitation is no longer waiting for approval.".into(), true))
                    }
                    Err(error) => page.link_outcome = Some((error, true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn load_control(&mut self, cx: &mut Context<Self>) {
        self.watch_control(cx);
        let Some(commands) = (self.ctx.code)(cx).projects else {
            self.phone_control =
                Load::Failed("The other Nexus app owns Code sessions. Change phone control there.".into());
            return;
        };
        let request = commands.code_control();
        cx.spawn(async move |this, cx| {
            let answer = request
                .await
                .unwrap_or_else(|_| Err("Code sessions did not answer.".into()));
            this.update(cx, |page, cx| {
                // A change being saved answers with what it saved; this older read does not replace it.
                if page.control_busy {
                    return;
                }
                page.phone_control = match answer {
                    Ok(settings) => Load::Ready(settings),
                    Err(error) => Load::Failed(error.into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads Phone control again every `CONTROL_POLL` while the page is open. Session sync connects in the background
    /// after launch and after the device key is made, so the first read can come too early. The loop ends with the page.
    fn watch_control(&mut self, cx: &mut Context<Self>) {
        if self.control_watched {
            return;
        }
        self.control_watched = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::wakeups::pace(CONTROL_POLL, cx))
                    .await;
                let open = this.update(cx, |page, cx| {
                    if !page.control_busy {
                        page.load_control(cx);
                    }
                    // Phone connection waits for the approver: a failed read is read again, so it does not wait forever.
                    if matches!(page.approver, Load::Failed(_)) {
                        page.load_approver(cx);
                    }
                });
                if open.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn set_control(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.control_busy {
            return;
        }
        let Some(commands) = (self.ctx.code)(cx).projects else {
            self.load_control(cx);
            return;
        };
        self.control_busy = true;
        let request = commands.set_code_control(enabled);
        cx.spawn(async move |this, cx| {
            let answer = request
                .await
                .unwrap_or_else(|_| Err("Code sessions did not answer.".into()));
            this.update(cx, |page, cx| {
                page.control_busy = false;
                page.phone_control = match answer {
                    Ok(settings) => Load::Ready(settings),
                    Err(error) => Load::Failed(error.into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn phone_connected(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let name = name.unwrap_or_else(|| "your device".into());
        let message = if self.remote_computers().is_some() {
            format!("Connected to {name}. It can now control this computer.")
        } else {
            format!("Connected to {name}. Use Remote control to allow it to run work here.")
        };
        self.link_invitation = None;
        self.link_candidate = None;
        self.link_outcome = Some((message, false));
        self.load(cx);
        self.load_control(cx);
    }

    /// Try again on a key the keychain refused: the person asked, so it may ask them once more.
    fn retry_key(&mut self, cx: &mut Context<Self>) {
        crate::keychain::retry_device_key(self.ctx.gateway.url());
        self.key = Load::Loading;
        self.load_key(cx);
    }

    fn load_key(&mut self, cx: &mut Context<Self>) {
        let gateway = self.ctx.gateway.url().to_string();
        cx.spawn(async move |this, cx| {
            let key = off_thread(move || load_or_create(&gateway)).await;
            this.update(cx, |page, cx| {
                page.key = match key {
                    Ok((key, made)) => {
                        if made {
                            page.key_changed(cx);
                        }
                        Load::Ready(Key::new(key))
                    }
                    Err(error) => Load::Failed(error.into()),
                };
                page.poll_if_pending(cx);
                page.load_approver(cx);
                page.load_org_key(cx);
                page.register_if_gateway(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/devices".into());
        super::read(cx, request, |this, answer, cx| {
            match answer {
                Ok(value) => this.devices = Load::Ready(this.take_directory(&value)),
                // A failed refresh keeps the list read.
                Err(error) if !matches!(this.devices, Load::Ready(_)) => this.devices = Load::Failed(error.into()),
                Err(_) => {}
            }
            this.poll_if_pending(cx);
            this.load_approver(cx);
            this.load_org_key(cx);
            this.settle_trust(cx);
            this.register_if_gateway(cx);
        });
    }

    fn me(&self) -> Option<&Device> {
        let (Load::Ready(key), Load::Ready(devices)) = (&self.key, &self.devices) else {
            return None;
        };
        this_device(devices, &key.encryption_key, &key.signing_key)
    }

    /// While this device waits for approval, read the directory again every few seconds.
    fn poll_if_pending(&mut self, cx: &mut Context<Self>) {
        let pending = self.me().is_some_and(|me| status(me, now_ms()) == Status::Pending);
        if !pending || self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::wakeups::pace(POLL, cx)).await;
            this.update(cx, |page, cx| {
                page.polling = false;
                page.load(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Register this device: with the typed name, or `again` (the name a new key keeps).
    fn register(&mut self, again: Option<String>, cx: &mut Context<Self>) {
        let Load::Ready(key) = &self.key else {
            return;
        };
        let name = again.unwrap_or_else(|| self.name.read(cx).value().trim().to_string());
        if self.busy || name.is_empty() {
            return;
        }
        let body = json!({ "name": name, "encryptionKey": key.encryption_key, "signingKey": key.signing_key });
        self.busy = true;
        self.outcome = None;
        let request = self.ctx.gateway.request(Verb::Post, "/v1/devices".into(), Some(body));
        fetch(cx, request, |this, answer, cx| {
            this.busy = false;
            match answer {
                // Gateway trusts the key at once: session sync reads the list again and syncs with it.
                Ok(_) if this.trust == DeviceTrust::Gateway => this.key_changed(cx),
                Ok(_) => {}
                Err(error) => {
                    this.outcome = Some((
                        Part::ThisDevice,
                        format!("This device wasn't set up: {}", refusal(&error)),
                        true,
                    ))
                }
            }
            this.load(cx);
        });
        cx.notify();
    }

    /// A revoked or expired device makes a new key and registers again under its name. The old key would not do:
    /// Gateway answers a known key with its row as it is, revoked or expired.
    fn new_key(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self
            .me()
            .map(|me| me.name.clone())
            .unwrap_or_else(crate::computer_name::desktop_label);
        let gateway = self.ctx.gateway.url().to_string();
        self.busy = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let key = off_thread(move || {
                forget(&gateway)?;
                load_or_create(&gateway)
            })
            .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match key {
                    Ok((key, _)) => {
                        page.key = Load::Ready(Key::new(key));
                        page.key_changed(cx);
                        page.register(Some(name), cx);
                    }
                    Err(error) => {
                        page.outcome = Some((
                            Part::ThisDevice,
                            format!("This device wasn't set up again: {error}"),
                            true,
                        ))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The keychain holds a new device key: session sync takes it up, here and in the background service.
    fn key_changed(&self, cx: &App) {
        if let Some(changed) = (self.ctx.code)(cx).device_key_changed {
            changed();
        }
    }

    /// Find the pending device by the typed code, in the list Gateway serves now.
    fn find(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.approve_outcome = None;
        let input = match read_pairing_input(&self.code.read(cx).value()) {
            Ok(input) => input,
            Err(error) => {
                self.approve_outcome = Some((error, true));
                cx.notify();
                return;
            }
        };
        self.busy = true;
        let request = self.ctx.gateway.request(Verb::Get, "/v1/devices".into(), None);
        fetch(cx, request, move |this, answer, _| {
            this.busy = false;
            match answer {
                Ok(value) => {
                    let devices = this.take_directory(&value);
                    match find_pending(&devices, &input, now_ms()) {
                        Ok(device) => this.found = Some((device, input)),
                        Err(error) => this.approve_outcome = Some((error, true)),
                    }
                    this.devices = Load::Ready(devices);
                }
                Err(error) => this.approve_outcome = Some((error, true)),
            }
        });
        cx.notify();
    }

    fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Load::Ready(key), Some(me), Some((device, input))) = (&self.key, self.me(), &self.found) else {
            return;
        };
        if self.busy {
            return;
        }
        if !matches_input(device, input) {
            self.approve_outcome = Some((
                "That device's keys do not match the code. Do not approve it.".into(),
                true,
            ));
            cx.notify();
            return;
        }
        let signature = match sign_approval(&key.key, device, &input.code) {
            Ok(signature) => signature,
            Err(error) => {
                self.approve_outcome = Some((error, true));
                cx.notify();
                return;
            }
        };
        let name = device.name.clone();
        let path = format!("/v1/devices/{}/approve", device.id);
        let mut body = json!({ "approverDeviceId": me.id, "signature": signature, "keys": [] });
        let devices = match &self.devices {
            Load::Ready(devices) => devices.clone(),
            _ => Vec::new(),
        };
        let handover = approver::handover_for(&self.ctx.gateway, &key.key, &devices, device);
        let gateway = self.ctx.gateway.clone();
        self.busy = true;
        self.code.update(cx, |state, cx| state.set_value("", window, cx));
        // This computer's trusted list and revoked ids go with the approval, sealed to the new device.
        let request = async move {
            if let Some(handover) = handover.await {
                body["handover"] = json!(handover);
            }
            gateway.request(Verb::Post, path, Some(body)).await
        };
        fetch(cx, request, move |this, answer, cx| match answer {
            Ok(_) => this.show_approved(name, cx),
            Err(error) => {
                this.busy = false;
                this.found = None;
                this.approve_outcome = Some((format!("The device wasn't approved: {}", refusal(&error)), true));
                this.load(cx);
            }
        });
        cx.notify();
    }

    /// Read the list again, then end "Approving…", so the note and the device's row show together.
    /// `load` can skip an answer that matches its saved read, so this reads Gateway directly.
    fn show_approved(&mut self, name: String, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.request(Verb::Get, "/v1/devices".into(), None);
        fetch(cx, request, move |this, answer, cx| {
            this.busy = false;
            this.found = None;
            this.approve_outcome = Some((
                format!("{name} is trusted. It shows this device's fingerprint now; check that it matches."),
                false,
            ));
            match answer {
                Ok(value) => this.devices = Load::Ready(this.take_directory(&value)),
                // The approval stands. The list catches up on its next read.
                Err(_) => this.load(cx),
            }
        });
    }

    fn revoke(&mut self, device_id: String, cx: &mut Context<Self>) {
        if self.confirm_revoke.as_deref() != Some(device_id.as_str()) {
            self.confirm_revoke = Some(device_id);
            cx.notify();
            return;
        }
        self.confirm_revoke = None;
        let (Load::Ready(key), Some(me), Load::Ready(devices)) = (&self.key, self.me(), &self.devices) else {
            return;
        };
        if self.busy {
            return;
        }
        let signature = match e2e::revocation_message(&device_id) {
            Ok(message) => e2e::b64u(&key.key.sign(&message)),
            Err(error) => {
                self.outcome = Some((Part::AllDevices, format!("The device wasn't revoked: {error}"), true));
                cx.notify();
                return;
            }
        };
        let done = if device_id == me.id {
            "This device is revoked. It gets no new keys.".to_string()
        } else {
            let name = devices
                .iter()
                .find(|device| device.id == device_id)
                .map(|device| device.name.clone())
                .unwrap_or_else(|| "The device".into());
            format!("{name} is revoked. It gets no new keys.")
        };
        let path = format!("/v1/devices/{device_id}/revoke");
        let body = json!({ "revokerDeviceId": me.id, "signature": signature });
        self.busy = true;
        self.outcome = None;
        let request = self.ctx.gateway.request(Verb::Post, path, Some(body));
        fetch(cx, request, move |this, answer, cx| {
            this.busy = false;
            this.outcome = Some(match answer {
                Ok(_) => (Part::AllDevices, done, false),
                Err(error) => (
                    Part::AllDevices,
                    format!("The device wasn't revoked: {}", refusal(&error)),
                    true,
                ),
            });
            this.load(cx);
        });
        cx.notify();
    }

    /// Cancel the running invitation and make a new one.
    fn renew_phone_link(&mut self, cx: &mut Context<Self>) {
        self.cancel_phone_link(cx);
        self.connect_phone(cx);
    }

    /// Copy this device's fingerprint, and show a check on the button for `COPIED`.
    fn copy_fingerprint(&mut self, text: String, what: Copied, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied_at = Some((what, Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED).await;
            this.update(cx, |page, cx| {
                // A later copy keeps its own check for its whole time.
                if page.copied_at.is_some_and(|(_, at)| at.elapsed() >= COPIED) {
                    page.copied_at = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn cancel_revoke(&mut self, cx: &mut Context<Self>) {
        self.confirm_revoke = None;
        cx.notify();
    }
}

/// Gateway's refusal, for a person.
fn refusal(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("forbidden") || lower.contains("only the owner") || lower.contains("admins") {
        format!("You do not have permission for this: {error}")
    } else {
        error.to_string()
    }
}

#[cfg(test)]
#[path = "devices/approve_tests.rs"]
mod approve_tests;
#[cfg(test)]
#[path = "devices/gateway_page_tests.rs"]
mod gateway_page_tests;
#[cfg(test)]
#[path = "devices/start_again_tests.rs"]
mod start_again_tests;

#[cfg(test)]
mod tests {
    use super::{
        Device, DeviceKey, PAIRING_TTL_MS, PHONE_QR_BORDER, PhoneConnection, Status, Trust, b64u_decode, device_code,
        e2e, find_pending, format_code, iso_ms, qr_modules, read_devices, read_pairing_input, sign_approval, status,
        this_device, trust,
    };

    /// Sync has no switch in the app: only these gates stop a phone connection.
    #[test]
    fn phone_pairing_respects_local_gates_even_with_a_ready_backend() {
        assert_eq!(
            PhoneConnection::read(false, Some(Status::Trusted), Some(true), Some(true)),
            PhoneConnection::Managed
        );
        assert_eq!(
            PhoneConnection::read(true, Some(Status::Trusted), Some(true), Some(true)),
            PhoneConnection::Ready,
            "no environment switch is needed"
        );
        for (device, expected) in [
            (None, PhoneConnection::Setup),
            (Some(Status::Pending), PhoneConnection::Pending),
            (Some(Status::Expired), PhoneConnection::Expired),
            (Some(Status::Revoked), PhoneConnection::Revoked),
        ] {
            assert_eq!(PhoneConnection::read(true, device, Some(true), Some(true)), expected);
        }
    }

    #[test]
    fn a_computer_can_invite_a_trusted_phone_before_its_own_setup_finishes() {
        for (state, can_invite) in [
            (PhoneConnection::Setup, true),
            (PhoneConnection::Pending, true),
            (PhoneConnection::Approver, true),
            (PhoneConnection::Ready, true),
            (PhoneConnection::Managed, false),
            (PhoneConnection::Expired, false),
            (PhoneConnection::Revoked, false),
            (PhoneConnection::Checking, false),
            (PhoneConnection::Unavailable, false),
        ] {
            assert_eq!(state.can_invite(), can_invite, "{state:?}");
        }
    }

    #[test]
    fn phone_pairing_waits_for_sync_and_recovers_when_it_connects() {
        for (available, expected) in [
            (None, PhoneConnection::Checking),
            (Some(false), PhoneConnection::Unavailable),
            (Some(true), PhoneConnection::Ready),
        ] {
            assert_eq!(
                PhoneConnection::read(true, Some(Status::Trusted), Some(true), available),
                expected
            );
        }
    }

    /// The runner locks nothing for an approver the person has not confirmed (`APPROVER_UNCONFIRMED`), so neither
    /// a phone connection nor Phone control is ready until then. A root has no approver: `approved` reads true for it.
    #[test]
    fn phone_pairing_waits_for_the_confirmed_approver() {
        for available in [None, Some(false), Some(true)] {
            assert_eq!(
                PhoneConnection::read(true, Some(Status::Trusted), Some(false), available),
                PhoneConnection::Approver
            );
        }
        assert_eq!(
            PhoneConnection::read(true, Some(Status::Trusted), None, Some(true)),
            PhoneConnection::Checking
        );
        assert!(PhoneConnection::Approver.message().contains("under This device"));
    }

    #[test]
    fn pairing_qr_has_a_four_module_quiet_zone() {
        let modules = qr_modules("nexus://connect#invite=sample", PHONE_QR_BORDER).expect("QR");
        for offset in 0..4 {
            assert!(modules[offset].iter().all(|&dark| !dark));
            assert!(modules[modules.len() - 1 - offset].iter().all(|&dark| !dark));
            assert!(modules.iter().all(|row| !row[offset] && !row[row.len() - 1 - offset]));
        }
        assert!(modules[4][4], "the position marker starts after the quiet zone");
    }

    /// A device row as Gateway lists it, for `key`.
    fn row(id: &str, key: &DeviceKey, state: &str, created_at: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": format!("Device {id}"),
            "encryptionKey": e2e::b64u(&key.encryption_key()), "signingKey": e2e::b64u(&key.signing_key()),
            "state": state, "approvedBy": null, "approvalSignature": null,
            "createdAt": created_at, "approvedAt": null, "revokedAt": null
        })
    }

    #[test]
    fn reads_the_directory_and_checks_approvals() {
        let first = DeviceKey::generate();
        let second = DeviceKey::generate();
        let mut approved = row("dev_second00000", &second, "trusted", "2026-11-05T10:05:00Z");
        let device = read_devices(&serde_json::json!({ "devices": [approved.clone()] })).remove(0);
        let signature = sign_approval(&first, &device, &device_code(&device)).expect("signed");
        approved["approvedBy"] = "dev_first000000".into();
        approved["approvalSignature"] = signature.into();
        approved["approvedAt"] = "2026-11-05T10:06:00Z".into();
        let devices = read_devices(&serde_json::json!({ "devices": [
            row("dev_first000000", &first, "trusted", "2026-11-05T10:00:00.250Z"),
            approved,
            {"id": "broken"}
        ]}));
        assert_eq!(devices.len(), 2, "a row without keys is dropped");
        assert_eq!(trust(&devices[0], &devices), Some(Trust::Root));
        assert_eq!(trust(&devices[1], &devices), Some(Trust::Approved));
        let mut forged = devices.clone();
        forged[1].approved_by = Some("dev_missing0000".into());
        assert_eq!(trust(&forged[1], &forged), Some(Trust::Unverified));
        let mine = this_device(
            &devices,
            &e2e::b64u(&second.encryption_key()),
            &e2e::b64u(&second.signing_key()),
        );
        assert_eq!(mine.map(|d| d.id.as_str()), Some("dev_second00000"));
    }

    #[test]
    fn finds_a_pending_device_by_its_code() {
        let key = DeviceKey::generate();
        let now = iso_ms("2026-11-05T10:10:00Z").unwrap();
        let devices: Vec<Device> = read_devices(&serde_json::json!({ "devices": [
            row("dev_pending0000", &key, "pending", "2026-11-05T10:00:00Z")
        ]}));
        let code = device_code(&devices[0]);
        assert_eq!(code.len(), 16);
        let typed = format_code(&code).to_lowercase();
        let input = read_pairing_input(&typed).expect("a code");
        assert_eq!(
            find_pending(&devices, &input, now).expect("found").id,
            "dev_pending0000"
        );
        let later = now + PAIRING_TTL_MS;
        assert_eq!(status(&devices[0], later), Status::Expired);
        assert!(
            find_pending(&devices, &input, later)
                .unwrap_err()
                .contains("15 minutes")
        );
        // A pairing QR code's text, as another app draws it, still reads.
        let qr = format!("nexus-pair:v1:dev_pending0000:{}", e2e::b64u(&key.fingerprint()));
        let scanned = read_pairing_input(&qr).expect("QR text");
        assert_eq!(scanned.code, code);
        assert!(read_pairing_input("not a code").is_err());
        let other = DeviceKey::generate();
        assert!(sign_approval(&other, &devices[0], "AAAAAAAAAAAAAAAA").is_err());
    }

    #[test]
    fn draws_a_qr_code_with_a_light_border_and_its_finder_pattern() {
        let b = PHONE_QR_BORDER as usize;
        let modules = qr_modules("nexus://connect#invite=sample", PHONE_QR_BORDER).expect("fits");
        let side = modules.len();
        assert!(modules.iter().all(|row| row.len() == side), "square");
        // A version's side is 17 + 4 × version, plus the border on each side.
        assert_eq!((side - 2 * b - 17) % 4, 0);
        let border = |x: usize, y: usize| x < b || y < b || x >= side - b || y >= side - b;
        for (y, row) in modules.iter().enumerate() {
            for (x, dark) in row.iter().enumerate() {
                if border(x, y) {
                    assert!(!dark, "the border is light at {x},{y}");
                }
            }
        }
        // The top-left finder pattern: a dark 7 × 7 ring, a light ring, a dark 3 × 3 centre.
        for i in 0..7 {
            assert!(modules[b][b + i] && modules[b + 6][b + i] && modules[b + i][b] && modules[b + i][b + 6]);
        }
        assert!(!modules[b + 1][b + 1] && modules[b + 3][b + 3]);
    }

    #[test]
    fn reads_times_and_base64url() {
        assert_eq!(iso_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(iso_ms("2026-11-05T12:00:00+01:00"), iso_ms("2026-11-05T11:00:00Z"));
        assert_eq!(iso_ms("2026-11-05T12:00:00.5Z").unwrap() % 1000, 500);
        assert_eq!(b64u_decode("aGk"), Some(b"hi".to_vec()));
        assert_eq!(b64u_decode("a+b"), None);
    }
}
