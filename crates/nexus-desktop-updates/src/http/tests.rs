//! The HTTP feed against a local fake feed. The fixture is the Nexus app's: a file signed with a disposable test key
//! for version 1.0.0.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::{Channel, fetch_channel};

const BYTES: &[u8] = include_bytes!("../../fixtures/update.txt");
const KEY: &str = include_str!("../../fixtures/test.pub");
const SIGNATURE: &str = include_str!("../../fixtures/update.txt.sig");
const LATEST: &str = "/desktop/stable/latest.json";
const FILE: &str = "/desktop/stable/1.0.0/update.txt";

type Routes = Vec<(String, u16, Vec<u8>)>;

/// A fake feed: each path's status and body, built from the server's own address. Counts the requests per path.
struct Server {
    address: String,
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

fn serve(routes: impl FnOnce(&str) -> Routes) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fake feed");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    let routes: HashMap<_, _> = routes(&address)
        .into_iter()
        .map(|(path, status, body)| (path, (status, body)))
        .collect();
    let hits = Arc::new(Mutex::new(HashMap::new()));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else { return };
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("deadline");
            let mut reader = BufReader::new(&mut connection);
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let path = first.split(' ').nth(1).unwrap_or("/").to_string();
            *counted.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
            let (status, body) = routes.get(&path).cloned().unwrap_or((404, Vec::new()));
            let _ = write!(
                connection,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/never\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = connection.write_all(&body);
        }
    });
    Server { address, hits }
}

impl Server {
    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

fn manifest(version: &str, url: &str) -> Vec<u8> {
    serde_json::json!({
        "version": version,
        "platforms": { "test": { "url": url, "signature": SIGNATURE.trim() } }
    })
    .to_string()
    .into_bytes()
}

/// A feed announcing 1.0.0 at `FILE`, which serves `bytes`.
fn release(bytes: &[u8]) -> Server {
    let bytes = bytes.to_vec();
    serve(move |address| {
        vec![
            (LATEST.into(), 200, manifest("1.0.0", &format!("{address}{FILE}"))),
            (FILE.into(), 200, bytes),
        ]
    })
}

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "nexus-update-feed-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn feed(server: &Server, installed: &str, mode: Mode, cache: Option<&Scratch>) -> Feed {
    Feed {
        client: client().expect("client"),
        app: App::Desktop,
        endpoint: format!("{}{LATEST}", server.address),
        target: "test".into(),
        installed: installed.into(),
        pubkey: KEY.into(),
        authorization: None,
        mode,
        cache: cache.map(|scratch| Cache::new(scratch.0.clone())),
        progress: Box::new(|_, _, _, _| {}),
    }
}

async fn check(feed: &Feed) -> Result<Option<Release<Prepared>>, Failure> {
    fetch_channel(feed, &feed.installed, None, Channel::of(&feed.installed)).await
}

#[tokio::test]
async fn a_newer_signed_release_is_downloaded_and_verified() {
    let server = release(BYTES);
    let release = check(&feed(&server, "0.9.0", Mode::Install, None))
        .await
        .expect("checked")
        .expect("a newer release");
    assert_eq!(release.version, "1.0.0");
    assert_eq!(
        release.artifact,
        Prepared::Verified {
            bytes: BYTES.to_vec(),
            url: format!("{}{FILE}", server.address)
        }
    );
}

/// The ready toast (`Snapshot::toast_offer`) from a real check: offered once the release is verified and in the
/// cache, gone after Not now while the same version stays ready, and offered again for the next launch.
#[tokio::test]
async fn a_verified_release_is_offered_in_the_toast_until_not_now() {
    let server = release(BYTES);
    let scratch = Scratch::new();
    let feed = feed(&server, "0.9.0", Mode::Install, Some(&scratch));
    let mut coordinator = crate::Coordinator::of(App::Desktop, "0.9.0".into(), None, 0);
    assert!(coordinator.begin(crate::Trigger::Manual, 0));
    assert_eq!(coordinator.snapshot.toast_offer(), None);
    let found = check(&feed).await.map(|release| release.map(|release| release.version));
    coordinator.finish(found, 0, 0);
    assert_eq!(coordinator.snapshot.toast_offer(), Some("1.0.0"));
    coordinator.defer();
    assert!(coordinator.begin(crate::Trigger::Manual, crate::THROTTLE_MS));
    assert_eq!(coordinator.snapshot.toast_offer(), None);
    let found = fetch_channel(&feed, "0.9.0", Some("1.0.0"), Channel::of("0.9.0"))
        .await
        .map(|release| release.map(|release| release.version));
    coordinator.finish(found, crate::THROTTLE_MS, 0);
    assert_eq!(coordinator.snapshot.ready_version.as_deref(), Some("1.0.0"));
    assert_eq!(coordinator.snapshot.toast_offer(), None);
    // The next launch: the cached, verified release is offered again without another download.
    let mut next = crate::Coordinator::of(App::Desktop, "0.9.0".into(), None, 0);
    assert!(next.begin(crate::Trigger::Launch, 0));
    next.finish(
        check(&feed).await.map(|release| release.map(|release| release.version)),
        0,
        0,
    );
    assert_eq!(next.snapshot.toast_offer(), Some("1.0.0"));
    assert_eq!(server.hits(FILE), 1);
}

