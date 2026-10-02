//! Reading areas: which translation table a name is read by, from where the named thing is
//! (中山 is Nakayama in Japan, Zhongshan in Taiwan).
//!
//! The translation work (`place-translations`, `tools/export/places.py` `region()`, and its
//! vectorised twin for roads) gave every name the area of the feature it came from:
//!
//! ```python
//! def region(lon, lat):
//!     for r, w, s, e, n in (("hk", 113.8, 22.1, 114.5, 22.6), ("sg", 103.55, 1.1, 104.2, 1.5),
//!                           ("tw", 118.0, 21.8, 122.3, 26.5), ("jp", 122.5, 20.0, 154.5, 46.0)):
//!         if w <= lon <= e and s <= lat <= n: return r
//!     if lon < -30: return "pt" if lat < 45 and lon > -35 else "na"
//!     if lon < -12: return "pt"
//!     if lat >= 49.8 and lon < 2.0 or lat >= 51.5: return "gb"
//!     return "ib" if lat < 42.7 or (lon < -1.7 and lat < 43.4) else "fr"
//! ```
//!
//! It was only ever applied to features in the map's coverage (Canada, the northeastern US,
//! Britain and Ireland, France, Spain and Portugal with the Azores and Madeira, Japan, Taiwan, Hong
//! Kong, Singapore), so beyond them its answers mean nothing: Beijing falls through to "ib", Berlin
//! to "gb", Seattle to "na", and Harbin and Seoul lie in Japan's box. [`area_at`] is that function
//! exactly within its *reach* (the coverage plus a margin, as rectangles below) and `None` outside
//! it, so a name far from the coverage never takes a reading made for somewhere else.
//!
//! The reach was checked against the 2.4 M label points of `data/build/labels.tiles` and the first
//! vertices of the 10.5 M named roads the translation work read (the point it gave a road its area
//! by): all but 15 get the function's answer. Those 15 lie just outside the coverage and get `None`
//! (own English, which for them is all their translation lines hold): two parks the function sends
//! to "ib" by accident (on Pedra Branca, east of Singapore's box, and in the Pearl River estuary,
//! west of Hong Kong's; reaching them would put Macau and Zhuhai under Iberia's table), a Xiamen
//! marine reserve beside Kinmen, a Franco-German biosphere reserve labelled in the Palatinate,
//! Michigan's islands in Lake Huron, a North Dakota refuge and two roads starting south of the 49th
//! parallel, and the Lincoln Sea and Davis Strait.
//!
//! The function's quirks are kept on purpose, since the tables were built with them: Jersey and
//! Guernsey read as "fr", the French coast west of 2° E and north of 49.8° N (Calais, Boulogne,
//! Dieppe) as "gb", northern Spain above 43.4° N west of 1.7° W (Santander, Gijón, Ferrol) as "fr",
//! southern Corsica (Ajaccio, Bonifacio) as "ib", and Saint-Pierre-et-Miquelon as "na".
//!
//! When the coverage grows, the translation work's function and these rectangles grow with it.

/// A lon/lat rectangle, closed on every side.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    w: f64,
    s: f64,
    e: f64,
    n: f64,
}

const fn r(w: f64, s: f64, e: f64, n: f64) -> Rect {
    Rect { w, s, e, n }
}

impl Rect {
    fn contains(&self, lon: f64, lat: f64) -> bool {
        self.w <= lon && lon <= self.e && self.s <= lat && lat <= self.n
    }

    /// The overlap with another rectangle, if they touch.
    fn intersect(&self, o: &Rect) -> Option<Rect> {
        let i = r(self.w.max(o.w), self.s.max(o.s), self.e.min(o.e), self.n.min(o.n));
        (i.w <= i.e && i.s <= i.n).then_some(i)
    }
}

/// The translation work's area codes (its folder and file-name codes).
pub const AREAS: [&str; 9] = ["jp", "tw", "hk", "sg", "fr", "ib", "pt", "na", "gb"];

/// Whether `code` is one of the translation work's area codes ([`AREAS`]).
pub fn is_area(code: &str) -> bool {
    AREAS.contains(&code)
}

