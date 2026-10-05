//! Unit tests of the 1.4 catch-up of WP-D1 to WP-D4 (WP-D1.4, design D.13.1's 1.4 table, D.14.4,
//! D.14.5, D.14.7): a take's `reopened`, a checkpoint's and a release's intents, `open_intent`, the
//! status line's `now` and `checkpoint_by`, `hello`'s caps, empty offers' `why` and orientation,
//! the nodes that wait, `view agreements`, the hold that expired without a checkpoint, the
//! `series_stopped` warning of a close and of a merge, the requests a create or a join recorded,
//! `told`, the departed entities a tracked job hands the writer, and the inbox's delivery bound.
//! Every call runs through `World::call_ctx`, which checks `check_step`, the dirty set and the
//! departures, `decide == apply`, the fold of the events, and every state invariant with R1.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use super::keys::tests::{inv, key, key_add, ks, nf, node, one, request, user_entry};
use super::restructure::tests::{blank_init, merge, series};
use crate::apply::tests::{
    at, checkpoint, class, close, create, create_with, created, entry, label, next, ok, refused,
    release, take, text,
};
use crate::cmd::*;
use crate::error::{What, Why};
use crate::event::{EntryBody, Ev, IntentEnd};
use crate::facts::{Departed, Fact, Tracked};
use crate::query::{self, BriefItem, NoMeter, SectionName};
use crate::reply::{AgreementItem, NextReply, Reply, Total, WarningCode, WarningDetail};
use crate::state::{Author, ExpiredHold, Routing};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Helpers

/// A checkpoint of `t` that opens an intent.
fn with_intent(t: Token) -> Cmd {
    Cmd::Hold(HoldCmd::Checkpoint {
        token: t,
        done: text("ready"),
        next_step: text("post it"),
        intent: Some(text("post the summary")),
        intent_done: None,
        files: Vec::new(),
    })
}

/// A checkpoint of `t` that closes the node's open intent.
fn closing_intent(t: Token) -> Cmd {
    Cmd::Hold(HoldCmd::Checkpoint {
        token: t,
        done: text("posted"),
        next_step: text("close"),
        intent: None,
        intent_done: Some(text("posted it")),
        files: Vec::new(),
    })
}

/// A release of `t`, `blocked` on `waits_for` when given, with `intent_done` when given.
fn release_with(t: Token, waits_for: Option<WaitArg>, intent_done: Option<&str>) -> Cmd {
    Cmd::Hold(HoldCmd::Release {
        token: t,
        done: text("did"),
        next_step: text("next"),
        reason: ReleaseReason::Handoff,
        waits_for,
        intent_done: intent_done.map(text),
        learnings: None,
        unsent: BTreeSet::new(),
        settles: BTreeSet::new(),
    })
}

/// The token of a reply that issued a hold.
fn token_of(d: &crate::reply::Done) -> Token {
    match &d.reply {
        Reply::Created { token: Some(t), .. } | Reply::Taken { token: t, .. } => *t,
        Reply::Next(NextReply::Hold { token, .. }) => *token,
        other => panic!("no hold: {other:?}"),
    }
}

/// Creates `p` with a hold, and returns the node and the token.
fn held(w: &mut World, c: &ClassName, s: SessionId, p: &str) -> (NodeId, Token) {
    let d = ok(w.call(c, s, create(p)));
    let (n, t) = created(&d);
    (n, t.expect("a hold"))
}

fn view_agreements(under: Option<&str>, limit: Option<u32>) -> Cmd {
    Cmd::View(ViewCmd::Agreements {
        under: under.map(at),
        limit,
    })
}

// ------------------------------------------------------------------------------------------------
// R4: a take or `next` that reopens a closed node says what it was, and the hold keeps it

