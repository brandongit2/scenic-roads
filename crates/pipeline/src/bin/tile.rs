//! Post-process sampled elevations and cut the road tile pyramid.
//!
//! usage: tile <build_dir> [minzoom] [maxzoom]
//!        tile <build_dir> elev      processed elevations only (final.u16, grade.u8), which the
//!                                   scenic stage needs before the tiles can be cut
//!
//! Elevations are post-processed first (see `pipeline::elev`).
//!
//! Tiles: per zoom, combined planar + elevation Douglas-Peucker, clipped exactly at tile
//! edges (round caps make the seams invisible), encoded with `roadcore::tile`.

use anyhow::Result;
use pipeline::tiling::{self, WayIn};
use roadcore::archive::ArchiveWriter;
use roadcore::elev::{self, Elevs};
use roadcore::tile::{TileLine, NCH};
use roadcore::{class, dist_m, Array, Ways, E7};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("data/build"));
    let elev_only = args.get(2).map(String::as_str) == Some("elev");
    let minz: u8 = if elev_only { 4 } else { args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(4) };
    let maxz: u8 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(14);
    let t0 = std::time::Instant::now();

    let wv = Ways::open(&dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let raw = Array::<f32>::open(&dir.join("elev.f32"))?;
    // Scenic inputs (optional: absent before the scenic stage has run).
    let drape_a = Array::<i16>::open(&dir.join("vterrain.i16")).ok();
    let sc_a = Array::<[u8; NCH]>::open(&dir.join("scenic.u8")).ok();
    let drape_all: Option<&[i16]> = drape_a.as_ref().map(|a| a.get()).filter(|a| a.len() == verts.len());
    let sc_all: Option<&[[u8; NCH]]> = sc_a.as_ref().map(|a| a.get()).filter(|a| a.len() == verts.len());
    eprintln!("drape heights: {}, scenic channels: {}", drape_all.is_some(), sc_all.is_some());
    let raw = raw.get();
    eprintln!("{} ways, {} vertices", ways.len(), verts.len());

    let strings = roadcore::read_strings(&dir)?;
    let net = pipeline::elev::Net::build(ways, verts);
    let p = pipeline::elev::process(&net, raw);
    let du: Vec<u16> = p.elev.iter().map(|&e| elev::to_u16((e * 10.0).round() as i32)).collect();
    if elev_only {
        std::fs::write(roadcore::tmp(&dir, "final.u16"), bytemuck::cast_slice(&du))?;
        std::fs::write(roadcore::tmp(&dir, "grade.u8"), &p.grade)?;
        roadcore::commit(&dir, &["final.u16", "grade.u8"])?;
        // (A folder made before final.u16: its readers would take the new file anyway.)
        std::fs::remove_file(dir.join("final.i16")).ok();
        eprintln!("elevations processed ({:.0?})", t0.elapsed());
        return Ok(());
    }
    let cl = pipeline::climbs::find(&net, &p.elev);
    std::fs::write(roadcore::tmp(&dir, "climbs.bin"), bytemuck::cast_slice(&cl.recs))?;
    std::fs::write(roadcore::tmp(&dir, "climbs.geom"), bytemuck::cast_slice(&cl.geom))?;
    // Strokes (chained ways) for the server's scenic-drive ranking: offsets + items, where an
    // item is a way index with the top bit set when traversed in reverse.
    let mut offs: Vec<u32> = vec![0];
    let mut items: Vec<u32> = Vec::new();
    for st in &cl.strokes {
        items.extend(st.iter().map(|&(w, r)| w | if r { 1 << 31 } else { 0 }));
        offs.push(items.len() as u32);
    }
    std::fs::write(roadcore::tmp(&dir, "strokes.off"), bytemuck::cast_slice(&offs))?;
    std::fs::write(roadcore::tmp(&dir, "strokes.u32"), bytemuck::cast_slice(&items))?;
    drop(cl);
    // Whole-road lengths, for the length filter (per line in the tiles, per way for the server).
    let road_len = pipeline::roads::lengths(&net, &strings);
    std::fs::write(roadcore::tmp(&dir, "roadlen.f32"), bytemuck::cast_slice(&road_len))?;
    drop(net);
    std::fs::write(roadcore::tmp(&dir, "final.u16"), bytemuck::cast_slice(&du))?;
    std::fs::write(roadcore::tmp(&dir, "grade.u8"), &p.grade)?;
    eprintln!("elevations processed ({:.0?})", t0.elapsed());

    // Global stats for the metadata (length-weighted elevation histogram, 10 m bins).
    let mut hist = vec![0f64; 256];
    let (mut emin, mut emax) = (f32::MAX, f32::MIN);
    for w in ways.iter().filter(|w| !class::is_rail(w.class)) {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let v = &verts[r.clone()];
        let e = &p.elev[r];
        for k in 1..v.len() {
            let l = dist_m(v[k - 1][0] as f64 * E7, v[k - 1][1] as f64 * E7, v[k][0] as f64 * E7, v[k][1] as f64 * E7);
            let m = (e[k - 1] + e[k]) * 0.5;
            hist[((m / 10.0).max(0.0) as usize).min(255)] += l / 1000.0;
        }
        for &x in e {
            emin = emin.min(x);
            emax = emax.max(x);
        }
    }

    // Rail track length per service group (a track counts for every group using it).
    let mut rail_km = [0f64; 5];
    for w in ways.iter().filter(|w| class::is_rail(w.class)) {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let l: f64 = verts[r].windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum();
        for (k, km) in rail_km.iter_mut().enumerate() {
            if w.rail >> k & 1 == 1 {
                *km += l;
            }
        }
    }

    // ---- tiles ------------------------------------------------------------------------
    let mut aw = ArchiveWriter::create(&roadcore::tmp(&dir, "roads.tiles"), "{}")?;
    let mut rw = ArchiveWriter::create(&roadcore::tmp(&dir, "rails.tiles"), "{}")?;
    let mut zoom_stats = Vec::new();
    let mut rail_stats = Vec::new();
    let (mut bminx, mut bminy, mut bmaxx, mut bmaxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for v in verts.iter().step_by(97) {
        let (x, y) = (v[0] as f64 * E7, v[1] as f64 * E7);
        bminx = bminx.min(x);
        bmaxx = bmaxx.max(x);
        bminy = bminy.min(y);
        bmaxy = bmaxy.max(y);
    }
    let win: Vec<WayIn> = ways
        .iter()
        .enumerate()
        .map(|(wi, w)| {
            let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
            WayIn {
                rec: w,
                id: w.id as u32,
                verts: &verts[r.clone()],
                elev_dm: Elevs::U16(&du[r.clone()]),
                grade: &p.grade[r.clone()],
                drape: drape_all.map(|d| &d[r.clone()]),
                sc: sc_all.map(|a| &a[r.clone()]),
                surface: strings[w.surface as usize].as_str(),
                road_m: road_len[wi].round() as u32,
            }
        })
        .collect();
    for z in minz..=maxz {
        let pieces = tiling::cut(z, &win, &|_, _| true, true);
        // Roads and rail go to their own archives.
        let (rail_pieces, road_pieces): (Vec<(u64, TileLine)>, Vec<(u64, TileLine)>) =
            pieces.into_iter().partition(|p| class::is_rail(p.1.style & 0x0f));
        for (pcs, w, what, stats) in [(&road_pieces, &mut aw, "roads", &mut zoom_stats), (&rail_pieces, &mut rw, "rails", &mut rail_stats)] {
            let enc = tiling::encode_zoom(z, maxz, pcs, Some(what));
            for e in &enc {
                let (tz, tx, ty) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                w.add(tz, tx, ty, &e.gz, e.raw_len)?;
            }
            stats.push(tiling::zoom_stats(z, what, &enc, t0));
        }
    }
    aw.finish()?;
    rw.finish()?;

    let dem_stats = std::fs::read_to_string(dir.join("dem-stats.json")).unwrap_or_else(|_| "{}".into());
    let hist_s: Vec<String> = hist.iter().map(|v| format!("{:.1}", v)).collect();
    let meta = format!(
        "{{\"minzoom\":{minz},\"maxzoom\":{maxz},\
         \"bounds\":[{bminx:.4},{bminy:.4},{bmaxx:.4},{bmaxy:.4}],\"ways\":{},\"vertices\":{},\
         \"elev_min\":{emin:.1},\"elev_max\":{emax:.1},\"elev_hist_10m_km\":[{}],\
         \"classes\":{:?},\"zooms\":{{{}}},\"rail_zooms\":{{{}}},\"rail_km\":[{}],\"dem\":{},\"built\":{}}}",
        ways.len(),
        verts.len(),
        hist_s.join(","),
        class::NAMES,
        zoom_stats.join(","),
        rail_stats.join(","),
        rail_km.iter().map(|v| format!("{:.1}", v / 1000.0)).collect::<Vec<_>>().join(","),
        dem_stats.trim(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()
    );
    std::fs::write(roadcore::tmp(&dir, "roads.json"), meta)?;
    // final.u16 / grade.u8 are usually what `tile elev` wrote already: a rewrite would make the
    // scenic steps that read them look out of date.
    roadcore::commit_if_changed(&dir, &["final.u16", "grade.u8"])?;
    std::fs::remove_file(dir.join("final.i16")).ok();
    roadcore::commit(&dir, &["climbs.bin", "climbs.geom", "strokes.off", "strokes.u32", "roadlen.f32", "roads.tiles", "rails.tiles", "roads.json"])?;
    eprintln!("done in {:.0?}", t0.elapsed());
    Ok(())
}
