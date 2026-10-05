//! The observer: it reads every event the core emits, from commands and from firings, and keeps
//! what the reports of D.15.4 need, from the events alone (finished entities leave the core's
//! memory, D.4.12, so it never looks them up later). It checks what the server must guarantee
//! independently of the core's own checks: a settlement, an answer, or a shown digest entry that
//! reaches another principal than the one it is for (CS-4, CS-5), and a digest entry addressed to
//! someone the question never concerned.

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    DigestId, DigestItem, Eid, EntryBody, Ev, Event, HoldEnd, Mark, NodeId, PrincipalId,
    QuestionBody, QuestionId, RequestId, Requester, SystemNote, Time, ToUserKind, USER_DIGEST,
    UqSource, WaitOn,
};

/// The most trace lines kept per node.
const TRACE_MAX: usize = 160;

/// What became of a question for the user.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UqOutcome {
    /// Still pending.
    Pending,
    /// Answered by this entry's principal (None: by a `heard` or an entry the observer did not
    /// see), at this stage.
    Answered {
        /// Whose `from_user` answered it.
        by: Option<PrincipalId>,
        /// The stage it was at.
        stage: u8,
    },
    /// Its default became the answer, at this stage.
    Defaulted {
        /// The stage.
        stage: u8,
    },
    /// Nobody answered by the last stage's due.
    Lapsed,
}

/// Why a question for the user was asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UqKind {
    /// An agent's `talk ask`.
    Agent,
    /// A blocking people question escalated.
    Escalation,
    /// Every active class refused the node (E1).
    Refusal,
}

/// A question for the user, as the events told it.
#[derive(Clone, Debug)]
pub struct Uq {
    /// When it was asked.
    pub created: Time,
    /// Why.
    pub kind: UqKind,
    /// The people question it escalates, for an escalation.
    pub escalates: Option<QuestionId>,
    /// The node it is about.
    pub about: Option<NodeId>,
    /// The chain.
    pub to: Vec<PrincipalId>,
    /// The current stage.
    pub stage: u8,
    /// With a default.
    pub has_default: bool,
    /// What became of it.
    pub outcome: UqOutcome,
    /// When it left `Pending`.
    pub resolved_at: Option<Time>,
    /// The assumption of its lapse was recorded (`UqAssumed`).
    pub assumed: bool,
    /// Nodes its lapse marked `proceed_on_assumption`.
    pub marked: u32,
}

/// A people question.
#[derive(Clone, Debug)]
pub struct Pq {
    /// Its node.
    pub node: NodeId,
    /// When it was asked.
    pub created: Time,
    /// Blocking.
    pub blocking: bool,
    /// Its end: "answered", "expired", "escalated", "superseded".
    pub end: Option<&'static str>,
}

/// A request.
#[derive(Clone, Debug)]
pub struct Req {
    /// Its node.
    pub node: NodeId,
    /// Who asked.
    pub requester: Requester,
    /// When it was recorded.
    pub created: Time,
    /// When it settled.
    pub settled: Option<Time>,
}

/// A digest entry.
#[derive(Clone, Debug)]
pub struct Dg {
    /// What it reports.
    pub item: DigestItem,
    /// For whom.
    pub to: PrincipalId,
    /// When.
    pub created: Time,
    /// When it was shown.
    pub shown: Option<Time>,
}

/// A contradiction of what the server must guarantee, with the object it concerns.
#[derive(Clone, Debug)]
pub struct Finding {
    /// When.
    pub at: Time,
    /// What kind.
    pub what: &'static str,
    /// The details.
    pub detail: String,
    /// The node whose trace shows it.
    pub node: Option<NodeId>,
}

