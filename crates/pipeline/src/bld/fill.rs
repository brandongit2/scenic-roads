//! Filling the missing heights (docs/buildings3d.md §2.3, as B0 set them): a building's height is
//! the first of these it has, and it says which (`s`):
//!
//! 0. measured: Overture's `height` from any source but Microsoft's estimates, 2–700 m;
//! 1. floors: `num_floors` (1–200) × the country's storey height + its roof allowance ([`storey`]);
//! 2. Microsoft's estimate: Overture's `height` from Microsoft ML Buildings, 2–700 m;
//! 3. neighbours: the median of the heights (by rules 0–2) of the buildings within 150 m whose
//!    footprint is between half and twice its own, when there are at least 5; else, for a footprint
//!    of 60 m² or more, of any footprint within 300 m, when there are at least 8 ([`Near`]): a
//!    smaller one goes on to rules 4–5 (the area's median made kiosks and poles towers);
//! 4. GHSL in high-rise cores: the 3″ cell's average height at the centroid when it's 20 m or more,
//!    at most 4 m for a footprint under 60 m²;
//! 5. size and kind: the measured median of its class in its country (where B0 measured 200 of
//!    them), else in the coverage ([`sizes`], [`size_bin`]).
//!
//! A building part (OSM's `building:part`) takes rules 0, 1, 2 and 5 (its footprint alone: parts
//! have no class); its base is `min_height`, else `min_floor` storeys up; a top at or under its base
//! is raised a storey above it. Heights are whole decimetres; a median of an even count takes the
//! lower middle. The tables are B0's measurements (dem/bldmeasure.py's fit.json, 2026-10-06):
//! [`super::BUILDINGS_V`] names them.

/// A height taken (rules 0 and 2), decimetres.
pub const H_MIN_DM: u16 = 20;
pub const H_MAX_DM: u16 = 7000;
/// Floors taken (rule 1).
pub const F_MAX: u8 = 200;
/// The dataset whose heights are estimates, not measurements.
pub const ESTIMATES: &str = "Microsoft ML Buildings";
/// Rule 4: GHSL where its cell is this tall (dm); a footprint under `GHSL_SMALL_M2` takes at most
/// `GHSL_SMALL_DM`.
pub const GHSL_TALL_DM: u16 = 200;
pub const GHSL_SMALL_M2: f32 = 60.0;
pub const GHSL_SMALL_DM: u16 = 40;
/// Rule 3's radii (m) and counts.
pub const NEAR_M: f64 = 150.0;
pub const FAR_M: f64 = 300.0;
pub const NEAR_MIN: usize = 5;
pub const FAR_MIN: usize = 8;
/// Rule 3's 300 m stage takes footprints of this or more (m²): it takes any footprint's height, so
/// a smaller one (a kiosk, a stair housing, a pole: rarely measured, rarely like its neighbours)
/// would stand as tall as the area's median, a 2.5 m² needle 22 m tall in Paris. The same bound as
/// rule 4's small footprints.
pub const FAR_AREA_M2: f32 = GHSL_SMALL_M2;

/// `s`: where the height comes from.
pub mod src {
    pub const MEASURED: u8 = 0;
    pub const FLOORS: u8 = 1;
    pub const MICROSOFT: u8 = 2;
    pub const NEIGHBOURS: u8 = 3;
    pub const GHSL: u8 = 4;
    pub const SIZE: u8 = 5;
}

/// Rule 1: metres a floor and the roof allowance, fitted by least absolute deviations on the
/// measured buildings with both (B0: countries with 500 of them).
const STOREY: [(&str, f64, f64); 12] = [
    ("CA", 3.47, 1.59),
    ("ES", 3.0, 0.0),
    ("FR", 2.95, 1.6),
    ("GB", 3.205, 1.59),
    ("HK", 3.32, -1.32),
    ("IE", 2.5, 2.5),
    ("JP", 2.815, 1.87),
    ("PR", 2.92, 0.37),
    ("PT", 3.0, 1.0),
    ("SG", 3.615, 1.54),
    ("TW", 3.32, -0.96),
    ("US", 3.1, 0.9),
];
/// Rule 1 elsewhere: the coverage's fit.
const STOREY_ALL: (f64, f64) = (3.03, 1.27);

