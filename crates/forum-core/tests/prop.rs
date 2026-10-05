//! The property test of design D.12.3, over the holds universe of WP-D3, the keys and
//! restructuring universe of WP-D4, the research universe of WP-D5b, and the full universe of
//! WP-D6: random sequences of commands and advances of the clock through the public API, as the
//! shell runs them (pre-settle, `apply_tracked`, post-settle), with every check after every step.
//!
//! proptest is not a dependency of `forum-core` (whether to add it is still open, see the package
//! report), so the cases come from a seeded generator (xorshift64*): each case is reproducible from
//! its seed, which a failure prints. `FORUM_PROP_CASES` and `FORUM_PROP_STEPS` set the number of
//! cases and of steps per case (256 and 200 by default, D.12.3's CI size; the full universe runs
//! 32 cases in a debug build, see `run_profile`); `FORUM_PROP_SEED` runs one case.
//!
//! The universe: 3 classes (1 in the one-class profile), up to 6 sessions (some with caps, some with horizons, some from a
//! token with default caps), names `{a, b, c}` to depth 3 and the persons and teams of `@people`
//! and `@teams`, caps and needs from `{x, y}`, `refusal.quorum` 7 (see `params`), advances from 0 to 3 days,
//! `digest.max` 32 and `text.max` 64 with texts of 0 to 80 bytes (A-114). The commands are those of
//! WP-D3: `hello`, `view`, `node create`, `update`, `close`, `hide`, every `hold` action, `pause`,
//! and `unpause`; tokens, nodes, and versions are drawn from the current state, sometimes stale or
//! unknown, and `op_id`s are reused. So that commands mostly succeed (D.12.3), a call is mostly made
//! by a session that holds something, a session that holds nothing mostly takes rather than write
//! with a stale token, a take mostly targets a free agent node, the texts of hold writes are rarely
//! over `text.max`, and an `op_id`'s digest is that of its command (a reused one is a replay when
//! the command repeats). `FORUM_PROP_STATS` prints the outcomes per command and the firings per
//! kind.
//!
//! The keys universe (WP-D4, D.12.3's restructuring-heavy profile) adds the keys `{chat:x/1,
//! chat:x/2, issue:p#1}` and, 4 % of the time, a `repeat:` key, which every command refuses (item 9);
//! request origins among those keys and the messages `{chat:x/1/1, chat:x/1/2, issue:p#1/1}`, with
//! `where` or without (A-61); and `node create` with keys (create-or-join), `node key`, `node
//! link`, `node reopen`, `node move` (into and out of `@people` and `@teams`, A-113), `node
//! merge` (a third of them aimed at a cycle of dependencies and waits), and `node update` of
//! `depends_on` between open nodes, beside the holds commands. Each case draws `tree.max_children`
//! 2 or 6 and `tree.max_depth` 3 or 16, so that creates and moves meet both limits of L1. Every step
//! runs the step checks of K7, N5 (with Settled's merge clause), C1, and L1's half for series, and
//! the state checks of K1, K3, K4, K5, K8, Q1, and S1. `conv_node` against `spec::conv_node` (item
//! 8) is the crate's own randomized test `conv_node_agrees_with_spec_on_random_trees`: the helper
//! is not public.
//!
//! The keys universe reaches no series, `said`, commitment, people question, objection, or mark;
//! the clauses of the step checks that guard them (C1's commitments, N5's moves of questions,
//! commitments, objections, `said` entries, and marks, Settled's merge credits, and L1's half for
//! series) run in the research universe (WP-D5b) and the full universe (WP-D6), and on hand-emitted
//! events in the unit tests of `cmd/restructure_tests.rs`.
//!
//! The full universe (WP-D6, D.12.3) draws every command: the research universe's, the user's
//! side (`from_user` answering, routing, and clarifying, `to_user` of each kind naming requests,
//! entries, and digest entries, `route`, `ask` to a human with and without a default, `hello` with
//! a principal or a message, principals `test:p1` and `test:p2`), and recurring work (a series with
//! a 10-minute cron and daily ones, `update {repeat}`, edits of a series while its occurrence
//! waits, takes, closes, and merges of occurrences, a series merged into its own occurrence);
//! `refusal.quorum` 1 or 2, so that E1's questions for the user, their defaults, drops, and lapses
//! are reached; advances past FINISHED_RETAIN and DIGEST_RETAIN; and the shell's restore step
//! before every command (D.4.12), from the rows the departures left (`Tracked::departed`), with
//! its own checks. Some of its advances stop just past an entity's leaving time with no event
//! since, and a call from a new session then names that entity: the restore brings it back first
//! (WP-D6's fix of the restore plan), and the command finds it, with the outcome it has on a twin
//! state where the entity never left (item 12, up to a `moved` warning's total, which memory makes
//! "at least", D.4.12). A targeted flow takes an available occurrence just before its window ends
//! and has another session move it after the window, so the move ends the hold and the occurrence
//! is missed in that step (A-80, R2). Beside the checks below it runs item 8 for the user's side
//! (`hello`'s items and counts against `spec::hello_items`, U1).
//!
//! The full universe also runs in four of D.12.3's profiles, each a parameterisation of it: one
//! class; no user (CS-2, CS-6); churn (sessions that open with their one or two calls, long
//! advances past `session.retain`, `op_id`s reused across sessions, CS-8, CS-17); and refusals
//! (open and queried `next`, `cannot` releases, changes of needs and caps, offered nodes with
//! needs and requests, CS-7). D.12.3's other profiles are covered by the other universes (holds,
//! restructuring in keys, people and research, many principals by the research universe's user
//! side) and by WP-D7's guided profile.
//!
//! Checked for every step (D.12.3): item 1, `check_fire` after each firing, `check_step` after the
//! command, `check_invariants` after the post-settle; item 2, a retry with the same `op_id` from the
//! same session or with no session (a call that would open one) is `Replay` and appends nothing,
//! from another session or with another digest `op_id_conflict`, after `op.retain` it executes
//! afresh, and a text over `text.max` is refused whatever the record; item 3, T2 at random points;
//! item 4, settling again emits nothing; item 5, the log folds to the state (and, with the feature
//! `serde`, every event survives a JSON round trip); item 6, a second run of the same steps gives
//! the same log; item 7, `decide` equals `apply`; item 8, the refusal predicates, `ranked`, and
//! `leave_better` (both forms, against an oracle written here from D.8.7: `spec` is WP-D2's and has
//! none) equal their naive counterparts for every session and node; item 11, the dirty sets cover
//! every changed fact; item 12, the state stays bounded; item 13, closed nodes stay clean.
//!
//! The next calls (WP-D7, D.8.26): after every call, `query::guide` for the calling session passes
//! `check_guide` (G1, item 1) and equals `spec::guide`, and so does `query::guide` for every other
//! session on the same response (item 8). The guided profile (`Profile::Guided`) adds items 14 and
//! 15: a third of the sessions follow their first `now` next call at once with the standard fill,
//! which must be accepted and remove the condition that produced it, and a guided session whose
//! response has no next calls owes nothing that a standing rule tracks, and has told every person
//! it serves who has items pending (1.4, R13). The research and guided universes also run the user
//! side (`research_user_command`): sessions opened with a principal, `from_user` with and without
//! a node, answering a user question or clarifying a request, `to_user` of each kind naming
//! entries, requests, and digest entries, `route`, `ask` to the user, and a person whose keys make
//! two principals aliases, so that rules 1, 9 (`answers_next`, `route_item`), and 12, the user
//! requests of rules 6 and 7, and R3's recorded requests meet `spec::guide` and G1. The full
//! universe draws the research universe's commands, this user side among them (but in the no-user
//! profile), and checks the next calls after every call like every other universe.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    clippy::disallowed_methods,
    reason = "test code: the deny list guards the crate's own code, and this test reads its size \
              from the environment"
)]

use std::collections::{BTreeMap, BTreeSet};

use forum_core::facts::changed_facts;
use forum_core::flags::CLOSED_TAKE;
use forum_core::query::{self, NoMeter};
use forum_core::*;

/// Every entity that left memory in a job of one instant is in `Tracked::departed` with its value
/// before the job, and every departed value is the entity's before the job (review E1-spec-1).
fn departures(seed: u64, before: &State, after: &State, t: &Tracked) {
    for f in changed_facts(before, after) {
        if let Some(v) = forum_core::facts::departed_of(before, &f)
            && forum_core::facts::departed_of(after, &f).is_none()
        {
            assert_eq!(
                t.departed.get(&f),
                Some(&v),
                "seed {seed}: the departure of {f:?}"
            );
        }
    }
    for (f, v) in &t.departed {
        assert_eq!(
            forum_core::facts::departed_of(before, f).as_ref(),
            Some(v),
            "seed {seed}: {f:?} departed with another value"
        );
    }
}

/// xorshift64*.
#[derive(Clone)]
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
}

const START: Time = Time(1_700_000_000_000);
const CLASSES: [&str; 3] = ["a", "b", "c"];
const NAMES: [&str; 3] = ["a", "b", "c"];
const LABELS: [&str; 2] = ["x", "y"];

fn label(l: &str) -> Label {
    Label::new(l).unwrap()
}

fn text_of(rng: &mut Rng) -> Text {
    let len = match rng.below(10) {
        0 => 0,
        1 => 64,
        2 => 65,
        3 => 32 + rng.below(49) as usize,
        _ => 1 + rng.below(12) as usize,
    };
    Text("t".repeat(len))
}

/// A text of a hold write (`done`, `why`): over `text.max` 3 % of the time, so that most
/// checkpoints, renewals, and releases reach the token and the hold (D.12.3: "commands mostly
/// succeed").
fn hold_text(rng: &mut Rng) -> Text {
    let len = match rng.below(100) {
        0..=2 => 65,
        3..=7 => 0,
        _ => 1 + rng.below(12) as usize,
    };
    Text("t".repeat(len))
}

fn nonempty(rng: &mut Rng) -> Text {
    if rng.chance(5) {
        Text(String::new())
    } else {
        Text("w".repeat(1 + rng.below(8) as usize))
    }
}

fn labels(rng: &mut Rng) -> BTreeSet<Label> {
    LABELS
        .iter()
        .filter(|_| rng.chance(30))
        .map(|l| label(l))
        .collect()
}

/// The universe's parameters. In the holds, keys, and research universes, written when E1's
/// question for the user was a stub (WP-D5a's `e1_check`, D.13.1), a class never reaches its
/// quorum: `refusal.quorum` is above the sessions a class can have, and a `cannot` only backs the
/// class off (CS-7). D.12.3's quorum of 1 or 2 is the full universe's and its profiles' (WP-D6).
///
/// The keys universe draws its tree limits per case: `tree.max_children` 2 or 6 and
/// `tree.max_depth` 3 or 16, so that creates and moves meet both limits of L1 (with three names a
/// parent never has more than three children, and paths are at most four steps deep), and also
/// run free of them.
fn params(rng: &mut Rng, profile: Profile) -> Params {
    let pick = rng.below(if profile == Profile::Keys { 4 } else { 2 });
    let (children, depth) = match profile {
        Profile::Holds
        | Profile::Research
        | Profile::Guided
        | Profile::Full
        | Profile::OneClass
        | Profile::NoUser
        | Profile::Churn
        | Profile::Refusal => (6, Params::default().tree_max_depth),
        Profile::Keys => (
            if pick & 1 == 1 { 2 } else { 6 },
            if pick & 2 == 2 { 3 } else { 16 },
        ),
    };
    // The full universe and its parameterisations reach E1's question for the user at a quorum of
    // 1 or 2 (D.12.3). Every profile runs the default `question.max_rechecks`: in the research and
    // guided universes a question to a person reaches its last recheck, so it expires (an
    // assumption mark and a digest entry) or is escalated to the user (rules 10 and 12, WP-D7's
    // review; WP-D5a's `add_digest` and `create_uq`, stubs when WP-D5b's research profile was
    // written, are in).
    let quorum = if profile.full() {
        1 + rng.below(2) as u32
    } else {
        7
    };
    Params {
        digest_max: 32,
        text_max: 64,
        refusal_quorum: quorum,
        hold_per_session: 2,
        tree_max_children: children,
        tree_max_depth: depth,
        // 1.8: within the universes' advances of up to 3 days, so that offers go stale (§14).
        offer_stale: Some(Dur(2 * 86_400_000)),
        ..Params::default()
    }
}

/// The world of one case: the state, the clock, the log, and the sessions opened, with their class.
#[derive(Clone)]
struct Run {
    s: State,
    clock: Time,
    log: Vec<Event>,
    sessions: Vec<(ClassName, SessionId)>,
    ops: Vec<(ClassName, SessionId, OpCtx, Cmd)>,
    /// Outcomes by command and result, for `FORUM_PROP_STATS`.
    stats: BTreeMap<String, u64>,
    /// The full universe (WP-D6): the shell's restore step runs before every command (D.4.12),
    /// from `rows`.
    restores: bool,
    /// The last value of every entity that left memory, as the shell's not-resident rows hold it
    /// (`Tracked::departed`, D.11.6).
    rows: BTreeMap<Fact, Departed>,
    /// Each session's last next calls (WP-D7): what a guided session follows (items 14 and 15).
    guidance: BTreeMap<SessionId, query::guide::Guidance>,
    /// Each session's last outcome: a response that reaches nothing else carries no next calls
    /// (§20.9), and rule 12 runs only after a call that succeeded (rule E).
    last: BTreeMap<SessionId, Result<Done, Error>>,
    /// The checkpoint entries, releases among them, that a guided session made by following a
    /// next call (item 15): a request they left unfinished was left so by the guide.
    guided_checkpoints: BTreeSet<Eid>,
}

impl Run {
    fn new(p: Params) -> Run {
        let mut s = State::default();
        let g = genesis(p, START);
        evolve_mut(&mut s, &g);
        Run {
            s,
            clock: START,
            log: vec![g],
            sessions: Vec::new(),
            ops: Vec::new(),
            stats: BTreeMap::new(),
            restores: false,
            rows: BTreeMap::new(),
            guidance: BTreeMap::new(),
            last: BTreeMap::new(),
            guided_checkpoints: BTreeSet::new(),
        }
    }

    /// The shell's restore step before a command (D.4.12, WP-D6's fix): every entity the command
    /// names that left memory and has a row, or that is resident with its leaving time at or
    /// before `now` (the command's first event would prune it), is restored from its row (here
    /// `rows`, or the resident value, which is what the row of a resident entity holds), by one
    /// `Restored` event, with its checks: the fold, the dirty set, the departures, no anomaly.
    fn restore(&mut self, seed: u64, cmd: &Cmd) {
        let now = self.clock.max(self.s.last_now);
        let items: Vec<Restored> = named(self, cmd)
            .into_iter()
            .filter_map(|x| item_of(self, x, now))
            .collect();
        if items.is_empty() {
            return;
        }
        let before = self.s.clone();
        let mut dirty = Tracked::new();
        let evs = restore_tracked(&mut self.s, self.clock, items, &mut dirty);
        assert_eq!(
            self.s.anomalies, before.anomalies,
            "seed {seed}: a restore was refused"
        );
        let missing: Vec<Fact> = changed_facts(&before, &self.s)
            .into_iter()
            .filter(|f| !dirty.facts.contains(f))
            .collect();
        assert!(
            missing.is_empty(),
            "seed {seed}: restore untracked {missing:?}"
        );
        departures(seed, &before, &self.s, &dirty);
        let mut folded = before;
        for e in &evs {
            evolve_mut(&mut folded, e);
        }
        assert!(folded == self.s, "seed {seed}: the restore is not its fold");
        self.rows.extend(dirty.departed);
        *self.stats.entry(String::from("restored")).or_insert(0) += 1;
        self.log.extend(evs);
    }

    /// The pre- and post-settle: one firing at a time, each checked.
    fn settle(&mut self, seed: u64) {
        while let Some(key) = self.s.ix.deadlines.first().cloned() {
            if key.at > self.clock {
                break;
            }
            let before = self.s.clone();
            let mut dirty = Tracked::new();
            let (evs, st) = settle_tracked(&mut self.s, self.clock, 1, &mut dirty);
            assert!(
                !matches!(st, Settled::Stuck(_)),
                "seed {seed}: stuck {st:?}"
            );
            if let Err(v) = check_fire(&before, &key, &evs, &self.s) {
                panic!("seed {seed}: check_fire {key:?}: {v:?}");
            }
            let missing: Vec<Fact> = changed_facts(&before, &self.s)
                .into_iter()
                .filter(|f| !dirty.facts.contains(f))
                .collect();
            assert!(missing.is_empty(), "seed {seed}: untracked {missing:?}");
            departures(seed, &before, &self.s, &dirty);
            self.rows.extend(dirty.departed);
            *self
                .stats
                .entry(format!("fired {:?}", key.kind))
                .or_insert(0) += 1;
            self.log.extend(evs);
        }
    }

