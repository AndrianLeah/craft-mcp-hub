//! The shared server (`crafthub serve <app>`): one engine process, shared by every client session.
//!
//! It listens on 127.0.0.1:<port> and speaks newline-delimited JSON-RPC. A connection first presents
//! the hub token, then talks MCP: `initialize` is answered from the engine's own answer, requests are
//! forwarded with their ids rewritten so sessions can't collide, and answers go back to whoever asked.
//! `hub/*` methods are the hub's own (status, shutdown, closeLibrary).
//!
//! Engines that work on a photo library choose it at start (`--library DIR`) and can't change it
//! afterwards, so for those the server adds library_list / library_open / library_close and restarts
//! the engine to switch.

use crate::logger::Logger;
use crate::rpc::{self, clip, read_line};
use crate::{proc, timefmt, token, VERSION};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub struct ServerSpec {
    pub app: String,
    pub port: u16,
    /// The engine's command line, without a library.
    pub engine: Vec<String>,
    pub library_tools: bool,
    /// The app's settings files that list its recent libraries.
    pub recent: Vec<PathBuf>,
    /// Brings the engine's program up to date (`Ok(true)` when it put a new one in place). Asked
    /// now and then; a new program is taken up when no session is connected, so an AI client is
    /// never cut off in the middle of its work.
    pub update: Option<ToolUpdate>,
    pub update_timing: UpdateTiming,
}

pub type ToolUpdate = Arc<dyn Fn() -> Result<bool, String> + Send + Sync>;

/// When a server looks for a newer engine program.
#[derive(Clone, Copy)]
pub struct UpdateTiming {
    /// After starting, so a server started for a session first serves it.
    pub grace: Duration,
    /// Between two looks.
    pub every: Duration,
    /// Between two looks for a moment with no session, once a newer program is waiting.
    pub idle_check: Duration,
}

impl Default for UpdateTiming {
    fn default() -> Self {
        UpdateTiming { grace: Duration::from_secs(30), every: Duration::from_secs(10 * 60), idle_check: Duration::from_secs(20) }
    }
}

/// The most a connection may send before it has presented the token.
const HELLO_LIMIT: u64 = 4096;

const LIBRARY_INSTRUCTIONS: &str = "Shared hub server: this engine is shared by every AI client session on this computer. \
It starts with no library open (an empty, temporary session) so the desktop app stays free to open its libraries. \
To work on the user's photos call library_list to see their libraries, then library_open with the one they mean; \
library_close releases it. Opening another library restarts the engine for every session.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum EngineState {
    Starting,
    Ready,
    Failed,
    Stopped,
}

impl EngineState {
    fn as_str(self) -> &'static str {
        match self {
            EngineState::Starting => "starting",
            EngineState::Ready => "ready",
            EngineState::Failed => "failed",
            EngineState::Stopped => "stopped",
        }
    }
}

struct Engine {
    state: EngineState,
    pid: u32,
    started_at: u64,
    /// Counts engine starts, so a restart can tell the new engine from the old one.
    generation: u64,
    /// The engine's answer to `initialize`, replayed to every session.
    init: Option<Map<String, Value>>,
    last_error: String,
    stderr_tail: VecDeque<String>,
    /// The library the engine has open; `None` is an empty, temporary session.
    library: Option<String>,
}

struct Client {
    id: u64,
    /// Lines for this session, written by its own thread so a slow session can't stall the others.
    out: Sender<Vec<u8>>,
    name: Mutex<String>,
    initialized: AtomicBool,
}

impl Client {
    fn send(&self, line: &str) {
        let mut data = Vec::with_capacity(line.len() + 1);
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        let _ = self.out.send(data);
    }

    fn send_value(&self, value: &Value) {
        self.send(&value.to_string());
    }
}

struct Pending {
    client: Arc<Client>,
    orig_id: Value,
    progress_token: Option<Value>,
    add_library_tools: bool,
}

