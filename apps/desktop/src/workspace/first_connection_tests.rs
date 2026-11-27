use std::{path::Path, rc::Rc, sync::Arc};

use gpui_kit::{Entity, Focusable, Keystroke, Modifiers, Task, TestAppContext, VisualTestContext};
use nexus_providers::{InstanceId, KeyVendor, ProviderKind};
use nexus_workbench::event::{SessionAgent, Via};

const LOCAL_SCOPE: &str = "|";

use super::{Page, Progress};
use crate::code_run::{CodeEvent, CodeRunner};
use crate::pages::local_providers::{self, SetupForm};
use crate::workspace::{Auth, View, Workspace, local_mode::Entry, machine_source::MachineSource, overlay::OverlaySlot};

fn fixture<'a>(
    cx: &'a mut TestAppContext,
    gateway: &str,
    runner: Rc<CodeRunner>,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::local_providers_opt_in::install(cx, runner.local_providers_opt_in().clone());
    });
    let gateway = crate::gateway::Gateway::for_view_test_at(crate::code_run::runtime(), gateway);
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.auth = Auth::SignedOut { error: None };
        workspace.runner = Some(runner);
        workspace
    })
}

pub(crate) enum Connection {
    Subscription,
    Key(KeyVendor),
    Server { address: String, keyed: bool },
    RemovedServer,
}

pub(crate) fn start_first_turn_from_added_projects(
    runner: Rc<CodeRunner>,
    gateway: &str,
    folder: &Path,
    dialog_folder: &Path,
    project_added: impl FnOnce() -> CodeEvent,
    first_created: impl FnOnce() -> CodeEvent,
) {
    let mut app = TestAppContext::single();
    app.executor().allow_parking();
    let (workspace, cx) = fixture(&mut app, gateway, runner);
    let window = workspace.read_with(cx, |workspace, _| workspace.window_handle);
    let mut cx = VisualTestContext::from_window(window, &app);

    draw(&mut cx);
    let continue_button = cx
        .debug_bounds("continue-without-account")
        .expect("Continue without an account");
    cx.simulate_click(continue_button.center(), Modifiers::default());
    draw(&mut cx);
    workspace.read_with(&cx, |workspace, _| {
        assert!(workspace.is_local());
        assert!(workspace.connection_open());
        assert!(workspace.projects.is_empty());
    });
    let open_folder = cx
        .debug_bounds("open-folder")
        .expect("Open a folder in the Local sidebar");
    cx.simulate_click(open_folder.center(), Modifiers::default());
    assert!(app.did_prompt_for_paths(), "Open a folder must show the folder picker");
    app.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![folder.to_path_buf()])
    });
    cx.run_until_parked();

    // The service fixture owns the real runner receiver, as the app's window manager does.
    let event = project_added();
    let CodeEvent::ProjectAdded { id, origin, .. } = &event else {
        panic!("the folder picker must add a project");
    };
    assert_eq!(*origin, Some(workspace.entity_id()));
    let project = id.clone();
    workspace.update_in(&mut cx, |workspace, window, cx| {
        workspace.on_code_events(vec![event], window, cx);
    });
    cx.run_until_parked();
    workspace.update_in(&mut cx, |workspace, window, cx| {
        assert!(workspace.projects.iter().any(|entry| entry.id == project));
        assert!(matches!(workspace.view, View::NewCode));
        assert_eq!(workspace.code_form.project.as_ref(), Some(&project));
        assert!(!workspace.connection_open());
        workspace.open_project_dialog(None, window, cx);
    });
    draw(&mut cx);
    let browse = cx.debug_bounds("project-browse").expect("Browse in the project dialog");
    cx.simulate_click(browse.center(), Modifiers::default());
    assert!(app.did_prompt_for_paths());
    app.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![dialog_folder.to_path_buf()])
    });
    draw(&mut cx);
    let add = cx.debug_bounds("project-submit").expect("Add in the project dialog");
    cx.simulate_click(add.center(), Modifiers::default());

    // The dialog uses the runner's async reply instead of the sidebar's ProjectAdded event.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if workspace.read_with(&cx, |workspace, _| {
            workspace.project_dialog.is_none()
                && workspace.projects.iter().any(|entry| {
                    entry.name == dialog_folder.file_name().unwrap().to_string_lossy()
                        && workspace.code_form.project.as_ref() == Some(&entry.id)
                })
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the dialog project was not listed and selected"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    workspace.update_in(&mut cx, |workspace, window, cx| {
        assert_eq!(workspace.projects.len(), 2);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Reply with OK.", window, cx));
        workspace.submit(window, cx);
        assert!(matches!(workspace.view, View::Code(_)));
    });
    cx.run_until_parked();
    let created = first_created();
    workspace.update_in(&mut cx, |workspace, window, cx| {
        assert!(Progress::load().is_none());
        workspace.open_new_code(window, cx);
        assert!(workspace.waits_for(&created));
        workspace.on_code_events(vec![created], window, cx);
        assert_local_relaunch_skips_chooser(workspace, window, cx);
    });
}

