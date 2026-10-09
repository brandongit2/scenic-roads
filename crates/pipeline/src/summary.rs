//! A unit's summary (its base pack's meta `summary`), added up over a catalog's units into the
//! map's meta (`Catalog::meta`): where the data is, its way and vertex counts, and road length by
//! elevation (what `tile` used to write as `roads.json` for the whole build).

use roadcore::elev::Elevs;
use roadcore::{class, dist_m, WayRec, E7};
use serde::{Deserialize, Serialize};

/// Elevation bands of 10 m.
pub const BANDS: usize = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// [west, south, east, north] of every vertex, E7 (inverted when empty).
    pub extent: [i32; 4],
    pub ways: u64,
    pub vertices: u64,
    /// Roads' (not rail) elevations, metres (inverted when there are none).
    pub elev_min: f32,
    pub elev_max: f32,
    /// Road (not rail) km by 10 m of elevation, at each segment's midpoint.
    pub hist: Vec<f64>,
    /// Rail track km per service group (`WayRec::rail` bits; a track counts for every group on it).
    pub rail_km: [f64; 5],
}

impl Default for Summary {
    fn default() -> Self {
        Summary { extent: [i32::MAX, i32::MAX, i32::MIN, i32::MIN], ways: 0, vertices: 0, elev_min: f32::MAX, elev_max: f32::MIN, hist: vec![0.0; BANDS], rail_km: [0.0; 5] }
    }
}

impl Summary {
    /// Of a unit's ways, their vertices and processed elevations (decimetres), as in its base pack.
    pub fn of(ways: &[WayRec], verts: &[[i32; 2]], elev_dm: Elevs) -> Summary {
        let mut s = Summary { ways: ways.len() as u64, vertices: verts.len() as u64, ..Default::default() };
        for p in verts {
            s.extent = [s.extent[0].min(p[0]), s.extent[1].min(p[1]), s.extent[2].max(p[0]), s.extent[3].max(p[1])];
        }
        for w in ways {
            let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
            let (v, e) = (&verts[r.clone()], elev_dm.slice(r));
            let seg = |k: usize| dist_m(v[k - 1][0] as f64 * E7, v[k - 1][1] as f64 * E7, v[k][0] as f64 * E7, v[k][1] as f64 * E7);
            if class::is_rail(w.class) {
                let l: f64 = (1..v.len()).map(seg).sum();
                for (k, km) in s.rail_km.iter_mut().enumerate() {
                    if w.rail >> k & 1 == 1 {
                        *km += l / 1000.0;
                    }
                }
                continue;
            }
            for k in 1..v.len() {
                let m = (e.dm(k - 1) as f32 + e.dm(k) as f32) * 0.05;
                s.hist[((m / 10.0).max(0.0) as usize).min(BANDS - 1)] += seg(k) / 1000.0;
            }
            for k in 0..e.len() {
                s.elev_min = s.elev_min.min(e.m(k));
                s.elev_max = s.elev_max.max(e.m(k));
            }
        }
        s
    }

    pub fn add(&mut self, o: &Summary) {
        self.extent = [self.extent[0].min(o.extent[0]), self.extent[1].min(o.extent[1]), self.extent[2].max(o.extent[2]), self.extent[3].max(o.extent[3])];
        self.ways += o.ways;
        self.vertices += o.vertices;
        self.elev_min = self.elev_min.min(o.elev_min);
        self.elev_max = self.elev_max.max(o.elev_max);
        for (a, b) in self.hist.iter_mut().zip(&o.hist) {
            *a += b;
        }
        for (a, b) in self.rail_km.iter_mut().zip(&o.rail_km) {
            *a += b;
        }
    }

    /// The map's meta: the fields the app reads.
    pub fn meta(&self) -> serde_json::Value {
        let deg = |v: i32| (v as f64 * E7 * 1e4).round() / 1e4;
        let r1 = |v: f64| (v * 10.0).round() / 10.0;
        let bounds = if self.vertices == 0 { [0.0; 4] } else { [deg(self.extent[0]), deg(self.extent[1]), deg(self.extent[2]), deg(self.extent[3])] };
        let finite = |v: f32| if v.is_finite() && v != f32::MAX && v != f32::MIN { (v * 10.0).round() / 10.0 } else { 0.0 };
        serde_json::json!({
            "minzoom": 4,
            "maxzoom": 14,
            "bounds": bounds,
            "ways": self.ways,
            "vertices": self.vertices,
            "elev_min": finite(self.elev_min),
            "elev_max": finite(self.elev_max),
            "elev_hist_10m_km": self.hist.iter().map(|&v| r1(v)).collect::<Vec<_>>(),
            "rail_km": self.rail_km.iter().map(|&v| r1(v)).collect::<Vec<_>>(),
            "classes": class::NAMES,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(vstart: u64, vcount: u32, cls: u8, rail: u8) -> WayRec {
        WayRec { vstart, vcount, class: cls, rail, ..bytemuck::Zeroable::zeroed() }
    }

    #[test]
    fn sums_road_length_by_elevation_and_rail_by_group() {
        // A road climbing from 5 m to 25 m over two segments; a rail track of group 1.
        let verts = [[0, 0], [0, 90_000], [0, 180_000], [100_000, 0], [100_000, 90_000]];
        let elev = [50i16, 150, 250, 0, 0];
        let ways = [way(0, 3, class::PRIMARY, 0), way(3, 2, class::INTERCITY, 0b10)];
        let s = Summary::of(&ways, &verts, Elevs::I16(&elev));
        let su: Vec<u16> = elev.iter().map(|&d| roadcore::elev::to_u16(d as i32)).collect();
        assert_eq!(serde_json::to_string(&Summary::of(&ways, &verts, Elevs::U16(&su))).unwrap(), serde_json::to_string(&s).unwrap());
        let seg = dist_m(0.0, 0.0, 0.0, 0.009);
        // Midpoints at 10 m and 20 m: bands 1 and 2.
        assert!((s.hist[1] - seg / 1000.0).abs() < 1e-9 && (s.hist[2] - seg / 1000.0).abs() < 1e-9);
        assert_eq!(s.hist.iter().filter(|&&v| v > 0.0).count(), 2);
        assert_eq!((s.elev_min, s.elev_max), (5.0, 25.0));
        assert!((s.rail_km[1] - seg / 1000.0).abs() < 1e-9 && s.rail_km[0] == 0.0);
        assert_eq!(s.extent, [0, 0, 100_000, 180_000]);
        let mut t = Summary::default();
        t.add(&s);
        t.add(&s);
        assert_eq!((t.ways, t.vertices, t.extent), (4, 10, s.extent));
        assert!((t.hist[1] - 2.0 * s.hist[1]).abs() < 1e-12);
        let m = t.meta();
        assert_eq!(m["bounds"], serde_json::json!([0.0, 0.0, 0.01, 0.018]));
        assert_eq!(m["elev_hist_10m_km"].as_array().unwrap().len(), BANDS);
        // Nothing at all: zero bounds, not inverted ones.
        assert_eq!(Summary::default().meta()["bounds"], serde_json::json!([0.0, 0.0, 0.0, 0.0]));
    }
}