/// Rule 5's medians (m) by [`size_bin`]: churches and cathedrals; sheds, garages, carports and
/// huts, or under 30 m²; houses and residential kinds, or under 250 m²; 250–2,000 m²; larger. A
/// country's where B0 measured 200 of the class, else the coverage's.
const SIZE: [(&str, [f64; 5]); 12] = [
    ("CA", [8.8, 3.3, 6.0, 7.4, 10.4]),
    ("ES", [8.8, 3.0, 6.0, 6.0, 8.0]),
    ("FR", [13.0, 5.0, 7.0, 10.0, 12.0]),
    ("GB", [8.8, 3.0, 7.7, 8.0, 13.0]),
    ("HK", [8.8, 3.3, 38.0, 55.0, 9.1]),
    ("IE", [8.8, 3.9, 6.5, 8.0, 9.1]),
    ("JP", [8.8, 3.2, 7.5, 9.0, 14.8]),
    ("PR", [8.8, 3.0, 3.5, 4.1, 8.0]),
    ("PT", [8.8, 4.0, 7.0, 8.0, 10.0]),
    ("SG", [8.8, 3.3, 10.0, 21.0, 25.0]),
    ("TW", [8.8, 9.0, 12.0, 12.0, 22.0]),
    ("US", [8.6, 3.3, 5.5, 6.2, 8.9]),
];
const SIZE_ALL: [f64; 5] = [8.8, 3.3, 6.0, 6.4, 9.1];

/// Rule 5's kinds (Overture's classes).
const CHURCHES: [&str; 2] = ["cathedral", "church"];
const SHEDS: [&str; 5] = ["carport", "garage", "garages", "hut", "shed"];
const HOUSES: [&str; 17] = [
    "allotment_house", "apartments", "bungalow", "cabin", "detached", "dormitory", "dwelling_house", "farm", "ger", "house", "houseboat", "residential", "semidetached_house", "static_caravan",
    "stilt_house", "terrace", "trullo",
];

/// A country's storey height and roof allowance (rule 1), metres.
pub fn storey(country: &str) -> (f64, f64) {
    STOREY.iter().find(|s| s.0 == country).map_or(STOREY_ALL, |s| (s.1, s.2))
}

/// A country's rule-5 medians, metres.
pub fn sizes(country: &str) -> [f64; 5] {
    SIZE.iter().find(|s| s.0 == country).map_or(SIZE_ALL, |s| s.1)
}

/// Rule 5's class of a building: 0 church or cathedral, 1 shed, garage, carport or hut (or under
/// 30 m²), 2 house or residential (or under 250 m²), 3 under 2,000 m², 4 larger.
pub fn size_bin(class: &str, subtype: &str, area: f32) -> usize {
    if CHURCHES.contains(&class) {
        0
    } else if SHEDS.contains(&class) || area < 30.0 {
        1
    } else if HOUSES.contains(&class) || subtype == "residential" || area < 250.0 {
        2
    } else if area < 2000.0 {
        3
    } else {
        4
    }
}

/// Metres as whole decimetres.
pub fn dm(m: f64) -> u16 {
    (m * 10.0).round().clamp(0.0, 65535.0) as u16
}

/// Rule 1: `f` floors' height in a country, decimetres (kept within 2–700 m).
pub fn floors_dm(f: u8, fit: (f64, f64)) -> u16 {
    dm((fit.0 * f as f64 + fit.1).clamp(2.0, 700.0))
}

/// Rules 0–2: (height in dm, `s`) by a measurement, floors or Microsoft's estimate, if any. `h` the
/// stored height (dm, 0 none), `est` whether it's Microsoft's, `f` the floors (0 none).
pub fn first_rules(h: u16, est: bool, f: u8, fit: (f64, f64)) -> Option<(u16, u8)> {
    let taken = (H_MIN_DM..=H_MAX_DM).contains(&h);
    if taken && !est {
        Some((h, src::MEASURED))
    } else if (1..=F_MAX).contains(&f) {
        Some((floors_dm(f, fit), src::FLOORS))
    } else if taken {
        Some((h, src::MICROSOFT))
    } else {
        None
    }
}

/// Rule 4, else rule 5: (height in dm, `s`).
pub fn last_rules(ghsl: u16, area: f32, bin: usize, country: &str) -> (u16, u8) {
    if ghsl >= GHSL_TALL_DM {
        (if area < GHSL_SMALL_M2 { ghsl.min(GHSL_SMALL_DM) } else { ghsl }, src::GHSL)
    } else {
        (dm(sizes(country)[bin]), src::SIZE)
    }
}

