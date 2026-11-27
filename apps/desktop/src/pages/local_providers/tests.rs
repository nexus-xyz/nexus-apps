use core::prelude::v1::test;
use std::sync::Mutex;

use nexus_desktop_core::provider_settings::ProviderSettingsStore;
use nexus_providers::{KeyRef, KeyVendor, Secret};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use super::form::{absent_from_list, matches_checked_address};
use super::store::Edit;
use super::*;

#[derive(Default)]
pub(super) struct Keys {
    values: Mutex<HashMap<KeyRef, Secret>>,
    locked: std::sync::atomic::AtomicBool,
}

impl Keys {
    fn lock(&self, locked: bool) {
        self.locked.store(locked, std::sync::atomic::Ordering::SeqCst);
    }

    fn available(&self) -> Result<(), ProviderError> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ProviderError::KeychainUnread("Unlock the OS key store.".into()));
        }
        Ok(())
    }
}

impl KeySource for Keys {
    fn read(&self, key: &KeyRef) -> Result<Option<Secret>, ProviderError> {
        self.available()?;
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(key)
            .map(|value| Secret::new(value.expose().to_owned())))
    }

    fn write(&self, key: &KeyRef, secret: Secret) -> Result<(), ProviderError> {
        self.available()?;
        self.values.lock().unwrap().insert(key.clone(), secret);
        Ok(())
    }

    fn delete(&self, key: &KeyRef) -> Result<(), ProviderError> {
        self.available()?;
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

fn key() -> Secret {
    Secret::new("fixture-secret".into())
}

#[test]
fn editor_operations_save_metadata_and_keep_provider_identity_on_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let file = ProviderSettingsStore::in_config(dir.path());
    let keys = Keys::default();
    Edit::AddKey(KeyVendor::Anthropic, key()).apply(&file, &keys).unwrap();
    let old = file.read().unwrap().api_keys.remove(0);
    Edit::RotateKey(old.clone(), Secret::new("replacement-fixture".into()))
        .apply(&file, &keys)
        .unwrap();
    let rotated = file.read().unwrap().api_keys.remove(0);
    assert_eq!(rotated.id, old.id);
    assert_eq!(rotated.vendor, KeyVendor::Anthropic);
    assert_ne!(rotated.key, old.key);
    assert!(keys.read(&old.key).unwrap().is_none());
    assert_eq!(
        keys.read(&rotated.key).unwrap().unwrap().expose(),
        "replacement-fixture"
    );
    let contents = std::fs::read_to_string(dir.path().join("providers.json")).unwrap();
    assert!(!contents.contains("fixture"));
    Edit::Remove(Saved::Key(rotated)).apply(&file, &keys).unwrap();
    assert!(file.read().unwrap().api_keys.is_empty());
    assert!(keys.values.lock().unwrap().is_empty());
}

#[test]
fn server_edits_require_key_removal_before_address_change_and_bind_rotation_to_seen_address() {
    let dir = tempfile::tempdir().unwrap();
    let file = ProviderSettingsStore::in_config(dir.path());
    let keys = Keys::default();
    let address = "http://localhost:4242/v1";
    Edit::AddServer {
        name: "Models".into(),
        address: address.into(),
        models: vec!["small".into()],
        key: Some(key()),
    }
    .apply(&file, &keys)
    .unwrap();
    let server = file.read().unwrap().servers.remove(0);
    let change = |expected: ServerSetting| Edit::UpdateServer {
        expected,
        name: "Other models".into(),
        address: "http://localhost:4343/v1".into(),
        models: vec!["large".into()],
    };
    assert!(change(server.clone()).apply(&file, &keys).is_err());
    Edit::UpdateServer {
        expected: server.clone(),
        name: "Renamed".into(),
        address: address.into(),
        models: vec!["medium".into()],
    }
    .apply(&file, &keys)
    .unwrap();
    assert_eq!(file.read().unwrap().servers[0].models, ["medium"]);
    Edit::ClearServer(file.read().unwrap().servers.remove(0))
        .apply(&file, &keys)
        .unwrap();
    assert!(change(server.clone()).apply(&file, &keys).is_err());
    assert_eq!(file.read().unwrap().servers[0].models, ["medium"]);
    change(file.read().unwrap().servers.remove(0))
        .apply(&file, &keys)
        .unwrap();
    assert!(Edit::RotateServer(server.clone(), key()).apply(&file, &keys).is_err());
    let current = file.read().unwrap().servers.remove(0);
    assert_eq!(current.name, "Other models");
    assert_eq!(current.models, ["large"]);
    assert!(current.key.is_none());
    Edit::RotateServer(current, key()).apply(&file, &keys).unwrap();
    assert!(file.read().unwrap().servers[0].key.is_some());
    Edit::Remove(Saved::Server(file.read().unwrap().servers.remove(0)))
        .apply(&file, &keys)
        .unwrap();
    assert!(file.read().unwrap().servers.is_empty());
    assert!(keys.values.lock().unwrap().is_empty());
}

