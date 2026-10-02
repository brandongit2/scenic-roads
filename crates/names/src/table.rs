//! One translation file, compiled: every line's name, main and sub packed into a single byte arena,
//! found through an open-addressing index. About 1.5× the bytes of the strings themselves (95 MB
//! for the 3.3 M lines of 2026-10, whose names, mains and subs come to 64 MB); a hash map of boxed
//! strings would take about 300 MB.
//!
//! A record is `varint(len n) n · varint(0 | len main + 1) [main] · varint(0 | len sub + 1) [sub]`:
//! a main of 0 is the name itself (nearly every line), a sub of 0 is none.

use anyhow::{bail, Result};
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::borrow::Cow;
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

/// A compiled translation file.
#[derive(Debug)]
pub(crate) struct Table {
    arena: Box<[u8]>,
    ctrl: Box<[u8]>,
    offs: Box<[u32]>,
    len: usize,
    /// Lines left out as not translated yet (`via` "todo" or "skipped").
    ignored: usize,
}

/// A line as stored: `main` is `None` when it is the name itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rec<'a> {
    pub main: Option<&'a str>,
    pub sub: Option<&'a str>,
}

impl Table {
    /// The line for `name`, whose [`hash`] is `h`.
    pub fn get(&self, h: u64, name: &[u8]) -> Option<Rec<'_>> {
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
                        return self.rec_at(off);
                    }
                }
                _ => {}
            }
            i = (i + 1) & mask;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Lines left out as not translated yet.
    pub fn ignored(&self) -> usize {
        self.ignored
    }

    /// Heap bytes held.
    pub fn heap_bytes(&self) -> usize {
        self.arena.len() + self.ctrl.len() + self.offs.len() * 4
    }

    fn name_at(&self, off: usize) -> Option<&[u8]> {
        let mut p = off;
        let n = get_varint(&self.arena, &mut p)?;
        self.arena.get(p..p + n)
    }

    fn rec_at(&self, off: usize) -> Option<Rec<'_>> {
        let a = &self.arena;
        let mut p = off;
        let n = get_varint(a, &mut p)?;
        p += n;
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
        Some(Rec { main, sub })
    }

    /// Compiles a file's lines (see [`parse_line`]); later lines win for the same name, and lines
    /// not translated yet are left out (counted).
    pub fn read(r: &mut impl BufRead) -> Result<(Table, Report)> {
        let mut b = Builder::default();
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
                Parsed::Entry(entry) => b.push(&entry.n, &entry.main, entry.sub.as_deref())?,
                Parsed::NotYet => report.ignored += 1,
                Parsed::Bad if complete => {
                    report.malformed += 1;
                    report.first_malformed.get_or_insert(report.lines);
                }
                // Still being written, or cut short: not a line yet.
                Parsed::Bad => report.unfinished = true,
            }
        }
        let mut table = b.finish();
        table.ignored = report.ignored as usize;
        Ok((table, report))
    }
}

/// What reading a file found besides its entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub bytes: u64,
    pub lines: u64,
    /// Lines not translated yet, left out.
    pub ignored: u64,
    pub malformed: u64,
    pub first_malformed: Option<u64>,
    pub unfinished: bool,
}

#[derive(Default)]
struct Builder {
    arena: Vec<u8>,
    items: Vec<(u64, u32)>,
}

impl Builder {
    fn push(&mut self, n: &str, main: &str, sub: Option<&str>) -> Result<()> {
        let Ok(off) = u32::try_from(self.arena.len()) else {
            bail!("over 4 GB of names in one file");
        };
        let a = &mut self.arena;
        put_varint(a, n.len());
        a.extend_from_slice(n.as_bytes());
        if main == n {
            put_varint(a, 0);
        } else {
            put_varint(a, main.len() + 1);
            a.extend_from_slice(main.as_bytes());
        }
        match sub {
            None => put_varint(a, 0),
            Some(s) => {
                put_varint(a, s.len() + 1);
                a.extend_from_slice(s.as_bytes());
            }
        }
        self.items.push((hash(n.as_bytes()), off));
        Ok(())
    }

    fn finish(self) -> Table {
        let Builder { arena, items } = self;
        let slots = if items.is_empty() {
            0
        } else {
            ((items.len() as f64 / MAX_LOAD).ceil() as usize + 1).next_power_of_two()
        };
        let mut t = Table { arena: Box::default(), ctrl: vec![0u8; slots].into(), offs: vec![0u32; slots].into(), len: 0, ignored: 0 };
        let mask = slots.wrapping_sub(1);
        for (h, off) in items {
            let tg = tag(h);
            let mut i = h as usize & mask;
            loop {
                let c = t.ctrl[i];
                if c == 0 {
                    t.ctrl[i] = tg;
                    t.offs[i] = off;
                    t.len += 1;
                    break;
                }
                // The same name again: the later line wins.
                if c == tg && name_in(&arena, t.offs[i]) == name_in(&arena, off) {
                    t.offs[i] = off;
                    break;
                }
                i = (i + 1) & mask;
            }
        }
        t.arena = arena.into_boxed_slice();
        t
    }
}