    /// One call through the pipeline, with its checks.
    fn call(
        &mut self,
        seed: u64,
        class: &ClassName,
        sc: SessionCtx,
        op: Option<OpCtx>,
        cmd: &Cmd,
    ) -> Result<Done, Error> {
        self.settle(seed);
        // The shell restores unless a live `op_id` record answers (D.4.12, D.8.1).
        let live = op.as_ref().is_some_and(|o| {
            self.s
                .ops
                .get(&(class.clone(), o.id.clone()))
                .is_some_and(|r| r.expires_at > self.clock)
        });
        if self.restores && !live {
            self.restore(seed, cmd);
        }
        let ctx = Ctx {
            now: self.clock,
            class: class.clone(),
            caps: (class.as_str() == "c").then(|| [label("x")].into()),
            session: sc,
            op,
        };
        let before = self.s.clone();
        let before_anomalies = before.anomalies;
        let pure = decide(&before, &ctx, cmd);
        let mut dirty = Tracked::new();
        let (evs, out) = apply_tracked(&mut self.s, &ctx, cmd, &mut dirty);
        assert_eq!(
            pure,
            (evs.clone(), out.clone()),
            "seed {seed}: decide != apply"
        );
        if let Err(v) = check_step(&before, &ctx, cmd, &out, &evs, &self.s) {
            panic!("seed {seed}: check_step {cmd:?} -> {out:?}: {v:?}\nevents {evs:?}");
        }
        let missing: Vec<Fact> = changed_facts(&before, &self.s)
            .into_iter()
            .filter(|f| !dirty.facts.contains(f))
            .collect();
        assert!(
            missing.is_empty(),
            "seed {seed}: {cmd:?} untracked {missing:?}"
        );
        departures(seed, &before, &self.s, &dirty);
        self.rows.extend(dirty.departed);
        let mut folded = before;
        for e in &evs {
            evolve_mut(&mut folded, e);
        }
        assert!(
            folded == self.s,
            "seed {seed}: the fold differs after {cmd:?}"
        );
        if matches!(cmd, Cmd::Node(NodeCmd::Move { .. }))
            && evs.iter().any(|e| {
                matches!(
                    e.ev,
                    Ev::NodeClosed {
                        outcome: Outcome::Missed,
                        ..
                    }
                )
            })
        {
            *self
                .stats
                .entry(String::from("missed by a move"))
                .or_insert(0) += 1;
        }
        let name = format!("{cmd:?}");
        let name: String = name
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '(')
            .collect();
        let res = match &out {
            Ok(_) => String::from("ok"),
            Err(e) => format!("{:?}", e.hint),
        };
        *self.stats.entry(format!("{name} {res}")).or_insert(0) += 1;
        let anomalies = self.s.anomalies;
        assert!(
            anomalies == before_anomalies,
            "seed {seed}: an anomaly in {cmd:?}: {evs:?}"
        );
        let gs = guide_session(&ctx, &out, &evs);
        self.log.extend(evs);
        if let Ok(d) = &out
            && let Some(x) = d.opened
        {
            self.sessions.push((class.clone(), x));
        }
        self.settle(seed);
        if let Err(v) = check_invariants(&self.s, self.clock) {
            panic!("seed {seed}: invariants after {cmd:?}: {v:?}");
        }
        // Item 1's G1 and item 8's `guide == spec::guide` (1.3, D.8.26): for the calling session,
        // as the writer computes them, and for every other session on the same response.
        let g = query::guide(&self.s, gs, self.clock, cmd, &out, &mut NoMeter);
        if let Err(v) = check_guide(&self.s, gs, self.clock, cmd, &out, &g) {
            panic!("seed {seed}: G1 after {cmd:?} -> {out:?}: {v:?}\nnext calls {g:?}");
        }
        assert_eq!(
            g,
            spec::guide(&self.s, gs, self.clock, cmd, &out),
            "seed {seed}: guide != spec::guide after {cmd:?} -> {out:?}"
        );
        for &x in self.s.sessions.keys() {
            if Some(x) != gs {
                assert_eq!(
                    query::guide(&self.s, Some(x), self.clock, cmd, &out, &mut NoMeter),
                    spec::guide(&self.s, Some(x), self.clock, cmd, &out),
                    "seed {seed}: guide != spec::guide for {x:?} after {cmd:?} -> {out:?}"
                );
            }
        }
        for item in &g.items {
            *self
                .stats
                .entry(format!("next call {}", item.rule.name()))
                .or_insert(0) += 1;
            let because: String = format!("{:?}", item.because)
                .chars()
                .take_while(|c| c.is_alphanumeric())
                .collect();
            *self
                .stats
                .entry(format!("next call because {because}"))
                .or_insert(0) += 1;
        }
        if let Some(x) = gs {
            self.guidance.insert(x, g);
            self.last.insert(x, out.clone());
        }
        out
    }
}

/// The session `guide` reads for a call, as the writer passes it (D.8.26): the call's session, or
/// the one a first call opened, whether it succeeded or not; none when the session check refused
/// the call.
fn guide_session(ctx: &Ctx, out: &Result<Done, Error>, events: &[Event]) -> Option<SessionId> {
    if matches!(
        out,
        Err(Error {
            hint: Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch),
            ..
        })
    ) {
        return None;
    }
    match ctx.session {
        SessionCtx::Existing(x) => Some(x),
        SessionCtx::New => out.as_ref().ok().and_then(|d| d.opened).or_else(|| {
            events.iter().find_map(|e| match &e.ev {
                Ev::SessionOpened { session, .. } => Some(*session),
                _ => None,
            })
        }),
        SessionCtx::System => None,
    }
}

/// Item 8 and item 13, on the settled state.
fn agree_with_spec(seed: u64, r: &Run) {
    let now = r.clock;
    for &s in r.s.sessions.keys() {
        let (got, _) = query::ranked(
            &r.s,
            s,
            now,
            &NextQuery::default(),
            usize::MAX,
            &mut NoMeter,
        );
        let got: Vec<Cand> = got.into_iter().map(|x| x.cand).collect();
        assert_eq!(
            got,
            spec::ranked(&r.s, s, now, &NextQuery::default()),
            "seed {seed}: ranked"
        );
    }
    for (&n, node) in &r.s.nodes {
        for c in CLASSES {
            let c = ClassName::new(c).unwrap();
            assert_eq!(
                refusals::excluded(&r.s, n, &c, now),
                spec::excluded(&r.s, n, &c, now),
                "seed {seed}: excluded"
            );
        }
        assert_eq!(
            refusals::all_refused(&r.s, n, now),
            spec::all_refused(&r.s, n, now),
            "seed {seed}: all_refused"
        );
        assert_eq!(refusals::active(&r.s, n, now), spec::active(&r.s, n, now));
        if !node.is_open() && node.merged_into().is_none() {
            let flags = r.s.ix.node[&n].flags;
            assert!(
                flags.intersection(CLOSED_TAKE) == flags,
                "seed {seed}: closed node {n:?} keeps {flags:?}"
            );
        }
    }
}

/// Whether `m` is a node `leave_better` may suggest (D.8.7, A-100), naively over the base state: it
/// carries a flag, nobody holds it, it is a `next` candidate of its own (a node or a status item),
/// and no pause or open dependency holds it back.
fn suggestible(s: &State, m: NodeId, now: Time) -> bool {
    let Some(node) = s.nodes.get(&m) else {
        return false;
    };
    !spec::flags(s, m, now).is_empty()
        && spec::live_holds(s, m, now) == 0
        && matches!(
            spec::classify(s, m, now),
            Some(Cand::Node(_) | Cand::Status(_))
        )
        && !node
            .depends_on
            .iter()
            .any(|d| s.nodes.get(d).is_some_and(Node::is_open))
        && !spec::paused(s, m, now)
        // 1.11 (§10.4, §12): nor a series whose open occurrence waits for its slot.
        && !(node.is_open()
            && matches!(node.role, forum_core::Role::Series(_))
            && s.nodes.values().any(|x| {
                x.is_open()
                    && matches!(&x.role, forum_core::Role::Occurrence(o)
                        if o.series == m && o.phase == forum_core::OccPhase::Waiting)
            }))
}

/// 1.12 (§10.4), naively: what session `x` put down, the review of a closed node whose first open
/// objection by id `x` raised, and the node of the hold `x` last released as `handoff`, while no
/// hold of it was issued since and `last_now` is before the end of its keeping.
fn put_down(s: &State, x: SessionId, m: NodeId) -> bool {
    let node = &s.nodes[&m];
    let own = !node.is_open()
        && s.objections
            .iter()
            .filter(|(_, o)| o.node == m && o.state == ObjectionState::Open)
            .map(|(o, _)| *o)
            .min()
            .and_then(|o| s.entries.get(&o))
            .is_some_and(|e| e.by == forum_core::Author::Session(x));
    let handed = s.sessions[&x]
        .handed_off
        .is_some_and(|h| h.token.node == m && s.last_now < h.until && node.fence == h.token.fence);
    own || handed
}

/// `leave_better` written from D.8.7 over the base state, the oracle of item 8: anchored at `n`,
/// level by level up to ROOT, the ancestor first at its level and then its siblings by id, never
/// `n`, nor (1.11) a closed node with an open objection that the caller's class excludes at
/// `last_now`, a review it deferred; without an anchor, by id, the suggestible nodes the caller
/// could take (its caps cover the needs, its horizon the estimate, and its class does not exclude
/// them at `last_now`). 1.12: either way, nothing the caller put down (`put_down`).
fn naive_leave_better(
    s: &State,
    ok: &BTreeSet<NodeId>,
    anchor: Option<NodeId>,
    caller: Option<SessionId>,
) -> Vec<NodeId> {
    let max = s.params.leave_better_max as usize;
    let mut out = Vec::new();
    match anchor {
        Some(n) => {
            let class = caller.and_then(|c| s.sessions.get(&c)).map(|x| &x.class);
            let kept_off = |m: NodeId| {
                class.is_some_and(|c| {
                    !s.nodes[&m].is_open()
                        && s.objections
                            .values()
                            .any(|o| o.node == m && o.state == ObjectionState::Open)
                        && spec::excluded(s, m, c, s.last_now)
                }) || caller.is_some_and(|x| put_down(s, x, m))
            };
            let mut cur = n;
            while let Some(p) = s.nodes.get(&cur).and_then(Node::parent) {
                if out.len() >= max {
                    break;
                }
                let level: Vec<NodeId> = ok
                    .iter()
                    .copied()
                    .filter(|m| s.nodes[m].parent() == Some(p) && !kept_off(*m))
                    .collect();
                if cur != n && level.contains(&cur) {
                    out.push(cur);
                }
                out.extend(level.into_iter().filter(|&m| m != cur && m != n));
                cur = p;
            }
        }
        None => {
            let Some(x) = caller.and_then(|c| s.sessions.get(&c)) else {
                return out;
            };
            for &m in ok {
                let node = &s.nodes[&m];
                let fits = node.needs.is_subset(&x.caps)
                    && !matches!((node.est, x.horizon), (Some(e), Some(h)) if e > h);
                if fits
                    && !spec::excluded(s, m, &x.class, s.last_now)
                    && !caller.is_some_and(|c| put_down(s, c, m))
                {
                    out.push(m);
                }
            }
        }
    }
    out.truncate(max);
    out
}

/// Item 8 for `leave_better`, in both forms: anchored at every node, and empty for every session.
fn leave_better_agrees(seed: u64, r: &Run) {
    let ok: BTreeSet<NodeId> =
        r.s.nodes
            .keys()
            .copied()
            .filter(|&m| suggestible(&r.s, m, r.clock))
            .collect();
    for &n in r.s.nodes.keys() {
        assert_eq!(
            query::leave_better(&r.s, Some(n), None),
            naive_leave_better(&r.s, &ok, Some(n), None),
            "seed {seed}: leave_better anchored at {n:?}"
        );
        // 1.11: anchored for a caller, whose class's deferred reviews it leaves out.
        for &x in r.s.sessions.keys() {
            assert_eq!(
                query::leave_better(&r.s, Some(n), Some(x)),
                naive_leave_better(&r.s, &ok, Some(n), Some(x)),
                "seed {seed}: leave_better anchored at {n:?} for {x:?}"
            );
        }
    }
    for &x in r.s.sessions.keys() {
        assert_eq!(
            query::leave_better(&r.s, None, Some(x)),
            naive_leave_better(&r.s, &ok, None, Some(x)),
            "seed {seed}: leave_better for {x:?}"
        );
    }
}

/// Item 12: bounded state.
fn bounded(seed: u64, r: &Run) {
    let last = r.s.last_now;
    for x in r.s.sessions.values() {
        assert!(
            x.retire_at > r.clock,
            "seed {seed}: a session past its retirement"
        );
    }
    for o in r.s.ops.values() {
        assert!(o.expires_at > last, "seed {seed}: an expired op record");
    }
    for n in r.s.notes.values() {
        assert!(n.until > last, "seed {seed}: an expired note");
    }
}

/// The request digest of a command, as the shell computes one from the call's arguments (D.11.5):
/// the same command gives the same digest, so a reused `op_id` is a replay when the command repeats
/// and a conflict when it differs (O1). FNV-1a over the command's debug form, spread to 32 bytes.
fn digest_of(cmd: &Cmd) -> ReqDigest {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{cmd:?}").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let mut d = [0_u8; 32];
    for (i, x) in d.iter_mut().enumerate() {
        *x = (h >> ((i % 8) * 8)) as u8;
    }
    ReqDigest(d)
}

/// A node of the state, or an unknown one.
fn node_of(rng: &mut Rng, r: &Run) -> NodeId {
    if rng.chance(5) {
        return NodeId(9_999);
    }
    let ids: Vec<NodeId> = r.s.nodes.keys().copied().collect();
    rng.pick(&ids).unwrap_or(NodeId(0))
}

/// A node to take: mostly an open node an agent made, with a slot, that the session does not
/// hold; sometimes any node, or an unknown one.
fn take_target(rng: &mut Rng, r: &Run, sess: SessionId) -> NodeId {
    let now = r.clock;
    let free: Vec<NodeId> =
        r.s.nodes
            .iter()
            .filter(|(n, x)| {
                x.is_open()
                    && matches!(x.origin, Origin::Agent(_) | Origin::Ancestor(_))
                    && (r
                        .s
                        .holds
                        .iter()
                        .filter(|(t, h)| t.node == **n && now < h.lease_until)
                        .count() as u32)
                        < x.capacity
                    && !r
                        .s
                        .holds
                        .iter()
                        .any(|(t, h)| t.node == **n && h.session == sess)
            })
            .map(|(n, _)| *n)
            .collect();
    if rng.chance(80)
        && let Some(n) = rng.pick(&free)
    {
        return n;
    }
    node_of(rng, r)
}

/// A token: one of a session's, one ended, or one made up.
fn token_of(rng: &mut Rng, r: &Run, sess: SessionId) -> Token {
    let mine: Vec<Token> =
        r.s.holds
            .iter()
            .filter(|(_, h)| h.session == sess)
            .map(|(t, _)| *t)
            .collect();
    if rng.chance(85)
        && let Some(t) = rng.pick(&mine)
    {
        return t;
    }
    let n = node_of(rng, r);
    Token {
        node: n,
        fence: Fence(rng.below(3)),
    }
}

fn path_of(rng: &mut Rng) -> Path {
    let depth = 1 + rng.below(3) as usize;
    let mut v = Vec::new();
    if rng.chance(10) {
        v.push(Name::new(if rng.chance(50) { "@people" } else { "@teams" }).unwrap());
    }
    for _ in 0..depth {
        v.push(Name::new(NAMES[rng.below(3) as usize]).unwrap());
    }
    Path(v)
}

fn when_of(rng: &mut Rng) -> When {
    When::In(Dur([0_u64, 1_000, 60_000, 3_600_000][rng.below(4) as usize]))
}

