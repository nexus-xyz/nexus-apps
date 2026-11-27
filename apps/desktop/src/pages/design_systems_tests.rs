//! Exercise the page's real Gateway requests and GPUI callbacks, with controlled HTTP replies.
use super::{Ctx, DesignSystemsPage, Load};
use crate::gateway::Gateway;
use gpui_kit::{Entity, ScrollHandle, TestAppContext, VisualContext, VisualTestContext, WindowAppearance};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct Gate(Mutex<bool>, Condvar);
impl Gate {
    fn release(&self) {
        *self.0.lock().unwrap() = true;
        self.1.notify_all();
    }
    fn wait(&self) {
        let (released, _) = self
            .1
            .wait_timeout_while(self.0.lock().unwrap(), std::time::Duration::from_secs(5), |released| {
                !*released
            })
            .unwrap();
        assert!(*released, "the test did not release its HTTP reply");
    }
}

struct Server {
    runtime: tokio::runtime::Runtime,
    url: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
    a: Arc<Gate>,
    write: Arc<Gate>,
    fail_write: Arc<AtomicBool>,
    default: Arc<Mutex<String>>,
    thread: Option<std::thread::JoinHandle<Vec<std::thread::JoinHandle<()>>>>,
}
impl Server {
    fn new(hold_a: bool) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let a = Arc::new(Gate::default());
        if !hold_a {
            a.release();
        }
        let write = Arc::new(Gate::default());
        let fail_write = Arc::new(AtomicBool::new(false));
        let default = Arc::new(Mutex::new("a".to_string()));
        let thread = std::thread::spawn({
            let seen = seen.clone();
            let a = a.clone();
            let write = write.clone();
            let fail_write = fail_write.clone();
            let default = default.clone();
            move || {
                let mut workers = Vec::new();
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut head = Vec::new();
                    while !head.windows(4).any(|part| part == b"\r\n\r\n") {
                        let mut buffer = [0; 1024];
                        let count = stream.read(&mut buffer).unwrap();
                        if count == 0 {
                            break;
                        }
                        head.extend_from_slice(&buffer[..count]);
                    }
                    let head = String::from_utf8_lossy(&head);
                    let mut words = head.split_whitespace();
                    let method = words.next().unwrap_or("").to_string();
                    let path = words.next().unwrap_or("").to_string();
                    if path == "/stop" {
                        break;
                    }
                    seen.lock().unwrap().push((method.clone(), path.clone()));
                    let a = a.clone();
                    let write = write.clone();
                    let fail = fail_write.clone();
                    let default = default.clone();
                    workers.push(std::thread::spawn(move || {
                        let (status, body) = match (method.as_str(), path.as_str()) {
                            (_, "/v1/tenants/me") => ("200 OK", json!({"permissions":["chat.use","org.settings"]})),
                            (_, "/v1/design-systems") => {
                                let default = default.lock().unwrap().clone();
                                ("200 OK", json!({"defaultId":default,"designSystems":[system("a", &default),system("b", &default)]}))
                            }
                            ("POST", "/v1/design-systems/b/default") => {
                                write.wait();
                                if fail.load(Ordering::SeqCst) {
                                    ("403 Forbidden", json!({"error":{"code":"forbidden","message":"Default refused"}}))
                                } else {
                                    *default.lock().unwrap() = "b".into();
                                    ("200 OK", system("b", "b"))
                                }
                            }
                            (_, "/v1/design-systems/a") => { a.wait(); ("200 OK", detail("a")) }
                            (_, "/v1/design-systems/b") => ("200 OK", detail("b")),
                            _ => ("404 Not Found", json!({"error":{"code":"not_found","message":"Unknown fixture route"}})),
                        };
                        let body = body.to_string();
                        let response = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                        let _ = stream.write_all(response.as_bytes());
                    }));
                }
                workers
            }
        });
        Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            url,
            seen,
            a,
            write,
            fail_write,
            default,
            thread: Some(thread),
        }
    }
    fn posts(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == "POST")
            .count()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.a.release();
        self.write.release();
        if let Ok(mut stream) = std::net::TcpStream::connect(self.url.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nhost: fake\r\n\r\n");
        }
        for worker in self.thread.take().unwrap().join().unwrap() {
            worker.join().unwrap();
        }
    }
}
fn system(id: &str, default: &str) -> Value {
    json!({"id":id,"name":format!("System {id}"),"builtIn":id=="a","isDefault":id==default,
        "source":"fixture","variableCount":1,"componentCount":0})
}
fn detail(id: &str) -> Value {
    let mut value = system(id, "a");
    value["variables"] = json!([{"name":format!("--{id}"),"value":"#fff","selector":":root","kind":"color"}]);
    value["notes"] = json!(format!("Notes {id}"));
    value["components"] = json!([]);
    value
}
fn fixture<'a>(
    cx: &'a mut TestAppContext,
    server: &Server,
) -> (Entity<DesignSystemsPage>, Gateway, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let gateway = Gateway::for_view_test(server.runtime.handle().clone())
        .switched(&server.url)
        .0;
    let backend = Arc::new(gateway.clone());
    let (page, cx) = cx.add_window_view(|window, cx| {
        let ctx = Ctx {
            device: crate::pages::DeviceCtx {
                local_workspace: false,
                settings_fields: crate::settings_fields::SettingsFields::new(window, ScrollHandle::new()),
                gateway_url: "http://gateway.invalid".into(),
                navigate: Rc::new(|_, _, _| {}),
                navigate_settings: Rc::new(|_, _, _| {}),
                switch_gateway: Rc::new(|_, _, _| Ok(())),
                settings_saved: Rc::new(|_, _| {}),
                restart_for_update: Rc::new(|_| Ok(())),
                restart_for_service: Rc::new(|_| Ok(())),
                code: Rc::new(|_| super::super::CodeAccess::default()),
                new_code_session: Rc::new(|_, _| {}),
            },
            gateway: crate::workspace::gateway_access::AccountGateway::fixture(backend),
            org: "fixture".into(),
            org_name: "Fixture".into(),
            email: "fixture@example.test".into(),
            new_cloud_session: Rc::new(|_, _| {}),
            team: Rc::new(|_| Default::default()),
            watch: Rc::new(|_, _, _| {}),
            fixer: crate::setup::Fixer::inert(),
        };
        cx.set_global(super::super::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::System,
            WindowAppearance::Light,
        )));
        DesignSystemsPage::new(ctx, cx)
    });
    (page, gateway, cx)
}
fn wait_for(
    server: &Server,
    page: &Entity<DesignSystemsPage>,
    cx: &mut VisualTestContext,
    predicate: impl Fn(&DesignSystemsPage) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if page.read_with(cx, |page, _| predicate(page)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "design-system request did not settle"
        );
        server.runtime.block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        });
    }
}
fn selected(page: &DesignSystemsPage) -> Option<&str> {
    match &page.detail {
        Load::Ready(detail) => Some(detail.id.as_str()),
        _ => None,
    }
}
fn default(page: &DesignSystemsPage) -> Option<&str> {
    match &page.systems {
        Load::Ready(systems) => Some(systems.default_id.as_str()),
        _ => None,
    }
}

