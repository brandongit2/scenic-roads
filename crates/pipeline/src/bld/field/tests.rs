use super::*;
use crate::bld::e7;

/// A square footprint (E7) of `half` degrees around (lon, lat), wound `ccw` or not.
fn square(lon: f64, lat: f64, half: f64, ccw: bool) -> Vec<[i32; 2]> {
    let mut r = vec![[e7(lon - half), e7(lat - half)], [e7(lon + half), e7(lat - half)], [e7(lon + half), e7(lat + half)], [e7(lon - half), e7(lat + half)]];
    if !ccw {
        r.reverse();
    }
    r
}

/// A texel's area (m²) at level `z`'s row `y` of the world.
fn texel_m2(z: u8, y: f64) -> f64 {
    let n = (1u64 << z) as f64 * T as f64;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan();
    (crate::bld::EQ * lat.cos() / n).powi(2)
}

/// Decodes a field3-webp tile: its mask, and each class held's texels (r, g, b).
fn decode(b: &[u8]) -> (u8, Vec<Vec<[u8; 3]>>) {
    let (mask, webp, _) = split(b).unwrap();
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).unwrap();
    let (w, h) = d.dimensions();
    assert_eq!((w as usize, h as usize), (T, T * mask.count_ones() as usize));
    let mut buf = vec![0u8; d.output_buffer_size().unwrap()];
    d.read_image(&mut buf).unwrap();
    let px = if d.has_alpha() { 4 } else { 3 };
    let all: Vec<[u8; 3]> = buf.chunks(px).map(|c| [c[0], c[1], c[2]]).collect();
    (mask, all.chunks(T * T).map(|c| c.to_vec()).collect())
}

fn feet_of(rings: &[(Vec<[i32; 2]>, u16, u16)]) -> Vec<Foot<'_>> {
    rings.iter().map(|(r, h, m)| Foot { polys: vec![vec![r.as_slice()]], h: *h, m: *m }).collect()
}

/// Area 8/129/88's buildings: a grid of houses of both windings, a 60 m tower, a 150 m one on a
/// 30 m base, and a house across the area's west edge.
fn town() -> Vec<(Vec<[i32; 2]>, u16, u16)> {
    let mut b = Vec::new();
    for i in 0..40 {
        for j in 0..30 {
            b.push((square(2.35 + 0.0007 * i as f64, 48.86 + 0.0006 * j as f64, 0.0001, (i + j) % 2 == 0), 70 + (i * 7 + j) as u16 % 140, 0));
        }
    }
    b.push((square(2.40, 48.85, 0.0003, true), 600, 0));
    b.push((square(2.41, 48.85, 0.0002, true), 1500, 300));
    let edge = crate::bld::tile_box_deg(8, 129, 88)[0];
    b.push((square(edge, 48.87, 0.0002, true), 90, 0));
    b
}

#[test]
fn areas_and_tops_kept() {
    // One 15 m house, alone: Σ f × texel area is its footprint, v / f its top, Σ w × texel area its
    // walls (perimeter × height), at level 8 (means of level 14's).
    let r = square(2.3501, 48.8602, 0.0002, true);
    let rings = vec![(r.clone(), 150u16, 0u16)];
    let feet = feet_of(&rings);
    let a = (129, 88);
    let mut field = area(a, &feet, &[]);
    let mid = field.mid.take().unwrap();
    let (w0, n0) = world7(r[3]);
    let (w1, s1) = world7(r[1]);
    let cy = 0.5 * (n0 + s1) * WORLD_TEXELS;
    let m_per_world = side_m(cy) * WORLD_TEXELS;
    let fp = (w1 - w0) * (s1 - n0) * m_per_world * m_per_world;
    let per = 2.0 * ((w1 - w0) + (s1 - n0)) * m_per_world;
    let c = mid.c[class_of(150)].as_ref().unwrap();
    let (mut sf, mut sv, mut sw) = (0.0, 0.0, 0.0);
    for y in 0..T {
        let ta = texel_m2(8, (88 * T + y) as f64 + 0.5);
        for x in 0..T {
            sf += c[0][y * T + x] as f64 * ta;
            sv += c[2][y * T + x] as f64 * ta;
            sw += c[1][y * T + x] as f64 * ta;
        }
    }
    assert!((sf / fp - 1.0).abs() < 1e-3, "{sf} {fp}");
    assert!((sv / sf - 15.0).abs() < 1e-3, "{}", sv / sf);
    assert!((sw / (per * 15.0) - 1.0).abs() < 1e-3, "{sw} {}", per * 15.0);
    // Its tiles: levels 14–8, one each, the finest first.
    let levels: Vec<u8> = field.tiles.iter().map(|t| t.0).collect();
    assert_eq!(levels, [14, 13, 12, 11, 10, 9, 8]);
}

