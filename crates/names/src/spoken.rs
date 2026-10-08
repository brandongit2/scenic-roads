//! The languages spoken where a thing is (docs/plan.md §7): the only place location enters the
//! names. They set the order a name's translations are looked up in, and which to-do list a name
//! goes on.
//!
//! - **Per territory:** CLDR's territory data (`territory-languages.tsv`, made by
//!   `tools/names/cldr-languages.py`): the languages official or de facto official there, most
//!   spoken first.
//! - **Refined** where a subdivision (ISO 3166-2) or a territory reads its names otherwise
//!   ([`REFINED`]): Quebec French then English, Catalonia Catalan then Spanish, Wales English then
//!   Welsh, Brittany French then Breton; Hong Kong and Macau Cantonese, not Mandarin.
//! - **Where:** the pass's outlines of ISO 3166-1 territories and of the refined subdivisions, as a
//!   raster of [`RES`] cells a degree ([`Spoken`]), the smallest outline winning where they overlap
//!   (Hong Kong over China, Quebec over Canada). Cells are about 870 m at the equator, finer than
//!   the outlines' own simplification (1 km for countries).

use anyhow::{bail, ensure, Result};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::LazyLock;

/// Cells per degree, both ways.
pub const RES: u32 = 128;
const COLS: u32 = 360 * RES;
const ROWS: u32 = 180 * RES;

/// Bumped with any change to the table, the refinements or the raster, so versions made under the
/// old ones stop matching.
pub const RULES: &str = "spoken 1";

/// A language: its base subtag (`fr`, `yue`), lower-case ASCII, at most four letters (NUL-padded).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Lang([u8; 4]);

impl Lang {
    /// The language of a tag (`fr`, `zh_Hant`, `zh-Latn-pinyin`, `ja_kana`): its first subtag, when
    /// that is two to four ASCII letters.
    pub fn parse(tag: &str) -> Option<Lang> {
        let base = tag.split(['-', '_']).next()?;
        if !(2..=4).contains(&base.len()) || !base.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        let mut v = [0u8; 4];
        for (i, b) in base.bytes().enumerate() {
            v[i] = b.to_ascii_lowercase();
        }
        Some(Lang(v))
    }

    pub fn as_str(&self) -> &str {
        let n = self.0.iter().position(|b| *b == 0).unwrap_or(4);
        std::str::from_utf8(&self.0[..n]).unwrap_or_default()
    }
}

impl fmt::Debug for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Territories and subdivisions whose languages aren't CLDR's territory list as it is: an ISO 3166
/// code and its languages in lookup order.
pub const REFINED: &[(&str, &[&str])] = &[
    // Canada: Quebec French first, New Brunswick both.
    ("CA-QC", &["fr", "en"]),
    ("CA-NB", &["en", "fr"]),
    // Spain's co-official languages where they are.
    ("ES-CT", &["ca", "es"]),
    ("ES-IB", &["ca", "es"]),
    ("ES-VC", &["es", "ca"]),
    ("ES-GA", &["gl", "es"]),
    ("ES-PV", &["es", "eu"]),
    ("ES-NC", &["es", "eu"]),
    // Britain: Wales Welsh after English; Scotland Gaelic.
    ("GB-WLS", &["en", "cy"]),
    ("GB-SCT", &["en", "gd"]),
    // France: Brittany Breton after French.
    ("FR-BRE", &["fr", "br"]),
    // Hong Kong's and Macau's names are read in Cantonese (the government's romanisation), not
    // Mandarin's Pinyin, which Chinese lines carry.
    ("HK", &["yue", "en"]),
    ("MO", &["yue", "pt"]),
];

/// Where OSM's language tag names a language whose names are read as another spoken there: a name
/// tagged `name:zh` in Hong Kong is read in Cantonese.
const READ_AS: &[(&str, &str)] = &[("zh", "yue")];

/// CLDR's languages per territory, then the refinements.
static TABLE: LazyLock<HashMap<String, Vec<Lang>>> = LazyLock::new(|| {
    let mut m: HashMap<String, Vec<Lang>> = HashMap::new();
    for line in include_str!("territory-languages.tsv").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let Some((code, langs)) = line.split_once('\t') else { continue };
        m.insert(code.to_owned(), langs.split(',').filter_map(Lang::parse).collect());
    }
    for (code, langs) in REFINED {
        m.insert((*code).to_owned(), langs.iter().filter_map(|l| Lang::parse(l)).collect());
    }
    m
});

/// The languages for an ISO 3166-1 code or a refined ISO 3166-2 one (`None` for a subdivision that
/// isn't refined: its country's apply).
pub fn languages_of(code: &str) -> Option<&'static [Lang]> {
    TABLE.get(code).map(Vec::as_slice)
}

