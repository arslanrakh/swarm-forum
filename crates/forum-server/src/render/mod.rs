//! Rendering within budgets (design D.11.7; WP-E3): every tool result as MCP text content and
//! `structuredContent`.
//!
//! The text is, in order: the status line (§20.1); the `hello` when the call opened its session
//! (a `session hello` shows it as its body); `STALE SESSION — re-read before writing` when the
//! session was silent; the compact JSON body, or `error: <code>` and `hint: <text>`; `warnings:
//! [...]`; and, last, the next calls (§20.9). A refusal sets `isError`. A refusal of a call that
//! names no existing session of its class has no status line, and starts with `error: <code>`
//! (1.4, §20.1, R27). A failed first call that opened its session carries the `session_opened`
//! warning, which says that nothing else in the call was applied (1.5, §20.1, §21, R3).
//!
//! `structuredContent` carries everything the text does (1.5, §20.1), since some clients show
//! their model only the structured content, and read it from the top; its keys come in §20.1's
//! order (1.5, R3), [`ENVELOPE_KEYS`]: `status` (the status line, absent without a session),
//! `stale_session`, `error` and `hint`, `warnings`, `next_calls`, `hello` when the call opened its
//! session, the `notes` a write delivered (1.7, §13), and then the body's fields (or the body as
//! `body` when one of its fields has an envelope key's name). The order reaches the client because
//! `serde_json` keeps insertion order (its feature `preserve_order`). No tool has an output schema
//! (1.7, §20.1): the `session` description says what `?` means in a next call.
//!
//! Modules:
//! - `status`: the status line, with `now` and each hold's `checkpoint by` (1.4, R8);
//! - `hello`: `hello` within `hello.budget`, the search hits, and an empty `next`'s `why`;
//! - `brief`: the brief within `brief.budget`, and the full and review views;
//! - `lists`: `view list`, `feed`, `history`, `requests`, `user_log`, `find`, and `talk who`;
//! - `events`: each event of `view history` in the shell's formats, hidden content masked;
//! - `errors`: errors with their hints (R5's key hint among them) and warnings;
//! - `next_calls`: the next calls, with one `why` template per `Because` and the `fill` of each
//!   `Skeleton` (1.3);
//! - `body`, `entries`, `format`, `json`: replies, entries, value formats, and ordered JSON;
//! - `source`: the texts and pages the shell's tables hold (D.11.6);
//! - `usability` (feature `usability`, WP-U only): the arm without guidance.

pub mod body;
pub mod brief;
pub mod entries;
pub mod errors;
pub mod events;
pub mod format;
pub mod hello;
pub mod json;
pub mod lists;
pub mod next_calls;
pub mod seen;
pub mod source;
pub mod status;
#[cfg(feature = "usability")]
pub mod usability;

#[cfg(test)]
mod tests;

use forum_core::health::Health;
use forum_core::query::guide::Guidance;
use forum_core::{
    Cmd, Done, Error, Hint, LimitKind, Reply, SessionCmd, SessionId, State, Time, Warning,
    WarningCode, WarningDetail, What, Why,
};
use serde_json::Value;

pub use hello::{Hits, SearchHits};
use json::Json;
pub use seen::{Seen, SeenBySession};
pub use source::{EntryView, MemorySource, Source, StoreSource};

use crate::decode::{NodeArgText, ShellAnswer};

/// One rendered tool result.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// The text content.
    pub text: String,
    /// `structuredContent`.
    pub structured: Value,
    /// Whether the call was refused (`isError`).
    pub is_error: bool,
    /// What the response showed its session, for the shell's memory (1.7, §20.1): None without a
    /// session.
    pub seen: Option<Seen>,
}

/// The details of the `ignored_arguments` warning (1.7, §21), as `decode::Prepared` gives them:
/// for a call that was applied, and for one the core refused, where the arguments would have been
/// ignored (1.8).
#[derive(Clone, Copy, Debug)]
pub struct IgnoredDetail<'a> {
    /// "…, so the call was applied without it".
    pub applied: &'a str,
    /// "…, so it would be ignored: leave it out when you retry".
    pub refused: &'a str,
}

