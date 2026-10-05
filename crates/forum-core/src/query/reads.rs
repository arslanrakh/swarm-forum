//! The read queries of D.8.25 that are not `next`, the brief, `leave_better`, the status line,
//! `hello`, or the board: the retry record, `precheck`, node resolution and paths, archiving, keys,
//! `list` with its subtree counts (CS-32), what rests on a node (CS-22), principals, `who`
//! (CS-21), the holders of a node, and `inbox_ready`.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::{
    BACKLINKS_MAX, Backlinks, FindResult, ListFilter, ListPage, Meter, Present, SCAN_MAX, WHO_MAX,
    WhoPage,
};
use crate::cmd::{Cmd, Ctx, RequesterFilter};
use crate::error::Error;
use crate::reply::{BoardScope, Total};
use crate::state::{BasisTarget, Life, OpRecord, State};
use crate::types::{
    ClassName, DecisionId, KeyStr, LinkRel, NodeId, NodeRef, OpId, Path, PrincipalId, PrincipalKey,
    ROOT, Seq, SessionId, Time, Token,
};

/// The live record only: `expires_at > now` (CS-8).
#[must_use]
pub fn op_lookup<'a>(
    s: &'a State,
    class: &ClassName,
    op: &OpId,
    now: Time,
) -> Option<&'a OpRecord> {
    s.ops
        .get(&(class.clone(), op.clone()))
        .filter(|r| r.expires_at > now)
}

/// The errors that §21 puts before `write.per_hour`, as `apply` would return them: the session
/// (step 3 of D.8.1) and the target (step 2 of D.8.24), through the same functions `apply` uses.
#[must_use]
pub fn precheck(s: &State, ctx: &Ctx, cmd: &Cmd) -> Option<Error> {
    if let Err(e) = crate::apply::session_check(s, ctx, cmd) {
        return Some(e);
    }
    crate::apply::target(s, cmd).err()
}

/// Resolves a node reference: a path step by step through live names, then aliases (§4), or an
/// id. Command targets do not follow `merged_into` (D.8.4).
#[must_use]
pub fn resolve(s: &State, r: &NodeRef) -> Option<NodeId> {
    match r {
        NodeRef::Id(n) => s.nodes.contains_key(n).then_some(*n),
        NodeRef::Path(path) => resolve_path(s, path),
    }
}

/// Resolves a path from ROOT, each step through live names, then aliases.
#[must_use]
pub(crate) fn resolve_path(s: &State, path: &Path) -> Option<NodeId> {
    let mut cur = ROOT;
    for step in &path.0 {
        let key = (cur, step.clone());
        cur = *s.names.get(&key).or_else(|| s.aliases.get(&key))?;
    }
    Some(cur)
}

/// The node `n` lives on in, following `merged_into` (A-88), bounded by `nodes.len()` steps.
#[must_use]
pub fn resolve_merged(s: &State, n: NodeId) -> NodeId {
    crate::flags::resolve_merged(s, n)
}

/// The node's live path: its names from ROOT, at most `nodes.len()` steps.
#[must_use]
pub fn path_of(s: &State, n: NodeId) -> Path {
    let mut names = Vec::new();
    let mut cur = n;
    while let Some(place) = s.nodes.get(&cur).and_then(|x| x.place.as_ref()) {
        if names.len() >= s.nodes.len() {
            break;
        }
        names.push(place.name.clone());
        cur = place.parent;
    }
    names.reverse();
    Path(names)
}

/// Whether `n` is `ancestor` or below it.
#[must_use]
pub fn is_under(s: &State, n: NodeId, ancestor: NodeId) -> bool {
    n == ancestor || crate::index::ancestors(s, n).contains(&ancestor)
}

/// AR: closed and `now >= closed_at + archive.after`. Derived, never stored.
#[must_use]
pub fn archived(s: &State, n: NodeId, now: Time) -> bool {
    match s.nodes.get(&n).map(|x| &x.life) {
        Some(Life::Closed(c)) => now >= c.at.plus(s.params.archive_after),
        Some(Life::Open) | None => false,
    }
}

