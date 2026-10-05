//! The request digest and the question digest (design D.11.5, O1, A-39, CS-35).
//!
//! **Request digest.** When a call carries an `op_id`, right after secret masking and before
//! anything else touches the arguments: `SHA-256(tool ‖ 0x00 ‖ canonical_json(masked arguments
//! without "session" and "op_id"))`. Canonical JSON: object keys sorted by their bytes, no
//! insignificant whitespace, strings as `serde_json` writes them, and integers only (a float
//! anywhere is refused by the decoder before this runs). The digest is a function of what the
//! client sent and of the masking version only: it comes before normalisation, tz tables, and
//! search, so a retry never conflicts because shell-derived data changed.
//!
//! **Question digest.** For `talk ask {to: user | principal:<p>}`: `SHA-256(canonical_json([masked
//! text, masked options]))`, so the core can tell an identical question from a different one
//! without holding its text (D.8.12).

use forum_core::ReqDigest;
use serde_json::{Map, Value};

use crate::auth::sha256;

/// The arguments that never enter the request digest.
pub const EXCLUDED: [&str; 2] = ["session", "op_id"];

/// Writes `v` as canonical JSON. None if `v` holds a float.
#[must_use]
pub fn canonical_json(v: &Value) -> Option<String> {
    let mut out = String::new();
    write(v, &mut out)?;
    Some(out)
}

fn write(v: &Value, out: &mut String) -> Option<()> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                return None;
            }
        }
        Value::String(s) => out.push_str(&serde_json::to_string(s).ok()?),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => write_object(map.iter(), out)?,
    }
    Some(())
}

fn write_object<'a>(
    entries: impl Iterator<Item = (&'a String, &'a Value)>,
    out: &mut String,
) -> Option<()> {
    let mut entries: Vec<(&String, &Value)> = entries.collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    out.push('{');
    for (i, (k, v)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(k).ok()?);
        out.push(':');
        write(v, out)?;
    }
    out.push('}');
    Some(())
}

/// The request digest of a call to `tool` with the masked arguments `args` (`session` and `op_id`
/// are left out here). None if the arguments hold a float.
#[must_use]
pub fn request_digest(tool: &str, args: &Map<String, Value>) -> Option<ReqDigest> {
    let mut body = String::new();
    write_object(
        args.iter().filter(|(k, _)| !EXCLUDED.contains(&k.as_str())),
        &mut body,
    )?;
    let mut bytes = Vec::with_capacity(tool.len() + 1 + body.len());
    bytes.extend_from_slice(tool.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(body.as_bytes());
    Some(ReqDigest(sha256(&bytes)))
}

/// The question digest of a question for a human: its masked text and options.
#[must_use]
pub fn question_digest(text: &str, options: &[String]) -> ReqDigest {
    let v = Value::Array(vec![
        Value::String(text.to_string()),
        Value::Array(options.iter().cloned().map(Value::String).collect()),
    ]);
    // Strings and arrays only: canonical_json cannot fail here.
    let body = canonical_json(&v).unwrap_or_default();
    ReqDigest(sha256(body.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::hex;
    use serde_json::json;

    fn args(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => panic!("an object"),
        }
    }

    fn digest_hex(tool: &str, v: Value) -> String {
        hex(&request_digest(tool, &args(v)).unwrap().0)
    }

    #[test]
    fn canonical_form() {
        let v = json!({"b": 1, "a": [true, null, -3, "x\n\"y\u{1f}é/"], "A": {"z": {}, "y": []}});
        assert_eq!(
            canonical_json(&v).unwrap(),
            r#"{"A":{"y":[],"z":{}},"a":[true,null,-3,"x\n\"y\u001fé/"],"b":1}"#
        );
        assert_eq!(
            canonical_json(&json!(u64::MAX)).unwrap(),
            "18446744073709551615"
        );
        assert_eq!(canonical_json(&json!(1.5)), None);
        assert_eq!(canonical_json(&json!({"a": [0.0]})), None);
        // Keys sort by bytes: "Z" (0x5a) < "a" (0x61) < "é" (0xc3 0xa9).
        assert_eq!(
            canonical_json(&json!({"é": 1, "a": 2, "Z": 3})).unwrap(),
            r#"{"Z":3,"a":2,"é":1}"#
        );
    }

    /// Golden vectors, computed independently with Python's `hashlib.sha256(tool.encode() + b"\0"
    /// + json.dumps(args, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode())`.
    #[test]
    fn golden_vectors() {
        assert_eq!(
            digest_hex("hold", json!({"action": "next"})),
            "0852020861da32a28dfcc4644559a7f55c1d1ea3da13dbf3021fd3e729647352"
        );
        assert_eq!(
            digest_hex(
                "node",
                json!({"action": "create", "path": "/digest/daily", "title": "Daily «tickets»",
                       "keys": ["user:e1"], "capacity": 2, "take": false})
            ),
            "5f4cb462a53efb1cf358c19819ed330831ba8d5a56494e75361b89699c61482e"
        );
        assert_eq!(
            digest_hex(
                "talk",
                json!({"action": "say", "at": "#12", "text": "line one\nline \"two\"", "until": "in 10m"})
            ),
            "658328958dff8e008e75d65786d1f740fa1e22fcf4ab4020c9026adc40171fbe"
        );
    }

    #[test]
    fn key_order_whitespace_session_and_op_id_do_not_matter() {
        let a: Map<String, Value> = serde_json::from_str(
            r#"{"action":"checkpoint","token":"12.3","done":"d","next_step":"n"}"#,
        )
        .unwrap();
        let b: Map<String, Value> = serde_json::from_str(
            "{ \"next_step\" : \"n\",\n \"done\": \"d\", \"token\": \"12.3\", \"action\": \"checkpoint\",\
             \"session\": \"s-4\", \"op_id\": \"0b9e2c5e-1d7a-4b45-9a8c-3f8d2e0c6a11\" }",
        )
        .unwrap();
        assert_eq!(request_digest("hold", &a), request_digest("hold", &b));
        // The tool is part of the digest, and so is every other argument.
        assert_ne!(request_digest("hold", &a), request_digest("talk", &a));
        let mut c = a.clone();
        c.insert("intent".into(), json!("x"));
        assert_ne!(request_digest("hold", &a), request_digest("hold", &c));
        // A float anywhere has no digest (the decoder refuses it first).
        c.insert("lease".into(), json!(1.5));
        assert_eq!(request_digest("hold", &c), None);
    }

    #[test]
    fn question_digests() {
        let a = question_digest("Ship it?", &["yes".into(), "no".into()]);
        assert_eq!(a, question_digest("Ship it?", &["yes".into(), "no".into()]));
        assert_ne!(a, question_digest("Ship it?", &["no".into(), "yes".into()]));
        assert_ne!(a, question_digest("Ship it!", &["yes".into(), "no".into()]));
        assert_ne!(
            question_digest("q", &[]),
            question_digest("q", &["".into()])
        );
        assert_eq!(
            hex(&question_digest("Ship it?", &["yes".into(), "no".into()]).0),
            "ec14f233dd0257ec4651628546847440f29ccd498ba2134d60fc5a6f05fd6259"
        );
    }
}
