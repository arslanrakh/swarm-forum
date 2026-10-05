//! `project(&State, …) -> V`: the core's state in the shape of the model's abstract state (design
//! D.10.3, D.10.8), so that the two compare with `crate::itf::diff`.
//!
//! - **Time.** A replay runs at `T0 + tick × 1 min` (D.10.5); every time and duration is projected
//!   to ticks, and a value that is not a whole number of ticks after `T0` shows as a string, so it
//!   differs from any model value.
//! - **Content.** The replay sends a model content `v` as the title `c<v>`, a status `v > 0` as
//!   `s<v>`, and a digest `v > 0` as `d<v>` (`crate::driver`); a node the server made has content 0
//!   (its title is its name).
//! - **Keys, names, and principals** are parsed into the model's sum types: `chat:t/<c>` is
//!   `Conv(c)`, `chat:t/<c>/<n>` is `Msg((c, n))`, `lean:<o>` is `Obj(o)`, `user:e<e>` is `User(e)`,
//!   `repeat:<series>:<slot>` is `RepeatSlot((series, slot))`, `person:test:<p>` and `person:user`
//!   are `Person(p)`; a principal `test:<p>` is the model's `p` (D.10.8).
//! - **The retry store.** Rust keeps a request digest where the model keeps the command; the
//!   driver gives each model command its own digest, and the projection maps it back.
//! - **Dropped** (D.10.8): the indices, `stats`, `since`, `held_ms`, `counters.events`, `params`,
//!   text other than the content above, the cited version and the external references of a basis,
//!   `leaving`, `digest_left`, `people_left`, and `left`.
//! - **Compared by id only** (`KEY_SETS`): requests, the three kinds of question, commitments,
//!   `said` entries, decisions, objections, user entries, digest entries, and notes. No key
//!   invariant of D.0.3 reads their fields, and the commands that write them are WP-D4's and
//!   WP-D5's; their ids still have to match, because they are entry ids.

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    Author, BasisTarget, ClassActivity, Dur, EntryMeta, EntryType, Hold, KeyStr, Life, LinkRel,
    Name, Node, OccPhase, Origin, Outcome, Pause, PrincipalKey, Refusal, ReqDigest, Role, Session,
    SignalKind, State, Time, WaitOn,
};

use crate::itf::{R, V};

/// `T0 = 2026-01-01T00:00:00Z`, tick 0 of a replay (D.10.5).
pub const T0: Time = Time(1_767_225_600_000);

/// One tick of the model is one minute of a replay (D.10.5).
pub const TICK: u64 = 60_000;

/// The state fields compared by their key sets only (module documentation).
pub const KEY_SETS: [&str; 11] = [
    "requests",
    "pquestions",
    "aquestions",
    "uquestions",
    "commitments",
    "saids",
    "decisions",
    "objections",
    "user_entries",
    "digest",
    "notes",
];

/// The time of tick `t`.
#[must_use]
pub fn at_tick(t: u64) -> Time {
    Time(T0.0.saturating_add(t.saturating_mul(TICK)))
}

/// `t` ticks as a duration.
#[must_use]
pub fn ticks(t: u64) -> Dur {
    Dur(t.saturating_mul(TICK))
}

/// A time in ticks, or a string when it is not a whole tick at or after `T0`.
#[must_use]
pub fn time(t: Time) -> V {
    match t.0.checked_sub(T0.0) {
        Some(d) if d.is_multiple_of(TICK) => V::int(d / TICK),
        _ => V::Str(format!("{}ms", t.0)),
    }
}

/// An optional time: -1 for none.
#[must_use]
pub fn opt_time(t: Option<Time>) -> V {
    t.map_or(V::Int(-1), time)
}

/// A duration in ticks, or a string when it is not a whole number of ticks.
#[must_use]
pub fn dur(d: Dur) -> V {
    if d.0.is_multiple_of(TICK) {
        V::int(d.0 / TICK)
    } else {
        V::Str(format!("{}ms", d.0))
    }
}

fn opt_int<T: TryInto<i64>>(x: Option<T>) -> V {
    x.map_or(V::Int(-1), V::int)
}

fn strs<'a>(xs: impl IntoIterator<Item = &'a str>) -> V {
    V::Set(xs.into_iter().map(V::str).collect())
}

