//! The tests: what an AI client and the person using the hub see, through `crafthub serve` and
//! `crafthub connect` as clients use them.

use crate::support::{arg, port_in_use, wait_for, Raw, Session, TestHub};
use serde_json::{json, Value};
use std::fs;
use std::thread;
use std::time::Duration;

pub type Test = (&'static str, fn());

pub const ALL: &[Test] = &[
    ("two_sessions_share_one_engine", two_sessions_share_one_engine),
    ("a_second_server_for_the_same_app_ends_at_once", a_second_server_for_the_same_app_ends_at_once),
    ("progress_reaches_the_session_that_asked", progress_reaches_the_session_that_asked),
    ("notifications_reach_every_session", notifications_reach_every_session),
    ("the_token_is_required", the_token_is_required),
    ("a_crashed_engine_is_restarted", a_crashed_engine_is_restarted),
    ("the_bridge_reconnects_when_the_server_restarts", the_bridge_reconnects_when_the_server_restarts),
    ("the_bridge_follows_the_server_to_another_port", the_bridge_follows_the_server_to_another_port),
    ("library_open_restarts_the_engine_on_the_library", library_open_restarts_the_engine_on_the_library),
    ("the_engine_gets_the_folders_it_may_use", the_engine_gets_the_folders_it_may_use),
    ("turning_an_app_off_stops_its_server", turning_an_app_off_stops_its_server),
    ("zed_settings_keep_their_comments", zed_settings_keep_their_comments),
    ("an_idle_engine_stops_and_the_next_session_starts_it", an_idle_engine_stops_and_the_next_session_starts_it),
    ("requests_wake_an_idle_engine_on_its_library", requests_wake_an_idle_engine_on_its_library),
];

/// What the stand-in engine put in a tool's answer.
fn content(result: &Value) -> &Value {
    &result["structuredContent"]
}

/// The text of a tool's answer, read as JSON (the hub's library tools answer that way).
fn text_json(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text in {result}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn rendezvous(id: &str, text: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": "rendezvous", "arguments": { "text": text } } })
}

fn two_sessions_share_one_engine() {
    let hub = TestHub::new("two_sessions_share_one_engine");
    let port = hub.add_app("fake");
    let mut a = hub.connect("fake");
    let init_a = a.initialize("client-a");
    let mut b = hub.connect("fake");
    let init_b = b.initialize("client-b");
    // Both hear the engine's own answer to initialize, with the hub's word on libraries added.
    assert_eq!(init_a["serverInfo"]["name"], "fake-engine");
    assert_eq!(init_a, init_b);
    assert!(init_a["instructions"].as_str().is_some_and(|text| text.contains("library_open")), "{init_a}");

    // The engine's tools, then the hub's library tools.
    let tools = a.call("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().expect("a list of tools").iter().filter_map(|tool| tool["name"].as_str()).collect();
    assert_eq!(names, ["echo", "rendezvous", "announce", "crash", "library_list", "library_open", "library_close"]);

    // The same id from both sessions at once. The engine sees two different ids, and each answer
    // goes back to the session that asked, under the id it asked with. (The engine keeps both
    // answers until it has both requests, then answers the later one first.)
    a.send(&rendezvous("same", "from a"));
    b.send(&rendezvous("same", "from b"));
    let answer_a = a.answer(&json!("same"));
    let answer_b = b.answer(&json!("same"));
    let (seen_a, seen_b) = (content(&answer_a["result"]), content(&answer_b["result"]));
    assert_eq!(seen_a["text"], "from a", "{answer_a}");
    assert_eq!(seen_b["text"], "from b", "{answer_b}");
    assert_ne!(seen_a["engineId"], seen_b["engineId"]);
    assert_ne!(seen_a["engineId"], "same");

    // One engine for both.
    assert_eq!(seen_a["pid"], seen_b["pid"]);
    assert_eq!(hub.engine_starts().len(), 1);
    let status = hub.status(port).expect("the server answers");
    assert_eq!(status["sessions"], 2, "{status}");
    assert_eq!(status["engine"]["restarts"], 0, "{status}");
    let mut names: Vec<&str> = status["sessionNames"].as_array().expect("session names").iter().filter_map(Value::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["client-a 1.0", "client-b 1.0"]);
}

fn a_second_server_for_the_same_app_ends_at_once() {
    let hub = TestHub::new("a_second_server_for_the_same_app_ends_at_once");
    let port = hub.add_app("fake");
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let first = hub.status(port).expect("the server answers")["pid"].clone();

    // Sessions that start at the same moment may each start a server, as this one is started
    // here. It ends at once: it doesn't wait for the port and take it over when the first one
    // stops, even after the app was turned off.
    hub.run(&["serve", "fake"]);
    let log = fs::read_to_string(hub.home.join("logs").join("fake.log")).unwrap_or_default();
    assert!(log.contains("already serves fake"), "{log}");
    // The first server and its engine carry on.
    assert_eq!(hub.status(port).expect("the server answers")["pid"], first);
    assert_eq!(content(&a.tool("echo", json!({ "text": "still here" })))["text"], "still here");
    assert_eq!(hub.engine_starts().len(), 1);
}

fn echo_with_progress(session: &mut Session, token: Value, steps: u64) -> Value {
    let id = session.request("tools/call", json!({ "name": "echo", "arguments": { "text": "working", "steps": steps }, "_meta": { "progressToken": token } }));
    session.answer(&id)
}

fn progress_reaches_the_session_that_asked() {
    let hub = TestHub::new("progress_reaches_the_session_that_asked");
    hub.add_app("fake");
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let mut b = hub.connect("fake");
    b.initialize("client-b");

    // Each session's progress comes back under the token it gave (the engine saw another one),
    // before the answer.
    let answer = echo_with_progress(&mut a, json!("token-a"), 3);
    assert!(answer.get("result").is_some(), "{answer}");
    let answer = echo_with_progress(&mut b, json!(42), 2);
    assert!(answer.get("result").is_some(), "{answer}");

    // Progress sent to the wrong session would have arrived before that session's next answer.
    a.tool("echo", json!({ "text": "anything else?" }));
    let progress_a = a.notifications("notifications/progress");
    let progress_b = b.notifications("notifications/progress");
    assert_eq!(progress_a.len(), 3, "{progress_a:?}");
    assert_eq!(progress_b.len(), 2, "{progress_b:?}");
    for (step, progress) in progress_a.iter().enumerate() {
        assert_eq!(progress["params"]["progressToken"], "token-a");
        assert_eq!(progress["params"]["progress"], step as u64 + 1);
    }
    assert!(progress_b.iter().all(|progress| progress["params"]["progressToken"] == 42), "{progress_b:?}");
}

fn notifications_reach_every_session() {
    let hub = TestHub::new("notifications_reach_every_session");
    hub.add_app("fake");
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let mut b = hub.connect("fake");
    b.initialize("client-b");
    // Connected, but it hasn't finished the handshake: the engine's notifications aren't for it yet.
    let mut c = hub.connect("fake");

    a.tool("announce", json!({ "text": "hello, everyone" }));
    for session in [&mut a, &mut b] {
        session.expect("the announcement", |message| message["method"] == "notifications/message" && message["params"]["data"] == "hello, everyone");
    }
    // A notification sent its way would have arrived before this answer.
    let pong = c.call("ping", json!({}));
    assert!(pong.get("result").is_some(), "{pong}");
    assert_eq!(c.unclaimed(), Vec::<Value>::new());
}

fn initialize_request() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "raw", "version": "1.0" } } })
}

