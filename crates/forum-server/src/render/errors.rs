//! Errors and warnings (§21, design D.6.3, D.6.4, D.11.7): `error: <code>` and `hint: <text>`
//! rendered from the structured `Hint`, and `warnings: [{code, id?, detail, entries?}]` from each
//! `Warning`.
//!
//! The hint is the shell's text: §21 fixes the remedy, not the words. A node argument that is a
//! key or an entry id gets the hint of 1.4 (R5): what was given, and where it belongs.

use std::collections::BTreeMap;

use forum_core::query::guide::NEXT_IDS_MAX;
use forum_core::query::{PAGE_DEFAULT, find_key};
use forum_core::{
    AddresseeArg, AgentAddressee, AskTo, Author, Cmd, Eid, EntryBody, Error, ErrorCode, Expects,
    Field, Hint, HoldCmd, KeyStr, LimitKind, NoHoldWhy, NodeCmd, NodeId, PEOPLE, PrincipalId,
    PrincipalKey, QuestionBody, ROOT, Requester, Role, SessionId, State, TalkCmd, TextField, Time,
    Warning, WarningCode, WarningDetail, What, Why, fnv1a64,
};

use super::format::{cut, node_ref, principal, shown, time, total_json};
use super::json::Json;
use super::source::Source;
use crate::decode::NodeArgText;

/// A principal key quoted in a warning is cut to this many bytes.
const QUOTED_MAX: usize = 64;

/// §21's name of an error code.
#[must_use]
pub fn code_name(c: ErrorCode) -> &'static str {
    match c {
        ErrorCode::Full => "full",
        ErrorCode::HoldsLimit => "holds_limit",
        ErrorCode::Paused => "paused",
        ErrorCode::StaleToken => "stale_token",
        ErrorCode::StaleVersion => "stale_version",
        ErrorCode::Cycle => "cycle",
        ErrorCode::Closed => "closed",
        ErrorCode::OpIdConflict => "op_id_conflict",
        ErrorCode::Limit => "limit",
        ErrorCode::Invalid => "invalid",
        ErrorCode::NotFound => "not_found",
    }
}

/// §21's name of a warning code.
#[must_use]
pub fn warning_name(c: WarningCode) -> &'static str {
    match c {
        WarningCode::Moved => "moved",
        WarningCode::UnsettledRequests => "unsettled_requests",
        WarningCode::OpenCommitments => "open_commitments",
        WarningCode::OpenObjections => "open_objections",
        WarningCode::Agreement => "agreement",
        WarningCode::ConversationHeld => "conversation_held",
        WarningCode::HeldByOther => "held_by_other",
        WarningCode::NoHold => "no_hold",
        WarningCode::UncoveredSlot => "uncovered_slot",
        WarningCode::DuplicateKey => "duplicate_key",
        WarningCode::SecretMasked => "secret_masked",
        WarningCode::UserSaturated => "user_saturated",
        WarningCode::SessionOpened => "session_opened",
        WarningCode::OpenIntent => "open_intent",
        WarningCode::ExpectsIgnored => "expects_ignored",
        WarningCode::SeriesStopped => "series_stopped",
        WarningCode::SelfBasisDropped => "self_basis_dropped",
        WarningCode::SupersededBasisDropped => "superseded_basis_dropped",
        WarningCode::ResultAnsweredNothing => "result_answered_nothing",
        WarningCode::Waiting => "waiting",
        WarningCode::UnnamedUser => "unnamed_user",
        WarningCode::IgnoredArguments => "ignored_arguments",
        WarningCode::NoHumanInReach => "no_human_in_reach",
        WarningCode::ClientContext => "client_context",
        WarningCode::UnmetNeeds => "unmet_needs",
        WarningCode::NoBasis => "no_basis",
        WarningCode::Paused => "paused",
        WarningCode::FollowUp => "follow_up",
        WarningCode::NoRecipient => "no_recipient",
        WarningCode::AddresseeGone => "addressee_gone",
        WarningCode::AnsweredNothing => "answered_nothing",
    }
}

fn field_name(f: Field) -> &'static str {
    match f {
        Field::Title => "title",
        Field::Summary => "summary",
        Field::NextStep => "next_step",
        Field::Text => "text",
        Field::Reason => "reason",
    }
}

fn ids<T>(xs: &[T], f: impl Fn(&T) -> String) -> String {
    xs.iter().map(f).collect::<Vec<_>>().join(", ")
}

/// The text of `invalid(why)`.
#[must_use]
pub fn invalid_text(s: &State, why: Why) -> String {
    let p = &s.params;
    match why {
        Why::NameTaken => "a live node already has that name there: pick another name, or find \
                           the existing node with view find"
            .into(),
        Why::ReservedNode => "that is a reserved node: it cannot be moved, merged, closed, given \
                              a key, a capacity, or a repeat, routed to, or hold a conversation"
            .into(),
        Why::ReservedKey => "repeat: keys are the server's own: pass another key".into(),
        Why::AlreadyClosed => "the node is already closed".into(),
        Why::NotClosed => "the node is open: only a closed node reopens".into(),
        Why::OccurrenceReopen => "the series of this occurrence is open and still recurring: \
                                  its occurrences are neither reopened nor taken"
            .into(),
        Why::OccurrenceNotYetAvailable => "the occurrence's slot has not come: it opens at its \
                                           slot"
            .into(),
        Why::SelfLink => "a node cannot link to itself".into(),
        Why::SelfMerge => "a node cannot be merged into itself".into(),
        Why::MergeFromClosed => {
            "the node to merge from is closed: reopen it first, or link it".into()
        }
        Why::MergeIntoOwnOccurrence => "a series cannot be merged into its own open occurrence: \
                                        closing the series would drop it"
            .into(),
        Why::IntentOpen => "an intent is already open on the node: check whether its action \
                            happened, close it with intent_done in the same checkpoint, and pass \
                            intent only if your own action is still to be done"
            .into(),
        Why::NoIntent => "no intent is open on the node, so there is nothing for intent_done to \
                          close: leave it out"
            .into(),
        Why::NoConversationKey => "no node from at up carries a conversation key: pass where, \
                                   such as chat:<channel>/<thread>"
            .into(),
        Why::AmbiguousConversationKey => "the conversation node carries several conversation \
                                          keys: pass where to name one"
            .into(),
        Why::NoRequestOrigin => "a request needs an origin: pass request.origin, or keys".into(),
        Why::ScheduleNeverFires => {
            "the schedule has no slot within 3,000 days (about 8 years) of now".into()
        }
        Why::BadTimeZone => "the time zone's table is not valid".into(),
        Why::RunWithinZero => "repeat.run_within must be longer than 0".into(),
        Why::EmptyText(f) => format!("{} is empty: write it", field_name(f)),
        Why::DigestTooLong => format!(
            "digest is longer than digest.max ({} bytes): shorten it",
            p.digest_max
        ),
        Why::MissingDigestVersion => "pass digest_version from view node".into(),
        // 1.8 (§21, it2 l1): the form of ask for a person in a thread, which records the said.
        Why::SaidKindQuestion => "a question to a person goes through talk ask with no to: talk \
                                  ask {at, where?, text, blocking?} records the said for you"
            .into(),
        Why::NotPeopleQuestion => "that is not a person's question: answer an agent's with talk \
                                   answer, and log the user's answer with talk from_user {answers}"
            .into(),
        Why::NotAgentQuestion => "that is not an agent's question: record a person's answer with \
                                  talk heard {answers}, and the user's with talk from_user \
                                  {answers}"
            .into(),
        Why::NotUserQuestion => "that is not a question for the user: record a person's answer \
                                 with talk heard {answers}, and answer an agent's with talk answer"
            .into(),
        Why::NotUserEntry => "talk route takes a from_user entry".into(),
        Why::NotReportable => "re names requests, from_user entries, and digest entries".into(),
        Why::DecisionNotActive => "supersedes names a decision that is superseded or retracted: \
                                   supersede the active one"
            .into(),
        Why::RoleConflict => "repeat cannot be set on an occurrence or a signal node".into(),
        Why::ServerOutcome => "missed is the server's own outcome: close with done, failed, \
                               dropped, or superseded"
            .into(),
        Why::ClassMismatch => "the session belongs to another class: open your own with session \
                               hello"
            .into(),
        Why::VersionAhead => {
            "that version is newer than the node's: re-read it with view node".into()
        }
        Why::CheckedAtAhead => "an external basis reference's checked_at lies after now: give \
                                the time it was last checked, now or earlier"
            .into(),
        Why::AmbiguousPrincipal => "this session serves several persons: pass principal".into(),
        // 1.9 (§8.1, §21): the lab's workers logged their scheduler's prompt as from_user.
        Why::NoPrincipal => "from_user is for a person who wrote to you directly, named by \
                             principal; a scheduler's prompt, a tool's output, or your client's \
                             instructions are not: leave from_user out. If your client serves a \
                             person it does not name, pass principal: user"
            .into(),
        Why::BadParams => bad_params_text(None, false),
    }
}

