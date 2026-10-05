//! What a run records: for every phase and every kind of call, the latency from the call's
//! intended time (so that a slow server cannot hide its stalls by delaying the calls after them:
//! no coordinated omission), the service time from the moment the call was sent, and the count of
//! each outcome; and, per second of the run, the calls that completed.
//!
//! **Phases by intended time.** A call counts in the phase that contains its intended time, not
//! the one current when its answer came: the tail of a burst at the end of a phase stays in that
//! phase. A call still unanswered when the run is closed is recorded as late, with its latency so
//! far ([`Recorder::late`]).
//!
//! **Stubs apart.** An answer of a command this build does not do yet (`stubbed`) is counted in its
//! phase, but in a table of its own: it is in no latency histogram, so it neither dilutes the
//! percentiles of the real calls nor counts as one of the calls the server served.
//!
//! **What a call is.** A *call* is a forum tool call: every `tools/call`, the parked `talk wait`s
//! and the stubbed answers included, the MCP handshakes left out. D.15.2's rates (250 calls a
//! second at base) count calls so, waits included, as the review's load model does. A *served*
//! call is one the server really did: neither a stub nor a transport failure.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::clock::Instant;
use crate::hist::Hist;
use crate::mcp::Outcome;

/// The latency class a gate bounds (D.12.8, G-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// `session hello`, with its search.
    Hello,
    /// `hold next`.
    Next,
    /// Reads: `view`, `hold check`, `talk who`.
    Read,
    /// Writes: everything that may append.
    Write,
    /// `control health`.
    Health,
    /// `talk wait`: parked, so outside the latency bounds.
    Wait,
    /// The MCP handshake, outside the forum's tools.
    Init,
}

impl Category {
    /// Its name in reports.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Category::Hello => "hello",
            Category::Next => "next",
            Category::Read => "read",
            Category::Write => "write",
            Category::Health => "health",
            Category::Wait => "wait",
            Category::Init => "init",
        }
    }

    /// Whether D.5.2 counts it as a read (as the review's load model does): `hello`, the views,
    /// `hold check`, `talk who`, `control health`, and `talk wait`.
    #[must_use]
    pub fn is_read(self) -> bool {
        matches!(
            self,
            Category::Hello | Category::Read | Category::Health | Category::Wait
        )
    }
}

/// What to do with a command that this build of the server does not implement yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StubPolicy {
    /// Send it, and count its answer as `stubbed`, not as an error (the default while the core's
    /// later packages are stubs).
    Count,
    /// Do not send the commands known to be stubs at all.
    Skip,
    /// Send it, and count `stubbed` as an unexpected error: for when every command must work.
    Require,
}

impl StubPolicy {
    /// From its name.
    #[must_use]
    pub fn parse(s: &str) -> Option<StubPolicy> {
        match s {
            "count" => Some(StubPolicy::Count),
            "skip" => Some(StubPolicy::Skip),
            "require" => Some(StubPolicy::Require),
            _ => None,
        }
    }
}

/// One kind of call within a phase.
#[derive(Clone, Debug)]
struct KindRec {
    category: Category,
    latency: Hist,
    service: Hist,
    outcomes: BTreeMap<String, u64>,
    unexpected: u64,
}

impl KindRec {
    fn new(category: Category) -> KindRec {
        KindRec {
            category,
            latency: Hist::new(),
            service: Hist::new(),
            outcomes: BTreeMap::new(),
            unexpected: 0,
        }
    }

    fn calls(&self) -> u64 {
        self.outcomes.values().sum()
    }
}

/// One phase.
#[derive(Clone, Debug, Default)]
struct PhaseRec {
    name: String,
    start: Option<Instant>,
    end: Option<Instant>,
    /// Answered calls, by kind.
    kinds: BTreeMap<&'static str, KindRec>,
    /// Stubbed answers, by kind: in no latency histogram.
    stubbed: BTreeMap<&'static str, KindRec>,
}

/// One completed call.
pub struct Sample<'a> {
    /// Its kind, such as `hold.next`.
    pub kind: &'static str,
    /// Its latency class.
    pub category: Category,
    /// When it should have been sent.
    pub intended: Instant,
    /// When it was sent.
    pub sent: Instant,
    /// When its answer was read.
    pub done: Instant,
    /// How it ended.
    pub outcome: &'a Outcome,
    /// The refusals this kind of call may meet in the mix without anything being wrong.
    pub expected: &'a [&'a str],
}

