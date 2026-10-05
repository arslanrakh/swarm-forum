//! `forum-sim`: the year simulation (design D.15.4, WP-Y), in the light form chosen for the first
//! run: two profiles, `company` and `no-user`, over a short horizon (30 simulated days).
//!
//! It drives `forum-core` in process: `apply` and `settle` with a simulated clock, as the shell's
//! pipeline does (D.8.1: pre-settle, apply, post-settle), with no shell, no storage, and no HTTP.
//! Simulated humans (principals) write to their agents, answer the questions shown to them at the
//! review's message rates, never answer, or leave; simulated agents open sessions, take work with
//! `next`, checkpoint, ask, release, report, close, and vanish mid-task. An observer reads every
//! event and computes the "Reports" column of D.15.4 for the profile, which the report sets beside
//! the "Expected" column, with the trace of every contradiction.
//!
//! # Running it
//!
//! ```text
//! cargo run --release -p forum-sim -- --profile company --days 30 --seed 1
//! cargo run --release -p forum-sim -- --profile no-user --days 30 --seed 1
//! ```
//!
//! Each prints its report in Markdown; `--out <file>` writes it to a file instead. A run repeats
//! exactly with its seed: the generator is the simulation's own (`rng`), every rate is an integer,
//! and every collection is ordered. The company profile runs 28 days before the measurement
//! (`--warmup-days <d>`), so that every principal has a message history within `user_gap.window`
//! when it starts, and 28 days after it (`--cooldown-days <d>`), so that the questions asked
//! within it reach their fate; everything else is measured within the measured days. Other flags:
//! `--principals <n>`, `--p-follow <0..1>`, and `--check-hours <h>` (how often
//! `check_invariants` runs, every 24 simulated hours by default; 0 turns it off). The report's
//! first lines give the command that reproduces it.
//!
//! A run of the company profile at 200 principals takes about 40 s in a release build, most of it
//! in `check_invariants`; the no-user profile takes under a second.
//!
//! # What the light form leaves out
//!
//! - `query::health` (WP-D6) is sampled hourly, and the report gives its red and yellow shares.
//! - Recurring work (WP-D6) is in the core, but the no-user profile's scheduled-job class does not
//!   create occurrences of a `repeat` node: its jobs arrive as external requests from a scheduler.
//! - Next calls (WP-D7): `query::guide` returns the items of §20.9, and a guided worker follows
//!   the first `now` one with probability `--p-follow` (`guided`): a minute after the response,
//!   not after a step of work, while its own next action stays due when it was; a hold the call
//!   issues becomes its task, a node it creates is work, and a hold it ends or lost ends its
//!   task. The report counts the followed calls by rule, call, and outcome. A worker decides
//!   only where its own step would come: after a close or a release it asks `next` or ends as
//!   unguided, so the `now` items of those responses (rule 13's take or `next`, rule 6's report
//!   after a misaddressed one) are never followed. The run with 0.5 and the per-rule slips of
//!   D.15.4 are not part of the light form.
//!
//! # Layout
//!
//! - `rng`: the deterministic generator (integer exponential draws);
//! - `profile`: the two profiles and their rates;
//! - `cmds`: the commands the simulated agents send;
//! - `forum`: the forum in process, the call pipeline, and the counters of calls and errors;
//! - `observe`: the observer of events, its checks, and the traces;
//! - `guided`: the hook of the guided agents (next calls to commands);
//! - `sim`: the discrete-event simulation of humans and agents;
//! - `report`: the report against D.15.4.

pub mod cmds;
pub mod forum;
pub mod guided;
pub mod observe;
pub mod profile;
pub mod report;
pub mod rng;
pub mod sim;

/// The run's day 0: 2027-01-04T00:00:00Z, a Monday. The warm-up starts here.
pub const START: forum_core::Time = forum_core::Time(20_822 * 86_400_000);

/// Runs a profile to its end and returns the finished simulation.
#[must_use]
pub fn run(p: profile::Profile) -> sim::Sim {
    let mut s = sim::Sim::new(p);
    s.run();
    s
}