/// The function's four boxes, checked first and in its order, each with where inside it the
/// coverage lies. A point in a box but outside its reach is `None`, never a fallthrough area.
const BOXES: &[(&str, Rect, &[Rect])] = &[
    // Hong Kong, with the strip of Shenzhen its box holds (Futian, Luohu and Yantian are in the
    // "hk" table).
    ("hk", r(113.8, 22.1, 114.5, 22.6), &[r(113.8, 22.1, 114.5, 22.6)]),
    // Singapore, with the edges of Johor Bahru and Batam its box holds.
    ("sg", r(103.55, 1.1, 104.2, 1.5), &[r(103.55, 1.1, 104.2, 1.5)]),
    // Taiwan, without Fujian (Xiamen, Quanzhou, Putian, Pingtan, Fuzhou) beside its islands.
    (
        "tw",
        r(118.0, 21.8, 122.3, 26.5),
        &[
            r(119.2, 21.8, 122.3, 24.6),     // the south and centre, Penghu, Green and Orchid Islands
            r(120.6, 24.6, 122.3, 25.8),     // the north, Keelung's islets
            r(118.19, 24.36, 118.55, 24.56), // Kinmen and Lieyu (not Xiamen Island, west of 118.19°)
            r(118.12, 24.33, 118.19, 24.40), // Dadan and Erdan, south of Xiamen Island
            r(119.88, 25.93, 120.56, 26.42), // Matsu
            r(119.40, 24.94, 119.50, 25.02), // Wuqiu
        ],
    ),
    // Japan, without Korea, China or the Russian mainland, which its box also holds.
    (
        "jp",
        r(122.5, 20.0, 154.5, 46.0),
        &[
            r(122.5, 20.0, 143.0, 30.0),  // the Ryukyus, Daito, Okinotorishima, Ogasawara, Iwo
            r(153.5, 23.8, 154.5, 24.8),  // Minamitorishima
            r(127.5, 30.0, 130.0, 33.8),  // the Goto Islands, western Kyushu (Jeju is west of 127°)
            r(129.0, 33.8, 130.0, 34.8),  // Tsushima and Iki (Busan is north of 35°)
            r(130.0, 30.0, 146.0, 37.0),  // Kyushu, Shikoku, Honshu to Noto, the Izu Islands
            r(136.5, 37.0, 146.0, 41.6),  // northern Honshu
            r(139.0, 41.3, 149.0, 46.0),  // Hokkaido and the southern Kurils
        ],
    ),
];

