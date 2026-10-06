//! Place search (docs/plan.md §4, The map): the map's own place names, from its labels layer
//! (dem/labels.py): every label in the tiles of the map's areas (its z6 tiles with roads), and the
//! labels that show by zoom 8 worldwide (cities and towns, seas, states, big lakes and parks).
//! Found by a word of their name, or of their own English, as typed: accents, case and punctuation
//! aside, the words in order. Made on this Mac the first time a search asks after a new catalog
//! (`Index`), in the background: nothing more for the build to make or the mirror to copy.

use crate::data::Data;
use crate::S;
use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rayon::prelude::*;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use unicode_normalization::UnicodeNormalization;

/// The labels' kinds (dem/labels.py `k`).
const KINDS: [&str; 4] = ["place", "state", "water", "park"];
/// Labels of a kind and a name nearer than this are one place (a label at zoom 8 and at 12, in two
/// tiles' margins).
const SAME_KM: f64 = 3.0;

/// Where a word may start for searching: after a space, and at every ideograph, kana or hangul
/// (names written without spaces: 横浜 found by 浜 too).
fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff | 0x20000..=0x2fa1f)
}

/// A name as searched: lower case, accents and other marks gone, a few letters folded (ß ss, æ ae,
/// ø o, …), apostrophes dropped, any other punctuation a single space between words.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.nfkd() {
        if unicode_normalization::char::is_combining_mark(c) {
            continue;
        }
        if matches!(c, '\'' | '’' | 'ʻ' | 'ʼ' | '`' | '´') {
            continue;
        }
        if !c.is_alphanumeric() {
            gap = !out.is_empty();
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
                l => out.push(l),
            }
        }
    }
    out
}

/// The byte offsets in `folded` where its words start.
fn word_starts(folded: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    for (i, c) in folded.char_indices() {
        let start = match prev {
            None => true,
            Some(p) => p == ' ' || p == SEP || (c != ' ' && c != SEP && cjk(c)),
        };
        if start && c != ' ' && c != SEP {
            out.push(i);
        }
        prev = Some(c);
    }
    out
}

/// Between a place's name and its English, in `text` and `folded` (no word runs across it).
const SEP: char = '\u{1}';

#[derive(Clone, Copy, Debug)]
struct Place {
    lon: f32,
    lat: f32,
    /// Its importance among all kinds (`rank`).
    score: f32,
    /// The zoom to show it at.
    zoom: f32,
    kind: u8,
    class: u8,
    /// Its name (and, after SEP, its own English) in `text`, and folded in `folded`.
    text: (u32, u32),
    folded: (u32, u32),
}

/// The map's places, searchable.
#[derive(Default)]
pub struct Places {
    places: Vec<Place>,
    classes: Vec<String>,
    text: String,
    folded: String,
    /// Each word's start: (place, offset in its folded names), sorted by the folded names from there.
    words: Vec<(u32, u32)>,
}

/// A label as its tile has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub name: String,
    pub en: Option<String>,
    pub kind: String,
    pub class: String,
    /// dem/labels.py's importance (its kind's own scale), the zoom it shows from at the default
    /// spacing, and an area's zoom it spans 20 px from.
    pub s: f64,
    pub mz: f64,
    pub ms: Option<f64>,
    pub lon: f64,
    pub lat: f64,
}

/// One importance for every kind, as a search ranks them: a place's own (10 a locality to 79 a
/// big capital), a state above the towns, a sea above all; a lake or a park by its area, as a
/// village at 1 km² and a city at 10,000.
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
    let z = match (l.kind.as_str(), l.class.as_str()) {
        ("place", "city") => 11.0,
        ("place", "town") => 12.5,
        ("place", "village" | "suburb") => 13.5,
        ("place", _) => 14.5,
        ("state", _) => 6.5,
        _ => l.ms.map_or(l.mz + 2.0, |ms| ms + 4.0),
    };
    z.clamp(4.0, 15.0)
}

