//! Passenger ferries: lines, their service groups, and how often they sail, for the `ferries` step
//! (crate::ovconv::ferries_job, which makes its blocks from what this writes).
//!
//! Geometry and descriptions come from OpenStreetMap: ways tagged route=ferry and ferry route
//! relations (the step's osmium exports of the pass's ferries set). A *line* is one service in both
//! directions: the relations of a route_master, or the direction variants of the same ref / the
//! same pair of ports, or a named route=ferry way (or run of them) that no route relation uses.
//!
//! Frequencies are never estimated. Each line's sailings come from, in order:
//!   1. a timetable feed (GTFS) the operator publishes: the declared feeds the rail-feeds step
//!      fetched, counted by the ferry counter (crate::gtfs::ferrycount), each from the day it was
//!      fetched, in feed id order (the first feed's count for a line wins);
//!   2. a published timetable looked up by hand: the static files compiled by hand (the accepted
//!      zips of inputs/timetables/gtfs/, docs/inputs.md §6.7.2), their count rows as given
//!      (crate::gtfs::statics) and their timed sailings counted by the ferry counter, each with the
//!      page it came from, in the zips' order (the first's record for a line wins),
//!   3. the line's OSM interval and opening_hours tags, when they give both the headway and the
//!      day's span of service.
//! Lines with none of these stay "unknown".
//!
//! Service groups: urban & commuter (city water buses and commuter boats), short crossings, long-
//! distance & overnight (2 h 30 or more), cable & chain ferries.
//!
//! Outputs (`Written`): ferries.json (GeoJSON: one feature per way, plus terminals),
//! ferry-lines.json (every line's details, for the hover card), and timetables.json (the lines no
//! timetable counts and the timetable routes that match no line, for the timetables' to-do list and
//! report: docs/inputs.md §6.7.3), each as the Python wrote it (crate::pyjson): its numbers' types,
//! its dicts' orders, the order of a Python set where one decided an order (`line_ends`).

use crate::gtfs::ferrycount;
use crate::gtfs::{statics, Date, Num, Zip};
use crate::interest::py_round;
use crate::pyjson::{dumps, loads, Py};
use crate::pytext::{decimal, space, strip, word_char};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const URBAN: i64 = 0;
const CROSSING: i64 = 1;
const LONG: i64 = 2;
const CABLE: i64 = 3;
const GROUP_NAMES: [&str; 4] =
    ["Urban & commuter", "Short crossing", "Long-distance & overnight", "Cable & chain ferry"];

/// Networks and operators of city transit systems (water buses, commuter boats). Lower case,
/// matched as words within network / operator.
const URBAN_NETS: [&str; 56] = [
    "mbta",
    "nyc ferry",
    "ny waterway",
    "staten island",
    "seastreak",
    "london river",
    "thames clipper",
    "uber boat",
    "transtejo",
    "soflusa",
    "mistral",
    "rtm",
    "batobus",
    "star ferry",
    "sun ferry",
    "hong kong & kowloon",
    "hkkf",
    "fortune ferry",
    "coral sea",
    "discovery bay",
    "park island",
    "tsui wah",
    "chuen kee",
    "halifax transit",
    "toronto island",
    "navette fluviale",
    "navibus",
    "tan",
    "semitan",
    "batcub",
    "tbm",
    "yélo",
    "yelo",
    "ctrl",
    "bibus",
    "vaporetto",
    "casco bay",
    "boston harbor city cruises",
    "city cruises",
    "new york city ferry",
    "gosport ferry",
    "nexus",
    "mersey ferries",
    "clyde",
    "bus de mer",
    "navette maritime",
    "tpm",
    "stm",
    "exo",
    "metrobus",
    "rhode island public transit",
    "ripta",
    "bateau-bus",
    "catamaran",
    "metro de",
    "tmb",
];
const CABLE_WORDS: [&str; 8] = [
    "cable ferry",
    "chain ferry",
    "bac à câble",
    "traille",
    "reaction ferry",
    "floating bridge",
    "chain link ferry",
    "transbordador",
];
const ROAD_FERRY: [&str; 12] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "service",
    "track",
    "yes",
    "regular",
    "local",
];
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const MONTH_DAYS: [usize; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// `round(x)`: half to even, an int.
fn round0(x: f64) -> i64 {
    x.round_ties_even() as i64
}

// ---- CPython's set order -----------------------------------------------------------------------

const HASH_BITS: u32 = 61;
const HASH_MODULUS: u64 = (1 << HASH_BITS) - 1;

/// `hash(float)` (CPython's `_Py_HashDouble`, 64-bit), of a finite number.
fn hash_float(v: f64) -> i64 {
    if v.is_infinite() {
        return if v > 0.0 { 314_159 } else { -314_159 };
    }
    let (mut m, mut e) = frexp(v);
    let sign: i64 = if m < 0.0 {
        m = -m;
        -1
    } else {
        1
    };
    let mut x: u64 = 0;
    while m != 0.0 {
        x = ((x << 28) & HASH_MODULUS) | x >> (HASH_BITS - 28);
        m *= 268_435_456.0;
        e -= 28;
        let y = m as u64;
        m -= y as f64;
        x += y;
        if x >= HASH_MODULUS {
            x -= HASH_MODULUS;
        }
    }
    let e = if e >= 0 { e as u32 % HASH_BITS } else { HASH_BITS - 1 - ((-1 - e) as u32 % HASH_BITS) };
    x = ((x << e) & HASH_MODULUS) | x >> (HASH_BITS - e);
    let mut h = (x as i64).wrapping_mul(sign);
    if h == -1 {
        h = -2;
    }
    h
}

/// `hash(int)` of a small int.
fn hash_int(i: i64) -> i64 {
    let m = HASH_MODULUS as i64;
    let mut h = if i >= 0 { i % m } else { -((-i) % m) };
    if h == -1 {
        h = -2;
    }
    h
}

fn frexp(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        let (m, e) = frexp(x * 2f64.powi(54));
        return (m, e - 54);
    }
    (f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52)), exp - 1022)
}

fn hash_num(n: Num) -> i64 {
    match n {
        Num::Int(i) => hash_int(i),
        Num::Float(f) => hash_float(f),
    }
}

