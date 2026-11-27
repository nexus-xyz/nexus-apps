//! The encrypted, signed phone-command transport. The only plaintext sent back is delivery state.

use super::*;
use crate::session_sync::control::{Action, Command, MAX_COMMAND_BYTES, WAIT_MS};
use serde::Deserialize;

const INVALID: &str = "phone control: the command is invalid or is not from a trusted device";
/// The longest life a phone may sign on a command, as Gateway allows it.
const MAX_COMMAND_LIFE_MS: i64 = 60_000;
/// How far the phone's clock may run ahead of this one. Gateway allows the same (`CLOCK_SKEW_MS`), so the runner
/// does not drop a command that Gateway accepted.
const CLOCK_SKEW_MS: i64 = 30_000;

fn receipt_target_gone(error: &ClientError) -> bool {
    matches!(error, ClientError::Core(core) if matches!((core.status, core.code.as_deref()),
        (410, Some("thread_deleted"))
            | (404, Some("thread_not_found" | "code_command_not_found"))
            | (409, Some("code_runner_changed" | "code_command_not_claimed"))))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    id: String,
    thread_id: String,
    sender_device_id: String,
    runner_device_id: String,
    expires_at: i64,
    locked_key: String,
    sealed: String,
    signature: String,
    state: String,
    /// The sender let it wait for this computer while it slept. Absent from a v1 command.
    #[serde(default)]
    waits: bool,
}

impl Envelope {
    /// v1, or v2 with the sender's signed choice to wait. A runner from before the wait option verifies only v1, so it
    /// never runs a waiting command: the command expires.
    fn message(&self) -> String {
        let routing = format!(
            "{}\n{}\n{}\n{}\n{}",
            self.thread_id, self.id, self.sender_device_id, self.runner_device_id, self.expires_at
        );
        if self.waits {
            format!(
                "nexus-code-command-v2\n{routing}\n{}\n{}\n{}",
                self.waits, self.locked_key, self.sealed
            )
        } else {
            format!("nexus-code-command-v1\n{routing}\n{}\n{}", self.locked_key, self.sealed)
        }
    }

    /// The latest expiry this computer accepts, as Gateway does: a minute ahead, or a day for a command that waits,
    /// plus the clock skew allowed to the phone.
    fn latest_expiry(&self, now: i64) -> i64 {
        let life = if self.waits { WAIT_MS } else { MAX_COMMAND_LIFE_MS };
        now.saturating_add(life + CLOCK_SKEW_MS)
    }
}

/// The encrypted body. Version 1 is a command for a computer that is online. Version 2 is a command that
/// may wait: it carries `sentAt`, the sender's clock when the person sent it, so this computer judges each action's age.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Payload {
    version: u8,
    thread_id: String,
    #[serde(default)]
    sent_at: Option<i64>,
    action: Action,
}

fn bytes(text: &str) -> Result<Vec<u8>> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| INVALID)?;
    if e2e::b64u(&decoded) != text {
        return Err(INVALID.into());
    }
    Ok(decoded)
}

fn open_command(thread: &str, envelope: Envelope, trusted: &Trusted, device: &DeviceKey, now: i64) -> Result<Command> {
    if thread_path(&envelope.id).is_err()
        || envelope.thread_id != thread
        || envelope.runner_device_id != trusted.me.id
        || envelope.state != "queued"
        || envelope.expires_at <= now
        || envelope.expires_at > envelope.latest_expiry(now)
        || envelope.sealed.len() > (MAX_COMMAND_BYTES + 64) * 4 / 3 + 4
        || envelope.locked_key.len() > 512
    {
        return Err(INVALID.into());
    }
    let sender = trusted
        .holders
        .iter()
        .find(|holder| holder.id == envelope.sender_device_id)
        .ok_or(INVALID)?;
    let signing = sender.keys.signing.ok_or(INVALID)?;
    if !e2e::verify(&signing, &envelope.message(), &bytes(&envelope.signature)?) {
        return Err(INVALID.into());
    }
    let locked = bytes(&envelope.locked_key)?;
    let key = device
        .unlock_content_key(&envelope.id, 0, &trusted.me.id, &locked)
        .map_err(|_| INVALID)?;
    let plaintext = key
        .open_event(&envelope.id, 0, &bytes(&envelope.sealed)?)
        .map_err(|_| INVALID)?;
    if plaintext.len() > MAX_COMMAND_BYTES {
        return Err(INVALID.into());
    }
    let payload: Payload = serde_json::from_slice(&plaintext).map_err(|_| INVALID)?;
    // A waiting command is version 2 and says when it was sent; a command for an online computer is version 1.
    let sent_at = match (envelope.waits, payload.version, payload.sent_at) {
        (false, 1, None) => None,
        (true, 2, Some(sent_at)) => Some(sent_at),
        _ => return Err(INVALID.into()),
    };
    if payload.thread_id != thread || !payload.action.valid() {
        return Err(INVALID.into());
    }
    Ok(Command {
        id: envelope.id,
        thread_id: envelope.thread_id,
        sender_device_id: envelope.sender_device_id,
        expires_at: envelope.expires_at,
        sent_at,
        action: payload.action,
    })
}

