//! Random event sequences (design D.12.3, items 5, 11, and R1): a deterministic generator draws
//! events of every variant from the current state, mostly valid, sometimes not, and after every
//! event requires that the dirty set covers every changed fact, that the indices equal their
//! recount (R1) and the queue its recount (T4), that an event counted as an anomaly changed nothing
//! in the base and history parts beyond what a pure anomaly at the same time changes (D.7.5), and,
//! while nothing is overdue, that the flags equal `spec::flags` (F) and the candidates
//! `spec::classify` (D.8.5). The log replays to the state. Some events are drawn to fail one
//! precondition: a hold whose ceiling passes its taker's retirement, a settlement by a decision that
//! does not list the request, and a firing before its time.
//!
//! proptest is not a dependency of `forum-core` (the full-universe proptest of `tests/prop.rs` is
//! WP-D3's to WP-D6's, through `apply`); this generator exercises `evolve` alone, on states no
//! command sequence would reach as well as on those it would.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::event::{
    ConfirmVia, EndCause, EntryText, Extension, IntentEnd, NextOutcome, NodeInit, NodePatch,
};
use crate::params::Params;
use crate::state::{
    BasisRef, Cand, DeadlineKey, DeadlineKind, DigestItem, LeaseTerms, Occurrence, Owner, Restored,
    Role, SignalKey, WaitOn,
};
use crate::testkit::World;
use crate::types::{
    Addressee, ClassName, HOUR, Key, KeyStr, Label, LinkRel, MINUTE, Mark, Name, NodeId, NormVer,
    OpId, Outcome, PrincipalKey, ReqDigest, Requester, SaidKind, SignalKind, ToUserKind, Token,
};

/// xorshift64*: a small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn pick<T: Clone>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() {
            None
        } else {
            Some(v[self.below(v.len() as u64) as usize].clone())
        }
    }
    fn subset<T: Clone + Ord>(&mut self, v: &[T]) -> BTreeSet<T> {
        v.iter().filter(|_| self.chance(30)).cloned().collect()
    }
}

const NAMES: [&str; 5] = ["a", "b", "c", "d", "e"];
const KEYS: [&str; 6] = [
    "chat:x/1",
    "chat:x/2",
    "issue:p#1",
    "lean:t1",
    "person:mail:a@x",
    "person:mail:b@x",
];
const PRINCIPALS: [&str; 3] = ["mail:a@x", "mail:b@x", "slack:t/u"];

fn label(s: &str) -> Label {
    Label::new(s).unwrap()
}

fn class(s: &str) -> ClassName {
    ClassName::new(s).unwrap()
}

fn keystr(s: &str) -> KeyStr {
    KeyStr::new(s).unwrap()
}

/// What the generator draws ids from.
struct Pool {
    nodes: Vec<NodeId>,
    sessions: Vec<SessionId>,
    principals: Vec<PrincipalId>,
    holds: Vec<Token>,
    requests: Vec<Eid>,
    pqs: Vec<Eid>,
    aqs: Vec<Eid>,
    uqs: Vec<Eid>,
    commitments: Vec<Eid>,
    saids: Vec<Eid>,
    decisions: Vec<Eid>,
    objections: Vec<Eid>,
    user_entries: Vec<Eid>,
    notes: Vec<Eid>,
    entries: Vec<Eid>,
    signals: Vec<SignalKey>,
    pauses: Vec<NodeId>,
    refusals: Vec<(NodeId, ClassName)>,
    series: Vec<NodeId>,
    occurrences: Vec<NodeId>,
    signal_nodes: Vec<NodeId>,
    acks: Vec<Eid>,
    open_pqs: Vec<Eid>,
    pending_uqs: Vec<Eid>,
    to_users: Vec<Eid>,
    pending_digest: Vec<Eid>,
    open_commitments: Vec<Eid>,
}

impl Pool {
    fn of(s: &State) -> Pool {
        let mut nodes: Vec<NodeId> = s.nodes.keys().copied().collect();
        nodes.push(NodeId(999));
        let mut sessions: Vec<SessionId> = s.sessions.keys().copied().collect();
        sessions.push(SessionId(999));
        Pool {
            nodes,
            sessions,
            principals: s.principal_ids.values().copied().collect(),
            holds: s.holds.keys().copied().collect(),
            requests: s.requests.keys().copied().collect(),
            pqs: s.pquestions.keys().copied().collect(),
            aqs: s.aquestions.keys().copied().collect(),
            uqs: s.uquestions.keys().copied().collect(),
            commitments: s.commitments.keys().copied().collect(),
            saids: s.saids.keys().copied().collect(),
            decisions: s.decisions.keys().copied().collect(),
            objections: s.objections.keys().copied().collect(),
            user_entries: s.user_entries.keys().copied().collect(),
            notes: s.notes.keys().copied().collect(),
            entries: s.entries.keys().copied().collect(),
            signals: s.signals.keys().copied().collect(),
            pauses: s.pauses.keys().copied().collect(),
            refusals: s.refusals.keys().cloned().collect(),
            series: s
                .nodes
                .iter()
                .filter(|(_, n)| matches!(n.role, Role::Series(_)))
                .map(|(&i, _)| i)
                .collect(),
            occurrences: s
                .nodes
                .iter()
                .filter(|(_, n)| matches!(n.role, Role::Occurrence(_)))
                .map(|(&i, _)| i)
                .collect(),
            signal_nodes: s
                .nodes
                .iter()
                .filter(|(_, n)| matches!(n.role, Role::Signal(_)))
                .map(|(&i, _)| i)
                .collect(),
            acks: s
                .saids
                .iter()
                .filter(|(_, x)| x.kind == SaidKind::Ack)
                .map(|(&i, _)| i)
                .collect(),
            open_pqs: s
                .pquestions
                .iter()
                .filter(|(_, x)| x.state == crate::state::PqState::Open)
                .map(|(&i, _)| i)
                .collect(),
            pending_uqs: s
                .uquestions
                .iter()
                .filter(|(_, x)| x.state == UqState::Pending)
                .map(|(&i, _)| i)
                .collect(),
            to_users: s
                .entries
                .iter()
                .filter(|(_, m)| m.ty == crate::types::EntryType::ToUser)
                .map(|(&i, _)| i)
                .collect(),
            pending_digest: s
                .digest
                .iter()
                .filter(|(_, x)| x.state == crate::state::DigestState::Pending)
                .map(|(&i, _)| i)
                .collect(),
            open_commitments: s
                .commitments
                .iter()
                .filter(|(_, x)| x.state == crate::state::CommitmentState::Open)
                .map(|(&i, _)| i)
                .collect(),
        }
    }
}

