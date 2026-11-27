//! Fixtures for the Library's rows.
use super::*;
use serde_json::json;

fn share(id: &str, page: Option<(&str, &str)>, mine: bool, date: &str) -> Value {
    let mut value = json!({"id":id,"version":2,"title":"Quarterly review","mine":mine,
        "status":"active","createdAt":date,"author":{"name":"Ada","email":"ada@acme.test"},
        "audience":{"kind":"org"}});
    if let Some((id, format)) = page {
        value["kind"] = json!("page");
        value["pageId"] = json!(id);
        value["pageFormat"] = json!(format);
    }
    value
}

fn mixed() -> Lists {
    let first = share("doc-share", Some(("shared-doc", "md")), false, "2026-11-06T00:00:00Z");
    let mut second = share(
        "doc-team-share",
        Some(("shared-doc", "md")),
        false,
        "2026-11-05T00:00:00Z",
    );
    second["audience"] = json!({"kind":"teams","teams":["platform"]});
    let chat = share("chat", None, false, "2026-11-04T00:00:00Z");
    let own_share = share("my-share", Some(("mine-site", "html")), true, "2026-11-07T00:00:00Z");
    let pages = json!({"pages":[
        {"id":"mine-doc","title":"Quarterly notes","format":"md","updatedAt":"2026-11-03T00:00:00Z"},
        {"id":"mine-site","title":"Launch","format":"jsx","updatedAt":"2026-11-02T00:00:00Z"},
        {"id":"mine-sheet","title":"Forecast","format":"sheet","updatedAt":"2026-11-01T00:00:00Z"},
        {"id":"mine-board","title":"Planning","format":"board","updatedAt":"2026-10-30T00:00:00Z"}
    ],"nextCursor":"older-pages"});
    Lists::from_responses(
        &json!({"artifacts":[first,second,chat.clone()],"nextCursor":"older-shares"}),
        &json!({"artifacts":[own_share,chat]}),
        Some(&pages),
        Some("Acme"),
    )
}

#[test]
fn ownership_kind_counts_search_and_open_targets_use_the_same_rows() {
    let lists = mixed();
    assert_eq!(
        Tab::ALL.map(Tab::label),
        ["All", "My drafts", "Shared with me", "Shared by me"]
    );
    assert_eq!(
        Kind::ALL.map(Kind::label),
        ["All", "Sites", "Docs", "Sheets", "Designs", "Chats"]
    );
    assert_eq!(lists.count(Tab::All, Kind::All), "6+");
    assert_eq!(lists.count(Tab::Pages, Kind::All), "4+");
    assert_eq!(lists.count(Tab::With, Kind::All), "2+");
    assert_eq!(lists.count(Tab::By, Kind::All), "2");
    for (kind, count) in [
        (Kind::Doc, "2+"),
        (Kind::Site, "1+"),
        (Kind::Sheet, "1+"),
        (Kind::Board, "1+"),
        (Kind::Chat, "1+"),
    ] {
        assert_eq!(lists.count(Tab::All, kind), count);
    }
    assert!(
        lists.visible(Tab::Pages, Kind::All, "ada").is_empty(),
        "drafts never include received shares"
    );
    let docs = lists.visible(Tab::All, Kind::Doc, "QUARTERLY platform ada@acme.test");
    assert_eq!(
        docs.len(),
        1,
        "search includes all grouped audiences and the named sharer's email"
    );
    assert_eq!(docs[0].open, Open::SharedPage("doc-share".into()));
    assert_eq!(docs[0].shares.len(), 2);
    assert_eq!(
        lists.find("doc-team-share").unwrap().open,
        Open::SharedPage("doc-team-share".into()),
        "the grouped-away share remains addressable for its own menu actions"
    );
    let sheets = lists.visible(Tab::Pages, Kind::Sheet, "forecast");
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].open, Open::Page("mine-sheet".into()));
    assert_eq!(sheets[0].sub, "Sheet");
    assert!(lists.visible(Tab::Pages, Kind::Chat, "").is_empty());
    assert!(lists.visible(Tab::All, Kind::Sheet, "quarterly").is_empty());
    assert_eq!(
        lists.count(Tab::All, Kind::Doc),
        "2+",
        "search does not alter tab counts"
    );
    assert_eq!(
        lists.visible(Tab::All, Kind::Chat, "")[0].open,
        Open::Chat("chat".into())
    );
    assert!(!lists.all.iter().any(|row| row.key == "my-share"));
    let by = lists.visible(Tab::By, Kind::Site, "");
    assert_eq!(by[0].key, "my-share");
    assert_eq!(by[0].owned_page.as_deref(), Some("mine-site"));
    assert!(
        lists.find("my-share").unwrap().actions(false).contains(&Action::Revoke),
        "a share omitted from All remains actionable in Shared by me"
    );
    assert!(lists.find("my-share").unwrap().actions(false).contains(&Action::Share));
}

#[test]
fn grouped_pages_prefer_owned_then_active_and_keep_the_first_groups_position() {
    let mut revoked = share("revoked", Some(("source", "md")), false, "2026-11-06T00:00:00Z");
    revoked["status"] = json!("revoked");
    let active = share("active", Some(("source", "md")), false, "2026-11-04T00:00:00Z");
    let lists = Lists::from_responses(
        &json!({"artifacts":[revoked.clone(),active.clone()]}),
        &json!({}),
        None,
        None,
    );
    assert_eq!(lists.with.len(), 1);
    assert_eq!(lists.with[0].open, Open::SharedPage("active".into()));
    assert!(!lists.with[0].revoked);
    assert_eq!(lists.with[0].shares[0].key, "revoked");
    let pages = json!({"pages":[{"id":"source","title":"Owned","format":"md","updatedAt":"2026-11-01T00:00:00Z"}]});
    let lists = Lists::from_responses(&json!({"artifacts":[revoked,active]}), &json!({}), Some(&pages), None);
    assert_eq!(lists.all.len(), 1);
    assert_eq!(lists.all[0].open, Open::Page("source".into()));
    assert_eq!(lists.all[0].shares.len(), 3);
    assert_eq!(lists.count(Tab::All, Kind::Doc), "1");
}

