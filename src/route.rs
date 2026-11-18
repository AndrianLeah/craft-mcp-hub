//! Where the requests an engine sends to its client go, in a server shared by several sessions.
//!
//! The engine has one client, the shared server, with any number of AI client sessions behind it.
//! `ping` and `roots/list` are answered by the server itself. Anything else (`sampling/createMessage`,
//! `elicitation/create`, …) needs a real client: it goes to the session whose request the engine is
//! working on. That is the session whose request the engine names in the request's `_meta`, else the
//! only session with requests in flight. When it can't be told, the request is refused rather than
//! put to a session it may not be for.

use serde_json::{json, Map, Value};
use std::path::Path;

/// The client capabilities the server declares to its engine: roots, which it answers itself, and
/// sampling and elicitation, which it puts to the session they are for (and refuses for a session
/// that can't do them). The folders are those the engine was started with, so they never change
/// while it runs: no `listChanged`.
pub fn hub_capabilities() -> Value {
    json!({ "roots": {}, "sampling": {}, "elicitation": {} })
}

/// The answer to `roots/list`: the folders the engine may use, or none when it may use any file.
pub fn roots_list(folders: &[String]) -> Value {
    let roots: Vec<Value> = folders
        .iter()
        .map(|folder| {
            let name = Path::new(folder).file_name().map_or_else(|| folder.clone(), |name| name.to_string_lossy().into_owned());
            json!({ "uri": file_uri(folder), "name": name })
        })
        .collect();
    json!({ "roots": roots })
}

/// A folder as a `file://` URI: `/home/me/Photos` is `file:///home/me/Photos`, `C:\Users\me` is
/// `file:///C:/Users/me` and `\\server\share` is `file://server/share`.
pub fn file_uri(path: &str) -> String {
    file_uri_on(path, cfg!(windows))
}

fn file_uri_on(path: &str, windows: bool) -> String {
    let mut path = if windows { path.replace('\\', "/") } else { path.to_string() };
    if windows {
        // Verbatim paths: `\\?\C:\Photos`, `\\?\UNC\server\share`.
        if let Some(rest) = path.strip_prefix("//?/").or_else(|| path.strip_prefix("//./")).map(str::to_owned) {
            path = rest.strip_prefix("UNC/").map_or(rest.clone(), |share| format!("//{share}"));
        }
    }
    let (host, path) = match path.strip_prefix("//") {
        // A network share: the server is the URI's host.
        Some(share) if windows => match share.split_once('/') {
            Some((host, rest)) => (host.to_string(), format!("/{rest}")),
            None => (share.to_string(), String::new()),
        },
        _ if path.starts_with('/') => (String::new(), path),
        // `C:/Users`
        _ => (String::new(), format!("/{path}")),
    };
    format!("file://{}{}", percent_encode(&host), percent_encode(&path))
}

/// Everything but letters, digits, `-._~`, `/` and `:` percent-encoded, as UTF-8.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The progress token the server gives the engine for a session's request, in place of the
/// session's own.
pub fn progress_token(request: u64) -> Value {
    json!(format!("p{request}"))
}

/// The request a progress token from [`progress_token`] stands for.
pub fn request_of_token(token: &Value) -> Option<u64> {
    token.as_str()?.strip_prefix('p')?.parse().ok()
}

/// The session request an engine's request says it belongs to, from its `_meta`: the id the server
/// gave that request in `relatedRequestId`, or the progress token it gave it in `progressToken`.
pub fn named_request(params: Option<&Value>) -> Option<u64> {
    let meta = params?.get("_meta")?;
    match meta.get("relatedRequestId") {
        Some(id) => id.as_u64().or_else(|| id.as_str()?.parse().ok()),
        None => request_of_token(meta.get("progressToken")?),
    }
}

/// Why an engine's request can't be put to a session.
#[derive(Debug, PartialEq, Eq)]
pub enum Unrouted {
    /// It names a session request that isn't in flight any more.
    Finished,
    /// No session has a request in flight.
    Idle,
    /// Requests of this many sessions are in flight, and it names none of them.
    Ambiguous(usize),
}