fn assert_local_relaunch_skips_chooser(
    workspace: &mut Workspace,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<Workspace>,
) {
    assert_eq!(Progress::load(), Some(Progress::Done));
    workspace.leave_local(Auth::SignedOut { error: None }, window, cx);
    workspace.enter_local(Entry::Launch, window, cx);
    assert!(workspace.is_local());
    assert!(!workspace.connection_open());
}

pub(crate) fn check_dismissal(runner: Rc<CodeRunner>, gateway: &str) {
    let mut app = TestAppContext::single();
    let (workspace, cx) = fixture(&mut app, gateway, runner);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.continue_without_account(window, cx);
        assert!(workspace.connection_open());
        workspace.open_palette(window, cx);
        let escape = Keystroke::parse("escape").unwrap();
        workspace.on_keystroke(&escape, window, cx);
        assert!(workspace.overlay.palette().is_none());
        assert!(workspace.connection_open());
        assert!(Progress::load().is_none());

        workspace.on_keystroke(&escape, window, cx);
        assert!(!workspace.connection_open());
        assert!(workspace.composer.focus_handle(cx).is_focused(window));
        assert_local_relaunch_skips_chooser(workspace, window, cx);
    });
    cx.run_until_parked();
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }
}

pub(crate) fn start_first_turn(
    runner: Rc<CodeRunner>,
    gateway: &str,
    config: &Path,
    project: String,
    connection: Connection,
    first_created: impl FnOnce() -> Option<CodeEvent>,
) -> Option<InstanceId> {
    let mut app = TestAppContext::single();
    app.executor().allow_parking();
    let (workspace, cx) = fixture(&mut app, gateway, runner);
    cx.update(|_, cx| {
        local_providers::install_test_store(config, Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys), cx);
    });
    let agent = match &connection {
        Connection::Subscription | Connection::Key(KeyVendor::Anthropic) => SessionAgent::Claude,
        Connection::Key(KeyVendor::Openai) => SessionAgent::Codex,
        Connection::Server { .. } | Connection::RemovedServer => SessionAgent::Opencode,
    };
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(Progress::load().is_none());
        workspace.continue_without_account(window, cx);
        assert!(matches!(workspace.local_mode.connection, Some(Page::Choices)));
        assert!(workspace.workspace_open());
        assert!(workspace.focus.is_focused(window));
        assert!(!workspace.composer.focus_handle(cx).is_focused(window));
        assert!(workspace.local_mode.remembered);
        assert!(matches!(workspace.view, View::NewCode));
        workspace.open_new_code(window, cx);
        assert!(workspace.local_mode.connection.is_none());
        assert!(workspace.composer.focus_handle(cx).is_focused(window));
        workspace.leave_local(Auth::SignedOut { error: None }, window, cx);
        workspace.enter_local(Entry::Launch, window, cx);
        assert!(workspace.connection_open());
        workspace.choose_connection(ProviderKind::ApiKey, window, cx);
        workspace.open_connection_form(SetupForm::Key(KeyVendor::Openai), window, cx);
        assert!(workspace.connection_back(window, cx));
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(Progress::load().is_none());
        assert!(matches!(workspace.local_mode.connection, Some(Page::KeyVendor)));
        assert!(workspace.connection_back(window, cx));
        let kind = match connection {
            Connection::Subscription => ProviderKind::CliSubscription,
            Connection::Key(_) => ProviderKind::ApiKey,
            Connection::Server { .. } | Connection::RemovedServer => ProviderKind::OpenAiCompatible,
        };
        workspace.choose_connection(kind, window, cx);
        if let Connection::Key(vendor) = connection {
            assert!(matches!(workspace.local_mode.connection, Some(Page::KeyVendor)));
            workspace.open_connection_form(SetupForm::Key(vendor), window, cx);
        }
        if let Some(Page::Form { editor, .. }) = &workspace.local_mode.connection {
            let (server, secret) = match &connection {
                Connection::Key(_) => (None, Some("nexus-fixture-key")),
                Connection::Server { address, keyed } => (
                    Some((address.as_str(), "team/model")),
                    keyed.then_some("nexus-fixture-key"),
                ),
                Connection::RemovedServer => (Some(("http://127.0.0.1:1/v1", "team/model")), None),
                Connection::Subscription => panic!("subscription should enter local Code"),
            };
            local_providers::submit_test_setup(editor, server, secret, window, cx);
            if matches!(connection, Connection::Key(KeyVendor::Anthropic)) {
                assert!(workspace.connection_save_pending(cx));
                workspace.open_settings(crate::workspace::SettingsSection::LocalProviders, window, cx);
            }
        }
    });
    cx.run_until_parked();
    let provider = workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.is_local());
        if matches!(connection, Connection::Key(KeyVendor::Anthropic)) {
            assert!(matches!(
                workspace.view,
                View::Settings(crate::workspace::SettingsSection::LocalProviders)
            ));
            assert!(workspace.local_mode.connection.is_none());
            workspace.back_to_app(window, cx);
        }
        assert_eq!(workspace.code_scope.as_deref(), Some(LOCAL_SCOPE));
        assert_eq!(workspace.code_form.agent, agent);
        assert_eq!(
            workspace.code_form.provider.is_some(),
            !matches!(connection, Connection::Subscription)
        );
        let provider = workspace.code_form.provider.clone();
        if let Some(id) = &provider {
            assert_eq!(Progress::load(), Some(Progress::Provider(id.clone())));
            workspace.leave_local(Auth::SignedOut { error: None }, window, cx);
            workspace.enter_local(Entry::Launch, window, cx);
        }
        provider
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert_eq!(workspace.code_form.provider, provider);
        workspace.open_new_code_in(project, window, cx);
        if matches!(connection, Connection::Key(KeyVendor::Openai)) {
            workspace.open_code("earlier-session", window, cx);
            assert!(matches!(workspace.view, View::Code(_)));
            assert!(workspace.code_form.starting.is_none());
            workspace.open_new_code(window, cx);
        }
        assert_eq!(workspace.code_form.provider, provider);
        if provider.is_some() {
            assert!(
                workspace.start_blocked(),
                "a saved provider waits for its agent's install check"
            );
            workspace.code_agents = vec![nexus_workbench::api::AgentInfo {
                agent,
                path: None,
                version: None,
                problem: None,
                models: None,
            }];
            assert!(workspace.start_blocked(), "a missing agent prevents Send");
            workspace.code_agents[0].path = Some("/fixture/agent".into());
        }
        if matches!(connection, Connection::RemovedServer) {
            let settings = nexus_desktop_core::provider_settings::ProviderSettingsStore::in_config(config);
            settings
                .remove(
                    &nexus_desktop_core::provider_keys::OsProviderKeys,
                    provider.as_ref().unwrap(),
                )
                .unwrap();
            workspace
                .composer
                .update(cx, |input, cx| input.set_value("Keep this draft.", window, cx));
            workspace.submit_code("Keep this draft.".into(), false, window, cx);
        } else {
            workspace.submit_code("Reply with OK.".into(), false, window, cx);
            if matches!(connection, Connection::Key(KeyVendor::Openai)) {
                assert!(workspace.code_form.provider_validation.is_some());
                workspace.draft.files.push(crate::attachments::Attachment {
                    id: 1,
                    name: "added-during-validation.txt".into(),
                    bytes: Arc::new(b"fixture".to_vec()),
                    media_type: None,
                    thumbnail: None,
                    artifact: None,
                });
            }
        }
    });
    cx.run_until_parked();
    if matches!(connection, Connection::Key(KeyVendor::Openai)) {
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                matches!(workspace.view, View::NewCode),
                "editing an attachment stops the pending Send"
            );
            assert_eq!(workspace.draft.files.len(), 1);
            workspace.draft.files.clear();
            workspace.submit_code("Reply with OK.".into(), false, window, cx);
        });
        cx.run_until_parked();
    }
    if matches!(connection, Connection::RemovedServer) {
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.start_blocked());
            assert!(workspace.code_form.provider_selection_error().is_some());
            assert!(matches!(workspace.view, View::NewCode));
            assert_eq!(workspace.composer.read(cx).value().as_str(), "Keep this draft.");
            workspace.open_settings(crate::workspace::SettingsSection::LocalProviders, window, cx);
            assert!(matches!(
                workspace.view,
                View::Settings(crate::workspace::SettingsSection::LocalProviders)
            ));
            workspace.back_to_app(window, cx);
            assert!(matches!(workspace.view, View::NewCode));
            assert_eq!(workspace.composer.read(cx).value().as_str(), "Keep this draft.");
        });
        return None;
    }
    if let Some(created) = first_created() {
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(matches!(workspace.view, View::Code(_)));
            workspace.on_code_events(vec![created], window, cx);
            workspace.open_new_code(window, cx);
            assert!(
                workspace.code_form.provider.is_none(),
                "a second form has no preselection"
            );
            if matches!(connection, Connection::Key(KeyVendor::Openai)) {
                assert_eq!(
                    workspace.code_form.agent,
                    SessionAgent::Codex,
                    "the project default still applies"
                );
                assert_eq!(workspace.code_form.effective_via(), Via::Subscription);
            }
            assert_local_relaunch_skips_chooser(workspace, window, cx);
        });
    }
    provider
}

