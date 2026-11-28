//! App assets: Lucide icons, Nexus marks, and model and agent logos.
//! Maker marks come from Nexus UI (simple-icons, CC0), OpenAI's brand kit, and lobe-icons (Z.ai, MIT).
//! Cursor's cube comes from its official brand kit: https://cursor.com/brand.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

/// A logo the app embeds: a third-party mark or Nexus's own. build.rs stages it in `$OUT_DIR/held/`, with a generic
/// stand-in when the file is not in the tree (the open-source repo, while Legal reviews the marks).
macro_rules! held {
    ($path:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/held/", $path))
    };
}
pub(crate) use held;

pub struct Assets;

/// Nexus's wordmark (Nexus UI's `public/brand/nexus-wordmark.svg`): "by NEXUS" under an organization's logo.
pub const WORDMARK: &str = "brand/nexus-wordmark.svg";

/// The app's icon (a dark tile and the white Nexus AI spiral), drawn in colour with `img`. Where the app shows Nexus's
/// own logo as a picture (the app tile), it shows this.
pub const APP_ICON: &str = "brand/app-icon.svg";

/// The panel's resize grip at its bottom-left corner.
pub const PANEL_GRIP: &str = "panel/grip.svg";

/// The app's mark, a tinted stand-in for `APP_ICON` where the theme's ink should colour it: the app icon's 16-layer
/// Nexus AI spiral without its dark tile, one colour so `svg().text_color` tints it with the theme's ink (Home, the
/// Auto model, the Nexus agent).
pub const APP_MARK: &str = "brand/app-mark.svg";
/// The arc that turns on Stop's ring while a turn runs: a quarter of a 28px circle, 1.5px, drawn on the ring itself.
pub const STOP_ARC: &str = "composer/stop-arc.svg";
/// A background session's 12px ring in the sidebar, 1.5px, in a 16px box that leaves room for its dot.
pub const ORBIT_RING: &str = "sidebar/orbit-ring.svg";
/// The dot that goes round that ring, at the top of the same 16px box, centred on the ring's line: the mark turns it
/// as a sprite, so it moves between device pixels.
pub const ORBIT_DOT: &str = "sidebar/orbit-dot.svg";

/// Where the app mark's layers are, one SVG each, rim first (`mark_layer`): the breathing mark dims each on its own.
const MARK_LAYER: &str = "brand/mark-layer-";

/// The app mark's layer `index` (0 is the rim) as its own SVG, in the whole mark's frame, so the layers stack exactly.
pub fn mark_layer(index: usize) -> SharedString {
    format!("{MARK_LAYER}{index}.svg").into()
}

/// The app mark cut into its layers, once: each `<path>` of `app-mark.svg` in the mark's own `<svg>` and `<g>`.
pub fn mark_layers() -> &'static [Vec<u8>] {
    static LAYERS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    LAYERS.get_or_init(|| {
        let mark = std::str::from_utf8(held!("brand/app-mark.svg")).expect("the mark is UTF-8");
        let (head, rest) = mark.split_at(mark.find("<path").expect("the mark has paths"));
        let tail = &rest[rest.rfind("/>").expect("the last path closes") + 2..];
        rest.split_inclusive("/>")
            .filter(|path| path.trim_start().starts_with("<path"))
            .map(|path| format!("{head}{}{tail}", path.trim()).into_bytes())
            .collect()
    })
}

const MARKS: &[(&str, &[u8])] = &[
    ("anthropic", held!("marks/anthropic.svg")),
    ("openai", held!("marks/openai.svg")),
    ("cursor", held!("marks/cursor.svg")),
    ("google", held!("marks/google.svg")),
    ("deepseek", held!("marks/deepseek.svg")),
    ("meta", held!("marks/meta.svg")),
    ("mistral", held!("marks/mistral.svg")),
    ("qwen", held!("marks/qwen.svg")),
    ("alibaba", held!("marks/qwen.svg")),
    ("nvidia", held!("marks/nvidia.svg")),
    ("zai", held!("marks/zai.svg")),
    ("z-ai", held!("marks/zai.svg")),
    ("zhipu", held!("marks/zai.svg")),
    ("nexus", held!("marks/nexus.svg")),
];

