//! Entries, nodes, and the items of a brief as JSON (design D.11.7): the pieces that the brief,
//! `hello`, the lists, and the replies share.
//!
//! A text comes from the entry's event (the core never holds it, D.11.6); a hidden entry renders
//! as `[hidden: reason]` (§16).

use forum_core::query::BriefItem;
use forum_core::{
    Addressee, Author, BasisRef, BasisTarget, CommitmentState, DecisionId, DecisionState,
    EntryBody, Life, NodeId, QuestionBody, Requester, SaidState, State, SystemNote, Text,
    TextField, UqState, WaitOn, flags::resolve_merged, flags::stale,
};

use super::format::{
    cut, date, entry_type, node_ref, outcome, path, principal, release_reason, said_kind, time,
    to_user_kind,
};
use super::json::Json;
use super::source::{EntryView, Source};

/// Who wrote an entry: `s-3`, or `server`.
#[must_use]
pub fn author(by: &Author) -> String {
    match by {
        Author::Session(x) => x.render(),
        Author::Server => "server".into(),
    }
}

/// An addressee as §13 writes it.
#[must_use]
pub fn addressee(s: &State, a: &Addressee) -> String {
    match a {
        Addressee::Session(x) => format!("session:{}", x.render()),
        Addressee::Holder(n) => format!("holder:{}", node_ref(s, *n)),
        Addressee::Holders(n) => {
            let p = node_ref(s, *n);
            if p == "/" {
                "holders:/**".into()
            } else {
                format!("holders:{p}/**")
            }
        }
        Addressee::Principal(p) => format!("principal:{}", principal(s, *p)),
    }
}

/// One text of entry `v` as shown: `[hidden: reason]` when the entry is hidden, whichever of its
/// texts `t` is (§16: a hidden entry shows none of its texts), or `t` cut to `max` bytes.
pub(crate) fn shown_text(v: &EntryView, t: &Text, max: Option<usize>) -> String {
    match &v.hidden {
        Some(r) => format!("[hidden: {}]", r.0),
        None => max.map_or_else(|| t.0.clone(), |m| cut(&t.0, m)),
    }
}

/// An entry's main text as shown: `[hidden: reason]`, or the text cut to `max` bytes.
#[must_use]
pub fn text_of(v: &EntryView, max: Option<usize>) -> String {
    shown_text(v, &v.text.main, max)
}

/// A node's state: `open`, or `closed <outcome>` (§10.4, 1.4).
#[must_use]
pub fn node_state(s: &State, n: NodeId) -> String {
    match s.nodes.get(&n).map(|x| &x.life) {
        Some(Life::Closed(c)) => match c.merged_into {
            Some(m) => format!(
                "closed {} (merged into {})",
                outcome(&c.outcome),
                m.render()
            ),
            None => format!("closed {}", outcome(&c.outcome)),
        },
        Some(Life::Open) => "open".into(),
        None => "unknown".into(),
    }
}

/// The state a row shows (1.7, §20.4): an open node's unresolved wait, `waiting (external) until
/// <time>`, `waiting (time) until <time>`, `waiting (question <eN>)`, or `waiting (node <node>)`,
/// so that a worker does not take it only to find it waiting; else `node_state`. A time or outside
/// wait is unresolved until its one-shot deadline fires, which a settled state has done (T3); the
/// others do not depend on the time.
#[must_use]
pub fn row_state(s: &State, n: NodeId) -> String {
    if let Some(node) = s.nodes.get(&n)
        && node.is_open()
        && let Some(w) = node.wait.as_ref()
    {
        let waiting = match w.on {
            WaitOn::Time(_) | WaitOn::External(_) => !w.fired,
            WaitOn::Question { .. } | WaitOn::UserQuestion(_) | WaitOn::Node(_) => {
                !forum_core::flags::resolved(s, &w.on, s.last_now)
            }
        };
        if waiting {
            return match w.on {
                WaitOn::External(t) => format!("waiting (external) until {}", time(t)),
                WaitOn::Time(t) => format!("waiting (time) until {}", time(t)),
                WaitOn::Question { q, .. } | WaitOn::UserQuestion(q) => {
                    format!("waiting (question {})", q.render())
                }
                WaitOn::Node(t) => format!("waiting (node {})", node_ref(s, t)),
            };
        }
    }
    node_state(s, n)
}

