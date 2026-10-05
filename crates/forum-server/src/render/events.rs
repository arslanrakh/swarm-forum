//! The events of `view history` (§16, §20.4, design D.11.7): each event by its own renderer, in
//! the shell's formats (ids as `#12` or a path, `e345`, `s-7`, tokens as `12.3`, times as RFC 3339,
//! durations as `1h05m`, names in snake case), never as the core's internal form.
//!
//! Hiding affects rendering only (§16): an `Appended` event renders as its entry, `[hidden:
//! reason]` when the entry was hidden, and a node's file that `node hide {file}` hid renders as
//! `[hidden: reason]` in every `NodeUpdated` that set it. `control export` is the one place the
//! history is shown as it was written (D.11.15).

use std::collections::BTreeMap;

use forum_core::{
    Change, ConfirmVia, DeadlineKind, EndCause, Ev, Event, Extension, HideTarget, HoldEnd,
    IntentEnd, Mark, NextOutcome, NodeId, NodeInit, NodePatch, Origin, Owner, Restored, Role,
    Routing, SignalKey, SignalKind, State, Text, WaitOn,
};

use super::body::params_of;
use super::entries::entry;
use super::format::{dur, link_rel, node_ref, outcome, principal, time};
use super::json::Json;
use super::source::Source;

/// An event's name: its variant in snake case.
#[must_use]
pub fn event_name(ev: &Ev) -> &'static str {
    match ev {
        Ev::Genesis { .. } => "genesis",
        Ev::ParamsChanged { .. } => "params_changed",
        Ev::Fired { .. } => "fired",
        Ev::SessionOpened { .. } => "session_opened",
        Ev::SessionHello { .. } => "session_hello",
        Ev::PrincipalSeen { .. } => "principal_seen",
        Ev::Touched { .. } => "touched",
        Ev::SessionRetired { .. } => "session_retired",
        Ev::SessionStatus { .. } => "session_status",
        Ev::InboxRead { .. } => "inbox_read",
        Ev::Shown { .. } => "shown",
        Ev::NextCalled { .. } => "next_called",
        Ev::NodeCreated { .. } => "node_created",
        Ev::NodeUpdated { .. } => "node_updated",
        Ev::KeyAdded { .. } => "key_added",
        Ev::KeyRemoved { .. } => "key_removed",
        Ev::LinkAdded { .. } => "link_added",
        Ev::NodeMoved { .. } => "node_moved",
        Ev::NodeClosed { .. } => "node_closed",
        Ev::NodeReopened { .. } => "node_reopened",
        Ev::OfferSet { .. } => "offer_set",
        Ev::WaitSet { .. } => "wait_set",
        Ev::DependsRedirected { .. } => "depends_redirected",
        Ev::MarkSet { .. } => "mark_set",
        Ev::IntentOpened { .. } => "intent_opened",
        Ev::IntentClosed { .. } => "intent_closed",
        Ev::IntentMoved { .. } => "intent_moved",
        Ev::SeriesOutcome { .. } => "series_outcome",
        Ev::OccurrenceAvailable { .. } => "occurrence_available",
        Ev::OccurrenceDetached { .. } => "occurrence_detached",
        Ev::SignalDetached { .. } => "signal_detached",
        Ev::HoldIssued { .. } => "hold_issued",
        Ev::HoldExtended { .. } => "hold_extended",
        Ev::HoldEnded { .. } => "hold_ended",
        Ev::Appended { .. } => "appended",
        Ev::RequestAnswered { .. } => "request_answered",
        Ev::RequestSettled { .. } => "request_settled",
        Ev::RequestMoved { .. } => "request_moved",
        Ev::PqRecheckAt { .. } => "pq_recheck_at",
        Ev::PqRechecked { .. } => "pq_rechecked",
        Ev::PqExpired { .. } => "pq_expired",
        Ev::PqEscalated { .. } => "pq_escalated",
        Ev::PqAnswered { .. } => "pq_answered",
        Ev::PqSuperseded { .. } => "pq_superseded",
        Ev::PqWithdrawn { .. } => "pq_withdrawn",
        Ev::PqAssumed { .. } => "pq_assumed",
        Ev::PqMoved { .. } => "pq_moved",
        Ev::AqAnswered { .. } => "aq_answered",
        Ev::UqAnswered { .. } => "uq_answered",
        Ev::UqDefaulted { .. } => "uq_defaulted",
        Ev::UqReaddressed { .. } => "uq_readdressed",
        Ev::UqLapsed { .. } => "uq_lapsed",
        Ev::UqAssumed { .. } => "uq_assumed",
        Ev::CommitmentOpened { .. } => "commitment_opened",
        Ev::CommitmentRefreshed { .. } => "commitment_refreshed",
        Ev::CommitmentFulfilled { .. } => "commitment_fulfilled",
        Ev::CommitmentMoved { .. } => "commitment_moved",
        Ev::SaidConfirmed { .. } => "said_confirmed",
        Ev::SaidUnconfirmed { .. } => "said_unconfirmed",
        Ev::SaidWithheld { .. } => "said_withheld",
        Ev::SaidMoved { .. } => "said_moved",
        Ev::EntryRouted { .. } => "entry_routed",
        Ev::DecisionSuperseded { .. } => "decision_superseded",
        Ev::DecisionRetracted { .. } => "decision_retracted",
        Ev::ObjectionResolved { .. } => "objection_resolved",
        Ev::ObjectionMoved { .. } => "objection_moved",
        Ev::DigestShown { .. } => "digest_shown",
        Ev::Refused { .. } => "refused",
        Ev::RefusalExpired { .. } => "refusal_expired",
        Ev::RefusalsCleared { .. } => "refusals_cleared",
        Ev::SignalOpened { .. } => "signal_opened",
        Ev::SignalResolved { .. } => "signal_resolved",
        Ev::Paused { .. } => "paused",
        Ev::Unpaused { .. } => "unpaused",
        Ev::PauseExpired { .. } => "pause_expired",
        Ev::NoteBound { .. } => "note_bound",
        Ev::Hidden { .. } => "hidden",
        Ev::OpRecorded { .. } => "op_recorded",
        Ev::Restored { .. } => "restored",
        Ev::NodeCorrected { .. } => "node_corrected",
        Ev::CorrectionWithdrawn { .. } => "correction_withdrawn",
    }
}

