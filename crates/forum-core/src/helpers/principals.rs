//! Principals and the directory's aliases (design D.8.11, CS-1, D-3).

use alloc::collections::BTreeSet;

use super::Exec;
use crate::error::{Error, Why};
use crate::event::Ev;
use crate::state::State;
use crate::types::{GONE, PrincipalId, PrincipalKey, SessionId, USER_PRINCIPAL};

/// A principal as validation finds it: interned already, or a key that execution interns first
/// with `PrincipalSeen` (CS-1). Validation reads the state only, so it cannot intern.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PrincipalRef {
    /// An interned principal.
    Id(PrincipalId),
    /// A key not interned yet.
    Key(PrincipalKey),
}

/// `intern(key)` (D.8.11): `principal_ids[key]`, or `PrincipalSeen { id: next_principal, key }`
/// first when the key is new. Only commands intern, never a firing.
pub fn intern(ex: &mut Exec<'_>, key: &PrincipalKey) -> PrincipalId {
    if let Some(&id) = ex.s().principal_ids.get(key) {
        return id;
    }
    let id = ex.s().counters.next_principal;
    ex.emit(Ev::PrincipalSeen {
        id,
        key: key.clone(),
    });
    id
}

/// The principal a `PrincipalRef` names, interning a new key (D.8.11).
pub fn intern_ref(ex: &mut Exec<'_>, r: &PrincipalRef) -> PrincipalId {
    match r {
        PrincipalRef::Id(p) => *p,
        PrincipalRef::Key(k) => intern(ex, k),
    }
}

/// A key as a `PrincipalRef`: its id if interned, else the key.
#[must_use]
pub fn principal_ref(s: &State, key: &PrincipalKey) -> PrincipalRef {
    s.principal_ids
        .get(key)
        .map_or_else(|| PrincipalRef::Key(key.clone()), |&p| PrincipalRef::Id(p))
}

/// `resolve_principal(sess, arg)` (D.8.11): the principal of a `from_user` or a `to_user`, the
/// first that applies: the call's `principal` argument; the session's only principal, or the
/// lowest id when it serves only aliases of one person; `USER_PRINCIPAL` when it serves none
/// (also a session the call opens, `sess = None`); `invalid(AmbiguousPrincipal)` when it serves
/// principals of several persons. Since 1.9 a `from_user` in a session that serves none names its
/// principal or is `invalid(NoPrincipal)` before this (§8.1), so the case of `USER_PRINCIPAL` by
/// default is a `to_user`'s only.
///
/// # Errors
///
/// `invalid(AmbiguousPrincipal)`.
pub fn resolve_principal(
    s: &State,
    sess: Option<SessionId>,
    arg: Option<&PrincipalKey>,
) -> Result<PrincipalRef, Error> {
    if let Some(k) = arg {
        return Ok(principal_ref(s, k));
    }
    let served = sess.and_then(|x| s.sessions.get(&x)).map(|x| &x.principals);
    let Some(first) = served.and_then(|ps| ps.first().copied()) else {
        return Ok(PrincipalRef::Id(USER_PRINCIPAL));
    };
    let one_person = served
        .into_iter()
        .flatten()
        .all(|&p| same_person(s, p, first));
    if one_person {
        Ok(PrincipalRef::Id(first))
    } else {
        Err(Error::invalid(Why::AmbiguousPrincipal))
    }
}

/// `same_person(p, q) := p = q ∨ (person_of[p] = m ∧ person_of[q] = m for some m)` (D.8.11).
#[must_use]
pub fn same_person(s: &State, p: PrincipalId, q: PrincipalId) -> bool {
    p == q
        || matches!(
            (s.ix.person_of.get(&p), s.ix.person_of.get(&q)),
            (Some(a), Some(b)) if a == b
        )
}

/// `aliases(p) := aliases_of[person_of[p]]` if `p` has a person, else `{p}` (D.8.11).
#[must_use]
pub fn aliases(s: &State, p: PrincipalId) -> BTreeSet<PrincipalId> {
    let mut out =
        s.ix.person_of
            .get(&p)
            .and_then(|m| s.ix.aliases_of.get(m))
            .cloned()
            .unwrap_or_default();
    out.insert(p);
    out
}

/// `gone(p) := p has a person m and `gone` ∈ m.tags` (D.8.11).
#[must_use]
pub fn gone(s: &State, p: PrincipalId) -> bool {
    s.ix.person_of
        .get(&p)
        .and_then(|m| s.nodes.get(m))
        .is_some_and(|x| x.tags.iter().any(|t| t.as_str() == GONE))
}

/// Whether nobody would read what is filed for `user` (1.8, §8.1, §21's `no_human_in_reach`):
/// `user` is no alias of another principal (§4), and no live session serves it. At most
/// `query::SCAN_MAX` live sessions are read (§18): with more, someone may serve it, and this is
/// false, so the warning comes only when every live session was read.
#[must_use]
pub fn user_unread(s: &State) -> bool {
    if aliases(s, USER_PRINCIPAL).len() > 1 {
        return false;
    }
    let max = usize::try_from(crate::query::SCAN_MAX).unwrap_or(usize::MAX);
    if s.ix.live_sessions.len() > max {
        return false;
    }
    !s.ix.live_sessions.iter().any(|x| {
        s.sessions
            .get(x)
            .is_some_and(|y| y.principals.contains(&USER_PRINCIPAL))
    })
}

/// 1.12 (§8.4, §21's `no_human_in_reach`): whether some session serves `p`'s person at `now`: a
/// session that has not retired serves one of its aliases, or one of its aliases logged a
/// `from_user`, which a session that serves them records, within `user_gap.window`. Only a
/// session that serves a person sees the questions for them (§8.4), so a question none of whose
/// stages is served reaches nobody. At most `query::SCAN_MAX` sessions are read (§18): with more,
/// a person counts as served, so the warning comes only when every session was read.
#[must_use]
pub fn served(s: &State, p: PrincipalId, now: crate::types::Time) -> bool {
    let all = aliases(s, p);
    let from = now.minus(s.params.user_gap_window);
    if all.iter().any(|a| {
        s.user_msgs
            .get(a)
            .and_then(|ts| ts.last())
            .is_some_and(|t| *t >= from)
    }) {
        return true;
    }
    let max = usize::try_from(crate::query::SCAN_MAX).unwrap_or(usize::MAX);
    if s.sessions.len() > max {
        return true;
    }
    s.sessions
        .values()
        .any(|x| x.principals.iter().any(|q| all.contains(q)))
}

/// Whether session `x` serves no principal: a worker (§20.9), or `None`.
#[must_use]
pub fn serves_none(s: &State, x: Option<SessionId>) -> bool {
    x.and_then(|x| s.sessions.get(&x))
        .is_none_or(|y| y.principals.is_empty())
}