/// Counters of the run.
#[derive(Clone, Debug, Default)]
pub struct Counts {
    /// Holds issued.
    pub holds: u64,
    /// Holds that expired (a session vanished, or overran its lease).
    pub expired: u64,
    /// Lapses, by time.
    pub lapses: Vec<Time>,
    /// `UqAssumed` events.
    pub uq_assumed: u64,
    /// `PqAssumed` events.
    pub pq_assumed: u64,
    /// `proceed_on_assumption` marks set.
    pub assume_marks: u64,
    /// `to_user` reports of kind result.
    pub results: u64,
    /// Reports an agent addressed to another person than a request's requester (injected mistakes).
    pub misaddressed_reports: u64,
    /// Digest entries an agent named in a `to_user` to another person (injected mistakes).
    pub misaddressed_shows: u64,
    /// `Refused` events (one session's `cannot`).
    pub refusals: u64,
    /// Nodes dropped after every active class refused (E1), with when.
    pub refusal_drops: Vec<(Time, NodeId)>,
    /// Requests recorded.
    pub requests: u64,
    /// Requests settled.
    pub settled: u64,
    /// Nodes closed.
    pub closed: u64,
    /// Nodes reopened.
    pub reopened: u64,
    /// Every event.
    pub events: u64,
}

/// What the events told.
#[derive(Clone, Debug, Default)]
pub struct Observer {
    /// The measurement starts here; questions and lapses before it are warm-up.
    pub measure_from: Time,
    /// The simulation's own knowledge: principal key → person.
    pub person_of_key: BTreeMap<String, usize>,
    /// The simulation's own knowledge: task node → team.
    pub node_team: BTreeMap<NodeId, usize>,
    /// The simulation's own knowledge: every person who has been a member of each team.
    pub team_members: Vec<BTreeSet<usize>>,
    /// Interned principals → person (None: `user`, or a key the simulation did not make).
    pub person: BTreeMap<PrincipalId, Option<usize>>,
    /// Principal ids by key.
    pub pid_of_key: BTreeMap<String, PrincipalId>,
    /// `from_user` entries → their principal.
    pub from_user: BTreeMap<Eid, PrincipalId>,
    /// `to_user` entries → their principal and kind.
    pub to_user: BTreeMap<Eid, (PrincipalId, ToUserKind)>,
    /// Requests.
    pub requests: BTreeMap<RequestId, Req>,
    /// Requests by node.
    pub node_requests: BTreeMap<NodeId, BTreeSet<RequestId>>,
    /// Questions for the user.
    pub uqs: BTreeMap<QuestionId, Uq>,
    /// People questions.
    pub pqs: BTreeMap<QuestionId, Pq>,
    /// Digest entries.
    pub digests: BTreeMap<DigestId, Dg>,
    /// Waits by node: when set and on what (cleared by `WaitSet { None }`).
    pub waits: BTreeMap<NodeId, (Time, WaitOn)>,
    /// Counters.
    pub n: Counts,
    /// Contradictions.
    pub findings: Vec<Finding>,
    /// Per node, its events in short form.
    pub traces: BTreeMap<NodeId, Vec<String>>,
    /// Nodes reopened since the simulation last read this.
    pub reopened_nodes: Vec<NodeId>,
}

impl Observer {
    /// Whether two principals are one person.
    #[must_use]
    pub fn same_person(&self, a: PrincipalId, b: PrincipalId) -> bool {
        if a == b {
            return true;
        }
        match (self.person.get(&a), self.person.get(&b)) {
            (Some(Some(x)), Some(Some(y))) => x == y,
            _ => false,
        }
    }

    /// The person of a principal, if the simulation made it.
    #[must_use]
    pub fn person_of(&self, p: PrincipalId) -> Option<usize> {
        self.person.get(&p).copied().flatten()
    }

    fn trace(&mut self, node: NodeId, at: Time, line: String) {
        if node.0 < forum_core::FIRST_FREE_NODE.0 && node != USER_DIGEST {
            return;
        }
        let t = self.traces.entry(node).or_default();
        if t.len() < TRACE_MAX {
            t.push(format!("{} {line}", stamp(at)));
        }
    }

    fn find(&mut self, at: Time, what: &'static str, detail: String, node: Option<NodeId>) {
        self.findings.push(Finding {
            at,
            what,
            detail,
            node,
        });
    }