fn deadline_kind(k: DeadlineKind) -> &'static str {
    match k {
        DeadlineKind::Lease => "lease",
        DeadlineKind::Wait => "wait",
        DeadlineKind::Recheck => "recheck",
        DeadlineKind::UserQuestion => "user_question",
        DeadlineKind::Commitment => "commitment",
        DeadlineKind::OccurrenceSlot => "occurrence_slot",
        DeadlineKind::OccurrenceWindow => "occurrence_window",
        DeadlineKind::RefusalExpiry => "refusal_expiry",
        DeadlineKind::Due => "due",
        DeadlineKind::Pause => "pause",
        DeadlineKind::Silence => "silence",
        DeadlineKind::Retire => "retire",
        DeadlineKind::Stale => "stale",
    }
}

fn signal_kind(k: SignalKind) -> &'static str {
    match k {
        SignalKind::Duplicate => "duplicate",
        SignalKind::Conflict => "conflict",
    }
}

fn mark(m: Mark) -> &'static str {
    match m {
        Mark::Recheck => "question_recheck",
        Mark::Assume => "proceed_on_assumption",
    }
}

fn text(t: &Text) -> Json {
    Json::S(t.0.clone())
}

fn node(s: &State, n: NodeId) -> Json {
    Json::S(node_ref(s, n))
}