/// 1.12 (§20.1, §21): a call refused as `not_found` for the session it named, from an MCP session
/// bound to another live session of its class. Its response names that session, and its next call
/// is the same call in it, not `session hello`, which would open a second session. In the lab's
/// seventh round (it6 l3), a call sent beside `hello` guessed `s-1`, and the refusal sent the agent
/// to open a session while its connection's was `s-23`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebind {
    /// The session the MCP session is bound to.
    pub session: SessionId,
    /// The call's tool.
    pub tool: &'static str,
    /// The call's action.
    pub action: &'static str,
    /// The arguments the call gave (`decode::Prepared::given`), which the agent writes again.
    pub given: Vec<&'static str>,
}

/// The hint of a refusal that [`Rebind`] covers: the session named, and the connection's. When the
/// refused call was itself a `session hello` that resumes a session (1.8), the same call resumes the
/// connection's and opens none, so the hint does not warn against it.
fn rebind_hint(named: Option<SessionId>, rb: &Rebind) -> String {
    let named = named.map_or_else(|| "the session you named".to_string(), |x| x.render());
    let not_hello = if (rb.tool, rb.action) == ("session", "hello") {
        ""
    } else {
        ", not session hello"
    };
    format!(
        "{named} is unknown or retired; this connection's session is {0}: make the same call with \
         session: {0}{not_hello}",
        rb.session.render()
    )
}

/// The next calls of a refusal that [`Rebind`] covers: the same call, in the connection's session,
/// its arguments to write again cut to keep the call within `next_calls::CALL_MAX` (§20.9).
fn rebind_block(rb: &Rebind) -> (String, Json) {
    let mut c = next_calls::Call {
        tool: rb.tool,
        action: rb.action,
        args: Json::obj()
            .with("action", Json::s(rb.action))
            .with("session", Json::S(rb.session.render())),
        fill: rb.given.clone(),
    };
    while next_calls::call_text(&c).len() > next_calls::CALL_MAX && c.fill.pop().is_some() {}
    let why = format!(
        "this connection's session is {}: the same call in it",
        rb.session.render()
    );
    let text = format!(
        "next_calls (1 of 1):\n1. {} — {why}",
        next_calls::call_text(&c)
    );
    let item = Json::obj()
        .with("tool", Json::s(c.tool))
        .with("args", c.args.clone())
        .with("fill", Json::strs(c.fill.iter().copied()))
        .with("when", Json::s("now"))
        .with("why", Json::S(why))
        .with("rule", Json::s("remedy"));
    let json = Json::obj()
        .with("items", Json::A(vec![item]))
        .with("total", Json::from(1_u32));
    (text, json)
}

/// What a response renders from: the state after the call and its post-settle (T3), the call, its
/// outcome, and its next calls, with the shell's own inputs.
pub struct Input<'a> {
    /// The state after the call and its post-settle.
    pub state: &'a State,
    /// The call's clamped `now` (T1).
    pub now: Time,
    /// The call's session, opened by it if new; None when the call failed at the session check
    /// (`not_found(Session)`, `invalid(ClassMismatch)`), which renders no status line.
    pub session: Option<SessionId>,
    /// The session this call opened: `Done::opened`, or, for a refused first call, the session its
    /// `SessionOpened` event opened (§20.1: it still gets the `hello`).
    pub opened: Option<SessionId>,
    /// The decoded call.
    pub cmd: &'a Cmd,
    /// The raw text of each node argument (R5).
    pub node_args: &'a [NodeArgText],
    /// The outcome.
    pub out: &'a Result<Done, Error>,
    /// The next calls (`query::guide`); None renders no block: the usability switch of WP-U
    /// (feature `usability`) only. Every response the core computes has them (§20.1), except a
    /// refusal by `write.per_hour`, busy, or a text over `text.max`, which carries none whatever
    /// this holds (§20.1, §21, D.11.7).
    pub guidance: Option<&'a Guidance>,
    /// The writer's health cache (D.11.2).
    pub health: &'a Health,
    /// When the cache was computed.
    pub health_at: Time,
    /// The search hits the handler found (D.11.12).
    pub hits: &'a SearchHits,
    /// How many free texts masking changed: the `secret_masked` warning the shell adds.
    pub masked_fields: u32,
    /// The details of the `ignored_arguments` warning the shell adds, when the decoder dropped an
    /// argument of another action (1.7, §21): one for a call that was applied, one for a refusal
    /// (1.8).
    pub ignored: Option<IgnoredDetail<'a>>,
    /// The shell's tables.
    pub src: &'a dyn Source,
    /// What the session's earlier responses showed it (1.7, §20.1): None for nothing yet.
    pub seen: Option<&'a Seen>,
    /// 1.12 (§20.1): the connection's own session, for a call refused as `not_found` for the
    /// session it named; None otherwise.
    pub rebind: Option<Rebind>,
}