/// The labels of one tile of the labels layer (gzip'd or not).
pub fn labels_of(tile: &[u8], z: u8, x: u32, y: u32) -> Result<Vec<Label>> {
    let raw = names::mvt::gunzip_if_gzip(tile)?;
    let t = names::mvt::Tile::decode(&raw)?;
    let mut out = Vec::new();
    for layer in t.layers.iter().filter(|l| l.name == "l") {
        let key = |k: &str| layer.keys.iter().position(|x| x == k).map(|i| i as u32);
        let (kn, ken, kk, kc, ks, kmz, kms) = (key("n"), key("en"), key("k"), key("c"), key("s"), key("mz"), key("ms"));
        for f in &layer.features {
            let get = |k: Option<u32>| -> Option<&names::mvt::Value> {
                let k = k?;
                f.tags.chunks_exact(2).find(|p| p[0] == k).and_then(|p| layer.values.get(p[1] as usize))
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
            let text = |k: Option<u32>| get(k).and_then(|v| v.as_str()).map(str::to_string);
            let (Some(name), Some((px, py))) = (text(kn), f.first_point()) else { continue };
            let (lon, lat) = names::mvt::tile_to_lonlat(z as u32, x, y, layer.extent, px as f64, py as f64);
            out.push(Label {
                name,
                en: text(ken).filter(|e| !e.is_empty()),
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
    Ok(out)
}

impl Places {
    /// The places among `labels` (a label in two tiles, or at two zooms, counted once: one of a
    /// kind and a name within SAME_KM of another; isolated dwellings, a farm's name at most, left
    /// out).
    pub fn from_labels(labels: impl IntoIterator<Item = Label>) -> Places {
        let mut p = Places::default();
        let mut seen: HashMap<(String, String), Vec<(f64, f64)>> = HashMap::new();
        let mut keys: Vec<String> = Vec::new();
        for l in labels {
            if l.class == "isolated_dwelling" || !KINDS.contains(&l.kind.as_str()) {
                continue;
            }
            let at = seen.entry((l.kind.clone(), l.name.clone())).or_default();
            if at.iter().any(|&(lon, lat)| km(lon, lat, l.lon, l.lat) < SAME_KM) {
                continue;
            }
            at.push((l.lon, l.lat));
            let class = match p.classes.iter().position(|c| *c == l.class) {
                Some(i) => i,
                None if p.classes.len() < 255 => {
                    p.classes.push(l.class.clone());
                    p.classes.len() - 1
                }
                None => continue,
            };
            let start = p.text.len();
            p.text.push_str(&l.name);
            let mut key = fold(&l.name);
            if let Some(en) = l.en.as_ref().filter(|e| fold(e) != key) {
                p.text.push(SEP);
                p.text.push_str(en);
                key.push(SEP);
                key.push_str(&fold(en));
            }
            let fstart = p.folded.len();
            p.folded.push_str(&key);
            p.places.push(Place {
                lon: l.lon as f32,
                lat: l.lat as f32,
                score: rank(&l.kind, &l.class, l.s) as f32,
                zoom: show_zoom(&l) as f32,
                kind: KINDS.iter().position(|k| *k == l.kind).unwrap_or(0) as u8,
                class: class as u8,
                text: (start as u32, (p.text.len() - start) as u32),
                folded: (fstart as u32, key.len() as u32),
            });
            keys.push(key);
        }
        let mut words: Vec<(u32, u32)> = keys.iter().enumerate().flat_map(|(i, k)| word_starts(k).into_iter().map(move |o| (i as u32, o as u32))).collect();
        let suffix = |w: &(u32, u32)| &keys[w.0 as usize][w.1 as usize..];
        words.par_sort_unstable_by(|a, b| suffix(a).cmp(suffix(b)).then(a.cmp(b)));
        p.words = words;
        p
    }

    pub fn len(&self) -> usize {
        self.places.len()
    }

    /// Bytes held, about.
    pub fn bytes(&self) -> usize {
        self.places.len() * std::mem::size_of::<Place>() + self.text.len() + self.folded.len() + self.words.len() * 8
    }

    fn folded_of(&self, i: usize) -> &str {
        let (a, n) = self.places[i].folded;
        &self.folded[a as usize..(a + n) as usize]
    }

    /// The `n` places best found by `q`: those with a word starting so (whole words before its
    /// last), a name starting so first, one that's all of it before those; then the most
    /// important, and the nearest to `near` (lon, lat) among like ones.
    pub fn search(&self, q: &str, near: Option<(f64, f64)>, n: usize) -> Vec<Hit> {
        let q = fold(q);
        if q.is_empty() || n == 0 {
            return Vec::new();
        }
        let suffix = |w: &(u32, u32)| &self.folded_of(w.0 as usize)[w.1 as usize..];
        let lo = self.words.partition_point(|w| suffix(w) < q.as_str());
        let hi = lo + self.words[lo..].partition_point(|w| suffix(w).starts_with(q.as_str()));
        // Each place's best match: the whole name (3), its start (2), a word's (1).
        let mut tier: HashMap<u32, u8> = HashMap::new();
        for w in &self.words[lo..hi] {
            let f = self.folded_of(w.0 as usize);
            let at = w.1 as usize;
            let (name, en) = match f.split_once(SEP) {
                Some((a, b)) => (a, Some(b)),
                None => (f, None),
            };
            // (The start of its name, or of its English.)
            let start = at == 0 || f[..at].ends_with(SEP);
            let t = if (at == 0 && name == q) || (at > 0 && start && en == Some(q.as_str())) { 3 } else if start { 2 } else { 1 };
            let e = tier.entry(w.0).or_insert(0);
            *e = (*e).max(t);
        }
        // (A heap of the n best so far: the least on top.)
        let mut best: BinaryHeap<std::cmp::Reverse<Hit>> = BinaryHeap::new();
        for (&i, &t) in &tier {
            let p = &self.places[i as usize];
            let mut r = t as f64 * 100.0 + p.score as f64;
            if let Some((lon, lat)) = near {
                let d = km(lon, lat, p.lon as f64, p.lat as f64);
                r += (12.0 - 3.0 * (1.0 + d).log10()).max(0.0);
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

    /// A place found: its name and own English, kind and class, where, and the zoom to show it at.
    pub fn place(&self, h: &Hit) -> Found<'_> {
        let p = &self.places[h.place as usize];
        let t = &self.text[p.text.0 as usize..(p.text.0 + p.text.1) as usize];
        let (name, en) = match t.split_once(SEP) {
            Some((a, b)) => (a, Some(b)),
            None => (t, None),
        };
        Found { name, en, kind: KINDS[p.kind as usize], class: &self.classes[p.class as usize], lon: p.lon as f64, lat: p.lat as f64, zoom: p.zoom as f64 }
    }
}

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

pub struct Found<'a> {
    pub name: &'a str,
    pub en: Option<&'a str>,
    pub kind: &'a str,
    pub class: &'a str,
    pub lon: f64,
    pub lat: f64,
    pub zoom: f64,
}

/// The places of the catalog being served: made in the background when a search first asks for
/// them (and again for a new catalog, the last made searched meanwhile), and let go once no search
/// has asked for IDLE (they take a few hundred MB); one that fails to be made (the NAS away) is
/// tried again a minute later at the soonest.
#[derive(Default)]
pub struct Index {
    made: RwLock<Option<(u64, Arc<Places>)>>,
    making: AtomicBool,
    failed: Mutex<Option<std::time::Instant>>,
    used: Mutex<Option<std::time::Instant>>,
}

/// How long the places are kept without a search.
const IDLE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

impl Index {
    /// The places to search now (None: none made yet), their making started when they aren't of
    /// catalog generation `g`.
    pub fn get(self: &Arc<Self>, data: &Arc<Data>, g: u64) -> Option<Arc<Places>> {
        *self.used.lock().unwrap() = Some(std::time::Instant::now());
        let cur = self.made.read().unwrap().clone();
        let stale = cur.as_ref().is_none_or(|(made, _)| *made != g);
        let resting = self.failed.lock().unwrap().is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60));
        if stale && !resting && !self.making.swap(true, Ordering::SeqCst) {
            let (me, data) = (self.clone(), data.clone());
            let spawned = std::thread::Builder::new().name("places".into()).spawn(move || {
                let t = std::time::Instant::now();
                match build(&data) {
                    Ok(p) => {
                        eprintln!("places: {} of catalog {} ({} MB) in {:.1} s", p.len(), data.catalog().n, p.bytes() >> 20, t.elapsed().as_secs_f64());
                        *me.made.write().unwrap() = Some((g, Arc::new(p)));
                        *me.failed.lock().unwrap() = None;
                    }
                    Err(e) => {
                        eprintln!("places: {e:#}");
                        *me.failed.lock().unwrap() = Some(std::time::Instant::now());
                        me.making.store(false, Ordering::SeqCst);
                        return;
                    }
                }
                me.making.store(false, Ordering::SeqCst);
                // Kept while they're searched; let go IDLE after the last (unless others, of a
                // newer catalog, took their place: their own thread keeps those).
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    if me.made.read().unwrap().as_ref().is_none_or(|(made, _)| *made != g) {
                        return;
                    }
                    if me.used.lock().unwrap().is_none_or(|t| t.elapsed() >= IDLE) {
                        *me.made.write().unwrap() = None;
                        eprintln!("places: let go, unsearched for {} min", IDLE.as_secs() / 60);
                        return;
                    }
                }
            });
            if spawned.is_err() {
                self.making.store(false, Ordering::SeqCst);
            }
        }
        cur.map(|(_, p)| p)
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
/// each its name and own English, kind and class, where and the zoom to show it at; `ready` false
/// while the map's places are first made (an empty `q` asks for that alone: the search box,
/// opened, has them made while its words are typed).
pub async fn search(State(s): State<S>, Query(q): Query<SearchQ>) -> Response {
    let s2 = s.clone();
    let found = tokio::task::spawn_blocking(move || {
        let near = q.near.as_deref().and_then(|v| v.split_once(',')).and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))).filter(|(lon, lat)| lon.is_finite() && lat.is_finite());
        let Some(p) = s2.places.get(&s2.data, s2.data.generation.load(Ordering::Relaxed)) else { return serde_json::json!({ "ready": false, "hits": [] }) };
        let hits: Vec<serde_json::Value> = p
            .search(&q.q, near, q.n.unwrap_or(8).min(20))
            .iter()
            .map(|h| {
                let f = p.place(h);
                serde_json::json!({ "name": f.name, "en": f.en, "kind": f.kind, "class": f.class, "lon": f.lon, "lat": f.lat, "zoom": f.zoom })
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

/// The map's places from the catalog's labels layer: each lo pack's zoom-8 tiles (what shows by
/// zoom 8, worldwide), and the zoom-12 tiles (every label) of the hi packs of the map's areas.
pub fn build(data: &Data) -> Result<Places> {
    let cat = data.catalog();
    let Some(layer) = cat.layers.get("labels") else { return Ok(Places::default()) };
    let mut packs: Vec<(String, u8)> = layer.lo.values().map(|l| (l.clone(), 8u8)).collect();
    packs.extend(layer.hi.iter().filter(|(t, _)| cat.hidata.contains_key(*t)).map(|(_, l)| (l.clone(), 12u8)));
    let tiles: Vec<(u8, u32, u32)> = packs
        .par_iter()
        .map(|(l, z)| data.pack_tiles(l, *z).map(|ts| ts.into_iter().map(|(x, y)| (*z, x, y)).collect::<Vec<_>>()))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let labels: Vec<Vec<Label>> = tiles
        .par_iter()
        .map(|&(z, x, y)| -> Result<Vec<Label>> {
            match data.tile("labels", z, x, y)? {
                Some((b, _)) => labels_of(b.bytes(), z, x, y),
                None => Ok(Vec::new()),
            }
        })
        .collect::<Result<_>>()?;
    Ok(Places::from_labels(labels.into_iter().flatten()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(name: &str, en: Option<&str>, kind: &str, class: &str, s: f64, lon: f64, lat: f64) -> Label {
        Label { name: name.into(), en: en.map(str::to_string), kind: kind.into(), class: class.into(), s, mz: 10.0, ms: None, lon, lat }
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
        assert_eq!(word_starts("lake louise"), [0, 5]);
        // Every ideograph starts a word; after the separator, the English's first word.
        assert_eq!(word_starts("横浜\u{1}yokohama"), [0, 3, 7]);
    }

    #[test]
    fn a_search_finds_by_any_word_the_whole_name_first_then_the_most_important_and_near() {
        let p = Places::from_labels([
            label("Lake Louise", None, "place", "hamlet", 33.0, -116.18, 51.43),
            label("Louiseville", None, "place", "town", 63.0, -72.94, 46.26),
            label("Louise", None, "place", "village", 52.0, -95.0, 45.0),
            label("Louise", None, "place", "village", 52.0, -122.0, 49.0),
            label("Lake Louise", None, "water", "lake", 4.0, -116.24, 51.41),
            label("横浜", Some("Yokohama"), "place", "city", 76.0, 139.64, 35.44),
            label("Somewhere", None, "place", "isolated_dwelling", 0.0, 1.0, 1.0),
        ]);
        assert_eq!(p.len(), 6, "the isolated dwelling left out");
        let names = |q: &str, near: Option<(f64, f64)>| p.search(q, near, 10).iter().map(|h| { let f = p.place(h); format!("{} {}", f.name, f.class) }).collect::<Vec<_>>();
        // The whole name first (the two villages, the nearer first), then names that start so,
        // then a word inside one.
        assert_eq!(names("louise", Some((-120.0, 49.5))), ["Louise village", "Louise village", "Louiseville town", "Lake Louise hamlet", "Lake Louise lake"]);
        assert_eq!(p.place(&p.search("louise", Some((-120.0, 49.5)), 1)[0]).lon, -122.0);
        // Words in order, as typed; the English, and inside a name without spaces.
        assert_eq!(names("lake lou", None), ["Lake Louise hamlet", "Lake Louise lake"]);
        assert!(names("louise lake", None).is_empty());
        assert_eq!(names("yoko", None), ["横浜 city"]);
        assert_eq!(names("浜", None), ["横浜 city"]);
        assert!(p.search("", None, 10).is_empty());
        let f = p.place(&p.search("yokohama", None, 1)[0]);
        assert_eq!((f.name, f.en, f.kind, f.zoom), ("横浜", Some("Yokohama"), "place", 11.0));
        // At most n, the best.
        assert_eq!(names("l", None).len(), 5);
        assert_eq!(p.search("l", None, 2).len(), 2);
    }

    #[test]
    fn a_label_in_two_tiles_is_one_place() {
        let l = label("Banff", None, "place", "town", 62.0, -115.57, 51.18);
        let p = Places::from_labels([l.clone(), l.clone(), Label { lon: -115.570_01, ..l.clone() }, Label { lat: 51.183, ..l.clone() }]);
        assert_eq!(p.len(), 1);
        // Another of its name and kind farther off is another place.
        assert_eq!(Places::from_labels([l.clone(), Label { lon: -2.52, lat: 57.66, ..l }]).len(), 2);
    }

    /// A zoom-`z` labels tile of `labels` (name, English, kind, class, importance, where in the
    /// tile), as dem/labels.py writes them.
    fn labels_tile(labels: &[(&str, Option<&str>, &str, &str, f64, (i32, i32))]) -> Vec<u8> {
        use names::mvt::Value;
        let keys: Vec<String> = ["n", "en", "k", "c", "s"].iter().map(|k| k.to_string()).collect();
        let (mut values, mut features) = (Vec::new(), Vec::new());
        for (name, en, kind, class, imp, (px, py)) in labels {
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
            features.push(names::mvt::Feature { tags, geom_type: Some(1), geometry: vec![9, zz(*px), zz(*py)], ..Default::default() });
        }
        let layer = names::mvt::Layer { name: "l".into(), version: 2, extent: 4096, keys, values, features, unknown: Vec::new() };
        names::mvt::Tile { layers: vec![layer], unknown: Vec::new() }.encode()
    }

    /// A NAS folder whose catalog's labels are a lo pack (3/4/2) with a zoom-8 tile of two labels,
    /// and a hi pack of one of the map's areas (6/32/21, its hidata named) with a zoom-12 tile of one.
    fn nas_with_labels() -> tempfile::TempDir {
        let nas = tempfile::tempdir().unwrap();
        let mut cat = store::catalog::Catalog::new(1);
        let mut layer = store::catalog::Layer::default();
        for (key, z, x, y, tile) in [
            ("3/4/2", 8u8, 130u32, 70u32, labels_tile(&[("Banff", None, "place", "town", 62.0, (100, 200)), ("横浜", Some("Yokohama"), "place", "city", 76.0, (900, 900))])),
            ("6/32/21", 12, 2050, 1350, labels_tile(&[("Lake Louise", None, "water", "lake", 5.0, (2048, 2048))])),
        ] {
            let logical = format!("layers/labels/{}", key.replace('/', "-"));
            let content = format!("{logical}.0123456789abcdef.pack");
            let path = nas.path().join(&content);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut w = store::pack::PackWriter::create(&path, serde_json::json!({ "layer": "labels" }), false).unwrap();
            w.add(z, x, y, &tile, tile.len() as u32).unwrap();
            w.finish().unwrap();
            if z == 8 {
                layer.lo.insert(key.into(), logical.clone());
            } else {
                layer.hi.insert(key.into(), logical.clone());
                cat.hidata.insert(key.into(), "global/hidata/6-32-21".into());
            }
            cat.files.insert(logical, store::catalog::FileRef { file: content, size: std::fs::metadata(&path).unwrap().len(), ..Default::default() });
        }
        cat.layers.insert("labels".into(), layer);
        store::catalog::write_copy(&nas.path().join("catalog"), &cat).unwrap();
        nas
    }

    #[tokio::test]
    async fn the_search_answers_once_the_maps_places_are_made() {
        let nas = nas_with_labels();
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), nas.path());
        let ask = |q: &str, near: Option<&str>| {
            let (s, q) = (s.clone(), SearchQ { q: q.into(), near: near.map(str::to_string), n: None });
            async move {
                let r = search(State(s), Query(q)).await;
                assert_eq!(r.headers().get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap().to_string()).as_deref(), Some("no-store"));
                let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&b).unwrap()
            }
        };
        // The first ask has them made: not ready until they are.
        assert_eq!(ask("", None).await, serde_json::json!({ "ready": false, "hits": [] }));
        let mut ready = false;
        for _ in 0..200 {
            if ask("", None).await["ready"] == true {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(ready, "the places were never made");
        let v = ask("banff", Some("-115.5,51.2")).await;
        assert_eq!(v["hits"].as_array().map(Vec::len), Some(1));
        assert_eq!((v["hits"][0]["name"].as_str(), v["hits"][0]["class"].as_str(), v["hits"][0]["zoom"].as_f64()), (Some("Banff"), Some("town"), Some(12.5)));
        let y = &ask("yoko", Some("nonsense")).await["hits"][0];
        assert_eq!((y["name"].as_str(), y["en"].as_str()), (Some("横浜"), Some("Yokohama")));
        // The area's own labels too, from its zoom-12 tile; where each is, from its tile.
        let l = &ask("lake lou", None).await["hits"][0];
        let (lon, lat) = names::mvt::tile_to_lonlat(12, 2050, 1350, 4096, 2048.0, 2048.0);
        assert_eq!((l["kind"].as_str(), l["lon"].as_f64(), l["lat"].as_f64()), (Some("water"), Some(lon as f32 as f64), Some(lat as f32 as f64)));
        assert!(ask("nowhere", None).await["hits"].as_array().unwrap().is_empty());
    }

    /// The live build's labels, from the project folder named by SCENIC_PLACES_ROOT: how many
    /// places, how long, how much memory (run by hand).
    #[test]
    #[ignore]
    fn the_builds_places() {
        let Some(root) = std::env::var_os("SCENIC_PLACES_ROOT") else { return };
        let home = tempfile::tempdir().unwrap();
        let data = crate::data::Data::open(crate::data::Options { home: home.path().to_owned(), nas_root: Some(root.into()), mirror: false, reserve_gb: 0 }).unwrap();
        let t = std::time::Instant::now();
        let p = build(&data).unwrap();
        eprintln!("{} places, {} words, {} MB, {:.1} s", p.len(), p.words.len(), p.bytes() >> 20, t.elapsed().as_secs_f64());
        let mut by: HashMap<(u8, u8), usize> = HashMap::new();
        for q in &p.places {
            *by.entry((q.kind, q.class)).or_default() += 1;
        }
        let mut by: Vec<_> = by.into_iter().collect();
        by.sort_by(|a, b| b.1.cmp(&a.1));
        eprintln!("{}", by.iter().take(25).map(|((k, c), n)| format!("{} {} {n}", KINDS[*k as usize], p.classes[*c as usize])).collect::<Vec<_>>().join(", "));
        for q in ["banff", "lake louise", "paris", "tokyo", "san fran", "st john"] {
            let t = std::time::Instant::now();
            let hits = p.search(q, Some((-100.0, 45.0)), 5);
            eprintln!("{q}: {:.1} ms: {}", t.elapsed().as_secs_f64() * 1000.0, hits.iter().map(|h| { let f = p.place(h); format!("{} ({} {}, {:.2} {:.2})", f.name, f.kind, f.class, f.lon, f.lat) }).collect::<Vec<_>>().join("; "));
        }
    }
}
