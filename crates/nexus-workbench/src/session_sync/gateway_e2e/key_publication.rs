use super::*;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use nexus_client::Error as ClientError;
use std::collections::BTreeSet;
use std::convert::Infallible;

#[tokio::test]
async fn a_copy_received_without_acknowledgement_remains_an_exposure_before_the_next_write() {
    let Some(gateway) = boot().await else { return };
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({"name":"Uncertain key copy","email":"uncertain-copy@test.example"}),
        )
        .await
        .expect("signup");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    client
        .put(
            "/v1/orgs/settings",
            json!({"sessionContent":"user","multiplayerSessions":false}),
        )
        .await
        .expect("personal policy");
    let device = DeviceKey::generate();
    let id = register(&client, &device, "Runner").await;
    let fail = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy");
    let proxy_url = format!("http://{}", listener.local_addr().expect("address"));
    let serving = client.clone();
    let refusing = fail.clone();
    let proxy = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let upstream = serving.clone();
            let refusing = refusing.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let upstream = upstream.clone();
                    let refusing = refusing.clone();
                    async move {
                        let method = request.method().to_string();
                        let path = request.uri().path_and_query().expect("path").as_str().to_string();
                        let body = request.into_body().collect().await.expect("body").to_bytes();
                        let result = match method.as_str() {
                            "GET" => upstream.get(&path).await,
                            "PUT" => upstream.put(&path, serde_json::from_slice(&body).expect("JSON")).await,
                            "POST" => upstream.post(&path, serde_json::from_slice(&body).expect("JSON")).await,
                            _ => panic!("unexpected method"),
                        };
                        let (status, answer) = match result {
                            Ok(_) if method == "PUT" && path.ends_with("/keys") && refusing.load(Ordering::SeqCst) => (
                                500,
                                json!({"error":{"message":"copy arrived but acknowledgement was lost"}}),
                            ),
                            Ok(answer) => (200, answer),
                            Err(ClientError::Core(error)) => (
                                error.status,
                                json!({"error":{"code":error.code,"message":error.message}}),
                            ),
                            Err(error) => panic!("upstream: {error}"),
                        };
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(answer.to_string())))
                                .expect("response"),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    let keys = Arc::new(KeyStore::in_memory().expect("keys"));
    let backend = GatewayBackend::connect(
        person(&proxy_url, signup["apiKey"].as_str().expect("key")),
        "uncertain".into(),
        DeviceKey::from_secret_bytes(&device.secret_bytes()),
        keys.clone(),
        true,
    )
    .await
    .expect("runner");
    let thread = uuid::Uuid::new_v4().to_string();
    let initial = backend
        .key(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("initial publication");
    let other_key = DeviceKey::generate();
    let other = register(&client, &other_key, "Later revoked").await;
    approve(&client, (&id, &device), &other, &other_key).await;
    fail.store(true, Ordering::SeqCst);
    assert!(
        backend
            .key(&thread, Some(initial.epoch()), ContentPolicy::UserOnly)
            .await
            .is_err()
    );
    assert_eq!(
        keys.latest(&thread).expect("read").expect("old metadata").holders,
        BTreeSet::from([id.clone()])
    );
    fail.store(false, Ordering::SeqCst);
    revoke(&client, (&id, &device), &other).await;
    let next = backend
        .key(&thread, Some(initial.epoch()), ContentPolicy::UserOnly)
        .await
        .expect("write after revocation");
    assert!(
        next.epoch() > initial.epoch(),
        "an unacknowledged recipient still knows the previous epoch"
    );
    proxy.abort();
}