/// `hash((a, b))` (CPython's xxHash-based tuple hash, 64-bit).
fn hash_pair(a: Num, b: Num) -> i64 {
    const P1: u64 = 11_400_714_785_074_694_791;
    const P2: u64 = 14_029_467_366_897_019_727;
    const P5: u64 = 2_870_177_450_012_600_261;
    let mut acc = P5;
    for lane in [hash_num(a) as u64, hash_num(b) as u64] {
        acc = acc.wrapping_add(lane.wrapping_mul(P2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(P1);
    }
    acc = acc.wrapping_add(2u64 ^ (P5 ^ 3_527_539));
    if acc == u64::MAX {
        return 1_546_275_796;
    }
    acc as i64
}

/// `list({(x, y), …})`: the distinct pairs in the order CPython's set holds them (its table's
/// slots: open addressing, nine linear probes, then perturbed jumps; grown fourfold when 3/5 full).
fn set_order(items: &[(Num, Num)]) -> Vec<(Num, Num)> {
    const LINEAR_PROBES: usize = 9;
    const PERTURB_SHIFT: u32 = 5;
    let mut table: Vec<Option<(i64, usize)>> = vec![None; 8];
    let mut used = 0usize;
    let eq = |a: &(Num, Num), b: &(Num, Num)| a.0.f() == b.0.f() && a.1.f() == b.1.f();
    let insert_clean = |table: &mut Vec<Option<(i64, usize)>>, hash: i64, k: usize| {
        let mask = table.len() - 1;
        let mut perturb = hash as u64;
        let mut i = (hash as u64 as usize) & mask;
        loop {
            if table[i].is_none() {
                table[i] = Some((hash, k));
                return;
            }
            if i + LINEAR_PROBES <= mask {
                for j in 1..=LINEAR_PROBES {
                    if table[i + j].is_none() {
                        table[i + j] = Some((hash, k));
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb as usize)) & mask;
        }
    };
    for (k, it) in items.iter().enumerate() {
        let hash = hash_pair(it.0, it.1);
        let mask = table.len() - 1;
        let mut perturb = hash as u64;
        let mut i = (hash as u64 as usize) & mask;
        let mut found = None;
        'probe: loop {
            let probes = if i + LINEAR_PROBES <= mask { LINEAR_PROBES } else { 0 };
            for j in 0..=probes {
                match table[i + j] {
                    None => {
                        found = Some(i + j);
                        break 'probe;
                    }
                    Some((h, o)) if h == hash && eq(&items[o], it) => break 'probe,
                    _ => {}
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb as usize)) & mask;
        }
        let Some(slot) = found else { continue };
        table[slot] = Some((hash, k));
        used += 1;
        // (`fill` is `used`: nothing is ever removed.)
        if used * 5 >= mask * 3 {
            let min = if used > 50_000 { used * 2 } else { used * 4 };
            let mut size = 8;
            while size <= min {
                size <<= 1;
            }
            let old = std::mem::replace(&mut table, vec![None; size]);
            for (h, o) in old.into_iter().flatten() {
                insert_clean(&mut table, h, o);
            }
        }
    }
    table.into_iter().flatten().map(|(_, k)| items[k]).collect()
}

// ---- Distances ----------------------------------------------------------------------------------

fn seg_km(coords: &[[f64; 2]]) -> f64 {
    let mut d = 0.0;
    for w in coords.windows(2) {
        let ([x0, y0], [x1, y1]) = (w[0], w[1]);
        let k = ((y0 + y1) / 2.0).to_radians().cos();
        d += ferrycount::hypot((x1 - x0) * k, y1 - y0) * 111.195;
    }
    d
}

fn km2(a: [f64; 2], b: [f64; 2]) -> f64 {
    seg_km(&[a, b])
}

// ---- OSM text -----------------------------------------------------------------------------------

/// OPL's escapes: `%<hex>%` a character.
fn unesc(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '%' {
            let mut j = i + 1;
            while j < c.len() && c[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > i + 1 && j < c.len() && c[j] == '%' {
                let hex: String = c[i + 1..j].iter().collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// A relation of the OPL export.
struct Rel {
    id: i64,
    tags: Vec<(String, String)>,
    members: Vec<(char, i64, String)>,
}

fn tag<'a>(t: &'a [(String, String)], k: &str) -> Option<&'a str> {
    t.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// `t.get(k, "")`.
fn tg<'a>(t: &'a [(String, String)], k: &str) -> &'a str {
    tag(t, k).unwrap_or("")
}

fn set_default(t: &mut Vec<(String, String)>, k: &str, v: &str) {
    if tag(t, k).is_none() {
        t.push((k.to_string(), v.to_string()));
    }
}

fn read_relations(text: &str) -> Result<Vec<Rel>> {
    let mut rels = Vec::new();
    for line in text.split_inclusive('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let parts: Vec<&str> = line.split(' ').collect();
        let id =
            parts[0].get(1..).and_then(|s| s.parse().ok()).with_context(|| format!("a relation's id: {line:?}"))?;
        let mut r = Rel { id, tags: Vec::new(), members: Vec::new() };
        for p in &parts[1..] {
            if let Some(t) = p.strip_prefix('T').filter(|t| !t.is_empty()) {
                for kv in t.split(',') {
                    if let Some((k, v)) = kv.split_once('=') {
                        let (k, v) = (unesc(k), unesc(v));
                        match r.tags.iter_mut().find(|(n, _)| *n == k) {
                            Some(e) => e.1 = v,
                            None => r.tags.push((k, v)),
                        }
                    }
                }
            } else if let Some(ms) = p.strip_prefix('M').filter(|t| !t.is_empty()) {
                for m in ms.split(',') {
                    let mut ch = m.chars();
                    let typ = ch.next().context("a member")?;
                    let rest = ch.as_str();
                    let (rf, role) = rest.split_once('@').unwrap_or((rest, ""));
                    r.members.push((typ, rf.parse().with_context(|| format!("a member's ref: {m:?}"))?, unesc(role)));
                }
            }
        }
        rels.push(r);
    }
    Ok(rels)
}

/// Python's `str.lower()`.
fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn digits_at(c: &[char], i: usize, max: usize) -> usize {
    let mut n = 0;
    while i + n < c.len() && n < max && decimal(c[i + n]).is_some() {
        n += 1;
    }
    n
}

fn int_of(c: &[char]) -> i64 {
    c.iter().fold(0i64, |v, &d| v.saturating_mul(10).saturating_add(decimal(d).unwrap_or(0) as i64))
}

/// OSM duration / interval: HH:MM, H:MM:SS, 90, 1h30, 10min, 48:00.
fn minutes(v: Option<&str>) -> Option<Num> {
    let v = v.filter(|v| !v.is_empty())?;
    let v: String = lower(strip(v)).replace(' ', "");
    let c: Vec<char> = v.chars().collect();
    // (\d+):(\d{1,2})(?::(\d{1,2}))?
    let a = digits_at(&c, 0, usize::MAX);
    if a > 0 && c.get(a) == Some(&':') {
        let b = digits_at(&c, a + 1, 2);
        if b > 0 {
            let e = a + 1 + b;
            let (h, m) = (int_of(&c[..a]), int_of(&c[a + 1..e]));
            // (`int(h) * 60 + int(m) + int(s or 0) / 60`: a float, whatever its value.)
            if e == c.len() {
                return Some(Num::Float((h * 60 + m) as f64));
            }
            if c[e] == ':' {
                let s = digits_at(&c, e + 1, 2);
                if s > 0 && e + 1 + s == c.len() {
                    let sec = int_of(&c[e + 1..]);
                    return Some(Num::Float((h * 60 + m) as f64 + sec as f64 / 60.0));
                }
            }
        }
    }
    // (\d+)h(?:(\d+)(?:m|min)?)?
    if a > 0 && c.get(a) == Some(&'h') {
        let rest = &c[a + 1..];
        let b = digits_at(rest, 0, usize::MAX);
        let tail: String = rest[b..].iter().collect();
        if (b == 0 && rest.is_empty()) || (b > 0 && matches!(tail.as_str(), "" | "m" | "min")) {
            return Some(Num::Int(int_of(&c[..a]) * 60 + int_of(&rest[..b])));
        }
    }
    // (\d+(?:\.\d+)?)(?:m|min|mins|minutes)?
    if a > 0 {
        let mut e = a;
        if c.get(a) == Some(&'.') {
            let f = digits_at(&c, a + 1, usize::MAX);
            if f > 0 {
                e = a + 1 + f;
            }
        }
        let tail: String = c[e..].iter().collect();
        if matches!(tail.as_str(), "" | "m" | "min" | "mins" | "minutes") {
            return crate::pytext::float(&c[..e].iter().collect::<String>()).map(Num::Float);
        }
    }
    None
}

/// `(?:Mo-Su\s+)?(\d{1,2}):(\d{2})-(\d{1,2}):(\d{2})` fullmatched (`lower`: the pattern's letters
/// lower case): the four numbers.
fn span_match(s: &str, prefix: &str) -> Option<[i64; 4]> {
    let c: Vec<char> = s.chars().collect();
    let p: Vec<char> = prefix.chars().collect();
    let mut starts = vec![0];
    if c.len() >= p.len() && c[..p.len()] == p[..] {
        let mut k = p.len();
        while k < c.len() && space(c[k]) {
            k += 1;
        }
        if k > p.len() {
            starts.insert(0, k);
        }
    }
    'start: for i in starts {
        let mut at = i;
        let mut nums = [0i64; 4];
        for (n, (lo, hi, sep)) in
            [(1, 2, Some(':')), (2, 2, Some('-')), (1, 2, Some(':')), (2, 2, None)].into_iter().enumerate()
        {
            let d = digits_at(&c, at, hi);
            if d < lo {
                continue 'start;
            }
            nums[n] = int_of(&c[at..at + d]);
            at += d;
            match sep {
                Some(ch) => {
                    if c.get(at) != Some(&ch) {
                        continue 'start;
                    }
                    at += 1;
                }
                None => {
                    if at != c.len() {
                        continue 'start;
                    }
                }
            }
        }
        return Some(nums);
    }
    None
}

/// Minutes of service per day from opening_hours, only when it is one simple daily span.
fn daily_span(oh: Option<&str>) -> Option<i64> {
    let oh = strip(oh.filter(|o| !o.is_empty())?);
    if oh == "24/7" {
        return Some(24 * 60);
    }
    let m = span_match(oh, "Mo-Su")?;
    let (a, mut b) = (m[0] * 60 + m[1], m[2] * 60 + m[3]);
    if b <= a {
        b += 24 * 60;
    }
    Some(b - a)
}

/// Whether `w` stands at `i` of `c` as a whole word (Python's `\b…\b`).
fn word_at(c: &[char], i: usize, w: &[char]) -> bool {
    i + w.len() <= c.len()
        && c[i..i + w.len()] == w[..]
        && (i == 0 || !word_char(c[i - 1]))
        && c.get(i + w.len()).is_none_or(|&n| !word_char(n))
}

/// (category, text) from OSM tags: 0 unknown, 1 year-round daily, 2 year-round some days, 3 seasonal.
fn season_of(t: &[(String, String)]) -> (i64, String) {
    let s = lower(tag(t, "seasonal").unwrap_or(""));
    let oh = strip(tag(t, "opening_hours").unwrap_or(""));
    let ohl = lower(oh);
    if !s.is_empty() && s != "no" {
        return (3, if s == "yes" { "Seasonal".into() } else { format!("Seasonal ({})", s.replace(';', ", ")) });
    }
    let c: Vec<char> = ohl.chars().collect();
    let month_word = (0..c.len()).any(|i| MONTHS.iter().any(|m| word_at(&c, i, &m.chars().collect::<Vec<_>>())));
    if month_word || ohl.contains("agu") {
        return (3, format!("Seasonal: {oh}"));
    }
    if ohl == "24/7" || span_match(&ohl, "mo-su").is_some() {
        return (1, if ohl == "24/7" { "Daily".into() } else { format!("Daily {}", oh.replace("Mo-Su ", "")) });
    }
    let starts = ["mo-fr", "mo-sa", "sa", "su", "sa-su"].iter().any(|w| word_at(&c, 0, &w.chars().collect::<Vec<_>>()));
    let last = if ohl.contains(';') { ohl.rsplit(';').next().unwrap_or("") } else { "" };
    let weekend = ["sa ", "sa-", "su ", "su-"].iter().any(|w| last.contains(w));
    if starts && !ohl.contains("mo-su") && !weekend {
        return (2, oh.to_string());
    }
    if s == "no" {
        return (if oh.is_empty() { 1 } else { 0 }, "Year-round".into());
    }
    (0, String::new())
}

/// A date of a season text (`_DATE`): its end, and (modifier, day, month, day after).
struct DateM {
    start: usize,
    end: usize,
    modi: Option<&'static str>,
    day1: Option<i64>,
    mon: usize,
    day2: Option<i64>,
}

/// `(?:(early|mid|late)[\s-]*)?(?:(\d{1,2})\s*)?(jan|…|dec)[a-z]*\.?(?:\s+(\d{1,2})(?![\d:]))?`
/// at `i`, as the regex engine tries it.
fn date_at(c: &[char], i: usize) -> Option<DateM> {
    let starts_with = |at: usize, w: &str| {
        let w: Vec<char> = w.chars().collect();
        at + w.len() <= c.len()
            && c[at..at + w.len()].iter().zip(&w).all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
    };
    let rest = |at: usize, modi: Option<&'static str>| -> Option<DateM> {
        // (\d{1,2})\s* then the month, else the month.
        let mut tries = Vec::new();
        let d = digits_at(c, at, 2);
        if d > 0 {
            let mut k = at + d;
            while k < c.len() && space(c[k]) {
                k += 1;
            }
            tries.push((k, Some(int_of(&c[at..at + d]))));
        }
        tries.push((at, None));
        for (m_at, day1) in tries {
            let Some(mon) = (0..12).find(|&m| starts_with(m_at, MONTHS[m])) else { continue };
            let mut e = m_at + 3;
            while e < c.len() && c[e].is_ascii_lowercase() || e < c.len() && c[e].is_ascii_uppercase() {
                e += 1;
            }
            if c.get(e) == Some(&'.') {
                e += 1;
            }
            let mut day2 = None;
            let mut k = e;
            while k < c.len() && space(c[k]) {
                k += 1;
            }
            if k > e {
                let n = digits_at(c, k, 2);
                // (Two digits, or one, not followed by a digit or a colon.)
                for take in (1..=n).rev() {
                    let next = c.get(k + take);
                    if next.is_none_or(|&ch| decimal(ch).is_none() && ch != ':') {
                        day2 = Some(int_of(&c[k..k + take]));
                        e = k + take;
                        break;
                    }
                }
            }
            return Some(DateM { start: i, end: e, modi, day1, mon, day2 });
        }
        None
    };
    for m in ["early", "mid", "late"] {
        if starts_with(i, m) {
            let mut k = i + m.len();
            while k < c.len() && (space(c[k]) || c[k] == '-') {
                k += 1;
            }
            if let Some(d) = rest(k, Some(m)) {
                return Some(d);
            }
        }
    }
    rest(i, None)
}

fn dates_in(c: &[char]) -> Vec<DateM> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match date_at(c, i) {
            Some(d) => {
                i = d.end.max(i + 1);
                out.push(d);
            }
            None => i += 1,
        }
    }
    out
}

