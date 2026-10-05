//! Primitive types and units (design D.2): time, identifiers and their external forms, the validated
//! string newtypes, keys, the small enums used everywhere, and the flag set.
//!
//! Every type derives `Clone, PartialEq, Eq, PartialOrd, Ord, Debug`; the integer newtypes also derive
//! `Copy, Default`, and, under `cfg(kani)`, `kani::Arbitrary`, as the field-less enums do. No type
//! derives `Hash`. Every newtype over `String` has a private field and a validating constructor, and
//! deserialises through it (`try_from = "String"`), so an invalid value cannot exist inside the core.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

// ------------------------------------------------------------------------------------------------
// Change<T>

/// A field change in a patch. `Keep` is the serde default, so an absent field means Keep.
///
/// No `Option<Option<T>>` appears in a command or an event: `serde_json` writes `None` and
/// `Some(None)` both as `null`, so a cleared field would replay as "unchanged" (D.2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Change<T> {
    /// Leave the field as it is.
    #[default]
    Keep,
    /// Clear an optional field.
    Clear,
    /// Set the field to this value.
    Set(T),
}

// ------------------------------------------------------------------------------------------------
// Time and durations (D.2.1)

/// Milliseconds since 1970-01-01T00:00:00Z. The core's only notion of time.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Time(pub u64);

/// A non-negative duration in milliseconds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Dur(pub u64);

/// An agent-supplied point in time: absolute, or relative to the command's clamped `now`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum When {
    /// An absolute time.
    At(Time),
    /// A duration after the command's clamped `now`.
    In(Dur),
}

impl Time {
    /// The epoch.
    pub const ZERO: Time = Time(0);

    /// Saturating: a deadline never wraps.
    #[must_use]
    pub const fn plus(self, d: Dur) -> Time {
        Time(self.0.saturating_add(d.0))
    }

    /// Saturating at zero.
    #[must_use]
    pub const fn minus(self, d: Dur) -> Time {
        Time(self.0.saturating_sub(d.0))
    }

    /// The duration from `earlier` to `self`, zero if `earlier` is later.
    #[must_use]
    pub const fn since(self, earlier: Time) -> Dur {
        Dur(self.0.saturating_sub(earlier.0))
    }
}

impl When {
    /// The absolute time this names, relative to `now`.
    #[must_use]
    pub const fn resolve(self, now: Time) -> Time {
        match self {
            When::At(t) => t,
            When::In(d) => now.plus(d),
        }
    }
}

/// One second.
pub const SECOND: Dur = Dur(1_000);
/// One minute.
pub const MINUTE: Dur = Dur(60_000);
/// One hour.
pub const HOUR: Dur = Dur(3_600_000);
/// One day.
pub const DAY: Dur = Dur(86_400_000);

// ------------------------------------------------------------------------------------------------
// Identifiers and counters (D.2.2)

/// A node id: `Counters::next_node`; 0..=7 are the reserved nodes. External form `#12`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct NodeId(pub u64);

/// A global feed-entry id: `Counters::next_eid`, the first is 1. External form `e57`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Eid(pub u64);

/// A session id: `Counters::next_session`, the first is 1. External form `s-4`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct SessionId(pub u64);

/// A per-node feed sequence number: the first entry is 1, and 0 means empty.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Seq(pub u64);

/// A per-node version, for `version` and `digest_version`; a new node has 1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Version(pub u64);

/// A per-node hold counter: a new node has 0, and the first token has 1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Fence(pub u64);

/// An interned principal: `Counters::next_principal`; 0 is the reserved `user` (CS-1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct PrincipalId(pub u32);

/// A hold token `<node id>.<fence>`: the hold's identity, since holds are keyed by token.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Token {
    /// The held node.
    pub node: NodeId,
    /// The node's fence when the hold was issued.
    pub fence: Fence,
}

/// Every entity born from a feed entry has that entry's `Eid` as its id.
pub type RequestId = Eid;
/// People, agent, and user questions.
pub type QuestionId = Eid;
/// A `said` entry.
pub type SaidId = Eid;
/// A commitment: the eid of the opening `ack`.
pub type CommitmentId = Eid;
/// A decision.
pub type DecisionId = Eid;
/// An objection.
pub type ObjectionId = Eid;
/// A `from_user` entry.
pub type UserEntryId = Eid;
/// A digest entry.
pub type DigestId = Eid;
/// An addressed message: `say`, an agent question, or an agent answer.
pub type NoteId = Eid;

/// `/`.
pub const ROOT: NodeId = NodeId(0);
/// `/@user`.
pub const USER: NodeId = NodeId(1);
/// `/@user/decisions`.
pub const USER_DECISIONS: NodeId = NodeId(2);
/// `/@user/digest`.
pub const USER_DIGEST: NodeId = NodeId(3);
/// `/sys`.
pub const SYS: NodeId = NodeId(4);
/// `/sys/signals`.
pub const SIGNALS: NodeId = NodeId(5);
/// `/@people`: the directory of people (FQ-5, D-3).
pub const PEOPLE: NodeId = NodeId(6);
/// `/@teams`: the directory of teams (FQ-5, D-3).
pub const TEAMS: NodeId = NodeId(7);
/// The first id an agent-made or server-made node gets.
pub const FIRST_FREE_NODE: NodeId = NodeId(8);