/// One generated command for session `sess`.
fn command(rng: &mut Rng, r: &Run, sess: SessionId) -> Cmd {
    let node = |rng: &mut Rng| NodeRef::Id(node_of(rng, r));
    let mut pick = rng.below(20);
    // A session that holds nothing mostly takes something rather than write with a stale token,
    // so that hold writes mostly carry a live one (D.12.3: "commands mostly succeed").
    if (12..=16).contains(&pick) && !r.s.holds.values().any(|h| h.session == sess) && rng.chance(70)
    {
        pick = 8;
    }
    match pick {
        0 => Cmd::Session(SessionCmd::Hello {
            intent: rng.chance(30).then(|| text_of(rng)),
            caps: rng.chance(40).then(|| labels(rng)),
            horizon: rng
                .chance(30)
                .then(|| [20_u32, 64, 200][rng.below(3) as usize]),
            principal: None,
            from_user: None,
        }),
        1..=3 => Cmd::Node(NodeCmd::Create(CreateArgs {
            path: path_of(rng),
            keys: Vec::new(),
            kind: None,
            title: nonempty(rng),
            body: rng.chance(30).then(|| text_of(rng)),
            digest: rng.chance(30).then(|| text_of(rng)),
            fields: BTreeMap::new(),
            tags: if rng.chance(50) {
                [label("offer")].into()
            } else {
                BTreeSet::new()
            },
            capacity: rng.chance(40).then(|| rng.below(3) as u32),
            needs: rng.chance(40).then(|| labels(rng)),
            est: rng
                .chance(40)
                .then(|| [10_u32, 64, 100, 300][rng.below(4) as usize]),
            priority: rng.chance(30).then(|| rng.below(3) as i32 - 1),
            due: rng.chance(20).then(|| when_of(rng)),
            depends_on: if rng.chance(15) {
                vec![node(rng)]
            } else {
                Vec::new()
            },
            owners: Vec::new(),
            repeat: None,
            agreements: rng.chance(10).then(|| text_of(rng)),
            request: None,
            take: rng.chance(70).then(|| rng.chance(70)),
        })),
        4 => {
            let n = node_of(rng, r);
            let v = r.s.nodes.get(&n).map_or(Version(1), |x| x.version);
            let dv = r.s.nodes.get(&n).map(|x| x.digest_version);
            Cmd::Node(NodeCmd::Update {
                at: NodeRef::Id(n),
                version: if rng.chance(85) { v } else { Version(v.0 + 1) },
                digest_version: if rng.chance(70) { dv } else { None },
                token: rng.chance(20).then(|| token_of(rng, r, sess)),
                patch: Patch {
                    title: rng.chance(30).then(|| nonempty(rng)),
                    status: rng.chance(30).then(|| text_of(rng)),
                    digest: rng.chance(20).then(|| text_of(rng)),
                    tags_add: if rng.chance(30) {
                        [label("offer")].into()
                    } else {
                        BTreeSet::new()
                    },
                    tags_remove: if rng.chance(20) {
                        [label("offer")].into()
                    } else {
                        BTreeSet::new()
                    },
                    needs_add: labels(rng),
                    needs_remove: labels(rng),
                    capacity: rng.chance(20).then(|| rng.below(3) as u32),
                    priority: rng.chance(20).then(|| rng.below(3) as i32),
                    est: if rng.chance(20) {
                        Change::Set(64)
                    } else {
                        Change::Keep
                    },
                    depends_on: rng.chance(10).then(|| vec![node(rng)]),
                    ..Patch::default()
                },
            })
        }
        5 | 6 => Cmd::Node(NodeCmd::Close {
            // A close may leave another session's open intent behind, which flags the closed
            // node: a take or `next` of it then reopens it (D.8.20, WP-D4).
            at: NodeRef::Id(node_of(rng, r)),
            outcome: [
                Outcome::Done,
                Outcome::Failed,
                Outcome::Dropped,
                Outcome::Missed,
            ][rng.below(4) as usize]
                .clone(),
            summary: nonempty(rng),
            verified: rng.chance(30).then(|| text_of(rng)),
            settles: Some(BTreeSet::new()),
            learnings: None,
            token: rng.chance(40).then(|| token_of(rng, r, sess)),
            unsent: BTreeSet::new(),
        }),
        7 => Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: Eid(rng.below(r.s.counters.next_eid.0 + 2)),
                node: node_of(rng, r),
            },
            reason: nonempty(rng),
        }),
        8 | 9 => Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(take_target(rng, r, sess)),
            lease: rng.chance(30).then(|| Dur(rng.below(3) * 600_000)),
        }),
        10 | 11 => Cmd::Hold(HoldCmd::Next {
            query: if rng.chance(70) {
                NextQuery::default()
            } else {
                NextQuery {
                    under: Some(node(rng)),
                    kind: None,
                    tags: BTreeSet::new(),
                }
            },
            lease: None,
        }),
        12 | 13 => {
            let open =
                r.s.holds
                    .keys()
                    .find(|t| t.node == token_of(&mut rng.clone(), r, sess).node)
                    .and_then(|t| r.s.nodes.get(&t.node))
                    .and_then(|x| x.open_intent)
                    .is_some();
            Cmd::Hold(HoldCmd::Checkpoint {
                token: token_of(rng, r, sess),
                done: hold_text(rng),
                next_step: nonempty(rng),
                intent: rng.chance(30).then(|| nonempty(rng)),
                intent_done: (open || rng.chance(10)).then(|| nonempty(rng)),
                files: Vec::new(),
            })
        }
        14 => Cmd::Hold(HoldCmd::Renew {
            token: token_of(rng, r, sess),
            why: hold_text(rng),
        }),
        15 | 16 => Cmd::Hold(HoldCmd::Release {
            token: token_of(rng, r, sess),
            done: hold_text(rng),
            next_step: nonempty(rng),
            reason: [
                ReleaseReason::Handoff,
                ReleaseReason::Blocked,
                ReleaseReason::Cannot,
            ][rng.below(3) as usize],
            waits_for: match rng.below(6) {
                0 => Some(WaitArg::Time(when_of(rng))),
                1 => Some(WaitArg::External {
                    what: text_of(rng),
                    recheck_after: when_of(rng),
                }),
                2 => Some(WaitArg::Node(node(rng))),
                _ => None,
            },
            intent_done: rng.chance(25).then(|| text_of(rng)),
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
        17 => Cmd::Hold(HoldCmd::Check {
            target: node(rng),
            token: rng.chance(50).then(|| token_of(rng, r, sess)),
            // 1.8 (§10.2): sometimes what became of the node's open intent.
            intent_done: rng.chance(30).then(|| nonempty(rng)),
        }),
        18 => {
            if rng.chance(60) {
                Cmd::Control(ControlCmd::Pause {
                    scope: node(rng),
                    reason: nonempty(rng),
                })
            } else {
                Cmd::Control(ControlCmd::Unpause { scope: node(rng) })
            }
        }
        _ => Cmd::View(ViewCmd::Node {
            targets: vec![node(rng)],
            view: None,
            page: None,
        }),
    }
}

// ------------------------------------------------------------------------------------------------
// The keys and restructuring universe (WP-D4)

/// Which mix of commands a case draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Profile {
    /// WP-D3's holds universe.
    Holds,
    /// WP-D4's keys and restructuring universe, with the holds commands beside.
    Keys,
    /// WP-D5b's research and people universe (D.12.3's research profile, with the people and
    /// agent commands): decisions with basis references, supersessions, retractions, closes as
    /// `failed`, objections, and moves in trees four deep (CS-22, CS-32, D-4); requests, `said`,
    /// `heard`, questions to people, notes and agent questions to every kind of addressee, the
    /// sender's own `holder:` and `holders:` and retired sessions among them (1.4, R12), `say
    /// {expects}` (R17), a basis naming `at` or a node merged into it (R14), with the keys and
    /// holds commands beside; and the user side of `research_user_command` (WP-D7's review), with
    /// sessions that serve principals, and questions that reach their last recheck.
    Research,
    /// D.12.3's guided profile (1.3, WP-D7): the research universe, in which a third of the
    /// sessions follow their first next call whenever it is `now`, with G1's standard fill, at
    /// once (item 14: the call removes the condition that produced it), and otherwise do
    /// generated work; a guided session whose response has no next calls owes nothing a standing
    /// rule tracks (item 15).
    Guided,
    /// WP-D6's full universe (D.12.3): every command, the research universe's and, beside it,
    /// the user's (`from_user`, `to_user`, `route`, `ask` to a human, `hello {from_user,
    /// principal}`) and recurring work (series with a 10-minute cron and daily ones, `update
    /// {repeat}`, takes, closes, and merges of occurrences, edits of a series while its occurrence
    /// waits); `refusal.quorum` 1 or 2, so E1's questions for the user and their defaults,
    /// lapses, and drops are reached; advances that pass FINISHED_RETAIN and DIGEST_RETAIN, among
    /// them some that stop just past an entity's leaving time with no event since, after which a
    /// call from a new session names it; and the shell's restore step before every command.
    Full,
    /// The full universe with one class (D.12.3's one-class profile): every session is of class
    /// `a`, so a `cannot` of one class refuses a node for every active class, and E1's question
    /// for the user follows at a quorum of 1 or 2.
    OneClass,
    /// The full universe with no user (D.12.3's no-user profile, CS-2, CS-6): no `from_user`,
    /// `to_user`, `route`, `ask` to a human, or `hello` with a principal or a message, so the
    /// questions for the user that escalations and refusals raise lapse or default unanswered.
    NoUser,
    /// The full universe with churn (D.12.3, CS-8, CS-17): calls that open a session and are its
    /// only one or two, long advances (a day and more, past `session.retain`), so that sessions
    /// retire and records and notes expire, and `op_id`s from a pool of 6, reused across
    /// sessions.
    Churn,
    /// The full universe with refusals (D.12.3's refusal-heavy profile, CS-7): open and queried
    /// `next`, `cannot` releases, changes of needs and caps, and offered nodes with needs, some
    /// carrying requests, under parents with user requests, so that refusal chains and drops reach
    /// `chain` and `audience` with and without requests.
    Refusal,
}

impl Profile {
    /// The full universe and its parameterisations (WP-D6): every command, the restore step, and
    /// the user's checks.
    fn full(self) -> bool {
        matches!(
            self,
            Profile::Full | Profile::OneClass | Profile::NoUser | Profile::Churn | Profile::Refusal
        )
    }

    /// The classes whose sessions the case opens.
    fn classes(self) -> &'static [&'static str] {
        if self == Profile::OneClass {
            &CLASSES[..1]
        } else {
            &CLASSES
        }
    }
}

const KEYS: [&str; 3] = ["chat:x/1", "chat:x/2", "issue:p#1"];
const MESSAGES: [&str; 3] = ["chat:x/1/1", "chat:x/1/2", "issue:p#1/1"];
/// An agent-supplied `repeat:` key: refused wherever it appears (A-90, item 9).
const REPEAT: &str = "repeat:9:20270101T0700Z";

fn key_str(k: &str) -> Key {
    Key {
        key: KeyStr::new(k).unwrap(),
        nv: NormVer(1),
    }
}

/// A key of the pool, or 4 % of the time the `repeat:` key.
fn key_of(rng: &mut Rng) -> Key {
    if rng.chance(4) {
        key_str(REPEAT)
    } else {
        key_str(KEYS[rng.below(3) as usize])
    }
}

/// Zero to three keys, duplicates allowed (the core deduplicates, D.8.9).
fn keys_of(rng: &mut Rng) -> Vec<Key> {
    (0..rng.below(4)).map(|_| key_of(rng)).collect()
}

/// A request of a create: an origin among the messages and the keys, or none (A-61), and a
/// `where` or none.
fn request_of(rng: &mut Rng) -> Option<RequestArg> {
    if !rng.chance(40) {
        return None;
    }
    let origin = match rng.below(4) {
        0 => None,
        1 => Some(key_of(rng)),
        _ => Some(key_str(MESSAGES[rng.below(3) as usize])),
    };
    let where_ = rng.chance(50).then(|| key_of(rng));
    Some(RequestArg {
        text: nonempty(rng),
        origin,
        where_,
        requester: Text(String::from("bob")),
    })
}

/// A node that is open, or closed, as `open` asks, most of the time; sometimes any node.
fn node_in(rng: &mut Rng, r: &Run, open: bool) -> NodeId {
    let pool: Vec<NodeId> =
        r.s.nodes
            .iter()
            .filter(|(n, x)| x.is_open() == open && !n.is_reserved())
            .map(|(n, _)| *n)
            .collect();
    if rng.chance(85)
        && let Some(n) = rng.pick(&pool)
    {
        return n;
    }
    node_of(rng, r)
}

/// A path for a move: mostly one of the universe's, sometimes into `@people` or `@teams`.
fn move_path(rng: &mut Rng) -> Path {
    let mut p = path_of(rng);
    if rng.chance(15) {
        p.0.insert(
            0,
            Name::new(if rng.chance(50) { "@people" } else { "@teams" }).unwrap(),
        );
        p.0.truncate(2);
    }
    p
}

/// One generated command of the keys universe for session `sess`.
fn keys_command(rng: &mut Rng, r: &Run, sess: SessionId) -> Cmd {
    let node = |rng: &mut Rng| NodeRef::Id(node_of(rng, r));
    match rng.below(24) {
        0..=3 => Cmd::Node(NodeCmd::Create(CreateArgs {
            path: path_of(rng),
            keys: keys_of(rng),
            kind: None,
            title: nonempty(rng),
            body: None,
            digest: None,
            fields: BTreeMap::new(),
            tags: if rng.chance(30) {
                [label("offer")].into()
            } else {
                BTreeSet::new()
            },
            capacity: rng.chance(20).then(|| rng.below(3) as u32),
            needs: None,
            est: None,
            priority: None,
            due: None,
            depends_on: if rng.chance(15) {
                vec![node(rng)]
            } else {
                Vec::new()
            },
            owners: Vec::new(),
            repeat: None,
            agreements: None,
            request: request_of(rng),
            take: rng.chance(70).then(|| rng.chance(50)),
        })),
        4 | 5 => {
            let open = rng.chance(80);
            Cmd::Node(NodeCmd::Key {
                at: NodeRef::Id(node_in(rng, r, open)),
                change: KeyChange::Add(key_of(rng)),
            })
        }
        6 => {
            let n = node_of(rng, r);
            let carried: Vec<KeyStr> =
                r.s.nodes
                    .get(&n)
                    .map(|x| x.keys.keys().cloned().collect())
                    .unwrap_or_default();
            let k = if rng.chance(80)
                && let Some(k) = rng.pick(&carried)
            {
                k
            } else {
                key_of(rng).key
            };
            Cmd::Node(NodeCmd::Key {
                at: NodeRef::Id(n),
                change: KeyChange::Remove(k),
            })
        }
        7 | 8 => Cmd::Node(NodeCmd::Link {
            at: NodeRef::Id(node_in(rng, r, true)),
            rel: [
                LinkRel::Continues,
                LinkRel::DerivedFrom,
                LinkRel::ConflictsWith,
                LinkRel::Duplicate,
                LinkRel::SupersededBy,
            ][rng.below(5) as usize],
            to: NodeRef::Id(node_in(rng, r, true)),
        }),
        9 | 10 => Cmd::Node(NodeCmd::Reopen {
            at: NodeRef::Id(node_in(rng, r, false)),
            reason: nonempty(rng),
        }),
        11 | 12 => {
            let open = rng.chance(80);
            Cmd::Node(NodeCmd::Move {
                at: NodeRef::Id(node_in(rng, r, open)),
                to: move_path(rng),
            })
        }
        13 | 14 => {
            // A third of the merges aim at a cycle (N5, T5): `into` depends on or waits for a
            // node `w` that depends on or waits for `from`, so that the redirected edge closes
            // `into → w → into`.
            let chains = cycle_merges(r);
            let (from, into) = match rng.pick(&chains) {
                Some(pair) if rng.chance(33) => pair,
                _ => (node_in(rng, r, true), node_in(rng, r, true)),
            };
            Cmd::Node(NodeCmd::Merge {
                from: NodeRef::Id(from),
                into: NodeRef::Id(into),
            })
        }
        15 => Cmd::Node(NodeCmd::Close {
            at: NodeRef::Id(node_in(rng, r, true)),
            outcome: [Outcome::Done, Outcome::Failed, Outcome::Dropped][rng.below(3) as usize]
                .clone(),
            summary: nonempty(rng),
            verified: None,
            settles: Some(BTreeSet::new()),
            learnings: None,
            token: None,
            unsent: BTreeSet::new(),
        }),
        // Dependencies between open nodes, at the node's current version, so that chains of
        // edges exist for merges to redirect, and to refuse with `cycle`.
        16 => {
            let n = node_in(rng, r, true);
            let version = r.s.nodes.get(&n).map_or(Version(1), |x| x.version);
            Cmd::Node(NodeCmd::Update {
                at: NodeRef::Id(n),
                version,
                digest_version: None,
                token: None,
                patch: Patch {
                    depends_on: Some(vec![NodeRef::Id(node_in(rng, r, true))]),
                    ..Patch::default()
                },
            })
        }
        // The holds commands, so that holds exist to be ended by moves and merges, and closed
        // flagged nodes are taken (and reopened).
        _ => command(rng, r, sess),
    }
}

