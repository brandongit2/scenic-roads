//! base(U) (docs/plan.md §6): one unit's base pack, from its OSM piece, by today's steps run on a
//! unit-sized build folder (`extract`, `dem/sample.py`, `tile … elev`, `scenic-metrics`), then the
//! same conversion as today's data (`legacy::base_sections`) for the ways the unit owns (first
//! vertex inside it) that touch the coverage, with the pass's worldwide road values.
//!
//! Before the expensive steps the folder is cut down to the ways that touch the coverage (whoever
//! owns them: a way just outside still gives the clean-up its junction context at shared nodes).
//! The global-source layers come from the catalog's packs (`stage`). Each unit has its own slice of
//! the per-vertex DEM cache (`dem/sample.py` keeps only the vertices of its last run).

use crate::coverage::Coverage;
use crate::legacy::Unit;
use anyhow::{bail, ensure, Context, Result};
use roadcore::WayRec;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Copies the DEM cache entries inside `b` (w, s, e, n, E7) from `src` (`dem-cache.*` files) to
/// `dst`. Keys are `(lon + 2³¹) << 32 | (lat + 2³¹)`, sorted, so a longitude range is contiguous.
pub fn dem_cache_slice(src: &Path, b: [i32; 4], dst: &Path) -> Result<usize> {
    std::fs::create_dir_all(dst)?;
    let open = |n: &str| roadcore::mmap(&src.join(format!("dem-cache.{n}")));
    let (km, em, sm) = match (open("keys.u64"), open("elev.f32"), open("src.u8")) {
        (Ok(k), Ok(e), Ok(s)) => (k, e, s),
        _ => {
            // No cache yet: an empty slice (sample.py samples everything).
            return Ok(0);
        }
    };
    let keys: &[u64] = bytemuck::cast_slice(&km[..]);
    let elev: &[f32] = bytemuck::cast_slice(&em[..]);
    let srcs: &[u8] = &sm[..];
    ensure!(keys.len() == elev.len() && keys.len() == srcs.len(), "DEM cache files out of step");
    let k = |lon: i32, lat: i32| (((lon as i64 + (1i64 << 31)) as u64) << 32) | ((lat as i64 + (1i64 << 31)) as u64);
    let lo = keys.partition_point(|&x| x < k(b[0], i32::MIN));
    let hi = keys.partition_point(|&x| x <= k(b[2], i32::MAX));
    let lat_of = |x: u64| ((x & 0xffff_ffff) as i64 - (1i64 << 31)) as i32;
    let (mut ok, mut oe, mut os) = (Vec::new(), Vec::new(), Vec::new());
    for i in lo..hi {
        let lat = lat_of(keys[i]);
        if lat >= b[1] && lat <= b[3] {
            ok.push(keys[i]);
            oe.push(elev[i]);
            os.push(srcs[i]);
        }
    }
    for (n, bytes) in [("keys.u64", bytemuck::cast_slice::<u64, u8>(&ok)), ("elev.f32", bytemuck::cast_slice(&oe)), ("src.u8", &os[..])] {
        let tmp = dst.join(format!("dem-cache.{n}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, dst.join(format!("dem-cache.{n}")))?;
    }
    Ok(ok.len())
}

/// Rewrites a build folder's `ways.bin` and `verts.bin` keeping the ways `keep` says (in order);
/// run right after `extract`, before any per-vertex array exists. Returns (ways, vertices) kept.
pub fn subset(dir: &Path, keep: impl Fn(&WayRec, &[[i32; 2]]) -> bool) -> Result<(usize, usize)> {
    let w = roadcore::Ways::open(dir)?;
    let (ways, verts) = (w.ways(), w.verts());
    let mut out_ways: Vec<WayRec> = Vec::new();
    let mut out_verts: Vec<[i32; 2]> = Vec::new();
    for r in ways {
        let vs = &verts[r.vstart as usize..(r.vstart + r.vcount as u64) as usize];
        if keep(r, vs) {
            let mut n = *r;
            n.vstart = out_verts.len() as u64;
            out_verts.extend_from_slice(vs);
            out_ways.push(n);
        }
    }
    let mut head = Vec::with_capacity(16);
    head.extend_from_slice(roadcore::WAYS_MAGIC);
    head.extend_from_slice(&(out_ways.len() as u64).to_le_bytes());
    let mut wb = head;
    wb.extend_from_slice(bytemuck::cast_slice(&out_ways));
    drop(w);
    std::fs::write(roadcore::tmp(dir, "ways.bin"), &wb)?;
    std::fs::write(roadcore::tmp(dir, "verts.bin"), bytemuck::cast_slice(&out_verts))?;
    roadcore::commit(dir, &["ways.bin", "verts.bin"])?;
    Ok((out_ways.len(), out_verts.len()))
}

/// Whether the unit with bounds `tb` (w, s, e, n, E7) owns a way starting at `p`.
pub fn owns(tb: [i32; 4], p: [i32; 2]) -> bool {
    p[0] >= tb[0] && p[0] < tb[2] && p[1] >= tb[1] && p[1] < tb[3]
}

/// Where a unit's steps find their programs and caches.
pub struct Tools {
    /// `extract`, `tile`, `scenic-metrics`.
    pub bin: PathBuf,
    /// The repository's `dem/` folder (run with `uv run python`).
    pub dem: PathBuf,
    /// Shared caches: `chm10/` (canopy 10° files) and `dem-cache.*` (per-vertex elevations).
    pub cache: PathBuf,
    /// Overture building boxes (`data/buildings`), when there are any.
    pub buildings: Option<PathBuf>,
    /// Densification spacing (m).
    pub spacing_m: u32,
}

fn run(mut c: Command, what: &str, log: &Path) -> Result<()> {
    let f = std::fs::File::options().create(true).append(true).open(log)?;
    let t = std::time::Instant::now();
    let st = c.stdout(f.try_clone()?).stderr(f).status().with_context(|| format!("start {what}"))?;
    if !st.success() {
        bail!("{what} failed ({st}); see {}", log.display());
    }
    eprintln!("  {what}: {:.0?}", t.elapsed());
    Ok(())
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub unit: String,
    pub piece_ways: usize,
    pub kept_ways: usize,
    pub kept_verts: usize,
    pub owned: usize,
    pub dem_cache: usize,
    pub staged: crate::stage::Staged,
    /// Heritage sites staged around the unit.
    pub heritage: usize,
}

/// Runs today's steps for unit `u` in `dir` from `piece`, with the coverage and the global-source
/// layers on the NAS (`src`). Leaves the build folder ready for conversion.
pub fn build_folder(u: Unit, piece: &Path, dir: &Path, cov: &Coverage, src: &crate::stage::Source, tools: &Tools, heritage: Option<&crate::stage::Heritage>) -> Result<Report> {
    std::fs::create_dir_all(dir)?;
    let log = dir.join("steps.log");
    let mut rep = Report { unit: u.slash(), ..Default::default() };
    // 1. Every way of the piece, densified.
    let mut c = Command::new(tools.bin.join("extract"));
    c.arg(dir).arg(tools.spacing_m.to_string()).arg(piece);
    run(c, "extract", &log)?;
    rep.piece_ways = roadcore::Ways::open(dir)?.ways().len();
    // 2. Only what touches the coverage goes on.
    let (kw, kv) = subset(dir, |_, vs| cov.touches(vs))?;
    (rep.kept_ways, rep.kept_verts) = (kw, kv);
    if kw == 0 {
        return Ok(rep);
    }
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    {
        let w = roadcore::Ways::open(dir)?;
        let verts = w.verts();
        rep.owned = w.ways().iter().filter(|r| owns(tb, verts[r.vstart as usize])).count();
    }
    // 3. Elevations: this unit's slice of the DEM cache, grown by the piece's buffer.
    let grow = (crate::osmpass::BUFFER_KM * 1.2 / 111.32 * 1e7) as i32;
    let slice = [tb[0] - 2 * grow, tb[1] - grow, tb[2] + 2 * grow, tb[3] + grow];
    rep.dem_cache = dem_cache_slice(&tools.cache, slice, &dir.join("dem-cache"))?;
    let mut c = Command::new("uv");
    c.current_dir(&tools.dem).args(["run", "python", "sample.py"]).arg(dir).arg("--cache").arg(dir.join("dem-cache"));
    run(c, "elevations (sample.py)", &log)?;
    // 4. The global-source layers the steps read, from the packs.
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    rep.staged = crate::stage::stage(src, b, dir)?;
    // Today's heritage sites around the unit (the flags step's `heritage.json`).
    if let Some(h) = heritage {
        rep.heritage = h.write_in(b, dir)?;
    }
    // Land cover the packs lack (new coverage): ESA WorldCover for those grid tiles only; the
    // rest stays as staged.
    if rep.staged.missing.get("class").copied().unwrap_or(0) > 0 {
        let mut c = Command::new("uv");
        c.current_dir(&tools.dem).args(["run", "python", "landcover.py"]).arg(dir).arg("--only").arg(dir.join("grid.class.missing.u32"));
        run(c, "land cover (landcover.py)", &log)?;
    }
    // 5. Clean-up and grade; road samples; canopy; views; buildings; flags.
    let mut c = Command::new(tools.bin.join("tile"));
    c.arg(dir).arg("elev");
    run(c, "clean-up and grade (tile elev)", &log)?;
    for step in ["prep", "canopy", "view"] {
        let mut c = Command::new(tools.bin.join("scenic-metrics"));
        c.arg(dir).arg(step).env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", dir.join("scache"));
        run(c, &format!("scenic {step}"), &log)?;
    }
    if let Some(bd) = &tools.buildings {
        let mut c = Command::new(tools.bin.join("scenic-metrics"));
        c.arg(dir).arg("buildings").arg(bd).env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", dir.join("scache"));
        run(c, "scenic buildings", &log)?;
    }
    let mut c = Command::new(tools.bin.join("scenic-metrics"));
    c.arg(dir).arg("flags").env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", dir.join("scache"));
    run(c, "scenic flags", &log)?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dem_slice_by_box() {
        let d = tempfile::tempdir().unwrap();
        let k = |lon: i32, lat: i32| (((lon as i64 + (1i64 << 31)) as u64) << 32) | ((lat as i64 + (1i64 << 31)) as u64);
        let mut pts = vec![(-10, -10), (-10, 5), (0, 0), (0, 20), (3, 3), (5, -1), (20, 0)];
        pts.sort_by_key(|&(a, b)| k(a, b));
        let keys: Vec<u64> = pts.iter().map(|&(a, b)| k(a, b)).collect();
        let elev: Vec<f32> = (0..pts.len()).map(|i| i as f32).collect();
        std::fs::write(d.path().join("dem-cache.keys.u64"), bytemuck::cast_slice(&keys)).unwrap();
        std::fs::write(d.path().join("dem-cache.elev.f32"), bytemuck::cast_slice(&elev)).unwrap();
        std::fs::write(d.path().join("dem-cache.src.u8"), vec![4u8; pts.len()]).unwrap();
        let n = dem_cache_slice(d.path(), [-5, -5, 10, 10], &d.path().join("s")).unwrap();
        // (0,0), (3,3), (5,-1)
        assert_eq!(n, 3);
        let got: Vec<u64> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("s/dem-cache.keys.u64")).unwrap());
        assert_eq!(got, vec![k(0, 0), k(3, 3), k(5, -1)]);
        // No cache: an empty slice.
        assert_eq!(dem_cache_slice(&d.path().join("none"), [0, 0, 1, 1], &d.path().join("t")).unwrap(), 0);
    }

    #[test]
    fn subset_rewrites_ways_and_verts() {
        let d = tempfile::tempdir().unwrap();
        let mk = |id: i64, vstart: u64, vcount: u32| WayRec { id, vstart, vcount, ..bytemuck::Zeroable::zeroed() };
        let ways = [mk(1, 0, 2), mk(2, 2, 3), mk(3, 5, 2)];
        let verts: Vec<[i32; 2]> = (0..7).map(|i| [i, i]).collect();
        let mut wb = roadcore::WAYS_MAGIC.to_vec();
        wb.extend_from_slice(&3u64.to_le_bytes());
        wb.extend_from_slice(bytemuck::cast_slice(&ways));
        std::fs::write(d.path().join("ways.bin"), wb).unwrap();
        std::fs::write(d.path().join("verts.bin"), bytemuck::cast_slice(&verts)).unwrap();
        let (n, v) = subset(d.path(), |w, _| w.id != 2).unwrap();
        assert_eq!((n, v), (2, 4));
        let w = roadcore::Ways::open(d.path()).unwrap();
        assert_eq!(w.ways().iter().map(|r| (r.id, r.vstart, r.vcount)).collect::<Vec<_>>(), vec![(1, 0, 2), (3, 2, 2)]);
        assert_eq!(w.verts(), &[[0, 0], [1, 1], [5, 5], [6, 6]]);
    }
}