#[test]
fn a_take_that_reopens_a_node_names_the_outcome_it_had() {
    let (mut w, c, s) = one();
    let x = node(&mut w, &c, s, "/x", &[]);
    ok(w.call(&c, s, close(x, Outcome::Failed)));
    // An unanswered request flags the closed node, so a take reopens it (§10.1); 1.9: an
    // objection would make it a review, which no take reopens.
    request(&mut w, s, x, "chat:t/1/1", "chat:t/1");
    let d = ok(w.call(&c, s, take(x)));
    let Reply::Taken {
        token,
        reopened,
        existing,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(reopened, Some(Outcome::Failed));
    assert!(!existing);
    assert_eq!(w.s.holds[&token].reopened, Some(Outcome::Failed));
    assert!(w.last_events.iter().any(|e| matches!(
        &e.ev,
        Ev::HoldIssued {
            reopened: Some(Outcome::Failed),
            ..
        }
    )));
    // The existing hold is returned with nothing reopened.
    let d = ok(w.call(&c, s, take(x)));
    assert!(matches!(
        d.reply,
        Reply::Taken {
            reopened: None,
            existing: true,
            ..
        }
    ));
    // A take of an open node reopens nothing.
    let y = node(&mut w, &c, s, "/y", &[]);
    let d = ok(w.call(&c, s, take(y)));
    let t = token_of(&d);
    assert!(matches!(d.reply, Reply::Taken { reopened: None, .. }));
    assert_eq!(w.s.holds[&t].reopened, None);
}

#[test]
fn a_next_that_reopens_a_node_names_the_outcome_it_had() {
    let (mut w, c, s) = one();
    let x = node(&mut w, &c, s, "/x", &[]);
    ok(w.call(&c, s, close(x, Outcome::Dropped)));
    request(&mut w, s, x, "chat:t/1/1", "chat:t/1");
    let d = ok(w.call(&c, s, next()));
    let Reply::Next(NextReply::Hold {
        node: n,
        token,
        reopened,
        ..
    }) = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!((n, reopened), (x, Some(Outcome::Dropped)));
    assert_eq!(w.s.holds[&token].reopened, Some(Outcome::Dropped));
}

// ------------------------------------------------------------------------------------------------
// R9, R15: what a checkpoint and a release did with the intents

#[test]
fn a_checkpoint_names_its_entry_and_the_intents_it_opened_and_closed() {
    let (mut w, c, s) = one();
    let (n, t) = held(&mut w, &c, s, "/a");
    let d = ok(w.call(&c, s, with_intent(t)));
    let Reply::Checkpointed {
        entry,
        intent,
        intent_done,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(intent, Some(entry));
    assert_eq!(intent_done, None);
    assert_eq!(w.s.nodes[&n].last_checkpoint, Some(entry));
    let opened = entry;
    let d = ok(w.call(&c, s, closing_intent(t)));
    let Reply::Checkpointed {
        entry,
        intent,
        intent_done,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!((intent, intent_done), (None, Some(opened)));
    assert!(entry > opened);
    assert!(w.s.nodes[&n].open_intent.is_none());
    // A plain checkpoint names its entry and no intent.
    let d = ok(w.call(&c, s, checkpoint(t)));
    assert!(matches!(
        d.reply,
        Reply::Checkpointed {
            intent: None,
            intent_done: None,
            ..
        }
    ));
}

#[test]
fn a_checkpoint_closes_another_sessions_intent_and_names_it() {
    let (mut w, c, s1) = one();
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s1, create_with("/a", |a| a.capacity = Some(2))));
    let (n, t1) = created(&d);
    let t1 = t1.unwrap();
    let opened = match ok(w.call(&c, s1, with_intent(t1))).reply {
        Reply::Checkpointed { entry, .. } => entry,
        other => panic!("{other:?}"),
    };
    let t2 = token_of(&ok(w.call(&c, s2, take(n))));
    let d = ok(w.call(&c, s2, closing_intent(t2)));
    assert!(matches!(
        d.reply,
        Reply::Checkpointed { intent_done: Some(e), .. } if e == opened
    ));
}

#[test]
fn a_release_closes_the_open_intent_with_intent_done() {
    let (mut w, c, s) = one();
    let (n, t) = held(&mut w, &c, s, "/a");
    let opened = match ok(w.call(&c, s, with_intent(t))).reply {
        Reply::Checkpointed { entry, .. } => entry,
        other => panic!("{other:?}"),
    };
    let d = ok(w.call(&c, s, release_with(t, None, Some("posted"))));
    assert!(matches!(
        d.reply,
        Reply::Released { intent_done: Some(e), .. } if e == opened
    ));
    assert!(w.s.nodes[&n].open_intent.is_none());
    assert!(d.warnings.iter().all(|x| x.code != WarningCode::OpenIntent));
    // D.8.10's order: the release's checkpoint entry, then the intent closed by it.
    let evs: Vec<&Ev> = w.last_events.iter().map(|e| &e.ev).collect();
    let at_entry = evs
        .iter()
        .position(|e| {
            matches!(
                e,
                Ev::Appended {
                    body: EntryBody::Checkpoint {
                        release: Some(_),
                        ..
                    },
                    ..
                }
            )
        })
        .unwrap();
    let Ev::Appended { eid, .. } = evs[at_entry] else {
        panic!("{:?}", evs[at_entry])
    };
    assert!(matches!(
        evs[at_entry + 1],
        Ev::IntentClosed { node, end: IntentEnd::Done { by } } if *node == n && *by == *eid
    ));
}

#[test]
fn a_release_refuses_intent_done_with_none_open_in_d824s_order() {
    let (mut w, c, s) = one();
    let (n, t) = held(&mut w, &c, s, "/a");
    // A wait target that does not resolve comes first (`not_found`), then the intent (`invalid`).
    refused(
        &mut w,
        &c,
        s,
        release_with(t, Some(WaitArg::Node(at("/nowhere"))), Some("x")),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        release_with(t, None, Some("x")),
        inv(Why::NoIntent),
    );
    // `invalid(NoIntent)`, a state check, comes before `cycle` (D.8.24).
    refused(
        &mut w,
        &c,
        s,
        release_with(t, Some(WaitArg::Node(NodeRef::Id(n))), Some("x")),
        inv(Why::NoIntent),
    );
}

#[test]
fn a_release_that_leaves_its_own_intent_open_warns_open_intent() {
    let (mut w, c, s) = one();
    let (n, t) = held(&mut w, &c, s, "/a");
    let opened = match ok(w.call(&c, s, with_intent(t))).reply {
        Reply::Checkpointed { entry, .. } => entry,
        other => panic!("{other:?}"),
    };
    let d = ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    assert!(matches!(
        d.reply,
        Reply::Released {
            intent_done: None,
            ..
        }
    ));
    let warn: Vec<_> = d
        .warnings
        .iter()
        .filter(|x| x.code == WarningCode::OpenIntent)
        .collect();
    assert_eq!(warn.len(), 1);
    assert_eq!(warn[0].id, Some(n));
    assert_eq!(warn[0].detail, WarningDetail::OpenIntent { intent: opened });
    assert!(
        w.s.nodes[&n].open_intent.is_some(),
        "the release is applied"
    );
}

#[test]
fn a_release_with_another_sessions_intent_open_does_not_warn() {
    let (mut w, c, s1) = one();
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s1, create_with("/a", |a| a.capacity = Some(2))));
    let (n, t1) = created(&d);
    ok(w.call(&c, s1, with_intent(t1.unwrap())));
    let t2 = token_of(&ok(w.call(&c, s2, take(n))));
    let d = ok(w.call(&c, s2, release(t2, ReleaseReason::Handoff)));
    assert!(d.warnings.iter().all(|x| x.code != WarningCode::OpenIntent));
}

