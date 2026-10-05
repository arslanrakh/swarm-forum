//! The harnesses of WP-D1 (design D.12.5): external forms, time arithmetic, the deadline order, the
//! flag set, and FNV-1a-64.

use core::cmp::Ordering;

use super::r#gen;
use crate::fnv::{fnv_id, fnv_session, fnv1a64, tie_hash};
use crate::state::{DeadlineKind, Owner};
use crate::types::{Dur, Eid, Fence, Flag, FlagSet, NodeId, SessionId, Time, Token, When};

/// The values the symbolic part of `token_roundtrip` covers: every value of up to four digits.
const SYMBOLIC_BELOW: u64 = 10_000;

/// `parse(render(t)) == Some(t)` for tokens, and the same for `#n`, `e<n>`, `s-<n>`, on the bytes
/// that `render` turns into a string and `parse` reads from one (`Form`, `parse_bytes`).
///
/// Narrowed (D.12.5), and recorded for `docs/verification.md`: symbolic values below 10⁴ rather
/// than any pair of `u64`. Over the whole range the harness did not finish: on the strings it was
/// still in symbolic execution after an hour, and on the bytes the solver had not proved the 64-bit
/// decimal identity after 30 minutes. The full range is the unit tests' (`types::tests`): every
/// edge of the digit counts up to `u64::MAX`, and 100,000 pseudo-random values of every magnitude.
#[kani::proof]
#[kani::unwind(42)]
fn token_roundtrip() {
    let node: u64 = kani::any();
    let fence: u64 = kani::any();
    kani::assume(node < SYMBOLIC_BELOW && fence < SYMBOLIC_BELOW);
    let t = Token {
        node: NodeId(node),
        fence: Fence(fence),
    };
    assert_eq!(Token::parse_bytes(t.form().as_bytes()), Some(t));
    assert_eq!(
        NodeId::parse_bytes(NodeId(node).form().as_bytes()),
        Some(NodeId(node))
    );
    assert_eq!(
        Eid::parse_bytes(Eid(fence).form().as_bytes()),
        Some(Eid(fence))
    );
    assert_eq!(
        SessionId::parse_bytes(SessionId(node).form().as_bytes()),
        Some(SessionId(node))
    );
}

/// `plus`, `minus`, `since`, and `When::resolve` never overflow: they saturate.
#[kani::proof]
#[kani::unwind(1)]
fn time_arith_total() {
    let t: Time = kani::any();
    let u: Time = kani::any();
    let d: Dur = kani::any();
    let plus = t.plus(d);
    assert!(plus >= t && plus.0 >= d.0);
    if t.0 <= u64::MAX - d.0 {
        assert_eq!(plus.0, t.0 + d.0);
    } else {
        assert_eq!(plus, Time(u64::MAX));
    }
    let minus = t.minus(d);
    assert!(minus <= t);
    if d.0 <= t.0 {
        assert_eq!(minus.0, t.0 - d.0);
    } else {
        assert_eq!(minus, Time::ZERO);
    }
    let since = t.since(u);
    if u <= t {
        assert_eq!(since.0, t.0 - u.0);
    } else {
        assert_eq!(since, Dur(0));
    }
    assert_eq!(When::At(u).resolve(t), u);
    assert_eq!(When::In(d).resolve(t), t.plus(d));
}

/// The rank of a deadline kind in §17's order.
fn kind_rank(k: DeadlineKind) -> u8 {
    match k {
        DeadlineKind::Lease => 0,
        DeadlineKind::Wait => 1,
        DeadlineKind::Recheck => 2,
        DeadlineKind::UserQuestion => 3,
        DeadlineKind::Commitment => 4,
        DeadlineKind::OccurrenceSlot => 5,
        DeadlineKind::OccurrenceWindow => 6,
        DeadlineKind::RefusalExpiry => 7,
        DeadlineKind::Due => 8,
        DeadlineKind::Pause => 9,
        DeadlineKind::Silence => 10,
        DeadlineKind::Retire => 11,
        DeadlineKind::Stale => 12,
    }
}

/// The rank of an owner's variant in declaration order (A-43).
fn owner_rank(o: &Owner) -> u8 {
    match o {
        Owner::Hold(_) => 0,
        Owner::Node(_) => 1,
        Owner::Entry(_) => 2,
        Owner::Refusal(..) => 3,
        Owner::Class(_) => 4,
        Owner::Session(_) => 5,
    }
}

