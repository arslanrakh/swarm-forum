//! A minimal MCP client over streamable HTTP (protocol 2025-11-25, the server's legacy session
//! mode): `initialize` returns an `Mcp-Session-Id`, `notifications/initialized` follows, and every
//! `tools/call` carries the id. A response is JSON or an event stream whose `data:` lines carry the
//! JSON-RPC message; the client reads the whole body, so a parked `talk wait` simply takes longer.
//!
//! Each simulated agent owns one [`Mcp`], with its own HTTP client and so its own connection, as
//! a real agent process would.

use std::time::Duration;

use serde_json::{Value, json};

/// The protocol version the client asks for: the last one with `initialize` and sessions.
pub const PROTOCOL: &str = "2025-11-25";

/// How a call ended.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    /// The call succeeded.
    Ok,
    /// The server refused it with this code (§21), such as `full` or `limit(busy)`.
    Refused(String),
    /// A command this build of the server does not do yet: `invalid` with the stub's hint, for an
    /// action the driver knows to be a stub ([`crate::mix::Action::stubbed_at_writing`]). The
    /// agent decides it ([`Called::is_stub_answer`]); [`classify`] never does.
    Stubbed,
    /// The call never got a tool result: an HTTP status, a JSON-RPC error, a connection error, a
    /// timeout, or a body that does not parse.
    Transport(String),
}

impl Outcome {
    /// A short name for counting: `ok`, the code, `stubbed`, or `transport:<what>`.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Outcome::Ok => "ok".into(),
            Outcome::Refused(c) => c.clone(),
            Outcome::Stubbed => "stubbed".into(),
            Outcome::Transport(t) => format!("transport:{t}"),
        }
    }
}

/// What a tool call returned.
#[derive(Clone, Debug)]
pub struct Called {
    /// How it ended.
    pub outcome: Outcome,
    /// The text of the result's first content block.
    pub text: String,
    /// `structuredContent`, if any.
    pub structured: Option<Value>,
    /// The hint of a refusal, if any.
    pub hint: String,
}

impl Called {
    fn transport(what: impl Into<String>) -> Called {
        Called {
            outcome: Outcome::Transport(what.into()),
            text: String::new(),
            structured: None,
            hint: String::new(),
        }
    }

    /// Whether the answer is the one a stub gives: `invalid` with the stub's hint. The server gives
    /// that hint for every `invalid(BadParams)`, a real refusal of an implemented command's
    /// arguments included, so only the caller, who knows which commands are stubs, may count it
    /// as stubbed.
    #[must_use]
    pub fn is_stub_answer(&self) -> bool {
        self.outcome == Outcome::Refused("invalid".into()) && self.hint.contains(STUB_HINT)
    }

    /// A field of the structured content.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.structured.as_ref().and_then(|s| s.get(key))
    }

    /// A string field of the structured content.
    #[must_use]
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// The forum session named in the status line (`session s-<n> (<class>)`, 1.7).
    #[must_use]
    pub fn session(&self) -> Option<String> {
        let rest = self.text.lines().next()?.split(" · session ").nth(1)?;
        let s = rest.split([' ', '·']).next()?;
        s.starts_with("s-").then(|| s.to_string())
    }
}

/// Part of the hint of every `invalid(BadParams)`, which the core also answers for a command that
/// a later work package will implement: the driver counts such an answer as stubbed only for the
/// actions it knows to be stubs, and as an error for any other.
pub const STUB_HINT: &str = "does not do this yet";