/// The pairs `(from, into)` of open nodes whose merge closes a cycle: `into` has an edge (a
/// dependency or a `{node}` wait) to some `w`, and `w` one to `from`.
fn cycle_merges(r: &Run) -> Vec<(NodeId, NodeId)> {
    let edges = |x: &forum_core::state::Node| -> Vec<NodeId> {
        let mut out: Vec<NodeId> = x.depends_on.iter().copied().collect();
        if let Some(WaitOn::Node(t)) = x.wait.as_ref().map(|w| w.on) {
            out.push(t);
        }
        out
    };
    let mut pairs = Vec::new();
    for (&into, i) in &r.s.nodes {
        if !i.is_open() || into.is_reserved() {
            continue;
        }
        for w in edges(i) {
            let Some(wn) = r.s.nodes.get(&w) else {
                continue;
            };
            for from in edges(wn) {
                if from != into
                    && from != w
                    && !from.is_reserved()
                    && r.s.nodes.get(&from).is_some_and(|x| x.is_open())
                {
                    pairs.push((from, into));
                }
            }
        }
    }
    pairs
}

// ------------------------------------------------------------------------------------------------
// The research and people universe (WP-D5b)

/// The node the shell adds to an entry it cannot find (D.11.5, D.14.6).
const UNKNOWN_NODE: NodeId = NodeId(u64::MAX);

/// A path up to four steps deep, for the research profile's trees.
fn deep_path(rng: &mut Rng) -> Path {
    let depth = 1 + rng.below(4) as usize;
    Path(
        (0..depth)
            .map(|_| Name::new(NAMES[rng.below(3) as usize]).unwrap())
            .collect(),
    )
}

/// An id of `ids`, mostly; sometimes one at or past `next_eid` (`not_found`).
fn eid_of(rng: &mut Rng, r: &Run, ids: &[Eid]) -> Eid {
    if rng.chance(90)
        && let Some(e) = rng.pick(ids)
    {
        return e;
    }
    Eid(r.s.counters.next_eid.0 + rng.below(3))
}

/// A conversation key among the pool, or none (the default of §9.1), or the `repeat:` key.
fn where_of(rng: &mut Rng) -> Option<Key> {
    rng.chance(60).then(|| key_of(rng))
}

/// A basis reference (§15): a node at its version, ahead of it, or without one, `at` itself and
/// merged nodes among them (R14); an entry, mostly a decision, the one superseded included (R33);
/// or something outside, checked now, earlier, or in the future (`invalid`, R27b).
fn basis_of(rng: &mut Rng, r: &Run, at: NodeId, supersedes: Option<Eid>) -> BasisArg {
    match rng.below(6) {
        0 | 1 => {
            let n = if rng.chance(20) { at } else { node_of(rng, r) };
            let v = r.s.nodes.get(&n).map_or(1, |x| x.version.0);
            BasisArg::Node {
                at: NodeRef::Id(n),
                version: match rng.below(4) {
                    0 => None,
                    1 => Some(Version(v + 1)),
                    _ => Some(Version(1 + rng.below(v))),
                },
            }
        }
        2 => {
            let merged: Vec<NodeId> =
                r.s.nodes
                    .iter()
                    .filter(|(_, x)| x.merged_into().is_some())
                    .map(|(n, _)| *n)
                    .collect();
            let n = rng.pick(&merged).unwrap_or_else(|| node_of(rng, r));
            BasisArg::Node {
                at: NodeRef::Id(n),
                version: None,
            }
        }
        3 | 4 => {
            let ds: Vec<Eid> = r.s.decisions.keys().copied().collect();
            match supersedes {
                Some(d) if rng.chance(30) => BasisArg::Entry(d),
                _ => BasisArg::Entry(eid_of(rng, r, &ds)),
            }
        }
        _ => BasisArg::External {
            what: nonempty(rng),
            checked_at: match rng.below(3) {
                0 => When::In(Dur(0)),
                1 => When::At(Time(r.clock.0.saturating_sub(60_000))),
                _ => When::In(Dur(60_000)),
            },
            recheck: rng.chance(30).then(|| when_of(rng)),
        },
    }
}

/// An addressee of `say` (§13): a session (sometimes a retired or unknown one, R12), the holder or
/// the holders of a node (often one the sender holds, R12), `user`, or a principal.
fn addressee_of(rng: &mut Rng, r: &Run, sess: SessionId) -> AddresseeArg {
    let held: Vec<NodeId> =
        r.s.holds
            .iter()
            .filter(|(_, h)| h.session == sess)
            .map(|(t, _)| t.node)
            .collect();
    let target = |rng: &mut Rng| {
        if rng.chance(50)
            && let Some(n) = rng.pick(&held)
        {
            NodeRef::Id(n)
        } else {
            NodeRef::Id(node_of(rng, r))
        }
    };
    match rng.below(6) {
        0 | 1 => {
            let all: Vec<SessionId> = r.sessions.iter().map(|(_, x)| *x).collect();
            AddresseeArg::Session(rng.pick(&all).unwrap_or(SessionId(77)))
        }
        2 => AddresseeArg::Holder(target(rng)),
        3 => AddresseeArg::Holders(target(rng)),
        4 => AddresseeArg::User,
        _ => AddresseeArg::Principal(
            PrincipalKey::new(["test:p1", "test:p2"][rng.below(2) as usize]).unwrap(),
        ),
    }
}

/// One generated command of the research universe for session `sess`; with `user`, the user side
/// of `research_user_command` too (left out in the full universe's no-user profile).
fn research_command(rng: &mut Rng, r: &Run, sess: SessionId, user: bool) -> Cmd {
    let requests: Vec<Eid> = r.s.requests.keys().copied().collect();
    let decisions: Vec<Eid> = r.s.decisions.keys().copied().collect();
    let saids: Vec<Eid> = r.s.saids.keys().copied().collect();
    let pqs: Vec<Eid> = r.s.pquestions.keys().copied().collect();
    let objections: Vec<Eid> = r.s.objections.keys().copied().collect();
    match rng.below(38) {
        0..=3 => {
            let at = node_of(rng, r);
            let supersedes = if rng.chance(30) {
                Some(eid_of(rng, r, &decisions))
            } else {
                None
            };
            let n = rng.below(3);
            let basis = (0..n).map(|_| basis_of(rng, r, at, supersedes)).collect();
            let settles = (0..rng.below(3))
                .map(|_| eid_of(rng, r, &requests))
                .collect();
            Cmd::Talk(TalkCmd::Decide {
                at: NodeRef::Id(at),
                text: nonempty(rng),
                settles,
                supersedes,
                basis,
                evidence: rng.chance(30).then_some(Evidence::Inference),
            })
        }
        4 => {
            let target = if rng.chance(70) {
                let e = eid_of(rng, r, &decisions);
                let node =
                    r.s.entries
                        .get(&e)
                        .map(|m| m.node)
                        .or_else(|| r.s.decisions.get(&e).map(|d| d.node))
                        .unwrap_or(UNKNOWN_NODE);
                RetractArg::Entry { eid: e, node }
            } else {
                RetractArg::Node(NodeRef::Id(node_of(rng, r)))
            };
            Cmd::Talk(TalkCmd::Retract {
                target,
                text: nonempty(rng),
                reason: nonempty(rng),
            })
        }
        5 | 6 => Cmd::Node(NodeCmd::Close {
            at: NodeRef::Id(node_in(rng, r, true)),
            outcome: [
                Outcome::Done,
                Outcome::Failed,
                Outcome::Failed,
                Outcome::Dropped,
            ][rng.below(4) as usize]
                .clone(),
            summary: nonempty(rng),
            verified: None,
            settles: Some(
                (0..rng.below(2))
                    .map(|_| eid_of(rng, r, &requests))
                    .collect(),
            ),
            learnings: None,
            token: None,
            unsent: BTreeSet::new(),
        }),
        7 => Cmd::Node(NodeCmd::Reopen {
            at: NodeRef::Id(node_in(rng, r, false)),
            reason: nonempty(rng),
        }),
        8 => Cmd::Talk(TalkCmd::Object {
            on: NodeRef::Id(node_of(rng, r)),
            note: nonempty(rng),
        }),
        9 => {
            // 1.8 (§11.3): an objection that stands, which corrects a closed node; 1.11: a
            // deferral, which backs the caller's class off a closed node's review.
            let upheld = rng.chance(40);
            Cmd::Talk(TalkCmd::Resolve {
                objection: eid_of(rng, r, &objections),
                note: nonempty(rng),
                upheld,
                defer: !upheld && rng.chance(30),
            })
        }
        10 => {
            let n = node_of(rng, r);
            let v = r.s.nodes.get(&n).map_or(1, |x| x.version.0);
            Cmd::Talk(TalkCmd::Approve {
                on: NodeRef::Id(n),
                version: Version(if rng.chance(10) {
                    v + 1
                } else {
                    1 + rng.below(v)
                }),
                checked: nonempty(rng),
            })
        }
        11 => Cmd::Node(NodeCmd::Move {
            at: NodeRef::Id(node_in(rng, r, true)),
            to: deep_path(rng),
        }),
        12 => Cmd::Node(NodeCmd::Merge {
            from: NodeRef::Id(node_in(rng, r, true)),
            into: NodeRef::Id(node_in(rng, r, true)),
        }),
        13 => Cmd::Talk(TalkCmd::Request {
            at: NodeRef::Id(node_of(rng, r)),
            text: nonempty(rng),
            origin: if rng.chance(70) {
                key_str(MESSAGES[rng.below(3) as usize])
            } else {
                key_of(rng)
            },
            where_: where_of(rng),
            requester: Text(String::from("bob")),
        }),
        14..=16 => Cmd::Talk(TalkCmd::Said {
            at: NodeRef::Id(node_of(rng, r)),
            where_: where_of(rng),
            text: nonempty(rng),
            kind: [
                SaidKind::Ack,
                SaidKind::Reply,
                SaidKind::Result,
                SaidKind::Correction,
                SaidKind::Question,
            ][rng.below(5) as usize],
            corrects: rng.chance(10).then(|| eid_of(rng, r, &saids)),
            seen: rng.chance(20).then(|| Seq(rng.below(6))),
        }),
        17 => Cmd::Talk(TalkCmd::SaidConfirm {
            said: eid_of(rng, r, &saids),
            confirmed: nonempty(rng),
        }),
        18 => Cmd::Talk(TalkCmd::Heard {
            at: NodeRef::Id(node_of(rng, r)),
            where_: where_of(rng),
            from: nonempty(rng),
            text: nonempty(rng),
            answers: rng.chance(60).then(|| eid_of(rng, r, &pqs)),
        }),
        19 | 20 => Cmd::Talk(TalkCmd::Ask {
            at: NodeRef::Id(node_of(rng, r)),
            text: nonempty(rng),
            to: AskTo::Person {
                where_: where_of(rng),
                blocking: rng.chance(30),
            },
            again: rng.chance(25),
        }),
        21 | 22 => Cmd::Talk(TalkCmd::Say {
            at: NodeRef::Id(node_of(rng, r)),
            text: nonempty(rng),
            to: rng.chance(85).then(|| addressee_of(rng, r, sess)),
            expects: rng
                .chance(30)
                .then(|| [Expects::None, Expects::Ack, Expects::Reply][rng.below(3) as usize]),
            reply_to: rng
                .chance(10)
                .then(|| Eid(rng.below(r.s.counters.next_eid.0 + 2))),
            until: rng.chance(30).then(|| when_of(rng)),
        }),
        23 => {
            let to = match addressee_of(rng, r, sess) {
                AddresseeArg::Holder(n) => AgentAddressee::Holder(n),
                AddresseeArg::Holders(n) => AgentAddressee::Holders(n),
                AddresseeArg::Session(x) => AgentAddressee::Session(x),
                AddresseeArg::User | AddresseeArg::Principal(_) => AgentAddressee::Session(sess),
            };
            Cmd::Talk(TalkCmd::Ask {
                at: NodeRef::Id(node_of(rng, r)),
                text: nonempty(rng),
                to: AskTo::Agent(to),
                again: false,
            })
        }
        24 => {
            // The shell adds the question's node and asker from its `entries` table, or
            // `UNKNOWN_NODE` for an entry it lacks (D.11.5, D.14.6).
            let aqs: Vec<Eid> = r.s.aquestions.keys().copied().collect();
            let pool = if rng.chance(80) { &aqs } else { &pqs };
            let q = eid_of(rng, r, pool);
            let (node, asker) =
                r.s.aquestions
                    .get(&q)
                    .map(|x| (x.node, Some(x.asker)))
                    .or_else(|| r.s.entries.get(&q).map(|m| (m.node, None)))
                    .unwrap_or((UNKNOWN_NODE, None));
            Cmd::Talk(TalkCmd::Answer {
                question: q,
                node,
                asker,
                text: nonempty(rng),
            })
        }
        25 => Cmd::Talk(TalkCmd::Wait {
            timeout: Dur(0),
            for_: rng.chance(50).then(|| NodeRef::Id(node_of(rng, r))),
            since: rng.chance(30).then(|| Seq(rng.below(6))),
        }),
        26 => {
            if rng.chance(50) {
                Cmd::Talk(TalkCmd::Who {
                    at: rng.chance(50).then(|| NodeRef::Id(node_of(rng, r))),
                })
            } else {
                Cmd::Session(SessionCmd::Status {
                    text: nonempty(rng),
                })
            }
        }
        27 => Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: rng
                .chance(70)
                .then(|| PrincipalKey::new(["test:p1", "test:p2"][rng.below(2) as usize]).unwrap()),
            from_user: None,
        }),
        28..=30 => keys_command(rng, r, sess),
        31..=34 if user => research_user_command(rng, r),
        _ => command(rng, r, sess),
    }
}

/// The principals of the user side: `test:p1` and `test:p2`, which a person in `@people` may make
/// aliases of one another, and `test:p3`.
const PRINCIPALS: [&str; 3] = ["test:p1", "test:p2", "test:p3"];

fn any_principal(rng: &mut Rng) -> PrincipalKey {
    PrincipalKey::new(PRINCIPALS[rng.below(3) as usize]).unwrap()
}