fn the_token_is_required() {
    let hub = TestHub::new("the_token_is_required");
    let port = hub.add_app("fake");
    // Another hub, with a token of its own, set up for the same port before the first one's
    // server takes it.
    let other = TestHub::new("the_token_is_required-other");
    other.add_app_at("fake", port);
    let mut a = hub.connect("fake");
    a.initialize("client-a");

    // A wrong token: refused, and the connection closed.
    let mut wrong = Raw::open(port).expect("the server takes connections");
    wrong.send(&json!({ "hub": "hello", "token": "0".repeat(64), "v": 1 }));
    assert_eq!(wrong.read(), Some(json!({ "hub": "denied" })));
    assert_eq!(wrong.read(), None);

    // MCP straight away, without the token: the same.
    let mut none = Raw::open(port).expect("the server takes connections");
    none.send(&initialize_request());
    assert_eq!(none.read(), Some(json!({ "hub": "denied" })));
    assert_eq!(none.read(), None);

    // The token is all it takes.
    let mut right = Raw::open(port).expect("the server takes connections");
    right.send(&json!({ "hub": "hello", "token": hub.token().expect("the hub has a token"), "v": 1 }));
    right.send(&initialize_request());
    let answer = right.answer(&json!(1)).expect("an answer");
    assert_eq!(answer["result"]["serverInfo"]["name"], "fake-engine", "{answer}");

    // A bridge of the other hub is turned away, and says why.
    let said = other.run_failing(&["connect", "fake"]);
    assert!(said.contains("token differs"), "{said}");
}

