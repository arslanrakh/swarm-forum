//! The Informal Trace Format (ITF, Apalache ADR-015) as Quint 0.33 writes it, read into a small
//! dynamically typed value, `V` (design D.10.8).
//!
//! Quint writes integers as `{"#bigint": "…"}` (a fixture may write those in JSON's safe range as
//! plain numbers, which ADR-015 allows), tuples as `{"#tup": […]}`, sets as `{"#set": […]}`, maps as
//! `{"#map": [[k, v], …]}`, sum types as `{"tag": …, "value": …}` (a variant without a payload has
//! the empty tuple), records as JSON objects, and lists as JSON arrays. The state variables are
//! qualified by module (`holds_two::forum::s`, D.14.5): `Trace::var` finds a variable by its last
//! segment.
//!
//! The projection of the core's state (`crate::project`) builds the same values, so a model state
//! and a projected Rust state compare with `==`, and `diff` names the paths where they differ.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

/// A Quint value.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum V {
    /// A boolean.
    Bool(bool),
    /// An integer (every model integer fits in 64 bits).
    Int(i64),
    /// A string.
    Str(String),
    /// A list.
    List(Vec<V>),
    /// A tuple.
    Tup(Vec<V>),
    /// A set.
    Set(BTreeSet<V>),
    /// A map.
    Map(BTreeMap<V, V>),
    /// A record.
    Rec(BTreeMap<String, V>),
    /// A variant of a sum type with its payload (the empty tuple for none).
    Tag(String, Box<V>),
}

/// The result of reading a value: the error says what was expected where.
pub type R<T> = Result<T, String>;

impl V {
    /// A variant without a payload.
    #[must_use]
    pub fn unit(tag: &str) -> V {
        V::Tag(tag.to_owned(), Box::new(V::Tup(Vec::new())))
    }

    /// A variant with a payload.
    #[must_use]
    pub fn tag(tag: &str, v: V) -> V {
        V::Tag(tag.to_owned(), Box::new(v))
    }

    /// A string.
    #[must_use]
    pub fn str(s: &str) -> V {
        V::Str(s.to_owned())
    }

    /// An integer from any integer type that fits.
    #[must_use]
    pub fn int<T: TryInto<i64>>(x: T) -> V {
        x.try_into().map_or_else(|_| V::str("<overflow>"), V::Int)
    }

