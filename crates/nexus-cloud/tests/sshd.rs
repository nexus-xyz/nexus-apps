//! The real path over a real `ssh`: a throwaway user-level sshd on 127.0.0.1 stands in for the cloud machine, and the
//! fake boat hands out its address and host key. Skips (and says why) on a computer without `/usr/sbin/sshd`.
#![cfg(unix)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Scratch, sshd, stubs};
use nexus_cloud::fake::{Config, FakeBoat, FakeSsh};
use nexus_cloud::keys::load_or_generate;
use nexus_cloud::link::{CloudLink, Progress, Provision, alias, provision};
use nexus_remote::install::Source;
use nexus_remote::ssh::{Target, write_known_hosts};

const LIMIT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn a_pinned_host_key_connects_and_a_wrong_one_does_not() {
    let dir = Scratch::new("sshd-direct");
    let home = dir.path().join("home");
    let _ = std::fs::create_dir_all(&home);
    let Some(sshd) = sshd(dir.path(), &home, "/usr/bin:/bin") else {
        return;
    };
    let key = load_or_generate(&dir.path().join("id_ed25519"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let _ = std::fs::write(&sshd.authorized_keys, format!("{}\n", key.public));
    let known_hosts = dir.path().join("known_hosts");
    let target = Target::Direct {
        host: "127.0.0.1".into(),
        port: sshd.port,
        user: sshd.user.clone(),
        identity: key.private.clone(),
        known_hosts: known_hosts.clone(),
        alias: alias("bx_23456789"),
    };
    write_known_hosts(&known_hosts, &alias("bx_23456789"), 22, &sshd.host_key).unwrap_or_else(|e| panic!("{e}"));
    let out = target
        .run("echo \"hello $1 from $HOME\"", &["there"], None, LIMIT)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(out.ok, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), format!("hello there from {}", home.display()));

    // Another machine's key under the same name: ssh refuses before running anything.
    let other = dir.path().join("other");
    let _ = std::fs::create_dir_all(&other);
    let made = nexus_workbench::process::command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(other.join("k"))
        .status();
    assert!(made.is_ok_and(|s| s.success()));
    let wrong = std::fs::read_to_string(other.join("k.pub")).unwrap_or_default();
    write_known_hosts(&known_hosts, &alias("bx_23456789"), 22, &wrong).unwrap_or_else(|e| panic!("{e}"));
    let marker = dir.path().join("ran");
    let out = target
        .run("touch \"$1\"", &[&marker.display().to_string()], None, LIMIT)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!out.ok);
    assert!(!marker.exists());
    assert!(
        out.stderr.contains("HOST IDENTIFICATION HAS CHANGED") || out.stderr.contains("verification failed"),
        "{}",
        out.stderr
    );
}

/// A sleeping machine: the link wakes it, registers the app's key, pins the host key the API gave, and the machine
/// answers over ssh and takes its provisioning there.
#[tokio::test]
async fn a_sleeping_machine_is_woken_reached_and_prepared() {
    let dir = Scratch::new("sshd-cloud");
    let home = dir.path().join("home");
    let _ = std::fs::create_dir_all(&home);
    let bin = stubs(dir.path());
    let Some(sshd) = sshd(dir.path(), &home, &format!("{}:/usr/bin:/bin", bin.display())) else {
        return;
    };
    let fake = FakeBoat::start(Config {
        ssh: Some(FakeSsh {
            authorized_keys: sshd.authorized_keys.clone(),
            endpoint: format!("127.0.0.1:{}", sshd.port),
            host_key: sshd.host_key.clone(),
            user: sshd.user.clone(),
        }),
        ..Config::default()
    })
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let id = fake.insert("archived");
    let boat = nexus_cloud::boat::Boat::new(Some(&fake.url()), "boat_fake", None).unwrap_or_else(|e| panic!("{e}"));
    let key = load_or_generate(&dir.path().join("keys/id_ed25519"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut link = CloudLink::new(
        Arc::new(boat),
        id.clone(),
        key,
        dir.path().join("keys/known_hosts"),
        Source::Local {
            path: dir.path().join("unused"),
        },
    );
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    link.progress = Arc::new(move |p| sink.lock().map(|mut h| h.push(p)).unwrap_or(()));
    let target = link.reach().await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        heard.lock().map(|h| h.clone()).unwrap_or_default(),
        [Progress::Waking, Progress::Starting, Progress::Preparing]
    );
    assert!(matches!(&target, Target::Direct { alias, .. } if *alias == format!("nexus-cloud-{id}")));
    let out = target
        .run("echo up", &[], None, LIMIT)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(out.ok, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "up");

    let env = "CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-over-ssh\n";
    let fields = provision(
        &target,
        &Provision {
            agent_env: Some(env.into()),
            ..Provision::default()
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fields.get("provisioned").map(String::as_str), Some("yes"));
    assert_eq!(
        std::fs::read_to_string(home.join(".nexus/host/agent.env")).unwrap_or_default(),
        env
    );
    // The secret went through stdin: sshd's log of the command never holds it, nor does any call a stub saw.
    assert!(!common::calls(dir.path()).contains("sk-ant"));
}