fn a_crashed_engine_is_restarted() {
    let hub = TestHub::new("a_crashed_engine_is_restarted");
    let port = hub.add_app("fake");
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let before = content(&a.tool("echo", json!({ "text": "before" })))["pid"].clone();

    // The engine dies halfway through a request: the session hears it at once.
    let answer = a.call("tools/call", json!({ "name": "crash", "arguments": {} }));
    assert_eq!(answer["error"]["code"], -32000, "{answer}");

    // The server starts it again, and the same session carries on with the new one.
    hub.wait_for_server(port, "the engine to restart", |status| status["engine"]["state"] == "ready" && status["engine"]["restarts"] == 1);
    let after = content(&a.tool("echo", json!({ "text": "after" })))["pid"].clone();
    assert_ne!(before, after);
    assert_eq!(hub.engine_starts().len(), 2);
}

fn the_bridge_reconnects_when_the_server_restarts() {
    let hub = TestHub::new("the_bridge_reconnects_when_the_server_restarts");
    let port = hub.add_app("fake");
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let before = hub.status(port).expect("the server answers");

    // A request the engine is holding when the server goes away is answered, not left hanging.
    let held = a.request("tools/call", json!({ "name": "rendezvous", "arguments": { "text": "nobody comes" } }));
    hub.run(&["restart", "fake"]);
    let answer = a.answer(&held);
    assert!(answer["error"]["message"].as_str().is_some_and(|message| message.contains("restarted")), "{answer}");

    // The bridge finds the new server and does the handshake again by itself.
    let after = hub.wait_for_server(port, "the bridge to reconnect", |status| status["sessions"] == 1);
    assert_ne!(after["pid"], before["pid"]);
    assert_eq!(after["sessionNames"], json!(["client-a 1.0"]));
    // Until the bridge has its new connection in hand, a request is told to come back shortly.
    let result =
        wait_for("the session to work again", || a.call("tools/call", json!({ "name": "echo", "arguments": { "text": "again" } })).get("result").cloned());
    assert_eq!(content(&result)["text"], "again");
    assert_eq!(hub.engine_starts().len(), 2);
    // The client sent one initialize; it never sees the bridge's own.
    assert!(a.unclaimed().iter().all(|message| message.get("result").is_none()), "{:?}", a.unclaimed());
}

fn the_bridge_follows_the_server_to_another_port() {
    let hub = TestHub::new("the_bridge_follows_the_server_to_another_port");
    let old = hub.add_app("fake");
    let mut a = hub.connect("fake");
    a.initialize("client-a");

    // The person moves the app to another port while a session is open.
    let new = hub.move_app("fake");
    assert!(!port_in_use(old), "the server still runs on its old port");
    // The bridge looks the port up again, finds the server there and does the handshake again.
    hub.wait_for_server(new, "the bridge to reconnect on the new port", |status| status["sessions"] == 1);
    let result =
        wait_for("the session to work again", || a.call("tools/call", json!({ "name": "echo", "arguments": { "text": "moved" } })).get("result").cloned());
    assert_eq!(content(&result)["text"], "moved");
}