    /// The persons who made requests on `n` (every request ever recorded there).
    fn requesters_on(&self, n: NodeId) -> BTreeSet<Option<usize>> {
        self.node_requests
            .get(&n)
            .into_iter()
            .flatten()
            .filter_map(|r| self.requests.get(r))
            .filter_map(|r| match r.requester {
                Requester::User(p) => Some(self.person_of(p)),
                Requester::External => None,
            })
            .collect()
    }

    /// Whether `to` is a principal a digest entry about `item` may be for: someone whose request is
    /// on the node concerned, someone the question was put to, an owner of the work (a member of
    /// the node's team), and `user` only when there is nobody else (§8.5, a loose form of D1).
    fn audience_ok(&self, item: &DigestItem, to: PrincipalId) -> bool {
        let (node, asked): (Option<NodeId>, Vec<PrincipalId>) = match item {
            DigestItem::DefaultApplied { question } | DigestItem::Lapsed { question } => {
                let u = self.uqs.get(question);
                (
                    u.and_then(|u| u.about),
                    u.map(|u| u.to.clone()).unwrap_or_default(),
                )
            }
            DigestItem::RefusalDrop { node, question } => (
                Some(*node),
                self.uqs
                    .get(question)
                    .map(|u| u.to.clone())
                    .unwrap_or_default(),
            ),
            DigestItem::Assumption { question } => (self.pqs.get(question).map(|q| q.node), vec![]),
        };
        let mut allowed: BTreeSet<Option<usize>> = BTreeSet::new();
        if let Some(n) = node {
            allowed.extend(self.requesters_on(n));
            if let Some(&t) = self.node_team.get(&n)
                && let Some(m) = self.team_members.get(t)
            {
                allowed.extend(m.iter().map(|&p| Some(p)));
            }
        }
        allowed.extend(
            asked
                .iter()
                .filter(|&&p| p != forum_core::USER_PRINCIPAL)
                .map(|&p| self.person_of(p)),
        );
        let to_person = self.person_of(to);
        if to == forum_core::USER_PRINCIPAL {
            // `user` only when nobody else is concerned.
            return allowed.iter().all(Option::is_none);
        }
        allowed.contains(&to_person)
    }

