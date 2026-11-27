//! Who may read the internal channel: a Nexus session on Nexus's own Gateway whose account the Gateway verifies as an
//! employee (`GET /v1/desktop/download-access`). Nexus GPUI uses it.
//! Tokens stay in native memory, and only reach downloads.nexus.inc's app folders.

use reqwest::header::HeaderValue;
use std::time::Duration;

use crate::{App, Channel, Failure};

const ACCESS: &str = "https://api.nexus.inc/v1/desktop/download-access";

pub const SIGN_IN: &str = "Sign in to Nexus with your @nexus.xyz account to receive internal updates.";

/// Names the Gateway's host only, so a URL that carries credentials never reaches the screen.
pub fn other_gateway(gateway: &str) -> String {
    let host = reqwest::Url::parse(gateway).ok().and_then(|url| {
        let host = url.host_str()?.to_string();
        Some(url.port().map_or(host.clone(), |port| format!("{host}:{port}")))
    });
    format!(
        "Internal updates need a Nexus account on api.nexus.inc. You are signed in to {}.",
        host.as_deref().unwrap_or("another Gateway")
    )
}

/// Never send a Nexus credential to a manifest-selected host or a development server.
pub fn employee_url(url: &reqwest::Url) -> bool {
    employee_url_of(App::Desktop, url)
}

/// [`employee_url`] for `app`'s folder.
pub fn employee_url_of(app: App, url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("downloads.nexus.inc")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .path()
            .strip_prefix('/')
            .and_then(|path| path.strip_prefix(app.folder()))
            .is_some_and(|rest| rest.starts_with('/'))
        && url.query().is_none()
        && url.fragment().is_none()
}

fn header(token: &str) -> Result<HeaderValue, String> {
    if token.is_empty()
        || token.len() > 8000
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~+/=-".contains(&byte))
    {
        return Err(SIGN_IN.into());
    }
    // A distinct scheme prevents GCS from interpreting the app token as a Google credential.
    let mut header = HeaderValue::from_str(&format!("Nexus {token}")).map_err(|_| SIGN_IN.to_string())?;
    header.set_sensitive(true);
    Ok(header)
}

/// Whether `gateway` is one of Nexus's own Gateways, where the employee check runs.
pub fn nexus_gateway(gateway: &str) -> bool {
    matches!(
        gateway.trim_end_matches('/'),
        "https://api.nexus.inc" | "https://api.nexus.xyz"
    )
}

/// The download service's credential for the session on `gateway` with `token`.
pub fn authorization(gateway: &str, token: Option<&str>) -> Result<HeaderValue, String> {
    // Self-hosted or development sessions must never leave their Gateway for Nexus's download service.
    if !nexus_gateway(gateway) {
        return Err(other_gateway(gateway));
    }
    header(token.ok_or(SIGN_IN)?)
}

/// Where the employee check goes, and how long it may take. Tests point it at a fake endpoint (`held_access.rs`).
pub(crate) struct Check<'a> {
    pub endpoint: &'a str,
    pub wait: Duration,
}

/// The live check: Gateway's endpoint, eight seconds.
pub(crate) const LIVE: Check<'static> = Check {
    endpoint: ACCESS,
    wait: Duration::from_secs(8),
};

/// Check access before offering cached bytes too. The edge independently checks every network download.
pub async fn authorize(gateway: &str, token: Option<&str>) -> Result<HeaderValue, Failure> {
    authorize_with(&LIVE, gateway, token).await
}

pub(crate) async fn authorize_with(
    check: &Check<'_>,
    gateway: &str,
    token: Option<&str>,
) -> Result<HeaderValue, Failure> {
    let header =
        authorization(gateway, token).map_err(|message| Failure::new("employee_sign_in_required", message, false))?;
    crate::http::install_tls();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(check.wait)
        .build()
        .map_err(|_| unavailable())?;
    // Match the edge's authority, even while an old hostname is migrating.
    // A token valid only on a legacy backend must not select an unusable internal feed.
    verify(&client, check.endpoint, token.unwrap_or_default()).await?;
    Ok(header)
}

/// Public updates remain available when no eligible app account is signed in. Internal builds stay internal.
pub async fn update_channel(
    gateway: &str,
    token: Option<&str>,
    installed: &str,
) -> Result<(Channel, Option<HeaderValue>), Failure> {
    match authorize(gateway, token).await {
        Ok(header) => Ok((Channel::Internal, Some(header))),
        Err(error) => Ok((crate::unverified_channel(installed, error)?, None)),
    }
}

/// The app's sign-in now, for a download: its Gateway and its token.
pub type Session = std::sync::Arc<dyn Fn() -> (String, Option<String>) + Send + Sync>;

