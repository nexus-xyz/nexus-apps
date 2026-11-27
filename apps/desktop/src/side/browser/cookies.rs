//! "Import sign-ins", as the Nexus app's (`cookie-import.tsx` and `cookie-import-model.ts`): the
//! chosen sites' cookies, copied from a browser profile on this computer into this session's browser only. Finding the
//! profiles and reading their cookies is `nexus_browser::cookies`, shared with the Nexus app. This writes them into
//! the engine's cookie store through the page's DevTools Protocol (`Network.setCookie`), and holds the form.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_browser::cookies::{Cookie, SameSite};
use serde_json::{Value, json};

use crate::palette::Palette;

/// What the form says above its fields.
pub const INTRO: &str = "Copy the sign-in cookies of these sites from your browser into this session's browser only. \
                         Nothing leaves this computer.";
/// What the import says when no tab has a page to write through.
pub const NO_PAGE: &str = "Open a page in this session's Browser first.";

/// The sites typed in the form: split on commas, spaces and new lines, each once.
pub fn split_sites(text: &str) -> Vec<String> {
    let mut sites: Vec<String> = Vec::new();
    for site in text
        .split([' ', ',', '\n', '\t', '\r'])
        .map(str::trim)
        .filter(|site| !site.is_empty())
    {
        if !sites.iter().any(|known| known == site) {
            sites.push(site.to_string());
        }
    }
    sites
}

/// The site to suggest for the page shown: its host without `www.`, or nothing for a blank tab or a local server.
pub fn site_of(address: &str) -> String {
    let Some(host) = url::Url::parse(address)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
    else {
        return String::new();
    };
    if host == "localhost" || host.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.') {
        return String::new();
    }
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// The profiles found, as the form lists them.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub supported: bool,
    /// Each profile's id and name.
    pub sources: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// Look for the profiles under the person's home folder. It reads folders only, no cookie.
pub fn discover() -> Sources {
    let found = nexus_browser::cookies::sources::discover_here().unwrap_or_default();
    Sources {
        supported: cfg!(any(target_os = "linux", target_os = "macos")),
        sources: found
            .sources
            .iter()
            .map(|source| (source.id.clone(), source.name.clone()))
            .collect(),
        warnings: found.warnings,
    }
}

/// An empty discovery is conclusive only when no browser failed to read.
pub fn source_notice(found: &Sources) -> Option<&'static str> {
    if !found.supported {
        return Some("Importing sign-ins is available on macOS and Linux.");
    }
    if !found.sources.is_empty() || !found.warnings.is_empty() {
        return None;
    }
    Some("No Chrome, Chromium, Brave, Edge or Firefox profile was found.")
}

/// What one profile gave for the sites asked, before it is written.
#[derive(Clone)]
pub struct Read {
    pub cookies: Vec<Cookie>,
    pub skipped: usize,
    pub warning: Option<String>,
    /// The profile's name, and the sites as checked.
    pub source: String,
    pub sites: Vec<String>,
}

/// Read the cookies of `sites` (as typed) in profile `source`. The keyring may ask the person (the macOS Keychain's
/// prompt), so this runs off the UI thread.
pub fn read(source: &str, sites: &[String]) -> Result<Read, String> {
    let sites = nexus_browser::cookies::sites(&json!(sites))?;
    let found = nexus_browser::cookies::sources::discover_here().unwrap_or_default();
    let source = found.select(Some(source))?;
    let read = source.read(&sites)?;
    Ok(Read {
        cookies: read.cookies,
        skipped: read.skipped,
        warning: read.warning,
        source: source.name,
        sites,
    })
}

/// One or two sentences for the form, after an import of `imported` cookies.
pub fn summary(read: &Read, imported: usize) -> String {
    let sites = read.sites.join(", ");
    let mut parts = vec![if imported > 0 {
        format!(
            "Imported {imported} {} for {sites} from {}. The page reloads to use them.",
            if imported == 1 { "cookie" } else { "cookies" },
            read.source
        )
    } else {
        format!("{} has no sign-in cookies for {sites}.", read.source)
    }];
    if read.skipped > 0 {
        parts.push(format!("{} could not be read.", read.skipped));
    }
    if let Some(warning) = &read.warning {
        parts.push(warning.clone());
    }
    parts.join(" ")
}

/// The DevTools message that sets `cookie` in the engine's store. A domain cookie (`.example.com`) keeps its domain,
/// so its subdomains get it; a host-only one is set for its address, so they do not.
pub fn set_cookie(id: u64, cookie: &Cookie) -> String {
    let mut params = json!({
        "name": cookie.name,
        "value": cookie.value,
        "path": cookie.path,
        "secure": cookie.secure,
        "httpOnly": cookie.http_only,
    });
    if cookie.domain.starts_with('.') {
        params["domain"] = json!(cookie.domain);
    } else {
        let scheme = if cookie.secure { "https" } else { "http" };
        params["url"] = json!(format!("{scheme}://{}{}", cookie.domain, cookie.path));
    }
    if let Some(expires) = cookie.expires {
        params["expires"] = json!(expires);
    }
    if let Some(same_site) = cookie.same_site {
        params["sameSite"] = json!(match same_site {
            SameSite::None => "None",
            SameSite::Lax => "Lax",
            SameSite::Strict => "Strict",
        });
    }
    json!({ "id": id, "method": "Network.setCookie", "params": params }).to_string()
}

