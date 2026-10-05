//! A map whose keys are neither strings nor integers, encoded as a list of `[key, value]` pairs in
//! key order (design D.7.6), which is canonical too: `#[serde(with = "forum_core::serde_pairs")]`.
//!
//! JSON object keys are strings, so such a map cannot be a JSON object. Decoding rejects a list
//! that repeats a key, which a canonical encoding never does.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Writes the map as a sequence of `(key, value)` pairs in key order.
///
/// # Errors
///
/// Whatever the serializer reports.
pub fn serialize<K, V, S>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    K: Serialize,
    V: Serialize,
    S: Serializer,
{
    serializer.collect_seq(map.iter())
}

/// Reads a sequence of `(key, value)` pairs into a map.
///
/// # Errors
///
/// Whatever the deserializer reports, and a repeated key.
pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let pairs: Vec<(K, V)> = Vec::deserialize(deserializer)?;
    let len = pairs.len();
    let map: BTreeMap<K, V> = pairs.into_iter().collect();
    if map.len() == len {
        Ok(map)
    } else {
        Err(D::Error::custom("a list of map pairs repeats a key"))
    }
}