/// The reserved principal `user`, interned by Genesis: the principal of every entry when no other
/// is known, and the last stage of every addressee chain (CS-1, CS-2).
pub const USER_PRINCIPAL: PrincipalId = PrincipalId(0);

impl NodeId {
    /// One of the eight reserved nodes made by Genesis (D.2.2).
    #[must_use]
    pub const fn is_reserved(self) -> bool {
        self.0 < FIRST_FREE_NODE.0
    }

    /// The external form `#<n>`.
    #[must_use]
    pub fn render(&self) -> String {
        self.form().into_string()
    }

    /// Parses the external form `#<n>`, canonical decimal only (no sign, no leading zero).
    #[must_use]
    pub fn parse(s: &str) -> Option<NodeId> {
        NodeId::parse_bytes(s.as_bytes())
    }

    /// The external form as bytes.
    pub(crate) fn form(&self) -> Form {
        let mut f = Form::new();
        f.push(b'#');
        f.push_decimal(self.0);
        f
    }

    /// Parses the external form from bytes.
    pub(crate) fn parse_bytes(b: &[u8]) -> Option<NodeId> {
        parse_decimal(b.strip_prefix(b"#")?).map(NodeId)
    }
}

impl Eid {
    /// The external form `e<n>`.
    #[must_use]
    pub fn render(&self) -> String {
        self.form().into_string()
    }

    /// Parses the external form `e<n>`, canonical decimal only.
    #[must_use]
    pub fn parse(s: &str) -> Option<Eid> {
        Eid::parse_bytes(s.as_bytes())
    }

    /// The external form as bytes.
    pub(crate) fn form(&self) -> Form {
        let mut f = Form::new();
        f.push(b'e');
        f.push_decimal(self.0);
        f
    }

    /// Parses the external form from bytes.
    pub(crate) fn parse_bytes(b: &[u8]) -> Option<Eid> {
        parse_decimal(b.strip_prefix(b"e")?).map(Eid)
    }
}

impl SessionId {
    /// The external form `s-<n>`.
    #[must_use]
    pub fn render(&self) -> String {
        self.form().into_string()
    }

    /// Parses the external form `s-<n>`, canonical decimal only.
    #[must_use]
    pub fn parse(s: &str) -> Option<SessionId> {
        SessionId::parse_bytes(s.as_bytes())
    }

    /// The external form as bytes.
    pub(crate) fn form(&self) -> Form {
        let mut f = Form::new();
        f.push(b's');
        f.push(b'-');
        f.push_decimal(self.0);
        f
    }

    /// Parses the external form from bytes.
    pub(crate) fn parse_bytes(b: &[u8]) -> Option<SessionId> {
        parse_decimal(b.strip_prefix(b"s-")?).map(SessionId)
    }
}

impl Token {
    /// The external form `<node id>.<fence>`, for example `12.3`.
    #[must_use]
    pub fn render(&self) -> String {
        self.form().into_string()
    }

    /// Parses the external form `<node id>.<fence>`, canonical decimal only.
    #[must_use]
    pub fn parse(s: &str) -> Option<Token> {
        Token::parse_bytes(s.as_bytes())
    }

    /// The external form as bytes.
    pub(crate) fn form(&self) -> Form {
        let mut f = Form::new();
        f.push_decimal(self.node.0);
        f.push(b'.');
        f.push_decimal(self.fence.0);
        f
    }

    /// Parses the external form from bytes.
    pub(crate) fn parse_bytes(b: &[u8]) -> Option<Token> {
        let dot = b.iter().position(|&c| c == b'.')?;
        let node = b.get(..dot)?;
        let fence = b.get(dot.checked_add(1)?..)?;
        Some(Token {
            node: NodeId(parse_decimal(node)?),
            fence: Fence(parse_decimal(fence)?),
        })
    }
}

/// The most decimal digits a `u64` has.
const U64_DIGITS: usize = 20;
/// The longest external form: a token, two `u64`s and a dot.
const FORM_MAX: usize = 41;

/// An external form being written: at most `FORM_MAX` ASCII bytes, on the stack. `render` and
/// `parse` go through it and through the byte parsers below, so that Kani checks them on bytes; on
/// strings it does not finish (`token_roundtrip`, D.12.5).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Form {
    buf: [u8; FORM_MAX],
    len: usize,
}

impl Form {
    /// An empty form.
    pub(crate) const fn new() -> Form {
        Form {
            buf: [0; FORM_MAX],
            len: 0,
        }
    }

    /// Appends one byte; a byte past `FORM_MAX` is dropped, which no form reaches.
    pub(crate) fn push(&mut self, b: u8) {
        if let Some(slot) = self.buf.get_mut(self.len) {
            *slot = b;
            self.len = self.len.saturating_add(1);
        }
    }

    /// Appends the decimal digits of `v`, without a sign or leading zeros.
    pub(crate) fn push_decimal(&mut self, v: u64) {
        let mut digits = [b'0'; U64_DIGITS];
        let mut rest = v;
        let mut len: usize = 0;
        for slot in digits.iter_mut() {
            let digit = u8::try_from(rest.checked_rem(10).unwrap_or(0)).unwrap_or(0);
            *slot = b'0'.saturating_add(digit);
            len = len.saturating_add(1);
            rest = rest.checked_div(10).unwrap_or(0);
            if rest == 0 {
                break;
            }
        }
        for &d in digits.iter().take(len).rev() {
            self.push(d);
        }
    }

