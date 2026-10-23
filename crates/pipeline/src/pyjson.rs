//! JSON as Python's `json` module reads and writes it, for files whose bytes must stay what the
//! Python steps made (the heritage chain's: crate::filterprops, crate::heritage_layers): objects keep
//! their keys' order (a repeated key keeps its first place and its last value), integers stay
//! integers of any size (their digits) and floats floats (`1` and `1.0`; NaN and Infinity read),
//! surrogate pairs join, and `dumps` writes as `json.dumps(v, ensure_ascii=False,
//! separators=(",", ":"))` does: floats by `repr` (`1e-05`, `5.0`, `-0.0`), strings with only `"`,
//! `\` and control characters escaped.

use anyhow::{bail, Context, Result};

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
    /// `d[k]`, to change it.
    pub fn get_mut(&mut self, k: &str) -> Option<&mut Py> {
        match self {
            Py::Dict(d) => d.iter_mut().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }
    /// A string's text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Py::Str(s) => Some(s),
            _ => None,
        }
    }
    /// A list's items.
    pub fn list(&self) -> Option<&Vec<Py>> {
        match self {
            Py::List(l) => Some(l),
            _ => None,
        }
    }
    /// A dict's entries, to change them.
    pub fn dict_mut(&mut self) -> Option<&mut Vec<(String, Py)>> {
        match self {
            Py::Dict(d) => Some(d),
            _ => None,
        }
    }
    pub fn get(&self, k: &str) -> Option<&Py> {
        match self {
            Py::Dict(d) => d.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }
    /// `d.get(k)`, None as Python's None.
    pub fn at(&self, k: &str) -> &Py {
        self.get(k).unwrap_or(&Py::Null)
    }
    /// `d[k] = v`: in place when there, else at the end.
    pub fn set(&mut self, k: &str, v: Py) {
        if let Py::Dict(d) = self {
            match d.iter_mut().find(|(key, _)| key == k) {
                Some(e) => e.1 = v,
                None => d.push((k.to_string(), v)),
            }
        }
    }
    pub fn pop(&mut self, k: &str) {
        if let Py::Dict(d) = self {
            d.retain(|(key, _)| key != k);
        }
    }
    pub fn truthy(&self) -> bool {
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
    pub fn number(&self) -> Option<f64> {
        match self {
            Py::Bool(b) => Some(*b as u8 as f64),
            Py::Int(s) => s.parse().ok(),
            Py::Float(f) => Some(*f),
            _ => None,
        }
    }
    /// Python's `str(v)` (its `repr` within lists and dicts).
    pub fn py_str(&self) -> String {
        match self {
            Py::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }
    pub fn py_repr(&self) -> String {
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
    pub fn key(&self) -> String {
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
    // "d.ddde<exp>", the shortest round-trip digits; of the strings that short, the one nearest the
    // exact value, a tie to even (Python's dtoa: `{:e}` takes a tie up, 3.2678298950195313 for
    // 3.26782989501953125), which the exact formatting at that many digits gives.
    let shortest = format!("{:e}", f);
    let n = shortest.split_once('e').unwrap().0.chars().filter(|c| c.is_ascii_digit()).count();
    let e = format!("{:.*e}", n - 1, f);
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
    pub fn number(&mut self) -> Result<Py> {
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
    dump(v, &mut s, (",", ":"));
    s
}

/// `json.dumps(v, ensure_ascii=False)`: Python's default separators, ", " and ": ".
pub fn dumps_spaced(v: &Py) -> String {
    let mut s = String::new();
    dump(v, &mut s, (", ", ": "));
    s
}

/// `json.dumps(v, ensure_ascii=False, indent=n)`: each item on its own line, `n` spaces a level.
pub fn dumps_indent(v: &Py, n: usize) -> String {
    let mut s = String::new();
    dump_indent(v, &mut s, n, 0);
    s
}

fn dump_indent(v: &Py, out: &mut String, n: usize, level: usize) {
    let pad = |out: &mut String, l: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', n * l));
    };
    match v {
        Py::List(l) if !l.is_empty() => {
            out.push('[');
            for (k, x) in l.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                pad(out, level + 1);
                dump_indent(x, out, n, level + 1);
            }
            pad(out, level);
            out.push(']');
        }
        Py::Dict(d) if !d.is_empty() => {
            out.push('{');
            for (k, (key, x)) in d.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                pad(out, level + 1);
                dump_str(key, out);
                out.push_str(": ");
                dump_indent(x, out, n, level + 1);
            }
            pad(out, level);
            out.push('}');
        }
        other => dump(other, out, (",", ":")),
    }
}

/// A serde_json value as Python's `json` would have read its text (its objects' keys in the order
/// serde_json keeps them).
pub fn from_serde(v: &serde_json::Value) -> Py {
    use serde_json::Value as V;
    match v {
        V::Null => Py::Null,
        V::Bool(b) => Py::Bool(*b),
        V::Number(x) if x.is_f64() => Py::Float(x.as_f64().unwrap_or(f64::NAN)),
        V::Number(x) => Py::Int(x.to_string()),
        V::String(s) => Py::Str(s.clone()),
        V::Array(a) => Py::List(a.iter().map(from_serde).collect()),
        V::Object(o) => Py::Dict(o.iter().map(|(k, x)| (k.clone(), from_serde(x))).collect()),
    }
}

fn dump(v: &Py, out: &mut String, sep: (&str, &str)) {
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
                    out.push_str(sep.0);
                }
                dump(x, out, sep);
            }
            out.push(']');
        }
        Py::Dict(d) => {
            out.push('{');
            for (k, (key, x)) in d.iter().enumerate() {
                if k > 0 {
                    out.push_str(sep.0);
                }
                dump_str(key, out);
                out.push_str(sep.1);
                dump(x, out, sep);
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
            (3.26782989501953125, "3.2678298950195312"),
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
    fn spaced_as_python() {
        let v = loads(r#"{"i":3,"a":[1,2.5,{"x":null}],"s":"é"}"#).unwrap();
        assert_eq!(dumps_spaced(&v), r#"{"i": 3, "a": [1, 2.5, {"x": null}], "s": "é"}"#);
        assert_eq!(dumps_spaced(&loads("[]").unwrap()), "[]");
    }

    #[test]
    fn indented_as_python() {
        // json.dumps(v, ensure_ascii=False, indent=1)
        let v = loads(r#"[{"a":1,"b":[],"c":{},"d":[1.5,"é"]},null]"#).unwrap();
        assert_eq!(
            dumps_indent(&v, 1),
            "[\n {\n  \"a\": 1,\n  \"b\": [],\n  \"c\": {},\n  \"d\": [\n   1.5,\n   \"é\"\n  ]\n },\n null\n]"
        );
    }
}