/// Whether a refusal failed at the session check: it reaches no session, so it has no status line
/// (§20.1, 1.4, R27).
fn session_refused(out: &Result<Done, Error>) -> bool {
    matches!(
        out,
        Err(Error {
            hint: Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch),
            ..
        })
    )
}

/// Whether a refusal carries no next calls: `write.per_hour`, busy, and a text over `text.max`
/// (§20.1, §21: "The answers that reach nothing else, and a refusal by `write.per_hour`, carry
/// none").
fn carries_no_next_calls(out: &Result<Done, Error>) -> bool {
    matches!(
        out,
        Err(Error {
            hint: Hint::Limit(LimitKind::WritesPerHour | LimitKind::Busy | LimitKind::TextTooLong),
            ..
        })
    )
}

/// The keys of `structuredContent` beside the body's own fields, in §20.1's order, which the
/// structured content keeps (1.5, R3, D.11.7): the body's fields follow them.
pub const ENVELOPE_KEYS: [&str; 9] = [
    "status",
    "stale_session",
    "error",
    "hint",
    "warnings",
    "next_calls",
    "hello",
    "notes",
    "body",
];

/// A body's place in `structuredContent`: its fields after the envelope's (1.5, §20.1, D.11.7),
/// or, when one of them has a name the envelope uses, the whole body as `body`, so that neither
/// hides the other. Two bodies do: `hold next`'s status item, `{status: <node>, …}` (§10.4), and
/// `view node {view: review}`, whose content has the node's `status`.
fn body_fields(body: Json) -> Vec<(String, Json)> {
    match body {
        Json::O(fields)
            if fields
                .iter()
                .all(|(k, _)| !ENVELOPE_KEYS.contains(&k.as_str())) =>
        {
            fields
        }
        other => vec![("body".to_string(), other)],
    }
}