fn nodes<'a>(s: &State, ns: impl IntoIterator<Item = &'a NodeId>) -> Json {
    Json::strs(ns.into_iter().map(|n| node_ref(s, *n)))
}

fn labels<'a, T: ToString + 'a>(xs: impl IntoIterator<Item = &'a T>) -> Json {
    Json::strs(xs.into_iter().map(ToString::to_string))
}

fn texts(m: &BTreeMap<Text, Text>) -> Json {
    Json::O(
        m.iter()
            .map(|(k, v)| (k.0.clone(), Json::S(v.0.clone())))
            .collect(),
    )
}

/// A signal's pair: `{kind, a, b}`.
fn signal(s: &State, k: &SignalKey) -> Json {
    Json::obj()
        .with("kind", Json::s(signal_kind(k.kind)))
        .with("a", node(s, k.a))
        .with("b", node(s, k.b))
}

/// A node's role, or None for a plain node.
fn role(s: &State, r: &Role) -> Option<Json> {
    match r {
        Role::Plain => None,
        Role::Series(series) => Some(
            Json::obj().with(
                "repeat",
                Json::obj()
                    .with("cron", text(&series.repeat.cron_text))
                    .with("tz", Json::S(series.repeat.tz.name.clone()))
                    .with("run_within", Json::S(dur(series.repeat.run_within))),
            ),
        ),
        Role::Occurrence(o) => Some(
            Json::obj().with(
                "occurrence",
                Json::obj()
                    .with("series", node(s, o.series))
                    .with("slot", Json::S(time(o.slot)))
                    .with("window_end", Json::S(time(o.window_end))),
            ),
        ),
        Role::Signal(k) => Some(Json::obj().with("signal", signal(s, k))),
    }
}

/// A wait as `waits_for` names it (§10.3): `{question}`, `{user_question}`, `{node}`, `{time}`,
/// or `{external, recheck_after}` (the `what` of an external wait is a text of the release that
/// set it, which the caller adds when it has one).
#[must_use]
pub fn wait(s: &State, w: &WaitOn) -> Json {
    match w {
        WaitOn::Question { q, .. } => Json::obj().with("question", Json::S(q.render())),
        WaitOn::UserQuestion(q) => Json::obj().with("user_question", Json::S(q.render())),
        WaitOn::Node(n) => Json::obj().with("node", node(s, *n)),
        WaitOn::Time(t) => Json::obj().with("time", Json::S(time(*t))),
        WaitOn::External(t) => Json::obj().with("recheck_after", Json::S(time(*t))),
    }
}

/// A routing (§8.2).
fn routing(s: &State, r: &Routing) -> Json {
    match r {
        Routing::Unrouted => Json::s("unrouted"),
        Routing::Request { node: n, request } => Json::obj()
            .with("request", Json::S(request.render()))
            .with("node", node(s, *n)),
        Routing::Clarifies { request } => Json::obj().with("clarifies", Json::S(request.render())),
        Routing::Answers { question } => Json::obj().with("answers", Json::S(question.render())),
        Routing::NoAction => Json::s("none"),
    }
}