#[test]
fn the_same_bytes_in_any_order() {
    let rings = town();
    let feet = feet_of(&rings);
    let a = (129, 88);
    let her = heritage(a, &feet, &[[e7(2.40), e7(48.85)]]);
    assert!(!her.is_empty());
    let one = area(a, &feet, &her);
    // Reversed, and on one thread.
    let rev: Vec<(Vec<[i32; 2]>, u16, u16)> = rings.iter().rev().cloned().collect();
    let feet2 = feet_of(&rev);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let two = pool.install(|| area(a, &feet2, &her));
    assert_eq!(one, two);
    // The towers: in every level, the finest too.
    let l13: u8 = one.tiles.iter().filter(|t| t.0 == 13).map(|t| t.3[0]).fold(0, |m, k| m | k);
    let l12: u8 = one.tiles.iter().filter(|t| t.0 == 12).map(|t| t.3[0]).fold(0, |m, k| m | k);
    assert_eq!((l13 & 0b11000, l12 & 0b11000), (0b11000, 0b11000), "{l13:b} {l12:b}");
    // The house across the west edge: its part in the area counted, in the first column.
    let mid = one.mid.as_ref().unwrap();
    let k = class_of(90);
    let col: f32 = (0..T).map(|y| mid.c[k].as_ref().unwrap()[0][y * T]).sum();
    assert!(col > 0.0);
}

#[test]
fn encoding_round_trip() {
    let rings = town();
    let feet = feet_of(&rings);
    let a = (129, 88);
    let field = area(a, &feet, &[]);
    let mid = field.mid.as_ref().unwrap();
    let b = field.tiles.iter().find(|t| t.0 == 8).unwrap();
    let (mask, classes) = decode(&b.3);
    assert_eq!(mask, mid.mask());
    let mut ks = (0..K).filter(|k| mask & 1 << k != 0);
    for got in classes {
        let k = ks.next().unwrap();
        let c = mid.c[k].as_ref().unwrap();
        for (i, g) in got.iter().enumerate() {
            assert_eq!(*g, quantize(c, k, i), "class {k} texel {i}");
        }
    }
    // Decoded back, f is within √f's step.
    let c = mid.c[0].as_ref().unwrap();
    for i in 0..T * T {
        let q = quantize(c, 0, i);
        let f = (q[0] as f64 / 255.0).powi(2);
        assert!((f - c[0][i] as f64).abs() <= 2.0 * (c[0][i] as f64).sqrt() / 255.0 + 1e-5);
    }
    // Every tile decodes.
    for t in &field.tiles {
        decode(&t.3);
    }
}

#[test]
fn mids_and_lo() {
    let rings = town();
    let feet = feet_of(&rings);
    let a = (129, 88);
    let field = area(a, &feet, &[]);
    let mid = field.mid.clone().unwrap();
    let mut areas = BTreeMap::new();
    areas.insert(a, mid.clone());
    let bytes = write_mid(&areas).unwrap();
    assert_eq!(read_mid(&bytes).unwrap(), areas, "kept exactly");
    // Z3 tile 3/4/2 holds z6 6/32/22 (area 129/88's).
    let lo_tiles = lo((4, 2), &|x, y| Ok(((x, y) == (32, 22)).then(|| bytes.clone()))).unwrap();
    let got: Vec<(u8, u32, u32)> = lo_tiles.iter().map(|t| (t.0, t.1, t.2)).collect();
    assert_eq!(got, [(7, 64, 44), (6, 32, 22), (5, 16, 11), (4, 8, 5)]);
    // Level 7 is level 8's parent (8/129/88 the ne quadrant of 7/64/44).
    let p7 = Planes::parent([None, Some(&mid), None, None]).unwrap();
    assert_eq!(lo_tiles[0].3, encode(&p7, ALL_CLASSES, &[]).unwrap());
}

