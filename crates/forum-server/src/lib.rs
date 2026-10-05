//! The Swarm Forum shell: MCP over streamable HTTP, sessions, the SQLite state tables and history,
//! rendering, search, masking, and configuration (design D.11).
//!
//! Track E builds it in packages (design D.13.2).
//!
//! WP-E2 (MCP, decoding, and masking):
//! - `mcp`: the handler and its route, the six tools, the instructions, and the input schemas
//!   (D.11.2, D.11.5);
//! - `decode`: JSON to `Cmd`, with the argument tables that the schemas, the decoder, and the
//!   masker share (D.11.5);
//! - `mask`: secret masking (D.11.13); `digest`: the request and question digests (D.11.5);
//! - `auth` and `dotenv`: bearer tokens from the environment and `.env` (D.11.3);
//! - `session_bind`: MCP sessions to forum sessions (D.11.4); `tz`: frozen time-zone tables
//!   (D.11.10).
//!
//! WP-E1 (storage):
//! - `store`: the state tables, the history, and the shell's tables in one SQLite file, the
//!   two-pass writing of a job's dirty facts, startup's load, migrations, history replay, and the
//!   restores of entities that left memory (D.11.6, D.11.8, D.7.6, D.4.12).
//!
//! WP-E3 (rendering):
//! - `render`: every tool result within its budgets (D.11.7): the status line, `hello`, the brief
//!   and the views of `view node`, the lists, errors with their hints and warnings, the replies,
//!   and the next calls (§20.9), with the shell's texts read through a `render::Source`; and, under
//!   the feature `usability` (WP-U only), the arm without guidance.
//!
//! WP-E4 (the engine):
//! - `engine`: the single writer thread and its per-class queues (deficit round-robin), group
//!   commit, the pipeline of D.8.1 around each call, idle firing, the health cache, startup
//!   (D.11.8), `check`, and the `mcp::server::Backend` the handler calls (D.11.2);
//! - `wait`: `talk wait`'s long poll and the targeted wake-ups after each commit (D.11.11);
//! - `shown`: the versions each session was shown of each node, which `node update` takes when it
//!   leaves them out (1.7, §20.5);
//! - `search` and `search_feed`: the search file, its bounded query, and the feeder that fills it
//!   from the committed events (D.11.12);
//! - `checkpointer`: WAL checkpoints off the writer (D.11.2);
//! - `config`: the settings, the parameters, and the tokens from the environment and `.env`
//!   (D.11.3, D.11.9); `export`: `control export` as JSON lines (D.11.15); `limits`:
//!   `write.per_hour`, `queue.max_wait`, and the `WriteSurge` counts (D.11.2, D.11.15); `clock`:
//!   the shell's only reads of the system's clocks.
//!
//! WP-F4 (the scale gates, design D.12.8), with the feature `bench` only, never in a release
//! build:
//! - `bench`: the gates' instruments (the WAL bytes the commits write, startup's statements on the
//!   history, the time of each search, the sizes of the retained collections, and a commit probe).

pub mod auth;
#[cfg(feature = "bench")]
pub mod bench;
pub mod checkpointer;
pub mod clock;
pub mod config;
pub mod decode;
pub mod digest;
pub mod dotenv;
pub mod engine;
pub mod export;
pub mod limits;
pub mod mask;
pub mod mcp;
pub mod render;
pub mod search;
pub mod search_feed;
pub mod session_bind;
pub mod shown;
pub mod store;
pub mod tz;
pub mod wait;