/// `view find {key}`: the open carrier first, then the closed carriers, newest (largest id)
/// first (§7, A-117).
#[must_use]
pub fn find_key(s: &State, k: &KeyStr) -> FindResult {
    let open = s.ix.keys_open.get(k).copied();
    let closed =
        s.ix.keys_all
            .get(k)
            .into_iter()
            .flatten()
            .rev()
            .copied()
            .filter(|n| Some(*n) != open)
            .collect();
    FindResult { open, closed }
}

/// Whether node `n` passes the filter of `view list` at `now`.
fn list_match(s: &State, n: NodeId, f: &ListFilter, now: Time) -> bool {
    let Some(node) = s.nodes.get(&n) else {
        return false;
    };
    let state_ok = match f.closed {
        None => node.is_open() || !archived(s, n, now),
        Some(true) => !node.is_open(),
        Some(false) => node.is_open(),
    };
    let flags =
        s.ix.node
            .get(&n)
            .map_or(crate::types::FlagSet::EMPTY, |x| x.flags);
    state_ok
        && f.kind
            .as_ref()
            .is_none_or(|k| node.kind.as_ref() == Some(k))
        && f.tags.is_subset(&node.tags)
        && f.status.as_ref().is_none_or(|t| node.status == *t)
        && f.fields.iter().all(|(k, v)| node.fields.get(k) == Some(v))
        && f.flags.intersection(flags) == f.flags
}

/// `view list` (§20.4): the nodes of the subtree of `under` (`under` included), or of the whole
/// forum, that match the filter, by id, at most `limit`; the scan stops after `SCAN_MAX` visited
/// nodes, and `total` is then `≥k` (§18). `under` carries the exact subtree counts of the filter's
/// `under` node, or ROOT's (CS-32).
#[must_use]
pub fn list<M: Meter>(s: &State, f: &ListFilter, now: Time, m: &mut M) -> ListPage {
    let limit = usize::try_from(f.limit).unwrap_or(usize::MAX);
    let scan = usize::try_from(SCAN_MAX).unwrap_or(usize::MAX);
    let mut matches: Vec<NodeId> = Vec::new();
    let mut visited = 0_usize;
    let mut finished = true;
    match f.under {
        None => {
            for &n in s.nodes.keys() {
                if visited >= scan {
                    finished = false;
                    break;
                }
                visited = visited.saturating_add(1);
                m.tick(1);
                if list_match(s, n, f, now) {
                    matches.push(n);
                }
            }
        }
        Some(u) => {
            let mut stack = alloc::vec![u];
            while let Some(n) = stack.pop() {
                if visited >= scan {
                    finished = false;
                    break;
                }
                visited = visited.saturating_add(1);
                m.tick(1);
                if list_match(s, n, f, now) {
                    matches.push(n);
                }
                if let Some(cs) = s.ix.children.get(&n) {
                    stack.extend(cs.iter().rev().copied());
                }
            }
            matches.sort_unstable();
        }
    }
    let count = u32::try_from(matches.len()).unwrap_or(u32::MAX);
    let total = if finished {
        Total::Exact(count)
    } else {
        Total::AtLeast(count)
    };
    matches.truncate(limit);
    let under =
        s.ix.node
            .get(&f.under.unwrap_or(ROOT))
            .map(|x| x.sub)
            .unwrap_or_default();
    ListPage {
        nodes: matches,
        total,
        under,
    }
}

