use super::*;
use std::path::PathBuf;

/// numpy's own headers (`np.save` of zeros, numpy 2.5.3), for the chunks kept on the NAS.
#[test]
fn npy_as_numpy_writes_it() {
    let head = |h: usize, w: usize| {
        let b = npy(&vec![0; h * w], h, w);
        b[..b.len() - h * w].to_vec()
    };
    let pad = |s: &str, n: usize| format!("{s}{}\n", " ".repeat(n));
    // (Each header 118 bytes after its length, the dict padded with spaces to a newline.)
    let want = |dict: &str| {
        let mut v = b"\x93NUMPY\x01\x00v\x00".to_vec();
        v.extend(pad(dict, 117 - dict.len()).into_bytes());
        v
    };
    assert_eq!(head(4100, 7500), want("{'descr': '|u1', 'fortran_order': False, 'shape': (4100, 7500), }"));
    assert_eq!(head(1, 1), want("{'descr': '|u1', 'fortran_order': False, 'shape': (1, 1), }"));
    assert_eq!(head(820, 1500), want("{'descr': '|u1', 'fortran_order': False, 'shape': (820, 1500), }"));
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.npy");
    let a: Vec<u8> = (0..6).collect();
    keep(&p, &npy(&a, 2, 3)).unwrap();
    assert_eq!(read_npy(&p, 2, 3), Some(a));
    assert_eq!(read_npy(&p, 3, 2), None, "another shape");
    std::fs::write(&p, &npy(&[1, 2, 3], 2, 3)[..]).unwrap();
    assert_eq!(read_npy(&p, 2, 3), None, "cut short");
}

/// A square's GeoTIFF reads back as written, whole, laid out as GDAL lays it out: the first
/// directory at byte 8 (no metadata, zero tile arrays), the tiles from byte 13202 in order, the
/// last directory at the next even byte, pointed at by the header.
#[test]
fn square_tiff_as_gdal_lays_it_out() {
    let n = 1100;
    let a: Vec<u8> = (0..n * n).map(|i| ((i * 7 + i / n) % 5) as u8).collect();
    let b =
        tiff::square(&a, n, -120.0, 50.0, RES, &[("source", "x"), ("classes", CLASSES), ("complete", "1")]).unwrap();
    assert!(crate::whole::tiff_bytes_whole(&b));
    let t = crate::geotiff::Tiff::open(Arc::new(b.clone())).unwrap();
    assert_eq!(read_u8(&t, 0, 0, n, n).unwrap(), a);
    assert_eq!(t.transform(0).unwrap(), [-120.0, RES, 0.0, 50.0, 0.0, -RES]);
    assert_eq!(t.nodata(), Some(255.0));
    assert_eq!(t.metadata_item("complete"), Some("1"));
    assert_eq!(t.metadata_item("source"), Some("x"));
    assert_eq!(u16::from_le_bytes([b[8], b[9]]), 19);
    let first_tile = 8 + 2 + 19 * 12 + 4 + 2 * 4 * 9 + 24 + 48 + 64 + 16 + 8;
    assert_eq!(b[first_tile..first_tile + 2], [0x78, 0x9c], "a zlib stream at {first_tile}");
    let at = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    assert_eq!(at % 2, 0);
    assert_eq!(u16::from_le_bytes([b[at], b[at + 1]]), 20);
    let xml = String::from_utf8_lossy(&b[b.len() - 200..]).into_owned();
    assert!(xml.ends_with("<Item name=\"complete\">1</Item>\n  <Item name=\"source\">x</Item>\n</GDALMetadata>\n\0"));
}

/// PROJ's forward (pyproj's numbers through rasterio's `transform`, the same pipeline run the
/// other way: within a nanometre).
#[test]
fn laea_as_proj_computes_it() {
    let l = warp::Laea::new(45.0, -100.0).unwrap();
    for (lon, lat, x, y) in [
        (-100.0, 45.0, 0.0, -1.8936780655940106e-11),
        (-90.0, 50.0, 715316.8848106978, 600770.9796309897),
        (-150.25, 61.125, -2478142.1684093056, 2666578.544416679),
        (-60.5, 20.0005, 4070178.4040986123, -1838265.6152338798),
        (-179.9995, 70.5, -2278003.5988176605, 4320314.352726639),
    ] {
        let (gx, gy) = l.forward(lon, lat).unwrap();
        assert!((gx - x).abs() < 1e-6 && (gy - y).abs() < 1e-6, "{lon},{lat}: {gx},{gy} not {x},{y}");
    }
    assert!(l.forward(0.0, 91.0).is_none());
}