fn library_open_restarts_the_engine_on_the_library() {
    let hub = TestHub::new("library_open_restarts_the_engine_on_the_library");
    let port = hub.add_app("fake");
    // A library, and one the app's settings remember that is gone.
    let library = hub.user.join("Photos").join("Spain");
    fs::create_dir_all(&library).unwrap();
    fs::write(library.join("catalog.snap"), "").unwrap();
    let gone = hub.user.join("Photos").join("Gone");
    let settings = hub.app_settings("fake");
    fs::create_dir_all(&settings).unwrap();
    let recent = json!({ "library": { "libraryPath": library, "recentLibraries": [library, gone] } });
    fs::write(settings.join("ui.json"), recent.to_string()).unwrap();
    let (library, gone) = (arg(&library).to_string(), arg(&gone).to_string());

    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let mut b = hub.connect("fake");
    b.initialize("client-b");
    // The engine starts with no library open.
    assert!(!hub.engine_args().contains(&"--library".to_string()), "{:?}", hub.engine_args());

    let listed = text_json(&a.tool("library_list", json!({})));
    assert_eq!(
        listed,
        json!({ "open": null, "libraries": [
            { "name": "Spain", "path": library, "available": true },
            { "name": "Gone", "path": gone, "available": false },
        ] })
    );

    // Opening one restarts the engine on it, for every session.
    let opened = a.tool("library_open", json!({ "path": library }));
    assert_eq!(opened["isError"], false, "{opened}");
    assert_eq!(hub.engine_starts().len(), 2);
    assert!(hub.engine_args().ends_with(&["--library".to_string(), library.clone()]), "{:?}", hub.engine_args());
    assert_eq!(hub.status(port).expect("the server answers")["library"], library);
    assert_eq!(content(&b.tool("echo", json!({ "text": "and me?" })))["library"], library);
    assert_eq!(text_json(&b.tool("library_list", json!({})))["open"], library);

    // A folder that isn't a library is refused, and the engine keeps the one it has.
    let refused = a.tool("library_open", json!({ "path": arg(&hub.user) }));
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(hub.engine_starts().len(), 2);

    // Closing it goes back to no library.
    let closed = b.tool("library_close", json!({}));
    assert_eq!(closed["isError"], false, "{closed}");
    assert_eq!(hub.engine_starts().len(), 3);
    assert!(!hub.engine_args().contains(&"--library".to_string()), "{:?}", hub.engine_args());
    assert_eq!(hub.status(port).expect("the server answers")["library"], Value::Null);
    assert_eq!(content(&a.tool("echo", json!({ "text": "and now?" })))["library"], Value::Null);
}

fn the_engine_gets_the_folders_it_may_use() {
    let hub = TestHub::new("the_engine_gets_the_folders_it_may_use");
    hub.add_app("fake");
    // `check` starts the server and makes a full round trip through it.
    let checked = hub.run(&["check", "fake"]);
    assert!(checked.contains("OK") && checked.contains("7 tools") && checked.contains("fake-engine 1.0"), "{checked}");
    // Until folders are chosen: the home folder, for reading and for writing.
    let home = arg(&hub.user);
    assert_eq!(hub.engine_args(), ["mcp", "--automation-read-root", home, "--automation-write-root", home]);

    // Two folders: the server restarts with both, each for reading and for writing.
    let (one, two) = (hub.user.join("One"), hub.user.join("Two"));
    for folder in [&one, &two] {
        fs::create_dir_all(folder).unwrap();
    }
    let (one, two) = (arg(&one), arg(&two));
    hub.run(&["root", "fake", one, two]);
    assert_eq!(
        hub.engine_args(),
        ["mcp", "--automation-read-root", one, "--automation-read-root", two, "--automation-write-root", one, "--automation-write-root", two]
    );

    // No restriction: no folder options at all.
    hub.run(&["root", "fake", "--anywhere"]);
    assert_eq!(hub.engine_args(), ["mcp"]);
    assert_eq!(hub.engine_starts().len(), 3);
}

