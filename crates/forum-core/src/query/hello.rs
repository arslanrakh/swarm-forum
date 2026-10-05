//! What `session hello` shows (design D.8.25, §20.3): the session's holds and caps (1.4, R25), the
//! notes it delivers (at most `INBOX_DELIVER_MAX`, oldest first, 1.5), the top 3 offers that `next`
//! would give it (nothing taken), with the `why` of an empty read (1.4, R25), who is present, the
//! orientation (1.4, D.14.5), whom it serves, and what is pending for those principals under any
//! alias (CS-3, CS-33, U1).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::next::{Caller, rank};
use super::{HelloData, NoMeter, Orientation, UserItems, WhoPage};
use crate::cmd::NextQuery;
use crate::state::{State, UqRank};
use crate::types::{DigestId, QuestionId, ROOT, SessionId, Time};
use crate::user::HELLO_USER_ITEMS;

/// The pending questions addressed now to the persons of `served`, in `UqRank` order (the `about`
/// node's priority, then the due, CS-33): the first `HELLO_USER_ITEMS`, read as a range of each
/// alias's set, and the exact count (U1).
fn questions(s: &State, served: &BTreeSet<crate::types::PrincipalId>) -> UserItems<QuestionId> {
    let mut first: BTreeSet<UqRank> = BTreeSet::new();
    let mut count = 0_usize;
    for a in served {
        if let Some(set) = s.ix.pending_uq_by.get(a) {
            count = count.saturating_add(set.len());
            first.extend(set.iter().take(HELLO_USER_ITEMS).copied());
        }
    }
    UserItems {
        count: u32::try_from(count).unwrap_or(u32::MAX),
        items: first.iter().take(HELLO_USER_ITEMS).map(|r| r.q).collect(),
        older: None,
    }
}

/// The digest entries pending for the persons of `served`, oldest first, the first
/// `HELLO_USER_ITEMS` of them, their count, and the entries that left the pending list unshown
/// (`digest_left`, never part of `count`, D.8.13).
fn digest(s: &State, served: &BTreeSet<crate::types::PrincipalId>) -> UserItems<DigestId> {
    let mut first: BTreeSet<DigestId> = BTreeSet::new();
    let mut count = 0_usize;
    let mut older = 0_u64;
    for a in served {
        if let Some(set) = s.ix.pending_digest_by.get(a) {
            count = count.saturating_add(set.len());
            first.extend(set.iter().take(HELLO_USER_ITEMS).copied());
        }
        older = older.saturating_add(s.digest_left.get(a).copied().unwrap_or(0));
    }
    UserItems {
        count: u32::try_from(count).unwrap_or(u32::MAX),
        items: first.into_iter().take(HELLO_USER_ITEMS).collect(),
        older: Some(older),
    }
}

/// What `session hello` shows (§20.3).
#[must_use]
pub fn hello(s: &State, sess: SessionId, now: Time) -> HelloData {
    let holds = crate::helpers::holds::live_holds_of(s, sess, now)
        .into_iter()
        .filter_map(|t| s.holds.get(&t).map(|h| (t, t.node, h.lease_until)))
        .collect();
    let (offers, why) = Caller::of(s, sess).map_or_else(
        || (Vec::new(), crate::reply::EmptyWhy::default()),
        |c| rank(s, &c, now, &NextQuery::default(), 3, &mut NoMeter),
    );
    let offers_why = offers.is_empty().then_some(why);
    let root = s.nodes.get(&ROOT);
    let principals: Vec<_> = s
        .sessions
        .get(&sess)
        .map(|x| x.principals.iter().copied().collect())
        .unwrap_or_default();
    let served = super::reads::served(s, sess);
    HelloData {
        holds,
        caps: s
            .sessions
            .get(&sess)
            .map(|x| x.caps.clone())
            .unwrap_or_default(),
        inbox: crate::helpers::agents::deliverable(s, sess, now),
        offers,
        offers_why,
        orientation: Orientation {
            map: root.map(|r| r.digest.clone()).unwrap_or_default(),
            house_rules: root.map(|r| r.agreements.clone()).unwrap_or_default(),
        },
        present: if s.sessions.contains_key(&sess) {
            super::who(s, None, now)
        } else {
            WhoPage::default()
        },
        principals,
        user_decisions: questions(s, &served),
        user_digest: digest(s, &served),
    }
}