/// 1.8 (§20.4): what a row says of an open intent whose session is gone.
pub const GONE_INTENT: &str = "open intent of a gone session: check whether its action happened";

/// 1.8 (§10.4): what a brief says of an open intent whose session is gone, with the call that
/// records what was found.
pub const GONE_INTENT_BRIEF: &str = "open intent of a gone session: check whether its action \
     happened, then record what you found with hold check {target, intent_done}";

/// A row's state as `viewer` sees it (1.9, §20.4): an open node that other sessions hold live
/// reads `held by s-2, s-7`, by session id, before any wait; otherwise `row_state`. In the lab, an
/// idle worker shown a held node as `open` could not tell whether it was work for it.
#[must_use]
pub fn row_state_for(s: &State, n: NodeId, viewer: Option<forum_core::SessionId>) -> String {
    if s.nodes.get(&n).is_some_and(forum_core::Node::is_open) {
        let mut others: Vec<forum_core::SessionId> = forum_core::query::holders(s, n, s.last_now)
            .into_iter()
            .map(|(x, _)| x)
            .filter(|x| Some(*x) != viewer)
            .collect();
        others.sort_unstable();
        others.dedup();
        if !others.is_empty() {
            let names: Vec<String> = others.iter().map(|x| x.render()).collect();
            return format!("held by {}", names.join(", "));
        }
    }
    row_state(s, n)
}

/// A pause's reason is cut to this many bytes where a brief or a row marks its node paused (1.12).
pub const PAUSED_REASON_MAX: usize = 80;

/// 1.12 (§19, §10.4, §20.4): the pause over node `n` at `now`, the nearest one set on `n` or an
/// ancestor below `/` that has not ended, as a brief's header and a row say it: "paused at
/// /payments by s-25 until 07:36: Freeze for INC-311 …", its reason cut to
/// [`PAUSED_REASON_MAX`] bytes; None when no such pause covers it. A pause of `/` is on every
/// status line. In the lab's seventh round (it6 l6), the incident lead read the paused hotfix node,
/// its row and its brief, with no word of the pause, and asked the pause's silent author why.
#[must_use]
pub fn paused_note(
    s: &State,
    n: NodeId,
    now: forum_core::Time,
    src: &dyn Source,
) -> Option<String> {
    if s.pauses.is_empty() {
        return None;
    }
    let (scope, p) = forum_core::spec::ancestors_or_self(s, n)
        .into_iter()
        .filter(|m| *m != forum_core::ROOT)
        .find_map(|m| s.pauses.get(&m).filter(|p| p.until > now).map(|p| (m, p)))?;
    let reason = src
        .pause_reason(scope)
        .map(|t| t.0)
        .filter(|t| !t.is_empty())
        .map(|t| format!(": {}", cut(&t, PAUSED_REASON_MAX)))
        .unwrap_or_default();
    Some(format!(
        "paused at {} by {} until {}{reason}",
        node_ref(s, scope),
        p.by.render(),
        super::format::hhmm(p.until)
    ))
}