/// Where the fallthrough branches (na, pt, gb, fr, ib) hold: Europe's and North America's
/// coverage, with margins of 20–50 km over land borders and more at sea. None of these touch a box.
const FALLTHROUGH: &[Rect] = &[
    // Iberia and France.
    r(-11.5, 35.9, -1.5, 44.0),   // Portugal, western and southern Spain, Gibraltar, Gorringe Bank
    r(-1.5, 37.2, 4.6, 44.0),     // eastern Spain, the Balearics, Andorra (Algeria is south of 37°)
    r(-5.45, 35.82, -5.25, 35.95), // Ceuta
    r(-3.05, 35.22, -2.85, 35.35), // Melilla
    r(-2.45, 35.16, -2.39, 35.2), // the Chafarinas
    r(-3.92, 35.2, -3.89, 35.225), // Alhucemas
    r(-4.31, 35.165, -4.29, 35.18), // Vélez de la Gomera
    r(-5.6, 42.2, 2.5, 51.3),     // France west of 2.5° E, the Channel Islands
    r(2.5, 42.2, 3.0, 51.15),     // Dunkirk to the Belgian coast
    r(3.0, 42.2, 4.3, 50.8),      // Lille and Hainaut
    r(4.3, 42.2, 5.9, 50.2),      // the Ardennes
    r(5.9, 42.2, 7.0, 49.6),      // Lorraine, the Jura, Savoy, Geneva
    r(7.0, 47.3, 8.4, 49.25),     // Alsace, the Rhine, Moselle and Delle east of 7° E
    r(7.0, 44.3, 7.25, 46.0),     // the Alps east of 7° E, to Mont Dolent
    r(7.0, 43.5, 7.75, 44.3),     // Nice, Monaco, Menton, the Roya
    r(8.0, 41.2, 9.9, 43.2),      // Corsica and its marine parks (and Capraia, in France's outline)
    // Britain and Ireland, the Portuguese islands.
    r(-11.0, 49.7, 2.0, 61.1),    // Britain, Ireland, Man, Orkney, Shetland
    r(2.0, 51.5, 3.4, 53.6),      // the North Sea's banks, short of the Dutch coast
    r(-14.0, 57.4, -13.4, 57.8),  // Rockall
    r(-31.6, 36.6, -24.6, 40.6),  // the Azores and their seamounts
    r(-17.6, 29.8, -15.6, 33.4),  // Madeira, Porto Santo, the Desertas and Savage Islands
    // Canada north of the 49th parallel (Alaska's panhandle mostly left out, in steps).
    r(-130.3, 48.8, -52.0, 54.8), // British Columbia's mainland to Newfoundland
    r(-136.5, 51.5, -130.3, 54.8), // Haida Gwaii, the Bowie Seamount
    r(-129.5, 48.1, -122.7, 48.8), // southern Vancouver Island
    r(-131.0, 47.8, -125.0, 48.8), // the marine areas off it (Cape Flattery is east of 125° W)
    r(-130.7, 54.8, -52.0, 56.2), // then along the panhandle's border, from Portland Canal north
    r(-131.3, 56.2, -52.0, 56.5),
    r(-132.0, 56.5, -52.0, 57.2),
    r(-132.95, 57.2, -52.0, 58.1),
    r(-133.75, 58.1, -52.0, 58.6),
    r(-134.0, 58.6, -52.0, 59.1),
    r(-139.2, 59.1, -52.0, 60.0), // with British Columbia's Tatshenshini corner
    r(-141.5, 60.0, -61.0, 75.0), // the territories (Greenland is east of 61° W)
    r(-125.5, 75.0, -71.0, 79.0), // the Queen Elizabeth Islands
    r(-110.0, 79.0, -60.0, 84.0), // Ellesmere and Axel Heiberg (with Greenland across Nares Strait)
    // Ontario's south, around the Great Lakes, and the east.
    r(-95.2, 47.8, -88.4, 48.8),  // Rainy River, Quetico, Thunder Bay
    r(-88.4, 47.6, -83.5, 48.8),  // Lake Superior's north shore
    r(-85.2, 46.0, -83.5, 47.6),  // Sault Ste. Marie, Algoma, St. Joseph Island
    r(-86.0, 47.2, -85.2, 47.6),  // Caribou Island
    r(-83.6, 45.4, -80.0, 48.8),  // Manitoulin Island, Sudbury
    r(-82.6, 43.1, -80.0, 45.4),  // Lake Huron's east shore, the Bruce Peninsula
    r(-83.2, 41.6, -80.0, 43.1),  // Windsor, Sarnia, Lake Erie's islands
    r(-80.0, 41.75, -52.0, 48.8), // Toronto to Newfoundland, New York, New England
    r(-75.2, 40.3, -69.5, 41.75), // New York City, Long Island, Connecticut, Rhode Island, the Cape
];

/// The fallthrough branches of the translation work's function, verbatim.
fn fallthrough(lon: f64, lat: f64) -> &'static str {
    if lon < -30.0 {
        return if lat < 45.0 && lon > -35.0 { "pt" } else { "na" };
    }
    if lon < -12.0 {
        return "pt";
    }
    if (lat >= 49.8 && lon < 2.0) || lat >= 51.5 {
        return "gb";
    }
    if lat < 42.7 || (lon < -1.7 && lat < 43.4) {
        "ib"
    } else {
        "fr"
    }
}

/// The area whose translation table a name at `lon`, `lat` is read by: the translation work's
/// `region()` within the coverage's reach, `None` outside it (and for non-finite input).
pub fn area_at(lon: f64, lat: f64) -> Option<&'static str> {
    if !lon.is_finite() || !lat.is_finite() {
        return None;
    }
    for (code, bx, reach) in BOXES {
        if bx.contains(lon, lat) {
            return reach.iter().any(|q| q.contains(lon, lat)).then_some(*code);
        }
    }
    FALLTHROUGH.iter().any(|q| q.contains(lon, lat)).then(|| fallthrough(lon, lat))
}