    /// The bytes written.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// The form as a string: its bytes are ASCII.
    pub(crate) fn into_string(self) -> String {
        self.as_bytes().iter().map(|&b| char::from(b)).collect()
    }
}

/// Appends the decimal digits of `v`, without a sign or leading zeros.
pub(crate) fn push_decimal(out: &mut String, v: u64) {
    let mut f = Form::new();
    f.push_decimal(v);
    out.push_str(&f.into_string());
}

/// Parses canonical decimal: one or more ASCII digits, no sign, no leading zero unless the number is
/// zero, and no overflow. So `parse_decimal` and `Form::push_decimal` are inverse bijections.
pub(crate) fn parse_decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > U64_DIGITS {
        return None;
    }
    if bytes.len() > 1 && bytes.first() == Some(&b'0') {
        return None;
    }
    let mut v: u64 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v
            .checked_mul(10)?
            .checked_add(u64::from(b.wrapping_sub(b'0')))?;
    }
    Some(v)
}

// ------------------------------------------------------------------------------------------------
// Validated string newtypes (D.2.3, D.2.4)

/// Why a string is not a valid value of a validated newtype; serde reports it on decoding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct InvalidValue(pub &'static str);

impl fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Implements `as_str`, `Display`, and the `String` conversions serde decodes through, for a
/// newtype over `String` with a validating `new`.
macro_rules! validated_string {
    ($ty:ident, $what:literal) => {
        impl $ty {
            /// The value as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $ty {
            type Error = InvalidValue;
            fn try_from(s: String) -> Result<Self, InvalidValue> {
                if $ty::new(&s).is_some() {
                    Ok($ty(s))
                } else {
                    Err(InvalidValue($what))
                }
            }
        }

        impl From<$ty> for String {
            fn from(v: $ty) -> String {
                v.0
            }
        }
    };
}

/// A class name from the bearer-token configuration: `[a-z0-9][a-z0-9-]{0,31}`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct ClassName(String);

impl ClassName {
    /// Validates `[a-z0-9][a-z0-9-]{0,31}`.
    #[must_use]
    pub fn new(s: &str) -> Option<ClassName> {
        let b = s.as_bytes();
        let first_ok = b
            .first()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok = b
            .iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        (first_ok && rest_ok && b.len() <= 32).then(|| ClassName(String::from(s)))
    }
}
validated_string!(ClassName, "a class name is [a-z0-9][a-z0-9-]{0,31}");

impl ClassName {
    /// `unknown`: the class a query shows for a session the state does not hold.
    #[must_use]
    pub fn unknown() -> ClassName {
        ClassName(String::from("unknown"))
    }
}

/// Tags, kind, needs, caps: `[a-z0-9][a-z0-9_.:-]{0,63}`. The core reads two tags: `offer`, and
/// `gone` on a person node (D.2.2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct Label(String);

impl Label {
    /// Validates `[a-z0-9][a-z0-9_.:-]{0,63}`.
    #[must_use]
    pub fn new(s: &str) -> Option<Label> {
        let b = s.as_bytes();
        let first_ok = b
            .first()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok = b.iter().all(|&c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b':' | b'-')
        });
        (first_ok && rest_ok && b.len() <= 64).then(|| Label(String::from(s)))
    }
}
validated_string!(Label, "a label is [a-z0-9][a-z0-9_.:-]{0,63}");

/// The tag that offers a node to `next` (§5).
pub const OFFER: &str = "offer";
/// The tag that marks a person who has left (D-3).
pub const GONE: &str = "gone";
/// The kind of a node that states a claim (§5, 1.8): an upheld objection corrects its `done` to
/// `refuted`.
pub const CLAIM: &str = "claim";
/// The outcome an upheld objection gives a claim that was closed `done` (§11.3, 1.8).
pub const REFUTED: &str = "refuted";

/// One path step: 1..=128 bytes of UTF-8, no `/`, no control characters, not `.` or `..`, no
/// leading or trailing whitespace, not starting with `#`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct Name(String);

impl Name {
    /// Validates one path step (§4).
    #[must_use]
    pub fn new(s: &str) -> Option<Name> {
        let len_ok = !s.is_empty() && s.len() <= 128;
        let chars_ok = !s.chars().any(|c| c == '/' || c.is_control());
        let dots_ok = s != "." && s != "..";
        let edges_ok = !s.starts_with(char::is_whitespace) && !s.ends_with(char::is_whitespace);
        let hash_ok = !s.starts_with('#');
        (len_ok && chars_ok && dots_ok && edges_ok && hash_ok).then(|| Name(String::from(s)))
    }
}
validated_string!(
    Name,
    "a name is 1 to 128 bytes, without '/' or control characters, not '.' or '..', not starting \
     with '#', and without surrounding whitespace"
);

/// A path as a sequence of names from the root. `/` is the empty sequence.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Path(pub Vec<Name>);

/// How a command names a node: by path (resolved step by step, §4) or by id (`#12`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NodeRef {
    /// A path from the root.
    Path(Path),
    /// A node id.
    Id(NodeId),
}

