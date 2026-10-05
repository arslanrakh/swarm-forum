//! The MCP handler and its HTTP route (design D.11.2 to D.11.5).
//!
//! [`ForumServer`] implements rmcp's `ServerHandler` by hand: `get_info` serves the instructions
//! (§22), `list_tools` the six tools with their schemas (§20.2), with the cache hints that MCP
//! `2026-07-28` requires for a client on that version ([`list_tools_result`], D.14.7), and
//! `call_tool` runs the shell's part of a call: it reads the [`Caller`] that the bearer layer put
//! in the request, resolves the forum session (the `session` argument, else the binding of the
//! request's `Mcp-Session-Id`, else a new one for `session hello` and a refusal for any other call,
//! D.11.4, 1.6, §20.1), and decodes, masks, digests, and measures the call
//! ([`crate::decode::prepare`]). A call the decoder refuses is answered at once, with no session
//! and no next calls (§20.1, R27); any other goes to the [`Backend`] (the engine, WP-E4), and the
//! session its reply names is bound to the MCP session. While the first call of an MCP session
//! opens a forum session, the others of that MCP session wait for it, and then run in that
//! session ([`crate::session_bind::Claim`]). A call whose client cancels it is dropped, so the
//! engine skips it if it is still queued.
//!
//! [`router`] mounts the handler at `/mcp` behind the bearer layer: an unknown token gets HTTP
//! 401, and a request body over 1 MiB HTTP 413 before any tool call is decoded (the transport's
//! own limit, §19). `legacy_session_mode` keeps an `Mcp-Session-Id` for clients on protocol
//! versions before `2026-07-28` (A-32), and an idle MCP session expires after
//! `presence.silence` (CS-17).

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Router;
use axum::http::request::Parts;
use forum_core::{Dur, SessionCtx, SessionId, Time};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerConfig, Tool as McpTool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;

use super::tools::tools;
use crate::auth::{Caller, SharedTokens, bearer};
use crate::decode::{DecodeEnv, EntryLookup, Prepared, ShellAnswer, no_session, prepare};
use crate::session_bind::{Claim, Opening, SessionBinding};

/// The cap on an HTTP request body: 1 MiB (D.11.5).
pub const MAX_BODY_BYTES: usize = 1 << 20;

/// The header that carries the MCP session id.
pub const MCP_SESSION_ID: &str = "mcp-session-id";

/// One call, decoded and ready for the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The token's class and default caps.
    pub caller: Caller,
    /// The request's MCP session, if the client has one.
    pub mcp_session: Option<String>,
    /// The forum session the call runs in (D.11.4).
    pub session: SessionCtx,
    /// The decoded call.
    pub prepared: Prepared,
}

/// The engine's answer to a call.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The rendered text (D.11.7).
    pub text: String,
    /// `structuredContent`, if any.
    pub structured: Option<Value>,
    /// Whether the call was refused.
    pub is_error: bool,
    /// The forum session the call ran in or opened, which the MCP session is bound to.
    pub session: Option<SessionId>,
}

/// A call the shell refused before queueing (§21: it reaches nothing else).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The token's class and default caps.
    pub caller: Caller,
    /// The request's MCP session, if the client has one.
    pub mcp_session: Option<String>,
    /// The session the call names, by its `session` argument (when that decodes) or by its
    /// binding; `New` when it names none (D.11.4).
    pub session: SessionCtx,
    /// The refusal.
    pub answer: ShellAnswer,
}

/// What the handler needs from the engine (WP-E4).
pub trait Backend: Send + Sync + 'static {
    /// The shell's clock (D.8.1: `clock::now_ms`).
    fn now(&self) -> Time;
    /// `text.max`.
    fn text_max(&self) -> u32;
    /// `coverage.window`.
    fn coverage_window(&self) -> Dur;
    /// The shell's entry reads (D.11.5).
    fn entries(&self) -> &(dyn EntryLookup + Sync);
    /// Queues the call and answers it (D.11.2).
    fn call(&self, call: Call) -> impl Future<Output = Reply> + Send;
    /// Renders a refusal of the shell. A call that names no existing session of its class has no
    /// status line: its text starts with `error: <code>` (§20.1, 1.4, R27), which is this default.
    /// The engine overrides it to keep the status line, rendered from the state as it stands, for
    /// a call that names an existing session of its class (D.11.7). It appends nothing and opens
    /// no session either way.
    fn refuse(&self, refusal: Refusal) -> impl Future<Output = Reply> + Send {
        std::future::ready(Reply {
            text: refusal.answer.render(),
            structured: None,
            is_error: true,
            session: None,
        })
    }
}

/// The MCP handler. Cloned once per MCP session; the clones share the backend and the binding
/// map.
pub struct ForumServer<B> {
    backend: Arc<B>,
    binding: Arc<Mutex<SessionBinding>>,
    instructions: Arc<str>,
    tools: Arc<Vec<McpTool>>,
}

