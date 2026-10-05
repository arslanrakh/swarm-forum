//! Tools, JSON to `Cmd`, and the request digest (design D.11.5, D.8.1).
//!
//! [`prepare`] is the shell's part of a call before it is queued, in D.8.1's order:
//! 1. the node the call is about, under any of its names, renamed to the one its action documents
//!    (1.7, §20.2: [`spec::NODE_ALIASES`]), and an argument under its older name renamed to its new
//!    one (1.10: [`spec::ARG_ALIASES`]); two names for one with different values are `invalid`;
//! 2. the arguments' shape against the action's form (`spec`): a missing or mistyped argument, a
//!    float anywhere, a name no action takes, or a directing argument the form does not take
//!    (`spec::directing`) is `invalid` naming it, and an argument of another action or form also
//!    names the actions that take it (1.5, §20.2: the input schema is one object per tool, so the
//!    decoder, not the client, checks what each action takes); any other argument of another
//!    action is dropped, and the response warns `ignored_arguments` (1.7, §21: the server notices
//!    rather than forbids);
//! 3. secret masking of every free-text argument (`mask`, D.11.13);
//! 4. the `Cmd`: keys normalised (`forum-keys`), tz tables frozen (`tz`), paths, ids, and times
//!    parsed, and the shell's lookups of an entry's node; a malformed value is `invalid`;
//! 5. with an `op_id`, the request digest of the masked arguments as applied: without the ignored
//!    ones, each node under its documented name, and without the versions the writer fills in
//!    (1.7, §20.1), so that a retry of the same call, or of it without what was ignored, gets the
//!    stored response (`digest`);
//! 6. the `text.max` check of the built command: `limit(TextTooLong)` (A-114).
//!
//! **Every problem at once** (1.6, §20.2: in the lab's first round, a call with two problems was
//! refused twice in a row). Steps 2 and 4 do not stop at a call's first problem. The shape check
//! notes each problem with a repair (an argument the form does not take is dropped; a missing or
//! malformed one gets a placeholder of its kind that parses), the build runs on the repaired
//! arguments, and each value it cannot parse is noted and replaced in turn
//! ([`build_all`]). The refusal names every problem, foreign arguments first, then missing ones,
//! then values, and ends with the action's signature. The repairs only ever find problems: a call
//! with one is refused.
//!
//! Each refusal here is a [`ShellAnswer`]: it reaches nothing else, so it opens no session, logs
//! nothing, and gets no stored `op_id` answer (§20.1, §21's precedence). An entry the shell's
//! lookup does not find is not one of them: the command names [`UNKNOWN_NODE`] for its node, and
//! the core refuses it with `not_found` in §21's order, after a stored `op_id` answer and the
//! session check, opening the session of a first call (§20.1). The decoder keeps the raw text of
//! every node argument beside the `Cmd`, and in its error the raw text of a node argument it
//! refuses, for the hint of a key or an entry id given where a node goes (1.4, R5, D.11.7).

mod build;
pub mod spec;
pub mod values;

use std::fmt;

use forum_core::{Cmd, Dur, Eid, ErrorCode, NodeId, OpId, ReqDigest, SessionId, Time};
use serde_json::{Map, Value, json};

use crate::digest::request_digest;
use crate::mask::{MASKING_VERSION, mask_v};
use spec::{
    Action, Arg, BASIS_EXTERNAL_ARGS, Form, Kind, NODE_ALIASES, Select, ToolSpec, name_forms,
};

/// The arguments of a call that are not its action's own.
pub const CONTROL_ARGS: [&str; 3] = ["action", "session", "op_id"];

/// The node the decoder names for an entry that the shell's `entries` table does not have (`hide
/// {entry}`, `retract {entry}`, `answer {question}`): no node ever has this id, since ids come from
/// a counter. The command still goes to the core, which answers `not_found` in §21's order: after a
/// stored `op_id` answer and the session check, and opening the session of a first call (§20.1,
/// review E2-spec-3). The core never appends to a node that does not exist.
pub const UNKNOWN_NODE: NodeId = NodeId(u64::MAX);

/// The raw text of one node argument, as the agent sent it (R5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeArgText {
    /// The argument, such as `at`, `depends_on[1]`, or `waits_for.node`.
    pub arg: String,
    /// Its text.
    pub raw: String,
}

/// Why the decoder refused a call: `invalid`, with a hint naming every problem of the call and the
/// action's signature (1.6, §20.2). The fields `arg`, `why`, and `raw_node` are its first problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    /// The argument, such as `at`, `repeat.cron`, or `basis[2]`.
    pub arg: String,
    /// What is wrong with it.
    pub why: String,
    /// The raw text of the node argument refused, if it is one (R5), or of a `to_user {re}` item
    /// that is plainly a node, where the eNs of requests go: §21's hint then names the node's
    /// unsettled requests, which the renderer reads from the state (1.4, review E2-spec-7).
    pub raw_node: Option<String>,
    /// The call's other problems, in the order of the hint, each with nothing in `more` and
    /// `signature` of its own (1.6).
    pub more: Vec<DecodeError>,
    /// What the action takes, its required arguments marked `*`, such as `talk to_user takes
    /// {text*, kind*, verbatim, principal, re}`: the hint's last line (1.6, §20.2). Empty when the
    /// call names no action of its tool.
    pub signature: String,
}

impl DecodeError {
    pub(crate) fn new(arg: impl Into<String>, why: impl Into<String>) -> DecodeError {
        DecodeError {
            arg: arg.into(),
            why: why.into(),
            raw_node: None,
            more: Vec::new(),
            signature: String::new(),
        }
    }

    /// The call's problems, the first first.
    pub fn problems(&self) -> impl Iterator<Item = &DecodeError> {
        std::iter::once(self).chain(self.more.iter())
    }

    /// The hint: one line per problem, `<argument>: <what is wrong>`, with what `extra` adds for
    /// it after a `;` (the renderer's R5 hint of a node argument, read from the state), then the
    /// signature (1.6).
    pub fn hint_with(&self, extra: impl Fn(&DecodeError) -> Option<String>) -> String {
        let mut lines: Vec<String> = self
            .problems()
            .map(|p| match extra(p) {
                Some(x) => format!("{}: {}; {x}", p.arg, p.why),
                None => format!("{}: {}", p.arg, p.why),
            })
            .collect();
        if !self.signature.is_empty() {
            lines.push(self.signature.clone());
        }
        lines.join("\n")
    }

    /// This refusal with `first` named before its own problems: the session that a call lacks
    /// (1.6, §20.1), which the MCP handler adds.
    #[must_use]
    pub fn with_first(mut self, first: DecodeError) -> DecodeError {
        let signature = std::mem::take(&mut self.signature);
        let more = std::mem::take(&mut self.more);
        DecodeError {
            more: std::iter::once(self).chain(more).collect(),
            signature,
            ..first
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hint_with(|_| None))
    }
}

/// The problem of a call other than `session hello` that names no session, from an MCP session
/// bound to none (1.6, §20.1): it is refused and opens nothing. Before 1.6 it opened a session of
/// its own, and agents whose client keeps no MCP session worked in two sessions at once (l2, l3,
/// l4).
#[must_use]
pub fn no_session() -> DecodeError {
    DecodeError::new(
        "session",
        "missing: no session is bound to this connection: pass session: the s-N your hello \
         returned, shown in every status line as session s-N; a new agent starts with session \
         hello",
    )
}

