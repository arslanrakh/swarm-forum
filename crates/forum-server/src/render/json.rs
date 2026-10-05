//! Ordered compact JSON for the text content (design D.11.7).
//!
//! The text content renders each response's body as compact JSON whose keys keep the order of
//! §20 (a `hello`'s sections in §20.3's order, a reply's fields in its tool's). [`Json`] keeps
//! insertion order, measures its rendered size, and converts to a `serde_json::Value` for
//! `structuredContent`, which keeps the same order (1.5, §20.1, R3: a client that shows its model
//! only the structured content reads it from the top), since this crate builds `serde_json` with
//! its feature `preserve_order`.

use serde_json::Value;

/// A JSON value whose objects keep their keys in insertion order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An unsigned integer.
    U(u64),
    /// A signed integer.
    I(i64),
    /// A string.
    S(String),
    /// A list.
    A(Vec<Json>),
    /// An object, in insertion order.
    O(Vec<(String, Json)>),
}

impl Json {
    /// A string.
    pub fn s(v: impl Into<String>) -> Json {
        Json::S(v.into())
    }

    /// An empty object.
    #[must_use]
    pub fn obj() -> Json {
        Json::O(Vec::new())
    }

    /// The object with `key` set to `v` (appended; a key already present is replaced in place).
    #[must_use]
    pub fn with(mut self, key: &str, v: Json) -> Json {
        self.put(key, v);
        self
    }

    /// The object with `key` set to `v` when `v` is `Some`.
    #[must_use]
    pub fn with_opt(self, key: &str, v: Option<Json>) -> Json {
        match v {
            Some(v) => self.with(key, v),
            None => self,
        }
    }

    /// Whether this value is set (1.7, §20.1): not null, and not an empty text, list, or object.
    #[must_use]
    pub fn is_set(&self) -> bool {
        match self {
            Json::Null => false,
            Json::S(x) => !x.is_empty(),
            Json::A(_) | Json::O(_) => !self.is_empty_container(),
            Json::Bool(_) | Json::U(_) | Json::I(_) => true,
        }
    }

    /// The object with `key` set to `v` when `v` is set (1.7, §20.1: a response shows only what is
    /// set, so an absent field is empty, zero, or its default).
    #[must_use]
    pub fn with_set(self, key: &str, v: Json) -> Json {
        if v.is_set() { self.with(key, v) } else { self }
    }

    /// Sets `key` to `v` in an object (appended, or replaced in place). Nothing for another value.
    pub fn put(&mut self, key: &str, v: Json) {
        if let Json::O(fields) = self {
            if let Some(slot) = fields.iter_mut().find(|(k, _)| k == key) {
                slot.1 = v;
            } else {
                fields.push((key.to_string(), v));
            }
        }
    }

    /// The value of `key` in an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::O(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            Json::Null | Json::Bool(_) | Json::U(_) | Json::I(_) | Json::S(_) | Json::A(_) => None,
        }
    }

    /// Whether this is an empty list or object.
    #[must_use]
    pub fn is_empty_container(&self) -> bool {
        match self {
            Json::A(v) => v.is_empty(),
            Json::O(v) => v.is_empty(),
            Json::Null | Json::Bool(_) | Json::U(_) | Json::I(_) | Json::S(_) => false,
        }
    }

    /// A list of strings.
    pub fn strs<I, T>(items: I) -> Json
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        Json::A(items.into_iter().map(|x| Json::S(x.into())).collect())
    }

    /// Appends the compact rendering to `out`.
    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::U(n) => out.push_str(&n.to_string()),
            Json::I(n) => out.push_str(&n.to_string()),
            Json::S(s) => write_str(s, out),
            Json::A(items) => {
                out.push('[');
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::O(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// The compact rendering.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    /// The rendered size in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.text().len()
    }

    /// The same value as `serde_json` holds it, its keys in the same order (D.11.7:
    /// `structuredContent` carries the same JSON, in §20.1's order, 1.5).
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::U(n) => Value::from(*n),
            Json::I(n) => Value::from(*n),
            Json::S(s) => Value::String(s.clone()),
            Json::A(items) => Value::Array(items.iter().map(Json::to_value).collect()),
            Json::O(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_value()))
                    .collect(),
            ),
        }
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::S(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::S(s)
    }
}

impl From<u64> for Json {
    fn from(n: u64) -> Json {
        Json::U(n)
    }
}

impl From<u32> for Json {
    fn from(n: u32) -> Json {
        Json::U(u64::from(n))
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

/// A JSON string literal, escaped as `serde_json` escapes it.
fn write_str(s: &str, out: &mut String) {
    match serde_json::to_string(s) {
        Ok(q) => out.push_str(&q),
        // Serialising a `&str` cannot fail; keep the output valid JSON regardless.
        Err(_) => out.push_str("\"\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_keep_their_order_and_render_compactly() {
        let j = Json::obj()
            .with("z", Json::U(1))
            .with("a", Json::s("x\"y\n"))
            .with(
                "m",
                Json::A(vec![Json::Bool(true), Json::Null, Json::I(-3)]),
            )
            .with_opt("none", None)
            .with("z", Json::U(2));
        assert_eq!(j.text(), r#"{"z":2,"a":"x\"y\n","m":[true,null,-3]}"#);
        assert_eq!(j.size(), j.text().len());
        let v = j.to_value();
        assert_eq!(serde_json::from_str::<Value>(&j.text()).unwrap(), v);
        // 1.5 (§20.1, R3): `structuredContent` keeps the order too, as serialised.
        assert_eq!(serde_json::to_string(&v).unwrap(), j.text());
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["z", "a", "m"]);
        assert_eq!(j.get("a"), Some(&Json::s("x\"y\n")));
        assert!(Json::A(vec![]).is_empty_container());
        assert!(!Json::U(0).is_empty_container());
    }
}