/// The app's entry in the test client's settings file.
fn registered(file: &std::path::Path) -> Value {
    let text = fs::read_to_string(file).unwrap_or_default();
    serde_json::from_str::<Value>(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))["mcpServers"]["fake"].clone()
}

fn turning_an_app_off_stops_its_server() {
    let hub = TestHub::new("turning_an_app_off_stops_its_server");
    // An AI client of the test's own, instead of the real ones.
    let client_file = hub.user.join("e2e-client").join("mcp.json");
    fs::create_dir_all(client_file.parent().unwrap()).unwrap();
    hub.run(&["clients", "off", "claude-code"]);
    hub.run(&["clients", "add", "e2e", "--file", arg(&client_file)]);
    hub.run(&["clients", "on", "e2e"]);
    let port = hub.add_app("fake");

    // On: registered with the client, and its server running.
    hub.run(&["on", "fake"]);
    let entry = registered(&client_file);
    assert_eq!(entry["args"], json!(["connect", "fake"]), "{entry}");
    // The same program, though perhaps by another path (macOS's temporary folder is behind a link).
    let command = entry["command"].as_str().unwrap_or_else(|| panic!("no command in {entry}"));
    assert_eq!(fs::canonicalize(command).ok(), fs::canonicalize(hub.program()).ok(), "{entry}");
    hub.wait_for_server(port, "the server", |status| status["app"] == "fake" && status["engine"]["state"] == "ready");
    let listed: Value = serde_json::from_str(&hub.run(&["list", "fake", "--json"])).unwrap();
    assert_eq!(listed[0]["state"], "On", "{listed}");
    let mut a = hub.connect("fake");
    a.initialize("client-a");

    // Off: out of the client, and nothing listens on the port any more.
    hub.run(&["off", "fake"]);
    assert_eq!(registered(&client_file), Value::Null);
    assert!(!port_in_use(port), "the server still runs");
    let listed: Value = serde_json::from_str(&hub.run(&["list", "fake", "--json"])).unwrap();
    assert_eq!(listed[0]["state"], "Off", "{listed}");
    // A session that was open is told the server is gone, instead of waiting on it.
    let answer = a.call("tools/call", json!({ "name": "echo", "arguments": { "text": "still there?" } }));
    assert!(answer.get("error").is_some(), "{answer}");
}

/// A built-in client whose settings have comments: Zed's, as Zed starts them. Turning an app on
/// adds only its entry, and turning it off gives back the file as it was.
fn zed_settings_keep_their_comments() {
    let hub = TestHub::new("zed_settings_keep_their_comments");
    // %APPDATA%\Zed on Windows; ~/.config/zed on macOS, and the XDG config folder on Linux.
    let dir = if cfg!(windows) { hub.user.join("AppData").join("Roaming").join("Zed") } else { hub.user.join(".config").join("zed") };
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    let before = "// Zed settings\n//\n// For information on how to configure Zed, see the Zed\n// documentation: https://zed.dev/docs/configuring-zed\n{\n  \"ui_font_size\": 16, // big\n  \"buffer_font_size\": 15\n}\n";
    fs::write(&file, before).unwrap();
    hub.run(&["clients", "off", "claude-code"]);
    let clients: Value = serde_json::from_str(&hub.run(&["clients", "--json"])).unwrap();
    let zed = clients.as_array().unwrap().iter().find(|client| client["id"] == "zed").cloned().unwrap_or_default();
    assert_eq!(zed["found"], true, "{zed}");
    hub.run(&["clients", "on", "zed"]);
    hub.add_app("fake");

    hub.run(&["on", "fake"]);
    let after = fs::read_to_string(&file).unwrap();
    assert!(after.starts_with("// Zed settings\n//\n// For information"), "{after}");
    assert!(after.contains("\"ui_font_size\": 16, // big"), "{after}");
    let parsed: Value = serde_json::from_str(&after.lines().filter(|line| !line.starts_with("//")).collect::<Vec<_>>().join("\n").replace(", // big", ","))
        .unwrap_or_else(|e| panic!("not JSON once the comments are out ({e}): {after}"));
    let entry = &parsed["context_servers"]["fake"];
    assert_eq!(entry["args"], json!(["connect", "fake"]), "{after}");
    let command = entry["command"].as_str().unwrap_or_else(|| panic!("no command in {after}"));
    assert_eq!(fs::canonicalize(command).ok(), fs::canonicalize(hub.program()).ok(), "{after}");
    let listed: Value = serde_json::from_str(&hub.run(&["list", "fake", "--json"])).unwrap();
    assert_eq!(listed[0]["state"], "On", "{listed}");

    hub.run(&["off", "fake"]);
    let off = fs::read_to_string(&file).unwrap();
    assert!(!off.contains("\"fake\""), "{off}");
    assert!(off.starts_with("// Zed settings\n") && off.contains("\"ui_font_size\": 16, // big"), "{off}");
}