impl<B> Clone for ForumServer<B> {
    fn clone(&self) -> Self {
        ForumServer {
            backend: Arc::clone(&self.backend),
            binding: Arc::clone(&self.binding),
            instructions: Arc::clone(&self.instructions),
            tools: Arc::clone(&self.tools),
        }
    }
}

impl<B: Backend> ForumServer<B> {
    /// A handler over `backend`, serving `instructions` (from `tools::instructions`), with
    /// `binding` shared with the engine, which drops a session's entries at its silence or
    /// retirement (CS-17).
    #[must_use]
    pub fn new(backend: Arc<B>, instructions: String, binding: Arc<Mutex<SessionBinding>>) -> Self {
        ForumServer {
            backend,
            binding,
            instructions: instructions.into(),
            tools: Arc::new(tools()),
        }
    }

    /// The handler serving `tools` instead of §20.2's: the usability test's arm without guidance
    /// serves 1.2's `session` description (feature `usability`, WP-U only).
    #[cfg(feature = "usability")]
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<McpTool>) -> Self {
        self.tools = Arc::new(tools);
        self
    }

    fn binding(&self) -> MutexGuard<'_, SessionBinding> {
        match self.binding.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The shell's part of one call: decode, resolve the session, ask the backend, bind.
    pub async fn handle(
        &self,
        caller: Caller,
        mcp_session: Option<String>,
        tool: &str,
        args: serde_json::Map<String, Value>,
    ) -> CallToolResult {
        let named = session_argument(&args);
        let names_session = args.contains_key("session");
        let hello =
            tool == "session" && args.get("action").and_then(Value::as_str) == Some("hello");
        let prepared = {
            let env = DecodeEnv {
                now: self.backend.now(),
                text_max: self.backend.text_max(),
                coverage_window: self.backend.coverage_window(),
                entries: self.backend.entries(),
            };
            prepare(tool, args, &env)
        };
        let prepared = match prepared {
            Ok(p) => p,
            Err(answer) => {
                let session = self
                    .binding()
                    .resolve(&caller.class, mcp_session.as_deref(), named);
                // 1.6 (§20.1, §21): a refusal of the arguments of a call that names an action
                // also names the session it lacks, so that one retry suffices.
                let answer = match answer {
                    ShellAnswer::Invalid(e)
                        if !hello
                            && !names_session
                            && !e.signature.is_empty()
                            && self
                                .binding()
                                .unbound(&caller.class, mcp_session.as_deref()) =>
                    {
                        ShellAnswer::Invalid(e.with_first(no_session()))
                    }
                    other => other,
                };
                let reply = self
                    .backend
                    .refuse(Refusal {
                        caller,
                        mcp_session,
                        session,
                        answer,
                    })
                    .await;
                return result_of(reply);
            }
        };
        // Parallel first calls of one MCP session open one forum session (review E2-bugs-9).
        let (session, opening) = loop {
            let claim =
                self.binding()
                    .claim(&caller.class, mcp_session.as_deref(), prepared.session);
            match claim {
                Claim::Ready(s) => break (s, None),
                Claim::Open(o) => {
                    let guard = OpeningGuard {
                        binding: Arc::clone(&self.binding),
                        opening: Some(o),
                    };
                    break (SessionCtx::New, Some(guard));
                }
                // The channel closes when the opening ends; claim again then.
                Claim::Wait(mut rx) => while rx.changed().await.is_ok() {},
            }
        };
        // 1.6 (§20.1): only `session hello` opens a session. Another call that names none, from
        // an MCP session bound to none, is refused and opens nothing; its opening, if it claimed
        // one, passes to the next call of its MCP session. Before 1.6 it opened a stray session
        // (l2, l3, l4).
        if session == SessionCtx::New && !hello {
            drop(opening);
            let reply = self
                .backend
                .refuse(Refusal {
                    caller,
                    mcp_session,
                    session,
                    answer: ShellAnswer::Invalid(no_session()),
                })
                .await;
            return result_of(reply);
        }
        let class = caller.class.clone();
        let reply = self
            .backend
            .call(Call {
                caller,
                mcp_session: mcp_session.clone(),
                session,
                prepared,
            })
            .await;
        if let (Some(m), Some(s)) = (&mcp_session, reply.session) {
            self.binding().bind(&class, m, s);
        }
        drop(opening);
        result_of(reply)
    }
}

/// Hands an opening back when its call ends, however it ends: answered, refused, or dropped
/// because the client cancelled it.
struct OpeningGuard {
    binding: Arc<Mutex<SessionBinding>>,
    opening: Option<Opening>,
}

impl Drop for OpeningGuard {
    fn drop(&mut self) {
        if let Some(o) = self.opening.take() {
            let mut b = match self.binding.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            b.opened(o);
        }
    }
}

/// The `session` argument of a call, when it decodes, whatever else does not.
fn session_argument(args: &serde_json::Map<String, Value>) -> Option<SessionId> {
    args.get("session")
        .and_then(Value::as_str)
        .and_then(SessionId::parse)
}

