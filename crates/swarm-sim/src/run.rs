//! A load run: seed a backlog, start the population, and conduct its phases (the base mix, the
//! heavy mix with bursts, a broadcast every minute, a pause of the root followed by a herd of
//! `control health`), polling the server's counters as it goes.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::agent::{Agent, ROOT, Settings, Shared};
use crate::clock::{Instant, now};
use crate::mix::{Action, BURST, Profile};
use crate::record::StubPolicy;
use crate::server::StatsTail;
use crate::text;

/// Periodic bursts within a phase.
#[derive(Clone, Copy, Debug)]
pub struct Burst {
    /// From the start of one burst to the next.
    pub period: Duration,
    /// How long one lasts.
    pub length: Duration,
}

/// One phase of a run.
#[derive(Clone, Debug)]
pub struct Phase {
    /// Its name in the result.
    pub name: String,
    /// How long it lasts, in real time.
    pub duration: Duration,
    /// Its rate relative to the base mix (1 or 2.5).
    pub mix: f64,
    /// Bursts of three times the rate.
    pub burst: Option<Burst>,
    /// A `talk say {to: holders:/**}` this often.
    pub broadcast_every: Option<Duration>,
    /// A pause of `/` this long after the phase starts, lifted after `pause_for`, with a herd of
    /// `control health` within `herd_window` after it.
    pub pause_at: Option<Duration>,
    /// How long the pause lasts.
    pub pause_for: Duration,
    /// The herd's window.
    pub herd_window: Duration,
}

impl Phase {
    /// A phase of the base mix.
    #[must_use]
    pub fn base(name: &str, duration: Duration) -> Phase {
        Phase {
            name: name.into(),
            duration,
            mix: 1.0,
            burst: None,
            broadcast_every: None,
            pause_at: None,
            pause_for: Duration::ZERO,
            herd_window: Duration::from_secs(60),
        }
    }
}

/// A run.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The MCP endpoint.
    pub url: String,
    /// Bearer tokens, one per class.
    pub tokens: Vec<String>,
    /// The seed.
    pub seed: u64,
    /// Simulated agents (agent slots).
    pub agents: usize,
    /// The profile.
    pub profile: Profile,
    /// The server's clock compression.
    pub scale: f64,
    /// Over how long the agents start.
    pub ramp: Duration,
    /// A warm-up after the ramp, recorded apart.
    pub warmup: Duration,
    /// The phases.
    pub phases: Vec<Phase>,
    /// Nodes created before the agents start.
    pub backlog: usize,
    /// Connections that create the backlog.
    pub seeders: usize,
    /// The share of parked sessions.
    pub parked_share: f64,
    /// The share of sessions serving a principal.
    pub principal_share: f64,
    /// The principals.
    pub principals: usize,
    /// The share of sessions that vanish mid-task.
    pub drop_share: f64,
    /// The stub policy.
    pub stubs: StubPolicy,
    /// How long one request may take, a parked wait's included.
    pub request_timeout: Duration,
    /// Paths of nodes that exist already (a backlog seeded earlier).
    pub known: Vec<String>,
    /// Keys that nodes carry already (a backlog seeded earlier), for the creates that join.
    pub known_keys: Vec<String>,
    /// The first number for fresh paths and keys: past those of a backlog seeded earlier, so that
    /// the run's creates make new work, not joins (gate G-8 seeds, restarts, and runs).
    pub first_fresh: u64,
}

impl Spec {
    /// A run of `agents` at `profile` against `url`, with D.15.2's shares: a third parked, one in
    /// ten serving one of 1,000 principals, half vanishing mid-task.
    #[must_use]
    pub fn new(url: &str, tokens: Vec<String>, agents: usize, profile: Profile) -> Spec {
        Spec {
            url: url.into(),
            tokens,
            seed: 1,
            agents,
            profile,
            scale: 1.0,
            ramp: Duration::from_secs(10),
            warmup: Duration::from_secs(10),
            phases: Vec::new(),
            backlog: 1_000,
            seeders: 16,
            parked_share: 1.0 / 3.0,
            principal_share: 0.1,
            principals: 1_000,
            drop_share: 0.5,
            stubs: StubPolicy::Count,
            request_timeout: Duration::from_secs(120),
            known: Vec::new(),
            known_keys: Vec::new(),
            first_fresh: 0,
        }
    }

    /// The shared state of a run of this spec, its first phase named `phase`.
    #[must_use]
    pub fn shared(&self, phase: &str) -> Arc<Shared> {
        let sh = Arc::new(Shared::new(self.settings(), phase));
        for p in &self.known {
            sh.known(p.clone());
        }
        for k in &self.known_keys {
            sh.known_key(k.clone());
        }
        sh
    }