#[test]
fn stale_key_drafts_cannot_change_a_second_writers_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let file = ProviderSettingsStore::in_config(dir.path());
    let writer = ProviderSettingsStore::in_config(dir.path());
    let keys = Keys::default();

    let id = file.add_api_key(&keys, KeyVendor::Openai, key()).unwrap();
    let original = file.read().unwrap().api_keys.remove(0);
    writer.rotate_api_key(&keys, &id, key()).unwrap();
    let replacement = file.read().unwrap().api_keys.remove(0);
    assert!(Edit::RotateKey(original.clone(), key()).apply(&file, &keys).is_err());
    assert!(Edit::Remove(Saved::Key(original)).apply(&file, &keys).is_err());
    assert_eq!(file.read().unwrap().api_keys, [replacement]);

    let address = "http://localhost:4242/v1";
    let id = file
        .add_server("Models".into(), address, vec![], Some((&keys, key())))
        .unwrap();
    let original = file.read().unwrap().servers.remove(0);
    writer.rotate_server_key(&keys, &id, address, key()).unwrap();
    let replacement = file.read().unwrap().servers.remove(0);
    assert!(Edit::RotateServer(original.clone(), key()).apply(&file, &keys).is_err());
    assert!(Edit::ClearServer(original.clone()).apply(&file, &keys).is_err());
    assert!(Edit::Remove(Saved::Server(original)).apply(&file, &keys).is_err());
    assert_eq!(file.read().unwrap().servers, [replacement]);
}

#[gpui_kit::test]
fn startup_retries_pending_cleanup_and_publishes_status(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(Keys::default());
    let file = ProviderSettingsStore::in_config(dir.path());
    let id = file.add_api_key(keys.as_ref(), KeyVendor::Openai, key()).unwrap();
    keys.lock(true);
    assert!(file.remove(keys.as_ref(), &id).is_err());
    let (settings, pending) = file.read_with_cleanup_status().unwrap();
    assert!(settings.api_keys.is_empty());
    assert!(pending);
    assert!(
        Edit::AddKey(KeyVendor::Openai, key())
            .apply(&file, keys.as_ref())
            .is_err()
    );
    assert!(file.read().unwrap().api_keys.is_empty());

    let start = |cx: &mut TestAppContext| {
        let store = cx.update(|cx| cx.new(|_| Store::new(dir.path(), keys.clone())));
        cx.update(|cx| store.update(cx, Store::startup_cleanup));
        cx.run_until_parked();
        store
    };
    let locked = start(cx);
    cx.read(|cx| {
        assert!(matches!(locked.read(cx).snapshot, Some(Ok((_, true)))));
    });
    keys.lock(false);
    let reopened = start(cx);
    cx.read(|cx| {
        assert!(matches!(reopened.read(cx).snapshot, Some(Ok((_, false)))));
    });
    assert!(keys.values.lock().unwrap().is_empty());
}

async fn request(listener: TcpListener) -> TcpStream {
    let (mut conn, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(conn.read_u8().await.unwrap());
    }
    let request = String::from_utf8(request).unwrap();
    assert!(request.starts_with("GET /v1/models "));
    assert!(!request.to_ascii_lowercase().contains("authorization:"));
    conn
}

