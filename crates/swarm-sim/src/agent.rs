//! A simulated agent: one slot of the population that runs sessions back to back, every session a
//! new agent with a new MCP session (§1, D.15.2). A session is a worker (it takes work with `hold
//! next`, reads, checkpoints, writes, and finishes with a close or a release) or a waiter (it parks
//! in `talk wait`); one in ten serves a principal; and a share of sessions vanish mid-task without
//! a word, leaving their holds to expire (D.15.1: half of all sessions).
//!
//! **No coordinated omission.** Every call has an intended time, drawn from the profile's rate as a
//! Poisson process; the agent sleeps until then and sends it, and the latency recorded runs from the
//! intended time. A call that comes back late does not move the next call's intended time, so a
//! stall of the server shows in the latency of every call that should have been sent during it.
//! When the run stops, an agent still sends the calls it intended before the stop, and a call
//! still in flight when the run gives up on it is recorded as late ([`Shared::in_flight`]). A
//! parked wait is the exception: it is a long poll, sent when the previous one returns.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::clock::{Instant, now};
use crate::mcp::{Called, Mcp, Outcome};
use crate::mix::{self, Action, Profile};
use crate::record::{Category, Recorder, Sample, StubPolicy};
use crate::rng::Rng;
use crate::text;

/// Where the simulated work lives: `/bench/w/a<i>/b<j>/n<k>`, at most 50 × 50 parents, each with
/// fewer open children than `tree.max_children`.
pub const ROOT: &str = "/bench";
const FANOUT: u64 = 50;
/// A checkpoint is due 5 minutes after the last (§20.9); agents make it a little before.
const CHECKPOINT_EVERY_MIN: f64 = 4.5;
/// A well-behaved agent stays below `write.per_hour` (120 per session, §22).
const WRITES_PER_HOUR: usize = 110;
/// Known node paths kept for `take`, `view`, and `find`.
const BACKLOG_KEPT: usize = 100_000;
/// Known keys of nodes, kept for the creates that join one.
const KEYS_KEPT: usize = 10_000;
/// How many times a session draws again when it drew a stub under `--stubs skip`.
const REDRAWS: usize = 16;
/// `hold.per_session` (§22): the holds a session may have at once.
const HOLDS: usize = 2;