/// Services' marks: the conversation's MCP calls and Settings › Connectors' rows. The first 16 are the exact paths and
/// colours of Nexus UI's `session-timeline/mcp-marks.tsx` (simple-icons 16.35, CC0). Slack uses the same existing
/// landing path. Marks remain their owners' trademarks.
const CONNECTORS: &[(&str, &str, &[u8], Option<u32>)] = &[
    ("Linear", "linear", held!("connectors/linear.svg"), Some(0x5E6AD2)),
    ("Slack", "slack", held!("connectors/slack.svg"), Some(0xE01E5A)),
    ("GitHub", "github", held!("connectors/github.svg"), None),
    ("Notion", "notion", held!("connectors/notion.svg"), None),
    (
        "Google Drive",
        "googleDrive",
        held!("connectors/googleDrive.svg"),
        Some(0x4285F4),
    ),
    ("Gmail", "gmail", held!("connectors/gmail.svg"), Some(0xEA4335)),
    (
        "Google Calendar",
        "googleCalendar",
        held!("connectors/googleCalendar.svg"),
        Some(0x4285F4),
    ),
    ("Vercel", "vercel", held!("connectors/vercel.svg"), None),
    ("Figma", "figma", held!("connectors/figma.svg"), Some(0xF24E1E)),
    ("Sentry", "sentry", held!("connectors/sentry.svg"), None),
    ("Stripe", "stripe", held!("connectors/stripe.svg"), Some(0x635BFF)),
    (
        "Atlassian",
        "atlassian",
        held!("connectors/atlassian.svg"),
        Some(0x0052CC),
    ),
    ("Jira", "jira", held!("connectors/jira.svg"), Some(0x0052CC)),
    ("Confluence", "confluence", held!("connectors/confluence.svg"), None),
    ("PostHog", "posthog", held!("connectors/posthog.svg"), None),
    (
        "Chrome DevTools",
        "chrome",
        held!("connectors/chrome.svg"),
        Some(0x4285F4),
    ),
    // Settings › Connectors' other Gateway connectors: simple-icons 16.35 (CC0), padded to the same 28px box as the
    // marks above. Salesforce, Microsoft Teams and Outlook have no simple-icons mark and keep a letter tile. Intercom
    // and Zendesk draw in the text colour: their hex is lost on a light or a dark page. Airtable draws in the text
    // colour too: its trademark guidelines say the logo is never modified, and the NOTICE calls the drawing unaltered.
    ("Airtable", "airtable", held!("connectors/airtable.svg"), None),
    ("Asana", "asana", held!("connectors/asana.svg"), Some(0xF06A6A)),
    (
        "Bitbucket",
        "bitbucket",
        held!("connectors/bitbucket.svg"),
        Some(0x0052CC),
    ),
    (
        "Cloudflare",
        "cloudflare",
        held!("connectors/cloudflare.svg"),
        Some(0xF38020),
    ),
    ("Datadog", "datadog", held!("connectors/datadog.svg"), Some(0x632CA6)),
    ("Dropbox", "dropbox", held!("connectors/dropbox.svg"), Some(0x0061FF)),
    ("GitLab", "gitlab", held!("connectors/gitlab.svg"), Some(0xFC6D26)),
    (
        "Google Sheets",
        "googleSheets",
        held!("connectors/googleSheets.svg"),
        Some(0x34A853),
    ),
    ("HubSpot", "hubspot", held!("connectors/hubspot.svg"), Some(0xFF7A59)),
    ("Intercom", "intercom", held!("connectors/intercom.svg"), None),
    ("Okta", "okta", held!("connectors/okta.svg"), Some(0x007DC1)),
    (
        "PagerDuty",
        "pagerduty",
        held!("connectors/pagerduty.svg"),
        Some(0x06AC38),
    ),
    ("Postgres", "postgres", held!("connectors/postgres.svg"), Some(0x4169E1)),
    (
        "QuickBooks",
        "quickbooks",
        held!("connectors/quickbooks.svg"),
        Some(0x2CA01C),
    ),
    ("Shopify", "shopify", held!("connectors/shopify.svg"), Some(0x7AB55C)),
    (
        "Snowflake",
        "snowflake",
        held!("connectors/snowflake.svg"),
        Some(0x29B5E8),
    ),
    ("Zendesk", "zendesk", held!("connectors/zendesk.svg"), None),
    ("Zoom", "zoom", held!("connectors/zoom.svg"), Some(0x0B5CFF)),
];

pub fn connector(name: &str) -> Option<(SharedString, Option<gpui_kit::Hsla>)> {
    CONNECTORS
        .iter()
        .find(|(app, _, _, _)| *app == name)
        .map(connector_asset)
}