/// The core packages whose commands the core still answers with `invalid(BadParams)`, the
/// placeholder of a package that has not landed (D.13.1, D.13.2). When a package lands, its name
/// leaves this list and its commands leave [`stub_of`]; the render tests check both against the
/// core, so neither can outlive the stub. WP-D6, the last package with a command stub (`node
/// create` with `repeat`, `node update` of `repeat`), has landed: none is left.
pub const STUB_PACKAGES: [&str; 0] = [];

/// A command the core answers with the placeholder of a later package (D.13.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stub {
    /// The call, as the hint names it: `talk decide`, `node create with repeat`.
    pub call: &'static str,
    /// The core package that brings it.
    pub package: &'static str,
    /// For a stub that one argument makes (`session hello` with `from_user`, `node create` with
    /// `repeat`, `node update` of `repeat`), the call that works without it, which the hint names;
    /// None when no other arguments help.
    pub instead: Option<&'static str>,
}

/// The stub a command meets: the call and the core package of a command the core answers with the
/// placeholder of a later package, `invalid(BadParams)` (D.13.1): the shell can tell, so the hint
/// says which (1.5, R3, D.14.7). In round R3, `talk decide`'s placeholder read as a real refusal of
/// the arguments, and agents rewrote it two to five times.
/// None remains since WP-D6 landed ([`STUB_PACKAGES`]).
#[must_use]
pub fn stub_of(_cmd: &Cmd) -> Option<Stub> {
    None
}

/// The hint of `invalid(BadParams)` for `cmd` (1.5, R3, D.14.7), a call that `opened` its session
/// or not. For a stub of a core package ([`stub_of`]) it names the call and the package, and keeps
/// the words "does not do this yet", which the usability harness and the simulation read; it says
/// what was applied (nothing, or only the opening of the session, which the `session_opened`
/// warning of a failed first call names), and either the call that works without the argument
/// that makes the stub, or that other arguments will not help. Otherwise it says that the
/// arguments do not fit the call, adding "or this server does not do this yet" only while some
/// package is still a stub ([`STUB_PACKAGES`]). It no longer cites "a rule of the constitution",
/// which explained no such refusal.
#[must_use]
pub fn bad_params_text(cmd: Option<&Cmd>, opened: bool) -> String {
    match cmd.and_then(stub_of) {
        Some(Stub {
            call,
            package,
            instead,
        }) => format!(
            "this server does not do this yet: {call} comes with the core's package {package}, \
             which this server lacks; {}; {}",
            if opened {
                "apart from opening your session, nothing was applied"
            } else {
                "nothing was applied"
            },
            instead.unwrap_or("other arguments will not help")
        ),
        None if STUB_PACKAGES.is_empty() => {
            "these arguments do not fit this call: check them against the tool's description".into()
        }
        None => "these arguments do not fit this call, or this server does not do this yet: \
                 check them against the tool's description"
            .into(),
    }
}

/// The text of `not_found(what)`.
#[must_use]
pub fn not_found_text(what: What) -> &'static str {
    match what {
        What::Node => "no such node",
        What::Session => {
            "unknown or retired session (sessions retire after session.retain \
                          without a call); open a new one with session hello"
        }
        What::Entry => "no such entry",
        What::Request => "no such request",
        What::Question => "no such question",
        What::Said => "no such said entry",
        What::Decision => "no such decision",
        What::Objection => "no such objection",
        What::Pause => "no pause is active there",
        What::Key => "the node does not carry that key",
        What::UserEntry => "no such from_user entry",
        What::Addressee => "to names an unknown or retired session; your own session is unaffected",
    }
}

/// The session a `say` or an agent `ask` names in `to` (1.4, §21, R12): what
/// `not_found(Addressee)` names.
fn addressee(cmd: &Cmd) -> Option<SessionId> {
    match cmd {
        Cmd::Talk(
            TalkCmd::Say {
                to: Some(AddresseeArg::Session(x)),
                ..
            }
            | TalkCmd::Ask {
                to: AskTo::Agent(AgentAddressee::Session(x)),
                ..
            },
        ) => Some(*x),
        _ => None,
    }
}

