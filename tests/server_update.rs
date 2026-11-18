//! A server whose engine program gets a newer version restarts its engine on it by itself.
//!
//! Its own test program, so the hub folder it points at a temporary one is nobody else's.

#![cfg(unix)]

use crafthub::control::{self, Probe};
use crafthub::server::{self, ServerSpec, ToolUpdate, UpdateTiming};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// An engine that answers the hub's initialize and nothing else.
const FAKE_ENGINE: &str = r#"while IFS= read -r line; do
  case "$line" in
    *'"hub-init"'*) printf '%s\n' '{"jsonrpc":"2.0","id":"hub-init","result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"1"}}}' ;;
  esac
done
"#;

fn wait_for(what: &str, port: u16, done: impl Fn(&control::Status) -> bool) -> control::Status {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Probe::Up(status) = control::status(port) {
            if done(&status) {
                return *status;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_engine_restarts_on_an_updated_program_when_no_session_is_connected() {
    let home = std::env::temp_dir().join(format!("crafthub-update-test-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("CRAFTHUB_HOME", &home);
    let engine = home.join("engine.sh");
    std::fs::write(&engine, FAKE_ENGINE).unwrap();

    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    // A newer program the first time, nothing new afterwards.
    let update: ToolUpdate = Arc::new(move || Ok(counter.fetch_add(1, Ordering::SeqCst) == 0));
    let spec = ServerSpec {
        app: "fakecraft".to_string(),
        port,
        engine: vec!["/bin/sh".to_string(), engine.to_string_lossy().into_owned()],
        roots: Vec::new(),
        library_tools: false,
        recent: Vec::new(),
        update: Some(update),
        update_timing: UpdateTiming { grace: Duration::from_millis(300), every: Duration::from_millis(200), idle_check: Duration::from_millis(100) },
        idle_stop: None,
    };
    thread::spawn(move || server::run(spec));

    wait_for("the engine to start", port, |s| s.engine.state == "ready");
    let after = wait_for("the engine to restart on the update", port, |s| s.engine.restarts >= 1 && s.engine.state == "ready");
    assert_eq!(after.engine.restarts, 1);
    // Nothing newer since: no more restarts.
    thread::sleep(Duration::from_millis(800));
    let later = wait_for("the status", port, |_| true);
    assert_eq!(later.engine.restarts, 1);
    assert!(asked.load(Ordering::SeqCst) >= 2, "the server keeps looking for updates");

    // Not shut down: a server's shutdown ends its process, which here is the test's. The server
    // ends with the test.
    let _ = std::fs::remove_dir_all(&home);
}