/// A service's mark by a name a person typed (an MCP server's `linear`, `GitHub`): its display name or its asset id,
/// any case. `connector` stays exact, for the names the app itself writes.
pub fn connector_named(name: &str) -> Option<(SharedString, Option<gpui_kit::Hsla>)> {
    let name = name.trim();
    CONNECTORS
        .iter()
        .find(|(app, id, _, _)| app.eq_ignore_ascii_case(name) || id.eq_ignore_ascii_case(name))
        .map(connector_asset)
}

fn connector_asset((_, id, _, color): &(&str, &str, &[u8], Option<u32>)) -> (SharedString, Option<gpui_kit::Hsla>) {
    (
        format!("connectors/{id}.svg").into(),
        color.map(|color| gpui_kit::rgb(color).into()),
    )
}

/// A maker's brand colour (simple-icons' `hex`), as `model-mark.ts` fills it; `None` draws the mark in the strongest
/// foreground (OpenAI, Z.ai and Nexus are black or white marks).
pub fn mark_color(maker: &str) -> Option<gpui_kit::Hsla> {
    let hex = match maker {
        "anthropic" => 0xD97757,
        "deepseek" => 0x5786FE,
        "google" => 0x8E75B2,
        "meta" => 0x0467DF,
        "mistral" => 0xFA520F,
        "qwen" | "alibaba" => 0x6950EF,
        "nvidia" => 0x76B900,
        _ => return None,
    };
    Some(gpui_kit::rgb(hex).into())
}

/// The asset path of a maker's mark, when the app carries one.
pub fn mark(maker: &str) -> Option<SharedString> {
    MARKS
        .iter()
        .any(|(name, _)| *name == maker)
        .then(|| format!("marks/{maker}.svg").into())
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == WORDMARK {
            return Ok(Some(Cow::Borrowed(held!("brand/nexus-wordmark.svg"))));
        }
        if path == APP_MARK {
            return Ok(Some(Cow::Borrowed(held!("brand/app-mark.svg"))));
        }
        if path == APP_ICON {
            return Ok(Some(Cow::Borrowed(held!("brand/app-icon.svg"))));
        }
        if path == PANEL_GRIP {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/panel/grip.svg"))));
        }
        if path == STOP_ARC {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/composer/stop-arc.svg"))));
        }
        if path == ORBIT_RING {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/sidebar/orbit-ring.svg"))));
        }
        if path == ORBIT_DOT {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/sidebar/orbit-dot.svg"))));
        }
        if let Some(index) = path
            .strip_prefix(MARK_LAYER)
            .and_then(|file| file.strip_suffix(".svg"))
            .and_then(|index| index.parse::<usize>().ok())
        {
            return Ok(mark_layers().get(index).map(|layer| Cow::Borrowed(layer.as_slice())));
        }
        if path.starts_with("file_icons/") {
            return Ok(crate::file_icons::load(path));
        }
        if let Some(name) = path.strip_prefix("marks/").and_then(|file| file.strip_suffix(".svg")) {
            return Ok(MARKS
                .iter()
                .find(|(mark, _)| *mark == name)
                .map(|(_, bytes)| Cow::Borrowed(*bytes)));
        }
        if let Some(name) = path
            .strip_prefix("connectors/")
            .and_then(|file| file.strip_suffix(".svg"))
        {
            return Ok(CONNECTORS
                .iter()
                .find(|(_, id, _, _)| *id == name)
                .map(|(_, _, bytes, _)| Cow::Borrowed(*bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(nexus_marks_held, ignore = "the app mark is a stand-in in this tree")]
    fn the_mark_cuts_into_its_eleven_layers_each_in_the_marks_frame() {
        let layers = mark_layers();
        assert_eq!(layers.len(), 11, "the mark's 11-layer cut");
        for (index, layer) in layers.iter().enumerate() {
            let svg = std::str::from_utf8(layer).unwrap();
            assert!(
                svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"),
                "layer {index}: {svg:.80}"
            );
            assert!(svg.contains(r#"viewBox="-84 -84 168 168""#) && svg.contains("scale(0.8853)"));
            assert_eq!(svg.matches("<path").count(), 1);
            let loaded = Assets.load(&mark_layer(index)).unwrap().unwrap();
            assert_eq!(&*loaded, layer.as_slice());
        }
        // The rim is the thinnest edge, drawn first; nothing past the last layer loads.
        assert!(
            std::str::from_utf8(&layers[0])
                .unwrap()
                .contains(r#"stroke-width="0.59""#)
        );
        assert!(Assets.load(&mark_layer(11)).unwrap().is_none());
    }
}