    /// Reads the events of one command or one batch of firings.
    pub fn events(&mut self, evs: &[Event]) {
        for e in evs {
            self.event(e);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per event the reports read; splitting it hides the table"
    )]
    fn event(&mut self, e: &Event) {
        self.n.events += 1;
        let at = e.at;
        match &e.ev {
            Ev::PrincipalSeen { id, key } => {
                let p = self.person_of_key.get(key.as_str()).copied();
                self.person.insert(*id, p);
                self.pid_of_key.insert(key.as_str().to_string(), *id);
            }
            Ev::Appended {
                eid, node, body, ..
            } => self.appended(at, *eid, *node, body),
            Ev::RequestMoved { request, to } => {
                if let Some(r) = self.requests.get_mut(request) {
                    let from = r.node;
                    r.node = *to;
                    if let Some(s) = self.node_requests.get_mut(&from) {
                        s.remove(request);
                    }
                    self.node_requests.entry(*to).or_default().insert(*request);
                }
            }
            Ev::RequestAnswered { request, by } => {
                self.check_answer(at, *request, *by, "answer to another principal");
            }
            Ev::RequestSettled {
                request, answer, ..
            } => {
                self.n.settled += 1;
                if let Some(r) = self.requests.get_mut(request) {
                    r.settled = Some(at);
                }
                self.check_answer(
                    at,
                    *request,
                    *answer,
                    "settlement by a report to another principal",
                );
                let node = self.requests.get(request).map(|r| r.node);
                if let Some(n) = node {
                    self.trace(
                        n,
                        at,
                        format!("request e{} settled by e{}", request.0, answer.0),
                    );
                }
            }
            Ev::UqReaddressed { q, stage, due } => {
                if let Some(u) = self.uqs.get_mut(q) {
                    u.stage = *stage;
                    let about = u.about;
                    let to = u.to.get(usize::from(*stage)).copied();
                    if let Some(n) = about {
                        self.trace(
                            n,
                            at,
                            format!(
                                "question e{} re-addressed to stage {stage} (p{}), due {}",
                                q.0,
                                to.map_or(0, |p| p.0),
                                stamp(*due)
                            ),
                        );
                    }
                }
            }
            Ev::UqAnswered { q, by } => {
                let by_p = self.from_user.get(by).copied();
                if let Some(u) = self.uqs.get_mut(q) {
                    u.outcome = UqOutcome::Answered {
                        by: by_p,
                        stage: u.stage,
                    };
                    u.resolved_at = Some(at);
                    let about = u.about;
                    if let Some(n) = about {
                        self.trace(
                            n,
                            at,
                            format!(
                                "question e{} answered by e{} (p{})",
                                q.0,
                                by.0,
                                by_p.map_or(0, |p| p.0)
                            ),
                        );
                    }
                }
            }
            Ev::UqDefaulted { q } => {
                if let Some(u) = self.uqs.get_mut(q) {
                    u.outcome = UqOutcome::Defaulted { stage: u.stage };
                    u.resolved_at = Some(at);
                    if let Some(n) = u.about {
                        self.trace(n, at, format!("question e{} defaulted", q.0));
                    }
                }
            }
            Ev::UqLapsed { q } => {
                self.n.lapses.push(at);
                if let Some(u) = self.uqs.get_mut(q) {
                    u.outcome = UqOutcome::Lapsed;
                    u.resolved_at = Some(at);
                    if let Some(n) = u.about {
                        self.trace(n, at, format!("question e{} lapsed", q.0));
                    }
                }
            }
            Ev::UqAssumed { q, .. } => {
                self.n.uq_assumed += 1;
                if let Some(u) = self.uqs.get_mut(q) {
                    u.assumed = true;
                }
            }
            Ev::PqAssumed { .. } => self.n.pq_assumed += 1,
            Ev::PqExpired { q } => self.pq_end(at, *q, "expired"),
            Ev::PqEscalated { q, uq } => {
                self.pq_end(at, *q, "escalated");
                let _ = uq;
            }
            Ev::PqAnswered { q, .. } => self.pq_end(at, *q, "answered"),
            Ev::PqSuperseded { q, .. } => self.pq_end(at, *q, "superseded"),
            Ev::PqWithdrawn { q, .. } => self.pq_end(at, *q, "withdrawn"),
            Ev::MarkSet {
                node,
                mark,
                question,
                on,
            } => {
                if *mark == Mark::Assume && *on {
                    self.n.assume_marks += 1;
                    if let Some(u) = self.uqs.get_mut(question) {
                        u.marked += 1;
                    }
                }
                self.trace(
                    *node,
                    at,
                    format!(
                        "mark {mark:?} for e{} {}",
                        question.0,
                        if *on { "on" } else { "off" }
                    ),
                );
            }
            Ev::DigestShown { entry, by } => {
                let shown_to = self.to_user.get(by).map(|x| x.0);
                if let Some(d) = self.digests.get_mut(entry) {
                    d.shown = Some(at);
                    let (to, item) = (d.to, d.item);
                    if let Some(p) = shown_to
                        && !self.same_person(p, to)
                    {
                        self.find(
                            at,
                            "digest entry shown to another principal",
                            format!(
                                "digest e{} for p{} shown by e{} to p{} ({item:?})",
                                entry.0, to.0, by.0, p.0
                            ),
                            None,
                        );
                    }
                }
            }
            Ev::HoldIssued { token, session, .. } => {
                self.n.holds += 1;
                self.trace(
                    token.node,
                    at,
                    format!("hold {}.{} by s-{}", token.node.0, token.fence.0, session.0),
                );
            }
            Ev::HoldEnded { token, end } => {
                if *end == HoldEnd::Expired {
                    self.n.expired += 1;
                }
                self.trace(
                    token.node,
                    at,
                    format!("hold {}.{} ended {end:?}", token.node.0, token.fence.0),
                );
            }
            Ev::WaitSet { node, wait } => {
                match wait {
                    Some(w) => {
                        self.waits.insert(*node, (at, *w));
                    }
                    None => {
                        self.waits.remove(node);
                    }
                }
                self.trace(*node, at, format!("wait {wait:?}"));
            }
            Ev::NodeClosed { node, outcome, .. } => {
                self.n.closed += 1;
                self.waits.remove(node);
                self.trace(*node, at, format!("closed {outcome:?}"));
            }
            Ev::NodeReopened { node } => {
                self.n.reopened += 1;
                self.reopened_nodes.push(*node);
                self.trace(*node, at, "reopened".to_string());
            }
            Ev::Refused { node, session, .. } => {
                self.n.refusals += 1;
                self.trace(*node, at, format!("refused by s-{}", session.0));
            }
            Ev::RefusalsCleared { node, .. } => {
                self.trace(*node, at, "refusals cleared".to_string());
            }
            Ev::NodeCreated { id, .. } => self.trace(*id, at, "created".to_string()),
            _ => {}
        }
    }

