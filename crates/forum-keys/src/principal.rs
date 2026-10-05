//! The principal-key normaliser (CS-1, design D.11.14). Frozen once released and without a version,
//! because a change would split one human into two principals.
//!
//! Rules: NFC, then the surrounding whitespace trimmed; the character rules of
//! `crate::text::check_chars` (no control character and no U+2028 or U+2029 inside, no
//! default-ignorable code point other than a variation selector anywhere, the edges included, and
//! no code point that Unicode 17.0 leaves unassigned), read as the key is sent whatever its scheme,
//! `http` and `https` included; `user` in any case is the reserved principal; otherwise
//! `<scheme>:<value>` with the scheme lower-cased and matching `[a-z][a-z0-9+.-]{0,15}`, a
//! non-empty value, and a normal form of at most 256 bytes. The scheme `user` is rejected, because
//! `user:e12` names a log entry, not a person. `mail:` takes a bare address: one pair of
//! surrounding angle brackets and the whitespace inside them is stripped, as a `mail:` key does,
//! any other angle bracket (a display name, a second pair) is refused, and the whole address is
//! lower-cased (a person, unlike a message id), so `mail:<Bob@X>` and `mail:bob@x` are one
//! principal; every other scheme keeps its value as it is. Aliases (one human under several keys)
//! are grouped by person nodes in the forum (D-3); this normaliser merges nothing.

use crate::KeyError;
use crate::text::{check_chars, clean, fixed_point, split_scheme, strip_angle_pair};

/// The reserved principal (D.2.3 `USER_KEY`).
const USER: &str = "user";

/// The longest principal key, in bytes.
const MAX_BYTES: usize = 256;

/// Normalises a principal key. The length limit applies to the normal form, after every round.
pub(crate) fn normalise(raw: &str) -> Result<String, KeyError> {
    let key = fixed_point(raw, step)?;
    if key.len() > MAX_BYTES {
        return Err(KeyError::TooLong);
    }
    Ok(key)
}

/// One application of the principal-key rules, without the length limit.
fn step(raw: &str) -> Result<String, KeyError> {
    let cleaned = clean(raw);
    if cleaned.is_empty() {
        return Err(KeyError::Empty);
    }
    check_chars(&cleaned)?;
    if cleaned.eq_ignore_ascii_case(USER) {
        return Ok(USER.to_owned());
    }
    let (scheme, value) = split_scheme(&cleaned)?;
    if scheme == USER {
        return Err(KeyError::BadScheme);
    }
    if value.is_empty() {
        return Err(KeyError::BadValue("the value after the scheme is empty"));
    }
    Ok(if scheme == "mail" {
        format!("{scheme}:{}", mail(value)?)
    } else {
        format!("{scheme}:{value}")
    })
}

/// A bare address, lower-cased: one pair of surrounding angle brackets is stripped, and any other
/// angle bracket is refused, so that a header's `Bob <bob@x>` is refused rather than becoming a
/// second principal beside `mail:bob@x`.
fn mail(value: &str) -> Result<String, KeyError> {
    let address = strip_angle_pair(value)?;
    if address.contains(['<', '>']) {
        return Err(KeyError::BadValue(
            "mail: a principal is a bare address, without a display name or angle brackets",
        ));
    }
    Ok(address.to_lowercase())
}
