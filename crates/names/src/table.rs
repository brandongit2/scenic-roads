//! One translation file, compiled: every line's name, kinds, languages, main and sub packed into a
//! single byte arena, found by name through an open-addressing index. A name's lines are chained,
//! the latest first, so a later line wins for the same name, kind and language.
//!
//! A record is `varint(len n) n · u8 kinds · u8 nlangs · nlangs × [u8; 4] · varint(0 or len main
//! plus 1) [main] · varint(0 or len sub plus 1) [sub] · u32 previous (0 or offset plus 1)`: a main
//! of 0 is the name itself (nearly every line), a sub of 0 is none.

use crate::display::Kind;
use crate::spoken::Lang;
use anyhow::{bail, Result};
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::io::BufRead;

/// The highest share of index slots in use.
const MAX_LOAD: f64 = 0.8;

/// A name's hash: word-at-a-time multiply-rotate with a final avalanche, the same in every run
/// (so it can also hash versions).
pub(crate) fn hash(bytes: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut h = (bytes.len() as u64).wrapping_mul(K);
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let mut w = [0u8; 8];
        w.copy_from_slice(c);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(K);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut w = [0u8; 8];
        w[..rest.len()].copy_from_slice(rest);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(K);
    }
    // MurmurHash3's finaliser: every input bit reaches the low bits (the slot) and the top (the tag).
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// A slot's control byte: 0 when empty, else the hash's top 7 bits with the high bit set.
fn tag(h: u64) -> u8 {
    0x80 | (h >> 57) as u8
}

fn put_varint(out: &mut Vec<u8>, mut v: usize) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(buf: &[u8], pos: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7f) as usize) << shift;
        if b < 0x80 {
            return Some(v);
        }
    }
    None
}

/// The bit of a kind in a line's kinds.
pub(crate) fn kind_bit(k: Kind) -> u8 {
    match k {
        Kind::Road => 1,
        Kind::Settlement => 2,
        Kind::Other => 4,
    }
}

/// A compiled translation file.
#[derive(Debug, Default)]
pub(crate) struct Table {
    arena: Vec<u8>,
    ctrl: Vec<u8>,
    /// Per slot, the name's latest record.
    offs: Vec<u32>,
    /// Distinct names.
    names: usize,
    /// Lines kept.
    lines: usize,
    /// The languages its lines hold for.
    langs: BTreeSet<Lang>,
}

/// A line as stored: `main` is `None` when it is the name itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rec<'a> {
    pub main: Option<&'a str>,
    pub sub: Option<&'a str>,
}

/// A record's parts.
struct Raw<'a> {
    kinds: u8,
    langs: &'a [u8],
    rec: Rec<'a>,
    prev: Option<usize>,
}

