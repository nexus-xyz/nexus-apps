use std::sync::atomic::{AtomicBool, Ordering};

use nexus_remote::machine::Status;
const LOCAL_SCOPE: &str = "|";

use super::*;

#[test]
#[ignore = "needs the local host and fake Claude binaries"]
fn sign_out_waits_for_the_old_service_before_the_first_local_turn() {
    fn signal(pid: i32, signal: i32) -> std::io::Result<()> {
        assert!(pid > 0);
        // The PID comes from the stand-in host running in this test's temporary directory.
        if unsafe { libc::kill(pid, signal) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    struct Paused(i32);
    impl Drop for Paused {
        fn drop(&mut self) {
            let _ = signal(self.0, libc::SIGCONT);
        }
    }

    let root = root("byo-handoff");
    let data = root.join("data");
    let repo = root.join("project");
    std::fs::create_dir_all(&repo).expect("project");
    let gateway = GATEWAY;
    let scope = format!("{gateway}|org-before-sign-out");
    let signed_in = Arc::new(AtomicBool::new(true));
    let client = Client::new(crate::gateway::client_options(gateway, Some(TOKEN.into())));
    let loops = Loops {
        local_providers_opt_in: crate::local_providers_opt_in::enabled_for_test(),
        client: Arc::new({
            let signed_in = signed_in.clone();
            move || signed_in.load(Ordering::SeqCst).then(|| client.clone())
        }),
        ..switch(Arc::new(|| true))
    };
    let mut local = stand_in(&root, gateway, &scope);
    local.env.push(("NEXUS_BYO_PROVIDERS".into(), "1".into()));
    let hello = local.sign_in.clone();
    local.sign_in = Arc::new({
        let signed_in = signed_in.clone();
        move || {
            if signed_in.load(Ordering::SeqCst) {
                hello()
            } else {
                Some(json!({"localMcp": {"servers": [], "values": {}}, "accounts": []}))
            }
        }
    });
    let _stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) = CodeRunner::start_with(data.clone(), gateway, &scope, loops, |_| Ok(service))
        .expect("start runner")
        .expect("runner owns the store");
    assert!(
        wait_for(Duration::from_secs(20), || matches!(
            runner.service_status(),
            Some(Status::Connected { .. })
        )),
        "{}",
        report(&local)
    );

    let old_pid = local.running().expect("signed-in service");
    signal(old_pid, libc::SIGSTOP).expect("pause the old service");
    let paused = Paused(old_pid);
    signed_in.store(false, Ordering::SeqCst);
    runner.retire_service_after_sign_out();
    runner.retire_service_after_sign_out();
    assert!(runner.host.scope().is_empty());
    runner.set_scope(LOCAL_SCOPE);
    assert!(runner.service_handoff() == ServiceHandoff::Stopping);
    assert!(runner.api.remote().expect("service connection").is_held());
    assert_eq!(local.running(), Some(old_pid));
    let attempted = runtime().block_on(runner.api.remote().unwrap().call("host_status", Value::Null));
    assert!(attempted.is_err(), "the signed-in service still owns the store");
    drop(paused);

    assert!(
        wait_for(Duration::from_secs(20), || {
            matches!(runner.service_status(), Some(Status::Connected { .. }))
                && local.running().is_some_and(|pid| pid != old_pid)
        }),
        "{}",
        report(&local)
    );
    let id = start_local_turn(&runner, &mut events, &local, repo);
    assert!(wait_for(Duration::from_secs(20), || turn_ends(&data, &id) == 1));
    assert_local_turn(&data, &id);
    runner.shutdown();
}