/// A client-chosen retry id: 1..=64 bytes of printable ASCII (0x20..=0x7E).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct OpId(String);

impl OpId {
    /// Validates 1..=64 bytes of printable ASCII.
    #[must_use]
    pub fn new(s: &str) -> Option<OpId> {
        let b = s.as_bytes();
        let ok = !b.is_empty() && b.len() <= 64 && b.iter().all(|&c| (0x20..=0x7E).contains(&c));
        ok.then(|| OpId(String::from(s)))
    }
}
validated_string!(OpId, "an op_id is 1 to 64 bytes of printable ASCII");

/// Whether `s` matches the scheme shape the key normalisers produce: `[a-z][a-z0-9+.-]{0,15}`
/// (D.11.14).
fn is_scheme(s: &str) -> bool {
    let b = s.as_bytes();
    let first_ok = b.first().is_some_and(u8::is_ascii_lowercase);
    let rest_ok = b
        .iter()
        .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'+' | b'.' | b'-'));
    first_ok && rest_ok && b.len() <= 16
}

/// A principal: a human an agent serves (§3, CS-1), taken at the agent's word. Either the reserved
/// `user`, or `<scheme>:<value>` (`mail:alice@corp.example`, `slack:t01/u02abc`), at most 256
/// bytes, already normalised by the shell (`forum_keys::normalise_principal`, D.11.14). The `user:`
/// scheme is refused, because it names log entries (`user:e12`). A principal key is not itself a
/// node key, and a principal is never a permission; a person node carries it inside a
/// `person:<key>` key (D.2.2, D.2.4), to which K1 and K2 apply.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct PrincipalKey(String);

/// The key of the reserved principal.
pub const USER_KEY: &str = "user";

impl PrincipalKey {
    /// Validates the shape the shell's normaliser produces: `user`, or `<scheme>:<value>` with the
    /// scheme `[a-z][a-z0-9+.-]{0,15}` other than `user` and a non-empty value, at most 256 bytes.
    #[must_use]
    pub fn new(s: &str) -> Option<PrincipalKey> {
        if s.len() > 256 {
            return None;
        }
        if s == USER_KEY {
            return Some(PrincipalKey(String::from(s)));
        }
        let (scheme, value) = s.split_once(':')?;
        let ok = is_scheme(scheme) && scheme != USER_KEY && !value.is_empty();
        ok.then(|| PrincipalKey(String::from(s)))
    }

    /// Whether this is the reserved principal `user`.
    #[must_use]
    pub fn is_user(&self) -> bool {
        self.0 == USER_KEY
    }
}
validated_string!(
    PrincipalKey,
    "a principal is user or <scheme>:<value>, at most 256 bytes, and not of the user: scheme"
);

/// SHA-256 of the request after secret masking (computed by the shell, compared by the core, D.8.1,
/// D.11.5, CS-35), so a stored digest reveals nothing that masking removed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct ReqDigest(pub [u8; 32]);

/// Free text, already masked by the shell. The core compares node-content texts for equality (O2)
/// and measures lengths in bytes: the digest's (`digest.max`) and every text of a command
/// (`text.max`, L1, A-114); it never parses text.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Text(pub String);

impl Text {
    /// The length in bytes, the measure of `digest.max` and `text.max`.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        u64::try_from(self.0.len()).unwrap_or(u64::MAX)
    }

    /// Whether the text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A normalised key `<scheme>:<value>`. The identity of a key is this string only. A request's
/// origin is stored like a key, as a `Key` with its normaliser version (§7: "message ids are
/// normalised like keys"; K2 covers every origin, A-61): the id of the message it came in, in its
/// conversation's scheme (`chat:<channel>/<thread>/<message>`, `issue:<project>#<n>/<comment>`,
/// `mail:<message-id>`, D.11.14), a `user:e<n>` entry (`NormVer(0)`), or, when the message has no
/// id, the conversation key. The core never parses one: it compares origins by their `KeyStr` for
/// Q1 and passes them through.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct KeyStr(String);

/// The scheme of the keys that route `from_user` entries (§8.2).
const USER_SCHEME: &str = "user";
/// The scheme of the keys the server gives occurrences (§12, A-90).
const REPEAT_SCHEME: &str = "repeat";
/// The scheme of a person's aliases (D.2.2).
const PERSON_SCHEME: &str = "person";

impl KeyStr {
    /// Validates the shape every normaliser produces: `<scheme>:<value>` with the scheme
    /// `[a-z][a-z0-9+.-]{0,15}` and a non-empty value.
    #[must_use]
    pub fn new(s: &str) -> Option<KeyStr> {
        let (scheme, value) = s.split_once(':')?;
        (is_scheme(scheme) && !value.is_empty()).then(|| KeyStr(String::from(s)))
    }

    /// Text before the first `:`.
    #[must_use]
    pub fn scheme(&self) -> &str {
        self.0.split_once(':').map_or(self.0.as_str(), |(s, _)| s)
    }

    /// Text after the first `:`.
    fn value(&self) -> &str {
        self.0.split_once(':').map_or("", |(_, v)| v)
    }