/// A part's (top, base, `s`), decimetres: rules 0–2, else its size (rule 5); its base `min_height`
/// (`m`), else `min_floor` (`mf`) storeys up; a top at or under its base raised a storey above it.
pub fn part(h: u16, est: bool, f: u8, m: u16, mf: u8, area: f32, country: &str) -> (u16, u16, u8) {
    let fit = storey(country);
    let (top, s) = first_rules(h, est, f, fit).unwrap_or_else(|| (dm(sizes(country)[size_bin("", "", area)]), src::SIZE));
    let base = if (1..=H_MAX_DM).contains(&m) { m } else if mf >= 1 { dm(fit.0 * mf as f64) } else { 0 };
    let top = if top <= base { base.saturating_add(dm(fit.0)) } else { top };
    (top, base, s)
}

/// A height by rules 0–2, where the neighbours' rule finds it: Web Mercator metres (a world unit
/// = [`super::EQ`]), footprint, height (dm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub area: f32,
    pub h: u16,
}

/// The neighbours' rule over a set of points: a grid of cells at least 300 ground metres across
/// wherever it's asked, so a query reads its cell and the eight around it.
pub struct Near {
    x0: f64,
    y0: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    /// Per cell, its first point (prefix sums: one more than the cells).
    start: Vec<u32>,
    pts: Vec<Point>,
}