// ------------------------------------------------------------------------------------------------
// R8, R25, R27, D.14.5: the status line, hello, and the nodes that wait

#[test]
fn the_status_line_shows_now_and_each_holds_checkpoint_time() {
    let (mut w, c, s) = one();
    let (n, t) = held(&mut w, &c, s, "/a");
    w.advance(Dur(MINUTE.0));
    let line = query::status_line(&w.s, s, w.clock);
    assert_eq!(line.now, w.clock);
    let h = &w.s.holds[&t];
    assert_eq!(
        line.holds,
        vec![query::HoldStatus {
            node: n,
            lease_until: h.lease_until,
            checkpoint_by: query::checkpoint_by(h),
        }]
    );
    // §20.9 "due": five minutes after the issue, before the lease's last minute.
    assert_eq!(
        line.holds[0].checkpoint_by,
        h.taken_at.plus(Dur(5 * MINUTE.0))
    );
}

#[test]
fn hello_shows_the_caps_the_orientation_and_why_offers_are_empty() {
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let d = ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some([label("rust")].into()),
            horizon: None,
            principal: None,
            from_user: None,
        }),
    ));
    let s = d.opened.unwrap();
    // The root's digest is the forum map, its agreements the house rules (D.14.5).
    let root = &w.s.nodes[&ROOT];
    let (v, dv) = (root.version, root.digest_version);
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(ROOT),
            version: v,
            digest_version: Some(dv),
            token: None,
            patch: Patch {
                digest: Some(text("work/ holds the tasks")),
                agreements: Some(text("review every merge")),
                ..Patch::default()
            },
        }),
    ));
    let h = query::hello(&w.s, s, w.clock);
    assert_eq!(h.caps, [label("rust")].into());
    assert_eq!(h.orientation.map, text("work/ holds the tasks"));
    assert_eq!(h.orientation.house_rules, text("review every merge"));
    assert!(h.offers.is_empty());
    assert_eq!(h.offers_why, Some(crate::reply::EmptyWhy::default()));
    // An offered node that waits is counted, though nothing is offered (R27).
    let (_, t) = held(&mut w, &c, s, "/w");
    ok(w.call(
        &c,
        s,
        release_with(t, Some(WaitArg::Time(When::In(Dur(DAY.0)))), None),
    ));
    let h = query::hello(&w.s, s, w.clock);
    assert!(h.offers.is_empty());
    assert_eq!(h.offers_why.map(|x| x.waiting), Some(1));
}

#[test]
fn an_empty_next_counts_the_nodes_that_wait_until_their_waits_resolve() {
    let (mut w, c, s1) = one();
    let s2 = w.hello(&c);
    let (n, t) = held(&mut w, &c, s1, "/w");
    ok(w.call(
        &c,
        s1,
        release_with(t, Some(WaitArg::Time(When::In(Dur(HOUR.0)))), None),
    ));
    assert_eq!(w.s.ix.next.waiting, [n].into());
    let d = ok(w.call(&c, s2, next()));
    let Reply::Next(NextReply::Empty { why, .. }) = d.reply else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(why.waiting, 1);
    assert_eq!(why.candidates, 0);
    // The wait resolves: the node leaves the set and is offered again.
    w.advance(Dur(HOUR.0));
    assert!(w.s.ix.next.waiting.is_empty());
    let d = ok(w.call(&c, s2, next()));
    assert!(matches!(d.reply, Reply::Next(NextReply::Hold { node, .. }) if node == n));
}

#[test]
fn a_node_that_waits_without_being_offered_is_not_counted() {
    let (mut w, c, s) = one();
    // Released with a wait, a node is tagged `offer`; untag it, and nothing would offer it.
    let (n, t) = held(&mut w, &c, s, "/w");
    ok(w.call(
        &c,
        s,
        release_with(t, Some(WaitArg::Time(When::In(Dur(HOUR.0)))), None),
    ));
    let x = &w.s.nodes[&n];
    let (v, dv) = (x.version, x.digest_version);
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(n),
            version: v,
            digest_version: Some(dv),
            token: None,
            patch: Patch {
                tags_remove: [label("offer")].into(),
                ..Patch::default()
            },
        }),
    ));
    assert!(w.s.ix.next.waiting.is_empty());
}

// ------------------------------------------------------------------------------------------------
// D.14.5: view agreements