/// `\s*(\d{4})?\s*(-|–|—|to|au|a|até)\s*`, fullmatched.
fn range_sep(c: &[char]) -> bool {
    let mut i = 0;
    while i < c.len() && space(c[i]) {
        i += 1;
    }
    if digits_at(c, i, 4) == 4 {
        i += 4;
    }
    while i < c.len() && space(c[i]) {
        i += 1;
    }
    let mut j = c.len();
    while j > i && space(c[j - 1]) {
        j -= 1;
    }
    let core: String = c[i..j].iter().collect();
    matches!(core.as_str(), "-" | "–" | "—" | "to" | "au" | "a" | "até")
}

/// Months a year a line runs, from a season text: "Apr-Oct", "mid-Jun–15 Sep", "late May-early
/// Sep", "4 Jul-30 Aug daily; 3 Apr-2 Jul & 5 Sep-1 Nov weekends" (ranges united, wrapping over
/// the new year), "year-round" / "all year" / reduced winter service = 12. None if no months.
fn months_of_text(text: &str) -> Option<f64> {
    let t = lower(text);
    if t.is_empty() {
        return None;
    }
    if [
        "year-round",
        "year round",
        "all year",
        "toute l'année",
        "todo el año",
        "reduced",
        "winter timetable",
        "winter service",
    ]
    .iter()
    .any(|w| t.contains(w))
    {
        return Some(12.0);
    }
    let mut days = [false; 365];
    let mstart: Vec<usize> = (0..12).map(|i| MONTH_DAYS[..i].iter().sum()).collect();
    let doy = |m: &DateM, end: bool| -> usize {
        let mon = m.mon;
        let d = match (m.day1.or(m.day2), m.modi) {
            (Some(day), _) => (day - 1).max(-1),
            (None, Some(md)) => match md {
                "early" => 4,
                "mid" => 14,
                _ => 24,
            },
            (None, None) => {
                if end {
                    MONTH_DAYS[mon] as i64 - 1
                } else {
                    0
                }
            }
        };
        let d = d.min(MONTH_DAYS[mon] as i64 - 1);
        // (Day 0 written: -1, Python's index from the end.)
        ((mstart[mon] as i64 + d).rem_euclid(365)) as usize
    };
    let mut found = false;
    // Ranges "A–B" (hyphen, en dash, "to"), or single months.
    let tc: Vec<char> = t.chars().collect();
    let mut parts: Vec<Vec<char>> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < tc.len() {
        if matches!(tc[i], ';' | ',' | '&' | '(' | ')' | '+') {
            parts.push(std::mem::take(&mut cur));
            i += 1;
        } else if tc[i..].starts_with(&[' ', 'a', 'n', 'd', ' ']) {
            parts.push(std::mem::take(&mut cur));
            i += 5;
        } else {
            cur.push(tc[i]);
            i += 1;
        }
    }
    parts.push(cur);
    for part in &parts {
        let ms = dates_in(part);
        let mut i = 0;
        while i < ms.len() {
            let a = &ms[i];
            let b = (i + 1 < ms.len() && range_sep(&part[a.end..ms[i + 1].start])).then(|| &ms[i + 1]);
            let (x, y) = match b {
                Some(b) => {
                    i += 2;
                    (doy(a, false), doy(b, true))
                }
                None => {
                    i += 1;
                    (doy(a, false), doy(a, true))
                }
            };
            found = true;
            let mut k = x;
            loop {
                days[k] = true;
                if k == y {
                    break;
                }
                k = (k + 1) % 365;
            }
        }
    }
    if !found {
        return None;
    }
    let n = days.iter().filter(|d| **d).count() as f64;
    Some(0.5f64.max(round0(n / (365.0 / 12.0) * 2.0) as f64 / 2.0))
}