/// The refusal of a `node update` that left out a version this session was not shown (1.7,
/// §20.5, §21): after a restart, after the session fell silent, or for a node it never read. The
/// hint names the node's current versions and the same update with them filled, so that the
/// retry needs no `view node`; checking that the edit still fits them is the agent's.
#[must_use]
pub fn versions_not_shown(
    node: &str,
    version: u64,
    digest_version: u64,
    fill: FillVersions,
) -> DecodeError {
    let arg = match (fill.version, fill.digest_version) {
        (true, true) => "version, digest_version",
        (true, false) => "version",
        _ => "digest_version",
    };
    let filled = if fill.digest_version {
        format!("version: {version}, digest_version: {digest_version}")
    } else {
        format!("version: {version}")
    };
    let spec = Tool::Node.spec();
    let signature = spec
        .action("update")
        .and_then(|a| a.forms.first().map(|f| signature(Tool::Node, a, f)))
        .unwrap_or_default();
    DecodeError {
        signature,
        ..DecodeError::new(
            arg,
            format!(
                "missing, and this session was not shown {node} since the server started or the \
                 session fell silent: it is at version {version} and digest_version \
                 {digest_version} now; if your edit still fits it, retry with them: node update \
                 {{path: {node}, {filled}, …}}"
            ),
        )
    }
}

/// An answer the shell gives itself, before queueing or, for `write.per_hour`, before `apply`: it
/// reaches nothing else, opens no session, and logs nothing (§21).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellAnswer {
    /// The arguments cannot be decoded: `invalid`.
    Invalid(DecodeError),
    /// A free-text argument is longer than `text.max` bytes after masking: `limit(TextTooLong)`
    /// (§19, L1, A-114).
    TextTooLong {
        /// `text.max`.
        max: u32,
        /// The longest text's length in bytes.
        longest: u64,
    },
    /// The class's oldest queued call has waited longer than `queue.max_wait`: `limit(Busy)`,
    /// answered before queueing (§19, CS-29, D.11.2).
    Busy,
    /// The session made `write.per_hour` write calls in the last hour: `limit(WritesPerHour)`,
    /// answered by the writer before `apply` (§19, D.11.15).
    WritesPerHour {
        /// `write.per_hour`.
        max: u32,
    },
}

impl ShellAnswer {
    /// Its code (§21).
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            ShellAnswer::Invalid(_) => ErrorCode::Invalid,
            ShellAnswer::TextTooLong { .. }
            | ShellAnswer::Busy
            | ShellAnswer::WritesPerHour { .. } => ErrorCode::Limit,
        }
    }

    /// The code as §21 writes it.
    #[must_use]
    pub fn code_name(&self) -> &'static str {
        match self {
            ShellAnswer::Invalid(_) => "invalid",
            ShellAnswer::TextTooLong { .. }
            | ShellAnswer::Busy
            | ShellAnswer::WritesPerHour { .. } => "limit",
        }
    }

    /// The hint.
    #[must_use]
    pub fn hint(&self) -> String {
        match self {
            ShellAnswer::Invalid(e) => e.to_string(),
            ShellAnswer::TextTooLong { max, longest } => format!(
                "a free-text argument has {longest} bytes, over text.max ({max} bytes); nothing \
                 was applied: put a large artefact in a file outside the forum and pass a \
                 reference to it"
            ),
            ShellAnswer::Busy => "the server is busy; nothing was applied; retry in a few \
                                  seconds with the same op_id"
                .to_string(),
            ShellAnswer::WritesPerHour { max } => format!(
                "write.per_hour ({max}) writes in the last hour; nothing was applied: write less \
                 often"
            ),
        }
    }

    /// The response text: no status line, because no session is involved (§20.1, R27), and no
    /// next calls (§20.9).
    #[must_use]
    pub fn render(&self) -> String {
        format!("error: {}\nhint: {}", self.code_name(), self.hint())
    }
}

/// The shell's reads of entries whose metadata the core does not keep (CS-17, D.4.12): from the
/// `entries` table on a read-only connection.
pub trait EntryLookup {
    /// The node of entry `eid`, if it exists.
    fn entry_node(&self, eid: Eid) -> Option<NodeId>;
    /// The node and author session of the question `eid`, if the entry exists.
    fn question(&self, eid: Eid) -> Option<(NodeId, Option<SessionId>)>;
}

/// What the decoder needs from the shell.
pub struct DecodeEnv<'a> {
    /// The shell's clock: tz tables span `now − coverage.window` to `now + 50 years` (D.11.10).
    pub now: Time,
    /// `text.max` (A-114).
    pub text_max: u32,
    /// `coverage.window`.
    pub coverage_window: Dur,
    /// The shell's entry reads.
    pub entries: &'a dyn EntryLookup,
}

/// The six tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    /// `session`.
    Session,
    /// `view`.
    View,
    /// `node`.
    Node,
    /// `hold`.
    Hold,
    /// `talk`.
    Talk,
    /// `control`.
    Control,
}

impl Tool {
    /// Every tool, in §20's order.
    pub const ALL: [Tool; 6] = [
        Tool::Session,
        Tool::View,
        Tool::Node,
        Tool::Hold,
        Tool::Talk,
        Tool::Control,
    ];

    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// Its argument table.
    #[must_use]
    pub fn spec(self) -> &'static ToolSpec {
        let i = match self {
            Tool::Session => 0,
            Tool::View => 1,
            Tool::Node => 2,
            Tool::Hold => 3,
            Tool::Talk => 4,
            Tool::Control => 5,
        };
        &spec::TOOLS[i]
    }

    /// The tool named `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// A decoded call, ready to queue. Its `Debug` leaves out the arguments as sent, which may hold
/// the secrets that masking removed.
#[derive(Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The tool.
    pub tool: Tool,
    /// The action.
    pub action: &'static str,
    /// The `session` argument.
    pub session: Option<SessionId>,
    /// The `op_id` argument.
    pub op_id: Option<OpId>,
    /// The command, with every free text masked.
    pub cmd: Cmd,
    /// The raw text of every node argument (R5).
    pub node_args: Vec<NodeArgText>,
    /// `view feed {since}` given as an entry id: the writer turns it into the node's last seq at
    /// that entry, as it turns `wait`'s `since` (1.6, §20.4).
    pub since_entry: Option<Eid>,
    /// How many free texts masking changed: the `secret_masked` warning's count.
    pub masked_fields: u32,
    /// The arguments of other actions that the call gave and the decoder dropped: the
    /// `ignored_arguments` warning (1.7, §21).
    pub ignored: Vec<Ignored>,
    /// The versions a `node update` left out, which the writer fills with those last shown to the
    /// session (1.7, §20.5).
    pub fill: Option<FillVersions>,
    /// With an `op_id`, the request digest under the current masking version.
    pub digest: Option<ReqDigest>,
    /// The arguments as the decoder applied them, before masking: as the agent sent them, without
    /// the ignored ones, each node under its documented name (1.7). Kept only to digest a retry
    /// under the masking version of its `op_id` record; never stored or logged.
    raw: Map<String, Value>,
    form: &'static Form,
}

/// An argument of another action, which the call gave and the decoder dropped (1.7, §21).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    /// Its name.
    pub name: String,
    /// The actions that take it, as a refusal names them: `talk decide`, or `talk ask (person,
    /// agent) and hold renew`.
    pub takers: String,
    /// What else §21 says of it, if anything (1.4): for `at` in a `to_user`, that a message about
    /// to be sent in a conversation is a `talk said`; it starts with `; `.
    pub note: &'static str,
}

/// What a `node update` left out (1.7, §20.5): its `version`, and its `digest_version` beside a
/// `digest`. The writer fills each with the value last shown to the session, or refuses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillVersions {
    /// `version` was left out.
    pub version: bool,
    /// `digest_version` was left out, and `digest` given.
    pub digest_version: bool,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("tool", &self.tool)
            .field("action", &self.action)
            .field("session", &self.session)
            .field("op_id", &self.op_id)
            .field("cmd", &self.cmd)
            .field("node_args", &self.node_args)
            .field("since_entry", &self.since_entry)
            .field("masked_fields", &self.masked_fields)
            .field("ignored", &self.ignored)
            .field("fill", &self.fill)
            .field("digest", &self.digest)
            .finish_non_exhaustive()
    }
}