/// A node in a list: its id, path, title, and state, a waiting node's wait instead of `open`
/// (1.7), an open intent whose session is gone (1.8), and, 1.9, the other sessions that hold it,
/// as `viewer` sees it (`row_state_for`); 1.12, the pause over it (`paused_note`), for an open node,
/// as the brief has it (§19, §20.4).
#[must_use]
pub fn node_line(
    s: &State,
    n: NodeId,
    title_max: usize,
    viewer: Option<forum_core::SessionId>,
    src: &dyn Source,
) -> Json {
    let title = s
        .nodes
        .get(&n)
        .map(|x| cut(&x.title.0, title_max))
        .unwrap_or_default();
    Json::obj()
        .with("id", Json::S(n.render()))
        .with("path", Json::S(path(s, n)))
        .with("title", Json::S(title))
        .with("state", Json::S(row_state_for(s, n, viewer)))
        .with_opt(
            "paused",
            s.nodes
                .get(&n)
                .filter(|x| x.is_open())
                .and_then(|_| paused_note(s, n, s.last_now, src))
                .map(Json::S),
        )
        .with_opt(
            "intent",
            forum_core::query::gone_intent(s, n).map(|_| Json::s(GONE_INTENT)),
        )
}

/// One basis reference (§15, CS-22): `#12@4 (closed done)`, `e57`, or `external: <what> (checked
/// <date>, recheck <date>)`; a stale one marked with what changed (`#12: now closed failed`,
/// `e57: retracted`, D-4).
#[must_use]
pub fn basis_ref(s: &State, b: &BasisRef, what: Option<&Text>) -> String {
    match b {
        BasisRef::Node { id, version, .. } => {
            let f = resolve_merged(s, *id);
            let at = version.map(|v| format!("@{}", v.0)).unwrap_or_default();
            let base = format!("{}{at}", id.render());
            if stale(s, b) {
                let retracted = s.retracted.contains(&BasisTarget::Node(*id))
                    || s.retracted.contains(&BasisTarget::Node(f));
                if retracted {
                    format!("{base}: retracted")
                } else {
                    format!("{base}: now {}", node_state(s, f))
                }
            } else {
                format!("{base} ({})", node_state(s, f))
            }
        }
        BasisRef::Entry(e) => {
            if s.retracted.contains(&BasisTarget::Entry(*e)) {
                return format!("{}: retracted", e.render());
            }
            match s.decisions.get(e).map(|d| d.state) {
                Some(DecisionState::Superseded { by }) => {
                    format!("{}: superseded by {}", e.render(), by.render())
                }
                Some(DecisionState::Retracted { .. }) => format!("{}: retracted", e.render()),
                Some(DecisionState::Active) | None => e.render(),
            }
        }
        BasisRef::External {
            checked_at,
            recheck,
        } => {
            let what = what.map(|t| t.0.clone()).unwrap_or_default();
            match recheck {
                Some(r) => format!(
                    "external: {what} (checked {}, recheck {})",
                    date(*checked_at),
                    date(*r)
                ),
                None => format!("external: {what} (checked {})", date(*checked_at)),
            }
        }
    }
}

/// A decision with its text, what it settles, what it supersedes, and its basis with stale marks.
#[must_use]
pub fn decision(s: &State, d: DecisionId, src: &dyn Source, max: Option<usize>) -> Json {
    let v = src.entry(d);
    let mut j = Json::obj().with("decision", Json::S(d.render()));
    if let Some(v) = &v {
        j.put("by", Json::S(author(&v.by)));
        j.put("at", Json::S(time(v.at)));
        j.put("text", Json::S(text_of(v, max)));
    }
    let st = s.decisions.get(&d);
    if let Some(dec) = st {
        if !dec.settles.is_empty() {
            j.put(
                "settles",
                Json::strs(dec.settles.iter().map(|r| r.render())),
            );
        }
        match dec.state {
            DecisionState::Active => {}
            DecisionState::Superseded { by } => {
                j.put("superseded_by", Json::S(by.render()));
            }
            DecisionState::Retracted { by } => {
                j.put("retracted_by", Json::S(by.render()));
            }
        }
    }
    if let Some(v) = &v
        && let EntryBody::Decision {
            supersedes, basis, ..
        } = &v.body
    {
        if let Some(x) = supersedes {
            j.put("supersedes", Json::S(x.render()));
        }
        if !basis.is_empty() {
            let mut whats = v.extras(TextField::Basis);
            let refs: Vec<String> = basis
                .iter()
                .map(|b| {
                    // An external reference's `what` is a text of the decision: hidden with it.
                    let what = match b {
                        BasisRef::External { .. } => {
                            whats.next().map(|t| Text(shown_text(v, t, max)))
                        }
                        BasisRef::Node { .. } | BasisRef::Entry(_) => None,
                    };
                    basis_ref(s, b, what.as_ref())
                })
                .collect();
            j.put("basis", Json::strs(refs));
            let stale_now = st.is_some_and(|dec| {
                dec.state == DecisionState::Active && basis.iter().any(|b| stale(s, b))
            });
            if stale_now {
                j.put("stale_basis", Json::Bool(true));
            }
        }
    }
    j
}

