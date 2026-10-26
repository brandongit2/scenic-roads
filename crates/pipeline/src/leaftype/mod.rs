//! Dominant leaf type of forests, on the canopy files' 10° squares (for the tree cover layer).
//!
//! Classes (u8): 0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data. Written per 10°
//! square, at 0.0005° (~50 m): `<dir>/lat<top>_lon<left>.tif` (`tiff`: GDAL's bytes).
//!
//! Sources:
//! - Europe: Copernicus HRL Dominant Leaf Type 2018, 10 m (broadleaf / coniferous), read at
//!   0.0005° from the EEA's public image service (exportImage, nearest neighbour): a documented API
//!   (crate::fetch::Kind::Api), asked with the build's User-Agent through the job's proxy, a 429
//!   told to the budgets (crate::budget::seen) and backed off from.
//! - North America: NALCMS 2020 land cover, 30 m (CEC; NRCan, USGS, INEGI…): needleleaf forest →
//!   conifer, broadleaf deciduous → broadleaf, mixed forest → mixed. The GeoTIFF is streamed out of
//!   CEC's 3.9 GB zip by byte range, its size and CRC checked against the zip's, and kept beside the
//!   squares (in `<dir>`'s parent: the NAS's `sources/trees/`), so it's downloaded once; each square
//!   resampled from it as GDAL's warper did (`warp`).
//! - Hong Kong: none (no data).
//!
//! Where each source is, and the box it covers, are its declaration's (docs/inputs.md §6.9: the
//! `leaf-type-europe` and `leaf-type-north-america` roles: crate::trees::LeafSources).
//!
//! The trees job makes the squares its z3 tile needs (crate::trees): whole squares, tagged
//! complete (a square not tagged so, or not whole, is made again, keeping its chunks fetched
//! whole). An EEA square's chunks are kept on the NAS as they come (`parts/`, numpy's `.npy`), so a
//! square that fails part way asks again only for what it lacks.

mod tiff;
mod warp;

use crate::timings::{phase, Class};
use crate::trees::LeafSources;
use anyhow::{bail, ensure, Context, Result};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Degrees a pixel, and a square's side in pixels.
const RES: f64 = 0.0005;
const N: usize = 20000;
/// The image service's largest image.
const MAX_W: usize = 7500;
const MAX_H: usize = 4100;
const CLASSES: &str = "0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data";
const EEA_SOURCE: &str = "Copernicus HRL Dominant Leaf Type 2018 (EEA), 10 m, read at 0.0005°";
const NALCMS_SOURCE: &str = "NALCMS 2020 land cover 30 m (CEC), resampled to 0.0005°";

/// NALCMS's classes → ours (254: GDAL's 255 kept off the nodata, not forest).
fn na_class(v: u8) -> u8 {
    match v {
        1 | 2 => 2,
        3..=5 => 1,
        6 => 3,
        0 | 127 | 255 => 255,
        _ => 0,
    }
}

/// The EEA's classes → ours (anything but 0–2 no data).
fn eea_class(v: u8) -> u8 {
    if v <= 2 {
        v
    } else {
        255
    }
}

/// Whether boxes `a` and `b` (w, s, e, n) meet.
fn meets_box(a: [f64; 4], b: [f64; 4]) -> bool {
    b[0] < a[2] && b[2] > a[0] && b[1] < a[3] && b[3] > a[1]
}

fn square_box(top: i32, left: i32) -> [f64; 4] {
    [left as f64, (top - 10) as f64, (left + 10) as f64, top as f64]
}

fn name(top: i32, left: i32) -> String {
    format!("lat{top}_lon{left}")
}

