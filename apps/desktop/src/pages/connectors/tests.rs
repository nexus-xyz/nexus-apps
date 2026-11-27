use super::*;
use core::prelude::v1::test;

fn catalog() -> Value {
    json!({ "connectors": [
        {
            "name": "jira",
            "description": "Read-only access to Jira: issues, comments and projects; search with JQL.",
            "operations": [],
            "offered": true,
            "usable": true,
            "connects": ["user", "org"],
            "form": {
                "required": ["site", "token"],
                "properties": {
                    "site": { "description": "Your Jira address", "examples": ["acme.atlassian.net"] },
                    "token": { "x-nexus-secret": true },
                    "user_email": { "description": "The account's email" }
                }
            }
        },
        { "name": "notion", "description": "Pages.", "connects": ["user"], "form": null, "oauth": true, "usable": null },
        { "name": "github", "description": "Its repositories.", "connects": [], "usable": false },
        { "name": "old-thing", "description": "Old.", "connects": ["user"], "offered": false },
        { "description": "no name, skipped" }
    ]})
}

#[test]
fn failed_refresh_retains_snapshot_and_retry_clears_error() {
    let mut current = Load::Ready(vec!["connected"]);
    let mut errors = HashMap::new();
    refreshed(&mut current, Err("offline".into()), &mut errors, "connections");
    assert!(matches!(&current, Load::Ready(values) if values == &vec!["connected"]));
    assert_eq!(errors.get("connections").map(String::as_str), Some("offline"));
    refreshed(&mut current, Ok(vec!["refreshed"]), &mut errors, "connections");
    assert!(errors.is_empty());
    assert!(matches!(&current, Load::Ready(values) if values == &vec!["refreshed"]));
    let mut initial: Load<Vec<String>> = Load::Loading;
    refreshed(&mut initial, Err("offline".into()), &mut errors, "catalog");
    assert!(matches!(initial, Load::Failed(_)));
}

#[test]
fn reads_the_catalogue_defensively() {
    let connectors = parse_connectors(&catalog());
    assert_eq!(connectors.len(), 4);
    let jira = &connectors[0];
    assert!(jira.keyed && !jira.oauth && jira.offered);
    assert_eq!(jira.usable, Access::Usable);
    let site = jira.fields.iter().find(|field| field.name == "site").unwrap();
    assert!(site.required && !site.secret);
    assert_eq!(site.placeholder.as_deref(), Some("acme.atlassian.net"));
    let token = jira.fields.iter().find(|field| field.name == "token").unwrap();
    assert!(token.required && token.secret);
    let email = jira.fields.iter().find(|field| field.name == "user_email").unwrap();
    assert_eq!(email.label, "User email");
    assert!(!email.required);
    assert!(!connectors[1].keyed && connectors[1].oauth);
    assert_eq!(connectors[1].usable, Access::Unknown);
    assert_eq!(connectors[2].usable, Access::Unavailable);
    assert!(!connectors[3].offered);
    assert!(parse_connectors(&json!({ "connectors": "nope" })).is_empty());
}

#[test]
fn reads_connections_availability_and_rights() {
    let connections = parse_connections(&json!({ "connections": [
        { "connector": "jira", "scope": "org", "account": "a1", "name": "acme", "status": "needs_reconnect",
          "kind": "api_key", "last4": "9f2c", "createdAt": "2026-11-01T09:00:00Z" },
        { "connector": "jira", "scope": "user", "account": "a2", "status": "active", "createdAt": "2026-11-02T09:00:00Z" },
        { "connector": "jira", "scope": "user" }
    ]}));
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[0].scope, "org");
    assert!(connections[0].needs_reconnect);
    assert_eq!(connections[1].name, "a2");

    let availability = parse_availability(&json!({ "connectors": [
        { "name": "jira", "availability": "available", "teams": ["web"], "default": false },
        { "name": "notion", "availability": "enabled_for_all", "teams": null, "default": true },
        { "name": "bad", "availability": "sometimes" }
    ]}));
    assert_eq!(availability.len(), 2);
    assert_eq!(
        availability_status(&availability[0]).label,
        "The web team connects its own"
    );
    assert_eq!(availability_status(&availability[1]).label, "On for everyone");

    assert_eq!(parse_chat(&json!({ "enabled": true, "operations": [] })), Some(true));
    assert_eq!(parse_chat(&json!({})), None);
    assert_eq!(
        parse_rights(&json!({ "permissions": ["policy.manage", "connections.manage_own"] })),
        Rights {
            manage: true,
            own: true
        }
    );
    assert_eq!(parse_rights(&json!({})), Rights::default());
}

