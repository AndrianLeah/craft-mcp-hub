//! The stand-in engine: what the hub starts in place of an app's command-line tool. It speaks MCP
//! over stdio like the real ones, and writes down each start with its arguments, so the tests can
//! see the library and folder options the hub gave it.
//!
//! Its tools:
//! - `echo` answers with its `text`, after `steps` progress notifications when the caller asked
//!   for progress;
//! - `rendezvous` keeps its answer until a second call arrives, then answers both, the later one
//!   first;
//! - `announce` sends a logging notification with its `text`, then answers;
//! - `crash` ends the engine without answering.
//!
//! Every answer says which id the engine saw, its process id and the library it has open.

use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// Set to a file, it makes the test program the engine, which records its starts in that file.
pub const RECORD_VAR: &str = "CRAFTHUB_E2E_ENGINE";

/// The hub works out how to start a tool it has no profile for from its help: here `mcp`, a
/// library chosen with `--library`, and folders it may be given several of.
const HELP: &str = "fake-engine: the stand-in engine of craft-mcp-hub's end-to-end tests

Usage: fake-engine mcp [--library DIR] [--automation-read-root DIR]... [--automation-write-root DIR]...

Repeat either flag to grant access to more folders.
";

pub fn run(record: PathBuf) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let start = json!({ "pid": std::process::id(), "args": args });
    let written = OpenOptions::new().create(true).append(true).open(&record).and_then(|mut file| file.write_all(format!("{start}\n").as_bytes()));
    if let Err(e) = written {
        eprintln!("fake-engine: cannot write to {}: {e}", record.display());
        return ExitCode::FAILURE;
    }
    eprintln!("fake-engine: started with {}", args.join(" "));

    let library = args.iter().position(|arg| arg == "--library").and_then(|i| args.get(i + 1)).cloned();
    let mut engine = Engine { library, held: None };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Ok(message) = serde_json::from_str::<Value>(&line) {
            engine.handle(&message);
        }
    }
    ExitCode::SUCCESS
}

struct Engine {
    library: Option<String>,
    /// The `rendezvous` call waiting for a second one: its id and text.
    held: Option<(Value, String)>,
}

fn send(message: &Value) {
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{message}").and_then(|_| out.flush()).is_err() {
        // The hub has gone.
        std::process::exit(0);
    }
}

fn answer(id: &Value, result: Value) {
    send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn tools() -> Value {
    let text = |description: &str| json!({ "type": "object", "properties": { "text": { "type": "string", "description": description } } });
    json!([
        { "name": "echo", "description": "Answers with the text, reporting progress first when asked to.", "inputSchema": {
            "type": "object",
            "properties": { "text": { "type": "string" }, "steps": { "type": "integer", "description": "How many progress notifications to send" } }
        } },
        { "name": "rendezvous", "description": "Answers once a second call arrives, both at once.", "inputSchema": text("Returned in the answer") },
        { "name": "announce", "description": "Sends the text to every session as a logging notification.", "inputSchema": text("What to announce") },
        { "name": "crash", "description": "Ends the engine without answering.", "inputSchema": { "type": "object", "properties": {} } },
    ])
}

impl Engine {
    fn handle(&mut self, message: &Value) {
        // Notifications (no id) need nothing, and the engine never asks anything itself.
        let (Some(method), Some(id)) = (message.get("method").and_then(Value::as_str), message.get("id")) else { return };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        match method {
            "initialize" => answer(
                id,
                json!({
                    "protocolVersion": params.get("protocolVersion").cloned().unwrap_or_else(|| json!("2025-06-18")),
                    "capabilities": { "tools": {}, "logging": {} },
                    "serverInfo": { "name": "fake-engine", "version": "1.0" },
                    "instructions": "The stand-in engine of the end-to-end tests."
                }),
            ),
            "ping" => answer(id, json!({})),
            "tools/list" => answer(id, json!({ "tools": tools() })),
            "tools/call" => self.call(id.clone(), &params),
            _ => send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Unknown method {method}") } })),
        }
    }

    fn call(&mut self, id: Value, params: &Value) {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let text = arguments.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        match name {
            "echo" => {
                if let Some(token) = params.get("_meta").and_then(|meta| meta.get("progressToken")) {
                    let steps = arguments.get("steps").and_then(Value::as_u64).unwrap_or(0);
                    for step in 1..=steps {
                        send(&json!({
                            "jsonrpc": "2.0", "method": "notifications/progress",
                            "params": { "progressToken": token, "progress": step, "total": steps }
                        }));
                    }
                }
                answer(&id, self.reply(&id, &text));
            }
            "rendezvous" => match self.held.take() {
                None => self.held = Some((id, text)),
                Some((first_id, first_text)) => {
                    answer(&id, self.reply(&id, &text));
                    answer(&first_id, self.reply(&first_id, &first_text));
                }
            },
            "announce" => {
                send(&json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": { "level": "info", "logger": "fake-engine", "data": text } }));
                answer(&id, self.reply(&id, &text));
            }
            "crash" => {
                eprintln!("fake-engine: crashing, as asked");
                std::process::exit(3);
            }
            _ => answer(&id, json!({ "content": [{ "type": "text", "text": format!("Unknown tool {name}") }], "isError": true })),
        }
    }

    /// A tool's answer: the text, and what the engine knows about itself.
    fn reply(&self, id: &Value, text: &str) -> Value {
        json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": { "text": text, "engineId": id, "pid": std::process::id(), "library": self.library },
            "isError": false
        })
    }
}