/// The answer to a [`set_cookie`] message: its id, and whether the cookie was set.
pub fn set_answer(message: &str) -> Option<(u64, bool)> {
    let message: Value = serde_json::from_str(message).ok()?;
    let id = message.get("id")?.as_u64()?;
    let set = message
        .get("result")
        .is_some_and(|result| result.get("success").and_then(Value::as_bool) != Some(false));
    Some((id, set))
}

/// What the form asks of its browser.
pub enum FormEvent {
    /// Write these cookies, and say how many it holds with [`CookieForm::finish`].
    Write(Read),
    Close,
}

/// The form under the address bar, not in a dialog, so the page stays in view.
pub struct CookieForm {
    sites: Entity<InputState>,
    sources: Option<Sources>,
    chosen: Option<String>,
    loading: bool,
    busy: bool,
    said: Option<String>,
    _look: Option<Task<()>>,
}

impl EventEmitter<FormEvent> for CookieForm {}

impl CookieForm {
    /// The form, with `site` (the page's) in its Sites field.
    pub fn new(site: String, window: &mut Window, cx: &mut Context<Self>) -> CookieForm {
        let sites = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("github.com, example.com")
                .default_value(site)
        });
        let mut form = CookieForm {
            sites,
            sources: None,
            chosen: None,
            loading: true,
            busy: false,
            said: None,
            _look: None,
        };
        form.look(cx);
        form
    }

    /// Look for the browser profiles again.
    fn look(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.sources = None;
        self.chosen = None;
        self.said = None;
        self._look = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { discover() }).await;
            let _ = this.update(cx, |this, cx| {
                this.chosen = found.sources.first().map(|(id, _)| id.clone());
                this.sources = Some(found);
                this.loading = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.chosen.clone() else {
            return;
        };
        let sites = split_sites(&self.sites.read(cx).value());
        self.busy = true;
        self.said = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let read = cx.background_spawn(async move { read(&source, &sites) }).await;
            let _ = this.update(cx, |this, cx| match read {
                Ok(read) => cx.emit(FormEvent::Write(read)),
                Err(problem) => this.finish(problem, cx),
            });
        })
        .detach();
    }

    /// The import ended: what it says.
    pub fn finish(&mut self, said: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.said = Some(said);
        cx.notify();
    }

    fn render_sources(&self, found: &Sources, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_wrap()
            .gap(px(4.))
            .child(div().text_color(palette.muted).mr(px(4.)).child("Browser"))
            .children(found.sources.iter().enumerate().map(|(index, (id, name))| {
                let chosen = self.chosen.as_deref() == Some(id.as_str());
                let id = id.clone();
                super::super::chip(("cookie-source", index), "", chosen, palette)
                    .child(name.clone())
                    .when(!self.busy, |chip| {
                        chip.on_click(cx.listener(move |this, _, _, cx| {
                            this.chosen = Some(id.clone());
                            cx.notify();
                        }))
                    })
            }))
    }
}