fn ids(xs: impl IntoIterator<Item = u64>) -> V {
    V::Set(xs.into_iter().map(V::int).collect())
}

/// The model's integer for a text the replay sends as `<prefix><v>`: 0 for the empty text, `v` for
/// `<prefix><v>`, and the text itself otherwise.
fn content_int(text: &str, prefix: char) -> V {
    if text.is_empty() {
        return V::Int(0);
    }
    text.strip_prefix(prefix)
        .and_then(|d| d.parse::<i64>().ok())
        .map_or_else(|| V::str(text), V::Int)
}

/// A node's content: `c<v>` is `v`, any other title (a name the server gave) is 0.
fn content(title: &str) -> V {
    title
        .strip_prefix('c')
        .and_then(|d| d.parse::<i64>().ok())
        .map_or(V::Int(0), V::Int)
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The tick of a slot name `YYYYMMDDTHHMMZ` (`forum_cron::slot_name`), if it is one.
fn slot_tick(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 14 || b.get(8) != Some(&b'T') || b.get(13) != Some(&b'Z') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|x| x.parse::<i64>().ok());
    let (y, mo, d, h, mi) = (
        num(0..4)?,
        num(4..6)?,
        num(6..8)?,
        num(9..11)?,
        num(11..13)?,
    );
    let minutes = (days_from_civil(y, mo, d) * 24 + h) * 60 + mi;
    let t0 = i64::try_from(T0.0 / TICK).ok()?;
    Some(minutes - t0)
}

/// A node name as the model's `Name` (D.10.3).
#[must_use]
pub fn name(n: &Name) -> V {
    let s = n.as_str();
    if let Some(t) = slot_tick(s) {
        return V::tag("Slot", V::Int(t));
    }
    if let Some((head, tail)) = s.split_once('-')
        && let (Some(t), Ok(id)) = (slot_tick(head), tail.parse::<i64>())
    {
        return V::tag("SlotDup", V::Tup(vec![V::Int(t), V::Int(id)]));
    }
    let parts: Vec<&str> = s.split('-').collect();
    if let [kind, a, b, id] = parts.as_slice()
        && let (Ok(a), Ok(b), Ok(id)) = (a.parse::<i64>(), b.parse::<i64>(), id.parse::<i64>())
    {
        let k = match *kind {
            "duplicate" => Some("SigDuplicate"),
            "conflict" | "conflicts_with" => Some("SigConflict"),
            _ => None,
        };
        if let Some(k) = k {
            return V::tag(
                "Signal",
                V::Tup(vec![V::unit(k), V::Int(a), V::Int(b), V::Int(id)]),
            );
        }
    }
    V::tag("Lit", V::str(s))
}

/// A principal key as the model writes it: `test:<p>` is `p`.
#[must_use]
pub fn principal(p: &PrincipalKey) -> V {
    let s = p.as_str();
    V::str(s.strip_prefix("test:").unwrap_or(s))
}

/// A key as the model's `Key` (D.10.3, `types.qnt`).
#[must_use]
pub fn key(k: &KeyStr) -> V {
    let s = k.as_str();
    let (scheme, value) = s.split_once(':').unwrap_or((s, ""));
    match scheme {
        "chat" => {
            let conv = value.strip_prefix("t/").unwrap_or(value);
            match conv.split_once('/') {
                Some((c, n)) => match n.parse::<i64>() {
                    Ok(n) => V::tag("Msg", V::Tup(vec![V::str(c), V::Int(n)])),
                    Err(_) => V::tag("Unknown", V::str(s)),
                },
                None => V::tag("Conv", V::str(conv)),
            }
        }
        "lean" => V::tag("Obj", V::str(value)),
        "user" => value
            .strip_prefix('e')
            .and_then(|e| e.parse::<i64>().ok())
            .map_or_else(
                || V::tag("Unknown", V::str(s)),
                |e| V::tag("User", V::Int(e)),
            ),
        "repeat" => match value.split_once(':') {
            Some((series, slot)) => match (series.parse::<i64>(), slot_tick(slot)) {
                (Ok(x), Some(t)) => V::tag("RepeatSlot", V::Tup(vec![V::Int(x), V::Int(t)])),
                _ => V::tag("Unknown", V::str(s)),
            },
            None => V::tag("Unknown", V::str(s)),
        },
        "person" => V::tag(
            "Person",
            V::str(value.strip_prefix("test:").unwrap_or(value)),
        ),
        _ => V::tag("Unknown", V::str(s)),
    }
}

