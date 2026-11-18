//! End-to-end tests: the real `crafthub` program, its shared server and the bridge AI clients
//! launch, against a stand-in engine. Each test has a hub folder and a home folder of its own in
//! a temporary folder, so the person's own settings and AI clients are never touched.
//!
//! This test program is also the stand-in engine: started with `CRAFTHUB_E2E_ENGINE` set, it is
//! an MCP engine instead (see `engine.rs`). So the engine is built with the tests and nothing
//! else, and no release build contains it. That is why the tests have their own small runner
//! (`harness = false`): the standard one owns `main`, and prints to stdout, which the engine
//! needs for MCP.
//!
//! `cargo test --test e2e` runs them; a name, or part of one, runs only those tests.

mod engine;
mod scenarios;
mod support;

use std::process::ExitCode;
use std::thread;
use std::time::Instant;

fn main() -> ExitCode {
    if let Some(record) = std::env::var_os(engine::RECORD_VAR) {
        return engine::run(record.into());
    }
    let outcome = run_tests(scenarios::ALL);
    support::clean_up();
    outcome
}

/// Runs the tests that the command line picks, all at once: each has its own folders and ports.
fn run_tests(all: &[scenarios::Test]) -> ExitCode {
    let (mut filters, mut skips) = (Vec::new(), Vec::new());
    let (mut exact, mut list, mut only_ignored) = (false, false, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exact" => exact = true,
            "--list" => list = true,
            "--ignored" => only_ignored = true,
            "--skip" => skips.extend(args.next()),
            // The standard runner's options that take a value; the others don't matter here.
            "--test-threads" | "--format" | "--color" | "--logfile" | "-Z" => {
                args.next();
            }
            flag if flag.starts_with('-') => {}
            filter => filters.push(filter.to_string()),
        }
    }
    let matches = |name: &str, pattern: &String| if exact { name == pattern } else { name.contains(pattern.as_str()) };
    let chosen: Vec<&scenarios::Test> = all
        .iter()
        .filter(|(name, _)| !only_ignored && (filters.is_empty() || filters.iter().any(|f| matches(name, f))) && !skips.iter().any(|s| matches(name, s)))
        .collect();
    if list {
        for (name, _) in &chosen {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }

    println!("\nrunning {} test{}", chosen.len(), if chosen.len() == 1 { "" } else { "s" });
    let started = Instant::now();
    let running: Vec<_> =
        chosen.iter().map(|&&(name, test)| (name, thread::Builder::new().name(name.to_string()).spawn(test).expect("a thread for the test"))).collect();
    let mut failed = Vec::new();
    for (name, test) in running {
        let passed = test.join().is_ok();
        println!("test {name} ... {}", if passed { "ok" } else { "FAILED" });
        if !passed {
            failed.push(name);
        }
    }
    let verdict = if failed.is_empty() { "ok" } else { "FAILED" };
    if !failed.is_empty() {
        println!("\nfailures:\n    {}", failed.join("\n    "));
    }
    println!(
        "\ntest result: {verdict}. {} passed; {} failed; {} filtered out; finished in {:.2}s\n",
        chosen.len() - failed.len(),
        failed.len(),
        all.len() - chosen.len(),
        started.elapsed().as_secs_f64()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