    /// A conversation key names a conversation with people outside the swarm (§9.1): its scheme is
    /// one of `params.conversation_schemes` (by default `chat`, `issue`, and `mail`); every other
    /// scheme names an object, such as a paper or a proof (A-30, CS-36).
    #[must_use]
    pub fn is_conversation(&self, schemes: &BTreeSet<Label>) -> bool {
        let scheme = self.scheme();
        schemes.iter().any(|l| l.as_str() == scheme)
    }

    /// `repeat:` keys are made only by the server (A-90): an agent-supplied key with this scheme is
    /// `invalid(ReservedKey)`, and `forum-keys` rejects the scheme as well.
    #[must_use]
    pub fn is_reserved(&self) -> bool {
        self.scheme() == REPEAT_SCHEME
    }

    /// `user:e<eid>` keys route `from_user` entries (§8.2): the entry, for the canonical form.
    #[must_use]
    pub fn user_entry(&self) -> Option<UserEntryId> {
        if self.scheme() == USER_SCHEME {
            Eid::parse(self.value())
        } else {
            None
        }
    }

    /// `person:<principal key>` names one alias of a person (D.2.2, FQ-5): the value is a principal
    /// key, normalised by `forum_keys::normalise_principal` (D.11.14); `user` is allowed, so that
    /// one human's anonymous client and named client can be one person.
    #[must_use]
    pub fn person(&self) -> Option<PrincipalKey> {
        if self.scheme() == PERSON_SCHEME {
            PrincipalKey::new(self.value())
        } else {
            None
        }
    }

    /// The canonical key of a user entry: `user:e<eid>`.
    #[must_use]
    pub fn of_user_entry(e: UserEntryId) -> KeyStr {
        let mut s = String::from("user:");
        s.push_str(&e.render());
        KeyStr(s)
    }

    /// The canonical key of an occurrence: `repeat:<series id>:<YYYYMMDDTHHMMZ>`.
    #[must_use]
    pub fn of_occurrence(series: NodeId, slot: Time) -> KeyStr {
        let mut s = String::from("repeat:");
        push_decimal(&mut s, series.0);
        s.push(':');
        s.push_str(&forum_cron::slot_name(slot.0));
        KeyStr(s)
    }
}
validated_string!(
    KeyStr,
    "a key is <scheme>:<value> with the scheme [a-z][a-z0-9+.-]{0,15} and a non-empty value"
);

/// Normaliser version recorded with every stored key and every request's origin (K2). Version 0
/// marks keys and origins the core makes itself (`user:e<eid>` on routing,
/// `repeat:<id>:<slot>`), whose canonical form is fixed by construction; versions 1.. are
/// `forum_keys::VERSION` values.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct NormVer(pub u16);

/// A key as it arrives in a command: already normalised by the shell (§7).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Key {
    /// The normal form.
    pub key: KeyStr,
    /// The normaliser version that produced it.
    pub nv: NormVer,
}

// ------------------------------------------------------------------------------------------------
// Small enums used everywhere (D.2.5)

/// The relation of a link (§16).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum LinkRel {
    /// A later attempt at the same key (§7).
    Continues,
    /// Derived from the target.
    DerivedFrom,
    /// Conflicts with the target (opens a signal, §14).
    ConflictsWith,
    /// A duplicate of the target (opens a signal, §14).
    Duplicate,
    /// Superseded by the target (resolves the pair's signals).
    SupersededBy,
}

/// §5: by convention done, failed, dropped, or superseded; `Missed` is the server's own (§12); any
/// other label is kept as `Other`. Agents may not pass `Missed` (A-66).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Outcome {
    /// The work is done.
    Done,
    /// The work failed.
    Failed,
    /// The work was dropped.
    Dropped,
    /// Superseded, for example by a merge.
    Superseded,
    /// An occurrence nobody took within its window (server-made only).
    Missed,
    /// Any other label.
    Other(Label),
}

/// The kind of a `said` entry. `Question` only through `talk ask`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum SaidKind {
    /// "On it": opens or refreshes a commitment.
    Ack,
    /// A reply.
    Reply,
    /// A result.
    Result,
    /// A correction of an earlier `said`.
    Correction,
    /// A question (recorded by `talk ask`).
    Question,
}

/// The kind of a `to_user` report.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum ToUserKind {
    /// An acknowledgement.
    Ack,
    /// A question.
    Question,
    /// A result: the only kind that answers a user request (A-23).
    Result,
}

/// Why a hold is released (§10.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum ReleaseReason {
    /// Someone else should continue.
    Handoff,
    /// Waiting for something.
    Blocked,
    /// This session cannot do it (a refusal, §10.3).
    Cannot,
}

/// How `view node` shows a node.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum ViewMode {
    /// The brief (§10.4).
    Brief,
    /// Every field, and what rests on the node (CS-22).
    Full,
    /// The review view (§11.3).
    Review,
}

/// What a note expects; stored, semantics later (§29).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Expects {
    /// Nothing.
    None,
    /// An acknowledgement.
    Ack,
    /// A reply.
    Reply,
}

/// The evidence of a decision; stored, semantics later (§29); `checked` is later (D.15.7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Evidence {
    /// Tried it.
    Attempt,
    /// Saw it.
    Observation,
    /// Inferred it.
    Inference,
    /// Was told.
    Reported,
}

/// The kind of a signal (§14).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum SignalKind {
    /// Two nodes carry the same work.
    Duplicate,
    /// Two nodes conflict.
    Conflict,
}

