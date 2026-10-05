//! The argument tables of the six tools (design D.11.5, §20.3 to §20.8): for every action, its
//! forms, and for every form, its arguments with their kind, whether each is required, and its
//! description.
//!
//! One table serves three readers, so that they cannot drift apart: the input schemas
//! (`mcp::schema`, one object per tool whose descriptions list each action's arguments, 1.5,
//! §20.2), the decoder's shape check (an argument the action's form does not take is ignored with
//! a warning, or, when it is directing or no action takes it, `invalid` naming it and the actions
//! that take it, 1.7; a missing required one is `invalid` naming it), and secret masking (every
//! argument of kind [`Kind::Text`], at any depth, is free text, D.11.13). Since 1.7 the table also
//! names the node each action is about ([`ABOUT`]), which any of [`NODE_ALIASES`] names too, and
//! the directing arguments ([`DIRECTING`]); since 1.8, the arguments that mark a form of `talk
//! ask` ([`ASK_FORM_ARGS`]) are refused, not ignored, on another form.

use crate::mcp::schema::{
    d_basis, d_checked_at, d_confirmed, d_defer, d_evidence, d_from_user_hello, d_from_user_node,
    d_intent, d_kind_answers, d_lease, d_list_status, d_needs, d_object_on, d_op_id, d_options,
    d_origin, d_re, d_repeat, d_run_within, d_said_kinds, d_session, d_since, d_stands,
    d_to_user_at, d_to_user_kinds, d_to_user_principal, d_unsent, d_verbatim, d_verified,
};

/// What an argument holds, and how it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Free text: masked, then a `Text`.
    Text,
    /// A cron expression (not free text: never masked).
    Cron,
    /// An IANA time-zone name.
    Zone,
    /// A node: a path or `#<id>`.
    Node,
    /// A place: a path only (`create {path}`, `move {to}`).
    Place,
    /// A node at a version: `#12@4`, `/a/b@4`.
    NodeAt,
    /// A list of nodes.
    Nodes,
    /// A key, normalised by `forum_keys::normalise`.
    Key,
    /// A list of keys.
    Keys,
    /// A principal key, normalised by `forum_keys::normalise_principal`.
    Principal,
    /// `me`, `all`, or a principal key (`view requests {requester}`).
    Requester,
    /// A hold token `12.3`.
    Token,
    /// An entry id `e57`.
    Eid,
    /// A list of entry ids.
    Eids,
    /// A feed sequence number.
    Seq,
    /// `view feed {since}`: a feed sequence number, or an entry id `e57`, the entries recorded
    /// after it (1.6, §20.4).
    SeqOrEid,
    /// A duration: `50s`, `15m`, `2h`, `7d`, or an integer of milliseconds.
    Dur,
    /// A time: RFC 3339, or `in <duration>`.
    When,
    /// When something outside the forum was checked (`checked_at`, 1.6, §15): `now`, a date
    /// `2026-10-04` (00:00 UTC), or a time as [`Kind::When`] reads it.
    Checked,
    /// A label.
    Label,
    /// A list of labels.
    Labels,
    /// An unsigned 32-bit integer.
    U32,
    /// An unsigned 64-bit integer.
    U64,
    /// A signed 32-bit integer.
    I32,
    /// A boolean.
    Bool,
    /// One of these values.
    Enum(&'static [&'static str]),
    /// A list of these values.
    Enums(&'static [&'static str]),
    /// An object of free texts by name (`fields`, `files`).
    TextMap,
    /// A list of free texts.
    TextList,
    /// An object with these arguments.
    Object(&'static [Arg]),
    /// An object with the arguments of exactly one of these variants (`waits_for`).
    OneOf(&'static [&'static [Arg]]),
    /// `decide {basis}`: a list of references, each a string or `{external, checked_at,
    /// recheck?}`.
    Basis,
    /// The `to` of `say`.
    Addressee,
    /// The `to` of a question for a human: `user` or `principal:<key>`.
    AskHuman,
    /// The `to` of a question for agents.
    AskAgent,
    /// The `to` of `from_user` and `route`: a node or `none` (1.7).
    RouteTo,
    /// A close's outcome: `done`, `failed`, `dropped`, `superseded`, `missed`, or a label.
    Outcome,
    /// The common `session` argument: a session id `s-4`.
    SessionId,
    /// The common `op_id` argument: 1 to 64 bytes of printable ASCII.
    OpId,
}

/// One argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arg {
    /// Its name.
    pub name: &'static str,
    /// What it holds.
    pub kind: Kind,
    /// Whether the form requires it.
    pub required: bool,
    /// Whether `null` is accepted, and clears the field (§20.5): `update`'s `kind`, `est`, `due`,
    /// and `repeat`, which `Change::Clear` clears, and its `body`, `fields`, `files`, `status`,
    /// `depends_on`, `owners`, and `agreements`, which their empty value clears.
    pub nullable: bool,
    /// Its description in the input schema.
    pub desc: &'static str,
    /// Where an agent finds its value, which a refusal for a missing argument names (1.5, R3,
    /// D.14.7), when the decoder's rules by name and kind would say too little: None for those.
    pub source: Option<&'static str>,
    /// Whether the input schema carries its description (1.7, §20.2): not where its name, its
    /// type, and the action's line say it all, such as a node a call is about, `title`, or `why`.
    /// A refusal still reads it.
    pub described: bool,
}

impl Arg {
    /// The argument with `source` as where to find its value.
    const fn found(self, source: &'static str) -> Arg {
        Arg {
            source: Some(source),
            ..self
        }
    }

    /// The argument without its description in the input schema (1.7).
    const fn quiet(self) -> Arg {
        Arg {
            described: false,
            ..self
        }
    }
}

const fn req(name: &'static str, kind: Kind, desc: &'static str) -> Arg {
    Arg {
        name,
        kind,
        required: true,
        nullable: false,
        desc,
        source: None,
        described: true,
    }
}

const fn opt(name: &'static str, kind: Kind, desc: &'static str) -> Arg {
    Arg {
        name,
        kind,
        required: false,
        nullable: false,
        desc,
        source: None,
        described: true,
    }
}

const fn clr(name: &'static str, kind: Kind, desc: &'static str) -> Arg {
    Arg {
        name,
        kind,
        required: false,
        nullable: true,
        desc,
        source: None,
        described: true,
    }
}

