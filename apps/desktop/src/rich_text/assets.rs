//! Embedded assets only. Private Markdown travels in the URL fragment.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::Duration,
};
const HTML: &[u8] = include_bytes!("../../rich-text/assets/renderer.html");
const CSP: &str = include_str!("../../rich-text/assets/renderer.csp");
fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "/" => Some(("text/html; charset=utf-8", HTML)),
        "/renderer.js" => Some(("text/javascript", include_bytes!("../../rich-text/assets/renderer.js"))),
        "/renderer.css" => Some(("text/css", include_bytes!("../../rich-text/assets/renderer.css"))),
        _ => None,
    }
}
fn cached_origin(
    cache: &Mutex<Option<String>>,
    start: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let mut origin = cache.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(origin) = &*origin {
        return Ok(origin.clone());
    }
    let value = start()?;
    *origin = Some(value.clone());
    Ok(value)
}
pub(super) fn origin() -> Result<String, String> {
    static ORIGIN: Mutex<Option<String>> = Mutex::new(None);
    cached_origin(&ORIGIN, || {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        let origin = format!("http://{}/", listener.local_addr().map_err(|e| e.to_string())?);
        std::thread::Builder::new()
            .name("rich-text-assets".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = std::thread::Builder::new()
                        .name("rich-text-request".into())
                        .spawn(move || serve(stream));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(origin)
    })
}
fn serve(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut request = [0; 8192];
    let Ok(len) = stream.read(&mut request) else { return };
    let line = String::from_utf8_lossy(&request[..len]);
    let mut parts = line.split_whitespace();
    let method = parts.next();
    let path = parts.next().unwrap_or("");
    let resource = if method == Some("GET") { asset(path) } else { None };
    let (status, mime, body) = resource.map_or(("404 Not Found", "text/plain", &b"Not found"[..]), |(mime, body)| {
        ("200 OK", mime, body)
    });
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nContent-Security-Policy: {CSP}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(body));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_start_can_retry_and_success_is_cached() {
        let cache = Mutex::new(None);
        assert!(cached_origin(&cache, || Err("bind failed".into())).is_err());
        assert_eq!(
            cached_origin(&cache, || Ok("http://localhost/".into())).unwrap(),
            "http://localhost/"
        );
        assert_eq!(
            cached_origin(&cache, || panic!("success must be cached")).unwrap(),
            "http://localhost/"
        );
    }
    #[test]
    fn stalled_connection_does_not_block_other_requests() {
        let origin = origin().unwrap();
        let address = origin.trim_start_matches("http://").trim_end_matches('/');
        let _stalled = TcpStream::connect(address).unwrap();
        let mut request = TcpStream::connect(address).unwrap();
        request.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        request.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut reply = String::new();
        request.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200 OK"));
    }
    #[test]
    fn assets_never_accept_paths_or_query_content() {
        for path in ["/etc/passwd", "/../renderer.js", "/?source=private", "/renderer.js?x=1"] {
            assert!(asset(path).is_none());
        }
        assert!(CSP.contains("connect-src 'none'"));
    }
    #[test]
    fn server_serves_embedded_html_with_policy() {
        let origin = origin().unwrap();
        let address = origin.trim_start_matches("http://").trim_end_matches('/');
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200 OK"));
        assert!(reply.contains("Content-Security-Policy:"));
        assert!(reply.ends_with(std::str::from_utf8(HTML).unwrap()));
    }
}