async fn respond(mut conn: TcpStream, status: &str, body: &str) {
    conn.write_all(
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<ProviderSettingsStore>,
    Arc<Keys>,
    Saved,
    TcpListener,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let file = Arc::new(ProviderSettingsStore::in_config(dir.path()));
    let id = file.add_server("Test models".into(), &address, vec![], None).unwrap();
    let saved = Saved::from_settings(&file.read().unwrap(), &id).unwrap();
    (dir, file, Arc::new(Keys::default()), saved, listener)
}

#[tokio::test]
async fn check_uses_the_server_models_list_and_supports_typed_fallback() {
    for (status, body, expected) in [
        (
            "200 OK",
            r#"{"data":[{"id":"small"},{"id":"large"},{"id":"small"}]}"#,
            Ok(vec!["large".into(), "small".into()]),
        ),
        ("200 OK", "", Ok(vec![])),
        ("200 OK", "not a model list", Ok(vec![])),
        (
            "401 Unauthorized",
            "credential detail to discard",
            Err(ProviderError::Refused(
                "The provider refused this credential. Check the saved key.".into(),
            )),
        ),
    ] {
        let (_dir, file, keys, saved, listener) = fixture().await;
        // Reading provider metadata does not open the endpoint.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), listener.accept())
                .await
                .is_err()
        );
        let endpoint = tokio::spawn(async move { respond(request(listener).await, status, body).await });
        let result = check_saved(file, keys, saved).await.unwrap();
        assert_eq!(result, expected);
        endpoint.await.unwrap();
        if let Ok(models) = result {
            assert!(!absent_from_list(&models, "small"));
            assert_eq!(absent_from_list(&models, "unknown"), !models.is_empty());
        }
    }

    let (_dir, file, keys, saved, listener) = fixture().await;
    drop(listener);
    assert!(matches!(
        check_saved(file, keys, saved).await,
        Some(Err(ProviderError::Unreachable { .. }))
    ));
}

