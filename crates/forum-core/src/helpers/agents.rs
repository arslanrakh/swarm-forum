//! Agent communication: recipients, the inbox, and who holds a conversation (design D.8.16,
//! D.8.26). Bodies: WP-D5b.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::state::{Author, State};
use crate::types::{Addressee, Eid, KeyStr, NodeId, NoteId, SessionId, Time, Token};

/// `recipients(to, sender)` (D.8.16, A-49): the sessions a note is delivered to, fixed when it is
/// recorded, and whether it waits for whoever takes the node next: `Session(x)` → `{x}`, the
/// sender's own session included; `Holder(n)` → the sessions other than the sender with a live
/// hold on `n`, or none and pending (1.4, §13, R12: the sender is never a `holder:` recipient);
/// `Holders(a)` and `Principal(p)` → none, their readers being found when they read (D-6, CS-3).
/// 1.12 (§13): `Holder(n)` of a closed node, which is never taken, → the session that closed it
/// while it is live and is not the sender (`closer`), else none, and never pending: what a note
/// without `to` reaches there since 1.11. In the lab's seventh round (it6 l6), two notes to
/// `holder:` of an incident closed seconds before waited for a next holder, while its live closer
/// received neither.
///
/// 1.4 adds `sender` to the signature WP-D1 froze (`recipients(to, sender)` of D.8.16): the note's
/// author, None for a note the server records.
#[must_use]
pub fn recipients(
    s: &State,
    to: &Addressee,
    sender: Option<SessionId>,
    now: Time,
) -> (BTreeSet<SessionId>, bool) {
    match to {
        Addressee::Session(x) => (BTreeSet::from([*x]), false),
        Addressee::Holder(n) if s.nodes.get(n).is_some_and(|x| !x.is_open()) => {
            (closer(s, *n, sender).into_iter().collect(), false)
        }
        Addressee::Holder(n) => {
            let hs: BTreeSet<SessionId> = crate::query::holders(s, *n, now)
                .into_iter()
                .map(|(x, _)| x)
                .filter(|x| Some(*x) != sender)
                .collect();
            let pending = hs.is_empty();
            (hs, pending)
        }
        Addressee::Holders(_) | Addressee::Principal(_) => (BTreeSet::new(), false),
    }
}

/// 1.11 (§13): the session that closed `n`, when `n` is closed and that session is live (neither
/// silent nor retired, §3) and is not `sender`: a `say` at `n` without `to` reaches it, as
/// `session:<it>`, and (1.12) so does a note to `holder:<n>`. None otherwise, and the note reaches
/// nobody. In the lab's sixth round (l5), 10 of a run's 12 notes were at closed nodes and reported
/// "the next holder", who never came; one, meant for the reviewer who had closed its node 4.5 s
/// before, reached nobody.
#[must_use]
pub fn closer(s: &State, n: NodeId, sender: Option<SessionId>) -> Option<SessionId> {
    let by = match &s.nodes.get(&n)?.life {
        crate::state::Life::Closed(c) => c.by?,
        crate::state::Life::Open => return None,
    };
    (Some(by) != sender && s.sessions.get(&by).is_some_and(|x| !x.silent)).then_some(by)
}

/// `inbox(s, sess, now)` (D.8.16): the live notes delivered to `sess` and not yet read, ascending:
/// bound notes, `principal:` notes of the principals it serves under any alias, and `holders:`
/// notes of the branches it holds in, newer than its cursor for their scope and not written by it
/// (1.4, R12). A query too (`query::inbox`).
#[must_use]
pub fn inbox(s: &State, sess: SessionId, now: Time) -> Vec<NoteId> {
    let Some(session) = s.sessions.get(&sess) else {
        return Vec::new();
    };
    let live = |x: &NoteId| s.notes.get(x).is_some_and(|n| n.until > now);
    let mut out: BTreeSet<NoteId> = BTreeSet::new();
    // Bound notes: `session:`, `holder:` (bound at recording or by `bind_holder_notes`), agent
    // questions to them, and agent answers.
    out.extend(
        s.ix.inbox
            .get(&sess)
            .into_iter()
            .flatten()
            .copied()
            .filter(live),
    );
    // `principal:` notes, under every alias of every principal the session serves (CS-3, A-18).
    let mut principals = BTreeSet::new();
    for &p in &session.principals {
        principals.extend(crate::helpers::principals::aliases(s, p));
    }
    for a in principals {
        let from = (now.plus(crate::types::Dur(1)), Eid(0));
        for &(_, x) in
            s.ix.user_notes
                .get(&a)
                .into_iter()
                .flat_map(|u| u.range(from..))
        {
            if s.notes.get(&x).is_some_and(|n| !n.read_by.contains(&sess)) {
                out.insert(x);
            }
        }
    }
    // `holders:` notes of each node the session holds and of its ancestors, read at read time
    // (D-6), newer than the session's cursor for that scope, and never the session's own (1.4).
    let mut scopes: BTreeSet<NodeId> = BTreeSet::new();
    for t in s.ix.holds_by_session.get(&sess).into_iter().flatten() {
        if s.holds.get(t).is_none_or(|h| now >= h.lease_until) {
            continue;
        }
        scopes.insert(t.node);
        scopes.extend(crate::index::ancestors(s, t.node));
    }
    for m in scopes {
        let Some(notes) = s.ix.branch_notes.get(&m) else {
            continue;
        };
        let cursor = session.branch_seen.get(&m).copied().unwrap_or(Eid(0));
        let lo = Eid(cursor.0.saturating_add(1));
        for &x in notes.range(lo..) {
            if live(&x) && s.entries.get(&x).map(|e| e.by) != Some(Author::Session(sess)) {
                out.insert(x);
            }
        }
    }
    out.into_iter().collect()
}

