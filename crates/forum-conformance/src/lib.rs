//! Test-only: replays the Quint model's traces and counterexamples against the core (design D.10.8,
//! WP-F1). Never linked into the server.
//!
//! D.0.3 narrows the replay to the key parts of the model: the `holds` slice, and the `keys` slice
//! as far as the core implements its commands. The driver is hand-written over the ITF files
//! (`itf`), because Quint 0.33 writes no action names into `quint test` traces or into Apalache and
//! TLC counterexamples, while every state carries the model's `last_action` (D.14.5); and because
//! the fixtures replay without Quint, which quint-connect would run.
//!
//! - `itf`: the ITF reader, its value type `V`, and `diff`;
//! - `project`: the core's state in the model's shape (`project`, D.10.3);
//! - `driver`: the replay (`replay`, `Driver`, `Report`).
//!
//! The fixtures are in `tests/traces/`, and `scripts/traces.sh` regenerates them with Quint.

pub mod driver;
pub mod itf;
pub mod project;

use std::path::{Path, PathBuf};

pub use driver::{Config, Driver, Options, Report, Verdict, replay};
pub use itf::{Diff, Trace, V, diff};

/// The `*.itf.json` files of a directory, sorted by name.
///
/// # Errors
///
/// When the directory cannot be read.
pub fn trace_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.to_string_lossy().ends_with(".itf.json") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// Replays every trace of a directory.
///
/// # Errors
///
/// When the directory or a file cannot be read.
pub fn replay_dir(dir: &Path, opts: Options) -> Result<Vec<Report>, String> {
    let mut out = Vec::new();
    for p in trace_files(dir)? {
        let t = Trace::read(&p)?;
        let name = p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        out.push(replay(&t, &name, opts));
    }
    Ok(out)
}

/// The directory of the committed fixtures: `tests/traces/<sub>` of this crate.
#[must_use]
pub fn fixtures(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("traces")
        .join(sub)
}