/// Any entry: its id, type, author, time, the fields of its body, and its text.
#[must_use]
pub fn entry(s: &State, v: &EntryView, max: Option<usize>) -> Json {
    let mut j = Json::obj()
        .with("e", Json::S(v.eid.render()))
        .with("type", Json::s(entry_type(v.body.entry_type())))
        .with("by", Json::S(author(&v.by)))
        .with("at", Json::S(time(v.at)));
    let extra = |j: &mut Json, key: &str, f: TextField| {
        if let Some(t) = v.extra(f) {
            j.put(key, Json::S(shown_text(v, t, max)));
        }
    };
    match &v.body {
        EntryBody::Note { to, until, .. } => {
            if let Some(a) = to {
                j.put("to", Json::S(addressee(s, a)));
            }
            if let Some(u) = until {
                j.put("until", Json::S(time(*u)));
            }
        }
        EntryBody::Request {
            origin,
            conv,
            requester,
            ..
        } => {
            j.put("origin", Json::S(origin.key.to_string()));
            j.put("where", Json::S(conv.to_string()));
            match requester {
                Requester::User(p) => j.put("requester", Json::S(principal(s, *p))),
                Requester::External => extra(&mut j, "requester", TextField::Requester),
            }
        }
        EntryBody::Question(q) => match q {
            QuestionBody::People { conv, blocking, .. } => {
                j.put("where", Json::S(conv.to_string()));
                if *blocking {
                    j.put("blocking", Json::Bool(true));
                }
            }
            QuestionBody::Agent { to, until, .. } => {
                j.put("to", Json::S(addressee(s, to)));
                j.put("until", Json::S(time(*until)));
            }
            QuestionBody::User { to, due, .. } => {
                j.put("to", Json::strs(to.iter().map(|p| principal(s, *p))));
                j.put("due", Json::S(time(*due)));
                let options: Vec<String> = v
                    .extras(TextField::Option)
                    .map(|t| shown_text(v, t, max))
                    .collect();
                if !options.is_empty() {
                    j.put("options", Json::strs(options));
                }
                extra(&mut j, "recommendation", TextField::Recommendation);
                extra(&mut j, "default", TextField::Default);
            }
        },
        EntryBody::Answer { question, .. } => {
            j.put("question", Json::S(question.render()));
        }
        EntryBody::Decision {
            settles,
            supersedes,
            ..
        } => {
            if !settles.is_empty() {
                j.put("settles", Json::strs(settles.iter().map(|r| r.render())));
            }
            if let Some(x) = supersedes {
                j.put("supersedes", Json::S(x.render()));
            }
        }
        EntryBody::Checkpoint { token, release, .. } => {
            // A checkpoint's main text is its `done` (and a release's record is a checkpoint).
            j.put("token", Json::S(token.render()));
            if let Some(r) = release {
                j.put("release", Json::s(release_reason(*r)));
            }
            j.put("done", Json::S(text_of(v, max)));
            extra(&mut j, "next_step", TextField::NextStep);
            extra(&mut j, "intent", TextField::Intent);
            extra(&mut j, "intent_done", TextField::IntentDone);
            let files: Vec<String> = v
                .extras(TextField::File)
                .map(|t| shown_text(v, t, max))
                .collect();
            if !files.is_empty() {
                j.put("files", Json::strs(files));
            }
            extra(&mut j, "learnings", TextField::Learnings);
            extra(&mut j, "waits_for", TextField::What);
            return j;
        }
        EntryBody::IntentChecked { intent, .. } => {
            // 1.8 (§10.2): a check's record of what became of an open intent; its main text is
            // the `intent_done`, and its author is the checking session.
            j.put("intent", Json::S(intent.render()));
            j.put("intent_done", Json::S(text_of(v, max)));
            return j;
        }
        EntryBody::Said {
            conv,
            kind,
            corrects,
            voice,
        } => {
            j.put("where", Json::S(conv.to_string()));
            j.put("kind", Json::s(said_kind(*kind)));
            if let Some(c) = corrects {
                j.put("corrects", Json::S(c.render()));
            }
            // 1.11 (§9.1, §9.2): its author spoke as the conversation's voice: it had the cap of
            // the conversation's scheme, and no holder of the conversation had.
            if *voice {
                j.put(
                    "voice",
                    Json::S(format!(
                        "the conversation's voice: its author has {}, and no holder of the \
                         conversation has",
                        forum_core::query::voice_cap(conv)
                    )),
                );
            }
            if let Some(x) = s.saids.get(&v.eid) {
                j.put("state", Json::s(said_shown(x)));
            }
        }
        EntryBody::Heard { conv, answers } => {
            j.put("where", Json::S(conv.to_string()));
            extra(&mut j, "from", TextField::From);
            if let Some(q) = answers {
                j.put("answers", Json::S(q.render()));
            }
        }
        EntryBody::FromUser { principal: p } => {
            j.put("principal", Json::S(principal(s, *p)));
            extra(&mut j, "client", TextField::Client);
        }
        EntryBody::ToUser {
            kind,
            verbatim,
            re,
            principal: p,
        } => {
            j.put("principal", Json::S(principal(s, *p)));
            j.put("kind", Json::s(to_user_kind(*kind)));
            j.put("verbatim", Json::Bool(*verbatim));
            if !re.is_empty() {
                j.put("re", Json::strs(re.iter().map(|e| e.render())));
            }
        }
        EntryBody::Approve { version } => {
            j.put("version", Json::U(version.0));
            extra(&mut j, "checked", TextField::Checked);
        }
        EntryBody::Object => {}
        EntryBody::Resolve {
            objection,
            upheld,
            deferred,
        } => {
            j.put("objection", Json::S(objection.render()));
            if *upheld {
                j.put("upheld", Json::Bool(true));
            }
            // 1.11 (§11.3): a deferral, which decided nothing; its text says why.
            if *deferred {
                j.put("deferred", Json::Bool(true));
            }
        }
        EntryBody::Retract { target } => {
            j.put(
                "retracts",
                Json::S(match target {
                    BasisTarget::Node(n) => n.render(),
                    BasisTarget::Entry(e) => e.render(),
                }),
            );
            extra(&mut j, "reason", TextField::Reason);
        }
        EntryBody::System(note) => {
            j.put("system", Json::s(system_name(note)));
            match note {
                SystemNote::Closed { outcome: o } => {
                    // A close's main text is its summary.
                    j.put("outcome", Json::S(outcome(o)));
                    j.put("summary", Json::S(text_of(v, max)));
                    extra(&mut j, "verified", TextField::Verified);
                    extra(&mut j, "learnings", TextField::Learnings);
                    return j;
                }
                SystemNote::Paused { until } => {
                    j.put("until", Json::S(time(*until)));
                }
                SystemNote::MergedFrom { from, .. } => {
                    j.put("from", Json::S(from.render()));
                }
                SystemNote::Moved { from } => {
                    j.put("from_parent", Json::S(from.parent.render()));
                    j.put("from_name", Json::S(from.name.to_string()));
                }
                SystemNote::Defaulted { question } => {
                    j.put("question", Json::S(question.render()));
                }
                SystemNote::Digest { .. }
                | SystemNote::Reopened
                | SystemNote::KeysDropped { .. }
                | SystemNote::Renewed { .. }
                | SystemNote::ScheduleEnded
                | SystemNote::Unpaused
                | SystemNote::PauseExpired
                | SystemNote::Hidden(_) => {}
            }
        }
        EntryBody::Late { token, .. } => {
            j.put("token", Json::S(token.render()));
        }
    }
    j.put("text", Json::S(text_of(v, max)));
    j
}

