use super::*;
use core::prelude::v1::test;

fn test_app() -> TestAppContext {
    let cx = TestAppContext::single();
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(Palette::for_choice(
            crate::palette::ThemeChoice::System,
            WindowAppearance::Light,
        )));
    });
    cx
}

#[test]
fn switches_reveal_only_their_dependent_settings() {
    let mut cx = test_app();
    let window = cx.add_window(SubsessionsPage::new);
    cx.update_window(window.into(), |page, _, cx| {
        let page = page.downcast::<SubsessionsPage>().expect("Subsessions page");
        page.update(cx, |page, _| {
            assert_eq!(page.visible(), (true, false));
            page.settings.orchestration = Some(OrchestrationLimits {
                limits_on: true,
                ..Default::default()
            });
            assert_eq!(page.visible(), (true, true));
            page.settings.subsessions = Some(false);
            assert_eq!(page.visible(), (false, false));
            page.settings.subsessions = Some(true);
            assert_eq!(page.visible(), (true, true));
        });
    })
    .expect("open window");
}

#[test]
fn number_input_keeps_custom_values_and_can_clear_a_limit() {
    let mut cx = test_app();
    let window = cx.add_window(SubsessionsPage::new);
    cx.update_window(window.into(), |page, window, cx| {
        let page = page.downcast::<SubsessionsPage>().expect("Subsessions page");
        page.update(cx, |page, cx| {
            let kind = Limit::Starts;
            let number = page.limits[kind as usize].number.clone();
            let mut limits = OrchestrationLimits::default();
            for (input, expected, thumb) in [
                ("500", Some(500.), kind.slider_max()),
                ("0", Some(0.), 0.),
                ("", None, 0.),
            ] {
                number.update(cx, |state, cx| state.set_value(input, window, cx));
                let value = page.read_limit(kind, window, cx).expect("valid input");
                kind.set(&mut limits, value);
                assert_eq!(kind.get(&limits), expected);
                assert_eq!(page.limits[kind as usize].slider.read(cx).value().start(), thumb);
            }
            for input in ["-1", "abc"] {
                number.update(cx, |state, cx| state.set_value(input, window, cx));
                assert!(page.read_limit(kind, window, cx).is_err());
                assert!(page.limits[kind as usize].problem.is_some());
            }
        });
    })
    .expect("open window");
}

#[test]
fn depth_input_reports_the_engines_maximum() {
    let mut cx = test_app();
    let window = cx.add_window(SubsessionsPage::new);
    cx.update_window(window.into(), |page, window, cx| {
        let page = page.downcast::<SubsessionsPage>().expect("Subsessions page");
        page.update(cx, |page, cx| {
            let max = nexus_workbench::linked::MAX_DEPTH;
            let number = page.limits[Limit::Depth as usize].number.clone();
            number.update(cx, |state, cx| state.set_value(max.to_string(), window, cx));
            assert_eq!(page.read_limit(Limit::Depth, window, cx), Ok(Some(f64::from(max))));
            number.update(cx, |state, cx| state.set_value((max + 1).to_string(), window, cx));
            let error = page
                .read_limit(Limit::Depth, window, cx)
                .expect_err("depth exceeds the bound");
            assert!(error.contains(&max.to_string()));
            assert!(Limit::Depth.detail().contains(&max.to_string()));
        });
    })
    .expect("open window");
}

#[test]
fn finishing_a_save_restores_unfocused_rows_to_persisted_values() {
    let mut cx = test_app();
    let window = cx.add_window(SubsessionsPage::new);
    cx.update_window(window.into(), |page, window, cx| {
        let page = page.downcast::<SubsessionsPage>().expect("Subsessions page");
        page.update(cx, |page, cx| {
            let starts = page.limits[Limit::Starts as usize].number.clone();
            let workers = page.limits[Limit::Workers as usize].number.clone();
            let thumb = page.limits[Limit::Workers as usize].slider.clone();
            for result in [
                Ok(Settings {
                    orchestration: Some(OrchestrationLimits {
                        limits_on: true,
                        max_starts: Some(5),
                        ..Default::default()
                    }),
                    ..Settings::default()
                }),
                Err("Save failed.".into()),
            ] {
                let failed = result.is_err();
                page.saving = true;
                starts.update(cx, |state, cx| state.set_value("6", window, cx));
                workers.update(cx, |state, cx| state.set_value("7", window, cx));
                thumb.update(cx, |state, cx| state.set_value(7., window, cx));
                page.finish_save(result, window, cx);
                assert!(!page.saving);
                assert_eq!(starts.read(cx).value().as_ref(), "5");
                assert_eq!(workers.read(cx).value().as_ref(), "");
                assert_eq!(thumb.read(cx).value().start(), 0.);
                assert_eq!(page.problem.is_some(), failed);
            }
        });
    })
    .expect("open window");
}

#[test]
fn slider_release_during_save_is_ignored_and_focused_input_keeps_its_draft() {
    let mut cx = test_app();
    let window = cx.add_window(SubsessionsPage::new);
    cx.run_until_parked();
    cx.update_window(window.into(), |page, window, cx| {
        let page = page.downcast::<SubsessionsPage>().expect("Subsessions page");
        page.update(cx, |page, cx| {
            let limits = OrchestrationLimits {
                limits_on: true,
                max_live_children: Some(2),
                max_starts: Some(5),
                ..Default::default()
            };
            page.settings.orchestration = Some(limits);
            page.sync_inputs(window, cx);
            page.saving = true;
            page.limits[Limit::Workers as usize].slider.update(cx, |slider, cx| {
                slider.set_value(7., window, cx);
                cx.emit(SliderEvent::Release(7_f32.into()));
            });
        });
    })
    .expect("open window");
    cx.run_until_parked();
    cx.update_window(window.into(), |page, window, cx| {
        let page = page.downcast::<SubsessionsPage>().expect("Subsessions page");
        page.update(cx, |page, cx| {
            let workers = &page.limits[Limit::Workers as usize];
            assert_eq!(
                workers.number.read(cx).value().as_ref(),
                "2",
                "release did not edit the field"
            );
            let starts = page.limits[Limit::Starts as usize].number.clone();
            let focus = starts.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            starts.update(cx, |state, cx| state.set_value("12", window, cx));

            page.finish_save(Ok(page.settings.clone()), window, cx);
            assert_eq!(page.limits[Limit::Workers as usize].slider.read(cx).value().start(), 2.);
            assert_eq!(starts.read(cx).value().as_ref(), "12");
            assert_eq!(page.settings.orchestration.as_ref().unwrap().max_starts, Some(5));
        });
    })
    .expect("open window");
}