/// A call in flight: its kind, latency class, intended time, and when it was sent.
type InFlight = (&'static str, Category, Instant, Instant);

/// What every agent of a run shares.
pub struct Shared {
    /// The MCP endpoint.
    pub url: String,
    /// Bearer tokens, one per class; agent `i` uses token `i % n`.
    pub tokens: Vec<String>,
    /// The run's seed.
    pub seed: u64,
    /// The profile.
    pub profile: Profile,
    /// How many times as fast as real time the server's clock runs (1: real time).
    pub scale: f64,
    /// The recorder.
    pub rec: Recorder,
    /// When the run started.
    pub start: Instant,
    /// Set when the run ends: agents finish their current call and stop.
    pub stop: AtomicBool,
    mult: AtomicU64,
    herd: Mutex<Option<(u64, Instant, Duration)>>,
    herd_tx: tokio::sync::watch::Sender<u64>,
    backlog: Mutex<VecDeque<String>>,
    known_keys: Mutex<VecDeque<String>>,
    keys: AtomicU64,
    /// When the run stopped: calls intended before it are still sent.
    stop_at: Mutex<Option<Instant>>,
    /// The call each agent has in flight, by agent.
    flying: Mutex<BTreeMap<u64, InFlight>>,
    /// What to do with commands that are stubs at the commit the driver runs against.
    pub stubs: StubPolicy,
    /// The share of sessions that park in `talk wait`.
    pub parked_share: f64,
    /// The share of sessions that serve a principal.
    pub principal_share: f64,
    /// How many principals there are.
    pub principals: usize,
    /// The share of sessions that vanish mid-task.
    pub drop_share: f64,
    /// A node that waiters wait `for` (G-4's hot node), if any.
    pub wait_for: Mutex<Option<String>>,
    /// The `timeout` of a wait, in real time.
    pub wait_timeout: Duration,
    /// Sessions running now.
    pub live: AtomicUsize,
    /// Waits in flight now.
    pub parked: AtomicUsize,
    /// Sessions started.
    pub sessions: AtomicU64,
    /// Sessions that vanished mid-task.
    pub vanished: AtomicU64,
}

/// The run settings an agent needs.
pub struct Settings {
    /// The MCP endpoint.
    pub url: String,
    /// Bearer tokens.
    pub tokens: Vec<String>,
    /// The seed.
    pub seed: u64,
    /// The profile.
    pub profile: Profile,
    /// The server's clock compression.
    pub scale: f64,
    /// The stub policy.
    pub stubs: StubPolicy,
    /// The share of parked sessions.
    pub parked_share: f64,
    /// The share of sessions serving a principal.
    pub principal_share: f64,
    /// The number of principals.
    pub principals: usize,
    /// The share of sessions that vanish.
    pub drop_share: f64,
    /// The first number for fresh paths and keys: a run on a database seeded by an earlier run
    /// starts past the seed's, so that its creates make new work rather than joins.
    pub first_fresh: u64,
}

impl Shared {
    /// The shared state of a run starting now, its first phase named `phase`.
    #[must_use]
    pub fn new(s: Settings, phase: &str) -> Shared {
        let start = now();
        // talk.wait_max is 50 s of the forum's time (§22); a wait asks for that, in real time.
        let wait = Duration::from_secs_f64((50.0 / s.scale).max(1.0));
        Shared {
            url: s.url,
            tokens: s.tokens,
            seed: s.seed,
            profile: s.profile,
            scale: s.scale.max(1.0),
            rec: Recorder::new(start, s.stubs, phase),
            start,
            stop: AtomicBool::new(false),
            mult: AtomicU64::new(1.0_f64.to_bits()),
            herd: Mutex::new(None),
            herd_tx: tokio::sync::watch::Sender::new(0),
            backlog: Mutex::new(VecDeque::new()),
            known_keys: Mutex::new(VecDeque::new()),
            keys: AtomicU64::new(s.first_fresh),
            stop_at: Mutex::new(None),
            flying: Mutex::new(BTreeMap::new()),
            stubs: s.stubs,
            parked_share: s.parked_share,
            principal_share: s.principal_share,
            principals: s.principals.max(1),
            drop_share: s.drop_share,
            wait_for: Mutex::new(None),
            wait_timeout: wait,
            live: AtomicUsize::new(0),
            parked: AtomicUsize::new(0),
            sessions: AtomicU64::new(0),
            vanished: AtomicU64::new(0),
        }
    }

    /// The rate multiplier now: the mix's (1 or 2.5) times a burst's (1 or 3).
    #[must_use]
    pub fn mult(&self) -> f64 {
        f64::from_bits(self.mult.load(Ordering::Relaxed))
    }

    /// Sets the rate multiplier.
    pub fn set_mult(&self, m: f64) {
        self.mult.store(m.to_bits(), Ordering::Relaxed);
    }

    /// Starts a herd: every agent calls `control health` once at a random time within `window`.
    pub fn herd(&self, window: Duration) {
        let mut h = lock(&self.herd);
        let generation = h.as_ref().map_or(1, |x| x.0 + 1);
        *h = Some((generation, now(), window));
        // Wakes the agents that are waiting for their next call, so that they hear of it now.
        self.herd_tx.send_replace(generation);
    }

    fn herd_due(&self, seen: &mut u64, rng: &mut Rng) -> Option<Instant> {
        let h = lock(&self.herd);
        let (generation, at, window) = (*h)?;
        if generation <= *seen {
            return None;
        }
        *seen = generation;
        Some(at + window.mul_f64(rng.unit()))
    }

    /// Remembers an open node's path.
    pub fn known(&self, path: String) {
        let mut b = lock(&self.backlog);
        if b.len() >= BACKLOG_KEPT {
            b.pop_front();
        }
        b.push_back(path);
    }

    /// A known node's path, recent ones more likely.
    fn some_node(&self, rng: &mut Rng) -> Option<String> {
        let b = lock(&self.backlog);
        if b.is_empty() {
            return None;
        }
        let n = b.len();
        let i = n - 1 - rng.zipf(n);
        b.get(i).cloned()
    }

    /// Remembers a key that a node carries, for a create that joins it.
    pub fn known_key(&self, key: String) {
        let mut b = lock(&self.known_keys);
        if b.len() >= KEYS_KEPT {
            b.pop_front();
        }
        b.push_back(key);
    }

    /// A known key, recent ones more likely.
    fn some_key(&self, rng: &mut Rng) -> Option<String> {
        let b = lock(&self.known_keys);
        if b.is_empty() {
            return None;
        }
        let n = b.len();
        b.get(n - 1 - rng.zipf(n)).cloned()
    }

    /// Every known key, oldest first.
    #[must_use]
    pub fn known_key_list(&self) -> Vec<String> {
        lock(&self.known_keys).iter().cloned().collect()
    }

    /// Known paths.
    #[must_use]
    pub fn backlog_len(&self) -> usize {
        lock(&self.backlog).len()
    }

    /// Every known path, oldest first.
    #[must_use]
    pub fn known_paths(&self) -> Vec<String> {
        lock(&self.backlog).iter().cloned().collect()
    }

    /// A fresh number for a path or a key.
    pub fn fresh(&self) -> u64 {
        self.keys.fetch_add(1, Ordering::Relaxed)
    }

    /// The next fresh number, without taking it: where a later run on the same database starts.
    #[must_use]
    pub fn next_fresh(&self) -> u64 {
        self.keys.load(Ordering::Relaxed)
    }

    /// A real-time duration for `minutes` of the forum's time.
    #[must_use]
    pub fn real(&self, minutes: f64) -> Duration {
        Duration::from_secs_f64((minutes * 60.0 / self.scale).max(0.0))
    }

    /// Whether the run is ending.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Stops the run at `at`: agents send the calls they intended before `at`, and no other.
    pub fn stop_at(&self, at: Instant) {
        *lock(&self.stop_at) = Some(at);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Whether a call intended at `at` is past the run's end.
    #[must_use]
    pub fn ended(&self, at: Instant) -> bool {
        self.stopped() && lock(&self.stop_at).is_none_or(|s| at >= s)
    }

    /// The calls in flight now, which a run that gives up on them records as late.
    #[must_use]
    pub fn in_flight(&self) -> Vec<InFlight> {
        lock(&self.flying).values().copied().collect()
    }

    /// A path for new work.
    pub fn new_path(&self, rng: &mut Rng) -> String {
        let n = self.fresh();
        format!(
            "{ROOT}/w/a{}/b{}/n{n}",
            rng.below(FANOUT),
            rng.below(FANOUT)
        )
    }
}

/// Agents made in this process, for their `op_id` streams.
static NEXT_AGENT: AtomicU64 = AtomicU64::new(0);

/// The process's salt for `op_id`s: the wall clock at its first use, so that a later run against
/// the same database (a restart in G-7 or G-8) draws other ids.
fn id_salt() -> u64 {
    static SALT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SALT.get_or_init(crate::clock::unix_millis)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// One of the agent's holds.
#[derive(Clone, Debug)]
struct Held {
    path: String,
    token: String,
    version: Option<u64>,
    last_checkpoint: Instant,
}

/// The kind of session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Takes work and does it.
    Worker,
    /// Parks in `talk wait`.
    Waiter,
}

/// One simulated agent slot.
pub struct Agent {
    sh: Arc<Shared>,
    /// Its number within the process, for the calls in flight.
    id: u64,
    rng: Rng,
    /// Draws `op_id`s: salted by the process and the agent, so that no two agents, runs, or
    /// restarts share one (§20.1: a repeat from another session is a conflict).
    ids: Rng,
    mcp: Mcp,
    session: Option<String>,
    principal: Option<String>,
    holds: Vec<Held>,
    seen: Vec<String>,
    writes: VecDeque<Instant>,
    herd_seen: u64,
    herd_rx: tokio::sync::watch::Receiver<u64>,
    pending_herd: Option<Instant>,
    gone: bool,
}

fn tool_of(a: Action) -> (&'static str, &'static str) {
    match a {
        Action::Hello | Action::HelloSearch => ("session", "hello"),
        Action::Status => ("session", "status"),
        Action::Next => ("hold", "next"),
        Action::Take => ("hold", "take"),
        Action::Check => ("hold", "check"),
        Action::Release => ("hold", "release"),
        Action::Renew => ("hold", "renew"),
        Action::Checkpoint => ("hold", "checkpoint"),
        Action::ViewNode => ("view", "node"),
        Action::FindText | Action::FindKey => ("view", "find"),
        Action::List => ("view", "list"),
        Action::Feed => ("view", "feed"),
        Action::History => ("view", "history"),
        Action::Requests => ("view", "requests"),
        Action::UserLog => ("view", "user_log"),
        Action::Health => ("control", "health"),
        Action::Pause => ("control", "pause"),
        Action::Unpause => ("control", "unpause"),
        Action::Create | Action::CreateJoin => ("node", "create"),
        Action::Update => ("node", "update"),
        Action::Close => ("node", "close"),
        Action::Key => ("node", "key"),
        Action::Link => ("node", "link"),
        Action::Say | Action::SayHolders => ("talk", "say"),
        Action::Decide => ("talk", "decide"),
        Action::Said => ("talk", "said"),
        Action::Heard => ("talk", "heard"),
        Action::AskAgents | Action::AskUser => ("talk", "ask"),
        Action::FromUser => ("talk", "from_user"),
        Action::ToUser => ("talk", "to_user"),
        Action::Wait => ("talk", "wait"),
        Action::Who => ("talk", "who"),
    }
}

impl Agent {
    /// Agent `index` of the run.
    ///
    /// # Errors
    ///
    /// Its HTTP client could not be built.
    pub fn new(sh: Arc<Shared>, index: usize, request_timeout: Duration) -> Result<Agent, String> {
        let token = sh
            .tokens
            .get(index % sh.tokens.len().max(1))
            .cloned()
            .ok_or("no token")?;
        let mcp = Mcp::new(&sh.url, &token, request_timeout)?;
        let rng = Rng::new(sh.seed, u64::try_from(index).unwrap_or(0));
        let id = NEXT_AGENT.fetch_add(1, Ordering::Relaxed);
        let ids = Rng::new(id_salt(), id);
        let herd_rx = sh.herd_tx.subscribe();
        Ok(Agent {
            sh,
            id,
            rng,
            ids,
            mcp,
            session: None,
            principal: None,
            holds: Vec::new(),
            seen: Vec::new(),
            writes: VecDeque::new(),
            herd_seen: 0,
            herd_rx,
            pending_herd: None,
            gone: false,
        })
    }

    /// Runs sessions back to back until the run stops.
    pub async fn run(mut self) {
        while !self.sh.stopped() {
            let role = if self.rng.chance(self.sh.parked_share) {
                Role::Waiter
            } else {
                Role::Worker
            };
            self.session_of(role).await;
        }
    }

    /// One session of `role`, from a new MCP session to its end or its vanishing.
    pub async fn session_of(&mut self, role: Role) {
        let sh = Arc::clone(&self.sh);
        let len = sh.real(sh.profile.session_min * self.rng.between(0.5, 1.5));
        let begin = now();
        let end = begin + len;
        let vanish = self
            .rng
            .chance(sh.drop_share)
            .then(|| begin + len.mul_f64(self.rng.between(0.2, 0.9)));
        self.session = None;
        self.holds.clear();
        self.writes.clear();
        self.gone = false;
        self.pending_herd = None;
        self.herd_rx.mark_unchanged();
        self.principal = self.rng.chance(sh.principal_share).then(|| {
            format!(
                "mail:p{:04}@example.org",
                self.rng.below(u64::try_from(sh.principals).unwrap_or(1))
            )
        });
        let t = now();
        let init = self.mcp.initialize().await;
        let done = now();
        let outcome = match &init {
            Ok(()) => Outcome::Ok,
            Err(e) => Outcome::Transport(e.clone()),
        };
        sh.rec.record(&Sample {
            kind: "mcp.initialize",
            category: crate::record::Category::Init,
            intended: t,
            sent: t,
            done,
            outcome: &outcome,
            expected: &[],
        });
        if init.is_err() {
            // A server that refuses connections: try again a little later.
            tokio::time::sleep(Duration::from_millis(500)).await;
            return;
        }
        sh.sessions.fetch_add(1, Ordering::Relaxed);
        sh.live.fetch_add(1, Ordering::Relaxed);
        let at = now();
        self.act(Action::Hello, at).await;
        if self.session.is_some() {
            match role {
                Role::Worker => self.work(at, end, vanish).await,
                Role::Waiter => self.wait_loop(end, vanish).await,
            }
        }
        sh.live.fetch_sub(1, Ordering::Relaxed);
    }

    async fn work(&mut self, mut at: Instant, end: Instant, vanish: Option<Instant>) {
        loop {
            at += self.think();
            // A call intended before the run stopped is still sent: a backlog that a slow server
            // built up must show in the latencies (no coordinated omission at the end).
            if self.sh.ended(at) || self.gone {
                return;
            }
            if vanish.is_some_and(|v| at >= v) {
                self.sh.vanished.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if at >= end {
                break;
            }
            self.until(at).await;
            if self.sh.ended(at) || self.gone {
                return;
            }
            let a = self.choose(at);
            self.act(a, at).await;
        }
        // A clean end: finish or hand off what it holds (a call that fails is tried once more).
        for _ in 0..self.holds.len() * 2 {
            if self.holds.is_empty() || self.gone || self.sh.stopped() {
                break;
            }
            let a = if self.rng.chance(0.6) {
                Action::Close
            } else {
                Action::Release
            };
            let t = now();
            self.act(a, t).await;
        }
    }

    /// Sleeps until `at`; a herd that starts meanwhile is heard at once, and its `control health`
    /// call is made at its own intended time when that comes first.
    async fn until(&mut self, at: Instant) {
        loop {
            if let Some(h) = self.pending_herd
                && h <= at
            {
                self.pending_herd = None;
                tokio::time::sleep_until(h).await;
                self.act(Action::Health, h).await;
                continue;
            }
            if self.sh.stopped() {
                return;
            }
            // At most a second at a time, so that the end of a run is heard.
            let step = at.min(now() + Duration::from_secs(1));
            tokio::select! {
                () = tokio::time::sleep_until(step) => {
                    if step >= at {
                        return;
                    }
                }
                changed = self.herd_rx.changed() => {
                    if changed.is_err() {
                        tokio::time::sleep_until(at).await;
                        return;
                    }
                    if let Some(h) = self.sh.herd_due(&mut self.herd_seen, &mut self.rng) {
                        self.pending_herd = Some(h);
                    }
                }
            }
        }
    }

    async fn wait_loop(&mut self, end: Instant, vanish: Option<Instant>) {
        let mut k = 0_u32;
        while now() < end && !self.sh.stopped() && !self.gone {
            if vanish.is_some_and(|v| now() >= v) {
                self.sh.vanished.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if let Some(h) = self.sh.herd_due(&mut self.herd_seen, &mut self.rng) {
                // A parked agent hears of the herd when its wait returns.
                let t = h.max(now());
                tokio::time::sleep_until(t).await;
                self.act(Action::Health, t).await;
            }
            k += 1;
            let a = if k.is_multiple_of(mix::WAITER_READ_EVERY)
                || (self.sh.stubs == StubPolicy::Skip && Action::Wait.stubbed_at_writing())
            {
                // A read drawn from the mix's reads, at the profile's pace.
                tokio::time::sleep(self.think()).await;
                self.a_read(&[])
            } else {
                Action::Wait
            };
            let t = now();
            self.act(a, t).await;
        }
    }

    /// The time to the next call: exponential, with the profile's mean at the current rate.
    fn think(&mut self) -> Duration {
        let rate = self.sh.profile.calls_per_min * self.sh.mult();
        let mean_s = 60.0 / rate.max(1e-9) / self.sh.scale;
        Duration::from_secs_f64(self.rng.exp(mean_s).min(3_600.0))
    }

    fn writes_this_hour(&mut self, at: Instant) -> usize {
        let hour = self.sh.real(60.0);
        while self
            .writes
            .front()
            .is_some_and(|t| at.saturating_duration_since(*t) > hour)
        {
            self.writes.pop_front();
        }
        self.writes.len()
    }

    /// The next action: a checkpoint when one is due, else a draw from the mix, adjusted to what
    /// the session holds as an agent would: work it cannot do without a hold makes it take work
    /// first (`next`), and work it cannot take with `hold.per_session` (2) held makes it finish or
    /// hand off one first (`close` or `release`). A write past `write.per_hour` becomes a read of
    /// the mix, and a stub under `--stubs skip` is drawn again.
    fn choose(&mut self, at: Instant) -> Action {
        let due = self.sh.real(CHECKPOINT_EVERY_MIN);
        if let Some(i) = self
            .holds
            .iter()
            .position(|h| at.saturating_duration_since(h.last_checkpoint) >= due)
        {
            // The due one first.
            self.holds.swap(0, i);
            return Action::Checkpoint;
        }
        let extra = if self.principal.is_some() {
            mix::PRINCIPAL
        } else {
            &[]
        };
        let total = mix::total(mix::WORKER, extra);
        let mut a = Action::ViewNode;
        for _ in 0..REDRAWS {
            let pick = u32::try_from(self.rng.below(u64::from(total))).unwrap_or(0);
            a = mix::draw(mix::WORKER, extra, pick);
            if !(self.sh.stubs == StubPolicy::Skip && a.stubbed_at_writing()) {
                break;
            }
        }
        if a.needs_hold() && self.holds.is_empty() {
            a = Action::Next;
        } else if a.acquires() && self.holds.len() >= HOLDS {
            a = if self.rng.chance(0.6) {
                Action::Close
            } else {
                Action::Release
            };
        }
        if (a.category() == Category::Write && self.writes_this_hour(at) >= WRITES_PER_HOUR)
            || (self.sh.stubs == StubPolicy::Skip && a.stubbed_at_writing())
        {
            a = self.a_read(extra);
        }
        a
    }

    /// A read drawn from the mix's reads.
    fn a_read(&mut self, extra: &[(Action, u32)]) -> Action {
        let reads = mix::reads(mix::WORKER, extra);
        let pick = u32::try_from(self.rng.below(u64::from(mix::total(&reads, &[])))).unwrap_or(0);
        mix::draw(&reads, &[], pick)
    }

    fn a_path(&mut self) -> String {
        if !self.holds.is_empty() && self.rng.chance(0.5) {
            let i = self.rng.index(self.holds.len());
            return self.holds[i].path.clone();
        }
        if !self.seen.is_empty() && self.rng.chance(0.3) {
            let i = self.rng.index(self.seen.len());
            return self.seen[i].clone();
        }
        self.sh
            .some_node(&mut self.rng)
            .unwrap_or_else(|| ROOT.to_string())
    }

    fn see(&mut self, path: &str) {
        if self.seen.len() >= 16 {
            self.seen.remove(0);
        }
        self.seen.push(path.to_string());
    }

    /// The arguments of `a`, or None when it cannot be made now.
    fn args(&mut self, a: Action) -> Option<Map<String, Value>> {
        let mut m = Map::new();
        let (_, action) = tool_of(a);
        m.insert("action".into(), action.into());
        let held = self.holds.first().cloned();
        match a {
            Action::Hello | Action::HelloSearch => {
                m.insert("intent".into(), text::words(&mut self.rng, 6).into());
                m.insert("horizon".into(), 60.into());
                if let Some(p) = &self.principal {
                    m.insert("principal".into(), p.clone().into());
                }
            }
            Action::Next | Action::Health | Action::Requests | Action::UserLog => {}
            Action::Take => {
                let p = self.sh.some_node(&mut self.rng)?;
                m.insert("target".into(), p.into());
            }
            Action::ViewNode | Action::History | Action::Check => {
                let p = self.a_path();
                let key = if a == Action::Check { "target" } else { "path" };
                m.insert(key.into(), p.into());
            }
            Action::Feed => {
                let p = self.a_path();
                m.insert("at".into(), p.into());
                m.insert("limit".into(), 20.into());
            }
            Action::FindText => {
                m.insert("text".into(), text::words(&mut self.rng, 3).into());
            }
            Action::FindKey => {
                let n = self.sh.keys.load(Ordering::Relaxed).max(1);
                m.insert(
                    "key".into(),
                    format!("issue:bench#{}", self.rng.below(n)).into(),
                );
            }
            Action::List => {
                let under = format!("{ROOT}/w/a{}", self.rng.below(FANOUT));
                m.insert("under".into(), under.into());
                m.insert("limit".into(), 20.into());
            }
            Action::Create | Action::CreateJoin => {
                // A join names a key that a node carries; with none known yet, new work.
                let join = if a == Action::CreateJoin {
                    Some(self.sh.some_key(&mut self.rng)?)
                } else {
                    None
                };
                let n = self.sh.fresh();
                let path = format!(
                    "{ROOT}/w/a{}/b{}/n{n}",
                    self.rng.below(FANOUT),
                    self.rng.below(FANOUT)
                );
                m.insert("path".into(), path.into());
                m.insert("title".into(), text::words(&mut self.rng, 5).into());
                m.insert("body".into(), text::words(&mut self.rng, 40).into());
                m.insert("est".into(), (15 + self.rng.below(46)).into());
                if let Some(k) = join {
                    m.insert("keys".into(), json!([k]));
                } else if self.rng.chance(0.5) {
                    m.insert("keys".into(), json!([format!("issue:bench#{n}")]));
                }
            }
            Action::Update => {
                let h = held?;
                m.insert("path".into(), h.path.into());
                m.insert("version".into(), h.version.unwrap_or(1).into());
                m.insert("body".into(), text::words(&mut self.rng, 40).into());
                m.insert("token".into(), h.token.into());
            }
            Action::Close => {
                let h = held?;
                m.insert("path".into(), h.path.into());
                m.insert("outcome".into(), "done".into());
                m.insert("summary".into(), text::words(&mut self.rng, 10).into());
                m.insert("token".into(), h.token.into());
            }
            Action::Release => {
                let h = held?;
                m.insert("token".into(), h.token.into());
                m.insert("done".into(), text::words(&mut self.rng, 8).into());
                m.insert("next_step".into(), text::words(&mut self.rng, 6).into());
                m.insert("reason".into(), "handoff".into());
            }
            Action::Renew => {
                let h = held?;
                m.insert("token".into(), h.token.into());
                m.insert("why".into(), text::words(&mut self.rng, 4).into());
            }
            Action::Checkpoint => {
                let h = held?;
                m.insert("token".into(), h.token.into());
                m.insert("done".into(), text::words(&mut self.rng, 8).into());
                m.insert("next_step".into(), text::words(&mut self.rng, 6).into());
            }
            Action::Say | Action::AskAgents => {
                let p = self.a_path();
                m.insert("at".into(), p.clone().into());
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
                m.insert("to".into(), format!("holders:{p}/**").into());
            }
            Action::SayHolders => {
                m.insert("at".into(), ROOT.into());
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
                m.insert("to".into(), "holders:/**".into());
            }
            Action::Decide => {
                let h = held?;
                m.insert("at".into(), h.path.into());
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
            }
            Action::Said => {
                let p = self.a_path();
                m.insert("at".into(), p.into());
                m.insert(
                    "where".into(),
                    format!("chat:bench/t{}", self.rng.below(1_000)).into(),
                );
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
                m.insert("kind".into(), "reply".into());
            }
            Action::Heard => {
                let p = self.a_path();
                m.insert("at".into(), p.into());
                // As `said`: a conversation key, or a node with none refuses the call
                // (`invalid(NoConversationKey)`, §9.1).
                m.insert(
                    "where".into(),
                    format!("chat:bench/t{}", self.rng.below(1_000)).into(),
                );
                m.insert("from".into(), "mail:someone@example.org".into());
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
            }
            Action::Key => {
                let p = self.a_path();
                let n = self.sh.fresh();
                m.insert("path".into(), p.into());
                m.insert("add".into(), format!("issue:bench#{n}").into());
            }
            Action::Link => {
                let p = self.a_path();
                // Another node: a link to itself is refused (`invalid`, since WP-D1.4 made `node
                // link` real), and G-1 counted it as an unexpected error.
                let q = (0..8).map(|_| self.a_path()).find(|q| *q != p)?;
                m.insert("path".into(), p.into());
                m.insert("rel".into(), "derived_from".into());
                m.insert("to".into(), q.into());
            }
            Action::Status => {
                m.insert("text".into(), text::words(&mut self.rng, 6).into());
            }
            Action::Who => {
                if self.rng.chance(0.5) {
                    let p = self.a_path();
                    m.insert("at".into(), p.into());
                }
            }
            Action::FromUser => {
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
                m.insert("client".into(), "ide".into());
            }
            Action::ToUser => {
                m.insert("text".into(), text::words(&mut self.rng, 12).into());
                m.insert("kind".into(), "result".into());
                m.insert("verbatim".into(), true.into());
            }
            Action::AskUser => {
                let p = self.a_path();
                m.insert("at".into(), p.into());
                m.insert("text".into(), text::words(&mut self.rng, 10).into());
                m.insert("to".into(), "user".into());
                m.insert("options".into(), json!(["yes", "no"]));
            }
            Action::Wait => {
                m.insert(
                    "timeout".into(),
                    format!("{}s", self.sh.wait_timeout.as_secs().max(1)).into(),
                );
                if let Some(n) = lock(&self.sh.wait_for).clone() {
                    m.insert("for".into(), n.into());
                }
            }
            Action::Pause => {
                m.insert("scope".into(), "/".into());
                m.insert(
                    "reason".into(),
                    "scale gate G-1: a pause of the root".into(),
                );
            }
            Action::Unpause => {
                m.insert("scope".into(), "/".into());
            }
        }
        Some(m)
    }

    /// Makes one call of `a`, intended at `at`, and records it.
    pub async fn act(&mut self, a: Action, at: Instant) -> Option<Called> {
        let a = if a.needs_hold() && self.holds.is_empty() {
            Action::ViewNode
        } else {
            a
        };
        let (a, m) = match self.args(a) {
            Some(m) => (a, m),
            // A join with no key known yet is new work.
            None if a == Action::CreateJoin => (Action::Create, self.args(Action::Create)?),
            // Nothing to take yet: read instead.
            None => (Action::FindText, self.args(Action::FindText)?),
        };
        Some(self.send(a, m, at).await)
    }

    /// Sends `a` with `args` (the session and an `op_id` added), intended at `at`, records it,
    /// and learns from its answer.
    async fn send(&mut self, a: Action, mut args: Map<String, Value>, at: Instant) -> Called {
        if let Some(s) = &self.session {
            args.entry("session").or_insert_with(|| s.clone().into());
        }
        let category = a.category();
        if category == Category::Write {
            args.entry("op_id")
                .or_insert_with(|| self.ids.uuid().into());
            self.writes.push_back(at);
        }
        let (tool, action) = tool_of(a);
        args.entry("action").or_insert_with(|| action.into());
        let key = args
            .get("keys")
            .and_then(Value::as_array)
            .and_then(|k| k.first())
            .and_then(Value::as_str)
            .map(str::to_string);
        if a == Action::Wait {
            self.sh.parked.fetch_add(1, Ordering::Relaxed);
        }
        let sent = now();
        // A parked wait is a long poll: its latency runs from when it was sent.
        let intended = if a == Action::Wait { sent } else { at };
        lock(&self.sh.flying).insert(self.id, (a.name(), category, intended, sent));
        let mut r = self.mcp.call(tool, Value::Object(args)).await;
        let done = now();
        lock(&self.sh.flying).remove(&self.id);
        if a == Action::Wait {
            self.sh.parked.fetch_sub(1, Ordering::Relaxed);
        }
        // Only an action known to be a stub has its `invalid` counted as stubbed.
        if a.stubbed_at_writing() && r.is_stub_answer() {
            r.outcome = Outcome::Stubbed;
        }
        self.sh.rec.record(&Sample {
            kind: a.name(),
            category,
            intended,
            sent,
            done,
            outcome: &r.outcome,
            expected: a.expected(),
        });
        if r.outcome == Outcome::Ok
            && matches!(a, Action::Create | Action::CreateJoin)
            && let Some(k) = key
        {
            self.sh.known_key(k);
        }
        self.learn(a, &r);
        r
    }

    /// What a reply teaches the agent: its session, its holds, versions, and paths.
    fn learn(&mut self, a: Action, r: &Called) {
        if let Some(s) = r.session() {
            self.session = Some(s);
        }
        let version = |v: &Value| {
            v.get("brief")
                .and_then(|b| b.get("node"))
                .or_else(|| v.get("node"))
                .and_then(|n| n.get("version"))
                .and_then(Value::as_u64)
        };
        match &r.outcome {
            Outcome::Ok => {}
            Outcome::Refused(code) => {
                match code.as_str() {
                    "stale_token" | "closed" if a.needs_hold() => {
                        if !self.holds.is_empty() {
                            self.holds.remove(0);
                        }
                    }
                    "stale_version" => {
                        if let Some(h) = self.holds.first_mut() {
                            h.version = None;
                        }
                    }
                    "not_found" if r.text.contains("session") => {
                        // The session retired: this agent is gone (§3).
                        self.gone = true;
                    }
                    _ => {}
                }
                return;
            }
            Outcome::Stubbed | Outcome::Transport(_) => return,
        }
        let Some(st) = r.structured.as_ref() else {
            return;
        };
        let t = now();
        match a {
            Action::Next | Action::Take | Action::Create | Action::CreateJoin => {
                let path = st
                    .get("target")
                    .or_else(|| st.get("path"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        st.get("brief")
                            .and_then(|b| b.get("node"))
                            .and_then(|n| n.get("path"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                if let Some(p) = &path {
                    if matches!(a, Action::Create | Action::CreateJoin) {
                        self.sh.known(p.clone());
                    }
                    self.see(p);
                }
                if let (Some(p), Some(tok)) = (path, st.get("token").and_then(Value::as_str))
                    && self.holds.len() < HOLDS
                {
                    self.holds.push(Held {
                        path: p,
                        token: tok.to_string(),
                        version: version(st),
                        last_checkpoint: t,
                    });
                }
            }
            Action::ViewNode => {
                if let (Some(p), Some(v)) = (
                    st.get("node")
                        .and_then(|n| n.get("path"))
                        .and_then(Value::as_str),
                    version(st),
                ) && let Some(h) = self.holds.iter_mut().find(|h| h.path == p)
                {
                    h.version = Some(v);
                }
            }
            Action::Update => {
                if let (Some(h), Some(v)) = (
                    self.holds.first_mut(),
                    st.get("version").and_then(Value::as_u64),
                ) {
                    h.version = Some(v);
                }
            }
            Action::Checkpoint | Action::Renew => {
                if let Some(h) = self.holds.first_mut() {
                    h.last_checkpoint = t;
                }
            }
            Action::Close | Action::Release => {
                if !self.holds.is_empty() {
                    self.holds.remove(0);
                }
            }
            _ => {}
        }
    }

    /// The forum session, once `hello` opened it.
    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// The paths this agent holds.
    #[must_use]
    pub fn held(&self) -> Vec<String> {
        self.holds.iter().map(|h| h.path.clone()).collect()
    }

    /// Opens a new MCP and forum session (`hello`) for a scripted role, outside the mix.
    ///
    /// # Errors
    ///
    /// The handshake failed, or `hello` opened no session.
    pub async fn open(&mut self) -> Result<(), String> {
        self.session = None;
        self.holds.clear();
        self.mcp.initialize().await?;
        let t = now();
        self.act(Action::Hello, t).await;
        self.session
            .clone()
            .map(|_| ())
            .ok_or_else(|| "hello opened no session".into())
    }

    /// Opens a new MCP session and forgets the forum session, without `hello`: the caller makes
    /// its own first call (a `hello` with an intent of its choosing, say).
    ///
    /// # Errors
    ///
    /// The handshake failed.
    pub async fn reconnect(&mut self) -> Result<(), String> {
        self.session = None;
        self.holds.clear();
        self.mcp.initialize().await
    }

    /// Makes a call with arguments of the caller's choosing, recorded as `a`.
    pub async fn call_with(&mut self, a: Action, args: Map<String, Value>, at: Instant) -> Called {
        self.send(a, args, at).await
    }

    /// The agent's generator, for scripted roles.
    pub fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }
}