/// What a node starts with: every field that is not empty.
fn init(s: &State, i: &NodeInit) -> Json {
    let mut j = Json::obj().with("title", text(&i.title));
    let put_text = |j: &mut Json, k: &str, t: &Text| {
        if !t.0.is_empty() {
            j.put(k, text(t));
        }
    };
    put_text(&mut j, "body", &i.body);
    put_text(&mut j, "status", &i.status);
    put_text(&mut j, "digest", &i.digest);
    put_text(&mut j, "agreements", &i.agreements);
    if !i.fields.is_empty() {
        j.put("fields", texts(&i.fields));
    }
    if let Some(k) = &i.kind {
        j.put("kind", Json::S(k.to_string()));
    }
    if !i.tags.is_empty() {
        j.put("tags", labels(&i.tags));
    }
    j.put("capacity", Json::from(i.capacity));
    if i.priority != 0 {
        j.put("priority", Json::I(i64::from(i.priority)));
    }
    if let Some(e) = i.est {
        j.put("est", Json::from(e));
    }
    if !i.needs.is_empty() {
        j.put("needs", labels(&i.needs));
    }
    if let Some(d) = i.due {
        j.put("due", Json::S(time(d)));
    }
    if !i.depends_on.is_empty() {
        j.put("depends_on", nodes(s, &i.depends_on));
    }
    if !i.owners.is_empty() {
        j.put("owners", nodes(s, &i.owners));
    }
    if !i.keys.is_empty() {
        j.put("keys", labels(i.keys.keys()));
    }
    if !i.links.is_empty() {
        j.put(
            "links",
            Json::A(
                i.links
                    .iter()
                    .map(|(r, to)| {
                        Json::obj()
                            .with("rel", Json::s(link_rel(*r)))
                            .with("to", node(s, *to))
                    })
                    .collect(),
            ),
        );
    }
    if let Some(Json::O(fields)) = role(s, &i.role) {
        for (k, v) in fields {
            j.put(&k, v);
        }
    }
    j
}

/// `Change<T>`: absent when kept, null when cleared.
fn change<T>(j: &mut Json, k: &str, c: &Change<T>, f: impl Fn(&T) -> Json) {
    match c {
        Change::Keep => {}
        Change::Clear => j.put(k, Json::Null),
        Change::Set(v) => j.put(k, f(v)),
    }
}

/// The fields an update set, a hidden file of `n` as `[hidden: reason]` (§16).
fn patch(s: &State, n: NodeId, p: &NodePatch, src: &dyn Source) -> Json {
    let mut j = Json::obj();
    let opt_text = |j: &mut Json, k: &str, t: &Option<Text>| {
        if let Some(t) = t {
            j.put(k, text(t));
        }
    };
    opt_text(&mut j, "title", &p.title);
    opt_text(&mut j, "body", &p.body);
    if let Some(f) = &p.fields {
        j.put("fields", texts(f));
    }
    if let Some(files) = &p.files {
        j.put(
            "files",
            Json::O(
                files
                    .iter()
                    .map(|(name, v)| {
                        let shown = src
                            .hidden_file(n, &name.0)
                            .map_or_else(|| v.0.clone(), |r| format!("[hidden: {}]", r.0));
                        (name.0.clone(), Json::S(shown))
                    })
                    .collect(),
            ),
        );
    }
    opt_text(&mut j, "status", &p.status);
    opt_text(&mut j, "digest", &p.digest);
    opt_text(&mut j, "agreements", &p.agreements);
    change(&mut j, "kind", &p.kind, |k| Json::S(k.to_string()));
    if let Some(t) = &p.tags {
        j.put("tags", labels(t));
    }
    if let Some(c) = p.capacity {
        j.put("capacity", Json::from(c));
    }
    if let Some(x) = p.priority {
        j.put("priority", Json::I(i64::from(x)));
    }
    change(&mut j, "est", &p.est, |e| Json::from(*e));
    if let Some(x) = &p.needs {
        j.put("needs", labels(x));
    }
    change(&mut j, "due", &p.due, |d| Json::S(time(*d)));
    if let Some(d) = &p.depends_on {
        j.put("depends_on", nodes(s, d));
    }
    if let Some(o) = &p.owners {
        j.put("owners", nodes(s, o));
    }
    change(&mut j, "repeat", &p.repeat, |r| {
        Json::obj()
            .with("cron", text(&r.cron_text))
            .with("tz", Json::S(r.tz.name.clone()))
            .with("run_within", Json::S(dur(r.run_within)))
    });
    j
}