/// One generated command of the user side (WP-D7's review, D.12.3's many-principals and
/// people-heavy templates in the research and guided universes): `from_user` with and without
/// `node`, answering a user question or clarifying a request; `to_user` of each kind, with `re`
/// naming user entries, requests, and digest entries; `route` to a node or to none; `ask` to the
/// user, with and without a default and a due; and a person in `@people` whose keys make `test:p1`
/// and `test:p2` aliases. Sessions with principals are opened in `case`.
fn research_user_command(rng: &mut Rng, r: &Run) -> Cmd {
    let entries: Vec<Eid> = r.s.user_entries.keys().copied().collect();
    let unrouted: Vec<Eid> =
        r.s.user_entries
            .iter()
            .filter(|(_, u)| u.routing == Routing::Unrouted)
            .map(|(e, _)| *e)
            .collect();
    let uqs: Vec<Eid> =
        r.s.uquestions
            .iter()
            .filter(|(_, q)| q.state == UqState::Pending)
            .map(|(e, _)| *e)
            .collect();
    let requests: Vec<Eid> = r.s.requests.keys().copied().collect();
    let digest: Vec<Eid> = r.s.digest.keys().copied().collect();
    match rng.below(10) {
        0..=2 => {
            let answers = (rng.chance(30) && !uqs.is_empty()).then(|| eid_of(rng, r, &uqs));
            Cmd::Talk(TalkCmd::FromUser {
                text: nonempty(rng),
                client: Text(String::from("chat")),
                principal: rng.chance(60).then(|| any_principal(rng)),
                answers,
                to: ((answers.is_none() && rng.chance(50)).then(|| NodeRef::Id(node_of(rng, r))))
                    .map(forum_core::RouteTo::Node),
                clarifies: (answers.is_none() && rng.chance(10)).then(|| eid_of(rng, r, &requests)),
                reason: None,
            })
        }
        3 | 4 => {
            let mut re = BTreeSet::new();
            for _ in 0..rng.below(4) {
                let pool = match rng.below(3) {
                    0 => &entries,
                    1 => &requests,
                    _ => &digest,
                };
                re.insert(eid_of(rng, r, pool));
            }
            Cmd::Talk(TalkCmd::ToUser {
                text: nonempty(rng),
                kind: [ToUserKind::Ack, ToUserKind::Question, ToUserKind::Result]
                    [rng.below(3) as usize],
                verbatim: rng.chance(50),
                principal: rng.chance(60).then(|| any_principal(rng)),
                re,
                at: None,
            })
        }
        5 => {
            let pool = if rng.chance(80) { &unrouted } else { &entries };
            Cmd::Talk(TalkCmd::Route {
                entry: eid_of(rng, r, pool),
                to: Some(
                    (rng.chance(60).then(|| NodeRef::Id(node_of(rng, r))))
                        .map_or(forum_core::RouteTo::Nowhere, forum_core::RouteTo::Node),
                ),
                answers: None,
                clarifies: None,
                reason: Some(nonempty(rng)),
            })
        }
        6 | 7 => Cmd::Talk(TalkCmd::Ask {
            at: NodeRef::Id(node_of(rng, r)),
            text: nonempty(rng),
            to: AskTo::User(UserAsk {
                to: rng.chance(70).then(|| any_principal(rng)),
                options: (0..rng.below(3)).map(|_| nonempty(rng)).collect(),
                recommendation: rng.chance(30).then(|| nonempty(rng)),
                default: rng.chance(50).then(|| nonempty(rng)),
                due: rng.chance(30).then(|| when_of(rng)),
                digest: ReqDigest([rng.below(256) as u8; 32]),
            }),
            again: rng.chance(10),
        }),
        8 => Cmd::Node(NodeCmd::Create(CreateArgs {
            path: Path(vec![
                Name::new("@people").unwrap(),
                Name::new(NAMES[rng.below(3) as usize]).unwrap(),
            ]),
            keys: (0..1 + rng.below(2))
                .map(|_| key_str(&format!("person:{}", PRINCIPALS[rng.below(2) as usize])))
                .collect(),
            kind: None,
            title: nonempty(rng),
            body: None,
            digest: None,
            fields: BTreeMap::new(),
            tags: BTreeSet::new(),
            capacity: None,
            needs: None,
            est: None,
            priority: None,
            due: None,
            depends_on: Vec::new(),
            owners: Vec::new(),
            repeat: None,
            agreements: None,
            request: None,
            take: Some(rng.chance(30)),
        })),
        _ => Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: rng.chance(80).then(|| any_principal(rng)),
            from_user: rng.chance(30).then(|| FromUserArg {
                text: nonempty(rng),
                client: Text(String::from("chat")),
                principal: None,
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
    }
}

/// `conversation_holder` written from D.8.26 over the base state, for every node and session at
/// once: None when the session holds the node or a node below it; else, over the holds of its
/// subtree in preorder with children by id, each node's by token, the first holder whose caps
/// include the cap of the conversation's scheme (1.11, §9.1), else None when the session has that
/// cap, else the first holder. One walk of the tree per node (a walk per node and session cost
/// O(sessions × nodes²) per step in the full universe).
struct NaiveHolders {
    /// For each session, the ancestors-or-self of the nodes it holds.
    covers: BTreeMap<SessionId, BTreeSet<NodeId>>,
    /// For each node, the sessions of the holds of its subtree, in preorder, by token.
    holders: BTreeMap<NodeId, Vec<SessionId>>,
}

impl NaiveHolders {
    fn holder(&self, s: &State, c: NodeId, x: SessionId, k: &KeyStr) -> Option<SessionId> {
        if self.covers.get(&x).is_some_and(|a| a.contains(&c)) {
            return None;
        }
        let cap = k.scheme();
        let capable = |y: SessionId| {
            s.sessions
                .get(&y)
                .is_some_and(|z| z.caps.iter().any(|l| l.as_str() == cap))
        };
        let hs = self.holders.get(&c)?;
        let first = *hs.first()?;
        if let Some(h) = hs.iter().copied().find(|h| capable(*h)) {
            return Some(h);
        }
        (!capable(x)).then_some(first)
    }
}

fn naive_holders(s: &State) -> NaiveHolders {
    let mut covers: BTreeMap<SessionId, BTreeSet<NodeId>> = BTreeMap::new();
    for (t, h) in &s.holds {
        let e = covers.entry(h.session).or_default();
        e.insert(t.node);
        e.extend(spec::ancestors(s, t.node));
    }
    let mut children: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for (&m, node) in &s.nodes {
        if let Some(p) = node.parent() {
            children.entry(p).or_default().push(m);
        }
    }
    let mut own: BTreeMap<NodeId, Vec<SessionId>> = BTreeMap::new();
    for (t, h) in &s.holds {
        own.entry(t.node).or_default().push(h.session);
    }
    let mut holders = BTreeMap::new();
    for &c in s.nodes.keys() {
        let mut found: Vec<SessionId> = Vec::new();
        let mut stack = vec![c];
        while let Some(n) = stack.pop() {
            found.extend(own.get(&n).into_iter().flatten().copied());
            if let Some(kids) = children.get(&n) {
                stack.extend(kids.iter().rev().copied());
            }
        }
        holders.insert(c, found);
    }
    NaiveHolders { covers, holders }
}

/// `inbox` written from §13 and D.8.16 over the notes: the live notes delivered to the session and
/// not read by it, its principals' notes under any alias, and the `holders:` notes of the branches
/// it holds in that it did not write and has not read past.
fn naive_inbox(s: &State, sess: SessionId, now: Time) -> Vec<Eid> {
    let Some(x) = s.sessions.get(&sess) else {
        return Vec::new();
    };
    let mut principals = BTreeSet::new();
    for &p in &x.principals {
        principals.extend(spec::aliases(s, p));
    }
    let held: Vec<NodeId> = s
        .holds
        .iter()
        .filter(|(_, h)| h.session == sess && now < h.lease_until)
        .map(|(t, _)| t.node)
        .collect();
    let mut out = Vec::new();
    for (&e, note) in &s.notes {
        if note.until <= now {
            continue;
        }
        let unread = !note.read_by.contains(&sess);
        let mine = match note.to {
            Addressee::Principal(p) => unread && principals.contains(&p),
            Addressee::Holders(a) => {
                held.iter()
                    .any(|&n| spec::ancestors_or_self(s, n).contains(&a))
                    && s.entries.get(&e).map(|m| m.by) != Some(Author::Session(sess))
                    && e > x.branch_seen.get(&a).copied().unwrap_or(Eid(0))
            }
            Addressee::Session(_) | Addressee::Holder(_) => {
                unread && note.recipients.contains(&sess)
            }
        };
        if mine {
            out.push(e);
        }
    }
    out
}

/// Item 8 for WP-D5b's helpers: `conversation_holder` and `inbox` equal their naive forms, and a
/// request for which §26's Settled holds is settled (the converse need not hold: settling is final,
/// so a retraction after it leaves the request settled, A-11).
fn people_agree(seed: u64, r: &Run) {
    let now = r.clock;
    let holders = naive_holders(&r.s);
    for &x in r.s.sessions.keys() {
        assert_eq!(
            query::inbox(&r.s, x, now),
            naive_inbox(&r.s, x, now),
            "seed {seed}: inbox of {x:?}"
        );
        // 1.11: a conversation of each scheme the caps of the universe name (`x`, `y`), and one
        // of a scheme no session's caps carry (`chat`).
        for k in ["chat:x/1", "x:a", "y:b"] {
            let k = KeyStr::new(k).unwrap();
            for &c in r.s.nodes.keys() {
                assert_eq!(
                    query::conversation_holder(&r.s, c, x, &k),
                    holders.holder(&r.s, c, x, &k),
                    "seed {seed}: conversation_holder of {c:?} for {x:?} in {k}"
                );
            }
        }
    }
    for (&q, req) in &r.s.requests {
        assert!(
            req.settled.is_some() || !spec::settled(&r.s, q),
            "seed {seed}: {q:?} is not settled"
        );
    }
}

// ------------------------------------------------------------------------------------------------
// The full universe (WP-D6)

/// The one 10-minute series of the universe (D.12.3), and the other schedules.
const TEN_MINUTES: &str = "*/10 * * * *";
const CRONS: [&str; 2] = ["0 7 * * *", "30 */6 * * *"];

fn repeat_arg(cron: &str, within: u64) -> RepeatArg {
    RepeatArg {
        cron_text: Text(String::from(cron)),
        cron: forum_cron::parse_cron(cron).unwrap(),
        tz: forum_cron::TzTable {
            name: String::from("UTC"),
            tzdb_version: String::new(),
            initial: 0,
            transitions: Vec::new(),
        },
        run_within: Dur(within),
    }
}

/// The open series whose schedule is `cron`.
fn open_series(r: &Run, cron: &str) -> Option<NodeId> {
    r.s.nodes.iter().find_map(|(n, x)| match &x.role {
        Role::Series(y) if x.is_open() && y.repeat.cron_text.0 == cron => Some(*n),
        _ => None,
    })
}

/// The series, open or closed, and the open occurrences.
fn series_and_occurrences(r: &Run) -> (Vec<NodeId>, Vec<NodeId>) {
    let mut series = Vec::new();
    let mut occ = Vec::new();
    for (&n, x) in &r.s.nodes {
        match &x.role {
            Role::Series(_) => series.push(n),
            Role::Occurrence(_) if x.is_open() => occ.push(n),
            _ => {}
        }
    }
    (series, occ)
}

fn principal_of(rng: &mut Rng) -> Option<PrincipalKey> {
    rng.chance(60)
        .then(|| PrincipalKey::new(["test:p1", "test:p2"][rng.below(2) as usize]).unwrap())
}

/// One recurring command (§12): a series, `update {repeat}` and edits of a series, takes,
/// closes, and merges of occurrences.
fn recurring_command(rng: &mut Rng, r: &Run) -> Cmd {
    let (series, occ) = series_and_occurrences(r);
    let pick_series = |rng: &mut Rng| rng.pick(&series).unwrap_or_else(|| node_of(rng, r));
    let pick_occ = |rng: &mut Rng| rng.pick(&occ).unwrap_or_else(|| node_of(rng, r));
    let update = |n: NodeId, patch: Patch| {
        let version = r.s.nodes.get(&n).map_or(Version(1), |x| x.version);
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(n),
            version,
            digest_version: None,
            token: None,
            patch,
        })
    };
    match rng.below(13) {
        // A move of an occurrence, mostly a held one whose window has passed: the move ends the
        // other sessions' holds, and the occurrence is then missed in the same step (A-80, R2).
        12 => {
            let held_late: Vec<NodeId> = occ
                .iter()
                .copied()
                .filter(|o| {
                    r.s.holds.keys().any(|t| t.node == *o)
                        && matches!(&r.s.nodes[o].role, Role::Occurrence(x) if x.window_fired)
                })
                .collect();
            let o = rng.pick(&held_late).unwrap_or_else(|| pick_occ(rng));
            Cmd::Node(NodeCmd::Move {
                at: NodeRef::Id(o),
                to: move_path(rng),
            })
        }
        0..=2 => {
            let slot_key = if rng.chance(10) {
                occ.iter().find_map(|o| match &r.s.nodes[o].role {
                    Role::Occurrence(x) => Some(Key {
                        key: KeyStr::of_occurrence(x.series, x.slot),
                        nv: NormVer(0),
                    }),
                    _ => None,
                })
            } else {
                None
            };
            let cron = if open_series(r, TEN_MINUTES).is_none()
                && r.s.nodes.len() < 60
                && rng.chance(50)
            {
                TEN_MINUTES
            } else {
                CRONS[rng.below(2) as usize]
            };
            Cmd::Node(NodeCmd::Create(CreateArgs {
                path: path_of(rng),
                // Item 9: sometimes the key of an open occurrence's slot, which only the server
                // makes (A-90): refused, and K1 holds.
                keys: slot_key.into_iter().collect(),
                kind: None,
                title: nonempty(rng),
                body: rng.chance(50).then(|| nonempty(rng)),
                digest: None,
                fields: BTreeMap::new(),
                tags: BTreeSet::new(),
                capacity: None,
                needs: rng.chance(30).then(|| labels(rng)),
                est: rng.chance(40).then(|| [10_u32, 64][rng.below(2) as usize]),
                priority: None,
                due: None,
                depends_on: Vec::new(),
                owners: Vec::new(),
                repeat: Some(repeat_arg(
                    cron,
                    [300_000_u64, 3_600_000][rng.below(2) as usize],
                )),
                agreements: None,
                request: None,
                take: rng.chance(50).then(|| rng.chance(50)),
            }))
        }
        3 => {
            let n = if rng.chance(70) {
                pick_series(rng)
            } else {
                node_of(rng, r)
            };
            let repeat = match rng.below(4) {
                0 => Change::Clear,
                1 => Change::Set(repeat_arg(TEN_MINUTES, 300_000)),
                _ => Change::Set(repeat_arg(
                    CRONS[rng.below(2) as usize],
                    [300_000_u64, 3_600_000][rng.below(2) as usize],
                )),
            };
            update(
                n,
                Patch {
                    repeat,
                    ..Patch::default()
                },
            )
        }
        // R10: edits of a series while its occurrence waits, and of the occurrence itself.
        4 | 5 => {
            let n = if rng.chance(70) {
                pick_series(rng)
            } else {
                pick_occ(rng)
            };
            update(
                n,
                Patch {
                    title: rng.chance(60).then(|| nonempty(rng)),
                    body: rng.chance(60).then(|| nonempty(rng)),
                    needs_add: labels(rng),
                    needs_remove: labels(rng),
                    est: match rng.below(3) {
                        0 => Change::Set(64),
                        1 => Change::Clear,
                        _ => Change::Keep,
                    },
                    ..Patch::default()
                },
            )
        }
        6 | 7 => Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(pick_occ(rng)),
            lease: None,
        }),
        8 => Cmd::Node(NodeCmd::Close {
            at: NodeRef::Id(pick_occ(rng)),
            outcome: [Outcome::Dropped, Outcome::Done][rng.below(2) as usize].clone(),
            summary: nonempty(rng),
            verified: None,
            settles: Some(BTreeSet::new()),
            learnings: None,
            token: None,
            unsent: BTreeSet::new(),
        }),
        9 => {
            // A series into its own occurrence (refused, 1.5), an occurrence into its series, or
            // an occurrence into another node.
            let s_ = pick_series(rng);
            let o =
                r.s.ix
                    .series_open
                    .get(&s_)
                    .and_then(|x| x.iter().next().copied())
                    .unwrap_or_else(|| pick_occ(rng));
            let (from, into) = match rng.below(3) {
                0 => (s_, o),
                1 => (o, s_),
                _ => (o, node_in(rng, r, true)),
            };
            Cmd::Node(NodeCmd::Merge {
                from: NodeRef::Id(from),
                into: NodeRef::Id(into),
            })
        }
        10 => Cmd::Node(NodeCmd::Close {
            at: NodeRef::Id(pick_series(rng)),
            outcome: Outcome::Done,
            summary: nonempty(rng),
            verified: None,
            settles: Some(BTreeSet::new()),
            learnings: None,
            token: None,
            unsent: BTreeSet::new(),
        }),
        _ => Cmd::Node(NodeCmd::Reopen {
            at: NodeRef::Id(pick_series(rng)),
            reason: nonempty(rng),
        }),
    }
}

