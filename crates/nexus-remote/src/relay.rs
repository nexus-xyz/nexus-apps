//! The relay through Gateway. A cloud machine's host opens a WebSocket to Gateway when it
//! boots, and the desktop app opens one too. Gateway joins the two and passes every frame through as it is. Both ends
//! then speak the host protocol of src/client.rs, with no SSH under it.
//!
//! - [`dial`]: one WebSocket to Gateway (`wss://`, or `ws://` for a Gateway on `http://`), with the caller's headers
//!   (`Authorization`). A refusal before the upgrade is a [`DialError`] with Gateway's status and code.
//! - [`RelayLink`]: a [`crate::machine::Link`] that reaches a host through Gateway's `/connect`.
//!
//! Both sides ping every [`PING`]: Cloudflare closes a connection idle for 100 seconds.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};

use crate::machine::{Link, OpenError, Opened, Reached, SocketOpened};
use crate::{Duplex, Result};

/// A WebSocket through Gateway.
pub type Socket = WebSocketStream<Box<dyn Duplex>>;

/// How often each end pings.
pub const PING: Duration = Duration::from_secs(20);

/// With no frame from the other end for this long (three pings missed), the connection is taken as lost.
pub const SILENCE: Duration = Duration::from_secs(70);

/// How long the TCP connect, the TLS handshake and the upgrade may take together.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a dial failed: Gateway's refusal (its HTTP status and code), or no answer at all.
#[derive(Debug, Clone, PartialEq)]
pub struct DialError {
    /// None when Gateway was not reached or did not answer HTTP.
    pub status: Option<u16>,
    /// Gateway's error code (`host_offline`, `too_many_streams`, `not_machine_token`).
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl DialError {
    fn network(error: impl std::fmt::Display) -> DialError {
        DialError {
            status: None,
            code: None,
            message: format!("Could not reach Nexus: {error}"),
        }
    }

    /// Worth trying again: no answer, a server error, a timeout, a conflict (`host_offline`) or too many streams.
    /// Any other 4xx needs a person (sign in again, ask for access).
    pub fn heals(&self) -> bool {
        match self.status {
            None => true,
            Some(status) => status >= 500 || matches!(status, 408 | 409 | 429),
        }
    }

    /// The token was refused (401 or 403).
    pub fn refused(&self) -> bool {
        matches!(self.status, Some(401 | 403))
    }
}

/// Gateway's refusal before the upgrade: `{ "error": { "code", "message" } }`, as its HTTP answers.
fn refusal(status: u16, body: Option<&[u8]>) -> DialError {
    let json: Value = body
        .and_then(|body| serde_json::from_slice(body).ok())
        .unwrap_or(Value::Null);
    let field = |key: &str| {
        json.get("error")
            .and_then(|error| error.get(key))
            .or_else(|| json.get(key))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(String::from)
    };
    let code = field("code");
    let message = match code.as_deref() {
        Some("host_offline") => "The machine's Nexus host is not online yet.".to_string(),
        Some("too_many_streams") => "Too many connections to this machine are open.".to_string(),
        _ => field("message").unwrap_or_else(|| format!("Nexus refused the connection (HTTP {status}).")),
    };
    DialError {
        status: Some(status),
        code,
        message,
    }
}

/// `https://gw/` and `/v1/…` as `wss://gw/v1/…`. Plain `http`/`ws` is accepted for a loopback host only, so a wrong
/// address can never send a token in clear text.
pub fn ws_url(gateway: &str, path: &str) -> Result<String> {
    let base = gateway.trim().trim_end_matches('/');
    let (scheme, rest, plain) = if let Some(rest) = base.strip_prefix("https://") {
        ("wss://", rest, false)
    } else if let Some(rest) = base.strip_prefix("http://") {
        ("ws://", rest, true)
    } else if let Some(rest) = base.strip_prefix("wss://") {
        ("wss://", rest, false)
    } else if let Some(rest) = base.strip_prefix("ws://") {
        ("ws://", rest, true)
    } else {
        return Err(format!("{gateway:?} is not a Nexus address."));
    };
    if plain && !is_loopback(rest) {
        return Err(format!(
            "{gateway:?} is not a secure Nexus address (https is required)."
        ));
    }
    Ok(format!("{scheme}{rest}{path}"))
}

/// Whether `authority` (`host[:port][/path]`) names this machine: `localhost`, `127.x.x.x` or `[::1]`.
fn is_loopback(authority: &str) -> bool {
    let authority = authority.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(host, _)| host),
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The TLS setup, made once: ring, the system's roots, HTTP/1.1 (a WebSocket upgrade is HTTP/1.1).
fn tls() -> std::result::Result<Arc<rustls::ClientConfig>, String> {
    static CONFIG: OnceLock<std::result::Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            use rustls_platform_verifier::BuilderVerifierExt as _;
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let mut config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_platform_verifier()
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Ok(Arc::new(config))
        })
        .clone()
}

/// One WebSocket to `url` with `headers`.
pub async fn dial(url: &str, headers: &[(String, String)]) -> std::result::Result<Socket, DialError> {
    dial_over(url, headers, |tcp| Box::new(tcp)).await
}