fn eid_of(rng: &mut Rng, v: &[Eid]) -> Eid {
    rng.pick(v).unwrap_or(Eid(9_999))
}

fn later(rng: &mut Rng, now: Time) -> Time {
    now.plus(Dur(rng.below(6) * 10 * MINUTE.0))
}

fn random_init(rng: &mut Rng, p: &Pool, s: &State, now: Time) -> NodeInit {
    let labels = [label("x"), label("y")];
    let keys: Vec<KeyStr> = KEYS.iter().map(|k| keystr(k)).collect();
    let mut tags = BTreeSet::new();
    if rng.chance(40) {
        tags.insert(label("offer"));
    }
    let role = match rng.below(10) {
        0 => Role::Occurrence(Occurrence {
            series: rng.pick(&p.nodes).unwrap(),
            slot: later(rng, now),
            window_end: later(rng, now).plus(HOUR),
            phase: if rng.chance(50) {
                crate::state::OccPhase::Waiting
            } else {
                crate::state::OccPhase::Available
            },
            window_fired: false,
        }),
        1 => {
            let (a, b) = (rng.pick(&p.nodes).unwrap(), rng.pick(&p.nodes).unwrap());
            Role::Signal(SignalKey {
                kind: SignalKind::Duplicate,
                a: a.min(b),
                b: a.max(b),
            })
        }
        2 => Role::Series(crate::state::Series {
            repeat: crate::state::Repeat {
                cron_text: Text("0 7 * * *".into()),
                cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
                tz: forum_cron::TzTable {
                    name: String::from("UTC"),
                    tzdb_version: String::new(),
                    initial: 0,
                    transitions: Vec::new(),
                },
                run_within: HOUR,
            },
            last_missed: rng.chance(30),
        }),
        _ => Role::Plain,
    };
    let _ = s;
    NodeInit {
        title: Text("t".into()),
        body: Text::default(),
        fields: BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        // `ix.agreements` (D.14.5).
        agreements: if rng.chance(20) {
            Text("a".into())
        } else {
            Text::default()
        },
        kind: if rng.chance(20) {
            Some(label("k"))
        } else {
            None
        },
        tags,
        capacity: rng.below(3) as u32,
        priority: rng.below(3) as i32 - 1,
        est: match rng.below(3) {
            0 => None,
            1 => Some(5),
            _ => Some(100),
        },
        needs: rng.subset(&labels),
        due: if rng.chance(30) {
            Some(later(rng, now))
        } else {
            None
        },
        depends_on: rng.subset(&p.nodes[..p.nodes.len() - 1]),
        owners: rng.subset(&p.nodes[..p.nodes.len() - 1]),
        keys: rng
            .subset(&keys)
            .into_iter()
            .map(|k| (k, NormVer(1)))
            .collect(),
        links: if rng.chance(30) {
            [(LinkRel::DerivedFrom, rng.pick(&p.nodes).unwrap())].into()
        } else {
            BTreeSet::new()
        },
        role,
    }
}