impl Table {
    /// The latest line for `name` (whose [`hash`] is `h`) that holds for kind `k` and language
    /// `lang`.
    pub fn get(&self, h: u64, name: &[u8], k: Kind, lang: Lang) -> Option<Rec<'_>> {
        let mut off = self.find(h, name)?;
        let bit = kind_bit(k);
        let want = lang_bytes(lang);
        loop {
            let r = self.raw_at(off)?;
            if r.kinds & bit != 0 && r.langs.chunks_exact(4).any(|l| l == want) {
                return Some(r.rec);
            }
            off = r.prev?;
        }
    }

    /// The name's latest record's offset.
    fn find(&self, h: u64, name: &[u8]) -> Option<usize> {
        if self.ctrl.is_empty() {
            return None;
        }
        let mask = self.ctrl.len() - 1;
        let t = tag(h);
        let mut i = h as usize & mask;
        loop {
            match self.ctrl[i] {
                0 => return None,
                c if c == t => {
                    let off = self.offs[i] as usize;
                    if self.name_at(off) == Some(name) {
                        return Some(off);
                    }
                }
                _ => {}
            }
            i = (i + 1) & mask;
        }
    }

    /// Distinct names.
    pub fn len(&self) -> usize {
        self.names
    }

    /// Lines kept.
    pub fn lines(&self) -> usize {
        self.lines
    }

    pub fn langs(&self) -> &BTreeSet<Lang> {
        &self.langs
    }

    /// Heap bytes held.
    pub fn heap_bytes(&self) -> usize {
        self.arena.capacity() + self.ctrl.capacity() + self.offs.capacity() * 4
    }

    fn name_at(&self, off: usize) -> Option<&[u8]> {
        let mut p = off;
        let n = get_varint(&self.arena, &mut p)?;
        self.arena.get(p..p + n)
    }

    fn raw_at(&self, off: usize) -> Option<Raw<'_>> {
        let a = &self.arena;
        let mut p = off;
        let n = get_varint(a, &mut p)?;
        p += n;
        let kinds = *a.get(p)?;
        let nl = *a.get(p + 1)? as usize;
        p += 2;
        let langs = a.get(p..p + nl * 4)?;
        p += nl * 4;
        let part = |p: &mut usize| -> Option<Option<&str>> {
            let m = get_varint(a, p)?;
            if m == 0 {
                return Some(None);
            }
            let s = a.get(*p..*p + m - 1)?;
            *p += m - 1;
            std::str::from_utf8(s).ok().map(Some)
        };
        let main = part(&mut p)?;
        let sub = part(&mut p)?;
        let prev = u32::from_le_bytes(a.get(p..p + 4)?.try_into().ok()?);
        Some(Raw { kinds, langs, rec: Rec { main, sub }, prev: prev.checked_sub(1).map(|x| x as usize) })
    }

    fn push(&mut self, e: &Entry) -> Result<()> {
        let Ok(off) = u32::try_from(self.arena.len()) else {
            bail!("over 4 GB of names in one file");
        };
        if (self.names + 1) as f64 > self.ctrl.len() as f64 * MAX_LOAD {
            self.grow();
        }
        let h = hash(e.n.as_bytes());
        let mask = self.ctrl.len() - 1;
        let tg = tag(h);
        let mut i = h as usize & mask;
        let prev = loop {
            let c = self.ctrl[i];
            if c == 0 {
                break None;
            }
            if c == tg && self.name_at(self.offs[i] as usize) == Some(e.n.as_bytes()) {
                break Some(self.offs[i]);
            }
            i = (i + 1) & mask;
        };
        let a = &mut self.arena;
        put_varint(a, e.n.len());
        a.extend_from_slice(e.n.as_bytes());
        a.push(e.kinds);
        a.push(e.langs.len() as u8);
        for l in &e.langs {
            a.extend_from_slice(&lang_bytes(*l));
        }
        if e.main == e.n {
            put_varint(a, 0);
        } else {
            put_varint(a, e.main.len() + 1);
            a.extend_from_slice(e.main.as_bytes());
        }
        match &e.sub {
            None => put_varint(a, 0),
            Some(s) => {
                put_varint(a, s.len() + 1);
                a.extend_from_slice(s.as_bytes());
            }
        }
        a.extend_from_slice(&prev.map_or(0, |p| p + 1).to_le_bytes());
        if prev.is_none() {
            self.ctrl[i] = tg;
            self.names += 1;
        }
        self.offs[i] = off;
        self.lines += 1;
        self.langs.extend(e.langs.iter().copied());
        Ok(())
    }

    fn grow(&mut self) {
        let slots = (self.ctrl.len() * 2).max(1024);
        let (old_ctrl, old_offs) = (std::mem::replace(&mut self.ctrl, vec![0; slots]), std::mem::replace(&mut self.offs, vec![0; slots]));
        let mask = slots - 1;
        for (c, off) in old_ctrl.into_iter().zip(old_offs) {
            if c == 0 {
                continue;
            }
            let h = self.name_at(off as usize).map_or(0, hash);
            let mut i = h as usize & mask;
            while self.ctrl[i] != 0 {
                i = (i + 1) & mask;
            }
            self.ctrl[i] = c;
            self.offs[i] = off;
        }
    }

    /// Compiles a file's lines (see [`parse_line`]); later lines win for the same name, kind and
    /// language, and lines not translated yet are left out (counted).
    pub fn read(r: &mut impl BufRead) -> Result<(Table, Report)> {
        let mut t = Table::default();
        let mut report = Report::default();
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = r.read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            report.bytes += n as u64;
            report.lines += 1;
            let complete = line.last() == Some(&b'\n');
            let mut text = line.as_slice();
            if report.lines == 1 {
                text = text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(text);
            }
            if text.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match parse_line(text) {
                Parsed::Entry(entry) => t.push(&entry)?,
                Parsed::NotYet => report.ignored += 1,
                Parsed::Old if complete => report.old += 1,
                Parsed::Bad if complete => {
                    report.malformed += 1;
                    report.first_malformed.get_or_insert(report.lines);
                }
                // Still being written, or cut short: not a line yet.
                Parsed::Bad | Parsed::Old => report.unfinished = true,
            }
        }
        t.arena.shrink_to_fit();
        Ok((t, report))
    }
}

