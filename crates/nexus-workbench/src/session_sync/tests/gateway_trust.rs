//! Remote control under Gateway's device trust: on until the person turns it off on this computer.
use super::*;

#[tokio::test]
async fn remote_control_is_on_under_gateway_trust_until_the_person_turns_it_off() {
    let f = Fixture::new();
    f.backend.controls.store(true, Ordering::SeqCst);
    let identity = f.backend.identity.clone();
    let reads = || f.backend.inbox_reads.load(Ordering::SeqCst);
    // Under approvals it is off until the person turns it on, and no command is read.
    assert!(!f.wb.code_control().expect("settings").enabled);
    f.wb.control_sessions_once().await;
    assert_eq!(reads(), 0);
    // Gateway vouches for the person's devices: on, and any of their trusted devices may control it unpaired.
    f.backend.gateway_trust.store(true, Ordering::SeqCst);
    assert!(f.wb.code_control().expect("settings").enabled);
    f.wb.control_sessions_once().await;
    assert_eq!(reads(), 1);
    assert!(
        f.wb.store()
            .control_allows_in(&identity, DeviceTrust::Gateway, "dev_phone")
            .expect("allows")
    );
    assert!(
        !f.wb
            .store()
            .control_allows_in(&identity, DeviceTrust::Approvals, "dev_phone")
            .expect("approvals")
    );
    // The person turns it off on this computer: it stays off.
    f.wb.set_code_control(false).expect("off");
    assert!(!f.wb.code_control().expect("settings").enabled);
    f.wb.control_sessions_once().await;
    assert_eq!(reads(), 1);
    assert!(
        !f.wb
            .store()
            .control_allows_in(&identity, DeviceTrust::Gateway, "dev_phone")
            .expect("off")
    );
    f.wb.set_code_control(true).expect("on");
    assert!(f.wb.code_control().expect("settings").enabled);
}