fn carries_vehicles(t: &[(String, String)]) -> bool {
    if ["motorcar", "motor_vehicle", "vehicle", "hgv"]
        .iter()
        .any(|k| matches!(tag(t, k), Some("yes" | "designated" | "permissive")))
    {
        return true;
    }
    ROAD_FERRY.contains(&tg(t, "ferry")) && tag(t, "motor_vehicle") != Some("no") && tag(t, "motorcar") != Some("no")
}

fn closed(t: &[(String, String)]) -> bool {
    matches!(tag(t, "access"), Some("private" | "no"))
        && !matches!(tag(t, "foot"), Some("yes" | "designated" | "permissive"))
}

fn is_cable(t: &[(String, String)]) -> bool {
    let text = lower(&["name", "name:en", "description", "note"].map(|k| tg(t, k)).join(" "));
    matches!(tag(t, "ferry"), Some("cable" | "chain" | "chain_tray"))
        || tag(t, "ferry:cable") == Some("yes")
        || tag(t, "cable") == Some("yes")
        || CABLE_WORDS.iter().any(|w| text.contains(w))
}

fn is_urban(t: &[(String, String)]) -> bool {
    let text: Vec<char> =
        lower(&["network", "operator", "network:short", "operator:short", "brand"].map(|k| tg(t, k)).join(" "))
            .chars()
            .collect();
    URBAN_NETS.iter().any(|w| {
        let w: Vec<char> = strip(w).chars().collect();
        (0..text.len()).any(|i| {
            i + w.len() <= text.len()
                && text[i..i + w.len()] == w[..]
                && (i == 0 || !word_char(text[i - 1]))
                && text.get(i + w.len()).is_none_or(|&n| !word_char(n))
        })
    })
}

fn clean_name(n: &str) -> String {
    // PTv2 direction names: "Ferry X: A => B" / "A → B" → keep the line name part.
    let arrows = ["=>", "→", "->"];
    if !arrows.iter().any(|a| n.contains(a)) {
        return strip(n).to_string();
    }
    let c: Vec<char> = n.chars().collect();
    for (k, &ch) in c.iter().enumerate() {
        if ch != ':' {
            continue;
        }
        let seg: String = c[k + 1..].iter().take_while(|&&x| x != ':').collect();
        if ["=>", "→", "->", "-->", "—>"].iter().any(|a| seg.contains(a)) {
            let mut s = k;
            while s > 0 && space(c[s - 1]) {
                s -= 1;
            }
            return strip(&c[..s].iter().collect::<String>()).to_string();
        }
    }
    strip(n).to_string()
}

/// The two ends of a line made of several ways: the farthest-apart pair of way ends (the ports;
/// with each direction mapped as its own way, the ends aren't the ones used once).
fn line_ends(ways: &[&[[Num; 2]]]) -> [[Num; 2]; 2] {
    let pts: Vec<(Num, Num)> =
        ways.iter().flat_map(|c| [c[0], c[c.len() - 1]]).map(|p| (p[0].round(5), p[1].round(5))).collect();
    let ends = set_order(&pts);
    if ends.len() == 1 {
        let e = [ends[0].0, ends[0].1];
        return [e, e];
    }
    let f = |p: (Num, Num)| [p.0.f(), p.1.f()];
    let (mut best, mut pair) = (-1.0, (ends[0], ends[1]));
    for i in 0..ends.len() {
        for j in i + 1..ends.len() {
            let d = km2(f(ends[i]), f(ends[j]));
            if d > best {
                (best, pair) = (d, (ends[i], ends[j]));
            }
        }
    }
    [[pair.0 .0, pair.0 .1], [pair.1 .0, pair.1 .1]]
}

/// A ferry way: its tags and points.
struct Way {
    tags: Vec<(String, String)>,
    coords: Vec<[Num; 2]>,
    plain: Vec<[f64; 2]>,
}

/// A line: its tags, ways and relations.
struct Line {
    tags: Vec<(String, String)>,
    ways: Vec<i64>,
    rels: Vec<i64>,
}

fn num_pair(v: &Py) -> Option<[Num; 2]> {
    let l = v.list()?;
    Some([Num::of(l.first()?)?, Num::of(l.get(1)?)?])
}

