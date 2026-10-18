//! Elevations at every road vertex: the `elev` program, its command line and outputs, so a unit's
//! build can run anywhere (docs/workers.md). The heights come from the declared elevation datasets
//! (docs/inputs.md §6.8): the unit job's plan for the unit (`declared::Plan`, its folder's
//! `elevation.json`) read through one generic sampler (`generic`), the finest valid dataset at each
//! point (a declared rank overriding), blended at the edges. No source is named here.
//!
//! Only the blocks (and tiles) holding vertices are read (`crate::fetch`). Incremental: the sorted
//! (vertex → elevation, source) cache of the last run serves the vertices it has; only the others
//! are sampled. Outputs go to `.tmp` files and are renamed into place at the end (`dem-progress.json`:
//! an interrupted run takes the cache's heights again and samples the rest).

pub mod declared;
pub mod encoding;
pub mod fabdem_fetch;
pub mod generic;
pub mod kept;
pub mod proj;
pub mod sample;
pub mod spacing;

use anyhow::{ensure, Result};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use store::sys::PosIo;

/// A vertex's cache key: `(lon + 2³¹) << 32 | (lat + 2³¹)` (E7), as `crate::unit`'s DEM cache.
pub fn pack(v: [i32; 2]) -> u64 {
    (((v[0] as i64 + (1i64 << 31)) as u64) << 32) | ((v[1] as i64 + (1i64 << 31)) as u64)
}

/// The run's settings: the `elev` program's arguments and environment.
pub struct Config {
    pub build: PathBuf,
    /// Threads reading the datasets' blocks (they mostly wait on the servers).
    pub workers: usize,
    /// Where the last run's per-vertex cache is (`dem-cache.*`), and this run's goes.
    pub cache: PathBuf,
    pub no_cache: bool,
    /// The stores' folder (`sources/fetched/`) for datasets declared `store`: their files read
    /// there alone, never fetched (crate::dem::generic::stored).
    pub store: Option<PathBuf>,
}