/// The recorder of a run, shared by its agents.
pub struct Recorder {
    start: Instant,
    stubs: StubPolicy,
    phases: Mutex<Vec<PhaseRec>>,
    per_second: Mutex<BTreeMap<u64, u64>>,
    closed: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Whether an outcome is unexpected for a call that may meet `expected`.
#[must_use]
pub fn unexpected(outcome: &Outcome, expected: &[&str], stubs: StubPolicy) -> bool {
    match outcome {
        Outcome::Ok => false,
        Outcome::Stubbed => stubs == StubPolicy::Require,
        Outcome::Refused(code) => !expected.contains(&code.as_str()),
        Outcome::Transport(_) => true,
    }
}

impl Recorder {
    /// A recorder whose first phase, `name`, starts at `start`.
    #[must_use]
    pub fn new(start: Instant, stubs: StubPolicy, name: &str) -> Recorder {
        Recorder {
            start,
            stubs,
            phases: Mutex::new(vec![PhaseRec {
                name: name.to_string(),
                start: Some(start),
                ..PhaseRec::default()
            }]),
            per_second: Mutex::new(BTreeMap::new()),
            closed: AtomicBool::new(false),
        }
    }

    /// The stub policy of the run.
    #[must_use]
    pub fn stubs(&self) -> StubPolicy {
        self.stubs
    }

    /// Ends the current phase at `at` and starts `name`; returns its index.
    pub fn phase(&self, name: &str, at: Instant) -> usize {
        let mut p = lock(&self.phases);
        if let Some(last) = p.last_mut() {
            last.end = Some(at);
        }
        p.push(PhaseRec {
            name: name.to_string(),
            start: Some(at),
            ..PhaseRec::default()
        });
        p.len() - 1
    }

