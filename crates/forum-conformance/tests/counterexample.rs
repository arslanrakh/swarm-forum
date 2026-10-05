//! A counterexample of the `holds` slice replayed through `last_action` (D.10.8): the model's
//! simulation, asked to check the deliberately false invariant "no node is ever held twice"
//! (`s.nodes.keys().forall(n => s.nodes.get(n).fence <= 1)`), stops at the first state that breaks
//! it and writes that run (`#meta.status` "violation"). If the core reproduces the violating state,
//! the fault is in the invariant (as here, by construction) or in both; if it did not, the model and
//! the core would disagree, a conformance failure in its own right. This tells model bugs from core
//! bugs.

mod common;

use forum_conformance::{Options, Trace, Verdict, fixtures, replay, trace_files};

#[test]
fn the_core_reproduces_the_counterexample() {
    let files = trace_files(&fixtures("cex")).unwrap_or_default();
    assert!(!files.is_empty(), "no counterexample in tests/traces/cex");
    for p in files {
        let t = Trace::read(&p).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.status, "violation", "{}", p.display());
        let r = replay(&t, &p.display().to_string(), Options::default());
        println!("{}", r.describe());
        assert_eq!(r.verdict, Verdict::Pass, "{}", r.describe());
        assert_eq!(r.replayed, t.states.len());
        // The core's last state breaks the invariant as the model's does: some node was held
        // twice.
        let s = r.state.unwrap_or_default();
        let twice: Vec<u64> = s
            .nodes
            .iter()
            .filter(|(_, n)| n.fence.0 >= 2)
            .map(|(id, _)| id.0)
            .collect();
        assert!(!twice.is_empty(), "the core does not reach the violation");
        println!("held twice in the core: {twice:?}");
    }
}