/// `DeadlineKey`'s order is total and lexicographic in `(at, kind, owner)`, with §17's kind order
/// and the owners in declaration order, then by payload.
#[kani::proof]
#[kani::unwind(3)]
fn deadline_order() {
    let a = r#gen::deadline_key();
    let b = r#gen::deadline_key();
    let c = r#gen::deadline_key();

    // Lexicographic in (at, kind, owner).
    let lex =
        a.at.cmp(&b.at)
            .then(kind_rank(a.kind).cmp(&kind_rank(b.kind)))
            .then(a.owner.cmp(&b.owner));
    assert_eq!(a.cmp(&b), lex);
    // Owners by variant first.
    if owner_rank(&a.owner) != owner_rank(&b.owner) {
        assert_eq!(
            a.owner.cmp(&b.owner),
            owner_rank(&a.owner).cmp(&owner_rank(&b.owner))
        );
    }
    // A total order: antisymmetric, consistent with equality, and transitive.
    assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
    assert_eq!(a.cmp(&b) == Ordering::Equal, a == b);
    if a <= b && b <= c {
        assert!(a <= c);
    }
}

/// `insert`, `remove`, and `contains` agree with a reference `[bool; 14]` (D-4, 1.8: fourteen
/// flags). The start is `gen`'s flag set, which holds only the fourteen bits.
#[kani::proof]
#[kani::unwind(15)]
fn flagset_ops() {
    let mut set: FlagSet = kani::any();
    assert_eq!(r#gen::FLAG_BITS, (1 << 14) - 1);
    assert!(set.0 < (1 << 14));
    let start = set.0;
    let mut reference = [false; 14];
    for (i, r) in reference.iter_mut().enumerate() {
        *r = start & (1 << i) != 0;
    }
    // Three symbolic operations.
    for _ in 0..3 {
        let i: usize = kani::any();
        kani::assume(i < 14);
        let flag = Flag::ALL[i];
        assert_eq!(usize::from(flag.index()), i);
        if kani::any() {
            set.insert(flag);
            reference[i] = true;
        } else {
            set.remove(flag);
            reference[i] = false;
        }
    }
    for (i, flag) in Flag::ALL.into_iter().enumerate() {
        assert_eq!(set.contains(flag), reference[i]);
        assert_eq!(set.with(flag).contains(flag), true);
        assert_eq!(set.without(flag).contains(flag), false);
    }
    assert_eq!(set.is_empty(), reference.iter().all(|r| !r));
    assert!(set.0 < (1 << 14));
}

/// `fnv1a64`, `fnv_id`, `fnv_session`, and `tie_hash` equal the pinned vectors of D.2.6.
#[kani::proof]
#[kani::unwind(20)]
fn fnv_reference() {
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    assert_eq!(fnv_id(0), 0xe604_823a_2490_29bf);
    assert_eq!(fnv_id(6), 0x2c23_d803_e2f4_6cf9);
    assert_eq!(fnv_id(42), 0x8f91_9d01_1520_8895);
    assert_eq!(fnv_id(u64::MAX), 0x7a49_69ca_2d63_1437);
    assert_eq!(fnv_session(SessionId(0)), 0xe604_813a_2490_280c);
    assert_eq!(fnv_session(SessionId(1)), 0x529a_2ddc_8ff5_355f);
    assert_eq!(fnv_session(SessionId(2)), 0x0cd9_2bf5_4dc6_1432);
    assert_eq!(fnv_session(SessionId(17)), 0x893f_83b5_d9a4_968f);
    assert_eq!(fnv_session(SessionId(u64::MAX)), 0x2804_57de_49e0_7df4);
    assert_eq!(tie_hash(SessionId(0), 0), 0x0000_0100_0000_01b3);
    assert_eq!(tie_hash(SessionId(1), 6), 0xd989_aa27_52ff_379a);
    assert_eq!(tie_hash(SessionId(1), 42), 0x3cf7_6f24_852b_5336);
    assert_eq!(tie_hash(SessionId(2), 42), 0x82b8_710b_c75a_7463);
    assert_eq!(tie_hash(SessionId(17), 5), 0xc1df_1b57_143f_208b);
    assert_eq!(
        tie_hash(SessionId(u64::MAX), u64::MAX),
        0x5245_11eb_e382_9643
    );
}

/// No panic for any 9-byte input of `fnv_id` or `fnv_session`, and `tie_hash` wraps instead of
/// overflowing (CS-12).
#[kani::proof]
#[kani::unwind(20)]
fn fnv_total() {
    let id: u64 = kani::any();
    let s: u64 = kani::any();
    let h = tie_hash(SessionId(s), id);
    assert_eq!(h, fnv_id(id).wrapping_sub(fnv_session(SessionId(s))));
    // The 9-byte forms of D.2.6.
    let mut id_bytes = [0_u8; 9];
    id_bytes[1..].copy_from_slice(&id.to_le_bytes());
    assert_eq!(fnv_id(id), fnv1a64(&id_bytes));
    let mut session_bytes = [1_u8; 9];
    session_bytes[..8].copy_from_slice(&s.to_le_bytes());
    assert_eq!(fnv_session(SessionId(s)), fnv1a64(&session_bytes));
}