#[test]
fn first_connection_round_trips_only_progress_and_the_saved_id() {
    let id = InstanceId::new("fixture".into());
    for state in [Progress::Done, Progress::Provider(id)] {
        let encoded = serde_json::to_value(&state).unwrap();
        assert_eq!(serde_json::from_value::<Progress>(encoded).unwrap(), state);
    }
}

pub(crate) fn check_concurrent_setup(runner: Rc<CodeRunner>, gateway: &str, config: &Path) {
    let mut app = TestAppContext::single();
    app.executor().allow_parking();
    let (first, _) = fixture(&mut app, gateway, runner.clone());
    let (second, _) = fixture(&mut app, gateway, runner.clone());
    app.update(|cx| {
        local_providers::install_test_store(config, Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys), cx);
    });
    for (workspace, vendor) in [(&first, KeyVendor::Anthropic), (&second, KeyVendor::Openai)] {
        let window = workspace.read_with(&app, |workspace, _| workspace.window_handle);
        window
            .update(&mut app, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.continue_without_account(window, cx);
                    workspace.choose_connection(ProviderKind::ApiKey, window, cx);
                    workspace.open_connection_form(SetupForm::Key(vendor), window, cx);
                });
            })
            .unwrap();
    }
    for workspace in [&first, &second] {
        let suspend = workspace == &first;
        let window = workspace.read_with(&app, |workspace, _| workspace.window_handle);
        window
            .update(&mut app, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    let Some(Page::Form { editor, .. }) = &workspace.local_mode.connection else {
                        panic!("open API key form");
                    };
                    local_providers::submit_test_setup(editor, None, Some("nexus-fixture-key"), window, cx);
                    if suspend {
                        assert!(workspace.connection_save_pending(cx));
                        // Pause for browser sign-in without making the intentional Gateway request.
                        workspace.local_mode.signing_in = true;
                        workspace.leave_local(Auth::Checking, window, cx);
                        assert!(workspace.connection_save_pending(cx));
                    }
                });
            })
            .unwrap();
        app.run_until_parked();
        if suspend {
            window
                .update(&mut app, |_, window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        assert!(workspace.code_form.provider_loading());
                        workspace.finish_sign_in_attempt(None, window, cx);
                    });
                })
                .unwrap();
            app.run_until_parked();
        }
    }
    let first_provider = first.read_with(&app, |workspace, _| {
        assert_eq!(workspace.code_form.agent, SessionAgent::Claude);
        workspace.code_form.provider.clone().expect("first window's saved key")
    });
    second.read_with(&app, |workspace, _| {
        assert_eq!(workspace.code_form.agent, SessionAgent::Codex);
        assert_ne!(
            workspace
                .code_form
                .provider
                .as_ref()
                .expect("second window's saved key"),
            &first_provider
        );
    });
    assert_eq!(Progress::load(), Some(Progress::Provider(first_provider)));

    let draft = "first-provider-in-flight";
    let first_window = first.read_with(&app, |workspace, _| workspace.window_handle);
    first_window
        .update(&mut app, |_, window, cx| {
            first.update(cx, |workspace, cx| {
                workspace.open_code(draft, window, cx);
                workspace.code_form.starting = Some(draft.into());
                workspace.local_mode.starts.insert(draft.into(), None);
                workspace.open_new_code(window, cx);
                assert!(workspace.code_form.provider.is_none());
            });
        })
        .unwrap();
    let (third, cx) = fixture(&mut app, gateway, runner);
    third.update_in(cx, |workspace, window, cx| {
        workspace.continue_without_account(window, cx);
        assert!(!workspace.code_form.provider_loading());
        assert!(workspace.code_form.provider.is_none());
    });
    first_window
        .update(&mut app, |_, window, cx| {
            first.update(cx, |workspace, cx| {
                workspace.on_code_events(
                    vec![CodeEvent::Created {
                        draft: draft.into(),
                        id: "first-provider-session".into(),
                    }],
                    window,
                    cx,
                );
                assert!(workspace.local_mode.pending_first_provider.is_none());
                assert_eq!(Progress::load(), Some(Progress::Done));
            });
        })
        .unwrap();
    app.run_until_parked();
}

