//! What the tests stand on: a hub folder and a home folder of their own for each test, `crafthub`
//! run the way a person or an AI client runs it, and waiting with a deadline rather than for a
//! fixed time.

use crate::engine;
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// The longest anything may take. A test that is right never comes near it: it is there so that
/// one that is wrong fails instead of hanging, even on a slow CI machine.
pub const DEADLINE: Duration = Duration::from_secs(30);

/// Asks `check` again and again until it gives a value. Fails the test at the deadline.
pub fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A path as an argument. The test folders are made by the tests, so they are always Unicode.
pub fn arg(path: &Path) -> &str {
    path.to_str().expect("test paths are Unicode")
}

fn exe(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// This run's folder, with copies of the two programs in it.
struct Run {
    dir: PathBuf,
    hub: PathBuf,
    engine: PathBuf,
}

static RUN: OnceLock<Run> = OnceLock::new();

/// Servers and engines run from copies, so one left running (a test stopped halfway) never keeps
/// the next build from replacing the originals: Windows doesn't overwrite a program that runs.
fn run() -> &'static Run {
    RUN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("crafthub-e2e-tests-{}", std::process::id()));
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).expect("the run's folder can be made");
        let hub = bin.join(exe("crafthub"));
        fs::copy(env!("CARGO_BIN_EXE_crafthub"), &hub).expect("crafthub can be copied");
        let engine = bin.join(exe("fake-engine"));
        fs::copy(std::env::current_exe().expect("the test program knows where it is"), &engine).expect("the test program can be copied");
        Run { dir, hub, engine }
    })
}

/// Removes this run's folder, once every test is done.
pub fn clean_up() {
    if let Some(run) = RUN.get() {
        remove_dir(&run.dir);
    }
}

/// Removes a folder, waiting a little for programs that are ending to let go of their files.
fn remove_dir(dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while dir.exists() && fs::remove_dir_all(dir).is_err() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

/// Whether something listens on the port, asked the way the hub asks.
pub fn port_in_use(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err()
}

/// Where the tests' servers listen: below the ports systems give outgoing connections (from 32768
/// on Linux, 49152 on Windows and macOS), so nothing takes a port between the moment a test picks
/// it and the moment its server listens on it, and above the hub's own default ports.
const PORTS: std::ops::Range<u16> = 21000..29000;

/// A port nothing listens on, and that no other test of this run has been given. Each run starts
/// at a random place in the range, so that two runs at once don't pick the same ports.
pub fn free_port() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    static GIVEN: Mutex<Option<(u16, HashSet<u16>)>> = Mutex::new(None);
    let mut given = lock(&GIVEN);
    let (next, taken) = given.get_or_insert_with(|| {
        let random = std::collections::hash_map::RandomState::new().build_hasher().finish();
        ((random % u64::from(PORTS.end - PORTS.start)) as u16, HashSet::new())
    });
    for _ in PORTS {
        let port = PORTS.start + *next;
        *next = (*next + 1) % (PORTS.end - PORTS.start);
        if !taken.contains(&port) && !port_in_use(port) {
            taken.insert(port);
            return port;
        }
    }
    panic!("no free port between {} and {}", PORTS.start, PORTS.end);
}

