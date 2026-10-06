//! Place search (docs/plan.md §4, The map): the map's own place names, from its labels layer
//! (dem/labels.py): every label in the tiles of the map's areas (its z6 tiles with roads), and the
//! labels that show by zoom 8 worldwide (cities and towns, seas, states, big lakes and parks).
//! Found by a word of the name the map shows (its translation's main and sub, plan §7) or of the
//! place's own name and English, as typed: accents, case and punctuation aside, the words in order.
//! Made on this Mac the first time a search asks after the labels or the translations change
//! (`Index`), in the background on threads of its own: nothing more for the build to make or the
//! mirror to copy.

use crate::data::Data;
use crate::names_live::{self, NamesState};
use crate::S;
use anyhow::{anyhow, Result};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rayon::prelude::*;
use std::collections::BinaryHeap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use store::catalog::Catalog;
use unicode_normalization::UnicodeNormalization;

/// The labels' kinds (dem/labels.py `k`).
const KINDS: [&str; 4] = ["place", "state", "water", "park"];

/// The threads the places are made on: their own few, not the pool the map's requests use, which
/// also bounds the build's memory and its reads of the NAS.
const THREADS: usize = 3;

/// Between a place's names in `text`, and after each in `folded` (no word runs across it).
const SEP: char = '\u{1}';

/// Where a word may start for searching: after a space, and at every ideograph or kana (names
/// written without spaces: 横浜 found by 浜 too). A query of one of these alone is a word already.
/// (Hangul syllables, folded, are jamo: `lead_jamo`.)
fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x2fa1f)
}

/// A hangul syllable's first jamo, as folding (NFKD) leaves it: each syllable starts a word too
/// (특별시 finds 서울특별시).
fn lead_jamo(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115f | 0xa960..=0xa97f)
}

/// A name as searched: lower case, accents and other marks gone, a few letters folded (ß ss, æ ae,
/// ø o, ħ h, ə e, ς σ …), apostrophes dropped, any other punctuation a single space between words.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    fold_into(s, &mut out);
    out
}

/// `fold`, onto the end of `out`.
fn fold_into(s: &str, out: &mut String) {
    let start = out.len();
    let mut gap = false;
    for c in s.nfkd() {
        if unicode_normalization::char::is_combining_mark(c) {
            continue;
        }
        if matches!(c, '\'' | '’' | 'ʻ' | 'ʼ' | '`') {
            continue;
        }
        if !c.is_alphanumeric() {
            gap = out.len() > start;
            continue;
        }
        if gap {
            out.push(' ');
            gap = false;
        }
        for l in c.to_lowercase() {
            match l {
                'ß' => out.push_str("ss"),
                'æ' => out.push_str("ae"),
                'œ' => out.push_str("oe"),
                'þ' => out.push_str("th"),
                'ø' => out.push('o'),
                'đ' | 'ð' => out.push('d'),
                'ł' => out.push('l'),
                'ı' => out.push('i'),
                'ħ' => out.push('h'),
                'ə' => out.push('e'),
                'ς' => out.push('σ'),
                l => out.push(l),
            }
        }
    }
}

/// The byte offsets in `folded` where its words start.
fn word_starts(folded: &str) -> impl Iterator<Item = usize> + '_ {
    let mut prev: Option<char> = None;
    folded.char_indices().filter_map(move |(i, c)| {
        let start = c != ' ' && c != SEP && prev.is_none_or(|p| p == ' ' || p == SEP || cjk(c) || lead_jamo(c));
        prev = Some(c);
        start.then_some(i)
    })
}

/// Whether a folded query is too short to search: a single letter of a script written with spaces
/// (Latin, Cyrillic …) would find every place with a word so starting, so two at least; one
/// ideograph or kana is a word.
fn too_short(q: &str) -> bool {
    let mut cs = q.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => !cjk(c),
        (first, _) => first.is_none(),
    }
}

/// An array in memory mapped for it alone, given back to the system as it's let go: macOS's
/// allocator keeps the last 64 big blocks freed for reuse, whatever their size (its large cache),
/// so the build's arenas and the places, in Vecs, would stay the server's long after they're gone.
struct Big<T> {
    map: Option<memmap2::MmapMut>,
    len: usize,
    of: std::marker::PhantomData<T>,
}

impl<T> Default for Big<T> {
    fn default() -> Self {
        Big { map: None, len: 0, of: std::marker::PhantomData }
    }
}

impl<T: bytemuck::Pod> Big<T> {
    fn with_capacity(n: usize) -> Big<T> {
        let mut b = Big::default();
        b.reserve(n);
        b
    }

    /// `n` zeroes.
    fn zeroed(n: usize) -> Big<T> {
        let mut b = Big::with_capacity(n);
        b.len = n;
        b
    }

    fn len(&self) -> usize {
        self.len
    }

    fn capacity(&self) -> usize {
        self.map.as_ref().map_or(0, |m| m.len() / std::mem::size_of::<T>())
    }

    fn as_slice(&self) -> &[T] {
        match &self.map {
            Some(m) => &bytemuck::cast_slice(&m[..])[..self.len],
            None => &[],
        }
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        match &mut self.map {
            Some(m) => &mut bytemuck::cast_slice_mut(&mut m[..])[..self.len],
            None => &mut [],
        }
    }

    /// Room for `more`: a map twice the size at least, what's in it copied, the old let go.
    fn reserve(&mut self, more: usize) {
        let want = self.len + more;
        if want <= self.capacity() {
            return;
        }
        let size = std::mem::size_of::<T>();
        let mut m = memmap2::MmapMut::map_anon(want.max(2 * self.capacity()).max(16384 / size) * size).expect("memory for the places");
        let old = self.as_slice();
        bytemuck::cast_slice_mut::<u8, T>(&mut m[..])[..old.len()].copy_from_slice(old);
        self.map = Some(m);
    }

    fn push(&mut self, x: T) {
        self.reserve(1);
        self.len += 1;
        let n = self.len;
        self.as_mut_slice()[n - 1] = x;
    }

