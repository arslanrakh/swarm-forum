//! `talk wait`: the long poll and its targeted wake-ups (design D.11.11, CS-15, §13, §20.7).
//!
//! A handler parks a session's wait here and sleeps until the writer wakes it or its timeout
//! (`min(timeout, talk.wait_max)`) passes; then it runs the `wait` command as an ordinary job, so
//! that its response is computed on a settled state (T3). The writer registers the waiter inside a
//! read-only job, before it asks whether the session is ready (`query::inbox_ready`), so that a
//! commit between the two still wakes it: a `Notify` keeps its permit.
//!
//! After each commit the writer derives from the group's events the sessions they concern
//! ([`concerned`]) and wakes only those that are parked: the recipients of an addressed note, of an
//! agent question, and of an answer; for a `principal:` note, the parked sessions that serve the
//! principal or an alias of it; for a `holders:<a>` note, the parked sessions that hold in `a`'s
//! branch, found by descending `ix.attended_children`; never the sender of a note or question,
//! whose inbox does not show it (§13); the session of a `NoteBound`; and, for an
//! append on node `n`, the sessions waiting `for: n`, at most once per [`WAIT_COALESCE`] per node
//! ([`Coalescer`]): an append within that second of the node's last wake-up is delivered by a
//! wake-up at the end of the second. No waiter is woken by a commit that does not concern it.
//!
//! [`NewsSeen`] is the shell's memory of the last `seq` reported to each session for each node
//! (`wait`'s `since`): it is not state, it drops a session's entries at its silence or retirement
//! (CS-17), and after a restart `since` falls back to the session's `shown`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use forum_core::query::holders;
use forum_core::{
    Addressee, Author, Dur, EntryBody, Ev, Event, NodeId, PrincipalId, QuestionBody, Seq,
    SessionId, State, Time,
};
use tokio::sync::Notify;

/// The waiters of one node are woken at most once per this interval (D.11.11).
pub const WAIT_COALESCE: Dur = Dur(1_000);

/// One parked wait.
#[derive(Clone, Debug)]
pub struct Parked {
    /// Its registration.
    pub id: u64,
    /// What wakes it.
    pub notify: Arc<Notify>,
}

#[derive(Debug)]
struct Reg {
    session: SessionId,
    node: Option<NodeId>,
    notify: Arc<Notify>,
}

/// The parked waits, by session and by the node each waits for.
#[derive(Debug, Default)]
pub struct Waiters {
    next: u64,
    regs: BTreeMap<u64, Reg>,
    by_session: BTreeMap<SessionId, BTreeSet<u64>>,
    by_node: BTreeMap<NodeId, BTreeSet<u64>>,
}

impl Waiters {
    /// None parked.
    #[must_use]
    pub fn new() -> Waiters {
        Waiters::default()
    }

    /// Parks a wait of `session`, for news on `node` if given.
    pub fn register(&mut self, session: SessionId, node: Option<NodeId>) -> Parked {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        let notify = Arc::new(Notify::new());
        self.regs.insert(
            id,
            Reg {
                session,
                node,
                notify: Arc::clone(&notify),
            },
        );
        self.by_session.entry(session).or_default().insert(id);
        if let Some(n) = node {
            self.by_node.entry(n).or_default().insert(id);
        }
        Parked { id, notify }
    }

    /// Removes a parked wait (the handler does, when it wakes or times out).
    pub fn unregister(&mut self, id: u64) {
        let Some(r) = self.regs.remove(&id) else {
            return;
        };
        remove(&mut self.by_session, &r.session, id);
        if let Some(n) = r.node {
            remove(&mut self.by_node, &n, id);
        }
    }

    /// Wakes every parked wait of `session`; returns how many.
    pub fn wake_session(&self, session: SessionId) -> usize {
        self.wake(self.by_session.get(&session))
    }

    /// Wakes every wait parked for news on `node`; returns how many.
    pub fn wake_node(&self, node: NodeId) -> usize {
        self.wake(self.by_node.get(&node))
    }

    /// Wakes every parked wait: at shutdown, so that the server's drain does not wait out
    /// `talk.wait_max`. Returns how many.
    pub fn wake_all(&self) -> usize {
        for r in self.regs.values() {
            r.notify.notify_one();
        }
        self.regs.len()
    }