fn lang_bytes(l: Lang) -> [u8; 4] {
    let mut b = [0u8; 4];
    b[..l.as_str().len()].copy_from_slice(l.as_str().as_bytes());
    b
}

/// What reading a file found besides its lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub bytes: u64,
    pub lines: u64,
    /// Lines not translated yet, left out.
    pub ignored: u64,
    /// Lines of the area tables' format (no kind or no languages), left out.
    pub old: u64,
    pub malformed: u64,
    pub first_malformed: Option<u64>,
    pub unfinished: bool,
}

/// A translation line, interpreted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry<'a> {
    pub n: Cow<'a, str>,
    pub kinds: u8,
    pub langs: Vec<Lang>,
    pub main: Cow<'a, str>,
    pub sub: Option<Cow<'a, str>>,
}

/// What a line holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed<'a> {
    Entry(Entry<'a>),
    /// A well-formed line not translated yet (`"via": "todo"` or `"skipped"`): as if absent.
    NotYet,
    /// A line of the area tables (`{"n", "main", "sub"}` or `{"n", "en"}`, no kind or languages).
    Old,
    /// Not JSON, no name, or a kind or language that can't be read.
    Bad,
}

/// Reads one line: `{"n", "kind", "langs", "main", "sub"}`, `kind` one of road, settlement, other
/// or a list of them, `langs` a language or a list (`zh_Hant` is `zh`). A null or empty main is the
/// name, a null or empty sub none; an `en` stands for a missing `sub`. Other fields are ignored, but for `via`, which marks lines not
/// translated yet.
pub(crate) fn parse_line<'a>(text: &'a [u8]) -> Parsed<'a> {
    let Ok(line) = serde_json::from_slice::<Line<'a>>(text) else { return Parsed::Bad };
    let Some(n) = line.n.filter(|n| !n.is_empty()) else { return Parsed::Bad };
    let nonempty = |f: Field<'a>| -> Option<Cow<'a, str>> {
        match f {
            Field::Str(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    };
    if matches!(line.main, Field::Missing) && matches!(line.sub, Field::Missing) && matches!(line.en, Field::Missing) {
        return Parsed::Bad;
    }
    let (Some(kinds), Some(langs)) = (line.kinds, line.langs) else {
        return if line.kind_bad || line.langs_bad { Parsed::Bad } else { Parsed::Old };
    };
    let mut ls: Vec<Lang> = Vec::new();
    for l in &langs {
        match Lang::parse(l) {
            Some(l) if !ls.contains(&l) => ls.push(l),
            Some(_) => {}
            None => return Parsed::Bad,
        }
    }
    let mut bits = 0u8;
    for k in &kinds {
        bits |= match k.as_ref() {
            "road" => 1,
            "settlement" => 2,
            "other" => 4,
            _ => return Parsed::Bad,
        };
    }
    if bits == 0 || ls.is_empty() || ls.len() > 255 {
        return Parsed::Bad;
    }
    if line.not_yet {
        return Parsed::NotYet;
    }
    let main = nonempty(line.main).unwrap_or_else(|| n.clone());
    // The area tables' `en` on a line by language is its sub (when it has no sub of its own).
    let sub = if matches!(line.sub, Field::Missing) { nonempty(line.en) } else { nonempty(line.sub) };
    Parsed::Entry(Entry { n, kinds: bits, langs: ls, main, sub })
}

/// A field of a line: absent, null, or a string (any other type fails the line).
#[derive(Debug, Default)]
enum Field<'a> {
    #[default]
    Missing,
    Null,
    Str(Cow<'a, str>),
}

#[derive(Debug, Default)]
struct Line<'a> {
    n: Option<Cow<'a, str>>,
    main: Field<'a>,
    sub: Field<'a>,
    en: Field<'a>,
    kinds: Option<Vec<Cow<'a, str>>>,
    kind_bad: bool,
    langs: Option<Vec<Cow<'a, str>>>,
    langs_bad: bool,
    /// `via` is "todo" or "skipped".
    not_yet: bool,
}