#[tokio::test]
async fn check_discards_an_answer_after_a_second_writer_edits_or_removes_the_row() {
    for remove in [false, true] {
        let (dir, file, keys, saved, listener) = fixture().await;
        let id = saved.id().clone();
        let Saved::Server(server) = &saved else { unreachable!() };
        let address = server.base_url.clone();
        let (asked, seen) = oneshot::channel();
        let (edited, release) = oneshot::channel();
        let endpoint = tokio::spawn(async move {
            let conn = request(listener).await;
            asked.send(()).unwrap();
            release.await.unwrap();
            respond(conn, "200 OK", r#"{"data":[{"id":"stale"}]}"#).await;
        });
        let check = tokio::spawn(check_saved(file, keys.clone(), saved));
        tokio::time::timeout(std::time::Duration::from_secs(2), seen)
            .await
            .unwrap()
            .unwrap();
        let writer = ProviderSettingsStore::in_config(dir.path());
        if remove {
            writer.remove(keys.as_ref(), &id).unwrap();
        } else {
            writer.edit_server(&id, "Changed".into(), &address, vec![]).unwrap();
        }
        edited.send(()).unwrap();
        assert!(check.await.unwrap().is_none());
        endpoint.await.unwrap();
    }
}

#[test]
fn model_offers_apply_only_to_the_address_that_answered_check() {
    let checked = Some("http://localhost:8000/v1");
    assert!(matches_checked_address(checked, " http://LOCALHOST:8000/v1/ "));
    assert!(!matches_checked_address(checked, "http://localhost:8001/v1"));
    assert!(!matches_checked_address(checked, ""));
    assert!(!matches_checked_address(None, "http://localhost:8000/v1"));
}

#[tokio::test]
async fn check_deadline_includes_a_blocked_keychain_read() {
    struct BlockingKeychain {
        entered: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl KeySource for BlockingKeychain {
        fn read(&self, _: &KeyRef) -> Result<Option<Secret>, ProviderError> {
            let _ = self.entered.lock().unwrap().take().unwrap().send(());
            let _ = self.release.lock().unwrap().recv_timeout(Duration::from_secs(2));
            Ok(Some(key()))
        }

        fn write(&self, _: &KeyRef, _: Secret) -> Result<(), ProviderError> {
            unreachable!()
        }

        fn delete(&self, _: &KeyRef) -> Result<(), ProviderError> {
            unreachable!()
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let file = Arc::new(ProviderSettingsStore::in_config(dir.path()));
    let address = "http://localhost:9/v1";
    let keys = Keys::default();
    let id = file
        .add_server("Models".into(), address, vec![], Some((&keys, key())))
        .unwrap();
    let saved = Saved::from_settings(&file.read().unwrap(), &id).unwrap();
    let (entered, waiting) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let keys = Arc::new(BlockingKeychain {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(blocked),
    });
    let check = tokio::spawn(check_saved_with_deadline(file, keys, saved, Duration::from_millis(250)));
    tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .expect("the check starts reading the saved credential")
        .unwrap();
    let result = check.await.unwrap();
    release.send(()).unwrap();
    assert_eq!(
        result,
        Some(Err(ProviderError::Unreachable {
            endpoint: address.into()
        }))
    );
}

#[gpui_kit::test]
fn editor_ignores_checks_with_an_old_request_id(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let id = InstanceId::new("fixture-provider".into());
    cx.update(|window, cx| {
        let editor = cx.new(|cx| Editor::new(None, Some(MachineFacts::default()), window, cx));
        editor.update(cx, |editor, cx| {
            editor.checks.insert(id.clone(), Check::Running(2));
            editor.finish_check(id.clone(), 1, Some(Ok(vec!["old".into()])), &[], cx);
            assert!(matches!(editor.checks.get(&id), Some(Check::Running(2))));
            editor.finish_check(id.clone(), 2, Some(Ok(vec!["new".into()])), &["absent".into()], cx);
            assert!(matches!(
                editor.checks.get(&id),
                Some(Check::Finished { result: Ok(models), missing_models: true }) if models == &["new"]
            ));
            editor.finish_check(id.clone(), 1, None, &[], cx);
            assert!(matches!(editor.checks.get(&id), Some(Check::Finished { .. })));
        });
    });
}

#[gpui_kit::test]
fn editors_sharing_a_store_clear_the_draft_when_another_removes_its_provider(cx: &mut TestAppContext) {
    use super::form::Action;

    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(Keys::default());
    let file = ProviderSettingsStore::in_config(dir.path());
    let id = file.add_api_key(keys.as_ref(), KeyVendor::Openai, key()).unwrap();
    let saved = Saved::from_settings(&file.read().unwrap(), &id).unwrap();
    let cx = cx.add_empty_window();
    let (first, second) = cx.update(|window, cx| {
        let store = cx.new(|_| Store::new(dir.path(), keys));
        (
            cx.new(|cx| Editor::new(Some(store.clone()), Some(MachineFacts::default()), window, cx)),
            cx.new(|cx| Editor::new(Some(store), Some(MachineFacts::default()), window, cx)),
        )
    });
    cx.run_until_parked();
    let secret = cx.update(|window, cx| {
        assert_eq!(first.read(cx).rows.len(), 1);
        assert_eq!(second.read(cx).rows.len(), 1);
        first.update(cx, |first, cx| {
            first.open_form(Action::Rotate(saved.clone()), window, cx);
            let secret = first.form.as_ref().unwrap().secret_input().unwrap().clone();
            secret.update(cx, |input, cx| input.set_value("draft-key", window, cx));
            secret
        })
    });
    cx.update(|window, cx| {
        second.update(cx, |second, cx| {
            second.open_form(Action::Remove(saved, "OpenAI key".into()), window, cx);
            second.submit(window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(first.read(cx).rows.is_empty());
        assert!(second.read(cx).rows.is_empty());
        assert!(first.read(cx).draft_changed);
        assert!(first.read(cx).form.is_none());
        assert!(secret.read(cx).value().is_empty());
    });
}

#[gpui_kit::test]
fn editor_clears_secrets_after_save_cancel_and_release_and_preserves_a_failed_draft(cx: &mut TestAppContext) {
    use super::form::Action;

    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(Keys::default());
    let cx = cx.add_empty_window();
    let editor = cx.update(|window, cx| {
        let store = cx.new(|_| Store::new(dir.path(), keys.clone()));
        cx.new(|cx| Editor::new(Some(store), Some(MachineFacts::default()), window, cx))
    });
    keys.lock(true);
    let secret = cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.open_form(Action::AddServer, window, cx);
            let form = editor.form.as_ref().unwrap();
            let secret = form.secret_input().unwrap().clone();
            let (name, address, model) = form.server_inputs();
            for (input, value) in [
                (name, "My models"),
                (address, "http://localhost:11434/v1"),
                (model, "my-model"),
                (secret.clone(), "  fixture-key  "),
            ] {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            editor.submit(window, cx);
            secret
        })
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(editor.read(cx).form.is_some(), "a refused write preserves the draft");
        assert_eq!(secret.read(cx).value().as_str(), "  fixture-key  ");
        let (_, _, model) = editor.read(cx).form.as_ref().unwrap().server_inputs();
        assert_eq!(model.read(cx).value().as_str(), "my-model");
    });
    keys.lock(false);
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.submit(window, cx)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(editor.read(cx).form.is_none());
        assert!(secret.read(cx).value().is_empty());
    });
    let file = ProviderSettingsStore::in_config(dir.path());
    let server = file.read().unwrap().servers.remove(0);
    assert_eq!(server.models, ["my-model"]);
    assert_eq!(
        keys.read(&server.key.unwrap()).unwrap().unwrap().expose(),
        "fixture-key"
    );

    let open_key = |editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>| {
        editor.open_form(Action::AddKey(KeyVendor::Openai), window, cx);
        let secret = editor.form.as_ref().unwrap().secret_input().unwrap().clone();
        secret.update(cx, |input, cx| input.set_value("draft-key", window, cx));
        secret
    };
    let cancelled = cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let secret = open_key(editor, window, cx);
            editor.close_form(window, cx);
            secret
        })
    });
    cx.update(|_, cx| assert!(cancelled.read(cx).value().is_empty()));
    let released = cx.update(|window, cx| editor.update(cx, |editor, cx| open_key(editor, window, cx)));
    drop(editor);
    cx.update(|_, _| {});
    cx.run_until_parked();
    cx.update(|_, cx| assert!(released.read(cx).value().is_empty()));
}