    fn pq_end(&mut self, at: Time, q: QuestionId, end: &'static str) {
        if let Some(p) = self.pqs.get_mut(&q) {
            if p.end.is_none() || end == "answered" {
                p.end = Some(end);
            }
            let n = p.node;
            self.trace(n, at, format!("people question e{} {end}", q.0));
        }
    }

    fn check_answer(&mut self, at: Time, request: RequestId, by: Eid, what: &'static str) {
        let Some(r) = self.requests.get(&request) else {
            return;
        };
        let Requester::User(p) = r.requester else {
            return;
        };
        let node = r.node;
        match self.to_user.get(&by) {
            Some(&(to, kind)) => {
                if !self.same_person(to, p) || kind != ToUserKind::Result {
                    self.find(
                        at,
                        what,
                        format!(
                            "request e{} of p{} answered by e{} ({kind:?} to p{})",
                            request.0, p.0, by.0, to.0
                        ),
                        Some(node),
                    );
                }
            }
            None => self.find(
                at,
                what,
                format!(
                    "user request e{} of p{} answered by e{}, which is no report to a principal",
                    request.0, p.0, by.0
                ),
                Some(node),
            ),
        }
    }

    fn appended(&mut self, at: Time, eid: Eid, node: NodeId, body: &EntryBody) {
        match body {
            EntryBody::FromUser { principal } => {
                self.from_user.insert(eid, *principal);
            }
            EntryBody::ToUser {
                kind,
                re,
                principal,
                ..
            } => {
                self.to_user.insert(eid, (*principal, *kind));
                if *kind == ToUserKind::Result {
                    self.n.results += 1;
                }
                for x in re {
                    if let Some(r) = self.requests.get(x)
                        && let Requester::User(p) = r.requester
                        && !self.same_person(p, *principal)
                    {
                        self.n.misaddressed_reports += 1;
                        let n = r.node;
                        self.trace(
                            n,
                            at,
                            format!(
                                "report e{} to p{} names e{} of p{}",
                                eid.0, principal.0, x.0, p.0
                            ),
                        );
                    }
                    if let Some(d) = self.digests.get(x)
                        && !self.same_person(d.to, *principal)
                    {
                        self.n.misaddressed_shows += 1;
                    }
                }
            }
            EntryBody::Request { requester, .. } => {
                self.n.requests += 1;
                self.requests.insert(
                    eid,
                    Req {
                        node,
                        requester: *requester,
                        created: at,
                        settled: None,
                    },
                );
                self.node_requests.entry(node).or_default().insert(eid);
                self.trace(
                    node,
                    at,
                    format!("request e{eid:?} from {requester:?}", eid = eid.0),
                );
            }
            EntryBody::Question(QuestionBody::User {
                source,
                about,
                to,
                has_default,
                due,
            }) => {
                let (kind, escalates) = match source {
                    UqSource::Agent { .. } => (UqKind::Agent, None),
                    UqSource::Escalation { question } => (UqKind::Escalation, Some(*question)),
                    UqSource::Refusal { .. } => (UqKind::Refusal, None),
                };
                self.uqs.insert(
                    eid,
                    Uq {
                        created: at,
                        kind,
                        escalates,
                        about: *about,
                        to: to.clone(),
                        stage: 0,
                        has_default: *has_default,
                        outcome: UqOutcome::Pending,
                        resolved_at: None,
                        assumed: false,
                        marked: 0,
                    },
                );
                if let Some(n) = about {
                    let chain: Vec<String> = to.iter().map(|p| format!("p{}", p.0)).collect();
                    self.trace(
                        *n,
                        at,
                        format!(
                            "question e{} ({kind:?}, default {has_default}) to [{}], due {}",
                            eid.0,
                            chain.join(", "),
                            stamp(*due)
                        ),
                    );
                }
            }
            EntryBody::Question(QuestionBody::People { blocking, .. }) => {
                self.pqs.insert(
                    eid,
                    Pq {
                        node,
                        created: at,
                        blocking: *blocking,
                        end: None,
                    },
                );
                self.trace(
                    node,
                    at,
                    format!("people question e{} (blocking {blocking})", eid.0),
                );
            }
            EntryBody::System(SystemNote::Digest { item, to }) if node == USER_DIGEST => {
                if !self.audience_ok(item, *to) {
                    let about = match item {
                        DigestItem::DefaultApplied { question }
                        | DigestItem::Lapsed { question } => {
                            self.uqs.get(question).and_then(|u| u.about)
                        }
                        DigestItem::RefusalDrop { node, .. } => Some(*node),
                        DigestItem::Assumption { question } => {
                            self.pqs.get(question).map(|q| q.node)
                        }
                    };
                    self.find(
                        at,
                        "digest entry for a principal the item does not concern",
                        format!("digest e{} for p{}: {item:?}", eid.0, to.0),
                        about,
                    );
                }
                if let DigestItem::RefusalDrop { node: n, .. } = item
                    && !self.n.refusal_drops.iter().any(|(_, x)| x == n)
                {
                    self.n.refusal_drops.push((at, *n));
                }
                self.digests.insert(
                    eid,
                    Dg {
                        item: *item,
                        to: *to,
                        created: at,
                        shown: None,
                    },
                );
            }
            EntryBody::Decision {
                settles, assumes, ..
            } => {
                if !settles.is_empty() || !assumes.is_empty() {
                    self.trace(
                        node,
                        at,
                        format!(
                            "decision e{} settles {} assumes {}",
                            eid.0,
                            settles.len(),
                            assumes.len()
                        ),
                    );
                }
            }
            EntryBody::Said { kind, .. } => self.trace(node, at, format!("said {kind:?}")),
            EntryBody::Heard { answers, .. } => {
                self.trace(
                    node,
                    at,
                    format!("heard, answers {:?}", answers.map(|q| q.0)),
                );
            }
            EntryBody::Checkpoint {
                release: Some(r), ..
            } => {
                self.trace(node, at, format!("released {r:?}"));
            }
            _ => {}
        }
    }
}

/// A time as days, hours, and minutes since the run's start (`crate::START`, day 0 of the
/// warm-up), for traces.
#[must_use]
pub fn stamp(t: Time) -> String {
    let ms = t.0.saturating_sub(crate::START.0);
    let day = ms / forum_core::DAY.0;
    let rest = ms % forum_core::DAY.0;
    let h = rest / forum_core::HOUR.0;
    let m = (rest % forum_core::HOUR.0) / forum_core::MINUTE.0;
    format!("d{day}+{h:02}:{m:02}")
}