    /// A record from its fields.
    #[must_use]
    pub fn rec<const N: usize>(fields: [(&str, V); N]) -> V {
        V::Rec(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    /// A pair.
    #[must_use]
    pub fn pair(a: V, b: V) -> V {
        V::Tup(vec![a, b])
    }

    /// A record's field.
    ///
    /// # Errors
    ///
    /// When `self` is not a record with that field.
    pub fn get(&self, field: &str) -> R<&V> {
        match self {
            V::Rec(m) => m
                .get(field)
                .ok_or_else(|| format!("no field `{field}` in {self}")),
            V::Bool(_)
            | V::Int(_)
            | V::Str(_)
            | V::List(_)
            | V::Tup(_)
            | V::Set(_)
            | V::Map(_)
            | V::Tag(..) => Err(format!("`{field}` of a non-record {self}")),
        }
    }

    /// The integer.
    ///
    /// # Errors
    ///
    /// When `self` is not an integer.
    pub fn as_int(&self) -> R<i64> {
        match self {
            V::Int(x) => Ok(*x),
            V::Bool(_)
            | V::Str(_)
            | V::List(_)
            | V::Tup(_)
            | V::Set(_)
            | V::Map(_)
            | V::Rec(_)
            | V::Tag(..) => Err(format!("an integer expected, found {self}")),
        }
    }

    /// The integer as a `u64`.
    ///
    /// # Errors
    ///
    /// When `self` is not a non-negative integer.
    pub fn as_u64(&self) -> R<u64> {
        let x = self.as_int()?;
        u64::try_from(x).map_err(|_| format!("a non-negative integer expected, found {x}"))
    }

    /// The boolean.
    ///
    /// # Errors
    ///
    /// When `self` is not a boolean.
    pub fn as_bool(&self) -> R<bool> {
        match self {
            V::Bool(b) => Ok(*b),
            V::Int(_)
            | V::Str(_)
            | V::List(_)
            | V::Tup(_)
            | V::Set(_)
            | V::Map(_)
            | V::Rec(_)
            | V::Tag(..) => Err(format!("a boolean expected, found {self}")),
        }
    }

    /// The string.
    ///
    /// # Errors
    ///
    /// When `self` is not a string.
    pub fn as_str(&self) -> R<&str> {
        match self {
            V::Str(s) => Ok(s),
            V::Bool(_)
            | V::Int(_)
            | V::List(_)
            | V::Tup(_)
            | V::Set(_)
            | V::Map(_)
            | V::Rec(_)
            | V::Tag(..) => Err(format!("a string expected, found {self}")),
        }
    }

    /// The variant and its payload.
    ///
    /// # Errors
    ///
    /// When `self` is not a variant.
    pub fn as_tag(&self) -> R<(&str, &V)> {
        match self {
            V::Tag(t, v) => Ok((t, v)),
            V::Bool(_)
            | V::Int(_)
            | V::Str(_)
            | V::List(_)
            | V::Tup(_)
            | V::Set(_)
            | V::Map(_)
            | V::Rec(_) => Err(format!("a variant expected, found {self}")),
        }
    }

    /// The elements of a set.
    ///
    /// # Errors
    ///
    /// When `self` is not a set.
    pub fn as_set(&self) -> R<&BTreeSet<V>> {
        match self {
            V::Set(s) => Ok(s),
            V::Bool(_)
            | V::Int(_)
            | V::Str(_)
            | V::List(_)
            | V::Tup(_)
            | V::Map(_)
            | V::Rec(_)
            | V::Tag(..) => Err(format!("a set expected, found {self}")),
        }
    }

    /// The entries of a map.
    ///
    /// # Errors
    ///
    /// When `self` is not a map.
    pub fn as_map(&self) -> R<&BTreeMap<V, V>> {
        match self {
            V::Map(m) => Ok(m),
            V::Bool(_)
            | V::Int(_)
            | V::Str(_)
            | V::List(_)
            | V::Tup(_)
            | V::Set(_)
            | V::Rec(_)
            | V::Tag(..) => Err(format!("a map expected, found {self}")),
        }
    }

    /// The elements of a list or a tuple.
    ///
    /// # Errors
    ///
    /// When `self` is neither.
    pub fn as_seq(&self) -> R<&[V]> {
        match self {
            V::List(xs) | V::Tup(xs) => Ok(xs),
            V::Bool(_) | V::Int(_) | V::Str(_) | V::Set(_) | V::Map(_) | V::Rec(_) | V::Tag(..) => {
                Err(format!("a list or a tuple expected, found {self}"))
            }
        }
    }

    /// The pair of integers of a tuple `(a, b)`, as a model token is.
    ///
    /// # Errors
    ///
    /// When `self` is not a pair of integers.
    pub fn as_int_pair(&self) -> R<(i64, i64)> {
        match self.as_seq()? {
            [a, b] => Ok((a.as_int()?, b.as_int()?)),
            _ => Err(format!("a pair expected, found {self}")),
        }
    }

    /// Reads an ITF value.
    ///
    /// # Errors
    ///
    /// On an encoding this reader does not know, or an integer beyond 64 bits.
    pub fn from_json(j: &serde_json::Value) -> R<V> {
        use serde_json::Value as J;
        match j {
            J::Null => Err("null in an ITF value".to_owned()),
            J::Bool(b) => Ok(V::Bool(*b)),
            J::Number(n) => n
                .as_i64()
                .map(V::Int)
                .ok_or_else(|| format!("a non-integer number {n}")),
            J::String(s) => Ok(V::Str(s.clone())),
            J::Array(xs) => xs.iter().map(V::from_json).collect::<R<_>>().map(V::List),
            J::Object(o) => {
                if let Some(b) = o.get("#bigint") {
                    let s = b.as_str().ok_or("a #bigint that is not a string")?;
                    return s
                        .parse::<i64>()
                        .map(V::Int)
                        .map_err(|e| format!("#bigint {s}: {e}"));
                }
                if let Some(J::Array(xs)) = o.get("#tup") {
                    return xs.iter().map(V::from_json).collect::<R<_>>().map(V::Tup);
                }
                if let Some(J::Array(xs)) = o.get("#set") {
                    return xs.iter().map(V::from_json).collect::<R<_>>().map(V::Set);
                }
                if let Some(J::Array(xs)) = o.get("#map") {
                    let mut m = BTreeMap::new();
                    for kv in xs {
                        match kv.as_array().map(Vec::as_slice) {
                            Some([k, v]) => {
                                m.insert(V::from_json(k)?, V::from_json(v)?);
                            }
                            _ => return Err(format!("a #map entry that is not a pair: {kv}")),
                        }
                    }
                    return Ok(V::Map(m));
                }
                if o.contains_key("#unserializable") {
                    return Err(format!("an unserializable ITF value: {j}"));
                }
                if o.len() == 2
                    && let (Some(J::String(t)), Some(v)) = (o.get("tag"), o.get("value"))
                {
                    return Ok(V::Tag(t.clone(), Box::new(V::from_json(v)?)));
                }
                let mut m = BTreeMap::new();
                for (k, v) in o {
                    if k != "#meta" {
                        m.insert(k.clone(), V::from_json(v)?);
                    }
                }
                Ok(V::Rec(m))
            }
        }
    }
}

impl fmt::Display for V {
    /// Quint's notation, compact.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn seq(f: &mut fmt::Formatter<'_>, xs: impl Iterator<Item = String>) -> fmt::Result {
            let v: Vec<String> = xs.collect();
            write!(f, "{}", v.join(", "))
        }
        match self {
            V::Bool(b) => write!(f, "{b}"),
            V::Int(x) => write!(f, "{x}"),
            V::Str(s) => write!(f, "{s:?}"),
            V::List(xs) => {
                write!(f, "[")?;
                seq(f, xs.iter().map(ToString::to_string))?;
                write!(f, "]")
            }
            V::Tup(xs) => {
                write!(f, "(")?;
                seq(f, xs.iter().map(ToString::to_string))?;
                write!(f, ")")
            }
            V::Set(xs) => {
                write!(f, "Set(")?;
                seq(f, xs.iter().map(ToString::to_string))?;
                write!(f, ")")
            }
            V::Map(m) => {
                write!(f, "Map(")?;
                seq(f, m.iter().map(|(k, v)| format!("{k} -> {v}")))?;
                write!(f, ")")
            }
            V::Rec(m) => {
                write!(f, "{{ ")?;
                seq(f, m.iter().map(|(k, v)| format!("{k}: {v}")))?;
                write!(f, " }}")
            }
            V::Tag(t, v) => match v.as_ref() {
                V::Tup(xs) if xs.is_empty() => write!(f, "{t}"),
                V::Tup(_) => write!(f, "{t}{v}"),
                V::Bool(_)
                | V::Int(_)
                | V::Str(_)
                | V::List(_)
                | V::Set(_)
                | V::Map(_)
                | V::Rec(_)
                | V::Tag(..) => write!(f, "{t}({v})"),
            },
        }
    }
}