impl Unrouted {
    /// What the engine is told.
    pub fn message(&self) -> String {
        let cannot = "The shared hub server can't put this request to an AI client session";
        match self {
            Unrouted::Finished => format!("{cannot}: the session request it belongs to has already been answered or cancelled"),
            Unrouted::Idle => format!("{cannot}: no session has a request in flight, so none is waiting for it"),
            Unrouted::Ambiguous(sessions) => format!(
                "{cannot}: {sessions} sessions have requests in flight and it doesn't say which one it belongs to \
                 (the server routes by the id it gave that request, in _meta.relatedRequestId)"
            ),
        }
    }
}

/// The session an engine's request is for: the one whose request it names, else the only session
/// with requests in flight. `in_flight` holds each request the engine is working on as (the id the
/// server gave it, the session it came from).
pub fn pick_session(named: Option<u64>, in_flight: &[(u64, u64)]) -> Result<u64, Unrouted> {
    if let Some(named) = named {
        return in_flight.iter().find(|(request, _)| *request == named).map(|(_, session)| *session).ok_or(Unrouted::Finished);
    }
    let mut sessions: Vec<u64> = in_flight.iter().map(|(_, session)| *session).collect();
    sessions.sort_unstable();
    sessions.dedup();
    match sessions.as_slice() {
        [] => Err(Unrouted::Idle),
        [only] => Ok(*only),
        several => Err(Unrouted::Ambiguous(several.len())),
    }
}

/// The client capability a request needs, for the request families MCP gives clients.
pub fn needed_capability(method: &str) -> Option<&str> {
    let (family, _) = method.split_once('/')?;
    matches!(family, "sampling" | "elicitation" | "roots" | "tasks").then_some(family)
}

/// Whether a session declared a capability in its `initialize`.
pub fn declares(capabilities: &Map<String, Value>, capability: &str) -> bool {
    capabilities.get(capability).is_some_and(|value| !matches!(value, Value::Null | Value::Bool(false)))
}