/// One form of an action: most actions have one; `talk ask` has three, and `talk said` two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Form {
    /// Its name, for hints: `human`, `person`, `agent`, `record`, `confirm`, or the action's.
    pub name: &'static str,
    /// How the hint names the form.
    pub label: &'static str,
    /// When a call takes this form, for an action with several (`to user or principal:<key>`);
    /// empty for an action's only form. The input schema names the form as `<action>
    /// (<name>: <when>)` in the description of `action`, and as `<action> (<name>)` elsewhere.
    pub when: &'static str,
    /// Its arguments, beside `action`, `session`, and `op_id`.
    pub args: &'static [Arg],
    /// Groups of arguments of which exactly one must be given (`path | paths`).
    pub one_of: &'static [&'static [&'static str]],
}

/// How the decoder picks an action's form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// The action has one form.
    Single,
    /// `talk ask`: `to` of `user` or `principal:<key>` is the form for a human, another `to` the
    /// form for agents, and no `to` the form for a person.
    AskTo,
    /// `talk said`: `said_id` is the confirmation.
    SaidId,
}

impl Select {
    /// The argument that picks the form, whose description in the input schema lists the forms
    /// with their arguments (D.11.5): `to` for `talk ask`, `said_id` for `talk said`.
    #[must_use]
    pub fn selector(self) -> Option<&'static str> {
        match self {
            Select::Single => None,
            Select::AskTo => Some("to"),
            Select::SaidId => Some("said_id"),
        }
    }
}

/// One action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    /// Its name.
    pub name: &'static str,
    /// Its forms.
    pub forms: &'static [Form],
    /// How a call picks its form.
    pub select: Select,
}

/// One tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolSpec {
    /// Its name.
    pub name: &'static str,
    /// Its actions, in §20's order.
    pub actions: &'static [Action],
}

/// The arguments every action takes beside its own.
pub const COMMON: [Arg; 2] = [
    opt("session", Kind::SessionId, d_session!()),
    opt("op_id", Kind::OpId, d_op_id!()),
];

// ------------------------------------------------------------------------------------------------
// Values

/// `view node {view}`.
pub const VIEW_MODES: &[&str] = &["brief", "full", "review"];
/// §6's entry types.
pub const ENTRY_TYPES: &[&str] = &[
    "note",
    "request",
    "question",
    "answer",
    "decision",
    "checkpoint",
    "said",
    "heard",
    "from_user",
    "to_user",
    "approve",
    "object",
    "resolve",
    "retract",
    "system",
    "late",
];
/// §14's flags.
pub const FLAGS: &[&str] = &[
    "unrouted",
    "unanswered_request",
    "open_commitment",
    "unconfirmed_said",
    "open_intent",
    "question_recheck",
    "proceed_on_assumption",
    "objection",
    "basis_changed",
    "too_big",
    "overdue",
    "ready_to_close",
    "missed",
    "stale",
];
/// §8.3's board states.
pub const BOARD_STATES: &[&str] = &[
    "unrouted",
    "answered",
    "closed_unanswered",
    "decided",
    "waiting",
    "in_progress",
];
/// §16's link relations.
pub const LINK_RELS: &[&str] = &[
    "continues",
    "derived_from",
    "conflicts_with",
    "duplicate",
    "superseded_by",
];
/// §10.3's release reasons.
pub const RELEASE_REASONS: &[&str] = &["handoff", "blocked", "cannot"];
/// §8.1's report kinds.
pub const TO_USER_KINDS: &[&str] = &["ack", "question", "result"];
/// §9.2's `said` kinds (a question goes through `talk ask`).
pub const SAID_KINDS: &[&str] = &["ack", "reply", "result", "correction"];
/// §13's `expects`.
pub const EXPECTS: &[&str] = &["none", "ack", "reply"];
/// §15's evidence.
pub const EVIDENCE: &[&str] = &["attempt", "observation", "inference", "reported"];

// ------------------------------------------------------------------------------------------------
// Nested objects

/// `hello {from_user}`.
pub const FROM_USER_ARGS: &[Arg] = &[
    req("text", Kind::Text, VERBATIM),
    req("client", Kind::Text, "Such as slack, ide, or mail"),
    opt(
        "principal",
        Kind::Principal,
        "Whose words, as a principal key such as mail:alice@corp.example; by default the \
         session's principal",
    )
    .quiet(),
    // 1.12 (§20.2): quiet, for the tool list's bound, as `talk from_user`'s say it: the words a
    // `hello` logs are the human's first, and the lab's 1,955 calls passed these 0 to 4 times.
    opt("answers", Kind::Eid, ANSWERS).quiet(),
    opt("to", Kind::RouteTo, d_from_user_node!()).quiet(),
    opt("node", Kind::Node, d_from_user_node!()).quiet(),
    opt("clarifies", Kind::Eid, CLARIFIES).quiet(),
    opt("reason", Kind::Text, ROUTE_REASON).quiet(),
];

/// `create {request}`.
pub const REQUEST_ARGS: &[Arg] = &[
    req("text", Kind::Text, VERBATIM),
    opt("origin", Kind::Key, d_origin!()),
    opt(
        "where",
        Kind::Key,
        "By default the first conversation key among keys, else the origin",
    ),
    req("requester", Kind::Text, "Who asked, by name"),
];

/// `repeat`.
pub const REPEAT_ARGS: &[Arg] = &[
    req(
        "cron",
        Kind::Cron,
        "5 fields: minute, hour, day of month, month, day of week, such as 0 9 * * 1-5",
    ),
    opt(
        "tz",
        Kind::Zone,
        "An IANA time zone, such as Europe/Berlin; UTC by default",
    ),
    req("run_within", Kind::Dur, d_run_within!()),
];

/// `next {query}`.
pub const QUERY_ARGS: &[Arg] = &[
    opt("under", Kind::Node, "Only nodes in this subtree"),
    opt("kind", Kind::Label, "Only nodes of this kind").quiet(),
    opt(
        "tags",
        Kind::Labels,
        "Only nodes that carry every one of these tags",
    )
    .quiet(),
];