#[test]
fn view_agreements_lists_a_subtrees_agreements_by_id() {
    let (mut w, c, s) = one();
    let p = created(&ok(w.call(
        &c,
        s,
        create_with("/playbooks", |a| {
            a.take = Some(false);
            a.agreements = Some(text("review every merge"));
        }),
    )))
    .0;
    let t0 = w.clock;
    w.advance(Dur(MINUTE.0));
    node(&mut w, &c, s, "/playbooks/plain", &[]);
    let q = created(&ok(w.call(
        &c,
        s,
        create_with("/playbooks/deploys", |a| {
            a.take = Some(false);
            a.agreements = Some(text("deploy on weekdays"));
        }),
    )))
    .0;
    let d = ok(w.call(&c, s, view_agreements(None, None)));
    assert_eq!(
        d.reply,
        Reply::Agreements {
            items: vec![
                AgreementItem {
                    node: p,
                    approvals: 0,
                    changed: Some(t0),
                },
                AgreementItem {
                    node: q,
                    approvals: 0,
                    changed: Some(w.clock),
                },
            ],
            total: Total::Exact(2),
        }
    );
    // A page of one, with the total; and a subtree.
    let d = ok(w.call(&c, s, view_agreements(None, Some(1))));
    assert!(matches!(
        &d.reply,
        Reply::Agreements { items, total: Total::Exact(2) } if items.len() == 1 && items[0].node == p
    ));
    let d = ok(w.call(&c, s, view_agreements(Some("/playbooks/deploys"), None)));
    assert!(matches!(
        &d.reply,
        Reply::Agreements { items, total: Total::Exact(1) } if items[0].node == q
    ));
    // An update of the agreements stamps their change; approvals of the current content count.
    w.advance(Dur(MINUTE.0));
    let x = &w.s.nodes[&q];
    let (v, dv) = (x.version, x.digest_version);
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(q),
            version: v,
            digest_version: Some(dv),
            token: None,
            patch: Patch {
                agreements: Some(text("deploy on weekdays, before noon")),
                ..Patch::default()
            },
        }),
    ));
    assert_eq!(w.s.nodes[&q].agreements_at, Some(w.clock));
    let v = w.s.nodes[&q].version;
    entry(
        &mut w,
        q,
        Author::Session(s),
        EntryBody::Approve { version: v },
    );
    let page = query::agreements(&w.s, Some(q), 50, &mut NoMeter);
    assert_eq!(page.items[0].approvals, 1);
    assert_eq!(page.items[0].changed, Some(w.clock));
    // The subtree must resolve; a view emits nothing but presence.
    refused(
        &mut w,
        &c,
        s,
        view_agreements(Some("/nowhere"), None),
        nf(What::Node),
    );
}

// ------------------------------------------------------------------------------------------------
// D.14.5, re-run R30: the last hold that expired without a checkpoint of its own

#[test]
fn a_hold_that_expired_without_a_checkpoint_shows_in_the_brief_until_the_next_one() {
    let (mut w, c, s) = one();
    let (n, t) = held(&mut w, &c, s, "/a");
    let lease_until = w.s.holds[&t].lease_until;
    w.advance(w.s.params.lease_first);
    let gone = ExpiredHold {
        token: t,
        session: s,
        at: lease_until,
    };
    assert_eq!(w.s.nodes[&n].expired_hold, Some(gone));
    let b = query::brief(&w.s, n, Some(s), w.clock, &mut NoMeter);
    let unfinished = b
        .sections
        .iter()
        .find(|x| x.name == SectionName::Unfinished)
        .unwrap();
    assert!(unfinished.items.contains(&BriefItem::ExpiredHold(gone)));
    assert_eq!(unfinished.total, 1);
    // The next checkpoint on the node clears it.
    let t2 = token_of(&ok(w.call(&c, s, take(n))));
    assert_eq!(
        w.s.nodes[&n].expired_hold,
        Some(gone),
        "a take is no checkpoint"
    );
    ok(w.call(&c, s, checkpoint(t2)));
    assert_eq!(w.s.nodes[&n].expired_hold, None);
    // A hold that checkpointed and then expired leaves nothing.
    w.advance(w.s.params.lease_ceiling);
    assert!(w.s.holds.is_empty());
    assert_eq!(w.s.nodes[&n].expired_hold, None);
}

// ------------------------------------------------------------------------------------------------
// R2, R9: closing a series, or merging from one, names the occurrence it dropped

#[test]
fn closing_a_series_names_the_dropped_occurrence_and_warns_series_stopped() {
    let (mut w, c, s) = one();
    let (x, o) = series(&mut w, s, ROOT, "daily");
    let d = ok(w.call(&c, s, close(x, Outcome::Done)));
    assert!(matches!(
        d.reply,
        Reply::Closed { dropped: Some(m), .. } if m == o
    ));
    let warn: Vec<_> = d
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::SeriesStopped)
        .collect();
    assert_eq!(warn.len(), 1);
    assert_eq!(warn[0].id, Some(x));
    assert_eq!(
        warn[0].detail,
        WarningDetail::SeriesStopped { occurrence: o }
    );
    assert!(!w.s.nodes[&o].is_open());
    // A plain close drops nothing.
    let y = node(&mut w, &c, s, "/y", &[]);
    let d = ok(w.call(&c, s, close(y, Outcome::Done)));
    assert!(matches!(d.reply, Reply::Closed { dropped: None, .. }));
    assert!(
        d.warnings
            .iter()
            .all(|w| w.code != WarningCode::SeriesStopped)
    );
}

