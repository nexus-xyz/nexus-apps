//! The chat composer's choices in Nexus GPUI: the team the spend goes to, whether a chat may search the web, and how
//! an answer's web sources read. Nexus UI holds the same rules in TypeScript (`TeamPicker` in `chat/composer.tsx`, and
//! `chat/web-search.ts`). The tests are `web-search.test.ts`'s cases.

use serde_json::Value;

/// A team the person is in (`GET /v1/tenants/me`'s `teams[]`): where a chat's spend may go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub id: String,
    pub slug: String,
    pub name: String,
}

impl Team {
    fn from_json(team: &Value) -> Option<Team> {
        let field = |name: &str| team.get(name).and_then(Value::as_str).map(String::from);
        Some(Team {
            id: field("id")?,
            slug: field("slug").filter(|slug| !slug.is_empty())?,
            name: field("name")?,
        })
    }
}

/// The teams of `GET /v1/tenants/me` (`teams`), in its order; none when it lists none.
pub fn teams(me: &Value) -> Vec<Team> {
    me.get("teams")
        .and_then(Value::as_array)
        .map(|teams| teams.iter().filter_map(Team::from_json).collect())
        .unwrap_or_default()
}

/// The person's default team's slug (`GET /v1/tenants/me`'s `team`): where spend goes when a call names none.
pub fn default_team(me: &Value) -> Option<String> {
    me.pointer("/team/slug")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .map(String::from)
}

/// The team chip's team, as `TeamPicker`: the chosen one, else the default, else none.
pub fn current_team<'a>(teams: &'a [Team], chosen: &str, default: Option<&str>) -> Option<&'a Team> {
    teams
        .iter()
        .find(|team| team.slug == chosen)
        .or_else(|| teams.iter().find(|team| Some(team.slug.as_str()) == default))
}

/// Gateway's offer rule (`webSearchOffered`): the organization's `webSearch.engine` is `borrowed`, and nothing holds
/// search back ([`web_search_held_back`]). `settings` is `GET /v1/orgs/settings`.
pub fn web_search_offered(settings: &Value) -> bool {
    settings.get("webSearch.engine").and_then(Value::as_str) == Some("borrowed") && !web_search_held_back(settings)
}

/// Whether zero data retention or a domain list is set (`webSearchHeldBack`): Gateway cannot apply them yet, so no
/// search runs.
pub fn web_search_held_back(settings: &Value) -> bool {
    let listed = |name: &str| {
        settings
            .pointer(&format!("/webSearch.domains/{name}"))
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
    };
    settings.get("webSearch.requireZdr").and_then(Value::as_bool) == Some(true) || listed("allow") || listed("deny")
}

/// The new web search flag, if it changes after the organization's settings load.
pub fn web_search_sync(current: Option<bool>, settings: Option<&Value>) -> Option<bool> {
    let settings = settings?;
    let want = web_search_offered(settings);
    (current.unwrap_or(false) != want).then_some(want)
}

/// A web page an answer's `web_search` read (`ChatWebSource`), and the first query that found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSource {
    pub title: String,
    pub url: String,
    /// As the engine gave it.
    pub date: Option<String>,
    pub query: String,
}

impl WebSource {
    fn from_json(source: &Value) -> Option<WebSource> {
        let field = |name: &str| source.get(name).and_then(Value::as_str).map(String::from);
        Some(WebSource {
            title: field("title").unwrap_or_default(),
            url: field("url")?,
            date: field("date").filter(|date| !date.is_empty()),
            query: field("query").unwrap_or_default(),
        })
    }

    /// The link, only for an http or https URL ([`source_href`]).
    pub fn href(&self) -> Option<String> {
        source_href(&self.url)
    }

    /// The quiet line under the title ([`source_facts`]).
    pub fn facts(&self) -> String {
        source_facts(&self.url, self.date.as_deref())
    }

    /// The words of the link: the title, else the link, else the URL as it came (`WebSources`).
    pub fn label(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else {
            self.href().unwrap_or_else(|| self.url.clone())
        }
    }

    /// The site it is on ([`source_host`]).
    pub fn host(&self) -> String {
        source_host(&self.url)
    }