/// A deadline's owner.
fn owner(s: &State, o: &Owner) -> Json {
    match o {
        Owner::Hold(t) => Json::obj().with("hold", Json::S(t.render())),
        Owner::Node(n) => Json::obj().with("node", node(s, *n)),
        Owner::Entry(e) => Json::obj().with("entry", Json::S(e.render())),
        Owner::Refusal(n, c) => Json::obj()
            .with("node", node(s, *n))
            .with("class", Json::S(c.to_string())),
        Owner::Class(c) => Json::obj().with("class", Json::S(c.to_string())),
        Owner::Session(x) => Json::obj().with("session", Json::S(x.render())),
    }
}

/// One entity brought back into memory: its kind and id.
fn restored(r: &Restored) -> Json {
    let (kind, id) = match r {
        Restored::Request(id, ..) => ("request", *id),
        Restored::PQuestion(id, ..) => ("people_question", *id),
        Restored::UQuestion(id, ..) => ("user_question", *id),
        Restored::Commitment(id, ..) => ("commitment", *id),
        Restored::Said(id, ..) => ("said", *id),
        Restored::Decision(id, ..) => ("decision", *id),
        Restored::Objection(id, ..) => ("objection", *id),
        Restored::UserEntry(id, ..) => ("user_entry", *id),
        Restored::Digest(id, ..) => ("digest", *id),
        Restored::Meta(id, ..) => ("entry", *id),
    };
    Json::obj()
        .with("kind", Json::s(kind))
        .with("id", Json::S(id.render()))
}