/// Classifies a tool result: its `isError`, and its code from `structuredContent.error` or from
/// the text's `error: <code>` line, with the kind of a `limit` named from its hint. An `invalid`
/// stays `invalid` whatever its hint (see [`Called::is_stub_answer`]).
#[must_use]
pub fn classify(result: &Value) -> Called {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let structured = result.get("structuredContent").cloned();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !is_error {
        return Called {
            outcome: Outcome::Ok,
            text,
            structured,
            hint: String::new(),
        };
    }
    let from_text = || {
        text.lines()
            .find_map(|l| l.strip_prefix("error: "))
            .map(|c| c.trim().to_string())
    };
    let code = structured
        .as_ref()
        .and_then(|s| s.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(from_text)
        .unwrap_or_else(|| "unknown".into());
    let hint = structured
        .as_ref()
        .and_then(|s| s.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            text.lines()
                .find_map(|l| l.strip_prefix("hint: "))
                .map(str::to_string)
        })
        .unwrap_or_default();
    let outcome = if code == "limit" {
        Outcome::Refused(format!("limit({})", limit_kind(&hint)))
    } else {
        Outcome::Refused(code)
    };
    Called {
        outcome,
        text,
        structured,
        hint,
    }
}

/// Which safety valve a `limit` names (§19, §21), from its hint.
fn limit_kind(hint: &str) -> &'static str {
    let h = hint.to_ascii_lowercase();
    if h.contains("busy") || h.contains("queue") {
        "busy"
    } else if h.contains("per_hour") || h.contains("per hour") {
        "per_hour"
    } else if h.contains("text.max") || h.contains("bytes") {
        "text"
    } else if h.contains("renew") {
        "renews"
    } else if h.contains("tree") || h.contains("children") || h.contains("depth") {
        "tree"
    } else {
        "other"
    }
}

/// Finds the JSON-RPC message with `id` in a response body: plain JSON or an event stream.
#[must_use]
pub fn parse_body(body: &str, id: u64) -> Option<Value> {
    let matches = |v: &Value| v.get("id").and_then(Value::as_u64) == Some(id);
    if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
        return matches(&v).then_some(v);
    }
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(matches)
}

/// One simulated agent's MCP connection.
pub struct Mcp {
    http: reqwest::Client,
    url: String,
    auth: String,
    session: Option<String>,
    next_id: u64,
    timeout: Duration,
}

impl Mcp {
    /// A client for `url` with bearer `token`, not yet initialised. `timeout` bounds every
    /// request (a parked wait's included).
    ///
    /// # Errors
    ///
    /// The HTTP client could not be built.
    pub fn new(url: &str, token: &str, timeout: Duration) -> Result<Mcp, String> {
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(1)
            .tcp_nodelay(true)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Mcp {
            http,
            url: url.to_string(),
            auth: format!("Bearer {token}"),
            session: None,
            next_id: 1,
            timeout,
        })
    }

    /// The MCP session id, once initialised.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Forgets the MCP session: the next [`Mcp::initialize`] opens a new one on the same
    /// connection, as a new agent on the same machine would.
    pub fn reset(&mut self) {
        self.session = None;
    }

    async fn post(
        &self,
        body: &Value,
    ) -> Result<(reqwest::StatusCode, Option<String>, String), String> {
        let mut req = self
            .http
            .post(&self.url)
            .timeout(self.timeout)
            .header("authorization", &self.auth)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL)
            .body(body.to_string());
        if let Some(s) = &self.session {
            req = req.header("mcp-session-id", s);
        }
        let resp = req.send().await.map_err(|e| error_kind(&e))?;
        let status = resp.status();
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = resp.text().await.map_err(|e| error_kind(&e))?;
        Ok((status, sid, text))
    }

    /// `initialize` and `notifications/initialized`: a new MCP session.
    ///
    /// # Errors
    ///
    /// A transport failure, an HTTP status other than 200, or no session id.
    pub async fn initialize(&mut self) -> Result<(), String> {
        self.session = None;
        let id = self.take_id();
        let body = json!({
            "jsonrpc": "2.0", "id": id, "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "swarm-sim", "version": env!("CARGO_PKG_VERSION")}
            }
        });
        let (status, sid, text) = self.post(&body).await?;
        if !status.is_success() {
            return Err(format!("http_{}", status.as_u16()));
        }
        if parse_body(&text, id).is_none_or(|v| v.get("result").is_none()) {
            return Err("bad_initialize".into());
        }
        self.session = Some(sid.ok_or("no_session_id")?);
        let (status, _, _) = self
            .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        if !status.is_success() {
            return Err(format!("http_{}", status.as_u16()));
        }
        Ok(())
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// `tools/call` of `tool` with `args`.
    pub async fn call(&mut self, tool: &str, args: Value) -> Called {
        let id = self.take_id();
        let body = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": args}
        });
        let (status, _, text) = match self.post(&body).await {
            Ok(x) => x,
            Err(e) => return Called::transport(e),
        };
        if !status.is_success() {
            return Called::transport(format!("http_{}", status.as_u16()));
        }
        let Some(msg) = parse_body(&text, id) else {
            return Called::transport("bad_body");
        };
        if let Some(e) = msg.get("error") {
            let code = e.get("code").and_then(Value::as_i64).unwrap_or(0);
            return Called::transport(format!("rpc_{code}"));
        }
        match msg.get("result") {
            Some(r) => classify(r),
            None => Called::transport("no_result"),
        }
    }
}