fn random_body(rng: &mut Rng, p: &Pool, s: &State, now: Time) -> EntryBody {
    let conv = keystr(rng.pick(&KEYS[..3]).unwrap());
    let sess = rng.pick(&p.sessions).unwrap();
    match rng.below(18) {
        0 | 1 => {
            let to = match rng.below(5) {
                0 => None,
                1 => Some(Addressee::Session(sess)),
                2 => Some(Addressee::Holder(rng.pick(&p.nodes).unwrap())),
                3 => Some(Addressee::Holders(rng.pick(&p.nodes).unwrap())),
                _ => Some(Addressee::Principal(
                    rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
                )),
            };
            EntryBody::Note {
                to,
                until: Some(later(rng, now).plus(MINUTE)),
                recipients: rng.subset(&p.sessions),
                pending_holder: rng.chance(30),
                expects: None,
                reply_to: None,
            }
        }
        2 | 3 => {
            let n = s.counters.next_eid.0;
            let user = rng.pick(&p.user_entries);
            let requester = match (user, rng.pick(&p.principals)) {
                (Some(_), Some(x)) if rng.chance(50) => Requester::User(x),
                _ => Requester::External,
            };
            EntryBody::Request {
                origin: Key {
                    key: keystr(&alloc::format!("chat:m/{n}")),
                    nv: NormVer(1),
                },
                conv,
                from_user: if matches!(requester, Requester::User(_)) {
                    user
                } else {
                    None
                },
                requester,
            }
        }
        4 => EntryBody::Question(crate::event::QuestionBody::People {
            conv,
            blocking: rng.chance(50),
            said: eid_of(rng, &p.saids),
            supersedes: None,
            recheck_at: later(rng, now).plus(MINUTE),
        }),
        5 => EntryBody::Question(crate::event::QuestionBody::Agent {
            to: Addressee::Session(sess),
            until: later(rng, now).plus(MINUTE),
            recipients: [sess].into(),
            pending_holder: false,
        }),
        6 => {
            let mut to: Vec<PrincipalId> = Vec::new();
            if let Some(x) = rng.pick(&p.principals)
                && x != USER_PRINCIPAL
            {
                to.push(x);
            }
            to.push(USER_PRINCIPAL);
            let source = match rng.below(3) {
                0 => UqSource::Agent {
                    session: sess,
                    digest: ReqDigest([0; 32]),
                },
                1 => UqSource::Escalation {
                    question: eid_of(rng, &p.pqs),
                },
                _ => {
                    // `e1_check` asks only while no refusal question about the node is pending
                    // (D.8.14): at most one per node.
                    let n = rng.pick(&p.nodes).unwrap();
                    if s.ix.refusal_question.contains_key(&n) {
                        UqSource::Agent {
                            session: sess,
                            digest: ReqDigest([0; 32]),
                        }
                    } else {
                        UqSource::Refusal { node: n }
                    }
                }
            };
            EntryBody::Question(crate::event::QuestionBody::User {
                source,
                about: if rng.chance(70) {
                    rng.pick(&p.nodes)
                } else {
                    None
                },
                to,
                due: later(rng, now).plus(MINUTE),
                has_default: rng.chance(50),
            })
        }
        7 => EntryBody::Answer {
            question: eid_of(rng, &p.aqs),
            recipient: sess,
            until: later(rng, now).plus(MINUTE),
        },
        8 | 9 => {
            let mut basis = Vec::new();
            if rng.chance(60) {
                basis.push(BasisRef::Node {
                    id: rng.pick(&p.nodes).unwrap(),
                    version: None,
                    life_at: match rng.below(3) {
                        0 => None,
                        1 => Some(Outcome::Done),
                        _ => Some(Outcome::Failed),
                    },
                });
            }
            if rng.chance(40) {
                basis.push(BasisRef::Entry(eid_of(rng, &p.decisions)));
            }
            EntryBody::Decision {
                settles: rng.subset(&p.requests),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis,
            }
        }
        // 1.12: a checkpoint's flag of a pull request named, a `handoff` release's keeping, and a
        // hold check, whose flag sets the node's.
        10 if rng.chance(20) => EntryBody::IntentChecked {
            intent: eid_of(rng, &p.entries),
            pr_named: rng.chance(50),
        },
        10 => {
            let handoff = rng.chance(20);
            EntryBody::Checkpoint {
                token: rng.pick(&p.holds).unwrap_or(Token {
                    node: NodeId(0),
                    fence: Fence(0),
                }),
                release: handoff.then_some(crate::types::ReleaseReason::Handoff),
                pr_named: match rng.below(3) {
                    0 => None,
                    1 => Some(false),
                    _ => Some(true),
                },
                handed_off_until: handoff.then(|| later(rng, now)),
            }
        }
        11 | 12 => EntryBody::Said {
            conv,
            kind: match rng.below(4) {
                0 => SaidKind::Ack,
                1 => SaidKind::Reply,
                2 => SaidKind::Result,
                _ => SaidKind::Correction,
            },
            corrects: None,
            // 1.11: the voice by its cap, stored only.
            voice: rng.chance(20),
        },
        13 => EntryBody::FromUser {
            principal: rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
        },
        14 => EntryBody::ToUser {
            kind: ToUserKind::Result,
            verbatim: true,
            re: BTreeSet::new(),
            principal: rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
        },
        15 => match rng.below(4) {
            0 => EntryBody::Object,
            1 => EntryBody::Approve {
                version: crate::types::Version(1),
            },
            2 => EntryBody::Heard {
                conv,
                answers: None,
            },
            _ => {
                let upheld = rng.chance(30);
                EntryBody::Resolve {
                    objection: eid_of(rng, &p.objections),
                    upheld,
                    // 1.11: a deferral, which marks an open objection.
                    deferred: !upheld && rng.chance(25),
                }
            }
        },
        16 => EntryBody::Retract {
            target: if rng.chance(50) {
                BasisTarget::Node(rng.pick(&p.nodes).unwrap())
            } else {
                BasisTarget::Entry(eid_of(rng, &p.decisions))
            },
        },
        _ => EntryBody::System(SystemNote::Digest {
            item: DigestItem::Lapsed {
                question: eid_of(rng, &p.uqs),
            },
            to: rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
        }),
    }
}