#[test]
fn merging_from_a_series_warns_series_stopped() {
    let (mut w, c, s) = one();
    let (x, o) = series(&mut w, s, ROOT, "weekly");
    let into = node(&mut w, &c, s, "/into", &[]);
    let d = ok(w.call(&c, s, merge(x, into)));
    let warn: Vec<_> = d
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::SeriesStopped)
        .collect();
    assert_eq!(warn.len(), 1);
    assert_eq!(
        warn[0].detail,
        WarningDetail::SeriesStopped { occurrence: o }
    );
    assert!(!w.s.nodes[&o].is_open());
}

// ------------------------------------------------------------------------------------------------
// R3: the requests a create, a join, or a key add recorded

#[test]
fn a_create_names_the_requests_its_user_keys_recorded() {
    let (mut w, c, s) = one();
    let e1 = user_entry(&mut w, s, USER_PRINCIPAL);
    let e2 = user_entry(&mut w, s, USER_PRINCIPAL);
    let user = |e: Eid| Key {
        key: KeyStr::of_user_entry(e),
        nv: NormVer(0),
    };
    let d = ok(w.call(
        &c,
        s,
        create_with("/a", |a| {
            a.keys = vec![user(e2), user(e1)];
            a.take = Some(false);
        }),
    ));
    let Reply::Created {
        id, user_requests, ..
    } = d.reply.clone()
    else {
        panic!("{:?}", d.reply);
    };
    let routed = |w: &World, e: Eid| match w.s.user_entries[&e].routing {
        Routing::Request { request, node } if node == id => request,
        other => panic!("{other:?}"),
    };
    let mut want = vec![routed(&w, e1), routed(&w, e2)];
    want.sort_unstable();
    assert_eq!(user_requests, want);
    // A join that adds a user key names the request its routing recorded; one that adds none
    // names none.
    let e3 = user_entry(&mut w, s, USER_PRINCIPAL);
    let d = ok(w.call(
        &c,
        s,
        create_with("/b", |a| a.keys = vec![user(e1), user(e3)]),
    ));
    let Reply::Joined {
        existing,
        user_requests,
        ..
    } = &d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(*existing, id);
    assert_eq!(user_requests, &vec![routed(&w, e3)]);
    let d = ok(w.call(&c, s, create_with("/c", |a| a.keys = vec![user(e1)])));
    assert!(matches!(
        &d.reply,
        Reply::Joined { user_requests, .. } if user_requests.is_empty()
    ));
    // A node without user keys records none.
    let d = ok(w.call(&c, s, create_with("/d", |a| a.keys = vec![key("chat:x/9")])));
    assert!(matches!(
        &d.reply,
        Reply::Created { user_requests, .. } if user_requests.is_empty()
    ));
    let _ = ks;
}

// ------------------------------------------------------------------------------------------------
// R13: a `to_user` adds its principal to the author session's `told`

#[test]
fn a_to_user_tells_its_principal() {
    let (mut w, _c, s) = one();
    assert!(w.s.sessions[&s].told.is_empty());
    entry(
        &mut w,
        USER,
        Author::Session(s),
        EntryBody::ToUser {
            kind: ToUserKind::Result,
            verbatim: true,
            re: BTreeSet::new(),
            principal: USER_PRINCIPAL,
        },
    );
    assert_eq!(w.s.sessions[&s].told, [USER_PRINCIPAL].into());
    assert!(w.dirty.contains(&Fact::Session(s)));
    // A second report to the same principal changes nothing.
    entry(
        &mut w,
        USER,
        Author::Session(s),
        EntryBody::ToUser {
            kind: ToUserKind::Ack,
            verbatim: true,
            re: BTreeSet::new(),
            principal: USER_PRINCIPAL,
        },
    );
    assert!(!w.dirty.contains(&Fact::Session(s)));
}

// ------------------------------------------------------------------------------------------------
// Review E1-spec-1: the last value of every entity that leaves memory

#[test]
fn a_tracked_event_hands_over_each_departing_entity_with_its_last_value() {
    let (mut w, _c, s) = one();
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    // Routed to none, the entry is finished and leaves memory `FINISHED_RETAIN` later (D.4.12).
    w.emit(Ev::EntryRouted {
        entry: e,
        routing: Routing::NoAction,
        reason: Some(text("nothing to do")),
    });
    let entity = w.s.user_entries[&e];
    let meta = w.s.entries[&e];
    w.at(w.clock.plus(crate::state::FINISHED_RETAIN));
    let mut t = Tracked::new();
    crate::evolve::evolve_tracked(
        &mut w.s,
        &crate::event::Event {
            at: w.clock,
            ev: Ev::SessionStatus {
                session: s,
                text: text("later"),
            },
        },
        &mut t,
    );
    assert!(!w.s.user_entries.contains_key(&e));
    assert_eq!(
        t.departed.get(&Fact::UserEntry(e)),
        Some(&Departed::UserEntry(entity))
    );
    assert_eq!(t.departed.get(&Fact::Entry(e)), Some(&Departed::Meta(meta)));
    assert!(t.facts.contains(&Fact::UserEntry(e)));
    // An untracked fold records nothing and leaves the same state.
    let mut u = w.s.clone();
    crate::evolve::evolve_mut(
        &mut u,
        &crate::event::Event {
            at: w.clock,
            ev: Ev::SessionStatus {
                session: s,
                text: text("again"),
            },
        },
    );
    assert!(!u.user_entries.contains_key(&e));
}