/// `n` with thousands separators.
fn th(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The outputs being made (`elev.f32.tmp`, `src.u8.tmp`) and the DEM files done
/// (`dem-progress.json`), so an interrupted run resumes where it stopped. What changed since the
/// last checkpoint is written then, by pages.
struct Progress {
    dir: PathBuf,
    stamp: String,
    elev: Vec<f32>,
    src: Vec<u8>,
    done: BTreeSet<String>,
    dirty: Vec<bool>,
    resumed: bool,
    files: (File, File),
}

const PAGE: usize = 16384;

impl Progress {
    fn open(dir: &Path, stamp: String, n: usize) -> Result<Progress> {
        let (te, ts) = (dir.join("elev.f32.tmp"), dir.join("src.u8.tmp"));
        let prev = std::fs::read_to_string(dir.join("dem-progress.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let pages = n.div_ceil(PAGE);
        if let Some(p) = prev.filter(|p| p["stamp"].as_str() == Some(stamp.as_str())) {
            if let (Ok(e), Ok(s)) = (std::fs::read(&te), std::fs::read(&ts)) {
                if e.len() == 4 * n && s.len() == n {
                    let done = p["done"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    let files = (OpenOptions::new().write(true).open(&te)?, OpenOptions::new().write(true).open(&ts)?);
                    return Ok(Progress {
                        dir: dir.to_owned(),
                        stamp,
                        elev: bytemuck::pod_collect_to_vec(&e),
                        src: s,
                        done,
                        dirty: vec![false; pages],
                        resumed: true,
                        files,
                    });
                }
            }
        }
        let (fe, fs) = (File::create(&te)?, File::create(&ts)?);
        fe.set_len(4 * n as u64)?;
        fs.set_len(n as u64)?;
        Ok(Progress {
            dir: dir.to_owned(),
            stamp,
            elev: vec![f32::NAN; n],
            src: vec![0; n],
            done: BTreeSet::new(),
            dirty: vec![true; pages],
            resumed: false,
            files: (fe, fs),
        })
    }

    fn set(&mut self, i: usize, e: f32, s: u8) {
        if self.elev[i].to_bits() != e.to_bits() || self.src[i] != s {
            self.elev[i] = e;
            self.src[i] = s;
            self.dirty[i / PAGE] = true;
        }
    }

    /// The sampled values (`loc`, over `miss`) into the outputs.
    fn scatter(&mut self, miss: &[u32], loc_elev: &[f32], loc_src: &[u8]) {
        for (k, &i) in miss.iter().enumerate() {
            if loc_elev[k].is_finite() {
                self.set(i as usize, loc_elev[k], loc_src[k]);
            }
        }
    }

    fn flush(&mut self) -> Result<()> {
        let n = self.elev.len();
        let mut p = 0;
        while p < self.dirty.len() {
            if !self.dirty[p] {
                p += 1;
                continue;
            }
            let q = (p..self.dirty.len()).find(|&q| !self.dirty[q]).unwrap_or(self.dirty.len());
            let (s, e) = (p * PAGE, (q * PAGE).min(n));
            self.files.0.write_all_at(bytemuck::cast_slice(&self.elev[s..e]), 4 * s as u64)?;
            self.files.1.write_all_at(&self.src[s..e], s as u64)?;
            self.dirty[p..q].fill(false);
            p = q;
        }
        Ok(())
    }

    /// A checkpoint: the outputs so far, and `name` done.
    fn mark(&mut self, name: &str) -> Result<()> {
        self.flush()?;
        self.done.insert(name.to_string());
        let j = serde_json::json!({ "stamp": self.stamp, "done": self.done });
        std::fs::write(self.dir.join("dem-progress.json"), j.to_string())?;
        Ok(())
    }

    /// The outputs renamed into place.
    fn finish(mut self) -> Result<(Vec<f32>, Vec<u8>)> {
        self.flush()?;
        drop(self.files);
        std::fs::rename(self.dir.join("elev.f32.tmp"), self.dir.join("elev.f32"))?;
        std::fs::rename(self.dir.join("src.u8.tmp"), self.dir.join("src.u8"))?;
        Ok((self.elev, self.src))
    }
}

/// The last run's cache (`dem-cache.*` in `cache`: sorted keys, elevations, sources) for the
/// vertices it has.
fn reuse(cache: &Path, verts: &[[i32; 2]], prog: &mut Progress) -> Result<()> {
    let ck = cache.join("dem-cache.keys.u64");
    if !std::fs::metadata(&ck).is_ok_and(|m| m.len() > 0) {
        return Ok(());
    }
    let km = roadcore::mmap(&ck)?;
    let em = roadcore::mmap(&cache.join("dem-cache.elev.f32"))?;
    let sm = roadcore::mmap(&cache.join("dem-cache.src.u8"))?;
    let (ckeys, celev): (&[u64], &[f32]) = (bytemuck::cast_slice(&km[..]), bytemuck::cast_slice(&em[..]));
    let csrc: &[u8] = &sm;
    ensure!(
        ckeys.len() == celev.len() && ckeys.len() == csrc.len(),
        "{}: DEM cache files out of step",
        cache.display()
    );
    let found: Vec<Option<usize>> = verts
        .par_iter()
        .map(|&v| {
            let k = pack(v);
            let p = ckeys.partition_point(|&x| x < k);
            (p < ckeys.len() && ckeys[p] == k).then_some(p)
        })
        .collect();
    let mut hit = 0;
    for (i, f) in found.into_iter().enumerate() {
        if let Some(p) = f {
            prog.set(i, celev[p], csrc[p]);
            hit += 1;
        }
    }
    let n = verts.len();
    println!("cache: reused {} of {} vertices ({:.1} %)", th(hit), th(n), hit as f64 / n.max(1) as f64 * 100.0);
    Ok(())
}

/// A thread pool of `n` threads, when threads can be made (not in WebAssembly).
fn pool(n: usize) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new().num_threads(n.max(1)).build().ok()
}

fn mtime_ns(p: &Path) -> Result<u128> {
    Ok(std::fs::metadata(p)?.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos())
}

/// How far the sampling is, for the unit job's progress (crate::agent::jobs::report: a line on
/// stderr): `frac` (0 to 1) of the `of` vertices to sample done, to three decimals of a vertex.
fn said(frac: f64, of: usize) {
    if of > 0 {
        crate::agent::jobs::report_f(frac.clamp(0.0, 1.0) * of as f64, of as u64, "vertices");
    }
}

/// The cache for the next run (`dem-cache.*` in `cache`): every vertex's key, sorted, with its
/// height and source.
fn write_cache(cache: &Path, verts: &[[i32; 2]], elev: &[f32], src: &[u8]) -> Result<()> {
    println!("updating elevation cache");
    let n = verts.len();
    std::fs::create_dir_all(cache)?;
    let keys: Vec<u64> = verts.par_iter().map(|&v| pack(v)).collect();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.par_sort_unstable_by_key(|&i| (keys[i as usize], i));
    let ok: Vec<u64> = order.iter().map(|&i| keys[i as usize]).collect();
    let oe: Vec<f32> = order.iter().map(|&i| elev[i as usize]).collect();
    let os: Vec<u8> = order.iter().map(|&i| src[i as usize]).collect();
    for (name, bytes) in [
        ("keys.u64", bytemuck::cast_slice::<u64, u8>(&ok)),
        ("elev.f32", bytemuck::cast_slice(&oe)),
        ("src.u8", &os[..]),
    ] {
        let tmp = cache.join(format!("dem-cache.{name}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, cache.join(format!("dem-cache.{name}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