impl Prepared {
    /// The arguments the call gave, beside `action`, `session`, and `op_id`, each by its
    /// documented name, in the order of its form's table (1.12, §20.1): the next call of a call
    /// refused for a session other than its connection's names them for the agent to write again.
    #[must_use]
    pub fn given(&self) -> Vec<&'static str> {
        self.form
            .args
            .iter()
            .map(|a| a.name)
            .filter(|n| self.raw.contains_key(*n))
            .collect()
    }

    /// The detail of the `ignored_arguments` warning (1.7, §21) on a call that was applied, or
    /// None when the call gave no argument of another action: each argument ignored, with the
    /// actions that take it.
    #[must_use]
    pub fn ignored_detail(&self) -> Option<String> {
        self.ignored_text(true)
    }

    /// The detail of the `ignored_arguments` warning on a call the core refused (1.8, §21): that
    /// the arguments would be ignored, since nothing was applied (it2 l1: "it was applied without
    /// it" on a refusal left the agent unsure what had happened).
    #[must_use]
    pub fn ignored_detail_refused(&self) -> Option<String> {
        self.ignored_text(false)
    }

    fn ignored_text(&self, applied: bool) -> Option<String> {
        if self.ignored.is_empty() {
            return None;
        }
        let label = form_hint(self.tool, self.tool.spec().action(self.action)?, self.form);
        let each: Vec<String> = self
            .ignored
            .iter()
            .map(|i| format!("{} ({} takes it)", i.name, i.takers))
            .collect();
        let notes: String = self.ignored.iter().map(|i| i.note).collect();
        let one = self.ignored.len() == 1;
        let what = match (applied, one) {
            (true, true) => "so the call was applied without it",
            (true, false) => "so the call was applied without them",
            (false, true) => "so it would be ignored: leave it out when you retry",
            (false, false) => "so they would be ignored: leave them out when you retry",
        };
        Some(format!(
            "{label} does not take {}, {what}{notes}",
            each.join(", ")
        ))
    }

    /// The request digest under masking version `version`, the version an existing `op_id`
    /// record stores (CS-35): None without an `op_id`, or for a version that does not exist.
    #[must_use]
    pub fn digest_with(&self, version: u32) -> Option<ReqDigest> {
        self.digest_under(version, MASKING_VERSION, mask_v)
    }

    /// [`Prepared::digest_with`] for a call prepared under masking version `current` with the
    /// masker `masker`, which answers None for a version it does not have: the digest of the
    /// call for `current`, else the arguments as sent masked again with `version`. The tests give
    /// it masker sets that differ, which the one released version cannot (review E2-spec-12).
    fn digest_under(&self, version: u32, current: u32, masker: Masker) -> Option<ReqDigest> {
        self.op_id.as_ref()?;
        masker(version, "")?;
        if version == current {
            return self.digest;
        }
        let mut args = self.raw.clone();
        mask_form(self.form, &mut args, version, masker).ok()?;
        request_digest(self.tool.name(), &args)
    }
}

/// Masks one text with a masking version; None for a version that does not exist.
type Masker = fn(u32, &str) -> Option<String>;

/// Why masking a call's arguments failed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum MaskError {
    /// The masking version does not exist.
    Version,
    /// Two names of the object at this argument mask to the same text (review E2-bugs-5).
    Collision(String),
}

/// Decodes a call to `tool` with `args`, masks it, digests it, and builds its command; or the
/// answer the shell gives before queueing.
pub fn prepare(
    tool: &str,
    args: Map<String, Value>,
    env: &DecodeEnv<'_>,
) -> Result<Prepared, ShellAnswer> {
    let t = Tool::named(tool).ok_or_else(|| {
        ShellAnswer::Invalid(DecodeError::new(
            "tool",
            format!("{tool:?} is not a tool: session, view, node, hold, talk, or control"),
        ))
    })?;
    let invalid = ShellAnswer::Invalid;
    let (action, form) = pick(t, &args).map_err(invalid)?;
    let signature = signature(t, action, form);
    let mut found = Found::default();
    let root = At::default();
    let mut args = args;
    name_the_node(t, action, form, &mut args, &mut found);
    rename_aliases(t, action, form, &mut args, &mut found);
    let session = match args.get("session") {
        None => None,
        Some(Value::String(s)) => {
            let parsed = SessionId::parse(s);
            if parsed.is_none() {
                found.problem(
                    Order::Value,
                    DecodeError::new(
                        "session",
                        format!("{s:?} is not a session id: such as s-4, from a status line"),
                    ),
                    [(root.key("session"), None)],
                );
            }
            parsed
        }
        Some(_) => {
            found.problem(
                Order::Value,
                DecodeError::new("session", "expected a string such as s-4"),
                [(root.key("session"), None)],
            );
            None
        }
    };
    let op_id = match args.get("op_id") {
        None => None,
        Some(Value::String(s)) => {
            let parsed = OpId::new(s);
            if parsed.is_none() {
                found.problem(
                    Order::Value,
                    DecodeError::new("op_id", "1 to 64 bytes of printable ASCII, such as a UUID"),
                    [(root.key("op_id"), None)],
                );
            }
            parsed
        }
        Some(_) => {
            found.problem(
                Order::Value,
                DecodeError::new("op_id", "expected a string such as a UUID"),
                [(root.key("op_id"), None)],
            );
            None
        }
    };
    check_form(t, action, form, &args, &mut found);
    // The arguments with each problem found so far repaired, so that the build can name every
    // value that does not parse too (1.6). Without a problem, they are the arguments as applied:
    // as sent, without the ignored ones (1.7).
    let mut masked = args.clone();
    found.repair(&mut masked);
    let applied = masked.clone();
    let fill = (t == Tool::Node && action.name == "update")
        .then(|| FillVersions {
            version: !applied.contains_key("version"),
            digest_version: applied.contains_key("digest")
                && !applied.contains_key("digest_version"),
        })
        .filter(|f| f.version || f.digest_version);
    let masked_fields = match mask_form(form, &mut masked, MASKING_VERSION, mask_v) {
        Ok(n) => n,
        Err(MaskError::Collision(at)) => {
            found.problem(
                Order::Value,
                DecodeError::new(
                    at,
                    "two names are the same once their secrets are masked, and one would be \
                     lost: keep secrets out of the names",
                ),
                [],
            );
            return Err(invalid(found.refusal(signature)));
        }
        Err(MaskError::Version) => 0,
    };
    let built = build_all(t, action, form, &masked, env, &mut found);
    let built = match built {
        Some(b) if found.is_empty() => b,
        _ => return Err(invalid(found.refusal(signature))),
    };
    let digest = match op_id {
        Some(_) => Some(request_digest(t.name(), &masked).ok_or_else(|| {
            invalid(DecodeError {
                signature: signature.clone(),
                ..DecodeError::new("arguments", "a number must be an integer")
            })
        })?),
        None => None,
    };
    let longest = built.cmd.longest_text();
    if longest > u64::from(env.text_max) {
        return Err(ShellAnswer::TextTooLong {
            max: env.text_max,
            longest,
        });
    }
    Ok(Prepared {
        tool: t,
        action: action.name,
        session,
        op_id,
        cmd: built.cmd,
        node_args: built.node_args,
        since_entry: built.since_entry,
        masked_fields,
        ignored: found.ignored.into_iter().map(|(i, _)| i).collect(),
        fill,
        digest,
        raw: applied,
        form,
    })
}

/// The node the call is about, under whichever of [`NODE_ALIASES`] the call gave it, renamed to
/// the name its action documents (1.7, §20.2): agents guessed each other action's name for it.
/// A name the form takes as an argument of its own is that argument, never an alias. Two names
/// with the same value are one; with different values, the call is `invalid`, and the alias is
/// dropped so that the rest can be checked.
fn name_the_node(
    t: Tool,
    action: &Action,
    form: &'static Form,
    args: &mut Map<String, Value>,
    found: &mut Found,
) {
    let Some(name) = spec::about(t.name(), action.name, form) else {
        return;
    };
    let label = form_hint(t, action, form);
    for alias in NODE_ALIASES {
        if alias == name || form.arg(alias).is_some() {
            continue;
        }
        let Some(v) = args.remove(alias) else {
            continue;
        };
        match args.get(name) {
            None => {
                args.insert(name.to_string(), v);
            }
            Some(own) if *own == v => {}
            Some(own) => found.problem(
                Order::Missing,
                DecodeError::new(
                    format!("{name}|{alias}"),
                    format!(
                        "two nodes given: {name} {} and {alias} {}; {label} is about one node: \
                         give it once, as {name}",
                        own, v
                    ),
                ),
                [],
            ),
        }
    }
}