impl Near {
    /// The grid over `pts` for queries at latitudes whose cosine is at least `cos_min`.
    pub fn new(mut pts: Vec<Point>, cos_min: f64) -> Near {
        let cell = (FAR_M + 10.0) / cos_min.max(0.01);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &pts {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        }
        if pts.is_empty() {
            (x0, y0, x1, y1) = (0.0, 0.0, 0.0, 0.0);
        }
        let nx = (((x1 - x0) / cell).floor() as usize + 1).min(1 << 16);
        let ny = (((y1 - y0) / cell).floor() as usize + 1).min(1 << 16);
        let cell_of = |p: &Point| (((p.y - y0) / cell).floor() as usize).min(ny - 1) * nx + (((p.x - x0) / cell).floor() as usize).min(nx - 1);
        // (A stable sort by cell: the points' order within a cell is theirs.)
        pts.sort_by_key(cell_of);
        let mut start = vec![0u32; nx * ny + 1];
        for p in &pts {
            start[cell_of(p) + 1] += 1;
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        Near { x0, y0, cell, nx, ny, start, pts }
    }

    /// Rule 3 for a building at Web Mercator metres (x, y), where a ground metre is `cos` of one,
    /// with footprint `area`: the median height (dm) and the stage that gave it (1: 150 m and like
    /// footprints, 2: 300 m, for a footprint of [`FAR_AREA_M2`] or more), if any. `a`, `b`: scratch.
    pub fn height(&self, x: f64, y: f64, cos: f64, area: f32, a: &mut Vec<u16>, b: &mut Vec<u16>) -> Option<(u16, u8)> {
        match self.stages(x, y, cos, area, a, b) {
            (Some(h), _) => Some((h, 1)),
            (None, Some(h)) if area >= FAR_AREA_M2 => Some((h, 2)),
            _ => None,
        }
    }

    /// Rule 3's two stages for a building, each's median height (dm) where it has enough
    /// buildings: (150 m and like footprints, 300 m), whatever the footprint's size.
    pub fn stages(&self, x: f64, y: f64, cos: f64, area: f32, a: &mut Vec<u16>, b: &mut Vec<u16>) -> (Option<u16>, Option<u16>) {
        a.clear();
        b.clear();
        if self.pts.is_empty() {
            return (None, None);
        }
        let cx = ((x - self.x0) / self.cell).floor() as i64;
        let cy = ((y - self.y0) / self.cell).floor() as i64;
        let (lo, hi) = (area as f64 * 0.5, area as f64 * 2.0);
        for gy in cy - 1..=cy + 1 {
            if gy < 0 || gy >= self.ny as i64 {
                continue;
            }
            for gx in cx - 1..=cx + 1 {
                if gx < 0 || gx >= self.nx as i64 {
                    continue;
                }
                let c = gy as usize * self.nx + gx as usize;
                for p in &self.pts[self.start[c] as usize..self.start[c + 1] as usize] {
                    let (dx, dy) = (p.x - x, p.y - y);
                    let d = (dx * dx + dy * dy).sqrt() * cos;
                    if d <= FAR_M {
                        b.push(p.h);
                        if d <= NEAR_M && p.area as f64 >= lo && p.area as f64 <= hi {
                            a.push(p.h);
                        }
                    }
                }
            }
        }
        let median = |v: &mut Vec<u16>| {
            let k = (v.len() - 1) / 2;
            *v.select_nth_unstable(k).1
        };
        let s1 = (a.len() >= NEAR_MIN).then(|| median(a));
        let s2 = (b.len() >= FAR_MIN).then(|| median(b));
        (s1, s2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_in_order() {
        let us = storey("US");
        assert_eq!(us, (3.1, 0.9));
        assert_eq!(storey("XX"), (3.03, 1.27));
        // Measured beats floors; floors beat Microsoft's estimate.
        assert_eq!(first_rules(215, false, 6, us), Some((215, src::MEASURED)));
        assert_eq!(first_rules(215, true, 6, us), Some((dm(3.1 * 6.0 + 0.9), src::FLOORS)));
        assert_eq!(first_rules(215, true, 0, us), Some((215, src::MICROSOFT)));
        // Out of 2–700 m: none; over 200 floors: none.
        assert_eq!(first_rules(15, false, 0, us), None);
        assert_eq!(first_rules(7001, false, 0, us), None);
        assert_eq!(first_rules(0, false, 201, us), None);
        // Floors kept within 2–700 m (Hong Kong's negative allowance on one floor).
        assert_eq!(floors_dm(1, storey("HK")), dm(2.0));
        // GHSL in its tall cells, capped for small footprints; else the size.
        assert_eq!(last_rules(315, 400.0, 3, "US"), (315, src::GHSL));
        assert_eq!(last_rules(315, 50.0, 1, "US"), (40, src::GHSL));
        assert_eq!(last_rules(150, 400.0, 3, "US"), (62, src::SIZE));
        assert_eq!(last_rules(0, 100.0, 2, "JP"), (75, src::SIZE));
        assert_eq!(last_rules(0, 100.0, 2, "XX"), (60, src::SIZE));
        // Size classes.
        assert_eq!(size_bin("church", "", 5000.0), 0);
        assert_eq!(size_bin("garage", "", 500.0), 1);
        assert_eq!(size_bin("", "", 20.0), 1);
        assert_eq!(size_bin("", "residential", 900.0), 2);
        assert_eq!(size_bin("", "", 900.0), 3);
        assert_eq!(size_bin("office", "commercial", 3000.0), 4);
        // Parts: a base from min_floor, a top raised above it.
        assert_eq!(part(300, false, 0, 0, 0, 100.0, "FR"), (300, 0, src::MEASURED));
        assert_eq!(part(0, false, 0, 0, 4, 100.0, "FR"), (dm(2.95 * 4.0) + dm(2.95), dm(2.95 * 4.0), src::SIZE));
        assert_eq!(part(100, false, 0, 120, 0, 100.0, "FR"), (120 + dm(2.95), 120, src::MEASURED));
    }

    #[test]
    fn neighbours() {
        // Points every 50 m along x at y = 0 (at the equator: cos 1), heights 10..=19 m.
        let pts: Vec<Point> = (0..10).map(|i| Point { x: i as f64 * 50.0, y: 0.0, area: 100.0, h: 100 + 10 * i as u16 }).collect();
        let n = Near::new(pts, 1.0);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        // At x = 0: within 150 m, four points (0, 50, 100, 150): too few; within 300 m, seven: too
        // few too.
        assert_eq!(n.height(0.0, 0.0, 1.0, 100.0, &mut a, &mut b), None);
        // At x = 225: within 150 m, x 100–350: six, like footprints: the lower middle of 120..=170.
        assert_eq!(n.height(225.0, 0.0, 1.0, 100.0, &mut a, &mut b), Some((140, 1)));
        // Footprints too unlike: the 300 m stage (x 0–450 within 300 m of 225: ten points).
        assert_eq!(n.height(225.0, 0.0, 1.0, 1000.0, &mut a, &mut b), Some((140, 2)));
        // But not for a small footprint (none like it near): left to rules 4–5.
        assert_eq!(n.height(225.0, 0.0, 1.0, 20.0, &mut a, &mut b), None);
        // At 60° a Mercator metre is half a ground metre: x 0–450 lie within 150 m of 225.
        let n = Near::new((0..10).map(|i| Point { x: i as f64 * 50.0, y: 0.0, area: 100.0, h: 100 + 10 * i as u16 }).collect(), 0.5);
        assert_eq!(n.height(225.0, 0.0, 0.5, 100.0, &mut a, &mut b), Some((140, 1)));
        // No points: none.
        assert_eq!(Near::new(Vec::new(), 1.0).height(0.0, 0.0, 1.0, 1.0, &mut a, &mut b), None);
    }
}