/// One command of the user's side (§8): `from_user` (answering a question, routed to a node, or
/// clarifying a request), `to_user` of each kind naming requests, entries, and digest entries,
/// `route`, `ask` to a human, and `hello` with a principal or a message, which may answer a
/// question (1.6).
fn user_command(rng: &mut Rng, r: &Run) -> Cmd {
    let requests: Vec<Eid> = r.s.requests.keys().copied().collect();
    let uqs: Vec<Eid> = r.s.uquestions.keys().copied().collect();
    let entries: Vec<Eid> = r.s.user_entries.keys().copied().collect();
    let digest: Vec<Eid> = r.s.digest.keys().copied().collect();
    let gone: Vec<Eid> = r
        .rows
        .keys()
        .filter_map(|f| match f {
            Fact::Request(e) | Fact::UserEntry(e) | Fact::Digest(e) | Fact::UQuestion(e) => {
                Some(*e)
            }
            _ => None,
        })
        .collect();
    let any_of = |rng: &mut Rng, pool: &[Eid]| {
        if rng.chance(15)
            && let Some(e) = rng.pick(&gone)
        {
            e
        } else {
            eid_of(rng, r, pool)
        }
    };
    match rng.below(10) {
        0..=2 => Cmd::Talk(TalkCmd::FromUser {
            text: nonempty(rng),
            client: Text(String::from("cli")),
            principal: principal_of(rng),
            answers: rng.chance(30).then(|| any_of(rng, &uqs)),
            to: (rng.chance(30).then(|| NodeRef::Id(node_of(rng, r))))
                .map(forum_core::RouteTo::Node),
            clarifies: rng.chance(20).then(|| any_of(rng, &requests)),
            reason: None,
        }),
        3..=5 => {
            let mut pool = requests.clone();
            pool.extend(entries.iter().copied());
            pool.extend(digest.iter().copied());
            Cmd::Talk(TalkCmd::ToUser {
                text: nonempty(rng),
                kind: [ToUserKind::Ack, ToUserKind::Result, ToUserKind::Question]
                    [rng.below(3) as usize],
                verbatim: rng.chance(50),
                principal: principal_of(rng),
                re: (0..rng.below(3)).map(|_| any_of(rng, &pool)).collect(),
                // 1.8 (§8.1): a report about a node's work.
                at: rng.chance(30).then(|| NodeRef::Id(node_of(rng, r))),
            })
        }
        6 => Cmd::Talk(TalkCmd::Route {
            entry: any_of(rng, &entries),
            to: Some(
                (rng.chance(70).then(|| NodeRef::Id(node_of(rng, r))))
                    .map_or(forum_core::RouteTo::Nowhere, forum_core::RouteTo::Node),
            ),
            answers: None,
            clarifies: None,
            reason: Some(nonempty(rng)),
        }),
        7 | 8 => Cmd::Talk(TalkCmd::Ask {
            at: NodeRef::Id(node_of(rng, r)),
            text: nonempty(rng),
            to: AskTo::User(UserAsk {
                to: principal_of(rng),
                options: (0..rng.below(3)).map(|_| nonempty(rng)).collect(),
                recommendation: rng.chance(50).then(|| nonempty(rng)),
                default: rng.chance(50).then(|| nonempty(rng)),
                due: rng.chance(50).then(|| when_of(rng)),
                digest: ReqDigest([rng.below(4) as u8; 32]),
            }),
            again: rng.chance(10),
        }),
        _ => Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: principal_of(rng),
            from_user: rng.chance(60).then(|| FromUserArg {
                text: nonempty(rng),
                client: Text(String::from("ide")),
                principal: principal_of(rng),
                // 1.6 (§8.2): hello's words may answer a question for the human.
                answers: rng.chance(30).then(|| any_of(rng, &uqs)),
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
    }
}

/// One command of the refusal-heavy profile (D.12.3, CS-7): open and queried `next`, `cannot`
/// releases, takes, changes of needs, offered nodes with needs (some with an external request),
/// `hello` with caps, and the user's side, whose `from_user {node}` puts user requests on nodes and
/// their parents.
fn refusal_command(rng: &mut Rng, r: &Run, sess: SessionId) -> Cmd {
    match rng.below(10) {
        0..=2 => Cmd::Hold(HoldCmd::Next {
            query: if rng.chance(60) {
                NextQuery::default()
            } else {
                NextQuery {
                    under: Some(NodeRef::Id(node_of(rng, r))),
                    kind: None,
                    tags: BTreeSet::new(),
                }
            },
            lease: None,
        }),
        3 | 4 => Cmd::Hold(HoldCmd::Release {
            token: token_of(rng, r, sess),
            done: hold_text(rng),
            next_step: nonempty(rng),
            reason: if rng.chance(80) {
                ReleaseReason::Cannot
            } else {
                ReleaseReason::Handoff
            },
            waits_for: None,
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
        5 => Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(take_target(rng, r, sess)),
            lease: None,
        }),
        6 => {
            let n = node_of(rng, r);
            let version = r.s.nodes.get(&n).map_or(Version(1), |x| x.version);
            Cmd::Node(NodeCmd::Update {
                at: NodeRef::Id(n),
                version,
                digest_version: None,
                token: None,
                patch: Patch {
                    needs_add: labels(rng),
                    needs_remove: labels(rng),
                    ..Patch::default()
                },
            })
        }
        7 => Cmd::Node(NodeCmd::Create(CreateArgs {
            path: path_of(rng),
            keys: Vec::new(),
            kind: None,
            title: nonempty(rng),
            body: None,
            digest: None,
            fields: BTreeMap::new(),
            tags: [label("offer")].into(),
            capacity: None,
            needs: Some(labels(rng)),
            est: None,
            priority: None,
            due: None,
            depends_on: Vec::new(),
            owners: Vec::new(),
            repeat: None,
            agreements: None,
            request: if rng.chance(40) {
                request_of(rng)
            } else {
                None
            },
            take: Some(false),
        })),
        8 => Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some(labels(rng)),
            horizon: None,
            principal: None,
            from_user: None,
        }),
        _ => user_command(rng, r),
    }
}

/// One command of the full universe for session `sess`, in `profile`'s mix. While the 10-minute
/// series is open and the forum has grown past 100 nodes, its occurrences are stopped: the series
/// is closed.
fn full_command(rng: &mut Rng, r: &Run, sess: SessionId, profile: Profile) -> Cmd {
    if let Some(ten) = open_series(r, TEN_MINUTES)
        && r.s.nodes.len() > 100
    {
        return Cmd::Node(NodeCmd::Close {
            at: NodeRef::Id(ten),
            outcome: Outcome::Done,
            summary: nonempty(rng),
            verified: None,
            settles: Some(BTreeSet::new()),
            learnings: None,
            token: None,
            unsent: BTreeSet::new(),
        });
    }
    match (rng.below(10), profile) {
        (0 | 1, _) => recurring_command(rng, r),
        (2..=6, Profile::Refusal) => refusal_command(rng, r, sess),
        // No user (CS-2, CS-6): the user's side is left out.
        (2 | 3, Profile::NoUser) => research_command(rng, r, sess, false),
        (2 | 3, _) => user_command(rng, r),
        _ => research_command(rng, r, sess, profile != Profile::NoUser),
    }
}

/// What a command names that may have left memory: `store::restore::named`'s list (D.4.12), the
/// shell's, written again here. An origin names every request with that origin, resident or in
/// `rows`, as the shell's `origins` table does.
fn named(r: &Run, cmd: &Cmd) -> Vec<EntityRef> {
    let mut out = Vec::new();
    let requests = |out: &mut Vec<EntityRef>, ids: &mut dyn Iterator<Item = Eid>| {
        out.extend(ids.map(EntityRef::Request));
    };
    let origin = |out: &mut Vec<EntityRef>, k: &KeyStr| {
        for (&e, q) in &r.s.requests {
            if q.origin.key == *k {
                out.push(EntityRef::Request(e));
            }
        }
        for (f, v) in &r.rows {
            if let (Fact::Request(e), Departed::Request(q)) = (f, v)
                && q.origin.key == *k
            {
                out.push(EntityRef::Request(*e));
            }
        }
    };
    match cmd {
        Cmd::Node(NodeCmd::Close {
            settles, unsent, ..
        }) => {
            requests(&mut out, &mut settles.iter().flatten().copied());
            out.extend(unsent.iter().map(|a| EntityRef::Said(*a)));
        }
        Cmd::Node(NodeCmd::Create(a)) => {
            out.extend(
                a.keys
                    .iter()
                    .filter_map(|k| k.key.user_entry())
                    .map(EntityRef::UserEntry),
            );
            if let Some(q) = &a.request {
                let schemes = &r.s.params.conversation_schemes;
                if let Some(o) = q
                    .origin
                    .as_ref()
                    .or(q.where_.as_ref())
                    .or_else(|| a.keys.iter().find(|k| k.key.is_conversation(schemes)))
                    .or_else(|| a.keys.first())
                {
                    origin(&mut out, &o.key);
                }
            }
        }
        Cmd::Hold(HoldCmd::Release {
            waits_for, unsent, ..
        }) => {
            out.extend(unsent.iter().map(|a| EntityRef::Said(*a)));
            if let Some(WaitArg::Question(q)) = waits_for {
                out.push(EntityRef::PQuestion(*q));
                out.push(EntityRef::UQuestion(*q));
            }
        }
        Cmd::Session(SessionCmd::Hello { .. }) => {}
        Cmd::Talk(t) => match t {
            TalkCmd::FromUser {
                answers, clarifies, ..
            } => {
                out.extend(answers.iter().map(|q| EntityRef::UQuestion(*q)));
                requests(&mut out, &mut clarifies.iter().copied());
            }
            TalkCmd::ToUser { re, .. } => {
                for e in re {
                    out.push(EntityRef::Request(*e));
                    out.push(EntityRef::UserEntry(*e));
                    out.push(EntityRef::Digest(*e));
                }
            }
            TalkCmd::Route { entry, .. } => out.push(EntityRef::UserEntry(*entry)),
            TalkCmd::Request { origin: o, .. } => origin(&mut out, &o.key),
            TalkCmd::Said { corrects, .. } => {
                out.extend(corrects.iter().map(|a| EntityRef::Said(*a)));
            }
            TalkCmd::SaidConfirm { said, .. } => out.push(EntityRef::Said(*said)),
            TalkCmd::Heard { answers, .. } => {
                out.extend(answers.iter().map(|q| EntityRef::PQuestion(*q)));
            }
            TalkCmd::Decide {
                settles,
                supersedes,
                basis,
                ..
            } => {
                requests(&mut out, &mut settles.iter().copied());
                out.extend(supersedes.iter().map(|d| EntityRef::Decision(*d)));
                for b in basis {
                    if let BasisArg::Entry(e) = b {
                        out.push(EntityRef::Decision(*e));
                    }
                }
            }
            TalkCmd::Resolve { objection, .. } => out.push(EntityRef::Objection(*objection)),
            _ => {}
        },
        _ => {}
    }
    out
}

/// The fact of an entity's row.
fn fact_of(x: EntityRef) -> Fact {
    match x {
        EntityRef::Request(i) => Fact::Request(i),
        EntityRef::PQuestion(i) => Fact::PQuestion(i),
        EntityRef::UQuestion(i) => Fact::UQuestion(i),
        EntityRef::Commitment(i) => Fact::Commitment(i),
        EntityRef::Said(i) => Fact::Said(i),
        EntityRef::Decision(i) => Fact::Decision(i),
        EntityRef::Objection(i) => Fact::Objection(i),
        EntityRef::UserEntry(i) => Fact::UserEntry(i),
        EntityRef::Digest(i) => Fact::Digest(i),
        EntityRef::Meta(i) => Fact::Entry(i),
    }
}

/// The restorable item for `x` at `now`, as the shell's plan builds it (`items_at`): from its
/// row when it left memory, or from its resident value when its leaving time has come; None
/// otherwise, or without the metadata of its entry.
fn item_of(r: &Run, x: EntityRef, now: Time) -> Option<Restored> {
    let f = fact_of(x);
    let resident = forum_core::facts::departed_of(&r.s, &f);
    let value = match resident {
        Some(v) if leaving_due(&r.s, x, now) => v,
        Some(_) => return None,
        None => r.rows.get(&f)?.clone(),
    };
    let id = match x {
        EntityRef::Request(i)
        | EntityRef::PQuestion(i)
        | EntityRef::UQuestion(i)
        | EntityRef::Commitment(i)
        | EntityRef::Said(i)
        | EntityRef::Decision(i)
        | EntityRef::Objection(i)
        | EntityRef::UserEntry(i)
        | EntityRef::Digest(i)
        | EntityRef::Meta(i) => i,
    };
    let meta = match r.s.entries.get(&id) {
        Some(m) => *m,
        None => match r.rows.get(&Fact::Entry(id)) {
            Some(Departed::Meta(m)) => *m,
            _ => return None,
        },
    };
    Some(match value {
        Departed::Request(v) => Restored::Request(id, v, meta),
        Departed::PQuestion(v) => Restored::PQuestion(id, v, meta),
        Departed::UQuestion(v) => Restored::UQuestion(id, v, meta),
        Departed::Commitment(v) => Restored::Commitment(id, v, meta),
        Departed::Said(v) => Restored::Said(id, v, meta),
        Departed::Decision(v) => Restored::Decision(id, v, meta),
        Departed::Objection(v) => Restored::Objection(id, v, meta),
        Departed::UserEntry(v) => Restored::UserEntry(id, v, meta),
        Departed::Digest(v) => Restored::Digest(id, v, meta),
        Departed::Meta(m) => Restored::Meta(id, m),
    })
}

/// A command that names entity `x` (D.4.12), for the probe after an advance that stops just past
/// its leaving time: the commands of `named`'s list, one per kind of entity; with `no_user`, none
/// of the user's side (`from_user`, `to_user`): a request is named by a decision, a user entry by a
/// route, and a question for the user or a digest entry by nothing.
fn naming(rng: &mut Rng, r: &Run, x: EntityRef, no_user: bool) -> Option<Cmd> {
    let node_of_entry = |e: Eid| r.s.entries.get(&e).map(|m| m.node);
    if no_user && matches!(x, EntityRef::UQuestion(_) | EntityRef::Digest(_)) {
        return None;
    }
    Some(match x {
        EntityRef::Request(e) => match if no_user { 0 } else { rng.below(3) } {
            0 => Cmd::Talk(TalkCmd::Decide {
                at: NodeRef::Id(r.s.requests.get(&e)?.node),
                text: nonempty(rng),
                settles: [e].into(),
                supersedes: None,
                basis: Vec::new(),
                evidence: None,
            }),
            1 => Cmd::Talk(TalkCmd::ToUser {
                text: nonempty(rng),
                kind: ToUserKind::Result,
                verbatim: true,
                principal: None,
                re: [e].into(),
                at: None,
            }),
            _ => Cmd::Talk(TalkCmd::FromUser {
                text: nonempty(rng),
                client: Text(String::from("cli")),
                principal: None,
                answers: None,
                to: None,
                clarifies: Some(e),
                reason: None,
            }),
        },
        EntityRef::Said(a) => {
            if rng.chance(50) {
                Cmd::Talk(TalkCmd::SaidConfirm {
                    said: a,
                    confirmed: nonempty(rng),
                })
            } else {
                Cmd::Talk(TalkCmd::Said {
                    at: NodeRef::Id(r.s.saids.get(&a)?.node),
                    where_: None,
                    text: nonempty(rng),
                    kind: SaidKind::Correction,
                    corrects: Some(a),
                    seen: None,
                })
            }
        }
        EntityRef::PQuestion(q) => Cmd::Talk(TalkCmd::Heard {
            at: NodeRef::Id(r.s.pquestions.get(&q)?.node),
            where_: None,
            from: nonempty(rng),
            text: nonempty(rng),
            answers: Some(q),
        }),
        EntityRef::UQuestion(q) => Cmd::Talk(TalkCmd::FromUser {
            text: nonempty(rng),
            client: Text(String::from("cli")),
            principal: None,
            answers: Some(q),
            to: None,
            clarifies: None,
            reason: None,
        }),
        EntityRef::Decision(d) => Cmd::Talk(TalkCmd::Decide {
            at: NodeRef::Id(node_of_entry(d)?),
            text: nonempty(rng),
            settles: BTreeSet::new(),
            supersedes: rng.chance(50).then_some(d),
            basis: vec![BasisArg::Entry(d)],
            evidence: None,
        }),
        EntityRef::Objection(o) => Cmd::Talk(TalkCmd::Resolve {
            objection: o,
            note: nonempty(rng),
            upheld: false,
            defer: false,
        }),
        EntityRef::UserEntry(e) => {
            if no_user || rng.chance(50) {
                Cmd::Talk(TalkCmd::Route {
                    entry: e,
                    to: Some(forum_core::RouteTo::Nowhere),
                    answers: None,
                    clarifies: None,
                    reason: Some(nonempty(rng)),
                })
            } else {
                Cmd::Talk(TalkCmd::ToUser {
                    text: nonempty(rng),
                    kind: ToUserKind::Ack,
                    verbatim: true,
                    principal: None,
                    re: [e].into(),
                    at: None,
                })
            }
        }
        EntityRef::Digest(d) => Cmd::Talk(TalkCmd::ToUser {
            text: nonempty(rng),
            kind: ToUserKind::Ack,
            verbatim: true,
            principal: {
                let to = r.s.digest.get(&d)?.to;
                r.s.principal_ids
                    .iter()
                    .find(|(_, p)| **p == to)
                    .map(|(k, _)| k.clone())
            },
            re: [d].into(),
            at: None,
        }),
        EntityRef::Commitment(_) | EntityRef::Meta(_) => return None,
    })
}

/// An outcome without what memory may change (D.4.12, "Reads"): a `moved` warning's total, which
/// is exact only for a `seen` at or above the node's `people_left`, so a restored `said` makes it
/// "at least".
fn memory_blind(out: Result<Done, Error>) -> Result<Done, Error> {
    out.map(|mut d| {
        for w in &mut d.warnings {
            if let WarningDetail::Moved { total, .. } = &mut w.detail {
                *total = Total::Exact(0);
            }
        }
        d
    })
}

