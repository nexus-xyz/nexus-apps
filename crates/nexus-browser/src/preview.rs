//! Local preview checks. No redirects, proxy, DNS lookup, cookies or Gateway credentials reach a dev server.
//! Websites load directly in the isolated native browser, without a separate HTTP probe.
use serde::Serialize;
use std::time::Duration;
use url::Url;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub url: String,
    pub reachable: bool,
    pub frame_error: Option<String>,
    pub error: Option<String>,
}

pub fn local_url(text: &str) -> Result<Url, String> {
    let mut url = Url::parse(text).map_err(|_| "Invalid preview address.")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0" | "[::]")
        )
    {
        return Err("Only addresses on this computer open here.".into());
    }
    if url.host_str() != Some("127.0.0.1") && !matches!(url.host_str(), Some("[::1]" | "[::]")) {
        url.set_host(Some("127.0.0.1")).map_err(|_| "Invalid preview host.")?;
    }
    if url.host_str() == Some("[::]") {
        url.set_host(Some("[::1]")).map_err(|_| "Invalid preview host.")?;
    }
    Ok(url)
}

/// The host every Nexus page is served from, one label per page (`NEXUS_PAGES_ORIGIN`).
const PAGES_HOST: &str = ".nexususercontent.dev";

/// A Nexus page's signed frame (`GET /v1/pages/{id}/versions/{version}/frame`): https on the Pages sandbox host, no
/// credentials. It holds the page alone, apart from Nexus UI and Gateway, so it opens like a local server's page.
pub fn page_frame_url(text: &str) -> Result<Url, String> {
    let url = Url::parse(text).map_err(|_| "Invalid preview address.")?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let label = host.strip_suffix(PAGES_HOST).unwrap_or_default();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || label.is_empty()
        || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Only addresses on this computer open here.".into());
    }
    Ok(url)
}

/// Native navigation accepts web URLs. Remote pages never receive Nexus commands or Gateway credentials.
pub fn browser_url(text: &str) -> Result<Url, String> {
    let url = Url::parse(text).map_err(|_| "Enter an HTTP or HTTPS address.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Enter an HTTP or HTTPS address without a username or password.".into());
    }
    Ok(url)
}

pub async fn probe(text: String) -> Probe {
    let mut out = Probe {
        url: text.clone(),
        reachable: false,
        frame_error: None,
        error: None,
    };
    if let Err(error) = browser_url(&text) {
        out.error = Some(error);
        return out;
    }
    // Let the native browser handle website redirects, cookies, certificates and errors itself.
    // Signed page frames also load only once: a probe could consume their signature or count an open.
    if page_frame_url(&text).is_ok() || local_url(&text).is_err() {
        out.reachable = true;
        return out;
    }
    let url = match local_url(&text) {
        Ok(url) => url,
        Err(error) => {
            out.error = Some(error);
            return out;
        }
    };
    nexus_client::install_tls();
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            out.error = Some(error.to_string());
            return out;
        }
    };
    match client.get(url).send().await {
        Err(_) => out.error = Some("Nothing answers at this address.".into()),
        Ok(response) => {
            out.reachable = true;
            if !response.status().is_success() && !response.status().is_redirection() {
                out.error = Some(format!("The server answered HTTP {}.", response.status().as_u16()));
            }
            let headers = response.headers();
            if headers.get("x-frame-options").is_some()
                || headers.get_all("content-security-policy").iter().any(|value| {
                    value
                        .to_str()
                        .is_ok_and(|text| text.to_ascii_lowercase().contains("frame-ancestors"))
                })
            {
                out.frame_error = Some("This server restricts embedded previews. Open it in your browser.".into());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_loopback_without_credentials() {
        for address in [
            "https://example.com",
            "http://127.0.0.1.example.com",
            "file:///etc/passwd",
            "http://user:secret@localhost",
            "http://192.168.1.1",
        ] {
            assert!(local_url(address).is_err(), "{address}");
        }
        assert_eq!(
            local_url("http://localhost:5173/app").expect("local").as_str(),
            "http://127.0.0.1:5173/app"
        );
        assert!(local_url("http://[::1]:5173/").is_ok());
    }
    #[test]
    fn page_frames_on_the_sandbox_host_only() {
        let frame = "https://p-abc123.nexususercontent.dev/v/pag_1/3?exp=1&sig=x";
        assert!(page_frame_url(frame).is_ok());
        assert!(browser_url(frame).is_ok());
        assert!(browser_url("http://localhost:5173/").is_ok());
        for address in [
            "http://p-abc.nexususercontent.dev/v/pag_1/3",
            "https://nexususercontent.dev/",
            "https://.nexususercontent.dev/",
            "https://p-abc.nexususercontent.dev.example.com/",
            "https://p-abc.nexususercontent.dev@evil.example/",
            "https://user:pw@p-abc.nexususercontent.dev/",
            "https://p-abc.nexususercontent.dev:8443/",
            "https://a.b.nexususercontent.dev/",
            "https://example.com/",
        ] {
            assert!(page_frame_url(address).is_err(), "{address}");
        }
    }
    #[test]
    fn websites_and_local_servers_share_the_native_navigation_policy() {
        for address in [
            "https://google.com/",
            "http://example.com/docs?query=nexus#result",
            "https://example.com:8443/",
            "http://192.168.1.4:3000/",
            "http://localhost:5173/",
            "http://[::1]:5173/",
            "https://p-abc.nexususercontent.dev/v/page/1?sig=x",
        ] {
            assert_eq!(browser_url(address).expect("web address").as_str(), address);
        }
        for address in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,test",
            "app://localhost/app",
            "https://user:secret@example.com/",
            "http://user:secret@localhost/",
            "not a url",
        ] {
            assert!(browser_url(address).is_err(), "{address}");
        }
    }
    #[tokio::test]
    async fn websites_skip_the_dev_server_probe() {
        let result = probe("https://website.invalid/redirect?sig=sample".into()).await;
        assert!(result.reachable && result.error.is_none() && result.frame_error.is_none());
        let invalid = probe("file:///etc/passwd".into()).await;
        assert!(!invalid.reachable && invalid.error.is_some());
    }
    #[tokio::test]
    async fn a_page_frame_is_not_fetched() {
        let result = probe("https://p-abc.nexususercontent.dev/v/pag_1/3?sig=x".into()).await;
        assert!(result.reachable && result.error.is_none() && result.frame_error.is_none());
    }
    #[tokio::test]
    async fn checks_status_and_framing_without_following_redirects() {
        use std::io::{Read, Write};
        for (response, reachable, error, framed) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", true, false, false),
            (
                "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
                true,
                true,
                false,
            ),
            (
                "HTTP/1.1 302 Found\r\nLocation: https://example.com/\r\nContent-Length: 0\r\n\r\n",
                true,
                false,
                false,
            ),
            (
                "HTTP/1.1 200 OK\r\nX-Frame-Options: DENY\r\nContent-Length: 0\r\n\r\n",
                true,
                false,
                true,
            ),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
            let address = listener.local_addr().expect("address");
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                stream.write_all(response.as_bytes()).expect("response");
            });
            let result = probe(format!("http://{address}/")).await;
            server.join().expect("server");
            assert_eq!(result.reachable, reachable);
            assert_eq!(result.error.is_some(), error);
            assert_eq!(result.frame_error.is_some(), framed);
        }
    }
}
