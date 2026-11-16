//! The bridge an AI client launches (`crafthub connect <app>`): stdio on one side, the app's shared
//! server on the other.
//!
//! It starts the server when none is running. If the server restarts on the same port (a new
//! version, a changed setting) it reconnects and repeats the handshake, so the client's session
//! carries on; requests that were in flight get an error answer instead of hanging.

use crate::config::Config;
use crate::control::{self, Conn};
use crate::hub::Hub;
use crate::logger::Logger;
use crate::rpc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

struct Shim {
    app: String,
    port: u16,
    log: Logger,
    /// The way to the server; `None` while reconnecting.
    server: Mutex<Option<TcpStream>>,
    /// Requests the client is waiting on, by their id as text.
    pending: Mutex<HashMap<String, Value>>,
    /// The client's `initialize`, to repeat after a reconnect.
    init_line: Mutex<Option<String>>,
    out: Mutex<std::io::Stdout>,
}

/// The two fields of a message the bridge needs. Reading only these skips over the rest of the
/// message instead of building it in memory, which matters for answers that carry whole images.
#[derive(Default, Deserialize)]
struct Head {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
}

impl Head {
    fn of(line: &str) -> Head {
        serde_json::from_str(line).unwrap_or_default()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs the bridge until the client closes its side. Returns the process exit code.
pub fn run(app_id: &str) -> i32 {
    let Some(app) = Hub::load().app(app_id) else {
        eprintln!("crafthub: `{app_id}` is not an app the hub knows. Run `crafthub list`, or add it with `crafthub add {app_id} --cli <path>`.");
        return 1;
    };
    let log = Logger::new(&format!("{app_id}-client"));
    let conn = control::ensure_server(&app.id, app.port, Duration::from_secs(20))
        .and_then(|_| Conn::open(app.port, Duration::from_secs(2)).ok_or_else(|| format!("the server on port {} does not answer", app.port)));
    let conn = match conn {
        Ok(conn) => conn,
        Err(why) => {
            log.write(&format!("Connection failed: {why}"));
            eprintln!("crafthub {app_id}: {why}");
            return 1;
        }
    };

    let shim = Arc::new(Shim {
        app: app.id.clone(),
        port: app.port,
        log,
        server: Mutex::new(conn.writer()),
        pending: Mutex::new(HashMap::new()),
        init_line: Mutex::new(None),
        out: Mutex::new(std::io::stdout()),
    });
    let from_server = shim.clone();
    thread::spawn(move || from_server.relay_from_server(conn));

    let mut stdin = BufReader::new(std::io::stdin());
    while let Some(line) = rpc::read_line(&mut stdin) {
        if line.trim().is_empty() {
            continue;
        }
        shim.forward(&line);
    }
    0
}

impl Shim {
    fn to_client(&self, line: &str) -> bool {
        let mut out = lock(&self.out);
        writeln!(out, "{line}").and_then(|_| out.flush()).is_ok()
    }

    fn answer_error(&self, id: &Value, why: &str) {
        lock(&self.pending).remove(&id.to_string());
        self.to_client(&rpc::error(id, -32000, why).to_string());
    }

    /// One line from the client, on to the server.
    fn forward(&self, line: &str) {
        let head = Head::of(line);
        let method = head.method.as_deref();
        let request_id = if method.is_some() { head.id.clone() } else { None };
        if let Some(id) = &request_id {
            if method == Some("initialize") {
                *lock(&self.init_line) = Some(line.to_string());
            }
            lock(&self.pending).insert(id.to_string(), id.clone());
        }
        let mut data = Vec::with_capacity(line.len() + 1);
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        let sent = lock(&self.server).as_mut().is_some_and(|server| server.write_all(&data).is_ok());
        if !sent {
            if let Some(id) = &request_id {
                self.answer_error(id, &format!("the {} server is restarting: try again in a few seconds", self.app));
            }
        }
    }

    /// Everything the server says, on to the client; reconnects when the server goes away.
    fn relay_from_server(&self, mut conn: Conn) {
        loop {
            while let Some(line) = conn.read_line() {
                if let Head { id: Some(id), method: None } = Head::of(&line) {
                    lock(&self.pending).remove(&id.to_string());
                }
                if !self.to_client(&line) {
                    std::process::exit(0);
                }
            }

            *lock(&self.server) = None;
            let lost: Vec<Value> = lock(&self.pending).values().cloned().collect();
            for id in lost {
                self.answer_error(&id, &format!("the {} server restarted during the request", self.app));
            }
            self.log.write("The server closed the connection: reconnecting");
            match self.reconnect() {
                Some(new_conn) => {
                    *lock(&self.server) = new_conn.writer();
                    conn = new_conn;
                    self.log.write("Reconnected to the server");
                }
                None => {
                    self.log.write("The server is gone: closing");
                    std::process::exit(0);
                }
            }
        }
    }

    /// Waits for the server to come back. It is not restarted from here: when the app was turned
    /// off, an open session must not turn it on again.
    fn reconnect(&self) -> Option<Conn> {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(500));
            // The server may have come back on another port: the user can move it at any time.
            let port = Config::load().apps.get(&self.app).map_or(self.port, |saved| saved.port);
            if !control::port_in_use(port) {
                continue;
            }
            let Some(mut conn) = Conn::open(port, Duration::from_millis(500)) else { continue };
            let init = lock(&self.init_line).clone().and_then(|line| serde_json::from_str::<Value>(&line).ok());
            if let Some(init) = init {
                let answer = conn.call("hub-reinit", "initialize", init.get("params").cloned(), Duration::from_secs(30));
                if !answer.is_some_and(|a| a.get("result").is_some()) {
                    continue;
                }
                conn.send_line(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string());
            }
            return Some(conn);
        }
        None
    }
}