    fn extend_from_slice(&mut self, xs: &[T]) {
        self.reserve(xs.len());
        let n = self.len;
        self.len += xs.len();
        self.as_mut_slice()[n..].copy_from_slice(xs);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Place {
    lon: f32,
    lat: f32,
    /// Where its names start in `text` (they run to where the next place's do).
    text: u32,
    /// Its importance among all kinds (`rank`).
    score: f32,
    /// The zoom to show it at, in tenths.
    zoom: u8,
    kind: u8,
    class: u8,
    pad: u8,
}

/// The map's places, searchable.
#[derive(Default)]
pub struct Places {
    places: Big<Place>,
    classes: Vec<String>,
    /// Each place's names (`put_names`).
    text: Big<u8>,
    /// Each place's names folded, each once and each followed by SEP.
    folded: Big<u8>,
    /// Each word's start in `folded`, and its place, sorted by the folded text from there.
    words: Big<[u32; 2]>,
}

/// A label as its tile has it.
#[derive(Clone, Copy, Debug)]
struct Label<'a> {
    /// Its feature id: dem/labels.py's row, the same in each tile and zoom it's in (`same`).
    id: Option<u64>,
    name: &'a str,
    en: Option<&'a str>,
    kind: &'a str,
    class: &'a str,
    /// dem/labels.py's importance (its kind's own scale), the zoom it shows from at the default
    /// spacing, and an area's zoom it spans 20 px from.
    s: f64,
    mz: f64,
    ms: Option<f64>,
    lon: f64,
    lat: f64,
}

/// One importance for every kind, as a search ranks them: a place's own (dem/labels.py: 10 a
/// locality to 79 a big capital, ten a class), a state 66 and the log10 of its population (with
/// the big towns and the cities), a sea or an ocean 85, above all; other water and parks by their
/// area (labels.py's: a bay or a strait mapped as a point is 1 km², or 10), a hamlet's at 1 km²
/// (30), a village's at 10,000 (58), up to a city's (72) from 1,000,000, a national park as one ten
/// times its size.
fn rank(kind: &str, class: &str, s: f64) -> f64 {
    match kind {
        "place" => s,
        "state" => 66.0 + s,
        "water" if matches!(class, "ocean" | "sea") => 85.0,
        "water" | "park" => 30.0 + 7.0 * (s - 4.0).clamp(0.0, 6.0),
        _ => s,
    }
}

/// The zoom a place is shown at: a town's streets, a city's whole, a lake or a park as it fills
/// the view, a state.
fn show_zoom(l: &Label) -> f64 {
    let z = match (l.kind, l.class) {
        ("place", "city") => 11.0,
        ("place", "town") => 12.5,
        ("place", "village" | "suburb") => 13.5,
        ("place", _) => 14.5,
        ("state", _) => 6.5,
        _ => l.ms.map_or(l.mz + 2.0, |ms| ms + 4.0),
    };
    z.clamp(4.0, 15.0)
}

/// Each label of one tile of the labels layer (gzip'd or not).
fn each_label(tile: &[u8], z: u8, x: u32, y: u32, mut f: impl FnMut(&Label)) -> Result<()> {
    let raw = names::mvt::gunzip_if_gzip(tile)?;
    let t = names::mvt::Tile::decode(&raw)?;
    for layer in t.layers.iter().filter(|l| l.name == "l" && l.extent > 0) {
        let key = |k: &str| layer.keys.iter().position(|x| x == k).map(|i| i as u32);
        let (kn, ken, kk, kc, ks, kmz, kms) = (key("n"), key("en"), key("k"), key("c"), key("s"), key("mz"), key("ms"));
        for feat in &layer.features {
            let get = |k: Option<u32>| -> Option<&names::mvt::Value> {
                let k = k?;
                feat.tags.chunks_exact(2).find(|p| p[0] == k).and_then(|p| layer.values.get(p[1] as usize))
            };
            let num = |k: Option<u32>| -> Option<f64> {
                match get(k)? {
                    names::mvt::Value::Double(v) => Some(*v),
                    names::mvt::Value::Float(v) => Some(*v as f64),
                    names::mvt::Value::Int(v) | names::mvt::Value::Sint(v) => Some(*v as f64),
                    names::mvt::Value::Uint(v) => Some(*v as f64),
                    _ => None,
                }
            };
            let text = |k: Option<u32>| get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
            let (Some(name), Some((px, py))) = (text(kn), feat.first_point()) else { continue };
            let (lon, lat) = names::mvt::tile_to_lonlat(z as u32, x, y, layer.extent, px as f64, py as f64);
            f(&Label {
                id: feat.id,
                name,
                en: text(ken),
                kind: text(kk).unwrap_or_default(),
                class: text(kc).unwrap_or_default(),
                s: num(ks).unwrap_or(0.0),
                mz: num(kmz).unwrap_or(12.0),
                ms: num(kms),
                lon,
                lat,
            });
        }
    }
    Ok(())
}

/// What makes two labels one place: their feature id (dem/labels.py's row: a label's copies at
/// zooms 8 and 12 have the same) and their name (ids are rows of one run of labels.py, and nothing
/// else ties a catalog's packs to one run). 0 for a label without an id: kept, its copies too.
fn same(id: Option<u64>, name: &str) -> u64 {
    let Some(id) = id else { return 0 };
    let mut h = std::hash::DefaultHasher::new();
    (id, name).hash(&mut h);
    h.finish() | 1
}

/// Puts `k` (`same`'s, not 0) in `set` (open-addressed, its size a power of two); whether it wasn't
/// there.
fn first(set: &mut [u64], k: u64) -> bool {
    let m = set.len() - 1;
    let mut i = (k >> 1) as usize & m;
    loop {
        match set[i] {
            0 => {
                set[i] = k;
                return true;
            }
            x if x == k => return false,
            _ => i = (i + 1) & m,
        }
    }
}

/// A place's names into `text`: its own, then (SEP between) its own English, and the map's main
/// and sub where they aren't its name and English (`names_of` reads them back).
fn put_names(text: &mut String, name: &str, en: Option<&str>, main: &str, sub: Option<&str>) {
    let put = |text: &mut String, s: &str| {
        if s.contains(SEP) {
            text.extend(s.chars().map(|c| if c == SEP { ' ' } else { c }));
        } else {
            text.push_str(s);
        }
    };
    put(text, name);
    if main == name && sub == en {
        if let Some(en) = en {
            text.push(SEP);
            put(text, en);
        }
    } else {
        for s in [en.unwrap_or_default(), main, sub.unwrap_or_default()] {
            text.push(SEP);
            put(text, s);
        }
    }
}

/// A place's names as `put_names` keeps them: its own and its own English, the map's main and sub.
fn names_of(t: &str) -> (&str, Option<&str>, &str, Option<&str>) {
    fn some(s: &str) -> Option<&str> {
        (!s.is_empty()).then_some(s)
    }
    let mut f = t.split(SEP);
    let name = f.next().unwrap_or_default();
    match (f.next(), f.next(), f.next()) {
        (None, ..) => (name, None, name, None),
        (Some(en), None, _) => (name, Some(en), name, Some(en)),
        (Some(en), Some(main), sub) => (name, some(en), main, sub.and_then(some)),
    }
}

/// The places of one pack, read on one of the build's threads into arenas as `Places` keeps them,
/// put together after (`Places::from_chunks`).
#[derive(Default)]
struct Chunk {
    places: Big<Place>,
    /// Each place's label (`same`).
    ids: Big<u64>,
    /// Where each place's folded names start in `folded`.
    starts: Big<u32>,
    classes: Vec<String>,
    text: Big<u8>,
    folded: Big<u8>,
    /// Each word's start: its place, and its offset in the place's folded names.
    words: Big<[u32; 2]>,
    /// (A label's names as they're worked out.)
    buf: String,
    /// The tiles read, and those that couldn't be (the first, said).
    tiles: usize,
    bad: usize,
    first_bad: Option<String>,
}

impl Chunk {
    /// Adds a label (not an isolated dwelling's: a farm's name at most) with its names and the
    /// map's for it: `names`, the translations (plan §7; None before they've loaded), read where
    /// it is, as the map's server does for its tiles (names::mvt::attach).
    fn add(&mut self, l: &Label, names: Option<&names::Names>) {
        let Some(kind) = KINDS.iter().position(|k| *k == l.kind) else { return };
        if l.class == "isolated_dwelling" {
            return;
        }
        let class = match self.classes.iter().position(|c| c == l.class) {
            Some(i) => i,
            None if self.classes.len() < 255 => {
                self.classes.push(l.class.to_string());
                self.classes.len() - 1
            }
            None => return,
        };
        let shown = match names {
            Some(n) => n.display_ref(names::Kind::Place, l.name, l.en, l.lon, l.lat),
            None => names::DisplayRef::new(l.name, l.en),
        };
        // Folded, each name once (one that folds to nothing, or to one before it, not again).
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        for s in [Some(l.name), l.en, Some(shown.main), shown.sub].into_iter().flatten() {
            let at = buf.len();
            fold_into(s, &mut buf);
            if at == buf.len() || buf[..at].split(SEP).any(|g| g == &buf[at..]) {
                buf.truncate(at);
            } else {
                buf.push(SEP);
            }
        }
        if !buf.is_empty() {
            let place = self.places.len() as u32;
            for o in word_starts(&buf) {
                self.words.push([place, o as u32]);
            }
            self.starts.push(self.folded.len() as u32);
            self.folded.extend_from_slice(buf.as_bytes());
            buf.clear();
            put_names(&mut buf, l.name, l.en, shown.main, shown.sub);
            self.places.push(Place {
                lon: l.lon as f32,
                lat: l.lat as f32,
                text: self.text.len() as u32,
                score: rank(l.kind, l.class, l.s) as f32,
                zoom: (show_zoom(l) * 10.0).round() as u8,
                kind: kind as u8,
                class: class as u8,
                pad: 0,
            });
            self.text.extend_from_slice(buf.as_bytes());
            self.ids.push(same(l.id, l.name));
        }
        self.buf = buf;
    }