/// [`dial`], with the TCP stream wrapped by `wrap` under TLS (the host's write deadline).
pub async fn dial_over(
    url: &str,
    headers: &[(String, String)],
    wrap: impl FnOnce(TcpStream) -> Box<dyn Duplex> + Send,
) -> std::result::Result<Socket, DialError> {
    let dialing = async {
        let mut request = url.into_client_request().map_err(DialError::network)?;
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(DialError::network)?;
            let mut value = HeaderValue::from_str(value).map_err(DialError::network)?;
            if name == tungstenite::http::header::AUTHORIZATION {
                value.set_sensitive(true);
            }
            request.headers_mut().insert(name, value);
        }
        let uri = request.uri().clone();
        let secure = uri.scheme_str() == Some("wss");
        let host = uri
            .host()
            .map(|host| host.trim_start_matches('[').trim_end_matches(']').to_string())
            .ok_or_else(|| DialError::network("no host"))?;
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(DialError::network)?;
        let _ = tcp.set_nodelay(true);
        let stream = wrap(tcp);
        let stream: Box<dyn Duplex> = if secure {
            let connector = tokio_rustls::TlsConnector::from(tls().map_err(DialError::network)?);
            let name = rustls::pki_types::ServerName::try_from(host).map_err(DialError::network)?;
            Box::new(connector.connect(name, stream).await.map_err(DialError::network)?)
        } else {
            stream
        };
        match tokio_tungstenite::client_async(request, stream).await {
            Ok((socket, _)) => Ok(socket),
            Err(tungstenite::Error::Http(response)) => {
                Err(refusal(response.status().as_u16(), response.body().as_deref()))
            }
            Err(error) => Err(DialError::network(error)),
        }
    };
    tokio::time::timeout(DIAL_TIMEOUT, dialing)
        .await
        .unwrap_or_else(|_| Err(DialError::network("it did not answer in time")))
}

/// A host through Gateway's `/connect`: the app's side of the relay. The hello's token is empty, because Gateway
/// already checked that the person owns the machine.
pub struct RelayLink {
    /// `wss://…/v1/cloud/machines/{id}/connect` ([`ws_url`]).
    pub url: String,
    /// `Authorization: Bearer <the person's Gateway token>`, and the client's own headers.
    pub headers: Vec<(String, String)>,
    /// Calls made after the hello, before the relay says it is connected (`host_provision`).
    pub setup: Vec<(String, Value)>,
}

impl RelayLink {
    /// The link to machine `machine` through `gateway`, signed in with `token`.
    pub fn new(gateway: &str, machine: &str, token: &str) -> Result<RelayLink> {
        Ok(RelayLink {
            url: ws_url(gateway, &format!("/v1/cloud/machines/{machine}/connect"))?,
            headers: vec![("authorization".into(), format!("Bearer {token}"))],
            setup: Vec::new(),
        })
    }
}

impl Link for RelayLink {
    fn open(&self) -> BoxFuture<'_, Result<Opened>> {
        Box::pin(async { Err("A machine through Nexus has no byte stream: it is a WebSocket.".to_string()) })
    }

    fn update(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Err("A cloud machine's host updates with its image.".to_string()) })
    }

    fn reach_host(&self) -> BoxFuture<'_, std::result::Result<Reached, OpenError>> {
        Box::pin(async move {
            let socket = dial(&self.url, &self.headers).await.map_err(OpenError::from)?;
            Ok(Reached::Socket(SocketOpened {
                socket,
                token: String::new(),
                setup: self.setup.clone(),
            }))
        })
    }
}

impl From<DialError> for OpenError {
    fn from(error: DialError) -> OpenError {
        OpenError {
            heals: error.heals(),
            message: error.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_addresses_become_websocket_addresses() {
        assert_eq!(
            ws_url("https://api.example.test/", "/v1/x").ok(),
            Some("wss://api.example.test/v1/x".into())
        );
        assert_eq!(
            ws_url("http://127.0.0.1:9/", "/a").ok(),
            Some("ws://127.0.0.1:9/a".into())
        );
        assert!(ws_url("api.example.test", "/a").is_err());
        assert!(ws_url("http://api.example.test", "/a").is_err());
        assert!(ws_url("ws://10.0.0.5:80", "/a").is_err());
        assert!(ws_url("http://127.0.0.1@evil.example", "/a").is_err());
        assert!(ws_url("http://localhost:8080", "/a").is_ok());
        assert!(ws_url("http://[::1]:8080", "/a").is_ok());
    }

    #[test]
    fn refusals_say_whether_they_heal() {
        let offline = refusal(409, Some(br#"{"error":{"code":"host_offline","message":"raw"}}"#));
        assert_eq!(offline.code.as_deref(), Some("host_offline"));
        assert!(offline.heals());
        assert!(offline.message.contains("not online"));
        assert!(refusal(429, None).heals());
        assert!(refusal(503, None).heals());
        assert!(refusal(408, None).heals());
        let forbidden = refusal(403, Some(br#"{"error":{"code":"not_machine_token","message":"No."}}"#));
        assert!(!forbidden.heals() && forbidden.refused());
        assert_eq!(forbidden.message, "No.");
        assert!(!refusal(404, None).heals());
        assert!(refusal(401, None).refused());
        assert!(DialError::network("refused").heals());
    }

    #[tokio::test]
    async fn a_closed_port_is_a_network_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(1);
        drop(listener);
        let failed = dial(&format!("ws://127.0.0.1:{port}/x"), &[]).await;
        assert!(failed.is_err_and(|e| e.status.is_none() && e.heals()));
    }
}
