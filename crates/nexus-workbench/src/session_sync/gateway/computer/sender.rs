//! Native renderer client: closed operations, authenticated replies and read-only synced history.
use super::*;
use crate::computer::ClientTransport;
use crate::session_sync::control::Action;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Presence {
    version: u8,
    scope: c::Scope,
    computer_id: String,
    runner_device_id: String,
    name: String,
    generation: i64,
    issued_at: i64,
    expires_at: i64,
    enabled: bool,
    capabilities: Vec<String>,
    signature: String,
}
impl Presence {
    fn message(&self) -> String {
        let mut caps = self.capabilities.clone();
        caps.sort();
        message(
            "nexus-computer-presence-v1",
            &self.scope,
            &[
                self.computer_id.clone(),
                self.runner_device_id.clone(),
                self.name.clone(),
                self.generation.to_string(),
                self.issued_at.to_string(),
                self.expires_at.to_string(),
                self.enabled.to_string(),
                caps.join(","),
            ],
        )
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplyEnvelope {
    version: u8,
    scope: c::Scope,
    computer_id: String,
    request_id: String,
    request_hash: String,
    runner_device_id: String,
    recipient_device_id: String,
    locked_key: String,
    sealed: String,
    signature: String,
}
impl ReplyEnvelope {
    fn message(&self) -> String {
        message(
            "nexus-computer-reply-v1",
            &self.scope,
            &[
                self.computer_id.clone(),
                self.request_id.clone(),
                self.request_hash.clone(),
                self.runner_device_id.clone(),
                self.recipient_device_id.clone(),
                self.locked_key.clone(),
                self.sealed.clone(),
            ],
        )
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplyPayload {
    version: u8,
    kind: String,
    scope: c::Scope,
    computer_id: String,
    request_id: String,
    request_hash: String,
    runner_device_id: String,
    recipient_device_id: String,
    result: c::Outcome,
}
impl GatewayBackend {
    async fn computer_presences(&self, trusted: &Trusted) -> Result<Vec<(Presence, Value)>> {
        let listed = self.client.get("/v1/computers").await.map_err(|e| refusal(&e))?;
        let mut result = Vec::new();
        for value in listed["computers"].as_array().ok_or(INVALID)?.iter().take(128) {
            let Ok(p) = serde_json::from_value::<Presence>(value.clone()) else {
                continue;
            };
            if p.version != 1
                || p.scope != self.computer_scope
                || thread_path(&p.computer_id).is_err()
                || p.generation < 1
                || p.generation > 9_007_199_254_740_991
                || p.issued_at < 0
                || p.issued_at > crate::now_ms() + 5_000
                || p.expires_at
                    .checked_sub(p.issued_at)
                    .is_none_or(|ttl| ttl <= 0 || ttl > 45_000)
                || p.name.len() > 160
                || p.name.chars().count() > 64
                || p.name.chars().any(char::is_control)
                || !known_capabilities(&p.capabilities)
            {
                continue;
            }
            let Some(signing) = trusted
                .holders
                .iter()
                .find(|h| h.id == p.runner_device_id)
                .and_then(|h| h.keys.signing)
            else {
                continue;
            };
            if !raw(&p.signature).is_ok_and(|sig| e2e::verify(&signing, &p.message(), &sig)) {
                continue;
            }
            result.push((p, value.clone()));
        }
        Ok(result)
    }
    async fn native_request_result(&self, out: c::Outbound) -> Result<c::CallResult> {
        thread_path(&out.target)?;
        thread_path(&out.id)?;
        let trusted = self.directory().await?;
        let expected: Envelope = serde_json::from_value(out.body.clone()).map_err(|_| INVALID)?;
        let row = self
            .client
            .get(&format!("/v1/computers/{}/requests/{}", out.target, out.id))
            .await
            .map_err(|e| refusal(&e))?;
        if row["envelope"] != out.body {
            return Err(INVALID.into());
        }
        let request_hash = hash(format!("{}{}", expected.message(), part(&expected.signature)).as_bytes());
        if row["requestHash"] != request_hash {
            return Err(INVALID.into());
        }
        let state = val(&row, "state")?;
        if !["queued", "claimed", "replied", "expired"].contains(&state.as_str()) {
            return Err(INVALID.into());
        }
        let result = if row["reply"].is_null() {
            None
        } else {
            Some(open_reply(
                &row["reply"],
                &expected,
                &request_hash,
                &self.computer_scope,
                &trusted,
                &self.device,
            )?)
        };
        Ok(c::CallResult {
            request_id: out.id,
            state,
            result,
            expires_at: out.expires_at,
        })
    }
}
/// Whether a presence lists only capabilities a runner signs ([`CAPABILITIES`]), each once.
fn known_capabilities(capabilities: &[String]) -> bool {
    capabilities.len() <= CAPABILITIES.len()
        && capabilities
            .iter()
            .enumerate()
            .all(|(i, c)| CAPABILITIES.contains(&c.as_str()) && !capabilities[..i].contains(c))
}
fn open_reply(
    value: &Value,
    request: &Envelope,
    expected_hash: &str,
    scope: &c::Scope,
    trusted: &Trusted,
    device: &DeviceKey,
) -> Result<c::Outcome> {
    let r: ReplyEnvelope = serde_json::from_value(value.clone()).map_err(|_| INVALID)?;
    if r.version != 1
        || r.scope != *scope
        || r.computer_id != request.computer_id
        || r.request_id != request.id
        || r.request_hash != expected_hash
        || r.runner_device_id != request.runner_device_id
        || r.recipient_device_id != trusted.me.id
        || r.sealed.len() > 350_000
        || r.locked_key.len() > 512
    {
        return Err(INVALID.into());
    }
    let signing = trusted
        .holders
        .iter()
        .find(|h| h.id == r.runner_device_id)
        .and_then(|h| h.keys.signing)
        .ok_or(INVALID)?;
    if !e2e::verify(&signing, &r.message(), &raw(&r.signature)?) {
        return Err(INVALID.into());
    }
    let key = device
        .unlock_content_key(&r.request_id, 0, &trusted.me.id, &raw(&r.locked_key)?)
        .map_err(|_| INVALID)?;
    let plain = key
        .open_event(&r.request_id, 0, &raw(&r.sealed)?)
        .map_err(|_| INVALID)?;
    if plain.len() > 256 * 1024 {
        return Err(INVALID.into());
    }
    let p: ReplyPayload = serde_json::from_slice(&plain).map_err(|_| INVALID)?;
    if p.version != 1
        || p.kind != "computerReply"
        || p.scope != r.scope
        || p.computer_id != r.computer_id
        || p.request_id != r.request_id
        || p.request_hash != r.request_hash
        || p.runner_device_id != r.runner_device_id
        || p.recipient_device_id != r.recipient_device_id
    {
        return Err(INVALID.into());
    }
    Ok(p.result)
}
impl ClientTransport for GatewayBackend {
    fn computers(&self) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let trusted = self.directory().await?;
            let computers = self
                .computer_presences(&trusted)
                .await?
                .into_iter()
                .map(|(_, v)| v)
                .collect::<Vec<_>>();
            Ok(json!({"computers":computers,"serverTime":crate::now_ms()}))
        })
    }
    fn seal_request(
        &self,
        computer: String,
        id: String,
        operation: c::Operation,
    ) -> BoxFuture<'_, Result<c::Outbound>> {
        Box::pin(async move {
            thread_path(&computer)?;
            thread_path(&id)?;
            if !operation.valid() {
                return Err(INVALID.into());
            }
            let trusted = self.directory().await?;
            let (p, _) = self
                .computer_presences(&trusted)
                .await?
                .into_iter()
                .find(|(p, _)| p.computer_id == computer && p.enabled && p.expires_at > crate::now_ms())
                .ok_or("This computer is offline or phone control is off.")?;
            let capability = match &operation {
                c::Operation::DescribeProjects | c::Operation::DescribeBranches { .. } => "projects",
                c::Operation::StartSession { .. } => "start",
                c::Operation::ReadSessionChanges { .. } => "changes",
                _ => "settings",
            };
            if !p.capabilities.iter().any(|c| c == capability) {
                return Err("Update Nexus on this computer to use this operation.".into());
            }
            let settings = operation.requires_settings();
            if settings && !p.capabilities.iter().any(|c| c == "settings") {
                return Err("Update Nexus on this computer to change its execution settings.".into());
            }
            let recipient = trusted
                .holders
                .iter()
                .find(|h| h.id == p.runner_device_id)
                .ok_or(INVALID)?;
            let now = crate::now_ms();
            let expiry = now + operation.admission_ms();
            let payload = json!({"version":1,"kind":"computerRequest","scope":self.computer_scope,"computerId":computer,"id":id,"senderDeviceId":trusted.me.id,"runnerDeviceId":p.runner_device_id,"generation":p.generation,"issuedAt":now,"expiresAt":expiry,"operation":operation});
            if serde_json::to_vec(&payload).map_err(|_| INVALID)?.len() > 64 * 1024 {
                return Err(INVALID.into());
            }
            let (locked_key, sealed) = self.seal_for(&id, recipient, &payload)?;
            let mut e = Envelope {
                version: 1,
                scope: self.computer_scope.clone(),
                computer_id: computer.clone(),
                id: id.clone(),
                sender_device_id: trusted.me.id.clone(),
                runner_device_id: p.runner_device_id,
                generation: p.generation,
                issued_at: now,
                expires_at: expiry,
                locked_key,
                sealed,
                signature: String::new(),
            };
            e.signature = e2e::b64u(&self.device.sign(&e.message()));
            Ok(c::Outbound {
                id,
                target: computer,
                kind: "computer".into(),
                body: serde_json::to_value(e).map_err(|_| INVALID)?,
                expires_at: expiry,
            })
        })
    }
    fn send_request(&self, out: c::Outbound) -> BoxFuture<'_, Result<c::CallResult>> {
        Box::pin(async move {
            let _ = self.directory().await?;
            let posted = self
                .client
                .post(&format!("/v1/computers/{}/requests", out.target), out.body.clone())
                .await;
            match self.native_request_result(out).await {
                Ok(result) => Ok(result),
                Err(error) => Err(posted.err().map(|e| refusal(&e)).unwrap_or(error)),
            }
        })
    }
    fn get_request(&self, out: c::Outbound) -> BoxFuture<'_, Result<c::CallResult>> {
        Box::pin(self.native_request_result(out))
    }
    fn seal_command(&self, thread: String, id: String, action: Action) -> BoxFuture<'_, Result<c::Outbound>> {
        Box::pin(async move {
            thread_path(&thread)?;
            thread_path(&id)?;
            if !action.valid() {
                return Err(INVALID.into());
            }
            let trusted = self.directory().await?;
            let control = self
                .client
                .get(&format!("/v1/threads/{thread}/control"))
                .await
                .map_err(|e| refusal(&e))?;
            let runner = val(&control, "runnerDeviceId")?;
            if control["enabled"] != true || control["expiresAt"].as_i64().is_none_or(|e| e <= crate::now_ms()) {
                return Err("This computer is offline or phone control is off.".into());
            }
            let recipient = trusted
                .holders
                .iter()
                .find(|h| h.id == runner && h.keys.signing.is_some())
                .ok_or(INVALID)?;
            let expiry = crate::now_ms() + 60_000;
            let payload = json!({"version":1,"threadId":thread,"action":action});
            if serde_json::to_vec(&payload).map_err(|_| INVALID)?.len() > 64 * 1024 {
                return Err(INVALID.into());
            }
            let (locked, sealed) = self.seal_for(&id, recipient, &payload)?;
            let m = format!(
                "nexus-code-command-v1\n{thread}\n{id}\n{}\n{runner}\n{expiry}\n{locked}\n{sealed}",
                trusted.me.id
            );
            Ok(c::Outbound {
                id: id.clone(),
                target: thread,
                kind: "thread".into(),
                expires_at: expiry,
                body: json!({"id":id,"senderDeviceId":trusted.me.id,"runnerDeviceId":runner,"expiresAt":expiry,"lockedKey":locked,"sealed":sealed,"signature":e2e::b64u(&self.device.sign(&m))}),
            })
        })
    }
    fn send_command(&self, out: c::Outbound) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let _ = self.directory().await?;
            let posted = self
                .client
                .post(&format!("/v1/threads/{}/commands", out.target), out.body)
                .await;
            match self.get_command(out.target, out.id).await {
                Ok(row) => Ok(row),
                Err(error) => Err(posted.err().map(|e| refusal(&e)).unwrap_or(error)),
            }
        })
    }
    fn get_command(&self, thread: String, id: String) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            thread_path(&thread)?;
            thread_path(&id)?;
            let _ = self.directory().await?;
            let row = self
                .client
                .get(&format!("/v1/threads/{thread}/commands/{id}"))
                .await
                .map_err(|e| refusal(&e))?;
            if row["id"] != id || row["threadId"] != thread {
                return Err(INVALID.into());
            }
            Ok(row)
        })
    }
    fn synced_page(
        &self,
        thread: String,
        before: Option<i64>,
        after: Option<i64>,
        limit: usize,
    ) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            thread_path(&thread)?;
            if before.is_some() && after.is_some() || before.is_some_and(|n| n < 0) || after.is_some_and(|n| n < -1) {
                return Err(INVALID.into());
            }
            let trusted = self.directory().await?;
            let header = self
                .client
                .get(&format!("/v1/threads/{thread}"))
                .await
                .map_err(|e| refusal(&e))?;
            if header["threadId"] != thread {
                return Err(INVALID.into());
            }
            let query = if let Some(n) = before {
                format!("before={n}")
            } else if let Some(n) = after {
                format!("after={n}")
            } else if let Some(n) = header["lastSeq"].as_i64().and_then(|n| n.checked_add(1)) {
                format!("before={n}")
            } else {
                "after=-1".into()
            };
            let page = self
                .client
                .get(&format!(
                    "/v1/threads/{thread}/events?{query}&limit={}",
                    limit.clamp(1, 100)
                ))
                .await
                .map_err(|e| refusal(&e))?;
            let mut keys = std::collections::HashMap::<i32, e2e::ContentKey>::new();
            let mut events = Vec::new();
            for event in page["events"].as_array().ok_or(INVALID)?.iter().take(100) {
                let Some(seq) = event["seq"].as_u64() else { continue };
                let at = event["at"].as_i64().unwrap_or(0);
                let mut opened = json!({"state":"unreadable","seq":seq,"at":at});
                if event["sealed"].is_null() {
                    opened["state"] = json!("local");
                    events.push(opened);
                    continue;
                }
                if let Some(version) = event["keyVersion"]
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .filter(|n| *n > 0)
                {
                    if let std::collections::hash_map::Entry::Vacant(slot) = keys.entry(version) {
                        match self
                            .client
                            .get(&format!(
                                "/v1/threads/{thread}/keys?device={}&version={version}",
                                trusted.me.id
                            ))
                            .await
                        {
                            // A teammate's thread, or one locked to the team key: its team copy.
                            Err(error) if not_found(&error) => match self
                                .team_content_key(
                                    &thread,
                                    version,
                                    header["personId"].as_str().unwrap_or_default(),
                                    &trusted,
                                )
                                .await?
                            {
                                Some(key) => {
                                    slot.insert(key);
                                }
                                None => opened["state"] = json!("no-key"),
                            },
                            Err(error) => return Err(refusal(&error)),
                            Ok(copy) => {
                                let result = (|| {
                                    if copy["threadId"] != thread
                                        || copy["deviceId"] != trusted.me.id
                                        || copy["keyVersion"] != version
                                    {
                                        return Err(INVALID.to_string());
                                    }
                                    let sender = val(&copy, "senderDeviceId")?;
                                    let signer = trusted
                                        .holders
                                        .iter()
                                        .find(|h| h.id == sender)
                                        .and_then(|h| h.keys.signing)
                                        .ok_or(INVALID)?;
                                    let locked = raw(&val(&copy, "lockedKey")?)?;
                                    let epoch = e2e::epoch_of(version).ok_or(INVALID)?;
                                    if !e2e::verify(
                                        &signer,
                                        &e2e::thread_key_message(&thread, epoch, &trusted.me.id, &locked)
                                            .map_err(|_| INVALID)?,
                                        &raw(&val(&copy, "signature")?)?,
                                    ) {
                                        return Err(INVALID.into());
                                    }
                                    self.device
                                        .unlock_content_key(&thread, epoch, &trusted.me.id, &locked)
                                        .map_err(|_| INVALID.into())
                                })();
                                if let Ok(key) = result {
                                    slot.insert(key);
                                }
                            }
                        }
                    }
                    if let Some(key) = keys.get(&version) {
                        let plain = (|| {
                            let bytes = key
                                .open_event(&thread, seq, &raw(&val(event, "sealed")?)?)
                                .map_err(|_| INVALID)?;
                            if bytes.len() > 256 * 1024 {
                                return Err(INVALID.to_string());
                            }
                            serde_json::from_slice::<Value>(&bytes).map_err(|_| INVALID.into())
                        })();
                        if let Ok(p) = plain
                            && p["version"] == 1
                            && let Some(text) = p["event_json"].as_str()
                            && let Ok(body) = serde_json::from_str::<Value>(text)
                            && body["kind"].is_string()
                        {
                            let h = &p["thread"];
                            let origin = h
                                .get("origin")
                                .and_then(|v| serde_json::from_value::<c::Origin>(v.clone()).ok())
                                .unwrap_or(c::Origin::Unknown);
                            opened = json!({"state":"open","seq":seq,"at":p["at"],"thread":{"localId":h["local_id"],"title":h["title"],"projectName":h["project_name"],"projectPath":h["project_path"],"origin":origin},"event":body});
                        }
                    }
                }
                events.push(opened);
            }
            Ok(json!({"thread":header,"events":events,"more":page["more"].as_bool().unwrap_or(false)}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn a_sender_accepts_every_capability_a_runner_signs() {
        assert!(known_capabilities(&caps(&CAPABILITIES)));
        assert!(known_capabilities(&caps(&["projects", "start", "settings"])));
        assert!(!known_capabilities(&caps(&["projects", "projects"])));
        assert!(!known_capabilities(&caps(&["projects", "shell"])));
    }
}