fn kill(pid: u64) {
    let pid = pid.to_string();
    let mut command = if cfg!(windows) {
        let mut taskkill = Command::new("taskkill");
        taskkill.args(["/F", "/PID", &pid]);
        taskkill
    } else {
        let mut kill = Command::new("kill");
        kill.args(["-9", &pid]);
        kill
    };
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// A hub of its own for one test: its settings, token and logs in a temporary folder, and a home
/// folder of its own so that the AI clients' settings files it looks at are the test's too.
/// Whatever servers it started are stopped when it is dropped.
pub struct TestHub {
    name: String,
    dir: PathBuf,
    /// The hub's folder (`CRAFTHUB_HOME`).
    pub home: PathBuf,
    /// The home folder the hub sees.
    pub user: PathBuf,
    record: PathBuf,
    ports: Mutex<Vec<u16>>,
    bridges: AtomicUsize,
}

impl TestHub {
    pub fn new(name: &str) -> TestHub {
        let dir = run().dir.join(name);
        remove_dir(&dir);
        let (home, user) = (dir.join("hub"), dir.join("user"));
        for folder in [&home, &user] {
            fs::create_dir_all(folder).expect("the test's folders can be made");
        }
        TestHub {
            name: name.to_string(),
            record: dir.join("engine-starts.jsonl"),
            dir,
            home,
            user,
            ports: Mutex::new(Vec::new()),
            bridges: AtomicUsize::new(0),
        }
    }

    /// The stand-in engine's program.
    pub fn engine(&self) -> &'static Path {
        &run().engine
    }

    /// The `crafthub` program the tests run.
    pub fn program(&self) -> &'static Path {
        &run().hub
    }

    /// `crafthub`, with everything it reads from the environment pointed at the test's folders.
    /// The servers it starts, and their engines, inherit the same.
    fn command(&self) -> Command {
        let user = &self.user;
        let mut command = Command::new(self.program());
        command
            .current_dir(&self.dir)
            .env("CRAFTHUB_HOME", &self.home)
            .env("HOME", user)
            .env("USERPROFILE", user)
            .env("APPDATA", user.join("AppData").join("Roaming"))
            .env("LOCALAPPDATA", user.join("AppData").join("Local"))
            .env("XDG_CONFIG_HOME", user.join(".config"))
            .env("XDG_DATA_HOME", user.join(".local").join("share"))
            .env("XDG_CACHE_HOME", user.join(".cache"))
            .env("CLAUDE_CONFIG_DIR", user.join(".claude"))
            .env("CODEX_HOME", user.join(".codex"))
            .env("CRAFTHUB_NO_UPDATE_CHECK", "1")
            .env_remove("CRAFTHUB_CLAUDE")
            .env_remove("CRAFTHUB_INSTALL_DIR")
            .env(engine::RECORD_VAR, &self.record)
            .stdin(Stdio::null());
        command
    }

    /// Runs `crafthub` with these arguments and returns what it printed. Fails the test when the
    /// command fails.
    pub fn run(&self, args: &[&str]) -> String {
        let output = self.command().args(args).output().expect("crafthub runs");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "`crafthub {}` failed: {printed}{}", args.join(" "), String::from_utf8_lossy(&output.stderr));
        printed
    }

    /// Runs `crafthub` with these arguments, which must fail, and returns what it said.
    pub fn run_failing(&self, args: &[&str]) -> String {
        let output = self.command().args(args).output().expect("crafthub runs");
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(!output.status.success(), "`crafthub {}` should have failed: {said}", args.join(" "));
        said
    }

    /// Adds the stand-in engine as a custom app, the way a person adds one, on a free port. It is
    /// not turned on: no AI client is told about it. Returns its port.
    pub fn add_app(&self, id: &str) -> u16 {
        let port = free_port();
        self.add_app_at(id, port);
        port
    }

    pub fn add_app_at(&self, id: &str, port: u16) {
        self.run(&["add", id, "--cli", arg(self.engine()), "--port", &port.to_string(), "--off"]);
        lock(&self.ports).push(port);
    }

    /// Has the servers started from now on stop their engine after this many minutes with no
    /// session, instead of 10 (`idle_stop_minutes` in the hub's settings).
    pub fn stop_idle_engines_after(&self, minutes: f64) {
        let file = self.home.join("config.toml");
        let settings = fs::read_to_string(&file).unwrap_or_default();
        // At the top: a setting for every app goes before the apps' tables.
        fs::write(&file, format!("idle_stop_minutes = {minutes}\n{settings}")).expect("the hub's settings can be written");
    }

    /// Moves the app's server to another free port (`crafthub port`). Returns the new port.
    pub fn move_app(&self, id: &str) -> u16 {
        let port = free_port();
        lock(&self.ports).push(port);
        self.run(&["port", id, &port.to_string()]);
        port
    }

    /// Starts `crafthub connect <app>` as an AI client does: a session talking MCP over its stdio.
    pub fn connect(&self, app: &str) -> Session {
        let n = self.bridges.fetch_add(1, Ordering::Relaxed) + 1;
        let stderr = File::create(self.dir.join(format!("bridge-{n}.stderr.txt"))).expect("the bridge's error output can be kept");
        let mut child =
            self.command().args(["connect", app]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).spawn().expect("crafthub connect starts");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("the bridge's output");
        let (lines, received) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let message = serde_json::from_str(&line).unwrap_or(Value::String(line));
                if lines.send(message).is_err() {
                    break;
                }
            }
        });
        Session { child, stdin, received, kept: VecDeque::new(), next_id: 0 }
    }

    /// The hub token, once a server or a bridge has made it.
    pub fn token(&self) -> Option<String> {
        fs::read_to_string(self.home.join("token")).ok().map(|token| token.trim().to_string())
    }

    /// A connection to the server on the port that has presented the token, as the panel opens one.
    fn hub_connection(&self, port: u16) -> Option<Raw> {
        let token = self.token()?;
        if !port_in_use(port) {
            return None;
        }
        let mut raw = Raw::open(port)?;
        raw.send(&json!({ "hub": "hello", "token": token, "v": 1 }));
        Some(raw)
    }

    /// What the server on the port says it is doing (`hub/status`); `None` when none answers.
    pub fn status(&self, port: u16) -> Option<Value> {
        let mut raw = self.hub_connection(port)?;
        raw.send(&json!({ "jsonrpc": "2.0", "id": "status", "method": "hub/status" }));
        raw.answer(&json!("status"))?.get("result").cloned()
    }

    /// Waits until the server on the port says what `done` wants, and returns that status.
    pub fn wait_for_server(&self, port: u16, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        wait_for(what, || self.status(port).filter(&done))
    }

    /// Every start of the engine so far, oldest first, each with its process id and arguments.
    pub fn engine_starts(&self) -> Vec<Value> {
        fs::read_to_string(&self.record).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    /// The arguments of the engine's latest start.
    pub fn engine_args(&self) -> Vec<String> {
        let starts = self.engine_starts();
        let latest = starts.last().expect("the engine has started");
        serde_json::from_value(latest["args"].clone()).expect("the engine recorded its arguments")
    }

    /// The folder an app keeps its own settings in on this system, where the hub looks for the
    /// libraries it used recently.
    pub fn app_settings(&self, folder: &str) -> PathBuf {
        let base = if cfg!(windows) {
            self.user.join("AppData").join("Roaming")
        } else if cfg!(target_os = "macos") {
            self.user.join("Library").join("Application Support")
        } else {
            self.user.join(".config")
        };
        base.join(folder)
    }

    fn stop_server(&self, port: u16) {
        let Some(status) = self.status(port) else { return };
        if let Some(mut raw) = self.hub_connection(port) {
            raw.send(&json!({ "jsonrpc": "2.0", "id": "stop", "method": "hub/shutdown" }));
            let _ = raw.answer(&json!("stop"));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while port_in_use(port) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if let (true, Some(pid)) = (port_in_use(port), status["pid"].as_u64()) {
            kill(pid);
        }
    }

    /// The logs and what the bridges said, for a test that failed.
    fn show_what_happened(&self) {
        let mut files: Vec<PathBuf> = fs::read_dir(self.home.join("logs")).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
        files.extend(fs::read_dir(&self.dir).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.is_file()));
        files.sort();
        for file in files {
            let text = fs::read_to_string(&file).unwrap_or_default();
            eprintln!("---- {}: {} ----\n{}", self.name, file.display(), text.trim_end());
        }
    }
}

