//! The numbers the heritage layers' filters work on, stamped onto their features in the heritage
//! chain's stand-in root (`data/build`, after heritagedetails.py and areadetails.py; the details
//! stay on the server, these few fields are what MapLibre filters and the in-view counts need):
//!
//!   heritage.json   by (year built), dy (year designated), sl (Wikipedia/Wikimedia sitelinks),
//!                   wp (has a Wikipedia article), t (kind of designation: the register's `tier`,
//!                   else by level: `tier`)
//!   heritage-areas.json, special.json, indigenous.json   a (area km²)
//!
//! The stops & sights' values are the marks job's (`interest::poi_filter_props`). The files are
//! read and written as Python's `json.load`/`json.dumps(ensure_ascii=False, separators=(",",":"))`
//! do (the chain's scripts after it read them, and the outputs keep their bytes): keys in their
//! order, numbers as Python writes them (`Py`).

use crate::timings::{phase, Class};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// A JSON value as Python holds it: objects keep their keys' order, integers stay integers.
#[derive(Clone, Debug, PartialEq)]
pub enum Py {
    Null,
    Bool(bool),
    /// An integer, its digits (Python's are unbounded).
    Int(String),
    Float(f64),
    Str(String),
    List(Vec<Py>),
    Dict(Vec<(String, Py)>),
}