/// The name of a server entry.
fn system_name(n: &SystemNote) -> &'static str {
    match n {
        SystemNote::Digest { .. } => "digest",
        SystemNote::Closed { .. } => "closed",
        SystemNote::Reopened => "reopened",
        SystemNote::KeysDropped { .. } => "keys_dropped",
        SystemNote::MergedFrom { .. } => "merged_from",
        SystemNote::Moved { .. } => "moved",
        SystemNote::Renewed { .. } => "renewed",
        SystemNote::ScheduleEnded => "schedule_ended",
        SystemNote::Paused { .. } => "paused",
        SystemNote::Unpaused => "unpaused",
        SystemNote::PauseExpired => "pause_expired",
        SystemNote::Hidden(_) => "hidden",
        SystemNote::Defaulted { .. } => "defaulted",
    }
}

/// A `said` entry's state as briefs and feeds show it (1.7, §9.2): a confirm its session's release
/// or close made is `with_reply`, which the server cannot verify. 1.9 (§9.2): an unconfirmed entry
/// is "possibly sent", with what to do, since no next call asks to confirm a `said` any more; a
/// withheld one is shown as such (`said_state`).
#[must_use]
pub fn said_shown(x: &forum_core::Said) -> &'static str {
    match x.state {
        SaidState::Confirmed if x.with_reply => "sent with its author's last reply, unverified",
        SaidState::Unconfirmed => {
            "possibly sent — check the conversation; confirm it with its link if it went out, else \
             send it first"
        }
        SaidState::Pending | SaidState::Confirmed | SaidState::Withheld => said_state(x.state),
    }
}

