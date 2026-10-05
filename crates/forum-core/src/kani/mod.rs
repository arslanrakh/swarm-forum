//! Kani harnesses of `forum-core` (design D.12.5), behind `#[cfg(kani)]`.
//!
//! `kani::Arbitrary` is derived for the integer newtypes, the field-less enums, and the few
//! fixed-size types built only from them (`When`, `Token`, `ReqDigest`, `LeaseTerms`,
//! `RefusalEntry`, `SignalKey`, `NextKey`, `Subtree`, `Rollup`, `Total`); `FlagSet`'s
//! comes from `gen.rs`, which sets only the flag bits. States, names, keys, and collections come
//! from the bounded generators of `gen.rs` (`gen` is a reserved word in edition 2024, hence
//! `r#gen`), whose collections hold at most 2 or 3 elements. Every
//! harness has an explicit `#[kani::unwind(n)]`. Run one with
//! `cargo kani -p forum-core --harness <name>`.
//!
//! Arithmetic in the harnesses is plain on purpose: Kani checks it for overflow, so a reference
//! that overflowed would fail the harness rather than hide a bug.

mod evolve;
pub(crate) mod r#gen;
mod holds;
mod primitives;
mod restructure;