impl Render for CookieForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        let found = self.sources.clone();
        let has_sources = found.as_ref().is_some_and(|found| !found.sources.is_empty());
        let supported = found.as_ref().is_none_or(|found| found.supported);
        let can_import = !self.busy
            && !self.loading
            && self.chosen.is_some()
            && !split_sites(&self.sites.read(cx).value()).is_empty();
        let action = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(palette.border)
                .text_color(palette.foreground)
                .when(enabled, |button| {
                    button.cursor_pointer().hover(|style| style.bg(palette.hover))
                })
                .when(!enabled, |button| button.opacity(0.5))
                .child(label)
        };
        v_flex()
            .id("cookie-import")
            .flex_none()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(palette.border)
            .text_size(px(12.5))
            .child(div().text_color(palette.muted).child(INTRO))
            .when(self.loading, |form| form.child("Looking for browser profiles…"))
            .children(found.as_ref().and_then(source_notice).map(|notice| div().child(notice)))
            .children(
                found
                    .iter()
                    .flat_map(|found| found.warnings.clone())
                    .map(|warning| div().child(warning)),
            )
            .when_some(found.as_ref().filter(|_| has_sources), |form, found| {
                form.child(self.render_sources(found, palette, cx)).child(
                    h_flex()
                        .gap(px(8.))
                        .child(div().text_color(palette.muted).child("Sites"))
                        .child(
                            div()
                                .flex_1()
                                .h(px(28.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(palette.raised)
                                .border_1()
                                .border_color(palette.border)
                                .child(Input::new(&self.sites).appearance(false)),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap(px(6.))
                    .when(has_sources, |row| {
                        let label = if self.busy { "Importing…" } else { "Import" };
                        let button = action("cookie-import-go", label, can_import);
                        row.child(if can_import {
                            button.on_click(cx.listener(|this, _, _, cx| this.import(cx)))
                        } else {
                            button
                        })
                    })
                    .when(supported, |row| {
                        let enabled = !self.busy && !self.loading;
                        let button = action("cookie-import-again", "Try again", enabled);
                        row.child(if enabled {
                            button.on_click(cx.listener(|this, _, _, cx| this.look(cx)))
                        } else {
                            button
                        })
                    })
                    .child(
                        action("cookie-import-close", "Close", true)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FormEvent::Close))),
                    ),
            )
            .children(self.said.clone().map(|said| div().child(said)))
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn cookie(domain: &str, secure: bool) -> Cookie {
        Cookie {
            domain: domain.into(),
            name: "session".into(),
            value: "v".into(),
            path: "/".into(),
            expires: Some(4_000_000_000),
            secure,
            http_only: true,
            same_site: Some(SameSite::Lax),
        }
    }

    #[test]
    fn sites_split_on_commas_spaces_and_lines_each_once() {
        assert_eq!(
            split_sites(" github.com, example.com\nexample.com  gitlab.com,"),
            ["github.com", "example.com", "gitlab.com"]
        );
        assert!(split_sites(" , ").is_empty());
    }

    #[test]
    fn the_suggested_site_is_the_pages_host_without_www() {
        assert_eq!(site_of("https://www.github.com/nexus"), "github.com");
        assert_eq!(site_of("https://app.example.com/"), "app.example.com");
        assert_eq!(site_of("http://localhost:3000/"), "");
        assert_eq!(site_of("http://127.0.0.1:8080/"), "");
        assert_eq!(site_of(""), "");
    }

    #[test]
    fn an_empty_discovery_says_so_only_without_failures() {
        let mut found = Sources {
            supported: true,
            ..Sources::default()
        };
        assert_eq!(
            source_notice(&found),
            Some("No Chrome, Chromium, Brave, Edge or Firefox profile was found.")
        );
        found.warnings.push("Nexus cannot access Chrome profiles.".into());
        assert_eq!(source_notice(&found), None);
        found.supported = false;
        assert_eq!(
            source_notice(&found),
            Some("Importing sign-ins is available on macOS and Linux.")
        );
    }

    #[test]
    fn the_summary_names_the_count_the_sites_and_the_source() {
        let mut read = Read {
            cookies: Vec::new(),
            skipped: 0,
            warning: None,
            source: "Firefox · default".into(),
            sites: vec!["github.com".into(), "example.com".into()],
        };
        assert_eq!(
            summary(&read, 1),
            "Imported 1 cookie for github.com, example.com from Firefox · default. The page reloads to use them."
        );
        read.skipped = 2;
        read.warning = Some("No keyring.".into());
        assert_eq!(
            summary(&read, 0),
            "Firefox · default has no sign-in cookies for github.com, example.com. 2 could not be read. No keyring."
        );
        assert!(summary(&read, 3).starts_with("Imported 3 cookies"));
    }

    #[test]
    fn a_domain_cookie_keeps_its_domain_and_a_host_cookie_its_address() {
        let domain: Value = serde_json::from_str(&set_cookie(7, &cookie(".example.com", true))).unwrap();
        assert_eq!(domain["id"], 7);
        assert_eq!(domain["method"], "Network.setCookie");
        assert_eq!(domain["params"]["domain"], ".example.com");
        assert!(domain["params"].get("url").is_none());
        assert_eq!(domain["params"]["sameSite"], "Lax");
        assert_eq!(domain["params"]["expires"], 4_000_000_000_i64);
        assert_eq!(domain["params"]["httpOnly"], true);
        let host: Value = serde_json::from_str(&set_cookie(8, &cookie("app.example.com", false))).unwrap();
        assert_eq!(host["params"]["url"], "http://app.example.com/");
        assert!(host["params"].get("domain").is_none());
    }

    #[test]
    fn an_answer_says_whether_the_cookie_was_set() {
        assert_eq!(set_answer(r#"{"id":7,"result":{"success":true}}"#), Some((7, true)));
        assert_eq!(set_answer(r#"{"id":7,"result":{}}"#), Some((7, true)));
        assert_eq!(set_answer(r#"{"id":7,"result":{"success":false}}"#), Some((7, false)));
        assert_eq!(
            set_answer(r#"{"id":8,"error":{"message":"Invalid"}}"#),
            Some((8, false))
        );
        assert_eq!(set_answer(r#"{"method":"Page.loadEventFired"}"#), None);
    }
}