/// One random event, mostly valid on `s` at `now`.
fn random_event(rng: &mut Rng, s: &State, now: Time) -> Ev {
    let p = Pool::of(s);
    let params = &s.params;
    let node = rng.pick(&p.nodes).unwrap();
    let sess = rng.pick(&p.sessions).unwrap();
    let terms = LeaseTerms {
        first: params.lease_first,
        default: params.lease_default,
        max: params.lease_max,
        ceiling: params.lease_ceiling,
        renew_max: params.renew_max,
    };
    match rng.below(64) {
        0..=3 => Ev::SessionOpened {
            session: s.counters.next_session,
            class: class(if rng.chance(50) { "a" } else { "b" }),
            caps: rng.subset(&[label("x"), label("y")]),
            horizon: if rng.chance(50) { None } else { Some(60) },
            intent: None,
            silence_at: now.plus(params.presence_silence),
            retire_at: now.plus(params.session_retain),
            principal: rng.pick(&p.principals).filter(|_| rng.chance(30)),
        },
        4 => Ev::SessionHello {
            session: sess,
            caps: if rng.chance(50) {
                Some(rng.subset(&[label("x"), label("y")]))
            } else {
                None
            },
            horizon: if rng.chance(50) { Some(30) } else { None },
            intent: None,
            principal: rng.pick(&p.principals).filter(|_| rng.chance(50)),
        },
        5 => Ev::PrincipalSeen {
            id: s.counters.next_principal,
            key: PrincipalKey::new(rng.pick(&PRINCIPALS).unwrap()).unwrap(),
        },
        6 => Ev::Touched {
            session: sess,
            silence_at: now.plus(params.presence_silence),
            retire_at: s
                .sessions
                .get(&sess)
                .map_or(now, |x| x.retire_at)
                .max(now.plus(params.session_retain)),
        },
        7 => Ev::InboxRead {
            session: sess,
            notes: rng.subset(&p.notes),
        },
        8 => Ev::Shown {
            session: sess,
            node,
            seq: s.nodes.get(&node).map_or(Seq(1), |n| n.seq),
        },
        9 => Ev::NextCalled {
            session: sess,
            outcome: NextOutcome::Nothing,
            open: rng.chance(60),
            class_window_end: now.plus(params.refusal_window),
        },
        10..=14 => {
            let n = if rng.chance(30) {
                String::from(rng.pick(&NAMES).unwrap())
            } else {
                alloc::format!("n{}", s.counters.next_node.0)
            };
            Ev::NodeCreated {
                id: s.counters.next_node,
                parent: if rng.chance(70) {
                    rng.pick(&p.nodes[..p.nodes.len() - 1]).unwrap()
                } else {
                    node
                },
                name: Name::new(&n).unwrap(),
                origin: match rng.below(4) {
                    0 => Origin::Ancestor(sess),
                    1 => Origin::Occurrence,
                    _ => Origin::Agent(sess),
                },
                init: random_init(rng, &p, s, now),
            }
        }
        15 | 16 => {
            let mut patch = NodePatch::default();
            match rng.below(10) {
                0 => patch.title = Some(Text(alloc::format!("t{}", rng.below(9)))),
                1 => patch.status = Some(Text(alloc::format!("s{}", rng.below(9)))),
                2 => patch.capacity = Some(rng.below(3) as u32),
                3 => patch.priority = Some(rng.below(4) as i32),
                4 => patch.needs = Some(rng.subset(&[label("x"), label("y")])),
                5 => patch.est = Change::Set(rng.below(200) as u32),
                6 => patch.due = Change::Set(later(rng, now)),
                7 => patch.depends_on = Some(rng.subset(&p.nodes)),
                // Set or clear the agreements (`ix.agreements`, D.14.5).
                8 => {
                    patch.agreements = Some(if rng.chance(50) {
                        Text("b".into())
                    } else {
                        Text::default()
                    });
                }
                _ => patch.tags = Some(rng.subset(&[label("offer"), label("x")])),
            }
            Ev::NodeUpdated { node, patch }
        }
        17 => Ev::KeyAdded {
            node,
            key: Key {
                key: keystr(rng.pick(&KEYS).unwrap()),
                nv: NormVer(1),
            },
        },
        18 => Ev::KeyRemoved {
            node,
            key: s
                .nodes
                .get(&node)
                .and_then(|n| n.keys.keys().next().cloned())
                .unwrap_or_else(|| keystr("chat:x/1")),
        },
        19 => Ev::LinkAdded {
            node,
            rel: LinkRel::Continues,
            to: rng.pick(&p.nodes).unwrap(),
        },
        20 | 21 => Ev::NodeMoved {
            node,
            parent: rng.pick(&p.nodes).unwrap(),
            name: Name::new(rng.pick(&NAMES).unwrap()).unwrap(),
        },
        22 | 23 => Ev::NodeClosed {
            node,
            outcome: match rng.below(4) {
                0 => Outcome::Done,
                1 => Outcome::Failed,
                2 => Outcome::Dropped,
                _ => Outcome::Superseded,
            },
            // `merge` refuses a closed `into` (D.8.20), so no command makes a merged_into cycle
            // (K5): merge only into an open node.
            merged_into: if rng.chance(30) {
                let open: Vec<NodeId> = s
                    .nodes
                    .iter()
                    .filter(|(m, n)| **m != node && n.is_open())
                    .map(|(&m, _)| m)
                    .collect();
                rng.pick(&open)
            } else {
                None
            },
            // 1.11: the closing session, or the server.
            by: if rng.chance(70) {
                rng.pick(&p.sessions)
            } else {
                None
            },
        },
        24 => Ev::NodeReopened { node },
        25 => Ev::OfferSet {
            node,
            on: rng.chance(50),
        },
        26 | 27 => {
            // Often a day's wait on the node of an open commitment, so that a waiting status item
            // (D.8.5's row "n open, waiting(n), open_commitment") arises once the follow-up is
            // due, and `ix.next.waiting` is checked against `spec::waiting_nodes` on it.
            let kept: Vec<NodeId> = p
                .open_commitments
                .iter()
                .filter_map(|c| s.commitments.get(c).map(|x| x.node))
                .filter(|n| !n.is_reserved())
                .collect();
            match rng.pick(&kept).filter(|_| rng.chance(50)) {
                Some(n) => Ev::WaitSet {
                    node: n,
                    wait: Some(WaitOn::Time(now.plus(crate::types::DAY))),
                },
                None => Ev::WaitSet {
                    node,
                    wait: match rng.below(6) {
                        0 => None,
                        1 => Some(WaitOn::Time(later(rng, now))),
                        2 => Some(WaitOn::External(later(rng, now))),
                        3 => Some(WaitOn::Node(rng.pick(&p.nodes).unwrap())),
                        4 => Some(WaitOn::Question {
                            q: eid_of(rng, &p.pqs),
                            rechecks_at_set: 0,
                        }),
                        _ => Some(WaitOn::UserQuestion(eid_of(rng, &p.uqs))),
                    },
                },
            }
        }
        28 => Ev::DependsRedirected {
            node,
            from: s
                .nodes
                .get(&node)
                .and_then(|n| n.depends_on.iter().next().copied())
                .unwrap_or(NodeId(0)),
            to: rng.pick(&p.nodes).filter(|_| rng.chance(50)),
        },
        29 => {
            let mut qs = p.pqs.clone();
            qs.extend(p.uqs.iter().copied());
            Ev::MarkSet {
                node,
                mark: if rng.chance(50) {
                    Mark::Recheck
                } else {
                    Mark::Assume
                },
                question: eid_of(rng, &qs),
                on: rng.chance(70),
            }
        }
        30 => Ev::IntentOpened {
            node,
            intent: crate::state::Intent {
                eid: eid_of(rng, &p.entries),
                session: sess,
                pr_named: rng.chance(30),
            },
        },
        31 => Ev::IntentClosed {
            node,
            end: IntentEnd::ResolvedByClose,
        },
        32 => Ev::IntentMoved {
            from: node,
            to: rng.pick(&p.nodes).unwrap(),
        },
        33 => match rng.below(4) {
            0 => Ev::SeriesOutcome {
                series: rng.pick(&p.series).unwrap_or(node),
                missed: rng.chance(50),
            },
            1 => Ev::OccurrenceAvailable {
                node: rng.pick(&p.occurrences).unwrap_or(node),
            },
            2 => Ev::OccurrenceDetached {
                node: rng.pick(&p.occurrences).unwrap_or(node),
            },
            _ => Ev::SignalDetached {
                node: rng.pick(&p.signal_nodes).unwrap_or(node),
            },
        },
        34..=36 => Ev::HoldIssued {
            token: Token {
                node,
                fence: Fence(s.nodes.get(&node).map_or(1, |n| n.fence.0 + 1)),
            },
            session: sess,
            lease_until: now.plus(params.lease_first),
            // Sometimes a ceiling past every session's retirement (CS-8): an anomaly.
            terms: if rng.chance(15) {
                LeaseTerms {
                    ceiling: Dur(params.session_retain.0.saturating_add(HOUR.0)),
                    ..terms
                }
            } else {
                terms
            },
            l: params.lease_default,
            reopened: rng.chance(20).then_some(Outcome::Failed),
        },
        37 => {
            let t = rng.pick(&p.holds).unwrap_or(Token {
                node,
                fence: Fence(1),
            });
            Ev::HoldExtended {
                token: t,
                lease_until: s
                    .holds
                    .get(&t)
                    .map_or(now, |h| h.lease_until)
                    .max(now.plus(params.lease_default)),
                by: if rng.chance(50) {
                    Extension::Checkpoint
                } else {
                    Extension::Renew
                },
            }
        }
        38 => Ev::HoldEnded {
            token: rng.pick(&p.holds).unwrap_or(Token {
                node,
                fence: Fence(1),
            }),
            end: match rng.below(4) {
                0 => HoldEnd::Released,
                1 => HoldEnd::Expired,
                2 => HoldEnd::EndedByClose(EndCause::Move),
                _ => HoldEnd::EndedByClose(EndCause::Close),
            },
        },
        39..=46 => {
            let n = s.nodes.get(&node);
            Ev::Appended {
                eid: s.counters.next_eid,
                node,
                seq: Seq(n.map_or(1, |n| n.seq.0 + 1)),
                by: if rng.chance(90) {
                    Author::Session(sess)
                } else {
                    Author::Server
                },
                body: random_body(rng, &p, s, now),
                text: EntryText::default(),
            }
        }
        47 => {
            let r = eid_of(rng, &p.requests);
            let mut answers = p.saids.clone();
            answers.extend(p.entries.iter().copied());
            Ev::RequestAnswered {
                request: r,
                by: eid_of(rng, &answers),
            }
        }
        48 => {
            let r = eid_of(rng, &p.requests);
            let listing = s.ix.settling.get(&r).and_then(|x| x.iter().next().copied());
            // Sometimes a decision that does not list it: an anomaly.
            let d = if rng.chance(20) {
                p.decisions
                    .iter()
                    .copied()
                    .find(|d| s.ix.settling.get(&r).is_none_or(|x| !x.contains(d)))
                    .unwrap_or(Eid(1))
            } else {
                listing.unwrap_or(Eid(1))
            };
            Ev::RequestSettled {
                request: r,
                decision: d,
                answer: s
                    .requests
                    .get(&r)
                    .and_then(|x| x.answered_by)
                    .unwrap_or(Eid(1)),
            }
        }
        49 => match rng.below(4) {
            0 => Ev::RequestMoved {
                request: eid_of(rng, &p.requests),
                to: node,
            },
            1 => Ev::PqMoved {
                q: eid_of(rng, &p.pqs),
                to: node,
            },
            2 => Ev::CommitmentMoved {
                c: eid_of(rng, &p.commitments),
                to: node,
            },
            _ => Ev::SaidMoved {
                said: eid_of(rng, &p.saids),
                to: node,
            },
        },
        50 | 51 => {
            let q = if rng.chance(70) {
                eid_of(rng, &p.open_pqs)
            } else {
                eid_of(rng, &p.pqs)
            };
            match rng.below(8) {
                0 => Ev::PqRecheckAt {
                    q,
                    at: later(rng, now).plus(MINUTE),
                },
                // 1.12 (§9.3): a withdrawal, of an open question mostly, and otherwise of one that
                // is not open, an anomaly.
                7 => Ev::PqWithdrawn {
                    q,
                    by: eid_of(rng, &p.entries),
                },
                1 => Ev::PqRechecked {
                    q,
                    rechecks: s.pquestions.get(&q).map_or(1, |x| x.rechecks + 1),
                    next: later(rng, now).plus(MINUTE),
                },
                2 => Ev::PqExpired { q },
                3 => Ev::PqEscalated {
                    q,
                    // The question for the user created for `q` (D.8.2's Recheck arm).
                    uq: s
                        .uquestions
                        .iter()
                        .find(|(_, u)| u.source == UqSource::Escalation { question: q })
                        .map_or(Eid(9_999), |(&u, _)| u),
                },
                4 => Ev::PqAnswered {
                    q,
                    by: eid_of(rng, &p.entries),
                },
                5 => Ev::PqSuperseded {
                    q,
                    by: eid_of(rng, &p.pqs),
                },
                _ => Ev::PqAssumed {
                    q,
                    decision: eid_of(rng, &p.decisions),
                },
            }
        }
        52 | 53 => {
            let q = if rng.chance(70) {
                eid_of(rng, &p.pending_uqs)
            } else {
                eid_of(rng, &p.uqs)
            };
            match rng.below(6) {
                0 => Ev::UqAnswered {
                    q,
                    by: eid_of(rng, &p.user_entries),
                },
                1 => Ev::UqDefaulted { q },
                2 => Ev::UqReaddressed {
                    q,
                    stage: s.uquestions.get(&q).map_or(1, |x| x.stage + 1),
                    due: later(rng, now).plus(MINUTE),
                },
                3 => Ev::UqLapsed { q },
                4 => Ev::UqAssumed {
                    q,
                    decision: eid_of(rng, &p.decisions),
                },
                _ => Ev::AqAnswered {
                    q: eid_of(rng, &p.aqs),
                    by: eid_of(rng, &p.entries),
                },
            }
        }
        54 | 55 => {
            let c = if rng.chance(70) {
                eid_of(rng, &p.open_commitments)
            } else {
                eid_of(rng, &p.commitments)
            };
            match rng.below(4) {
                0 => {
                    let a = eid_of(rng, &p.acks);
                    let (n, conv) = s
                        .saids
                        .get(&a)
                        .map_or((node, keystr("chat:x/1")), |x| (x.node, x.conv.clone()));
                    Ev::CommitmentOpened {
                        c: a,
                        node: n,
                        conv,
                        followup_at: later(rng, now).plus(MINUTE),
                    }
                }
                1 => Ev::CommitmentRefreshed {
                    c,
                    followup_at: later(rng, now).plus(MINUTE),
                },
                _ => Ev::CommitmentFulfilled {
                    c,
                    by: eid_of(rng, &p.saids),
                },
            }
        }
        56 => {
            let a = eid_of(rng, &p.saids);
            if rng.chance(60) {
                Ev::SaidConfirmed {
                    said: a,
                    via: ConfirmVia::Explicit,
                    link: None,
                }
            } else if rng.chance(50) {
                Ev::SaidUnconfirmed { said: a }
            } else {
                // 1.9 (§9.2).
                Ev::SaidWithheld { said: a }
            }
        }
        57 => {
            let entry = eid_of(rng, &p.user_entries);
            // `route_to` records the request from the entry (D.8.11): a routing to a request
            // names one whose `from_user` is the entry.
            let own: Vec<Eid> = s
                .requests
                .iter()
                .filter(|(_, r)| r.from_user == Some(entry))
                .map(|(&r, _)| r)
                .collect();
            Ev::EntryRouted {
                entry,
                routing: match rng.below(4) {
                    0 => Routing::NoAction,
                    1 => Routing::Clarifies {
                        request: eid_of(rng, &p.requests),
                    },
                    2 => Routing::Answers {
                        question: eid_of(rng, &p.uqs),
                    },
                    _ => match rng.pick(&own) {
                        Some(request) => Routing::Request { node, request },
                        None => Routing::NoAction,
                    },
                },
                reason: None,
            }
        }
        58 => {
            let d = eid_of(rng, &p.decisions);
            if rng.chance(50) {
                Ev::DecisionSuperseded {
                    d,
                    by: eid_of(rng, &p.decisions),
                }
            } else {
                Ev::DecisionRetracted {
                    d,
                    by: eid_of(rng, &p.entries),
                    // 1.11: the decision's node, which keeps it; at times a node of a pre-1.11
                    // log (none), or a wrong one, which is an anomaly.
                    node: match rng.below(4) {
                        0 => None,
                        1 => rng.pick(&p.nodes),
                        _ => s.decisions.get(&d).map(|x| x.node),
                    },
                }
            }
        }
        59 => {
            let o = eid_of(rng, &p.objections);
            match rng.below(5) {
                0 => Ev::ObjectionResolved {
                    o,
                    by: eid_of(rng, &p.entries),
                    upheld: rng.chance(30),
                },
                1 => Ev::ObjectionMoved { o, to: node },
                // 1.8: a correction, of a closed node or (an anomaly) an open one.
                2 => Ev::NodeCorrected {
                    node,
                    outcome: if rng.chance(50) {
                        Outcome::Failed
                    } else {
                        Outcome::Other(crate::types::Label::new("refuted").unwrap())
                    },
                    by: eid_of(rng, &p.entries),
                    was: rng.chance(50).then_some(Outcome::Done),
                },
                // 1.10: a correction withdrawn, of a node marked upheld or (an anomaly) another.
                3 => Ev::CorrectionWithdrawn {
                    node,
                    outcome: Outcome::Done,
                },
                _ => Ev::DigestShown {
                    entry: eid_of(rng, &p.pending_digest),
                    by: eid_of(rng, &p.to_users),
                },
            }
        }
        60 => {
            let c = class(if rng.chance(50) { "a" } else { "b" });
            match rng.below(3) {
                0 => Ev::Refused {
                    node,
                    class: c,
                    session: sess,
                    backoff_until: now.plus(params.refusal_backoff),
                    expires_at: now.plus(params.refusal_ttl),
                    quorum: 1 + rng.below(2) as u32,
                },
                1 => {
                    let (n, c) = rng.pick(&p.refusals).unwrap_or((node, c));
                    Ev::RefusalExpired { node: n, class: c }
                }
                _ => Ev::RefusalsCleared {
                    node,
                    by: eid_of(rng, &p.entries),
                },
            }
        }
        61 => {
            let other = rng.pick(&p.nodes).unwrap();
            let key = SignalKey {
                kind: SignalKind::Conflict,
                a: node.min(other),
                b: node.max(other),
            };
            if rng.chance(60) {
                Ev::SignalOpened { key, node }
            } else {
                Ev::SignalResolved {
                    key: rng.pick(&p.signals).unwrap_or(key),
                }
            }
        }
        62 => match rng.below(3) {
            0 => Ev::Paused {
                scope: node,
                until: now.plus(params.pause_ttl),
                ttl: params.pause_ttl,
                by: sess,
                class: class("a"),
                renewed: false,
            },
            1 => Ev::Unpaused {
                scope: rng.pick(&p.pauses).unwrap_or(node),
            },
            _ => Ev::NoteBound {
                note: eid_of(rng, &p.notes),
                session: sess,
            },
        },
        _ => {
            if rng.chance(50) {
                Ev::OpRecorded {
                    class: class("a"),
                    op: OpId::new(&alloc::format!("op-{}", rng.below(4))).unwrap(),
                    digest: ReqDigest([0; 32]),
                    session: sess,
                    expires_at: now.plus(params.op_retain),
                    outcome: Err(crate::error::Error {
                        code: crate::error::ErrorCode::OpIdConflict,
                        hint: crate::error::Hint::OpIdConflict,
                    }),
                }
            } else {
                // Sometimes a queued key before its time: an anomaly.
                let k = if rng.chance(20) {
                    s.ix.deadlines.iter().find(|k| k.at > now).cloned()
                } else {
                    s.ix.deadlines.iter().find(|k| k.at <= now).cloned()
                };
                match k {
                    Some(k) => Ev::Fired {
                        kind: k.kind,
                        owner: k.owner,
                    },
                    None => Ev::Fired {
                        kind: DeadlineKind::Due,
                        owner: Owner::Node(node),
                    },
                }
            }
        }
    }
}