/// A `said` entry's state.
#[must_use]
pub fn said_state(st: SaidState) -> &'static str {
    match st {
        SaidState::Pending => "pending",
        SaidState::Confirmed => "confirmed",
        SaidState::Unconfirmed => "unconfirmed",
        // 1.9 (§9.2): a non-holder's draft, or an unsent entry a later `said` replaced.
        SaidState::Withheld => "withheld: not sent, and not to be sent",
    }
}

/// A user question's state.
fn uq_state(s: &State, q: forum_core::QuestionId, st: UqState) -> String {
    match st {
        UqState::Pending => {
            let to = s
                .uquestions
                .get(&q)
                .and_then(|u| u.to.get(usize::from(u.stage)).copied())
                .map(|p| principal(s, p))
                .unwrap_or_default();
            format!("pending ({to})")
        }
        UqState::Answered { by } => format!("answered by {}", by.render()),
        UqState::Defaulted => "defaulted".into(),
        UqState::Lapsed => "lapsed".into(),
    }
}

/// One item of a brief section (§10.4, D.8.6), its texts cut to `max` bytes.
#[must_use]
pub fn brief_item(
    s: &State,
    n: NodeId,
    item: &BriefItem,
    src: &dyn Source,
    max: Option<usize>,
) -> Json {
    let text = |e: forum_core::Eid| src.entry(e).map(|v| text_of(&v, max));
    match item {
        BriefItem::Flag(f) => Json::s(super::format::flag(*f)),
        BriefItem::Intent(i) => {
            let v = src.entry(i.eid);
            let words = v
                .as_ref()
                .and_then(|v| v.extra(TextField::Intent).map(|t| shown_text(v, t, max)))
                .unwrap_or_default();
            let gone = forum_core::query::gone_intent(s, n).is_some_and(|g| g == *i);
            Json::obj()
                .with("intent", Json::S(i.eid.render()))
                .with("by", Json::S(i.session.render()))
                .with("text", Json::S(words))
                .with_opt("gone", gone.then(|| Json::s(GONE_INTENT_BRIEF)))
        }
        BriefItem::Said(y) => src.entry(*y).map_or_else(
            || Json::obj().with("said", Json::S(y.render())),
            |v| entry(s, &v, max),
        ),
        BriefItem::Commitment(c) => {
            let mut j = Json::obj().with("commitment", Json::S(c.render()));
            if let Some(x) = s.commitments.get(c) {
                j.put("where", Json::S(x.conv.to_string()));
                j.put("followup", Json::S(time(x.followup_at)));
                j.put(
                    "state",
                    Json::S(match x.state {
                        CommitmentState::Open => "open".into(),
                        CommitmentState::Fulfilled { by } => {
                            format!("fulfilled by {}", by.render())
                        }
                    }),
                );
            }
            j.with_opt("text", text(*c).map(Json::S))
        }
        BriefItem::Decision(d) => decision(s, *d, src, max),
        BriefItem::Agreement { source } => {
            let words = s
                .nodes
                .get(source)
                .map(|x| max.map_or_else(|| x.agreements.0.clone(), |m| cut(&x.agreements.0, m)))
                .unwrap_or_default();
            let mut j = Json::obj().with("agreements", Json::S(words));
            if *source != n {
                j.put("source", Json::S(path(s, *source)));
            }
            j
        }
        BriefItem::LateAnswer {
            question,
            answer,
            assumption,
        } => Json::obj()
            .with("late_answer", Json::S(answer.render()))
            .with("question", Json::S(question.render()))
            .with_opt("assumption", assumption.map(|d| Json::S(d.render())))
            .with_opt("text", text(*answer).map(Json::S)),
        BriefItem::UserQuestion { question, state } => Json::obj()
            .with("user_question", Json::S(question.render()))
            .with("state", Json::S(uq_state(s, *question, *state)))
            .with_opt("text", text(*question).map(Json::S)),
        BriefItem::Request(r) => src.entry(*r).map_or_else(
            || Json::obj().with("request", Json::S(r.render())),
            |v| entry(s, &v, max),
        ),
        BriefItem::Clarification(e) => Json::obj()
            .with("clarification", Json::S(e.render()))
            .with_opt("text", text(*e).map(Json::S)),
        BriefItem::Objection(o) => {
            let j = src.entry(*o).map_or_else(
                || Json::obj().with("objection", Json::S(o.render())),
                |v| entry(s, &v, max),
            );
            // 1.11 (§11.3): the newest deferral of the open objection, by whom and why: a reviewer
            // who could not decide it yet. In the lab's sixth round (l5), a second reviewer
            // repeated a first one's undecided review, which showed nowhere.
            let deferral = s
                .objections
                .get(o)
                .and_then(|x| x.deferred)
                .and_then(|d| src.entry(d));
            j.with_opt(
                "deferred",
                deferral.map(|v| {
                    Json::S(format!(
                        "deferred by {} in {}: {}",
                        author(&v.by),
                        v.eid.render(),
                        text_of(&v, max)
                    ))
                }),
            )
        }
        BriefItem::Digest => {
            let d = s
                .nodes
                .get(&n)
                .map(|x| max.map_or_else(|| x.digest.0.clone(), |m| cut(&x.digest.0, m)))
                .unwrap_or_default();
            Json::obj().with("digest", Json::S(d))
        }
        BriefItem::Checkpoint(e) | BriefItem::Entry(e) => src.entry(*e).map_or_else(
            || Json::obj().with("e", Json::S(e.render())),
            |v| entry(s, &v, max),
        ),
        BriefItem::ExpiredHold(h) => expired_hold(s, n, h, src),
    }
}