/// The leaf-type squares among `sqs` (top, left) that `out` lacks whole: inside the European
/// source's box from it, inside the North American's from it (its GeoTIFF kept in `out`'s parent,
/// downloaded once); none elsewhere (no source). How many of the squares to make are done, of how
/// many, is said as they come (crate::agent::jobs::report_f; an EEA square counted by its chunks).
pub fn make(sqs: &[(i32, i32)], out: &Path, src: &LeafSources) -> Result<()> {
    let missing: Vec<(i32, i32)> = {
        let _p = phase("the squares made checked", Class::NasRead);
        sqs.iter()
            .copied()
            .filter(|&(t, l)| !crate::trees::squares::complete(&out.join(format!("{}.tif", name(t, l)))))
            .collect()
    };
    let eea: Vec<(i32, i32)> =
        missing.iter().copied().filter(|&(t, l)| meets_box(src.europe_box, square_box(t, l))).collect();
    let na: Vec<(i32, i32)> = missing
        .iter()
        .copied()
        .filter(|&(t, l)| {
            meets_box(src.north_america_box, square_box(t, l)) && !meets_box(src.europe_box, square_box(t, l))
        })
        .collect();
    let total = (eea.len() + na.len()) as u64;
    let said = |done: f64| {
        if total > 0 {
            crate::agent::jobs::report_f(done, total, "leaf-type squares");
        }
    };
    for (k, &(top, left)) in eea.iter().enumerate() {
        said(k as f64);
        europe_square(top, left, out, src, &|f| said(k as f64 + f))?;
    }
    said(eea.len() as f64);
    let store = out.parent().context("the leaf-type squares' folder has no parent")?;
    north_america_squares(&na, out, &store.join("nalcms-2020.tif"), src, &|d| said((eea.len() + d) as f64))
}

/// A square, tagged complete, written by a temporary name (this Mac's and the process's), flushed
/// and checked whole before it takes its name.
fn save(top: i32, left: i32, a: &[u8], source: &str, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out).with_context(|| out.display().to_string())?;
    let path = out.join(format!("{}.tif", name(top, left)));
    let tmp = out.join(format!("{}.{}.{}.tmp.tif", name(top, left), store::sys::host(), std::process::id()));
    let r = (|| -> Result<()> {
        let b = tiff::square(
            a,
            N,
            left as f64,
            top as f64,
            RES,
            &[("classes", CLASSES), ("complete", "1"), ("source", source)],
        )?;
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&b)?;
            f.sync_all()?;
        }
        ensure!(crate::whole::tiff_file_whole(&tmp), "{}: written short", tmp.display());
        store::whole::rename_over(&tmp, &path)?;
        Ok(())
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r.with_context(|| path.display().to_string())?;
    let mut counts = [0u64; 256];
    for &v in a {
        counts[v as usize] += 1;
    }
    let pct = |c: u64| format!("{:.1}%", c as f64 / a.len() as f64 * 100.0);
    eprintln!(
        "  {}: broadleaf {}, conifer {}, mixed {}, no data {}",
        name(top, left),
        pct(counts[1]),
        pct(counts[2]),
        pct(counts[3]),
        pct(counts[255])
    );
    Ok(())
}

/// Band 1 of `t` in window (`x0`, `y0`, `w`, `h`), 8-bit samples, row by row.
fn read_u8(t: &crate::geotiff::Tiff, x0: usize, y0: usize, w: usize, h: usize) -> Result<Vec<u8>> {
    let img = t.level(0)?;
    ensure!(x0 + w <= img.width as usize && y0 + h <= img.height as usize, "a window off the image");
    let (bw, bh) = (img.block_w as usize, img.block_h as usize);
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return Ok(out);
    }
    for by in y0 / bh..=(y0 + h - 1) / bh {
        for bx in x0 / bw..=(x0 + w - 1) / bw {
            let b = t.block(0, bx as u32, by as u32)?;
            let px = b.u8s().context("not 8-bit samples")?;
            let (bx0, by0) = (bx * bw, by * bh);
            let (cx0, cx1) = (x0.max(bx0), (x0 + w).min(bx0 + b.width));
            for y in y0.max(by0)..(y0 + h).min(by0 + b.height) {
                if cx0 >= cx1 {
                    continue;
                }
                let s = &px[(y - by0) * b.width + (cx0 - bx0)..][..cx1 - cx0];
                out[(y - y0) * w + (cx0 - x0)..][..cx1 - cx0].copy_from_slice(s);
            }
        }
    }
    Ok(out)
}

/// What asks the EEA's image service and CEC's server: the build's User-Agent, the job's proxy.
fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(timeout)
        .timeout_connect(Some(Duration::from_secs(30)))
        .user_agent(crate::fetch::USER_AGENT)
        .http_status_as_error(false)
        .proxy(ureq::Proxy::try_from_env())
        .max_redirects(0)
        .build()
        .into()
}