/// A string or a list of strings (`kind`, `langs`); anything else is `None`.
struct OneOrMany<'a>(Option<Vec<Cow<'a, str>>>);

impl<'de> Deserialize<'de> for OneOrMany<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OneOrMany<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string or a list of strings")
            }
            fn visit_borrowed_str<E>(self, s: &'de str) -> Result<Self::Value, E> {
                Ok(OneOrMany(Some(vec![Cow::Borrowed(s)])))
            }
            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> {
                Ok(OneOrMany(Some(vec![Cow::Owned(s.to_owned())])))
            }
            fn visit_string<E>(self, s: String) -> Result<Self::Value, E> {
                Ok(OneOrMany(Some(vec![Cow::Owned(s)])))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                let mut ok = true;
                while let Some(v) = seq.next_element::<serde_json::Value>()? {
                    match v {
                        serde_json::Value::String(s) => out.push(Cow::Owned(s)),
                        _ => ok = false,
                    }
                }
                Ok(OneOrMany(ok.then_some(out)))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(OneOrMany(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(OneOrMany(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(OneOrMany(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(OneOrMany(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(OneOrMany(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(OneOrMany(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// The `via` field: whether it marks a line not translated yet. Any other value, of any type, is
/// no mark (like the other extra fields, it never fails a line).
struct Via(bool);

impl<'de> Deserialize<'de> for Via {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Via;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_str<E>(self, s: &str) -> Result<Via, E> {
                Ok(Via(matches!(s, "todo" | "skipped")))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Via, E> {
                Ok(Via(false))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Via, E> {
                Ok(Via(false))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Via, E> {
                Ok(Via(false))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Via, E> {
                Ok(Via(false))
            }
            fn visit_unit<E>(self) -> Result<Via, E> {
                Ok(Via(false))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Via, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Via(false))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Via, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Via(false))
            }
        }
        d.deserialize_any(V)
    }
}

/// A JSON string, borrowed from the line unless it holds escapes.
struct Str<'a>(Cow<'a, str>);

impl<'de> Deserialize<'de> for Str<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Str<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_borrowed_str<E>(self, s: &'de str) -> Result<Str<'de>, E> {
                Ok(Str(Cow::Borrowed(s)))
            }
            fn visit_str<E>(self, s: &str) -> Result<Str<'de>, E> {
                Ok(Str(Cow::Owned(s.to_owned())))
            }
            fn visit_string<E>(self, s: String) -> Result<Str<'de>, E> {
                Ok(Str(Cow::Owned(s)))
            }
        }
        d.deserialize_str(V)
    }
}

impl<'de> Deserialize<'de> for Line<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Line<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a translation line (a JSON object)")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Line<'de>, A::Error> {
                let mut line = Line::default();
                let field = |v: Option<Str<'de>>| v.map_or(Field::Null, |s| Field::Str(s.0));
                while let Some(key) = map.next_key::<Str<'de>>()? {
                    match &*key.0 {
                        "n" => line.n = map.next_value::<Option<Str<'de>>>()?.map(|s| s.0),
                        "main" => line.main = field(map.next_value()?),
                        "sub" => line.sub = field(map.next_value()?),
                        "en" => line.en = field(map.next_value()?),
                        "kind" => {
                            line.kinds = map.next_value::<OneOrMany<'de>>()?.0;
                            line.kind_bad = line.kinds.is_none();
                        }
                        "langs" => {
                            line.langs = map.next_value::<OneOrMany<'de>>()?.0;
                            line.langs_bad = line.langs.is_none();
                        }
                        "via" => line.not_yet = map.next_value::<Via>()?.0,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(line)
            }
        }
        d.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Lang {
        Lang::parse(s).unwrap()
    }

    type Owned = (String, u8, Vec<String>, String, Option<String>);

    fn entry(text: &str) -> Option<Owned> {
        match parse_line(text.as_bytes()) {
            Parsed::Entry(e) => Some((e.n.into_owned(), e.kinds, e.langs.iter().map(|x| x.as_str().to_owned()).collect(), e.main.into_owned(), e.sub.map(Cow::into_owned))),
            _ => None,
        }
    }

    fn owned(n: &str, kinds: u8, langs: &[&str], main: &str, sub: Option<&str>) -> Option<Owned> {
        Some((n.to_owned(), kinds, langs.iter().map(|x| x.to_string()).collect(), main.to_owned(), sub.map(str::to_owned)))
    }

    #[test]
    fn lines() {
        assert_eq!(
            entry(r#"{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake", "via": "agent:haiku"}"#),
            owned("Lac Bleu", 4, &["fr"], "Lac Bleu", Some("Blue Lake"))
        );
        assert_eq!(
            entry(r#"{"n": "Château", "kind": ["settlement", "other"], "langs": "fr", "main": "Castle", "sub": null}"#),
            owned("Château", 6, &["fr"], "Castle", None)
        );
        assert_eq!(entry(r#"{"n": "中山", "kind": "road", "langs": ["zh_Hant", "zh", "nan"], "sub": "Zhongshan"}"#), owned("中山", 1, &["zh", "nan"], "中山", Some("Zhongshan")));
        // A missing or empty main is the name; an empty sub is none; escapes are read.
        assert_eq!(entry(r#"{"n": "A", "kind": "other", "langs": "fr", "main": "", "sub": ""}"#), owned("A", 4, &["fr"], "A", None));
        assert_eq!(entry(r#"{"n": "L’Anse", "kind": "other", "langs": "fr", "main": null, "sub": "Cove \"x\""}"#), owned("L’Anse", 4, &["fr"], "L’Anse", Some("Cove \"x\"")));
        // Extra fields of any type, in any order.
        assert_eq!(entry(r#"{"x": [1, {"y": null}], "sub": "S", "langs": ["fr"], "n": "N", "z": 3.5, "kind": "road"}"#), owned("N", 1, &["fr"], "N", Some("S")));
        // The old `en` is the sub, unless there's a sub.
        assert_eq!(entry(r#"{"n": "Lac", "kind": "other", "langs": "fr", "en": "Lake"}"#), owned("Lac", 4, &["fr"], "Lac", Some("Lake")));
        assert_eq!(entry(r#"{"n": "Lac", "kind": "other", "langs": "fr", "en": "Lake", "sub": null}"#), owned("Lac", 4, &["fr"], "Lac", None));
        // The area tables' lines: told apart, left out.
        assert_eq!(parse_line(br#"{"n": "Lac Blanc", "en": "White Lake"}"#), Parsed::Old);
        assert_eq!(parse_line(br#"{"n": "A", "main": "A", "sub": "B", "via": "native"}"#), Parsed::Old);
        assert_eq!(parse_line(br#"{"n": "A", "kind": "other", "main": "A", "sub": "B"}"#), Parsed::Old);
        for bad in [
            "",
            "{",
            "[1, 2]",
            r#"{"main": "A", "sub": "B", "kind": "other", "langs": "fr"}"#,
            r#"{"n": "", "main": "A", "kind": "other", "langs": "fr"}"#,
            r#"{"n": "A", "kind": "other", "langs": "fr"}"#,
            r#"{"n": "A", "sub": 5, "kind": "other", "langs": "fr"}"#,
            r#"{"n": "A", "sub": "a", "kind": "village", "langs": "fr"}"#,
            r#"{"n": "A", "sub": "a", "kind": [], "langs": "fr"}"#,
            r#"{"n": "A", "sub": "a", "kind": "other", "langs": []}"#,
            r#"{"n": "A", "sub": "a", "kind": "other", "langs": ["français"]}"#,
            r#"{"n": "A", "sub": "a", "kind": "other", "langs": 5}"#,
            r#"{"n": "A", "sub": "a", "kind": 5, "langs": "fr"}"#,
            r#"{"n": "A", "sub": "a", "kind": "other", "langs": ["fr", 5]}"#,
        ] {
            assert_eq!(parse_line(bad.as_bytes()), Parsed::Bad, "{bad}");
        }
        assert_eq!(parse_line(br#"{"n": "A", "kind": "other", "langs": "fr", "sub": null, "via": "todo"}"#), Parsed::NotYet);
        assert_eq!(parse_line(br#"{"n": "A", "kind": "other", "langs": "fr", "sub": null, "via": "skipped"}"#), Parsed::NotYet);
        assert!(entry(r#"{"n": "A", "kind": "other", "langs": "fr", "sub": "a", "via": "osm"}"#).is_some());
        for odd in [r#"5"#, r#"null"#, r#"true"#, r#"["todo"]"#, r#"{"x": "todo"}"#] {
            let text = format!("{{\"n\": \"A\", \"kind\": \"other\", \"langs\": \"fr\", \"sub\": \"a\", \"via\": {odd}}}");
            assert!(entry(&text).is_some(), "{text}");
        }
    }

    #[test]
    fn table() {
        let text = "\u{feff}{\"n\": \"松島\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"Matsushima\"}\n\
            not json\n\
            {\"n\": \"Église\", \"kind\": \"other\", \"langs\": \"fr\", \"sub\": null, \"via\": \"todo\"}\n\
            \n\
            {\"n\": \"Église\", \"kind\": [\"settlement\", \"other\"], \"langs\": [\"fr\", \"br\"], \"main\": \"Church\", \"sub\": null}\r\n\
            {\"n\": \"Église\", \"kind\": \"settlement\", \"langs\": \"fr\", \"main\": \"Église\", \"sub\": null}\n\
            {\"n\": \"Lac\", \"en\": \"Lake\"}\n\
            {\"n\": \"Unfinished\", \"kind\": \"other\", \"langs\": \"fr\", \"main\":";
        let (t, report) = Table::read(&mut text.as_bytes()).expect("read");
        assert_eq!((t.len(), t.lines()), (2, 3));
        assert_eq!((report.ignored, report.old, report.malformed, report.first_malformed), (1, 1, 1, Some(2)));
        assert!(report.unfinished);
        assert_eq!(report.bytes, text.len() as u64);
        assert_eq!(t.langs().iter().map(Lang::as_str).collect::<Vec<_>>(), ["br", "fr", "ja"]);
        let get = |n: &str, k: Kind, lang: &str| t.get(hash(n.as_bytes()), n.as_bytes(), k, l(lang));
        assert_eq!(get("松島", Kind::Other, "ja"), Some(Rec { main: None, sub: Some("Matsushima") }));
        assert_eq!(get("松島", Kind::Other, "zh"), None);
        assert_eq!(get("松島", Kind::Road, "ja"), None);
        // The later line wins for its kind and language only.
        assert_eq!(get("Église", Kind::Settlement, "fr"), Some(Rec { main: None, sub: None }));
        assert_eq!(get("Église", Kind::Settlement, "br"), Some(Rec { main: Some("Church"), sub: None }));
        assert_eq!(get("Église", Kind::Other, "fr"), Some(Rec { main: Some("Church"), sub: None }));
        assert_eq!(get("Lac", Kind::Other, "fr"), None);
        assert_eq!(get("Unfinished", Kind::Other, "fr"), None);
        let (t, _) = Table::read(&mut &b""[..]).expect("read");
        assert_eq!((t.len(), t.get(hash(b"A"), b"A", Kind::Other, l("fr"))), (0, None));
    }

    #[test]
    fn many_names_and_long_ones() {
        let long = "x".repeat(300);
        let mut text = String::new();
        for i in 0..20_000 {
            text.push_str(&format!("{{\"n\": \"name {i}\", \"kind\": \"road\", \"langs\": [\"es\", \"ca\"], \"main\": \"main {i}\", \"sub\": \"{long}{i}\"}}\n"));
        }
        let (t, _) = Table::read(&mut text.as_bytes()).expect("read");
        assert_eq!(t.len(), 20_000);
        for i in (0..20_000).step_by(7) {
            let n = format!("name {i}");
            let sub = format!("{long}{i}");
            let main = format!("main {i}");
            assert_eq!(t.get(hash(n.as_bytes()), n.as_bytes(), Kind::Road, l("ca")), Some(Rec { main: Some(&main), sub: Some(&sub) }));
        }
        assert_eq!(t.get(hash(b"name 20000"), b"name 20000", Kind::Road, l("ca")), None);
        assert!(t.len() as f64 <= t.ctrl.len() as f64 * MAX_LOAD);
    }

    #[test]
    fn hash_is_fixed() {
        // Versions (ETags) hash with it, so it must not change between builds or runs.
        assert_eq!(hash(b""), 0);
        assert_eq!(hash(b"jp/places-jp.jsonl"), hash(b"jp/places-jp.jsonl"));
        assert_ne!(hash(b"a"), hash(b"b"));
        assert_ne!(hash(b"abcdefgh"), hash(b"abcdefgh\0"));
    }
}