/// `first(inbox)` of D.8.16 (1.5, §13): the `INBOX_DELIVER_MAX` oldest notes of an inbox listed
/// ascending, which `hello` and `talk wait` deliver and `InboxRead` marks read; the rest wait for
/// the next `hello` or `talk wait`, counted by the status line's `inbox`.
#[must_use]
pub fn first(mut inbox: Vec<NoteId>) -> Vec<NoteId> {
    inbox.sort_unstable();
    inbox.truncate(crate::query::INBOX_DELIVER_MAX);
    inbox
}

/// The notes a `hello` or a `talk wait` of `sess` delivers at `now`: `first(inbox(s, sess, now))`
/// (1.5, D.8.16).
#[must_use]
pub fn deliverable(s: &State, sess: SessionId, now: Time) -> Vec<NoteId> {
    first(inbox(s, sess, now))
}

/// The holds on node `n`, by token: every hold in the map is live on a settled state (T3).
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

/// 1.11 (§9.1): the cap that posting in conversation `k` needs: the name of its scheme, `chat` for
/// `chat:...`, `mail` for `mail:...`, `issue` for `issue:...`.
#[must_use]
pub fn voice_cap(k: &KeyStr) -> &str {
    k.scheme()
}

/// Whether session `x` has the cap `cap` (§3: the caps it works with).
fn has_cap(s: &State, x: SessionId, cap: &str) -> bool {
    s.sessions
        .get(&x)
        .is_some_and(|y| y.caps.iter().any(|l| l.as_str() == cap))
}

/// Who speaks in conversation `k` on its conversation node `c`, as session `x` sees it (§9.2,
/// D.8.26).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voice {
    /// `x` holds `c` or a node under it.
    Own,
    /// Nobody holds `c` or a node under it.
    Nobody,
    /// Another session holds `c` or a node under it, and speaks by convention: the first holder
    /// whose caps include `k`'s cap, else the first holder.
    Holder(SessionId),
    /// 1.11: other sessions hold `c` or a node under it, none of them with `k`'s cap, and `x` has
    /// it: `x` is the conversation's voice.
    Caller,
}

/// `voice(c, x, k)` (D.8.26): `Own` when `x` holds `c` or a descendant of it; else, over the held
/// nodes of `c`'s subtree in preorder, children by id (found descending `attended_children`), each
/// node's holds by token: the first holder whose caps include `k`'s cap (§9.1, 1.11), else, when
/// `x` has that cap, `Caller`, else the first holder; `Nobody` with no holder. O(depth × log n) for
/// `x`'s own holds, which are at most `hold.per_session`, each checked by its ancestors; the
/// descent follows only attended children, and stops at the first holder with the cap, so it
/// visits the attended nodes of the subtree at most. Reads a settled state, on which every hold in
/// the map is live (T3).
#[must_use]
pub fn voice(s: &State, c: NodeId, x: SessionId, k: &KeyStr) -> Voice {
    let own =
        s.ix.holds_by_session
            .get(&x)
            .into_iter()
            .flatten()
            .any(|t| crate::query::is_under(s, t.node, c));
    if own {
        return Voice::Own;
    }
    let cap = voice_cap(k);
    let mut first = None;
    let mut stack = alloc::vec![c];
    let mut steps = 0_usize;
    while let Some(n) = stack.pop() {
        steps = steps.saturating_add(1);
        if steps > s.nodes.len().saturating_add(1) {
            break;
        }
        for (_, h) in holds_on(s, n) {
            if has_cap(s, h.session, cap) {
                return Voice::Holder(h.session);
            }
            first = first.or(Some(h.session));
        }
        if let Some(cs) = s.ix.attended_children.get(&n) {
            stack.extend(cs.iter().rev().copied());
        }
    }
    match first {
        None => Voice::Nobody,
        Some(_) if has_cap(s, x, cap) => Voice::Caller,
        Some(h) => Voice::Holder(h),
    }
}

/// `conversation_holder(c, x, k)` (D.8.26): the holder `voice` names, None when `x` holds `c` or a
/// descendant, when nobody does, and (1.11) when `x` is the conversation's voice by its cap: the
/// condition of the `conversation_held` warning (D.8.15), which `said` and `ask` compute and rules
/// 2, 6, and 9 of the next calls recompute.
#[must_use]
pub fn conversation_holder(s: &State, c: NodeId, x: SessionId, k: &KeyStr) -> Option<SessionId> {
    match voice(s, c, x, k) {
        Voice::Holder(h) => Some(h),
        Voice::Own | Voice::Nobody | Voice::Caller => None,
    }
}