/// An argument under an older name (1.10, §20.2: [`spec::ARG_ALIASES`]), renamed to its name, as
/// the node a call is about is (1.7): `upheld` of `talk resolve` is `stands`. Two values that
/// differ are `invalid`, and the alias is dropped so that the rest can be checked.
fn rename_aliases(
    t: Tool,
    action: &Action,
    form: &'static Form,
    args: &mut Map<String, Value>,
    found: &mut Found,
) {
    for (tool, act, alias, name) in spec::ARG_ALIASES {
        if *tool != t.name() || *act != action.name || form.arg(name).is_none() {
            continue;
        }
        let Some(v) = args.remove(*alias) else {
            continue;
        };
        match args.get(*name) {
            None => {
                args.insert((*name).to_string(), v);
            }
            Some(own) if *own == v => {}
            Some(own) => found.problem(
                Order::Missing,
                DecodeError::new(
                    format!("{name}|{alias}"),
                    format!(
                        "two values given: {name} {own} and {alias} {v}; {alias} is the older \
                         name of {name}: give it once, as {name}"
                    ),
                ),
                [],
            ),
        }
    }
}

/// The most values one refusal names that the build could not parse; a call with more names
/// these.
const VALUES_MAX: usize = 32;

/// The command of a call; and, when a value does not parse, that value noted as a problem and
/// replaced by a placeholder of its kind, and the build run again, until it succeeds: so that a
/// refusal names every value that does not parse (1.6). The build is pure, so a value that parses
/// on its own parses beside placeholders too. None once a value was noted, or cannot be replaced.
fn build_all(
    t: Tool,
    action: &Action,
    form: &'static Form,
    args: &Map<String, Value>,
    env: &DecodeEnv<'_>,
    found: &mut Found,
) -> Option<build::Built> {
    let mut repaired: Option<Map<String, Value>> = None;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let e = match build::build(t, action, form, repaired.as_ref().unwrap_or(args), env) {
            Ok(b) => return (seen.is_empty()).then_some(b),
            Err(e) => e,
        };
        if seen.contains(&e.arg) {
            return None;
        }
        seen.push(e.arg.clone());
        let fix =
            At::parse(&e.arg).and_then(|at| placeholder_at(form.args, &at.0).map(|v| (at, v)));
        found.problem(Order::Value, e, []);
        let (at, v) = fix?;
        if seen.len() >= VALUES_MAX {
            return None;
        }
        let map = repaired.get_or_insert_with(|| args.clone());
        if !edit(map, &at.0, Some(v)) {
            return None;
        }
    }
}