/// Has the hub register its apps with an AI client of the test's own, instead of the real ones.
/// Returns that client's settings file.
fn use_test_client(hub: &TestHub) -> std::path::PathBuf {
    let client_file = hub.user.join("e2e-client").join("mcp.json");
    fs::create_dir_all(client_file.parent().unwrap()).unwrap();
    hub.run(&["clients", "off", "claude-code"]);
    hub.run(&["clients", "add", "e2e", "--file", arg(&client_file)]);
    hub.run(&["clients", "on", "e2e"]);
    client_file
}

fn an_idle_engine_stops_and_the_next_session_starts_it() {
    let hub = TestHub::new("an_idle_engine_stops_and_the_next_session_starts_it");
    let port = hub.add_app("fake");
    // 1.2 seconds with no session, instead of 10 minutes.
    hub.stop_idle_engines_after(0.02);
    use_test_client(&hub);
    hub.run(&["on", "fake"]);
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let first = content(&a.tool("echo", json!({ "text": "hello" })))["pid"].clone();

    // While a session is connected the engine keeps running, however quiet the session is.
    thread::sleep(Duration::from_secs(3));
    let status = hub.status(port).expect("the server answers");
    assert_eq!(status["engine"]["state"], "ready", "{status}");
    assert_eq!(content(&a.tool("echo", json!({ "text": "still you?" })))["pid"], first);

    // Once it has left, the engine is stopped: it exits by itself when its input is closed. The
    // server stays, on its port. Its status, asked all along as the panel does, doesn't count as
    // using the engine.
    drop(a);
    let idle = hub.wait_for_server(port, "the engine to stop", |status| status["engine"]["state"] == "idle" && status["engine"]["pid"] == 0);
    assert_eq!(idle["lastError"], "", "{idle}");
    assert_eq!(idle["engine"]["idleStop"], 1, "{idle}");
    // `crafthub list` says it plainly; the app is as ready for the next session as ever.
    let listed: Value = serde_json::from_str(&hub.run(&["list", "fake", "--json"])).unwrap();
    assert_eq!(listed[0]["state"], "Ready", "{listed}");
    assert_eq!(listed[0]["detail"], "idle: starts on the next request", "{listed}");

    // The next session's initialize starts it again, and waits for it. (Only the starts as an MCP
    // engine count: `doctor` below also runs the program for its version.)
    let engine_starts = || hub.engine_starts().into_iter().filter(|start| start["args"][0] == "mcp").count();
    let starts = engine_starts();
    let mut b = hub.connect("fake");
    let init = b.initialize("client-b");
    assert_eq!(init["serverInfo"]["name"], "fake-engine", "{init}");
    let second = content(&b.tool("echo", json!({ "text": "awake" })))["pid"].clone();
    assert_ne!(first, second);
    assert_eq!(engine_starts(), starts + 1);
    // Starting again is not a restart.
    let status = hub.status(port).expect("the server answers");
    assert_eq!(status["engine"]["restarts"], 0, "{status}");

    // `crafthub doctor` says it too, and its round trip starts the engine.
    drop(b);
    hub.wait_for_server(port, "the engine to stop again", |status| status["engine"]["state"] == "idle" && status["engine"]["pid"] == 0);
    let report = hub.run(&["doctor", "fake"]);
    assert!(report.contains("Engine    idle: starts on the next request"), "{report}");
    assert!(report.contains("[OK]  MCP test"), "{report}");
    assert_eq!(engine_starts(), starts + 2);
}