// ------------------------------------------------------------------------------------------------
// 1.5 (D.14.7, D.8.16): a hello or a wait delivers at most ten notes, oldest first

#[test]
fn a_delivery_keeps_the_ten_oldest_notes() {
    let notes: Vec<NoteId> = (1..=13).rev().map(Eid).collect();
    let first = crate::helpers::agents::first(notes);
    assert_eq!(first, (1..=10).map(Eid).collect::<Vec<_>>());
    assert_eq!(crate::query::INBOX_DELIVER_MAX, 10);
    assert_eq!(crate::helpers::agents::first(vec![Eid(4)]), vec![Eid(4)]);
    assert!(crate::helpers::agents::first(Vec::new()).is_empty());
}

// ------------------------------------------------------------------------------------------------
// D.14.6: an entry the shell's table lacks is not found

#[test]
fn hiding_an_entry_on_an_unknown_node_is_not_found_entry() {
    let (mut w, c, s) = one();
    refused(
        &mut w,
        &c,
        s,
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: Eid(1),
                node: NodeId(u64::MAX),
            },
            reason: text("r"),
        }),
        nf(What::Entry),
    );
}

// ------------------------------------------------------------------------------------------------
// Review of WP-D1.4: `view agreements` past `SCAN_MAX`, its page bound, and its subtrees

/// Creates `count` children of `parent` by their events alone, each with `agreements` (empty for
/// none), named `<prefix><i>`, without the per-event checks (a large forum; the caller checks).
fn many(w: &mut World, s: SessionId, parent: NodeId, prefix: &str, count: u32, agreements: &str) {
    for i in 0..count {
        let id = w.s.counters.next_node;
        let mut init = blank_init("t", crate::state::Role::Plain);
        init.agreements = text(agreements);
        w.emit_raw(Ev::NodeCreated {
            id,
            parent,
            name: Name::new(&alloc::format!("{prefix}{i}")).unwrap(),
            origin: crate::state::Origin::Agent(s),
            init,
        });
    }
}

/// The page `view agreements` must give, by a plain pass over every node: the nodes of the
/// subtree of `under` with agreements, by id, all of them counted.
fn agreements_oracle(s: &crate::state::State, under: NodeId, limit: usize) -> (Vec<NodeId>, Total) {
    let all: Vec<NodeId> = s
        .nodes
        .iter()
        .filter(|(n, x)| !x.agreements.is_empty() && query::is_under(s, **n, under))
        .map(|(&n, _)| n)
        .collect();
    let total = Total::Exact(u32::try_from(all.len()).unwrap());
    (all.into_iter().take(limit).collect(), total)
}

#[test]
fn view_agreements_finds_a_later_branch_past_scan_max_nodes() {
    // Finding 1: a branch with more than `SCAN_MAX` nodes and lower ids does not hide the
    // agreements of a later branch, such as `/zz` here (§18: a partial answer never looks like
    // nothing; §23: hello.orientation, then `view agreements`).
    let (mut w, c, s) = one();
    let work = node(&mut w, &c, s, "/work", &[]);
    many(&mut w, s, work, "t", query::SCAN_MAX + 100, "");
    let zz = created(&ok(w.call(
        &c,
        s,
        create_with("/zz", |a| {
            a.take = Some(false);
            a.agreements = Some(text("review every merge"));
        }),
    )))
    .0;
    let d = ok(w.call(&c, s, view_agreements(None, None)));
    let Reply::Agreements { items, total } = d.reply else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(items.iter().map(|x| x.node).collect::<Vec<_>>(), vec![zz]);
    assert_eq!(total, Total::Exact(1));
    // And under `/work`, which has none, the answer is an exact nothing.
    let d = ok(w.call(&c, s, view_agreements(Some("/work"), None)));
    assert_eq!(
        d.reply,
        Reply::Agreements {
            items: Vec::new(),
            total: Total::Exact(0),
        }
    );
}

#[test]
fn view_agreements_says_at_least_when_it_stops_and_shows_at_most_500() {
    // Finding 4: past `SCAN_MAX` examined nodes the total is `≥k` (§18), and a page holds at most
    // 500 items whatever `limit` asks (§20.4).
    let (mut w, c, s) = one();
    let work = node(&mut w, &c, s, "/work", &[]);
    let n = query::SCAN_MAX + 100;
    many(&mut w, s, work, "t", n, "ship on green");
    w.check().unwrap();
    // Every node with agreements is under `/`: an exact total, and the page bound.
    let d = ok(w.call(&c, s, view_agreements(None, Some(1_000))));
    let Reply::Agreements { items, total } = d.reply else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(items.len(), 500);
    assert_eq!(total, Total::Exact(n));
    let d = ok(w.call(&c, s, view_agreements(None, None)));
    assert!(matches!(&d.reply, Reply::Agreements { items, .. } if items.len() == 50));
    // Under `/work` the subtree and the agreements both exceed `SCAN_MAX`: the walk stops, and
    // the total says so.
    let d = ok(w.call(&c, s, view_agreements(Some("/work"), Some(500))));
    let Reply::Agreements { items, total } = d.reply else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(items.len(), 500);
    let Total::AtLeast(k) = total else {
        panic!("an exact total past SCAN_MAX: {total:?}");
    };
    assert!(k >= 500 && k <= n, "{k}");
    // A small subtree is walked, not filtered out of the large index: an exact answer.
    let small = created(&ok(w.call(
        &c,
        s,
        create_with("/small", |a| {
            a.take = Some(false);
            a.agreements = Some(text("small's rule"));
        }),
    )))
    .0;
    node(&mut w, &c, s, "/small/x", &[]);
    let d = ok(w.call(&c, s, view_agreements(Some("/small"), None)));
    assert!(
        matches!(&d.reply, Reply::Agreements { items, total: Total::Exact(1) } if items[0].node == small),
        "{:?}",
        d.reply
    );
    let d = ok(w.call(&c, s, view_agreements(Some("/work/t7"), None)));
    assert!(
        matches!(&d.reply, Reply::Agreements { items, total: Total::Exact(1) } if items.len() == 1),
        "{:?}",
        d.reply
    );
}