/// Fires every marker deadline due at or before `t`, each at its own time, and ends the leases,
/// pauses, and refusals that are over, so that the time-dependent predicates agree with the stored
/// state (a test's stand-in for `settle`, which is WP-D3's). Returns whether nothing is overdue.
fn catch_up(w: &mut World, t: Time) -> bool {
    for _ in 0..5_000 {
        let Some(k) = w.s.ix.deadlines.first().cloned() else {
            return true;
        };
        if k.at > t {
            return true;
        }
        let clock = w.clock;
        w.clock = k.at.max(w.s.last_now);
        if w.clock != k.at {
            w.clock = clock;
            return false;
        }
        let fired = Ev::Fired {
            kind: k.kind,
            owner: k.owner.clone(),
        };
        let before = w.s.clone();
        let dirty = w.emit_raw(fired);
        if let Err(b) = w.tracked(&before, &dirty) {
            panic!("firing {k:?}: {b:?}");
        }
        let follow = match (&k.kind, &k.owner) {
            (DeadlineKind::Lease, Owner::Hold(tk)) => Some(Ev::HoldEnded {
                token: *tk,
                end: HoldEnd::Expired,
            }),
            (DeadlineKind::Pause, Owner::Node(n)) => Some(Ev::PauseExpired { scope: *n }),
            (DeadlineKind::RefusalExpiry, Owner::Refusal(n, c)) => Some(Ev::RefusalExpired {
                node: *n,
                class: c.clone(),
            }),
            (DeadlineKind::OccurrenceSlot, Owner::Node(n)) => {
                Some(Ev::OccurrenceAvailable { node: *n })
            }
            (DeadlineKind::Recheck, Owner::Entry(q)) => Some(Ev::PqExpired { q: *q }),
            (DeadlineKind::UserQuestion, Owner::Entry(q)) => {
                let uq = w.s.uquestions.get(q).cloned();
                uq.map(|u| {
                    if u.has_default {
                        Ev::UqDefaulted { q: *q }
                    } else if usize::from(u.stage) + 1 < u.to.len() {
                        Ev::UqReaddressed {
                            q: *q,
                            stage: u.stage + 1,
                            due: k.at.plus(HOUR),
                        }
                    } else {
                        Ev::UqLapsed { q: *q }
                    }
                })
            }
            (DeadlineKind::Retire, Owner::Session(x)) => Some(Ev::SessionRetired { session: *x }),
            _ => None,
        };
        if let Some(ev) = follow {
            let shown = alloc::format!("{ev:?}");
            let before = w.s.clone();
            let dirty = w.emit_raw(ev);
            if let Err(b) = w.tracked(&before, &dirty) {
                panic!("after firing {k:?}: {shown}: {b:?}");
            }
        }
        // Once the last key of this instant has fired, the state is settled there (D.8.2).
        if w.s.ix.deadlines.first().is_none_or(|n| n.at > k.at)
            && let Err(b) = w.indices()
        {
            panic!("after the instant of {k:?}: {b:?}");
        }
        w.clock = clock.max(w.clock);
        if w.s.ix.deadlines.contains(&DeadlineKey {
            at: k.at,
            kind: k.kind,
            owner: k.owner.clone(),
        }) {
            // Stuck (a firing whose transition was refused): the state stays overdue.
            return false;
        }
    }
    false
}

