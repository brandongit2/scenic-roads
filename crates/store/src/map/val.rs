//! An entry's value kept as its canonical JSON (docs/maplog.md §4.3: the bytes its hash is over),
//! not as a `serde_json::Value`. The map's regions are most of its bytes (their outlines' points),
//! and a `Value` tree costs several times its JSON (#936: 691 MB for the lead's 80 MB map of
//! 4 Nov, 4.5 million allocations; 82 MB as text). A value is parsed only where it's read.
//!
//! Canonical is what serializing the `Value` gives (its objects' keys sorted, as serde_json's maps
//! are; numbers and strings as serde_json writes them): a `Val` is made from a `Value`, or read from
//! JSON through `Canon`, which writes what parsing the text into a `Value` and serializing it would,
//! without the tree (the same visits as `Value`'s own `Deserialize`). Batches, snapshots and the
//! digest are therefore the bytes they were with `Value`s.

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;

/// One entry's value: its canonical JSON, shared by the batch that set it and the maps holding it.
#[derive(Clone)]
pub struct Val(Arc<RawValue>);

impl Val {
    /// The value whose canonical JSON `json` is (the caller's word: `Canon`'s output, a `Value`'s
    /// serialization).
    pub(crate) fn of_canonical(json: String) -> Val {
        let raw = RawValue::from_string(json).expect("canonical JSON is JSON");
        Val(Arc::from(raw))
    }

    /// Whether it's the same text as `o`, not a copy of it (tests).
    #[cfg(test)]
    pub(crate) fn shares(&self, o: &Val) -> bool {
        Arc::ptr_eq(&self.0, &o.0)
    }

    /// Its canonical JSON.
    pub fn json(&self) -> &str {
        self.0.get()
    }

    /// It parsed as a `T`.
    pub fn parse<'a, T: Deserialize<'a>>(&'a self) -> serde_json::Result<T> {
        serde_json::from_str(self.json())
    }

    /// It as a `Value`, each list in it allocated to its length (`Exact`): a region's points as
    /// serde_json parses them would hold twice their room (a list of two numbers grown to four).
    pub fn value(&self) -> Value {
        let mut de = serde_json::Deserializer::from_str(self.json());
        let v = Exact(&mut Vec::new()).deserialize(&mut de).expect("a value's own JSON parses");
        de.end().expect("a value's own JSON is one value");
        v
    }

    /// The text of a string value; None for any other value.
    pub fn as_str(&self) -> Option<Cow<'_, str>> {
        let j = self.json();
        let inner = j.strip_prefix('"')?.strip_suffix('"')?;
        // (serde_json escapes only `"`, `\` and control characters: with no `\`, the text as it is.)
        match inner.contains('\\') {
            false => Some(Cow::Borrowed(inner)),
            true => self.parse::<String>().ok().map(Cow::Owned),
        }
    }
}

impl From<&Value> for Val {
    fn from(v: &Value) -> Val {
        Val::of_canonical(serde_json::to_string(v).expect("a JSON value always serializes"))
    }
}

impl From<Value> for Val {
    fn from(v: Value) -> Val {
        Val::from(&v)
    }
}

impl PartialEq for Val {
    fn eq(&self, o: &Val) -> bool {
        Arc::ptr_eq(&self.0, &o.0) || self.json() == o.json()
    }
}

impl std::fmt::Debug for Val {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.json())
    }
}

impl Serialize for Val {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Val, D::Error> {
        let mut b = Vec::new();
        Canon(&mut b).deserialize(d)?;
        let s = String::from_utf8(b).map_err(de::Error::custom)?;
        Ok(Val::of_canonical(s))
    }
}

/// Writes the value it's handed as canonical JSON onto its buffer: what `Value`'s `Deserialize`
/// would make of it, serialized (visit for visit the same: numbers as `Value::from` the visited
/// number, an object's keys sorted, the last of a key given twice kept).
struct Canon<'a>(&'a mut Vec<u8>);