#[test]
fn groups_offered_tools_by_access_and_keeps_account_management() {
    let connectors = parse_connectors(&catalog());
    let member = Rights {
        manage: false,
        own: true,
    };
    let groups = group_cards(&connectors, &[], None, member);
    let ids: Vec<Group> = groups.iter().map(|(group, _)| *group).collect();
    // GitHub stays visible even with no account form. Unoffered tools without accounts stay hidden.
    // Notion's unknown access is neither permission nor a policy refusal.
    assert_eq!(ids, vec![Group::Yours, Group::Unavailable, Group::Unknown]);
    assert_eq!(groups[0].1[0].name, "Jira");
    assert_eq!(groups[0].1[0].statuses[0].label, "You're not connected");
    assert_eq!(groups[1].1[0].name, "GitHub");
    assert!(groups[1].1[0].statuses.is_empty());
    assert_eq!(groups[2].1[0].name, "Notion");
    assert_eq!(groups[2].1[0].statuses[0].label, "Couldn't check your access");

    let admin = Rights {
        manage: true,
        own: true,
    };
    let chosen = vec![Availability {
        name: "jira".into(),
        level: "disabled".into(),
        teams: vec![],
        default: true,
    }];
    let groups = group_cards(&connectors, &[], Some(&chosen), admin);
    let ids: Vec<Group> = groups.iter().map(|(group, _)| *group).collect();
    assert_eq!(
        ids,
        vec![Group::Off, Group::Unavailable, Group::Unknown, Group::Preview]
    );
    // "Disabled" is said by the Off heading, and the account line adds nothing under it.
    assert!(groups[0].1[0].statuses.is_empty());
}

#[test]
fn gateway_managed_tools_are_visible_without_account_permissions() {
    let connectors = parse_connectors(&json!({ "connectors": [{
        "name": "github", "offered": true, "usable": true, "connects": [],
        "operations": [
            { "name": "pulls", "summary": "List a repository's pull requests." },
            { "name": "issues" },
            { "summary": "Missing name" }
        ]
    }] }));
    let groups = group_cards(&connectors, &[], None, Rights::default());
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].0, Group::Yours);
    let card = &groups[0].1[0];
    assert_eq!(card.name, "GitHub");
    assert_eq!(card.statuses, vec![status("Available through Nexus", Tone::On)]);
    assert_eq!(card.connector.operations.len(), 2);
    assert_eq!(card.connector.operations[0].1, "List a repository's pull requests.");
    assert!(!card.connector.keyed && !card.connector.oauth);
}

#[test]
fn uncertain_access_never_looks_available() {
    for usable in [Value::Null, json!("true"), json!(1)] {
        let connectors = parse_connectors(&json!({ "connectors": [{
            "name": "github", "offered": true, "usable": usable, "connects": []
        }] }));
        let groups = group_cards(&connectors, &[], None, Rights::default());
        assert_eq!(groups[0].0, Group::Unknown);
        assert_eq!(
            groups[0].1[0].statuses,
            vec![status("Couldn't check your access", Tone::Off)]
        );
    }
}

#[test]
fn a_saved_account_does_not_override_refused_or_unknown_access() {
    let connections = parse_connections(&json!({ "connections": [{
        "connector": "linear", "scope": "user", "account": "a1", "status": "active"
    }] }));
    for (usable, group) in [(json!(false), Group::Unavailable), (Value::Null, Group::Unknown)] {
        let connectors = parse_connectors(&json!({ "connectors": [{
            "name": "linear", "offered": true, "usable": usable, "connects": ["user", "org"]
        }] }));
        let groups = group_cards(
            &connectors,
            &connections,
            None,
            Rights {
                manage: false,
                own: true,
            },
        );
        assert_eq!(groups[0].0, group);
        assert_eq!(groups[0].1[0].mine, connections);
    }
    let connectors = parse_connectors(&json!({ "connectors": [{
        "name": "linear", "offered": false, "usable": false, "connects": ["user", "org"]
    }] }));
    let groups = group_cards(
        &connectors,
        &connections,
        None,
        Rights {
            manage: false,
            own: true,
        },
    );
    assert_eq!(groups[0].0, Group::Preview);
    assert_eq!(groups[0].1[0].mine, connections);
}

