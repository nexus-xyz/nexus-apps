use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext, px};
use nexus_desktop_core::chat::WebSource;

use super::super::transcript::Entry;
use super::super::{Auth, View, Workspace, machine_source::MachineSource};

fn source(title: &str, url: &str, query: &str) -> WebSource {
    WebSource {
        title: title.into(),
        url: url.into(),
        date: None,
        query: query.into(),
    }
}

/// A chat whose answer (entry 2) read `sources`, drawn.
fn answered(sources: Vec<WebSource>, cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        Workspace::with_machine_source(gateway, events, arrivals, false, MachineSource::isolated(), window, cx)
    });
    workspace.update(cx, |workspace, cx| {
        workspace.auth = Auth::SignedIn;
        workspace.view = View::Chat;
        let mut answer = Entry::answer(2, "Exponential growth outpaces polynomial growth.", false, cx);
        answer.web_sources = sources;
        workspace.entries = vec![Entry::user(1, "Was ETH solved?".into()), answer];
        cx.notify();
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    draw(cx);
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click_chip(cx: &mut VisualTestContext) {
    let chip = cx.debug_bounds("web-sources-chip").expect("the chip is drawn");
    cx.simulate_click(chip.center(), Modifiers::none());
    draw(cx);
}

fn open(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    workspace.read_with(cx, |workspace, _| workspace.entries[1].sources_open)
}

fn three() -> Vec<WebSource> {
    vec![
        source(
            "The Exponential Time Hypothesis is False",
            "https://github.com/a/eth",
            "eth solved",
        ),
        source(
            "On the Complexity of k-SAT",
            "https://cseweb.ucsd.edu/k-sat.pdf",
            "k-sat",
        ),
        source(
            "Exponential time hypothesis",
            "https://en.wikipedia.org/wiki/ETH",
            "eth",
        ),
    ]
}

/// The design: the answer's body lists no sources; one chip in its footer stands for them, and the list starts closed.
#[gpui_kit::test]
fn sources_start_as_a_closed_chip_in_the_footer(cx: &mut TestAppContext) {
    let (workspace, cx) = answered(three(), cx);
    let chip = cx.debug_bounds("web-sources-chip").expect("the chip is drawn");
    assert_eq!(chip.size.height, px(24.), "the chip is the footer's 24 line");
    assert!(!open(&workspace, cx));
    assert!(
        cx.debug_bounds("web-sources-list").is_none(),
        "no list until the chip opens it"
    );
    assert!(cx.debug_bounds("web-source-row").is_none());
}

#[gpui_kit::test]
fn an_answer_with_no_sources_has_no_chip(cx: &mut TestAppContext) {
    let (_, cx) = answered(Vec::new(), cx);
    assert!(cx.debug_bounds("web-sources-chip").is_none());
}

/// A click opens the list in place, under the footer: a row for each page, with its title and its site.
#[gpui_kit::test]
fn a_click_opens_the_list_under_the_footer(cx: &mut TestAppContext) {
    let (workspace, cx) = answered(three(), cx);
    click_chip(cx);
    assert!(open(&workspace, cx), "the answer's sources are open");
    let chip = cx.debug_bounds("web-sources-chip").unwrap();
    let list = cx.debug_bounds("web-sources-list").expect("the list is drawn");
    assert!(
        list.top() >= chip.bottom(),
        "the list sits under the footer: {list:?} under {chip:?}"
    );
    // 4 under the footer row, which centres the 24 chip on its 28 actions.
    assert!(
        list.top() - chip.bottom() <= px(8.),
        "right under the footer: {list:?} under {chip:?}"
    );
    assert!(
        cx.debug_bounds("web-source-title").is_some(),
        "a page's title is listed"
    );
    assert!(cx.debug_bounds("web-source-facts").is_some(), "with its site");
    // Three rows of 8 + 18 + 16 + 8, in a card padded 4: the message was measured again with the list in it.
    assert!(list.size.height >= px(150.), "every page has its row: {list:?}");
}

/// A second click closes what the first opened.
#[gpui_kit::test]
fn a_second_click_closes_the_list(cx: &mut TestAppContext) {
    let (workspace, cx) = answered(three(), cx);
    click_chip(cx);
    click_chip(cx);
    assert!(!open(&workspace, cx));
    assert!(cx.debug_bounds("web-sources-list").is_none(), "the list is gone");
}