/// One exportImage request: its band 1 (`height` rows of `width`), None when it keeps failing (three
/// tries, 10, 20 and 30 s apart; a 429 told to the budgets and waited out longer).
fn eea_request(src: &LeafSources, bb: [f64; 4], width: usize, height: usize) -> Result<Option<Vec<u8>>> {
    use crate::pyjson::float_repr as r;
    let url = format!(
        "{}?bbox={},{},{},{}&bboxSR=4326&imageSR=4326&size={width},{height}&format=tiff&pixelType=U8\
         &interpolation=RSP_NearestNeighbor&f=image",
        src.europe,
        r(bb[0]),
        r(bb[1]),
        r(bb[2]),
        r(bb[3])
    );
    crate::fetch::online(&url)?;
    let ag = agent(Some(Duration::from_secs(240)));
    for attempt in 1..=3u64 {
        let mut wait = 10 * attempt;
        let got = ag.get(&url).call().map_err(anyhow::Error::from).and_then(|mut resp| {
            let status = resp.status().as_u16();
            let retry = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
            if status == 429 || retry.is_some() {
                crate::budget::seen(&url, status, retry.as_deref());
                wait = wait.max(retry.and_then(|r| r.trim().parse::<u64>().ok()).unwrap_or(45 * attempt));
            }
            ensure!(status < 400, "HTTP {status}");
            Ok(resp.body_mut().with_config().limit(1 << 30).read_to_vec()?)
        });
        if let Ok(b) = got {
            if b.starts_with(b"II") || b.starts_with(b"MM") {
                let a = crate::geotiff::Tiff::open(Arc::new(b)).and_then(|t| {
                    let img = t.level(0)?;
                    ensure!(
                        (img.width as usize, img.height as usize) == (width, height),
                        "a {}x{} image",
                        img.width,
                        img.height
                    );
                    read_u8(&t, 0, 0, width, height)
                });
                if let Ok(a) = a {
                    return Ok(Some(a));
                }
            }
        }
        std::thread::sleep(Duration::from_secs(wait));
    }
    Ok(None)
}

/// One chunk (`width` by `height` over `bb`); a request that keeps failing is split in four, and
/// any part that fails fails it all (None).
fn eea_chunk(src: &LeafSources, bb: [f64; 4], width: usize, height: usize) -> Result<Option<Vec<u8>>> {
    let a = eea_request(src, bb, width, height)?;
    if a.is_some() || width < 1000 || height < 1000 {
        return Ok(a);
    }
    let [w, s, e, n] = bb;
    let (hw, hh) = (width / 2, height / 2);
    let (xm, ym) = (w + hw as f64 * RES, n - hh as f64 * RES);
    let mut out = vec![255u8; width * height];
    for (r0, c0, h, wd, b) in [
        (0, 0, hh, hw, [w, ym, xm, n]),
        (0, hw, hh, width - hw, [xm, ym, e, n]),
        (hh, 0, height - hh, hw, [w, s, xm, ym]),
        (hh, hw, height - hh, width - hw, [xm, s, e, ym]),
    ] {
        let Some(part) = eea_chunk(src, b, wd, h)? else { return Ok(None) };
        for r in 0..h {
            out[(r0 + r) * width + c0..][..wd].copy_from_slice(&part[r * wd..][..wd]);
        }
    }
    Ok(Some(out))
}

