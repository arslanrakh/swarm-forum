//! The test world (design D.12.1), behind the feature `testkit` and in the crate's own tests.
//!
//! WP-D2 builds the part that exists before `apply` and `settle` (WP-D3): a world that starts at
//! Genesis, applies events through `evolve_tracked` with a fake clock, and after every event
//! checks that nothing was an anomaly, that the dirty set covers every changed fact (D.12.3, item
//! 11), and that the incremental indices equal the recount (R1) and the deadline queue its recount
//! (T4); `replay` folds the log from Genesis. WP-D3 adds `World::hello`, `call`, and `advance`,
//! which run the shell's pipeline (pre-settle, `apply`, post-settle) with `check_fire` after every
//! firing, `check_step` after every command, the dirty-set check of D.12.3 item 11 for both,
//! `decide == apply` (item 7), and `check_invariants` with R1 at the end; `last_guidance` holds
//! the last next calls (WP-D7).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::apply::{apply_tracked, decide};
use crate::cmd::{Cmd, Ctx, OpCtx, SessionCmd, SessionCtx};
use crate::deadlines::recount_deadlines;
use crate::error::Error;
use crate::event::{Ev, Event, NodeInit, NodePatch, genesis};
use crate::evolve::evolve_tracked;
use crate::facts::{Fact, Tracked, changed_facts, departed_of};
use crate::invariants::{
    Violation, check_fire, check_guide, check_invariants, check_step, r1_differences,
};
use crate::params::Params;
use crate::query::NoMeter;
use crate::query::guide::Guidance;
use crate::reply::{Done, Settled};
use crate::settle::settle_tracked;
use crate::state::{Origin, Role, State};
use crate::types::{ClassName, Dur, Label, Name, NodeId, ROOT, SessionId, Text, Time};

/// A forum under test: its state, a fake clock, the sessions it opened, and every event applied.
#[derive(Clone, Debug)]
pub struct World {
    /// The state.
    pub s: State,
    /// The fake clock: the time of the next event.
    pub clock: Time,
    /// The sessions opened in the world, with their classes.
    pub sessions: Vec<(ClassName, SessionId)>,
    /// Every event applied since Genesis, Genesis included.
    pub log: Vec<Event>,
    /// The facts the last event marked dirty.
    pub dirty: BTreeSet<Fact>,
    /// The entities the last event took out of memory, with their last values.
    pub departed: alloc::collections::BTreeMap<Fact, crate::facts::Departed>,
    /// The events of the last command `call` ran.
    pub last_events: Vec<Event>,
    /// The next calls of the last response `call` computed (WP-D7, D.12.1), checked by G1.
    pub last_guidance: Guidance,
}

/// What `World::emit` found wrong after an event.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Broken {
    /// `evolve` counted an anomaly (D.7.5).
    Anomaly,
    /// A fact changed that the dirty set does not name (D.12.3, item 11).
    Untracked(Vec<Fact>),
    /// An entity left memory without its last value in `Tracked::departed`, or with another value
    /// (review E1-spec-1).
    Departed(Vec<Fact>),
    /// Indices that differ from the recount (R1).
    Indices(Vec<&'static str>),
    /// The deadline queue differs from its recount (T4).
    Deadlines,
}

impl World {
    /// A world whose log starts with Genesis at `start`.
    #[must_use]
    pub fn new(params: Params, start: Time) -> World {
        let mut w = World {
            s: State::default(),
            clock: start,
            sessions: Vec::new(),
            log: Vec::new(),
            dirty: BTreeSet::new(),
            departed: alloc::collections::BTreeMap::new(),
            last_events: Vec::new(),
            last_guidance: Guidance {
                items: Vec::new(),
                total: crate::reply::Total::Exact(0),
                end: None,
            },
        };
        let e = genesis(params, start);
        let mut t = Tracked::new();
        evolve_tracked(&mut w.s, &e, &mut t);
        w.log.push(e);
        w.dirty = t.facts;
        w.departed = t.departed;
        w
    }

    /// Moves the clock to `t` (never backwards). Settling is `apply`'s and `settle`'s (WP-D3).
    pub fn at(&mut self, t: Time) {
        self.clock = self.clock.max(t);
    }

    /// Applies `ev` at the clock without checks, and returns what it tracked.
    pub fn emit_raw(&mut self, ev: Ev) -> Tracked {
        let e = Event { at: self.clock, ev };
        let mut t = Tracked::new();
        evolve_tracked(&mut self.s, &e, &mut t);
        self.log.push(e);
        self.dirty = t.facts.clone();
        self.departed = t.departed.clone();
        t
    }