/// `view agreements {under?, limit?}` (1.4, §11.2, §20.4, D.14.5): the nodes of the subtree of
/// `under` (ROOT when None), `under` included, whose `agreements` are not empty, open or closed, by
/// id, at most `limit`, each with the approvals of its current content (CS-34) and when its
/// agreements were last set. They are read from `ix.agreements`, so where in the tree they sit does
/// not matter: under ROOT every one counts, exactly; under another node the cheaper of a walk of
/// its subtree and a filter of the index, each stopping after `SCAN_MAX` visits with a `≥k`
/// total (§18), so the answer is exact whenever the subtree or the index is within `SCAN_MAX`.
#[must_use]
pub fn agreements<M: Meter>(
    s: &State,
    under: Option<NodeId>,
    limit: u32,
    m: &mut M,
) -> super::AgreementsPage {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let scan = usize::try_from(SCAN_MAX).unwrap_or(usize::MAX);
    let u = under.unwrap_or(ROOT);
    let index = &s.ix.agreements;
    let (found, finished) = if u == ROOT {
        let page: Vec<NodeId> = index.iter().copied().take(limit).collect();
        m.tick(u64::try_from(page.len()).unwrap_or(u64::MAX));
        (page, true)
    } else if subtree_size(s, u) <= u64::try_from(index.len()).unwrap_or(u64::MAX) {
        // The walk visits the subtree, `u` included, and keeps the indexed nodes.
        let mut found: Vec<NodeId> = Vec::new();
        let mut visited = 0_usize;
        let mut finished = true;
        let mut stack = alloc::vec![u];
        while let Some(n) = stack.pop() {
            if visited >= scan {
                finished = false;
                break;
            }
            visited = visited.saturating_add(1);
            m.tick(1);
            if index.contains(&n) {
                found.push(n);
            }
            if let Some(cs) = s.ix.children.get(&n) {
                stack.extend(cs.iter().rev().copied());
            }
        }
        found.sort_unstable();
        (found, finished)
    } else {
        // The filter reads the index by id and keeps the nodes under `u`.
        let mut found: Vec<NodeId> = Vec::new();
        let mut finished = true;
        for (i, &n) in index.iter().enumerate() {
            if i >= scan {
                finished = false;
                break;
            }
            m.tick(1);
            if is_under(s, n, u) {
                found.push(n);
            }
        }
        (found, finished)
    };
    let count = if u == ROOT {
        u32::try_from(index.len()).unwrap_or(u32::MAX)
    } else {
        u32::try_from(found.len()).unwrap_or(u32::MAX)
    };
    let total = if finished {
        Total::Exact(count)
    } else {
        Total::AtLeast(count)
    };
    super::AgreementsPage {
        items: found
            .into_iter()
            .take(limit)
            .map(|n| crate::reply::AgreementItem {
                node: n,
                approvals: current_approvals(s, n),
                changed: s.nodes.get(&n).and_then(|x| x.agreements_at),
            })
            .collect(),
        total,
    }
}

/// The number of nodes in the subtree of `n`, `n` included, from its exact `sub` counts (CS-32).
fn subtree_size(s: &State, n: NodeId) -> u64 {
    s.ix.node.get(&n).map_or(1, |x| {
        x.sub
            .open
            .saturating_add(x.sub.done)
            .saturating_add(x.sub.failed)
            .saturating_add(x.sub.other)
            .saturating_add(1)
    })
}

/// A page of a set read newest first: items `page × BACKLINKS_MAX ..`, at most `BACKLINKS_MAX`.
fn page_of<T: Copy, M: Meter>(
    items: impl DoubleEndedIterator<Item = T>,
    page: u32,
    m: &mut M,
) -> Vec<T> {
    let skip = usize::try_from(page)
        .unwrap_or(usize::MAX)
        .saturating_mul(BACKLINKS_MAX);
    let out: Vec<T> = items.rev().skip(skip).take(BACKLINKS_MAX).collect();
    m.tick(u64::try_from(skip.saturating_add(out.len())).unwrap_or(u64::MAX));
    out
}

fn exact(n: usize) -> Total {
    Total::Exact(u32::try_from(n).unwrap_or(u32::MAX))
}

