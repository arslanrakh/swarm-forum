//! History replay (design D.12.6, D.11.6, FQ-2): folding the history through `evolve` from Genesis
//! must give the state the tables hold, and `check_invariants` must hold at its end. CI runs it on
//! the simulation's databases and on a fixture per released `state_version` ([`verify_checked`]);
//! `forum-server replay --database <db> --into <new>` uses the same code to fold the history into
//! new state tables, a last resort if the state tables were damaged ([`replay_into_checked`]).
//!
//! A production history can only be replayed through `evolve`, never re-executed: the transaction
//! rows keep no command (D.11.6).

use forum_core::{Ev, Settled, State, evolve_mut, evolve_tracked, settle};
use rusqlite::Connection;

use super::history::{events_after, events_of_tx, insert_tx, txs_after};
use super::load::{Checks, check_loaded, load_state};
use super::schema::{SHELL_VERSION, STATE_VERSION, create_tables, get_meta, meta, set_meta};
use super::state::{put_op, stored_op};
use super::{Result, StoreError, write};

/// Events read per page.
const PAGE: u32 = 1_000;

/// Folds the whole history through `evolve` from the empty state, checking that the `seq`s are 1,
/// 2, 3, … without gaps.
///
/// # Errors
///
/// A gap, an event that does not decode, or a failed query.
pub fn replay(conn: &Connection) -> Result<State> {
    let mut s = State::default();
    let mut after = 0_u64;
    loop {
        let page = events_after(conn, after, PAGE)?;
        let Some(last) = page.last() else {
            break;
        };
        let last = last.seq;
        for x in page {
            if x.seq != after.saturating_add(1) {
                return Err(StoreError::Replay(format!(
                    "the history has a gap before seq {}",
                    x.seq
                )));
            }
            after = x.seq;
            evolve_mut(&mut s, &x.event);
        }
        after = last;
    }
    Ok(s)
}

/// The fields of the base, history, and index parts in which two states differ.
#[must_use]
pub fn differences(a: &State, b: &State) -> Vec<&'static str> {
    let mut d = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {
            $( if a.$f != b.$f { d.push(stringify!($f)); } )*
        };
    }
    cmp!(
        params,
        last_now,
        counters,
        anomalies,
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
        digest_left,
        ix
    );
    d
}

/// Replays the history and compares the result with the state the tables hold (D.12.6), returning
/// the replayed state. It runs no invariant check: `verify_checked` does.
///
/// # Errors
///
/// `Replay` naming the fields that differ, or a failure of the replay or the load.
pub fn verify(conn: &Connection) -> Result<State> {
    let replayed = replay(conn)?;
    let loaded = load_state(conn)?;
    let d = differences(&replayed, &loaded);
    if d.is_empty() {
        Ok(replayed)
    } else {
        Err(StoreError::Replay(format!("the fields {d:?} differ")))
    }
}

/// The check that ends a replay (D.12.6, D.13.2): a copy of the replayed state is settled to its
/// `last_now`, as startup's step 3 settles a loaded state (a crash between two settle batches can
/// leave due deadlines queued), and must pass the full check of D.11.8 step 4, `check_keys` and
/// `check_invariants`.
///
/// # Errors
///
/// `Check` with the violations, or `Replay` when the settle is stuck on a key.
pub fn check_replayed(s: &State) -> Result<()> {
    let mut settled = s.clone();
    let now = settled.last_now;
    loop {
        let (_, how) = settle(&mut settled, now, u32::MAX);
        match how {
            Settled::Done => break,
            Settled::More => {}
            Settled::Stuck(k) => {
                return Err(StoreError::Replay(format!(
                    "settling the replayed state is stuck on {k:?}"
                )));
            }
        }
    }
    check_loaded(&settled, Checks::Full)
}

/// `verify`, then `check_replayed`: what the replay tests of CI and `forum-server replay` run on
/// a history (D.12.6). `verify` alone serves histories that are not meant to keep the invariants,
/// such as the random histories of the store's own tests.
///
/// # Errors
///
/// As `verify` and `check_replayed`.
pub fn verify_checked(conn: &Connection) -> Result<State> {
    let s = verify(conn)?;
    check_replayed(&s)?;
    Ok(s)
}

/// `replay_into`, then `check_replayed` on the state it wrote: `forum-server replay --into`.
///
/// # Errors
///
/// As `replay_into` and `check_replayed`.
pub fn replay_into_checked(src: &Connection, dst: &Connection) -> Result<State> {
    let s = replay_into(src, dst)?;
    check_replayed(&s)?;
    Ok(s)
}

/// Folds the history of `src` into new tables in `dst`, an empty database, inside the caller's
/// transaction on `dst`: every transaction of `src` is written again, in id order, with the same
/// events, the shell's rows, and the state rows its events dirtied, exactly as the writer wrote
/// them (`forum-server replay --into`, D.11.6). The history keeps no rendered response, so an
/// `st_ops` row takes its response and masking version from `src`'s row when `src` still holds the
/// record, and an empty response otherwise (a record that `src` no longer holds expired before the
/// end of the history, and its row goes with it). `dst`'s `meta` is `src`'s, with this release's
/// `state_version` and `shell_version`, the versions of the tables it wrote. It runs no invariant
/// check: `replay_into_checked` does.
///
/// # Errors
///
/// A failed statement on either side, or an event that does not decode.
pub fn replay_into(src: &Connection, dst: &Connection) -> Result<State> {
    create_tables(dst)?;
    let mut st = src.prepare("SELECT key, value FROM meta ORDER BY key")?;
    let metas: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (k, v) in &metas {
        set_meta(dst, k, v)?;
    }
    if get_meta(dst, meta::STATE_VERSION)?.is_none() {
        return Err(StoreError::Invalid(
            "the source has no state_version".to_owned(),
        ));
    }
    // `dst` has this release's tables and rows, whatever the source's versions were: a source of
    // an older version keeps its versions in its own `meta`, and its history replays through this
    // release's `evolve` (D.7.6), so the next startup must not migrate `dst` again.
    set_meta(dst, meta::STATE_VERSION, &STATE_VERSION.to_string())?;
    set_meta(dst, meta::SHELL_VERSION, &SHELL_VERSION.to_string())?;
    let mut s = State::default();
    let mut after = 0_i64;
    loop {
        let txs = txs_after(src, after, PAGE)?;
        let Some(last) = txs.last() else {
            break;
        };
        after = last.id;
        for t in txs {
            let events: Vec<forum_core::Event> = events_of_tx(src, t.id)?
                .into_iter()
                .map(|x| x.event)
                .collect();
            if events.is_empty() {
                // A migration's transaction: no events, only its row.
                insert_tx(dst, &t.meta)?;
                continue;
            }
            let mut dirty = forum_core::Tracked::new();
            for e in &events {
                evolve_tracked(&mut s, e, &mut dirty);
            }
            write(dst, &t.meta, &events, &s, &dirty)?;
            for e in &events {
                if let Ev::OpRecorded { class, op, .. } = &e.ev
                    && s.ops.contains_key(&(class.clone(), op.clone()))
                {
                    let (mv, response) = stored_op(src, class.as_str(), op.as_str())?
                        .map_or((0, Vec::new()), |x| (x.masking_version, x.response));
                    put_op(dst, &s, class, op, mv, &response)?;
                }
            }
        }
    }
    Ok(s)
}