/// First-release addressees (§13). `class:`, `cap:`, and `everyone` are a later release (§29): the
/// shell rejects them with `invalid` (hint: later release). `principal:<key>` and a bare `user` are
/// stored as the principal they name when the note is recorded (CS-3, D.8.16).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Addressee {
    /// One session.
    Session(SessionId),
    /// Whoever holds the node, or takes it next.
    Holder(NodeId),
    /// Whoever holds work in the node's branch, resolved when read (D-6).
    Holders(NodeId),
    /// The sessions that serve the principal or an alias of it, resolved when read (CS-3).
    Principal(PrincipalId),
}

/// Who asked for a request (§9.4, CS-4): a principal, for a user request (origin `user:e<n>`), or
/// someone outside the swarm or an agent, whose name is text in the request entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Requester {
    /// A user request, from this principal.
    User(PrincipalId),
    /// Someone outside the swarm, or an agent.
    External,
}

/// §6 entry types, in this order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum EntryType {
    /// A note, or an addressed message.
    Note,
    /// A request.
    Request,
    /// A question.
    Question,
    /// An answer to an agent question.
    Answer,
    /// A decision.
    Decision,
    /// A checkpoint, or the record of a release.
    Checkpoint,
    /// A `said` entry.
    Said,
    /// A `heard` entry.
    Heard,
    /// A `from_user` entry.
    FromUser,
    /// A `to_user` entry.
    ToUser,
    /// An approval.
    Approve,
    /// An objection.
    Object,
    /// The resolution of an objection.
    Resolve,
    /// A retraction.
    Retract,
    /// A server entry.
    System,
    /// A write made with a stale token (H4).
    Late,
}

// ------------------------------------------------------------------------------------------------
// Flags (D.4.10)

/// The fourteen flags of §14, in table order: the twelve of 1.1's first release, `basis_changed`
/// (one hop, D-4), and `stale` (1.8).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Flag {
    /// A `from_user` entry is unrouted (on `@user`).
    Unrouted,
    /// An unsettled request with nobody at work on it.
    UnansweredRequest,
    /// A commitment past its follow-up.
    OpenCommitment,
    /// A `said` whose sending nobody confirmed.
    UnconfirmedSaid,
    /// An open intent with no holder.
    OpenIntent,
    /// A question to a person is due a recheck.
    QuestionRecheck,
    /// Proceed on a recorded assumption.
    ProceedOnAssumption,
    /// An open objection.
    Objection,
    /// A decision's basis went stale (D-4).
    BasisChanged,
    /// Too many attempts without progress.
    TooBig,
    /// Past its due.
    Overdue,
    /// Every child is closed.
    ReadyToClose,
    /// A series whose last occurrence was missed.
    Missed,
    /// 1.8 (§14): an offer that nothing has touched for `offer.stale`.
    Stale,
}

impl Flag {
    /// Every flag, in table order: `ALL[i]` has bit `i`.
    pub const ALL: [Flag; 14] = [
        Flag::Unrouted,
        Flag::UnansweredRequest,
        Flag::OpenCommitment,
        Flag::UnconfirmedSaid,
        Flag::OpenIntent,
        Flag::QuestionRecheck,
        Flag::ProceedOnAssumption,
        Flag::Objection,
        Flag::BasisChanged,
        Flag::TooBig,
        Flag::Overdue,
        Flag::ReadyToClose,
        Flag::Missed,
        Flag::Stale,
    ];

    /// The flag's position in table order, which is its bit in a `FlagSet`.
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Flag::Unrouted => 0,
            Flag::UnansweredRequest => 1,
            Flag::OpenCommitment => 2,
            Flag::UnconfirmedSaid => 3,
            Flag::OpenIntent => 4,
            Flag::QuestionRecheck => 5,
            Flag::ProceedOnAssumption => 6,
            Flag::Objection => 7,
            Flag::BasisChanged => 8,
            Flag::TooBig => 9,
            Flag::Overdue => 10,
            Flag::ReadyToClose => 11,
            Flag::Missed => 12,
            Flag::Stale => 13,
        }
    }

    /// The flag's bit, `1 << index`.
    #[must_use]
    pub const fn bit(self) -> u16 {
        match self {
            Flag::Unrouted => 0x0001,
            Flag::UnansweredRequest => 0x0002,
            Flag::OpenCommitment => 0x0004,
            Flag::UnconfirmedSaid => 0x0008,
            Flag::OpenIntent => 0x0010,
            Flag::QuestionRecheck => 0x0020,
            Flag::ProceedOnAssumption => 0x0040,
            Flag::Objection => 0x0080,
            Flag::BasisChanged => 0x0100,
            Flag::TooBig => 0x0200,
            Flag::Overdue => 0x0400,
            Flag::ReadyToClose => 0x0800,
            Flag::Missed => 0x1000,
            Flag::Stale => 0x2000,
        }
    }
}

/// A set of flags: bit `i` is the flag with index `i` (`Flag::index`). Kani draws it from the
/// generator in `kani/gen.rs`, which sets only the bits of `Flag::ALL`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FlagSet(pub u16);

impl FlagSet {
    /// No flag.
    pub const EMPTY: FlagSet = FlagSet(0);