    /// Applies `ev` at the clock and checks it: no anomaly, every changed fact tracked, the indices
    /// equal to their recount at the clock, and the queue to its recount.
    ///
    /// # Errors
    ///
    /// What was wrong after the event; the event stays applied.
    pub fn try_emit(&mut self, ev: Ev) -> Result<(), Broken> {
        let before = self.s.clone();
        let dirty = self.emit_raw(ev);
        if self.s.anomalies != before.anomalies {
            return Err(Broken::Anomaly);
        }
        self.consistent(&before, &dirty)
    }

    /// The checks of `try_emit` after an event that may be an anomaly: the dirty set, R1, and T4.
    ///
    /// # Errors
    ///
    /// What was wrong.
    pub fn consistent(&self, before: &State, dirty: &Tracked) -> Result<(), Broken> {
        self.tracked(before, dirty)?;
        self.indices()
    }

    /// The dirty set of a job of one instant covers every fact that changed (D.12.3, item 11), and
    /// every entity that left memory in it is in `departed` with its value before the job (review
    /// E1-spec-1); every departed entity was resident before it (one may come back in the same
    /// event, by a `Restored` after the prune of its step 1, and the writer then writes its row
    /// resident).
    ///
    /// # Errors
    ///
    /// The facts that changed untracked, or the departures recorded wrong.
    pub fn tracked(&self, before: &State, t: &Tracked) -> Result<(), Broken> {
        let changed = changed_facts(before, &self.s);
        let untracked: Vec<Fact> = changed
            .iter()
            .filter(|f| !t.facts.contains(f))
            .cloned()
            .collect();
        if !untracked.is_empty() {
            return Err(Broken::Untracked(untracked));
        }
        let mut wrong: Vec<Fact> = changed
            .iter()
            .filter(|f| departed_of(&self.s, f).is_none())
            .filter_map(|f| departed_of(before, f).map(|v| (f, v)))
            .filter(|(f, v)| t.departed.get(*f) != Some(v))
            .map(|(f, _)| f.clone())
            .collect();
        wrong.extend(
            t.departed
                .iter()
                .filter(|(f, v)| departed_of(before, f).as_ref() != Some(*v))
                .map(|(f, _)| f.clone()),
        );
        if wrong.is_empty() {
            Ok(())
        } else {
            Err(Broken::Departed(wrong))
        }
    }

    /// The indices equal their recount at `last_now` (R1), and the queue its recount (T4). Between
    /// two firings at one instant the state is not yet settled (D.8.2), so this holds after the
    /// last of them.
    ///
    /// # Errors
    ///
    /// The indices that differ, or the queue.
    pub fn indices(&self) -> Result<(), Broken> {
        let diff = r1_differences(&self.s, self.s.last_now);
        if !diff.is_empty() {
            return Err(Broken::Indices(diff));
        }
        if self.s.ix.deadlines != recount_deadlines(&self.s) {
            return Err(Broken::Deadlines);
        }
        Ok(())
    }

    /// `try_emit`, panicking with what was wrong (for tests).
    ///
    /// # Panics
    ///
    /// When the event was an anomaly or left the state inconsistent.
    #[allow(clippy::panic, reason = "a test helper: a broken event fails the test")]
    pub fn emit(&mut self, ev: Ev) {
        let shown = alloc::format!("{ev:?}");
        if let Err(b) = self.try_emit(ev) {
            panic!("event {shown} at {:?}: {b:?}", self.clock);
        }
    }

    /// Every state invariant at the clock (D.9.2).
    ///
    /// # Errors
    ///
    /// The violations.
    pub fn check(&self) -> Result<(), Vec<Violation>> {
        check_invariants(&self.s, self.clock)
    }

    /// The state folded from Genesis through every event of the log (D.12.3, item 5).
    #[must_use]
    pub fn replay(&self) -> State {
        let mut s = State::default();
        for e in &self.log {
            crate::evolve::evolve_mut(&mut s, e);
        }
        s
    }

    // --------------------------------------------------------------------------------------------
    // The shell's pipeline (WP-D3)