impl Canon<'_> {
    fn put<E: de::Error>(&mut self, v: &(impl Serialize + ?Sized)) -> Result<(), E> {
        serde_json::to_writer(&mut *self.0, v).map_err(E::custom)
    }
}

impl<'de> DeserializeSeed<'de> for Canon<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Canon<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<(), E> {
        self.0.extend_from_slice(if v { b"true" } else { b"false" });
        Ok(())
    }

    fn visit_i64<E: de::Error>(mut self, v: i64) -> Result<(), E> {
        self.put(&Value::from(v))
    }

    fn visit_u64<E: de::Error>(mut self, v: u64) -> Result<(), E> {
        self.put(&Value::from(v))
    }

    fn visit_f64<E: de::Error>(mut self, v: f64) -> Result<(), E> {
        self.put(&Value::from(v))
    }

    fn visit_str<E: de::Error>(mut self, v: &str) -> Result<(), E> {
        self.put(v)
    }

    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        self.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.0.extend_from_slice(b"null");
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let b = self.0;
        b.push(b'[');
        let mut first = true;
        loop {
            // (The comma before the element, taken back when there's none.)
            let mark = b.len();
            if !first {
                b.push(b',');
            }
            if seq.next_element_seed(Canon(&mut *b))?.is_none() {
                b.truncate(mark);
                break;
            }
            first = false;
        }
        b.push(b']');
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let b = self.0;
        let start = b.len();
        b.push(b'{');
        // (Each member written where it comes, its key and span kept: in order, as canonical JSON
        // is, nothing more to do; else the members are put in order, the last of a key kept.)
        let mut spans: Vec<(String, usize, usize)> = Vec::new();
        let mut sorted = true;
        while let Some(k) = map.next_key::<String>()? {
            if !spans.is_empty() {
                b.push(b',');
            }
            let from = b.len();
            Canon(&mut *b).put(&k)?;
            b.push(b':');
            map.next_value_seed(Canon(&mut *b))?;
            sorted &= spans.last().is_none_or(|l| l.0 < k);
            spans.push((k, from, b.len()));
        }
        if sorted {
            b.push(b'}');
            return Ok(());
        }
        let members: std::collections::BTreeMap<String, (usize, usize)> =
            spans.into_iter().map(|(k, from, to)| (k, (from, to))).collect();
        let mut out = Vec::with_capacity(b.len() - start + 1);
        out.push(b'{');
        for (i, (from, to)) in members.into_values().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend_from_slice(&b[from..to]);
        }
        out.push(b'}');
        b.truncate(start);
        b.extend_from_slice(&out);
        Ok(())
    }
}

/// Makes the `Value` it's handed as `Value`'s own `Deserialize` would (visit for visit), but each
/// list allocated once to its length: its items gathered on a stack shared by the lists inside it,
/// then moved off it.
struct Exact<'a>(&'a mut Vec<Value>);