/// The `tools/list` result for a client on protocol version `version` (D.11.2, D.14.7). Version
/// `2026-07-28` requires every list result to carry the cache hints `ttlMs` and `cacheScope`
/// (SEP-2549), and a client that negotiated it drops a result without them, every tool with it
/// (Claude Code 2.1.287, the first live run, WP-U); rmcp 3.5 leaves them to a hand-written
/// handler. The tools never change while the server runs and are the same for every caller, so
/// they are `public`; `ttlMs` is 0, so a client asks again rather than keep a list across a
/// restart that changed it. Older versions do not know the fields, and get none, as rmcp's own
/// handlers do.
#[must_use]
pub fn list_tools_result(
    tools: Vec<McpTool>,
    version: Option<&ProtocolVersion>,
) -> ListToolsResult {
    let result = ListToolsResult::with_all_items(tools);
    if version.is_some_and(|v| *v >= ProtocolVersion::V_2026_07_28) {
        result.with_ttl_ms(0).with_cache_scope(CacheScope::Public)
    } else {
        result
    }
}

fn result_of(reply: Reply) -> CallToolResult {
    let mut result = if reply.is_error {
        CallToolResult::error(vec![ContentBlock::text(reply.text)])
    } else {
        CallToolResult::success(vec![ContentBlock::text(reply.text)])
    };
    result.structured_content = reply.structured;
    result
}

impl<B: Backend> ServerHandler for ForumServer<B> {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(self.instructions.to_string());
        info.server_info = Implementation::new("swarm-forum", env!("CARGO_PKG_VERSION"));
        info
    }

    fn get_tool(&self, name: &str) -> Option<McpTool> {
        self.tools.iter().find(|t| t.name == name).cloned()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(list_tools_result(
            self.tools.as_ref().clone(),
            context.protocol_version().as_ref(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let parts = context.extensions.get::<Parts>();
        let Some(caller) = parts.and_then(|p| p.extensions.get::<Caller>()).cloned() else {
            // The bearer layer answers 401 first; a call without a caller never reaches here.
            return Err(McpError::invalid_request("no authenticated caller", None));
        };
        let mcp_session = parts
            .and_then(|p| p.headers.get(MCP_SESSION_ID))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let args = request.arguments.unwrap_or_default();
        // rmcp answers a client's `notifications/cancelled` only by cancelling this token: the
        // call is dropped with it, and with it the engine's reply channel, so the writer skips the
        // job if it is still queued, and applies nothing (§19, D.11.2, review E2-bugs-2).
        tokio::select! {
            r = self.handle(caller, mcp_session, &request.name, args) => Ok(r.into()),
            () = context.ct.cancelled() => {
                Err(McpError::internal_error("the client cancelled the call", None))
            }
        }
    }
}

/// The HTTP settings of the route.
#[derive(Clone, Debug)]
pub struct HttpSettings {
    /// rmcp's `Host` validation (D.11.9).
    pub allowed_hosts: Vec<String>,
    /// An idle MCP session expires after `presence.silence` (CS-17).
    pub mcp_idle: Duration,
}

/// Stops the route's streams: rmcp ends every SSE stream of the route, the standalone `GET`
/// streams included, and refuses new requests, so that a server's graceful drain ends instead of
/// waiting on a client that holds a stream open (D.11.8; review E4-spec-2).
#[derive(Clone)]
pub struct StopStreams(Arc<dyn Fn() + Send + Sync>);

impl StopStreams {
    /// Stops them.
    pub fn stop(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for StopStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StopStreams")
    }
}

/// The `/mcp` route: the handler behind the bearer layer, with the 1 MiB body cap.
pub fn router<B: Backend>(
    server: ForumServer<B>,
    tokens: SharedTokens,
    http: &HttpSettings,
) -> Router {
    router_with_stop(server, tokens, http).0
}

/// [`router`], with the handle that stops its streams (the server's stop, D.11.8).
pub fn router_with_stop<B: Backend>(
    server: ForumServer<B>,
    tokens: SharedTokens,
    http: &HttpSettings,
) -> (Router, StopStreams) {
    let mut config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(http.allowed_hosts.clone())
        .with_legacy_session_mode(true)
        .with_max_request_body_bytes(MAX_BODY_BYTES);
    config.sse_keep_alive = Some(Duration::from_secs(15));
    let token = config.cancellation_token.clone();
    let stop = StopStreams(Arc::new(move || token.cancel()));
    let mut sessions = LocalSessionManager::default();
    sessions.session_config.keep_alive = Some(http.mcp_idle);
    let service: StreamableHttpService<ForumServer<B>, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(server.clone()), Arc::new(sessions), config);
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(tokens, bearer));
    (router, stop)
}

/// A minimal HTTP/1.1 client for the tests: no test dependency speaks raw HTTP.
#[cfg(test)]
pub(crate) mod test_http {
    use std::net::SocketAddr;

    use axum::Router;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// A response.
    #[derive(Debug)]
    pub(crate) struct Response {
        pub(crate) status: u16,
        pub(crate) body: String,
    }