/// A targeted flow of the full universe (A-80, §12, R2): an available occurrence that nobody holds
/// is taken five minutes before its window ends by a new session of class `c1`, with no checkpoint;
/// the window passes while the lease (15 minutes) is live; then a new session of class `c2` moves
/// it, which ends the hold, and the occurrence is missed in the same step. Every call and firing
/// runs with its checks (R2's step half among them).
fn move_a_held_occurrence(seed: u64, r: &mut Run, rng: &mut Rng, c1: &ClassName, c2: &ClassName) {
    let ready = |r: &Run| -> Vec<(NodeId, Time)> {
        r.s.nodes
            .iter()
            .filter_map(|(&o, x)| match &x.role {
                Role::Occurrence(y)
                    if x.is_open()
                        && y.phase == OccPhase::Available
                        && !y.window_fired
                        && !r.s.holds.keys().any(|t| t.node == o)
                        && r.s.nodes.get(&y.series).is_some_and(|z| z.is_open()) =>
                {
                    Some((o, y.window_end))
                }
                _ => None,
            })
            .collect()
    };
    let Some((o, end)) = rng.pick(&ready(r)) else {
        return;
    };
    let take_at = end.minus(Dur(300_000));
    if take_at > r.clock {
        r.clock = take_at;
        r.settle(seed);
    }
    if !ready(r).iter().any(|(x, _)| *x == o) {
        return;
    }
    let take = Cmd::Hold(HoldCmd::Take {
        target: NodeRef::Id(o),
        lease: None,
    });
    if r.call(seed, c1, SessionCtx::New, None, &take).is_err() {
        return;
    }
    let held = |r: &Run| r.s.holds.keys().any(|t| t.node == o);
    r.clock = end.plus(Dur(1_000)).max(r.clock);
    r.settle(seed);
    if !held(r) || !r.s.nodes[&o].is_open() {
        return;
    }
    let mv = Cmd::Node(NodeCmd::Move {
        at: NodeRef::Id(o),
        to: move_path(rng),
    });
    let clean = r.s.nodes[&o].checkpoints == 0 && r.s.nodes[&o].open_intent.is_none();
    let n_events = r.log.len();
    let out = r.call(seed, c2, SessionCtx::New, None, &mv);
    // A move to the node's own place moves nothing and ends no hold (D.8.20).
    let moved = r.log[n_events..]
        .iter()
        .any(|e| matches!(e.ev, Ev::NodeMoved { node, .. } if node == o));
    if out.is_ok() && moved && clean {
        assert!(
            matches!(&r.s.nodes[&o].life, Life::Closed(x) if x.outcome == Outcome::Missed),
            "seed {seed}: an occurrence moved off its holder after its window was not missed"
        );
    }
}

/// Item 12's twin of a due probe (D.12.3: commands that name entities that left "restore them and
/// then behave as on a run where nothing left"): the state as on a run where `x`, resident and due,
/// never leaves (its leaving time cancelled), with every other entity `cmd` names restored as the
/// shell restores it. The command's outcome on it must equal the outcome after the restore step.
fn nothing_left(seed: u64, r: &Run, x: EntityRef, cmd: &Cmd) -> Option<State> {
    let mut t = r.s.clone();
    let at = t.leaving.remove(&x)?;
    assert!(
        t.ix.leaving_by_time.remove(&(at, x)),
        "seed {seed}: {x:?} not in leaving_by_time"
    );
    let now = r.clock.max(t.last_now);
    let items: Vec<Restored> = named(r, cmd)
        .into_iter()
        .filter(|y| *y != x)
        .filter_map(|y| item_of(r, y, now))
        .collect();
    if !items.is_empty() {
        let mut dirty = Tracked::new();
        let _ = restore_tracked(&mut t, r.clock, items, &mut dirty);
    }
    Some(t)
}

/// The full universe's advance that stops just past the earliest leaving time with no event at
/// or after it (D.4.12's fix): settle to just before that time, then, if no deadline lies at or
/// before one millisecond past it, move there; the entity is then resident and due. Returns it.
fn past_a_leaving_time(seed: u64, r: &mut Run) -> Option<EntityRef> {
    let &(t, x) = r.s.ix.leaving_by_time.first()?;
    if open_series(r, TEN_MINUTES).is_some() || t <= r.clock {
        return None;
    }
    r.clock = t.minus(Dur(1));
    r.settle(seed);
    let (t, x) = match r.s.ix.leaving_by_time.first() {
        Some(&first) if first == (t, x) => first,
        _ => return None,
    };
    let quiet =
        r.s.ix
            .deadlines
            .first()
            .is_none_or(|k| k.at > t.plus(Dur(1)));
    if !quiet {
        return None;
    }
    r.clock = t.plus(Dur(1));
    r.settle(seed);
    assert!(leaving_due(&r.s, x, r.clock), "seed {seed}: {x:?} not due");
    Some(x)
}

/// Item 8 for the user's side (U1's query property, CS-3): `hello`'s user items and counts for
/// every session equal `spec::hello_items`, and `chain` and `audience` equal their naive forms
/// (D-3), over every pending question and digest entry.
fn user_agree(seed: u64, r: &Run) {
    for &x in r.s.sessions.keys() {
        let h = query::hello(&r.s, x, r.clock);
        let want = spec::hello_items(&r.s, x);
        assert_eq!(
            h.user_decisions.count as usize,
            want.questions.len(),
            "seed {seed}: hello's questions of {x:?}"
        );
        assert!(
            h.user_decisions
                .items
                .iter()
                .all(|q| want.questions.contains(q)),
            "seed {seed}: hello's question items of {x:?}"
        );
        assert_eq!(
            h.user_digest.count as usize,
            want.digest.len(),
            "seed {seed}: hello's digest of {x:?}"
        );
        assert!(
            h.user_digest.items.iter().all(|d| want.digest.contains(d)),
            "seed {seed}: hello's digest items of {x:?}"
        );
        if !r.s.sessions[&x].principals.is_empty() {
            assert_eq!(
                h.user_digest.older.unwrap_or(0),
                want.older,
                "seed {seed}: hello's older of {x:?}"
            );
        }
    }
    for d in r.s.digest.values() {
        let _ = spec::audience(&r.s, &d.item);
    }
}

/// One case: `steps` steps from `seed` in `profile`. Returns the log, for item 6.
fn case(seed: u64, steps: usize, profile: Profile) -> Vec<Event> {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut r = Run::new(params(&mut rng, profile));
    r.restores = profile.full();
    let classes = profile.classes();
    let class_of =
        |rng: &mut Rng| ClassName::new(classes[rng.below(classes.len() as u64) as usize]).unwrap();
    // Churn: `op_id`s from a pool of 6, so that they are reused across sessions (CS-8).
    let op_pool = if profile == Profile::Churn { 6 } else { 24 };
    for _ in 0..steps {
        match rng.below(12) {
            // The full universe: an advance that stops just past an entity's leaving time with
            // no event since, then a call from a new session that names it (D.4.12's fix: the
            // restore step brings it back first, and the command finds it).
            0 if profile.full() && rng.chance(25) => {
                if let Some(x) = past_a_leaving_time(seed, &mut r)
                    && let Some(cmd) = naming(&mut rng, &r, x, profile == Profile::NoUser)
                {
                    let c = class_of(&mut rng);
                    // Item 12: the twin, a run where `x` never left (its leaving time cancelled,
                    // and the other entities the command names restored, as the shell does).
                    let twin = nothing_left(seed, &r, x, &cmd).map(|mut t| {
                        let ctx = Ctx {
                            now: r.clock,
                            class: c.clone(),
                            caps: (c.as_str() == "c").then(|| [label("x")].into()),
                            session: SessionCtx::New,
                            op: None,
                        };
                        apply(&mut t, &ctx, &cmd).1
                    });
                    let out = r.call(seed, &c, SessionCtx::New, None, &cmd);
                    assert!(
                        forum_core::facts::departed_of(&r.s, &fact_of(x)).is_some(),
                        "seed {seed}: {x:?} named while due did not come back"
                    );
                    if let Some(twin) = twin {
                        assert_eq!(
                            memory_blind(out),
                            memory_blind(twin),
                            "seed {seed}: {cmd:?} after restoring {x:?} differs from a run where it never left"
                        );
                        *r.stats.entry(String::from("due probe, twin")).or_insert(0) += 1;
                    }
                    *r.stats.entry(String::from("due probe")).or_insert(0) += 1;
                }
            }
            0 => {
                // Advance: from a second to three days (FINISHED_RETAIN is 7 days: two long
                // advances in a case pass it); in the full universe sometimes eight days, and at
                // most half an hour while the 10-minute series is open (each slot makes a node).
                let long = if profile == Profile::Churn { 40 } else { 12 };
                let mut d = if rng.chance(long) {
                    [86_400_000_u64, 259_200_000][rng.below(2) as usize]
                } else {
                    // Mostly shorter than `lease.first` (15 minutes), so that holds live across
                    // a few calls; sometimes past it, or past `presence.silence`.
                    [
                        1_000_u64, 10_000, 60_000, 300_000, 300_000, 900_000, 3_600_000,
                    ][rng.below(7) as usize]
                };
                if profile.full() {
                    if open_series(&r, TEN_MINUTES).is_some() {
                        d = d.min(1_800_000);
                    } else if rng.chance(8) {
                        d = 8 * 86_400_000;
                    } else if profile == Profile::Churn && rng.chance(20) {
                        // Past `session.retain` (24 h): every session that has not called since
                        // retires.
                        d = 25 * 3_600_000;
                    }
                }
                // Item 3 (T2) at random points: run(run(S, t1), t2) == run(S, t2).
                if rng.chance(30) {
                    let t2 = r.clock.plus(Dur(d));
                    let t1 = r.clock.plus(Dur(rng.below(d + 1)));
                    let mut a = r.s.clone();
                    let (e1, _) = settle(&mut a, t1, u32::MAX);
                    let (e2, _) = settle(&mut a, t2, u32::MAX);
                    let mut b = r.s.clone();
                    let (all, _) = settle(&mut b, t2, u32::MAX);
                    assert!(a == b, "seed {seed}: T2");
                    let mut both = e1;
                    both.extend(e2);
                    assert_eq!(both, all, "seed {seed}: T2 events");
                }
                r.clock = r.clock.plus(Dur(d));
                r.settle(seed);
                // Item 4: settling again changes nothing.
                let mut again = r.s.clone();
                let (none, _) = settle(&mut again, r.clock, u32::MAX);
                assert!(none.is_empty(), "seed {seed}: idempotence");
                if let Err(v) = check_invariants(&r.s, r.clock) {
                    panic!("seed {seed}: invariants after an advance: {v:?}");
                }
            }
            // The full universe: a move by another session of an occurrence held past its window
            // (A-80, R2): the move ends the hold, and the occurrence is missed in that step.
            1 if profile.full() && rng.chance(10) => {
                let c1 = class_of(&mut rng);
                let c2 = class_of(&mut rng);
                move_a_held_occurrence(seed, &mut r, &mut rng, &c1, &c2);
            }
            1 if !r.ops.is_empty() => {
                // Item 2: a retry of a recorded call, from its session, from another session of
                // its class, with no session (a call that would open one), or with another digest;
                // sometimes after `op.retain`, when it executes afresh. The outcome follows the
                // live record (O1, CS-8).
                let (c, sess, op, cmd) = rng.pick(&r.ops).unwrap();
                if rng.chance(15) {
                    let retain = r.s.params.op_retain;
                    r.clock = r.clock.plus(retain);
                    r.settle(seed);
                    if let Err(v) = check_invariants(&r.s, r.clock) {
                        panic!("seed {seed}: invariants after op.retain: {v:?}");
                    }
                }
                // The pre-settle of the call runs at this clock, so the record read here is the one
                // the call meets.
                let rec =
                    r.s.ops
                        .get(&(c.clone(), op.id.clone()))
                        .filter(|x| x.expires_at > r.clock)
                        .copied();
                let other = r
                    .sessions
                    .iter()
                    .find(|(k, x)| *k == c && *x != sess)
                    .map(|(_, x)| *x);
                let (sc, digest) = match rng.below(4) {
                    0 => (SessionCtx::Existing(sess), op.digest),
                    1 => (SessionCtx::Existing(other.unwrap_or(sess)), op.digest),
                    2 => (SessionCtx::New, op.digest),
                    _ => (SessionCtx::Existing(sess), ReqDigest([0xEE; 32])),
                };
                let n_events = r.log.len();
                let o = OpCtx {
                    id: op.id.clone(),
                    digest,
                };
                let out = r.call(seed, &c, sc, Some(o), &cmd);
                if cmd.longest_text() > u64::from(r.s.params.text_max) {
                    assert_eq!(
                        out,
                        Err(Error::limit(LimitKind::TextTooLong)),
                        "seed {seed}"
                    );
                } else if let Some(rec) = rec {
                    let same = match sc {
                        SessionCtx::New => true,
                        SessionCtx::Existing(x) => x == rec.session,
                        SessionCtx::System => false,
                    };
                    if rec.digest == digest && same {
                        assert_eq!(out.map(|d| d.reply), Ok(Reply::Replay), "seed {seed}");
                    } else {
                        assert_eq!(out, Err(Error::op_id_conflict()), "seed {seed}");
                    }
                    assert_eq!(r.log.len(), n_events, "seed {seed}: a retry appended");
                } else {
                    // No live record: the call executes afresh, past the retry store.
                    assert!(
                        !matches!(&out, Ok(d) if d.reply == Reply::Replay)
                            && out != Err(Error::op_id_conflict()),
                        "seed {seed}: an expired record answered: {out:?}"
                    );
                    *r.stats.entry(String::from("retry afresh")).or_insert(0) += 1;
                }
            }
            2 | 3 if r.s.sessions.len() < 6 || r.s.sessions.len() < 3 => {
                let c = class_of(&mut rng);
                // The user side (research and guided): a session that serves a principal, often
                // one with questions or digest entries pending (rules 1 and 12).
                let principal = (matches!(profile, Profile::Research | Profile::Guided)
                    && rng.chance(40))
                .then(|| any_principal(&mut rng));
                let cmd = Cmd::Session(SessionCmd::Hello {
                    intent: None,
                    caps: rng.chance(60).then(|| labels(&mut rng)),
                    horizon: rng
                        .chance(30)
                        .then(|| [20_u32, 64, 200][rng.below(3) as usize]),
                    principal,
                    from_user: None,
                });
                let _ = r.call(seed, &c, SessionCtx::New, None, &cmd);
            }
            _ => {
                // A live session, mostly one that holds something when one does (so that hold
                // writes mostly carry a live token); 5 % of the time any session, a retired one
                // included (`not_found(Session)`).
                let live: Vec<(ClassName, SessionId)> = r
                    .sessions
                    .iter()
                    .filter(|(_, x)| r.s.sessions.contains_key(x))
                    .cloned()
                    .collect();
                let holders: Vec<(ClassName, SessionId)> = live
                    .iter()
                    .filter(|(_, x)| r.s.holds.values().any(|h| h.session == *x))
                    .cloned()
                    .collect();
                let pick = if rng.chance(5) {
                    rng.pick(&r.sessions)
                } else if rng.chance(85) && !holders.is_empty() {
                    rng.pick(&holders)
                } else {
                    rng.pick(&live)
                };
                // Churn: a third of the calls open a session of their own, which makes one or two
                // calls and vanishes (CS-8).
                let churn = profile == Profile::Churn && rng.chance(33);
                let (c, sc, sess) = match pick {
                    _ if churn => {
                        let c = class_of(&mut rng);
                        (c, SessionCtx::New, None)
                    }
                    Some((c, sess)) => (c, SessionCtx::Existing(sess), Some(sess)),
                    None => continue,
                };
                let as_sess = sess.unwrap_or(SessionId(u64::MAX));
                let cmd = match profile {
                    Profile::Holds => command(&mut rng, &r, as_sess),
                    Profile::Keys => keys_command(&mut rng, &r, as_sess),
                    Profile::Research | Profile::Guided => {
                        research_command(&mut rng, &r, as_sess, true)
                    }
                    Profile::Full
                    | Profile::OneClass
                    | Profile::NoUser
                    | Profile::Churn
                    | Profile::Refusal => full_command(&mut rng, &r, as_sess, profile),
                };
                // An `op_id` on 30 % of the calls, from a pool of 24 (6 under churn): a reused one
                // with the same command is a replay, and with another an `op_id_conflict` (O1).
                let op = rng.chance(30).then(|| OpCtx {
                    id: OpId::new(&format!("op-{}", rng.below(op_pool))).unwrap(),
                    digest: digest_of(&cmd),
                });
                let out = r.call(seed, &c, sc, op.clone(), &cmd);
                let sess = match (&out, sess) {
                    (_, Some(x)) => Some(x),
                    (Ok(d), None) => d.opened,
                    (Err(_), None) => None,
                };
                if let (Some(o), Some(sess), true) =
                    (op, sess, out.is_ok() || r.log.last().is_some())
                {
                    r.ops.push((c.clone(), sess, o, cmd));
                }
                if profile == Profile::Guided
                    && let Some(x) = sess
                    && guided(x)
                {
                    follow(seed, &mut r, &c, x);
                }
            }
        }
        agree_with_spec(seed, &r);
        leave_better_agrees(seed, &r);
        bounded(seed, &r);
        if matches!(profile, Profile::Research | Profile::Guided) || profile.full() {
            people_agree(seed, &r);
        }
        if profile.full() {
            user_agree(seed, &r);
        }
    }
    // Item 5: the log folds to the state.
    let mut s = State::default();
    for e in &r.log {
        evolve_mut(&mut s, e);
    }
    assert!(s == r.s, "seed {seed}: the log does not fold to the state");
    #[cfg(feature = "serde")]
    for e in &r.log {
        let j = serde_json::to_string(e).unwrap();
        let back: Event = serde_json::from_str(&j).unwrap();
        assert_eq!(&back, e, "seed {seed}: JSON round trip");
    }
    if std::env::var("FORUM_PROP_STATS").is_ok() {
        for (k, v) in &r.stats {
            eprintln!("{v:6} {k}");
        }
    }
    r.log
}

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The cases of a profile: `FORUM_PROP_SEED` runs one; otherwise `FORUM_PROP_CASES` of them, with
/// item 6 (determinism) on every eighth.
fn run_profile(profile: Profile) {
    let steps = env("FORUM_PROP_STEPS", 200);
    if let Ok(seed) = std::env::var("FORUM_PROP_SEED") {
        case(seed.parse().unwrap(), steps, profile);
        return;
    }
    // The full universe costs about 2 s a case in a debug build (its state grows to a hundred
    // nodes and thirty sessions, and item 8 compares every session's views), so a debug build runs
    // 32 of its cases and a release build D.12.3's 256 (`cargo test --release -p forum-core
    // --test prop`); the other profiles run 256 either way.
    let default = if profile.full() && cfg!(debug_assertions) {
        32
    } else {
        256
    };
    let cases = env("FORUM_PROP_CASES", default);
    for seed in 1..=cases as u64 {
        let log = case(seed, steps, profile);
        if seed % 8 == 0 {
            assert_eq!(
                case(seed, steps, profile),
                log,
                "seed {seed}: two runs differ"
            );
        }
    }
}

