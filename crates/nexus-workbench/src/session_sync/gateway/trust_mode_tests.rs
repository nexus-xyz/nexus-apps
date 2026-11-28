//! The runner follows the device list's `trust`: Gateway's own trust skips approvals and the approver's code. A
//! computer that relied on approvals follows it once the person opts in; one that relied on nothing follows it at once.
use super::*;
use crate::session_sync::keystore::KeyStore;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use nexus_client::{Client, ClientOptions};
use std::convert::Infallible;
use std::sync::Mutex;

/// A Gateway that answers who the person is and lists `devices`.
struct Listing {
    devices: Mutex<Value>,
}

impl Listing {
    fn answer(&self, request: &Request<Incoming>) -> Response<Full<Bytes>> {
        let (status, answer) = match (request.method().as_str(), request.uri().path()) {
            ("GET", "/v1/auth/whoami") => (200, json!({ "kind": "person", "userId": "ada", "tenantId": "acme" })),
            ("GET", "/v1/devices") => (200, self.devices.lock().expect("devices").clone()),
            _ => (404, json!({ "code": "not_found" })),
        };
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(answer.to_string())))
            .expect("response")
    }
}

struct Fixture {
    backend: GatewayBackend,
    listing: Arc<Listing>,
    root: std::path::PathBuf,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    async fn new(device: DeviceKey, devices: Value) -> Fixture {
        let listing = Arc::new(Listing {
            devices: Mutex::new(devices),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let serving = listing.clone();
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let serving = serving.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let answer = serving.answer(&request);
                        async move { Ok::<_, Infallible>(answer) }
                    });
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        eprintln!("test gateway: {error}");
                    }
                });
            }
        });
        let root = std::env::temp_dir().join(format!("nexus-device-trust-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("folder");
        let keys = Arc::new(KeyStore::open(&root.join("keys.sqlite3")).expect("keys"));
        let backend = GatewayBackend::connect(
            Client::new(ClientOptions::new(&url)),
            "scope".into(),
            device,
            keys,
            true,
        )
        .await
        .expect("backend");
        Fixture {
            backend,
            listing,
            root,
            server,
        }
    }

    fn list(&self, devices: Value) {
        *self.listing.devices.lock().expect("devices") = devices;
    }
}

/// A device row; `approver` signs it as Gateway lists an approval.
fn row(id: &str, key: &DeviceKey, approver: Option<(&str, &DeviceKey)>) -> Value {
    let signature = approver.map(|(_, by)| {
        let message = e2e::approval_message(id, &key.encryption_key(), &key.signing_key()).expect("message");
        e2e::b64u(&by.sign(&message))
    });
    json!({
        "id": id, "kind": "device", "state": "trusted",
        "encryptionKey": e2e::b64u(&key.encryption_key()), "signingKey": e2e::b64u(&key.signing_key()),
        "approvedBy": approver.map(|(by, _)| by), "approvalSignature": signature,
        "approvedAt": "2026-11-26T00:00:00Z",
    })
}

fn ids(trusted: &Trusted) -> Vec<&str> {
    trusted.holders.iter().map(|holder| holder.id.as_str()).collect()
}

/// The device lists of one person: under approvals (the laptop approved by the phone), and vouched for by Gateway.
fn lists(laptop: &DeviceKey, phone: &DeviceKey, desktop: &DeviceKey) -> (Value, Value) {
    let approvals = json!({ "devices": [
        row("dev_phone", phone, None),
        row("dev_laptop", laptop, Some(("dev_phone", phone))),
        row("dev_desktop", desktop, None),
    ]});
    // Gateway vouches for the person's devices: no approval and no code.
    let vouched = json!({ "trust": "gateway", "devices": [
        row("dev_phone", phone, None),
        row("dev_laptop", laptop, None),
        row("dev_desktop", desktop, None),
        { "id": "dev_code", "kind": "recovery_code", "state": "trusted", "signingKey": null,
          "encryptionKey": e2e::b64u(&DeviceKey::generate().encryption_key()),
          "approvedBy": null, "approvalSignature": null },
    ]});
    (approvals, vouched)
}

#[tokio::test]
async fn a_runner_that_only_waited_under_approvals_follows_gateway_s_list_with_no_opt_in() {
    // Under approvals the laptop never confirmed the phone's code, so it locked nothing and relied on no approval.
    let (laptop, phone, desktop) = (DeviceKey::generate(), DeviceKey::generate(), DeviceKey::generate());
    let (approvals, vouched) = lists(&laptop, &phone, &desktop);
    let fixture = Fixture::new(laptop, approvals).await;
    assert_eq!(
        fixture.backend.directory().await.expect_err("waits for the code"),
        trust::APPROVER_UNCONFIRMED
    );
    fixture.list(vouched);
    let trusted = fixture.backend.directory().await.expect("vouched for");
    assert_eq!(ids(&trusted), ["dev_laptop", "dev_phone", "dev_desktop"]);
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Gateway);
}

#[tokio::test]
async fn the_runner_trusts_gateway_s_list_only_after_the_person_opts_in_and_while_it_names_gateway_trust() {
    let (laptop, phone, desktop) = (DeviceKey::generate(), DeviceKey::generate(), DeviceKey::generate());
    let (approvals, vouched) = lists(&laptop, &phone, &desktop);
    let phone_holder = trust::Holder {
        id: "dev_phone".into(),
        keys: trust::Keys {
            encryption: phone.encryption_key(),
            signing: Some(phone.signing_key()),
        },
    };
    let fixture = Fixture::new(laptop, approvals.clone()).await;
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Approvals);
    // Under approvals the person confirmed the phone's code on this laptop: it relied on approvals.
    fixture
        .backend
        .keys
        .confirm_approver(&fixture.backend.owner, "dev_laptop", &phone_holder)
        .expect("confirmed");
    let confirmed = fixture.backend.directory().await.expect("approvals");
    assert_eq!(ids(&confirmed), ["dev_laptop", "dev_phone"]);
    fixture.list(vouched.clone());
    // Gateway's answer alone does not move this computer off approvals: the list has no approvals, so the laptop
    // trusts only what it verified before.
    let alone = fixture.backend.directory().await.expect("approvals");
    assert_eq!(ids(&alone), ["dev_laptop", "dev_phone"]);
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Approvals);
    // The person opts in on this computer.
    fixture
        .backend
        .keys
        .trust_gateway(&fixture.backend.owner)
        .expect("opted in");
    let trusted = fixture.backend.directory().await.expect("vouched for");
    assert_eq!(ids(&trusted), ["dev_laptop", "dev_phone", "dev_desktop"]);
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Gateway);
    // Back to approvals: nothing Gateway vouched for stays trusted, only what the laptop verified itself.
    fixture.list(approvals);
    let back = fixture.backend.directory().await.expect("approvals");
    assert_eq!(ids(&back), ["dev_laptop", "dev_phone"]);
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Approvals);
    // Gateway's trust again needs a new opt-in: the desktop Gateway lists does not become a holder.
    fixture.list(vouched);
    let again = fixture.backend.directory().await.expect("approvals");
    assert_eq!(ids(&again), ["dev_laptop", "dev_phone"]);
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Approvals);
}

#[tokio::test]
async fn the_trust_is_known_once_the_backend_connects() {
    let laptop = DeviceKey::generate();
    let devices = json!({ "trust": "gateway", "devices": [row("dev_laptop", &laptop, None)] });
    let fixture = Fixture::new(laptop, devices).await;
    assert_eq!(fixture.backend.device_trust(), DeviceTrust::Gateway);
}