#[test]
fn heritage_lists_the_buildings_holding_a_site() {
    let rings = town();
    let feet = feet_of(&rings);
    let a = (129, 88);
    // Inside the 60 m tower, and in a street (no building).
    let b = heritage(a, &feet, &[[e7(2.4001), e7(48.8501)], [e7(2.35035), e7(48.86)]]);
    assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), 1);
    assert_eq!((u16::from_le_bytes([b[4], b[5]]), u16::from_le_bytes([b[6], b[7]]), b[8]), (600, 0, 4));
    // Carried after the level-8 tile's WebP.
    let field = area(a, &feet, &b);
    let t8 = field.tiles.iter().find(|t| t.0 == 8).unwrap();
    assert_eq!(split(&t8.3).unwrap().2, &b[..]);
    // A ring of 40 points simplified to 16 at most.
    let ring: Vec<[f64; 2]> = (0..40).map(|i| {
        let t = i as f64 / 40.0 * std::f64::consts::TAU;
        [1000.0 + 300.0 * t.cos(), 1000.0 + 300.0 * t.sin()]
    }).collect();
    assert!(simplify_to(&ring, HERITAGE_PTS).len() <= HERITAGE_PTS);
}

/// The WebP coding's regions tried on a built pack's tiles (BLDFIELD_PACK, BLDFIELD_SAMPLE every
/// n-th tile): bytes and time for each region size. By hand: cargo test --release … -- --ignored.
#[test]
#[ignore]
fn region_sizes_tried() {
    let Ok(p) = std::env::var("BLDFIELD_PACK") else { return };
    let every: usize = std::env::var("BLDFIELD_SAMPLE").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let b = std::fs::read(p).unwrap();
    let idx = store::pack::PackIndex::parse_bytes(&b).unwrap();
    let mut tot: BTreeMap<u32, (usize, f64)> = BTreeMap::new();
    for e in idx.entries.iter().step_by(every) {
        let blob = &b[e.offset as usize..e.offset as usize + e.len as usize];
        let (mask, classes) = decode(blob);
        let rgb: Vec<u8> = classes.iter().flatten().flatten().copied().collect();
        let h = T as u32 * mask.count_ones();
        for rb in [5u32, 6, 7, 8] {
            let t0 = std::time::Instant::now();
            let w = crate::webp::encode_rgb_regions(&rgb, T as u32, h, rb);
            let s = tot.entry(rb).or_default();
            s.0 += w.len();
            s.1 += t0.elapsed().as_secs_f64();
        }
    }
    eprintln!("{} tiles of {}: {tot:?}", idx.entries.len().div_ceil(every), idx.entries.len());
}

#[test]
fn a_courtyard_and_overlaps() {
    // A courtyard building (its hole wound either way) covers its outer square less the yard; two
    // copies of a house on each other cover it once inside, their mean top kept.
    let a = (129, 88);
    let sum = |rings: &[(Vec<Vec<[i32; 2]>>, u16)]| -> (f64, f64) {
        let feet: Vec<Foot> = rings.iter().map(|(rs, h)| Foot { polys: vec![rs.iter().map(Vec::as_slice).collect()], h: *h, m: 0 }).collect();
        let mid = area(a, &feet, &[]).mid.unwrap();
        let c = mid.c[0].as_ref().unwrap();
        let (mut f, mut v) = (0.0, 0.0);
        for i in 0..T * T {
            f += c[0][i] as f64;
            v += c[2][i] as f64;
        }
        (f, v)
    };
    let outer = square(2.36, 48.87, 0.0004, true);
    let full = sum(&[(vec![outer.clone()], 60)]).0;
    for ccw in [true, false] {
        let yard = sum(&[(vec![outer.clone(), square(2.36, 48.87, 0.0002, ccw)], 60)]).0;
        assert!((yard / full - 0.75).abs() < 2e-3, "{yard} {full}");
    }
    let one = sum(&[(vec![outer.clone()], 40)]);
    let two = sum(&[(vec![outer.clone()], 40), (vec![outer.clone()], 70)]);
    // (Inside, once; in the edge texels each counts until the texel is full: §4.8's known overlap.)
    assert!(two.0 / one.0 > 1.0 && two.0 / one.0 < 1.2 && (two.1 / two.0 - 5.5).abs() < 1e-3, "{one:?} {two:?}");
}
