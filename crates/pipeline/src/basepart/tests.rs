use super::*;
use crate::hipack::{self, tile_bounds};
use crate::legacy::TileId;
use roadcore::packs::{RailRel, RoadRec, Sub9};
use roadcore::scenic::ch;

/// A base pack of unit 6/11/20 beside tile T = 6/10/20, its ways sorted west to east by their
/// first vertex, some reaching into T, in `dir`: (base, roads) paths.
fn make(dir: &Path, n: usize) -> (PathBuf, PathBuf) {
    let own = tile_bounds(6, 11, 20);
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rnd = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m.max(1)
    };
    let (w, h) = ((own[2] - own[0]) as u64, (own[3] - own[1]) as u64);
    let mut starts: Vec<[i32; 2]> = (0..n).map(|_| [own[0] + rnd(w) as i32, own[1] + rnd(h) as i32]).collect();
    starts.sort_unstable();
    let (mut ways, mut verts, mut samples, mut roads) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut rail = Vec::new();
    for (i, s) in starts.iter().enumerate() {
        let vc = 2 + rnd(30) as u32;
        // (Some of the ways near the west edge run west into T, far.)
        let step = if s[0] < own[0] + (w / 20) as i32 && rnd(3) == 0 { -2_000_000 } else { 3_000 };
        let vstart = verts.len() as u64;
        for k in 0..vc as i32 {
            verts.push([s[0] + k * step, s[1] + (rnd(2000) as i32 - 1000)]);
        }
        let class = if i % 17 == 0 { roadcore::class::COMMUTER } else { 1 + (i % 8) as u8 };
        ways.push(WayRec {
            id: 1000 + i as i64 * 3,
            vstart,
            vcount: vc,
            name: (i % 3) as u32,
            ref_: 0,
            surface: (i % 2) as u32,
            maxspeed: 50,
            class,
            flags: 0,
            lanes: 2,
            network: 0,
            rail: 0,
            _pad: 0,
            route: 0,
            colour: 0,
        });
        if roadcore::class::is_rail(class) {
            rail.push(RailRel { way: i as u32, rel_lo: i as u32, rel_hi: 0, _pad: 0 });
        }
        for k in 0..rnd(4) as usize {
            let p = verts[vstart as usize + k.min(vc as usize - 1)];
            samples.push(Sample {
                way: i as u32,
                dist: k as f32 * 100.0,
                lon: p[0],
                lat: p[1],
                eye: 1.5,
                flags: 0,
                _pad: [0; 3],
            });
        }
        roads.push(RoadRec {
            road: (i / 4) as u64,
            len: 1000.0,
            offset: (i % 4) as f32 * 250.0,
            dir: (i % 2) as u8,
            _pad: [0; 7],
        });
    }
    let nv = verts.len();
    let ext = verts.iter().fold(EMPTY, |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
    let sub9 = [Sub9 { key: 1, first: 0, count: n as u32 }];
    let elevu: Vec<u16> = (0..nv).map(|i| 5000 + (i % 300) as u16).collect();
    let raw: Vec<f32> = (0..nv).map(|i| i as f32).collect();
    let grade: Vec<u8> = (0..nv).map(|i| (i % 7) as u8).collect();
    let scenic: Vec<[u8; ch::N]> = (0..nv).map(|i| [(i % 251) as u8; ch::N]).collect();
    let drape: Vec<i16> = (0..nv).map(|i| (i % 50) as i16).collect();
    let samplech: Vec<[u8; ch::N]> = (0..samples.len()).map(|i| [(i % 13) as u8; ch::N]).collect();
    let base = dir.join("6-11-20.0000000000000001.sect");
    let meta = serde_json::json!({"fmt": 1, "unit": "6/11/20", "ways": n, "verts": nv, "samples": samples.len(), "extent": ext});
    let mut wr = store::sect::SectWriter::create(&base, meta).unwrap();
    let b = |x: &[u8]| x.to_vec();
    for (name, bytes) in [
        ("ways", b(bytemuck::cast_slice(&ways))),
        ("verts", b(bytemuck::cast_slice(&verts))),
        ("elevu", b(bytemuck::cast_slice(&elevu))),
        ("raw", b(bytemuck::cast_slice(&raw))),
        ("grade", grade),
        ("src", vec![1u8; nv]),
        ("strings", b"\nMain St\nHigh St\n".to_vec()),
        ("samples", b(bytemuck::cast_slice(&samples))),
        ("samplech", b(bytemuck::cast_slice(&samplech))),
        ("sub9", b(bytemuck::cast_slice(&sub9))),
        ("rail", b(bytemuck::cast_slice(&rail))),
        ("scenic", b(bytemuck::cast_slice(&scenic))),
        ("drape", b(bytemuck::cast_slice(&drape))),
    ] {
        wr.add(name, &bytes).unwrap();
    }
    wr.finish().unwrap();
    let rp = dir.join("6-11-20.0000000000000002.sect");
    let mut wr = store::sect::SectWriter::create(&rp, serde_json::json!({})).unwrap();
    wr.add("roads", bytemuck::cast_slice(&roads)).unwrap();
    wr.finish().unwrap();
    (base, rp)
}