impl Drop for TestHub {
    fn drop(&mut self) {
        for port in std::mem::take(&mut *lock(&self.ports)) {
            self.stop_server(port);
        }
        if thread::panicking() {
            self.show_what_happened();
        }
        remove_dir(&self.dir);
    }
}

/// An AI client's session: `crafthub connect` with MCP on its stdio.
pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    received: Receiver<Value>,
    /// What arrived while the test was waiting for something else, in order.
    kept: VecDeque<Value>,
    next_id: u64,
}

impl Session {
    pub fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("the session is open");
        writeln!(stdin, "{message}").and_then(|_| stdin.flush()).expect("the bridge reads its input");
    }

    /// Sends a request and returns its id.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = json!(self.next_id);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Waits for the first message `wanted` picks and returns it. The others are kept, in order.
    pub fn expect(&mut self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        if let Some(found) = self.kept.iter().position(&wanted) {
            return self.kept.remove(found).expect("it is there");
        }
        let deadline = Instant::now() + DEADLINE;
        loop {
            match self.received.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(message) if wanted(&message) => return message,
                Ok(message) => self.kept.push_back(message),
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for {what}; received meanwhile: {:?}", self.kept),
                Err(RecvTimeoutError::Disconnected) => panic!("the bridge ended while waiting for {what}; received: {:?}", self.kept),
            }
        }
    }

    /// The answer to the request with this id.
    pub fn answer(&mut self, id: &Value) -> Value {
        self.expect(&format!("the answer to request {id}"), |message| message.get("id") == Some(id) && message.get("method").is_none())
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.answer(&id)
    }

    /// The MCP handshake. Returns the server's answer to `initialize`.
    pub fn initialize(&mut self, client: &str) -> Value {
        let answer =
            self.call("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": client, "version": "1.0" } }));
        let result = answer.get("result").cloned().unwrap_or_else(|| panic!("{client} could not initialize: {answer}"));
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        result
    }

    /// Calls a tool and returns its result. Fails the test on an error answer.
    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.call("tools/call", json!({ "name": name, "arguments": arguments }));
        answer.get("result").cloned().unwrap_or_else(|| panic!("{name} got an error answer: {answer}"))
    }

    /// Everything that has arrived and not been picked yet, without waiting.
    pub fn unclaimed(&mut self) -> Vec<Value> {
        while let Ok(message) = self.received.try_recv() {
            self.kept.push_back(message);
        }
        self.kept.iter().cloned().collect()
    }

    /// The notifications with this method among those not picked yet.
    pub fn notifications(&mut self, method: &str) -> Vec<Value> {
        self.unclaimed().into_iter().filter(|message| message["method"] == method).collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A plain connection to a server, without the bridge.
pub struct Raw {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
}

impl Raw {
    pub fn open(port: u16) -> Option<Raw> {
        let stream = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(5)).ok()?;
        stream.set_read_timeout(Some(DEADLINE)).ok()?;
        // Each line goes out at once, as the hub's own connections send them.
        stream.set_nodelay(true).ok()?;
        Some(Raw { reader: BufReader::new(stream.try_clone().ok()?), stream })
    }

    pub fn send(&mut self, message: &Value) {
        let _ = self.stream.write_all(format!("{message}\n").as_bytes());
    }

    /// The next message, or `None` once the server has closed the connection.
    pub fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap_or(Value::String(line))),
        }
    }

    /// The answer with this id, or the server's refusal.
    pub fn answer(&mut self, id: &Value) -> Option<Value> {
        loop {
            let message = self.read()?;
            if message.get("id") == Some(id) || message.get("hub").is_some() {
                return Some(message);
            }
        }
    }
}