fn ends_py(e: &[[Num; 2]; 2]) -> Py {
    Py::List(e.iter().map(|p| Py::List(vec![p[0].py(), p[1].py()])).collect())
}

/// What the step writes: ferries.json, ferry-lines.json, timetables.json.
pub struct Written {
    pub ferries: String,
    pub lines: String,
    pub timetables: String,
}

/// The ferries from `src` (the exports: ways.geojsonseq, relations.opl, terminals.geojsonseq),
/// counted with the ferry `feeds` (serde objects with `id`, `path`, `fetched` and the facts) and the
/// static files (`statics`: name, path).
pub fn run(src: &Path, feeds: &[serde_json::Value], statics_: &[(String, std::path::PathBuf)]) -> Result<Written> {
    use crate::timings::{phase, Class};
    let p = phase("the ferry ways and relations read", Class::Disk);
    let mut ways: BTreeMap<i64, Way> = BTreeMap::new();
    let mut way_order: Vec<i64> = Vec::new();
    let text = std::fs::read_to_string(src.join("ways.geojsonseq")).context("ways.geojsonseq")?;
    for line in text.split_inclusive('\n') {
        let line = strip(line.trim_start_matches('\x1e'));
        if line.is_empty() {
            continue;
        }
        let f = loads(line)?;
        if f.at("geometry").at("type").as_str() != Some("LineString") {
            continue;
        }
        let props = f.at("properties");
        let id = Num::of(props.at("@id")).context("a way without its @id")?.f() as i64;
        let Py::Dict(pd) = props else { anyhow::bail!("way {id}: no properties") };
        let tags = pd.iter().filter(|(k, _)| !k.starts_with('@')).map(|(k, v)| (k.clone(), v.py_str())).collect();
        let coords: Vec<[Num; 2]> =
            f.at("geometry").at("coordinates").list().context("coordinates")?.iter().filter_map(num_pair).collect();
        let plain = coords.iter().map(|p| [p[0].f(), p[1].f()]).collect();
        if !ways.contains_key(&id) {
            way_order.push(id);
        }
        ways.insert(id, Way { tags, coords, plain });
    }
    let rels = read_relations(&std::fs::read_to_string(src.join("relations.opl")).context("relations.opl")?)?;
    drop(p);
    let p = phase("the lines grouped", Class::Compute);
    let mut routes: Vec<&Rel> = Vec::new();
    let mut route_at: HashMap<i64, usize> = HashMap::new();
    for r in &rels {
        if tag(&r.tags, "type") == Some("route") && tag(&r.tags, "route") == Some("ferry") {
            match route_at.get(&r.id) {
                Some(&i) => routes[i] = r,
                None => {
                    route_at.insert(r.id, routes.len());
                    routes.push(r);
                }
            }
        }
    }
    let mut master_of: HashMap<i64, &Rel> = HashMap::new();
    for m in rels.iter().filter(|r| tag(&r.tags, "type") == Some("route_master")) {
        for (typ, rf, _) in &m.members {
            if *typ == 'r' && route_at.contains_key(rf) {
                master_of.insert(*rf, m);
            }
        }
    }
    // ---- Lines
    let mut groups: crate::gtfs::railcount::Ordered<String, Vec<i64>> = Default::default();
    let mut group_master: Vec<bool> = Vec::new();
    for r in &routes {
        let t = &r.tags;
        if closed(t) {
            continue;
        }
        let who = lower([tg(t, "network"), tg(t, "operator")].into_iter().find(|s| !s.is_empty()).unwrap_or(""));
        let (key, is_m) = if let Some(m) = master_of.get(&r.id) {
            (format!("m\u{0}{}", m.id), true)
        } else if !tg(t, "ref").is_empty() {
            (format!("ref\u{0}{who}\u{0}{}", lower(tg(t, "ref"))), false)
        } else if !tg(t, "from").is_empty() && !tg(t, "to").is_empty() {
            let mut p = [lower(tg(t, "from")), lower(tg(t, "to"))];
            p.sort();
            (format!("ports\u{0}{who}\u{0}{}\u{0}{}", p[0], p[1]), false)
        } else {
            (format!("r\u{0}{}", r.id), false)
        };
        let n = groups.len();
        groups.entry(key, Vec::new).push(r.id);
        if groups.len() > n {
            group_master.push(is_m);
        }
    }
    let rel_by: HashMap<i64, &Rel> = routes.iter().map(|r| (r.id, *r)).collect();
    let mut lines: crate::gtfs::railcount::Ordered<String, (String, Line)> = Default::default();
    let mut way_lines: crate::gtfs::railcount::Ordered<i64, (i64, Vec<String>)> = Default::default();
    for (gi, rids) in groups.items.iter_mut().enumerate() {
        rids.sort();
        let lid = format!("r{}", rids[0]);
        let mut tags: Vec<(String, String)> = Vec::new();
        for rid in rids.iter() {
            // (First relation wins per key, the route master's tags on top.)
            for (k, v) in &rel_by[rid].tags {
                set_default(&mut tags, k, v);
            }
        }
        if group_master[gi] {
            for (k, v) in &master_of[&rids[0]].tags {
                if k != "type" && k != "route_master" {
                    match tags.iter_mut().find(|(n, _)| n == k) {
                        Some(e) => e.1 = v.clone(),
                        None => tags.push((k.clone(), v.clone())),
                    }
                }
            }
        }
        let mut wids: Vec<i64> = Vec::new();
        for rid in rids.iter() {
            for (typ, rf, role) in &rel_by[rid].members {
                if *typ == 'w'
                    && ways.contains_key(rf)
                    && matches!(role.as_str(), "" | "forward" | "backward" | "main" | "route")
                    && !wids.contains(rf)
                {
                    wids.push(*rf);
                }
            }
        }
        if wids.is_empty() {
            continue;
        }
        for w in &wids {
            way_lines.entry(*w, || (*w, Vec::new())).1.push(lid.clone());
        }
        let line = Line { tags, ways: wids, rels: rids.clone() };
        let slot =
            lines.entry(lid.clone(), || (String::new(), Line { tags: Vec::new(), ways: Vec::new(), rels: Vec::new() }));
        *slot = (lid, line);
    }
    // Named ferry ways outside any route relation: one line per (name, operator).
    let mut loose: crate::gtfs::railcount::Ordered<String, Vec<i64>> = Default::default();
    for wid in &way_order {
        let t = &ways[wid].tags;
        if tag(t, "route") != Some("ferry") || way_lines.get(wid).is_some() || closed(t) {
            continue;
        }
        let key = if !tg(t, "name").is_empty() {
            format!("n\u{0}{}\u{0}{}", lower(tg(t, "name")), lower(tg(t, "operator")))
        } else {
            format!("way\u{0}{wid}")
        };
        loose.entry(key, Vec::new).push(*wid);
    }
    for wids in loose.items.iter_mut() {
        wids.sort();
        let lid = format!("w{}", wids[0]);
        let mut tags = ways[&wids[0]].tags.clone();
        for wid in &wids[1..] {
            for (k, v) in &ways[wid].tags {
                set_default(&mut tags, k, v);
            }
        }
        for w in wids.iter() {
            way_lines.entry(*w, || (*w, Vec::new())).1.push(lid.clone());
        }
        let line = Line { tags, ways: wids.clone(), rels: Vec::new() };
        let slot =
            lines.entry(lid.clone(), || (String::new(), Line { tags: Vec::new(), ways: Vec::new(), rels: Vec::new() }));
        *slot = (lid, line);
    }
    drop(p);

    // ---- Frequencies from feeds and timetables
    let ends: Vec<(String, [[Num; 2]; 2])> = lines
        .items
        .iter()
        .map(|(lid, l)| (lid.clone(), line_ends(&l.ways.iter().map(|w| ways[w].coords.as_slice()).collect::<Vec<_>>())))
        .collect();
    let ends_of: HashMap<&str, &[[Num; 2]; 2]> = ends.iter().map(|(l, e)| (l.as_str(), e)).collect();
    let by_ends: Vec<(String, [[f64; 2]; 2])> =
        ends.iter().map(|(l, e)| (l.clone(), [[e[0][0].f(), e[0][1].f()], [e[1][0].f(), e[1][1].f()]])).collect();
    let line_ids: HashSet<&str> = lines.items.iter().map(|(l, _)| l.as_str()).collect();
    let mut counted: Vec<Vec<Py>> = Vec::new();
    let mut unmatched: Vec<Py> = Vec::new();
    {
        let _p = phase("the ferry feeds counted", Class::Mixed);
        let mut feeds: Vec<&serde_json::Value> = feeds.iter().collect();
        feeds.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        for f in feeds {
            let id = f["id"].as_str().unwrap_or("");
            let (Some(path), Some(day)) = (
                f.get("path").and_then(|p| p.as_str()),
                f.get("fetched").and_then(|d| d.as_str()).and_then(Date::parse_iso),
            ) else {
                // (A feed without a zip: the Python's KeyError, which passed it over.)
                eprintln!("  {id}: KeyError: no zip");
                continue;
            };
            let mut found = Vec::new();
            match ferrycount::process(&ferrycount::Feed::of(f), Path::new(path), &by_ends, Some(day), Some(&mut found))
            {
                Ok(recs) => {
                    eprintln!("{id}: {} lines matched", recs.len());
                    unmatched.extend(found);
                    counted.push(recs);
                }
                Err(e) => eprintln!("  {id}: {}: {e}", e.class()),
            }
        }
    }
    {
        let _p = phase("the static files counted", Class::Mixed);
        for (name, path) in statics_ {
            let res = (|| -> crate::gtfs::R<(Vec<Py>, Vec<Py>)> {
                let mut found: Vec<Py> = Vec::new();
                let z = Zip::open(path)?;
                let mut recs = statics::ferry_counts(&z)?;
                // Count rows whose pin names no line of this pass.
                let rs: HashMap<String, crate::gtfs::OwnedRow> = statics::routes(&z)?.into_iter().collect();
                let mut seen: HashSet<Option<String>> = HashSet::new();
                for c in statics::count_rows(&z)? {
                    let rid = match c.cell("route_id") {
                        crate::gtfs::Cell::Absent => Some(String::new()),
                        crate::gtfs::Cell::None => None,
                        crate::gtfs::Cell::Str(s) => Some(s.to_string()),
                    };
                    if !seen.insert(rid.clone()) {
                        continue;
                    }
                    let Some(r) = rid.as_ref().and_then(|id| rs.get(id)) else { continue };
                    let pin = strip(r.cell("osm").or_empty());
                    let rt = match r.cell("route_type") {
                        crate::gtfs::Cell::None => return Err(crate::gtfs::PyErr::none()),
                        c => c.get().unwrap_or(""),
                    };
                    if ferrycount::is_ferry(rt) && !pin.is_empty() && !line_ids.contains(pin) {
                        found.push(ferrycount::unmatched_route(
                            name,
                            true,
                            r,
                            Py::Str(String::new()),
                            pin,
                            "its osm pin names no ferry line of this pass",
                        ));
                    }
                }
                drop(z);
                let feed = ferrycount::Feed { id: name.clone(), ..Default::default() };
                recs.extend(ferrycount::process(&feed, path, &by_ends, None, Some(&mut found))?);
                Ok((recs, found))
            })();
            match res {
                Ok((recs, found)) => {
                    eprintln!("{name}: {} lines", recs.len());
                    counted.push(recs);
                    unmatched.extend(found);
                }
                Err(e) => eprintln!("  {name}: {}: {e}", e.class()),
            }
        }
    }
    let p = phase("the frequencies read", Class::Compute);
    let rank = |rec: &Py| match rec.get("kind").map_or(Some("timetable"), |k| k.as_str()) {
        Some("gtfs") => 0,
        Some("timetable") => 1,
        _ => 2,
    };
    let mut freq: HashMap<String, Py> = HashMap::new();
    for recs in counted {
        for mut rec in recs {
            if let Some(Py::Str(h)) = rec.get("headway") {
                // "20 minutes", "2 hours (peak)"
                let c: Vec<char> = h.chars().collect();
                let mut i = 0;
                while i < c.len() && space(c[i]) {
                    i += 1;
                }
                let a = digits_at(&c, i, usize::MAX);
                let v = if a == 0 {
                    Py::Null
                } else {
                    let mut e = i + a;
                    if c.get(e) == Some(&'.') && digits_at(&c, e + 1, usize::MAX) > 0 {
                        e += 1 + digits_at(&c, e + 1, usize::MAX);
                    }
                    let mut k = e;
                    while k < c.len() && space(c[k]) {
                        k += 1;
                    }
                    let n = crate::pytext::float(&c[i..e].iter().collect::<String>()).unwrap_or(0.0);
                    Py::Float(n * if c.get(k) == Some(&'h') { 60.0 } else { 1.0 })
                };
                rec.set("headway", v);
            }
            let none = |r: &Py, k: &str| matches!(r.get(k), None | Some(Py::Null));
            if none(&rec, "per_day") && !none(&rec, "per_week") {
                let w = Num::of(rec.at("per_week")).context("a per_week that isn't a number")?;
                rec.set("per_day", Py::Float(py_round(w.f() / 7.0, 3)));
            }
            if none(&rec, "per_day") && none(&rec, "headway") {
                continue;
            }
            let Some(lid) = rec.get("line").and_then(Py::as_str).map(str::to_string) else { continue };
            if line_ids.contains(lid.as_str()) && freq.get(&lid).is_none_or(|f| rank(&rec) < rank(f)) {
                freq.insert(lid, rec);
            }
        }
    }
    drop(p);

    let p = phase("the lines described", Class::Compute);
    let mut out_lines: Vec<(String, Py)> = Vec::new();
    let mut info_of: HashMap<String, usize> = HashMap::new();
    let mut n_src: BTreeMap<String, u64> = BTreeMap::new();
    for (lid, l) in &lines.items {
        let t = &l.tags;
        let km = crate::gtfs::py_sum(l.ways.iter().map(|w| Num::Float(seg_km(&ways[w].plain)))).f();
        let dur = minutes(tag(t, "duration"));
        let g = if is_cable(t) || l.ways.iter().any(|w| is_cable(&ways[w].tags)) {
            CABLE
        } else if dur.is_some_and(|d| d.f() >= 150.0) || (dur.is_none() && km >= 80.0) {
            LONG
        } else if is_urban(t) {
            URBAN
        } else {
            CROSSING
        };
        let (mut season, season_text) = season_of(t);
        let vehicles = carries_vehicles(t) || l.ways.iter().any(|w| carries_vehicles(&ways[w].tags));
        let osm: Vec<String> = if l.rels.is_empty() {
            l.ways.iter().take(3).map(|w| format!("way/{w}")).collect()
        } else {
            l.rels.iter().map(|r| format!("relation/{r}")).collect()
        };
        let s = |x: &str| Py::Str(x.to_string());
        let first_of = |ks: &[&str]| ks.iter().map(|k| tg(t, k)).find(|v| !v.is_empty()).unwrap_or("").to_string();
        let mut info: Vec<(String, Py)> = vec![
            ("name".into(), Py::Str(clean_name(&first_of(&["name", "name:en"])))),
            ("ref".into(), s(tg(t, "ref"))),
            ("operator".into(), s(tg(t, "operator"))),
            ("network".into(), s(tg(t, "network"))),
            ("from".into(), s(tg(t, "from"))),
            ("to".into(), s(tg(t, "to"))),
            ("via".into(), s(tg(t, "via"))),
            ("group".into(), Py::Int(g.to_string())),
            ("km".into(), Py::Float(py_round(km, 1))),
            ("duration".into(), dur.map_or(Py::Null, Num::py)),
            ("vehicles".into(), Py::Bool(vehicles)),
            ("bicycle".into(), s(tg(t, "bicycle"))),
            ("roundtrip".into(), Py::Bool(tag(t, "roundtrip") == Some("yes"))),
            ("colour".into(), s(tg(t, "colour"))),
            ("website".into(), Py::Str(first_of(&["website", "url", "operator:website"]))),
            ("wikidata".into(), s(tg(t, "wikidata"))),
            ("osm".into(), Py::List(osm.iter().map(|o| s(o)).collect())),
            ("season".into(), Py::Int(season.to_string())),
            ("seasonText".into(), Py::Str(season_text.clone())),
            ("ends".into(), ends_py(ends_of[lid.as_str()])),
        ];
        let season_txt_osm = format!("{} {}", tg(t, "seasonal"), tg(t, "opening_hours"));
        let mut freq_info: Option<Vec<(String, Py)>> = None;
        if let Some(rec) = freq.get(lid) {
            let mut fi = Vec::new();
            for k in ["per_day", "per_day_low", "headway", "days", "months", "overnight", "source", "url", "checked"] {
                if let Some(v) = rec.get(k) {
                    fi.push((k.to_string(), v.clone()));
                }
            }
            freq_info = Some(fi);
            if rec.get("season").is_some_and(Py::truthy) {
                let to = match rec.at("season").as_str() {
                    Some("year") => Some(1),
                    Some("some-days") => Some(2),
                    Some("seasonal") => Some(3),
                    _ => None,
                };
                if let Some(to) = to {
                    season = to;
                }
                let st = rec.get("season_text").filter(|v| v.truthy()).cloned().unwrap_or(Py::Str(season_text.clone()));
                info.iter_mut().find(|(k, _)| k == "season").unwrap().1 = Py::Int(season.to_string());
                info.iter_mut().find(|(k, _)| k == "seasonText").unwrap().1 = st;
            }
            *n_src.entry(rec.get("kind").map_or("timetable".into(), Py::py_str)).or_default() += 1;
        } else {
            let hw = minutes(tag(t, "interval"));
            let span = daily_span(tag(t, "opening_hours"));
            if let Some(hw) = hw.filter(|h| h.f() != 0.0 && (1.0..=1440.0).contains(&h.f())) {
                let mut fi = vec![
                    ("headway".to_string(), hw.py()),
                    ("source".into(), s("OpenStreetMap interval tag")),
                    ("url".into(), Py::Str(format!("https://www.openstreetmap.org/{}", osm[0]))),
                ];
                match span {
                    Some(sp) if sp != 0 && hw.f() <= sp as f64 => {
                        fi.push(("per_day".into(), Py::Int(round0(sp as f64 / hw.f()).to_string())));
                        *n_src.entry("osm".into()).or_default() += 1;
                    }
                    _ => *n_src.entry("osm (headway only)".into()).or_default() += 1,
                }
                freq_info = Some(fi);
            }
        }
        if let Some(fi) = &freq_info {
            info.push(("freq".into(), Py::Dict(fi.clone())));
        }
        // Months a year: year-round lines 12; seasonal ones from their published months (research
        // or GTFS), else their OSM season/opening hours; unknown otherwise.
        let mo = if season == 1 || season == 2 {
            Some(12.0)
        } else {
            let fm = freq_info
                .as_ref()
                .and_then(|fi| fi.iter().find(|(k, _)| k == "months"))
                .map_or(String::new(), |(_, v)| v.py_str());
            let mut mo = months_of_text(&fm);
            if mo.is_none() && season == 3 {
                let st = info.iter().find(|(k, _)| k == "seasonText").map_or(String::new(), |(_, v)| v.py_str());
                mo = months_of_text(&st).or_else(|| months_of_text(&season_txt_osm));
            }
            mo
        };
        if let Some(mo) = mo {
            info.push(("months".into(), Py::Float(mo)));
        }
        info_of.insert(lid.clone(), out_lines.len());
        out_lines.push((lid.clone(), Py::Dict(info)));
    }
    drop(p);

    // ---- Features
    let p = phase("the ways' features made", Class::Compute);
    const PRIORITY: [i64; 4] = [LONG, CROSSING, URBAN, CABLE];
    let mut feats: Vec<Py> = Vec::new();
    let g_of = |i: &Py| Num::of(i.at("group")).map_or(0, |n| n.f() as i64);
    for (wid, lids) in &way_lines.items {
        let infos: Vec<&Py> = lids.iter().map(|l| &out_lines[info_of[l]].1).collect();
        let gs: HashSet<i64> = infos.iter().map(|i| g_of(i)).collect();
        let g = PRIORITY.iter().copied().find(|x| gs.contains(x)).unwrap_or(CROSSING);
        // Sailings over the way: lines between different ports add up (routes sharing a
        // stretch); lines between the same two ports (duplicate OSM relations, one operator's line
        // and an all-operator one) count once, at the highest figure.
        let mut groups: Vec<([[f64; 2]; 2], Num)> = Vec::new();
        let fr = |i: &Py, k: &str| -> Py { i.at("freq").at(k).clone() };
        for i in &infos {
            let Some(pd) = Num::of(&fr(i, "per_day")) else { continue };
            let e = num_pair(&i.at("ends").list().unwrap()[0]).unwrap();
            let e2 = num_pair(&i.at("ends").list().unwrap()[1]).unwrap();
            let (a, b) = ([e[0].f(), e[1].f()], [e2[0].f(), e2[1].f()]);
            let mut placed = false;
            for (ends, best) in groups.iter_mut() {
                let (c, d) = (ends[0], ends[1]);
                if km2(a, c).max(km2(b, d)) < 3.0 || km2(a, d).max(km2(b, c)) < 3.0 {
                    if pd.f() > best.f() {
                        *best = pd;
                    }
                    placed = true;
                    break;
                }
            }
            if !placed {
                groups.push(([a, b], pd));
            }
        }
        let known: Vec<Num> = groups.iter().map(|g| g.1).collect();
        let seasons: Vec<i64> =
            infos.iter().filter_map(|i| Num::of(i.at("season"))).map(|n| n.f() as i64).filter(|s| *s != 0).collect();
        let first_str =
            |f: &dyn Fn(&Py) -> String| infos.iter().map(|i| f(i)).find(|s| !s.is_empty()).unwrap_or_default();
        let colour = first_str(&|i| i.at("colour").py_str());
        let op = first_str(&|i| {
            let o = i.at("operator").py_str();
            if o.is_empty() {
                i.at("network").py_str()
            } else {
                o
            }
        });
        let name = first_str(&|i| i.at("name").py_str());
        let hw = if known.is_empty() {
            let mut best: Option<Num> = None;
            for i in &infos {
                if let Some(h) = Num::of(&fr(i, "headway")).filter(|h| h.f() != 0.0) {
                    if best.is_none_or(|b| h.f() < b.f()) {
                        best = Some(h);
                    }
                }
            }
            best.unwrap_or(Num::Int(0))
        } else {
            Num::Int(0)
        };
        let months =
            infos.iter().filter_map(|i| i.get("months").and_then(Num::of)).fold(None, |b: Option<Num>, m| match b {
                Some(b) if m.f() <= b.f() => Some(b),
                _ => Some(m),
            });
        let props = vec![
            ("g".to_string(), Py::Int(g.to_string())),
            ("gb".into(), Py::Int(gs.iter().map(|x| 1i64 << x).sum::<i64>().to_string())),
            ("car".into(), Py::Int((infos.iter().any(|i| i.at("vehicles") == &Py::Bool(true)) as i64).to_string())),
            (
                "f".into(),
                if known.is_empty() {
                    Py::Int("-1".into())
                } else {
                    crate::gtfs::py_sum(known.iter().copied()).round(1).py()
                },
            ),
            ("hw".into(), hw.py()),
            (
                "fp".into(),
                Py::Int(
                    if known.is_empty() {
                        0
                    } else {
                        infos.iter().any(|i| matches!(i.at("freq").get("per_day"), None | Some(Py::Null))) as i64
                    }
                    .to_string(),
                ),
            ),
            ("s".into(), Py::Int(seasons.iter().min().copied().unwrap_or(0).to_string())),
            ("m".into(), months.map_or(Py::Int("-1".into()), Num::py)),
            ("col".into(), Py::Str(colour)),
            ("op".into(), Py::Str(op)),
            ("n".into(), Py::Str(name)),
            ("lines".into(), Py::Str(lids.join(","))),
        ];
        let coords = ways[wid].coords.iter().map(|p| Py::List(vec![p[0].round(6).py(), p[1].round(6).py()])).collect();
        feats.push(Py::Dict(vec![
            ("type".into(), Py::Str("Feature".into())),
            ("id".into(), Py::Int(wid.to_string())),
            (
                "geometry".into(),
                Py::Dict(vec![("type".into(), Py::Str("LineString".into())), ("coordinates".into(), Py::List(coords))]),
            ),
            ("properties".into(), Py::Dict(props)),
        ]));
    }
    drop(p);

    // Terminals: named ferry terminals within 1 km of a line's ends.
    let p = phase("the terminals placed", Class::Compute);
    let mut cell: HashMap<(i64, i64), Vec<[f64; 2]>> = HashMap::new();
    for (_, l) in &lines.items {
        for w in &l.ways {
            let c = &ways[w].plain;
            for e in [c[0], c[c.len() - 1]] {
                cell.entry((round0(e[0] * 50.0), round0(e[1] * 50.0))).or_default().push(e);
            }
        }
    }
    let mut n_term = 0;
    let tpath = src.join("terminals.geojsonseq");
    if tpath.exists() {
        let text = std::fs::read_to_string(&tpath)?;
        for line in text.split_inclusive('\n') {
            let line = strip(line.trim_start_matches('\x1e'));
            if line.is_empty() {
                continue;
            }
            let f = loads(line)?;
            let Some(name) = f.at("properties").get("name").filter(|n| n.truthy()) else { continue };
            let geom = f.at("geometry");
            let (x, y) = if geom.at("type").as_str() == Some("Point") {
                num_pair(geom.at("coordinates")).map(|p| (p[0], p[1])).context("a terminal's point")?
            } else {
                let c = geom.at("coordinates");
                let ring = match geom.at("type").as_str() {
                    Some("Polygon") => &c.list().context("a polygon")?[0],
                    Some("MultiPolygon") => &c.list().context("a multipolygon")?[0].list().context("a polygon")?[0],
                    _ => c,
                };
                let pts: Vec<[Num; 2]> = ring.list().context("a ring")?.iter().filter_map(num_pair).collect();
                let sum = |k: usize| crate::gtfs::py_sum(pts.iter().map(|p| p[k])).f();
                (Num::Float(sum(0) / pts.len() as f64), Num::Float(sum(1) / pts.len() as f64))
            };
            let (fx, fy) = (x.f(), y.f());
            let (cx, cy) = (round0(fx * 50.0), round0(fy * 50.0));
            let near = (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    cell.get(&(cx + dx, cy + dy)).is_some_and(|es| es.iter().any(|e| km2([fx, fy], *e) < 1.0))
                })
            });
            if near {
                n_term += 1;
                feats.push(Py::Dict(vec![
                    ("type".into(), Py::Str("Feature".into())),
                    (
                        "geometry".into(),
                        Py::Dict(vec![
                            ("type".into(), Py::Str("Point".into())),
                            ("coordinates".into(), Py::List(vec![x.round(6).py(), y.round(6).py()])),
                        ]),
                    ),
                    (
                        "properties".into(),
                        Py::Dict(vec![("kind".into(), Py::Str("terminal".into())), ("n".into(), name.clone())]),
                    ),
                ]));
            }
        }
    }
    drop(p);

    let _p = phase("the ferry layers made", Class::Compute);
    let ferries = dumps(&Py::Dict(vec![
        ("type".into(), Py::Str("FeatureCollection".into())),
        ("features".into(), Py::List(feats)),
    ]));
    let lines_json = dumps(&Py::Dict(out_lines.clone()));
    // For the timetables' to-do list and report (docs/inputs.md §6.7.3): the lines no timetable
    // counts (an OSM interval tag isn't one), and the timetable routes that match no line.
    let mut todo = Vec::new();
    for (lid, i) in &out_lines {
        if freq.contains_key(lid) {
            continue;
        }
        let mut d = vec![("osm".to_string(), Py::Str(lid.clone())), ("kind".into(), Py::Str("ferry".into()))];
        for k in ["name", "ref", "operator", "from", "to", "km"] {
            d.push((k.to_string(), i.at(k).clone()));
        }
        let passenger = tag(&lines.get(lid).unwrap().1.tags, "passenger") != Some("no");
        d.push(("passenger".into(), Py::Bool(passenger)));
        d.push(("ends".into(), i.at("ends").clone()));
        todo.push(Py::Dict(d));
    }
    let with = out_lines.len() - todo.len();
    let timetables = dumps(&Py::Dict(vec![
        ("lines".into(), Py::List(todo)),
        ("with_timetable".into(), Py::Int(with.to_string())),
        ("unmatched".into(), Py::List(unmatched)),
    ]));
    let mut by_g: BTreeMap<&str, u64> = BTreeMap::new();
    for (_, i) in &out_lines {
        *by_g.entry(GROUP_NAMES[g_of(i) as usize]).or_default() += 1;
    }
    eprintln!("{} lines on {} ways, {n_term} terminals; groups: {by_g:?}", out_lines.len(), way_lines.len());
    eprintln!(
        "frequency known for {} lines; sources: {n_src:?}",
        out_lines.iter().filter(|(_, i)| !matches!(i.at("freq").get("per_day"), None | Some(Py::Null))).count()
    );
    Ok(Written { ferries, lines: lines_json, timetables })
}

#[cfg(test)]
mod tests;