    fn settings(&self) -> Settings {
        Settings {
            url: self.url.clone(),
            tokens: self.tokens.clone(),
            seed: self.seed,
            profile: self.profile,
            scale: self.scale,
            stubs: self.stubs,
            parked_share: self.parked_share,
            principal_share: self.principal_share,
            principals: self.principals,
            drop_share: self.drop_share,
            first_fresh: self.first_fresh,
        }
    }
}

/// What a run returns.
pub struct Outcome {
    /// The shared state, with the recorder.
    pub shared: Arc<Shared>,
    /// The server's counters, polled through the run.
    pub stats: Option<StatsTail>,
    /// The phases' windows: name, start, and end, from the run's start.
    pub windows: Vec<(String, Duration, Duration)>,
    /// The bursts' windows, from the run's start.
    pub bursts: Vec<(Duration, Duration)>,
    /// The herds' windows, from the run's start.
    pub herds: Vec<(Duration, Duration)>,
}

impl Outcome {
    /// The window of a phase.
    #[must_use]
    pub fn window(&self, name: &str) -> Option<(Duration, Duration)> {
        self.windows
            .iter()
            .find(|w| w.0 == name)
            .map(|w| (w.1, w.2))
    }

    /// The JSON report of the run.
    #[must_use]
    pub fn report(&self, spec: &Spec) -> Value {
        let sh = &self.shared;
        json!({
            "agents": spec.agents,
            "profile": spec.profile.name,
            "clock_scale": spec.scale,
            "backlog": spec.backlog,
            "parked_share": spec.parked_share,
            "principal_share": spec.principal_share,
            "principals": spec.principals,
            "drop_share": spec.drop_share,
            "stubs": spec.stubs,
            "sessions": sh.sessions.load(Ordering::Relaxed),
            "vanished": sh.vanished.load(Ordering::Relaxed),
            "recorded": sh.rec.report(),
        })
    }
}