    /// The letter on its tile: the host's first letter or digit, upper case. The app draws it rather than fetch the
    /// site's icon, so no site learns that someone read the answer.
    pub fn tile(&self) -> char {
        self.host()
            .chars()
            .find(|c| c.is_alphanumeric())
            .map_or('#', |c| c.to_uppercase().next().unwrap_or(c))
    }
}

/// The tiles an answer's sources chip shows: the first `max` sites, each once, in the answer's order.
pub fn source_tiles(sources: &[WebSource], max: usize) -> Vec<char> {
    let mut hosts: Vec<String> = Vec::new();
    let mut tiles = Vec::new();
    for source in sources {
        if tiles.len() == max {
            break;
        }
        let host = source.host();
        if !hosts.contains(&host) {
            tiles.push(source.tile());
            hosts.push(host);
        }
    }
    tiles
}

/// The chip's words: "1 source", "3 sources".
pub fn sources_label(count: usize) -> String {
    if count == 1 {
        "1 source".into()
    } else {
        format!("{count} sources")
    }
}

/// A chat message's web sources (`sources`), in its order; none for an answer that ran no search.
pub fn web_sources(message: &Value) -> Vec<WebSource> {
    message
        .get("sources")
        .and_then(Value::as_array)
        .map(|sources| sources.iter().filter_map(WebSource::from_json).collect())
        .unwrap_or_default()
}

/// The link for a source, only for an http or https URL (`sourceHref`): a search result's URL comes from the open web.
pub fn source_href(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())
}

/// A source's site: its URL's host without `www.`. A URL that does not parse stands for itself.
pub fn source_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .host_str()
                .map(|host| host.strip_prefix("www.").unwrap_or(host).to_string())
        })
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| url.to_string())
}

