//! The zoomed-out query summaries (docs/phase5.md "Zoomed-out queries"): a z6 tile's long roads
//! and all its rail as bins of their query samples, which pack(T) writes into hidata (`lparts`,
//! `lbins`; rail `lrparts`, `lrbins`) and the tests build from older hidata.
//!
//! A bin is consecutive samples of one part. It closes `BIN_M` after its first sample, where the
//! drive filters' attributes change (roads: class, unpaved, toll, unnamed), and for rail at every
//! way. Parts already end at gaps (hipack: over 300 m).

use crate::class;
use crate::flag;
use crate::packs::{here_extra, Here, LBin, LPart, PSample, Part, RailInfo};
use crate::scenic::{self, ch};

/// The sections' format (hidata meta `lsum`).
pub const LSUM_V: u32 = 1;
/// A bin's longest span along the road (from its first sample), metres.
pub const BIN_M: f32 = 500.0;
/// Roads shorter than this aren't summarised: the shortest window answered from summaries.
pub const LO_MIN_ROAD: f32 = 2000.0;
/// `LBin::rinfo` of a bin without a `railinfo` row.
pub const NO_RINFO: u32 = u32::MAX;

#[derive(Default)]
pub struct Summaries {
    pub lparts: Vec<LPart>,
    pub lbins: Vec<LBin>,
    pub lrparts: Vec<LPart>,
    pub lrbins: Vec<LBin>,
}

/// The drive filters' attributes of a way: class, and the bits of `LBin::flags`.
pub fn attrs(h: &Here) -> (u8, u8) {
    let mut f = 0u8;
    if h.flags & flag::UNPAVED != 0 {
        f |= 1;
    }
    if h.flags & flag::TOLL != 0 {
        f |= 2;
    }
    if h.extra & here_extra::UNNAMED != 0 {
        f |= 4;
    }
    (h.class, f)
}

fn q8(v: f64) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// A tile's summaries from its query parts (`railinfo` sorted by `here`, as pack(T) writes it).
pub fn build(parts: &[Part], ps: &[PSample], pch: &[[u8; ch::N]], here: &[Here], railinfo: &[RailInfo]) -> Summaries {
    let mut out = Summaries::default();
    for p in parts {
        let rail = class::is_rail(p.class);
        if !rail && p.road_len < LO_MIN_ROAD {
            continue;
        }
        let (a, b) = (p.first as usize, (p.first + p.count) as usize);
        let (smp, chs) = (&ps[a..b], &pch[a..b]);
        if smp.is_empty() {
            continue;
        }
        let (lparts, lbins) = if rail { (&mut out.lrparts, &mut out.lrbins) } else { (&mut out.lparts, &mut out.lbins) };
        let first_bin = lbins.len();
        // What closes a bin besides its length: a rail bin's way, a road bin's attributes.
        let key = |k: usize| -> (u32, u8, u8) {
            let w = smp[k].way;
            if rail {
                (w, 0, 0)
            } else {
                let (c, f) = attrs(&here[w as usize]);
                (0, c, f)
            }
        };
        let mut i0 = 0usize;
        for k in 1..=smp.len() {
            if k < smp.len() && smp[k].offset - smp[i0].offset < BIN_M && key(k) == key(i0) {
                continue;
            }
            lbins.push(bin(rail, i0, k, smp, chs, here, railinfo));
            i0 = k;
        }
        lparts.push(LPart { road: p.road, first: first_bin as u32, count: (lbins.len() - first_bin) as u32, road_len: p.road_len, _pad: 0 });
    }
    out
}

