//! Byte and trust validation shared by live transport and cross-runtime vectors.
use super::*;
pub(super) fn open_request(
    e: Envelope,
    expected_hash: &str,
    scope: &c::Scope,
    computer: &str,
    trusted: &Trusted,
    device: &DeviceKey,
    now: i64,
) -> Result<c::Request> {
    if e.version != 1
        || e.scope != *scope
        || e.computer_id != computer
        || e.runner_device_id != trusted.me.id
        || thread_path(&e.id).is_err()
        || e.expires_at <= now
        || e.expires_at
            .checked_sub(e.issued_at)
            .is_none_or(|ttl| ttl <= 0 || ttl > 60_000)
        || e.issued_at < 0
        || e.generation < 1
        || e.generation > 9_007_199_254_740_991
        || e.issued_at > now + 5_000
        || e.sealed.len() > 90_000
        || e.locked_key.len() > 512
    {
        return Err(INVALID.into());
    }
    let Some(sender) = trusted.holders.iter().find(|d| d.id == e.sender_device_id) else {
        return Err(INVALID.into());
    };
    let Some(signing) = sender.keys.signing else {
        return Err(INVALID.into());
    };
    let Ok(signature) = raw(&e.signature) else {
        return Err(INVALID.into());
    };
    if !e2e::verify(&signing, &e.message(), &signature) {
        return Err(INVALID.into());
    }
    let request_hash = hash(format!("{}{}", e.message(), part(&e.signature)).as_bytes());
    if expected_hash != request_hash {
        return Err(INVALID.into());
    }
    let opened: Result<Payload> = (|| {
        let key = device
            .unlock_content_key(&e.id, 0, &trusted.me.id, &raw(&e.locked_key)?)
            .map_err(|_| INVALID)?;
        let plaintext = key.open_event(&e.id, 0, &raw(&e.sealed)?).map_err(|_| INVALID)?;
        if plaintext.len() > 64 * 1024 {
            return Err(INVALID.into());
        }
        serde_json::from_slice::<Payload>(&plaintext).map_err(|_| INVALID.into())
    })();
    let Ok(p) = opened else { return Err(INVALID.into()) };
    if p.version != 1
        || p.kind != "computerRequest"
        || p.scope != e.scope
        || p.computer_id != e.computer_id
        || p.id != e.id
        || p.sender_device_id != e.sender_device_id
        || p.runner_device_id != e.runner_device_id
        || p.generation != e.generation
        || p.issued_at != e.issued_at
        || p.expires_at != e.expires_at
        || !p.operation.valid()
        || p.expires_at - p.issued_at > p.operation.admission_ms()
    {
        return Err(INVALID.into());
    }
    Ok(c::Request {
        target: c::RequestTarget {
            id: e.id,
            hash: request_hash,
            sender: e.sender_device_id,
            computer_id: e.computer_id,
            generation: e.generation,
            expires_at: e.expires_at,
            creation_hash: None,
        },
        operation: p.operation,
    })
}
pub(super) fn open_pairing(
    e: PairEnvelope,
    i: &c::InvitationRecord,
    me: &str,
    listed: &[DirectoryDevice],
    device: &DeviceKey,
    now: i64,
) -> Result<c::Pairing> {
    if e.version != 1
        || e.scope != i.scope
        || e.invite_id != i.invite_id
        || e.inviter_device_id != me
        || e.expires_at <= now
        || e.expires_at > i.expires_at
        || (i.signed_decisions && e.expires_at != i.expires_at)
        || thread_path(&e.id).is_err()
        || e.sealed.len() > 12_000
        || e.locked_key.len() > 512
    {
        return Err(INVALID.into());
    }
    let Some(sender) = listed
        .iter()
        .find(|d| d.id == e.sender_device_id && d.state != "revoked")
    else {
        return Err(INVALID.into());
    };
    let Some(signing) = sender
        .signing_key
        .as_deref()
        .and_then(|s| raw(s).ok())
        .and_then(|s| s.try_into().ok())
    else {
        return Err(INVALID.into());
    };
    if !raw(&e.signature).is_ok_and(|sig| e2e::verify(&signing, &e.message(), &sig)) {
        return Err(INVALID.into());
    }
    let opened: Result<PairPayload> = (|| {
        let key = device
            .unlock_content_key(&e.id, 0, me, &raw(&e.locked_key)?)
            .map_err(|_| INVALID)?;
        let plaintext = key.open_event(&e.id, 0, &raw(&e.sealed)?).map_err(|_| INVALID)?;
        if plaintext.len() > 8 * 1024 {
            return Err(INVALID.into());
        }
        serde_json::from_slice::<PairPayload>(&plaintext).map_err(|_| INVALID.into())
    })();
    let Ok(p) = opened else { return Err(INVALID.into()) };
    if p.version != 1
        || p.kind != "phonePairing"
        || p.scope != i.scope
        || p.id != e.id
        || p.invite_id != i.invite_id
        || p.computer_id != i.computer_id
        || p.inviter_device_id != me
        || p.sender_device_id != e.sender_device_id
        || p.expires_at != e.expires_at
        || p.encryption_key != sender.encryption_key
        || Some(p.signing_key.as_str()) != sender.signing_key.as_deref()
        || !raw(&p.secret).is_ok_and(|s| s.len() == 32 && hash(&s) == i.secret_hash)
    {
        return Err(INVALID.into());
    }
    Ok(c::Pairing {
        id: e.id,
        sender: sender.id.clone(),
        name: sender.name.clone(),
        encryption_key: p.encryption_key,
        signing_key: p.signing_key,
        secret_hash: i.secret_hash.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typescript_computer_request_and_pairing_open_with_native_crypto() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../../tests/fixtures/computer-v1.json")).expect("fixture");
        let secret: [u8; 64] = raw(fixture["runnerSecret"].as_str().expect("public test seed"))
            .expect("decode")
            .try_into()
            .expect("64 bytes");
        let device = DeviceKey::from_secret_bytes(&secret);
        let listed: Vec<Listed> = serde_json::from_value(fixture["devices"].clone()).expect("directory");
        let me = Keys {
            encryption: device.encryption_key(),
            signing: Some(device.signing_key()),
        };
        let trusted =
            trust::verify(&listed, &me, &mut trust::confirmed(&listed, &me)).expect("anchored approval chain");
        let scope: c::Scope = serde_json::from_value(fixture["scope"].clone()).expect("scope");
        let now = fixture["now"].as_i64().expect("time");
        let envelope: Envelope = serde_json::from_value(fixture["request"].clone()).expect("request");
        assert_eq!(envelope.message(), fixture["messages"]["request"]);
        assert_eq!(
            hash(format!("{}{}", envelope.message(), part(&envelope.signature)).as_bytes()),
            fixture["requestHash"]
        );
        let computer = envelope.computer_id.clone();
        let expected = fixture["requestHash"].as_str().expect("hash");
        let opened = open_request(envelope.clone(), expected, &scope, &computer, &trusted, &device, now)
            .expect("TypeScript encrypted request opens");
        assert!(
            matches!(opened.operation,c::Operation::StartSession{project_id,text,..}if project_id=="p_public_fixture"&&text=="Reply PHONE_START_READY. Do not use tools.")
        );
        assert!(
            open_request(
                envelope.clone(),
                expected,
                &scope,
                &computer,
                &trusted,
                &device,
                envelope.expires_at
            )
            .is_err()
        );
        let mut wrong = scope.clone();
        wrong.tenant_id = "another-org".into();
        assert!(open_request(envelope.clone(), expected, &wrong, &computer, &trusted, &device, now).is_err());
        let mut changed = envelope.clone();
        changed
            .signature
            .replace_range(0..1, if changed.signature.starts_with('A') { "B" } else { "A" });
        assert!(open_request(changed, expected, &scope, &computer, &trusted, &device, now).is_err());
        let invitation: c::Invitation = serde_json::from_value(fixture["invitation"].clone()).expect("invitation");
        let commitment = hash(&raw(&invitation.secret).expect("secret"));
        let m = message(
            "nexus-phone-invitation-v1",
            &scope,
            &[
                invitation.invite_id.clone(),
                invitation.computer_id.clone(),
                invitation.computer_name.clone(),
                invitation.inviter_device_id.clone(),
                invitation.encryption_key.clone(),
                invitation.signing_key.clone(),
                invitation.expires_at.to_string(),
                commitment.clone(),
            ],
        );
        assert_eq!(m, fixture["messages"]["invitation"]);
        assert!(e2e::verify(
            &device.signing_key(),
            &m,
            &raw(&invitation.signature).expect("signature")
        ));
        let record = c::InvitationRecord {
            signed_decisions: true,
            scope: scope.clone(),
            invite_id: invitation.invite_id,
            computer_id: computer,
            inviter: trusted.me.id.clone(),
            expires_at: invitation.expires_at,
            secret_hash: commitment,
            state: "waiting".into(),
            accepted: None,
            accepted_encryption: None,
            accepted_signing: None,
            accepted_request: None,
            accepted_name: None,
        };
        let pair: PairEnvelope = serde_json::from_value(fixture["pairing"].clone()).expect("pair envelope");
        assert_eq!(pair.message(), fixture["messages"]["pairing"]);
        let directory: Vec<DirectoryDevice> = serde_json::from_value(fixture["devices"].clone()).expect("directory");
        let joined =
            open_pairing(pair, &record, &trusted.me.id, &directory, &device, now).expect("TypeScript scan proof opens");
        assert_eq!(joined.sender, "dev_phone");
        assert_eq!(joined.secret_hash, record.secret_hash);
        let mut wrong = record.clone();
        wrong.secret_hash = "another-QR".into();
        assert!(
            open_pairing(
                serde_json::from_value(fixture["pairing"].clone()).expect("pair"),
                &wrong,
                &trusted.me.id,
                &directory,
                &device,
                now
            )
            .is_err()
        );
    }
}