/// A transport error's kind, for counting: `timeout`, `connect` (with the OS error when it names
/// one, such as too many open files), `body`, or `request`.
fn error_kind(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "timeout".into();
    }
    let mut source: Option<&dyn std::error::Error> = std::error::Error::source(e);
    while let Some(s) = source {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            return match io.raw_os_error() {
                Some(24) => "emfile".into(),
                Some(n) => format!("os_{n}"),
                None => format!("io_{:?}", io.kind()).to_ascii_lowercase(),
            };
        }
        source = s.source();
    }
    if e.is_connect() {
        "connect".into()
    } else if e.is_body() || e.is_decode() {
        "body".into()
    } else {
        "request".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_is_classified_by_its_code_and_hint() {
        let ok = classify(&json!({"content": [{"type": "text", "text":
            "[swarm] running · now 2026-10-03T09:46:00Z · session s-12 (coding-agent) · holds 0 · inbox 0\n{}"}],
            "structuredContent": {"token": "9.1"}, "isError": false}));
        assert_eq!(ok.outcome, Outcome::Ok);
        assert_eq!(ok.session().as_deref(), Some("s-12"));
        assert_eq!(ok.str("token"), Some("9.1"));
        let stub = classify(
            &json!({"content": [{"type": "text", "text": "error: invalid"}],
            "structuredContent": {"error": "invalid", "hint":
            "the parameters break a rule of the constitution, or this server does not do this yet"},
            "isError": true}),
        );
        // The same answer as a real refusal of an implemented command's arguments: only the
        // caller can tell a stub.
        assert_eq!(stub.outcome, Outcome::Refused("invalid".into()));
        assert!(stub.is_stub_answer());
        assert!(!ok.is_stub_answer());
        let full = classify(
            &json!({"content": [{"type": "text", "text": "x\nerror: full\nhint: no slot"}],
            "isError": true}),
        );
        assert_eq!(full.outcome, Outcome::Refused("full".into()));
        let busy = classify(
            &json!({"content": [{"type": "text", "text": "error: limit"}],
            "structuredContent": {"error": "limit", "hint": "busy: the class's queue waited 10s; retry with the same op_id"},
            "isError": true}),
        );
        assert_eq!(busy.outcome.name(), "limit(busy)");
        assert_eq!(
            Outcome::Transport("timeout".into()).name(),
            "transport:timeout"
        );
    }

    #[test]
    fn a_body_is_json_or_an_event_stream() {
        let sse = "data: \nid: 0\nretry: 3000\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\nid: 1/0\n";
        assert!(parse_body(sse, 2).is_some());
        assert!(parse_body(sse, 3).is_none());
        assert!(parse_body("{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{}}", 5).is_some());
        assert!(parse_body("nonsense", 1).is_none());
    }
}