/// The text of `limit(kind)`.
#[must_use]
pub fn limit_text(s: &State, k: LimitKind) -> String {
    let p = &s.params;
    match k {
        LimitKind::Renew => format!(
            "renew.max ({}) renews without a checkpoint: checkpoint instead",
            p.renew_max
        ),
        LimitKind::Depth => format!(
            "the node would lie deeper than tree.max_depth ({})",
            p.tree_max_depth
        ),
        LimitKind::Children => format!(
            "the parent would have more than tree.max_children ({}) open children",
            p.tree_max_children
        ),
        LimitKind::WritesPerHour => format!(
            "write.per_hour ({}) writes in the last hour; nothing was applied: write less often",
            p.write_per_hour
        ),
        LimitKind::Busy => "the server is busy; nothing was applied; retry in a few seconds with \
                            the same op_id"
            .into(),
        LimitKind::TextTooLong => format!(
            "a free-text argument is longer than text.max ({} bytes); nothing was applied: put a \
             large artefact in a file outside the forum and pass a reference to it",
            p.text_max
        ),
    }
}

/// The newest checkpoint among the node's newest `PAGE_DEFAULT` entries that closed an intent
/// (`intent_done`): the node is the one of the token `cmd` names. What §21's hint names for an
/// `intent_done` with no intent open.
fn intent_closer(cmd: &Cmd, src: &dyn Source) -> Option<Eid> {
    let token = match cmd {
        Cmd::Hold(HoldCmd::Checkpoint { token, .. } | HoldCmd::Release { token, .. }) => *token,
        _ => return None,
    };
    src.newest(token.node, PAGE_DEFAULT)
        .into_iter()
        .find(|v| {
            matches!(v.body, EntryBody::Checkpoint { .. })
                && v.extra(TextField::IntentDone).is_some()
        })
        .map(|v| v.eid)
}

/// The hint of an error (§21), the outcome of `cmd`, a call that `opened` its session or not, with
/// R5's hint for a key or an entry id given where a node goes, and, for an `intent_done` with no
/// intent open, the checkpoint that closed the last one.
#[must_use]
pub fn hint(
    s: &State,
    e: &Error,
    cmd: &Cmd,
    node_args: &[NodeArgText],
    src: &dyn Source,
    opened: bool,
) -> String {
    let base = match &e.hint {
        Hint::Invalid(Why::NoIntent) => match intent_closer(cmd, src) {
            Some(c) => format!(
                "no intent is open on the node: {} closed the last one, so there is nothing for \
                 intent_done to close: leave it out",
                c.render()
            ),
            None => invalid_text(s, Why::NoIntent),
        },
        Hint::Full {
            node,
            capacity,
            live,
        } => format!(
            "{} is full: {live} of its {capacity} slots are held",
            node_ref(s, *node)
        ),
        Hint::HoldsLimit { holds } => format!(
            "you already hold hold.per_session ({}) nodes ({}): release one first",
            s.params.hold_per_session,
            ids(holds, |t| t.render())
        ),
        Hint::Paused { scope, until } => {
            let by = s
                .pauses
                .get(scope)
                .map(|p| format!(" by {}/{}", p.class, p.by.render()))
                .unwrap_or_default();
            let reason = src.pause_reason(*scope).map(|t| t.0).unwrap_or_default();
            format!(
                "paused at {}{by} until {}: {}",
                node_ref(s, *scope),
                time(*until),
                cut(&reason, super::status::PAUSE_REASON_MAX)
            )
        }
        Hint::StaleToken { token, late } => format!(
            "your hold {} ended; the write was saved as a late entry ({}); take the node again \
             and re-read",
            token.render(),
            late.render()
        ),
        Hint::StaleVersion {
            current,
            digest_current,
        } => format!(
            "re-read with view node and retry with version {} (and digest version {})",
            current.0, digest_current.0
        ),
        Hint::Cycle { path } => format!(
            "it would close a cycle: {}",
            ids(path, |n| n.render()).replace(", ", " → ")
        ),
        Hint::Closed { node, merged_into } => match merged_into {
            Some(m) => format!(
                "{} was merged into {}: work on that node",
                node.render(),
                node_ref(s, *m)
            ),
            None => match (
                s.nodes.get(node).map(|x| &x.life),
                forum_core::query::review_objection(s, *node),
            ) {
                // 1.8 (§10.1, §21): an upheld objection corrected it; it is not open work.
                (Some(forum_core::Life::Closed(c)), _) if c.upheld.is_some() => format!(
                    "{} is closed {}: an objection upheld on it corrected its outcome, so it is \
                     not offered as work; reopen it with node reopen if the work must be done again",
                    node_ref(s, *node),
                    super::format::outcome(&c.outcome)
                ),
                // 1.9 (§10.1, §21): an open objection is decided by its review, not by a take.
                // 1.10: the verdict is `stands`.
                (Some(forum_core::Life::Closed(c)), Some(o)) => {
                    format!(
                        "{} is closed {} with open objection {}: decide whether it stands with talk \
                         resolve {{objection: {}, text, stands}}; node reopen only if its work must \
                         be redone",
                        node_ref(s, *node),
                        super::format::outcome(&c.outcome),
                        o.render(),
                        o.render()
                    )
                }
                _ => format!(
                    "{} is closed and carries no flag: reopen it with node reopen if it must change",
                    node_ref(s, *node)
                ),
            },
        },
        Hint::OpIdConflict => "this op_id was used for another request, or by another session, \
                               within op.retain: use a new op_id"
            .into(),
        Hint::Limit(k) => limit_text(s, *k),
        Hint::Invalid(Why::BadParams) => bad_params_text(Some(cmd), opened),
        Hint::Invalid(Why::NameTaken) => {
            name_taken(s, cmd).unwrap_or_else(|| invalid_text(s, Why::NameTaken))
        }
        Hint::Invalid(w) => invalid_text(s, *w),
        // 1.4 (§21, R12): the hint names `to`, and the caller's own session is unaffected.
        Hint::NotFound(What::Addressee) => match addressee(cmd) {
            Some(x) => format!(
                "to names an unknown or retired session ({}); your own session is unaffected",
                x.render()
            ),
            None => not_found_text(What::Addressee).into(),
        },
        Hint::NotFound(w) => not_found_text(*w).into(),
    };
    let node_refused = matches!(e.hint, Hint::NotFound(What::Node));
    match node_refused.then(|| key_hint(s, node_args, src)).flatten() {
        Some(k) => format!("{base}; {k}"),
        None => base,
    }
}

