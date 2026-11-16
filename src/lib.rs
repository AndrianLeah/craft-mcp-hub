//! craft-mcp-hub: one shared, windowless MCP server per Artcraft app.
//!
//! Each app that is turned on gets a server on its own local port (`crafthub serve <app>`): the app's
//! command-line engine runs once inside it, and every AI client session connects to that one engine
//! through `crafthub connect <app>`, which is what the client launches as a stdio MCP server.

pub mod catalog;
pub mod clients;
pub mod config;
pub mod control;
pub mod discover;
pub mod doctor;
pub mod files;
pub mod guide;
pub mod hub;
pub mod logger;
pub mod paths;
pub mod proc;
pub mod profiles;
pub mod rpc;
pub mod server;
pub mod shim;
pub mod timefmt;
pub mod token;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