fn name_in(arena: &[u8], off: u32) -> Option<&[u8]> {
    let mut p = off as usize;
    let n = get_varint(arena, &mut p)?;
    arena.get(p..p + n)
}

/// A translation line, interpreted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry<'a> {
    pub n: Cow<'a, str>,
    pub main: Cow<'a, str>,
    pub sub: Option<Cow<'a, str>>,
}

/// What a line holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed<'a> {
    Entry(Entry<'a>),
    /// A well-formed line not translated yet (`"via": "todo"` or `"skipped"`): as if absent, so it
    /// doesn't hide the thing's own English.
    NotYet,
    /// Not JSON, no name, or neither format.
    Bad,
}

/// Reads one line: the display format `{"n", "main", "sub"}` (a null or empty main is the name),
/// else the older `{"n", "en"}` (main is the name, sub the English; null means checked, nothing to
/// add). Other fields are ignored, but for `via`, which marks lines not translated yet.
pub(crate) fn parse_line<'a>(text: &'a [u8]) -> Parsed<'a> {
    let Ok(line) = serde_json::from_slice::<Line<'a>>(text) else { return Parsed::Bad };
    let Some(n) = line.n.filter(|n| !n.is_empty()) else { return Parsed::Bad };
    let nonempty = |f: Field<'a>| -> Option<Cow<'a, str>> {
        match f {
            Field::Str(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    };
    let entry = if !matches!(line.main, Field::Missing) || !matches!(line.sub, Field::Missing) {
        let main = nonempty(line.main).unwrap_or_else(|| n.clone());
        Entry { n, main, sub: nonempty(line.sub) }
    } else if !matches!(line.en, Field::Missing) {
        Entry { main: n.clone(), n, sub: nonempty(line.en) }
    } else {
        return Parsed::Bad;
    };
    if line.not_yet {
        Parsed::NotYet
    } else {
        Parsed::Entry(entry)
    }
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
    /// `via` is "todo" or "skipped".
    not_yet: bool,
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

    fn entry(text: &str) -> Option<(String, String, Option<String>)> {
        match parse_line(text.as_bytes()) {
            Parsed::Entry(e) => Some((e.n.into_owned(), e.main.into_owned(), e.sub.map(Cow::into_owned))),
            _ => None,
        }
    }

    fn owned(n: &str, main: &str, sub: Option<&str>) -> Option<(String, String, Option<String>)> {
        Some((n.to_owned(), main.to_owned(), sub.map(str::to_owned)))
    }

    #[test]
    fn lines() {
        assert_eq!(
            entry(r#"{"n": "松島", "main": "松島", "sub": "Matsu-shima", "case": "7 romanise", "via": "rule:roman"}"#),
            owned("松島", "松島", Some("Matsu-shima"))
        );
        assert_eq!(
            entry(r#"{"n": "Château", "main": "Castle", "sub": null, "case": "3", "via": "rule:bare", "check": "wikipedia"}"#),
            owned("Château", "Castle", None)
        );
        // The older format.
        assert_eq!(entry(r#"{"n": "Lac Blanc", "en": "White Lake"}"#), owned("Lac Blanc", "Lac Blanc", Some("White Lake")));
        assert_eq!(entry(r#"{"n": "Montréal", "en": null}"#), owned("Montréal", "Montréal", None));
        // A missing or empty main is the name; an empty sub is none; escapes are read.
        assert_eq!(entry(r#"{"n": "A", "sub": "B"}"#), owned("A", "A", Some("B")));
        assert_eq!(entry(r#"{"n": "A", "main": "", "sub": ""}"#), owned("A", "A", None));
        assert_eq!(entry(r#"{"n": "L’Anse", "main": null, "sub": "Cove \"x\""}"#), owned("L’Anse", "L’Anse", Some("Cove \"x\"")));
        // Extra fields of any type, in any order.
        assert_eq!(entry(r#"{"x": [1, {"y": null}], "sub": "S", "n": "N", "z": 3.5}"#), owned("N", "N", Some("S")));
        // Malformed: not JSON, not an object, no name, an empty name, neither format, wrong types.
        for bad in [
            "",
            "{",
            "[1, 2]",
            r#"{"main": "A", "sub": "B"}"#,
            r#"{"n": "", "main": "A"}"#,
            r#"{"n": null, "en": "A"}"#,
            r#"{"n": "A"}"#,
            r#"{"n": "A", "case": "latin"}"#,
            r#"{"n": 5, "en": "A"}"#,
            r#"{"n": "A", "sub": 5}"#,
            r#"{"n": "A", "en": "B"} trailing"#,
        ] {
            assert_eq!(entry(bad), None, "{bad}");
        }
        assert_eq!(parse_line(b"{\"n\": \"\xff\", \"en\": null}"), Parsed::Bad);
        // Not translated yet: left out, whatever else the line holds; other `via` values are kept.
        for not_yet in [
            r#"{"n": "轆牛嶺", "main": "轆牛嶺", "sub": null, "case": "7 romanise", "via": "todo"}"#,
            r#"{"n": "迎仙谷", "main": "迎仙谷", "sub": null, "via": "skipped"}"#,
            r#"{"n": "TDK 歴史みらい館", "main": "TDK", "sub": null, "via": "skipped"}"#,
            r#"{"via": "todo", "n": "A", "en": "a"}"#,
        ] {
            assert_eq!(parse_line(not_yet.as_bytes()), Parsed::NotYet, "{not_yet}");
        }
        assert_eq!(entry(r#"{"n": "A", "main": "A", "sub": "a", "via": "osm"}"#), owned("A", "A", Some("a")));
        assert_eq!(entry(r#"{"n": "A", "sub": "a", "via": "TODO"}"#), owned("A", "A", Some("a")));
        for odd in [r#"5"#, r#"null"#, r#"true"#, r#"-1.5"#, r#"["todo"]"#, r#"{"x": "todo"}"#] {
            let text = format!("{{\"n\": \"A\", \"sub\": \"a\", \"via\": {odd}}}");
            assert_eq!(entry(&text), owned("A", "A", Some("a")), "{text}");
        }
        // A malformed line stays malformed, marked or not.
        assert_eq!(parse_line(br#"{"n": "A", "via": "todo"}"#), Parsed::Bad);
    }

    #[test]
    fn table() {
        let text = "\u{feff}{\"n\": \"松島\", \"main\": \"松島\", \"sub\": \"Matsu-shima\"}\n\
            not json\n\
            {\"n\": \"Église\", \"main\": \"Église\", \"sub\": null, \"via\": \"todo\"}\n\
            \n\
            {\"n\": \"Église\", \"main\": \"Church\", \"sub\": null}\r\n\
            {\"n\": \"Lac\", \"en\": \"Lake\"}\n\
            {\"n\": \"Lac\", \"en\": \"Lake (again)\"}\n\
            {\"n\": \"Unfinished\", \"main\":";
        let (t, report) = Table::read(&mut text.as_bytes()).expect("read");
        assert_eq!(t.len(), 3);
        assert_eq!((t.ignored(), report.ignored), (1, 1));
        assert_eq!(report.malformed, 1);
        assert_eq!(report.first_malformed, Some(2));
        assert!(report.unfinished);
        assert_eq!(report.bytes, text.len() as u64);
        let get = |n: &str| t.get(hash(n.as_bytes()), n.as_bytes());
        assert_eq!(get("松島"), Some(Rec { main: None, sub: Some("Matsu-shima") }));
        assert_eq!(get("Église"), Some(Rec { main: Some("Church"), sub: None }));
        assert_eq!(get("Lac"), Some(Rec { main: None, sub: Some("Lake (again)") }));
        assert_eq!(get("Unfinished"), None);
        assert_eq!(get("松"), None);
        // A complete last line without its newline counts.
        let (t, report) = Table::read(&mut &b"{\"n\": \"A\", \"en\": \"B\"}"[..]).expect("read");
        assert_eq!((t.len(), report.unfinished), (1, false));
        // Empty files.
        let (t, _) = Table::read(&mut &b""[..]).expect("read");
        assert_eq!((t.len(), t.get(hash(b"A"), b"A")), (0, None));
    }

    #[test]
    fn many_names_and_long_ones() {
        let long = "x".repeat(300);
        let mut text = String::new();
        for i in 0..20_000 {
            text.push_str(&format!("{{\"n\": \"name {i}\", \"main\": \"main {i}\", \"sub\": \"{long}{i}\"}}\n"));
        }
        let (t, _) = Table::read(&mut text.as_bytes()).expect("read");
        assert_eq!(t.len(), 20_000);
        for i in (0..20_000).step_by(7) {
            let n = format!("name {i}");
            let sub = format!("{long}{i}");
            let main = format!("main {i}");
            assert_eq!(t.get(hash(n.as_bytes()), n.as_bytes()), Some(Rec { main: Some(&main), sub: Some(&sub) }));
        }
        assert_eq!(t.get(hash(b"name 20000"), b"name 20000"), None);
        // The index stays under its load limit.
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