/// §21's hint for a create or a move onto the path of a live node (1.8, it2 l6: "pick another
/// name" sent the lead to a duplicate of the incident's node): the node there, its state and
/// holders, and, for a create, the calls that take it and give it the create's keys; None when the
/// path names no node but the root.
fn name_taken(s: &State, cmd: &Cmd) -> Option<String> {
    let (path, keys) = match cmd {
        Cmd::Node(NodeCmd::Create(a)) => (&a.path, Some(&a.keys)),
        Cmd::Node(NodeCmd::Move { to, .. }) => (to, None),
        _ => return None,
    };
    let n = forum_core::query::resolve(s, &forum_core::NodeRef::Path(path.clone()))
        .filter(|n| *n != ROOT)?;
    let node = s.nodes.get(&n)?;
    let holders = forum_core::query::holders(s, n, s.last_now);
    let held = if holders.is_empty() {
        if node.is_open() {
            ", unheld".to_string()
        } else {
            String::new()
        }
    } else {
        format!(", held by {}", ids(&holders, |(x, _)| x.render()))
    };
    let id = n.render();
    let there = format!(
        "{} is already {id} \"{}\", {}{held}",
        super::format::path(s, n),
        cut(&node.title.0, NAME_TAKEN_TITLE_MAX),
        super::entries::row_state(s, n),
    );
    Some(match keys {
        Some(keys) if node.is_open() => {
            // A `user:<entry>` key added so also routes the entry to the node (§8.2).
            let key = match keys.as_slice() {
                [] => String::new(),
                [k] => format!(
                    ", and give it your key: node key {{path: {id}, add: {}}}",
                    k.key.as_str()
                ),
                more => format!(
                    ", and give it your keys, one node key {{path: {id}, add: <key>}} each: {}",
                    ids(more, |k| k.key.as_str().to_string())
                ),
            };
            format!("{there}: if it is your work, take it: hold take {{target: {id}}}{key}")
        }
        Some(_) => format!(
            "{there}: to continue it, node reopen {{path: {id}, reason}}; for other work, a path \
             of its own"
        ),
        None => format!(
            "{there}: move to another path, or, if it is the same work, node merge {{from, into: \
             {id}}}"
        ),
    })
}

/// A title quoted in the hint of a name already taken is cut to this many bytes.
const NAME_TAKEN_TITLE_MAX: usize = 80;

/// Whether `arg` is `owners` or one of its items.
fn is_owners_arg(arg: &str) -> bool {
    arg == "owners" || arg.starts_with("owners[")
}

/// Whether `arg` is `re` or one of its items.
#[must_use]
pub fn is_re_arg(arg: &str) -> bool {
    arg == "re" || arg.starts_with("re[")
}

/// §21: "the person's node, for a principal in `owners`": the person node that carries
/// `person:<key>`, or how to make one.
fn principal_hint(s: &State, pk: &PrincipalKey) -> String {
    let person = s
        .principal_ids
        .get(pk)
        .and_then(|p| s.ix.person_of.get(p))
        .copied()
        .or_else(|| {
            KeyStr::new(&format!("person:{}", pk.as_str())).and_then(|k| find_key(s, &k).open)
        });
    match person {
        Some(m) => format!(
            "{0} is a principal: pass their person node {1} ({2}) in owners",
            pk.as_str(),
            super::format::path(s, m),
            m.render()
        ),
        None => format!(
            "{0} is a principal, and no person node carries person:{0}: create one under {1} \
             with that key, then pass it in owners",
            pk.as_str(),
            super::format::path(s, PEOPLE)
        ),
    }
}

/// §21: "a node's unsettled requests, for a node named in `re`": when the raw text of the `re`
/// item the decoder refused names a node, its unsettled requests, at most `NEXT_IDS_MAX`, as the
/// ids to pass. None when it names no node.
#[must_use]
pub fn re_node_hint(s: &State, arg: &str, raw: &str) -> Option<String> {
    let n = crate::decode::values::node(raw.trim())
        .ok()
        .and_then(|r| forum_core::query::resolve(s, &r))?;
    let unsettled: Vec<String> =
        s.ix.items
            .get(&n)
            .into_iter()
            .flat_map(|i| i.unsettled.iter())
            .take(NEXT_IDS_MAX)
            .map(|r| r.render())
            .collect();
    let all = s.ix.items.get(&n).map_or(0, |i| i.unsettled.len());
    let at = super::format::node_ref(s, n);
    Some(if unsettled.is_empty() {
        format!("{arg} names the node {at}, which has no unsettled request: re takes entry ids")
    } else {
        let more = if all > unsettled.len() {
            format!(" (the first {} of {all})", unsettled.len())
        } else {
            String::new()
        };
        format!(
            "{arg} names the node {at}; its unsettled requests{more} are {}: pass those in re",
            unsettled.join(", ")
        )
    })
}