/// One event of `view history`: `{seq, at, event, …}`, its fields in the shell's formats.
#[must_use]
pub fn history_event(s: &State, seq: u64, e: &Event, src: &dyn Source) -> Json {
    let j = Json::obj()
        .with("seq", Json::U(seq))
        .with("at", Json::S(time(e.at)))
        .with("event", Json::s(event_name(&e.ev)));
    let n = |x: NodeId| node(s, x);
    let id = |x: forum_core::Eid| Json::S(x.render());
    let sx = |x: forum_core::SessionId| Json::S(x.render());
    let t = |x: forum_core::Time| Json::S(time(x));
    match &e.ev {
        Ev::Genesis { params } | Ev::ParamsChanged { params } => {
            j.with("params", params_of(params))
        }
        Ev::Fired { kind, owner: o } => j
            .with("kind", Json::s(deadline_kind(*kind)))
            .with("owner", owner(s, o)),
        Ev::SessionOpened {
            session,
            class,
            caps,
            horizon,
            intent,
            principal: p,
            ..
        } => j
            .with("session", sx(*session))
            .with("class", Json::S(class.to_string()))
            .with("caps", labels(caps))
            .with_opt("horizon", horizon.map(Json::from))
            .with_opt("intent", intent.as_ref().map(text))
            .with_opt("principal", p.map(|p| Json::S(principal(s, p)))),
        Ev::SessionHello {
            session,
            caps,
            horizon,
            intent,
            principal: p,
        } => j
            .with("session", sx(*session))
            .with_opt("caps", caps.as_ref().map(labels))
            .with_opt("horizon", horizon.map(Json::from))
            .with_opt("intent", intent.as_ref().map(text))
            .with_opt("principal", p.map(|p| Json::S(principal(s, p)))),
        Ev::PrincipalSeen { key, .. } => j.with("principal", Json::S(key.as_str().to_string())),
        Ev::Touched { session, .. } | Ev::SessionRetired { session } => {
            j.with("session", sx(*session))
        }
        Ev::SessionStatus { session, text: x } => {
            j.with("session", sx(*session)).with("text", text(x))
        }
        Ev::InboxRead { session, notes } => j
            .with("session", sx(*session))
            .with("notes", Json::strs(notes.iter().map(|e| e.render()))),
        Ev::Shown {
            session,
            node: x,
            seq: q,
        } => j
            .with("session", sx(*session))
            .with("node", n(*x))
            .with("up_to", Json::U(q.0)),
        Ev::NextCalled {
            session,
            outcome: o,
            open,
            ..
        } => j
            .with("session", sx(*session))
            .with(
                "outcome",
                Json::s(match o {
                    NextOutcome::Item => "item",
                    NextOutcome::Nothing => "nothing",
                    NextOutcome::NothingForYou => "nothing_for_you",
                }),
            )
            .with("open", Json::Bool(*open)),
        Ev::NodeCreated {
            id: x,
            parent,
            name,
            origin,
            init: i,
        } => {
            let by = match origin {
                Origin::Reserved => Json::s("reserved"),
                Origin::Agent(x) | Origin::Ancestor(x) => sx(*x),
                Origin::Occurrence => Json::s("occurrence"),
                Origin::Signal => Json::s("signal"),
            };
            j.with("node", Json::S(x.render()))
                .with("parent", n(*parent))
                .with("name", Json::S(name.to_string()))
                .with("by", by)
                .with("init", init(s, i))
        }
        Ev::NodeUpdated { node: x, patch: p } => {
            j.with("node", n(*x)).with("patch", patch(s, *x, p, src))
        }
        Ev::KeyAdded { node: x, key } => j
            .with("node", n(*x))
            .with("key", Json::S(key.key.to_string())),
        Ev::KeyRemoved { node: x, key } => {
            j.with("node", n(*x)).with("key", Json::S(key.to_string()))
        }
        Ev::LinkAdded { node: x, rel, to } => j
            .with("node", n(*x))
            .with("rel", Json::s(link_rel(*rel)))
            .with("to", n(*to)),
        Ev::NodeMoved {
            node: x,
            parent,
            name,
        } => j
            .with("node", n(*x))
            .with("parent", n(*parent))
            .with("name", Json::S(name.to_string())),
        Ev::NodeClosed {
            node: x,
            outcome: o,
            merged_into,
            by,
        } => j
            .with("node", n(*x))
            .with("outcome", Json::S(outcome(o)))
            .with_opt("merged_into", merged_into.map(n))
            .with_opt("by", by.map(|x| Json::S(x.render()))),
        Ev::NodeReopened { node: x }
        | Ev::OccurrenceAvailable { node: x }
        | Ev::OccurrenceDetached { node: x }
        | Ev::SignalDetached { node: x } => j.with("node", n(*x)),
        Ev::OfferSet { node: x, on } => j.with("node", n(*x)).with("on", Json::Bool(*on)),
        Ev::WaitSet { node: x, wait: w } => j
            .with("node", n(*x))
            .with("wait", w.as_ref().map_or(Json::Null, |w| wait(s, w))),
        Ev::DependsRedirected { node: x, from, to } => j
            .with("node", n(*x))
            .with("from", n(*from))
            .with("to", to.map_or(Json::Null, n)),
        Ev::MarkSet {
            node: x,
            mark: m,
            question,
            on,
        } => j
            .with("node", n(*x))
            .with("mark", Json::s(mark(*m)))
            .with("question", id(*question))
            .with("on", Json::Bool(*on)),
        Ev::IntentOpened { node: x, intent } => j
            .with("node", n(*x))
            .with("intent", id(intent.eid))
            .with("session", sx(intent.session)),
        Ev::IntentClosed { node: x, end } => {
            let j = j.with("node", n(*x));
            match end {
                IntentEnd::Done { by } => j.with("by", id(*by)),
                IntentEnd::ResolvedByClose => j.with("by", Json::s("close")),
            }
        }
        Ev::IntentMoved { from, to } => j.with("from", n(*from)).with("to", n(*to)),
        Ev::SeriesOutcome { series, missed } => j
            .with("series", n(*series))
            .with("missed", Json::Bool(*missed)),
        Ev::HoldIssued {
            token,
            session,
            lease_until,
            l,
            ..
        } => j
            .with("token", Json::S(token.render()))
            .with("session", sx(*session))
            .with("lease_until", t(*lease_until))
            .with("lease", Json::S(dur(*l))),
        Ev::HoldExtended {
            token,
            lease_until,
            by,
        } => j
            .with("token", Json::S(token.render()))
            .with("lease_until", t(*lease_until))
            .with(
                "by",
                Json::s(match by {
                    Extension::Checkpoint => "checkpoint",
                    Extension::Renew => "renew",
                }),
            ),
        Ev::HoldEnded { token, end } => j.with("token", Json::S(token.render())).with(
            "end",
            Json::s(match end {
                HoldEnd::Released => "released",
                HoldEnd::Expired => "expired",
                HoldEnd::EndedByClose(EndCause::Close) => "ended_by_close",
                HoldEnd::EndedByClose(EndCause::Merge) => "ended_by_merge",
                HoldEnd::EndedByClose(EndCause::Move) => "ended_by_move",
            }),
        ),
        Ev::Appended { eid, body, .. } => match src.entry(*eid) {
            Some(v) => j.with("entry", entry(s, &v, None)),
            // An entry the shell's tables lack shows no text: it may have been hidden (§16).
            None => j.with(
                "entry",
                Json::obj().with("e", id(*eid)).with(
                    "type",
                    Json::s(super::format::entry_type(body.entry_type())),
                ),
            ),
        },
        Ev::RequestAnswered { request, by } => j.with("request", id(*request)).with("by", id(*by)),
        Ev::RequestSettled {
            request,
            decision,
            answer,
        } => j
            .with("request", id(*request))
            .with("decision", id(*decision))
            .with("answer", id(*answer)),
        Ev::RequestMoved { request, to } => j.with("request", id(*request)).with("to", n(*to)),
        Ev::PqRecheckAt { q, at } => j.with("question", id(*q)).with("recheck_at", t(*at)),
        Ev::PqRechecked { q, rechecks, next } => j
            .with("question", id(*q))
            .with("rechecks", Json::from(*rechecks))
            .with("next", t(*next)),
        Ev::PqExpired { q } | Ev::UqDefaulted { q } | Ev::UqLapsed { q } => {
            j.with("question", id(*q))
        }
        Ev::PqEscalated { q, uq } => j.with("question", id(*q)).with("user_question", id(*uq)),
        Ev::PqAnswered { q, by } | Ev::AqAnswered { q, by } | Ev::UqAnswered { q, by } => {
            j.with("question", id(*q)).with("by", id(*by))
        }
        Ev::PqSuperseded { q, by } => j.with("question", id(*q)).with("by", id(*by)),
        Ev::PqWithdrawn { q, by } => j.with("question", id(*q)).with("by", id(*by)),
        Ev::PqAssumed { q, decision } | Ev::UqAssumed { q, decision } => {
            j.with("question", id(*q)).with("decision", id(*decision))
        }
        Ev::PqMoved { q, to } => j.with("question", id(*q)).with("to", n(*to)),
        Ev::UqReaddressed { q, stage, due } => j
            .with("question", id(*q))
            .with("stage", Json::from(u32::from(*stage)))
            .with("due", t(*due)),
        Ev::CommitmentOpened {
            c,
            node: x,
            conv,
            followup_at,
        } => j
            .with("commitment", id(*c))
            .with("node", n(*x))
            .with("where", Json::S(conv.to_string()))
            .with("followup", t(*followup_at)),
        Ev::CommitmentRefreshed { c, followup_at } => j
            .with("commitment", id(*c))
            .with("followup", t(*followup_at)),
        Ev::CommitmentFulfilled { c, by } => j.with("commitment", id(*c)).with("by", id(*by)),
        Ev::CommitmentMoved { c, to } => j.with("commitment", id(*c)).with("to", n(*to)),
        Ev::SaidConfirmed { said, via, link } => j
            .with("said", id(*said))
            .with(
                "via",
                Json::s(match via {
                    ConfirmVia::Explicit => "said",
                    ConfirmVia::Release => "release",
                    ConfirmVia::Close => "close",
                    ConfirmVia::Recorded => "recorded",
                }),
            )
            .with_opt("link", link.as_ref().map(text)),
        Ev::SaidUnconfirmed { said } | Ev::SaidWithheld { said } => j.with("said", id(*said)),
        Ev::SaidMoved { said, to } => j.with("said", id(*said)).with("to", n(*to)),
        Ev::EntryRouted {
            entry: x,
            routing: r,
            reason,
        } => j
            .with("entry", id(*x))
            .with("routing", routing(s, r))
            .with_opt("reason", reason.as_ref().map(text)),
        Ev::DecisionSuperseded { d, by } => j.with("decision", id(*d)).with("by", id(*by)),
        Ev::DecisionRetracted { d, by, node } => j
            .with("decision", id(*d))
            .with("by", id(*by))
            .with_opt("node", node.map(n)),
        Ev::ObjectionResolved { o, by, upheld } => j
            .with("objection", id(*o))
            .with("by", id(*by))
            .with_opt("upheld", upheld.then_some(Json::Bool(true))),
        Ev::ObjectionMoved { o, to } => j.with("objection", id(*o)).with("to", n(*to)),
        Ev::DigestShown { entry: x, by } => j.with("digest", id(*x)).with("by", id(*by)),
        Ev::Refused {
            node: x,
            class,
            session,
            backoff_until,
            expires_at,
            ..
        } => j
            .with("node", n(*x))
            .with("class", Json::S(class.to_string()))
            .with("session", sx(*session))
            .with("backoff_until", t(*backoff_until))
            .with("expires_at", t(*expires_at)),
        Ev::RefusalExpired { node: x, class } => j
            .with("node", n(*x))
            .with("class", Json::S(class.to_string())),
        Ev::RefusalsCleared { node: x, by } => j.with("node", n(*x)).with("by", id(*by)),
        Ev::SignalOpened { key, node: x } => j.with("signal", n(*x)).with("pair", signal(s, key)),
        Ev::SignalResolved { key } => j.with("pair", signal(s, key)),
        Ev::Paused {
            scope,
            until,
            by,
            class,
            renewed,
            ..
        } => j
            .with("scope", n(*scope))
            .with("by", Json::S(format!("{class}/{}", by.render())))
            .with("until", t(*until))
            .with("renewed", Json::Bool(*renewed)),
        Ev::Unpaused { scope } | Ev::PauseExpired { scope } => j.with("scope", n(*scope)),
        Ev::NoteBound { note, session } => j.with("note", id(*note)).with("session", sx(*session)),
        Ev::Hidden { target, reason } => match target {
            HideTarget::Entry(x) => j.with("entry", id(*x)).with("reason", text(reason)),
            HideTarget::File { node: x, name } => j
                .with("node", n(*x))
                .with("file", text(name))
                .with("reason", text(reason)),
        },
        Ev::OpRecorded {
            class,
            op,
            session,
            expires_at,
            outcome: o,
            ..
        } => j
            .with("class", Json::S(class.to_string()))
            .with("op_id", Json::S(op.as_str().to_string()))
            .with("session", sx(*session))
            .with("expires_at", t(*expires_at))
            .with(
                "outcome",
                Json::S(match o {
                    Ok(_) => "ok".into(),
                    Err(err) => super::errors::code_name(err.code).into(),
                }),
            ),
        Ev::Restored { items } => j.with("items", Json::A(items.iter().map(restored).collect())),
        Ev::NodeCorrected {
            node: x,
            outcome: o,
            by,
            was,
        } => j
            .with("node", n(*x))
            .with("outcome", Json::S(outcome(o)))
            .with("by", id(*by))
            .with_opt("was", was.as_ref().map(|w| Json::S(outcome(w)))),
        Ev::CorrectionWithdrawn {
            node: x,
            outcome: o,
        } => j.with("node", n(*x)).with("outcome", Json::S(outcome(o))),
    }
}