/// `hide {file}`.
pub const HIDE_FILE_ARGS: &[Arg] = &[
    req("path", Kind::Node, "The node").quiet(),
    req("name", Kind::Text, "The file's name").quiet(),
];

/// `waits_for`, one variant per kind of wait.
pub const WAITS_FOR: &[&[Arg]] = &[
    &[req(
        "question",
        Kind::Eid,
        "The eN of a question you asked with talk ask",
    )],
    &[req("node", Kind::Node, "A node").quiet()],
    &[req("time", Kind::When, "A time").quiet()],
    &[
        req("external", Kind::Text, "Something outside the forum"),
        req("recheck_after", Kind::When, "When to look again"),
    ],
];

/// `decide {basis}`: an external reference.
pub const BASIS_EXTERNAL_ARGS: &[Arg] = &[
    req(
        "external",
        Kind::Text,
        "Something outside the forum, such as an artefact",
    ),
    opt("checked_at", Kind::Checked, d_checked_at!()),
    opt("recheck", Kind::When, "When to check it again"),
];

// ------------------------------------------------------------------------------------------------
// Shared argument descriptions

const TOKEN: &str = "Your hold's token, such as 12.3";
const TOKEN_IF_HELD: &str = "Your hold's token, if you hold the node";
/// `unsent` (1.10, §9.2): in the lab's fifth round (it4 l1), a post sent in the reply after the
/// release was listed as "not sent", and became unconfirmed.
const UNSENT: &str = d_unsent!();
const LEARNINGS: &str = "What the next agent should know";
const DONE: &str = "What you did since the last checkpoint";
const NEXT_STEP: &str = "What comes next, so that a stranger could take over";
const AT: &str = "The node";
const WHERE: &str = "The conversation, as a key such as chat:<channel>/<thread>; needed when the nodes from at up \
     carry several";
const PAGE_LIMIT: &str = "Per page: 50 by default";
const FIELDS: &str = "Named values, as text; update replaces them all";
const DEPENDS_ON: &str = "Nodes that must close first; update replaces them all";
const OWNERS: &str = "Person or team nodes; else the nearest owned ancestor's; update replaces \
     them all";
const AGREEMENTS_TEXT: &str = "How work is done in this subtree; update replaces them";
const CAPACITY: &str = "Sessions that may hold it at once: 1 by default, 0 for a record";
const PRIORITY: &str = "An integer, higher first: 0 by default";
const INTENT_DONE: &str = "What became of the node's open intent: whether the action happened";
// 1.11 (§20.2): quiet, for the tool list's bound, as are `why` of `renew` and `tags` of `next`'s
// `query`: none of the lab's 1,703 calls passed them.
const AGAIN: &str = "Ask again although the same question is pending";
const VERBATIM: &str = "The words, verbatim";
const ENTRY: &str = "An entry, such as e57";
const ANSWERS: &str = "The question these words answer, such as e9";
const CLARIFIES: &str = "The request these words follow up, such as e57";
/// The `reason` of a routing to none (1.7, §8.2).
const ROUTE_REASON: &str = "Why no work is needed, with to: none";

// ------------------------------------------------------------------------------------------------
// The forms

const HELLO: &[Arg] = &[
    opt(
        "intent",
        Kind::Text,
        "What you mean to do in this session; earlier related work is searched for it",
    ),
    opt(
        "caps",
        Kind::Labels,
        "What you can do, such as shell or sql; work that needs other caps is not offered",
    ),
    opt(
        "horizon",
        Kind::U32,
        "Minutes you have left; longer work is not offered",
    ),
    opt(
        "principal",
        Kind::Principal,
        "The human you serve, as a principal key such as mail:alice@corp.example; binds this \
         session to them",
    ),
    opt(
        "from_user",
        Kind::Object(FROM_USER_ARGS),
        d_from_user_hello!(),
    ),
];
const STATUS: &[Arg] = &[req(
    "text",
    Kind::Text,
    "Your session's status: what you are doing now",
)
.quiet()];

const VIEW_NODE: &[Arg] = &[
    opt("path", Kind::Node, AT).quiet(),
    opt("paths", Kind::Nodes, "Several nodes").quiet(),
    opt(
        "view",
        Kind::Enum(VIEW_MODES),
        "full adds every field, the feed, and what rests on the node; review leaves out the \
         author's reasoning",
    ),
    opt(
        "page",
        Kind::U32,
        "In view full, the page of the lists of what rests on the node, from 0",
    )
    .quiet(),
];
const FIND: &[Arg] = &[
    opt(
        "key",
        Kind::Key,
        "A key: the node that carries it, open first, then closed ones, newest first",
    )
    .quiet(),
    opt(
        "text",
        Kind::Text,
        "Words: up to 20 nodes and entries, by relevance",
    )
    .quiet(),
];
const FEED: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    opt("since", Kind::SeqOrEid, d_since!()),
    opt(
        "types",
        Kind::Enums(ENTRY_TYPES),
        "Only entries of these types",
    )
    .quiet(),
    opt("limit", Kind::U32, PAGE_LIMIT),
];
const LIST: &[Arg] = &[
    opt("under", Kind::Node, "The subtree; / by default"),
    opt("kind", Kind::Label, "Only nodes of this kind").quiet(),
    // 1.11 (§20.2): quiet, for the tool list's bound, as are `requester` of `requests`, and
    // `capacity` and `agreements` of `create` and `update`: none of the lab's 1,703 calls passed
    // them.
    opt(
        "tags",
        Kind::Labels,
        "Only nodes that carry every one of these tags",
    )
    .quiet(),
    opt("status", Kind::Text, d_list_status!()),
    opt(
        "fields",
        Kind::TextMap,
        "Only nodes whose fields have these values",
    )
    .quiet(),
    opt(
        "flags",
        Kind::Enums(FLAGS),
        "Only nodes that carry every one of these flags",
    )
    .quiet(),
    opt(
        "closed",
        Kind::Bool,
        "Whether to list closed nodes; by default, open ones and those not yet archived",
    ),
    opt("limit", Kind::U32, PAGE_LIMIT),
];
const AGREEMENTS: &[Arg] = &[
    opt("under", Kind::Node, "The subtree; / by default"),
    opt("limit", Kind::U32, PAGE_LIMIT),
];
const HISTORY: &[Arg] = &[
    req("path", Kind::Node, AT).quiet(),
    opt(
        "before",
        Kind::U64,
        "Only events before this seq; newest first",
    )
    .quiet(),
    opt("limit", Kind::U32, PAGE_LIMIT),
];
const REQUESTS: &[Arg] = &[
    opt(
        "status",
        Kind::Enum(BOARD_STATES),
        "Only requests in this state",
    )
    .quiet(),
    opt(
        "requester",
        Kind::Requester,
        "me (your principals', the default if you serve one), all, or a principal key",
    )
    .quiet(),
];
const USER_LOG: &[Arg] = &[
    opt("since", Kind::Eid, d_since!()),
    opt("limit", Kind::U32, PAGE_LIMIT),
];

