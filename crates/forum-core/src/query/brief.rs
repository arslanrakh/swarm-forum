//! The brief's data (design D.8.6, §10.4): nine sections, each read from an open subset or a full
//! set of the node's items newest first and cut at 100, each `total` a set's length, so a brief
//! costs O(100 per section), not O(the node's history) (CS-20).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use super::{
    BriefData, BriefItem, Meter, Predecessor, RestsOn, RestsVia, Rollup, Section, SectionName,
};
use crate::reply::Total;
use crate::state::{BasisRef, Lapse, Life, NodeItems, PqState, State};
use crate::types::{Eid, LinkRel, NodeId, Outcome, SessionId, Time};

/// Items per section and predecessors per brief.
const SECTION_MAX: usize = 100;

fn len32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A section from its items, cut at `SECTION_MAX`, with its total.
fn section(name: SectionName, mut items: Vec<BriefItem>, total: usize) -> Section {
    items.truncate(SECTION_MAX);
    Section {
        name,
        items,
        total: len32(total),
    }
}

/// The predecessors along the chain of `continues` links (§7, D.8.9): the nodes this node links to,
/// then the ones they link to, nearest first, each with its unsettled requests, at most 100, with a
/// total that is `≥k` when the walk stops early.
fn continues<M: Meter>(s: &State, n: NodeId, m: &mut M) -> (Vec<Predecessor>, Total) {
    let mut out: Vec<Predecessor> = Vec::new();
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    seen.insert(n);
    let mut frontier: Vec<NodeId> = alloc::vec![n];
    let mut finished = true;
    'walk: while !frontier.is_empty() {
        let mut next: Vec<NodeId> = Vec::new();
        for x in frontier {
            let Some(node) = s.nodes.get(&x) else {
                continue;
            };
            for &(rel, p) in &node.links {
                if rel != LinkRel::Continues || !seen.insert(p) {
                    continue;
                }
                m.tick(1);
                if out.len() >= SECTION_MAX {
                    finished = false;
                    break 'walk;
                }
                out.push(Predecessor {
                    node: p,
                    unsettled: s
                        .ix
                        .items
                        .get(&p)
                        .map(|it| it.unsettled.iter().take(SECTION_MAX).copied().collect())
                        .unwrap_or_default(),
                });
                next.push(p);
            }
        }
        frontier = next;
    }
    let count = len32(out.len());
    (
        out,
        if finished {
            Total::Exact(count)
        } else {
            Total::AtLeast(count)
        },
    )
}

