//! The driver catches a divergence: a fixture changed in one place, as a core with that bug would
//! have produced it, fails its replay at that step and names what differs (WP-F1: "an injected
//! bug is caught"; the injection into a copy of the core itself is in the package's report).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failure is the report"
)]

use forum_conformance::{Options, Trace, V, Verdict, fixtures, replay, trace_files};

/// The first `holds` fixture that replays to its end untouched and satisfies `pred`.
fn holds_fixture_where(pred: impl Fn(&Trace) -> bool) -> Trace {
    for p in trace_files(&fixtures("holds")).unwrap() {
        let t = Trace::read(&p).unwrap();
        if pred(&t) && replay(&t, "untouched", Options::default()).verdict == Verdict::Pass {
            return t;
        }
    }
    panic!("no holds fixture fits")
}

fn has_hold(t: &Trace) -> bool {
    t.states
        .iter()
        .any(|st| !st["s"].get("holds").unwrap().as_map().unwrap().is_empty())
}

/// The first state whose `s.holds` is not empty, and a mutable reference to its first hold.
fn first_hold(t: &mut Trace) -> (usize, &mut V) {
    let i = t
        .states
        .iter()
        .position(|st| !st["s"].get("holds").unwrap().as_map().unwrap().is_empty())
        .expect("a fixture with a hold");
    let V::Rec(s) = t.states[i].get_mut("s").unwrap() else {
        panic!()
    };
    let V::Map(holds) = s.get_mut("holds").unwrap() else {
        panic!()
    };
    let h = holds.values_mut().next().unwrap();
    (i, h)
}

fn field<'a>(v: &'a mut V, f: &str) -> &'a mut V {
    let V::Rec(m) = v else { panic!("{v}") };
    m.get_mut(f).unwrap()
}

fn divergence(t: &Trace) -> (usize, Vec<String>) {
    let r = replay(t, "tampered", Options::default());
    match r.verdict {
        Verdict::Divergence { step, details, .. } => (step, details),
        Verdict::Pass | Verdict::Gap { .. } => panic!("not caught: {}", r.describe()),
    }
}

fn holds_fixture() -> Trace {
    holds_fixture_where(has_hold)
}

/// A lease one tick longer in the model: as a core whose lease is off by one would differ.
#[test]
fn a_lease_off_by_one_is_caught() {
    let mut t = holds_fixture();
    let (i, h) = first_hold(&mut t);
    let lu = field(h, "lease_until");
    *lu = V::Int(lu.as_int().unwrap() + 1);
    let (step, details) = divergence(&t);
    assert_eq!(step, i);
    assert!(
        details.iter().any(|d| d.contains("lease_until")),
        "{details:?}"
    );
}

/// A fence one higher: H3 and H4 read fences, and a token is `(node, fence)`.
#[test]
fn a_fence_off_by_one_is_caught() {
    let mut t = holds_fixture();
    let (i, _) = first_hold(&mut t);
    let V::Rec(s) = t.states[i].get_mut("s").unwrap() else {
        panic!()
    };
    let V::Map(nodes) = s.get_mut("nodes").unwrap() else {
        panic!()
    };
    let held = nodes
        .values_mut()
        .find(|n| n.get("fence").unwrap().as_int().unwrap() > 0)
        .unwrap();
    let f = field(held, "fence");
    *f = V::Int(f.as_int().unwrap() + 1);
    let (step, details) = divergence(&t);
    assert_eq!(step, i);
    assert!(details.iter().any(|d| d.contains("fence")), "{details:?}");
}

/// An outcome the core does not give.
#[test]
fn a_wrong_outcome_is_caught() {
    let mut t = holds_fixture();
    let i = t
        .states
        .iter()
        .position(|st| {
            st["last_action"]
                .get("outcome")
                .unwrap()
                .as_tag()
                .unwrap()
                .0
                == "OutOk"
        })
        .unwrap();
    let la = t.states[i].get_mut("last_action").unwrap();
    *field(la, "outcome") = V::tag("OutErr", V::unit("ErrFull"));
    let (step, details) = divergence(&t);
    assert_eq!(step, i);
    assert!(
        details.iter().any(|d| d.starts_with("outcome")),
        "{details:?}"
    );
}

/// A model clock that a time action did not reach.
#[test]
fn a_wrong_clock_is_caught() {
    let mut t = holds_fixture();
    let i = t
        .states
        .iter()
        .position(|st| st["last_action"].get("name").unwrap().as_str().unwrap() == "act_tick")
        .unwrap();
    let c = t.states[i].get_mut("clock").unwrap();
    *c = V::Int(c.as_int().unwrap() + 1);
    let (step, details) = divergence(&t);
    assert_eq!(step, i);
    assert!(
        details.iter().any(|d| d.starts_with("clock")),
        "{details:?}"
    );
}

/// An `op_id` record the core did not keep (O1).
#[test]
fn a_missing_op_record_is_caught() {
    let with_op = |st: &std::collections::BTreeMap<String, V>| {
        !st["s"].get("ops").unwrap().as_map().unwrap().is_empty()
    };
    let mut t = holds_fixture_where(|t| t.states.iter().any(with_op));
    let i = t.states.iter().position(with_op).unwrap();
    let V::Rec(s) = t.states[i].get_mut("s").unwrap() else {
        panic!()
    };
    *s.get_mut("ops").unwrap() = V::Map(std::collections::BTreeMap::new());
    let (step, details) = divergence(&t);
    assert_eq!(step, i);
    assert!(details.iter().any(|d| d.contains("ops")), "{details:?}");
}

/// A key the core does not carry (K1, K3: the keys slice reads keys and the tree).
#[test]
fn a_wrong_key_is_caught() {
    let t = trace_files(&fixtures("keys")).unwrap();
    let mut t = Trace::read(t.first().unwrap()).unwrap();
    let last = t.states.len() - 1;
    let V::Rec(s) = t.states[last].get_mut("s").unwrap() else {
        panic!()
    };
    let V::Map(nodes) = s.get_mut("nodes").unwrap() else {
        panic!()
    };
    let w1 = nodes.get_mut(&V::Int(8)).unwrap();
    let V::Set(keys) = field(w1, "keys") else {
        panic!()
    };
    keys.insert(V::tag("Obj", V::str("tampered")));
    let (step, details) = divergence(&t);
    assert_eq!(step, last);
    assert!(details.iter().any(|d| d.contains("keys")), "{details:?}");
}
