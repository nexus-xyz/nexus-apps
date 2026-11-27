use super::{Event, Gateway};

fn runtime() -> tokio::runtime::Runtime {
    nexus_client::install_tls();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn sign_in(gateway: &Gateway, token: &str) {
    crate::keychain::save_token(gateway.url(), token);
    gateway.set_token(Some(token.into()));
}

fn assert_token(gateway: &Gateway, expected: Option<&str>) {
    assert_eq!(gateway.client().options().api_key.as_deref(), expected);
    assert_eq!(crate::keychain::token(gateway.url()).unwrap().as_deref(), expected);
}

#[test]
fn refusal_for_a_saved_token_cannot_clear_its_replacement() {
    let runtime = runtime();
    let (gateway, mut events) = Gateway::at(runtime.handle().clone(), "https://sign-in-replacement.test");
    sign_in(&gateway, "nxc_saved");
    let saved = gateway.account_epoch();
    sign_in(&gateway, "nxc_replacement");

    runtime.block_on(gateway.refuse_saved(&saved));
    assert_token(&gateway, Some("nxc_replacement"));
    assert!(events.try_recv().is_err());

    let replacement = gateway.account_epoch();
    runtime.block_on(gateway.refuse_saved(&replacement));
    assert_token(&gateway, None);
    assert!(matches!(events.try_recv(), Ok(Event::SignedOut)));
    assert!(matches!(events.try_recv(), Ok(Event::SignInFailed(_))));
}

#[test]
fn switching_away_and_back_retires_a_pending_refusal() {
    let runtime = runtime();
    let (old, mut events) = Gateway::at(runtime.handle().clone(), "https://sign-in-switch.test");
    sign_in(&old, "nxc_saved");
    let saved = old.account_epoch();

    let (away, _) = old.switched("https://sign-in-away.test");
    let (current, _) = away.switched(old.url());
    sign_in(&current, "nxc_current");

    runtime.block_on(old.refuse_saved(&saved));
    assert_token(&current, Some("nxc_current"));
    assert!(events.try_recv().is_err());
}
