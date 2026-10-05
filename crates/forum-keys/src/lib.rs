//! Versioned key normalisers: pure, idempotent, and used by the shell only (design D.11.14).
//!
//! The shell turns every key a client sends into its normal form with [`normalise`] and records the
//! normaliser [`VERSION`] with it (`Key { key, nv }`, design D.2.4), before the command is built. The
//! core never calls this crate: replaying the history or loading the state never consults it, and
//! the shell's `check_keys` (K2) checks stored keys with [`is_normal`] under the version each key
//! was stored with. Every published version therefore stays callable through [`normalise_v`].
//!
//! Principal keys (CS-1) have their own normaliser, [`normalise_principal`], whose rules are frozen
//! once released and which has no version. The value of a `person:` key is a principal key and goes
//! through the same function, so a person's alias and the principal a client passes are one string
//! (D-3).
//!
//! Whether a scheme names a conversation is not this crate's business: the parameter
//! `conversation_schemes` decides it (A-30, CS-36).
//!
//! Both rule sets refuse control characters and the code points that Unicode 17.0 leaves
//! unassigned in the text they read, so that a later Unicode version in the toolchain or in a
//! dependency cannot change a normal form that is already stored. They read a key as it is sent,
//! except an `http:` or `https:` key, which they read in its normal form (below): in such a key the
//! URL parser percent-encodes a control character or an unassigned code point inside a path or a
//! query (`https://x.example/a\u{0}b` is `https://x.example/a%00b`), drops a tab, a carriage return,
//! or a line feed wherever it is and C0 control characters at the end (`https://x.example/a\u{1}`
//! is `https://x.example/a`), and refuses a control character or an unassigned code point in a
//! host.
//!
//! They also refuse, so that no key and no principal can read as another by a character that does
//! not show:
//!
//! - U+2028 line separator and U+2029 paragraph separator, which break a rendered line as a line
//!   feed does; like the line feed, they are whitespace, so they are trimmed at the edges and
//!   refused inside;
//! - every code point that Unicode 17.0 assigns and marks `Default_Ignorable_Code_Point`, except
//!   the variation selectors (the `Variation_Selector` property: the Mongolian free variation
//!   selectors U+180B to U+180D and U+180F, U+FE00 to U+FE0F, and U+E0100 to U+E01EF): 145 code
//!   points, anywhere in a key, its edges included, since none is whitespace. They are the 12
//!   bidirectional controls (U+061C, U+200E, U+200F, U+202A to U+202E, and U+2066 to U+2069), which
//!   reorder the text around them; the 9 zero-width format characters (U+200B to U+200D, U+2060 to
//!   U+2064, and U+FEFF); U+00AD soft hyphen; U+034F combining grapheme joiner; the Hangul fillers
//!   (U+115F, U+1160, U+3164, and U+FFA0); the Khmer inherent vowels (U+17B4 and U+17B5); the
//!   Mongolian vowel separator (U+180E); the deprecated format characters (U+206A to U+206F); the
//!   shorthand format controls (U+1BCA0 to U+1BCA3); the musical symbol format controls (U+1D173 to
//!   U+1D17A); and the tag characters (U+E0001 and U+E0020 to U+E007F). Each kind is refused with
//!   its own text. An emoji sequence joined by U+200D, and a subdivision flag spelt with tag
//!   characters, are refused with them. The variation selectors are kept, because a key may need
//!   one: each chooses a glyph for the character before it (U+FE0F asks for the emoji
//!   presentation, and a Mongolian free variation selector the form of a Mongolian letter).
//!
//! Two keys that read alike can still be two strings: letters that look like other letters are
//! accepted, and so is a variation selector that chooses no glyph for the character before it,
//! which then shows nothing (`team:alice` and `team:alice\u{180B}` are two principals, and
//! `note:x` and `note:x\u{FE00}` two keys).
//!
//! An `http:` or `https:` key is checked in its normal form, which is ASCII (a punycode host, a
//! percent-encoded path and query, no fragment), not as it is sent. A host whose punycode holds
//! U+200C or U+200D, as Persian and Devanagari names may, is therefore accepted however it is sent,
//! and so is a refused character that the URL parser percent-encodes (in a path or a query), drops
//! (in a host, a code point that UTS 46 ignores, as it ignores every default-ignorable code point
//! but the bidirectional controls, U+200C, U+200D, and the tag characters, so
//! `https://a\u{AD}b.example/` and `https://a\u{180B}b.example/` are `https://ab.example/`), or
//! strips with the fragment. A key that the parser refuses, as it refuses a host that holds a
//! bidirectional control or a tag character, is refused with the text of the first refused
//! character in it, and as an invalid URL when it holds none. A principal key has no such rule: one
//! whose scheme is `https` is checked as it is sent, like any other.
//!
//! Every function is total: it returns an error for any input it cannot normalise and never panics.

