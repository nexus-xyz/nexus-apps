#[path = "budgets_test_backend.rs"]
pub mod backend;
use crate::pages::{Ctx, Theme};
use backend::BackendFixture;
use gpui_kit::{
    AnyView, AppContext, Context, IntoElement, ParentElement, Render, ScrollHandle, Styled, TestAppContext, Window,
    WindowAppearance, div,
};
use std::{rc::Rc, sync::Arc};
pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::System,
            WindowAppearance::Light,
        )));
    });
}
pub fn ctx(window: &Window, gateway: Arc<BackendFixture>) -> Ctx {
    Ctx {
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
            code: Rc::new(|_| crate::pages::CodeAccess::default()),
            new_code_session: Rc::new(|_, _| {}),
        },
        gateway: crate::workspace::gateway_access::AccountGateway::fixture(gateway),
        org: "example".into(),
        org_name: "Example".into(),
        email: "a@example.test".into(),
        new_cloud_session: Rc::new(|_, _| {}),
        team: Rc::new(|_| Default::default()),
        watch: Rc::new(|_, _, _| {}),
        fixer: crate::setup::Fixer::inert(),
    }
}
pub fn answer(pending: backend::Pending, value: serde_json::Value, cx: &mut TestAppContext) {
    let consumed = pending.consumed.clone();
    pending.answer.send(Ok(value)).unwrap();
    cx.run_until_parked();
    assert!(
        consumed.load(std::sync::atomic::Ordering::SeqCst),
        "the exact pending response must run"
    );
}

pub struct Host(pub AnyView);
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}
