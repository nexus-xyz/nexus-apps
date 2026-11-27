//! Auxiliary launch work waits until the first window draws. [`after_first_frame`] queues it, and
//! `windows.rs` releases it after the frame reaches the screen. A launch with no window releases it at once.
//! A covered or closed window cannot hold the work past [`AT_THE_LATEST`].

use std::time::Duration;

use gpui_kit::{App, Global};

/// How long held work waits for a first frame that does not come.
const AT_THE_LATEST: Duration = Duration::from_secs(1);

type Work = Box<dyn FnOnce(&mut App)>;

/// The held work, until the first frame; empty and `drawn` after it.
#[derive(Default)]
struct Held {
    drawn: bool,
    work: Vec<Work>,
}

impl Global for Held {}

/// Run `work` once the first window has drawn its first frame, or now when that has happened.
pub fn after_first_frame(cx: &mut App, work: impl FnOnce(&mut App) + 'static) {
    if !cx.has_global::<Held>() {
        cx.set_global(Held::default());
        cx.spawn(async |cx| {
            cx.background_executor().timer(AT_THE_LATEST).await;
            cx.update(first_frame_drawn);
        })
        .detach();
    }
    if cx.global::<Held>().drawn {
        work(cx);
    } else {
        cx.global_mut::<Held>().work.push(Box::new(work));
    }
}

/// The first window drew (or none will open): run what waited for it. Later calls do nothing.
pub fn first_frame_drawn(cx: &mut App) {
    let held = cx.default_global::<Held>();
    if held.drawn {
        return;
    }
    held.drawn = true;
    for work in std::mem::take(&mut held.work) {
        work(cx);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::TestAppContext;

    use super::*;

    #[gpui_kit::test]
    fn held_work_runs_once_at_the_first_frame_and_later_work_runs_at_once(cx: &mut TestAppContext) {
        let runs = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let held = runs.clone();
            after_first_frame(cx, move |_| held.set(held.get() + 1));
        });
        assert_eq!(runs.get(), 0, "held until the first frame");
        cx.update(first_frame_drawn);
        cx.update(first_frame_drawn);
        assert_eq!(runs.get(), 1, "run once");
        cx.update(|cx| {
            let held = runs.clone();
            after_first_frame(cx, move |_| held.set(held.get() + 10));
        });
        assert_eq!(runs.get(), 11, "after the first frame, at once");
    }

    #[gpui_kit::test]
    fn held_work_runs_without_a_frame_after_the_deadline(cx: &mut TestAppContext) {
        let runs = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let held = runs.clone();
            after_first_frame(cx, move |_| held.set(held.get() + 1));
        });
        cx.executor().advance_clock(AT_THE_LATEST);
        cx.run_until_parked();
        assert_eq!(runs.get(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn prefetched_font_families_include_fonts_registered_after_the_first_query() {
        use gpui_kit::PlatformTextSystem;

        let fonts = gpui_apple::AppleTextSystem::new();
        let _before = fonts.all_font_names();
        fonts
            .add_fonts(vec![std::borrow::Cow::Borrowed(
                include_bytes!("../assets/fonts/GeistMono-Regular.ttf").as_slice(),
            )])
            .expect("register the bundled font after the system query");
        let names = fonts.all_font_names();
        assert!(names.iter().any(|name| name == "Geist Mono"));
    }
}