const CREATE: &[Arg] = &[
    req("path", Kind::Place, "Where the new node goes"),
    opt(
        "keys",
        Kind::Keys,
        "Such as issue:<project>#<n> or chat:<channel>/<thread>: an open node that carries one is \
         joined instead",
    ),
    opt("kind", Kind::Label, "The node's kind, a label").quiet(),
    req("title", Kind::Text, "The title").quiet(),
    opt("body", Kind::Text, "What the work is"),
    opt(
        "digest",
        Kind::Text,
        "A summary that agents keep current, at most digest.max bytes",
    ),
    opt("fields", Kind::TextMap, FIELDS),
    opt(
        "tags",
        Kind::Labels,
        "Labels; offer offers the node to next",
    ),
    opt("capacity", Kind::U32, CAPACITY).quiet(),
    opt(
        "needs",
        Kind::Labels,
        concat!(d_needs!(), "; without it, the parent's; [] for none"),
    ),
    opt("est", Kind::U32, "Estimated minutes of work"),
    opt("priority", Kind::I32, PRIORITY),
    opt("due", Kind::When, "When it is due").quiet(),
    opt("depends_on", Kind::Nodes, DEPENDS_ON),
    opt("owners", Kind::Nodes, OWNERS),
    opt("repeat", Kind::Object(REPEAT_ARGS), d_repeat!()),
    opt("agreements", Kind::Text, AGREEMENTS_TEXT).quiet(),
    opt(
        "request",
        Kind::Object(REQUEST_ARGS),
        "A request to record with the node",
    )
    .quiet(),
    opt(
        "take",
        Kind::Bool,
        "Take a hold on the new node: true by default",
    ),
];
const UPDATE: &[Arg] = &[
    req("path", Kind::Node, AT).quiet(),
    opt(
        "version",
        Kind::U64,
        "From the brief you were shown; by default the one last shown to you",
    ),
    opt(
        "digest_version",
        Kind::U64,
        "From the brief you were shown, checked with digest; by default the one last shown to you",
    ),
    opt("title", Kind::Text, "The title").quiet(),
    clr("body", Kind::Text, "What the work is"),
    opt(
        "digest",
        Kind::Text,
        "A summary that agents keep current, at most digest.max bytes",
    ),
    clr("fields", Kind::TextMap, FIELDS),
    clr(
        "files",
        Kind::TextMap,
        "Artefacts outside the forum by name: URLs or paths, sha256:<hex> optional; update replaces \
         them all",
    ),
    clr("kind", Kind::Label, "The node's kind, a label").quiet(),
    clr("status", Kind::Text, "A free-text status"),
    opt("capacity", Kind::U32, CAPACITY).quiet(),
    opt("priority", Kind::I32, PRIORITY),
    clr("est", Kind::U32, "Estimated minutes of work"),
    clr("due", Kind::When, "When it is due").quiet(),
    clr("depends_on", Kind::Nodes, DEPENDS_ON),
    clr("owners", Kind::Nodes, OWNERS),
    clr("repeat", Kind::Object(REPEAT_ARGS), d_repeat!()),
    clr("agreements", Kind::Text, AGREEMENTS_TEXT).quiet(),
    opt("tags_add", Kind::Labels, "Tags to add, after the removals").quiet(),
    opt("tags_remove", Kind::Labels, "Tags to remove").quiet(),
    opt(
        "needs_add",
        Kind::Labels,
        concat!(d_needs!(), "; added after the removals"),
    ),
    opt(
        "needs_remove",
        Kind::Labels,
        "Cap labels to remove from needs",
    ),
    opt("token", Kind::Token, TOKEN_IF_HELD),
];
const LINK: &[Arg] = &[
    req("from", Kind::Node, "The node the link goes from").quiet(),
    req(
        "rel",
        Kind::Enum(LINK_RELS),
        "continues: a later attempt at the same key; derived_from; conflicts_with and duplicate \
         open a signal; superseded_by",
    ),
    req("to", Kind::Node, "The target").quiet(),
];
const KEY: &[Arg] = &[
    req("path", Kind::Node, AT).quiet(),
    opt("add", Kind::Key, "A key to add").quiet(),
    opt("remove", Kind::Key, "A key to remove").quiet(),
];
const CLOSE: &[Arg] = &[
    req("path", Kind::Node, AT).quiet(),
    req(
        "outcome",
        Kind::Outcome,
        "One label: done, failed, dropped, superseded, refuted, or another; a sentence goes in \
         summary",
    ),
    req(
        "summary",
        Kind::Text,
        "What came of it, for whoever comes next",
    ),
    opt("verified", Kind::Text, d_verified!()),
    opt(
        "settles",
        Kind::Eids,
        "The requests this close settles, such as e57",
    ),
    opt("learnings", Kind::Text, LEARNINGS).quiet(),
    opt("token", Kind::Token, TOKEN_IF_HELD),
    opt("unsent", Kind::Eids, UNSENT),
];
const REOPEN: &[Arg] = &[
    req("path", Kind::Node, AT).quiet(),
    req("reason", Kind::Text, "Why").quiet(),
];
const MOVE: &[Arg] = &[
    req("path", Kind::Node, AT).quiet(),
    req("to", Kind::Place, "The new path"),
];
const MERGE: &[Arg] = &[
    req("from", Kind::Node, "The node merged away"),
    req("into", Kind::Node, "The node that remains"),
];
const HIDE: &[Arg] = &[
    opt(
        "entry",
        Kind::Eid,
        "An entry, such as e57, a checkpoint's files included",
    ),
    opt(
        "file",
        Kind::Object(HIDE_FILE_ARGS),
        "A file of a node: {path, name}",
    )
    .quiet(),
    req("reason", Kind::Text, "Why; hiding affects rendering only"),
];