/// The parts of `structuredContent`, which [`Envelope::assemble`] puts in §20.1's order (1.5, R3).
#[derive(Default)]
struct Envelope {
    status: Option<String>,
    stale_session: bool,
    error: Option<(&'static str, String)>,
    warnings: Option<Json>,
    next_calls: Option<Json>,
    hello: Option<Json>,
    /// 1.7 (§13): the notes a write delivered.
    notes: Option<Json>,
    body: Vec<(String, Json)>,
}

impl Envelope {
    /// `structuredContent`: `status`, `stale_session`, `error` and `hint`, `warnings`,
    /// `next_calls`, `hello`, then the body's fields (§20.1).
    fn assemble(self) -> Json {
        let mut j = Json::obj();
        if let Some(line) = self.status {
            j.put("status", Json::S(line));
        }
        if self.stale_session {
            j.put("stale_session", Json::Bool(true));
        }
        if let Some((code, hint)) = self.error {
            j.put("error", Json::s(code));
            j.put("hint", Json::S(hint));
        }
        if let Some(w) = self.warnings {
            j.put("warnings", w);
        }
        if let Some(n) = self.next_calls {
            j.put("next_calls", n);
        }
        if let Some(h) = self.hello {
            j.put("hello", h);
        }
        if let Some(n) = self.notes {
            j.put("notes", n);
        }
        for (k, v) in self.body {
            j.put(&k, v);
        }
        j
    }
}

/// Renders one response.
#[must_use]
pub fn response(i: &Input<'_>) -> Response {
    let s = i.state;
    let mut lines: Vec<String> = Vec::new();
    let mut structured = Envelope::default();
    let session = if session_refused(i.out) {
        None
    } else {
        i.session.filter(|x| s.sessions.contains_key(x))
    };
    let mut seen = i.seen.cloned().unwrap_or_default();
    // 1.11 (§17): the health shown to the call's session, on its status line and in an empty
    // `why`, counts that session's caps and horizon; `control health` answers from the cache.
    let opened = i.opened.filter(|x| s.sessions.contains_key(x));
    let shown_health = status::health_of(s, session.or(opened), i.health);
    if let Some(x) = session {
        let (line, health) = status::line(s, x, i.now, &shown_health, i.src, seen.health.as_ref());
        seen.health = Some(health);
        structured.status = Some(line.clone());
        lines.push(line);
    }
    let hello_is_body = matches!(
        (i.cmd, i.out),
        (
            Cmd::Session(SessionCmd::Hello { .. }),
            Ok(Done {
                reply: Reply::Hello { .. },
                ..
            })
        )
    );
    if let Some(x) = opened
        && !hello_is_body
    {
        let h = hello::render_hello(
            s,
            x,
            i.now,
            &shown_health,
            &hello::HelloInput {
                delivered: None,
                user_entry: None,
                hits: i.hits,
                route: &hello::RouteCall::default(),
                first: true,
            },
            i.src,
        );
        lines.push(format!("hello: {}", h.text()));
        structured.hello = Some(h);
    }
    let ctx = body::BodyCtx {
        state: s,
        now: i.now,
        session,
        cmd: i.cmd,
        health: i.health,
        shown_health: &shown_health,
        health_at: i.health_at,
        hits: i.hits,
        src: i.src,
        opened: opened.is_some(),
    };
    let is_error = match i.out {
        Ok(done) => {
            if done.stale_session {
                lines.push(status::STALE_SESSION.to_string());
                structured.stale_session = true;
            }
            // `control export` pages the history as JSON lines after its header (D.11.15).
            let export = match &done.reply {
                Reply::Export { after, limit } => i
                    .src
                    .export(*after, *limit)
                    .map(|p| (crate::export::body(&p, *limit), p)),
                _ => None,
            };
            // 1.7 (§13): the notes this write delivered, before the body. 1.8: an open agent
            // question among them says that its answer may wait until the agent knows it.
            if !done.delivered.is_empty() {
                let notes = Json::A(
                    done.delivered
                        .iter()
                        .map(|e| {
                            let j = hello::inbox_item(s, *e, i.src, None);
                            match hello::answer_may_wait(s, *e, session) {
                                Some(a) => j.with("answer", Json::S(a)),
                                None => j,
                            }
                        })
                        .collect(),
                );
                lines.push(format!("notes: {}", notes.text()));
                structured.notes = Some(notes);
            }
            let b = match &export {
                Some(((head, _), _)) => head.clone(),
                None => body::body(&ctx, &done.reply),
            };
            lines.push(b.text());
            structured.body = body_fields(b);
            if let Some(((_, text_lines), page)) = export {
                lines.extend(text_lines);
                structured.body.push((
                    "lines".to_string(),
                    Json::A(page.lines.iter().map(crate::export::to_json).collect()),
                ));
            }
            let mut ws: Vec<Warning> = done.warnings.clone();
            if i.masked_fields > 0 {
                ws.push(Warning {
                    code: WarningCode::SecretMasked,
                    id: None,
                    detail: WarningDetail::SecretMasked {
                        fields: i.masked_fields,
                    },
                });
            }
            // 1.12 (§21): a `to_user`'s warnings read its addressee.
            let addressee = match &done.reply {
                Reply::ToUser { principal, .. } => Some(*principal),
                _ => None,
            };
            let mut list = match errors::warnings_line(
                s,
                i.src,
                session,
                &ws,
                &mut seen.agreements,
                addressee,
            ) {
                Some((_, Json::A(items))) => items,
                _ => Vec::new(),
            };
            list.extend(i.ignored.map(|d| errors::ignored_arguments(d.applied)));
            if !list.is_empty() {
                let list = Json::A(list);
                lines.push(format!("warnings: {}", list.text()));
                structured.warnings = Some(list);
            }
            false
        }
        Err(e) => {
            let hint = match (&i.rebind, &e.hint) {
                (Some(rb), Hint::NotFound(What::Session)) => rebind_hint(i.session, rb),
                _ => errors::hint(s, e, i.cmd, i.node_args, i.src, opened.is_some()),
            };
            lines.push(format!(
                "error: {}\nhint: {hint}",
                errors::code_name(e.code)
            ));
            structured.error = Some((errors::code_name(e.code), hint));
            // 1.5 (§20.1, §21, R3): a failed first call that opened its session says so, and, 1.10,
            // what it applied: the opening, with hello's intent, caps, horizon, and principal. 1.7
            // (§21): an argument the decoder ignored is named on a refusal too; 1.8: as one that
            // would be ignored, since nothing was applied.
            let list: Vec<Json> = opened
                .map(|x| errors::failed_opening(x, Some(i.cmd)))
                .into_iter()
                .chain(i.ignored.map(|d| errors::ignored_arguments(d.refused)))
                .collect();
            if !list.is_empty() {
                let list = Json::A(list);
                lines.push(format!("warnings: {}", list.text()));
                structured.warnings = Some(list);
            }
            true
        }
    };
    // 1.8 (§20.9): a standing item shown on each of the session's last 3 responses is folded; a
    // response without next calls breaks every run.
    let rebind = i.rebind.as_ref().filter(|_| {
        matches!(
            i.out,
            Err(Error {
                hint: Hint::NotFound(What::Session),
                ..
            })
        )
    });
    if let Some(rb) = rebind {
        let (text, json) = rebind_block(rb);
        seen.standing.clear();
        lines.push(text);
        structured.next_calls = Some(json);
    } else if let Some(g) = i.guidance.filter(|_| !carries_no_next_calls(i.out)) {
        let (text, json, standing) =
            next_calls::block_seen(s, session.or(i.session), g, &seen.standing);
        seen.standing = standing;
        lines.push(text);
        structured.next_calls = Some(json);
    } else {
        seen.standing.clear();
    }
    Response {
        text: lines.join("\n"),
        structured: structured.assemble().to_value(),
        is_error,
        seen: session.map(|_| seen),
    }
}

/// The shell's own refusal (an undecodable call, a text over `text.max`, busy, or an entry its
/// tables lack) of a call that names an existing session of its class: the status line, rendered
/// from the state as it stands, then `error` and `hint`, with R5's key hint for a node argument
/// the decoder refused, and, for a node named in `re`, that node's unsettled requests (§21); no
/// next calls (§20.1, D.11.7). With no such session it is the decoder's own rendering, which starts
/// with `error: <code>`. `seen` is what the session's earlier responses showed it (1.7).
#[must_use]
pub fn shell_refusal(
    s: &State,
    session: Option<SessionId>,
    now: Time,
    health: &Health,
    answer: &ShellAnswer,
    src: &dyn Source,
    seen: Option<&Seen>,
) -> Response {
    // R5's hint of each node argument refused, after its own problem (1.6: a refusal names every
    // problem of the call, each on its line).
    let hint = match answer {
        ShellAnswer::Invalid(e) => e.hint_with(|p| {
            let raw = p.raw_node.as_ref()?;
            if errors::is_re_arg(&p.arg) {
                errors::re_node_hint(s, &p.arg, raw)
            } else {
                let args = [NodeArgText {
                    arg: p.arg.clone(),
                    raw: raw.clone(),
                }];
                errors::key_hint(s, &args, src)
            }
        }),
        _ => answer.hint(),
    };
    let mut lines = Vec::new();
    let mut structured = Envelope::default();
    let mut shown = None;
    if let Some(x) = session.filter(|x| s.sessions.contains_key(x)) {
        let mut after = seen.cloned().unwrap_or_default();
        // 1.11 (§17): the health shown counts the session's caps and horizon.
        let shown_health = status::health_of(s, Some(x), health);
        let (line, h) = status::line(s, x, now, &shown_health, src, after.health.as_ref());
        after.health = Some(h);
        // 1.8 (§20.9): a response without next calls breaks every standing item's run.
        after.standing.clear();
        shown = Some(after);
        structured.status = Some(line.clone());
        lines.push(line);
    }
    lines.push(format!("error: {}\nhint: {hint}", answer.code_name()));
    structured.error = Some((answer.code_name(), hint));
    Response {
        text: lines.join("\n"),
        structured: structured.assemble().to_value(),
        is_error: true,
        seen: shown,
    }
}