/// R5 (1.4, §21, D.11.7): for each node argument whose raw text reads as a key or as an entry id,
/// what was given and where it belongs, and, for a principal key in `owners`, the person's node;
/// for a `basis` item that names no node, the forms of a basis (1.5, R3, D.14.7), since a bare
/// path is a node reference there and any line of prose decodes as one. None when no argument
/// reads so.
#[must_use]
pub fn key_hint(s: &State, node_args: &[NodeArgText], src: &dyn Source) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for a in node_args {
        let raw = a.raw.trim();
        if is_owners_arg(&a.arg)
            && let Ok(pk) = crate::decode::values::principal(raw)
        {
            out.push(principal_hint(s, &pk));
            continue;
        }
        if let Some(e) = forum_core::Eid::parse(raw) {
            let kind = src.entry(e).map(|v| match v.body {
                EntryBody::Decision { .. } => format!(
                    "{raw} is a decision: to change it, talk decide {{supersedes: {raw}}}, or talk \
                     retract"
                ),
                EntryBody::Question(QuestionBody::Agent { .. }) => {
                    format!("{raw} is an agent's question: talk answer {{question: {raw}}}")
                }
                EntryBody::Question(_) => format!("{raw} is a question: pass it as answers"),
                EntryBody::Request { .. } => {
                    format!("{raw} is a request: pass it in re of to_user, or in settles")
                }
                other => format!(
                    "{raw} is a {} entry, not a node",
                    super::format::entry_type(other.entry_type())
                ),
            });
            out.push(kind.unwrap_or_else(|| format!("{raw} reads as an entry id, not a node")));
            continue;
        }
        let basis = is_basis_arg(&a.arg);
        if let Ok(k) = crate::decode::values::key(raw) {
            let f = find_key(s, &k.key);
            let carrier = f.open.or_else(|| f.closed.first().copied());
            let place = match carrier {
                Some(n) => format!("it sits on {} ({})", super::format::path(s, n), n.render()),
                None => "no node carries it".into(),
            };
            let instead = if basis {
                format!(
                    "pass a node as {}, or something outside the forum as {{external, \
                     checked_at}}",
                    a.arg
                )
            } else {
                format!("pass a node as {}, and a conversation key as where", a.arg)
            };
            out.push(format!("{} is a key: {place}; {instead}", k.key));
            continue;
        }
        if basis && !names_a_node(s, raw) {
            out.push(format!("{} {raw:?} names no node: {BASIS_FORMS}", a.arg));
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

/// The forms of a basis reference (§15), for a basis item that names no node: since 1.6, words
/// not written as a reference are something outside the forum.
const BASIS_FORMS: &str = "a basis item is #12@4 or a path with @4 (a node at a version), #12 or \
     a path (a node), or e57 (an entry); for something outside the forum, such as a build, a run, \
     or a document, write it in words, or as {external, checked_at?}";

/// Whether `arg` is an item of `basis`.
fn is_basis_arg(arg: &str) -> bool {
    arg.starts_with("basis[")
}

/// Whether the basis item `raw`, a node with or without a version, names a node of `s`. A path
/// that names none meets `not_found(Node)`, whose hint then teaches the forms of a basis, and
/// that something outside the forum is written in words (1.6, §15).
fn names_a_node(s: &State, raw: &str) -> bool {
    match crate::decode::values::basis_text(raw) {
        Ok(forum_core::BasisArg::Node { at, .. }) => forum_core::query::resolve(s, &at).is_some(),
        _ => true,
    }
}

// ------------------------------------------------------------------------------------------------
// Warnings

/// The time zone of a series, for `uncovered_slot`.
fn zone(s: &State, n: Option<NodeId>) -> String {
    n.and_then(|n| s.nodes.get(&n))
        .and_then(|x| match &x.role {
            Role::Series(series) => Some(series.repeat.tz.name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "UTC".into())
}

/// `t` as a local time in the zone of series `n` (`2026-10-02 07:00`), or in UTC without one
/// (1.4, §12, R18: the first likely start "as a local time in the series' zone").
fn local_time(s: &State, n: Option<NodeId>, t: Time) -> String {
    let tz = n.and_then(|n| s.nodes.get(&n)).and_then(|x| match &x.role {
        Role::Series(series) => Some(&series.repeat.tz),
        _ => None,
    });
    let utc_s = i64::try_from(t.0 / 1_000).unwrap_or(i64::MAX);
    let offset = tz.map_or(0, |tz| i64::from(forum_cron::offset_at(tz, utc_s)));
    let local = utc_s.saturating_add(offset);
    let (y, m, d) = forum_cron::civil_from_days(local.div_euclid(86_400));
    let minute = local.rem_euclid(86_400) / 60;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", minute / 60, minute % 60)
}

/// What a warning reads beside its own detail (1.12, §21): the other warnings of the response, and
/// the addressee of the `to_user` it came with.
#[derive(Clone, Copy, Debug, Default)]
pub struct WarnCtx {
    /// The response also warns that `to_user {at}` was ignored (`AtIgnored`).
    pub at_ignored: bool,
    /// The principal a `to_user` was addressed to.
    pub addressee: Option<PrincipalId>,
}

/// Whether principal `p`'s person, under any alias, has an unsettled user request (1.12, §21):
/// read from `ix.unsettled_by`, one set per alias.
fn has_unsettled_request(s: &State, p: PrincipalId) -> bool {
    let mut aliases: Vec<PrincipalId> =
        s.ix.person_of
            .get(&p)
            .and_then(|m| s.ix.aliases_of.get(m))
            .map(|xs| xs.iter().copied().collect())
            .unwrap_or_default();
    aliases.push(p);
    aliases
        .iter()
        .any(|a| s.ix.unsettled_by.get(a).is_some_and(|rs| !rs.is_empty()))
}

/// Why a `to_user` of kind `result` answered none of `requests` (1.4, §21, re-run R34). 1.12: with
/// none named, it asks for `re` only when the addressee has an unsettled request: in the lab's
/// seventh round (it6 l2), a worker on a node nobody asked for was asked to name a request that did
/// not exist.
fn answered_nothing(s: &State, requests: &[Eid], addressee: Option<PrincipalId>) -> String {
    if requests.is_empty() {
        return match addressee.filter(|p| !has_unsettled_request(s, *p)) {
            Some(p) => format!(
                "this result answered no request: {} has none unsettled, so it stays on record as \
                 a report",
                cut(&principal(s, p), QUOTED_MAX)
            ),
            None => "this result named no request in re, so it answered none: name the request \
                     in re to report its result"
                .into(),
        };
    }
    let outside = requests.iter().any(|r| {
        s.requests
            .get(r)
            .is_some_and(|q| q.requester == Requester::External)
    });
    if outside {
        format!(
            "this result answered none of {}: a requester outside the swarm is answered by talk \
             said in their conversation",
            ids(requests, |e| e.render())
        )
    } else {
        format!(
            "this result answered none of {}: each request's requester is another person; report \
             to that principal",
            ids(requests, |e| e.render())
        )
    }
}

/// 1.10 (§10.2, §19): the bytes of a pause's reason that the `paused` warning of an intent shows,
/// more than the status line's, since the warning comes once, where the work is.
pub const PAUSED_REASON_MAX: usize = 200;

/// The session that wrote `e`, when it is a release (a checkpoint with a reason): the author of
/// the release that set a wait (1.10, §10.1).
fn release_by(src: &dyn Source, e: Eid) -> Option<SessionId> {
    let v = src.entry(e)?;
    match (v.body, v.by) {
        (
            EntryBody::Checkpoint {
                release: Some(_), ..
            },
            Author::Session(x),
        ) => Some(x),
        _ => None,
    }
}

/// One warning as `{code, id?, detail, entries?}` (§20.1); `src` gives a pause's reason and a
/// release's author, and `session` is the caller's (1.10); `cx` what the response's other parts
/// say (1.12).
#[must_use]
pub fn warning(
    s: &State,
    src: &dyn Source,
    session: Option<SessionId>,
    w: &Warning,
    cx: WarnCtx,
) -> Json {
    let detail = match &w.detail {
        WarningDetail::Moved { entries, total } => format!(
            "others recorded people-related entries here after you last read it ({}): read them \
             before you send",
            shown(entries.len(), *total)
        ),
        WarningDetail::UnsettledRequests { requests } => format!(
            "closed with unsettled requests ({}): they stay flagged until decided and answered",
            ids(requests, |e| e.render())
        ),
        WarningDetail::OpenCommitments { commitments } => format!(
            "closed with open commitments ({}): the flag stays until a reply in each conversation",
            ids(commitments, |e| e.render())
        ),
        WarningDetail::OpenObjections { objections } => format!(
            "closed with open objections ({}): the flag stays until each is resolved",
            ids(objections, |e| e.render())
        ),
        WarningDetail::Agreement {
            sources, approvals, ..
        } => agreement_detail(s, sources, *approvals, &mut BTreeMap::new()),
        // 1.10 (§9.2): a non-holder's `said` counts; by convention the holder speaks (1.9
        // withheld it).
        WarningDetail::ConversationHeld { holders } => format!(
            "{} holds the conversation node or work inside it, and you hold neither: what you \
             recorded counts, but by convention the holder speaks, so tell them with talk say",
            ids(holders, |x| x.render())
        ),
        WarningDetail::HeldByOther { holders, left } if left.is_empty() => format!(
            "{} held the node; that hold ended",
            ids(holders, |x| x.render())
        ),
        // 1.8 (§5): a close by a session that did not hold the node settles nothing by default.
        WarningDetail::HeldByOther { holders, left } => format!(
            "{} held the node; that hold ended. Since you did not hold it, your close settled \
             nothing by default; left unsettled for that holder: {}. Name them in settles only if \
             you did their work",
            ids(holders, |x| x.render()),
            ids(left, |r| r.render())
        ),
        WarningDetail::NoHold { why } => match why {
            NoHoldWhy::HoldsLimit => format!(
                "the node was made, but no hold was issued: you hold hold.per_session ({}) nodes",
                s.params.hold_per_session
            ),
            NoHoldWhy::Paused => "the node was made, but no hold was issued: it is paused".into(),
            NoHoldWhy::NoCapacity => {
                "the node was made, but no hold was issued: its capacity is 0".into()
            }
        },
        WarningDetail::UncoveredSlot {
            covered_days,
            days,
            usual_hours,
            first_start,
        } => {
            let hours = if usual_hours.is_empty() {
                "none known".to_string()
            } else {
                ids(usual_hours, |h| format!("{h:02}:00"))
            };
            let first = match first_start {
                Some(t) => format!(
                    "first likely start in the next window: {} ({})",
                    local_time(s, w.id, *t),
                    zone(s, w.id)
                ),
                None => "no usual start falls within the window: without an outside trigger, the \
                         run will likely be missed"
                    .into(),
            };
            format!(
                "this series' window rarely sees an arriving session ({covered_days}/{days} days); \
                 usual start hours {hours} ({}); {first}; set up an outside trigger that brings an \
                 agent within the window, recording its intent first, or tell the requester when \
                 runs are likely to start",
                zone(s, w.id)
            )
        }
        WarningDetail::DuplicateKey { keys } => format!(
            "the reopen dropped keys that other open nodes carry: {}",
            ids(keys, |(k, n)| format!("{k} (on {})", node_ref(s, *n)))
        ),
        WarningDetail::SecretMasked { fields } => {
            format!("a secret was found and masked in {fields} field(s)")
        }
        WarningDetail::UserSaturated {
            principal: p,
            pending,
        } => format!(
            "{} has {pending} pending questions already: decide on a recorded assumption where you \
             can; the question was recorded",
            cut(&principal(s, *p), QUOTED_MAX)
        ),
        WarningDetail::SessionOpened { session } => format!(
            "this call opened session {0}; pass session: {0} on every call",
            session.render()
        ),
        WarningDetail::OpenIntent { intent } => format!(
            "your intent {} is still open on {}: if the action happened, say so with \
             intent_done; otherwise the next holder must check whether it did",
            intent.render(),
            w.id.map_or_else(String::new, |n| node_ref(s, n))
        ),
        WarningDetail::ExpectsIgnored { expects } => format!(
            "expects is a later release: nothing waits for the answer; to get one from an agent, \
             use talk ask {{to}} (expects: {})",
            match expects {
                Expects::None => "none",
                Expects::Ack => "ack",
                Expects::Reply => "reply",
            }
        ),
        WarningDetail::SeriesStopped { occurrence } => format!(
            "closing the series dropped its occurrence {}: no further occurrence opens until the \
             series is reopened",
            node_ref(s, *occurrence)
        ),
        WarningDetail::SelfBasisDropped { dropped } => format!(
            "{dropped} basis reference(s) named this decision's own node and were dropped; the \
             decision was recorded with the others"
        ),
        WarningDetail::SupersededBasisDropped { decision, dropped } => format!(
            "{dropped} basis reference(s) named {}, the decision this one supersedes, and were \
             dropped; the decision was recorded with the others",
            decision.render()
        ),
        WarningDetail::ResultAnsweredNothing { requests } => {
            answered_nothing(s, requests, cx.addressee)
        }
        // 1.10 (§10.1, §10.2): a checkpoint ends the wait; the session that set it is told so.
        WarningDetail::Waiting {
            on,
            set_at,
            release: Some(e),
        } if release_by(src, *e).is_some_and(|by| Some(by) == session) => format!(
            "you set this wait in {}: since {} it has waited for {}; your next checkpoint ends \
             it, and to wait again, release with waits_for",
            e.render(),
            time(*set_at),
            super::hello::wait_words(s, Some(*on))
        ),
        WarningDetail::Waiting {
            on,
            set_at,
            release,
        } => format!(
            "you hold it, but since {} it has waited for {}: the wait may be an action in \
             flight; read {}, and ask its author before you redo that work; your first \
             checkpoint ends the wait",
            time(*set_at),
            super::hello::wait_words(s, Some(*on)),
            release.map_or_else(
                || "the release in the brief's last checkpoint".to_string(),
                |e| format!("the release that set it, {}", e.render())
            )
        ),
        WarningDetail::UnnamedUser { entry } => format!(
            "{} is filed as the words of user, a person your client does not name: if its \
             configuration names them, pass that principal instead",
            entry.render()
        ),
        // 1.11 (§8.2, §21): the default routed a follow-up as a clarification. 1.12: words that
        // needed no work cannot be routed again (the first routing wins), so the warning says what
        // to pass next time; in the lab's seventh round (it6 l4), "Great, I'll review." became a
        // clarification, and its author could not act on "words that need no work to: none".
        WarningDetail::FollowUp {
            entry,
            request,
            node,
            requester,
        } => format!(
            "{} clarifies {}, {}'s open request on {}: new work there takes to: {}; if these \
             words needed no work, pass to: none next time",
            entry.render(),
            request.render(),
            super::format::principal(s, *requester),
            node_ref(s, *node),
            node_ref(s, *node)
        ),
        // 1.9 (§8.1, §21).
        WarningDetail::ClientContext { entry } => format!(
            "{0} looks like your client's instructions, not your human's words: log what they \
             typed with talk from_user, and hide this entry with node hide {{entry: {0}, reason}}",
            entry.render()
        ),
        // 1.9 (§10.1, §20.5, §21). 1.10: with the caps in use, in place of "such as python or
        // sql, not what the work lacks": in the lab's fifth round (it4 l2), "none has git" left a
        // mail assistant unsure which label a worker would match, while the one present called
        // its git ability repo.
        WarningDetail::UnmetNeeds { needs, take: false } => format!(
            "no session can take {} now: none has {}; the caps in use: {}; needs name what a \
             taker must be able to do",
            w.id.map_or_else(|| "it".to_string(), |n| node_ref(s, n)),
            needs
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            caps_in_use(s)
        ),
        WarningDetail::UnmetNeeds { needs, take: true } => format!(
            "your caps lack {}, which {} needs: take it only if you can do the work; else \
             release it",
            needs
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            w.id.map_or_else(|| "it".to_string(), |n| node_ref(s, n))
        ),
        // 1.10 (§11.3, §21): a kept claim with no decision standing. 1.12: its alternative to a
        // decide is rule 9's reopen, so that the claim reads unproved; 1.10's "let the objection
        // stand" sent a reviewer to a new objection that made it refuted (it6 l5).
        WarningDetail::NoBasis { node, decision } => format!(
            "{} reads as done, but its decision {} is retracted and nothing else decides it: if \
             it is proved, talk decide {{at, text, basis}}; if not, reopen it (node reopen): it \
             reads unproved, not refuted",
            node_ref(s, *node),
            decision.render()
        ),
        // 1.10 (§10.2, §19, §21): an intent under a pause.
        WarningDetail::Paused { scope, until } => {
            let by = s
                .pauses
                .get(scope)
                .map(|p| format!(" by {}/{}", p.class, p.by.render()))
                .unwrap_or_default();
            let reason = src
                .pause_reason(*scope)
                .map(|t| t.0)
                .filter(|t| !t.is_empty())
                .map(|t| format!(": {}", cut(&t, PAUSED_REASON_MAX)))
                .unwrap_or_default();
            format!(
                "recorded; the subtree is paused at {}{by} until {}{reason}; make sure the pause \
                 allows this",
                node_ref(s, *scope),
                time(*until)
            )
        }
        // 1.12 (§8.1, §21): on a node with no unsettled request at all, that nobody asked for
        // the work, and where its outcome goes; in the lab's seventh round (it6 l2), the next
        // warning told the worker to pass the `at` this one had ignored.
        WarningDetail::AtIgnored { node }
            if s.ix
                .items
                .get(node)
                .is_none_or(|it| it.unsettled.is_empty()) =>
        {
            format!(
                "{} has no unsettled request, so at was ignored: nobody asked for this work; your \
                 outcome goes in the release's done or the close's summary; to tell a person \
                 anyway, pass principal",
                node_ref(s, *node)
            )
        }
        // 1.8 (§8.1, §21).
        WarningDetail::AtIgnored { node } => format!(
            "at: {} has no unsettled user request of the person this report is to, so at was \
             ignored and the report filed as without it; name the requester as principal, or \
             the requests in re",
            node_ref(s, *node)
        ),
        // 1.12: without "or at" when this response says that at was ignored.
        WarningDetail::NoHumanInReach { due: None, .. } if cx.at_ignored => {
            "nobody will read this: it is filed for user, whom no live session serves, and your \
             session serves no human; pass principal"
                .to_string()
        }
        WarningDetail::NoHumanInReach { due, default } => match due {
            None => "nobody will read this: it is filed for user, whom no live session serves, \
                     and your session serves no human; pass principal, or at: the node whose \
                     requester it is for"
                .to_string(),
            Some(t) => format!(
                "nobody will read this: the question is for user alone, whom no live session \
                 serves, and your session serves no human; {} at {}",
                if *default {
                    "its default applies"
                } else {
                    "it lapses"
                },
                time(*t)
            ),
        },
        // 1.12 (§8.4): no stage of the question's chain is a person a session serves; in the lab's
        // seventh round (it6 l1), a question for a commit's author was filed with no warning, and
        // its asker told the thread it was recorded for him.
        WarningDetail::NobodyServes { question, chain } => format!(
            "no session serves {}: nobody will put {} to them; to reach them, ask where they are: \
             talk ask {{at, where, text}} with no to",
            chain
                .iter()
                .map(|p| cut(&principal(s, *p), QUOTED_MAX))
                .collect::<Vec<_>>()
                .join(" or "),
            question.render()
        ),
        WarningDetail::NoRecipient { node, question } => {
            no_recipient(s, src, session, *node, *question)
        }
        // 1.12 (§13): a note to a silent session; in the lab's seventh round (it6 l6), an incident
        // lead waited for an answer from a session silent for 36 minutes.
        WarningDetail::AddresseeGone { session: x, since } => format!(
            "{} has been silent since {}: it will not read this",
            x.render(),
            time(*since)
        ),
        // 1.12 (§9.2): an answer with nothing asked; in the lab's seventh round (it6 l3), a chat
        // worker answered two questions in a thread with no request recorded for either.
        WarningDetail::AnsweredNothing { node, conv } => format!(
            "{conv} has no request on {}, so this said answers nothing: record the message you \
             answer first",
            node_ref(s, *node)
        ),
    };
    let mut j = Json::obj().with("code", Json::s(warning_name(w.code)));
    if let Some(n) = w.id {
        j.put("id", Json::S(n.render()));
    }
    j.put("detail", Json::S(detail));
    if let WarningDetail::Moved { entries, total } = &w.detail {
        j.put("entries", Json::strs(entries.iter().map(|e| e.render())));
        j.put("total", total_json(*total));
    }
    j
}

/// The most caps that `unmet_needs` names as in use (1.10, §20.5); more are counted.
pub const CAPS_IN_USE_MAX: usize = 20;

/// The caps in use (1.10, §20.5, §21's `unmet_needs`): every label of the caps profiles seen
/// within `capacity.window` of the state's last `now`, which meet needs as `unmet_needs` reads
/// them (`stats.profiles`, §17), ascending, at most [`CAPS_IN_USE_MAX`], then how many more; or
/// `none`. Bounded by the profiles kept, as `Unreachable` is.
fn caps_in_use(s: &State) -> String {
    let since = s.last_now.minus(s.params.capacity_window);
    let caps: std::collections::BTreeSet<&str> = s
        .stats
        .profiles
        .iter()
        .filter(|(_, p)| p.last_seen >= since)
        .flat_map(|((caps, _), _)| caps.iter().map(|l| l.as_str()))
        .collect();
    if caps.is_empty() {
        return "none".to_string();
    }
    let shown: Vec<&str> = caps.iter().copied().take(CAPS_IN_USE_MAX).collect();
    let more = caps.len().saturating_sub(shown.len());
    if more == 0 {
        shown.join(", ")
    } else {
        format!("{}, and {more} more", shown.join(", "))
    }
}

/// 1.12 (§13, §21): `no_recipient` of a note at closed node `k` that reached nobody: "#38 was closed
/// done by s-26 at 2026-10-05T06:27:00Z (e41): nobody holds or will take it; this note reaches
/// nobody", the closure's outcome, closer (`Closure.by`, none for the server), time, and close
/// entry, then "; to reach its last holder use to: session:s-26" while that session is silent,
/// not retired, and not the caller: the core delivers a note there to it while it is live. 1.11
/// put these words in `delivered_to`; in the lab's seventh round (it6 l6), two notes to the
/// holder of an incident closed seconds before reported "the next holder" and reached nobody.
/// 1.12's review: for an agent question by `holder:`, "this question reaches nobody".
fn no_recipient(
    s: &State,
    src: &dyn Source,
    session: Option<SessionId>,
    k: NodeId,
    question: bool,
) -> String {
    let n = node_ref(s, k);
    let what = if question { "question" } else { "note" };
    let Some(forum_core::Life::Closed(closure)) = s.nodes.get(&k).map(|x| &x.life) else {
        return format!("nobody holds or will take {n}: this {what} reaches nobody");
    };
    let by = closure
        .by
        .map(|x| format!(" by {}", x.render()))
        .unwrap_or_default();
    let entry = src
        .close_entry(k)
        .map(|v| format!(" ({})", v.eid.render()))
        .unwrap_or_default();
    let mut t = format!(
        "{n} was closed {}{by} at {}{entry}: nobody holds or will take it; this {what} reaches \
         nobody",
        super::format::outcome(&closure.outcome),
        time(closure.at)
    );
    if let Some(x) = closure.by
        && Some(x) != session
        && s.sessions.get(&x).is_some_and(|y| y.silent)
    {
        t.push_str(&format!(
            "; to reach its last holder use to: session:{}",
            x.render()
        ));
    }
    t
}

/// The `session_opened` warning of a failed first call that opened its session (1.5, §20.1, §21,
/// R3): the core's refusal carries no warnings, and its agent could not tell what the call had
/// applied, so the shell says that the session was opened, and with what. 1.10: the opening
/// carries `hello`'s intent, caps, horizon, and principal (A-38), and what failed is its
/// `from_user`, the only part of a first `hello` that is checked after the opening; in the lab's
/// fifth round (it4: six workers that logged their scheduler's prompt), "nothing else in the call
/// was applied" was wrong, and workers could not tell what held.
#[must_use]
pub fn failed_opening(x: SessionId, cmd: Option<&Cmd>) -> Json {
    let (given, from_user) = match cmd {
        Some(Cmd::Session(forum_core::SessionCmd::Hello {
            intent,
            caps,
            horizon,
            principal,
            from_user,
        })) => {
            let given: Vec<&str> = [
                ("intent", intent.is_some()),
                ("caps", caps.is_some()),
                ("horizon", horizon.is_some()),
                ("principal", principal.is_some()),
            ]
            .into_iter()
            .filter_map(|(name, is)| is.then_some(name))
            .collect();
            (given, from_user.is_some())
        }
        _ => (Vec::new(), false),
    };
    let with = match given.as_slice() {
        [] => String::new(),
        [one] => format!(", with its {one}"),
        [first, last] => format!(", with its {first} and {last}"),
        [init @ .., last] => format!(", with its {}, and {last}", init.join(", ")),
    };
    let detail = if from_user {
        format!(
            "this call opened session {0}{with}; only its from_user was refused, and it was not \
             logged: pass session: {0} on every call",
            x.render()
        )
    } else {
        format!(
            "this call failed, but it opened session {0}{with}; nothing else in the call was \
             applied; pass session: {0} on every call",
            x.render()
        )
    };
    Json::obj()
        .with("code", Json::s(warning_name(WarningCode::SessionOpened)))
        .with("detail", Json::S(detail))
}

/// An agreement text in the `agreement` warning is cut to this many bytes (1.7, §11.2).
pub const AGREEMENT_TEXT_MAX: usize = 1_024;

/// The sources of an `agreement` warning that it names (1.7, §11.2): those below `/`. The house
/// rules, the agreements of `/`, raise no warning, since `hello` shows them; in the lab's second
/// round the warning fired 14 times, without its texts, and nobody acted on it.
fn below_root(sources: &[NodeId]) -> Vec<NodeId> {
    sources.iter().copied().filter(|n| *n != ROOT).collect()
}

/// The detail of an `agreement` warning (1.7, §11.2): each source with its text, cut to
/// `AGREEMENT_TEXT_MAX`, the first time a warning shows that text to the session, and by its source
/// alone after that while the text is unchanged; `shown` is what the session's warnings showed
/// (the shell's memory, §20.1), and gains what this one shows.
fn agreement_detail(
    s: &State,
    sources: &[NodeId],
    approvals: u32,
    shown: &mut BTreeMap<NodeId, u64>,
) -> String {
    let named: Vec<String> = sources
        .iter()
        .map(|n| {
            let text = s
                .nodes
                .get(n)
                .map(|x| x.agreements.0.as_str())
                .unwrap_or_default();
            let digest = fnv1a64(text.as_bytes());
            if shown.get(n) == Some(&digest) {
                format!("{} (shown before)", node_ref(s, *n))
            } else {
                shown.insert(*n, digest);
                format!("{}: \"{}\"", node_ref(s, *n), cut(text, AGREEMENT_TEXT_MAX))
            }
        })
        .collect();
    format!(
        "agreements in force: {}; approvals of the current content: {approvals}; the server does \
         not interpret agreements, and the count implies no threshold",
        named.join("; ")
    )
}

/// The `ignored_arguments` warning the shell adds (1.7, §21): the call gave arguments of other
/// actions, and was applied without them.
#[must_use]
pub fn ignored_arguments(detail: &str) -> Json {
    Json::obj()
        .with("code", Json::s("ignored_arguments"))
        .with("detail", Json::S(detail.to_string()))
}

/// The `warnings: [...]` line, or None without warnings. `agreements` is what the session's
/// `agreement` warnings showed (1.7, §20.1): an `agreement` warning names only the sources below
/// `/`, is left out when there are none, and carries each text once per session. `addressee` is
/// the principal of a `to_user` (1.12).
#[must_use]
pub fn warnings_line(
    s: &State,
    src: &dyn Source,
    session: Option<SessionId>,
    ws: &[Warning],
    agreements: &mut BTreeMap<NodeId, u64>,
    addressee: Option<PrincipalId>,
) -> Option<(String, Json)> {
    let cx = WarnCtx {
        at_ignored: ws
            .iter()
            .any(|w| matches!(w.detail, WarningDetail::AtIgnored { .. })),
        addressee,
    };
    let items: Vec<Json> = ws
        .iter()
        .filter_map(|w| match &w.detail {
            WarningDetail::Agreement {
                sources, approvals, ..
            } => {
                let below = below_root(sources);
                if below.is_empty() {
                    return None;
                }
                let mut j = Json::obj().with("code", Json::s(warning_name(w.code)));
                if let Some(n) = w.id {
                    j.put("id", Json::S(n.render()));
                }
                Some(j.with(
                    "detail",
                    Json::S(agreement_detail(s, &below, *approvals, agreements)),
                ))
            }
            _ => Some(warning(s, src, session, w, cx)),
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    let list = Json::A(items);
    Some((format!("warnings: {}", list.text()), list))
}
