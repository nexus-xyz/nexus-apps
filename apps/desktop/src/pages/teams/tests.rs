use super::*;
// The glob import of gpui_kit brings its own `test` macro: name the built-in one.
use core::prelude::v1::test;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("a JSON sample")
}

fn tree() -> Value {
    parse(
        r#"{
        "default": "web",
        "tree": [
            {
                "id": "t_eng", "slug": "eng", "name": "Engineering", "level": "department", "depth": 1,
                "createdAt": "2026-01-01T00:00:00Z", "members": 2, "pending": 1, "myRole": "member",
                "leads": [{ "userId": "u_1", "name": "Ada" }, { "email": "new@acme.com", "pending": true }],
                "headcount": 5,
                "children": [
                    { "id": "t_web", "slug": "web", "name": "Web", "depth": 2, "createdAt": "2026-01-02T00:00:00Z",
                      "members": 2, "pending": 0, "leads": [], "headcount": 2, "myRole": "lead", "isDefault": true, "children": [] },
                    { "id": "t_api", "slug": "api", "name": "API", "depth": 2, "createdAt": "2026-01-02T00:00:00Z",
                      "members": 1, "pending": 0, "leads": [], "headcount": 1, "children": [
                        { "id": "t_edge", "slug": "edge", "name": "Edge", "depth": 3, "createdAt": "2026-01-03T00:00:00Z",
                          "members": 1, "pending": 0, "leads": [], "headcount": 1, "children": [] }
                      ] }
                ]
            },
            { "id": "t_ops", "slug": "ops", "name": "Operations", "depth": 1, "createdAt": "2026-01-01T00:00:00Z",
              "members": 1, "pending": 0, "leads": [], "headcount": 1, "children": [] }
        ]
    }"#,
    )
}

#[test]
fn reads_a_team_s_spend() {
    let spend = super::parse_spend(&serde_json::json!({
        "totals": { "spentUsd": 12.5, "spentUsdDecimal": "12.504", "calls": 1204 },
        "groups": [ { "key": "usr_1", "label": "Ana", "spentUsd": 0.001 }, { "key": "usr_2", "spentUsd": 3 } ]
    }));
    assert_eq!(spend.spent, Some(12.504), "the decimal twin first");
    assert_eq!(spend.calls, 1204);
    assert_eq!(super::spent_text(spend.people[0].1), "<$0.01");
    assert_eq!(super::spent_text(spend.people[1].1), "$3.00");
}