#[test]
fn a_row_says_one_status_or_offers_connect() {
    let member = Rights {
        manage: false,
        own: true,
    };
    let mut connectors = parse_connectors(&catalog());
    // Notion reads as usable here, so its row can offer Connect.
    connectors[1].usable = Access::Usable;
    let jira = connectors[0].clone();
    let notion = connectors[1].clone();
    let account = |connector: &str, account: &str, reconnect: bool| Connection {
        connector: connector.into(),
        scope: "user".into(),
        account: account.into(),
        name: account.into(),
        identity: None,
        needs_reconnect: reconnect,
        suspended: false,
        last4: None,
        created_at: String::new(),
    };
    let label = |card: &Card| row_status(card, member).map(|state| (state.label, state.tone));

    // A key form needs the open row, so Jira says Not connected; Notion connects in the browser from its row.
    let bare = card(&jira, &[], None, member);
    assert_eq!(label(&bare), Some(("Not connected".into(), Tone::Off)));
    assert!(!quick_connect(&bare, member));
    let oauth = card(&notion, &[], None, member);
    assert_eq!(label(&oauth), None);
    assert!(quick_connect(&oauth, member));
    // OAuth wins over a key form: Linear keeps its key form as a fallback, yet its row connects in the browser.
    let both = Connector {
        oauth: true,
        ..jira.clone()
    };
    let both = card(&both, &[], None, member);
    assert_eq!(label(&both), None);
    assert!(quick_connect(&both, member));

    let one = card(&jira, &[account("jira", "a1", false)], None, member);
    assert_eq!(label(&one), Some(("Connected".into(), Tone::On)));
    let two = card(
        &jira,
        &[account("jira", "a1", false), account("jira", "a2", false)],
        None,
        member,
    );
    assert_eq!(label(&two), Some(("2 accounts".into(), Tone::On)));
    let stale = card(&notion, &[account("notion", "a1", true)], None, member);
    assert_eq!(label(&stale), Some(("Reconnect needed".into(), Tone::Warn)));
    assert!(!quick_connect(&stale, member));

    // An admin's row says who may use it; Disabled is the Off group's heading, so it says nothing.
    let admin = Rights {
        manage: true,
        own: true,
    };
    let chosen = |level: &str| Availability {
        name: "jira".into(),
        level: level.into(),
        teams: vec![],
        default: false,
    };
    let everyone = chosen("enabled_for_all");
    let for_all = card(&jira, &[], Some(&everyone), admin);
    assert_eq!(
        row_status(&for_all, admin).map(|state| state.label),
        Some("On for everyone".into())
    );
    assert_eq!(
        org_status(&for_all).map(|state| state.label),
        Some("On for everyone".into())
    );
    let disabled = chosen("disabled");
    assert_eq!(row_status(&card(&jira, &[], Some(&disabled), admin), admin), None);

    // Preview rows and rows the group heading explains stay quiet.
    assert_eq!(label(&card(&connectors[3], &[], None, member)), None);
    assert_eq!(label(&card(&connectors[2], &[], None, member)), None);
}

#[test]
fn the_filter_matches_a_name_an_id_or_a_line() {
    let member = Rights {
        manage: false,
        own: true,
    };
    let connectors = parse_connectors(&catalog());
    let jira = card(&connectors[0], &[], None, member);
    assert!(matches_filter(&jira, ""));
    assert!(matches_filter(&jira, "  JI "));
    assert!(matches_filter(&jira, "comments"));
    assert!(!matches_filter(&jira, "notion"));
    assert!(Group::Off.folds() && Group::Preview.folds() && !Group::Yours.folds());
}

#[test]
fn every_gateway_connector_has_a_logo_but_three() {
    // The Gateway's connectors, by their catalogue ids.
    let ids = [
        "airtable",
        "asana",
        "bitbucket",
        "cloudflare",
        "confluence",
        "datadog",
        "dropbox",
        "figma",
        "github",
        "gitlab",
        "gmail",
        "google-calendar",
        "google-drive",
        "google-sheets",
        "hubspot",
        "intercom",
        "jira",
        "linear",
        "notion",
        "okta",
        "pagerduty",
        "postgres",
        "quickbooks",
        "sentry",
        "shopify",
        "slack",
        "snowflake",
        "stripe",
        "zendesk",
        "zoom",
    ];
    for id in ids {
        assert!(
            crate::assets::connector(&connector_name(id)).is_some(),
            "{id} reads as {} and has no mark",
            connector_name(id)
        );
    }
    // simple-icons carries no mark for these: their rows keep a letter tile.
    for id in ["salesforce", "teams", "outlook"] {
        assert!(crate::assets::connector(&connector_name(id)).is_none(), "{id}");
    }
}

#[test]
fn short_lines_names_and_refusals() {
    assert_eq!(
        short_line("Read-only access to Jira: issues, comments and projects; search with JQL."),
        "Issues, comments and projects"
    );
    assert_eq!(short_line("Its repositories."), "Repositories");
    assert_eq!(
        short_line("Read-only: metadata, file tree and file contents, pull requests and their reviews, issues"),
        "Metadata, file tree and file contents"
    );
    assert_eq!(connector_name("google-drive"), "Google Drive");
    assert_eq!(connector_name("acme_wiki-tool"), "Acme Wiki Tool");
    assert_eq!(segment("git/hub"), "git%2Fhub");
    assert_eq!(team_list(" web, ,data "), vec!["web", "data"]);
    assert_eq!(
        plain("only the org's owners or admins can change connectors (policy.manage)"),
        ADMINS_ONLY
    );
    assert_eq!(plain("Gateway is down"), "Gateway is down");
}