    /// Serves `app` on a free local port.
    pub(crate) async fn serve(app: Router) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    /// Sends one request and reads the whole response (`Connection: close`).
    pub(crate) async fn request(
        addr: SocketAddr,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Response {
        let mut s = TcpStream::connect(addr).await.unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).await.unwrap();
        // A server may answer before reading a large body; ignore a write the peer refused.
        let _ = s.write_all(body).await;
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out).await;
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        Response { status, body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{TokenMap, hex, sha256};
    use crate::decode::EntryLookup;
    use crate::mcp::tools::INSTRUCTIONS;
    use forum_core::{Cmd, Eid, HoldCmd, NodeId};
    use rmcp::ServiceExt;
    use rmcp::model::ClientConfig;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use serde_json::json;
    use std::net::SocketAddr;

    struct NoEntries;

    impl EntryLookup for NoEntries {
        fn entry_node(&self, _: Eid) -> Option<NodeId> {
            None
        }
        fn question(&self, _: Eid) -> Option<(NodeId, Option<SessionId>)> {
            None
        }
    }

    /// Records every call it gets, and answers as session s-7; records refusals too, and
    /// renders them as the default does.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<Call>>,
        refusals: Mutex<Vec<SessionCtx>>,
    }

    impl Backend for Fake {
        fn now(&self) -> Time {
            Time(1_790_812_800_000)
        }
        fn text_max(&self) -> u32 {
            64
        }
        fn coverage_window(&self) -> Dur {
            Dur(28 * 86_400_000)
        }
        fn entries(&self) -> &(dyn EntryLookup + Sync) {
            &NoEntries
        }
        fn call(&self, call: Call) -> impl Future<Output = Reply> + Send {
            let text = format!("[swarm] running · you={}/s-7\nok", call.caller.class);
            self.calls.lock().unwrap().push(call);
            std::future::ready(Reply {
                text,
                structured: Some(json!({"ok": true})),
                is_error: false,
                session: Some(SessionId(7)),
            })
        }
        fn refuse(&self, refusal: Refusal) -> impl Future<Output = Reply> + Send {
            self.refusals.lock().unwrap().push(refusal.session);
            std::future::ready(Reply {
                text: refusal.answer.render(),
                structured: None,
                is_error: true,
                session: None,
            })
        }
    }

    /// Answers each call after `delay`, in the session it names, or in a new one, s-1, s-2, …;
    /// records the sessions it was asked for, and counts the calls it finished and those dropped
    /// before they finished.
    struct Slow {
        delay: Duration,
        seen: Mutex<Vec<SessionCtx>>,
        opened: std::sync::atomic::AtomicU64,
        finished: std::sync::atomic::AtomicUsize,
        dropped: std::sync::atomic::AtomicUsize,
    }

    impl Slow {
        fn new(delay: Duration) -> Slow {
            Slow {
                delay,
                seen: Mutex::new(Vec::new()),
                opened: std::sync::atomic::AtomicU64::new(0),
                finished: std::sync::atomic::AtomicUsize::new(0),
                dropped: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn counts(&self) -> (usize, usize) {
            use std::sync::atomic::Ordering::SeqCst;
            (self.finished.load(SeqCst), self.dropped.load(SeqCst))
        }
    }

    /// Counts the call's end: finished, or dropped before it finished.
    struct End<'a> {
        slow: &'a Slow,
        finished: bool,
    }

    impl Drop for End<'_> {
        fn drop(&mut self) {
            let n = if self.finished {
                &self.slow.finished
            } else {
                &self.slow.dropped
            };
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl Backend for Slow {
        fn now(&self) -> Time {
            Time(1_790_812_800_000)
        }
        fn text_max(&self) -> u32 {
            64
        }
        fn coverage_window(&self) -> Dur {
            Dur(28 * 86_400_000)
        }
        fn entries(&self) -> &(dyn EntryLookup + Sync) {
            &NoEntries
        }
        fn call(&self, call: Call) -> impl Future<Output = Reply> + Send {
            self.seen.lock().unwrap().push(call.session);
            let session = match call.session {
                SessionCtx::Existing(s) => s,
                SessionCtx::New | SessionCtx::System => SessionId(
                    self.opened
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        + 1,
                ),
            };
            async move {
                let mut end = End {
                    slow: self,
                    finished: false,
                };
                tokio::time::sleep(self.delay).await;
                end.finished = true;
                Reply {
                    text: format!(
                        "[swarm] running · you=coding-agent/{}\nok",
                        session.render()
                    ),
                    structured: None,
                    is_error: false,
                    session: Some(session),
                }
            }
        }
    }

    const TOKEN: &str = "fsk_test-token-for-the-mcp-route-0123456789abcdef";

    fn tokens() -> SharedTokens {
        SharedTokens::new(
            TokenMap::load(
                vec![(
                    "FORUM_TOKEN_CI".to_string(),
                    format!(
                        "{};class=coding-agent;caps=rust",
                        hex(&sha256(TOKEN.as_bytes()))
                    ),
                )],
                None,
            )
            .unwrap(),
        )
    }

    /// Serves `backend` on a free local port, with `tokens`.
    async fn serve<B: Backend>(
        backend: &Arc<B>,
        binding: &Arc<Mutex<SessionBinding>>,
        tokens: SharedTokens,
    ) -> SocketAddr {
        let server = ForumServer::new(
            Arc::clone(backend),
            INSTRUCTIONS.to_string(),
            Arc::clone(binding),
        );
        let http = HttpSettings {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            mcp_idle: Duration::from_secs(900),
        };
        test_http::serve(router(server, tokens, &http)).await
    }

    async fn start() -> (Arc<Fake>, Arc<Mutex<SessionBinding>>, SocketAddr) {
        let backend = Arc::new(Fake::default());
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let addr = serve(&backend, &binding, tokens()).await;
        (backend, binding, addr)
    }

    type Client = rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>;

    async fn connect(addr: SocketAddr) -> Client {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
                .auth_header(TOKEN),
        );
        ClientConfig::default().serve(transport).await.unwrap()
    }