struct Shared {
    spec: ServerSpec,
    log: Logger,
    engine: Mutex<Engine>,
    engine_changed: Condvar,
    child: Mutex<Option<Child>>,
    /// Lines for the engine, written by one thread so nobody else ever blocks on its pipe.
    engine_in: Mutex<Option<Sender<Vec<u8>>>>,
    /// Why the engine was ended on purpose, to report instead of its exit code.
    exit_note: Mutex<Option<String>>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
    requests: AtomicU64,
    clients: Mutex<Vec<Arc<Client>>>,
    next_client: AtomicU64,
    switching: AtomicBool,
    switch_gate: Mutex<()>,
    wake: Mutex<bool>,
    wake_changed: Condvar,
    stopping: AtomicBool,
    started_at: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs the server until it is told to shut down. Returns the process exit code.
pub fn run(spec: ServerSpec) -> i32 {
    let log = Logger::new(&spec.app);
    // A status check elsewhere may hold the port for an instant, so a busy port gets a few tries.
    let mut listener = None;
    let mut last_error = String::new();
    for _ in 0..20 {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, spec.port)) {
            Ok(bound) => {
                listener = Some(bound);
                break;
            }
            Err(e) => {
                last_error = e.to_string();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let Some(listener) = listener else {
        log.write(&format!("Cannot use port {}: {last_error}", spec.port));
        return 2;
    };
    log.write(&format!("Server {VERSION} started on 127.0.0.1:{} (pid {})", spec.port, std::process::id()));

    let shared = Arc::new(Shared {
        spec,
        log,
        engine: Mutex::new(Engine {
            state: EngineState::Starting,
            pid: 0,
            started_at: 0,
            generation: 0,
            init: None,
            last_error: String::new(),
            stderr_tail: VecDeque::new(),
            library: None,
        }),
        engine_changed: Condvar::new(),
        child: Mutex::new(None),
        engine_in: Mutex::new(None),
        exit_note: Mutex::new(None),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(0),
        requests: AtomicU64::new(0),
        clients: Mutex::new(Vec::new()),
        next_client: AtomicU64::new(0),
        switching: AtomicBool::new(false),
        switch_gate: Mutex::new(()),
        wake: Mutex::new(false),
        wake_changed: Condvar::new(),
        stopping: AtomicBool::new(false),
        started_at: timefmt::now_secs(),
    });

    let engine = shared.clone();
    thread::spawn(move || engine.engine_loop());
    if shared.spec.update.is_some() {
        let updater = shared.clone();
        thread::spawn(move || updater.update_loop());
    }
    for stream in listener.incoming() {
        if shared.stopping.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(stream) = stream {
            let shared = shared.clone();
            thread::spawn(move || shared.serve_client(stream));
        }
    }
    0
}

impl Shared {
    // ------------------------------------------------------------ engine

    fn engine_command(&self) -> Vec<String> {
        let mut command = self.spec.engine.clone();
        if let Some(library) = &lock(&self.engine).library {
            command.push("--library".to_string());
            command.push(library.clone());
        }
        command
    }

    /// Keeps an engine running: restarts it when it exits, backing off while it keeps failing.
    fn engine_loop(self: Arc<Self>) {
        let mut failures: u32 = 0;
        while !self.stopping.load(Ordering::Relaxed) {
            // Any start satisfies a library change asked for before it.
            self.switching.store(false, Ordering::Relaxed);
            let began = Instant::now();
            let ended = self.run_engine();
            if self.stopping.load(Ordering::Relaxed) {
                break;
            }
            if self.switching.load(Ordering::Relaxed) {
                self.fail("Library change: restarting the engine", EngineState::Starting);
                failures = 0;
                continue;
            }
            self.fail(&ended, EngineState::Failed);
            failures = if began.elapsed() > Duration::from_secs(60) { 0 } else { failures + 1 };
            // A library change cuts the wait short.
            let pause = Duration::from_secs((1u64 << failures.min(5)).min(30));
            let woken = lock(&self.wake);
            let (mut woken, _) = self.wake_changed.wait_timeout_while(woken, pause, |woken| !*woken).unwrap_or_else(|p| p.into_inner());
            *woken = false;
        }
    }

    /// Keeps the engine's program up to date: fetches a newer one in the background while the engine
    /// keeps running, then restarts the engine on it once no session is connected. The engine keeps
    /// the library it had open.
    fn update_loop(self: Arc<Self>) {
        let Some(update) = self.spec.update.clone() else { return };
        let timing = self.spec.update_timing;
        thread::sleep(timing.grace);
        let mut waiting = false;
        while !self.stopping.load(Ordering::Relaxed) {
            if !waiting {
                match update() {
                    Ok(true) => {
                        self.log.write("A newer version of the engine's program was installed; the engine restarts on it once no session is connected");
                        waiting = true;
                    }
                    Ok(false) => {}
                    Err(why) => self.log.write(&format!("Could not update the engine's program (the current one keeps working): {why}")),
                }
            }
            if waiting {
                let sessions = lock(&self.clients).iter().filter(|c| c.initialized.load(Ordering::Relaxed)).count();
                let busy = !lock(&self.pending).is_empty() || self.switching.load(Ordering::Relaxed);
                if sessions == 0 && !busy {
                    let _one_at_a_time = lock(&self.switch_gate);
                    self.log.write("Restarting the engine on the updated program");
                    match self.restart_engine() {
                        Ok(()) => self.log.write("The engine runs the updated program"),
                        Err(why) => self.log.write(&format!("The engine did not restart on the updated program: {why}")),
                    }
                    waiting = false;
                }
            }
            thread::sleep(if waiting { timing.idle_check } else { timing.every });
        }
    }

    /// Starts the engine and relays what it says until it exits. Returns why it ended.
    fn run_engine(self: &Arc<Self>) -> String {
        let command_line = self.engine_command();
        {
            let mut engine = lock(&self.engine);
            engine.state = EngineState::Starting;
            engine.started_at = timefmt::now_secs();
            engine.generation += 1;
            engine.init = None;
            engine.stderr_tail.clear();
        }
        *lock(&self.exit_note) = None;
        self.engine_changed.notify_all();

        let Some((program, args)) = command_line.split_first() else { return "No engine command".to_string() };
        let mut command = Command::new(program);
        command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(dir) = Path::new(program).parent().filter(|d| d.is_dir()) {
            command.current_dir(dir);
        }
        let mut child = match proc::quiet(&mut command).spawn() {
            Ok(child) => child,
            Err(e) => return format!("Could not start the engine {program}: {e}"),
        };
        let (Some(mut stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return "Could not open the engine's pipes".to_string();
        };
        lock(&self.engine).pid = child.id();
        self.log.write(&format!("Engine started (pid {}): {}", child.id(), proc::display_command(&command_line)));
        *lock(&self.child) = Some(child);

        let (to_engine, lines) = mpsc::channel::<Vec<u8>>();
        *lock(&self.engine_in) = Some(to_engine);
        thread::spawn(move || {
            for line in lines {
                if stdin.write_all(&line).is_err() {
                    break;
                }
            }
        });

        let shared = self.clone();
        thread::spawn(move || {
            let mut stderr = BufReader::new(stderr);
            while let Some(line) = read_line(&mut stderr) {
                if line.trim().is_empty() {
                    continue;
                }
                shared.log.write(&format!("[engine] {}", clip(&line, 2000)));
                let mut engine = lock(&shared.engine);
                engine.stderr_tail.push_back(clip(&line, 500));
                while engine.stderr_tail.len() > 20 {
                    engine.stderr_tail.pop_front();
                }
            }
        });

        self.send_to_engine(
            &json!({
                "jsonrpc": "2.0", "id": "hub-init", "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "craft-mcp-hub", "version": VERSION }
                }
            })
            .to_string(),
        );

        let mut stdout = BufReader::new(stdout);
        while let Some(line) = read_line(&mut stdout) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(Value::Object(message)) => self.handle_engine_message(message, &line),
                _ => self.log.write(&format!("Not JSON from the engine: {}", clip(&line, 300))),
            }
        }

        // The engine closed its output: it has exited, or is about to.
        *lock(&self.engine_in) = None;
        thread::sleep(Duration::from_millis(50));
        let code = lock(&self.child).take().and_then(|mut child| child.wait().ok()).and_then(|status| status.code());
        if let Some(note) = lock(&self.exit_note).take() {
            return note;
        }
        let tail: Vec<String> = lock(&self.engine).stderr_tail.iter().rev().take(2).rev().cloned().collect();
        let code = code.map_or("unknown".to_string(), |c| c.to_string());
        if tail.is_empty() {
            format!("The engine exited (code {code})")
        } else {
            format!("The engine exited (code {code}): {}", tail.join(" "))
        }
    }