/// numpy's `.npy` (format 1.0, as `np.save` writes it) of a `h` by `w` uint8 array.
fn npy(a: &[u8], h: usize, w: usize) -> Vec<u8> {
    let mut head = format!("{{'descr': '|u1', 'fortran_order': False, 'shape': ({h}, {w}), }}");
    // (numpy's room for the first axis to grow, then 64-byte alignment, a newline last.)
    head.push_str(&" ".repeat(21usize.saturating_sub(h.to_string().len())));
    let pad = 64 - (10 + head.len() + 1) % 64;
    head.push_str(&" ".repeat(pad));
    head.push('\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend_from_slice(&(head.len() as u16).to_le_bytes());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(a);
    out
}

/// A `.npy` file's array when it's a C-ordered uint8 `h` by `w` one (as `np.load` checked).
fn read_npy(p: &Path, h: usize, w: usize) -> Option<Vec<u8>> {
    let b = std::fs::read(p).ok()?;
    if !b.starts_with(b"\x93NUMPY") || b.len() < 10 {
        return None;
    }
    let (len, start) = match b[6] {
        1 => (u16::from_le_bytes([b[8], b[9]]) as usize, 10),
        2 | 3 => (u32::from_le_bytes(b.get(8..12)?.try_into().ok()?) as usize, 12),
        _ => return None,
    };
    let head = std::str::from_utf8(b.get(start..start + len)?).ok()?;
    let compact: String = head.chars().filter(|c| !c.is_whitespace()).collect();
    let ok = (compact.contains("'descr':'|u1'") || compact.contains("'descr':'u1'"))
        && compact.contains("'fortran_order':False")
        && compact.contains(&format!("'shape':({h},{w})"));
    let data = &b[start + len..];
    (ok && data.len() == h * w).then(|| data.to_vec())
}

/// `a` written whole to `p` (a temporary name, flushed, renamed).
fn keep(p: &Path, a: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = store::whole::tmp_name(p);
    let r = (|| -> Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(a)?;
        f.sync_all()?;
        std::fs::rename(&tmp, p)?;
        Ok(())
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r.with_context(|| p.display().to_string())
}

/// A chunk of an EEA square: its row and column, size, and box (w, s, e, n).
#[derive(Clone, Copy)]
struct Job {
    r0: usize,
    c0: usize,
    h: usize,
    w: usize,
    bb: [f64; 4],
}

/// One square from the EEA, whole: every chunk in the EEA's box, each first asked at a fifth of
/// the resolution, so a chunk with no EEA data at all (Russia, the open sea) costs one small
/// request; a chunk that keeps failing fails the square (to be tried again). `progress` is told how
/// much of the square is done (0–1) as its chunks come.
fn europe_square(top: i32, left: i32, out: &Path, src: &LeafSources, progress: &(dyn Fn(f64) + Sync)) -> Result<()> {
    let mut a = vec![255u8; N * N];
    let mut jobs = Vec::new();
    for r0 in (0..N).step_by(MAX_H) {
        for c0 in (0..N).step_by(MAX_W) {
            let (h, w) = (MAX_H.min(N - r0), MAX_W.min(N - c0));
            let bb = [
                left as f64 + c0 as f64 * RES,
                top as f64 - (r0 + h) as f64 * RES,
                left as f64 + (c0 + w) as f64 * RES,
                top as f64 - r0 as f64 * RES,
            ];
            if meets_box(src.europe_box, bb) {
                jobs.push(Job { r0, c0, h, w, bb });
            }
        }
    }
    // The chunks, kept on the NAS as they come (a chunk: its array, or `.none` where the EEA has
    // no data), until the square is saved.
    let parts = out.join("parts").join(name(top, left));
    // Today's square, made over some regions only (not tagged complete): a chunk of it fetched whole
    // then is kept rather than asked for again. Whole: no holes where the probe shows the EEA has
    // data (a request that failed then left one: asked again; a fifth of a chunk's land, or of its
    // smallest split, is far more than the coastline's 0.05 %). The probe comes from the EEA's
    // coarser levels, so its classes aren't compared, only where there's data.
    let path = out.join(format!("{}.tif", name(top, left)));
    let old: Option<Vec<u8>> = {
        let _p = phase("today's squares read", Class::NasRead);
        (path.exists() && crate::whole::tiff_file_whole(&path))
            .then(|| {
                let t = crate::geotiff::Tiff::open(Arc::new(store::range::PlainFile::open(&path).ok()?)).ok()?;
                let img = t.level(0).ok()?;
                ((img.width as usize, img.height as usize) == (N, N)).then(|| read_u8(&t, 0, 0, N, N).ok()).flatten()
            })
            .flatten()
    };
    let kept_old = std::sync::atomic::AtomicUsize::new(0);
    let fetch = |j: &Job| -> Result<Option<Vec<u8>>> {
        let (h, w) = (j.h, j.w);
        let kept = parts.join(format!("{}-{}.npy", j.r0, j.c0));
        if parts.join(format!("{}-{}.none", j.r0, j.c0)).exists() {
            return Ok(Some(vec![255u8; h * w]));
        }
        if let Some(a) = read_npy(&kept, h, w) {
            return Ok(Some(a));
        }
        // Asked first at a fifth of the resolution (~250 m): land of a quarter kilometre shows.
        let (pw, ph) = ((w / 5).max(1), (h / 5).max(1));
        let Some(probe) = eea_chunk(src, j.bb, pw, ph)? else { return Ok(None) };
        std::fs::create_dir_all(&parts).with_context(|| parts.display().to_string())?;
        if !probe.iter().any(|&v| v <= 2) {
            std::fs::write(parts.join(format!("{}-{}.none", j.r0, j.c0)), b"")?;
            return Ok(Some(vec![255u8; h * w]));
        }
        if let Some(old) = &old {
            // here[2::5, 2::5], the probe's shape of it.
            let rows = (2..h).step_by(5).count().min(ph);
            let cols = (2..w).step_by(5).count().min(pw);
            if (rows, cols) == (ph, pw) {
                let (mut data, mut holes) = (0u64, 0u64);
                for r in 0..ph {
                    for c in 0..pw {
                        if eea_class(probe[r * pw + c]) != 255 {
                            data += 1;
                            if old[(j.r0 + 2 + 5 * r) * N + j.c0 + 2 + 5 * c] == 255 {
                                holes += 1;
                            }
                        }
                    }
                }
                if holes as f64 <= 0.002 * data as f64 {
                    kept_old.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let mut here = Vec::with_capacity(h * w);
                    for r in 0..h {
                        here.extend_from_slice(&old[(j.r0 + r) * N + j.c0..][..w]);
                    }
                    return Ok(Some(here));
                }
            }
        }
        let a = eea_chunk(src, j.bb, w, h)?;
        if let Some(a) = &a {
            keep(&kept, &npy(a, h, w))?;
        }
        Ok(a)
    };
    eprintln!("{}: {} EEA chunks", name(top, left), jobs.len());
    {
        let _p = phase("the EEA's chunks fetched", Class::Net);
        // Two at a time, taken in order; once one fails the rest still run (their chunks kept for
        // the next try), then the square fails.
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Option<Vec<u8>>>)>();
        let mut failed: Option<anyhow::Error> = None;
        std::thread::scope(|s| {
            for _ in 0..2 {
                let tx = tx.clone();
                let (next, jobs, fetch) = (&next, &jobs, &fetch);
                s.spawn(move || loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(j) = jobs.get(i) else { break };
                    if tx.send((i, fetch(j))).is_err() {
                        break;
                    }
                });
            }
            drop(tx);
            let mut waiting = std::collections::BTreeMap::new();
            let mut at = 0usize;
            for (i, r) in rx {
                waiting.insert(i, r);
                while let Some(r) = waiting.remove(&at) {
                    let j = jobs[at];
                    at += 1;
                    if failed.is_some() {
                        continue;
                    }
                    match r {
                        Ok(Some(chunk)) => {
                            for row in 0..j.h {
                                let d = &mut a[(j.r0 + row) * N + j.c0..][..j.w];
                                for (o, &v) in d.iter_mut().zip(&chunk[row * j.w..][..j.w]) {
                                    *o = eea_class(v);
                                }
                            }
                            // (The square's save counts as one chunk more.)
                            progress(at as f64 / (jobs.len() + 1) as f64);
                        }
                        Ok(None) => {
                            failed = Some(anyhow::anyhow!(
                                "EEA leaf type: the request for ({}, {}, {}, {}) keeps failing",
                                j.bb[0],
                                j.bb[1],
                                j.bb[2],
                                j.bb[3]
                            ))
                        }
                        Err(e) => failed = Some(e),
                    }
                }
            }
        });
        if let Some(e) = failed {
            return Err(e);
        }
    }
    let k = kept_old.into_inner();
    if k > 0 {
        eprintln!("  {k} chunks kept from today's square");
    }
    let _p = phase("the squares written", Class::NasWrite);
    save(top, left, &a, EEA_SOURCE, out)?;
    std::fs::remove_dir_all(&parts).ok();
    Ok(())
}