fn author(a: Author) -> V {
    match a {
        Author::Session(x) => V::int(x.0),
        Author::Server => V::Int(0),
    }
}

/// An entry type as the model's `EntryType` (`ENote`, …).
#[must_use]
pub fn entry_type(t: EntryType) -> V {
    V::unit(&format!("E{t:?}"))
}

/// An outcome as the model's `Outcome`.
#[must_use]
pub fn outcome(o: &Outcome) -> V {
    match o {
        Outcome::Done => V::unit("Done"),
        Outcome::Failed => V::unit("Failed"),
        Outcome::Dropped => V::unit("Dropped"),
        Outcome::Superseded => V::unit("Superseded"),
        Outcome::Missed => V::unit("Missed"),
        Outcome::Other(l) => V::tag("Other", V::str(l.as_str())),
    }
}

fn link_rel(r: LinkRel) -> V {
    V::unit(&format!("{r:?}"))
}

fn signal_kind(k: SignalKind) -> V {
    match k {
        SignalKind::Duplicate => V::unit("SigDuplicate"),
        SignalKind::Conflict => V::unit("SigConflict"),
    }
}

fn origin(o: Origin) -> V {
    match o {
        Origin::Reserved => V::unit("OReserved"),
        Origin::Agent(x) => V::tag("OAgent", V::int(x.0)),
        Origin::Ancestor(x) => V::tag("OAncestor", V::int(x.0)),
        Origin::Occurrence => V::unit("OOccurrence"),
        Origin::Signal => V::unit("OSignal"),
    }
}

/// The period of a cron `*/<p> * * * *`, the only schedule a replay sends (D.10.5, D.10.6).
fn period(cron_text: &str) -> V {
    cron_text
        .strip_prefix("*/")
        .and_then(|r| r.strip_suffix(" * * * *"))
        .and_then(|p| p.parse::<i64>().ok())
        .map_or_else(|| V::str(cron_text), V::Int)
}

fn role(r: &Role) -> V {
    match r {
        Role::Plain => V::unit("Plain"),
        Role::Series(x) => V::tag(
            "Series",
            V::rec([
                (
                    "repeat",
                    V::rec([
                        ("period", period(&x.repeat.cron_text.0)),
                        ("offset", V::Int(0)),
                        ("run_within", dur(x.repeat.run_within)),
                    ]),
                ),
                ("last_missed", V::Bool(x.last_missed)),
            ]),
        ),
        Role::Occurrence(o) => V::tag(
            "Occurrence",
            V::rec([
                ("series", V::int(o.series.0)),
                ("slot", time(o.slot)),
                ("window_end", time(o.window_end)),
                (
                    "phase",
                    V::unit(match o.phase {
                        OccPhase::Waiting => "Waiting",
                        OccPhase::Available => "Available",
                    }),
                ),
                ("window_fired", V::Bool(o.window_fired)),
            ]),
        ),
        Role::Signal(k) => V::tag(
            "RoleSignal",
            V::rec([
                ("kind", signal_kind(k.kind)),
                ("a", V::int(k.a.0)),
                ("b", V::int(k.b.0)),
            ]),
        ),
    }
}

fn wait(w: Option<&forum_core::Wait>) -> V {
    let Some(w) = w else {
        return V::unit("NoWait");
    };
    let on = match w.on {
        WaitOn::Question { q, rechecks_at_set } => V::tag(
            "WQuestion",
            V::rec([
                ("q", V::int(q.0)),
                ("rechecks_at_set", V::int(rechecks_at_set)),
            ]),
        ),
        WaitOn::UserQuestion(q) => V::tag("WUserQuestion", V::int(q.0)),
        WaitOn::Node(n) => V::tag("WNode", V::int(n.0)),
        WaitOn::Time(t) => V::tag("WTime", time(t)),
        WaitOn::External(t) => V::tag("WExternal", time(t)),
    };
    V::tag(
        "HasWait",
        V::rec([
            ("on", on),
            ("set_at", time(w.set_at)),
            ("fired", V::Bool(w.fired)),
        ]),
    )
}