    fn send_to_engine(&self, line: &str) -> bool {
        let mut data = Vec::with_capacity(line.len() + 1);
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        lock(&self.engine_in).as_ref().is_some_and(|to_engine| to_engine.send(data).is_ok())
    }

    fn kill_engine(&self) {
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }

    /// Records that the engine is gone and answers everyone who was waiting on it.
    fn fail(&self, why: &str, next: EngineState) {
        {
            let mut engine = lock(&self.engine);
            engine.state = next;
            engine.last_error = why.to_string();
            engine.init = None;
        }
        *lock(&self.engine_in) = None;
        self.engine_changed.notify_all();
        self.log.write(why);
        let dropped: Vec<Pending> = lock(&self.pending).drain().map(|(_, pending)| pending).collect();
        for pending in dropped {
            pending.client.send_value(&rpc::error(&pending.orig_id, -32000, why));
        }
    }

    fn handle_engine_message(&self, mut message: Map<String, Value>, raw: &str) {
        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
        match (id, method) {
            // An answer: to our own initialize, or to a session's request.
            (Some(id), None) => {
                if id == "hub-init" {
                    if let Some(error) = message.get("error") {
                        *lock(&self.exit_note) = Some(format!("The engine refused to initialize: {error}"));
                        self.kill_engine();
                        return;
                    }
                    let init = message.get("result").and_then(Value::as_object).cloned().unwrap_or_default();
                    let label = rpc::server_label(&init);
                    self.send_to_engine(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string());
                    {
                        let mut engine = lock(&self.engine);
                        engine.init = Some(init);
                        engine.state = EngineState::Ready;
                        engine.last_error.clear();
                    }
                    self.engine_changed.notify_all();
                    self.log.write(&format!("Engine ready: {label}"));
                    return;
                }
                let Some(pending) = id.as_u64().and_then(|n| lock(&self.pending).remove(&n)) else { return };
                message.insert("id".to_string(), pending.orig_id.clone());
                if pending.add_library_tools {
                    if let Some(tools) = message.get_mut("result").and_then(|r| r.get_mut("tools")).and_then(Value::as_array_mut) {
                        tools.extend(library_tool_defs());
                    }
                }
                pending.client.send_value(&Value::Object(message));
            }
            // A request from the engine to its client: answered here, since no one session owns it.
            (Some(id), Some(method)) => {
                let reply = match method.as_str() {
                    "ping" => rpc::result(&id, json!({})),
                    "roots/list" => rpc::result(&id, json!({ "roots": [] })),
                    _ => rpc::error(&id, -32601, "Not supported by the shared hub server"),
                };
                self.send_to_engine(&reply.to_string());
            }
            (None, Some(method)) => match method.as_str() {
                "notifications/progress" => {
                    let token = message.get("params").and_then(|p| p.get("progressToken")).and_then(Value::as_str).unwrap_or("");
                    let Some(n) = token.strip_prefix('p').and_then(|n| n.parse::<u64>().ok()) else { return };
                    let target = lock(&self.pending).get(&n).map(|p| (p.client.clone(), p.progress_token.clone()));
                    let Some((client, Some(original))) = target else { return };
                    if let Some(params) = message.get_mut("params").and_then(Value::as_object_mut) {
                        params.insert("progressToken".to_string(), original);
                    }
                    client.send_value(&Value::Object(message));
                }
                "notifications/cancelled" => {}
                _ => {
                    let sessions: Vec<Arc<Client>> = lock(&self.clients).iter().filter(|c| c.initialized.load(Ordering::Relaxed)).cloned().collect();
                    for session in sessions {
                        session.send(raw);
                    }
                }
            },
            (None, None) => {}
        }
    }