/// The chunks tile the square, each within GDAL's memory; the approximating transformer within its
/// error of the exact one; a warped chunk takes the pixel each point falls in.
#[test]
fn chunks_and_rows() {
    // NALCMS's grid.
    let src = [-4410000.0, 30.0, 0.0, 4310000.0, 0.0, -30.0];
    let t =
        warp::Transform::new(warp::Laea::new(45.0, -100.0).unwrap(), [-110.0, RES, 0.0, 50.0, 0.0, -RES], src).unwrap();
    let w = warp::Warp { t, src_w: 255000, src_h: 259000 };
    let chunks = w.chunks(N as i32, N as i32).unwrap();
    assert!(chunks.len() > 1);
    let area: i64 = chunks.iter().map(|c| c.dst.2 as i64 * c.dst.3 as i64).sum();
    assert_eq!(area, (N * N) as i64);
    for c in &chunks {
        let mem = (9.0 * c.src.2 as f64 * c.src.3 as f64 + 9.0 * c.dst.2 as f64 * c.dst.3 as f64) / 8.0;
        assert!(mem <= 64.0 * 1024.0 * 1024.0, "{c:?}");
    }
    let n = 5000;
    let mut xs: Vec<f64> = (0..n).map(|i| i as f64 + 0.5).collect();
    let mut ys = vec![7.5; n];
    let (mut ex, mut ey) = (xs.clone(), ys.clone());
    let mut ok = vec![false; n];
    w.t.approx(&mut xs, &mut ys, &mut ok);
    let mut eok = vec![false; n];
    for i in 0..n {
        w.t.approx(&mut ex[i..=i], &mut ey[i..=i], &mut eok[i..=i]);
        assert!(ok[i] && eok[i]);
        assert!((xs[i] - ex[i]).abs() + (ys[i] - ey[i]).abs() <= 0.125 * 1.01, "{i}");
    }
    // A chunk over a source of distinct values: each pixel the one its point falls in, 0 never
    // applied, 255 as 254.
    let c = chunks[0];
    let (sw, sh) = (c.src.2 as usize, c.src.3 as usize);
    let s: Vec<u8> = (0..sw * sh).map(|i| (i % 256) as u8).collect();
    let mut o = vec![255u8; c.dst.2 as usize * c.dst.3 as usize];
    w.warp_chunk(&c, &s, &mut o);
    assert!(!o.contains(&255) || s.contains(&0));
    assert!(o.iter().all(|&v| v != 0));
}

/// A square from chunks kept on the NAS (the EEA asked nothing): `.none` ones no data, an `.npy`
/// one as kept; the parts gone once it's saved, the square complete.
#[test]
fn europe_square_from_kept_chunks() {
    crate::fetch::go_offline();
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("leaf");
    let src = LeafSources {
        europe: "https://example.invalid/exportImage".into(),
        europe_box: [-40.0, 20.0, 40.0, 75.0],
        north_america: "https://example.invalid/nalcms.zip".into(),
        north_america_member: [0, 0],
        north_america_box: [-180.0, 14.0, -50.0, 84.0],
    };
    let parts = out.join("parts").join("lat50_lon0");
    std::fs::create_dir_all(&parts).unwrap();
    for r0 in (0..N).step_by(MAX_H) {
        for c0 in (0..N).step_by(MAX_W) {
            if (r0, c0) == (0, 0) {
                let (h, w) = (MAX_H, MAX_W);
                let a: Vec<u8> = (0..h * w).map(|i| (i % 4) as u8).collect();
                keep(&parts.join("0-0.npy"), &npy(&a, h, w)).unwrap();
            } else {
                std::fs::write(parts.join(format!("{r0}-{c0}.none")), b"").unwrap();
            }
        }
    }
    let said = std::sync::Mutex::new(Vec::new());
    europe_square(50, 0, &out, &src, &|f| said.lock().unwrap().push(f)).unwrap();
    let p = out.join("lat50_lon0.tif");
    assert!(crate::trees::squares::complete(&p));
    assert!(!parts.exists());
    let t = crate::geotiff::Tiff::open(Arc::new(store::range::PlainFile::open(&p).unwrap())).unwrap();
    // (3 is no class of the EEA's: no data.)
    assert_eq!(read_u8(&t, 0, 0, 4, 1).unwrap(), [0, 1, 2, 255]);
    assert_eq!(read_u8(&t, MAX_W, 0, 2, 1).unwrap(), [255, 255]);
    let said = said.into_inner().unwrap();
    assert_eq!(said.len(), 15);
    assert_eq!(said[14], 15.0 / 16.0);
}

/// Byte identity with leaftype.py, by hand on the Mac mini: the squares `LT_SQUARES`
/// ("top,left;…") made into `LT_OUT` (a scratch folder whose parent holds `nalcms-2020.tif`, or a
/// link to the NAS's) from the leaf-type sources in `LT_SOURCES` (a trees run's sources.json, or
/// its `leaf` alone), each compared with `LT_WANT`'s (the Python's, made the same way).
#[test]
#[ignore]
fn real_squares() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} isn't set"));
    let out = PathBuf::from(env("LT_OUT"));
    let want = PathBuf::from(env("LT_WANT"));
    let runs: serde_json::Value = serde_json::from_slice(&std::fs::read(env("LT_SOURCES")).unwrap()).unwrap();
    let leaf: LeafSources = serde_json::from_value(runs["leaf"].clone()).unwrap();
    let sqs: Vec<(i32, i32)> = env("LT_SQUARES")
        .split(';')
        .map(|s| {
            let (t, l) = s.split_once(',').unwrap();
            (t.parse().unwrap(), l.parse().unwrap())
        })
        .collect();
    make(&sqs, &out, &leaf).unwrap();
    for &(t, l) in &sqs {
        let f = format!("{}.tif", name(t, l));
        let (a, b) = (std::fs::read(out.join(&f)).unwrap(), std::fs::read(want.join(&f)).unwrap());
        eprintln!("{f}: {} bytes, {}", a.len(), if a == b { "the same" } else { "DIFFERENT" });
        assert!(a == b, "{f} differs");
    }
}