#[test]
fn view_agreements_equals_a_plain_pass_under_every_node() {
    // Every `under`, both ways of reading (the subtree's walk and the index's filter), against a
    // pass over every node: open, closed, and merged nodes with agreements, at several depths.
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = created(&ok(w.call(
        &c,
        s,
        create_with("/a/b", |x| {
            x.take = Some(false);
            x.agreements = Some(text("b's rule"));
        }),
    )))
    .0;
    many(&mut w, s, b, "c", 6, "c's rule");
    many(&mut w, s, b, "d", 6, "");
    many(&mut w, s, a, "e", 5, "e's rule");
    let f = created(&ok(w.call(
        &c,
        s,
        create_with("/f", |x| {
            x.take = Some(false);
            x.agreements = Some(text("f's rule"));
        }),
    )))
    .0;
    many(&mut w, s, f, "g", 3, "g's rule");
    w.check().unwrap();
    ok(w.call(&c, s, close(f, Outcome::Done)));
    let e0 = crate::query::resolve(&w.s, &at("/a/e0")).unwrap();
    let e1 = crate::query::resolve(&w.s, &at("/a/e1")).unwrap();
    ok(w.call(&c, s, merge(e0, e1)));
    // Clearing a node's agreements takes it out.
    let x = &w.s.nodes[&b];
    let (v, dv) = (x.version, x.digest_version);
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(b),
            version: v,
            digest_version: Some(dv),
            token: None,
            patch: Patch {
                agreements: Some(Text::default()),
                ..Patch::default()
            },
        }),
    ));
    assert!(!w.s.ix.agreements.contains(&b));
    // A node made later deep in an earlier branch: the walk's preorder is not the id order.
    let late = created(&ok(w.call(
        &c,
        s,
        create_with("/a/b/c0/late", |x| {
            x.take = Some(false);
            x.agreements = Some(text("late rule"));
        }),
    )))
    .0;
    let page = query::agreements(&w.s, Some(b), 500, &mut NoMeter);
    assert_eq!(page.items.last().map(|x| x.node), Some(late));
    let ids: Vec<NodeId> = w.s.nodes.keys().copied().collect();
    for u in ids {
        for limit in [1_u32, 4, 500] {
            let page = query::agreements(&w.s, Some(u), limit, &mut NoMeter);
            let (want, total) = agreements_oracle(&w.s, u, usize::try_from(limit).unwrap());
            assert_eq!(
                page.items.iter().map(|x| x.node).collect::<Vec<_>>(),
                want,
                "under {u:?}, limit {limit}"
            );
            assert_eq!(page.total, total, "under {u:?}");
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Review of WP-D1.4: a status item is not a waiting node

#[test]
fn a_waiting_status_item_is_a_candidate_not_a_waiting_node() {
    // Finding 2: a waiting node with `open_commitment` is a status item (D.8.5's row "n open,
    // waiting(n), open_commitment"), a candidate: `ix.next.waiting` leaves it out, and `next`
    // offers it.
    let (mut w, c, s1) = one();
    let s2 = w.hello(&c);
    let conv = KeyStr::new("chat:x/1").unwrap();
    let (m, t) = held(&mut w, &c, s1, "/conv");
    ok(w.call(&c, s1, key_add(m, "chat:x/1")));
    let said = entry(
        &mut w,
        m,
        Author::Session(s1),
        EntryBody::Said {
            conv: conv.clone(),
            kind: SaidKind::Ack,
            corrects: None,
            voice: false,
        },
    );
    w.emit(Ev::CommitmentOpened {
        c: said,
        node: m,
        conv,
        followup_at: w.clock.plus(Dur(MINUTE.0)),
    });
    w.emit(Ev::SaidConfirmed {
        said,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    w.advance(Dur(MINUTE.0));
    ok(w.call(
        &c,
        s1,
        release_with(t, Some(WaitArg::Time(When::In(Dur(HOUR.0)))), None),
    ));
    assert!(w.s.nodes[&m].wait.is_some());
    assert!(w.s.ix.next.of.contains_key(&crate::state::Cand::Status(m)));
    assert!(w.s.ix.next.waiting.is_empty(), "{:?}", w.s.ix.next.waiting);
    assert!(crate::spec::waiting_nodes(&w.s, w.clock).is_empty());
    let d = ok(w.call(&c, s2, next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Status { node, .. }) if node == m),
        "{:?}",
        d.reply
    );
    assert_eq!(query::hello(&w.s, s2, w.clock).offers_why, None);
}

#[test]
fn an_occurrence_before_its_slot_is_not_a_waiting_node() {
    // D.8.5's first rows: an occurrence in phase `Waiting` is no candidate at all, so a wait on it
    // does not make it one of the nodes an empty `next` counts.
    let (mut w, _c, s) = one();
    let (_, o) = series(&mut w, s, ROOT, "daily");
    w.emit(Ev::OfferSet { node: o, on: true });
    w.emit(Ev::WaitSet {
        node: o,
        wait: Some(crate::state::WaitOn::Time(w.clock.plus(Dur(DAY.0)))),
    });
    assert!(w.s.nodes[&o].capacity > 0);
    assert!(w.s.ix.next.waiting.is_empty(), "{:?}", w.s.ix.next.waiting);
    assert!(crate::spec::waiting_nodes(&w.s, w.clock).is_empty());
}

// ------------------------------------------------------------------------------------------------
// Review of WP-D1.4: only an expired hold is an expired hold

#[test]
fn a_hold_ended_by_a_move_a_close_or_a_merge_is_no_expired_hold() {
    // Finding 3: §10.4's section 2 shows "the last hold that expired"; a hold that another
    // session's move, close, or merge ended did not expire.
    let (mut w, c, s1) = one();
    let s2 = w.hello(&c);
    let (a, ta) = held(&mut w, &c, s1, "/a");
    node(&mut w, &c, s2, "/b", &[]);
    ok(w.call(
        &c,
        s2,
        Cmd::Node(NodeCmd::Move {
            at: NodeRef::Id(a),
            to: crate::apply::tests::path("/b/a"),
        }),
    ));
    assert!(!w.s.holds.contains_key(&ta), "the move ended the hold");
    assert_eq!(w.s.nodes[&a].expired_hold, None);
    let (x, tx) = held(&mut w, &c, s1, "/x");
    ok(w.call(&c, s2, close(x, Outcome::Done)));
    assert!(!w.s.holds.contains_key(&tx), "the close ended the hold");
    assert_eq!(w.s.nodes[&x].expired_hold, None);
    let (y, ty) = held(&mut w, &c, s1, "/y");
    let z = node(&mut w, &c, s2, "/z", &[]);
    ok(w.call(&c, s2, merge(y, z)));
    assert!(!w.s.holds.contains_key(&ty), "the merge ended the hold");
    assert_eq!(w.s.nodes[&y].expired_hold, None);
    assert_eq!(w.s.nodes[&z].expired_hold, None);
    // None of them shows in the brief.
    for n in [a, x, y, z] {
        let b = query::brief(&w.s, n, Some(s2), w.clock, &mut NoMeter);
        assert!(
            b.sections
                .iter()
                .flat_map(|x| x.items.iter())
                .all(|i| !matches!(i, BriefItem::ExpiredHold(_))),
            "{n:?}"
        );
    }
}

// ------------------------------------------------------------------------------------------------
// Review of WP-D1.4: the inbox bound of `talk wait` (D.14.7), which WP-D5b's inbox and wait fill

#[test]
fn hello_and_wait_deliver_the_ten_oldest_notes_and_leave_the_rest() {
    // D.14.7: `hello` and `talk wait` deliver at most `INBOX_DELIVER_MAX` notes, oldest first
    // (`helpers::agents::deliverable`), and `InboxRead` covers exactly those.
    let (mut w, c, s1) = one();
    let s2 = w.hello(&c);
    let a = node(&mut w, &c, s1, "/a", &[]);
    let until = w.clock.plus(Dur(DAY.0));
    let mut notes = Vec::new();
    for _ in 0..26 {
        notes.push(entry(
            &mut w,
            a,
            Author::Session(s1),
            EntryBody::Note {
                to: Some(Addressee::Session(s2)),
                until: Some(until),
                recipients: [s2].into(),
                pending_holder: false,
                expects: None,
                reply_to: None,
            },
        ));
    }
    assert_eq!(query::inbox(&w.s, s2, w.clock), notes);
    let wait = || {
        Cmd::Talk(TalkCmd::Wait {
            timeout: Dur(0),
            for_: None,
            since: None,
        })
    };
    // A wait delivers the ten oldest.
    let d = ok(w.call(&c, s2, wait()));
    assert!(
        matches!(&d.reply, Reply::Waited { delivered, .. } if *delivered == notes[..10]),
        "{:?}",
        d.reply
    );
    assert_eq!(query::inbox(&w.s, s2, w.clock), notes[10..]);
    // A hello the next ten.
    let d = ok(w.call(
        &c,
        s2,
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: None,
            from_user: None,
        }),
    ));
    assert!(
        matches!(&d.reply, Reply::Hello { inbox, .. } if *inbox == notes[10..20]),
        "{:?}",
        d.reply
    );
    // The last six, then nothing.
    let d = ok(w.call(&c, s2, wait()));
    assert!(matches!(&d.reply, Reply::Waited { delivered, .. } if *delivered == notes[20..]));
    assert!(query::inbox(&w.s, s2, w.clock).is_empty());
}
