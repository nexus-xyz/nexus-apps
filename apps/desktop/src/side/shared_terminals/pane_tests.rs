//! The Commands tab with a History of thousands: it draws only the rows near the screen, it scrolls, and a read that
//! changed nothing does not redraw it.
use super::SharedTerminals;
use crate::palette::{Palette, ThemeChoice};
use core::prelude::v1::test;
use futures::channel::oneshot;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const COUNT: usize = 3_000;

type Answer = oneshot::Sender<Result<Value, String>>;

struct Host(Entity<SharedTerminals>);
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(680.)).h(px(400.)).child(self.0.clone())
    }
}

/// `COUNT` finished commands, the last one the newest; `changed` marks one as failed.
fn history(changed: Option<usize>) -> Value {
    Value::Array(
        (0..COUNT)
            .map(|i| {
                let failed = changed == Some(i);
                json!({"id": format!("cmd-{i}"), "name": "", "command": format!("echo {i}"), "cwd": "/fixture",
                    "status": if failed { "failed" } else { "completed" }, "owner": "agent", "private": false,
                    "exitCode": if failed { 1 } else { 0 }, "createdAt": 1_000 + i as i64})
            })
            .collect(),
    )
}

/// The rows the last frame drew. The selectors are made once: `debug_bounds` takes a `'static` name.
fn drawn(cx: &mut VisualTestContext) -> Vec<usize> {
    static SELECTORS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    let selectors = SELECTORS.get_or_init(|| (0..COUNT).map(|i| &*format!("command-row-cmd-{i}").leak()).collect());
    (0..COUNT)
        .filter(|&i| cx.debug_bounds(selectors[i]).is_some())
        .collect()
}

fn answer_list(lists: &Arc<Mutex<Vec<Answer>>>, value: Value) {
    let answer = lists.lock().unwrap().pop().expect("a list read is waiting");
    answer.send(Ok(value)).unwrap();
}

#[gpui_kit::test]
fn a_long_history_draws_only_the_rows_near_the_screen_and_quiet_reads_do_not_redraw(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data = std::env::temp_dir().join(format!("nexus-commands-pane-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data).unwrap();
    let (runner, _events) = crate::code_run::test_runner(data.clone(), "scope");
    let lists: Arc<Mutex<Vec<Answer>>> = Arc::default();
    let waiting = lists.clone();
    runner.fixture_terminal(move |_, op, _, _| {
        let (answer, receive) = oneshot::channel();
        if op == "list" {
            waiting.lock().unwrap().push(answer);
        }
        receive
    });
    let (host, cx) = cx.add_window_view(|window, cx| {
        cx.set_global(crate::pages::Theme(Palette::for_choice(
            ThemeChoice::Light,
            WindowAppearance::Light,
        )));
        Host(cx.new(|cx| {
            let mut view = SharedTerminals::new(Rc::new(runner), "session".into(), window, cx);
            view.history_allowed = true;
            view.show_history = true;
            view
        }))
    });
    let view = host.read_with(cx, |host, _| host.0.clone());
    cx.run_until_parked();
    answer_list(&lists, history(None));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Newest first: the top rows are the last commands, and only about a screen of them is drawn.
    let rows = drawn(cx);
    assert!(rows.contains(&(COUNT - 1)), "the newest command is on top");
    assert!((5..40).contains(&rows.len()), "drew {} rows", rows.len());

    let list = cx.debug_bounds("commands-pane-list").unwrap();
    for _ in 0..10 {
        cx.simulate_event(ScrollEvent {
            position: list.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-360.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let rows = drawn(cx);
    assert!(!rows.contains(&(COUNT - 1)), "the top scrolled away");
    assert!((5..40).contains(&rows.len()), "drew {} rows", rows.len());
    // 3,600px of 36px rows: the screen starts about 100 rows down.
    let top = *rows.iter().max().unwrap();
    assert!(
        (COUNT - 110..COUNT - 90).contains(&top),
        "the screen starts at command {top}"
    );

    let redraws = Rc::new(Cell::new(0));
    let count = redraws.clone();
    let _observe = cx.update(|_, cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)));
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    answer_list(&lists, history(None));
    cx.run_until_parked();
    assert_eq!(redraws.get(), 0, "a read that changed nothing redrew the pane");

    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    answer_list(&lists, history(Some(5)));
    cx.run_until_parked();
    assert!(redraws.get() > 0, "a changed command redraws the pane");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let rows = drawn(cx);
    assert!(rows.contains(&top), "the reader stays where they were");

    let _ = std::fs::remove_dir_all(&data);
}