    /// Waits until `f` holds, at most 10 s.
    async fn until(what: &str, f: impl Fn() -> bool) {
        for _ in 0..1_000 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn call(name: &str, v: serde_json::Value) -> CallToolRequestParams {
        let serde_json::Value::Object(args) = v else {
            panic!("an object");
        };
        CallToolRequestParams::new(name.to_string()).with_arguments(args)
    }

    fn text(r: &CallToolResult) -> String {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn the_route_serves_the_tools_and_hands_decoded_calls_to_the_backend() {
        let (backend, binding, addr) = start().await;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
                .auth_header(TOKEN),
        );
        let client = ClientConfig::default().serve(transport).await.unwrap();
        let info = client.peer_info().unwrap();
        assert_eq!(info.instructions.as_deref(), Some(INSTRUCTIONS));
        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(
            tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>(),
            ["session", "view", "node", "hold", "talk", "control"]
        );
        assert_eq!(
            tools[4].description.as_deref(),
            Some(crate::mcp::tools::TALK)
        );

        // A call that does not decode reaches nothing: no status line, no backend call. Its
        // refusal names every problem, the session it lacks among them (1.6, §20.1, §21).
        let r = client
            .call_tool(call("hold", json!({"action": "take"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            text(&r),
            "error: invalid\nhint: session: missing: no session is bound to this connection: pass session: the s-N \
             your hello returned, shown in every status line as session s-N; a new agent \
             starts with session hello\ntarget: missing: hold take requires it; a \
             path or #<id>, as hello's offers, view find, and view list show them\nhold take takes \
             {target*, lease}"
        );
        assert!(backend.calls.lock().unwrap().is_empty());
        // Nor does a text over text.max (64 bytes here).
        let r = client
            .call_tool(call(
                "hold",
                json!({"action": "checkpoint", "token": "1.1", "done": "d".repeat(65), "next_step": "n"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).starts_with("error: limit\nhint: a free-text argument has 65 bytes"));
        assert!(backend.calls.lock().unwrap().is_empty());
        // 1.6 (§20.1): a call other than hello that names no session, from an MCP session bound
        // to none, is refused and opens nothing.
        let r = client
            .call_tool(call("hold", json!({"action": "next", "op_id": "op-1"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            text(&r),
            "error: invalid\nhint: session: missing: no session is bound to this connection: pass session: the s-N \
             your hello returned, shown in every status line as session s-N; a new agent \
             starts with session hello"
        );
        assert!(backend.calls.lock().unwrap().is_empty());
        assert!(binding.lock().unwrap().is_empty());

        // A hello opens a session, and its MCP session is bound to it.
        let r = client
            .call_tool(call("session", json!({"action": "hello", "op_id": "op-1"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        assert_eq!(text(&r), "[swarm] running · you=coding-agent/s-7\nok");
        assert_eq!(r.structured_content, Some(json!({"ok": true})));
        let r = client
            .call_tool(call("hold", json!({"action": "next"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        let r = client
            .call_tool(call("hold", json!({"action": "next", "session": "s-3"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        {
            let calls = backend.calls.lock().unwrap();
            assert_eq!(calls.len(), 3);
            let first = &calls[0];
            assert_eq!(first.caller.class.as_str(), "coding-agent");
            assert_eq!(
                first.caller.caps.as_ref().map(|c| c.len()),
                Some(1),
                "the token's caps reach the call (CS-26)"
            );
            assert_eq!(first.session, SessionCtx::New);
            assert!(first.mcp_session.is_some());
            assert!(first.prepared.digest.is_some());
            assert!(matches!(
                first.prepared.cmd,
                Cmd::Session(forum_core::SessionCmd::Hello { .. })
            ));
            assert_eq!(
                calls[1].session,
                SessionCtx::Existing(SessionId(7)),
                "bound"
            );
            assert_eq!(
                calls[1].prepared.cmd,
                Cmd::Hold(HoldCmd::Next {
                    query: forum_core::NextQuery::default(),
                    lease: None
                })
            );
            assert_eq!(
                calls[2].session,
                SessionCtx::Existing(SessionId(3)),
                "the argument wins"
            );
        }
        assert_eq!(binding.lock().unwrap().len(), 1);
        // The engine drops the binding at the session's silence; the next call names no session
        // and is refused, and a hello opens a new one.
        binding.lock().unwrap().drop_session(SessionId(7));
        let r = client
            .call_tool(call("view", json!({"action": "constitution"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert_eq!(backend.calls.lock().unwrap().len(), 3);
        client
            .call_tool(call("session", json!({"action": "hello"})))
            .await
            .unwrap();
        assert_eq!(backend.calls.lock().unwrap()[3].session, SessionCtx::New);
        // A refused call names its session by its argument, else by its binding (R27): the
        // engine keeps the status line for an existing one; with none, there is none.
        client
            .call_tool(call("hold", json!({"action": "take", "session": "s-9"})))
            .await
            .unwrap();
        client
            .call_tool(call("hold", json!({"action": "take", "session": "nine"})))
            .await
            .unwrap();
        assert_eq!(
            *backend.refusals.lock().unwrap(),
            [
                SessionCtx::New,
                SessionCtx::New,
                SessionCtx::New,
                SessionCtx::New,
                SessionCtx::Existing(SessionId(9)),
                SessionCtx::Existing(SessionId(7)),
            ],
            "the first four came with no binding; the last falls back to the binding"
        );
        assert_eq!(
            backend.calls.lock().unwrap().len(),
            4,
            "no refusal reached the engine"
        );
        client.cancel().await.unwrap();
    }

    /// D.14.7: a client that negotiates MCP `2026-07-28` (server/discover, no `initialize`, no
    /// MCP session) gets the six tools with the cache hints that version requires, `ttlMs` and
    /// `cacheScope`, and its calls reach the backend, with no MCP session to bind; a client on an
    /// older version gets the same tools without the hints. Claude Code 2.1.287 dropped every
    /// tool of a list without them (WP-U).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_2026_07_28_client_gets_the_tools_with_their_cache_hints() {
        use rmcp::model::{CacheScope, ProtocolVersion};
        use rmcp::{ClientLifecycleMode, ClientServiceExt};
        let (backend, binding, addr) = start().await;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
                .auth_header(TOKEN),
        );
        let client = ClientConfig::default()
            .serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .unwrap();
        assert_eq!(
            client.peer_info().map(|i| i.protocol_version.clone()),
            Some(ProtocolVersion::V_2026_07_28)
        );
        assert_eq!(
            client
                .peer_info()
                .and_then(|i| i.instructions.clone())
                .as_deref(),
            Some(INSTRUCTIONS)
        );
        let list = client.list_tools(None).await.unwrap();
        assert_eq!(list.ttl_ms, Some(0));
        assert_eq!(list.cache_scope, Some(CacheScope::Public));
        assert_eq!(
            list.tools
                .iter()
                .map(|t| t.name.as_ref())
                .collect::<Vec<_>>(),
            ["session", "view", "node", "hold", "talk", "control"]
        );
        for t in &list.tools {
            assert!(t.input_schema.get("oneOf").is_none());
            // 1.7 (§20.1): no output schema; structured content is still sent.
            assert!(t.output_schema.is_none());
        }
        // A hello: decoded, run, answered; there is no MCP session to bind, so the next call
        // names its session (A-32), and one that names none is refused and opens nothing (1.6,
        // §20.1: such clients opened a stray session at every call, l2, l3, l4).
        let r = client
            .call_tool(call("session", json!({"action": "hello"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        assert_eq!(text(&r), "[swarm] running · you=coding-agent/s-7\nok");
        assert_eq!(r.structured_content, Some(json!({"ok": true})));
        {
            let calls = backend.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert!(calls[0].mcp_session.is_none());
            assert_eq!(calls[0].session, SessionCtx::New);
        }
        assert!(binding.lock().unwrap().is_empty());
        let r = client
            .call_tool(call("hold", json!({"action": "next"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(
            text(&r).starts_with("error: invalid\nhint: session: missing: no session is bound"),
            "{}",
            text(&r)
        );
        assert_eq!(backend.calls.lock().unwrap().len(), 1);
        let r = client
            .call_tool(call("hold", json!({"action": "next", "session": "s-7"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        assert_eq!(
            backend.calls.lock().unwrap()[1].session,
            SessionCtx::Existing(SessionId(7))
        );
        client.cancel().await.unwrap();
        // An older client: the same tools, no cache hints.
        let legacy = connect(addr).await;
        assert_eq!(
            legacy.peer_info().map(|i| i.protocol_version.clone()),
            Some(ProtocolVersion::V_2025_11_25)
        );
        let list = legacy.list_tools(None).await.unwrap();
        assert_eq!((list.ttl_ms, list.cache_scope), (None, None));
        assert_eq!(list.tools.len(), 6);
        legacy.cancel().await.unwrap();
        // And the hints by version, directly.
        let hinted = |v: Option<&ProtocolVersion>| {
            let r = list_tools_result(Vec::new(), v);
            (r.ttl_ms, r.cache_scope)
        };
        assert_eq!(hinted(None), (None, None));
        assert_eq!(hinted(Some(&ProtocolVersion::V_2025_06_18)), (None, None));
        assert_eq!(
            hinted(Some(&ProtocolVersion::V_2026_07_28)),
            (Some(0), Some(CacheScope::Public))
        );
    }

    #[tokio::test]
    async fn an_unknown_token_gets_401_and_a_large_body_413() {
        let (backend, _, addr) = start().await;
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let headers = [
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ];
        let r = test_http::request(addr, "POST", "/mcp", &headers, body).await;
        assert_eq!(r.status, 401);
        let bad = [
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("authorization", "Bearer fsk_wrong"),
        ];
        assert_eq!(
            test_http::request(addr, "POST", "/mcp", &bad, body)
                .await
                .status,
            401
        );
        let auth = format!("Bearer {TOKEN}");
        let good = [
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("authorization", auth.as_str()),
        ];
        let mut big = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"talk","arguments":{"action":"say","at":"/a","text":""#.to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_BODY_BYTES));
        big.extend_from_slice(br#""}}}"#);
        let r = test_http::request(addr, "POST", "/mcp", &good, &big).await;
        assert_eq!(r.status, 413, "{}", r.body);
        assert!(backend.calls.lock().unwrap().is_empty());
    }

    /// §19: a queued call whose client has gone is skipped, and nothing is applied. rmcp only
    /// cancels the request's token when the client cancels; the handler drops the call with it,
    /// and so the engine's reply channel, which is what the writer's skip looks at (review
    /// E2-bugs-2).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_the_client_cancels_is_dropped() {
        use rmcp::model::{CallToolRequest, ClientRequest};
        use rmcp::service::{PeerRequestOptions, ServiceError};
        let backend = Arc::new(Slow::new(Duration::from_millis(800)));
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let addr = serve(&backend, &binding, tokens()).await;
        let client = connect(addr).await;
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(call(
            "hold",
            json!({"action": "next", "session": "s-1"}),
        )));
        let handle = client
            .peer()
            .send_request_with_option(
                request,
                PeerRequestOptions::with_timeout(Duration::from_millis(150)),
            )
            .await
            .unwrap();
        // On its timeout the client sends notifications/cancelled.
        let r = handle.await_response().await;
        assert!(matches!(r, Err(ServiceError::Timeout { .. })), "{r:?}");
        until("the backend's call to end", || backend.counts() != (0, 0)).await;
        assert_eq!(backend.counts(), (0, 1), "dropped, not finished");
        // Long after the call would have finished, it still has not.
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert_eq!(backend.counts(), (0, 1));
        // An uncancelled call finishes as before.
        let r = client
            .call_tool(call("hold", json!({"action": "next", "session": "s-1"})))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        assert_eq!(backend.counts(), (1, 1));
        client.cancel().await.unwrap();
    }

    /// Two first calls of one MCP session sent together open one forum session: the second waits
    /// for the first's reply, then runs in the session it opened (review E2-bugs-9). Only a hello
    /// opens one (1.6): a call that arrives while a hello opens the session runs in it, and one
    /// that arrives with no opening is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn parallel_first_calls_open_one_session() {
        let backend = Arc::new(Slow::new(Duration::from_millis(300)));
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let addr = serve(&backend, &binding, tokens()).await;
        let client = connect(addr).await;
        let (a, b) = tokio::join!(
            client.call_tool(call("session", json!({"action": "hello"}))),
            client.call_tool(call("session", json!({"action": "hello"}))),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(text(&a), "[swarm] running · you=coding-agent/s-1\nok");
        assert_eq!(text(&b), "[swarm] running · you=coding-agent/s-1\nok");
        assert_eq!(
            *backend.seen.lock().unwrap(),
            [SessionCtx::New, SessionCtx::Existing(SessionId(1))]
        );
        assert_eq!(binding.lock().unwrap().len(), 1);
        // A call that names its session never waits for the opening of another.
        let (a, b) = tokio::join!(
            client.call_tool(call("hold", json!({"action": "next", "session": "s-9"}))),
            client.call_tool(call("hold", json!({"action": "next", "session": "s-8"}))),
        );
        assert!(text(&a.unwrap()).contains("s-9"));
        assert!(text(&b.unwrap()).contains("s-8"));
        client.cancel().await.unwrap();
        // A call that names no session while a hello of its MCP session opens one waits for it,
        // and runs in it.
        let other = connect(addr).await;
        let (a, b) = tokio::join!(
            other.call_tool(call("session", json!({"action": "hello"}))),
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                other
                    .call_tool(call("hold", json!({"action": "next"})))
                    .await
            },
        );
        assert_eq!(
            text(&a.unwrap()),
            "[swarm] running · you=coding-agent/s-2\nok"
        );
        assert_eq!(
            text(&b.unwrap()),
            "[swarm] running · you=coding-agent/s-2\nok"
        );
        other.cancel().await.unwrap();
    }

    /// The engine behind the route, over a new database with a manual clock.
    fn engine(
        tag: &str,
        tokens: &SharedTokens,
    ) -> (Arc<crate::engine::Engine>, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("forum-server-mcp-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut o =
            crate::engine::Options::new(dir.join("forum.db"), None, forum_core::Params::default());
        o.clock = Arc::new(crate::clock::ManualClock::new(Time(1_790_845_200_000)));
        o.on_fatal = crate::engine::OnFatal::StopWriter;
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let e = crate::engine::Engine::start(o, tokens.clone(), binding).unwrap();
        (e, dir)
    }

    /// §20.1 and §21: a missing entry is not one of the answers that reach nothing else. A call
    /// that names one gets the core's `not_found` with its session's status line, and a first
    /// hello that names one (1.6: `from_user {answers}`) still opens its session, with the status
    /// line and `session_opened`; a call that names an unknown session gets the session's
    /// `not_found`, which comes first (review E2-spec-3).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_naming_a_missing_entry_gets_not_found_in_its_session() {
        let tokens = tokens();
        let (engine, dir) = engine("missing-entry", &tokens);
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let addr = serve(&engine, &binding, tokens).await;
        let client = connect(addr).await;
        let r = client
            .call_tool(call(
                "view",
                json!({"action": "node", "path": "/", "session": "s-99"}),
            ))
            .await
            .unwrap();
        assert!(text(&r).starts_with("error: not_found"), "{}", text(&r));
        let r = client
            .call_tool(call(
                "node",
                json!({"action": "hide", "entry": "e999", "reason": "r", "session": "s-99"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(
            text(&r).contains("unknown or retired session") && !text(&r).contains("no such entry"),
            "the session's not_found comes first: {}",
            text(&r)
        );
        assert!(binding.lock().unwrap().is_empty());
        // A first hello whose words answer a question that does not exist: refused, but its
        // session is opened and bound (1.5, §20.1).
        let r = client
            .call_tool(call(
                "session",
                json!({"action": "hello", "from_user": {"text": "yes", "client": "ide",
                       "answers": "e999"}}),
            ))
            .await
            .unwrap();
        let t = text(&r);
        assert_eq!(r.is_error, Some(true));
        assert!(
            t.starts_with("[swarm] running") && t.contains("session s-1 (coding-agent)"),
            "{t}"
        );
        assert!(t.contains("error: not_found"), "{t}");
        assert!(t.contains("session_opened"), "{t}");
        assert_eq!(
            binding.lock().unwrap().len(),
            1,
            "the session opened is bound"
        );
        let r = client
            .call_tool(call(
                "node",
                json!({"action": "hide", "entry": "e999", "reason": "r", "op_id": "op-hide"}),
            ))
            .await
            .unwrap();
        let t = text(&r);
        assert_eq!(r.is_error, Some(true));
        assert!(
            t.starts_with("[swarm] running") && t.contains("session s-1 (coding-agent)"),
            "{t}"
        );
        assert!(t.contains("error: not_found\nhint: no such entry"), "{t}");
        // The retry gets the stored response (O1), still in that session.
        let again = client
            .call_tool(call(
                "node",
                json!({"action": "hide", "entry": "e999", "reason": "r", "op_id": "op-hide"}),
            ))
            .await
            .unwrap();
        assert_eq!(text(&again), t);
        client.cancel().await.unwrap();
        engine.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The same for `talk retract {entry}` and `talk answer {question}` (D.14.6): the core refuses
    /// an entry the shell's table lacks with `not_found`, as `node hide` does, since the shell no
    /// longer refuses it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_retract_or_answer_naming_a_missing_entry_gets_not_found_in_its_session() {
        let tokens = tokens();
        let (engine, dir) = engine("missing-entry-talk", &tokens);
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let addr = serve(&engine, &binding, tokens).await;
        for (i, (args, hint)) in [
            (
                json!({"action": "retract", "entry": "e999", "text": "t", "reason": "r"}),
                "no such entry",
            ),
            (
                json!({"action": "answer", "question": "e999", "text": "yes"}),
                "no such question",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let client = connect(addr).await;
            client
                .call_tool(call("session", json!({"action": "hello"})))
                .await
                .unwrap();
            let r = client.call_tool(call("talk", args)).await.unwrap();
            let t = text(&r);
            assert!(
                t.starts_with("[swarm] running") && t.contains(&format!("session s-{} (", i + 1)),
                "{t}"
            );
            assert!(
                t.contains(&format!("error: not_found\nhint: {hint}")),
                "{t}"
            );
            client.cancel().await.unwrap();
        }
        engine.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}