    fn wake(&self, ids: Option<&BTreeSet<u64>>) -> usize {
        let mut n = 0;
        for id in ids.into_iter().flatten() {
            if let Some(r) = self.regs.get(id) {
                r.notify.notify_one();
                n += 1;
            }
        }
        n
    }

    /// Whether `session` has a parked wait.
    #[must_use]
    pub fn is_parked(&self, session: SessionId) -> bool {
        self.by_session.contains_key(&session)
    }

    /// The sessions with a parked wait, in order, without a copy.
    pub fn parked(&self) -> impl Iterator<Item = SessionId> + '_ {
        self.by_session.keys().copied()
    }

    /// Whether a wait is parked for news on `node`.
    #[must_use]
    pub fn watched(&self, node: NodeId) -> bool {
        self.by_node.contains_key(&node)
    }

    /// How many waits are parked for news on `node` (feature `bench`: gate G-4 counts the waits a
    /// commit concerns).
    #[cfg(feature = "bench")]
    #[must_use]
    pub fn parked_for(&self, node: NodeId) -> usize {
        self.by_node.get(&node).map_or(0, BTreeSet::len)
    }

    /// How many waits are parked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regs.len()
    }

    /// Whether none is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regs.is_empty()
    }
}

fn remove<K: Ord>(map: &mut BTreeMap<K, BTreeSet<u64>>, k: &K, id: u64) {
    if let Some(set) = map.get_mut(k) {
        set.remove(&id);
        if set.is_empty() {
            map.remove(k);
        }
    }
}

/// What a commit's events concern (D.11.11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Concerned {
    /// Sessions to wake now: addressees, recipients, and bound notes.
    pub sessions: BTreeSet<SessionId>,
    /// Nodes that got an entry: their waiters are woken through the [`Coalescer`].
    pub nodes: BTreeSet<NodeId>,
    /// Parked sessions looked at to find `sessions`: the sessions an event names, the holders found
    /// in a `holders:` branch, and, for a `principal:` note, every parked session, since the state
    /// keeps no index of sessions by principal (gate G-4 compares it with what the commit
    /// concerns).
    pub examined: usize,
}

/// Every principal `p` stands for: its aliases, through its person node (D.8.11).
fn aliases(s: &State, p: PrincipalId) -> BTreeSet<PrincipalId> {
    let mut out =
        s.ix.person_of
            .get(&p)
            .and_then(|m| s.ix.aliases_of.get(m))
            .cloned()
            .unwrap_or_default();
    out.insert(p);
    out
}

/// The parked sessions that serve `p` or an alias of it. The state keeps no index of sessions by
/// principal, so this looks at each parked session, and only for a `principal:` note.
fn serving(s: &State, p: PrincipalId, parked: &Waiters) -> Vec<SessionId> {
    let ps = aliases(s, p);
    parked
        .parked()
        .filter(|x| {
            s.sessions
                .get(x)
                .is_some_and(|x| x.principals.iter().any(|q| ps.contains(q)))
        })
        .collect()
}

/// The parked sessions that hold in `a`'s branch, found by descending `ix.attended_children`
/// from `a` (D.8.16), so the cost is that of the attended part of the branch.
fn holding_in(s: &State, a: NodeId, now: Time, parked: &Waiters) -> Vec<SessionId> {
    let mut out = BTreeSet::new();
    let mut stack = vec![a];
    let mut seen = BTreeSet::new();
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        for (x, _) in holders(s, n, now) {
            if parked.is_parked(x) {
                out.insert(x);
            }
        }
        if let Some(children) = s.ix.attended_children.get(&n) {
            stack.extend(children.iter().copied());
        }
    }
    out.into_iter().collect()
}

/// The sessions an addressed note or agent question concerns: its recipients, and for
/// `principal:` and `holders:` the parked sessions it reaches; never its sender, whose inbox does
/// not show what it sent (§13; review E4-spec-11).
fn addressed(
    s: &State,
    to: &Addressee,
    recipients: &BTreeSet<SessionId>,
    sender: Option<SessionId>,
    now: Time,
    parked: &Waiters,
    c: &mut Concerned,
) {
    let mut reached: BTreeSet<SessionId> = recipients.clone();
    c.examined = c.examined.saturating_add(recipients.len());
    match to {
        Addressee::Principal(p) => {
            c.examined = c.examined.saturating_add(parked.len());
            reached.extend(serving(s, *p, parked));
        }
        Addressee::Holders(a) => {
            let found = holding_in(s, *a, now, parked);
            c.examined = c.examined.saturating_add(found.len());
            reached.extend(found);
        }
        // Their recipients are fixed when the note is recorded; a holder who takes the node later
        // is reached by the `NoteBound` of the take.
        Addressee::Session(_) | Addressee::Holder(_) => {}
    }
    if let Some(x) = sender {
        reached.remove(&x);
    }
    c.sessions.extend(reached);
}

