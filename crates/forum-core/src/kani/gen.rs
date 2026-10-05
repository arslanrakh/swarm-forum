//! Bounded generators for the types `kani::Arbitrary` cannot be derived for (design D.2, D.12.5):
//! a string from a two-letter alphabet, a map of at most 2 entries. Later harnesses add theirs here.

use crate::state::{DeadlineKey, Owner};
use crate::types::{ClassName, Flag, FlagSet};

/// The bits of `Flag::ALL`, computed by the compiler, so no harness unwinds a loop for it.
pub(crate) const FLAG_BITS: u16 = {
    let mut bits = 0_u16;
    let mut i = 0;
    while i < Flag::ALL.len() {
        bits |= Flag::ALL[i].bit();
        i += 1;
    }
    bits
};

/// A flag set with only the bits of `Flag::ALL`: a derived `Arbitrary` would also set the 2
/// unused high bits, which `contains` and `iter` ignore but `==` and `is_empty` see, so harnesses
/// would explore flag sets no state can hold.
impl kani::Arbitrary for FlagSet {
    fn any() -> FlagSet {
        FlagSet(kani::any::<u16>() & FLAG_BITS)
    }
}

/// A class name of one letter from `{a, b}`.
pub(crate) fn class_name() -> ClassName {
    let name = if kani::any() { "a" } else { "b" };
    ClassName::new(name).unwrap()
}

/// Any deadline owner, with class names from `class_name`.
pub(crate) fn owner() -> Owner {
    let choice: u8 = kani::any();
    kani::assume(choice < 6);
    match choice {
        0 => Owner::Hold(kani::any()),
        1 => Owner::Node(kani::any()),
        2 => Owner::Entry(kani::any()),
        3 => Owner::Refusal(kani::any(), class_name()),
        4 => Owner::Class(class_name()),
        _ => Owner::Session(kani::any()),
    }
}

/// Any deadline key.
pub(crate) fn deadline_key() -> DeadlineKey {
    DeadlineKey {
        at: kani::any(),
        kind: kani::any(),
        owner: owner(),
    }
}
