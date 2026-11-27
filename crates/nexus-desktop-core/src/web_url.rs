//! URLs for the web app and its organization pages.

use nexus_client::url::enc;

/// Nexus UI's site: its URL without a trailing `/` or `/app`, as `desktopLinkOrigins` makes `ui`.
pub fn site(ui_url: &str) -> String {
    let ui = ui_url.trim_end_matches('/');
    ui.strip_suffix("/app").unwrap_or(ui).to_string()
}

/// The organization's pages in Nexus UI: `<site>/app/<org>`, as `appPath(org)`.
pub fn org_base(ui_url: &str, org: &str) -> String {
    format!("{}/app/{}", site(ui_url), enc(org))
}