fn life(l: &Life) -> V {
    match l {
        Life::Open => V::unit("Open"),
        Life::Closed(c) => V::tag(
            "Closed",
            V::rec([
                ("at", time(c.at)),
                ("outcome", outcome(&c.outcome)),
                ("merged_into", opt_int(c.merged_into.map(|n| n.0))),
                ("upheld", opt_int(c.upheld.map(|e| e.0))),
                // 1.10: the model's `from_done`: closed `done` before the correction that marks it.
                (
                    "from_done",
                    V::Bool(c.corrected_from == Some(forum_core::Outcome::Done)),
                ),
                // 1.11: the session that closed it, -1 for the server (§13).
                ("by", opt_int(c.by.map(|x| x.0))),
            ]),
        ),
    }
}

/// A node as the model's `Node` (`types.qnt`).
#[must_use]
pub fn node(n: &Node) -> V {
    let (parent, nm) = match &n.place {
        Some(p) => (V::int(p.parent.0), name(&p.name)),
        None => (V::Int(-1), V::tag("Lit", V::str(""))),
    };
    let intent = n.open_intent.map_or_else(
        || V::rec([("eid", V::Int(-1)), ("session", V::Int(-1))]),
        |i| V::rec([("eid", V::int(i.eid.0)), ("session", V::int(i.session.0))]),
    );
    V::rec([
        ("parent", parent),
        ("name", nm),
        ("origin", origin(n.origin)),
        ("created_at", time(n.created_at)),
        ("content", content(&n.title.0)),
        ("status", content_int(&n.status.0, 's')),
        ("digest", content_int(&n.digest.0, 'd')),
        ("agreements", V::Bool(!n.agreements.0.is_empty())),
        ("kind", V::str(n.kind.as_ref().map_or("", |k| k.as_str()))),
        ("tags", strs(n.tags.iter().map(|l| l.as_str()))),
        ("capacity", V::int(n.capacity)),
        ("priority", V::int(n.priority)),
        ("est", opt_int(n.est)),
        ("needs", strs(n.needs.iter().map(|l| l.as_str()))),
        ("due", opt_time(n.due)),
        ("due_fired", V::Bool(n.due_fired)),
        ("owners", ids(n.owners.iter().map(|x| x.0))),
        ("keys", V::Set(n.keys.keys().map(key).collect())),
        (
            "links",
            V::Set(
                n.links
                    .iter()
                    .map(|(r, m)| V::pair(link_rel(*r), V::int(m.0)))
                    .collect(),
            ),
        ),
        ("depends_on", ids(n.depends_on.iter().map(|x| x.0))),
        ("wait", wait(n.wait.as_ref())),
        ("role", role(&n.role)),
        ("life", life(&n.life)),
        ("version", V::int(n.version.0)),
        ("digest_version", V::int(n.digest_version.0)),
        ("fence", V::int(n.fence.0)),
        ("seq", V::int(n.seq.0)),
        ("content_version", V::int(n.content_version.0)),
        ("attempts", V::int(n.attempts)),
        ("checkpoints", V::int(n.checkpoints)),
        ("last_checkpoint", opt_int(n.last_checkpoint.map(|e| e.0))),
        ("open_intent", intent),
        ("recheck_marks", ids(n.recheck_marks.iter().map(|q| q.0))),
        ("assume_marks", ids(n.assume_marks.iter().map(|q| q.0))),
        ("touched", opt_time(n.touched_at)),
        ("stale_fired", V::Bool(n.stale_fired)),
    ])
}

fn hold(h: &Hold) -> V {
    V::rec([
        ("session", V::int(h.session.0)),
        ("taken_at", time(h.taken_at)),
        ("lease_until", time(h.lease_until)),
        ("last_extended", time(h.last_extended)),
        (
            "terms",
            V::rec([
                ("first", dur(h.terms.first)),
                ("default", dur(h.terms.default)),
                ("max", dur(h.terms.max)),
                ("ceiling", dur(h.terms.ceiling)),
                ("renew_max", V::int(h.terms.renew_max)),
            ]),
        ),
        ("l", dur(h.l)),
        ("renews", V::int(h.renews)),
        ("checkpointed", V::Bool(h.checkpointed)),
    ])
}

fn entry(m: &EntryMeta) -> V {
    V::rec([
        ("node", V::int(m.node.0)),
        ("seq", V::int(m.seq.0)),
        ("at", time(m.at)),
        ("by", author(m.by)),
        ("ty", entry_type(m.ty)),
        ("principal", opt_int(m.principal.map(|p| p.0))),
    ])
}

