//! The core of the Swarm Forum: a pure, deterministic function (`settle`, `decide`, `evolve`)
//! with executable invariants and Kani harnesses (design D.1).
//!
//! WP-D1 (design D.13.2) freezes the types (D.2 to D.7), the parameters (D.3), the query API and
//! its result types (D.8.25, D.8.26), the shared helpers' signatures (D.13.1), the command
//! dispatcher, the invariant ids and their registry (D.9), and FNV-1a-64 (D.2.6). The bodies of the
//! helpers, commands, queries, `evolve`, and the checks arrive with the later layers of track D:
//! until then each is a total stub that emits nothing and returns an empty value.
//!
//! Layout:
//! - `types`, `fnv`, `params`, `state`, `cmd`, `event`, `reply`, `error`: the data (D.2 to D.7);
//! - `query` and `query::guide`: the read-only queries (D.8.25, D.8.26);
//! - `invariants` and `invariants::registry`: the invariants (D.9);
//! - `health`, `user`: the constants of `forum_core::health` and `forum_core::user` (D.3);
//! - `words` (1.9): the stop words of text search and the title words of likely duplicates;
//! - `refusals`: the refusal predicates (D.8.14);
//! - `evolve`: `evolve`, `evolve_mut`, `evolve_tracked`, and `restore_tracked` (D.7.4, D.4.12,
//!   WP-D2), with `facts` (`Fact`, D.6.1), `flags` (D.8.3, D.8.17), `deadlines` (D.4.8), `index`
//!   (the incremental maintenance of the indices, D.4.10), and `recount` (their full recount);
//! - `spec`: the naive definitions the checks and the proptests compare with (D.9.1);
//! - `testkit` (feature `testkit`, and the crate's own tests): `World` (D.12.1);
//! - `serde_pairs`: the encoding of maps whose keys are neither strings nor integers (D.7.6);
//! - `apply` and `settle` (WP-D3): `apply`, `apply_tracked`, `decide` (and `apply_with` under the
//!   feature `conformance`), `settle`, `settle_tracked`, and `settle_events` (D.6.1, D.8.1, D.8.2);
//! - the crate-internal `helpers` (D.13.1) and `dispatch` (`src/cmd/mod.rs`, D.13.1), and
//!   `recurring`, the consequences of `repeat` in `node create` and `node update` (D.8.18, WP-D6).

#![cfg_attr(not(any(test, kani)), no_std)]
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::arithmetic_side_effects,   // only checked_*, saturating_*, wrapping_* (FNV)
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::as_conversions,            // casts go through From/TryFrom
    clippy::iter_over_hash_type,
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::wildcard_enum_match_arm,   // every match names every variant
)]

extern crate alloc;

// The command types are `src/cmd.rs`, and the dispatcher is `src/cmd/mod.rs` (design D.13.2): Rust
// cannot load both files as the module `cmd`, so each names its file.
mod apply;
#[path = "cmd.rs"]
pub mod cmd;
pub mod deadlines;
pub mod error;
pub mod event;
pub mod evolve;
pub mod facts;
pub mod flags;
pub mod fnv;
pub mod health;
pub mod index;
pub mod invariants;
pub mod params;
pub mod query;
pub mod recount;
mod recurring;
pub mod refusals;
pub mod reply;
#[cfg(feature = "serde")]
pub mod serde_pairs;
mod settle;
pub mod spec;
pub mod state;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod types;
pub mod user;
pub mod words;

#[path = "cmd/mod.rs"]
mod dispatch;
#[allow(
    dead_code,
    reason = "shared helpers until their callers land (WP-D3 to WP-D6), design D.13.1"
)]
mod helpers;

#[cfg(kani)]
mod kani;

#[cfg(feature = "conformance")]
pub use apply::apply_with;
pub use apply::{apply, apply_tracked, decide};
pub use cmd::*;
pub use deadlines::{deadline_of, recount_deadlines};
pub use error::*;
pub use event::*;
pub use evolve::{evolve, evolve_mut, evolve_tracked, leaving_due, restore_tracked};
pub use facts::{Departed, Fact, Tracked};
pub use fnv::*;
pub use invariants::{
    Inv, Violation, ViolationDetail, check_fire, check_guide, check_guide_with, check_invariants,
    check_keys, check_startup, check_step,
};
pub use params::*;
pub use recount::recount;
pub use reply::*;
pub use settle::{settle, settle_events, settle_tracked};
pub use state::*;
pub use types::*;
