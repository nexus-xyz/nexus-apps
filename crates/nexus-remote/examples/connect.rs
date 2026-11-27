//! Connects to a real server the way the app does, for a manual end-to-end check before the UI exists:
//!
//!   cargo run -p nexus-remote --example connect -- <ssh destination> <local nexus-host> [--update]
//!
//! Installs the local build there, starts it, connects through `nexus-host proxy`, prints the status and the overview,
//! drops the connection once to show the reconnect, and with `--update` runs "Update now".

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nexus_remote::client::Pushed;
use nexus_remote::install::{Installer, Source};
use nexus_remote::machine::{Backoff, Remote, Sink, SshLink, Status};
use nexus_remote::ssh::Target;
use serde_json::json;

struct Print;

impl Sink for Print {
    fn push(&self, push: &Pushed) {
        println!("push {}", push.event);
    }
    fn status(&self, status: &Status) {
        println!("status {}", serde_json::to_string(status).unwrap_or_default());
    }
}

async fn connected(remote: &Remote) {
    for _ in 0..600 {
        if matches!(remote.status(), Status::Connected { .. }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!("not connected after a minute: {:?}", remote.status());
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(destination), Some(binary)) = (args.first(), args.get(1)) else {
        eprintln!("usage: connect <ssh destination> <local nexus-host> [--update]");
        std::process::exit(2);
    };
    let installer = Installer {
        target: Target::Ssh {
            destination: destination.clone(),
        },
        source: Source::Local {
            path: PathBuf::from(binary),
        },
        fetch: None,
    };
    let remote = Remote::start(Arc::new(SshLink { installer }), Arc::new(Print), Backoff::default());
    connected(&remote).await;
    println!(
        "overview {}",
        remote
            .call("wb_overview", json!({}))
            .await
            .map(|o| o["persistent"].clone())
            .unwrap_or_default()
    );
    println!("host_status {:?}", remote.call("host_status", json!(null)).await);
    remote.drop_connection();
    tokio::time::sleep(Duration::from_millis(200)).await;
    connected(&remote).await;
    println!("reconnected");
    if args.iter().any(|a| a == "--update") {
        println!("update {:?}", remote.update().await);
        tokio::time::sleep(Duration::from_millis(200)).await;
        connected(&remote).await;
        println!("host_status {:?}", remote.call("host_status", json!(null)).await);
    }
    remote.close();
}