/// One difference between a model value and a projected Rust value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diff {
    /// Where: field names, map keys, and indices from the compared root.
    pub path: String,
    /// The model's value, `None` when only Rust has the key or element.
    pub model: Option<V>,
    /// The projected Rust value, `None` when only the model has it.
    pub rust: Option<V>,
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |x: &Option<V>| {
            x.as_ref()
                .map_or_else(|| "(absent)".to_owned(), V::to_string)
        };
        write!(
            f,
            "{}: model {} / rust {}",
            self.path,
            show(&self.model),
            show(&self.rust)
        )
    }
}

/// The differences between `model` and `rust`, descending into records, maps, sets, lists, tuples,
/// and variants with the same tag, so that a divergence names the smallest part that differs.
#[must_use]
pub fn diff(path: &str, model: &V, rust: &V) -> Vec<Diff> {
    let mut out = Vec::new();
    diff_into(path, model, rust, &mut out);
    out
}

fn diff_into(path: &str, model: &V, rust: &V, out: &mut Vec<Diff>) {
    if model == rust {
        return;
    }
    let differ = |out: &mut Vec<Diff>| {
        out.push(Diff {
            path: path.to_owned(),
            model: Some(model.clone()),
            rust: Some(rust.clone()),
        });
    };
    match (model, rust) {
        (V::Rec(a), V::Rec(b)) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let p = format!("{path}.{k}");
                match (a.get(k), b.get(k)) {
                    (Some(x), Some(y)) => diff_into(&p, x, y, out),
                    (x, y) => out.push(Diff {
                        path: p,
                        model: x.cloned(),
                        rust: y.cloned(),
                    }),
                }
            }
        }
        (V::Map(a), V::Map(b)) => {
            let keys: BTreeSet<&V> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let p = format!("{path}[{k}]");
                match (a.get(k), b.get(k)) {
                    (Some(x), Some(y)) => diff_into(&p, x, y, out),
                    (x, y) => out.push(Diff {
                        path: p,
                        model: x.cloned(),
                        rust: y.cloned(),
                    }),
                }
            }
        }
        (V::Set(a), V::Set(b)) => {
            for x in a.difference(b) {
                out.push(Diff {
                    path: format!("{path}{{}}"),
                    model: Some(x.clone()),
                    rust: None,
                });
            }
            for y in b.difference(a) {
                out.push(Diff {
                    path: format!("{path}{{}}"),
                    model: None,
                    rust: Some(y.clone()),
                });
            }
        }
        (V::List(a), V::List(b)) | (V::Tup(a), V::Tup(b)) if a.len() == b.len() => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff_into(&format!("{path}[{i}]"), x, y, out);
            }
        }
        (V::Tag(t, x), V::Tag(u, y)) if t == u => diff_into(&format!("{path}.{t}"), x, y, out),
        _ => differ(out),
    }
}