impl GatewayBackend {
    pub(super) async fn publish_control(&self, thread: &str, enabled: bool) -> Result<()> {
        thread_path(thread)?;
        let trusted = self.directory().await?;
        let expires_at = crate::now_ms() + 45_000;
        let message = format!(
            "nexus-code-control-v1\n{thread}\n{}\n{enabled}\n{expires_at}",
            trusted.me.id
        );
        self.client
            .put(
                &format!("/v1/threads/{thread}/control"),
                json!({
                    "runnerDeviceId":trusted.me.id,"enabled":enabled,"expiresAt":expires_at,
                    "signature":e2e::b64u(&self.device.sign(&message)),
                }),
            )
            .await
            .map_err(|error| refusal(&error))?;
        Ok(())
    }

    pub(super) async fn read_control(&self) -> Result<Vec<Command>> {
        let trusted = self.directory().await?;
        let answer = self
            .client
            .get(&format!("/v1/devices/{}/code-commands", trusted.me.id))
            .await
            .map_err(|error| refusal(&error))?;
        let commands = answer.get("commands").and_then(Value::as_array).ok_or(INVALID_ANSWER)?;
        if commands.len() > 20 {
            return Err(INVALID_ANSWER.into());
        }
        Ok(commands
            .iter()
            .filter_map(|value| {
                let envelope: Envelope = serde_json::from_value(value.clone()).ok()?;
                let thread = envelope.thread_id.clone();
                if thread_path(&thread).is_err() {
                    return None;
                }
                open_command(&thread, envelope, &trusted, &self.device, crate::now_ms()).ok()
            })
            .collect())
    }

    pub(super) async fn claim_control(&self, thread: &str, id: &str) -> Result<bool> {
        thread_path(thread)?;
        thread_path(id)?;
        let trusted = self.directory().await?;
        let message = format!("nexus-code-command-claim-v1\n{thread}\n{id}\n{}", trusted.me.id);
        let answer = self
            .client
            .post(
                &format!("/v1/threads/{thread}/commands/{id}/claim"),
                json!({
                    "runnerDeviceId":trusted.me.id,"signature":e2e::b64u(&self.device.sign(&message)),
                }),
            )
            .await
            .map_err(|error| refusal(&error))?;
        answer
            .get("claimed")
            .and_then(Value::as_bool)
            .ok_or_else(|| INVALID_ANSWER.into())
    }

    pub(super) async fn finish_control(&self, thread: &str, id: &str, completed: bool) -> Result<()> {
        thread_path(thread)?;
        thread_path(id)?;
        let trusted = self.directory().await?;
        let state = if completed { "completed" } else { "rejected" };
        let message = format!(
            "nexus-code-command-result-v1\n{thread}\n{id}\n{}\n{state}",
            trusted.me.id
        );
        match self
            .client
            .post(
                &format!("/v1/threads/{thread}/commands/{id}/result"),
                json!({
                    "runnerDeviceId":trusted.me.id,"state":state,"signature":e2e::b64u(&self.device.sign(&message)),
                }),
            )
            .await
        {
            Ok(_) => Ok(()),
            // The target cannot accept this receipt again. Retire delivery only: the local command-id
            // reservation remains, and Gateway's action outcome is neither replaced nor fabricated.
            Err(error) if receipt_target_gone(&error) => Ok(()),
            Err(error) => Err(refusal(&error)),
        }
    }