fn session(x: &Session) -> V {
    V::rec([
        ("class", V::str(x.class.as_str())),
        ("started_at", time(x.started_at)),
        ("last_seen", time(x.last_seen)),
        ("silence_at", time(x.silence_at)),
        ("retire_at", time(x.retire_at)),
        ("silent", V::Bool(x.silent)),
        ("caps", strs(x.caps.iter().map(|l| l.as_str()))),
        ("horizon", opt_int(x.horizon)),
        (
            "principals",
            V::Set(x.principals.iter().map(|p| V::int(p.0)).collect()),
        ),
        (
            "shown",
            V::Map(
                x.shown
                    .iter()
                    .map(|(n, q)| (V::int(n.0), V::int(q.0)))
                    .collect(),
            ),
        ),
        (
            "branch_seen",
            V::Map(
                x.branch_seen
                    .iter()
                    .map(|(n, e)| (V::int(n.0), V::int(e.0)))
                    .collect(),
            ),
        ),
        // 1.12 (§10.4): the hold it last released as `handoff`; -1s for none.
        (
            "handed_off",
            V::rec([
                ("node", opt_int(x.handed_off.map(|h| h.token.node.0))),
                ("fence", opt_int(x.handed_off.map(|h| h.token.fence.0))),
                ("until", opt_time(x.handed_off.map(|h| h.until))),
            ]),
        ),
    ])
}

fn pause(p: &Pause) -> V {
    V::rec([
        ("until", time(p.until)),
        ("last_renewed", time(p.last_renewed)),
        ("ttl", dur(p.ttl)),
        ("by", V::int(p.by.0)),
        ("class", V::str(p.class.as_str())),
    ])
}

fn refusal(r: &Refusal) -> V {
    V::rec([
        ("quorum", V::int(r.quorum)),
        (
            "by",
            V::Map(
                r.by.iter()
                    .map(|(x, e)| {
                        (
                            V::int(x.0),
                            V::rec([
                                ("backoff_until", time(e.backoff_until)),
                                ("expires_at", time(e.expires_at)),
                            ]),
                        )
                    })
                    .collect(),
            ),
        ),
    ])
}

fn class_activity(c: &ClassActivity) -> V {
    V::rec([(
        "caps_seen",
        V::Map(
            c.caps_seen
                .iter()
                .map(|(caps, t)| (strs(caps.iter().map(|l| l.as_str())), time(*t)))
                .collect(),
        ),
    )])
}

fn basis_target(t: BasisTarget) -> V {
    match t {
        BasisTarget::Node(n) => V::tag("TNode", V::int(n.0)),
        BasisTarget::Entry(e) => V::tag("TEntry", V::int(e.0)),
    }
}

fn key_set<K: Copy, T>(m: &BTreeMap<K, T>, f: impl Fn(K) -> u64) -> V {
    V::Set(m.keys().map(|k| V::int(f(*k))).collect())
}

