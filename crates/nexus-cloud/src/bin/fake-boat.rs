//! A stand-in for boat's API on this computer (src/fake.rs), for running the app's cloud machines end to end against a
//! local sshd before a boat account is involved:
//!
//!   cargo run -p nexus-cloud --features fake --bin fake-boat -- \
//!     --port 8787 --key boat_fake \
//!     --authorized-keys /tmp/sshd/authorized_keys --ssh-endpoint 127.0.0.1:22226 \
//!     --host-key-file /tmp/sshd/host_ed25519.pub --ssh-user "$USER" --cwd "$HOME"
//!
//! Then point the app at it with `NEXUS_BOAT_URL=http://127.0.0.1:8787/api/v1` and the key `boat_fake`. Without the
//! `--authorized-keys`/`--ssh-endpoint`/`--host-key-file` trio, `/sshkey` answers a made-up address.

use std::path::PathBuf;
use std::time::Duration;

use nexus_cloud::fake::{Config, FakeBoat, FakeSsh};

fn usage() -> ! {
    eprintln!(
        "usage: fake-boat [--port N] [--key K] [--ready-after-ms N] [--cwd DIR] \
         [--authorized-keys FILE --ssh-endpoint HOST:PORT --host-key-file FILE.pub [--ssh-user USER]] [--no-scoped-keys]"
    );
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut port = 8787u16;
    let mut config = Config::default();
    let mut authorized_keys = None;
    let mut endpoint = None;
    let mut host_key_file: Option<PathBuf> = None;
    let mut user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--port" => port = value().parse().unwrap_or_else(|_| usage()),
            "--key" => config.key = value(),
            "--ready-after-ms" => {
                config.ready_after = Duration::from_millis(value().parse().unwrap_or_else(|_| usage()));
            }
            "--cwd" => config.exec_cwd = Some(PathBuf::from(value())),
            "--authorized-keys" => authorized_keys = Some(PathBuf::from(value())),
            "--ssh-endpoint" => endpoint = Some(value()),
            "--host-key-file" => host_key_file = Some(PathBuf::from(value())),
            "--ssh-user" => user = value(),
            "--no-scoped-keys" => config.scoped_keys = false,
            _ => usage(),
        }
    }
    match (authorized_keys, endpoint, host_key_file) {
        (Some(authorized_keys), Some(endpoint), Some(file)) => {
            let host_key = std::fs::read_to_string(&file).unwrap_or_else(|e| {
                eprintln!("could not read {}: {e}", file.display());
                std::process::exit(1);
            });
            config.ssh = Some(FakeSsh {
                authorized_keys,
                endpoint,
                host_key: host_key.trim().to_string(),
                user,
            });
        }
        (None, None, None) => {}
        _ => usage(),
    }
    let fake = match FakeBoat::start_on(([127, 0, 0, 1], port).into(), config).await {
        Ok(fake) => fake,
        Err(e) => {
            eprintln!("could not listen on 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    };
    println!("fake boat at {}", fake.url());
    std::future::pending::<()>().await;
}