/// What rests on `n` (CS-22), page `page` of each list, `BACKLINKS_MAX` per page, newest first:
/// the incoming links (by relation, then newest source first), the dependents, and the active
/// decisions that cite `n` or one of its decisions. The decisions of `n` are scanned newest first
/// up to `SCAN_MAX`; the citers' total is exact when one set holds them all or the union is
/// counted within `SCAN_MAX`, and `≥k` otherwise (§18).
#[must_use]
pub fn backlinks<M: Meter>(s: &State, n: NodeId, page: u32, m: &mut M) -> Backlinks {
    let empty_links: BTreeSet<(LinkRel, NodeId)> = BTreeSet::new();
    let links_set = s.ix.backlinks.get(&n).unwrap_or(&empty_links);
    let links = page_of(links_set.iter().copied(), page, m);
    let empty_nodes: BTreeSet<NodeId> = BTreeSet::new();
    let deps = s.ix.dependents.get(&n).unwrap_or(&empty_nodes);
    let dependents = page_of(deps.iter().copied(), page, m);
    // The citers: of n itself and of each of n's decisions, merged newest first.
    let mut sources: Vec<&BTreeSet<DecisionId>> = Vec::new();
    if let Some(c) = s.ix.cited_by.get(&BasisTarget::Node(n)) {
        sources.push(c);
    }
    let mut scanned = 0_u32;
    let mut capped = false;
    for d in
        s.ix.items
            .get(&n)
            .into_iter()
            .flat_map(|it| it.decisions.iter().rev())
    {
        if scanned >= SCAN_MAX {
            capped = true;
            break;
        }
        scanned = scanned.saturating_add(1);
        m.tick(1);
        if let Some(c) = s.ix.cited_by.get(&BasisTarget::Entry(*d)) {
            sources.push(c);
        }
    }
    let (cited_by, cited_by_total) = merged_page(&sources, page, capped, m);
    Backlinks {
        links,
        links_total: exact(links_set.len()),
        dependents,
        dependents_total: exact(deps.len()),
        cited_by,
        cited_by_total,
    }
}

/// A page of the union of `sources`, newest first, and its total.
fn merged_page<M: Meter>(
    sources: &[&BTreeSet<DecisionId>],
    page: u32,
    capped: bool,
    m: &mut M,
) -> (Vec<DecisionId>, Total) {
    let skip = usize::try_from(page)
        .unwrap_or(usize::MAX)
        .saturating_mul(BACKLINKS_MAX);
    let want = skip.saturating_add(BACKLINKS_MAX);
    // A k-way merge, newest first, without duplicates: at most `want` steps.
    let mut iters: Vec<_> = sources.iter().map(|x| x.iter().rev().peekable()).collect();
    let mut out: Vec<DecisionId> = Vec::new();
    let mut seen = 0_usize;
    while seen < want {
        let mut best: Option<DecisionId> = None;
        for it in &mut iters {
            if let Some(&&d) = it.peek() {
                best = Some(best.map_or(d, |b: DecisionId| b.max(d)));
            }
        }
        let Some(d) = best else { break };
        for it in &mut iters {
            if it.peek().is_some_and(|x| **x == d) {
                it.next();
            }
        }
        m.tick(1);
        if seen >= skip {
            out.push(d);
        }
        seen = seen.saturating_add(1);
    }
    let total = if sources.len() <= 1 {
        let n = sources.first().map_or(0, |x| x.len());
        if capped {
            Total::AtLeast(u32::try_from(n).unwrap_or(u32::MAX))
        } else {
            exact(n)
        }
    } else {
        let scan = usize::try_from(SCAN_MAX).unwrap_or(usize::MAX);
        let mut union: BTreeSet<DecisionId> = BTreeSet::new();
        let mut over = false;
        'outer: for x in sources {
            for d in *x {
                if union.len() >= scan {
                    over = true;
                    break 'outer;
                }
                m.tick(1);
                union.insert(*d);
            }
        }
        let n = u32::try_from(union.len()).unwrap_or(u32::MAX);
        if over || capped {
            Total::AtLeast(n)
        } else {
            Total::Exact(n)
        }
    };
    (out, total)
}

/// The scope of `view requests {requester?}` for this session (D.8.8, CS-25): `Mine` is its
/// principals and every alias of them; `All` every row; a principal's key that principal and its
/// aliases, or nothing for a key never interned. Without a filter, `All` while `user` is the only
/// interned principal, otherwise `Mine` for a session that serves principals and `All` for one
/// that serves none.
#[must_use]
pub fn board_scope(s: &State, sess: SessionId, f: Option<&RequesterFilter>) -> BoardScope {
    let mine = || {
        let mut out = BTreeSet::new();
        for &p in s
            .sessions
            .get(&sess)
            .into_iter()
            .flat_map(|x| &x.principals)
        {
            out.extend(crate::helpers::principals::aliases(s, p));
        }
        BoardScope::Principals(out)
    };
    match f {
        Some(RequesterFilter::All) => BoardScope::All,
        Some(RequesterFilter::Mine) => mine(),
        Some(RequesterFilter::Principal(k)) => BoardScope::Principals(
            s.principal_ids
                .get(k)
                .map(|&p| crate::helpers::principals::aliases(s, p))
                .unwrap_or_default(),
        ),
        None => {
            let serves = s
                .sessions
                .get(&sess)
                .is_some_and(|x| !x.principals.is_empty());
            if s.principal_ids.len() <= 1 || !serves {
                BoardScope::All
            } else {
                mine()
            }
        }
    }
}

