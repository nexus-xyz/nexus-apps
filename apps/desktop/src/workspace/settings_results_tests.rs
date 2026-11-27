use super::{result_heads, result_hint, result_path};
use crate::workspace::settings_registry::Visibility;
use crate::workspace::settings_search::search;

#[test]
fn a_head_opens_each_run_of_hits_from_one_nav_row_and_keeps_their_rank() {
    let hits = search("a", &Visibility::all());
    assert!(hits.len() > 8);
    let heads = result_heads(&hits);
    assert_eq!(heads.len(), hits.len());
    assert_eq!(heads[0], hits[0].breadcrumb.first().copied());
    for ix in 1..hits.len() {
        let changed = hits[ix].breadcrumb.first() != hits[ix - 1].breadcrumb.first();
        assert_eq!(heads[ix].is_some(), changed, "hit {ix}");
    }
}

#[test]
fn the_path_names_only_what_lies_between_the_head_and_the_label() {
    let address = search("Gateway address", &Visibility::all()).remove(0);
    assert_eq!(address.breadcrumb, ["This computer", "Gateway address"]);
    assert_eq!(result_path(&address), None);
    let providers = search("BYOK", &Visibility::all()).remove(0);
    assert_eq!(providers.breadcrumb, ["Model access", "Providers"]);
    assert_eq!(result_path(&providers), None);
    let nested = search("a", &Visibility::all())
        .into_iter()
        .find(|hit| hit.breadcrumb.len() == 3)
        .expect("a field inside a tab");
    assert_eq!(result_path(&nested).as_deref(), Some(nested.breadcrumb[1]));
}

#[test]
fn only_the_selected_row_says_enter_and_a_browser_hit_always_says_so() {
    assert_eq!(result_hint(false, false), None);
    assert_eq!(result_hint(false, true), Some("↵ Open"));
    assert_eq!(result_hint(true, false), Some("Opens in browser"));
    assert_eq!(result_hint(true, true), Some("↵ Open in browser"));
}