    // ------------------------------------------------------------ sessions

    fn serve_client(self: Arc<Self>, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let Ok(read_half) = stream.try_clone() else { return };
        let mut reader = BufReader::new(read_half);

        // The first line must be the hub token: it keeps other programs (and web pages) out.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        // Read with a size limit: whoever hasn't shown the token yet can't make us buffer more than this.
        let hello = read_line(&mut (&mut reader).take(HELLO_LIMIT)).and_then(|line| serde_json::from_str::<Value>(&line).ok());
        let allowed = hello.is_some_and(|h| h.get("hub") == Some(&json!("hello")) && h.get("token").and_then(Value::as_str).is_some_and(token::matches));
        if !allowed {
            let _ = (&stream).write_all(b"{\"hub\":\"denied\"}\n");
            return;
        }
        let _ = stream.set_read_timeout(None);

        let (out, lines) = mpsc::channel::<Vec<u8>>();
        let Ok(mut write_half) = stream.try_clone() else { return };
        thread::spawn(move || {
            for line in lines {
                if write_half.write_all(&line).is_err() {
                    break;
                }
            }
        });
        let client = Arc::new(Client {
            id: self.next_client.fetch_add(1, Ordering::Relaxed) + 1,
            out,
            name: Mutex::new(String::new()),
            initialized: AtomicBool::new(false),
        });
        lock(&self.clients).push(client.clone());

        while let Some(line) = read_line(&mut reader) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(Value::Object(message)) => self.handle_client_message(&client, message),
                Ok(Value::Array(batch)) => {
                    for item in batch {
                        if let Value::Object(message) = item {
                            self.handle_client_message(&client, message);
                        }
                    }
                }
                _ => client.send_value(&rpc::error(&Value::Null, -32700, "Invalid JSON")),
            }
        }

        lock(&self.clients).retain(|c| c.id != client.id);
        lock(&self.pending).retain(|_, pending| pending.client.id != client.id);
        let _ = stream.shutdown(Shutdown::Both);
        if client.initialized.load(Ordering::Relaxed) {
            self.log.write(&format!("Session {} disconnected ({})", client.id, lock(&client.name)));
        }
    }

    fn handle_client_message(self: &Arc<Self>, client: &Arc<Client>, mut message: Map<String, Value>) {
        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        // No method: an answer to a request from the engine, and none of those are forwarded.
        let Some(method) = message.get("method").and_then(Value::as_str).map(str::to_owned) else { return };
        if let Some(own) = method.strip_prefix("hub/") {
            self.handle_own(client, id, own);
            return;
        }
        match method.as_str() {
            "initialize" => return self.initialize_client(client, id, &message),
            "notifications/initialized" => return,
            "notifications/cancelled" => {
                let wanted = message.get("params").and_then(|p| p.get("requestId")).cloned();
                let ours = lock(&self.pending).iter().find(|(_, p)| p.client.id == client.id && Some(&p.orig_id) == wanted.as_ref()).map(|(n, _)| *n);
                if let (Some(n), Some(params)) = (ours, message.get_mut("params").and_then(Value::as_object_mut)) {
                    params.insert("requestId".to_string(), json!(n));
                    self.send_to_engine(&Value::Object(message).to_string());
                }
                return;
            }
            _ => {}
        }
        let Some(id) = id else {
            self.send_to_engine(&Value::Object(message).to_string());
            return;
        };
        if method == "ping" {
            client.send_value(&rpc::result(&id, json!({})));
            return;
        }
        if self.spec.library_tools && method == "tools/call" {
            let params = message.get("params");
            let tool = params.and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("");
            if matches!(tool, "library_list" | "library_open" | "library_close") {
                let arguments = params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
                client.send_value(&rpc::result(&id, self.call_library_tool(tool, &arguments)));
                return;
            }
        }

        let (state, last_error) = {
            let engine = lock(&self.engine);
            (engine.state, engine.last_error.clone())
        };
        if state != EngineState::Ready {
            client.send_value(&rpc::error(&id, -32000, &self.not_ready(&last_error)));
            return;
        }
        let n = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let first_page = message.get("params").and_then(|p| p.get("cursor")).is_none_or(Value::is_null);
        let mut pending = Pending {
            client: client.clone(),
            orig_id: id.clone(),
            progress_token: None,
            add_library_tools: self.spec.library_tools && method == "tools/list" && first_page,
        };
        if let Some(meta) = message.get_mut("params").and_then(|p| p.get_mut("_meta")).and_then(Value::as_object_mut) {
            if let Some(token) = meta.get("progressToken").cloned() {
                pending.progress_token = Some(token);
                meta.insert("progressToken".to_string(), json!(format!("p{n}")));
            }
        }
        lock(&self.pending).insert(n, pending);
        message.insert("id".to_string(), json!(n));
        self.requests.fetch_add(1, Ordering::Relaxed);
        if !self.send_to_engine(&Value::Object(message).to_string()) {
            lock(&self.pending).remove(&n);
            client.send_value(&rpc::error(&id, -32000, &format!("The {} engine is not answering", self.spec.app)));
        }
    }

    fn not_ready(&self, last_error: &str) -> String {
        let why = if last_error.is_empty() { "it is still starting" } else { last_error };
        format!("The {} engine is not ready: {why}", self.spec.app)
    }

    fn initialize_client(&self, client: &Arc<Client>, id: Option<Value>, message: &Map<String, Value>) {
        let Some(id) = id else { return };
        if let Some(info) = message.get("params").and_then(|p| p.get("clientInfo")) {
            let field = |key: &str| info.get(key).and_then(Value::as_str).unwrap_or("");
            *lock(&client.name) = format!("{} {}", field("name"), field("version")).trim().to_string();
        }
        // Wait for the engine only while it is starting: when it has failed the session hears at once.
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut engine = lock(&self.engine);
        while engine.init.is_none() && engine.state == EngineState::Starting && Instant::now() < deadline {
            engine = self.engine_changed.wait_timeout(engine, Duration::from_millis(200)).unwrap_or_else(|p| p.into_inner()).0;
        }
        let Some(mut reply) = engine.init.clone() else {
            let why = self.not_ready(&engine.last_error);
            drop(engine);
            self.log.write(&format!("Session {} refused. {why}", client.id));
            client.send_value(&rpc::error(&id, -32000, &why));
            return;
        };
        drop(engine);
        if self.spec.library_tools {
            let own = reply.get("instructions").and_then(Value::as_str).unwrap_or("");
            let instructions = format!("{own}\n\n{LIBRARY_INSTRUCTIONS}").trim().to_string();
            reply.insert("instructions".to_string(), json!(instructions));
        }
        client.send_value(&rpc::result(&id, Value::Object(reply)));
        client.initialized.store(true, Ordering::Relaxed);
        self.log.write(&format!("Session {} connected ({})", client.id, lock(&client.name)));
    }

    fn handle_own(self: &Arc<Self>, client: &Arc<Client>, id: Option<Value>, method: &str) {
        let id = id.unwrap_or(Value::Null);
        match method {
            "status" => client.send_value(&rpc::result(&id, self.status())),
            "closeLibrary" => {
                let open = lock(&self.engine).library.is_some();
                let outcome = if open { self.switch_library(None) } else { Ok(()) };
                client.send_value(&rpc::result(&id, json!({ "ok": outcome.is_ok(), "error": outcome.err() })));
            }
            "shutdown" => {
                client.send_value(&rpc::result(&id, json!({ "stopping": true })));
                self.log.write("Shutdown requested");
                self.stopping.store(true, Ordering::Relaxed);
                lock(&self.engine).state = EngineState::Stopped;
                self.kill_engine();
                thread::spawn(|| {
                    thread::sleep(Duration::from_millis(300));
                    std::process::exit(0);
                });
            }
            _ => client.send_value(&rpc::error(&id, -32601, &format!("Unknown hub method: {method}"))),
        }
    }

    fn status(&self) -> Value {
        let sessions: Vec<String> = lock(&self.clients).iter().filter(|c| c.initialized.load(Ordering::Relaxed)).map(|c| lock(&c.name).clone()).collect();
        let pending = lock(&self.pending).len();
        let engine = lock(&self.engine);
        json!({
            "app": self.spec.app,
            "port": self.spec.port,
            "pid": std::process::id(),
            "version": VERSION,
            "startedAt": self.started_at,
            "engine": {
                "state": engine.state.as_str(),
                "pid": engine.pid,
                "startedAt": engine.started_at,
                "restarts": engine.generation.saturating_sub(1),
                "server": engine.init.as_ref().map(rpc::server_label).unwrap_or_default(),
                "command": proc::display_command(&self.spec.engine),
            },
            "library": engine.library,
            "sessions": sessions.len(),
            "sessionNames": sessions,
            "requests": self.requests.load(Ordering::Relaxed),
            "pending": pending,
            "lastError": engine.last_error,
            "stderrTail": engine.stderr_tail,
        })
    }

    // ------------------------------------------------------------ libraries

    /// Restarts the engine on another library (`None`: no library). On failure the previous one is
    /// reopened and the reason returned.
    fn switch_library(&self, library: Option<String>) -> Result<(), String> {
        let _one_at_a_time = lock(&self.switch_gate);
        let previous = std::mem::replace(&mut lock(&self.engine).library, library.clone());
        self.log.write(&format!("Opening library: {}", library.as_deref().unwrap_or("(none, temporary session)")));
        match self.restart_engine() {
            Ok(()) => Ok(()),
            Err(why) => {
                self.log.write(&format!("Library not opened: {why}"));
                lock(&self.engine).library = previous;
                let _ = self.restart_engine();
                Err(why)
            }
        }
    }

    fn restart_engine(&self) -> Result<(), String> {
        let before = lock(&self.engine).generation;
        self.switching.store(true, Ordering::Relaxed);
        self.kill_engine();
        *lock(&self.wake) = true;
        self.wake_changed.notify_all();

        let deadline = Instant::now() + Duration::from_secs(90);
        let mut engine = lock(&self.engine);
        loop {
            if engine.generation > before {
                match engine.state {
                    EngineState::Ready => return Ok(()),
                    EngineState::Failed => return Err(engine.last_error.clone()),
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return Err("the engine did not restart within 90 seconds".to_string());
            }
            engine = self.engine_changed.wait_timeout(engine, Duration::from_millis(100)).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// The libraries the app's settings list as recent, the open one first.
    fn known_libraries(&self) -> Vec<String> {
        let mut libraries: Vec<String> = Vec::new();
        let mut add = |path: &str| {
            let path = if path.to_ascii_lowercase().ends_with(".lccat") || path.ends_with("catalog.snap") {
                Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                path.to_string()
            };
            let path = trim_path(&path).to_string();
            if !path.is_empty() && !libraries.iter().any(|known| same_path(known, &path)) {
                libraries.push(path);
            }
        };
        if let Some(open) = &lock(&self.engine).library {
            add(open);
        }
        for file in &self.spec.recent {
            let settings = std::fs::read_to_string(file).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
            let mut paths = Vec::new();
            if let Some(settings) = settings {
                collect_library_paths(&settings, &mut paths);
            }
            for path in paths {
                add(&path);
            }
        }
        libraries
    }

    fn call_library_tool(&self, tool: &str, arguments: &Value) -> Value {
        let open = lock(&self.engine).library.clone();
        match tool {
            "library_list" => {
                let libraries: Vec<Value> = self
                    .known_libraries()
                    .iter()
                    .map(|path| {
                        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        json!({ "name": name, "path": path, "available": Path::new(path).is_dir() })
                    })
                    .collect();
                tool_result(false, &json!({ "open": open, "libraries": libraries }).to_string())
            }
            "library_close" => {
                if open.is_none() {
                    return tool_result(false, "No library was open: the engine is on an empty, temporary session.");
                }
                match self.switch_library(None) {
                    Ok(()) => tool_result(false, "Library closed: the engine is on an empty, temporary session."),
                    Err(why) => tool_result(true, &why),
                }
            }
            _ => {
                let Some(path) = arguments.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()) else {
                    return tool_result(true, "path is required");
                };
                let create = arguments.get("create").and_then(Value::as_bool).unwrap_or(false);
                let mut path = PathBuf::from(path);
                if path.is_file() {
                    path.pop();
                }
                let path = trim_path(&path.to_string_lossy()).to_string();
                if !create && !Path::new(&path).is_dir() {
                    return tool_result(true, &format!("This folder does not exist: {path}"));
                }
                if !create && !is_library(Path::new(&path)) {
                    return tool_result(
                        true,
                        &format!("This folder is not a library (no catalog.snap or .lccat): {path}. Pass create: true to make a new library there."),
                    );
                }
                let ready = lock(&self.engine).state == EngineState::Ready;
                if ready && open.as_deref().is_some_and(|open| same_path(open, &path)) {
                    return tool_result(false, &format!("Already open: {path}"));
                }
                match self.switch_library(Some(path.clone())) {
                    Ok(()) => tool_result(false, &format!("Library open: {path}")),
                    Err(why) => tool_result(true, &format!("Could not open {path}: {why}")),
                }
            }
        }
    }
}

fn tool_result(is_error: bool, text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn library_tool_defs() -> Vec<Value> {
    let no_arguments = json!({ "type": "object", "properties": {} });
    vec![
        json!({
            "name": "library_list",
            "description": "List the user's photo libraries (the recent ones from the app's settings), with the one open now. Start here before working on the user's photos.",
            "inputSchema": no_arguments,
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "library_open",
            "description": "Open a photo library in the shared engine (the engine restarts with it, for every AI client session). Fails, saying why, when another program (usually the desktop app) has that library open. A folder that is not a library yet is refused unless create is true.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "The library folder (as library_list gives it), or its .lccat / catalog.snap file" },
                    "create": { "type": "boolean", "description": "Create a new, empty library in that folder" }
                }
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
        json!({
            "name": "library_close",
            "description": "Close the open library and go back to an empty, temporary session, so the desktop app can open it.",
            "inputSchema": no_arguments,
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
    ]
}

fn trim_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['\\', '/']);
    // Keep a bare root such as `/` or `C:\`.
    if trimmed.is_empty() || trimmed.ends_with(':') {
        path
    } else {
        trimmed
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let (a, b) = (trim_path(a), trim_path(b));
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn is_library(dir: &Path) -> bool {
    dir.join("catalog.snap").is_file()
        || std::fs::read_dir(dir).is_ok_and(|entries| entries.flatten().any(|e| e.path().extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lccat"))))
}

/// Library paths anywhere in an app's settings: `libraryPath`, `recentLibraries`, and the `paths`
/// of a recent-catalogs list.
fn collect_library_paths(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "libraryPath" | "recentLibraries" | "paths") {
                    match value {
                        Value::String(path) => out.push(path.clone()),
                        Value::Array(paths) => out.extend(paths.iter().filter_map(|p| p.as_str().map(str::to_owned))),
                        _ => {}
                    }
                } else {
                    collect_library_paths(value, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_library_paths(item, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_library_paths_in_settings() {
        let settings = json!({
            "library": { "libraryPath": "F:\\Photos\\Spain", "recentLibraries": ["F:\\Photos\\Spain", "F:\\Photos\\Vienna"] },
            "paths": ["F:\\Classic\\Classic.lccat"],
            "other": { "libraryPath": "" }
        });
        let mut paths = Vec::new();
        collect_library_paths(&settings, &mut paths);
        assert!(paths.contains(&"F:\\Photos\\Vienna".to_string()));
        assert!(paths.contains(&"F:\\Classic\\Classic.lccat".to_string()));
    }

    #[test]
    fn compares_paths_without_trailing_separators() {
        assert!(same_path("/photos/spain/", "/photos/spain"));
        assert_eq!(trim_path("/"), "/");
        assert_eq!(trim_path("C:\\"), "C:\\");
    }
}