/// Where a test's part read is copied, in folder `d`.
fn to(d: &Path) -> [PathBuf; 2] {
    [d.join("base.sect"), d.join("roads.sect")]
}

fn refs(p: &[PathBuf; 2]) -> [&Path; 2] {
    [&p[0], &p[1]]
}

/// What a pack job makes of tile `t` from `bp`: its ways, road and rail tiles and hidata, as bytes.
fn made(bp: &BasePack, t: TileId) -> Vec<Vec<u8>> {
    made_of(&[bp], t)
}

/// `made`, of several packs.
fn made_of(packs: &[&BasePack], t: TileId) -> Vec<Vec<u8>> {
    let tb = tile_bounds(t.z, t.x, t.y);
    let packs: Vec<&BasePack> = packs.iter().copied().filter(|bp| hipack::meets(bp.extent, tb)).collect();
    let packs = &packs[..];
    let in_t = hipack::ways_in(packs, tb).unwrap();
    let mut out = vec![in_t
        .iter()
        .flat_map(|s| [s.way.to_le_bytes().to_vec(), bytemuck::cast_slice(&s.bbox).to_vec()])
        .flatten()
        .collect()];
    let win = hipack::way_inputs(packs, &in_t).unwrap();
    let (roads, rails) = hipack::tiles(&win, 6, t.x, t.y, 9..=11, false);
    for e in roads.iter().chain(&rails) {
        out.push([e.key.to_le_bytes().to_vec(), e.gz.clone()].concat());
    }
    let hd = hipack::hidata(t, packs, &in_t).unwrap();
    out.push(bytemuck::cast_slice(&hd.here).to_vec());
    out.push(bytemuck::cast_slice(&hd.ends).to_vec());
    out.push(bytemuck::cast_slice(&hd.parts).to_vec());
    out.push(bytemuck::cast_slice(&hd.psamples).to_vec());
    out.push(bytemuck::cast_slice(&hd.pch).to_vec());
    out.push(bytemuck::cast_slice(&hd.railinfo).to_vec());
    out.push(hd.railstr.clone());
    out
}

