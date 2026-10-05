//! The invariant registry against the repository (design D.9.4, at D.0.3's levels): every name and
//! path a row gives exists, so that a check the registry names but nobody wrote cannot pass as a
//! check.
//!
//! - every Quint name is a `val`, `def`, `run`, or `temporal` of `model/**/*.qnt`;
//! - every Kani name is a `#[kani::proof]` function under `crates/*/src`;
//! - every test file exists and names its invariant;
//! - every key invariant with a Quint check has one that the `holds` or `keys` slice uses, the
//!   slices that TLC checks (D.0.3).
//!
//! The tables of `docs/verification.md` are generated from the registry by
//! `scripts/gen-verification-table.py`, whose `--check` `scripts/verify.sh` and CI run: that
//! script is the one writer of the document, so this test does not render it a second time.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use forum_core::invariants::registry::{INVARIANTS, KEY, Level};

/// The repository's root: two levels above this crate.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the repository root")
        .to_path_buf()
}

/// Every file under `dir` with the extension `ext`, skipping the directories named in `skip`.
fn files(dir: &Path, ext: &str, skip: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if !skip.contains(&name) {
                files(&p, ext, skip, out);
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(p);
        }
    }
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The identifiers that follow `keyword` and a space in `text`.
fn defined_after(text: &str, keyword: &str, out: &mut BTreeSet<String>) {
    let pat = format!("{keyword} ");
    let mut rest = text;
    while let Some(i) = rest.find(&pat) {
        let before_ok = rest[..i].chars().next_back().is_none_or(|c| !is_ident(c));
        let after = rest[i + pat.len()..].trim_start();
        let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
        if before_ok && !name.is_empty() {
            out.insert(name);
        }
        rest = &rest[i + pat.len()..];
    }
}

/// The names the model defines.
fn quint_names() -> BTreeSet<String> {
    let mut qnt = Vec::new();
    files(
        &root().join("model"),
        "qnt",
        &["node_modules", "traces", "_apalache-out"],
        &mut qnt,
    );
    assert!(!qnt.is_empty(), "no model files");
    let mut out = BTreeSet::new();
    for p in qnt {
        let text = fs::read_to_string(&p).unwrap();
        for kw in ["val", "def", "run", "temporal"] {
            defined_after(&text, kw, &mut out);
        }
    }
    out
}

/// The `#[kani::proof]` functions under `crates/*/src`.
fn kani_harnesses() -> BTreeSet<String> {
    let mut rs = Vec::new();
    for c in fs::read_dir(root().join("crates")).unwrap() {
        files(&c.unwrap().path().join("src"), "rs", &[], &mut rs);
    }
    let mut out = BTreeSet::new();
    for p in rs {
        let text = fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim() != "#[kani::proof]" {
                continue;
            }
            let f = lines
                .iter()
                .skip(i + 1)
                .take(12)
                .find_map(|l| {
                    let l = l.trim_start();
                    let l = l.strip_prefix("pub ").unwrap_or(l);
                    l.strip_prefix("fn ")
                })
                .unwrap_or_else(|| panic!("a #[kani::proof] without a function in {p:?}"));
            out.insert(f.chars().take_while(|c| is_ident(*c)).collect());
        }
    }
    out
}

/// Whether `text` names `id` as a whole word (`Inv::K6`, "K6's", "(K6)").
fn names(text: &str, id: &str) -> bool {
    text.match_indices(id).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + id.len()..].chars().next();
        before.is_none_or(|c| !is_ident(c)) && after.is_none_or(|c| !is_ident(c))
    })
}

#[test]
fn every_quint_name_is_in_the_model() {
    let defined = quint_names();
    for row in INVARIANTS {
        for n in row.quint {
            assert!(defined.contains(*n), "{:?}: {n} is not in model/", row.id);
        }
    }
}

#[test]
fn every_kani_name_is_a_harness() {
    let written = kani_harnesses();
    assert!(written.contains("silence_after_leases"));
    for row in INVARIANTS {
        for n in row.kani {
            assert!(
                written.contains(*n),
                "{:?}: {n} is not a #[kani::proof] function",
                row.id
            );
        }
    }
}

#[test]
fn every_test_file_exists_and_names_its_invariant() {
    let root = root();
    for row in INVARIANTS {
        let id = format!("{:?}", row.id);
        for path in row.tests {
            let text =
                fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{id}: {path}: {e}"));
            assert!(names(&text, &id), "{id}: {path} does not name it");
        }
    }
}

/// The code of `text`, its `//` and `/* */` comments blanked.
fn code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let line = rest.find("//");
        let block = rest.find("/*");
        let (i, end) = match (line, block) {
            (Some(l), Some(b)) if b < l => (b, "*/"),
            (Some(l), _) => (l, "\n"),
            (None, Some(b)) => (b, "*/"),
            (None, None) => {
                out.push_str(rest);
                return out;
            }
        };
        out.push_str(&rest[..i]);
        out.push(' ');
        rest = &rest[i..];
        match rest.find(end) {
            Some(j) => rest = &rest[j + end.len()..],
            None => return out,
        }
    }
}

/// D.0.3: the model checks of the key invariants run with TLC on the `holds` and `keys` slices
/// (`scripts/verify.sh`), so each key invariant with a Quint check has one that those slices, or
/// the checks they share (`slices/common.qnt`), use.
#[test]
fn every_key_invariant_is_checked_by_a_key_slice() {
    let slices = root().join("model/slices");
    let used: String = ["common", "holds", "keys"]
        .iter()
        .map(|s| code(&fs::read_to_string(slices.join(format!("{s}.qnt"))).unwrap()))
        .collect();
    for row in INVARIANTS.iter().filter(|r| r.level == Level::Key) {
        assert!(KEY.contains(&row.id), "{:?}", row.id);
        if row.quint.is_empty() {
            assert!(row.quint_reason.is_some(), "{:?}", row.id);
            continue;
        }
        assert!(
            row.quint.iter().any(|n| names(&used, n)),
            "{:?}: none of {:?} is in the holds or keys slices",
            row.id,
            row.quint
        );
    }
}