impl<'de> DeserializeSeed<'de> for Exact<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Exact<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let stack = self.0;
        let base = stack.len();
        while let Some(v) = seq.next_element_seed(Exact(&mut *stack))? {
            stack.push(v);
        }
        Ok(Value::Array(stack.drain(base..).collect()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let stack = self.0;
        let mut o = serde_json::Map::new();
        while let Some(k) = map.next_key::<String>()? {
            let v = map.next_value_seed(Exact(&mut *stack))?;
            o.insert(k, v);
        }
        Ok(Value::Object(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Read from text: the bytes `Value` would serialize it to, for every kind of value, keys out of
    /// order, given twice, whitespace, escapes and numbers written another way.
    #[test]
    fn a_value_read_is_its_values_canonical_json() {
        for text in [
            r#"{"b": 1, "a": [1, 2.50, -3, 1e3, 18446744073709551615, 18446744073709551616], "c": null}"#,
            r#"{"z":{"y":true,"x":false},"z":{"w":"é\n\"q\""}, "a" : "\/" }"#,
            r#"[ ]"#,
            r#"{}"#,
            r#""plain""#,
            r#"-0"#,
            r#"0.1"#,
            r#"[{"b":[{"d":1,"c":2}],"a":{}}]"#,
            r#"{"regions":[{"id":"x","shapes":{"b":[[1.5,2.25]],"a":[]}}],"recorded":true}"#,
        ] {
            let v: Value = serde_json::from_str(text).unwrap();
            let got: Val = serde_json::from_str(text).unwrap();
            assert_eq!(got.json(), serde_json::to_string(&v).unwrap(), "{text}");
            assert_eq!(got, Val::from(&v));
            assert_eq!(got.value(), v);
        }
    }

    /// Random JSON texts (#936 review): numbers written every way (signs, zeros, fractions,
    /// exponents, past u64 and i64, past f64's range), strings with escapes, raw and escaped
    /// unicode, surrogate pairs and lone halves, keys given twice and out of byte order, nesting,
    /// whitespace. Read from text, bytes and a reader, and from the `Value`: a `Val` is the bytes
    /// the `Value` serializes to, or fails where the `Value` does.
    #[test]
    fn a_random_value_read_is_its_values_canonical_json() {
        struct Gen(u64);
        impl Gen {
            fn n(&mut self, k: u64) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0 % k
            }
            fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
                xs[self.n(xs.len() as u64) as usize]
            }
            fn ws(&mut self, o: &mut String) {
                o.push_str(self.pick(&["", "", "", " ", "\n\t ", "\r"]));
            }
            fn digits(&mut self, o: &mut String, most: u64) {
                for i in 0..1 + self.n(most) {
                    o.push((b'0' + self.n(10) as u8 + u8::from(i == 0 && self.n(4) == 0)).min(b'9') as char);
                }
            }
            fn number(&mut self, o: &mut String) {
                match self.n(8) {
                    0 => o.push_str(self.pick(&[
                        "0",
                        "-0",
                        "0.0",
                        "-0.0",
                        "-0e0",
                        "9223372036854775807",
                        "9223372036854775808",
                        "-9223372036854775808",
                        "-9223372036854775809",
                        "18446744073709551615",
                        "18446744073709551616",
                        "1e400",
                        "-1e400",
                        "1e-400",
                        "2.2250738585072014e-308",
                        "4.9e-324",
                        "1.7976931348623157e308",
                        "0.1",
                        "0.30000000000000004",
                        "123456789012345678901234567890",
                        "1E2",
                        "1e+2",
                        "1.5e-0",
                        "00",
                        "01",
                        "1.",
                        ".5",
                        "-",
                        "1e",
                        "+1",
                        "NaN",
                        "Infinity",
                    ])),
                    _ => {
                        if self.n(3) == 0 {
                            o.push('-');
                        }
                        if self.n(5) == 0 {
                            o.push('0');
                        } else {
                            o.push((b'1' + self.n(9) as u8) as char);
                            self.digits(o, 22);
                        }
                        if self.n(2) == 0 {
                            o.push('.');
                            self.digits(o, 20);
                        }
                        if self.n(3) == 0 {
                            o.push_str(self.pick(&["e", "E", "e+", "e-", "E-"]));
                            self.digits(o, 3);
                        }
                    }
                }
            }
            fn string(&mut self, o: &mut String, most: u64) {
                o.push('"');
                for _ in 0..self.n(most) {
                    o.push_str(self.pick(&[
                        "a",
                        "b",
                        "z",
                        "A",
                        "_",
                        "/",
                        "é",
                        "ß",
                        "\u{7f}",
                        "\u{80}",
                        "日本",
                        "😀",
                        "\\n",
                        "\\t",
                        "\\\"",
                        "\\\\",
                        "\\/",
                        "\\b",
                        "\\f",
                        "\\r",
                        "\\u0000",
                        "\\u001f",
                        "\\u0020",
                        "\\u00e9",
                        "\\u00E9",
                        "\\u2028",
                        "\\ud83d\\ude00",
                        "\\uD83D\\uDE00",
                        "\\ud800",
                        "\\udc00",
                        "\\ud83d",
                        "\\uffff",
                        "\\x",
                        "\\u12",
                        "\u{1}",
                        "\t",
                    ]));
                }
                o.push('"');
            }
            fn value(&mut self, o: &mut String, depth: u32) {
                self.ws(o);
                match if depth > 4 { self.n(4) } else { self.n(7) } {
                    0 => o.push_str(self.pick(&["true", "false", "null"])),
                    1 | 2 => self.number(o),
                    3 => self.string(o, 6),
                    4 | 5 => {
                        o.push('{');
                        for i in 0..self.n(6) {
                            if i > 0 {
                                o.push(',');
                            }
                            self.ws(o);
                            self.string(o, 2);
                            self.ws(o);
                            o.push(':');
                            self.value(o, depth + 1);
                        }
                        self.ws(o);
                        o.push('}');
                    }
                    _ => {
                        o.push('[');
                        for i in 0..self.n(6) {
                            if i > 0 {
                                o.push(',');
                            }
                            self.value(o, depth + 1);
                        }
                        o.push(']');
                    }
                }
                self.ws(o);
            }
        }
        let mut g = Gen(0x9e37_79b9_7f4a_7c15);
        let (mut good, mut bad) = (0, 0);
        for _ in 0..200_000 {
            let mut t = String::new();
            g.value(&mut t, 0);
            let want = serde_json::from_str::<Value>(&t).map(|v| serde_json::to_string(&v).unwrap());
            let got = serde_json::from_str::<Val>(&t).map(|v| v.json().to_string());
            assert_eq!(got.as_ref().ok(), want.as_ref().ok(), "{t}");
            let slice = serde_json::from_slice::<Val>(t.as_bytes()).map(|v| v.json().to_string());
            let reader = serde_json::from_reader::<_, Val>(t.as_bytes()).map(|v| v.json().to_string());
            assert_eq!((slice.ok(), reader.ok()), (want.as_ref().ok().cloned(), want.as_ref().ok().cloned()), "{t}");
            match want {
                Ok(w) => {
                    good += 1;
                    let v: Value = serde_json::from_str(&t).unwrap();
                    let val = Val::from(&v);
                    assert_eq!(val.json(), w, "{t}");
                    assert_eq!(serde_json::from_value::<Val>(v.clone()).unwrap(), val, "{t}");
                    assert_eq!(serde_json::to_string(&val.value()).unwrap(), w, "{t}");
                    assert_eq!(serde_json::from_str::<Val>(&w).unwrap(), val, "{t}");
                    assert_eq!(val.as_str().as_deref(), v.as_str(), "{t}");
                }
                Err(_) => bad += 1,
            }
        }
        // (Both kinds were made, enough of each to mean something.)
        assert!(good > 50_000 && bad > 10_000, "{good} read, {bad} refused");
    }

    #[test]
    fn a_string_is_read_as_its_text() {
        assert_eq!(Val::from(json!("a/b c")).as_str().as_deref(), Some("a/b c"));
        assert_eq!(Val::from(json!("é\"\\")).as_str().as_deref(), Some("é\"\\"));
        assert_eq!(Val::from(json!(1)).as_str(), None);
        assert_eq!(Val::from(json!({"a": "b"})).as_str(), None);
    }

    /// Written as it's held: the same bytes as its `Value`, inside a map or a list.
    #[test]
    fn a_value_is_written_as_its_json() {
        let v = json!({"f": {"file": "a.1.pack", "fmt": 1, "size": 2}, "u": true});
        let m: std::collections::BTreeMap<&str, Val> =
            v.as_object().unwrap().iter().map(|(k, v)| (k.as_str(), Val::from(v))).collect();
        assert_eq!(serde_json::to_string(&m).unwrap(), serde_json::to_string(&v).unwrap());
    }
}