/// How the hint ends: the action's signature, `<tool> <action> takes {…}`, its required
/// arguments marked `*` (1.6, §20.2). For `talk ask`, whose form `to` picks, every form's, one
/// per line, the call's own first (1.8, §20.7): an argument of another form is refused, and the
/// agent picks the form it meant.
fn signature(t: Tool, action: &Action, form: &'static Form) -> String {
    let one = |f: &Form| {
        let label = form_hint(t, action, f);
        if f.args.is_empty() {
            format!("{label} takes no arguments")
        } else {
            format!("{label} takes {{{}}}", f.arg_list())
        }
    };
    if action.select != Select::AskTo {
        return one(form);
    }
    std::iter::once(form)
        .chain(action.forms.iter().filter(|f| !std::ptr::eq(*f, form)))
        .map(one)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The order in which a refusal names its problems (1.6, §20.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Order {
    /// An argument the action, or a nested object, does not take.
    Foreign,
    /// A required argument missing, or a group given more than once.
    Missing,
    /// A value of the wrong kind, or that does not parse.
    Value,
}

/// A repair: the place, and the value it gets, or None to drop it.
type Fix = (At, Option<Value>);

/// What the checks of a call found: its problems, and how to repair each so that the rest of the
/// call can still be checked (1.6).
#[derive(Default)]
struct Found {
    problems: Vec<(Order, DecodeError)>,
    fixes: Vec<Fix>,
    /// The arguments of other actions, dropped (1.7): each with the line a refusal gives it, if
    /// the call is refused for something else.
    ignored: Vec<(Ignored, DecodeError)>,
}

impl Found {
    /// An argument of another action that records nothing of its own: dropped, with a warning
    /// (1.7, §21).
    fn ignore(&mut self, i: Ignored, line: DecodeError, at: &At) {
        self.ignored.push((i, line));
        self.fixes.push((at.clone(), None));
    }

    fn problem(&mut self, order: Order, e: DecodeError, fixes: impl IntoIterator<Item = Fix>) {
        self.problems.push((order, e));
        self.fixes.extend(fixes);
    }

    /// An argument the form, or the object at `at`'s parent, does not take: dropped.
    fn foreign(&mut self, e: DecodeError, at: &At) {
        self.problem(Order::Foreign, e, [(at.clone(), None)]);
    }

    /// A value of `a` that is not of its kind: dropped, or a placeholder if `a` is required.
    fn value(&mut self, e: DecodeError, at: &At, a: &Arg) {
        self.problem(Order::Value, e, [(at.clone(), fix_of(a))]);
    }

    fn is_empty(&self) -> bool {
        self.problems.is_empty()
    }

    /// `args` with every repair made.
    fn repair(&self, args: &mut Map<String, Value>) {
        for (at, to) in &self.fixes {
            edit(args, &at.0, to.clone());
        }
    }

    /// The refusal: every problem, in [`Order`], then `signature`. An argument it would have
    /// ignored is named too, among the foreign ones, so that one retry suffices (1.7).
    fn refusal(self, signature: String) -> DecodeError {
        let mut problems = self.problems;
        problems.extend(self.ignored.into_iter().map(|(_, e)| (Order::Foreign, e)));
        problems.sort_by_key(|(o, _)| *o);
        let mut all = problems.into_iter().map(|(_, e)| e);
        let first = all
            .next()
            .unwrap_or_else(|| DecodeError::new("arguments", "they do not fit the action"));
        DecodeError {
            more: all.collect(),
            signature,
            ..first
        }
    }
}

/// The repair of a value of `a`: a placeholder of its kind if it is required, else dropped.
fn fix_of(a: &Arg) -> Option<Value> {
    a.required.then(|| placeholder(a.kind))
}

/// One step of a place in a call's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    /// An argument of an object.
    Key(String),
    /// An item of a list.
    Index(usize),
}

/// A place in a call's arguments, as a hint names it: `repeat.cron`, `basis[2].checked_at`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct At(Vec<Seg>);

impl At {
    fn key(&self, k: &str) -> At {
        let mut v = self.0.clone();
        v.push(Seg::Key(k.to_string()));
        At(v)
    }

    fn index(&self, i: usize) -> At {
        let mut v = self.0.clone();
        v.push(Seg::Index(i));
        At(v)
    }

    fn text(&self) -> String {
        let mut out = String::new();
        for seg in &self.0 {
            match seg {
                Seg::Key(k) if out.is_empty() => out.push_str(k),
                Seg::Key(k) => {
                    out.push('.');
                    out.push_str(k);
                }
                Seg::Index(i) => out.push_str(&format!("[{i}]")),
            }
        }
        out
    }

    /// The place a hint names, such as `basis[2].checked_at`; None for a text that names none,
    /// such as `entry|file`.
    fn parse(text: &str) -> Option<At> {
        let mut out = Vec::new();
        for part in text.split('.') {
            let (name, mut rest) = part.split_at(part.find('[').unwrap_or(part.len()));
            if name.is_empty() || name.contains(['|', ',', ' ']) {
                return None;
            }
            out.push(Seg::Key(name.to_string()));
            while let Some(r) = rest.strip_prefix('[') {
                let end = r.find(']')?;
                out.push(Seg::Index(r.get(..end)?.parse().ok()?));
                rest = r.get(end + 1..)?;
            }
            if !rest.is_empty() {
                return None;
            }
        }
        Some(At(out))
    }
}

/// Sets the value at `at` to `to`, or drops it with None; whether there was such a place.
fn edit(args: &mut Map<String, Value>, at: &[Seg], to: Option<Value>) -> bool {
    let mut root = Value::Object(std::mem::take(args));
    let done = edit_value(&mut root, at, to);
    if let Value::Object(m) = root {
        *args = m;
    }
    done
}

fn edit_value(v: &mut Value, at: &[Seg], to: Option<Value>) -> bool {
    match at {
        [] => false,
        [Seg::Key(k)] => match (v.as_object_mut(), to) {
            (Some(m), Some(x)) => {
                m.insert(k.clone(), x);
                true
            }
            (Some(m), None) => m.remove(k).is_some(),
            (None, _) => false,
        },
        // A list item is replaced, never dropped, so that the places of the others stay.
        [Seg::Index(i)] => match (v.as_array_mut().and_then(|xs| xs.get_mut(*i)), to) {
            (Some(slot), Some(x)) => {
                *slot = x;
                true
            }
            _ => false,
        },
        [Seg::Key(k), rest @ ..] => v
            .as_object_mut()
            .and_then(|m| m.get_mut(k))
            .is_some_and(|x| edit_value(x, rest, to)),
        [Seg::Index(i), rest @ ..] => v
            .as_array_mut()
            .and_then(|xs| xs.get_mut(*i))
            .is_some_and(|x| edit_value(x, rest, to)),
    }
}

/// A value of `kind` that the build parses: the repair of a missing or malformed argument.
fn placeholder(kind: Kind) -> Value {
    match kind {
        Kind::Text | Kind::Label => json!("x"),
        Kind::Cron => json!("0 0 * * *"),
        Kind::Zone => json!("UTC"),
        Kind::Node => json!("/"),
        Kind::Place => json!("/x"),
        Kind::NodeAt => json!("#1@1"),
        Kind::Key => json!("chat:c1/t1"),
        Kind::Principal | Kind::Addressee | Kind::AskHuman => json!("user"),
        Kind::Requester => json!("all"),
        Kind::Token => json!("1.1"),
        Kind::Eid => json!("e1"),
        Kind::Seq | Kind::SeqOrEid | Kind::U64 | Kind::U32 | Kind::I32 => json!(0),
        Kind::Dur => json!("1s"),
        Kind::When => json!("in 1s"),
        Kind::Checked => json!("now"),
        Kind::Bool => json!(false),
        Kind::Enum(values) => json!(values.first().copied().unwrap_or("x")),
        Kind::Nodes
        | Kind::Keys
        | Kind::Eids
        | Kind::Labels
        | Kind::Enums(_)
        | Kind::TextList
        | Kind::Basis => json!([]),
        Kind::TextMap => json!({}),
        Kind::Object(args) => Value::Object(
            args.iter()
                .filter(|a| a.required)
                .map(|a| (a.name.to_string(), placeholder(a.kind)))
                .collect(),
        ),
        Kind::OneOf(variants) => variants
            .first()
            .map_or_else(|| json!({}), |args| placeholder(Kind::Object(args))),
        Kind::AskAgent => json!("session:s-1"),
        Kind::RouteTo => json!("none"),
        Kind::Outcome => json!("done"),
        Kind::SessionId => json!("s-1"),
        Kind::OpId => json!("x"),
    }
}

/// The placeholder of the value at `at` among `args`: a list item, or an argument of a nested
/// object, as the build names it.
fn placeholder_at(args: &'static [Arg], at: &[Seg]) -> Option<Value> {
    let (Seg::Key(name), rest) = at.split_first()? else {
        return None;
    };
    let a = args.iter().find(|a| a.name == name)?;
    match (a.kind, rest) {
        (k, []) => Some(placeholder(k)),
        (Kind::Object(inner), [Seg::Key(_), ..]) => placeholder_at(inner, rest),
        (Kind::OneOf(variants), [Seg::Key(n), ..]) => variants
            .iter()
            .find(|v| v.iter().any(|x| x.name == n))
            .and_then(|v| placeholder_at(v, rest)),
        (Kind::Basis, [Seg::Index(_)]) => Some(json!("e1")),
        (Kind::Basis, [Seg::Index(_), more @ ..]) => placeholder_at(BASIS_EXTERNAL_ARGS, more),
        (Kind::Nodes, [Seg::Index(_)]) => Some(placeholder(Kind::Node)),
        (Kind::Keys, [Seg::Index(_)]) => Some(placeholder(Kind::Key)),
        (Kind::Eids, [Seg::Index(_)]) => Some(placeholder(Kind::Eid)),
        (Kind::Labels, [Seg::Index(_)]) => Some(placeholder(Kind::Label)),
        (Kind::Enums(values), [Seg::Index(_)]) => Some(placeholder(Kind::Enum(values))),
        (Kind::TextList, [Seg::Index(_)]) => Some(placeholder(Kind::Text)),
        _ => None,
    }
}

/// Joins an argument's path.
pub(crate) fn fq(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

/// D.11.5: integers only; where a float sits in `v`, if anywhere.
fn float_at(v: &Value, at: &At) -> Option<At> {
    match v {
        Value::Number(n) if n.as_i64().is_none() && n.as_u64().is_none() => Some(at.clone()),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, x)| float_at(x, &at.index(i))),
        Value::Object(m) => m.iter().find_map(|(k, x)| float_at(x, &at.key(k))),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

/// The action, and the form the call takes (§20.7 for `ask`).
fn pick(
    t: Tool,
    args: &Map<String, Value>,
) -> Result<(&'static Action, &'static Form), DecodeError> {
    let spec = t.spec();
    let names = spec.action_names().join(", ");
    let name = match args.get("action") {
        Some(Value::String(s)) => s,
        Some(_) => {
            return Err(DecodeError::new(
                "action",
                format!("expected a string: one of {names}"),
            ));
        }
        None => {
            return Err(DecodeError::new(
                "action",
                format!("missing: {} takes one of {names}", t.name()),
            ));
        }
    };
    let action = spec.action(name).ok_or_else(|| {
        DecodeError::new(
            "action",
            format!(
                "{name:?} is not an action of {}: {names}{}",
                t.name(),
                owners_hint(t, name)
            ),
        )
    })?;
    let form_name = match action.select {
        Select::Single => action.name,
        Select::SaidId => {
            if args.contains_key("said_id") {
                "confirm"
            } else {
                "record"
            }
        }
        Select::AskTo => match args.get("to") {
            None => "person",
            Some(Value::String(s)) if s == "user" || s.starts_with("principal:") => "human",
            Some(_) => "agent",
        },
    };
    let form = action
        .form(form_name)
        .ok_or_else(|| DecodeError::new("action", format!("{name:?} has no form {form_name}")))?;
    Ok((action, form))
}

/// §21's hint for an action of another tool: the tools it belongs to, and the call to make
/// instead (1.4, review E2-spec-6); empty when no other tool has it.
fn owners_hint(t: Tool, name: &str) -> String {
    let calls: Vec<String> = Tool::ALL
        .into_iter()
        .filter(|o| *o != t)
        .filter_map(|o| {
            let a = o.spec().action(name)?;
            let takes = a.forms.iter().any(|f| !f.args.is_empty());
            Some(if takes {
                format!("{} {{action: {name}, …}}", o.name())
            } else {
                format!("{} {{action: {name}}}", o.name())
            })
        })
        .collect();
    if calls.is_empty() {
        String::new()
    } else {
        format!(
            "; {name} is an action of another tool: call {}",
            calls.join(" or ")
        )
    }
}

fn form_hint(t: Tool, action: &Action, form: &Form) -> String {
    if action.forms.len() > 1 {
        form.label.to_string()
    } else {
        format!("{} {}", t.name(), action.name)
    }
}

fn arg_names(args: &[Arg]) -> String {
    if args.is_empty() {
        "no arguments".to_string()
    } else {
        args.iter().map(|a| a.name).collect::<Vec<_>>().join(", ")
    }
}

/// The arguments of `map` sorted by name. `serde_json` keeps the client's key order here
/// (`preserve_order`, which the structured content needs, D.11.7), and the decoder checks in this
/// order, so that the argument a refusal names depends on the arguments and not on how the client
/// ordered them.
fn by_name(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut v: Vec<(&String, &Value)> = map.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

/// The shape of a call against its form: every argument known, every required one given, one of
/// each `one_of` group, and every value of its kind's JSON type; each problem noted with its
/// repair (1.6).
fn check_form(
    t: Tool,
    action: &Action,
    form: &'static Form,
    args: &Map<String, Value>,
    found: &mut Found,
) {
    let label = form_hint(t, action, form);
    let root = At::default();
    for (k, v) in by_name(args) {
        if CONTROL_ARGS.contains(&k.as_str()) {
            continue;
        }
        let here = root.key(k);
        let Some(a) = form.arg(k) else {
            let takers = takers(t, form, k);
            // 1.7 (§21): an argument of another action that records nothing of its own is
            // dropped, with a warning; a name no action takes (a typo), and a directing one,
            // stay `invalid`.
            if !takers.is_empty() && !spec::directing(t.name(), action.name, k) {
                found.ignore(
                    Ignored {
                        name: k.clone(),
                        takers: takers.join(" and "),
                        note: unknown_arg_hint(t, action, form, k),
                    },
                    // 1.8 (§21): the call is refused for something else, so nothing is applied,
                    // with this argument or without it.
                    DecodeError::new(
                        k.as_str(),
                        format!(
                            "{k} is not an argument of {label}, and would be ignored{}{}",
                            takers_hint(t, form, k),
                            unknown_arg_hint(t, action, form, k)
                        ),
                    ),
                    &here,
                );
                continue;
            }
            found.foreign(
                DecodeError::new(
                    k.as_str(),
                    format!(
                        "{k} is not an argument of {label}{}{}",
                        takers_hint(t, form, k),
                        unknown_arg_hint(t, action, form, k)
                    ),
                ),
                &here,
            );
            continue;
        };
        if let Some(f) = float_at(v, &here) {
            found.value(
                DecodeError::new(
                    f.text(),
                    "numbers are integers here: a duration is a string such as 15m, or \
                     milliseconds",
                ),
                &here,
                a,
            );
            continue;
        }
        // `update` clears a field set to null (§20.5); one that cannot be cleared says so. Its
        // versions, left out, are those last shown to the session (1.7).
        if v.is_null() && !a.nullable && !a.required && form.args.iter().any(|x| x.nullable) {
            let instead = match k.as_str() {
                "version" | "digest_version" => format!(
                    "give the {k} of the brief you were shown, or leave it out for the one last \
                     shown to you"
                ),
                _ => format!(
                    "give {}, or leave it out to leave it unchanged",
                    kind_noun(a.kind)
                ),
            };
            found.value(
                DecodeError::new(
                    k.as_str(),
                    format!("{k} cannot be cleared with null: {instead}"),
                ),
                &here,
                a,
            );
            continue;
        }
        check_value(a, v, &here, found);
    }
    check_required(form.args, args, &root, &label, form.one_of, found);
    // A group of which the call gives none is missing (`check_required`); one of which it gives
    // several keeps the first it gives.
    for group in form.one_of {
        let given: Vec<&str> = group
            .iter()
            .copied()
            .filter(|n| args.contains_key(*n))
            .collect();
        if given.len() > 1 {
            found.problem(
                Order::Missing,
                DecodeError::new(
                    group.join("|"),
                    format!("{label} takes exactly one of {}", group.join(" and ")),
                ),
                given.iter().skip(1).map(|n| (root.key(n), None)),
            );
        }
    }
}

/// The actions that take an argument the call's form does not (1.5, §20.2): those of the call's
/// tool first, named as its input schema names them (`talk ask (human)`), then those of the other
/// tools; or that no action takes it. A client that does not check the flat schema learns here
/// which action it meant.
fn takers_hint(t: Tool, form: &'static Form, name: &str) -> String {
    let groups = takers(t, form, name);
    if groups.is_empty() {
        format!("; no action takes {name}")
    } else {
        format!("; {name} is an argument of {}", groups.join(" and "))
    }
}

/// The forms other than `form` that take `name`, tool by tool, the call's tool first: `talk ask
/// (person, agent)`, `hold renew`.
fn takers(t: Tool, form: &'static Form, name: &str) -> Vec<String> {
    let tools = std::iter::once(t).chain(Tool::ALL.into_iter().filter(|o| *o != t));
    tools
        .filter_map(|o| {
            // 1.10: an older name is taken where its name is (`spec::ARG_ALIASES`).
            let forms: Vec<(&'static Action, &'static Form)> = o
                .spec()
                .takers(spec::alias_of(name).unwrap_or(name))
                .into_iter()
                .filter(|(_, f, _)| !std::ptr::eq(*f, form))
                .map(|(a, f, _)| (a, f))
                .collect();
            (!forms.is_empty()).then(|| format!("{} {}", o.name(), name_forms(&forms).join(", ")))
        })
        .collect()
}

/// §21's hint for an argument of another action, where it names one (1.4's `talk said` for `at` in
/// a `to_user` is gone, since `to_user` takes `at`, 1.8, §8.1): for an argument of another form of
/// `talk ask`, that it picks that form, whose signature the hint's last lines give, and, for
/// `where` or `blocking` with `to: user`, how a person in a thread is asked (1.8, §20.7: it2 l1);
/// for `kind` on `talk say`, ignored since 1.10, where the outcome of one's own work goes; for `to`
/// on `talk to_user` (1.12), that its addressee is named by `principal`; for `supersedes` on
/// `talk ask` (1.12), how an open question to a person is withdrawn.
fn unknown_arg_hint(t: Tool, action: &Action, form: &Form, name: &str) -> &'static str {
    match (t, action.name, name) {
        (Tool::Talk, "ask", "where" | "blocking") if form.name == "human" => {
            "; it marks another form of ask: to ask a person in a thread, leave out to, and ask \
             records the said for you; each form's arguments follow"
        }
        (Tool::Talk, "ask", n) if spec::ASK_FORM_ARGS.contains(&n) => {
            "; it marks another form of ask: each form's arguments follow"
        }
        // 1.12 (§9.3, §21): `supersedes` is `decide`'s; an agent that passes it to `ask` wants its
        // open question gone. In the lab's seventh round (it6 l1), a worker that had asked the wrong
        // person was told only where `supersedes` belongs.
        (Tool::Talk, "ask", "supersedes") => {
            "; to withdraw your open question to a person: talk retract {entry: <question>, text, \
             reason}"
        }
        // 1.10 (§20.7): ignored, as no longer directing on `say`; in the lab's fifth round (it4
        // l7), workers with no human sent their result as `say {kind: result}`. 1.12: where the
        // `talk` description puts the outcome, not in a decision (it6 l7).
        (Tool::Talk, "say", "kind") => {
            "; a note to other agents has no kind: the outcome of your own work goes in close's \
             summary, or in a release's done while it waits for a merge"
        }
        // 1.12 (§20.7, §21): in the lab's seventh round (it6 l2), a worker passed `to:
        // principal:<key>`, as `say` takes it, and the warning named only the actions that take
        // `to`.
        (Tool::Talk, "to_user", "to") => {
            "; to_user names its addressee with principal: <key>, not to"
        }
        _ => "",
    }
}

/// Where an agent finds the value of a required argument it left out (1.5, R3, D.14.7): the
/// refusal names it beside the argument, so that the retry needs no other refusal. The table's
/// own `source` first, where the rules below would say too little (`talk decide`'s `text`); then
/// rules by name and kind. Never only the kind's noun, nor a one-word echo of the name ("why" for
/// `reason`, "the title" for `title`): a test holds every required argument to that.
fn source_of(a: &Arg) -> String {
    // A text: what to write, from the argument's description up to its first `;`.
    let described = || {
        let first = a.desc.split(';').next().unwrap_or(a.desc).trim();
        let mut c = first.chars();
        match c.next() {
            Some(f) => f.to_lowercase().chain(c).collect(),
            None => String::new(),
        }
    };
    if let Some(s) = a.source {
        return s.into();
    }
    match (a.name, a.kind) {
        ("version" | "digest_version", _) => "in the brief's header, from view node".into(),
        (_, Kind::Token) => "your hold's token, on the status line and in hello's holds".into(),
        ("said_id", _) => "the eN that talk said returned".into(),
        ("options", _) => "the choices to offer; [] for an open question".into(),
        ("origin", _) => "the message's id in its conversation, such as \
                          chat:<channel>/<thread>/<message>"
            .into(),
        ("reason", Kind::Text) => "why, in a sentence".into(),
        ("title", _) => "a short line that names the work".into(),
        ("run_within", _) => "how long after its slot each occurrence may run, a duration such \
                              as 2h"
            .into(),
        ("timeout", _) => "how long to wait, a duration such as 15m, at most talk.wait_max".into(),
        (_, Kind::Eid) => "an eN, from your inbox, hello, or the brief".into(),
        (_, Kind::Node) => {
            "a path or #<id>, as hello's offers, view find, and view list show them".into()
        }
        (_, Kind::Nodes) => {
            "paths or #<id>s, as hello's offers, view find, and view list show them".into()
        }
        (_, Kind::RouteTo) => "a path or #<id>, as hello's offers, view find, and view list show \
                               them, or none"
            .into(),
        (_, Kind::Place) => "the path where it goes".into(),
        (_, Kind::NodeAt) => "#<id>@<version>, the version from the brief's header".into(),
        (_, Kind::Key) => "a key such as chat:<channel>/<thread> or issue:<project>#<n>".into(),
        (_, Kind::Cron) => "a 5-field cron expression: minute, hour, day of month, month, day of \
                            week, such as 0 9 * * 1-5"
            .into(),
        (_, Kind::AskHuman) => "user, or principal:<key>".into(),
        (_, Kind::AskAgent) => "session:<id>, holder:<node>, or holders:<node>/**".into(),
        (_, Kind::Enum(values)) => format!("one of {}", values.join(", ")),
        (
            _,
            Kind::Text
            | Kind::TextList
            | Kind::Outcome
            | Kind::Bool
            | Kind::Object(_)
            | Kind::OneOf(_),
        ) => described(),
        _ => kind_noun(a.kind).into(),
    }
}

/// `a`, `a and b`, or `a, b, and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

/// What a call lacks: a required argument, or every argument of a `one_of` group.
enum Lack<'a> {
    /// A required argument.
    Arg(&'a Arg),
    /// A `one_of` group none of whose arguments the call gives.
    OneOf(Vec<&'a Arg>),
}

/// Every required argument the call lacks, and each `one_of` group of which it gives none: the
/// refusal names them all, each with where to find it (1.5, R3, D.14.7: "one of entry and path"),
/// so that one retry supplies them. `arg` lists them, a group as `entry|path`. A group of which
/// the call gives several is noted after this check, by `check_form`. Each gets a placeholder, so
/// that the build can check the rest (1.6).
fn check_required(
    args: &[Arg],
    map: &Map<String, Value>,
    at: &At,
    label: &str,
    one_of: &[&[&str]],
    found: &mut Found,
) {
    let mut missing: Vec<Lack<'_>> = args
        .iter()
        .filter(|a| a.required && !map.contains_key(a.name))
        .map(Lack::Arg)
        .collect();
    for group in one_of {
        if group.iter().all(|n| !map.contains_key(*n)) {
            missing.push(Lack::OneOf(
                group
                    .iter()
                    .filter_map(|n| args.iter().find(|a| a.name == *n))
                    .collect(),
            ));
        }
    }
    if missing.is_empty() {
        return;
    }
    let all = missing;
    let names: Vec<String> = all
        .iter()
        .map(|l| match l {
            Lack::Arg(a) => at.key(a.name).text(),
            Lack::OneOf(g) => g
                .iter()
                .map(|a| at.key(a.name).text())
                .collect::<Vec<_>>()
                .join("|"),
        })
        .collect();
    let requires = match all.as_slice() {
        [Lack::Arg(_)] => "it".to_string(),
        _ => and_list(
            &all.iter()
                .map(|l| match l {
                    Lack::Arg(a) => a.name.to_string(),
                    Lack::OneOf(g) => format!(
                        "one of {}",
                        g.iter().map(|a| a.name).collect::<Vec<_>>().join(" and ")
                    ),
                })
                .collect::<Vec<_>>(),
        ),
    };
    // Where to find each, each argument of a group included, those that share a source together.
    let mut groups: Vec<(Vec<&str>, String)> = Vec::new();
    let lacking = all.iter().flat_map(|l| match l {
        Lack::Arg(a) => vec![*a],
        Lack::OneOf(g) => g.clone(),
    });
    for a in lacking {
        let src = source_of(a);
        match groups.iter_mut().find(|(_, s)| *s == src) {
            Some((ns, _)) => ns.push(a.name),
            None => groups.push((vec![a.name], src)),
        }
    }
    let sources = match groups.as_slice() {
        [(ns, src)] if ns.len() == 1 => src.clone(),
        _ => groups
            .iter()
            .map(|(ns, src)| {
                let ns: Vec<String> = ns.iter().map(|n| (*n).to_string()).collect();
                format!("{}: {src}", and_list(&ns))
            })
            .collect::<Vec<_>>()
            .join("; "),
    };
    let fixes: Vec<Fix> = all
        .iter()
        .filter_map(|l| match l {
            Lack::Arg(a) => Some(*a),
            Lack::OneOf(g) => g.first().copied(),
        })
        .map(|a| (at.key(a.name), Some(placeholder(a.kind))))
        .collect();
    found.problem(
        Order::Missing,
        DecodeError::new(
            names.join(", "),
            format!("missing: {label} requires {requires}; {sources}"),
        ),
        fixes,
    );
}

/// An object argument: each of its arguments known and of its kind, each required one given.
/// `fix` repairs a value that is no object at all.
fn check_object(args: &'static [Arg], v: &Value, at: &At, found: &mut Found, fix: Option<Value>) {
    let Value::Object(map) = v else {
        found.problem(
            Order::Value,
            DecodeError::new(
                at.text(),
                format!("expected an object with {}", arg_names(args)),
            ),
            [(at.clone(), fix)],
        );
        return;
    };
    for (k, x) in by_name(map) {
        let here = at.key(k);
        let Some(a) = args.iter().find(|a| a.name == k) else {
            found.foreign(
                DecodeError::new(
                    here.text(),
                    format!(
                        "{k} is not an argument of {}, which takes {}",
                        at.text(),
                        arg_names(args)
                    ),
                ),
                &here,
            );
            continue;
        };
        check_value(a, x, &here, found);
    }
    check_required(args, map, at, &at.text(), &[], found);
}

fn expected(at: &str, what: &str, found: &Value) -> DecodeError {
    let found = match found {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    };
    DecodeError::new(at, format!("expected {what}, found {found}"))
}

fn check_value(a: &Arg, v: &Value, at: &At, found: &mut Found) {
    if v.is_null() {
        if !a.nullable {
            found.value(expected(&at.text(), kind_noun(a.kind), v), at, a);
        }
        return;
    }
    let string = |v: &Value| v.is_string();
    let strings = |v: &Value| {
        v.as_array()
            .is_some_and(|xs| xs.iter().all(Value::is_string))
    };
    let ok = match a.kind {
        Kind::Text
        | Kind::Cron
        | Kind::Zone
        | Kind::Node
        | Kind::Place
        | Kind::NodeAt
        | Kind::Key
        | Kind::Principal
        | Kind::Requester
        | Kind::Token
        | Kind::Eid
        | Kind::When
        | Kind::Checked
        | Kind::Label
        | Kind::Enum(_)
        | Kind::Addressee
        | Kind::AskHuman
        | Kind::AskAgent
        | Kind::RouteTo
        | Kind::Outcome
        | Kind::SessionId
        | Kind::OpId => string(v),
        Kind::Nodes | Kind::Keys | Kind::Eids | Kind::Labels | Kind::Enums(_) | Kind::TextList => {
            strings(v)
        }
        Kind::Seq | Kind::U64 => v.as_u64().is_some(),
        Kind::SeqOrEid => v.is_string() || v.as_u64().is_some(),
        Kind::U32 => v.as_u64().is_some_and(|n| u32::try_from(n).is_ok()),
        Kind::I32 => v.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        Kind::Dur => v.is_string() || v.as_u64().is_some(),
        Kind::Bool => v.is_boolean(),
        Kind::TextMap => v
            .as_object()
            .is_some_and(|m| m.values().all(Value::is_string)),
        Kind::Object(args) => return check_object(args, v, at, found, fix_of(a)),
        Kind::OneOf(variants) => return check_one_of(variants, v, at, found, a),
        Kind::Basis => return check_basis(v, at, found, a),
    };
    if !ok {
        found.value(expected(&at.text(), kind_noun(a.kind), v), at, a);
    }
}

/// What a kind's JSON value is, for hints.
pub(crate) fn kind_noun(k: Kind) -> &'static str {
    match k {
        Kind::Text
        | Kind::Cron
        | Kind::Zone
        | Kind::Key
        | Kind::Principal
        | Kind::Requester
        | Kind::Label
        | Kind::Enum(_)
        | Kind::Addressee
        | Kind::AskHuman
        | Kind::AskAgent
        | Kind::Outcome
        | Kind::OpId => "a string",
        Kind::Node | Kind::Place | Kind::RouteTo => "a string: a path or #<id>",
        Kind::NodeAt => "a string such as #12@4",
        Kind::Token => "a string such as 12.3",
        Kind::Eid => "a string such as e57",
        Kind::SessionId => "a string such as s-4",
        Kind::When => "a string: an RFC 3339 time, or in <duration>",
        Kind::Checked => "a string: now, a date such as 2026-10-04, or an RFC 3339 time",
        Kind::Nodes => "a list of paths or #<id>s",
        Kind::Keys => "a list of keys",
        Kind::Eids => "a list of eids such as e57",
        Kind::Labels => "a list of labels",
        Kind::Enums(_) => "a list of strings",
        Kind::TextList => "a list of strings",
        Kind::Seq | Kind::U64 => "a non-negative integer",
        Kind::SeqOrEid => "a seq, or an entry id such as e57",
        Kind::U32 => "an integer from 0 to 4294967295",
        Kind::I32 => "an integer from -2147483648 to 2147483647",
        Kind::Dur => "a duration such as 15m, or an integer of milliseconds",
        Kind::Bool => "true or false",
        Kind::TextMap => "an object of strings by name",
        Kind::Object(_) | Kind::OneOf(_) => "an object",
        Kind::Basis => "a list of references",
    }
}

/// `waits_for`: the keys of exactly one variant.
fn check_one_of(variants: &[&'static [Arg]], v: &Value, at: &At, found: &mut Found, a: &Arg) {
    let Value::Object(map) = v else {
        found.value(expected(&at.text(), "an object", v), at, a);
        return;
    };
    let firsts: Vec<&str> = variants
        .iter()
        .filter_map(|args| args.first().map(|a| a.name))
        .collect();
    let chosen: Vec<&&[Arg]> = variants
        .iter()
        .filter(|args| args.first().is_some_and(|a| map.contains_key(a.name)))
        .collect();
    match chosen.as_slice() {
        [args] => check_object(args, v, at, found, fix_of(a)),
        _ => found.value(
            DecodeError::new(
                at.text(),
                format!(
                    "expected an object with exactly one of {} (external with recheck_after)",
                    firsts.join(", ")
                ),
            ),
            at,
            a,
        ),
    }
}

/// `decide {basis}`: strings, or `{external, checked_at?, recheck?}`.
fn check_basis(v: &Value, at: &At, found: &mut Found, a: &Arg) {
    let Value::Array(items) = v else {
        found.value(expected(&at.text(), "a list of references", v), at, a);
        return;
    };
    for (i, item) in items.iter().enumerate() {
        let here = at.index(i);
        match item {
            Value::String(_) => {}
            Value::Object(_) => {
                check_object(BASIS_EXTERNAL_ARGS, item, &here, found, Some(json!("e1")));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => found.problem(
                Order::Value,
                expected(
                    &here.text(),
                    "a reference: a string, or {external, checked_at?, recheck?}",
                    item,
                ),
                [(here.clone(), Some(json!("e1")))],
            ),
        }
    }
}

/// Masks every free text of a call's own arguments with masking version `version` of `masker`;
/// how many texts changed.
fn mask_form(
    form: &Form,
    args: &mut Map<String, Value>,
    version: u32,
    masker: Masker,
) -> Result<u32, MaskError> {
    let m = |s: &str| masker(version, s).ok_or(MaskError::Version);
    mask_args(form.args, args, "", &m)
}

/// Masks one text; whether it changed.
fn mask_string(
    s: &mut String,
    m: &dyn Fn(&str) -> Result<String, MaskError>,
) -> Result<u32, MaskError> {
    let masked = m(s)?;
    if masked == *s {
        Ok(0)
    } else {
        *s = masked;
        Ok(1)
    }
}

fn mask_args(
    args: &[Arg],
    map: &mut Map<String, Value>,
    at: &str,
    m: &dyn Fn(&str) -> Result<String, MaskError>,
) -> Result<u32, MaskError> {
    let mut changed = 0;
    for a in args {
        if let Some(v) = map.get_mut(a.name) {
            changed += mask_value(a.kind, v, &fq(at, a.name), m)?;
        }
    }
    Ok(changed)
}

fn mask_value(
    kind: Kind,
    v: &mut Value,
    at: &str,
    m: &dyn Fn(&str) -> Result<String, MaskError>,
) -> Result<u32, MaskError> {
    match (kind, v) {
        (Kind::Text, Value::String(s)) => mask_string(s, m),
        (Kind::TextList, Value::Array(items)) => {
            let mut changed = 0;
            for item in items {
                if let Value::String(s) = item {
                    changed += mask_string(s, m)?;
                }
            }
            Ok(changed)
        }
        (Kind::TextMap, Value::Object(map)) => {
            // The names are free text too. Two that mask to the same text would leave one entry
            // where the agent sent two, silently: refused instead (review E2-bugs-5).
            let mut changed = 0;
            let mut out = Map::new();
            for (k, x) in std::mem::take(map) {
                let mut k = k;
                let mut x = x;
                changed += mask_string(&mut k, m)?;
                if let Value::String(s) = &mut x {
                    changed += mask_string(s, m)?;
                }
                if out.insert(k, x).is_some() {
                    return Err(MaskError::Collision(at.to_string()));
                }
            }
            *map = out;
            Ok(changed)
        }
        (Kind::Object(args), Value::Object(map)) => mask_args(args, map, at, m),
        (Kind::OneOf(variants), Value::Object(map)) => {
            let mut changed = 0;
            for args in variants {
                changed += mask_args(args, map, at, m)?;
            }
            Ok(changed)
        }
        (Kind::Basis, Value::Array(items)) => {
            let mut changed = 0;
            for (i, item) in items.iter_mut().enumerate() {
                match item {
                    Value::Object(map) => {
                        changed += mask_args(BASIS_EXTERNAL_ARGS, map, &format!("{at}[{i}]"), m)?;
                    }
                    // A string not written as a reference is free text since 1.6 (§15).
                    Value::String(s) if !values::written_as_reference(s) => {
                        changed += mask_string(s, m)?;
                    }
                    _ => {}
                }
            }
            Ok(changed)
        }
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests;