#[gpui_kit::test]
fn design_system_late_a_detail_cannot_replace_selected_b(cx: &mut TestAppContext) {
    let server = Server::new(true);
    let (page, _, cx) = fixture(cx, &server);
    wait_for(&server, &page, cx, |page| page.detail_id.as_deref() == Some("a"));
    let a_revision = page.read_with(cx, |page, _| page.detail_revision);
    page.update(cx, |page, cx| page.pick("b".into(), cx));
    wait_for(&server, &page, cx, |page| selected(page) == Some("b"));
    server.a.release();
    wait_for(&server, &page, cx, |page| {
        page.detail_responses
            .iter()
            .any(|(revision, id, _)| *revision == a_revision && id == "a")
    });
    page.read_with(cx, |page, _| {
        assert!(
            page.detail_responses
                .iter()
                .any(|(revision, id, ok)| *revision == a_revision && id == "a" && *ok),
            "A must complete successfully before isolation is asserted"
        );
        assert_eq!(selected(page), Some("b"));
        let Load::Ready(detail) = &page.detail else {
            panic!("tokens not ready")
        };
        assert_eq!(detail.variables[0].name, "--b");
    });
}

#[gpui_kit::test]
fn design_system_failed_default_and_duplicate_click_preserve_saved_default(cx: &mut TestAppContext) {
    let server = Server::new(false);
    server.fail_write.store(true, Ordering::SeqCst);
    let (page, _, cx) = fixture(cx, &server);
    wait_for(&server, &page, cx, |page| {
        page.can_manage() && selected(page) == Some("a")
    });
    page.update(cx, |page, cx| {
        page.make_default("b".into(), cx);
        page.make_default("b".into(), cx);
        assert_eq!(default(page), Some("a"));
    });
    wait_for(&server, &page, cx, |_| server.posts() == 1);
    server.write.release();
    wait_for(&server, &page, cx, |page| {
        page.busy.is_none() && matches!(page.outcome, Some(Err(_)))
    });
    page.read_with(cx, |page, _| assert_eq!(default(page), Some("a")));
    assert_eq!(server.posts(), 1);
    assert_eq!(server.default.lock().unwrap().as_str(), "a");
    // A retry succeeds only after the first request has finished.
    server.fail_write.store(false, Ordering::SeqCst);
    page.update(cx, |page, cx| page.make_default("b".into(), cx));
    wait_for(&server, &page, cx, |page| {
        default(page) == Some("b") && matches!(page.outcome, Some(Ok(_)))
    });
    assert_eq!(server.posts(), 2);
}

#[gpui_kit::test]
fn design_system_replacing_settings_page_disposes_the_old_account_view(cx: &mut TestAppContext) {
    let server = Server::new(true);
    let (page, _, cx) = fixture(cx, &server);
    wait_for(&server, &page, cx, |page| page.detail_id.as_deref() == Some("a"));
    // The settings shell rebuilds page entities after an account or organization change.
    let old = page.downgrade();
    let mut next = page.read_with(cx, |page, _| page.ctx.clone());
    next.org = "second-organization".into();
    let replacement = cx.replace_root_view(|_, cx| DesignSystemsPage::new(next, cx));
    drop(page);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert!(old.upgrade().is_none());
    replacement.update(cx, |page, cx| page.pick("b".into(), cx));
    wait_for(&server, &replacement, cx, |page| selected(page) == Some("b"));
    // The disposed entity cannot receive its pending callback. This is an entity-lifetime
    // check; the stale-response test above separately observes a completed client callback.
    server.a.release();
    assert!(old.upgrade().is_none());
    replacement.read_with(cx, |page, _| {
        assert_eq!(page.ctx.org, "second-organization");
        assert_eq!(selected(page), Some("b"));
    });
}