pub(crate) fn check_sign_in_route(runner: Rc<CodeRunner>, gateway: &str, enabled: bool) {
    let mut app = TestAppContext::single();
    let (workspace, cx) = fixture(&mut app, gateway, runner);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.continue_without_account(window, cx);
        if !enabled {
            assert!(workspace.local_mode.connection.is_none());
            assert!(workspace.code_scope.is_none());
            assert!(Progress::load().is_none());
            return;
        }
        workspace.choose_connection(ProviderKind::NexusCloud, window, cx);
        assert!(workspace.local_mode.connection.is_none());
        assert!(matches!(workspace.auth, Auth::Checking));
        assert_eq!(workspace.code_scope.as_deref(), Some(LOCAL_SCOPE));
        assert!(Progress::load().is_none());
        workspace.handle(crate::gateway::Event::SignedIn, window, cx);
        assert!(workspace.onboarding.active());
    });
}

#[gpui_kit::test]
fn a_second_window_can_claim_the_first_form_only_after_the_owner_leaves(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let (runner, _) = crate::code_run::test_runner(data.path().to_path_buf(), LOCAL_SCOPE);
    let (first, _) = fixture(cx, "http://127.0.0.1:9", Rc::new(runner));
    let other = first.read_with(cx, |workspace, _| workspace.runner.as_ref().unwrap().clone());
    let (second, _) = fixture(cx, "http://127.0.0.1:9", other);
    let mut owner = super::ProviderFormOwner::default();
    assert!(owner.claim(&first));
    assert!(!owner.claim(&second));
    assert!(!owner.release(&second));
    assert!(owner.release(&first));
    assert!(owner.claim(&second));
}