#![forbid(unsafe_code)]
#![deny(
    missing_docs,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

use core::fmt;

mod principal;
mod text;
mod unicode17;
mod v1;

/// The current normaliser version, recorded with every key the shell normalises (K2). Version 0 is
/// the core's own (`user:e<n>` on routing, `repeat:<id>:<slot>`) and is not a version of this crate.
pub const VERSION: u16 = 1;

/// A normalised key and the version of the normaliser that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormKey {
    /// The key in normal form, `<scheme>:<value>`.
    pub key: String,
    /// The normaliser version that produced `key`.
    pub version: u16,
}

/// Why a key or a principal key cannot be normalised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// Nothing is left after trimming the surrounding whitespace.
    Empty,
    /// There is no `<scheme>:` (no `:`, or nothing before the first one).
    NoScheme,
    /// The scheme does not match `[a-z][a-z0-9+.-]{0,15}` after lower-casing, or it is reserved:
    /// `repeat:` for keys (A-90), `user:` for principal keys (CS-1).
    BadScheme,
    /// The value is empty or does not have its scheme's shape, or the key holds a control
    /// character, a line or paragraph separator, a default-ignorable code point other than a
    /// variation selector (the crate documentation lists them), or a code point that Unicode 17.0
    /// leaves unassigned; the text names the rule, and the kind of character refused.
    BadValue(&'static str),
    /// The normal form is longer than 512 bytes (a key) or 256 bytes (a principal key).
    TooLong,
    /// No normaliser of this version exists.
    UnknownVersion(u16),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Empty => f.write_str("the key is empty"),
            KeyError::NoScheme => f.write_str("the key has no scheme: expected <scheme>:<value>"),
            KeyError::BadScheme => f.write_str(
                "the scheme is malformed or reserved: expected [a-z][a-z0-9+.-]{0,15}, \
                 and not repeat: (or user: for a principal)",
            ),
            KeyError::BadValue(why) => f.write_str(why),
            KeyError::TooLong => f.write_str(
                "the key is too long: at most 512 bytes (256 bytes for a principal key)",
            ),
            KeyError::UnknownVersion(v) => write!(f, "no key normaliser has version {v}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// Normalises with the current version. Pure, total on all strings (returns Err, never panics).
pub fn normalise(raw: &str) -> Result<NormKey, KeyError> {
    normalise_v(VERSION, raw)
}

/// The normaliser of a given version; every published version stays callable (K2, `check_keys`).
pub fn normalise_v(version: u16, raw: &str) -> Result<NormKey, KeyError> {
    match version {
        1 => v1::normalise(raw).map(|key| NormKey { key, version }),
        other => Err(KeyError::UnknownVersion(other)),
    }
}

/// K2 oracle: normalise_v(version, key) == Ok(key). Version 0 keys are checked by the core.
pub fn is_normal(key: &str, version: u16) -> bool {
    normalise_v(version, key).is_ok_and(|n| n.key == key)
}

/// A principal key (CS-1): `user`, or `<scheme>:<value>`. Pure, total, idempotent. Its rules are
/// frozen once released, because a change would split one human into two principals; it therefore
/// has no version.
///
/// The rules: NFC, then the surrounding whitespace trimmed; no control character and no line or
/// paragraph separator, no default-ignorable code point other than a variation selector (at an
/// edge too; the crate documentation lists them), and no code point that Unicode 17.0 leaves
/// unassigned, all read as the key is sent, whatever its scheme; `user` in any case is the reserved
/// principal; otherwise the scheme lower-cased and matching `[a-z][a-z0-9+.-]{0,15}`, but not
/// `user` (which names log entries), and a non-empty value; `mail:` takes a bare address, strips
/// one pair of surrounding angle brackets, refuses any other, and lower-cases the whole address,
/// and every other scheme keeps its value as it is; at most 256 bytes (design D.11.14).
pub fn normalise_principal(raw: &str) -> Result<String, KeyError> {
    principal::normalise(raw)
}
