//! A server session's Browser: the agent runs on the server, so the `localhost:3000` it prints is the
//! server's port, not this computer's. Every local address the session prints or the person types (`localhost`,
//! `127.0.0.1`, `0.0.0.0`, `[::1]`) opens through an SSH forward to that port (`machines/previews.rs`): the page loads
//! `http://127.0.0.1:<forwarded port>/…` with the same path, query and fragment. The tabs and the address bar keep the
//! server's own address, and the bar names the server. Checks and the automatic Browser go through the forward too.
//! A local session never forwards.

use std::sync::Arc;

use futures::future::BoxFuture;
use nexus_browser::preview::Probe;

/// Opens (or reuses) the forward to a port on the server, and answers the port on this computer.
pub type Opener = Arc<dyn Fn(u16) -> BoxFuture<'static, Result<u16, String>> + Send + Sync>;

/// The port on the session's machine that `url` names, when it is a local address there: `http` or `https` at
/// `localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]` or `[::]`, its port or the scheme's.
pub fn server_port(url: &str) -> Option<u16> {
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let local = match url.host()? {
        url::Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback() || ip.is_unspecified(),
        url::Host::Ipv6(ip) => ip.is_loopback() || ip.is_unspecified(),
    };
    local.then(|| url.port_or_known_default()).flatten()
}

/// `url` on this computer's forward `local`: host `127.0.0.1` and port `local`, its path, query and fragment kept.
pub fn through(url: &str, local: u16) -> Option<String> {
    server_port(url)?;
    let mut url = url::Url::parse(url).ok()?;
    url.set_host(Some("127.0.0.1")).ok()?;
    url.set_port(Some(local)).ok()?;
    Some(url.to_string())
}

/// What the page's address `url` is on the server: a forward's `127.0.0.1:<local>` back as `localhost:<port>`.
/// Any other address stays as it is.
pub fn on_server(url: &str, ports: &[(u16, u16)]) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let Some(remote) = parsed
        .port()
        .filter(|_| parsed.host_str() == Some("127.0.0.1"))
        .and_then(|local| ports.iter().find(|(_, here)| *here == local).map(|(there, _)| *there))
    else {
        return url.to_string();
    };
    if parsed.set_host(Some("localhost")).is_err() || parsed.set_port(Some(remote)).is_err() {
        return url.to_string();
    }
    parsed.to_string()
}

/// How the browser of a session at `place` loads `url`: through a forward to the server's port, or as it is.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    Forward(u16),
}

pub fn route(place: Option<&nexus_browser::place::Place>, url: &str) -> Route {
    match (place.and_then(|place| place.server()), server_port(url)) {
        (Some(_), Some(port)) => Route::Forward(port),
        _ => Route::Direct,
    }
}

/// Checks `url` as [`nexus_browser::preview::probe`] does, through the server's forward when it is a local address
/// there. The answer names `url`, and the forward it went through: (port there, port here).
pub async fn probe(opener: Opener, url: String) -> (Probe, Option<(u16, u16)>) {
    let Some(port) = server_port(&url) else {
        return (nexus_browser::preview::probe(url).await, None);
    };
    match opener(port).await {
        Ok(local) => {
            let Some(forwarded) = through(&url, local) else {
                return (refused(url, "Invalid preview address.".into()), None);
            };
            let mut probe = nexus_browser::preview::probe(forwarded).await;
            probe.url = url;
            (probe, Some((port, local)))
        }
        Err(error) => (refused(url, error), None),
    }
}

fn refused(url: String, error: String) -> Probe {
    Probe {
        url,
        reachable: false,
        frame_error: None,
        error: Some(error),
    }
}

/// The server a session's browser shows pages of, and the forwards its pages went through.
pub struct ServerPreview {
    /// The server's name, for the address bar.
    pub name: String,
    /// (port there, port here), newest last.
    ports: Vec<(u16, u16)>,
    pub opener: Opener,
}

impl ServerPreview {
    /// Server `machine`'s preview, through the app's servers (`machines::get`).
    pub fn new(machine: &str) -> ServerPreview {
        let machines = crate::machines::get();
        let name = machines.server_name(machine).unwrap_or_else(|| machine.to_string());
        let machine = machine.to_string();
        let opener: Opener = Arc::new(move |port| {
            let machine = machine.clone();
            Box::pin(async move { crate::machines::get().preview(&machine, port).await })
        });
        ServerPreview::with_opener(name, opener)
    }

    pub fn with_opener(name: String, opener: Opener) -> ServerPreview {
        ServerPreview {
            name,
            ports: Vec::new(),
            opener,
        }
    }

    /// Port `remote` there is reached at `local` here now. Whether that changed what was known.
    pub fn remember(&mut self, remote: u16, local: u16) -> bool {
        if self.ports.contains(&(remote, local)) {
            return false;
        }
        self.ports.retain(|(there, _)| *there != remote);
        self.ports.push((remote, local));
        true
    }

    /// The address the page loads for the server's `url`, once its forward is known.
    pub fn page_url(&self, url: &str) -> Option<String> {
        let port = server_port(url)?;
        let (_, local) = self.ports.iter().find(|(there, _)| *there == port)?;
        through(url, *local)
    }

    /// The server's address for the page's `url`.
    pub fn on_server(&self, url: &str) -> String {
        on_server(url, &self.ports)
    }