/// The sessions and nodes the events of a commit concern, on the state after the commit, with the
/// sessions parked now: membership is looked up in `parked`, never copied, so a commit costs what
/// it concerns (D.11.11; review E4-spec-8).
#[must_use]
pub fn concerned(s: &State, now: Time, events: &[Event], parked: &Waiters) -> Concerned {
    let mut c = Concerned::default();
    for e in events {
        match &e.ev {
            Ev::Appended { node, body, by, .. } => {
                c.nodes.insert(*node);
                let sender = match by {
                    Author::Session(x) => Some(*x),
                    Author::Server => None,
                };
                match body {
                    EntryBody::Note {
                        to: Some(to),
                        recipients,
                        ..
                    } => addressed(s, to, recipients, sender, now, parked, &mut c),
                    EntryBody::Question(QuestionBody::Agent { to, recipients, .. }) => {
                        addressed(s, to, recipients, sender, now, parked, &mut c);
                    }
                    EntryBody::Answer { recipient, .. } => {
                        c.examined = c.examined.saturating_add(1);
                        c.sessions.insert(*recipient);
                    }
                    _ => {}
                }
            }
            Ev::NoteBound { session, .. } => {
                c.examined = c.examined.saturating_add(1);
                c.sessions.insert(*session);
            }
            _ => {}
        }
    }
    c.sessions.retain(|x| parked.is_parked(*x));
    c
}

/// At most one wake-up per node per [`WAIT_COALESCE`] (D.11.11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coalescer {
    last: BTreeMap<NodeId, Time>,
    pending: BTreeMap<NodeId, Time>,
}

impl Coalescer {
    /// No wake-up yet.
    #[must_use]
    pub fn new() -> Coalescer {
        Coalescer::default()
    }

    fn forget_old(&mut self, now: Time) {
        let pending = &self.pending;
        self.last
            .retain(|n, t| now.since(*t) < WAIT_COALESCE || pending.contains_key(n));
    }

    /// Nodes that got entries at `now`: those to wake now. A node woken within the last
    /// `WAIT_COALESCE` is woken at the end of that interval instead ([`Coalescer::due`]).
    pub fn appended(&mut self, nodes: impl IntoIterator<Item = NodeId>, now: Time) -> Vec<NodeId> {
        self.forget_old(now);
        let mut wake = Vec::new();
        for n in nodes {
            if self.pending.contains_key(&n) {
                continue;
            }
            match self.last.get(&n) {
                Some(t) if now.since(*t) < WAIT_COALESCE => {
                    self.pending.insert(n, t.plus(WAIT_COALESCE));
                }
                _ => {
                    self.last.insert(n, now);
                    wake.push(n);
                }
            }
        }
        wake
    }

    /// The deferred wake-ups due at `now`.
    pub fn due(&mut self, now: Time) -> Vec<NodeId> {
        let ready: Vec<NodeId> = self
            .pending
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(n, _)| *n)
            .collect();
        for n in &ready {
            self.pending.remove(n);
            self.last.insert(*n, now);
        }
        ready
    }

    /// When the next deferred wake-up is due.
    #[must_use]
    pub fn next_due(&self) -> Option<Time> {
        self.pending.values().min().copied()
    }
}

/// The last `seq` reported to each session for each node (`wait`'s `since`, D.11.11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewsSeen {
    map: BTreeMap<(SessionId, NodeId), Seq>,
}

impl NewsSeen {
    /// Nothing reported yet.
    #[must_use]
    pub fn new() -> NewsSeen {
        NewsSeen::default()
    }

    /// The last `seq` of `node` reported to `session`.
    #[must_use]
    pub fn get(&self, session: SessionId, node: NodeId) -> Option<Seq> {
        self.map.get(&(session, node)).copied()
    }