/// The core's state as the model's `State`, with the fields of `KEY_SETS` as sets of ids; `ops`
/// maps each request digest back to the model command it stands for (`cmds`), and an unknown
/// digest shows as a string.
#[must_use]
pub fn project(s: &State, cmds: &BTreeMap<ReqDigest, V>) -> V {
    let mut m: BTreeMap<String, V> = BTreeMap::new();
    let mut put = |k: &str, v: V| {
        m.insert(k.to_owned(), v);
    };
    put("last_now", time(s.last_now));
    put("next_node", V::int(s.counters.next_node.0));
    put("next_eid", V::int(s.counters.next_eid.0));
    put("next_session", V::int(s.counters.next_session.0));
    put("next_principal", V::int(s.counters.next_principal.0));
    put("anomalies", V::int(s.anomalies));
    put(
        "principal_ids",
        V::Map(
            s.principal_ids
                .iter()
                .map(|(k, p)| (principal(k), V::int(p.0)))
                .collect(),
        ),
    );
    put(
        "user_msgs",
        V::Map(
            s.user_msgs
                .iter()
                .map(|(p, ts)| (V::int(p.0), V::List(ts.iter().map(|t| time(*t)).collect())))
                .collect(),
        ),
    );
    put(
        "nodes",
        V::Map(
            s.nodes
                .iter()
                .map(|(id, n)| (V::int(id.0), node(n)))
                .collect(),
        ),
    );
    let names = |x: &BTreeMap<(forum_core::NodeId, Name), forum_core::NodeId>| {
        V::Map(
            x.iter()
                .map(|((p, nm), id)| (V::pair(V::int(p.0), name(nm)), V::int(id.0)))
                .collect(),
        )
    };
    put("names", names(&s.names));
    put("aliases", names(&s.aliases));
    put(
        "entries",
        V::Map(
            s.entries
                .iter()
                .map(|(e, x)| (V::int(e.0), entry(x)))
                .collect(),
        ),
    );
    put(
        "holds",
        V::Map(
            s.holds
                .iter()
                .map(|(t, h)| (V::pair(V::int(t.node.0), V::int(t.fence.0)), hold(h)))
                .collect(),
        ),
    );
    put("requests", key_set(&s.requests, |e| e.0));
    put("pquestions", key_set(&s.pquestions, |e| e.0));
    put("aquestions", key_set(&s.aquestions, |e| e.0));
    put("uquestions", key_set(&s.uquestions, |e| e.0));
    put("commitments", key_set(&s.commitments, |e| e.0));
    put("saids", key_set(&s.saids, |e| e.0));
    put("decisions", key_set(&s.decisions, |e| e.0));
    put("objections", key_set(&s.objections, |e| e.0));
    put("user_entries", key_set(&s.user_entries, |e| e.0));
    put("digest", key_set(&s.digest, |e| e.0));
    put("notes", key_set(&s.notes, |e| e.0));
    put(
        "retracted",
        V::Set(s.retracted.iter().map(|t| basis_target(*t)).collect()),
    );
    put(
        "approvals",
        V::Map(
            s.approvals
                .iter()
                .map(|((n, v), c)| (V::pair(V::int(n.0), V::int(v.0)), V::int(*c)))
                .collect(),
        ),
    );
    put(
        "refusals",
        V::Map(
            s.refusals
                .iter()
                .map(|((n, c), r)| (V::pair(V::int(n.0), V::str(c.as_str())), refusal(r)))
                .collect(),
        ),
    );
    put(
        "classes",
        V::Map(
            s.classes
                .iter()
                .map(|(c, a)| (V::str(c.as_str()), class_activity(a)))
                .collect(),
        ),
    );
    put(
        "signals",
        V::Map(
            s.signals
                .iter()
                .map(|(k, n)| {
                    (
                        V::Tup(vec![signal_kind(k.kind), V::int(k.a.0), V::int(k.b.0)]),
                        V::int(n.0),
                    )
                })
                .collect(),
        ),
    );
    put(
        "pauses",
        V::Map(
            s.pauses
                .iter()
                .map(|(n, p)| (V::int(n.0), pause(p)))
                .collect(),
        ),
    );
    put(
        "sessions",
        V::Map(
            s.sessions
                .iter()
                .map(|(x, ss)| (V::int(x.0), session(ss)))
                .collect(),
        ),
    );
    put(
        "ops",
        V::Map(
            s.ops
                .iter()
                .map(|((c, op), r)| {
                    let digest = cmds.get(&r.digest).cloned().unwrap_or_else(|| {
                        V::Str(format!("unknown digest {:02x?}", r.digest.0.get(..8)))
                    });
                    (
                        V::pair(V::str(c.as_str()), V::str(op.as_str())),
                        V::rec([
                            ("digest", digest),
                            ("session", V::int(r.session.0)),
                            ("expires_at", time(r.expires_at)),
                        ]),
                    )
                })
                .collect(),
        ),
    );
    V::Rec(m)
}

/// The model's state reduced as `project` reduces Rust's: the fields of `KEY_SETS` as sets of
/// their keys.
///
/// # Errors
///
/// When `s` is not a model state.
pub fn model_view(s: &V) -> R<V> {
    let V::Rec(m) = s else {
        return Err(format!("a model state expected, found {s}"));
    };
    let mut out = m.clone();
    for k in KEY_SETS {
        let keys: BTreeSet<V> = s.get(k)?.as_map()?.keys().cloned().collect();
        out.insert(k.to_owned(), V::Set(keys));
    }
    Ok(V::Rec(out))
}