    /// Whether the set holds `f`.
    #[must_use]
    pub const fn contains(self, f: Flag) -> bool {
        self.0 & f.bit() != 0
    }

    /// Adds `f`.
    pub fn insert(&mut self, f: Flag) {
        self.0 |= f.bit();
    }

    /// Removes `f`.
    pub fn remove(&mut self, f: Flag) {
        self.0 &= !f.bit();
    }

    /// The set with `f` added.
    #[must_use]
    pub const fn with(self, f: Flag) -> FlagSet {
        FlagSet(self.0 | f.bit())
    }

    /// The set with `f` removed.
    #[must_use]
    pub const fn without(self, f: Flag) -> FlagSet {
        FlagSet(self.0 & !f.bit())
    }

    /// Whether the set holds no flag.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags of both sets.
    #[must_use]
    pub const fn union(self, other: FlagSet) -> FlagSet {
        FlagSet(self.0 | other.0)
    }

    /// The flags common to both sets.
    #[must_use]
    pub const fn intersection(self, other: FlagSet) -> FlagSet {
        FlagSet(self.0 & other.0)
    }

    /// The flags of the set, in table order.
    pub fn iter(self) -> impl Iterator<Item = Flag> {
        Flag::ALL.into_iter().filter(move |&f| self.contains(f))
    }
}

impl FromIterator<Flag> for FlagSet {
    fn from_iter<I: IntoIterator<Item = Flag>>(iter: I) -> FlagSet {
        let mut set = FlagSet::EMPTY;
        for f in iter {
            set.insert(f);
        }
        set
    }
}