#[tokio::test]
async fn a_bad_signature_is_refused() {
    let server = release(b"tampered update");
    let failure = check(&feed(&server, "0.9.0", Mode::Install, None))
        .await
        .expect_err("tampered bytes");
    assert_eq!(failure.code, "verification_failed");
    // A manifest that relabels the signed file as another version is refused too.
    let relabeled = serve(|address| {
        vec![
            (LATEST.into(), 200, manifest("2.0.0", &format!("{address}{FILE}"))),
            (FILE.into(), 200, BYTES.to_vec()),
        ]
    });
    let failure = check(&feed(&relabeled, "0.9.0", Mode::Install, None))
        .await
        .expect_err("relabeled");
    assert_eq!(failure.code, "verification_failed");
}

#[tokio::test]
async fn the_same_or_an_older_version_is_ignored_and_clears_the_cache() {
    let server = release(BYTES);
    let scratch = Scratch::new();
    Cache::new(scratch.0.clone())
        .save(
            &Artifact {
                version: "1.0.0".into(),
                target: "test".into(),
                url: "u".into(),
                signature: "s".into(),
            },
            BYTES,
        )
        .expect("staged");
    for installed in ["1.0.0", "1.0.1"] {
        assert!(
            check(&feed(&server, installed, Mode::Install, Some(&scratch)))
                .await
                .expect("checked")
                .is_none()
        );
    }
    assert_eq!(server.hits(FILE), 0);
    assert!(!scratch.0.join("artifact").exists());
}

#[tokio::test]
async fn a_completed_download_is_reused_after_a_restart() {
    let server = release(BYTES);
    let scratch = Scratch::new();
    for _ in 0..2 {
        let release = check(&feed(&server, "0.9.0", Mode::Install, Some(&scratch)))
            .await
            .expect("checked")
            .expect("newer");
        assert!(matches!(release.artifact, Prepared::Verified { ref bytes, .. } if bytes == BYTES));
    }
    assert_eq!(server.hits(FILE), 1);
}

#[tokio::test]
async fn announce_mode_offers_the_file_without_downloading_it() {
    let server = release(BYTES);
    let release = check(&feed(&server, "0.9.0", Mode::Announce, None))
        .await
        .expect("checked")
        .expect("newer");
    assert_eq!(
        release.artifact,
        Prepared::Announced {
            url: format!("{}{FILE}", server.address)
        }
    );
    assert_eq!(server.hits(FILE), 0);
}

#[tokio::test]
async fn a_file_outside_the_manifests_folder_is_refused() {
    for url in [
        "https://evil.test/desktop/stable/1.0.0/update.txt".to_string(),
        "{a}/desktop-gpui/stable/1.0.0/update.txt".into(),
        "{a}/desktop/stable/../internal/x".into(),
        // Encoded or trailing dot segments: the client resolves them before it fetches.
        "{a}/desktop/stable/%2e%2e/internal/x".into(),
        "{a}/desktop/stable/.%2E/internal/x".into(),
        "{a}/desktop/stable/..".into(),
    ] {
        let server = serve(|address| vec![(LATEST.into(), 200, manifest("1.0.0", &url.replace("{a}", address)))]);
        let failure = check(&feed(&server, "0.9.0", Mode::Install, None))
            .await
            .expect_err(&url);
        assert_eq!(failure.code, "invalid_download_origin", "{url}");
    }
}

#[tokio::test]
async fn a_missing_feed_or_a_redirect_is_not_reported_as_unreachable() {
    for status in [404, 302, 403, 503] {
        let server = serve(|_| vec![(LATEST.into(), status, Vec::new())]);
        let failure = check(&feed(&server, "0.9.0", Mode::Install, None))
            .await
            .expect_err("no release");
        assert_eq!(failure.code, "release_not_found", "{status}");
        assert!(!failure.retryable);
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("port");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    drop(listener);
    let closed = Server {
        address,
        hits: Arc::default(),
    };
    let failure = check(&feed(&closed, "0.9.0", Mode::Install, None))
        .await
        .expect_err("closed port");
    assert_eq!(failure.code, "network");
    assert!(failure.retryable);
}

#[tokio::test]
async fn a_platform_without_a_signed_file_is_not_offered() {
    let server = serve(|_| vec![(LATEST.into(), 200, br#"{"version":"1.0.0","platforms":{}}"#.to_vec())]);
    assert!(
        check(&feed(&server, "0.9.0", Mode::Install, None))
            .await
            .expect("checked")
            .is_none()
    );
}
