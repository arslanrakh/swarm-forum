//! Divergences the replay found between the model and the core (WP-F1), kept as regression tests
//! once resolved.
//!
//! **The Silence key (1.4, R27), resolved in the model.** §26 H2 (1.4): "A silent session holds no
//! live hold (§3)". The core queues a session's Silence at `max(silence_at, the largest lease_until
//! of its holds, last_now)` (D.4.8), as 1.4 says. The model's `deadlines.qnt` queued it at
//! `silence_at` (1.3), and its `inv_H2` and `inv_T3` kept 1.3's rule, so its own checks did not see
//! it. Of 800 runs of the four `holds` modules, 24 diverged, each on a session's silence. The model
//! now queues 1.4's key (`silence_key_at`), and `inv_H2` and `inv_T3` carry 1.4's clause.
//!
//! The witness was run 49 (of 200) of the `holds_two` slice (seed `0x5eed01`). Since 1.8 it is run
//! 50 of `holds_silence_two` (`quint/conformance.qnt`, the same seed), `holds_two` without 1.8's
//! intent check, since 1.8's model draws other runs: at state 7 session 1 checkpoints session 2's
//! hold `(8, 1)` (a token is the capability, H2: "using a token from another session does not
//! transfer the hold"), which extends the lease to tick 12 while session 2's `silence_at` is 9; at
//! state 8 the clock moves to 11, and session 2 stays present, holding `(8, 1)` live until 12.
//! Before the fix, the model made it silent there.
//!
//! **The post-settle (D.8.1), resolved in the model.** With 1.4's key, a command that ends another
//! session's last hold after that session's `silence_at` leaves its Silence key at the command's
//! `now` (D.4.8's floor at `last_now`), which D.9.2's T4 allows and the core's post-settle fires in
//! the same call. The model's commands did not settle (D.10.4), so its state after such a command
//! was not settled: simulation of `two_class` failed `inv_T3`, `inv_T4`, and `step_T4` (seeds
//! `0x93f4da67ca4de794` and `0x94a12e7bdfc1e83a`), and the replay would have diverged on the
//! session's `silent`. Every command action of the model now ends with the post-settle.
//!
//! The witness is the first run of `post_settle_two` (`quint/conformance.qnt`, seed `0x5eed0a`)
//! that ends with a release, with a `{time}` wait at its `now` (resolved at once by its marker,
//! A-68), of another session's last hold past that session's `silence_at`.

mod common;

use forum_conformance::{Options, Trace, V, Verdict, fixtures, project::at_tick, replay};
use forum_core::{Fence, NodeId, SessionId, Token};

fn witness() -> Trace {
    Trace::read(&fixtures("findings").join("holds_two_silence-0.itf.json"))
        .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_silence_key_witness_replays_with_no_divergence() {
    let r = replay(&witness(), "holds_two_silence", Options::default());
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.describe());
}

#[test]
fn a_holder_stays_present_past_its_silence_at_while_its_hold_is_live() {
    // Replayed to state 8: the clock is 11, past session 2's `silence_at` of 9, and its hold
    // `(8, 1)` is live until 12.
    let mut t = witness();
    t.states.truncate(9);
    let r = replay(&t, "holds_two_silence", Options::default());
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.describe());
    let s = r.state.unwrap_or_default();
    let x = s
        .sessions
        .get(&forum_core::SessionId(2))
        .unwrap_or_else(|| panic!("no session 2"));
    assert!(!x.silent);
    assert!(s.holds.values().any(|h| h.session.0 == 2));
}

fn post_settle_witness() -> Trace {
    Trace::read(&fixtures("findings").join("post_settle_release-0.itf.json"))
        .unwrap_or_else(|e| panic!("{e}"))
}

fn get<'a>(v: &'a V, field: &str) -> &'a V {
    v.get(field).unwrap_or_else(|e| panic!("{e}"))
}

fn int(v: &V) -> u64 {
    v.as_u64().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_post_settle_witness_replays_with_no_divergence() {
    let t = post_settle_witness();
    assert_eq!(t.status, "violation");
    let r = replay(&t, "post_settle_release", Options::default());
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.describe());
    assert_eq!(r.replayed, t.states.len());
}

#[test]
fn a_release_of_another_sessions_last_hold_fires_its_silence_in_the_same_call() {
    let t = post_settle_witness();
    let last = t.states.last().unwrap_or_else(|| panic!("an empty trace"));
    let la = last
        .get("last_action")
        .unwrap_or_else(|| panic!("no last_action"));
    let clock = int(last.get("clock").unwrap_or_else(|| panic!("no clock")));
    let (tag, args) = get(la, "cmd").as_tag().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(tag, "CRelease");
    let (wait, at) = get(args, "wait").as_tag().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (wait, int(at)),
        ("WaTime", clock),
        "a wait at the release's now"
    );
    let (n, f) = get(args, "token")
        .as_int_pair()
        .unwrap_or_else(|e| panic!("{e}"));
    let token = Token {
        node: NodeId(u64::try_from(n).unwrap_or_default()),
        fence: Fence(u64::try_from(f).unwrap_or_default()),
    };
    let caller = SessionId(int(get(la, "sess")));

    // The core before the release: another session holds the token, and no other hold; it is
    // present, and its `silence_at` has passed.
    let mut before = t.clone();
    before.states.truncate(t.states.len() - 1);
    let r = replay(&before, "post_settle_release", Options::default());
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.describe());
    let b = r.state.unwrap_or_default();
    let holder = b
        .holds
        .get(&token)
        .map(|h| h.session)
        .unwrap_or_else(|| panic!("no hold {token:?}"));
    assert_ne!(holder, caller);
    assert_eq!(b.holds.values().filter(|h| h.session == holder).count(), 1);
    let x = b
        .sessions
        .get(&holder)
        .unwrap_or_else(|| panic!("no session {holder:?}"));
    assert!(!x.silent);
    assert!(x.silence_at <= at_tick(clock));

    // After it, in the same call: the hold has ended and the holder is silent, in the core and in
    // the model.
    let a = replay(&t, "post_settle_release", Options::default())
        .state
        .unwrap_or_default();
    assert!(!a.holds.contains_key(&token));
    assert!(a.sessions.get(&holder).is_some_and(|x| x.silent));
    let sessions = get(last.get("s").unwrap_or_else(|| panic!("no s")), "sessions")
        .as_map()
        .unwrap_or_else(|e| panic!("{e}"));
    let model = sessions
        .get(&V::int(holder.0))
        .unwrap_or_else(|| panic!("no session {holder:?} in the model"));
    assert_eq!(get(model, "silent").as_bool(), Ok(true));
}