/// The entities that left memory between `before` and `after`, as the shell's state rows would
/// give them back (D.4.12): each with the metadata of its entry.
fn departed(before: &State, after: &State) -> Vec<Restored> {
    let mut out = Vec::new();
    let meta = |e: &Eid| before.entries.get(e).copied();
    for (id, x) in &before.requests {
        if !after.requests.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::Request(*id, x.clone(), m));
        }
    }
    for (id, x) in &before.pquestions {
        if !after.pquestions.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::PQuestion(*id, x.clone(), m));
        }
    }
    for (id, x) in &before.uquestions {
        if !after.uquestions.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::UQuestion(*id, x.clone(), m));
        }
    }
    for (id, x) in &before.commitments {
        if !after.commitments.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::Commitment(*id, x.clone(), m));
        }
    }
    for (id, x) in &before.saids {
        if !after.saids.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::Said(*id, x.clone(), m));
        }
    }
    for (id, x) in &before.decisions {
        if !after.decisions.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::Decision(*id, x.clone(), m));
        }
    }
    for (id, x) in &before.objections {
        if !after.objections.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::Objection(*id, *x, m));
        }
    }
    for (id, x) in &before.user_entries {
        if !after.user_entries.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::UserEntry(*id, *x, m));
        }
    }
    for (id, x) in &before.digest {
        if !after.digest.contains_key(id)
            && let Some(m) = meta(id)
        {
            out.push(Restored::Digest(*id, *x, m));
        }
    }
    for (id, m) in &before.entries {
        if !after.entries.contains_key(id) && !crate::index::bears_entity(before, *id) {
            out.push(Restored::Meta(*id, *m));
        }
    }
    out
}