/// The brief of a node (D.8.6).
#[must_use]
pub fn brief<M: Meter>(
    s: &State,
    n: NodeId,
    _sess: Option<SessionId>,
    now: Time,
    m: &mut M,
) -> BriefData {
    let empty = NodeItems::default();
    let it = s.ix.items.get(&n).unwrap_or(&empty);
    let node = s.nodes.get(&n);
    let nix = s.ix.node.get(&n).copied().unwrap_or_default();
    let mut sections = Vec::new();
    // 1. Flags, in `Flag` order.
    let flags: Vec<BriefItem> = nix.flags.iter().map(BriefItem::Flag).collect();
    let total = flags.len();
    sections.push(section(SectionName::Flags, flags, total));
    // 2. The open intent; the last hold that expired after the node's last checkpoint without a
    // checkpoint of its own (1.4, §10.4, re-run R30: an outside action may have happened); and the
    // `said` entries that are pending or unconfirmed.
    let mut items: Vec<BriefItem> = Vec::new();
    let intent = node.and_then(|x| x.open_intent);
    if let Some(i) = intent {
        items.push(BriefItem::Intent(i));
    }
    let expired = node.and_then(|x| x.expired_hold);
    if let Some(h) = expired {
        items.push(BriefItem::ExpiredHold(h));
    }
    items.extend(
        it.open_saids
            .iter()
            .rev()
            .take(SECTION_MAX)
            .map(|x| BriefItem::Said(*x)),
    );
    m.tick(u64::try_from(items.len()).unwrap_or(u64::MAX));
    let total = it
        .open_saids
        .len()
        .saturating_add(usize::from(intent.is_some()))
        .saturating_add(usize::from(expired.is_some()));
    sections.push(section(SectionName::Unfinished, items, total));
    // 3. The other `said` entries and the open commitments, newest first by eid.
    let mut both: Vec<(Eid, BriefItem)> = Vec::new();
    for x in it
        .saids
        .iter()
        .rev()
        .filter(|x| !it.open_saids.contains(x))
        .take(SECTION_MAX)
    {
        both.push((*x, BriefItem::Said(*x)));
    }
    for c in it.open_commitments.iter().rev().take(SECTION_MAX) {
        both.push((*c, BriefItem::Commitment(*c)));
    }
    both.sort_by(|a, b| b.0.cmp(&a.0));
    m.tick(u64::try_from(both.len()).unwrap_or(u64::MAX));
    let total = it
        .saids
        .len()
        .saturating_sub(it.open_saids.len())
        .saturating_add(it.open_commitments.len());
    sections.push(section(
        SectionName::SaidAndCommitments,
        both.into_iter().map(|(_, x)| x).collect(),
        total,
    ));
    // 4. Active decisions; the user questions about the node; the agreements of the node and its
    // ancestors with their sources; answers that arrived after their question expired.
    let mut items: Vec<BriefItem> = it
        .active_decisions
        .iter()
        .rev()
        .take(SECTION_MAX)
        .map(|d| BriefItem::Decision(*d))
        .collect();
    let mut total = it.active_decisions.len();
    for q in it.uquestions.iter().rev().take(SECTION_MAX) {
        if let Some(uq) = s.uquestions.get(q) {
            items.push(BriefItem::UserQuestion {
                question: *q,
                state: uq.state,
            });
        }
    }
    total = total.saturating_add(it.uquestions.len());
    let sources = super::reads::agreement_sources(s, n);
    total = total.saturating_add(sources.len());
    items.extend(
        sources
            .into_iter()
            .map(|source| BriefItem::Agreement { source }),
    );
    let mut late = 0_usize;
    for q in it.pquestions.iter().rev().take(SECTION_MAX) {
        if let Some(pq) = s.pquestions.get(q)
            && let PqState::Answered { by } = pq.state
            && pq.lapse == Some(Lapse::Expired)
        {
            late = late.saturating_add(1);
            items.push(BriefItem::LateAnswer {
                question: *q,
                answer: by,
                assumption: pq.assumed_by,
            });
        }
    }
    total = total.saturating_add(late);
    m.tick(u64::try_from(items.len()).unwrap_or(u64::MAX));
    sections.push(section(SectionName::DecisionsAndAgreements, items, total));
    // 5. Unsettled requests, each followed by its clarifications.
    let mut items: Vec<BriefItem> = Vec::new();
    for r in it.unsettled.iter().rev().take(SECTION_MAX) {
        items.push(BriefItem::Request(*r));
        for e in s.ix.clarifications.get(r).into_iter().flatten() {
            items.push(BriefItem::Clarification(*e));
        }
    }
    m.tick(u64::try_from(items.len()).unwrap_or(u64::MAX));
    sections.push(section(
        SectionName::UnsettledRequests,
        items,
        it.unsettled.len(),
    ));
    // 6. Open objections.
    let items: Vec<BriefItem> = it
        .open_objections
        .iter()
        .rev()
        .take(SECTION_MAX)
        .map(|o| BriefItem::Objection(*o))
        .collect();
    sections.push(section(
        SectionName::Objections,
        items,
        it.open_objections.len(),
    ));
    // 7. The digest.
    let digest = node.is_some_and(|x| !x.digest.is_empty());
    sections.push(section(
        SectionName::Digest,
        if digest {
            alloc::vec![BriefItem::Digest]
        } else {
            Vec::new()
        },
        usize::from(digest),
    ));
    // 8. The last checkpoint.
    let last = node.and_then(|x| x.last_checkpoint);
    sections.push(section(
        SectionName::LastCheckpoint,
        last.map(BriefItem::Checkpoint).into_iter().collect(),
        usize::from(last.is_some()),
    ));
    // 9. Everything else: the shell pages the feed; the core gives the total.
    let seq = node.map_or(0, |x| x.seq.0);
    sections.push(Section {
        name: SectionName::Everything,
        items: Vec::new(),
        total: u32::try_from(seq).unwrap_or(u32::MAX),
    });
    let (continues, continues_total) = continues(s, n, m);
    let rests_on = rests_on(s, n, it, m);
    BriefData {
        node: n,
        version: node.map(|x| x.version).unwrap_or_default(),
        digest_version: node.map(|x| x.digest_version).unwrap_or_default(),
        holders: super::holders(s, n, now),
        rollup: Rollup {
            open_children: nix.open_children,
            held_children: nix.held_children,
            flagged_children: nix.flagged_children,
            sub: nix.sub,
        },
        continues,
        continues_total,
        sections,
        rests_on,
    }
}

/// Whether an outcome says that the node's result does not stand (1.8, §10.4): `failed`, or
/// `refuted`.
#[must_use]
pub fn fails(o: &Outcome) -> bool {
    match o {
        Outcome::Failed => true,
        Outcome::Other(l) => l.as_str() == crate::types::REFUTED,
        Outcome::Done | Outcome::Dropped | Outcome::Superseded | Outcome::Missed => false,
    }
}

/// The brief's `rests_on` (1.8, §10.4): among the first 100 `depends_on` targets of `n` and the
/// node references of its 100 newest active decisions, each followed through `merged_into`, those
/// closed `failed` or `refuted`, by id, the first way found for each, at most 100.
fn rests_on<M: Meter>(s: &State, n: NodeId, it: &NodeItems, m: &mut M) -> Vec<RestsOn> {
    let mut out: BTreeMap<NodeId, RestsOn> = BTreeMap::new();
    let mut put = |target: NodeId, via: RestsVia| {
        let f = crate::flags::resolve_merged(s, target);
        if f == n || out.contains_key(&f) {
            return;
        }
        if let Some(Life::Closed(c)) = s.nodes.get(&f).map(|x| &x.life)
            && fails(&c.outcome)
        {
            out.insert(
                f,
                RestsOn {
                    node: f,
                    outcome: c.outcome.clone(),
                    via,
                },
            );
        }
    };
    if let Some(node) = s.nodes.get(&n) {
        for &d in node.depends_on.iter().take(SECTION_MAX) {
            m.tick(1);
            put(d, RestsVia::DependsOn);
        }
    }
    for &d in it.active_decisions.iter().rev().take(SECTION_MAX) {
        let Some(dec) = s.decisions.get(&d) else {
            continue;
        };
        for b in &dec.basis {
            m.tick(1);
            if let BasisRef::Node { id, .. } = b {
                put(*id, RestsVia::Basis(d));
            }
        }
    }
    out.into_values().take(SECTION_MAX).collect()
}