/// Every area [`area_at`] can give for a point in the closed box `west..=east`, `south..=north`
/// (degrees), in [`AREAS`] order: the areas whose translation versions an HTTP ETag for that box
/// has to include. For a tile, give it the tile grown by its features' reach beyond the edge
/// ([`crate::mvt::tile_bounds`]).
pub fn areas_in(west: f64, south: f64, east: f64, north: f64) -> Vec<&'static str> {
    let mut found = [false; AREAS.len()];
    let mut add = |code: &str| {
        if let Some(i) = AREAS.iter().position(|a| *a == code) {
            found[i] = true;
        }
    };
    if west.is_finite() && south.is_finite() && east.is_finite() && north.is_finite() {
        let q = r(west, south, east, north);
        for (code, _, reach) in BOXES {
            if reach.iter().any(|p| p.intersect(&q).is_some()) {
                add(code);
            }
        }
        for p in FALLTHROUGH {
            let Some(c) = p.intersect(&q) else { continue };
            // The fallthrough is constant between its thresholds: one sample per open cell, open
            // edge and corner of the grid they cut the overlap into gives every value it takes.
            let xs = samples(c.w, c.e, &[-35.0, -30.0, -12.0, -1.7, 2.0]);
            let ys = samples(c.s, c.n, &[42.7, 43.4, 45.0, 49.8, 51.5]);
            for &x in &xs {
                for &y in &ys {
                    add(fallthrough(x, y));
                }
            }
        }
    }
    AREAS.iter().zip(found).filter(|(_, f)| *f).map(|(a, _)| *a).collect()
}