#[gpui_kit::test]
fn first_connection_cancels_provider_validation_after_sign_in(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), LOCAL_SCOPE);
    let (workspace, cx) = fixture(cx, "http://127.0.0.1:9", Rc::new(runner));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.continue_without_account(window, cx);
        workspace.open_new_code(window, cx);
        workspace.code_form.provider_validation = Some(Task::ready(()));
        workspace.sign_in_from_local(window, cx);
        workspace.handle(crate::gateway::Event::SignedIn, window, cx);
        assert!(matches!(workspace.auth, Auth::SignedIn));
        assert!(workspace.code_form.provider_validation.is_none());
    });
}

#[gpui_kit::test]
fn first_connection_keeps_a_rejected_start_in_its_original_project(cx: &mut TestAppContext) {
    use crate::workspace::local_mode::PendingProviderStart;

    let data = tempfile::tempdir().unwrap();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), LOCAL_SCOPE);
    let (workspace, cx) = fixture(cx, "http://127.0.0.1:9", Rc::new(runner));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.local_mode.isolated = true;
        workspace.continue_without_account(window, cx);
        // The test supplies its launch result. Neither synthetic project needs a Workbench request.
        workspace.runner = None;
        workspace.open_new_code_in("original".into(), window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Keep this first prompt.", window, cx));
        workspace.sync_draft(window, cx);
        let submission = workspace.draft_submission(cx).expect("local draft");
        workspace.local_mode.starts.insert(
            "draft-one".into(),
            Some(PendingProviderStart {
                project: "original".into(),
                submission: submission.clone(),
            }),
        );
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        workspace.draft_submitted(Some(submission), window, cx);
        workspace.open_new_code_in("other".into(), window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("New project draft.", window, cx));

        let failed = CodeEvent::StartFailed {
            draft: "draft-one".into(),
            error: "The server no longer exists.".into(),
        };
        assert!(workspace.waits_for(&failed));
        workspace.on_code_events(vec![failed], window, cx);
        assert_eq!(workspace.composer.read(cx).value().as_str(), "New project draft.");
        assert!(workspace.failed_provider_draft().is_none());
        workspace.open_new_code_in("original".into(), window, cx);
        assert_eq!(workspace.composer.read(cx).value().as_str(), "Keep this first prompt.");
        assert_eq!(workspace.failed_provider_draft(), Some("The server no longer exists."));
        assert!(workspace.start_blocked());
    });
}