/// Creates `n` nodes of work over `conns` connections, each session making at most 100 (below
/// `write.per_hour`) and releasing the two it holds before it ends. Recorded as the phase "seed".
pub async fn seed(sh: &Arc<Shared>, n: usize, conns: usize, timeout: Duration) {
    if n == 0 {
        return;
    }
    let per = n.div_ceil(conns.max(1));
    let mut tasks = Vec::new();
    for c in 0..conns.max(1) {
        let sh = Arc::clone(sh);
        let count = per.min(n.saturating_sub(c * per));
        tasks.push(tokio::spawn(async move {
            let Ok(mut a) = Agent::new(Arc::clone(&sh), 1_000_000 + c, timeout) else {
                return;
            };
            let mut made = 0;
            let mut failed = 0;
            while made < count && !sh.stopped() {
                if a.open().await.is_err() {
                    // A server that does not take sessions: give up after a minute.
                    failed += 1;
                    if failed > 300 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                let batch = (count - made).min(100);
                for _ in 0..batch {
                    let path = sh.new_path(a.rng());
                    let k = sh.fresh();
                    let mut m = Map::new();
                    m.insert("path".into(), path.into());
                    m.insert("title".into(), text::words(a.rng(), 5).into());
                    m.insert("body".into(), text::words(a.rng(), 40).into());
                    m.insert("est".into(), (15 + a.rng().below(46)).into());
                    m.insert("keys".into(), json!([format!("issue:bench#{k}")]));
                    let r = a.call_with(Action::Create, m, now()).await;
                    if let Some(p) = r.str("path") {
                        sh.known(p.to_string());
                    }
                    made += 1;
                }
                for _ in 0..a.held().len() {
                    a.act(Action::Release, now()).await;
                }
            }
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

/// Runs `spec` to its end: seeds, ramps, warms up, then each phase; polls `stats` throughout.
pub async fn run(spec: &Spec, stats: Option<StatsTail>) -> Outcome {
    let sh = spec.shared("seed");
    let mut stats = stats;
    seed(&sh, spec.backlog, spec.seeders, spec.request_timeout).await;
    if let Some(s) = stats.as_mut() {
        s.poll();
    }
    let origin = sh.start;
    let at = |t: Instant| t.saturating_duration_since(origin);
    let mut windows = Vec::new();
    let mut bursts = Vec::new();
    let mut herds = Vec::new();
    let ramp_start = now();
    sh.rec.phase("ramp", ramp_start);
    let mut agents = Vec::new();
    for i in 0..spec.agents {
        let Ok(a) = Agent::new(Arc::clone(&sh), i, spec.request_timeout) else {
            continue;
        };
        let delay = spec.ramp.mul_f64(
            #[allow(clippy::cast_precision_loss, reason = "a fraction of the ramp")]
            {
                i as f64 / spec.agents.max(1) as f64
            },
        );
        agents.push(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            a.run().await;
        }));
    }
    tick(&sh, &mut stats, ramp_start + spec.ramp).await;
    windows.push(("ramp".into(), at(ramp_start), at(now())));
    if !spec.warmup.is_zero() {
        let w = now();
        sh.rec.phase("warmup", w);
        tick(&sh, &mut stats, w + spec.warmup).await;
        windows.push(("warmup".into(), at(w), at(now())));
    }
    for p in &spec.phases {
        let start = now();
        sh.rec.phase(&p.name, start);
        sh.set_mult(p.mix);
        let end = start + p.duration;
        let mut roles = Vec::new();
        // The broadcaster and the controller open their sessions now, so that each call of
        // theirs is sent at its intended time.
        if let Some(every) = p.broadcast_every
            && let Ok(mut a) = Agent::new(Arc::clone(&sh), 2_000_000, spec.request_timeout)
        {
            roles.push(tokio::spawn(async move {
                if a.open().await.is_err() {
                    return None;
                }
                let mut t = start + every;
                while t < end {
                    tokio::time::sleep_until(t).await;
                    a.act(Action::SayHolders, t).await;
                    t += every;
                }
                None
            }));
        }
        if let Some(after) = p.pause_at
            && let Ok(mut a) = Agent::new(Arc::clone(&sh), 3_000_000, spec.request_timeout)
        {
            let sh2 = Arc::clone(&sh);
            let (window, length) = (p.herd_window, p.pause_for);
            roles.push(tokio::spawn(async move {
                if a.open().await.is_err() {
                    return None;
                }
                let ps = start + after;
                tokio::time::sleep_until(ps).await;
                a.act(Action::Pause, ps).await;
                let h = now();
                sh2.herd(window);
                let lift = (ps + length).min(end);
                tokio::time::sleep_until(lift).await;
                a.act(Action::Unpause, lift).await;
                Some((h, h + window))
            }));
        }
        let mut in_burst: Option<Instant> = None;
        loop {
            let t = now();
            if t >= end {
                break;
            }
            // Bursts: three times the phase's rate for `length` of every `period`.
            if let Some(b) = p.burst {
                let into = t.saturating_duration_since(start);
                let phase_in = Duration::from_nanos(
                    u64::try_from(into.as_nanos() % b.period.as_nanos().max(1)).unwrap_or(0),
                );
                let on = phase_in >= b.period.saturating_sub(b.length);
                match (on, in_burst) {
                    (true, None) => {
                        sh.set_mult(p.mix * BURST);
                        in_burst = Some(t);
                    }
                    (false, Some(s)) => {
                        sh.set_mult(p.mix);
                        bursts.push((at(s), at(t)));
                        in_burst = None;
                    }
                    _ => {}
                }
            }
            tick(&sh, &mut stats, (t + Duration::from_millis(100)).min(end)).await;
        }
        if let Some(s) = in_burst {
            bursts.push((at(s), at(now())));
        }
        for r in roles {
            if let Ok(Ok(Some((h0, h1)))) = tokio::time::timeout(spec.request_timeout, r).await {
                herds.push((at(h0), at(h1)));
            }
        }
        windows.push((p.name.clone(), at(start), at(now())));
    }
    let stop = now();
    sh.rec.phase("drain", stop);
    sh.stop_at(stop);
    // Every agent sends the calls it intended before the stop and finishes its call in flight;
    // together they get one request timeout. A call still unanswered then is recorded as late,
    // with its latency so far, and its agent is stopped.
    let deadline = now() + spec.request_timeout;
    let mut left = Vec::new();
    for a in agents {
        let abort = a.abort_handle();
        if tokio::time::timeout(deadline.saturating_duration_since(now()), a)
            .await
            .is_err()
        {
            left.push(abort);
        }
    }
    let end = now();
    sh.rec.late(&sh.in_flight(), end);
    for a in left {
        a.abort();
    }
    sh.rec.finish(end);
    if let Some(s) = stats.as_mut() {
        s.poll();
    }
    Outcome {
        shared: sh,
        stats,
        windows,
        bursts,
        herds,
    }
}

/// Sleeps until `until`, polling the stats about four times a second.
async fn tick(_sh: &Arc<Shared>, stats: &mut Option<StatsTail>, until: Instant) {
    loop {
        let t = now();
        if t >= until {
            return;
        }
        tokio::time::sleep((until - t).min(Duration::from_millis(250))).await;
        if let Some(s) = stats.as_mut() {
            s.poll();
        }
    }
}

/// The root of the simulated work, for scripts that build on it.
#[must_use]
pub fn root() -> &'static str {
    ROOT
}