/// The mark a question leaves on a node: a recheck is due, or proceed on an assumption.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Mark {
    /// `question_recheck`.
    Recheck,
    /// `proceed_on_assumption`.
    Assume,
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::ToString;

    #[test]
    fn external_forms_round_trip() {
        for v in [0_u64, 1, 9, 10, 12, 57, 1_000, u64::MAX - 1, u64::MAX] {
            assert_eq!(NodeId::parse(&NodeId(v).render()), Some(NodeId(v)));
            assert_eq!(Eid::parse(&Eid(v).render()), Some(Eid(v)));
            assert_eq!(SessionId::parse(&SessionId(v).render()), Some(SessionId(v)));
            let t = Token {
                node: NodeId(v),
                fence: Fence(v / 3),
            };
            assert_eq!(Token::parse(&t.render()), Some(t));
            let mut s = String::new();
            push_decimal(&mut s, v);
            assert_eq!(s, v.to_string());
        }
        assert_eq!(NodeId(12).render(), "#12");
        assert_eq!(Eid(57).render(), "e57");
        assert_eq!(SessionId(4).render(), "s-4");
        assert_eq!(
            Token {
                node: NodeId(12),
                fence: Fence(3)
            }
            .render(),
            "12.3"
        );
    }

    /// The end-to-end round trip of every external form over 100,000 pseudo-random values of every
    /// magnitude (SplitMix64, shifted so that short numbers occur too), beside the Kani harness
    /// `token_roundtrip`.
    #[test]
    fn external_forms_round_trip_at_random() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        for _ in 0..100_000 {
            let a = next() >> (next() % 64);
            let b = next() >> (next() % 64);
            let t = Token {
                node: NodeId(a),
                fence: Fence(b),
            };
            let rendered = t.render();
            assert_eq!(rendered, format!("{a}.{b}"));
            assert_eq!(Token::parse(&rendered), Some(t));
            assert_eq!(NodeId::parse(&NodeId(a).render()), Some(NodeId(a)));
            assert_eq!(Eid::parse(&Eid(b).render()), Some(Eid(b)));
            assert_eq!(SessionId::parse(&SessionId(a).render()), Some(SessionId(a)));
        }
    }

    #[test]
    fn external_forms_refuse_what_is_not_canonical() {
        for bad in [
            "",
            "#",
            "#012",
            "#+1",
            "#-1",
            "# 1",
            "#1 ",
            "#18446744073709551616",
            "12",
            "#1a",
        ] {
            assert_eq!(NodeId::parse(bad), None, "{bad:?}");
        }
        assert_eq!(NodeId::parse("#0"), Some(NodeId(0)));
        for bad in ["e", "E5", "e05", "s-4", "e-1"] {
            assert_eq!(Eid::parse(bad), None, "{bad:?}");
        }
        for bad in ["s4", "s-", "s-04", "S-4", "e4"] {
            assert_eq!(SessionId::parse(bad), None, "{bad:?}");
        }
        for bad in ["12", "12.", ".3", "12.3.4", "12.03", "a.b", "12,3"] {
            assert_eq!(Token::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn validated_strings() {
        assert!(ClassName::new("coding-agent").is_some());
        assert_eq!(ClassName::new("unknown"), Some(ClassName::unknown()));
        assert!(ClassName::new("a").is_some());
        assert!(ClassName::new("0x").is_some());
        assert!(ClassName::new(&"a".repeat(32)).is_some());
        for bad in ["", "-a", "Coding", "a_b", "a b", &"a".repeat(33)] {
            assert!(ClassName::new(bad).is_none(), "{bad:?}");
        }

        assert!(Label::new("offer").is_some());
        assert!(Label::new("team:payments").is_some());
        assert!(Label::new("needs-review_2.x").is_some());
        assert!(Label::new(&"a".repeat(64)).is_some());
        for bad in ["", ":x", "_x", "Offer", "a b", "a/b", &"a".repeat(65)] {
            assert!(Label::new(bad).is_none(), "{bad:?}");
        }

        assert!(Name::new("summary-gap").is_some());
        assert!(Name::new("a b").is_some());
        assert!(Name::new("ünïcode").is_some());
        assert!(Name::new("@user").is_some());
        assert!(Name::new(&"a".repeat(128)).is_some());
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "#12",
            " a",
            "a ",
            "a\tb",
            "a\u{7}",
            "\u{a0}a",
            &"a".repeat(129),
        ] {
            assert!(Name::new(bad).is_none(), "{bad:?}");
        }

        assert!(OpId::new("0b9e2c5e-1d7a-4b45-9a8c-3f8d2e0c6a11").is_some());
        assert!(OpId::new(" ~").is_some());
        for bad in ["", "é", "a\nb", "\u{7f}", &"a".repeat(65)] {
            assert!(OpId::new(bad).is_none(), "{bad:?}");
        }

        assert!(PrincipalKey::new("user").is_some_and(|p| p.is_user()));
        assert!(PrincipalKey::new("mail:alice@corp.example").is_some());
        assert!(PrincipalKey::new("slack:t01/u02abc").is_some());
        assert!(PrincipalKey::new("test:p1").is_some());
        for bad in [
            "",
            "User",
            "user:e12",
            "mail:",
            ":x",
            "Mail:a",
            "nocolon",
            &"a:".repeat(129),
        ] {
            assert!(PrincipalKey::new(bad).is_none(), "{bad:?}");
        }

        assert!(KeyStr::new("chat:team/thread-1").is_some());
        assert!(KeyStr::new("user:e1").is_some());
        for bad in ["", "chat", "chat:", ":x", "Chat:x", "1chat:x"] {
            assert!(KeyStr::new(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn keys() {
        let k = |s: &str| KeyStr::new(s).unwrap();
        let schemes: BTreeSet<Label> = ["chat", "issue", "mail"]
            .iter()
            .filter_map(|s| Label::new(s))
            .collect();
        assert_eq!(k("chat:team/thread-1").scheme(), "chat");
        assert!(k("chat:team/thread-1").is_conversation(&schemes));
        assert!(k("mail:<m@x>").is_conversation(&schemes));
        assert!(!k("lean:abc").is_conversation(&schemes));
        assert!(!k("user:e1").is_conversation(&schemes));
        assert!(k("repeat:12:20261002T0500Z").is_reserved());
        assert!(!k("chat:x").is_reserved());
        assert_eq!(k("user:e57").user_entry(), Some(Eid(57)));
        assert_eq!(k("user:e057").user_entry(), None);
        assert_eq!(k("chat:e57").user_entry(), None);
        assert_eq!(KeyStr::of_user_entry(Eid(57)), k("user:e57"));
        assert_eq!(
            k("person:mail:bob@corp.example").person(),
            PrincipalKey::new("mail:bob@corp.example")
        );
        assert_eq!(k("person:user").person(), PrincipalKey::new("user"));
        assert_eq!(k("person:user:e1").person(), None);
        assert_eq!(k("chat:x").person(), None);
        // 2026-10-02T05:00Z: the first occurrence of the §32 series.
        let slot = Time(1_790_917_200_000);
        assert_eq!(
            KeyStr::of_occurrence(NodeId(12), slot),
            k("repeat:12:20261002T0500Z")
        );
        assert!(KeyStr::new(KeyStr::of_occurrence(NodeId(12), slot).as_str()).is_some());
    }

    #[test]
    fn time_saturates() {
        assert_eq!(Time(u64::MAX).plus(Dur(1)), Time(u64::MAX));
        assert_eq!(Time(1).minus(Dur(2)), Time(0));
        assert_eq!(Time(1).since(Time(5)), Dur(0));
        assert_eq!(Time(5).since(Time(1)), Dur(4));
        assert_eq!(When::In(MINUTE).resolve(Time(1)), Time(60_001));
        assert_eq!(When::At(Time(7)).resolve(Time(100)), Time(7));
    }

    #[test]
    fn flag_sets() {
        for (i, f) in Flag::ALL.iter().enumerate() {
            assert_eq!(usize::from(f.index()), i);
            assert_eq!(f.bit(), 1_u16 << i);
        }
        let mut s = FlagSet::EMPTY;
        s.insert(Flag::BasisChanged);
        s.insert(Flag::Missed);
        assert!(s.contains(Flag::BasisChanged) && s.contains(Flag::Missed));
        assert!(!s.contains(Flag::Unrouted));
        assert_eq!(
            s.iter().collect::<Vec<_>>(),
            [Flag::BasisChanged, Flag::Missed]
        );
        s.remove(Flag::Missed);
        assert_eq!(s, FlagSet::EMPTY.with(Flag::BasisChanged));
        assert!(s.without(Flag::BasisChanged).is_empty());
        assert_eq!(
            [Flag::Overdue, Flag::TooBig]
                .into_iter()
                .collect::<FlagSet>(),
            FlagSet::EMPTY.with(Flag::TooBig).with(Flag::Overdue)
        );
    }
}