const TAKE: &[Arg] = &[
    req("target", Kind::Node, "The node to take").quiet(),
    opt("lease", Kind::Dur, d_lease!()),
];
const NEXT: &[Arg] = &[
    opt(
        "query",
        Kind::Object(QUERY_ARGS),
        "A filter: {under?, kind?, tags?}",
    )
    .quiet(),
    opt("lease", Kind::Dur, d_lease!()),
];
const CHECKPOINT: &[Arg] = &[
    req("token", Kind::Token, TOKEN),
    req("done", Kind::Text, DONE),
    req("next_step", Kind::Text, NEXT_STEP),
    opt("intent", Kind::Text, d_intent!()),
    opt("intent_done", Kind::Text, INTENT_DONE),
    opt(
        "files",
        Kind::TextList,
        "The step's files, as references: part of the checkpoint's text",
    )
    .quiet(),
];
const RENEW: &[Arg] = &[
    req("token", Kind::Token, TOKEN),
    req(
        "why",
        Kind::Text,
        "Why you need more time without a checkpoint",
    )
    .quiet(),
];
const RELEASE: &[Arg] = &[
    req("token", Kind::Token, TOKEN),
    req("done", Kind::Text, DONE),
    req("next_step", Kind::Text, NEXT_STEP),
    // 1.10 (§10.3): handoff offers the node at once; in the lab's fifth round (it4 l7), a handoff
    // after a PR was opened offered finished work to a second worker.
    req(
        "reason",
        Kind::Enum(RELEASE_REASONS),
        "handoff: someone else should continue now; blocked: it waits for something, such as a \
         review or merge; cannot: you cannot do it",
    ),
    opt(
        "waits_for",
        Kind::OneOf(WAITS_FOR),
        "{question}, {node}, {time}, or {external, recheck_after}; the reason is then blocked",
    ),
    opt("intent_done", Kind::Text, INTENT_DONE),
    // 1.12 (§10.3): as a close's, recorded as a decision after the release; quiet, for the tool
    // list's bound (the `hold` description says what it is for).
    opt("settles", Kind::Eids, "The requests it settles").quiet(),
    opt("learnings", Kind::Text, LEARNINGS).quiet(),
    opt("unsent", Kind::Eids, UNSENT),
];
const CHECK: &[Arg] = &[
    req("target", Kind::Node, "The node to check").quiet(),
    opt("token", Kind::Token, TOKEN),
    opt("intent_done", Kind::Text, INTENT_DONE),
];