/// The quiet line under a source's title (`sourceFacts`): its host ([`source_host`]), and its date when the engine
/// gave one.
pub fn source_facts(url: &str, date: Option<&str>) -> String {
    let host = source_host(url);
    match date {
        Some(date) => format!("{host} · {date}"),
        None => host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offers_web_search_when_the_engine_is_borrowed() {
        assert!(web_search_offered(&json!({ "webSearch.engine": "borrowed" })));
        assert!(web_search_offered(&json!({
            "webSearch.engine": "borrowed",
            "webSearch.domains": { "allow": [], "deny": [] },
            "webSearch.requireZdr": false,
        })));
    }

    #[test]
    fn does_not_offer_it_when_the_engine_is_off_or_the_setting_is_missing() {
        assert!(!web_search_offered(&json!({ "webSearch.engine": "off" })));
        assert!(!web_search_offered(&json!({})));
    }

    #[test]
    fn does_not_offer_it_while_an_allow_or_deny_domain_list_is_set() {
        assert!(!web_search_offered(&json!({
            "webSearch.engine": "borrowed",
            "webSearch.domains": { "allow": ["nexus.xyz"], "deny": [] },
        })));
        assert!(!web_search_offered(&json!({
            "webSearch.engine": "borrowed",
            "webSearch.domains": { "allow": [], "deny": ["example.com"] },
        })));
    }

    #[test]
    fn does_not_offer_it_while_require_zdr_is_set() {
        assert!(!web_search_offered(
            &json!({ "webSearch.engine": "borrowed", "webSearch.requireZdr": true })
        ));
    }

    #[test]
    fn labels_a_source_with_its_host_and_date() {
        assert_eq!(
            source_facts("https://docs.nexus.xyz/releases", Some("2026-10-20")),
            "docs.nexus.xyz · 2026-10-20"
        );
        assert_eq!(source_facts("https://docs.nexus.xyz/releases", None), "docs.nexus.xyz");
        assert_eq!(source_facts("https://www.example.com/a", None), "example.com");
        assert_eq!(source_facts("not a url", Some("2026-10-20")), "not a url · 2026-10-20");
    }

    #[test]
    fn links_only_an_http_or_https_url() {
        assert_eq!(
            source_href("https://example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            source_href("http://example.com/a").as_deref(),
            Some("http://example.com/a")
        );
        for unsafe_url in [
            "javascript:alert(1)",
            "not a url",
            "JAVASCRIPT:alert(1)",
            " javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "//example.com/a",
        ] {
            assert_eq!(source_href(unsafe_url), None, "{unsafe_url}");
        }
    }

    #[test]
    fn web_search_sync_turns_the_flag_on_in_chat_when_the_organization_offers_search() {
        let offered = json!({ "webSearch.engine": "borrowed" });
        assert_eq!(web_search_sync(None, Some(&offered)), Some(true));
        assert_eq!(web_search_sync(Some(false), Some(&offered)), Some(true));
        assert_eq!(web_search_sync(Some(true), Some(&offered)), None);
    }

    #[test]
    fn web_search_sync_follows_the_organization_and_waits_for_the_settings() {
        let off = json!({ "webSearch.engine": "off" });
        assert_eq!(web_search_sync(Some(true), Some(&off)), Some(false));
        assert_eq!(web_search_sync(None, Some(&off)), None);
        assert_eq!(web_search_sync(Some(true), None), None);
    }

    #[test]
    fn reads_an_answers_web_sources() {
        let message = json!({ "sources": [
            { "title": "Release notes", "url": "https://docs.nexus.xyz/releases", "date": "2026-10-20", "query": "nexus release" },
            { "title": "", "url": "javascript:alert(1)", "query": "x" },
            { "title": "No url" },
        ] });
        let sources = web_sources(&message);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].label(), "Release notes");
        assert_eq!(sources[0].facts(), "docs.nexus.xyz · 2026-10-20");
        assert_eq!(sources[1].href(), None);
        assert_eq!(sources[1].label(), "javascript:alert(1)");
        assert!(web_sources(&json!({})).is_empty());
    }

    fn source(url: &str) -> WebSource {
        WebSource {
            title: String::new(),
            url: url.into(),
            date: None,
            query: String::new(),
        }
    }

    #[test]
    fn a_sources_tile_is_its_sites_first_letter() {
        assert_eq!(source("https://www.github.com/a").tile(), 'G');
        assert_eq!(source("https://cseweb.ucsd.edu/k-sat").tile(), 'C');
        assert_eq!(source("https://9to5mac.com/").tile(), '9');
        assert_eq!(source("https://www.github.com/a").host(), "github.com");
        assert_eq!(source("").tile(), '#', "no host, no letter");
    }

    #[test]
    fn the_chip_shows_each_site_once_and_at_most_three() {
        let sources = [
            source("https://github.com/a"),
            source("https://www.github.com/b"),
            source("https://cseweb.ucsd.edu/"),
            source("https://en.wikipedia.org/wiki/ETH"),
            source("https://arxiv.org/abs/1"),
        ];
        assert_eq!(source_tiles(&sources, 3), vec!['G', 'C', 'E']);
        assert_eq!(source_tiles(&sources[..1], 3), vec!['G']);
        assert!(source_tiles(&[], 3).is_empty());
        assert_eq!(sources_label(1), "1 source");
        assert_eq!(sources_label(5), "5 sources");
    }

    #[test]
    fn the_team_chip_shows_the_chosen_team_else_the_default() {
        let me = json!({
            "teams": [
                { "id": "1", "slug": "platform", "name": "Platform", "role": "member" },
                { "id": "2", "slug": "growth", "name": "Growth", "role": "lead" },
                { "id": "3", "name": "No slug" },
            ],
            "team": { "id": "2", "slug": "growth", "name": "Growth" },
        });
        let teams = teams(&me);
        assert_eq!(teams.len(), 2);
        let default = default_team(&me);
        assert_eq!(default.as_deref(), Some("growth"));
        assert_eq!(
            current_team(&teams, "platform", default.as_deref()).map(|team| team.name.as_str()),
            Some("Platform")
        );
        assert_eq!(
            current_team(&teams, "", default.as_deref()).map(|team| team.name.as_str()),
            Some("Growth")
        );
        assert_eq!(current_team(&teams, "gone", None), None);
        assert!(super::teams(&json!({})).is_empty());
        assert_eq!(default_team(&json!({})), None);
    }
}