    fn text_of(&self, i: usize) -> &[u8] {
        let (ps, t) = (self.places.as_slice(), self.text.as_slice());
        &t[ps[i].text as usize..ps.get(i + 1).map_or(t.len(), |p| p.text as usize)]
    }

    fn folded_of(&self, i: usize) -> &[u8] {
        let (ss, f) = (self.starts.as_slice(), self.folded.as_slice());
        &f[ss[i] as usize..ss.get(i + 1).map_or(f.len(), |s| *s as usize)]
    }
}

impl Places {
    /// The places among `labels` (as one pack's), named with `names`.
    #[cfg(test)]
    fn from_labels(labels: &[Label], names: Option<&names::Names>) -> Places {
        let mut c = Chunk::default();
        for l in labels {
            c.add(l, names);
        }
        Places::from_chunks(vec![c])
    }

    /// The places of `chunks`, a label's copies once (`same`: the first chunk's kept), each array
    /// made at its size at once and each chunk let go as it's put in.
    fn from_chunks(chunks: Vec<Chunk>) -> Places {
        let total: usize = chunks.iter().map(|c| c.ids.len()).sum();
        let mut set: Big<u64> = Big::zeroed((total + total / 4).next_power_of_two());
        let mut keep: Big<u8> = Big::with_capacity(total);
        for c in &chunks {
            for &k in c.ids.as_slice() {
                keep.push(u8::from(k == 0 || first(set.as_mut_slice(), k)));
            }
        }
        drop(set);
        let (mut n, mut t, mut f, mut w, mut at) = (0, 0, 0, 0, 0);
        for c in &chunks {
            let keep = &keep.as_slice()[at..at + c.places.len()];
            for i in (0..keep.len()).filter(|&i| keep[i] == 1) {
                n += 1;
                t += c.text_of(i).len();
                f += c.folded_of(i).len();
            }
            w += c.words.as_slice().iter().filter(|x| keep[x[0] as usize] == 1).count();
            at += keep.len();
        }
        let mut p = Places { places: Big::with_capacity(n), classes: Vec::new(), text: Big::with_capacity(t), folded: Big::with_capacity(f), words: Big::with_capacity(w) };
        let mut at = 0;
        for c in chunks {
            let keep = &keep.as_slice()[at..at + c.places.len()];
            at += keep.len();
            let classes: Vec<Option<u8>> = c.classes.iter().map(|k| p.class(k)).collect();
            let (words, mut w) = (c.words.as_slice(), 0);
            for (i, q) in c.places.as_slice().iter().enumerate() {
                let ws = w..w + words[w..].partition_point(|x| x[0] == i as u32);
                w = ws.end;
                let Some(class) = classes[q.class as usize].filter(|_| keep[i] == 1) else { continue };
                let (place, start) = (p.places.len() as u32, p.folded.len() as u32);
                p.places.push(Place { text: p.text.len() as u32, class, ..*q });
                p.text.extend_from_slice(c.text_of(i));
                p.folded.extend_from_slice(c.folded_of(i));
                for x in &words[ws] {
                    p.words.push([start + x[1], place]);
                }
            }
        }
        // (Every name ends with SEP, which no query holds: a suffix to the arena's end orders as
        // its word's own would, and no two are the same.)
        let f = p.folded.as_slice();
        p.words.as_mut_slice().par_sort_unstable_by(|a, b| f[a[0] as usize..].cmp(&f[b[0] as usize..]));
        p
    }

    /// `class`'s number (at most 255 of them).
    fn class(&mut self, class: &str) -> Option<u8> {
        match self.classes.iter().position(|c| c == class) {
            Some(i) => Some(i as u8),
            None if self.classes.len() < 255 => {
                self.classes.push(class.to_string());
                Some((self.classes.len() - 1) as u8)
            }
            None => None,
        }
    }

    pub fn len(&self) -> usize {
        self.places.len()
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.places.capacity() * std::mem::size_of::<Place>()
            + self.text.capacity()
            + self.folded.capacity()
            + self.words.capacity() * std::mem::size_of::<[u32; 2]>()
            + self.classes.iter().map(|c| c.capacity() + std::mem::size_of::<String>()).sum::<usize>()
    }

    fn text_of(&self, i: usize) -> &str {
        let (ps, t) = (self.places.as_slice(), self.text.as_slice());
        std::str::from_utf8(&t[ps[i].text as usize..ps.get(i + 1).map_or(t.len(), |p| p.text as usize)]).unwrap_or_default()
    }

    /// The `n` places best found by `q` (two letters at least, or an ideograph or kana: `too_short`):
    /// those with a word starting so (whole words before its last), a name starting so first, one
    /// that's all of it before those; then the most important, and the nearest to `near` (lon,
    /// lat) among like ones.
    pub fn search(&self, q: &str, near: Option<(f64, f64)>, n: usize) -> Vec<Hit> {
        let q = fold(q);
        if too_short(&q) || n == 0 {
            return Vec::new();
        }
        self.find(&q, near, n)
    }

    /// `search` for a folded query.
    fn find(&self, q: &str, near: Option<(f64, f64)>, n: usize) -> Vec<Hit> {
        let (f, q, words, ps) = (self.folded.as_slice(), q.as_bytes(), self.words.as_slice(), self.places.as_slice());
        let lo = words.partition_point(|w| &f[w[0] as usize..] < q);
        let hi = lo + words[lo..].partition_point(|w| f[w[0] as usize..].starts_with(q));
        // Each place's best match: a whole name (3), a name's start (2), a word's (1).
        let mut tier: Big<u8> = Big::zeroed(ps.len());
        let tier = tier.as_mut_slice();
        let mut found: Vec<u32> = Vec::new();
        for &[at, i] in &words[lo..hi] {
            let at = at as usize;
            let start = at == 0 || f[at - 1] == SEP as u8;
            let t = if start && f[at + q.len()] == SEP as u8 { 3 } else if start { 2 } else { 1 };
            let e = &mut tier[i as usize];
            if *e == 0 {
                found.push(i);
            }
            *e = (*e).max(t);
        }
        // (A heap of the n best so far, the least on top: a place whose rank can't pass it however
        // near isn't measured.)
        let reach = if near.is_some() { NEAR } else { 0.0 };
        let mut best: BinaryHeap<std::cmp::Reverse<Hit>> = BinaryHeap::with_capacity(n + 1);
        for &i in &found {
            let p = &ps[i as usize];
            let mut r = tier[i as usize] as f64 * 100.0 + p.score as f64;
            if best.len() == n && best.peek().is_some_and(|b| r + reach < b.0.rank) {
                continue;
            }
            if let Some((lon, lat)) = near {
                let d = km(lon, lat, p.lon as f64, p.lat as f64);
                r += (NEAR - 3.0 * (1.0 + d).log10()).max(0.0);
            }
            best.push(std::cmp::Reverse(Hit { rank: r, place: i }));
            if best.len() > n {
                best.pop();
            }
        }
        let mut out: Vec<Hit> = best.into_iter().map(|r| r.0).collect();
        out.sort_by(|a, b| b.rank.total_cmp(&a.rank).then(a.place.cmp(&b.place)));
        out
    }