const FROM_USER: &[Arg] = &[
    req("text", Kind::Text, VERBATIM),
    req("client", Kind::Text, "Such as slack, ide, or mail"),
    opt(
        "principal",
        Kind::Principal,
        "Whose words, as a principal key such as mail:alice@corp.example",
    )
    .quiet(),
    opt("answers", Kind::Eid, ANSWERS),
    opt("to", Kind::RouteTo, d_from_user_node!()),
    opt("node", Kind::Node, d_from_user_node!()).quiet(),
    opt("clarifies", Kind::Eid, CLARIFIES),
    opt("reason", Kind::Text, ROUTE_REASON),
];
const TO_USER: &[Arg] = &[
    req("text", Kind::Text, "What you tell the human").quiet(),
    // 1.11 (§20.7, §21): a refusal for a missing kind says what each of to_user's kinds means.
    req("kind", Kind::Enum(TO_USER_KINDS), d_kind_answers!()).found(d_to_user_kinds!()),
    opt("verbatim", Kind::Bool, d_verbatim!()),
    opt("principal", Kind::Principal, d_to_user_principal!()),
    opt("re", Kind::Eids, d_re!()),
    // 1.8 (§8.1): the node whose work it reports, whose requester it is to.
    opt("at", Kind::Node, d_to_user_at!()),
];
const ROUTE: &[Arg] = &[
    req("entry", Kind::Eid, ENTRY),
    opt("to", Kind::RouteTo, d_from_user_node!()),
    opt("answers", Kind::Eid, ANSWERS),
    opt("clarifies", Kind::Eid, CLARIFIES),
    opt("reason", Kind::Text, ROUTE_REASON),
];
const REQUEST: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    req("text", Kind::Text, VERBATIM),
    req("origin", Kind::Key, d_origin!()),
    opt("where", Kind::Key, WHERE),
    req("requester", Kind::Text, "Who asked, by name"),
];
const SAID_RECORD: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    opt("where", Kind::Key, WHERE),
    req("text", Kind::Text, VERBATIM),
    // 1.9 (§9.2, §21): a refusal for a missing kind says what each kind means.
    req("kind", Kind::Enum(SAID_KINDS), d_kind_answers!()).found(d_said_kinds!()),
    // 1.10 (§20.2): quiet, for the tool list's bound: their names say them, and the lab's 1,615
    // calls passed `corrects` once and `seen` never.
    opt("corrects", Kind::Eid, "The said entry this one corrects").quiet(),
    opt(
        "seen",
        Kind::Seq,
        "The last seq of the node's feed that you read",
    )
    .quiet(),
];
const SAID_CONFIRM: &[Arg] = &[
    req("said_id", Kind::Eid, "The said entry you sent, such as e8"),
    req("confirmed", Kind::Text, d_confirmed!()),
];
const HEARD: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    opt("where", Kind::Key, WHERE),
    req("from", Kind::Text, "Who said it"),
    req("text", Kind::Text, VERBATIM),
    opt("answers", Kind::Eid, ANSWERS),
];
const ASK_HUMAN: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    req("text", Kind::Text, "The question").quiet(),
    req(
        "to",
        Kind::AskHuman,
        "user: the first human of at's chain; principal:<key>: that human",
    )
    .quiet(),
    req("options", Kind::TextList, d_options!()),
    opt("recommendation", Kind::Text, "The option you recommend").quiet(),
    // 1.12 (§20.2): quiet, for the tool list's bound, as are `for` of `wait`, `path` of
    // `retract`, and `after` and `limit` of `export`: none of the lab's 1,955 calls passed them.
    opt(
        "default",
        Kind::Text,
        "Assumed if nobody answers in time: a reversible choice",
    )
    .quiet(),
    opt("due", Kind::When, "When the answer is needed").quiet(),
    opt("again", Kind::Bool, AGAIN).quiet(),
];
const ASK_PERSON: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    req("text", Kind::Text, "The question").quiet(),
    opt("where", Kind::Key, WHERE),
    opt("again", Kind::Bool, AGAIN).quiet(),
    opt(
        "blocking",
        Kind::Bool,
        "Whether the work cannot go on without the answer",
    ),
];
const ASK_AGENT: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    req("text", Kind::Text, "The question").quiet(),
    req(
        "to",
        Kind::AskAgent,
        "session:<id>, holder:<node>, or holders:<node>/**",
    )
    .quiet(),
    opt("again", Kind::Bool, AGAIN).quiet(),
    opt(
        "blocking",
        Kind::Bool,
        "Whether the work cannot go on without the answer",
    ),
];
const ANSWER: &[Arg] = &[
    req("question", Kind::Eid, "The agent question, such as e9"),
    req("text", Kind::Text, "The answer").quiet(),
];
const SAY: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    req("text", Kind::Text, "The message").quiet(),
    // 1.12 (§13, §20.7): a closed node has no next holder; a note there without `to` goes to the
    // session that closed it while it is live (1.11), which it6 l6's "the next holder" hid.
    opt(
        "to",
        Kind::Addressee,
        "session:<id>, holder:<node>, holders:<node>/**, principal:<key>, or user; without to: \
         at's holders or its next holder; at a closed node, the session that closed it while live",
    ),
    opt(
        "expects",
        Kind::Enum(EXPECTS),
        "A later release: only none means anything; talk ask gets an answer",
    ),
    // 1.10 (§20.2): quiet, for the tool list's bound: the lab's 1,615 calls passed neither.
    opt("reply_to", Kind::Eid, "The entry this replies to").quiet(),
    opt(
        "until",
        Kind::When,
        "Delivered until then: deliver.window by default",
    )
    .quiet(),
];
const DECIDE: &[Arg] = &[
    req("at", Kind::Node, AT).quiet(),
    // 1.12 (§20.2): a choice, or what settles requests, as the `talk` description says; not the
    // outcome of one's own work, which goes in a close's summary or a release's done (it6 l7).
    req(
        "text",
        Kind::Text,
        "A choice, or what settles the requests; findings and open questions go in a checkpoint",
    )
    .found("the choice made, or what settles the requests, in a sentence or two"),
    opt(
        "settles",
        Kind::Eids,
        "The requests it settles, such as e57",
    ),
    // 1.10 (§20.2): quiet, for the tool list's bound: its name says it.
    opt("supersedes", Kind::Eid, "The decision it replaces").quiet(),
    opt("basis", Kind::Basis, d_basis!()),
    opt("evidence", Kind::Enum(EVIDENCE), d_evidence!()),
];
const APPROVE: &[Arg] = &[
    req(
        "on",
        Kind::NodeAt,
        "The node at the version you checked, such as #12@4",
    ),
    req("checked", Kind::Text, "What you checked").quiet(),
];
const OBJECT: &[Arg] = &[
    req("on", Kind::Node, d_object_on!()),
    req("text", Kind::Text, "The problem").quiet(),
];
const RESOLVE: &[Arg] = &[
    req("objection", Kind::Eid, "The objection, such as e12"),
    req("text", Kind::Text, "How it was resolved").quiet(),
    // 1.10 (§20.7): the verdict, `upheld` before, which a reviewer who found a claim true read as
    // "the claim is upheld" (it4 l5); `upheld` is still read as it ([`ARG_ALIASES`]).
    opt("stands", Kind::Bool, d_stands!()),
    // 1.11 (§11.3): no verdict yet; never with `stands` (it5 l5). 1.12: described (it6 l5).
    opt("defer", Kind::Bool, d_defer!()),
];
const RETRACT: &[Arg] = &[
    opt("entry", Kind::Eid, ENTRY),
    opt("path", Kind::Node, "A node to strike out").quiet(),
    req("text", Kind::Text, "The statement retracted").quiet(),
    req("reason", Kind::Text, "Why").quiet(),
];
const WAIT: &[Arg] = &[
    req(
        "timeout",
        Kind::Dur,
        "How long to wait, at most talk.wait_max",
    ),
    opt("for", Kind::Node, "Also wake on new entries of this node").quiet(),
];
const WHO: &[Arg] = &[opt(
    "at",
    Kind::Node,
    "Only sessions at work in this subtree",
)];

const PAUSE: &[Arg] = &[
    req("scope", Kind::Node, "The subtree"),
    req("reason", Kind::Text, "Why: every agent reads it")
        .found("why, in a sentence: every agent reads it"),
];
const UNPAUSE: &[Arg] = &[req("scope", Kind::Node, "The subtree")];
// 1.12: quiet, for the tool list's bound when `hold release` took `settles`: no agent of the lab's
// seven rounds paged an export.
const EXPORT: &[Arg] = &[
    opt("after", Kind::U64, "Events after this seq").quiet(),
    opt("limit", Kind::U32, "Events per page: 1,000 by default").quiet(),
];

const NONE: &[Arg] = &[];

macro_rules! form {
    ($name:literal, $args:expr) => {
        Form {
            name: $name,
            label: $name,
            when: "",
            args: $args,
            one_of: &[],
        }
    };
    ($name:literal, $args:expr, $one_of:expr) => {
        Form {
            name: $name,
            label: $name,
            when: "",
            args: $args,
            one_of: $one_of,
        }
    };
}

macro_rules! action {
    ($name:literal, $args:expr) => {
        Action {
            name: $name,
            forms: &[form!($name, $args)],
            select: Select::Single,
        }
    };
    ($name:literal, $args:expr, $one_of:expr) => {
        Action {
            name: $name,
            forms: &[form!($name, $args, $one_of)],
            select: Select::Single,
        }
    };
}