/// The state's base and history parts, without the counters, the anomaly count, and the indices
/// (D.4.1): what an anomaly must leave as a pure anomaly at the same time leaves it (D.7.5).
fn base_differences(a: &State, b: &State) -> Vec<&'static str> {
    let mut d = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {
            $( if a.$f != b.$f { d.push(stringify!($f)); } )*
        };
    }
    cmp!(
        params,
        last_now,
        nodes,
        names,
        aliases,
        entries,
        holds,
        requests,
        pquestions,
        aquestions,
        uquestions,
        commitments,
        saids,
        decisions,
        objections,
        user_entries,
        digest,
        notes,
        approvals,
        retracted,
        refusals,
        classes,
        signals,
        pauses,
        sessions,
        principal_ids,
        ops,
        stats,
        since,
        user_msgs,
        leaving,
        digest_left
    );
    d
}

/// D.7.5 (`evolve_counts_anomalies`): an event whose precondition failed changes nothing in the
/// base and history parts beyond what a pure anomaly at the same time changes (the prunes of its
/// time); only the counters of the ids it carries may advance.
fn anomaly_changes_nothing(
    before: &State,
    after: &State,
    at: Time,
) -> Result<(), Vec<&'static str>> {
    let mut probe = before.clone();
    crate::evolve::evolve_mut(
        &mut probe,
        &Event {
            at,
            ev: Ev::SessionStatus {
                session: SessionId(u64::MAX),
                text: crate::types::Text::default(),
            },
        },
    );
    let mut d = base_differences(after, &probe);
    let (a, b) = (&after.counters, &probe.counters);
    if a.events != b.events
        || a.next_node < b.next_node
        || a.next_eid < b.next_eid
        || a.next_session < b.next_session
        || a.next_principal < b.next_principal
    {
        d.push("counters");
    }
    if after.anomalies != probe.anomalies {
        d.push("anomalies");
    }
    if d.is_empty() { Ok(()) } else { Err(d) }
}