    /// Settles to the clock one firing at a time, as the shell's pre- and post-settle do (D.8.1),
    /// checking each firing with `check_fire` and its dirty set (D.12.3, item 11). Returns the
    /// events.
    ///
    /// # Panics
    ///
    /// On a `Stuck` key, a violated step check, or an untracked fact.
    #[allow(
        clippy::panic,
        reason = "a test helper: a broken firing fails the test"
    )]
    pub fn settle(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(key) = self.s.ix.deadlines.first().cloned() {
            if key.at > self.clock {
                break;
            }
            let before = self.s.clone();
            let mut dirty = Tracked::new();
            let (evs, st) = settle_tracked(&mut self.s, self.clock, 1, &mut dirty);
            if let Settled::Stuck(k) = st {
                panic!("settle is stuck at {k:?}");
            }
            if let Err(v) = check_fire(&before, &key, &evs, &self.s) {
                panic!("check_fire of {key:?}: {v:?}\nevents: {evs:?}");
            }
            if let Err(b) = self.tracked(&before, &dirty) {
                panic!("firing {key:?}: {b:?}");
            }
            self.log.extend(evs.iter().cloned());
            out.extend(evs);
        }
        out
    }

    /// Moves the clock by `d` and settles (D.12.1).
    pub fn advance(&mut self, d: Dur) -> Vec<Event> {
        self.clock = self.clock.plus(d);
        self.settle()
    }

    /// The context of a call of `class` in `session` at the clock.
    #[must_use]
    pub fn ctx(&self, class: &ClassName, session: SessionCtx, op: Option<OpCtx>) -> Ctx {
        Ctx {
            now: self.clock,
            class: class.clone(),
            caps: None,
            session,
            op,
        }
    }

    /// One call through the shell's pipeline (D.8.1): the pre-settle, `apply_tracked` with
    /// `check_step`, the dirty-set check, and `decide == apply`, the post-settle, and every state
    /// invariant with R1 at the clock. Returns the outcome; `last_events` holds the command's
    /// events.
    ///
    /// # Panics
    ///
    /// On any violated check.
    #[allow(clippy::panic, reason = "a test helper: a broken step fails the test")]
    pub fn call_ctx(&mut self, ctx: &Ctx, cmd: &Cmd) -> Result<Done, Error> {
        let mut ctx = ctx.clone();
        self.clock = self.clock.max(ctx.now);
        ctx.now = self.clock;
        self.settle();
        let before = self.s.clone();
        let (pure_events, pure_out) = decide(&before, &ctx, cmd);
        let mut dirty = Tracked::new();
        let (evs, out) = apply_tracked(&mut self.s, &ctx, cmd, &mut dirty);
        if (pure_events.clone(), pure_out.clone()) != (evs.clone(), out.clone()) {
            panic!("decide and apply differ on {cmd:?}");
        }
        if let Err(v) = check_step(&before, &ctx, cmd, &out, &evs, &self.s) {
            panic!("check_step of {cmd:?}: {v:?}\noutcome: {out:?}\nevents: {evs:?}");
        }
        if let Err(b) = self.tracked(&before, &dirty) {
            panic!("command {cmd:?}: {b:?}");
        }
        let mut folded = before;
        for e in &evs {
            crate::evolve::evolve_mut(&mut folded, e);
        }
        if folded != self.s {
            panic!("the state after {cmd:?} is not the fold of its events");
        }
        self.log.extend(evs.iter().cloned());
        self.last_events = evs;
        if let Ok(d) = &out
            && let Some(x) = d.opened
        {
            self.sessions.push((ctx.class.clone(), x));
        }
        self.settle();
        if let Err(v) = self.check() {
            panic!("invariants after {cmd:?}: {v:?}");
        }
        if let Err(b) = self.indices() {
            panic!("indices after {cmd:?}: {b:?}");
        }
        // The next calls, on the settled state, as the writer computes them, with G1 (D.8.26).
        let sess = guide_session(&ctx, &out, &self.last_events);
        let g = crate::query::guide(&self.s, sess, self.clock, cmd, &out, &mut NoMeter);
        if let Err(v) = check_guide(&self.s, sess, self.clock, cmd, &out, &g) {
            panic!("check_guide after {cmd:?} -> {out:?}: {v:?}\nnext calls: {g:?}");
        }
        self.last_guidance = g;
        out
    }

    /// The next calls of the last response (D.12.1).
    #[must_use]
    pub fn last_guidance(&self) -> &Guidance {
        &self.last_guidance
    }

    /// `call_ctx` for `class` in `session`, without an `op_id`.
    pub fn call(&mut self, class: &ClassName, session: SessionId, cmd: Cmd) -> Result<Done, Error> {
        let ctx = self.ctx(class, SessionCtx::Existing(session), None);
        self.call_ctx(&ctx, &cmd)
    }

    /// `call` for the class that opened `session` in this world.
    ///
    /// # Panics
    ///
    /// When the world did not open `session`.
    #[allow(clippy::panic, reason = "a test helper")]
    pub fn call_as(&mut self, session: SessionId, cmd: Cmd) -> Result<Done, Error> {
        let Some((class, _)) = self.sessions.iter().find(|(_, x)| *x == session).cloned() else {
            panic!("no session {session:?} in this world");
        };
        self.call(&class, session, cmd)
    }

    /// Opens a session of `class` with `session hello {}` through the pipeline (D.12.1).
    ///
    /// # Panics
    ///
    /// When the `hello` fails.
    #[allow(clippy::panic, reason = "a test helper")]
    pub fn hello(&mut self, class: &ClassName) -> SessionId {
        let ctx = self.ctx(class, SessionCtx::New, None);
        let cmd = Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: None,
            from_user: None,
        });
        match self.call_ctx(&ctx, &cmd) {
            Ok(d) => d
                .opened
                .unwrap_or_else(|| panic!("hello opened no session: {d:?}")),
            Err(e) => panic!("hello failed: {e:?}"),
        }
    }

    // --------------------------------------------------------------------------------------------
    // Event-level builders, for states the commands of this layer cannot reach yet

    /// Opens a session of `class` by its event alone, with no caps.
    pub fn open(&mut self, class: &ClassName) -> SessionId {
        let session = self.s.counters.next_session;
        let at = self.clock;
        let p = self.s.params.clone();
        self.emit(Ev::SessionOpened {
            session,
            class: class.clone(),
            caps: BTreeSet::new(),
            horizon: None,
            intent: None,
            silence_at: at.plus(p.presence_silence),
            retire_at: at.plus(p.session_retain),
            principal: None,
        });
        self.sessions.push((class.clone(), session));
        session
    }

    /// Creates an agent node `name` under ROOT by its event alone (capacity 1, no tags).
    ///
    /// # Panics
    ///
    /// For a name that is not valid.
    #[allow(clippy::expect_used, reason = "a test helper")]
    pub fn plain_node(&mut self, name: &str) -> NodeId {
        let id = self.s.counters.next_node;
        self.emit(Ev::NodeCreated {
            id,
            parent: ROOT,
            name: Name::new(name).expect("a valid name"),
            origin: Origin::Agent(SessionId(0)),
            init: NodeInit {
                title: Text(alloc::string::String::from(name)),
                body: Text::default(),
                fields: alloc::collections::BTreeMap::new(),
                status: Text::default(),
                digest: Text::default(),
                agreements: Text::default(),
                kind: None,
                tags: BTreeSet::new(),
                capacity: 1,
                priority: 0,
                est: None,
                needs: BTreeSet::new(),
                due: None,
                depends_on: BTreeSet::new(),
                owners: BTreeSet::new(),
                keys: alloc::collections::BTreeMap::new(),
                links: BTreeSet::new(),
                role: Role::Plain,
            },
        });
        id
    }

    /// Sets a node's needs by its event alone.
    pub fn set_needs(&mut self, n: NodeId, needs: BTreeSet<Label>) {
        self.emit(Ev::NodeUpdated {
            node: n,
            patch: NodePatch {
                needs: Some(needs),
                ..NodePatch::default()
            },
        });
    }
}

/// The session `guide` reads for a call (D.8.26, as the writer passes it): the call's session, or
/// the one a first call opened, whether or not it succeeded; None when the call failed at the
/// session check (`not_found(Session)`, `invalid(ClassMismatch)`), or ran as the server.
#[must_use]
pub fn guide_session(ctx: &Ctx, out: &Result<Done, Error>, events: &[Event]) -> Option<SessionId> {
    if matches!(
        out,
        Err(Error {
            hint: crate::error::Hint::NotFound(crate::error::What::Session)
                | crate::error::Hint::Invalid(crate::error::Why::ClassMismatch),
            ..
        })
    ) {
        return None;
    }
    match ctx.session {
        crate::cmd::SessionCtx::Existing(x) => Some(x),
        crate::cmd::SessionCtx::New => out.as_ref().ok().and_then(|d| d.opened).or_else(|| {
            events.iter().find_map(|e| {
                if let Ev::SessionOpened { session, .. } = &e.ev {
                    Some(*session)
                } else {
                    None
                }
            })
        }),
        crate::cmd::SessionCtx::System => None,
    }
}