/// The bin of samples `i0..k` of a part.
fn bin(rail: bool, i0: usize, k: usize, smp: &[PSample], chs: &[[u8; ch::N]], here: &[Here], railinfo: &[RailInfo]) -> LBin {
    let n = k - i0;
    let mid = i0 + n / 2;
    let mut sum = [0f64; 12];
    for t in i0..k {
        if rail {
            // The grade from the neighbours within the part (one-sided at its ends).
            let (a, b) = (t.saturating_sub(1), (t + 1).min(smp.len() - 1));
            let g = scenic::grade(smp[a].eye, smp[a].offset, smp[b].eye, smp[b].offset);
            let c = scenic::ride_components(&chs[t], smp[t].eye, smp[t].flags, g);
            for (s, v) in sum.iter_mut().zip(c) {
                *s += v as f64;
            }
        } else {
            for (s, v) in sum.iter_mut().zip(scenic::drive_components(&chs[t])) {
                *s += v as f64;
            }
        }
    }
    let mut comp = [0u8; 12];
    for (c, s) in comp.iter_mut().zip(sum) {
        *c = q8(s / n as f64);
    }
    let hw = smp[mid].way as usize;
    let (class, flags) = attrs(&here[smp[i0].way as usize]);
    let rinfo = if rail {
        railinfo.binary_search_by_key(&smp[i0].way, |r| r.here).map_or(NO_RINFO, |i| i as u32)
    } else {
        NO_RINFO
    };
    LBin {
        way: here[hw].id,
        off0: smp[i0].offset,
        len: smp[k - 1].offset - smp[i0].offset,
        lon0: smp[i0].lon,
        lat0: smp[i0].lat,
        lonm: smp[mid].lon,
        latm: smp[mid].lat,
        lon1: smp[k - 1].lon,
        lat1: smp[k - 1].lat,
        rinfo,
        n: n as u16,
        class,
        flags: if rail { 0 } else { flags },
        comp,
        _pad: [0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here(id: u64, class: u8, flags: u8) -> Here {
        Here { id, owner: 0, index: 0, class, flags, extra: 0, _pad: 0, bbox: [0; 4] }
    }

    fn smp(way: u32, off: f32) -> PSample {
        PSample { way, offset: off, lon: (off * 10.0) as i32, lat: 0, eye: 100.0, flags: 0, _pad: [0; 3] }
    }

    #[test]
    fn bins_close_at_500_m_attributes_and_rail_ways() {
        // A 2.5 km road, 100 m samples, toll from 1,200 m.
        let hs = vec![here(10, class::SECONDARY, 0), here(11, class::SECONDARY, flag::TOLL), here(20, class::TRAM, 0), here(21, class::TRAM, 0)];
        let mut ps: Vec<PSample> = (0..26).map(|i| smp(if i < 12 { 0 } else { 1 }, i as f32 * 100.0)).collect();
        // A rail part: two ways.
        ps.extend((0..6).map(|i| smp(if i < 4 { 2 } else { 3 }, i as f32 * 100.0)));
        let pch = vec![[0u8; ch::N]; ps.len()];
        let parts = vec![
            Part { road: 1, offset: 0.0, first: 0, count: 26, road_len: 2500.0, class: class::SECONDARY, flags: 0, _pad: [0; 6] },
            Part { road: 2, offset: 0.0, first: 26, count: 6, road_len: 500.0, class: class::TRAM, flags: 0, _pad: [0; 6] },
        ];
        let ri = vec![RailInfo { here: 3, colour: 0, rel: 7, name: 0, route: 0, rail: 1, class: class::TRAM, _pad: [0; 6] }];
        let s = build(&parts, &ps, &pch, &hs, &ri);
        // Road: 0–400, 500–900, 1000–1100 (toll from 1200), 1200–1600, 1700–2100, 2200–2500.
        let spans: Vec<(f32, f32, u16)> = s.lbins.iter().map(|b| (b.off0, b.off0 + b.len, b.n)).collect();
        assert_eq!(spans, vec![(0.0, 400.0, 5), (500.0, 900.0, 5), (1000.0, 1100.0, 2), (1200.0, 1600.0, 5), (1700.0, 2100.0, 5), (2200.0, 2500.0, 4)]);
        assert_eq!(s.lbins[2].flags, 0);
        assert_eq!(s.lbins[3].flags, 2);
        assert_eq!(s.lparts.len(), 1);
        assert_eq!((s.lparts[0].first, s.lparts[0].count), (0, 6));
        // Rail: one bin per way, the second with its railinfo row.
        assert_eq!(s.lrbins.len(), 2);
        assert_eq!((s.lrbins[0].way, s.lrbins[0].n, s.lrbins[0].rinfo), (20, 4, NO_RINFO));
        assert_eq!((s.lrbins[1].way, s.lrbins[1].n, s.lrbins[1].rinfo), (21, 2, 0));
        // Positions: first, middle, last samples.
        assert_eq!((s.lbins[0].lon0, s.lbins[0].lonm, s.lbins[0].lon1), (0, 2000, 4000));
    }

    #[test]
    fn short_roads_are_left_out_but_not_rail() {
        let hs = vec![here(10, class::RESIDENTIAL, 0), here(20, class::INTERCITY, 0)];
        let ps: Vec<PSample> = (0..4).map(|i| smp(if i < 2 { 0 } else { 1 }, (i % 2) as f32 * 100.0)).collect();
        let pch = vec![[0u8; ch::N]; 4];
        let parts = vec![
            Part { road: 1, offset: 0.0, first: 0, count: 2, road_len: 1999.0, class: class::RESIDENTIAL, flags: 0, _pad: [0; 6] },
            Part { road: 2, offset: 0.0, first: 2, count: 2, road_len: 150.0, class: class::INTERCITY, flags: 0, _pad: [0; 6] },
        ];
        let s = build(&parts, &ps, &pch, &hs, &[]);
        assert!(s.lparts.is_empty() && s.lbins.is_empty());
        assert_eq!(s.lrparts.len(), 1);
    }
}