#[gpui_kit::test]
fn a_draft_is_released_with_its_last_window(cx: &mut TestAppContext) {
    use super::form::Action;

    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::solid_for(false)));
    });
    let dir = tempfile::tempdir().unwrap();
    let (editor, window) = cx.add_window_view(|window, cx| {
        let store = cx.new(|_| Store::new(dir.path(), Arc::new(Keys::default())));
        Editor::new(Some(store), Some(MachineFacts::default()), window, cx)
    });
    let secret = editor.update_in(window, |editor, window, cx| {
        editor.open_form(Action::AddKey(KeyVendor::Openai), window, cx);
        let secret = editor.form.as_ref().unwrap().secret_input().unwrap();
        secret.update(cx, |input, cx| input.set_value("draft-key", window, cx));
        secret.downgrade()
    });
    drop(editor);
    window.update(|window, _| window.remove_window());
    window.run_until_parked();
    assert!(secret.upgrade().is_none());
}

#[gpui_kit::test]
fn a_timed_out_check_refreshes_the_row_after_an_external_removal(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(Keys::default());
    let file = ProviderSettingsStore::in_config(dir.path());
    let id = file.add_api_key(keys.as_ref(), KeyVendor::Openai, key()).unwrap();
    let cx = cx.add_empty_window();
    let editor = cx.update(|window, cx| {
        let store = cx.new(|_| Store::new(dir.path(), keys.clone()));
        cx.new(|cx| Editor::new(Some(store), Some(MachineFacts::default()), window, cx))
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| editor.read(cx).rows.len()), 1);
    file.remove(keys.as_ref(), &id).unwrap();
    cx.update(|_, cx| {
        editor.update(cx, |editor, cx| {
            editor.checks.insert(id.clone(), Check::Running(1));
            editor.finish_check(
                id.clone(),
                1,
                Some(Err(ProviderError::Unreachable {
                    endpoint: "OpenAI".into(),
                })),
                &[],
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(editor.read(cx).rows.is_empty());
        assert!(!editor.read(cx).checks.contains_key(&id));
    });
}