/// The ends of `lo..=hi`, the thresholds strictly inside it, and the midpoints between them.
fn samples(lo: f64, hi: f64, thresholds: &[f64]) -> Vec<f64> {
    let mut cuts = vec![lo];
    cuts.extend(thresholds.iter().copied().filter(|t| lo < *t && *t < hi));
    cuts.push(hi);
    let mut out = Vec::with_capacity(cuts.len() * 2);
    for pair in cuts.windows(2) {
        out.push(pair[0]);
        out.push((pair[0] + pair[1]) / 2.0);
    }
    out.push(hi);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The translation work's function, transcribed independently of the code above.
    fn python(lon: f64, lat: f64) -> &'static str {
        for (r, w, s, e, n) in [
            ("hk", 113.8, 22.1, 114.5, 22.6),
            ("sg", 103.55, 1.1, 104.2, 1.5),
            ("tw", 118.0, 21.8, 122.3, 26.5),
            ("jp", 122.5, 20.0, 154.5, 46.0),
        ] {
            if w <= lon && lon <= e && s <= lat && lat <= n {
                return r;
            }
        }
        if lon < -30.0 {
            return if lat < 45.0 && lon > -35.0 { "pt" } else { "na" };
        }
        if lon < -12.0 {
            return "pt";
        }
        if lat >= 49.8 && lon < 2.0 || lat >= 51.5 {
            return "gb";
        }
        if lat < 42.7 || (lon < -1.7 && lat < 43.4) {
            "ib"
        } else {
            "fr"
        }
    }

    #[test]
    fn agrees_with_the_function_wherever_it_answers() {
        // Every 0.1° over the world: an answer is always the function's.
        let mut answered = 0;
        for i in 0..=3600 {
            let lon = -180.0 + i as f64 * 0.1;
            for j in 0..=1700 {
                let lat = -85.0 + j as f64 * 0.1;
                if let Some(a) = area_at(lon, lat) {
                    assert_eq!(a, python(lon, lat), "{lon} {lat}");
                    answered += 1;
                }
            }
        }
        assert!(answered > 100_000, "{answered}");
    }

    #[test]
    fn reach_stays_out_of_the_boxes() {
        for p in FALLTHROUGH {
            for (code, bx, reach) in BOXES {
                assert!(p.intersect(bx).is_none(), "{p:?} touches {code}'s box");
                for q in *reach {
                    assert!(q.intersect(bx) == Some(*q), "{code}: {q:?} leaves its box");
                }
            }
            assert!(p.w < p.e && p.s < p.n, "{p:?}");
        }
    }

    /// (place, lon, lat, expected).
    const PLACES: &[(&str, f64, f64, Option<&str>)] = &[
        // Japan and its neighbours.
        ("Tokyo", 139.69, 35.69, Some("jp")),
        ("Sapporo", 141.35, 43.06, Some("jp")),
        ("Wakkanai", 141.67, 45.41, Some("jp")),
        ("Rishiri", 141.24, 45.18, Some("jp")),
        ("Hakodate", 140.73, 41.77, Some("jp")),
        ("Aomori", 140.74, 40.82, Some("jp")),
        ("Niigata", 139.02, 37.9, Some("jp")),
        ("Wajima", 136.9, 37.39, Some("jp")),
        ("Oki", 133.2, 36.2, Some("jp")),
        ("Fukuoka", 130.4, 33.59, Some("jp")),
        ("Nagasaki", 129.87, 32.75, Some("jp")),
        ("Goto", 128.84, 32.7, Some("jp")),
        ("Tsushima", 129.3, 34.4, Some("jp")),
        ("Naha", 127.68, 26.21, Some("jp")),
        ("Yonaguni", 122.99, 24.47, Some("jp")),
        ("Chichijima", 142.2, 27.07, Some("jp")),
        ("Torishima", 140.31, 30.48, Some("jp")),
        ("Minamitorishima", 153.98, 24.29, Some("jp")),
        ("Okinotorishima", 136.08, 20.42, Some("jp")),
        ("Kunashir", 145.9, 44.0, Some("jp")),
        ("Seoul", 126.98, 37.57, None),
        ("Busan", 129.03, 35.1, None),
        ("Jeju", 126.53, 33.5, None),
        ("Ulleungdo", 130.86, 37.5, None),
        ("Harbin", 126.6, 45.75, None),
        ("Shenyang", 123.43, 41.8, None),
        ("Vladivostok", 131.9, 43.1, None),
        ("Terney", 136.6, 45.05, None),
        ("Korsakov", 142.78, 46.64, None),
        ("Shanghai", 121.47, 31.23, None),
        ("Shengsi", 122.45, 30.72, None),
        ("Beijing", 116.4, 39.9, None),
        // Taiwan and Fujian.
        ("Taipei", 121.56, 25.04, Some("tw")),
        ("Kaohsiung", 120.3, 22.62, Some("tw")),
        ("Magong", 119.57, 23.57, Some("tw")),
        ("Lanyu", 121.55, 22.05, Some("tw")),
        ("Pengjia", 122.08, 25.63, Some("tw")),
        ("Kinmen", 118.32, 24.44, Some("tw")),
        ("Lieyu", 118.24, 24.43, Some("tw")),
        ("Dadan", 118.14, 24.38, Some("tw")),
        ("Nangan", 119.94, 26.16, Some("tw")),
        ("Dongyin", 120.49, 26.37, Some("tw")),
        ("Wuqiu", 119.45, 24.99, Some("tw")),
        ("Xiamen", 118.09, 24.48, None),
        ("Quanzhou", 118.6, 24.9, None),
        ("Putian", 119.0, 25.43, None),
        ("Pingtan", 119.79, 25.5, None),
        ("Fuzhou", 119.3, 26.07, None),
        ("Pratas", 116.72, 20.7, None),
        ("Qixingyan", 120.8, 21.76, None),
        // Hong Kong and Singapore.
        ("Central", 114.16, 22.28, Some("hk")),
        ("Futian", 114.05, 22.54, Some("hk")),
        ("Shenzhen north", 114.05, 22.65, None),
        ("Macau", 113.55, 22.19, None),
        ("Singapore", 103.85, 1.29, Some("sg")),
        ("Johor Bahru", 103.76, 1.46, Some("sg")),
        ("Pedra Branca", 104.41, 1.33, None),
        ("Kuala Lumpur", 101.69, 3.139, None),
        // Europe.
        ("Paris", 2.35, 48.86, Some("fr")),
        ("Lille", 3.06, 50.63, Some("fr")),
        ("Dunkirk", 2.37, 51.03, Some("fr")),
        ("Calais", 1.86, 50.95, Some("gb")),
        ("Dieppe", 1.08, 49.92, Some("gb")),
        ("Cherbourg", -1.62, 49.64, Some("fr")),
        ("St Helier", -2.1, 49.19, Some("fr")),
        ("St Peter Port", -2.54, 49.46, Some("fr")),
        ("Strasbourg", 7.75, 48.58, Some("fr")),
        ("Grosbliederstroff", 7.02, 49.16, Some("fr")),
        ("Montancy", 7.02, 47.36, Some("fr")),
        ("Bitche", 7.43, 49.05, Some("fr")),
        ("Kehl", 7.81, 48.57, Some("fr")),
        ("Basel", 7.59, 47.56, Some("fr")),
        ("Geneva", 6.14, 46.2, Some("fr")),
        ("Monaco", 7.42, 43.73, Some("fr")),
        ("Menton", 7.5, 43.77, Some("fr")),
        ("Bastia", 9.45, 42.7, Some("fr")),
        ("Ajaccio", 8.74, 41.92, Some("ib")),
        ("Capraia", 9.82, 43.05, Some("fr")),
        ("Santander", -3.8, 43.46, Some("fr")),
        ("Bilbao", -2.93, 43.26, Some("ib")),
        ("Madrid", -3.7, 40.42, Some("ib")),
        ("Lisbon", -9.14, 38.72, Some("ib")),
        ("Andorra la Vella", 1.52, 42.51, Some("ib")),
        ("Perpignan", 2.9, 42.7, Some("fr")),
        ("Gibraltar", -5.35, 36.14, Some("ib")),
        ("Ceuta", -5.32, 35.89, Some("ib")),
        ("Melilla", -2.94, 35.29, Some("ib")),
        ("Palma", 2.65, 39.57, Some("ib")),
        ("Mahón", 4.27, 39.89, Some("ib")),
        ("Funchal", -16.92, 32.65, Some("pt")),
        ("Selvagem Grande", -15.87, 30.14, Some("pt")),
        ("Ponta Delgada", -25.67, 37.74, Some("pt")),
        ("Flores", -31.2, 39.45, Some("pt")),
        ("London", -0.13, 51.51, Some("gb")),
        ("Dublin", -6.26, 53.35, Some("gb")),
        ("Douglas", -4.48, 54.15, Some("gb")),
        ("Lerwick", -1.14, 60.15, Some("gb")),
        ("St Kilda", -8.57, 57.81, Some("gb")),
        ("Rockall", -13.69, 57.6, Some("pt")),
        ("Brussels", 4.35, 50.85, None),
        ("Amsterdam", 4.9, 52.37, None),
        ("Westkapelle", 3.44, 51.53, None),
        ("Blighbank", 2.81, 51.63, Some("gb")),
        ("Cologne", 6.96, 50.94, None),
        ("Frankfurt", 8.68, 50.11, None),
        ("Berlin", 13.4, 52.52, None),
        ("Munich", 11.58, 48.14, None),
        ("Bern", 7.45, 46.95, None),
        ("Zürich", 8.54, 47.37, None),
        ("Turin", 7.68, 45.07, None),
        ("Sanremo", 7.78, 43.82, None),
        ("Milan", 9.19, 45.46, None),
        ("Oslo", 10.75, 59.91, None),
        ("Tórshavn", -6.77, 62.01, None),
        ("Reykjavik", -21.9, 64.15, None),
        ("Las Palmas", -15.43, 28.1, None),
        ("Tangier", -5.81, 35.77, None),
        ("Algiers", 3.06, 36.75, None),
        ("Dakar", -17.45, 14.7, None),
        ("Cape Town", 18.42, -33.92, None),
        ("Moscow", 37.62, 55.76, None),
        // North America.
        ("Montreal", -73.57, 45.5, Some("na")),
        ("Toronto", -79.38, 43.65, Some("na")),
        ("Windsor", -83.02, 42.3, Some("na")),
        ("Pelee Island", -82.65, 41.76, Some("na")),
        ("Thunder Bay", -89.25, 48.38, Some("na")),
        ("Sault Ste. Marie", -84.33, 46.52, Some("na")),
        ("Vancouver", -123.12, 49.28, Some("na")),
        ("Victoria", -123.37, 48.43, Some("na")),
        ("Haida Gwaii", -132.0, 53.25, Some("na")),
        ("Whitehorse", -135.06, 60.72, Some("na")),
        ("Iqaluit", -68.52, 63.75, Some("na")),
        ("Alert", -62.35, 82.5, Some("na")),
        ("Grise Fiord", -82.9, 76.42, Some("na")),
        ("St. John's", -52.71, 47.56, Some("na")),
        ("Saint-Pierre", -56.18, 46.78, Some("na")),
        ("Halifax", -63.57, 44.65, Some("na")),
        ("New York", -74.0, 40.71, Some("na")),
        ("Staten Island", -74.15, 40.5, Some("na")),
        ("Montauk", -71.95, 41.04, Some("na")),
        ("Nantucket", -70.1, 41.28, Some("na")),
        ("Boston", -71.06, 42.36, Some("na")),
        ("Buffalo", -78.88, 42.89, Some("na")),
        ("Detroit", -83.05, 42.33, Some("na")),
        ("Juneau", -134.42, 58.3, None),
        ("Skagway", -135.31, 59.45, Some("na")),
        ("Hyder", -130.02, 55.92, Some("na")),
        ("Lava Forks", -130.9, 56.44, Some("na")),
        ("Telegraph Creek", -131.16, 57.9, Some("na")),
        ("Atlin", -133.7, 59.58, Some("na")),
        ("Tatshenshini", -137.5, 59.6, Some("na")),
        ("Bowie Seamount", -135.86, 53.46, Some("na")),
        ("Endeavour Vents", -129.08, 47.96, Some("na")),
        ("Wrangell", -132.38, 56.47, None),
        ("Petersburg", -132.95, 56.81, None),
        ("Sitka", -135.33, 57.05, None),
        ("Ketchikan", -131.64, 55.34, None),
        ("Anchorage", -149.9, 61.22, None),
        ("Seattle", -122.33, 47.61, None),
        ("Minneapolis", -93.27, 44.98, None),
        ("Chicago", -87.63, 41.88, None),
        ("Cleveland", -81.69, 41.5, None),
        ("Pittsburgh", -80.0, 40.44, None),
        ("Philadelphia", -75.17, 39.95, None),
        ("Denver", -104.99, 39.74, None),
        ("Nuuk", -51.72, 64.18, None),
        ("Qaanaaq", -69.23, 77.47, None),
        ("Bermuda", -64.75, 32.3, None),
        ("Mexico City", -99.13, 19.43, None),
        ("Honolulu", -157.86, 21.31, None),
        ("Rio de Janeiro", -43.2, -22.9, None),
        ("Sydney", 151.21, -33.87, None),
    ];

    #[test]
    fn places() {
        for (place, lon, lat, want) in PLACES {
            assert_eq!(area_at(*lon, *lat), *want, "{place}");
        }
    }

    #[test]
    fn edges_of_the_function() {
        // Its own thresholds, inside the reach: closed boxes, open fallthrough comparisons.
        assert_eq!(area_at(114.5, 22.6), Some("hk"));
        assert_eq!(area_at(113.8, 22.1), Some("hk"));
        assert_eq!(area_at(103.55, 1.1), Some("sg"));
        assert_eq!(area_at(104.2, 1.5), Some("sg"));
        assert_eq!(area_at(122.3, 22.0), Some("tw"));
        assert_eq!(area_at(121.0, 21.8), Some("tw"));
        assert_eq!(area_at(121.0, 21.799), None);
        assert_eq!(area_at(122.5, 24.5), Some("jp"));
        assert_eq!(area_at(122.499, 24.5), None);
        assert_eq!(area_at(142.0, 46.0), Some("jp"));
        assert_eq!(area_at(142.0, 46.001), None);
        assert_eq!(area_at(-35.0, 39.0), None); // "na" by the function, far from Canada
        assert_eq!(area_at(-30.0, 39.0), Some("pt"));
        assert_eq!(area_at(-30.0001, 39.0), Some("pt"));
        assert_eq!(area_at(-31.0, 40.5), Some("pt"));
        assert_eq!(area_at(-11.0, 57.5), Some("gb"));
        assert_eq!(area_at(-12.0, 57.5), None); // "gb" by the function, between Ireland and Rockall
        assert_eq!(area_at(-11.5, 40.0), Some("ib"));
        assert_eq!(area_at(1.999, 49.8), Some("gb"));
        assert_eq!(area_at(1.999, 49.799), Some("fr"));
        assert_eq!(area_at(2.0, 49.8), Some("fr"));
        assert_eq!(area_at(2.5, 51.0), Some("fr"));
        assert_eq!(area_at(1.0, 51.5), Some("gb"));
        assert_eq!(area_at(2.0, 42.7), Some("fr"));
        assert_eq!(area_at(2.0, 42.699), Some("ib"));
        assert_eq!(area_at(-1.7, 43.0), Some("fr"));
        assert_eq!(area_at(-1.701, 43.0), Some("ib"));
        assert_eq!(area_at(-3.0, 43.4), Some("fr"));
        assert_eq!(area_at(-3.0, 43.399), Some("ib"));
        assert_eq!(area_at(-56.0, 45.0), Some("na"));
        assert_eq!(area_at(-56.0, 44.999), Some("na"));
        assert_eq!(area_at(f64::NAN, 40.0), None);
        assert_eq!(area_at(2.0, f64::INFINITY), None);
    }

    #[test]
    fn edges_of_the_reach() {
        // Each rectangle answers on its edge and stops just past it where nothing else continues.
        for q in FALLTHROUGH {
            for (lon, lat) in [(q.w, q.s), (q.e, q.n), ((q.w + q.e) / 2.0, q.s), (q.w, (q.s + q.n) / 2.0)] {
                assert_eq!(area_at(lon, lat), Some(fallthrough(lon, lat)), "{q:?} at {lon} {lat}");
            }
        }
        for (code, _, reach) in BOXES {
            for q in *reach {
                assert_eq!(area_at(q.w, q.s), Some(*code));
                assert_eq!(area_at(q.e, q.n), Some(*code));
            }
        }
        let outside = [
            (-11.501, 40.0),  // west of Portugal
            (-5.0, 35.899),   // Morocco's coast east of Ceuta
            (3.0, 37.199),    // off Algiers
            (4.601, 39.5),    // east of Menorca
            (8.401, 48.5),    // Baden, east of the Rhine strip
            (7.001, 46.001),  // Valais
            (7.251, 45.5),    // the Aosta valley
            (7.751, 43.8),    // Liguria
            (9.901, 43.0),    // east of Capraia
            (7.99, 42.0),     // west of Corsica
            (2.001, 55.0),    // the North Sea
            (-5.0, 61.101),   // north of Shetland
            (-24.599, 38.0),  // east of the Azores
            (-15.599, 31.0),  // east of Madeira
            (-100.0, 48.799), // North Dakota
            (-122.699, 48.5), // Bellingham
            (-131.0, 56.0),   // Misty Fjords
            (-133.5, 57.5),   // Petersburg's islands
            (-141.501, 65.0), // Alaska
            (-60.999, 70.0),  // Baffin Bay toward Greenland
            (-70.999, 77.0),  // north-western Greenland
            (-59.999, 82.0),  // northern Greenland
            (-88.5, 47.0),    // Michigan's Upper Peninsula
            (-83.601, 45.0),  // Michigan's Lower Peninsula
            (-83.201, 42.0),  // Toledo
            (-80.5, 41.599),  // Ohio
            (-75.201, 41.0),  // Pennsylvania
            (-75.0, 40.299),  // Trenton
            (-69.499, 41.0),  // east of Nantucket
        ];
        for (lon, lat) in outside {
            assert_eq!(area_at(lon, lat), None, "{lon} {lat}");
        }
    }

    #[test]
    fn areas_in_boxes() {
        // A tile over Calais holds both sides of 49.8° N (west of 2° E).
        assert_eq!(areas_in(1.4, 49.5, 2.2, 51.2), vec!["fr", "gb"]);
        // The Pyrenees' Basque end: ib south of 43.4° N west of 1.7° W, fr elsewhere.
        assert_eq!(areas_in(-2.0, 43.0, -1.0, 43.6), vec!["fr", "ib"]);
        // A box touching 1.7° W only on its edge still holds fr there.
        assert_eq!(areas_in(-2.0, 43.0, -1.7, 43.2), vec!["fr", "ib"]);
        assert_eq!(areas_in(-2.0, 43.0, -1.71, 43.2), vec!["ib"]);
        // Tokyo, and the world.
        assert_eq!(areas_in(139.0, 35.0, 140.0, 36.0), vec!["jp"]);
        assert_eq!(areas_in(-180.0, -85.0, 180.0, 85.0), AREAS.to_vec());
        // Nowhere near.
        assert!(areas_in(10.0, -10.0, 20.0, 0.0).is_empty());
        assert!(areas_in(f64::NAN, 0.0, 1.0, 1.0).is_empty());
        // Exhaustive against sampling, over boxes along the thresholds.
        for (w, s, e, n) in [(-36.0, 44.0, -29.0, 46.0), (-13.0, 49.0, 3.0, 52.0), (113.0, 21.0, 123.0, 27.0)] {
            let got = areas_in(w, s, e, n);
            let mut seen = Vec::new();
            for i in 0..=200 {
                for j in 0..=200 {
                    let lon = w + (e - w) * i as f64 / 200.0;
                    let lat = s + (n - s) * j as f64 / 200.0;
                    if let Some(a) = area_at(lon, lat) {
                        if !seen.contains(&a) {
                            seen.push(a);
                        }
                    }
                }
            }
            for a in &seen {
                assert!(got.contains(a), "{a} missing from {got:?} for {w} {s} {e} {n}");
            }
        }
    }
}