/// Takes out of an engine's capabilities, as a session is told them, what one engine shared by
/// every session can't do for each of them apart: resource subscriptions (the engine keeps one set
/// for everybody, so one session's unsubscribe would end another's, and every session would hear of
/// every update) and tasks (listed and announced to everybody).
pub fn keep_routable(capabilities: &mut Map<String, Value>) {
    if let Some(resources) = capabilities.get_mut("resources").and_then(Value::as_object_mut) {
        resources.remove("subscribe");
    }
    capabilities.remove("tasks");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_goes_to_the_session_it_names() {
        // Sessions 1 and 2 both have requests in flight; the engine says it works on request 11.
        let in_flight = [(10, 1), (11, 2), (12, 1)];
        assert_eq!(pick_session(Some(11), &in_flight), Ok(2));
        assert_eq!(pick_session(Some(10), &in_flight), Ok(1));
        // A request that was answered (or cancelled) since: nobody to ask, even with one session left.
        assert_eq!(pick_session(Some(9), &in_flight), Err(Unrouted::Finished));
        assert_eq!(pick_session(Some(9), &[(10, 1)]), Err(Unrouted::Finished));
    }

    #[test]
    fn without_a_name_it_goes_to_the_only_session_with_requests_in_flight() {
        assert_eq!(pick_session(None, &[(4, 7)]), Ok(7));
        // Several requests, all of one session: still that session.
        assert_eq!(pick_session(None, &[(4, 7), (5, 7), (6, 7)]), Ok(7));
    }

    #[test]
    fn it_is_refused_rather_than_guessed() {
        assert_eq!(pick_session(None, &[]), Err(Unrouted::Idle));
        assert_eq!(pick_session(None, &[(4, 7), (5, 8)]), Err(Unrouted::Ambiguous(2)));
        assert_eq!(pick_session(None, &[(4, 7), (5, 8), (6, 9), (7, 8)]), Err(Unrouted::Ambiguous(3)));
        assert!(Unrouted::Ambiguous(2).message().contains("_meta.relatedRequestId"));
    }

    #[test]
    fn the_engine_names_a_request_in_its_meta() {
        assert_eq!(named_request(Some(&json!({ "_meta": { "relatedRequestId": 12 } }))), Some(12));
        assert_eq!(named_request(Some(&json!({ "_meta": { "relatedRequestId": "12" } }))), Some(12));
        // The progress token the server put in the session's request.
        assert_eq!(named_request(Some(&json!({ "_meta": { "progressToken": progress_token(5) } }))), Some(5));
        // relatedRequestId wins over a progress token.
        assert_eq!(named_request(Some(&json!({ "_meta": { "relatedRequestId": 3, "progressToken": "p5" } }))), Some(3));
        // Tokens the server didn't make, and requests without a name.
        assert_eq!(named_request(Some(&json!({ "_meta": { "progressToken": "abc" } }))), None);
        assert_eq!(named_request(Some(&json!({ "_meta": { "progressToken": 5 } }))), None);
        assert_eq!(named_request(Some(&json!({ "messages": [] }))), None);
        assert_eq!(named_request(None), None);
    }

    #[test]
    fn progress_tokens_lead_back_to_their_request() {
        assert_eq!(request_of_token(&progress_token(42)), Some(42));
        assert_eq!(request_of_token(&json!("q42")), None);
    }

    #[test]
    fn requests_need_the_capability_of_their_family() {
        assert_eq!(needed_capability("sampling/createMessage"), Some("sampling"));
        assert_eq!(needed_capability("elicitation/create"), Some("elicitation"));
        assert_eq!(needed_capability("roots/list"), Some("roots"));
        assert_eq!(needed_capability("tasks/get"), Some("tasks"));
        // Nothing MCP defines for clients: the session answers for itself.
        assert_eq!(needed_capability("ping"), None);
        assert_eq!(needed_capability("vendor/thing"), None);

        let capabilities = json!({ "sampling": {}, "elicitation": null, "roots": { "listChanged": true }, "tasks": false });
        let capabilities = capabilities.as_object().unwrap();
        assert!(declares(capabilities, "sampling"));
        assert!(declares(capabilities, "roots"));
        assert!(!declares(capabilities, "elicitation"));
        assert!(!declares(capabilities, "tasks"));
        assert!(!declares(&Map::new(), "sampling"));
    }

    #[test]
    fn sessions_are_told_only_what_can_be_routed() {
        let mut capabilities = json!({
            "tools": { "listChanged": true },
            "resources": { "subscribe": true, "listChanged": true },
            "prompts": {},
            "logging": {},
            "tasks": { "list": {} }
        });
        keep_routable(capabilities.as_object_mut().unwrap());
        assert_eq!(capabilities, json!({ "tools": { "listChanged": true }, "resources": { "listChanged": true }, "prompts": {}, "logging": {} }));
    }

    #[test]
    fn roots_are_file_uris() {
        assert_eq!(file_uri_on("/home/me/Photos", false), "file:///home/me/Photos");
        assert_eq!(file_uri_on("/home/me/My Photos #2", false), "file:///home/me/My%20Photos%20%232");
        assert_eq!(file_uri_on("/home/me/Fotos/Málaga", false), "file:///home/me/Fotos/M%C3%A1laga");
        // A backslash is part of a name outside Windows.
        assert_eq!(file_uri_on("/tmp/a\\b", false), "file:///tmp/a%5Cb");

        assert_eq!(file_uri_on("C:\\Users\\me\\Pictures", true), "file:///C:/Users/me/Pictures");
        assert_eq!(file_uri_on("C:\\", true), "file:///C:/");
        assert_eq!(file_uri_on("D:/Art Work", true), "file:///D:/Art%20Work");
        assert_eq!(file_uri_on("\\\\nas\\photos\\2024", true), "file://nas/photos/2024");
        assert_eq!(file_uri_on("\\\\?\\C:\\Users\\me", true), "file:///C:/Users/me");
        assert_eq!(file_uri_on("\\\\?\\UNC\\nas\\photos", true), "file://nas/photos");
    }

    #[test]
    fn roots_list_names_each_folder() {
        assert_eq!(roots_list(&[]), json!({ "roots": [] }));
        let listed = roots_list(&["/home/me/Photos".to_string(), "/".to_string()]);
        assert_eq!(
            listed,
            json!({ "roots": [
                { "uri": "file:///home/me/Photos", "name": "Photos" },
                { "uri": "file:///", "name": "/" }
            ] })
        );
    }
}