    /// Follows `GET /v1/computers/stream` and notifies `wake` when a command is queued for this computer, and
    /// once when the stream opens: a command queued while it was closed is not announced again. The event says only
    /// that the inbox changed: the loop reads and verifies the inbox as before.
    pub(super) async fn watch_control(&self, wake: &tokio::sync::Notify) -> Result<()> {
        let trusted = self.directory().await?;
        let accept = [("accept".to_string(), "text/event-stream".to_string())];
        let (status, headers, body) = self
            .client
            .stream(nexus_client::Method::GET, "/v1/computers/stream", &accept, None)
            .await
            .map_err(|error| refusal(&error))?;
        let events = headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("content-type") && value.contains("text/event-stream"));
        if !(200..300).contains(&status) || !events {
            return Err(format!(
                "phone control: Gateway refused the live command stream ({status})"
            ));
        }
        wake.notify_one();
        let mut body = std::pin::pin!(body);
        let mut buffer = Vec::new();
        while let Some(chunk) = futures::StreamExt::next(&mut body).await {
            buffer.extend_from_slice(&chunk.map_err(|error| refusal(&error))?);
            while let Some(at) = buffer.windows(2).position(|pair| pair == b"\n\n") {
                let block: Vec<u8> = buffer.drain(..at + 2).collect();
                if queued_for(&String::from_utf8_lossy(&block), &trusted.me.id) {
                    wake.notify_one();
                }
            }
            // Gateway's events are a few hundred bytes: a longer one is not Gateway's.
            if buffer.len() > MAX_EVENT_BYTES {
                return Err(INVALID_ANSWER.into());
            }
        }
        Ok(())
    }
}

/// The longest server-sent event the live command stream may hold.
const MAX_EVENT_BYTES: usize = 16 * 1024;

/// Whether one server-sent event of `GET /v1/computers/stream` says a command was queued for `runner`.
fn queued_for(block: &str, runner: &str) -> bool {
    let mut kind = None;
    let mut data = String::new();
    for line in block.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            kind = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim_start());
        }
    }
    kind == Some("command")
        && serde_json::from_str::<Value>(&data)
            .is_ok_and(|event| event["runnerDeviceId"] == runner && event["state"] == "queued")
}

#[cfg(test)]
mod tests {
    use super::super::super::trust::Holder;