    /// What to call an area (`ring`: [lon, lat] degrees, read as a filled area; `c` its centre):
    /// the most important place in it, nearness to the centre adding to like ones' ranks as in a
    /// search (`NEAR`); towns and other places before water, parks and states. As the map names it.
    /// None when the area holds none.
    pub fn naming(&self, ring: &[[f64; 2]], c: (f64, f64)) -> Option<String> {
        let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in ring {
            (w, e, s, n) = (w.min(p[0]), e.max(p[0]), s.min(p[1]), n.max(p[1]));
        }
        let mut best: Option<(f64, u32)> = None;
        for (i, p) in self.places.as_slice().iter().enumerate() {
            let (lon, lat) = (p.lon as f64, p.lat as f64);
            // (A ring across the antimeridian has longitudes past ±180.)
            let lon = if lon < w { lon + 360.0 } else if lon > e { lon - 360.0 } else { lon };
            if lon < w || lon > e || lat < s || lat > n || !crate::keep::inside(ring, [lon, lat]) {
                continue;
            }
            let r = if p.kind == 0 { 1000.0 } else { 0.0 } + p.score as f64 + (NEAR - 3.0 * (1.0 + km(c.0, c.1, lon, lat)).log10()).max(0.0);
            if best.is_none_or(|(b, _)| r > b) {
                best = Some((r, i as u32));
            }
        }
        let f = self.place(&Hit { rank: 0.0, place: best?.1 });
        Some(if f.main.is_empty() { f.name } else { f.main }.to_string())
    }

    /// A place found: its names, kind and class, where, and the zoom to show it at.
    pub fn place(&self, h: &Hit) -> Found<'_> {
        let i = h.place as usize;
        let p = &self.places.as_slice()[i];
        let (name, en, main, sub) = names_of(self.text_of(i));
        Found { name, en, main, sub, kind: KINDS[p.kind as usize], class: &self.classes[p.class as usize], lon: p.lon as f64, lat: p.lat as f64, zoom: p.zoom as f64 / 10.0 }
    }
}

/// The most nearness adds to a rank (a place at the middle of the view; 3 less for each tenfold
/// the distance, none from 10,000 km).
const NEAR: f64 = 12.0;

/// A place a search found, and how well (higher: better).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub rank: f64,
    pub place: u32,
}

impl Eq for Hit {}
impl PartialOrd for Hit {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Hit {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.rank.total_cmp(&o.rank).then(o.place.cmp(&self.place))
    }
}

/// A place found: its own name and English, and the map's (main and sub); what it is, where, and
/// the zoom to show it at.
pub struct Found<'a> {
    pub name: &'a str,
    pub en: Option<&'a str>,
    pub main: &'a str,
    pub sub: Option<&'a str>,
    pub kind: &'a str,
    pub class: &'a str,
    pub lon: f64,
    pub lat: f64,
    pub zoom: f64,
}

/// The labels' packs to read in `cat`: the hi packs of the map's areas (zoom 12: every label), then
/// each lo pack (zoom 8: what shows by then, worldwide), as content names (so they're the one
/// catalog's) and the zoom read. (The areas' first: a label in both is kept at zoom 12, its place
/// the more exact.)
fn sources(cat: &Catalog) -> Vec<(String, u8)> {
    let Some(layer) = cat.layers.get("labels") else { return Vec::new() };
    let content = |l: &String| cat.files.get(l).map(|f| f.file.clone());
    let hi = layer.hi.iter().filter(|(t, _)| cat.hidata.contains_key(*t)).filter_map(|(_, l)| Some((content(l)?, 12u8)));
    hi.chain(layer.lo.values().filter_map(|l| Some((content(l)?, 8u8)))).collect()
}

/// The places' key: the packs they're read from and the translations' version they're named with.
/// A catalog whose labels and areas are the same, with the same translations, keeps them.
fn key_of(packs: &[(String, u8)], names_version: u64) -> u64 {
    let mut h = blake3::Hasher::new();
    for (c, z) in packs {
        h.update(c.as_bytes());
        h.update(&[0, *z]);
    }
    h.update(&names_version.to_le_bytes());
    u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap_or_default())
}

/// What a build made, and what it couldn't read.
#[derive(Default)]
struct Built {
    places: Places,
    /// Packs that couldn't be read (the NAS away).
    unread: usize,
    /// Tiles that couldn't be (their gzip or their tile).
    bad: usize,
    /// The first of either, said.
    first: Option<String>,
}

/// The places in `packs` (`sources`), named with `names` (the translations; None before they've
/// loaded), made on THREADS of their own. A pack or a tile that can't be read is passed over (and
/// counted); a build that read none is a failure, with why.
fn build(data: &Data, packs: &[(String, u8)], names: Option<&names::Names>) -> Result<Built> {
    if packs.is_empty() {
        return Ok(Built::default());
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(THREADS).thread_name(|i| format!("places-{i}")).build()?;
    pool.install(|| {
        let read: Vec<Result<Chunk>> = packs.par_iter().map(|(content, z)| read_pack(data, content, *z, names)).collect();
        let mut b = Built::default();
        let mut chunks = Vec::with_capacity(read.len());
        let mut first_bad = None;
        for (r, (content, _)) in read.into_iter().zip(packs) {
            match r {
                Ok(mut c) => {
                    b.bad += c.bad;
                    first_bad = first_bad.or(c.first_bad.take());
                    chunks.push(c);
                }
                Err(e) => {
                    b.unread += 1;
                    b.first.get_or_insert_with(|| format!("{content}: {e:#}"));
                }
            }
        }
        b.first = b.first.or(first_bad);
        if chunks.iter().all(|c| c.tiles == 0) && (b.unread > 0 || b.bad > 0) {
            let why = b.first.unwrap_or_default();
            return Err(if b.unread > 0 { anyhow!("none of the labels' {} packs could be read ({why})", packs.len()) } else { anyhow!("none of the labels' {} tiles could be read ({why})", b.bad) });
        }
        b.places = Places::from_chunks(chunks);
        Ok(b)
    })
}

/// The places in one pack's tiles at zoom `z`.
fn read_pack(data: &Data, content: &str, z: u8, names: Option<&names::Names>) -> Result<Chunk> {
    let mut c = Chunk::default();
    data.pack_tiles(content, z, |x, y, b| match each_label(b, z, x, y, |l| c.add(l, names)) {
        Ok(()) => c.tiles += 1,
        Err(e) => {
            c.bad += 1;
            c.first_bad.get_or_insert_with(|| format!("{z}/{x}/{y} in {content}: {e:#}"));
        }
    })?;
    Ok(c)
}

/// What a search finds of the index.
pub enum Got {
    /// Places to search (perhaps of labels or translations since changed: newer are being made).
    Ready(Arc<Places>),
    /// None yet: the first are being made.
    Making,
    /// None: the last build failed. Why, and how long until it's tried again.
    Failed(String, Duration),
}

/// The places of the catalog being served: made in the background when a search first asks for
/// them, and again when the map's labels or the translations change (the last made searched
/// meanwhile); let go once no search has asked for IDLE. A build that fails (the NAS away) is
/// tried again after REST at the soonest; one that couldn't read every pack, after PARTIAL.
#[derive(Default)]
pub struct Index {
    made: RwLock<Option<Made>>,
    making: AtomicBool,
    /// The places made so far (each made's number).
    count: AtomicU64,
    /// Why the last build failed, and when.
    failed: Mutex<Option<(String, Instant)>>,
    used: Mutex<Option<Instant>>,
    /// The key of the places to search (`key_of`) for the catalog generation and translations
    /// version it was worked out at.
    key: Mutex<Option<((u64, u64), u64)>>,
}

struct Made {
    places: Arc<Places>,
    key: u64,
    /// Which made it is (`Index::count`): its thread keeps it.
    n: u64,
    /// When, if some packs couldn't be read.
    partial: Option<Instant>,
}

/// How long the places are kept without a search.
const IDLE: Duration = Duration::from_secs(30 * 60);
/// How long after a failed build another may start.
const REST: Duration = Duration::from_secs(60);
/// How long places made without some packs are searched before they're made again.
const PARTIAL: Duration = Duration::from_secs(5 * 60);

/// Ends a build, however it ends: one that panicked is noted as failed.
struct Making<'a>(&'a Index);