    /// Records that `seq` of `node` was reported to `session`.
    pub fn set(&mut self, session: SessionId, node: NodeId, seq: Seq) {
        self.map.insert((session, node), seq);
    }

    /// Forgets `session`: at its silence or retirement (CS-17).
    pub fn drop_session(&mut self, session: SessionId) {
        let keys: Vec<_> = self
            .map
            .range((session, NodeId(0))..=(session, NodeId(u64::MAX)))
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            self.map.remove(&k);
        }
    }

    /// How many entries it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TxKind;
    use crate::store::testkit::{World, create_node, init, issued, session_opened, test_params};
    use forum_core::{Eid, EntryText, PEOPLE, PrincipalKey, ROOT, Text};
    use std::time::Duration;

    fn note(node: NodeId, to: Option<Addressee>, recipients: &[u64]) -> Event {
        Event {
            at: Time(5),
            ev: Ev::Appended {
                eid: Eid(9),
                node,
                seq: Seq(1),
                by: Author::Session(SessionId(1)),
                body: EntryBody::Note {
                    to,
                    until: None,
                    recipients: recipients.iter().map(|x| SessionId(*x)).collect(),
                    pending_holder: false,
                    expects: None,
                    reply_to: None,
                },
                text: EntryText {
                    main: Text("hi".into()),
                    extra: Vec::new(),
                },
            },
        }
    }

    fn parked(xs: &[u64]) -> BTreeSet<SessionId> {
        xs.iter().map(|x| SessionId(*x)).collect()
    }

    /// Waiters with one parked wait for each of `xs`.
    fn waiting(xs: &[u64]) -> Waiters {
        let mut w = Waiters::new();
        for x in xs {
            w.register(SessionId(*x), None);
        }
        w
    }

    #[test]
    fn only_the_parked_sessions_a_commit_concerns_are_woken() {
        let s = State::default();
        let n = NodeId(20);
        let events = vec![
            note(n, Some(Addressee::Session(SessionId(3))), &[3]),
            note(NodeId(21), None, &[]),
            Event {
                at: Time(5),
                ev: Ev::NoteBound {
                    note: Eid(4),
                    session: SessionId(7),
                },
            },
        ];
        let c = concerned(&s, Time(5), &events, &waiting(&[3, 4, 7]));
        assert_eq!(c.sessions, parked(&[3, 7]), "4 is parked but not concerned");
        assert_eq!(c.nodes, [n, NodeId(21)].into_iter().collect());
        let c = concerned(&s, Time(5), &events, &waiting(&[4]));
        assert!(c.sessions.is_empty(), "nobody concerned is parked");
    }

    /// A small forum for the addressed branches of `concerned` (review E4-spec-16): principals
    /// `mail:a@x` and `mail:b@x`, aliases of one person, and `slack:t/u`; sessions A serving the
    /// first, B the second, C the third, D and E none; A holds `/work/x`, B holds `/other`.
    struct Forum {
        w: World,
        a: SessionId,
        b: SessionId,
        c: SessionId,
        d: SessionId,
        e: SessionId,
        p1: PrincipalId,
        p3: PrincipalId,
        work: NodeId,
        other: NodeId,
    }

    fn forum() -> Forum {
        let mut w = World::memory(test_params());
        let principal = |w: &mut World, key: &str| {
            let id = w.s.counters.next_principal;
            w.tx(
                TxKind::Cmd,
                vec![Ev::PrincipalSeen {
                    id,
                    key: PrincipalKey::new(key).unwrap(),
                }],
            );
            id
        };
        let p1 = principal(&mut w, "mail:a@x");
        let p2 = principal(&mut w, "mail:b@x");
        let p3 = principal(&mut w, "slack:t/u");
        let open = |w: &mut World, p: Option<PrincipalId>| {
            let id = w.s.counters.next_session;
            let mut ev = session_opened(w, "coder");
            if let Ev::SessionOpened { principal, .. } = &mut ev {
                *principal = p;
            }
            w.tx(TxKind::Cmd, vec![ev]);
            id
        };
        let a = open(&mut w, Some(p1));
        let b = open(&mut w, Some(p2));
        let c = open(&mut w, Some(p3));
        let d = open(&mut w, None);
        let e = open(&mut w, None);
        create_node(
            &mut w,
            PEOPLE,
            "alice",
            d,
            init(1, &["person:mail:a@x", "person:mail:b@x"]),
        );
        let work = create_node(&mut w, ROOT, "work", d, init(1, &[]));
        let x = create_node(&mut w, work, "x", d, init(1, &[]));
        let other = create_node(&mut w, ROOT, "other", d, init(1, &[]));
        let ev = issued(&w, x, a);
        w.tx(TxKind::Cmd, vec![ev]);
        let ev = issued(&w, other, b);
        w.tx(TxKind::Cmd, vec![ev]);
        Forum {
            w,
            a,
            b,
            c,
            d,
            e,
            p1,
            p3,
            work,
            other,
        }
    }

    /// An entry on `node` by `by`.
    fn entry(node: NodeId, by: SessionId, body: EntryBody) -> Event {
        Event {
            at: Time(5),
            ev: Ev::Appended {
                eid: Eid(9),
                node,
                seq: Seq(1),
                by: Author::Session(by),
                body,
                text: EntryText {
                    main: Text("hi".into()),
                    extra: Vec::new(),
                },
            },
        }
    }

    fn addressed_note(to: Addressee, recipients: &[SessionId]) -> EntryBody {
        EntryBody::Note {
            to: Some(to),
            until: None,
            recipients: recipients.iter().copied().collect(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        }
    }

    #[test]
    fn a_principal_note_wakes_the_parked_sessions_serving_the_principal_or_an_alias() {
        let f = forum();
        let now = f.w.clock;
        let all = [f.a, f.b, f.c, f.d, f.e].map(|x| x.0);
        let ids = |xs: &[SessionId]| xs.iter().copied().collect::<BTreeSet<_>>();
        // From E to `mail:a@x`: A serves it, B serves its alias; C and D do not.
        let ev = entry(
            f.other,
            f.e,
            addressed_note(Addressee::Principal(f.p1), &[]),
        );
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.a, f.b]));
        // Only parked sessions wake.
        let ev = entry(
            f.other,
            f.e,
            addressed_note(Addressee::Principal(f.p1), &[]),
        );
        let c = concerned(&f.w.s, now, &[ev], &waiting(&[f.b.0, f.c.0]));
        assert_eq!(c.sessions, ids(&[f.b]));
        // A's own note to its principal does not wake A (review E4-spec-11).
        let ev = entry(
            f.other,
            f.a,
            addressed_note(Addressee::Principal(f.p1), &[]),
        );
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.b]), "the sender is left out");
        // `slack:t/u` has no alias: C alone.
        let ev = entry(
            f.other,
            f.e,
            addressed_note(Addressee::Principal(f.p3), &[]),
        );
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.c]));
    }

    #[test]
    fn a_holders_note_wakes_the_parked_holders_inside_the_branch_but_not_its_sender() {
        let f = forum();
        let now = f.w.clock;
        let all = [f.a, f.b, f.c, f.d, f.e].map(|x| x.0);
        let ids = |xs: &[SessionId]| xs.iter().copied().collect::<BTreeSet<_>>();
        // A holds /work/x, inside /work; B holds /other, outside it.
        let ev = entry(f.work, f.c, addressed_note(Addressee::Holders(f.work), &[]));
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.a]));
        assert_eq!(c.nodes, [f.work].into_iter().collect());
        // Not parked: nobody wakes.
        let ev = entry(f.work, f.c, addressed_note(Addressee::Holders(f.work), &[]));
        let c = concerned(&f.w.s, now, &[ev], &waiting(&[f.b.0, f.c.0]));
        assert!(c.sessions.is_empty());
        // A's own `holders:` note does not wake A (§13: "other than the sender").
        let ev = entry(f.work, f.a, addressed_note(Addressee::Holders(f.work), &[]));
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert!(c.sessions.is_empty(), "{:?}", c.sessions);
        // After A's lease ends, A no longer holds in the branch.
        let later = now.plus(Dur(16 * 60_000));
        let ev = entry(f.work, f.c, addressed_note(Addressee::Holders(f.work), &[]));
        let c = concerned(&f.w.s, later, &[ev], &waiting(&all));
        assert!(c.sessions.is_empty());
    }

    #[test]
    fn an_agent_question_wakes_its_recipients_and_an_answer_its_asker() {
        let f = forum();
        let now = f.w.clock;
        let all = [f.a, f.b, f.c, f.d, f.e].map(|x| x.0);
        let ids = |xs: &[SessionId]| xs.iter().copied().collect::<BTreeSet<_>>();
        let question = |to, recipients: &[SessionId]| {
            EntryBody::Question(QuestionBody::Agent {
                to,
                until: Time(u64::MAX),
                recipients: recipients.iter().copied().collect(),
                pending_holder: false,
            })
        };
        // To one session: its recipients.
        let ev = entry(f.other, f.e, question(Addressee::Session(f.d), &[f.d]));
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.d]));
        // To the holders of /work: A; asked by A, nobody.
        let ev = entry(f.work, f.c, question(Addressee::Holders(f.work), &[]));
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.a]));
        let ev = entry(f.work, f.a, question(Addressee::Holders(f.work), &[f.a]));
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert!(c.sessions.is_empty(), "the asker is left out");
        // An answer wakes the session that asked.
        let ev = entry(
            f.other,
            f.d,
            EntryBody::Answer {
                question: Eid(3),
                recipient: f.c,
                until: Time(u64::MAX),
            },
        );
        let c = concerned(&f.w.s, now, &[ev], &waiting(&all));
        assert_eq!(c.sessions, ids(&[f.c]));
        let c = concerned(
            &f.w.s,
            now,
            std::slice::from_ref(&entry(
                f.other,
                f.d,
                EntryBody::Answer {
                    question: Eid(3),
                    recipient: f.c,
                    until: Time(u64::MAX),
                },
            )),
            &waiting(&[f.a.0]),
        );
        assert!(c.sessions.is_empty(), "the asker is not parked");
    }

    #[tokio::test]
    async fn a_wake_between_registering_and_waiting_is_kept() {
        let mut w = Waiters::new();
        let p = w.register(SessionId(3), Some(NodeId(20)));
        let q = w.register(SessionId(4), None);
        assert_eq!(w.len(), 2);
        assert!(w.watched(NodeId(20)));
        // The commit comes before the handler awaits: the permit is kept.
        assert_eq!(w.wake_node(NodeId(20)), 1);
        tokio::time::timeout(Duration::from_secs(1), p.notify.notified())
            .await
            .expect("woken");
        // The other waiter was not woken.
        assert!(
            tokio::time::timeout(Duration::from_millis(20), q.notify.notified())
                .await
                .is_err()
        );
        assert_eq!(w.wake_session(SessionId(4)), 1);
        w.unregister(p.id);
        w.unregister(q.id);
        assert!(w.is_empty());
        assert!(!w.watched(NodeId(20)));
        assert_eq!(w.wake_session(SessionId(4)), 0);
    }

    #[test]
    fn a_hot_node_is_woken_at_most_once_a_second() {
        let mut c = Coalescer::new();
        let n = NodeId(20);
        let m = NodeId(21);
        assert_eq!(c.appended([n], Time(10_000)), vec![n]);
        // Ten appends in the next second: none wakes at once; one wake-up is deferred.
        for i in 1..=10 {
            assert!(c.appended([n], Time(10_000 + i * 90)).is_empty());
        }
        assert_eq!(c.next_due(), Some(Time(11_000)));
        assert!(c.due(Time(10_999)).is_empty());
        // Another node is not held back.
        assert_eq!(c.appended([m], Time(10_500)), vec![m]);
        assert_eq!(c.due(Time(11_000)), vec![n]);
        assert_eq!(c.next_due(), None);
        // Within a second of the deferred wake-up, the next append waits again.
        assert!(c.appended([n], Time(11_400)).is_empty());
        assert_eq!(c.due(Time(12_000)), vec![n]);
        // After a quiet second, an append wakes at once.
        assert_eq!(c.appended([n], Time(13_500)), vec![n]);
        assert!(c.last.len() <= 2, "old entries are forgotten");
    }

    #[test]
    fn news_seen_drops_a_session_whole() {
        let mut ns = NewsSeen::new();
        ns.set(SessionId(3), NodeId(20), Seq(4));
        ns.set(SessionId(3), NodeId(21), Seq(1));
        ns.set(SessionId(4), NodeId(20), Seq(2));
        assert_eq!(ns.get(SessionId(3), NodeId(20)), Some(Seq(4)));
        ns.drop_session(SessionId(3));
        assert_eq!(ns.len(), 1);
        assert_eq!(ns.get(SessionId(4), NodeId(20)), Some(Seq(2)));
    }
}