/// A principal's key, for rendering (CS-1); None for an id the state never interned.
#[must_use]
pub fn principal_key(s: &State, p: PrincipalId) -> Option<&PrincipalKey> {
    s.ix.principal_key.get(&p)
}

/// A present session: its class, its held nodes, and its last presence.
fn present(s: &State, x: SessionId) -> Option<Present> {
    let session = s.sessions.get(&x)?;
    Some(Present {
        session: x,
        class: session.class.clone(),
        holds: s
            .ix
            .holds_by_session
            .get(&x)
            .into_iter()
            .flatten()
            .map(|t| t.node)
            .collect(),
        last_seen: session.last_seen,
    })
}

/// At most `WHO_MAX` sessions, with counts (CS-21). Without `at`: the newest live sessions, the
/// live sessions per class, and their exact total. With `at`: the sessions of the first `WHO_MAX`
/// holds met descending from `at` through the attended children (preorder, children by id), and a
/// total that is exact if the descent finished and `≥k` otherwise.
#[must_use]
pub fn who(s: &State, at: Option<NodeId>, now: Time) -> WhoPage {
    match at {
        None => {
            let items =
                s.ix.live_sessions
                    .iter()
                    .rev()
                    .take(WHO_MAX)
                    .filter_map(|&x| present(s, x))
                    .collect();
            let by_class =
                s.ix.live_by_class
                    .iter()
                    .map(|(c, n)| (c.clone(), *n))
                    .collect();
            WhoPage {
                items,
                by_class,
                total: exact(s.ix.live_sessions.len()),
            }
        }
        Some(a) => {
            let mut sessions: Vec<SessionId> = Vec::new();
            let mut finished = true;
            let mut stack = alloc::vec![a];
            let mut steps = 0_usize;
            'walk: while let Some(n) = stack.pop() {
                steps = steps.saturating_add(1);
                if steps > s.nodes.len() {
                    break;
                }
                if s.ix.node.get(&n).is_none_or(|x| x.subtree_live == 0) {
                    continue;
                }
                for (_, h) in holds_on(s, n) {
                    if now >= h.lease_until {
                        continue;
                    }
                    if !sessions.contains(&h.session) {
                        if sessions.len() >= WHO_MAX {
                            finished = false;
                            break 'walk;
                        }
                        sessions.push(h.session);
                    }
                }
                if let Some(cs) = s.ix.attended_children.get(&n) {
                    stack.extend(cs.iter().rev().copied());
                }
            }
            let count = u32::try_from(sessions.len()).unwrap_or(u32::MAX);
            WhoPage {
                items: sessions.iter().filter_map(|&x| present(s, x)).collect(),
                by_class: Vec::new(),
                total: if finished {
                    Total::Exact(count)
                } else {
                    Total::AtLeast(count)
                },
            }
        }
    }
}

/// The holds on node `n`, by token.
fn holds_on(s: &State, n: NodeId) -> impl Iterator<Item = (&Token, &crate::state::Hold)> {
    let lo = Token {
        node: n,
        fence: crate::types::Fence(0),
    };
    let hi = Token {
        node: n,
        fence: crate::types::Fence(u64::MAX),
    };
    s.holds.range(lo..=hi)
}

/// 1.8 (§10.2, §10.4, §20.4): the open intent of node `n` when its session is gone, silent or
/// retired (§3), which briefs and rows mark: whether its action happened is for someone to check,
/// and `hold check {target, intent_done}` records it without a hold.
#[must_use]
pub fn gone_intent(s: &State, n: NodeId) -> Option<crate::state::Intent> {
    let i = s.nodes.get(&n)?.open_intent?;
    s.sessions
        .get(&i.session)
        .is_none_or(|x| x.silent)
        .then_some(i)
}