impl Drop for Making<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("places: the build panicked");
            *self.0.failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(("the build failed (a fault: the server's log says where)".into(), Instant::now()));
        }
        self.0.making.store(false, Ordering::SeqCst);
    }
}

impl Index {
    /// The places to search now, their making started when they aren't of the labels and the
    /// translations as they are (and may be made).
    pub fn get(self: &Arc<Self>, data: &Arc<Data>, names: &Arc<NamesState>) -> Got {
        *self.used.lock().unwrap() = Some(Instant::now());
        let key = self.key(data, names);
        let cur = self.made.read().unwrap().as_ref().map(|m| (m.places.clone(), m.key == key && m.partial.is_none_or(|t| t.elapsed() < PARTIAL)));
        let resting = self.failed.lock().unwrap().as_ref().is_some_and(|(_, t)| t.elapsed() < REST);
        if !cur.as_ref().is_some_and(|c| c.1) && !resting && !self.making.swap(true, Ordering::SeqCst) {
            self.spawn(data.clone(), names.clone());
        }
        match cur {
            Some((p, _)) => Got::Ready(p),
            None if self.making.load(Ordering::SeqCst) => Got::Making,
            None => match self.failed.lock().unwrap().as_ref() {
                Some((why, t)) => Got::Failed(why.clone(), REST.saturating_sub(t.elapsed())),
                None => Got::Making,
            },
        }
    }

    /// The key of the places to search (`key_of`), worked out again when the catalog or the
    /// translations change.
    fn key(&self, data: &Data, names: &NamesState) -> u64 {
        // (The generation before the catalog: a catalog newer than its generation is worked out
        // again at the next.)
        let now = (data.generation.load(Ordering::Relaxed), names.version_all());
        let mut k = self.key.lock().unwrap();
        match *k {
            Some((at, key)) if at == now => key,
            _ => {
                let key = key_of(&sources(&data.catalog()), now.1);
                *k = Some((now, key));
                key
            }
        }
    }

    /// Makes the places on a thread of its own, which keeps them while they're searched.
    fn spawn(self: &Arc<Self>, data: Arc<Data>, names: Arc<NamesState>) {
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("places".into()).spawn(move || {
            let made = {
                let _making = Making(&me);
                // (One catalog and one copy of the tables throughout: what its key says.)
                let (cat, tables) = (data.catalog(), names.snapshot());
                let packs = sources(&cat);
                let key = key_of(&packs, names_live::version_all(tables.as_ref()));
                let t = Instant::now();
                match build(&data, &packs, tables.as_ref()) {
                    Ok(b) => {
                        let skipped = match (b.unread, b.bad) {
                            (0, 0) => String::new(),
                            (u, n) => format!("; passed over: {u} packs, {n} tiles (the first, {})", b.first.as_deref().unwrap_or_default()),
                        };
                        eprintln!("places: {} of catalog {} ({} MB) in {:.1} s{skipped}", b.places.len(), cat.n, b.places.bytes() >> 20, t.elapsed().as_secs_f64());
                        let n = me.count.fetch_add(1, Ordering::SeqCst) + 1;
                        let partial = (b.unread > 0).then(Instant::now);
                        *me.made.write().unwrap() = Some(Made { places: Arc::new(b.places), key, n, partial });
                        *me.failed.lock().unwrap() = None;
                        Some(n)
                    }
                    Err(e) => {
                        eprintln!("places: {e:#}");
                        *me.failed.lock().unwrap() = Some((format!("{e:#}"), Instant::now()));
                        None
                    }
                }
            };
            let Some(n) = made else { return };
            // Kept while they're searched; let go IDLE after the last (unless others took their
            // place: their own thread keeps those).
            loop {
                std::thread::sleep(Duration::from_secs(60));
                if me.made.read().unwrap().as_ref().is_none_or(|m| m.n != n) {
                    return;
                }
                if me.used.lock().unwrap().is_none_or(|t| t.elapsed() >= IDLE) {
                    let mut made = me.made.write().unwrap();
                    if made.as_ref().is_some_and(|m| m.n == n) {
                        *made = None;
                        eprintln!("places: let go, unsearched for {} min", IDLE.as_secs() / 60);
                    }
                    return;
                }
            }
        });
        if spawned.is_err() {
            self.making.store(false, Ordering::SeqCst);
        }
    }
}

#[derive(serde::Deserialize)]
pub struct SearchQ {
    #[serde(default)]
    q: String,
    /// Where the map's looking, "lon,lat": like places nearer it first.
    near: Option<String>,
    n: Option<usize>,
}