/// An ITF trace: its states, each a map from the variable's last name segment to its value.
#[derive(Clone, Debug)]
pub struct Trace {
    /// The module that qualifies the state variables (`holds_two` in `holds_two::forum::s`), or
    /// "" when they are not qualified.
    pub module: String,
    /// `#meta.status`: "ok" for a trace, "violation" for a counterexample.
    pub status: String,
    /// The states, in order.
    pub states: Vec<BTreeMap<String, V>>,
}

impl Trace {
    /// Reads an ITF file.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or is not ITF as Quint writes it.
    pub fn read(path: &Path) -> R<Trace> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let j: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Trace::from_json(&j).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Reads an ITF document.
    ///
    /// # Errors
    ///
    /// When it is not ITF as Quint writes it.
    pub fn from_json(j: &serde_json::Value) -> R<Trace> {
        let status = j
            .get("#meta")
            .and_then(|m| m.get("status"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned();
        let states = j
            .get("states")
            .and_then(serde_json::Value::as_array)
            .ok_or("no `states` array")?;
        let mut module = String::new();
        let mut out = Vec::new();
        for st in states {
            let o = st.as_object().ok_or("a state that is not an object")?;
            let mut m = BTreeMap::new();
            for (k, v) in o {
                if k == "#meta" {
                    continue;
                }
                // `mbt::actionTaken` keeps its prefix; a model variable is known by its last
                // segment (`holds_two::forum::s` is `s`).
                let name = if k.starts_with("mbt::") {
                    k.clone()
                } else {
                    let mut parts = k.rsplit("::");
                    let last = parts.next().unwrap_or(k).to_owned();
                    if module.is_empty() {
                        let rest: Vec<&str> = parts.collect();
                        if let Some(first) = rest.last() {
                            (*first).clone_into(&mut module);
                        }
                    }
                    last
                };
                m.insert(
                    name,
                    V::from_json(v).map_err(|e| format!("variable {k}: {e}"))?,
                );
            }
            out.push(m);
        }
        Ok(Trace {
            module,
            status,
            states: out,
        })
    }
}

/// A state variable of one ITF state.
///
/// # Errors
///
/// When the state has no such variable.
pub fn var<'a>(st: &'a BTreeMap<String, V>, name: &str) -> R<&'a V> {
    st.get(name)
        .ok_or_else(|| format!("no state variable `{name}`"))
}
