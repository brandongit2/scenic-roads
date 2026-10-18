//! What changed between two catalogs, as JSON (docs/formats.md, Delta catalog): `diff(a, b)` is a
//! patch that `apply` turns `a` into `b`, exactly (`apply(a, &diff(a, b)) == b` for any two JSON
//! values: the tests' property). A patch is one of
//! - `{"=": v}`: the value is `v`;
//! - `{"o": {key: patch | null}}`: an object, each key named changed by its patch (null: gone),
//!   the others as they were;
//! - `{"a": {"ids": [id, …], "set": {id: patch}}}`: an array of objects each with a distinct string
//!   `id` (the coverage's regions), in the order `ids` lists them, each one `set` names changed by
//!   its patch (from nothing, for one that's new), the others as they were.
//!
//! The diff takes whichever of these is shortest written out, so a catalog's patch names what
//! changed (a region's shapes, the files of the tiles drawn again) and not what didn't.

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The patch that makes `a` into `b`.
pub fn diff(a: &Value, b: &Value) -> Value {
    let whole = || one("=", b.clone());
    if a == b {
        return if a.is_object() { one("o", Value::Object(Map::new())) } else { whole() };
    }
    let made = match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut o = Map::new();
            for k in x.keys().filter(|k| !y.contains_key(*k)) {
                o.insert(k.clone(), Value::Null);
            }
            for (k, v) in y {
                match x.get(k) {
                    Some(w) if w == v => {}
                    Some(w) => {
                        o.insert(k.clone(), diff(w, v));
                    }
                    None => {
                        o.insert(k.clone(), one("=", v.clone()));
                    }
                }
            }
            Some(one("o", Value::Object(o)))
        }
        (Value::Array(x), Value::Array(y)) => match (by_id(x), by_id(y)) {
            (Some(xs), Some(_)) => {
                let ids: Vec<Value> = y.iter().map(|e| e["id"].clone()).collect();
                let mut set = Map::new();
                for e in y {
                    let id = e["id"].as_str().unwrap_or_default();
                    match xs.get(id) {
                        Some(w) if *w == e => {}
                        Some(w) => {
                            set.insert(id.to_string(), diff(w, e));
                        }
                        None => {
                            set.insert(id.to_string(), one("=", e.clone()));
                        }
                    }
                }
                let mut a = Map::new();
                a.insert("ids".into(), Value::Array(ids));
                a.insert("set".into(), Value::Object(set));
                Some(one("a", Value::Object(a)))
            }
            _ => None,
        },
        _ => None,
    };
    match made {
        Some(p) if len(&p) < len(&whole()) => p,
        _ => whole(),
    }
}

/// `a` with patch `p` applied (an error when `p` isn't a patch, or names an element `a` hasn't).
pub fn apply(a: &Value, p: &Value) -> Result<Value> {
    let Some(o) = p.as_object().filter(|o| o.len() == 1) else { bail!("not a patch: {}", short(p)) };
    let (op, v) = o.iter().next().expect("one entry");
    match op.as_str() {
        "=" => Ok(v.clone()),
        "o" => {
            let Some(changes) = v.as_object() else { bail!("an object's patch isn't an object") };
            let mut out = a.as_object().cloned().unwrap_or_default();
            for (k, pk) in changes {
                if pk.is_null() {
                    out.remove(k);
                } else {
                    let now = apply(out.get(k).unwrap_or(&Value::Null), pk).with_context(|| format!("in {k}"))?;
                    out.insert(k.clone(), now);
                }
            }
            Ok(Value::Object(out))
        }
        "a" => {
            let (Some(ids), Some(set)) = (v["ids"].as_array(), v["set"].as_object()) else {
                bail!("an array's patch lacks ids or set")
            };
            let was = a.as_array().and_then(|x| by_id(x)).unwrap_or_default();
            ids.iter()
                .map(|id| {
                    let Some(id) = id.as_str() else { bail!("an id isn't a string") };
                    match (set.get(id), was.get(id)) {
                        (Some(pk), w) => {
                            apply(w.copied().unwrap_or(&Value::Null), pk).with_context(|| format!("in {id}"))
                        }
                        (None, Some(w)) => Ok((*w).clone()),
                        (None, None) => bail!("{id} is neither kept nor set"),
                    }
                })
                .collect::<Result<Vec<_>>>()
                .map(Value::Array)
        }
        _ => bail!("not a patch: {}", short(p)),
    }
}

/// The elements of an array by their `id`: when every one is an object with a string `id`, each
/// distinct (None otherwise: such an array is patched whole).
fn by_id(x: &[Value]) -> Option<BTreeMap<&str, &Value>> {
    let mut m = BTreeMap::new();
    for e in x {
        let id = e.as_object()?.get("id")?.as_str()?;
        if m.insert(id, e).is_some() {
            return None;
        }
    }
    Some(m)
}