/// The last hold that expired after the node's last checkpoint without one of its own (1.4,
/// §10.4, re-run R30), with what the server knows (1.5, R3, D.14.7): its holder, when it expired,
/// that it recorded no checkpoint and no intent, the `said` entries it left unconfirmed on the
/// node, the node's last checkpoint and its time, and the open intent, if any; never its token (a
/// bearer value). In round R3 every taker after such a hold saw "check before you repeat it", and
/// could not tell what to check.
///
/// The note claims nothing the server does not know: a holder also announces an outside action by
/// a `said`, which sets no checkpoint, so the note names the holder's open `said` entries rather
/// than concluding from the missing checkpoint that it announced none. The last checkpoint's id
/// comes from the node; the shell's tables give its time and author when they hold the entry.
fn expired_hold(s: &State, n: NodeId, h: &forum_core::ExpiredHold, src: &dyn Source) -> Json {
    /// The expired holder's open `said` entries the note names; it counts the rest.
    const UNSENT_SHOWN: usize = 5;
    let node = s.nodes.get(&n);
    let last_eid = node.and_then(|x| x.last_checkpoint);
    let last = last_eid.and_then(|e| src.entry(e));
    let intent = node.and_then(|x| x.open_intent);
    let holder = h.session.render();
    let until = time(h.at);
    let checkpoint = match (last_eid, &last) {
        (Some(e), Some(v)) => format!(
            "the last checkpoint is {}, at {} by {}",
            e.render(),
            time(v.at),
            author(&v.by)
        ),
        (Some(e), None) => format!("the last checkpoint is {}", e.render()),
        (None, _) => "the node has no checkpoint".to_string(),
    };
    // The holder's `said` entries on the node that are still pending or unconfirmed (§9.2).
    let unsent: Vec<String> =
        s.ix.items
            .get(&n)
            .into_iter()
            .flat_map(|i| i.open_saids.iter())
            .filter(|x| s.saids.get(x).is_some_and(|y| y.session == h.session))
            .map(|x| x.render())
            .collect();
    let said = if unsent.is_empty() {
        "left no said entry here unconfirmed; an action it did not record would show only outside \
         the forum"
            .to_string()
    } else {
        let more = unsent.len().saturating_sub(UNSENT_SHOWN);
        let mut ids = unsent.into_iter().take(UNSENT_SHOWN).collect::<Vec<_>>();
        if more > 0 {
            ids.push(format!("and {more} more"));
        }
        format!(
            "left said entries here unconfirmed ({}): check in their conversations whether they \
             went out before you send them again",
            ids.join(", ")
        )
    };
    let open = match intent {
        Some(i) => format!(
            "intent {} by {} is still open: check whether its action happened before you repeat \
             it",
            i.eid.render(),
            i.session.render()
        ),
        None => "no intent is open".to_string(),
    };
    let note = format!(
        "{holder} held this node until {until}, when its hold expired; it recorded no checkpoint \
         and no intent, and {said}; {checkpoint}; {open}"
    );
    Json::obj()
        .with("expired_hold", Json::S(holder))
        .with("at", Json::S(until))
        .with(
            "last_checkpoint",
            last_eid.map_or(Json::Null, |e| Json::S(e.render())),
        )
        .with_opt(
            "last_checkpoint_at",
            last.as_ref().map(|v| Json::S(time(v.at))),
        )
        .with(
            "open_intent",
            intent.map_or(Json::Null, |i| Json::S(i.eid.render())),
        )
        .with("note", Json::S(note))
}
