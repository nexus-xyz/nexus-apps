//! Session launches share the app's active sign-in. These tests never touch the OS credential store.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn host(gateway: &str, client: crate::backend::SessionClient) -> GpuiHost {
    let (events, _) = unbounded();
    GpuiHost {
        local_providers_opt_in: LocalProvidersOptIn::Disabled,
        sign_in: Arc::new(RwLock::new(SignIn {
            gateway: gateway.into(),
            client,
        })),
        scope: Arc::new(RwLock::new(format!("{gateway}|org").into())),
        source_generation: std::sync::atomic::AtomicU64::new(0),
        data: PathBuf::new(),
        prices: Arc::default(),
        agent_models: Arc::default(),
        events: Mutex::new(events),
        titles: Mutex::default(),
        agents: Default::default(),
        app: OnceLock::new(),
        sync: Arc::default(),
    }
}

fn request(scope: &str) -> LaunchRequest {
    LaunchRequest {
        thread_id: "thread".into(),
        agent: SessionAgent::Codex,
        cwd: PathBuf::new(),
        model: None,
        via: Via::Subscription,
        required_payer: None,
        provider: None,
        account_dir: None,
        args: Vec::new(),
        scope: scope.into(),
        prompt_counts: None,
    }
}

#[test]
fn a_launch_reads_one_active_client_and_honors_sign_out() {
    nexus_client::install_tls();
    let client = Client::new(crate::gateway::client_options(GATEWAY, Some("nxc_active".into())));
    let active = Arc::new(Mutex::new(Some(client)));
    let reads = Arc::new(AtomicUsize::new(0));
    let source = {
        let (active, reads) = (active.clone(), reads.clone());
        Arc::new(move || {
            reads.fetch_add(1, Ordering::SeqCst);
            active.lock().unwrap().clone()
        })
    };
    let host = host(GATEWAY, source);
    assert_eq!(host.token().as_deref(), Some("nxc_active"));
    let before = reads.load(Ordering::SeqCst);
    // A scope refusal stops before any network call or agent process. It still passes the sign-in guard.
    let error = futures::executor::block_on(host.launch(request("another|org"))).unwrap_err();
    assert!(error.contains("another organization"), "{error}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        before + 1,
        "one snapshot supplies both client and token"
    );

    *active.lock().unwrap() = None;
    let error = futures::executor::block_on(host.launch(request(&host.scope()))).unwrap_err();
    assert_eq!(error, "Sign in to Nexus to start Codex.");
    assert!(host.session_choice_identity().is_none());
}

#[test]
fn a_client_for_another_gateway_never_authorizes_a_session() {
    nexus_client::install_tls();
    let other = Client::new(crate::gateway::client_options(
        "http://127.0.0.1:10",
        Some("nxc_other".into()),
    ));
    let host = host(GATEWAY, Arc::new(move || Some(other.clone())));
    assert!(host.client().is_none());
    assert!(host.token().is_none());
    assert!(
        host.sign_in().is_none(),
        "the background service receives no mismatched credential"
    );
    let error = futures::executor::block_on(host.launch(request(&host.scope()))).unwrap_err();
    assert_eq!(error, "Sign in to Nexus to start Codex.");
}

#[test]
fn an_early_sign_in_keeps_saved_providers_signed_out() {
    const LOCAL_SCOPE: &str = "|";

    nexus_client::install_tls();
    let client = Client::new(crate::gateway::client_options(GATEWAY, Some("nxc_active".into())));
    let mut host = host(GATEWAY, Arc::new(move || Some(client.clone())));
    host.scope = Arc::new(RwLock::new(LOCAL_SCOPE.into()));
    assert_eq!(
        host.check_saved_provider_owner().unwrap_err(),
        "Sign out of Nexus to use this provider on this computer."
    );
    assert!(
        host.sign_in().is_none(),
        "the service cannot adopt a signed-in local scope"
    );
}

#[test]
fn retrying_an_unreachable_service_keeps_its_displayed_status() {
    use nexus_remote::machine::Status;
    let mut shown = Status::Connecting { attempt: 1 };
    let unreachable = |retry_in_secs| Status::Unreachable {
        error: "offline".into(),
        retry_in_secs,
    };
    assert!(update_presented_service_status(&mut shown, &unreachable(1)));
    assert!(!update_presented_service_status(
        &mut shown,
        &Status::Connecting { attempt: 2 }
    ));
    assert!(!update_presented_service_status(&mut shown, &unreachable(5)));
    assert!(matches!(shown, Status::Unreachable { .. }));
    assert!(update_presented_service_status(
        &mut shown,
        &Status::Connecting { attempt: 1 }
    ));
}