/// The order a name's lines are looked up in: the languages OSM gives the name (`osm`, as read where
/// it is: [`READ_AS`]; those spoken there first, in the order they're spoken, then the rest as
/// tagged), then those spoken where it is (`here`), each once.
pub fn lookup_order(osm: &[Lang], here: &[Lang]) -> Vec<Lang> {
    let mut out: Vec<Lang> = Vec::with_capacity(osm.len() + here.len());
    let mapped: Vec<Lang> = osm
        .iter()
        .map(|&l| {
            READ_AS
                .iter()
                .find(|(from, to)| Lang::parse(from) == Some(l) && Lang::parse(to).is_some_and(|t| here.contains(&t)) && !here.contains(&l))
                .and_then(|(_, to)| Lang::parse(to))
                .unwrap_or(l)
        })
        .collect();
    let mut osm: Vec<Lang> = mapped;
    // (Stable: those not spoken here keep their tag order, after.)
    osm.sort_by_key(|l| here.iter().position(|h| h == l).unwrap_or(usize::MAX));
    for l in osm {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    for &l in here {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// An outline to rasterise: its ISO 3166 code, area, and polygons (an outer ring then its holes;
/// lon, lat E7). Rings are closed or not, either way.
pub struct Area {
    pub code: String,
    pub area_km2: f64,
    pub polygons: Vec<Vec<Vec<[i32; 2]>>>,
}

/// Which outlines count: ISO 3166-1 territories, and the refined subdivisions.
pub fn wanted(code: &str) -> bool {
    !code.contains('-') && code.len() == 2 || REFINED.iter().any(|(c, _)| *c == code)
}

/// The languages spoken where, as a raster.
#[derive(Clone, PartialEq, Eq)]
pub struct Spoken {
    /// Region 0 is none; the others by code.
    regions: Vec<(String, Vec<Lang>)>,
    /// Per row (south to north), its runs' start in `runs`; one more at the end.
    row_start: Vec<u32>,
    /// (first column, region) runs, each row's in order, the first at column 0.
    runs: Vec<(u32, u16)>,
    version: u64,
}

impl fmt::Debug for Spoken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spoken").field("regions", &self.regions.len()).field("runs", &self.runs.len()).field("version", &format_args!("{:016x}", self.version)).finish()
    }
}

impl Spoken {
    /// The raster of `areas` ([`wanted`] ones; others are passed over). Where outlines overlap the
    /// smaller wins.
    pub fn build(areas: impl IntoIterator<Item = Area>) -> Spoken {
        let mut areas: Vec<Area> = areas.into_iter().filter(|a| wanted(&a.code) && languages_of(&a.code).is_some_and(|l| !l.is_empty())).collect();
        areas.sort_by(|a, b| a.area_km2.total_cmp(&b.area_km2).then_with(|| a.code.cmp(&b.code)));
        let mut regions: Vec<(String, Vec<Lang>)> = vec![(String::new(), Vec::new())];
        let mut region_of: HashMap<String, u16> = HashMap::new();
        // (row, rank, x) crossings of each area's edges with the rows' centre lines.
        let mut cross: Vec<(u32, u32, f64)> = Vec::new();
        for (rank, a) in areas.iter().enumerate() {
            let id = *region_of.entry(a.code.clone()).or_insert_with(|| {
                regions.push((a.code.clone(), languages_of(&a.code).unwrap_or_default().to_vec()));
                (regions.len() - 1) as u16
            });
            let _ = id;
            for poly in &a.polygons {
                for ring in poly {
                    let n = ring.len();
                    if n < 3 {
                        continue;
                    }
                    for i in 0..n {
                        let p = ring[i];
                        let q = ring[(i + 1) % n];
                        if p == q {
                            continue;
                        }
                        let (y0, y1) = (f64::from(p[1]) * 1e-7, f64::from(q[1]) * 1e-7);
                        let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                        // Rows whose centre c has lo <= c < hi (half-open: a vertex on a centre line
                        // counts once).
                        let first = ((lo + 90.0) * f64::from(RES) - 0.5).ceil().max(0.0) as i64;
                        let last = ((hi + 90.0) * f64::from(RES) - 0.5).ceil() as i64 - 1;
                        for r in first..=last.min(i64::from(ROWS) - 1) {
                            let c = (r as f64 + 0.5) / f64::from(RES) - 90.0;
                            if c < lo || c >= hi {
                                continue;
                            }
                            let (x0, x1) = (f64::from(p[0]) * 1e-7, f64::from(q[0]) * 1e-7);
                            let x = x0 + (x1 - x0) * (c - y0) / (y1 - y0);
                            cross.push((r as u32, rank as u32, x));
                        }
                    }
                }
            }
        }
        cross.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
        let ids: Vec<u16> = areas.iter().map(|a| region_of[&a.code]).collect();
        let mut row = vec![0u16; COLS as usize];
        let mut row_start = Vec::with_capacity(ROWS as usize + 1);
        let mut runs: Vec<(u32, u16)> = Vec::new();
        let mut k = 0;
        for r in 0..ROWS {
            row.fill(0);
            while k < cross.len() && cross[k].0 == r {
                let rank = cross[k].1;
                let mut j = k;
                while j < cross.len() && cross[j].0 == r && cross[j].1 == rank {
                    j += 1;
                }
                let id = ids[rank as usize];
                for pair in cross[k..j].chunks_exact(2) {
                    let col = |x: f64| ((x + 180.0) * f64::from(RES) - 0.5).ceil().clamp(0.0, f64::from(COLS)) as usize;
                    for cell in &mut row[col(pair[0].2)..col(pair[1].2)] {
                        if *cell == 0 {
                            *cell = id;
                        }
                    }
                }
                k = j;
            }
            row_start.push(runs.len() as u32);
            let mut prev = None;
            for (c, &v) in row.iter().enumerate() {
                if prev != Some(v) {
                    runs.push((c as u32, v));
                    prev = Some(v);
                }
            }
        }
        row_start.push(runs.len() as u32);
        let mut s = Spoken { regions, row_start, runs, version: 0 };
        s.version = crate::table::hash(&s.to_bytes());
        s
    }

    /// A hash of the raster and its languages (and the rules), for ETags.
    pub fn version(&self) -> u64 {
        self.version
    }

    fn region_at(&self, lon: f64, lat: f64) -> u16 {
        if !lon.is_finite() || !lat.is_finite() {
            return 0;
        }
        let r = (((lat + 90.0) * f64::from(RES)).floor() as i64).clamp(0, i64::from(ROWS) - 1) as usize;
        let c = (((lon + 180.0) * f64::from(RES)).floor() as i64).clamp(0, i64::from(COLS) - 1) as u32;
        let row = &self.runs[self.row_start[r] as usize..self.row_start[r + 1] as usize];
        let i = row.partition_point(|(first, _)| *first <= c);
        row.get(i.wrapping_sub(1)).map_or(0, |(_, id)| *id)
    }

    /// The ISO code of the territory or refined subdivision at a point ("" where none: the high
    /// seas, Antarctica's unclaimed parts).
    pub fn code_at(&self, lon: f64, lat: f64) -> &str {
        &self.regions[self.region_at(lon, lat) as usize].0
    }

    /// The languages spoken at a point, in lookup order (none at sea beyond any territory).
    pub fn langs_at(&self, lon: f64, lat: f64) -> &[Lang] {
        &self.regions[self.region_at(lon, lat) as usize].1
    }

    /// Every language spoken somewhere in the box (degrees), sorted: what a tile's ETag holds the
    /// versions of. A box over 1,000 rows or 40° (zoomed far out) is given every language the
    /// raster knows.
    pub fn langs_in(&self, west: f64, south: f64, east: f64, north: f64) -> Vec<Lang> {
        let mut ids: BTreeSet<u16> = BTreeSet::new();
        let r0 = (((south + 90.0) * f64::from(RES)).floor() as i64).clamp(0, i64::from(ROWS) - 1) as usize;
        let r1 = (((north + 90.0) * f64::from(RES)).floor() as i64).clamp(0, i64::from(ROWS) - 1) as usize;
        let c0 = (((west + 180.0) * f64::from(RES)).floor() as i64).clamp(0, i64::from(COLS) - 1) as u32;
        let c1 = (((east + 180.0) * f64::from(RES)).floor() as i64).clamp(0, i64::from(COLS) - 1) as u32;
        if r1 < r0 || c1 < c0 || r1 - r0 > 1000 || east - west > 40.0 {
            ids.extend(1..self.regions.len() as u16);
        } else {
            for r in r0..=r1 {
                let row = &self.runs[self.row_start[r] as usize..self.row_start[r + 1] as usize];
                let i = row.partition_point(|(first, _)| *first <= c0).saturating_sub(1);
                for (first, id) in &row[i..] {
                    if *first > c1 {
                        break;
                    }
                    ids.insert(*id);
                }
            }
        }
        let mut out: Vec<Lang> = ids.iter().flat_map(|i| self.regions[*i as usize].1.iter().copied()).collect();
        out.sort();
        out.dedup();
        out
    }

    /// The regions, by code, with their languages (region 0, none, left out).
    pub fn regions(&self) -> impl Iterator<Item = (&str, &[Lang])> {
        self.regions.iter().skip(1).map(|(c, l)| (c.as_str(), l.as_slice()))
    }

    /// The raster as bytes, for keeping between runs ([`Spoken::from_bytes`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.runs.len() * 6 + self.row_start.len() * 4);
        out.extend_from_slice(b"SPOKEN01");
        let head = format!("{RULES}\n{}\n", self.regions.iter().map(|(c, l)| format!("{c}\t{}", l.iter().map(Lang::as_str).collect::<Vec<_>>().join(","))).collect::<Vec<_>>().join("\n"));
        out.extend_from_slice(&(head.len() as u32).to_le_bytes());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&(self.runs.len() as u32).to_le_bytes());
        for s in &self.row_start {
            out.extend_from_slice(&s.to_le_bytes());
        }
        for (c, id) in &self.runs {
            out.extend_from_slice(&c.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
        }
        out
    }

    /// The raster [`Spoken::to_bytes`] wrote, checked (made under other rules: an error).
    pub fn from_bytes(b: &[u8]) -> Result<Spoken> {
        ensure!(b.starts_with(b"SPOKEN01") && b.len() >= 12, "not a spoken-languages raster");
        let u32_at = |p: usize| -> Result<u32> { Ok(u32::from_le_bytes(b.get(p..p + 4).ok_or_else(|| anyhow::anyhow!("cut short"))?.try_into()?)) };
        let hl = u32_at(8)? as usize;
        let head = std::str::from_utf8(b.get(12..12 + hl).ok_or_else(|| anyhow::anyhow!("cut short"))?)?;
        let mut lines = head.lines();
        if lines.next() != Some(RULES) {
            bail!("made under other rules");
        }
        let regions: Vec<(String, Vec<Lang>)> = lines.map(|l| {
            let (c, ls) = l.split_once('\t').unwrap_or((l, ""));
            (c.to_owned(), ls.split(',').filter_map(Lang::parse).collect())
        }).collect();
        // Region 0's line is empty and `lines` skips no empty line but the last: put it back.
        let regions = if regions.first().is_some_and(|r| r.0.is_empty()) { regions } else { std::iter::once((String::new(), Vec::new())).chain(regions).collect() };
        let mut p = 12 + hl;
        let nruns = u32_at(p)? as usize;
        p += 4;
        let mut row_start = Vec::with_capacity(ROWS as usize + 1);
        for _ in 0..=ROWS {
            row_start.push(u32_at(p)?);
            p += 4;
        }
        ensure!(b.len() == p + nruns * 6, "the wrong length");
        let mut runs = Vec::with_capacity(nruns);
        for i in 0..nruns {
            let q = p + i * 6;
            runs.push((u32_at(q)?, u16::from_le_bytes([b[q + 4], b[q + 5]])));
        }
        ensure!(row_start.windows(2).all(|w| w[0] <= w[1]) && row_start.last() == Some(&(nruns as u32)), "rows out of order");
        ensure!(runs.iter().all(|(_, id)| (*id as usize) < regions.len()), "a run names no region");
        let mut s = Spoken { regions, row_start, runs, version: 0 };
        s.version = crate::table::hash(&s.to_bytes());
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Lang {
        Lang::parse(s).unwrap()
    }

    fn strs(v: &[Lang]) -> Vec<&str> {
        v.iter().map(Lang::as_str).collect()
    }

    fn rect(code: &str, w: f64, s: f64, e: f64, n: f64) -> Area {
        let p = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        Area { code: code.into(), area_km2: (e - w) * (n - s) * 10_000.0, polygons: vec![vec![vec![p(w, s), p(e, s), p(e, n), p(w, n)]]] }
    }

    #[test]
    fn langs_parse() {
        assert_eq!(l("zh_Hant"), l("zh"));
        assert_eq!(l("zh-Latn-pinyin").as_str(), "zh");
        assert_eq!(l("YUE").as_str(), "yue");
        assert_eq!(Lang::parse("x"), None);
        assert_eq!(Lang::parse("abcde"), None);
        assert_eq!(Lang::parse("1a"), None);
        assert_eq!(format!("{:?}", [l("fr"), l("br")]), "[fr, br]");
    }

    #[test]
    fn table_and_refinements() {
        assert_eq!(strs(languages_of("FR").unwrap()), ["fr"]);
        assert_eq!(strs(languages_of("CA").unwrap()), ["en", "fr"]);
        assert_eq!(strs(languages_of("CA-QC").unwrap()), ["fr", "en"]);
        assert_eq!(strs(languages_of("TW").unwrap()), ["zh", "nan", "hak"]);
        assert_eq!(strs(languages_of("HK").unwrap()), ["yue", "en"]);
        assert_eq!(strs(languages_of("SG").unwrap()), ["en", "zh", "ms", "ta"]);
        assert_eq!(strs(languages_of("JP").unwrap()), ["ja"]);
        assert_eq!(languages_of("FR-IDF"), None);
        assert!(wanted("FR") && wanted("FR-BRE") && !wanted("FR-IDF") && !wanted("FRA"));
    }

    #[test]
    fn lookup_order_puts_osm_first() {
        let hk = languages_of("HK").unwrap();
        assert_eq!(strs(&lookup_order(&[l("zh")], hk)), ["yue", "en"]);
        let tw = languages_of("TW").unwrap();
        assert_eq!(strs(&lookup_order(&[l("zh")], tw)), ["zh", "nan", "hak"]);
        let fr = languages_of("FR-BRE").unwrap();
        assert_eq!(strs(&lookup_order(&[l("br")], fr)), ["br", "fr"]);
        assert_eq!(strs(&lookup_order(&[l("de"), l("fr")], fr)), ["fr", "de", "br"]);
        // OSM's languages in the order they're spoken here, whatever the tags' order.
        assert_eq!(strs(&lookup_order(&[l("br"), l("fr")], fr)), ["fr", "br"]);
        assert_eq!(strs(&lookup_order(&[], &[])), Vec::<&str>::new());
    }

    #[test]
    fn raster_smaller_wins_and_round_trips() {
        let s = Spoken::build([rect("CA", -80.0, 42.0, -60.0, 60.0), rect("CA-QC", -75.0, 45.0, -65.0, 55.0), rect("FR-IDF", 1.0, 48.0, 3.0, 49.0), rect("FR", -5.0, 42.0, 8.0, 51.0)]);
        assert_eq!(s.code_at(-73.57, 45.5), "CA-QC");
        assert_eq!(strs(s.langs_at(-73.57, 45.5)), ["fr", "en"]);
        assert_eq!(s.code_at(-79.4, 43.7), "CA");
        assert_eq!(strs(s.langs_at(-79.4, 43.7)), ["en", "fr"]);
        // A subdivision not refined isn't an area of its own.
        assert_eq!(s.code_at(2.35, 48.86), "FR");
        assert_eq!(s.code_at(-30.0, 40.0), "");
        assert!(s.langs_at(-30.0, 40.0).is_empty());
        assert!(s.langs_at(f64::NAN, 0.0).is_empty());
        // Just inside and outside an edge.
        assert_eq!(s.code_at(-79.99, 50.0), "CA");
        assert_eq!(s.code_at(-80.01, 50.0), "");
        assert_eq!(strs(&s.langs_in(-76.0, 44.0, -74.0, 46.0)), ["en", "fr"]);
        assert_eq!(strs(&s.langs_in(0.0, 45.0, 1.0, 46.0)), ["fr"]);
        assert_eq!(strs(&s.langs_in(-40.0, 45.0, -39.0, 46.0)), Vec::<&str>::new());
        let b = s.to_bytes();
        let t = Spoken::from_bytes(&b).unwrap();
        assert_eq!(t, s);
        assert_eq!(t.version(), s.version());
        assert!(Spoken::from_bytes(&b[..b.len() - 1]).is_err());
    }

    #[test]
    fn holes_and_islands() {
        let p = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        // A territory with a hole (an enclave) and the enclave's own outline.
        let outer = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0), p(0.0, 0.0)];
        let hole = vec![p(4.0, 4.0), p(6.0, 4.0), p(6.0, 6.0), p(4.0, 6.0)];
        let island = vec![p(20.0, 0.0), p(21.0, 0.0), p(21.0, 1.0)];
        let s = Spoken::build([
            Area { code: "IT".into(), area_km2: 1e6, polygons: vec![vec![outer, hole], vec![island]] },
            Area { code: "SM".into(), area_km2: 1e4, polygons: vec![vec![vec![p(4.5, 4.5), p(5.5, 4.5), p(5.5, 5.5), p(4.5, 5.5)]]] },
        ]);
        assert_eq!(s.code_at(1.0, 1.0), "IT");
        assert_eq!(s.code_at(4.2, 4.2), "");
        assert_eq!(s.code_at(5.0, 5.0), "SM");
        assert_eq!(s.code_at(20.9, 0.5), "IT");
        assert_eq!(s.code_at(20.1, 0.5), "");
    }
}