impl Py {
    fn get(&self, k: &str) -> Option<&Py> {
        match self {
            Py::Dict(d) => d.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }
    /// `d.get(k)`, None as Python's None.
    fn at(&self, k: &str) -> &Py {
        self.get(k).unwrap_or(&Py::Null)
    }
    /// `d[k] = v`: in place when there, else at the end.
    fn set(&mut self, k: &str, v: Py) {
        if let Py::Dict(d) = self {
            match d.iter_mut().find(|(key, _)| key == k) {
                Some(e) => e.1 = v,
                None => d.push((k.to_string(), v)),
            }
        }
    }
    fn pop(&mut self, k: &str) {
        if let Py::Dict(d) = self {
            d.retain(|(key, _)| key != k);
        }
    }
    fn truthy(&self) -> bool {
        match self {
            Py::Null => false,
            Py::Bool(b) => *b,
            Py::Int(s) => s.trim_start_matches('-').bytes().any(|c| c != b'0'),
            Py::Float(f) => *f != 0.0,
            Py::Str(s) => !s.is_empty(),
            Py::List(l) => !l.is_empty(),
            Py::Dict(d) => !d.is_empty(),
        }
    }
    /// Its number (True as 1), for Python's `==` between numbers.
    fn number(&self) -> Option<f64> {
        match self {
            Py::Bool(b) => Some(*b as u8 as f64),
            Py::Int(s) => s.parse().ok(),
            Py::Float(f) => Some(*f),
            _ => None,
        }
    }
    /// Python's `str(v)` (its `repr` within lists and dicts).
    fn py_str(&self) -> String {
        match self {
            Py::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }
    fn py_repr(&self) -> String {
        match self {
            Py::Null => "None".into(),
            Py::Bool(true) => "True".into(),
            Py::Bool(false) => "False".into(),
            Py::Int(s) => s.clone(),
            Py::Float(f) => float_repr(*f),
            Py::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
            Py::List(l) => format!("[{}]", l.iter().map(Py::py_repr).collect::<Vec<_>>().join(", ")),
            Py::Dict(d) => format!(
                "{{{}}}",
                d.iter()
                    .map(|(k, v)| format!("{}: {}", Py::Str(k.clone()).py_repr(), v.py_repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    /// A key for a dict lookup by value (`det.get(p.get("i"))`): equal numbers alike, as Python's.
    fn key(&self) -> String {
        match self.number() {
            Some(n) => format!("n{n:?}"),
            None => format!("v{}", dumps(self)),
        }
    }
}

/// Python's `repr(float)`: the shortest digits that read back, fixed between 1e-4 and 1e16.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    // "d.ddde<exp>", the shortest round-trip digits.
    let e = format!("{:e}", f);
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        format!("{sign}{s}")
    } else {
        let m = if digits.len() == 1 { digits } else { format!("{}.{}", &digits[..1], &digits[1..]) };
        format!("{sign}{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// `json.loads`.
pub fn loads(s: &str) -> Result<Py> {
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        bail!("extra data at byte {}", p.i);
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn lit(&mut self, w: &str) -> bool {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self) -> Result<Py> {
        let Some(&c) = self.b.get(self.i) else { bail!("unexpected end") };
        Ok(match c {
            b'{' => {
                self.i += 1;
                let mut d: Vec<(String, Py)> = Vec::new();
                self.ws();
                if self.lit("}") {
                    return Ok(Py::Dict(d));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        bail!("a key expected at byte {}", self.i);
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.lit(":") {
                        bail!("':' expected at byte {}", self.i);
                    }
                    self.ws();
                    let v = self.value()?;
                    // A repeated key: the last value, at the first's place (a dict's).
                    match d.iter_mut().find(|(key, _)| *key == k) {
                        Some(e) => e.1 = v,
                        None => d.push((k, v)),
                    }
                    self.ws();
                    if self.lit("}") {
                        return Ok(Py::Dict(d));
                    }
                    if !self.lit(",") {
                        bail!("',' expected at byte {}", self.i);
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut l = Vec::new();
                self.ws();
                if self.lit("]") {
                    return Ok(Py::List(l));
                }
                loop {
                    self.ws();
                    l.push(self.value()?);
                    self.ws();
                    if self.lit("]") {
                        return Ok(Py::List(l));
                    }
                    if !self.lit(",") {
                        bail!("',' expected at byte {}", self.i);
                    }
                }
            }
            b'"' => Py::Str(self.string()?),
            _ if self.lit("null") => Py::Null,
            _ if self.lit("true") => Py::Bool(true),
            _ if self.lit("false") => Py::Bool(false),
            _ if self.lit("NaN") => Py::Float(f64::NAN),
            _ if self.lit("Infinity") => Py::Float(f64::INFINITY),
            _ if self.lit("-Infinity") => Py::Float(f64::NEG_INFINITY),
            _ => self.number()?,
        })
    }
    fn number(&mut self) -> Result<Py> {
        let s = self.i;
        let digits = |p: &mut Self| {
            let a = p.i;
            while p.i < p.b.len() && p.b[p.i].is_ascii_digit() {
                p.i += 1;
            }
            p.i > a
        };
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        if self.b.get(self.i) == Some(&b'0') {
            self.i += 1;
        } else if !digits(self) {
            bail!("a value expected at byte {s}");
        }
        let mut float = false;
        if self.b.get(self.i) == Some(&b'.') && self.b.get(self.i + 1).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
            digits(self);
            float = true;
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) {
                float = true;
            } else {
                self.i = save;
            }
        }
        let t = std::str::from_utf8(&self.b[s..self.i])?;
        Ok(if float {
            Py::Float(t.parse()?)
        } else if t == "-0" {
            Py::Int("0".into())
        } else {
            Py::Int(t.into())
        })
    }
    fn hex4(&mut self) -> Result<u32> {
        let h = self.b.get(self.i..self.i + 4).context("a short \\u escape")?;
        self.i += 4;
        Ok(u32::from_str_radix(std::str::from_utf8(h)?, 16)?)
    }
    fn string(&mut self) -> Result<String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let s = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                if self.b[self.i] < 0x20 {
                    bail!("a control character in a string at byte {}", self.i);
                }
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.b[s..self.i])?);
            match self.b.get(self.i) {
                None => bail!("an unterminated string"),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => {
                    self.i += 1;
                    let c = *self.b.get(self.i).context("an unterminated string")?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut u = self.hex4()?;
                            if (0xd800..0xdc00).contains(&u) && self.b[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    u = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                                } else {
                                    self.i = save;
                                }
                            }
                            // (A lone surrogate, which Python keeps, a Rust string can't hold.)
                            out.push(char::from_u32(u).context("a lone surrogate in a string")?);
                        }
                        _ => bail!("a bad escape at byte {}", self.i - 1),
                    }
                }
            }
        }
    }
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`.
pub fn dumps(v: &Py) -> String {
    let mut s = String::new();
    dump(v, &mut s);
    s
}

fn dump(v: &Py, out: &mut String) {
    match v {
        Py::Null => out.push_str("null"),
        Py::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Py::Int(s) => out.push_str(s),
        Py::Float(f) => out.push_str(&float_repr(*f)),
        Py::Str(s) => dump_str(s, out),
        Py::List(l) => {
            out.push('[');
            for (k, x) in l.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                dump(x, out);
            }
            out.push(']');
        }
        Py::Dict(d) => {
            out.push('{');
            for (k, (key, x)) in d.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                dump_str(key, out);
                out.push(':');
                dump(x, out);
            }
            out.push('}');
        }
    }
}

fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The first run of 3–4 digits (a minus sign before it kept) in `str(v or "")`.
fn year(v: &Py) -> Option<Py> {
    let s = if v.truthy() { v.py_str() } else { String::new() };
    let b = s.as_bytes();
    for i in 0..b.len() {
        let j0 = if b[i] == b'-' { i + 1 } else { i };
        let n = b[j0.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n >= 3 {
            let t: i64 = s[i..j0 + n.min(4)].parse().ok()?;
            return Some(Py::Int(t.to_string()));
        }
    }
    None
}

/// A site's kind of designation for the Heritage sites toggles (web/src/basemap.ts
/// HERITAGE_TIERS): its register's (`t`, from the entry's `tier`: docs/inputs.md §6.5), else by
/// its level, World Heritage by its category.
fn tier(p: &Py) -> Py {
    if p.at("t").truthy() {
        return p.at("t").clone();
    }
    let lv = if p.at("level").truthy() { p.at("level").number() } else { Some(5.0) };
    let t = match lv {
        Some(l) if l == 1.0 => {
            if matches!(p.at("category"), Py::Str(c) if c == "Natural" || c == "Mixed") {
                "w.n"
            } else {
                "w.c"
            }
        }
        Some(l) if l == 2.0 => "n.top",
        Some(l) if l == 3.0 => "n.second",
        Some(l) if l == 4.0 => "p.des",
        _ => "m.des",
    };
    Py::Str(t.into())
}

/// A details file's records by their `i` (none when it's missing).
fn jsonl(b: &Path, name: &str) -> Result<HashMap<String, Py>> {
    let mut out = HashMap::new();
    let p = b.join(name);
    if p.exists() {
        for line in std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?.lines() {
            let r = loads(line).with_context(|| format!("a line of {name}"))?;
            out.insert(r.get("i").context("a record without `i`")?.key(), r);
        }
    }
    Ok(out)
}

fn read(b: &Path, name: &str) -> Result<Py> {
    let p = b.join(name);
    loads(&std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?)
        .with_context(|| format!("parse {name}"))
}

fn write(b: &Path, name: &str, fc: &Py) -> Result<()> {
    let tmp = b.join(format!("{name}.tmp"));
    std::fs::write(&tmp, dumps(fc))?;
    std::fs::rename(&tmp, b.join(name))?;
    Ok(())
}

fn features(fc: &mut Py) -> Result<&mut Vec<Py>> {
    match fc {
        Py::Dict(d) => match d.iter_mut().find(|(k, _)| k == "features") {
            Some((_, Py::List(l))) => Ok(l),
            _ => bail!("a FeatureCollection without features"),
        },
        _ => bail!("not a FeatureCollection"),
    }
}

fn props(f: &mut Py) -> Option<&mut Py> {
    match f {
        Py::Dict(d) => d.iter_mut().find(|(k, _)| k == "properties").map(|(_, v)| v),
        _ => None,
    }
}

/// The heritage sites' values (heritage.json, from details-heritage.jsonl).
fn heritage(b: &Path) -> Result<()> {
    let p = phase("the layers and details read", Class::Disk);
    let mut fc = read(b, "heritage.json")?;
    let det = jsonl(b, "details-heritage.jsonl")?;
    drop(p);
    let ph = phase("the filter values set", Class::Compute);
    let empty = Py::Dict(vec![]);
    let mut n = 0;
    for f in features(&mut fc)? {
        let p = props(f).context("a feature without properties")?;
        let d = det.get(&p.at("i").key()).unwrap_or(&empty);
        for key in ["by", "dy", "sl", "wp"] {
            p.pop(key);
        }
        if let Some(by) = year(d.at("inception")) {
            p.set("by", by);
        }
        if let Some(dy) = year(p.at("date")) {
            p.set("dy", dy);
        }
        if d.at("sl").truthy() {
            p.set("sl", d.at("sl").clone());
        }
        if d.at("wiki").truthy() {
            p.set("wp", Py::Int("1".into()));
        }
        let t = tier(p);
        p.set("t", t);
        n += (p.get("by").is_some() || p.get("wp").is_some()) as usize;
    }
    drop(ph);
    let _p = phase("the layers written", Class::Disk);
    write(b, "heritage.json", &fc)?;
    eprintln!("heritage.json: {n} sites with a build year or article");
    Ok(())
}

/// The areas' sizes (`a`, km²) from their details.
fn areas(b: &Path) -> Result<()> {
    for (name, layer) in
        [("heritage-areas.json", "harea"), ("special.json", "special"), ("indigenous.json", "indigenous")]
    {
        let p = phase("the layers and details read", Class::Disk);
        let mut fc = read(b, name)?;
        let det = jsonl(b, &format!("details-{layer}.jsonl"))?;
        drop(p);
        let p = phase("the filter values set", Class::Compute);
        for f in features(&mut fc)? {
            let Some(p) = props(f) else { bail!("a feature of {name} without properties") };
            let a = det.get(&p.at("i").key()).map(|r| r.at("area_km2").clone()).unwrap_or(Py::Null);
            if a.truthy() {
                p.set("a", a);
            }
        }
        drop(p);
        let _p = phase("the layers written", Class::Disk);
        write(b, name, &fc)?;
    }
    eprintln!("areas: a (km²) on heritage districts, biospheres & co., Indigenous lands");
    Ok(())
}

/// Stamps the filters' values onto the heritage layers in `b` (the stand-in root's data/build).
pub fn run(b: &Path) -> Result<()> {
    heritage(b)?;
    areas(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_as_python() {
        for (f, s) in [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (12.34, "12.34"),
            (-0.0, "-0.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345.6, "123456789012345.6"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-2.5e-7, "-2.5e-07"),
            (1e100, "1e+100"),
        ] {
            assert_eq!(float_repr(f), s);
        }
    }

    #[test]
    fn round_trips_as_python() {
        let src = "{ \"b\": 1, \"a\": [1.50, -0, 2E3, \"\\u00e9\\n\\ud83d\\ude00/\\u0001\"], \"b\": null }";
        assert_eq!(dumps(&loads(src).unwrap()), "{\"b\":null,\"a\":[1.5,0,2000.0,\"é\\n😀/\\u0001\"]}");
    }

    #[test]
    fn heritage_values() {
        let p = loads(r#"{"i":3,"level":1,"category":"Mixed","date":"designated 1985-05-01"}"#).unwrap();
        assert_eq!(tier(&p), Py::Str("w.n".into()));
        assert_eq!(year(p.at("date")), Some(Py::Int("1985".into())));
        assert_eq!(year(&Py::Float(-123.5)), Some(Py::Int("-123".into())));
        assert_eq!(year(&Py::Int("12".into())), None);
        assert_eq!(tier(&loads(r#"{"level":0}"#).unwrap()), Py::Str("m.des".into()));
        assert_eq!(tier(&loads(r#"{"t":"x","level":2}"#).unwrap()), Py::Str("x".into()));
    }
}