    /// What the address bar's label says when pointed at.
    pub fn tooltip(&self) -> String {
        format!(
            "This session runs on {}. Its localhost addresses are the server's, reached over SSH.",
            self.name
        )
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;
    use nexus_browser::place::Place;

    #[test]
    fn every_loopback_form_is_a_port_on_the_server() {
        for (url, port) in [
            ("http://localhost:3000/", 3000),
            ("http://LOCALHOST:5173/app", 5173),
            ("http://127.0.0.1:8080/x", 8080),
            ("http://127.0.0.2:8081/", 8081),
            ("http://0.0.0.0:4321/", 4321),
            ("http://[::1]:3001/", 3001),
            ("http://[::]:3002/", 3002),
            ("https://localhost:8443/", 8443),
            ("http://localhost/", 80),
        ] {
            assert_eq!(server_port(url), Some(port), "{url}");
        }
        for url in [
            "https://example.com/",
            "http://192.168.1.4:3000/",
            "http://localhost.example.com:3000/",
            "http://user:pw@localhost:3000/",
            "file:///etc/passwd",
            "not an address",
            "",
        ] {
            assert_eq!(server_port(url), None, "{url}");
        }
    }

    #[test]
    fn a_forward_keeps_the_path_query_and_fragment() {
        assert_eq!(
            through("http://localhost:3000/some/path?q=1&b=two#part", 41234).as_deref(),
            Some("http://127.0.0.1:41234/some/path?q=1&b=two#part")
        );
        assert_eq!(
            through("http://0.0.0.0:5173", 41000).as_deref(),
            Some("http://127.0.0.1:41000/")
        );
        assert_eq!(
            through("http://[::1]:3000/a?x", 41001).as_deref(),
            Some("http://127.0.0.1:41001/a?x")
        );
        assert_eq!(
            through("https://localhost:8443/s", 41002).as_deref(),
            Some("https://127.0.0.1:41002/s")
        );
        assert_eq!(through("https://example.com/a?q=1", 41000), None, "a website stays");
    }

    #[test]
    fn the_page_shows_the_servers_address() {
        let ports = [(3000, 41234), (5173, 41235)];
        assert_eq!(
            on_server("http://127.0.0.1:41234/some/path?q=1#x", &ports),
            "http://localhost:3000/some/path?q=1#x"
        );
        assert_eq!(on_server("http://127.0.0.1:41235/", &ports), "http://localhost:5173/");
        assert_eq!(
            on_server("http://127.0.0.1:9999/", &ports),
            "http://127.0.0.1:9999/",
            "not a forward"
        );
        assert_eq!(on_server("https://example.com/", &ports), "https://example.com/");
    }

    #[test]
    fn only_a_server_session_forwards_and_only_its_local_addresses() {
        let server = Place::Server("m1".into());
        assert_eq!(route(Some(&server), "http://localhost:3000/a"), Route::Forward(3000));
        assert_eq!(route(Some(&server), "http://0.0.0.0:8000/"), Route::Forward(8000));
        assert_eq!(route(Some(&server), "https://example.com/"), Route::Direct);
        for place in [Some(Place::Local), Some(Place::Cloud("c1".into())), None] {
            assert_eq!(
                route(place.as_ref(), "http://localhost:3000/a"),
                Route::Direct,
                "{place:?}"
            );
        }
    }

    #[test]
    fn a_forward_is_remembered_once_and_replaced_when_it_moves() {
        let opener: Opener = Arc::new(|_| Box::pin(async { Err("unused".to_string()) }));
        let mut preview = ServerPreview::with_opener("box".into(), opener);
        assert_eq!(preview.page_url("http://localhost:3000/a"), None);
        assert!(preview.remember(3000, 41000));
        assert!(!preview.remember(3000, 41000));
        assert_eq!(
            preview.page_url("http://localhost:3000/a?b#c").as_deref(),
            Some("http://127.0.0.1:41000/a?b#c")
        );
        assert!(preview.remember(3000, 42000), "a new forward after the old one died");
        assert_eq!(preview.on_server("http://127.0.0.1:41000/"), "http://127.0.0.1:41000/");
        assert_eq!(preview.on_server("http://127.0.0.1:42000/"), "http://localhost:3000/");
    }

    #[tokio::test]
    async fn a_check_goes_through_the_forward_and_names_the_servers_address() {
        let opener: Opener = Arc::new(|port| Box::pin(async move { Err(format!("no forward to {port}")) }));
        let (probe, forward) = super::probe(opener, "http://localhost:3000/a".into()).await;
        assert_eq!(probe.url, "http://localhost:3000/a");
        assert!(!probe.reachable);
        assert_eq!(probe.error.as_deref(), Some("no forward to 3000"));
        assert_eq!(forward, None);

        // A forward to a port where nothing listens: the check reaches this computer's port, and names the server's.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .expect("a free port");
        let opener: Opener = Arc::new(move |_| Box::pin(async move { Ok(closed) }));
        let (probe, forward) = super::probe(opener, "http://localhost:3000/a".into()).await;
        assert_eq!(probe.url, "http://localhost:3000/a");
        assert!(!probe.reachable);
        assert_eq!(forward, Some((3000, closed)));
    }
}