fn one(op: &str, v: Value) -> Value {
    let mut m = Map::new();
    m.insert(op.into(), v);
    Value::Object(m)
}

fn len(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(80).collect()
}

/// The keys a patch changes at its top (an object's), for logs.
pub fn changed_keys(p: &Value) -> BTreeSet<String> {
    p.get("o").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A small deterministic generator (no clock, no randomness from outside): xorshift.
    struct Gen(u64);
    impl Gen {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn value(&mut self, depth: u32) -> Value {
            match self.below(if depth == 0 { 4 } else { 8 }) {
                0 => Value::Null,
                1 => json!(self.below(5)),
                2 => json!(format!("s{}", self.below(4))),
                3 => json!(self.below(2) == 0),
                4 | 5 => {
                    let n = self.below(5);
                    Value::Object((0..n).map(|_| (format!("k{}", self.below(6)), self.value(depth - 1))).collect())
                }
                6 => {
                    // (An array of objects with ids: the coverage's regions; sometimes a repeat.)
                    let n = self.below(5);
                    Value::Array(
                        (0..n)
                            .map(|_| json!({"id": format!("r{}", self.below(6)), "v": self.value(depth - 1)}))
                            .collect(),
                    )
                }
                _ => {
                    let n = self.below(4);
                    Value::Array((0..n).map(|_| self.value(depth - 1)).collect())
                }
            }
        }
        /// `v` changed a little (so pairs share most of their parts, as catalogs do).
        fn nudge(&mut self, v: &Value, depth: u32) -> Value {
            if self.below(5) == 0 || depth == 0 {
                return self.value(depth);
            }
            match v {
                Value::Object(o) => {
                    let mut o = o.clone();
                    let keys: Vec<String> = o.keys().cloned().collect();
                    for k in keys {
                        match self.below(4) {
                            0 => {
                                o.remove(&k);
                            }
                            1 => {
                                let w = self.nudge(&o[&k], depth - 1);
                                o.insert(k, w);
                            }
                            _ => {}
                        }
                    }
                    if self.below(3) == 0 {
                        o.insert(format!("k{}", self.below(8)), self.value(depth - 1));
                    }
                    Value::Object(o)
                }
                Value::Array(x) => {
                    let mut out = Vec::new();
                    for e in x {
                        if self.below(5) == 0 {
                            continue;
                        }
                        out.push(if self.below(3) == 0 { self.nudge(e, depth - 1) } else { e.clone() });
                    }
                    let mut x = out;
                    if self.below(3) == 0 {
                        x.reverse();
                    }
                    Value::Array(x)
                }
                _ => self.value(depth),
            }
        }
    }

    #[test]
    fn any_patch_makes_its_target_exactly() {
        let mut g = Gen(0x9e37_79b9_7f4a_7c15);
        for _ in 0..20_000 {
            let a = g.value(4);
            let b = if g.below(2) == 0 { g.nudge(&a, 4) } else { g.value(4) };
            let p = diff(&a, &b);
            assert_eq!(apply(&a, &p).unwrap(), b, "a {a}\nb {b}\npatch {p}");
            // (Round-tripped as JSON too, as it's stored.)
            let back: Value = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
            assert_eq!(apply(&a, &back).unwrap(), b);
        }
    }

    #[test]
    fn a_patch_names_only_what_changed() {
        let big: Vec<u32> = (0..200).collect();
        let a = json!({"files": {"x": {"file": "x.1"}, "y": {"file": "y.1"}}, "coverage": {"regions": [{"id": "a", "shapes": big}, {"id": "b", "shapes": [4, 5, 6]}]}, "n": 1});
        let b = json!({"files": {"x": {"file": "x.2"}, "y": {"file": "y.1"}, "z": {"file": "z.1"}}, "coverage": {"regions": [{"id": "a", "shapes": big}, {"id": "b", "shapes": [4, 5, 7]}, {"id": "c", "shapes": []}]}, "n": 2});
        let p = diff(&a, &b);
        let s = p.to_string();
        assert!(!s.contains("y.1") && !s.contains("198,199"), "{s}");
        assert_eq!(changed_keys(&p), ["coverage", "files", "n"].map(String::from).into());
        assert_eq!(apply(&a, &p).unwrap(), b);
        // Nothing changed: an empty object patch.
        assert_eq!(diff(&a, &a), json!({"o": {}}));
        assert_eq!(apply(&a, &diff(&a, &a)).unwrap(), a);
    }

    #[test]
    fn a_patch_that_doesnt_fit_is_an_error() {
        assert!(apply(&json!({}), &json!({"x": 1})).is_err());
        assert!(apply(&json!([]), &json!({"a": {"ids": ["q"], "set": {}}})).is_err());
        assert!(apply(&json!(1), &json!({"=": 1, "o": {}})).is_err());
    }
}