#[test]
fn foreign_pages_and_chats_keep_their_viewer_urls_after_filtering() {
    for page in [None, Some(("foreign", "board"))] {
        let mut value = share("foreign-share", page, false, "2026-11-05T00:00:00Z");
        value["external"] = json!(true);
        value["organization"] = json!("Globex");
        value["viewerUrl"] = json!("https://nexus.inc/a/foreign-share?token=reader");
        let lists = Lists::from_responses(&json!({"artifacts":[value]}), &json!({}), None, None);
        let rows = lists.visible(
            Tab::With,
            if page.is_some() { Kind::Board } else { Kind::Chat },
            "globex",
        );
        assert_eq!(
            rows[0].open,
            Open::Browser("https://nexus.inc/a/foreign-share?token=reader".into())
        );
        assert!(
            !rows[0].actions(true).contains(&Action::Revoke),
            "an admin in another organization gains no authority"
        );
    }
}

#[test]
fn empty_and_missing_page_lists_preserve_ownership_and_pagination_counts() {
    let lists = Lists::from_responses(&json!({"artifacts":[],"nextCursor":"older"}), &json!({}), None, None);
    for kind in Kind::ALL {
        assert!(lists.visible(Tab::Pages, kind, "").is_empty());
        assert_eq!(lists.count(Tab::Pages, kind), "0");
        assert_eq!(lists.count(Tab::With, kind), "0+");
    }
}

#[test]
fn own_shares_remain_in_all_when_their_owned_page_is_not_loaded() {
    let mine = share("my-share", Some(("source", "html")), true, "2026-11-07T00:00:00Z");
    let cases = [
        None,
        Some(json!({"pages":[]})),
        Some(json!({"pages":[],"nextCursor":"older-pages"})),
        Some(json!({"pages":[{"id":"unrelated","format":"md","title":"Other draft"}],"nextCursor":"older-pages"})),
    ];
    for pages in cases {
        let lists = Lists::from_responses(
            &json!({"artifacts":[mine.clone()]}),
            &json!({"artifacts":[mine.clone()]}),
            pages.as_ref(),
            None,
        );
        let rows = lists.visible(Tab::All, Kind::Site, "quarterly");
        assert_eq!(
            rows.len(),
            1,
            "the available share appears once even when both artifact scopes return it"
        );
        assert_eq!(rows[0].key, "my-share");
        assert_eq!(rows[0].open, Open::SharedPage("my-share".into()));
        assert!(
            rows[0].owned_page.is_none(),
            "share authorship does not prove page ownership"
        );
        assert!(!rows[0].actions(false).contains(&Action::Share));
        assert!(!rows[0].actions(false).contains(&Action::Versions));
        assert!(!rows[0].actions(false).contains(&Action::Delete));
        assert!(rows[0].actions(false).contains(&Action::Revoke));
        assert!(lists.visible(Tab::Pages, Kind::Site, "").is_empty());
    }
}

#[test]
fn loaded_owned_page_replaces_its_own_share_in_all_without_duplicate_counts() {
    let mine = share("my-share", Some(("source", "html")), true, "2026-11-07T00:00:00Z");
    let pages = json!({"pages":[{"id":"source","format":"html","title":"My draft","latestVersion":2}]});
    let lists = Lists::from_responses(
        &json!({"artifacts":[mine.clone()]}),
        &json!({"artifacts":[mine]}),
        Some(&pages),
        None,
    );
    assert_eq!(lists.all.len(), 1);
    assert_eq!(lists.all[0].key, "page:source");
    assert_eq!(lists.all[0].open, Open::Page("source".into()));
    assert_eq!(lists.count(Tab::All, Kind::Site), "1");
    assert_eq!(lists.count(Tab::Pages, Kind::Site), "1");
    assert_eq!(lists.by[0].key, "my-share");
    assert_eq!(lists.by[0].owned_page.as_deref(), Some("source"));
    assert!(lists.by[0].actions(false).contains(&Action::Share));
    assert!(
        !lists.all[0].shared() && lists.shared(&lists.all[0]),
        "All drops the page's own share, and the page still reads as shared"
    );
}

/// The page header's Share reads Shared while a share of the page is live: not for a draft with none, nor for one
/// whose only share is revoked.
#[test]
fn a_page_is_shared_while_one_of_its_shares_is_live() {
    let lists = mixed();
    let draft = lists.find("page:mine-doc").unwrap();
    assert!(!lists.shared(draft), "a draft with no share is private");
    assert!(lists.shared(lists.find("doc-share").unwrap()), "a share is shared");
    let mut revoked = share("revoked", Some(("source", "md")), true, "2026-11-06T00:00:00Z");
    revoked["status"] = json!("revoked");
    let pages = json!({"pages":[{"id":"source","title":"Owned","format":"md","updatedAt":"2026-11-01T00:00:00Z"}]});
    let lists = Lists::from_responses(&json!({}), &json!({"artifacts":[revoked]}), Some(&pages), None);
    let page = lists.find("page:source").unwrap();
    assert!(!lists.shared(page), "a revoked share no longer shares the page");
}