/// A download of a URL on this computer, refusing a body above the given size: what nexus-remote's installer takes
/// (its `install::Fetch`).
pub type Fetch = std::sync::Arc<
    dyn Fn(String, u64) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send>> + Send + Sync,
>;

/// Whether downloading the host from this URL uses a signed-in account.
pub fn host_download_requires_account(url: &reqwest::Url) -> bool {
    employee_url(url) && url.path().starts_with("/desktop/internal/")
}

/// Downloads for a server's Nexus host (the manifest, and a host the server could not fetch). Employee credentials
/// stay on the download origin: only an internal build's URL gets the session's credential, and the server receives
/// only the verified binary. No redirects; ten minutes at most. The body is read as it arrives and stops at the size
/// the installer gives, so a body that never ends cannot fill this computer's memory.
pub fn host_fetch(session: Session) -> Fetch {
    crate::http::install_tls();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10 * 60))
        .build()
        .map_err(|_| "Could not create the download client.".to_string());
    std::sync::Arc::new(move |url: String, max: u64| {
        let http = http.clone();
        let session = session.clone();
        Box::pin(async move {
            let http = http?;
            let parsed = reqwest::Url::parse(&url).map_err(|_| "Invalid download URL.".to_string())?;
            let mut request = http.get(parsed.clone());
            if host_download_requires_account(&parsed) {
                let (gateway, token) = session();
                request = request.header(
                    reqwest::header::AUTHORIZATION,
                    authorize(&gateway, token.as_deref())
                        .await
                        .map_err(|failure| failure.message)?,
                );
            }
            let failed = |e: reqwest::Error| format!("Could not download {url}: {e}");
            let too_large = || format!("{url} is larger than the {max} bytes it may hold.");
            let mut response = request
                .send()
                .await
                .and_then(|response| response.error_for_status())
                .map_err(failed)?;
            // error_for_status lets a redirect through, and its body is not the file.
            if response.status().is_redirection() {
                return Err(format!("Could not download {url}: the server sent a redirect."));
            }
            if response.content_length().is_some_and(|size| size > max) {
                return Err(too_large());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(failed)? {
                if bytes.len() as u64 + chunk.len() as u64 > max {
                    return Err(too_large());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
    })
}

fn unavailable() -> Failure {
    Failure::new(
        "employee_verification_unavailable",
        "Cannot verify update access. Nexus will try again when the service is available.",
        true,
    )
}

async fn verify(client: &reqwest::Client, endpoint: &str, token: &str) -> Result<(), Failure> {
    let response = client
        .get(endpoint)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| unavailable())?;
    match response.status().as_u16() {
        401 => return Err(Failure::new("employee_sign_in_required", SIGN_IN, false)),
        403 => {
            return Err(Failure::new(
                "employee_access_required",
                "Internal updates are available only to verified @nexus.xyz accounts.",
                false,
            ));
        }
        200 => {}
        _ => return Err(unavailable()),
    }
    let body = response.text().await.map_err(|_| unavailable())?;
    let body: serde_json::Value = serde_json::from_str(&body).map_err(|_| unavailable())?;
    if body.get("allowed").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(Failure::new(
            "employee_access_required",
            "This account cannot receive internal updates.",
            false,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn verification_refuses_redirects_denials_outages_and_malformed_success() {
        use std::io::{BufRead, BufReader, Write};
        for (status, body, code) in [
            (200, r#"{"allowed":true}"#, None),
            (200, r#"{"allowed":false}"#, Some("employee_access_required")),
            (200, "{}", Some("employee_access_required")),
            (200, "invalid", Some("employee_verification_unavailable")),
            (401, "{}", Some("employee_sign_in_required")),
            (403, "{}", Some("employee_access_required")),
            (503, "{}", Some("employee_verification_unavailable")),
            (302, "{}", Some("employee_verification_unavailable")),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("fake gateway");
            let address = listener.local_addr().expect("address");
            let server = std::thread::spawn(move || {
                let (mut connection, _) = listener.accept().expect("request");
                connection
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("deadline");
                let mut reader = BufReader::new(&mut connection);
                let mut authenticated = false;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).expect("header") == 0 || line == "\r\n" {
                        break;
                    }
                    authenticated |= line
                        .trim()
                        .eq_ignore_ascii_case("authorization: Bearer test_device_token");
                }
                assert!(authenticated);
                write!(connection, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1:1/never\r\nConnection: close\r\n\r\n{body}", body.len()).expect("response");
            });
            crate::http::install_tls();
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .expect("client");
            let result = verify(
                &client,
                &format!("http://{address}/v1/desktop/download-access"),
                "test_device_token",
            )
            .await;
            assert_eq!(result.as_ref().err().map(|error| error.code), code);
            assert!(!format!("{result:?}").contains("test_device_token"));
            server.join().expect("fake gateway");
        }
    }

    #[test]
    fn pins_credentials_to_employee_download_origins() {
        assert!(employee_url(
            &"https://downloads.nexus.inc/desktop/internal/latest.json"
                .parse()
                .expect("url")
        ));
        // The retired `desktop-gpui/` folder of the 0.1 builds is not the app's folder: no credential goes there.
        assert!(!employee_url(
            &"https://downloads.nexus.inc/desktop-gpui/internal/latest.json"
                .parse()
                .expect("url")
        ));
        for url in [
            // The retired host still serves installed apps, but new builds never send it a credential.
            "https://downloads.nexus.xyz/desktop/internal/latest.json",
            "http://downloads.nexus.inc/desktop/internal/latest.json",
            "https://downloads.nexus.inc.evil.test/desktop/internal/latest.json",
            "https://downloads.nexus.inc@evil.test/desktop/internal/latest.json",
            "https://name@downloads.nexus.inc/desktop/internal/latest.json",
            "https://downloads.nexus.inc:8443/desktop/internal/latest.json",
            "https://downloads.nexus.inc/cli/latest.json",
            "https://downloads.nexus.inc/desktop/internal/latest.json?token=secret",
            "https://downloads.nexus.inc/desktop/internal/latest.json#fragment",
        ] {
            assert!(!employee_url(&url.parse().expect("url")), "{url}");
        }
    }

    #[test]
    fn rejects_malformed_credentials_without_echoing_them() {
        for token in ["", "secret\r\nx-leak: secret", "bad secret", "nxc_é"] {
            assert_eq!(header(token).expect_err("invalid token"), SIGN_IN);
        }
        let header = header("nxc_fake-token").expect("token");
        assert_eq!(header, "Nexus nxc_fake-token");
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains("nxc_"));
    }

    #[test]
    fn uses_only_the_current_production_session() {
        for gateway in ["https://api.nexus.inc", "https://api.nexus.xyz"] {
            assert_eq!(
                authorization(gateway, Some("nxc_employee")).expect("signed in"),
                "Nexus nxc_employee"
            );
            assert_eq!(authorization(gateway, None).expect_err("signed out"), SIGN_IN);
        }
        for untrusted in [
            "http://api.nexus.inc",
            "https://api.nexus.inc.evil.test",
            "http://localhost:8080",
            "https://self-hosted.test",
        ] {
            let message = authorization(untrusted, Some("nxc_private-development-key")).expect_err("private gateway");
            assert!(message.starts_with("Internal updates need a Nexus account on api.nexus.inc."));
        }
    }

    /// A server on this computer that sends `head`, then `chunk` until the client hangs up.
    async fn serve(head: &'static str, chunk: &'static [u8], chunks: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let address = listener.local_addr().expect("its address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("the client");
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(head.as_bytes()).await;
            for _ in 0..chunks {
                if socket.write_all(chunk).await.is_err() {
                    return;
                }
            }
        });
        format!("http://{address}/file")
    }

    async fn download(url: &str, max: u64) -> Result<Vec<u8>, String> {
        let fetch = host_fetch(std::sync::Arc::new(|| (String::new(), None)));
        fetch(url.to_string(), max).await
    }

    const NO_SIZE: &str = "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
    const TOO_BIG: &str = "HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n";
    const ONE_KB: &[u8] = &[b'x'; 1024];
    /// Past any limit here, and it never comes: the client must stop first.
    const ENDLESS: usize = usize::MAX;

    #[tokio::test]
    async fn a_body_with_no_size_that_never_ends_stops_at_the_limit() {
        let url = serve(NO_SIZE, ONE_KB, ENDLESS).await;
        let refused = download(&url, 64 * 1024).await;
        assert!(refused.is_err_and(|e| e.contains("larger")));
    }

    #[tokio::test]
    async fn an_announced_size_above_the_limit_reads_nothing() {
        let url = serve(TOO_BIG, ONE_KB, 0).await;
        let refused = download(&url, 1024 * 1024).await;
        assert!(refused.is_err_and(|e| e.contains("larger")));
    }

    #[tokio::test]
    async fn a_body_at_the_limit_comes_back_whole() {
        let url = serve(NO_SIZE, ONE_KB, 4).await;
        let bytes = download(&url, 4096).await.expect("the body");
        assert_eq!(bytes.len(), 4096);
    }

    #[tokio::test]
    async fn a_redirect_is_refused_and_never_followed() {
        const REDIRECT: &str =
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/elsewhere\r\nContent-Length: 0\r\n\r\n";
        let url = serve(REDIRECT, ONE_KB, 0).await;
        let refused = download(&url, 1024).await;
        assert!(refused.is_err_and(|e| e.contains("redirect")));
    }
}