/// The six tools.
pub const TOOLS: [ToolSpec; 6] = [
    ToolSpec {
        name: "session",
        actions: &[action!("hello", HELLO), action!("status", STATUS)],
    },
    ToolSpec {
        name: "view",
        actions: &[
            action!("node", VIEW_NODE, &[&["path", "paths"]]),
            action!("find", FIND, &[&["key", "text"]]),
            action!("feed", FEED),
            action!("list", LIST),
            action!("history", HISTORY),
            action!("requests", REQUESTS),
            action!("user_log", USER_LOG),
            action!("agreements", AGREEMENTS),
            action!("constitution", NONE),
        ],
    },
    ToolSpec {
        name: "node",
        actions: &[
            action!("create", CREATE),
            action!("update", UPDATE),
            action!("link", LINK),
            action!("key", KEY, &[&["add", "remove"]]),
            action!("close", CLOSE),
            action!("reopen", REOPEN),
            action!("move", MOVE),
            action!("merge", MERGE),
            action!("hide", HIDE, &[&["entry", "file"]]),
        ],
    },
    ToolSpec {
        name: "hold",
        actions: &[
            action!("take", TAKE),
            action!("next", NEXT),
            action!("checkpoint", CHECKPOINT),
            action!("renew", RENEW),
            action!("release", RELEASE),
            action!("check", CHECK),
        ],
    },
    ToolSpec {
        name: "talk",
        actions: &[
            action!("from_user", FROM_USER),
            action!("to_user", TO_USER),
            action!("route", ROUTE, &[&["to", "answers", "clarifies"]]),
            action!("request", REQUEST),
            Action {
                name: "said",
                forms: &[
                    Form {
                        name: "record",
                        label: "talk said, recording what you are about to send",
                        when: "without said_id, before you send",
                        args: SAID_RECORD,
                        one_of: &[],
                    },
                    Form {
                        name: "confirm",
                        label: "talk said, confirming a said entry as sent",
                        when: "with said_id, after you sent it",
                        args: SAID_CONFIRM,
                        one_of: &[],
                    },
                ],
                select: Select::SaidId,
            },
            action!("heard", HEARD),
            Action {
                name: "ask",
                forms: &[
                    Form {
                        name: "human",
                        label: "talk ask for a human (to: user or principal:<key>)",
                        when: "to user or principal:<key>",
                        args: ASK_HUMAN,
                        one_of: &[],
                    },
                    Form {
                        name: "person",
                        label: "talk ask for a person outside the swarm (no to)",
                        when: "no to, a person outside the swarm",
                        args: ASK_PERSON,
                        one_of: &[],
                    },
                    Form {
                        name: "agent",
                        label: "talk ask for agents (to: session:, holder:, or holders:)",
                        when: "to session:, holder:, or holders:",
                        args: ASK_AGENT,
                        one_of: &[],
                    },
                ],
                select: Select::AskTo,
            },
            action!("answer", ANSWER),
            action!("say", SAY),
            action!("decide", DECIDE),
            action!("approve", APPROVE),
            action!("object", OBJECT),
            action!("resolve", RESOLVE),
            action!("retract", RETRACT, &[&["entry", "path"]]),
            action!("wait", WAIT),
            action!("who", WHO),
        ],
    },
    ToolSpec {
        name: "control",
        actions: &[
            action!("pause", PAUSE),
            action!("unpause", UNPAUSE),
            action!("health", NONE),
            action!("constitution", NONE),
            action!("export", EXPORT),
        ],
    },
];

/// The tool named `name`.
#[must_use]
pub fn tool(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.name == name)
}

// ------------------------------------------------------------------------------------------------
// The node a call is about, and the arguments a call may not lose (1.7)

/// The names under which an action takes the one node it is about (1.7, §20.2): agents guessed
/// each other action's name for it (`merge {path}`, `route {node}`, `link {from}`, `to_user {at}`).
pub const NODE_ALIASES: [&str; 7] = ["path", "at", "on", "target", "under", "scope", "node"];

/// The actions about exactly one node, by tool and action, with the name each documents for it
/// (1.7, §20.2): any of [`NODE_ALIASES`] names that node too. Not a place (`create {path}`, `move
/// {to}`), not two nodes (`merge {from, into}`, `link {to}`), not the nodes of `depends_on` and
/// `owners`, and not a routing target (`from_user {to}`, read from `node` too, and `route {to}`).
pub const ABOUT: &[(&str, &str, &str)] = &[
    ("view", "node", "path"),
    ("view", "feed", "at"),
    ("view", "list", "under"),
    ("view", "history", "path"),
    ("view", "agreements", "under"),
    ("node", "update", "path"),
    ("node", "link", "from"),
    ("node", "key", "path"),
    ("node", "close", "path"),
    ("node", "reopen", "path"),
    ("node", "move", "path"),
    ("hold", "take", "target"),
    ("hold", "check", "target"),
    ("talk", "request", "at"),
    ("talk", "said", "at"),
    ("talk", "heard", "at"),
    ("talk", "ask", "at"),
    ("talk", "say", "at"),
    ("talk", "decide", "at"),
    ("talk", "to_user", "at"),
    ("talk", "approve", "on"),
    ("talk", "object", "on"),
    ("talk", "retract", "path"),
    ("talk", "wait", "for"),
    ("talk", "who", "at"),
    ("control", "pause", "scope"),
    ("control", "unpause", "scope"),
];

/// The name under which `form` of `action` of `tool` documents the node it is about, if it is
/// about one (1.7).
#[must_use]
pub fn about(tool: &str, action: &str, form: &Form) -> Option<&'static str> {
    ABOUT
        .iter()
        .find(|(t, a, _)| *t == tool && *a == action)
        .map(|(_, _, name)| *name)
        .filter(|name| form.arg(name).is_some())
}

/// The arguments whose silent drop would change what is recorded (1.7, §21): given to an action
/// that does not take them, they stay `invalid`; any other argument of another action is ignored,
/// with the warning `ignored_arguments`. `evidence` qualifies a decision, so it is directing only
/// where a decision is recorded without it ([`directing`]).
pub const DIRECTING: [&str; 20] = [
    "request",
    "intent",
    "intent_done",
    "waits_for",
    "settles",
    "supersedes",
    "answers",
    "clarifies",
    "outcome",
    "summary",
    "unsent",
    "reply_to",
    "token",
    "version",
    "digest_version",
    "principal",
    "kind",
    "verbatim",
    "re",
    "basis",
];