#[test]
fn the_holds_universe() {
    run_profile(Profile::Holds);
}

#[test]
fn the_keys_universe() {
    run_profile(Profile::Keys);
}

#[test]
fn the_research_universe() {
    run_profile(Profile::Research);
}

#[test]
fn the_full_universe() {
    run_profile(Profile::Full);
}

#[test]
fn the_full_universe_with_one_class() {
    run_profile(Profile::OneClass);
}

#[test]
fn the_full_universe_with_no_user() {
    run_profile(Profile::NoUser);
}

#[test]
fn the_full_universe_with_churn() {
    run_profile(Profile::Churn);
}

#[test]
fn the_full_universe_with_refusals() {
    run_profile(Profile::Refusal);
}

#[test]
fn the_guided_universe() {
    run_profile(Profile::Guided);
}

/// A third of the sessions are guided (D.12.3's guided profile).
fn guided(x: SessionId) -> bool {
    x.0.is_multiple_of(3)
}

/// A guided session makes its first next call whenever it is `now`, with the standard fill, at
/// once and up to three in a row (items 14 and 15).
fn follow(seed: u64, r: &mut Run, c: &ClassName, x: SessionId) {
    use query::guide::{Moment, Skeleton, standard_fill};
    for _ in 0..3 {
        owes_nothing_when_told_nothing(seed, r, x);
        let Some(item) = r
            .guidance
            .get(&x)
            .and_then(|g| g.items.first())
            .filter(|i| i.when == Moment::Now)
            .cloned()
        else {
            return;
        };
        if matches!(item.call, Skeleton::Hello) || !r.s.sessions.contains_key(&x) {
            return;
        }
        let (sc, cmd) = standard_fill(&r.s, Some(x), &item.call)
            .unwrap_or_else(|| panic!("seed {seed}: no standard fill for {item:?}"));
        let before = r.s.clone();
        let logged = r.log.len();
        let out = r.call(seed, c, sc, None, &cmd);
        let made: Vec<Eid> = r.log[logged..]
            .iter()
            .filter_map(|e| match &e.ev {
                Ev::Appended {
                    eid,
                    body: EntryBody::Checkpoint { .. },
                    ..
                } => Some(*eid),
                _ => None,
            })
            .collect();
        r.guided_checkpoints.extend(made);
        *r.stats
            .entry(format!("guided {}", item.rule.name()))
            .or_insert(0) += 1;
        // G1 (b) held on the response's state, which nothing has changed since; a tree limit met
        // by a `CreateFor`'s path is the agent's to choose another.
        let excused = matches!(item.call, Skeleton::CreateFor { .. })
            && matches!(
                out,
                Err(Error {
                    hint: Hint::Limit(LimitKind::Depth | LimitKind::Children),
                    ..
                })
            );
        if excused {
            return;
        }
        let d = out
            .unwrap_or_else(|e| panic!("seed {seed}: the next call {item:?} was refused: {e:?}"));
        effective(seed, &before, r, x, &item, &d);
    }
}

/// Item 14: the call an item describes removes the condition that produced it (rules 3, 5 to 8, 9,
/// 10 but the split and the merge, 11, 12, and 13; 1.9: rule 2 is its `tell_holder` alone, and no
/// item confirms a `said`).
fn effective(
    seed: u64,
    before: &State,
    r: &Run,
    x: SessionId,
    item: &query::guide::NextCall,
    d: &Done,
) {
    use query::guide::{Rule, Skeleton};
    let s = &r.s;
    let fail = |what: &str| -> ! { panic!("seed {seed}: {what} after following {item:?}: {d:?}") };
    match (&item.rule, &item.call) {
        // 1.9 (§10.4, §11.3): a review, after `next` or as a tidy item, decides the objection.
        (Rule::Review | Rule::Tidy, Skeleton::Resolve { objection }) => {
            if s.objections
                .get(objection)
                .is_some_and(|o| o.state == ObjectionState::Open)
            {
                fail("the objection is still open");
            }
        }
        (
            Rule::Checkpoint | Rule::IntentDone,
            Skeleton::Checkpoint {
                token, intent_done, ..
            },
        ) => {
            if s.holds
                .get(token)
                .is_none_or(|h| h.last_extended != r.clock)
            {
                fail("the hold was not extended now");
            }
            if *intent_done
                && s.nodes[&token.node].open_intent == before.nodes[&token.node].open_intent
            {
                fail("the intent is still open");
            }
        }
        (Rule::Route, Skeleton::CreateFor { entry } | Skeleton::Route { entry, .. })
        | (Rule::RouteItem, Skeleton::Route { entry, .. }) => {
            if s.ix.unrouted.contains(entry) {
                fail("the entry is still unrouted");
            }
        }
        (Rule::Reply, Skeleton::Said { .. }) => {
            if let Reply::Said { answered, .. } = &d.reply
                && answered.is_empty()
            {
                fail("the reply answered nothing");
            }
        }
        (Rule::Reply, Skeleton::ToUser { .. }) => {
            if let Reply::ToUser { settled, .. } = &d.reply
                && settled.is_empty()
                && d.warnings
                    .iter()
                    .any(|w| w.code == WarningCode::ResultAnsweredNothing)
            {
                fail("the report answered nothing");
            }
        }
        (Rule::Decide, Skeleton::Decide { settles, .. }) => {
            for q in settles {
                if s.requests.get(q).is_some_and(|x| x.settled.is_none())
                    && s.ix.settling.get(q).is_none_or(BTreeSet::is_empty)
                {
                    fail("a request is still undecided");
                }
            }
        }
        (Rule::Close, Skeleton::Close { node, .. }) => {
            let unclean = d.warnings.iter().any(|w| {
                matches!(
                    w.code,
                    WarningCode::UnsettledRequests
                        | WarningCode::OpenCommitments
                        | WarningCode::OpenObjections
                        | WarningCode::HeldByOther
                )
            });
            if s.nodes[node].is_open() || unclean {
                fail("the close is not clean");
            }
        }
        (
            Rule::ReleaseContainer | Rule::AskWait | Rule::HeldFlag,
            Skeleton::Release { token, .. },
        ) => {
            if s.holds.contains_key(token) {
                fail("the hold is still live");
            }
        }
        (Rule::Offer, Skeleton::AddOffer { node, .. }) => {
            if !s.nodes[node].tags.contains(&label("offer")) {
                fail("the node is not offered");
            }
        }
        (Rule::JoinAnswer, Skeleton::Heard { answers, .. }) => {
            if s.pquestions
                .get(answers)
                .is_some_and(|q| q.state == PqState::Open)
            {
                fail("the question is still open");
            }
        }
        (Rule::StatusReply, Skeleton::Said { .. }) => {
            if let Reply::Said { fulfilled, .. } = &d.reply
                && fulfilled.is_empty()
            {
                fail("the status reply fulfilled nothing");
            }
        }
        (Rule::HeldFlag, Skeleton::Decide { at, supersedes, .. }) => {
            if let Some(old) = supersedes
                && s.decisions
                    .get(old)
                    .is_some_and(|x| x.state == DecisionState::Active)
            {
                fail("the stale decision is still active");
            }
            if supersedes.is_none()
                && !s.nodes[at].assume_marks.is_empty()
                && s.nodes[at].assume_marks == before.nodes[at].assume_marks
            {
                fail("the assumption marks stay");
            }
        }
        (Rule::Inbox, _) => {
            let was = query::inbox(before, x, r.clock).len();
            let now = query::inbox(s, x, r.clock).len();
            if was > 0 && now >= was {
                fail("the inbox did not shrink");
            }
        }
        (Rule::AnswerAgent, Skeleton::AnswerAgent { question }) => {
            if s.aquestions
                .get(question)
                .is_some_and(|q| q.state == AqState::Open)
            {
                fail("the agent question is still open");
            }
        }
        (Rule::AnswersNext, Skeleton::AnswerUser { question, .. }) => {
            if s.uquestions
                .get(question)
                .is_some_and(|q| q.state == UqState::Pending)
            {
                fail("the user question is still pending");
            }
        }
        (Rule::HumanItems, Skeleton::ToUser { principal, .. }) => {
            if !s.sessions[&x].told.contains(principal) {
                fail("the person was not told");
            }
        }
        (Rule::Tidy | Rule::TakeJoined, Skeleton::Take { target }) => {
            if !s
                .holds
                .iter()
                .any(|(t, h)| t.node == *target && h.session == x)
            {
                fail("the node is not held");
            }
        }
        (Rule::MoreWork, Skeleton::Next) => {
            if matches!(d.reply, Reply::Next(NextReply::Empty { .. })) {
                fail("next returned nothing");
            }
        }
        _ => {}
    }
}

/// The last answer of request `q` (§20.9 Terms), as `spec::left_unfinished` reads it: the entry
/// that answered it, or a later `said` of kind `reply`, `result`, or `correction` in its
/// conversation on its node that is not withheld.
fn last_answer(s: &State, q: Eid, req: &Request) -> Eid {
    let answer = req.answered_by.unwrap_or(q);
    s.saids
        .iter()
        .filter(|(_, y)| {
            y.node == req.node
                && y.conv == req.conv
                && matches!(
                    y.kind,
                    SaidKind::Reply | SaidKind::Result | SaidKind::Correction
                )
                && y.state != SaidState::Withheld
        })
        .map(|(e, _)| *e)
        .fold(answer, Eid::max)
}

/// Whether node `n` has a checkpoint after `answer` that no guided session made by following a
/// next call (item 15): the work went on by a session's own choice.
fn unguided_after(r: &Run, n: NodeId, answer: Eid) -> bool {
    r.log.iter().any(|e| {
        matches!(
            &e.ev,
            Ev::Appended {
                eid,
                node,
                body: EntryBody::Checkpoint { .. },
                ..
            } if *node == n && *eid > answer && !r.guided_checkpoints.contains(eid)
        )
    })
}

/// Item 15: a guided session whose response says `next_calls: none` owes nothing that a standing
/// rule tracks (1.9: a `said` of its own may be pending, since a recorded `said` is taken as sent
/// and no rule asks to confirm it, §9.2), no entry of its own unrouted,
/// no open intent on a node it holds, no unread note, no request on a node it holds owed a reply
/// or a decision (but a stranded one, below), and, after a call that succeeded, no person it
/// serves with items pending and not told (1.4, R13). A response that reaches nothing else carries
/// no next calls at all, and is not asserted on.
fn owes_nothing_when_told_nothing(seed: u64, r: &Run, x: SessionId) {
    let Some(g) = r.guidance.get(&x) else {
        return;
    };
    if !g.items.is_empty() || !r.s.sessions.contains_key(&x) {
        return;
    }
    // A response that reaches nothing else (a replay, a text over `text.max`, busy, or
    // `write.per_hour`) carries no next calls at all (§20.9, §21): it does not say "none".
    let quiet = r.last.get(&x).is_none_or(|out| match out {
        Ok(d) => d.reply == Reply::Replay,
        Err(e) => matches!(
            e.hint,
            Hint::Limit(LimitKind::TextTooLong | LimitKind::Busy | LimitKind::WritesPerHour)
        ),
    });
    if quiet {
        return;
    }
    let s = &r.s;
    let now = r.clock;
    let fail = |what: &str| -> ! { panic!("seed {seed}: next_calls none for {x:?}, yet {what}") };
    if s.user_entries
        .values()
        .any(|u| u.session == x && u.routing == Routing::Unrouted)
    {
        fail("an entry of its own is unrouted");
    }
    if !query::inbox(s, x, now).is_empty() {
        fail("a note is unread");
    }
    // 1.4 (R13, D.12.3 item 15): it has told every person it serves who has items pending, under
    // any alias; rule 12 runs only after a call that succeeded (rule E), so only then.
    if r.last.get(&x).is_some_and(Result::is_ok) {
        for &p in &s.sessions[&x].principals {
            let aliases = spec::aliases(s, p);
            let told = s.sessions[&x].told.iter().any(|t| aliases.contains(t));
            let pending = s.uquestions.values().any(|q| {
                q.state == UqState::Pending
                    && q.to
                        .get(usize::from(q.stage))
                        .is_some_and(|to| aliases.contains(to))
            }) || s
                .digest
                .values()
                .any(|d| d.state == DigestState::Pending && aliases.contains(&d.to));
            if pending && !told {
                fail("a person it serves has items pending and was not told");
            }
        }
    }
    for (t, h) in &s.holds {
        if h.session != x || now >= h.lease_until {
            continue;
        }
        let n = t.node;
        if s.nodes[&n].open_intent.is_some() {
            fail("an intent is open on a node it holds");
        }
        for (q, req) in &s.requests {
            if req.node != n || req.settled.is_some() {
                continue;
            }
            let decided = s.ix.settling.get(q).is_some_and(|d| !d.is_empty());
            // Not asserted, pending an answer (WP-D7's report): a request of someone
            // outside the swarm whose conversation key its node no longer carries, while another
            // open node does. A `said` in that conversation would be recorded on that node and
            // answer nothing (§9.1, §9.4), so rule 6 suggests none, and nothing can answer it.
            let stranded = decided
                && req.answered_by.is_none()
                && req.requester == Requester::External
                && spec::conv_node(s, n, &req.conv) != n;
            // Not asserted either: a request left unfinished (1.10, §20.9 Terms), answered and
            // then checkpointed, whose answer was partial: its decision comes with the close once a
            // result ends the work, and no rule suggests one before (rule 7). Only when a
            // checkpoint after its last answer was made by a session of its own accord, not by
            // following a next call: the guide never strands the request it asked to close. 1.12
            // had excused every such request after a guided session, in seed 9, followed rule 3's
            // due checkpoint before rule 8's close of the request its result had just answered;
            // that checkpoint is now `later` beside the close.
            let unfinished = decided != req.answered_by.is_some()
                && req.answered_by.is_some()
                && r.last
                    .get(&x)
                    .and_then(|o| o.as_ref().ok())
                    .is_some_and(|d| {
                        // The call is not needed beyond its reply: a `to_user` that records a
                        // result is later than any checkpoint, and the last call checkpointed.
                        spec::left_unfinished(
                            s,
                            &Cmd::Control(forum_core::ControlCmd::Health),
                            Some(&d.reply),
                            *q,
                        )
                    })
                && unguided_after(r, n, last_answer(s, *q, req));
            if decided != req.answered_by.is_some() && !stranded && !unfinished {
                fail("a request on a node it holds is owed a reply or a decision");
            }
        }
    }
}