/// A request over a connection of its own, without the bridge and without initializing.
fn raw_request(port: u16, token: &str, request: Value) -> Value {
    let mut raw = Raw::open(port).expect("the server takes connections");
    raw.send(&json!({ "hub": "hello", "token": token, "v": 1 }));
    raw.send(&request);
    raw.answer(&request["id"]).expect("an answer")
}

fn requests_wake_an_idle_engine_on_its_library() {
    let hub = TestHub::new("requests_wake_an_idle_engine_on_its_library");
    let port = hub.add_app("fake");
    hub.stop_idle_engines_after(0.02);
    let library = hub.user.join("Photos").join("Spain");
    fs::create_dir_all(&library).unwrap();
    fs::write(library.join("catalog.snap"), "").unwrap();
    let library = arg(&library).to_string();

    // A session opens a library, then leaves: the engine is stopped, the library remembered.
    let mut a = hub.connect("fake");
    a.initialize("client-a");
    let opened = a.tool("library_open", json!({ "path": library }));
    assert_eq!(opened["isError"], false, "{opened}");
    drop(a);
    let idle = hub.wait_for_server(port, "the engine to stop", |status| status["engine"]["state"] == "idle" && status["engine"]["pid"] == 0);
    assert_eq!(idle["library"], library, "{idle}");
    let starts = hub.engine_starts().len();

    // Any request wakes it, also one that never initialized. Requests that arrive while it starts
    // all wait for it instead of being refused, and it opens the library again.
    let token = hub.token().expect("the hub has a token");
    let callers: Vec<_> = (0..3)
        .map(|n| {
            let token = token.clone();
            let echo = json!({ "jsonrpc": "2.0", "id": n, "method": "tools/call", "params": { "name": "echo", "arguments": { "text": "wake up" } } });
            thread::spawn(move || raw_request(port, &token, echo))
        })
        .collect();
    let answers: Vec<Value> = callers.into_iter().map(|caller| caller.join().expect("the request is answered")).collect();
    for answer in &answers {
        let seen = content(&answer["result"]);
        assert_eq!(seen["library"], library, "{answer}");
        assert_eq!(seen["pid"], content(&answers[0]["result"])["pid"], "one engine for them all: {answers:?}");
    }
    assert_eq!(hub.engine_starts().len(), starts + 1);
    assert!(hub.engine_args().ends_with(&["--library".to_string(), library.clone()]), "{:?}", hub.engine_args());

    // A crash is still a crash: the engine is restarted after the usual pause, and stopped again
    // since nobody uses it. Only the library change and the crash were restarts, however many
    // times the engine was woken.
    let crash = json!({ "jsonrpc": "2.0", "id": "crash", "method": "tools/call", "params": { "name": "crash", "arguments": {} } });
    let crashed = raw_request(port, &token, crash);
    assert_eq!(crashed["error"]["code"], -32000, "{crashed}");
    let idle = hub.wait_for_server(port, "the crashed engine to restart and stop again", |status| {
        status["engine"]["restarts"] == 2 && status["engine"]["state"] == "idle" && status["engine"]["pid"] == 0
    });
    assert_eq!(idle["library"], library, "{idle}");
}