/// NALCMS's GeoTIFF, streamed out of CEC's zip, whole: the deflate stream to its end, and its
/// size and CRC-32 those the zip records, before it takes its name.
fn fetch_nalcms(tif: &Path, src: &LeafSources) -> Result<()> {
    use std::io::{Read, Write};
    if tif.exists() {
        return Ok(());
    }
    let url = &src.north_america;
    let [offset, compressed] = src.north_america_member;
    let _p = phase("NALCMS's GeoTIFF downloaded", Class::Net);
    std::fs::create_dir_all(tif.parent().context("NALCMS: no folder")?)?;
    crate::fetch::online(url)?;
    let ag = agent(None);
    let get = |from: u64, to: u64| -> Result<ureq::http::Response<ureq::Body>> {
        let r = ag.get(url).header("Range", &format!("bytes={from}-{to}")).call()?;
        let status = r.status().as_u16();
        if status == 429 {
            crate::budget::seen(url, status, r.headers().get("retry-after").and_then(|v| v.to_str().ok()));
        }
        ensure!(status < 400, "NALCMS: HTTP {status}");
        Ok(r)
    };
    let head = get(offset, offset + 511)?.body_mut().with_config().limit(1 << 20).read_to_vec()?;
    if head.len() < 30 || &head[..4] != b"PK\x03\x04" {
        bail!("NALCMS: unexpected zip layout");
    }
    let u32_at = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().unwrap());
    let u16_at = |i: usize| u16::from_le_bytes(head[i..i + 2].try_into().unwrap());
    let (crc, csize, usize_, nlen, elen) = (u32_at(14), u32_at(18) as u64, u32_at(22) as u64, u16_at(26), u16_at(28));
    ensure!(csize == compressed, "NALCMS: the zip's member is {csize} bytes, not {compressed}");
    let start = offset + 30 + nlen as u64 + elen as u64;
    let tmp = store::whole::tmp_name(tif);
    eprintln!("streaming NALCMS GeoTIFF ({:.1} GB compressed)…", compressed as f64 / 1e9);
    let r = (|| -> Result<()> {
        let mut resp = get(start, start + compressed - 1)?;
        let mut body = resp.body_mut().with_config().limit(u64::MAX).reader();
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut dec = flate2::Decompress::new(false);
        let mut hasher = crc32fast::Hasher::new();
        let (mut inb, mut outb) = (vec![0u8; 8 << 20], vec![0u8; 8 << 20]);
        let (mut got, mut out_n, mut ended) = (0u64, 0u64, false);
        loop {
            let k = body.read(&mut inb)?;
            if k == 0 {
                break;
            }
            got += k as u64;
            let mut s = &inb[..k];
            while !s.is_empty() && !ended {
                let (i0, o0) = (dec.total_in(), dec.total_out());
                let st = dec.decompress(s, &mut outb, flate2::FlushDecompress::None)?;
                let (used, made) = ((dec.total_in() - i0) as usize, (dec.total_out() - o0) as usize);
                f.write_all(&outb[..made])?;
                hasher.update(&outb[..made]);
                out_n += made as u64;
                s = &s[used..];
                ended = st == flate2::Status::StreamEnd;
                if used == 0 && made == 0 {
                    break;
                }
            }
            eprint!("\r  {:.0}%", got as f64 / compressed as f64 * 100.0);
        }
        // What's still held in the inflater.
        while !ended {
            let o0 = dec.total_out();
            let st = dec.decompress(&[], &mut outb, flate2::FlushDecompress::Finish)?;
            let made = (dec.total_out() - o0) as usize;
            f.write_all(&outb[..made])?;
            hasher.update(&outb[..made]);
            out_n += made as u64;
            ended = st == flate2::Status::StreamEnd;
            if made == 0 {
                break;
            }
        }
        eprintln!();
        let f = f.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        let out_crc = hasher.finalize();
        ensure!(
            ended && out_n == usize_ && out_crc == crc,
            "NALCMS: {out_n} bytes, CRC {out_crc:08x}; the zip says {usize_}, {crc:08x}"
        );
        std::fs::rename(&tmp, tif)?;
        Ok(())
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r
}

/// NALCMS's projection as its GeoTIFF says it (Lambert azimuthal equal-area on WGS 84), its centre.
fn nalcms_centre(t: &crate::geotiff::Tiff) -> Result<(f64, f64)> {
    let k = t.geo_keys();
    let d = |key: u16| k.double(key);
    // ProjCoordTransGeoKey 10 (CT_LambertAzimEqualArea); ProjCenterLatGeoKey, ProjCenterLongGeoKey
    // (or the natural origin's); no false origin; WGS 84's ellipsoid.
    ensure!(k.short(crate::geotiff::key::PROJ_COORD_TRANS) == Some(10), "NALCMS: not Lambert azimuthal equal-area");
    let lat = d(3089).or_else(|| d(crate::geotiff::key::PROJ_NAT_ORIGIN_LAT)).context("NALCMS: no centre latitude")?;
    let lon =
        d(3088).or_else(|| d(crate::geotiff::key::PROJ_NAT_ORIGIN_LONG)).context("NALCMS: no centre longitude")?;
    for key in [crate::geotiff::key::PROJ_FALSE_EASTING, crate::geotiff::key::PROJ_FALSE_NORTHING] {
        ensure!(d(key).unwrap_or(0.0) == 0.0, "NALCMS: a false origin");
    }
    let wgs = k.short(crate::geotiff::key::GEOGRAPHIC_TYPE) == Some(4326)
        || k.short(crate::geotiff::key::GEOG_GEODETIC_DATUM) == Some(6326)
        || (d(crate::geotiff::key::GEOG_SEMI_MAJOR_AXIS) == Some(6378137.0)
            && d(crate::geotiff::key::GEOG_INV_FLATTENING) == Some(298.257223563));
    ensure!(wgs, "NALCMS: not on WGS 84");
    Ok((lat, lon))
}

/// Square (`top`, `left`) resampled from NALCMS (`t`), its own classes, 255 where it has none.
fn nalcms_square(t: &crate::geotiff::Tiff, top: i32, left: i32) -> Result<Vec<u8>> {
    use rayon::prelude::*;
    let (lat0, lon0) = nalcms_centre(t)?;
    let img = t.level(0)?;
    let tr = warp::Transform::new(
        warp::Laea::new(lat0, lon0)?,
        [left as f64, RES, 0.0, top as f64, 0.0, -RES],
        t.transform(0)?,
    )?;
    let w = warp::Warp { t: tr, src_w: img.width as i32, src_h: img.height as i32 };
    ensure!(!w.pole_inside(N as i32, N as i32), "a pole inside square {}: GDAL samples a grid", name(top, left));
    let chunks = w.chunks(N as i32, N as i32)?;
    // (rasterio's four threads; each chunk's source window read, then warped.)
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    let done: Vec<(warp::Chunk, Vec<u8>)> = pool.install(|| {
        chunks
            .par_iter()
            .map(|c| -> Result<(warp::Chunk, Vec<u8>)> {
                let (sx, sy, sw, sh) = c.src;
                let s = read_u8(t, sx as usize, sy as usize, sw as usize, sh as usize)?;
                let mut o = vec![255u8; c.dst.2 as usize * c.dst.3 as usize];
                w.warp_chunk(c, &s, &mut o);
                Ok((*c, o))
            })
            .collect::<Result<_>>()
    })?;
    let mut dst = vec![255u8; N * N];
    for (c, o) in done {
        let (dx, dy, dw, dh) = (c.dst.0 as usize, c.dst.1 as usize, c.dst.2 as usize, c.dst.3 as usize);
        for r in 0..dh {
            dst[(dy + r) * N + dx..][..dw].copy_from_slice(&o[r * dw..][..dw]);
        }
    }
    Ok(dst)
}

/// Squares from NALCMS (its GeoTIFF streamed to `tif` first, and kept). `progress` is told how
/// many are done after each.
fn north_america_squares(
    todo: &[(i32, i32)],
    out: &Path,
    tif: &Path,
    src: &LeafSources,
    progress: &dyn Fn(usize),
) -> Result<()> {
    if todo.is_empty() {
        return Ok(());
    }
    fetch_nalcms(tif, src)?;
    let f = store::range::PlainFile::open(tif).with_context(|| tif.display().to_string())?;
    let t = crate::geotiff::Tiff::open(Arc::new(f))?.with_cache(512 << 20);
    for (k, &(top, left)) in todo.iter().enumerate() {
        let t0 = std::time::Instant::now();
        let a = {
            let _p = phase("the NALCMS squares resampled", Class::Compute);
            let mut a = nalcms_square(&t, top, left).with_context(|| format!("square {}", name(top, left)))?;
            for v in &mut a {
                *v = na_class(*v);
            }
            a
        };
        {
            let _p = phase("the squares written", Class::NasWrite);
            save(top, left, &a, NALCMS_SOURCE, out)?;
        }
        eprintln!("    ({:.0} s)", t0.elapsed().as_secs_f64());
        progress(k + 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