#[test]
fn a_persons_page_groups_by_what_they_do_next_and_never_shows_the_organizations_accounts() {
    let member = Rights {
        manage: false,
        own: true,
    };
    let mut connectors = parse_connectors(&catalog());
    connectors[1].usable = Access::Usable;
    connectors.push(
        parse_connectors(&json!({ "connectors": [
            { "name": "linear", "connects": ["user", "org"], "usable": true, "oauth": true }
        ]}))[0]
            .clone(),
    );
    let connections = parse_connections(&json!({ "connections": [
        { "connector": "linear", "scope": "user", "account": "mine", "status": "needs_reconnect" },
        { "connector": "jira", "scope": "org", "account": "acme" },
        { "connector": "old-thing", "scope": "user", "account": "kept" }
    ]}));
    let shelves = mine_shelves(&connectors, &connections, member);
    let named: Vec<(Shelf, Vec<&str>)> = shelves
        .iter()
        .map(|(shelf, cards)| (*shelf, cards.iter().map(|card| card.name.as_str()).collect()))
        .collect();
    assert_eq!(
        named,
        vec![
            (Shelf::Connected, vec!["Linear"]),
            (Shelf::Ready, vec!["Jira", "Notion"]),
            (Shelf::Unavailable, vec!["GitHub"]),
            (Shelf::Preview, vec!["Old Thing"]),
        ]
    );
    // Jira's organization account is the organization's page's, not the person's.
    let jira = &shelves[1].1[0];
    assert!(jira.org.is_empty() && jira.mine.is_empty());
    // A person who may not connect their own sees a usable connector as served by the organization.
    let reader = Rights::default();
    let shelves = mine_shelves(&connectors[..1], &[], reader);
    assert_eq!(shelves[0].0, Shelf::Shared);
    assert!(Shelf::Shared.folds() && Shelf::Unavailable.folds() && !Shelf::Ready.folds());
}

#[test]
fn abilities_read_as_reads_and_changes_in_plain_words() {
    let abilities = parse_abilities(&json!({ "operations": [
        { "name": "repo", "summary": "Repository metadata.", "effect": "read" },
        { "name": "pull_merge", "effect": "mutate",
          "summary": "Write (needs the pull_merge_grant, which no other write includes): squash-merge a pull request." },
        { "name": "resolve_thread", "effect": "mutate", "summary": "Write (needs a write grant): resolve a thread." },
        { "name": "no_summary" },
        { "summary": "no name, skipped" }
    ]}));
    assert_eq!(abilities.len(), 4);
    assert!(!abilities[0].changes && abilities[1].changes && abilities[2].changes && !abilities[3].changes);
    assert_eq!(abilities[1].text, "Squash-merge a pull request.");
    assert_eq!(abilities[2].text, "Resolve a thread.");
    assert_eq!(abilities[3].text, "no summary");
    assert_eq!(abilities_summary(&abilities), "2 to read, 2 to change.");
    assert_eq!(abilities_summary(&abilities[..1]), "1 to read. It changes nothing.");
    assert_eq!(plain_summary("Writes the file."), "Writes the file.");
}

#[test]
fn each_answer_says_what_it_means() {
    let chosen = |level: &str, default: bool| Availability {
        name: "github".into(),
        level: level.into(),
        teams: vec![],
        default,
    };
    assert_eq!(LEVELS.map(|(_, label)| label), ["Off", "Their own account", "Everyone"]);
    assert_eq!(
        level_consequence(&chosen("enabled_for_all", false), "Acme"),
        "Everyone can use it. People without their own account, and runs for no person, use Acme's account."
    );
    assert!(level_consequence(&chosen("disabled", true), "Acme").starts_with("Nobody chose yet"));
    assert_eq!(
        availability_status(&chosen("available", false)).label,
        "People connect their own"
    );
}

#[test]
fn a_setup_line_waits_for_a_usable_account_and_its_oauth_page() {
    let answer = json!({ "connections": [
        { "connector": "linear", "account": "a1", "status": "needs_reconnect" },
        { "connector": "jira", "account": "j1" },
    ]});
    assert_eq!(usable_accounts(&answer, "linear"), 0, "one to reconnect does not count");
    let answer = json!({ "connections": [{ "connector": "linear", "account": "a2", "scope": "user" }]});
    assert_eq!(usable_accounts(&answer, "linear"), 1);
    assert_eq!(
        authorize_url(Ok(json!({ "authorizeUrl": "https://linear.app/oauth/authorize?x=1" }))),
        Ok("https://linear.app/oauth/authorize?x=1".to_string())
    );
    assert!(authorize_url(Ok(json!({}))).is_err());
    assert_eq!(row_anchor("linear"), Some("connector-linear"));
}
