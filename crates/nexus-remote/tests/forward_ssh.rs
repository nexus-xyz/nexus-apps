//! `nexus_remote::forward` against a real server, run by hand (it needs SSH access, so CI skips it):
//!
//! ```sh
//! NEXUS_FORWARD_HOST=myserver cargo test -p nexus-remote --test forward_ssh -- --ignored --nocapture
//! ```
//!
//! It starts `python3 -m http.server` in a scratch folder on the server, previews it through a forward, stops and
//! starts it again, kills the forward's ssh, closes the forwards, and removes the folder.

use std::time::Duration;

use nexus_remote::forward::Forwards;
use nexus_remote::ssh::Target;
use nexus_workbench::process::command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PORT: u16 = 38123;

/// `GET path` on this computer's `port`: the status line and body, or why not.
async fn fetch(port: u16, path: &str) -> Result<String, String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: localhost:{PORT}\r\n\r\n");
    stream.write_all(request.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut answer = String::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut answer))
        .await
        .map_err(|_| "no answer within 10 seconds".to_string())?
        .map_err(|e| e.to_string())?;
    if answer.is_empty() {
        return Err("the connection closed with no answer".into());
    }
    let status = answer.lines().next().unwrap_or_default().to_string();
    let body = answer.split("\r\n\r\n").nth(1).unwrap_or_default().trim().to_string();
    Ok(format!("{status} | {body}"))
}

async fn run(target: &Target, script: &str, args: &[&str]) -> String {
    let output = target
        .run(script, args, None, Duration::from_secs(30))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(output.ok, "{script}: {}", output.stderr);
    output.stdout.trim().to_string()
}

const START: &str = r#"cd "$1" && setsid python3 -m http.server "$2" --bind 127.0.0.1 </dev/null >server.log 2>&1 &
echo $! > "$1/pid"; i=0; while [ $i -lt 50 ] && ! ss -ltn | grep -q "127.0.0.1:$2 "; do sleep 0.1; i=$((i+1)); done
echo "server pid $(cat "$1/pid")""#;
const STOP: &str = r#"kill "$(cat "$1/pid")"; sleep 0.3; echo "server stopped""#;

fn ssh_children(local: u16) -> usize {
    let listing = command("ps")
        .args(["-eo", "args"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let needle = format!("127.0.0.1:{local}:127.0.0.1:{PORT}");
    listing
        .lines()
        .filter(|line| line.starts_with("ssh ") && line.contains(&needle))
        .count()
}

#[tokio::test]
#[ignore = "needs NEXUS_FORWARD_HOST, a server reached over ssh"]
async fn previews_a_dev_server_on_a_real_server() {
    let Some(host) = std::env::var("NEXUS_FORWARD_HOST").ok().filter(|h| !h.is_empty()) else {
        panic!("set NEXUS_FORWARD_HOST");
    };
    let target = Target::Ssh {
        destination: host.clone(),
    };
    let marker = format!("nexus-forward-e2e-{}", std::process::id());
    let dir = run(
        &target,
        r#"d=$(mktemp -d /tmp/nexus-forward-e2e.XXXXXX) && mkdir -p "$d/some" && echo "$1" > "$d/some/path" && echo "$d""#,
        &[&marker],
    )
    .await;
    println!("[{host}] scratch folder {dir}, marker {marker}");
    println!("[{host}] {}", run(&target, START, &[&dir, &PORT.to_string()]).await);

    let forwards = Forwards::default();
    let first = forwards.get(&target, PORT).await.expect("forwarded");
    println!(
        "forward {host}:{PORT} -> 127.0.0.1:{first} (ssh children: {})",
        ssh_children(first)
    );
    let page = fetch(first, "/some/path?q=1").await;
    println!("GET http://127.0.0.1:{first}/some/path?q=1 -> {page:?}");
    assert!(page.as_deref().is_ok_and(|page| page.contains(&marker)), "{page:?}");

    let again = forwards.get(&target, PORT).await.expect("reused");
    println!(
        "asked again -> 127.0.0.1:{again} (reused: {}, ssh children: {})",
        again == first,
        ssh_children(first)
    );
    assert_eq!(again, first);

    println!("[{host}] {}", run(&target, STOP, &[&dir]).await);
    let down = fetch(first, "/some/path?q=1").await;
    println!("GET while the server is down -> {down:?}");
    assert!(down.is_err(), "{down:?}");
    println!("forward still live: {:?}", forwards.live(PORT));
    println!("[{host}] {}", run(&target, START, &[&dir, &PORT.to_string()]).await);
    let back = fetch(first, "/some/path?q=1").await;
    println!("GET after the server started again, same forward -> {back:?}");
    assert!(back.as_deref().is_ok_and(|page| page.contains(&marker)), "{back:?}");

    // The forward's ssh dies (a lost network): the next ask starts a new one.
    let killed = command("pkill")
        .args(["-f", &format!("127.0.0.1:{first}:127.0.0.1:{PORT}")])
        .status()
        .map(|status| status.success());
    tokio::time::sleep(Duration::from_millis(300)).await;
    println!(
        "killed the forward's ssh: {killed:?}; live now: {:?}",
        forwards.live(PORT)
    );
    let second = forwards.get(&target, PORT).await.expect("forwarded again");
    let page = fetch(second, "/some/path?q=1").await;
    println!("restarted -> 127.0.0.1:{second}; GET -> {page:?}");
    assert_ne!(second, first);
    assert!(page.as_deref().is_ok_and(|page| page.contains(&marker)), "{page:?}");

    forwards.close();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let left = ssh_children(second);
    println!("closed: ssh children left for 127.0.0.1:{second}: {left}");
    assert_eq!(left, 0);

    println!("[{host}] {}", run(&target, STOP, &[&dir]).await);
    println!(
        "[{host}] {}",
        run(&target, r#"rm -rf "$1" && echo "removed $1""#, &[&dir]).await
    );
}