/// A neighbour's base pack read only where tile T's ways are: what's made of T is byte-identical
/// to what the whole pack makes, from a small part of the file; the way boxes kept, the next
/// read smaller still; read for every tile, the whole pack.
#[test]
fn a_part_read_makes_the_same_bytes() {
    let d = tempfile::tempdir().unwrap();
    let (base, roads) = make(d.path(), 20 * CHUNK + 77);
    let t = TileId { z: 6, x: 10, y: 20 };
    let whole = BasePack::open(&base, &roads).unwrap();
    let want = made(&whole, t);
    assert!(want[0].len() > 100, "some ways reach T");
    let size = std::fs::metadata(&base).unwrap().len() + std::fs::metadata(&roads).unwrap().len();
    let boxes = index_path(&d.path().join("cache"), "base/6-11-20.0000000000000001.sect");
    let tb = [tile_bounds(6, 10, 20)];
    let mut reads = Vec::new();
    for k in 0..2 {
        let s = d.path().join(format!("s{k}"));
        std::fs::create_dir_all(&s).unwrap();
        let (part, read) = open(&base, &roads, &boxes, refs(&to(&s)), Some(&tb)).unwrap();
        assert_eq!(made(&part, t), want, "read {k}");
        assert_eq!(part.bytes(), whole.bytes(), "the files' sizes, as their measure");
        reads.push(read.bytes);
    }
    assert!(boxes.exists());
    assert!(reads[0] < size * 2 / 3, "the first read, the index worked out: {} of {size}", reads[0]);
    assert!(reads[1] < size / 3, "with the index kept: {} of {size}", reads[1]);
    // (Every chunk: the whole pack, the same bytes for its own tile too.)
    let s = d.path().join("all");
    std::fs::create_dir_all(&s).unwrap();
    let (all, _) = open(&base, &roads, &boxes, refs(&to(&s)), None).unwrap();
    let own = TileId { z: 6, x: 11, y: 20 };
    assert_eq!(made(&all, own), made(&whole, own));
    assert_eq!(made(&all, t), want);
}

/// Way boxes kept for another chunking, or cut short, are worked out again.
#[test]
fn bad_boxes_are_worked_out_again() {
    let d = tempfile::tempdir().unwrap();
    let (base, roads) = make(d.path(), 3 * CHUNK);
    let boxes = d.path().join("b.boxes");
    std::fs::write(&boxes, b"WAYIDX02short").unwrap();
    let t = TileId { z: 6, x: 10, y: 20 };
    let s = d.path().join("s");
    std::fs::create_dir_all(&s).unwrap();
    let (part, _) = open(&base, &roads, &boxes, refs(&to(&s)), Some(&[tile_bounds(6, 10, 20)])).unwrap();
    assert_eq!(made(&part, t), made(&BasePack::open(&base, &roads).unwrap(), t));
    let kept = std::fs::read(&boxes).unwrap();
    assert_eq!(kept.len(), 16 + 3 * 16 + 4 * 8 + 8, "the index kept in its place");
    // (Damaged: its checksum doesn't hold, and it's worked out again.)
    let mut bad = kept.clone();
    bad[20] ^= 1;
    assert!(Index::decode(&bad, 3).is_none());
    std::fs::remove_file(&boxes).unwrap();
    std::fs::write(&boxes, &bad).unwrap();
    let (part, _) = open(&base, &roads, &boxes, refs(&to(&s)), Some(&[tile_bounds(6, 10, 20)])).unwrap();
    assert_eq!(made(&part, t), made(&BasePack::open(&base, &roads).unwrap(), t));
    assert_eq!(std::fs::read(&boxes).unwrap(), kept);
}

/// A read that brings no byte for its stall time is given up (Task #380: a stuck SMB read), its
/// thread left to it; one that keeps bringing bytes runs on past it; a quick one's result is its own.
#[test]
fn a_stuck_read_is_given_up() {
    let stall = std::time::Duration::from_millis(300);
    let t0 = std::time::Instant::now();
    let e = watched(stall, "read x", |_| -> Result<()> {
        std::thread::sleep(std::time::Duration::from_secs(60));
        Ok(())
    })
    .unwrap_err();
    store::clock::ended_before(t0, std::time::Duration::from_secs(60), "given up at its stall time");
    assert!(format!("{e:#}").contains("no byte read"), "{e:#}");
    let slow = watched(stall, "read y", |p| {
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            p.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(7)
    });
    assert_eq!(slow.unwrap(), 7);
    assert!(watched(stall, "read z", |_| -> Result<()> { anyhow::bail!("gone") }).is_err());
}