/// Each block as text: a band's slug, or a card's lines as `kind:slug`, so a test reads the shape at a glance.
fn shape(rows: &[TreeRow], blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .map(|block| match block {
            Block::Band(line) => format!("band {}", rows[line.row].slug),
            Block::Card(lines) => lines
                .iter()
                .map(|line| format!("{:?}:{}", line.kind, rows[line.row].slug))
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect()
}

#[test]
fn a_top_level_team_is_a_band_and_each_team_under_it_a_card() {
    let rows = parse_tree(&tree());
    let blocks = layout(&rows, &HashSet::new());
    assert_eq!(
        shape(&rows, &blocks),
        ["band eng", "Head:api Area:edge", "Area:web", "Area:ops"],
        "a team with nothing under it shares a card with its siblings; it opens no band of its own"
    );
    let Block::Band(band) = &blocks[0] else {
        panic!("a band first")
    };
    assert_eq!((band.below, band.fold), (3, Some(true)));
}

#[test]
fn a_folded_team_hides_the_teams_under_it() {
    let rows = parse_tree(&tree());
    let folded = HashSet::from(["api".to_string()]);
    assert_eq!(
        shape(&rows, &layout(&rows, &folded)),
        ["band eng", "Head:api", "Area:web", "Area:ops"]
    );
    let folded = HashSet::from(["eng".to_string()]);
    let blocks = layout(&rows, &folded);
    assert_eq!(shape(&rows, &blocks), ["band eng", "Area:ops"]);
    let Block::Band(band) = &blocks[0] else {
        panic!("a band first")
    };
    assert_eq!(band.fold, Some(false));
}

#[test]
fn leaves_hang_off_a_guide_that_stops_at_the_last_one() {
    let node = |slug: &str, depth: u64, children: Value| json!({ "id": slug, "slug": slug, "name": slug, "depth": depth, "headcount": 0, "children": children });
    let rows = parse_tree(&json!({ "tree": [node("core", 1, json!([node("group", 2, json!([
        node("area", 3, json!([node("pod-a", 4, json!([])), node("pod-b", 4, json!([node("sub", 5, json!([]))]))])),
    ]))]))] }));
    let blocks = layout(&rows, &HashSet::new());
    assert_eq!(
        shape(&rows, &blocks),
        ["band core", "Head:group Area:area Leaf:pod-a Leaf:pod-b Leaf:sub"]
    );
    let Block::Card(lines) = &blocks[1] else {
        panic!("a card")
    };
    let leaves: Vec<(usize, bool)> = lines[2..].iter().map(|line| (line.indent, line.last)).collect();
    assert_eq!(leaves, [(0, false), (0, true), (1, true)]);
}

#[test]
fn flattens_the_tree_by_name() {
    let rows = parse_tree(&tree());
    let order: Vec<&str> = rows.iter().map(|row| row.slug.as_str()).collect();
    assert_eq!(order, ["eng", "api", "edge", "web", "ops"]);
    assert_eq!(rows[0].leads, ["Ada", "new@acme.com · pending"]);
    assert_eq!(rows[0].headcount, 5);
    assert_eq!(rows[2].depth, 3);
    assert!(rows[3].is_default);
    assert_eq!(rows[3].my_role, "lead");
}

#[test]
fn a_team_cannot_move_under_itself() {
    let rows = parse_tree(&tree());
    let blocked = subtree(&rows, "api");
    assert!(blocked.contains("api") && blocked.contains("edge"));
    assert!(!blocked.contains("web") && !blocked.contains("eng"));
    assert_eq!(subtree(&rows, "eng").len(), 4);
}

#[test]
fn reads_one_team() {
    let detail = parse_detail(&parse(
        r#"{
        "id": "t_eng", "slug": "eng", "name": "Engineering", "description": "Builds it", "level": "department",
        "parentId": "t_root", "parent": "root", "depth": 2, "createdAt": "2026-01-01T00:00:00Z",
        "members": 2, "pending": 1, "childCount": 1, "myRole": "member", "isDefault": false,
        "memberList": [
            { "userId": "u_2", "email": "bo@acme.com", "role": "member", "isDefault": true, "joinedAt": "2026-01-05T00:00:00Z" },
            { "userId": "u_1", "email": "ada@acme.com", "name": "Ada", "role": "lead", "isDefault": false, "joinedAt": "2026-01-04T00:00:00Z" }
        ],
        "pendingList": [
            { "id": "p_1", "teamId": "t_eng", "team": "eng", "email": "new@acme.com", "role": "lead", "home": true, "createdAt": "2026-01-06T00:00:00Z" }
        ],
        "path": [{ "id": "t_root", "slug": "root", "name": "Acme" }],
        "children": [{ "id": "t_web", "slug": "web", "name": "Web", "members": 2, "pending": 0, "childCount": 0 }]
    }"#,
    ));
    assert_eq!(detail.parent, "root");
    assert_eq!(detail.member_list[0].who(), "Ada");
    assert!(detail.member_list[0].lead);
    assert!(detail.member_list[1].is_default);
    assert_eq!(detail.pending_list.len(), 1);
    assert!(detail.pending_list[0].lead && detail.pending_list[0].home);
    assert_eq!(detail.path, [("root".to_string(), "Acme".to_string())]);
    assert_eq!(detail.children[0].members, 2);
}

#[test]
fn groups_pending_people() {
    let entries = parse_pending(&parse(
        r#"{
        "pending": [
            { "id": "p_1", "teamId": "t_web", "team": "web", "email": "zed@acme.com", "role": "member", "home": false, "createdAt": "2026-01-06T00:00:00Z" },
            { "id": "p_2", "teamId": "t_api", "team": "api", "email": "amy@acme.com", "role": "lead", "home": true, "createdAt": "2026-01-07T00:00:00Z" },
            { "id": "p_3", "teamId": "t_ops", "team": "ops", "email": "zed@acme.com", "role": "member", "home": true, "createdAt": "2026-01-08T00:00:00Z" }
        ]
    }"#,
    ));
    let people = pending_people(&entries);
    assert_eq!(people.len(), 2);
    assert_eq!(people[0].0, "amy@acme.com");
    assert_eq!(people[1].1.len(), 2);
}

#[test]
fn checks_slugs_and_levels() {
    assert_eq!(slug_from("Platform Team!"), "platform-team");
    assert_eq!(slug_from("  Ünïcode & Co "), "n-code-co");
    assert!(valid_label("platform-2", 127));
    assert!(!valid_label("-platform", 127));
    assert!(!valid_label("Platform", 127));
    assert!(!valid_label("", 32));
    assert!(!valid_label(&"a".repeat(33), 32));
}

#[test]
fn says_who_was_added() {
    assert_eq!(
        added_text(
            &parse(
                r#"{ "pending": false, "team": "web", "member": { "userId": "u_2", "email": "bo@acme.com", "role": "member", "isDefault": true, "joinedAt": "2026-01-01T00:00:00Z" } }"#
            ),
            "Web"
        ),
        "Added bo@acme.com to Web; it is now their default team."
    );
    assert_eq!(
        added_text(
            &parse(
                r#"{ "pending": true, "team": "web", "entry": { "id": "p_1", "email": "new@acme.com", "role": "member", "home": false } }"#
            ),
            "Web"
        ),
        "new@acme.com is not in your organization yet: they join Web when they sign up with that email."
    );
}

#[test]
fn reads_who_is_looking() {
    let me = parse_me(&parse(
        r#"{
        "tenant": { "id": "t", "name": "Acme", "createdAt": "2026-01-01T00:00:00Z", "kind": "company" },
        "user": { "id": "u_1" }, "role": "admin", "permissions": ["teams.manage", "teams.use"]
    }"#,
    ));
    assert!(me.manage && !me.personal);
    assert_eq!(
        refusal("only the org's owner or admins can create teams (teams.manage)"),
        "Only organization admins can change this."
    );
}