/// The arguments that mark a form of `talk ask` (1.8, §20.7): `to`, `options`, `recommendation`,
/// `default`, and `due` of the form for a human, `where` and `blocking` of the form for a person,
/// and `blocking` of the form for agents. Given to a form that does not take it, such an argument
/// says that the call meant another form, and is `invalid`, as in 1.6: 1.7 ignored it, and a
/// question meant for a person in a thread went to the human (it2, l1 and l6).
pub const ASK_FORM_ARGS: [&str; 7] = [
    "to",
    "options",
    "recommendation",
    "default",
    "due",
    "where",
    "blocking",
];

/// Whether `name`, given to `action` of `tool`, which does not take it, is directing (1.7, §21):
/// one of [`DIRECTING`], `evidence` on `node close`, whose summary is the decision of what it
/// settles, or an argument of another form of `talk ask` (1.8, [`ASK_FORM_ARGS`]). Not `kind` on
/// `talk say` (1.10): a note to agents has no kind, and loses nothing without one; in the lab's
/// fifth round (it4 l7), four workers with no human sent their result as `say {kind: result}`,
/// were refused, and sent it again as a note.
#[must_use]
pub fn directing(tool: &str, action: &str, name: &str) -> bool {
    (DIRECTING.contains(&name) && !(name == "kind" && (tool, action) == ("talk", "say")))
        || (name == "evidence" && (tool, action) == ("node", "close"))
        || ((tool, action) == ("talk", "ask") && ASK_FORM_ARGS.contains(&name))
}

/// Arguments read under an older name (1.10, §20.2), by tool and action: `(tool, action, alias,
/// name)`. The alias is renamed to the name before the shape check, as a node's other names are
/// (1.7); the schema lists only the name. `upheld` of `talk resolve` is now `stands`: a reviewer
/// who found a claim true read `upheld: true` as "the claim is upheld" (it4 l5).
pub const ARG_ALIASES: &[(&str, &str, &str, &str)] = &[("talk", "resolve", "upheld", "stands")];

/// The name that `alias` stands for in some action (1.10): the forms that take the name take the
/// alias too, so a refusal names them for it.
#[must_use]
pub fn alias_of(alias: &str) -> Option<&'static str> {
    ARG_ALIASES
        .iter()
        .find(|(_, _, a, _)| *a == alias)
        .map(|(_, _, _, name)| *name)
}

impl ToolSpec {
    /// The action named `name`.
    #[must_use]
    pub fn action(&self, name: &str) -> Option<&'static Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    /// The names of its actions.
    #[must_use]
    pub fn action_names(&self) -> Vec<&'static str> {
        self.actions.iter().map(|a| a.name).collect()
    }

    /// The arguments of its actions, each once, in the order the table first names them.
    #[must_use]
    pub fn arg_names(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for a in self.forms().flat_map(|(_, f)| f.args) {
            if !out.contains(&a.name) {
                out.push(a.name);
            }
        }
        out
    }

    /// Every form of every action, in the table's order.
    pub fn forms(&self) -> impl Iterator<Item = (&'static Action, &'static Form)> {
        self.actions
            .iter()
            .flat_map(|a| a.forms.iter().map(move |f| (a, f)))
    }

    /// The forms that take the argument `name`, in the table's order, each with the argument.
    #[must_use]
    pub fn takers(&self, name: &str) -> Vec<(&'static Action, &'static Form, &'static Arg)> {
        self.forms()
            .filter_map(|(a, f)| f.arg(name).map(|x| (a, f, x)))
            .collect()
    }
}

impl Action {
    /// The form named `name`.
    #[must_use]
    pub fn form(&self, name: &str) -> Option<&'static Form> {
        self.forms.iter().find(|f| f.name == name)
    }

    /// How the input schema and the hints name some of the action's forms: the action's name when
    /// they are all of its forms, else `<action> (<form>, …)`, such as `ask (human, agent)` (1.5,
    /// §20.2).
    #[must_use]
    pub fn forms_ref(&self, forms: &[&Form]) -> String {
        if forms.len() == self.forms.len() {
            self.name.to_string()
        } else {
            let names: Vec<&str> = forms.iter().map(|f| f.name).collect();
            format!("{} ({})", self.name, names.join(", "))
        }
    }

    /// How the description of `action` names one form: the action's name for its only form, else
    /// `<action> (<form>: <when>)`, such as `ask (human: to user or principal:<key>)`.
    #[must_use]
    pub fn form_title(&self, form: &Form) -> String {
        if self.forms.len() == 1 {
            self.name.to_string()
        } else {
            format!("{} ({}: {})", self.name, form.name, form.when)
        }
    }
}

/// The forms of `forms`, named action by action in the order given: `say, ask (human, agent)`
/// (1.5, §20.2).
#[must_use]
pub fn name_forms(forms: &[(&'static Action, &'static Form)]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some((action, _)) = forms.get(i) {
        let group: Vec<&Form> = forms
            .get(i..)
            .unwrap_or_default()
            .iter()
            .take_while(|(a, _)| a.name == action.name)
            .map(|(_, f)| *f)
            .collect();
        i += group.len();
        out.push(action.forms_ref(&group));
    }
    out
}

impl Form {
    /// The argument named `name`.
    #[must_use]
    pub fn arg(&self, name: &str) -> Option<&'static Arg> {
        self.args.iter().find(|a| a.name == name)
    }

    /// Its arguments as the description of `action` and a refusal's signature list them (1.5,
    /// 1.6, §20.2): in the table's order, `token*` for a required one, `path|paths` for a group of
    /// which exactly one is given (at its first member's place), `no arguments` for none.
    #[must_use]
    pub fn arg_list(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut groups_done: Vec<&[&str]> = Vec::new();
        for a in self.args {
            match self.one_of.iter().find(|g| g.contains(&a.name)) {
                Some(g) => {
                    if !groups_done.contains(g) {
                        groups_done.push(g);
                        parts.push(g.join("|"));
                    }
                }
                None if a.required => parts.push(format!("{}*", a.name)),
                None => parts.push(a.name.to_string()),
            }
        }
        if parts.is_empty() {
            "no arguments".to_string()
        } else {
            parts.join(", ")
        }
    }
}
