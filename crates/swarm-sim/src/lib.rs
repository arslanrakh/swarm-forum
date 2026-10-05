//! The load driver of the scale gates (design D.12.8, D.15.2, D.15.4; WP-F4): simulated agents
//! that speak MCP over streamable HTTP to a build of `forum-server` with its `bench` feature (the
//! release profile with the gates' counters), like any client, and link no forum crate.
//!
//! - [`mcp`]: a minimal MCP client, one connection per simulated agent, and the classification
//!   of each answer: ok, a refusal by its code, or a transport failure (the agent counts a stub's
//!   answer as stubbed only for an action it knows to be a stub);
//! - [`mix`]: the profiles (low, base, high), the base and heavy mixes, and the call mix per
//!   session;
//! - [`agent`]: a simulated agent, whose sessions work, park in `talk wait`, serve a principal, or
//!   vanish mid-task, with every call sent at its intended time;
//! - [`record`] and [`hist`]: latency from the intended time and service time per kind of call,
//!   in the phase of the intended time, with percentiles, the count of every outcome, and stubbed
//!   answers apart;
//! - [`run`]: a run's phases, bursts, broadcasts, pause, and herd;
//! - [`server`]: the server under test, its tokens, its memory, and its counters (the `bench`
//!   feature's JSON lines);
//! - [`machine`]: the machine, its durable fsync, and the commit the server's SQLite pays,
//!   recorded with every result;
//! - [`gates`]: gates G-1, G-2, G-4 to G-10, and G-12 of D.12.8, each with its pass or fail.

pub mod agent;
pub mod clock;
pub mod gates;
pub mod hist;
pub mod machine;
pub mod mcp;
pub mod mix;
pub mod record;
pub mod rng;
pub mod run;
pub mod server;
pub mod text;