/// 1.10 (§10.4, §11.3): for a claim (kind `claim`) closed `done` that has no active decision, its
/// newest retracted decision: the claim reads as done with nothing that decided it, which the
/// brief's header and a resolve that keeps it (`no_basis`) say. With none active, the newest
/// decision is retracted, since a decision is superseded only by a newer one. In the lab's fifth
/// round (l5), three lemmas whose deciding decisions the objector had retracted read as done after
/// reviews that kept them.
///
/// 1.11 (§11.3, §24): the node keeps its newest retracted decision (`retracted_decision`), so the
/// answer does not depend on what is still in memory: an active decision never leaves it (§24),
/// and a retracted one leaves 7 days after its retraction. In the lab's sixth round (l5), 1.10's
/// answer, read from the resident decisions alone, was None for retractions 9 to 31 days old. A
/// retraction recorded before 1.11 is still found among the resident decisions, newest first, the
/// scan stopping at `SCAN_MAX` (§18).
#[must_use]
pub fn no_basis(s: &State, n: NodeId) -> Option<crate::types::DecisionId> {
    let node = s.nodes.get(&n)?;
    let closed_done =
        matches!(&node.life, Life::Closed(c) if c.outcome == crate::types::Outcome::Done);
    let claim = node
        .kind
        .as_ref()
        .is_some_and(|k| k.as_str() == crate::types::CLAIM);
    if !closed_done || !claim {
        return None;
    }
    let it = s.ix.items.get(&n);
    if it.is_some_and(|x| !x.active_decisions.is_empty()) {
        return None;
    }
    let max = usize::try_from(super::SCAN_MAX).unwrap_or(usize::MAX);
    let resident = it.and_then(|x| {
        x.decisions.iter().rev().take(max).copied().find(|d| {
            s.decisions
                .get(d)
                .is_some_and(|y| matches!(y.state, crate::state::DecisionState::Retracted { .. }))
        })
    });
    node.retracted_decision.max(resident)
}

/// The holders of a node, each with its lease end; no tokens.
#[must_use]
pub fn holders(s: &State, n: NodeId, now: Time) -> Vec<(SessionId, Time)> {
    holds_on(s, n)
        .filter(|(_, h)| now < h.lease_until)
        .map(|(_, h)| (h.session, h.lease_until))
        .collect()
}

/// `inbox(sess)` is not empty, or `for_ = n` and `n.seq > since.unwrap_or(shown[n])` (D.8.16).
#[must_use]
pub fn inbox_ready(
    s: &State,
    sess: SessionId,
    for_: Option<NodeId>,
    since: Option<Seq>,
    now: Time,
) -> bool {
    if !crate::helpers::agents::inbox(s, sess, now).is_empty() {
        return true;
    }
    let Some(n) = for_ else { return false };
    let seen = since.unwrap_or_else(|| {
        s.sessions
            .get(&sess)
            .and_then(|x| x.shown.get(&n).copied())
            .unwrap_or(Seq(0))
    });
    s.nodes.get(&n).is_some_and(|x| x.seq > seen)
}

/// The agreements in force on `n`: its own and its ancestors' non-empty ones, nearest first (§11.2).
#[must_use]
pub(crate) fn agreement_sources(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut chain = alloc::vec![n];
    chain.extend(crate::index::ancestors(s, n));
    for m in chain {
        if s.nodes.get(&m).is_some_and(|x| !x.agreements.is_empty()) {
            out.push(m);
        }
    }
    out
}

/// `current_approvals(n) := Σ approvals[(n, v)] for content_version <= v <= version` (CS-34).
#[must_use]
pub(crate) fn current_approvals(s: &State, n: NodeId) -> u32 {
    let Some(node) = s.nodes.get(&n) else {
        return 0;
    };
    if node.content_version > node.version {
        return 0;
    }
    s.approvals
        .range((n, node.content_version)..=(n, node.version))
        .fold(0_u32, |a, (_, c)| a.saturating_add(*c))
}

/// Every principal the session serves, with every alias of each (D.8.11).
#[must_use]
pub(crate) fn served(s: &State, sess: SessionId) -> BTreeSet<PrincipalId> {
    let mut out = BTreeSet::new();
    for &p in s
        .sessions
        .get(&sess)
        .into_iter()
        .flat_map(|x| &x.principals)
    {
        out.extend(crate::helpers::principals::aliases(s, p));
    }
    out
}