    #[test]
    fn a_phone_command_sealed_by_the_typescript_kit_opens_on_the_rust_runner() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/phone-command.json")).expect("fixture");
        let secret: [u8; 64] = bytes(fixture["runnerSecret"].as_str().expect("secret"))
            .expect("base64")
            .try_into()
            .expect("64 bytes");
        let runner = DeviceKey::from_secret_bytes(&secret);
        let listed: Vec<super::super::super::trust::Listed> =
            serde_json::from_value(fixture["devices"].clone()).expect("directory");
        let me = Keys {
            encryption: runner.encryption_key(),
            signing: Some(runner.signing_key()),
        };
        let trusted =
            super::super::super::trust::verify(&listed, &me, &mut super::super::super::trust::confirmed(&listed, &me))
                .expect("approval chain");
        let mut wire = fixture["command"].clone();
        wire["threadId"] = fixture["threadId"].clone();
        wire["state"] = json!("queued");
        let envelope: Envelope = serde_json::from_value(wire).expect("envelope");
        let thread = fixture["threadId"].as_str().expect("thread");
        let now = fixture["now"].as_i64().expect("now");
        let opened = open_command(thread, envelope.clone(), &trusted, &runner, now).expect("kit command opens");
        assert!(matches!(opened.action,Action::Send{text} if text=="Fix the retry race in decoder.py"));
        let mut altered = envelope;
        altered.runner_device_id = "other".into();
        assert!(open_command(thread, altered, &trusted, &runner, now).is_err());
    }
    use super::*;

    #[test]
    fn only_a_command_queued_for_this_computer_wakes_the_loop() {
        let event = |kind: &str, runner: &str, state: &str| {
            format!(
                "event: {kind}\ndata: {}\n\n",
                json!({"threadId":"t","id":"c","runnerDeviceId":runner,"state":state})
            )
        };
        assert!(queued_for(&event("command", "dev_me", "queued"), "dev_me"));
        assert!(!queued_for(&event("command", "dev_other", "queued"), "dev_me"));
        assert!(!queued_for(&event("command", "dev_me", "claimed"), "dev_me"));
        assert!(!queued_for(&event("request", "dev_me", "queued"), "dev_me"));
        assert!(!queued_for(": keep-alive\n\n", "dev_me"));
        assert!(!queued_for("event: command\ndata: not json\n\n", "dev_me"));
    }

    #[test]
    fn only_known_permanent_result_errors_abandon_receipt_delivery() {
        let error = |status, code: &str| {
            ClientError::Core(Box::new(nexus_client::CoreError {
                status,
                message: "a redacted error".into(),
                code: Some(code.into()),
                interval: None,
                details: None,
            }))
        };
        for (status, code) in [
            (410, "thread_deleted"),
            (404, "thread_not_found"),
            (404, "code_command_not_found"),
            (409, "code_runner_changed"),
            (409, "code_command_not_claimed"),
        ] {
            assert!(receipt_target_gone(&error(status, code)), "{status} {code}");
        }
        for (status, code) in [
            (404, "not_found"),
            (401, "unauthorized"),
            (503, "unavailable"),
            (500, "code_command_not_found"),
            (401, "code_runner_changed"),
            (409, "invalid_signature"),
        ] {
            assert!(!receipt_target_gone(&error(status, code)), "{status} {code} must retry");
        }
        assert!(!receipt_target_gone(&ClientError::Transport(
            nexus_client::Transport::Unreachable("offline".into())
        )));
    }

    /// A v2 command waits up to a day and says when it was sent. A runner that reads only v1 (the v1 message)
    /// cannot verify it, and a version that does not match `waits` is refused.
    #[test]
    fn a_waiting_command_opens_only_as_v2_with_its_sending_time() {
        let runner = DeviceKey::generate();
        let phone = DeviceKey::generate();
        let holder = |id: &str, device: &DeviceKey| Holder {
            id: id.into(),
            keys: Keys {
                encryption: device.encryption_key(),
                signing: Some(device.signing_key()),
            },
        };
        let trusted = Trusted {
            me: holder("dev_runner", &runner),
            holders: vec![holder("dev_runner", &runner), holder("dev_phone", &phone)],
        };
        let thread = uuid::Uuid::new_v4().to_string();
        let now = 1_000;
        let day = WAIT_MS;
        let waiting = |payload: Value, expires_at: i64| {
            let mut envelope = command(&thread, &phone, &runner, payload);
            envelope.waits = true;
            envelope.expires_at = expires_at;
            envelope.signature = e2e::b64u(&phone.sign(&envelope.message()));
            envelope
        };
        let v2 = json!({"version":2,"threadId":thread,"sentAt":now - 5,"action":{"kind":"send","text":"Later"}});
        let opened = open_command(&thread, waiting(v2.clone(), now + day), &trusted, &runner, now).expect("opens");
        assert_eq!(opened.sent_at, Some(now - 5));
        assert!(matches!(opened.action, Action::Send { text } if text == "Later"));
        // Gateway never lists a cancelled command; if one were listed, it still would not open.
        let mut cancelled = waiting(v2.clone(), now + day);
        cancelled.state = "cancelled".into();
        assert!(
            open_command(&thread, cancelled, &trusted, &runner, now).is_err(),
            "a cancelled command never runs"
        );
        assert!(
            open_command(
                &thread,
                waiting(v2.clone(), now + day + CLOCK_SKEW_MS),
                &trusted,
                &runner,
                now
            )
            .is_ok(),
            "a day ahead by a phone clock 30 s fast, as Gateway accepts"
        );
        assert!(
            open_command(
                &thread,
                waiting(v2.clone(), now + day + CLOCK_SKEW_MS + 1),
                &trusted,
                &runner,
                now
            )
            .is_err(),
            "more than a day ahead"
        );
        // A runner from before the wait option verifies the v1 message, which the phone did not sign.
        let mut old = waiting(v2.clone(), now + 30_000);
        let v1 = old
            .message()
            .replacen("nexus-code-command-v2", "nexus-code-command-v1", 1);
        assert!(!e2e::verify(
            &phone.signing_key(),
            &v1,
            &bytes(&old.signature).expect("signature")
        ));
        old.waits = false;
        assert!(
            open_command(&thread, old, &trusted, &runner, now).is_err(),
            "waits is signed"
        );
        let v1_body = json!({"version":1,"threadId":thread,"action":{"kind":"send","text":"Later"}});
        assert!(
            open_command(&thread, waiting(v1_body, now + day), &trusted, &runner, now).is_err(),
            "a waiting command says when it was sent"
        );
        assert!(
            open_command(&thread, command(&thread, &phone, &runner, v2), &trusted, &runner, now).is_err(),
            "a v1 command has no sending time"
        );
    }

    fn command(thread: &str, sender: &DeviceKey, runner: &DeviceKey, payload: Value) -> Envelope {
        let id = uuid::Uuid::new_v4().to_string();
        let key = e2e::ContentKey::generate(0);
        let mut envelope = Envelope {
            id: id.clone(),
            thread_id: thread.into(),
            sender_device_id: "dev_phone".into(),
            runner_device_id: "dev_runner".into(),
            expires_at: 2000,
            locked_key: e2e::b64u(&key.lock_for(&id, "dev_runner", &runner.encryption_key()).expect("lock")),
            sealed: e2e::b64u(
                &key.seal_event(&id, 0, &serde_json::to_vec(&payload).expect("JSON"))
                    .expect("seal"),
            ),
            signature: String::new(),
            state: "queued".into(),
            waits: false,
        };
        envelope.signature = e2e::b64u(&sender.sign(&envelope.message()));
        envelope
    }

    #[test]
    fn only_a_fresh_signed_owner_command_for_this_thread_opens() {
        let runner = DeviceKey::generate();
        let phone = DeviceKey::generate();
        let stranger = DeviceKey::generate();
        let holder = |id: &str, device: &DeviceKey| Holder {
            id: id.into(),
            keys: Keys {
                encryption: device.encryption_key(),
                signing: Some(device.signing_key()),
            },
        };
        let trusted = Trusted {
            me: holder("dev_runner", &runner),
            holders: vec![holder("dev_runner", &runner), holder("dev_phone", &phone)],
        };
        let thread = uuid::Uuid::new_v4().to_string();
        let payload = json!({"version":1,"threadId":thread,"action":{"kind":"send","text":"Continue"}});
        let original = command(&thread, &phone, &runner, payload.clone());
        assert!(
            matches!(open_command(&thread,original.clone(),&trusted,&runner,1000).expect("opens").action,Action::Send{text} if text=="Continue")
        );
        assert!(
            open_command(&thread, original.clone(), &trusted, &runner, 2000).is_err(),
            "expired"
        );
        let mut tampered = original.clone();
        tampered.expires_at = 3000;
        assert!(
            open_command(&thread, tampered, &trusted, &runner, 1000).is_err(),
            "signed metadata"
        );
        let mut missing = trusted.clone();
        missing.holders.retain(|holder| holder.id != "dev_phone");
        assert!(
            open_command(&thread, original.clone(), &missing, &runner, 1000).is_err(),
            "revoked or untrusted"
        );
        assert!(
            open_command(
                &thread,
                command(&thread, &stranger, &runner, payload),
                &trusted,
                &runner,
                1000
            )
            .is_err(),
            "another person's key"
        );
        assert!(
            open_command(
                &thread,
                command(
                    &thread,
                    &phone,
                    &runner,
                    json!({"version":1,"threadId":"other","action":{"kind":"stop"}})
                ),
                &trusted,
                &runner,
                1000
            )
            .is_err(),
            "plaintext bound to thread"
        );
        assert!(
            open_command(
                &thread,
                command(
                    &thread,
                    &phone,
                    &runner,
                    json!({"version":1,"threadId":thread,"action":{"kind":"shell","command":"rm -rf"}})
                ),
                &trusted,
                &runner,
                1000
            )
            .is_err(),
            "only allowlisted actions"
        );
    }

    #[test]
    fn a_phone_clock_up_to_the_skew_ahead_still_opens_and_a_past_expiry_does_not() {
        let runner = DeviceKey::generate();
        let phone = DeviceKey::generate();
        let holder = |id: &str, device: &DeviceKey| Holder {
            id: id.into(),
            keys: Keys {
                encryption: device.encryption_key(),
                signing: Some(device.signing_key()),
            },
        };
        let trusted = Trusted {
            me: holder("dev_runner", &runner),
            holders: vec![holder("dev_runner", &runner), holder("dev_phone", &phone)],
        };
        let thread = uuid::Uuid::new_v4().to_string();
        let payload = json!({"version":1,"threadId":thread,"action":{"kind":"stop"}});
        let now = 1_000_000;
        let expiring = |expires_at: i64| {
            let mut envelope = command(&thread, &phone, &runner, payload.clone());
            envelope.expires_at = expires_at;
            envelope.signature = e2e::b64u(&phone.sign(&envelope.message()));
            envelope
        };
        let latest = now + MAX_COMMAND_LIFE_MS + CLOCK_SKEW_MS;
        assert!(
            open_command(&thread, expiring(latest), &trusted, &runner, now).is_ok(),
            "a phone clock 30 s ahead, as Gateway accepts"
        );
        assert!(
            open_command(&thread, expiring(latest + 1), &trusted, &runner, now).is_err(),
            "past the cap and the skew"
        );
        assert!(
            open_command(&thread, expiring(now), &trusted, &runner, now).is_err(),
            "already expired"
        );
        assert!(
            open_command(&thread, expiring(now - 1), &trusted, &runner, now).is_err(),
            "expired before now"
        );
    }
}
