# Contributing

Thanks for looking. Craft MCP Hub is a small project with one maintainer, and it is not affiliated with ArtCraft.

## What helps most

- **macOS and Linux.** The hub has only been run on Windows. Where the apps keep their command-line tools on other systems is a guess ([`src/discover.rs`](src/discover.rs)). If you use one of them, an issue with the output of `crafthub list` and `crafthub doctor <app>` is enough to fix it.
- **Apps whose start-up arguments aren't verified.** They are marked `probe = true` in [`assets/profiles.toml`](assets/profiles.toml).
- **AI clients the hub can't register with.** Say where the client keeps its MCP servers, and in what format.

## Building and checking

You need [Rust](https://rustup.rs). On Linux the panel also needs the window-system packages listed in [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

CI runs the first three on Windows, macOS and Linux, and a pull request has to pass them.

`cargo test` includes end-to-end tests ([`tests/e2e`](tests/e2e)): they run the real `crafthub`, its shared server and the bridge AI clients launch, against a stand-in engine, as a client would. Each test has its own hub folder, home folder and ports, so your own setup and AI clients are never touched. `cargo test --test e2e <name>` runs only the tests whose name contains `<name>`; a test that fails prints the server's log.

## Trying a change without touching your own setup

The hub edits your AI clients' settings, so point it somewhere else while you experiment:

```sh
export CRAFTHUB_HOME=/tmp/crafthub-test      # the hub's settings, token and logs
export CLAUDE_CONFIG_DIR=/tmp/claude-test    # where Claude Code keeps its settings
crafthub add figcraft --port 17805
```

In a fresh `CRAFTHUB_HOME` only Claude Code is enabled, and the other clients' real settings files are left alone unless you enable them. Use ports away from the defaults if your own servers are running. On Windows, run test servers from a copy of the build: a running `crafthub.exe` can't be overwritten by the next `cargo build`.

## Pull requests

Keep them small and about one thing, and say what you tested and on which system. Changes to how the hub edits other programs' settings files ([`src/clients.rs`](src/clients.rs), [`src/files.rs`](src/files.rs)) need tests.

For a security problem, don't open an issue: see [SECURITY.md](SECURITY.md).