    /// Ends the last phase at `at` and closes the recorder: a sample that comes later is not
    /// recorded (a run records its unanswered calls with [`Recorder::late`] before it closes).
    pub fn finish(&self, at: Instant) {
        if let Some(last) = lock(&self.phases).last_mut() {
            last.end.get_or_insert(at);
        }
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Records one call in the phase that contains its intended time.
    pub fn record(&self, s: &Sample<'_>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let unexpected = unexpected(s.outcome, s.expected, self.stubs);
        let second = s.done.saturating_duration_since(self.start).as_secs();
        *lock(&self.per_second).entry(second).or_default() += 1;
        let mut p = lock(&self.phases);
        // The last phase that started at or before the intended time (the first if none did).
        let i = p
            .partition_point(|ph| ph.start.is_some_and(|t| t <= s.intended))
            .saturating_sub(1);
        let Some(phase) = p.get_mut(i) else {
            return;
        };
        let table = if *s.outcome == Outcome::Stubbed {
            &mut phase.stubbed
        } else {
            &mut phase.kinds
        };
        let k = table
            .entry(s.kind)
            .or_insert_with(|| KindRec::new(s.category));
        if *s.outcome != Outcome::Stubbed {
            k.latency
                .record(s.done.saturating_duration_since(s.intended));
            k.service.record(s.done.saturating_duration_since(s.sent));
        }
        *k.outcomes.entry(s.outcome.name()).or_default() += 1;
        if unexpected {
            k.unexpected += 1;
        }
    }

    /// Records calls still unanswered at `at` as late: their latency so far, as the transport
    /// failure `unfinished` (each is `(kind, category, intended, sent)`).
    pub fn late(&self, calls: &[(&'static str, Category, Instant, Instant)], at: Instant) {
        let unfinished = Outcome::Transport("unfinished".into());
        for (kind, category, intended, sent) in calls {
            self.record(&Sample {
                kind,
                category: *category,
                intended: *intended,
                sent: *sent,
                done: at,
                outcome: &unfinished,
                expected: &[],
            });
        }
    }

    /// The report of every phase, by kind and by category, as JSON.
    #[must_use]
    pub fn report(&self) -> Value {
        let p = lock(&self.phases);
        let phases: Vec<Value> = p.iter().map(|ph| phase_report(ph, self.start)).collect();
        let per_second: Vec<u64> = {
            let s = lock(&self.per_second);
            let last = s.keys().next_back().copied().unwrap_or(0);
            (0..=last)
                .map(|k| s.get(&k).copied().unwrap_or(0))
                .collect()
        };
        json!({"phases": phases, "completed_per_second": per_second})
    }

    fn merged(&self, phases: &[&str], keep: impl Fn(&str, &KindRec) -> bool) -> Hist {
        let p = lock(&self.phases);
        let mut h = Hist::new();
        for ph in p.iter().filter(|ph| phases.contains(&ph.name.as_str())) {
            for (name, k) in &ph.kinds {
                if keep(name, k) {
                    h.merge(&k.latency);
                }
            }
        }
        h
    }

    /// The merged latency histogram of `category` over the phases named in `phases`.
    #[must_use]
    pub fn category(&self, phases: &[&str], category: Category) -> Hist {
        self.merged(phases, |_, k| k.category == category)
    }

    /// The merged latency histogram of `category` over the phases named, leaving out one kind.
    #[must_use]
    pub fn category_without(&self, phases: &[&str], category: Category, kind: &str) -> Hist {
        self.merged(phases, |name, k| k.category == category && name != kind)
    }

    /// The merged latency histogram of every category but waits and the handshake.
    #[must_use]
    pub fn all_calls(&self, phases: &[&str]) -> Hist {
        self.merged(phases, |_, k| {
            !matches!(k.category, Category::Wait | Category::Init)
        })
    }

    /// The latency histogram of one kind over the phases named.
    #[must_use]
    pub fn kind(&self, phases: &[&str], kind: &str) -> Hist {
        self.merged(phases, |name, _| name == kind)
    }

    /// The calls of one kind over the phases named, stubbed answers included.
    #[must_use]
    pub fn calls_of(&self, phases: &[&str], kind: &str) -> u64 {
        let p = lock(&self.phases);
        p.iter()
            .filter(|ph| phases.contains(&ph.name.as_str()))
            .flat_map(|ph| [ph.kinds.get(kind), ph.stubbed.get(kind)])
            .flatten()
            .map(KindRec::calls)
            .sum()
    }

    /// Calls, unexpected outcomes, and stubbed answers over the phases named.
    #[must_use]
    pub fn counts(&self, phases: &[&str]) -> Counts {
        let p = lock(&self.phases);
        let mut c = Counts::default();
        for ph in p.iter().filter(|ph| phases.contains(&ph.name.as_str())) {
            c.seconds += match (ph.start, ph.end) {
                (Some(s), Some(e)) => e.saturating_duration_since(s).as_secs_f64(),
                _ => 0.0,
            };
            for (stub, table) in [(false, &ph.kinds), (true, &ph.stubbed)] {
                for (name, k) in table {
                    let n = k.calls();
                    c.unexpected += k.unexpected;
                    let mut failed = 0;
                    for (o, m) in &k.outcomes {
                        if o != "ok" {
                            *c.outcomes.entry(format!("{name}: {o}")).or_default() += m;
                        }
                        if o.starts_with("transport:") && k.category != Category::Init {
                            failed += m;
                        }
                    }
                    c.transport += failed;
                    if k.category == Category::Init {
                        c.handshakes += n;
                        continue;
                    }
                    c.calls += n;
                    if k.category == Category::Wait {
                        c.waits += n;
                        if !stub {
                            c.waits_served += n.saturating_sub(failed);
                        }
                    }
                    if k.category.is_read() {
                        c.reads += n;
                    }
                    if stub {
                        c.stubbed += n;
                        if k.category == Category::Write {
                            c.stubbed_writes += n;
                        }
                    } else if k.category == Category::Write {
                        c.writes += n;
                    }
                }
            }
        }
        c
    }

    /// The real-time window of a phase, in seconds from the run's start.
    #[must_use]
    pub fn window(&self, phase: &str) -> Option<(f64, f64)> {
        let p = lock(&self.phases);
        let ph = p.iter().find(|ph| ph.name == phase)?;
        let s = ph
            .start?
            .saturating_duration_since(self.start)
            .as_secs_f64();
        let e = ph.end?.saturating_duration_since(self.start).as_secs_f64();
        Some((s, e))
    }
}

/// Totals over some phases. A call is a forum tool call: waits and stubbed answers included, MCP
/// handshakes left out (see the module's documentation).
#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    /// The phases' length, in seconds.
    pub seconds: f64,
    /// Calls.
    pub calls: u64,
    /// MCP handshakes, apart from the calls.
    pub handshakes: u64,
    /// Parked waits among the calls.
    pub waits: u64,
    /// Waits the server served (neither stubbed nor failed in transport).
    pub waits_served: u64,
    /// Reads among the calls, as D.5.2 counts them (`hello`, views, checks, `who`, `health`, and
    /// waits).
    pub reads: u64,
    /// Writes the server did (stubs left out).
    pub writes: u64,
    /// Stubbed answers among the calls.
    pub stubbed: u64,
    /// Stubbed writes among them.
    pub stubbed_writes: u64,
    /// Transport failures among the calls.
    pub transport: u64,
    /// Outcomes that the mix does not expect (§21 codes outside the action's list, transport
    /// failures, and stubbed answers under `require`).
    pub unexpected: u64,
    /// Every outcome but `ok`, by kind.
    pub outcomes: BTreeMap<String, u64>,
}

impl Counts {
    /// Calls the server served: neither stubbed nor failed in transport.
    #[must_use]
    pub fn served(&self) -> u64 {
        self.calls
            .saturating_sub(self.stubbed)
            .saturating_sub(self.transport)
    }

    /// The share of calls that were stubbed.
    #[must_use]
    pub fn stub_share(&self) -> f64 {
        #[allow(clippy::cast_precision_loss, reason = "a share for a report")]
        let s = self.stubbed as f64 / self.calls.max(1) as f64;
        s
    }

    /// `n` per second of the phases.
    #[must_use]
    pub fn rate(&self, n: u64) -> f64 {
        #[allow(clippy::cast_precision_loss, reason = "a rate for a report")]
        let r = n as f64 / self.seconds.max(1e-9);
        r
    }
}

fn summarise(table: &BTreeMap<&'static str, KindRec>) -> serde_json::Map<String, Value> {
    table
        .iter()
        .map(|(name, k)| {
            (
                (*name).to_string(),
                json!({
                    "category": k.category.name(),
                    "latency": k.latency.summary(),
                    "service": k.service.summary(),
                    "outcomes": k.outcomes,
                    "unexpected": k.unexpected,
                }),
            )
        })
        .collect()
}

fn phase_report(ph: &PhaseRec, origin: Instant) -> Value {
    let at = |i: Option<Instant>| i.map(|x| x.saturating_duration_since(origin).as_secs_f64());
    let mut cats: BTreeMap<&'static str, (Hist, Hist, u64, u64)> = BTreeMap::new();
    for k in ph.kinds.values() {
        let e = cats
            .entry(k.category.name())
            .or_insert_with(|| (Hist::new(), Hist::new(), 0, 0));
        e.0.merge(&k.latency);
        e.1.merge(&k.service);
        e.2 += k.calls();
        e.3 += k.unexpected;
    }
    let categories: serde_json::Map<String, Value> = cats
        .into_iter()
        .map(|(c, (lat, svc, n, u))| {
            (
                c.to_string(),
                json!({"calls": n, "unexpected": u, "latency": lat.summary(), "service": svc.summary()}),
            )
        })
        .collect();
    let duration = match (ph.start, ph.end) {
        (Some(s), Some(e)) => e.saturating_duration_since(s),
        _ => Duration::ZERO,
    };
    let count = |table: &BTreeMap<&'static str, KindRec>| -> u64 {
        table
            .values()
            .filter(|k| k.category != Category::Init)
            .map(KindRec::calls)
            .sum()
    };
    let (answered, stubbed) = (count(&ph.kinds), count(&ph.stubbed));
    let calls = answered + stubbed;
    let rate = |n: u64| {
        if duration.is_zero() {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss, reason = "a rate for a report")]
            let r = n as f64 / duration.as_secs_f64();
            r
        }
    };
    json!({
        "name": ph.name,
        "start_s": at(ph.start),
        "end_s": at(ph.end),
        "calls": calls,
        "calls_per_second": rate(calls),
        "stubbed": stubbed,
        "categories": categories,
        "kinds": summarise(&ph.kinds),
        "stubbed_kinds": summarise(&ph.stubbed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_recorded_by_phase_kind_and_outcome() {
        let t0 = crate::clock::now();
        let r = Recorder::new(t0, StubPolicy::Count, "warmup");
        let ok = Outcome::Ok;
        let full = Outcome::Refused("full".into());
        let busy = Outcome::Refused("limit(busy)".into());
        let stub = Outcome::Stubbed;
        let at = |ms| t0 + Duration::from_millis(ms);
        r.record(&Sample {
            kind: "hold.take",
            category: Category::Write,
            intended: at(0),
            sent: at(5),
            done: at(10),
            outcome: &ok,
            expected: &["full"],
        });
        r.phase("base", at(20));
        for (o, e) in [(&full, true), (&busy, false), (&stub, true)] {
            r.record(&Sample {
                kind: "hold.take",
                category: Category::Write,
                intended: at(20),
                sent: at(21),
                done: at(60),
                outcome: o,
                expected: &["full"],
            });
            assert_eq!(!unexpected(o, &["full"], StubPolicy::Count), e);
        }
        let c = r.counts(&["base"]);
        assert_eq!((c.calls, c.unexpected, c.stubbed), (3, 1, 1));
        assert_eq!((c.writes, c.stubbed_writes, c.served()), (2, 1, 2));
        assert_eq!(c.outcomes.get("hold.take: limit(busy)"), Some(&1));
        let h = r.category(&["base"], Category::Write);
        assert_eq!(h.len(), 2, "a stubbed answer is in no latency histogram");
        assert!(
            h.quantile_us(0.5) >= 39_000,
            "latency runs from the intended time"
        );
        assert_eq!(r.counts(&["warmup"]).calls, 1);
        assert!(
            r.window("base").is_none(),
            "a phase not ended has no window"
        );
        assert!(unexpected(&stub, &[], StubPolicy::Require));
        let rep = r.report();
        assert_eq!(rep["phases"][1]["kinds"]["hold.take"]["unexpected"], 1);
        assert_eq!(rep["phases"][1]["stubbed"], 1);
        r.finish(at(100));
        assert!(r.window("base").is_some());
        r.record(&Sample {
            kind: "hold.take",
            category: Category::Write,
            intended: at(90),
            sent: at(90),
            done: at(110),
            outcome: &ok,
            expected: &[],
        });
        assert_eq!(
            r.counts(&["base"]).calls,
            3,
            "a closed recorder records nothing"
        );
    }

    #[test]
    fn a_call_counts_in_the_phase_of_its_intended_time() {
        let t0 = crate::clock::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let r = Recorder::new(t0, StubPolicy::Count, "heavy");
        r.phase("drain", at(1_000));
        let ok = Outcome::Ok;
        // Intended within the heavy phase, answered after it ended.
        r.record(&Sample {
            kind: "view.node",
            category: Category::Read,
            intended: at(990),
            sent: at(995),
            done: at(1_500),
            outcome: &ok,
            expected: &[],
        });
        assert_eq!(r.counts(&["heavy"]).calls, 1);
        assert_eq!(r.counts(&["drain"]).calls, 0);
        assert!(r.category(&["heavy"], Category::Read).quantile_us(1.0) >= 510_000);
        // A call still unanswered is recorded as late, with its latency so far.
        r.late(
            &[("hold.next", Category::Next, at(900), at(950))],
            at(3_000),
        );
        let c = r.counts(&["heavy"]);
        assert_eq!((c.calls, c.transport, c.unexpected), (2, 1, 1));
        assert!(r.kind(&["heavy"], "hold.next").quantile_us(1.0) >= 2_100_000);
    }

    #[test]
    fn handshakes_are_no_calls_and_waits_are_reads() {
        let t0 = crate::clock::now();
        let r = Recorder::new(t0, StubPolicy::Count, "base");
        let ok = Outcome::Ok;
        for (kind, category) in [
            ("mcp.initialize", Category::Init),
            ("talk.wait", Category::Wait),
            ("session.hello", Category::Hello),
            ("hold.next", Category::Next),
        ] {
            r.record(&Sample {
                kind,
                category,
                intended: t0,
                sent: t0,
                done: t0,
                outcome: &ok,
                expected: &[],
            });
        }
        let c = r.counts(&["base"]);
        assert_eq!((c.calls, c.handshakes, c.waits, c.reads), (3, 1, 1, 2));
    }
}