/// `/api/places?q=…&near=<lon>,<lat>&n=…`: the places `q` finds, the best first (8, at most 20),
/// each its own name and English, the map's (main, sub), kind and class, where and the zoom to
/// show it at; `ready` false while the map's places are first made (an empty `q` asks for that
/// alone: the search box, opened, has them made while its words are typed), with `failed` (why)
/// and `again` (in how many seconds it's tried again) when the last try failed. The box's asks
/// again while they're made carry `poll=1`: they aren't the map in use (main.rs).
pub async fn search(State(s): State<S>, Query(q): Query<SearchQ>) -> Response {
    let s2 = s.clone();
    let found = tokio::task::spawn_blocking(move || {
        let near = q.near.as_deref().and_then(|v| v.split_once(',')).and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))).filter(|(lon, lat)| lon.is_finite() && lat.is_finite());
        let p = match s2.places.get(&s2.data, &s2.names) {
            Got::Ready(p) => p,
            Got::Making => return serde_json::json!({ "ready": false, "hits": [] }),
            Got::Failed(why, again) => return serde_json::json!({ "ready": false, "hits": [], "failed": why, "again": again.as_secs() }),
        };
        let hits: Vec<serde_json::Value> = p
            .search(&q.q, near, q.n.unwrap_or(8).min(20))
            .iter()
            .map(|h| {
                let f = p.place(h);
                serde_json::json!({ "name": f.name, "en": f.en, "main": f.main, "sub": f.sub, "kind": f.kind, "class": f.class, "lon": f.lon, "lat": f.lat, "zoom": f.zoom })
            })
            .collect();
        serde_json::json!({ "ready": true, "hits": hits })
    })
    .await;
    match found {
        Ok(v) => ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Great-circle distance (km).
fn km(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().min(1.0).asin()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn label(name: &'static str, en: Option<&'static str>, kind: &'static str, class: &'static str, s: f64, lon: f64, lat: f64) -> Label<'static> {
        Label { id: None, name, en, kind, class, s, mz: 10.0, ms: None, lon, lat }
    }

    /// Each hit's name as the map shows it, and its class.
    fn hits_of(p: &Places, q: &str, near: Option<(f64, f64)>) -> Vec<String> {
        p.search(q, near, 10).iter().map(|h| format!("{} {}", p.place(h).main, p.place(h).class)).collect()
    }

    #[test]
    fn names_fold_as_typed() {
        assert_eq!(fold("Saint-Jean-sur-Richelieu"), "saint jean sur richelieu");
        assert_eq!(fold("Île d'Orléans"), "ile dorleans");
        assert_eq!(fold("  Mt. St. Helens "), "mt st helens");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Ærøskøbing"), "aeroskobing");
        assert_eq!(fold("Łódź"), "lodz");
        assert_eq!(fold("ＴＯＫＹＯ"), "tokyo");
        assert_eq!(fold("横浜"), "横浜");
        // Greek's final sigma as its other, Maltese ħ, Azerbaijani ə.
        assert_eq!(fold("ΠΑΤΡΑΣ"), "πατρασ");
        assert_eq!(fold("Πάτρας"), "πατρασ");
        assert_eq!(fold("Ħamrun"), "hamrun");
        assert_eq!(fold("Gəncə"), "gence");
        assert_eq!(word_starts("lake louise").collect::<Vec<_>>(), [0, 5]);
        // Every ideograph starts a word; after the separator, the English's first word.
        assert_eq!(word_starts("横浜\u{1}yokohama\u{1}").collect::<Vec<_>>(), [0, 3, 7]);
        // Each hangul syllable (its first jamo, folded).
        assert_eq!(word_starts(&fold("서울특별시")).count(), 5);
    }

    #[test]
    fn a_search_finds_by_any_word_the_whole_name_first_then_the_most_important_and_near() {
        let p = Places::from_labels(
            &[
                label("Lake Louise", None, "place", "hamlet", 33.0, -116.18, 51.43),
                label("Louiseville", None, "place", "town", 63.0, -72.94, 46.26),
                label("Louise", None, "place", "village", 52.0, -95.0, 45.0),
                label("Louise", None, "place", "village", 52.0, -122.0, 49.0),
                label("Lake Louise", None, "water", "lake", 4.0, -116.24, 51.41),
                label("横浜", Some("Yokohama"), "place", "city", 76.0, 139.64, 35.44),
                label("Somewhere", None, "place", "isolated_dwelling", 0.0, 1.0, 1.0),
            ],
            None,
        );
        assert_eq!(p.len(), 6, "the isolated dwelling left out");
        // The whole name first (the two villages, the nearer first), then names that start so,
        // then a word inside one.
        assert_eq!(hits_of(&p, "louise", Some((-120.0, 49.5))), ["Louise village", "Louise village", "Louiseville town", "Lake Louise hamlet", "Lake Louise lake"]);
        assert_eq!(p.place(&p.search("louise", Some((-120.0, 49.5)), 1)[0]).lon, -122.0);
        // Words in order, as typed; the English, and inside a name without spaces.
        assert_eq!(hits_of(&p, "lake lou", None), ["Lake Louise hamlet", "Lake Louise lake"]);
        assert!(hits_of(&p, "louise lake", None).is_empty());
        assert_eq!(hits_of(&p, "yoko", None), ["横浜 city"]);
        assert_eq!(hits_of(&p, "浜", None), ["横浜 city"]);
        assert!(p.search("", None, 10).is_empty());
        let f = p.place(&p.search("yokohama", None, 1)[0]);
        assert_eq!((f.name, f.en, f.main, f.sub, f.kind, f.zoom), ("横浜", Some("Yokohama"), "横浜", Some("Yokohama"), "place", 11.0));
        // At most n, the best.
        assert_eq!(hits_of(&p, "lo", None).len(), 5);
        assert_eq!(p.search("lo", None, 2).len(), 2);
    }

    #[test]
    fn one_letter_alone_isnt_searched_but_an_ideograph_is() {
        let p = Places::from_labels(&[label("Sable", None, "place", "town", 60.0, 0.0, 0.0), label("横浜", None, "place", "city", 76.0, 139.6, 35.4)], None);
        assert!(p.search("s", None, 8).is_empty());
        assert!(p.search(" S. ", None, 8).is_empty());
        assert_eq!(p.search("sa", None, 8).len(), 1);
        assert_eq!(p.search("浜", None, 8).len(), 1);
        // (Found without the rule all the same.)
        assert_eq!(p.find("s", None, 8).len(), 1);
    }

    #[test]
    fn a_place_ranks_by_the_best_of_its_names_matches() {
        // "louis": Port Louis's English is all of it (3, its own name has it as a word), so it
        // comes before Louisbourg's start (2) though less important; Saint-Louis-du-Ha! Ha! has
        // it only inside (1).
        let p = Places::from_labels(
            &[
                label("Saint-Louis-du-Ha! Ha!", None, "place", "village", 55.0, -68.98, 47.67),
                label("Louisbourg", None, "place", "town", 64.0, -59.97, 45.92),
                label("Port Louis", Some("Louis"), "place", "hamlet", 31.0, 57.5, -20.16),
            ],
            None,
        );
        assert_eq!(hits_of(&p, "louis", None), ["Port Louis hamlet", "Louisbourg town", "Saint-Louis-du-Ha! Ha! village"]);
        let r: Vec<f64> = p.search("louis", None, 10).iter().map(|h| h.rank).collect();
        assert_eq!(r, [331.0, 264.0, 155.0]);
    }

    #[test]
    fn hangul_is_found_from_any_syllable() {
        let p = Places::from_labels(&[label("서울특별시", Some("Seoul"), "place", "city", 79.0, 127.0, 37.5), label("부산광역시", Some("Busan"), "place", "city", 78.0, 129.0, 35.1)], None);
        assert_eq!(hits_of(&p, "특별시", None), ["서울특별시 city"]);
        assert_eq!(hits_of(&p, "서울", None), ["서울특별시 city"]);
        assert_eq!(hits_of(&p, "광역", None), ["부산광역시 city"]);
        assert_eq!(hits_of(&p, "seoul", None), ["서울특별시 city"]);
    }

    #[test]
    fn a_labels_copies_are_one_place_and_namesakes_two() {
        let banff = Label { id: Some(7), ..label("Banff", None, "place", "town", 62.0, -115.57, 51.18) };
        // Its zoom-12 copy, and its zoom-8 one a few metres off.
        let p = Places::from_labels(&[banff, Label { lon: -115.5702, lat: 51.1801, ..banff }], None);
        assert_eq!(p.len(), 1);
        assert_eq!(p.place(&p.search("banff", None, 1)[0]).lon, -115.57f32 as f64, "the first copy kept");
        // Another of its name a kilometre off is another place, as is another run's label of
        // that id with another name; a label without an id is kept as it is.
        let near = Label { id: Some(8), lon: -115.556, ..banff };
        let other = Label { id: Some(7), name: "Canmore", ..banff };
        let bare = Label { id: None, ..banff };
        assert_eq!(Places::from_labels(&[banff, near, other, bare, bare], None).len(), 5);
    }

    /// Translation tables, as the server reads them (`translations/<area>/…`), long settled.
    fn tables(lines: &[(&str, &str)]) -> (tempfile::TempDir, names::Names) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in lines {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))).unwrap();
        }
        let n = names::Names::load(dir.path()).unwrap();
        (dir, n)
    }

    #[test]
    fn what_the_map_shows_is_found_and_said_with_the_places_own_names() {
        assert_eq!((names::area_at(141.06, 38.37), names::area_at(1.0, 47.0)), (Some("jp"), Some("fr")));
        let (_dir, n) = tables(&[
            ("jp/places-jp.jsonl", "{\"n\": \"松島\", \"main\": \"松島\", \"sub\": \"Matsu-shima\"}\n"),
            ("fr/places-fr.jsonl", "{\"n\": \"Château\", \"main\": \"Castle\", \"sub\": null}\n"),
        ]);
        let p = Places::from_labels(
            &[
                label("松島", None, "place", "town", 60.0, 141.06, 38.37),
                label("Château", Some("The Castle"), "place", "hamlet", 30.0, 1.0, 47.0),
                // (No line for it: its own English the sub, as on the map.)
                label("Banff", Some("Banff Townsite"), "place", "town", 62.0, -115.57, 51.18),
            ],
            Some(&n),
        );
        let f = p.place(&p.search("shima", None, 1)[0]);
        assert_eq!((f.name, f.en, f.main, f.sub), ("松島", None, "松島", Some("Matsu-shima")));
        // The map's main, the place's own name, its own English: each finds it.
        for q in ["castle", "chateau", "the castle"] {
            let f = p.place(&p.search(q, None, 1)[0]);
            assert_eq!((f.name, f.en, f.main, f.sub), ("Château", Some("The Castle"), "Castle", None), "{q}");
        }
        let f = p.place(&p.search("townsite", None, 1)[0]);
        assert_eq!((f.name, f.en, f.main, f.sub), ("Banff", Some("Banff Townsite"), "Banff", Some("Banff Townsite")));
        // Its whole main name is a whole name.
        assert_eq!(p.search("castle", None, 1)[0].rank, 330.0);
    }

    /// A zoom-`z` labels tile of `labels` (id, name, English, kind, class, importance, where in
    /// the tile), as dem/labels.py writes them.
    fn labels_tile(labels: &[(u64, &str, Option<&str>, &str, &str, f64, (i32, i32))]) -> Vec<u8> {
        use names::mvt::Value;
        let keys: Vec<String> = ["n", "en", "k", "c", "s"].iter().map(|k| k.to_string()).collect();
        let (mut values, mut features) = (Vec::new(), Vec::new());
        for (id, name, en, kind, class, imp, (px, py)) in labels {
            let mut tags = Vec::new();
            let mut put = |key: u32, v: Value| {
                values.push(v);
                tags.extend([key, values.len() as u32 - 1]);
            };
            put(0, Value::String(name.to_string()));
            if let Some(en) = en {
                put(1, Value::String(en.to_string()));
            }
            put(2, Value::String(kind.to_string()));
            put(3, Value::String(class.to_string()));
            put(4, Value::Double(*imp));
            let zz = |n: i32| ((n << 1) ^ (n >> 31)) as u32;
            features.push(names::mvt::Feature { id: Some(*id), tags, geom_type: Some(1), geometry: vec![9, zz(*px), zz(*py)], ..Default::default() });
        }
        let layer = names::mvt::Layer { name: "l".into(), version: 2, extent: 4096, keys, values, features, unknown: Vec::new() };
        names::mvt::gzip(&names::mvt::Tile { layers: vec![layer], unknown: Vec::new() }.encode()).unwrap()
    }

    /// One pack of the labels layer: its key ("3/4/2" lo, "6/32/21" hi, its hidata named) and
    /// tiles; `hash` its content name's.
    type Pack<'a> = (&'a str, u64, Vec<(u8, u32, u32, Vec<u8>)>);

    /// Catalog `n` of a NAS folder, its labels these packs (written there unless `missing`).
    fn catalog(nas: &std::path::Path, n: u64, packs: &[Pack], missing: &[&str]) {
        let mut cat = store::catalog::Catalog::new(n);
        let mut layer = store::catalog::Layer::default();
        for (key, hash, tiles) in packs {
            let lo = key.starts_with("3/");
            let logical = format!("layers/labels/{}/{}", if lo { "lo" } else { "hi" }, key.replace('/', "-"));
            let content = format!("{logical}.{hash:016x}.pack");
            let path = nas.join(&content);
            if !missing.contains(key) && !path.exists() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let mut w = store::pack::PackWriter::create(&path, serde_json::json!({ "layer": "labels" }), true).unwrap();
                for (z, x, y, tile) in tiles {
                    w.add(*z, *x, *y, tile, 0).unwrap();
                }
                w.finish().unwrap();
            }
            if lo {
                layer.lo.insert(key.to_string(), logical.clone());
            } else {
                layer.hi.insert(key.to_string(), logical.clone());
                cat.hidata.insert(key.to_string(), format!("global/hidata/{}", key.replace('/', "-")));
            }
            cat.files.insert(logical, store::catalog::FileRef { file: content, size: 1, ..Default::default() });
        }
        cat.layers.insert("labels".into(), layer);
        store::catalog::write_copy(&nas.join("catalog"), &cat).unwrap();
    }

    /// A lo pack (3/4/2) with a zoom-8 tile of Banff and Yokohama, and a hi pack of one of the
    /// map's areas (6/32/21) with a zoom-12 tile of Lake Louise and Banff (its copy: dem/labels.py
    /// puts what shows by zoom 8 in its zoom-12 tile too).
    fn packs() -> Vec<Pack<'static>> {
        let lo = labels_tile(&[(1, "Banff", None, "place", "town", 62.0, (100, 200)), (2, "横浜", Some("Yokohama"), "place", "city", 76.0, (900, 900))]);
        let hi = labels_tile(&[(3, "Lake Louise", None, "water", "lake", 5.0, (2048, 2048)), (1, "Banff", None, "place", "town", 62.0, (1600, 3200))]);
        vec![("3/4/2", 1, vec![(8, 130, 70, lo)]), ("6/32/21", 2, vec![(12, 2050, 1350, hi)])]
    }

    async fn ask(s: &S, q: &str, near: Option<&str>) -> serde_json::Value {
        let r = search(State(s.clone()), Query(SearchQ { q: q.into(), near: near.map(str::to_string), n: None })).await;
        assert_eq!(r.headers().get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap().to_string()).as_deref(), Some("no-store"));
        let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()
    }

    /// Asks until the places are made (or the build fails): the last answer.
    async fn ready(s: &S) -> serde_json::Value {
        for _ in 0..400 {
            let v = ask(s, "", None).await;
            if v["ready"] == true || v.get("failed").is_some() {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the places were never made");
    }

    #[tokio::test]
    async fn the_search_answers_once_the_maps_places_are_made() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        catalog(nas.path(), 1, &packs(), &[]);
        let s = crate::test_state(home.path(), nas.path());
        // The first ask has them made: not ready until they are.
        assert_eq!(ask(&s, "", None).await, serde_json::json!({ "ready": false, "hits": [] }));
        assert_eq!(ready(&s).await["ready"], true);
        // Banff once (its zoom-12 copy: where it is, from its tile there).
        let v = ask(&s, "banff", Some("-115.5,51.2")).await;
        assert_eq!(v["hits"].as_array().map(Vec::len), Some(1));
        let (lon, lat) = names::mvt::tile_to_lonlat(12, 2050, 1350, 4096, 1600.0, 3200.0);
        let b = &v["hits"][0];
        assert_eq!((b["name"].as_str(), b["main"].as_str(), b["class"].as_str(), b["zoom"].as_f64()), (Some("Banff"), Some("Banff"), Some("town"), Some(12.5)));
        assert_eq!((b["lon"].as_f64(), b["lat"].as_f64()), (Some(lon as f32 as f64), Some(lat as f32 as f64)));
        let y = &ask(&s, "yoko", Some("nonsense")).await["hits"][0];
        assert_eq!((y["name"].as_str(), y["en"].as_str(), y["main"].as_str(), y["sub"].as_str()), (Some("横浜"), Some("Yokohama"), Some("横浜"), Some("Yokohama")));
        let l = &ask(&s, "lake lou", None).await["hits"][0];
        assert_eq!(l["kind"].as_str(), Some("water"));
        assert!(ask(&s, "nowhere", None).await["hits"].as_array().unwrap().is_empty());
        assert!(ask(&s, "b", None).await["hits"].as_array().unwrap().is_empty(), "one letter");
    }

    #[test]
    fn the_mirrors_packs_are_read_as_the_nass() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        catalog(nas.path(), 1, &packs(), &[]);
        let on_nas = crate::test_state(home.path(), nas.path());
        let b = build(&on_nas.data, &sources(&on_nas.data.catalog()), None).unwrap();
        // The same packs in this Mac's mirror.
        let home = tempfile::tempdir().unwrap();
        for (c, _) in sources(&on_nas.data.catalog()) {
            let p = home.path().join("mirror").join(&c);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::copy(nas.path().join(&c), p).unwrap();
        }
        let data = crate::data::Data::open(crate::data::Options { home: home.path().to_owned(), nas_root: Some(nas.path().to_owned()), mirror: true, reserve: 0 }).unwrap();
        assert!(sources(&data.catalog()).iter().all(|(c, _)| data.mirror.as_ref().unwrap().local(c).is_some()));
        let m = build(&data, &sources(&data.catalog()), None).unwrap();
        let all = |p: &Places| (0..p.len()).map(|i| p.text_of(i).to_string()).collect::<Vec<_>>();
        assert_eq!((m.places.len(), m.unread, m.bad), (3, 0, 0));
        assert_eq!(all(&m.places), all(&b.places));
    }

    #[tokio::test]
    async fn the_places_are_named_as_the_map_shows_them() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // Yokohama's tile is in Japan's area (z8 130/70 isn't: a tile there instead).
        let (x, y) = (227, 101);
        let lo = labels_tile(&[(2, "横浜", Some("Yokohama"), "place", "city", 76.0, (2048, 2048))]);
        catalog(nas.path(), 1, &[("3/7/3", 1, vec![(8, x, y, lo)])], &[]);
        let p = home.path().join("translations/jp/places-jp.jsonl");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{\"n\": \"横浜\", \"main\": \"横浜\", \"sub\": \"Yokohama-shi\"}\n").unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))).unwrap();
        let s = crate::test_state(home.path(), nas.path());
        let (lon, lat) = names::mvt::tile_to_lonlat(8, x, y, 4096, 2048.0, 2048.0);
        assert_eq!(names::area_at(lon, lat), Some("jp"));
        s.names.reload();
        assert_eq!(ready(&s).await["ready"], true);
        let h = &ask(&s, "yokohama shi", None).await["hits"][0];
        assert_eq!((h["name"].as_str(), h["en"].as_str(), h["main"].as_str(), h["sub"].as_str()), (Some("横浜"), Some("Yokohama"), Some("横浜"), Some("Yokohama-shi")));
    }

    #[tokio::test]
    async fn a_build_passes_over_what_it_cant_read_and_one_that_reads_nothing_says_why() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // A zoom-8 tile that isn't one (gzip's magic, then nonsense), beside a good one; the hi
        // pack not on the NAS.
        let mut ps = packs();
        ps[0].2.push((8, 131, 70, vec![0x1f, 0x8b, 1, 2, 3]));
        catalog(nas.path(), 1, &ps, &["6/32/21"]);
        let s = crate::test_state(home.path(), nas.path());
        assert_eq!(ready(&s).await["ready"], true);
        assert_eq!(ask(&s, "banff", None).await["hits"].as_array().map(Vec::len), Some(1));
        assert!(ask(&s, "lake louise", None).await["hits"].as_array().unwrap().is_empty());
        let made = s.places.made.read().unwrap();
        assert!(made.as_ref().is_some_and(|m| m.partial.is_some()), "made again later");
        drop(made);
        let b = build(&s.data, &sources(&s.data.catalog()), None).unwrap();
        assert_eq!((b.places.len(), b.unread, b.bad), (2, 1, 1));

        // Nothing readable: not ready, and why, and when it's tried again.
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        catalog(nas.path(), 1, &packs(), &["3/4/2", "6/32/21"]);
        let s = crate::test_state(home.path(), nas.path());
        let v = ready(&s).await;
        assert_eq!(v["ready"], false);
        let why = v["failed"].as_str().unwrap();
        assert!(why.starts_with("none of the labels' 2 packs could be read (layers/labels/"), "{why}");
        assert!((55..=60).contains(&v["again"].as_u64().unwrap()), "{v}");
        // (Resting meanwhile: asked again, it isn't made again.)
        assert_eq!(ask(&s, "banff", None).await["failed"].as_str(), Some(why));
        assert!(!s.places.making.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn the_places_are_kept_for_a_catalog_whose_labels_didnt_change() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ps = packs();
        catalog(nas.path(), 1, &ps, &[]);
        let s = crate::test_state(home.path(), nas.path());
        assert_eq!(ready(&s).await["ready"], true);
        let first = |s: &S| s.places.made.read().unwrap().as_ref().map(|m| (m.n, Arc::as_ptr(&m.places)));
        let was = first(&s);
        // A new catalog, its labels the same (another layer changed): the same places.
        catalog(nas.path(), 2, &ps, &[]);
        assert!(s.data.refresh_catalog());
        assert_eq!(s.data.catalog().n, 2);
        assert_eq!(ask(&s, "banff", None).await["hits"].as_array().map(Vec::len), Some(1));
        assert!(!s.places.making.load(Ordering::SeqCst), "not made again");
        assert_eq!(first(&s), was);
        // One whose hi pack changed: made again (the old searched meanwhile).
        let mut ps3 = packs();
        ps3[1].1 = 3;
        ps3[1].2 = vec![(12, 2050, 1350, labels_tile(&[(3, "Moraine Lake", None, "water", "lake", 5.0, (2048, 2048))]))];
        catalog(nas.path(), 3, &ps3, &[]);
        assert!(s.data.refresh_catalog());
        assert_eq!(ask(&s, "lake louise", None).await["hits"].as_array().map(Vec::len), Some(1), "the old, meanwhile");
        for _ in 0..400 {
            if first(&s).is_some_and(|f| f.0 == 2) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(first(&s).map(|f| f.0), Some(2));
        assert_eq!(ask(&s, "moraine", None).await["hits"].as_array().map(Vec::len), Some(1));
        assert!(ask(&s, "lake louise", None).await["hits"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_build_that_panics_ends_as_a_failure() {
        let ix = Arc::new(Index::default());
        ix.making.store(true, Ordering::SeqCst);
        let i2 = ix.clone();
        let r = std::thread::spawn(move || {
            let _making = Making(&i2);
            panic!("(a fault, as a test)");
        })
        .join();
        assert!(r.is_err());
        assert!(!ix.making.load(Ordering::SeqCst));
        assert!(ix.failed.lock().unwrap().as_ref().is_some_and(|(why, _)| why.starts_with("the build failed")));
    }

    /// The live build's labels, from the project folder named by SCENIC_PLACES_ROOT: how many
    /// places, how long, how much memory (run by hand).
    #[test]
    #[ignore]
    fn the_builds_places() {
        let Some(root) = std::env::var_os("SCENIC_PLACES_ROOT") else { return };
        let home = tempfile::tempdir().unwrap();
        let data = crate::data::Data::open(crate::data::Options { home: home.path().to_owned(), nas_root: Some(root.into()), mirror: false, reserve: 0 }).unwrap();
        let t = Instant::now();
        let b = build(&data, &sources(&data.catalog()), None).unwrap();
        let p = b.places;
        eprintln!("{} places, {} words, {} MB, {:.1} s; passed over: {} packs, {} tiles", p.len(), p.words.len(), p.bytes() >> 20, t.elapsed().as_secs_f64(), b.unread, b.bad);
        let mut by: std::collections::HashMap<(u8, u8), usize> = std::collections::HashMap::new();
        for q in p.places.as_slice() {
            *by.entry((q.kind, q.class)).or_default() += 1;
        }
        let mut by: Vec<_> = by.into_iter().collect();
        by.sort_by(|a, b| b.1.cmp(&a.1));
        eprintln!("{}", by.iter().take(25).map(|((k, c), n)| format!("{} {} {n}", KINDS[*k as usize], p.classes[*c as usize])).collect::<Vec<_>>().join(", "));
        for q in ["banff", "lake louise", "paris", "tokyo", "san fran", "st john"] {
            let t = Instant::now();
            let hits = p.search(q, Some((-100.0, 45.0)), 5);
            eprintln!("{q}: {:.1} ms: {}", t.elapsed().as_secs_f64() * 1000.0, hits.iter().map(|h| { let f = p.place(h); format!("{} ({} {}, {:.2} {:.2})", f.main, f.kind, f.class, f.lon, f.lat) }).collect::<Vec<_>>().join("; "));
        }
    }
}