fn run_seed(seed: u64, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let params = Params {
        refusal_quorum: 2,
        ..Params::default()
    };
    let mut w = World::new(params, Time(1_790_000_000_000));
    let mut grave: Vec<Restored> = Vec::new();
    let mut restored = 0_u32;
    for step in 0..steps {
        if rng.chance(10) {
            let jump = match rng.below(20) {
                0 => crate::types::DAY.0 * (1 + rng.below(30)),
                1 => HOUR.0 * (1 + rng.below(30)),
                _ => rng.below(4) * 15 * MINUTE.0,
            };
            let to = w.clock.plus(Dur(jump));
            let before = w.s.clone();
            let settled = catch_up(&mut w, to);
            grave.extend(departed(&before, &w.s));
            if settled {
                w.clock = to;
            }
        }
        let ev = if !grave.is_empty() && rng.chance(5) {
            // Restore some of what left memory (D.4.12), at the clock.
            let items: Vec<Restored> = grave.iter().filter(|_| rng.chance(50)).cloned().collect();
            Ev::Restored { items }
        } else {
            random_event(&mut rng, &w.s, w.clock)
        };
        let shown = alloc::format!("{ev:?}");
        let before = w.s.clone();
        let dirty = w.emit_raw(ev);
        if let Err(b) = w.consistent(&before, &dirty) {
            panic!("seed {seed} step {step}: {shown}: {b:?}");
        }
        if w.s.anomalies != before.anomalies {
            if let Err(d) = anomaly_changes_nothing(&before, &w.s, w.clock) {
                panic!("seed {seed} step {step}: the anomaly {shown} changed {d:?}");
            }
            ANOMALIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        grave.extend(departed(&before, &w.s));
        // T3's clause on memory: `leaving` holds exactly the resident entities that are finished
        // and not held back, and the pending digest entries (D.4.12).
        let have: BTreeSet<EntityRef> = w.s.leaving.keys().copied().collect();
        let want = crate::spec::leaving_expected(&w.s);
        assert!(
            have == want,
            "seed {seed} step {step}: leaving after {shown}: extra {:?}, missing {:?}",
            have.difference(&want).collect::<Vec<_>>(),
            want.difference(&have).collect::<Vec<_>>()
        );
        restored += u32::from(
            matches!(w.log.last().map(|e| &e.ev), Some(Ev::Restored { .. }))
                && w.s.anomalies == before.anomalies,
        );
        // While nothing is overdue, the flags and the candidates equal the naive definitions.
        let overdue =
            w.s.ix
                .deadlines
                .first()
                .is_some_and(|k| k.at <= w.s.last_now)
                || w.s.holds.values().any(|h| h.lease_until <= w.s.last_now)
                || w.s.pauses.values().any(|x| x.until <= w.s.last_now);
        if !overdue {
            let now = w.s.last_now;
            for &n in w.s.nodes.keys() {
                let f = crate::spec::flags(&w.s, n, now);
                assert_eq!(
                    w.s.ix.node[&n].flags, f,
                    "seed {seed} step {step}: flags of {n:?} after {shown}"
                );
                let want = crate::spec::classify(&w.s, n, now);
                let have = [Cand::Node(n), Cand::Status(n)]
                    .into_iter()
                    .find(|c| w.s.ix.next.of.contains_key(c));
                assert_eq!(
                    have, want,
                    "seed {seed} step {step}: candidate {n:?} after {shown}"
                );
            }
            // 1.4 (§10.4, R27): the nodes that wait, as `next` counts them.
            assert_eq!(
                w.s.ix.next.waiting,
                crate::spec::waiting_nodes(&w.s, now),
                "seed {seed} step {step}: the waiting nodes after {shown}"
            );
        }
    }
    assert_eq!(w.replay(), w.s, "seed {seed}: replay");
    RESTORED.fetch_add(restored, core::sync::atomic::Ordering::Relaxed);
}

/// Restores that applied, over every seed (the test requires some).
static RESTORED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Anomalies compared with a pure anomaly, over every seed (the test requires some).
static ANOMALIES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[test]
fn random_sequences_keep_every_index_equal_to_its_recount() {
    for seed in 1..=60 {
        run_seed(seed, 300);
    }
    assert!(RESTORED.load(core::sync::atomic::Ordering::Relaxed) > 0);
    assert!(ANOMALIES.load(core::sync::atomic::Ordering::Relaxed) > 1_000);
}

#[test]
fn random_events_are_deterministic() {
    let mut a = Rng(7);
    let mut b = Rng(7);
    let w = World::new(Params::default(), Time(0));
    for _ in 0..50 {
        assert_eq!(
            random_event(&mut a, &w.s, w.clock),
            random_event(&mut b, &w.s, w.clock)
        );
    }
}

#[test]
#[ignore = "a coverage report, run by hand"]
fn coverage_report() {
    let mut ok: BTreeMap<String, u32> = BTreeMap::new();
    let mut bad: BTreeMap<String, u32> = BTreeMap::new();
    for seed in 1..=60_u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut w = World::new(Params::default(), Time(1_790_000_000_000));
        for _ in 0..300 {
            if rng.chance(10) {
                let jump = match rng.below(20) {
                    0 => crate::types::DAY.0 * (1 + rng.below(30)),
                    1 => HOUR.0 * (1 + rng.below(30)),
                    _ => rng.below(4) * 15 * MINUTE.0,
                };
                let to = w.clock.plus(Dur(jump));
                if catch_up(&mut w, to) {
                    w.clock = to;
                } else {
                    *bad.entry(String::from("~stuck catch-up")).or_insert(0) += 1;
                }
            }
            let ev = random_event(&mut rng, &w.s, w.clock);
            let mut name = alloc::format!("{ev:?}");
            name.truncate(name.find([' ', '{', '(']).unwrap_or(name.len()));
            if let Ev::Appended { body, .. } = &ev {
                let mut b = alloc::format!("{body:?}");
                b.truncate(b.find([' ', '{', '(']).unwrap_or(b.len()));
                name = alloc::format!("Appended/{b}");
            }
            let a = w.s.anomalies;
            w.emit_raw(ev);
            let m = if w.s.anomalies == a {
                &mut ok
            } else {
                &mut bad
            };
            *m.entry(name).or_insert(0) += 1;
        }
        let left: u64 = w.s.nodes.values().map(|n| u64::from(n.left)).sum();
        *ok.entry(String::from("~left memory")).or_insert(0) += left as u32;
        *ok.entry(String::from("~days elapsed")).or_insert(0) +=
            ((w.clock.0 - 1_790_000_000_000) / crate::types::DAY.0) as u32;
        *ok.entry(String::from("~digest left")).or_insert(0) +=
            w.s.digest_left.values().sum::<u64>() as u32;
        *ok.entry(String::from("~retired")).or_insert(0) +=
            (w.s.counters.next_session.0 - 1) as u32 - w.s.sessions.len() as u32;
    }
    let names: BTreeSet<&String> = ok.keys().chain(bad.keys()).collect();
    for n in names {
        std::println!(
            "{n:28} ok {:5} anomalies {:5}",
            ok.get(n).unwrap_or(&0),
            bad.get(n).unwrap_or(&0)
        );
    }
}
