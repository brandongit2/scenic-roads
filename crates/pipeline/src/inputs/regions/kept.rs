//! The regions' check's resolutions kept as pieces (task #897; docs/pool.md §7.2): each region
//! without water resolved (`resolve::resolve_one`, by its memo key), and each region with water's
//! land and water (`tiles`, by the folder's whole key), kept as it's made in the job's scratch
//! (crate::resume_log::Pieces), each a safe point. A check resumed after a pause takes what its
//! last kept, by the same keys (a digest of everything a resolution reads), so it resolves to the
//! same geometry and holds the same files.
//!
//! A pause comes while regions are resolved side by side: the piece kept, the rest stop making
//! (`get_or_make` hands back None, the caller gives up on that region) and the check returns the
//! pause once its resolution returns (`stopped`), keeping nothing of what was given up.

use super::geom::Area;
use super::resolve::{Problem, Resolved};
use crate::resume_log::{BlockId, Get, Pieces, Put};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The stages a piece is of (its block's first number).
pub const RESOLVED: u8 = 1;
pub const LAND: u8 = 2;
pub const WATER: u8 = 3;

/// The pieces, and what this run made of them (a check resolves several versions, each region's
/// key the same where it reads the same).
#[derive(Default)]
pub struct Kept {
    pieces: Mutex<Option<Pieces>>,
    made: Mutex<HashMap<BlockId, Arc<Vec<u8>>>>,
    stop: Mutex<Option<anyhow::Error>>,
    /// Tests: crate::resume_log::STOP_AFTER as it was at `open`, counted here (the pieces are kept
    /// on the resolution's own threads, where the test's thread-local isn't).
    #[cfg(test)]
    stop_after: Mutex<Option<usize>>,
}

impl Kept {
    /// Pieces kept in `scratch` for identity `id` from now on (`keep`: resuming on).
    pub fn open(&self, scratch: &std::path::Path, id: &str, keep: bool) {
        *self.pieces.lock().unwrap() = Some(Pieces::open(scratch, (0, 0, 0), "inputs", id, keep));
        #[cfg(test)]
        {
            *self.stop_after.lock().unwrap() = crate::resume_log::STOP_AFTER.take();
        }
    }

    /// The piece of stage `stage` and key `key`: kept (by an earlier run, or this one), else
    /// `make`'s, kept (a safe point). None once a pause came (nothing made then).
    pub fn get_or_make<T>(
        &self,
        stage: u8,
        key: &str,
        make: impl FnOnce() -> T,
        enc: impl Fn(&T, &mut Put),
        dec: impl Fn(&mut Get) -> Result<T>,
    ) -> Option<T> {
        if self.stop.lock().unwrap().is_some() {
            return None;
        }
        let h = blake3::hash(format!("{stage} {key}").as_bytes());
        let b: BlockId =
            (stage, u32::from_le_bytes(h.as_bytes()[..4].try_into().unwrap()), u32::from_le_bytes(h.as_bytes()[4..8].try_into().unwrap()));
        let had = self.made.lock().unwrap().get(&b).cloned().or_else(|| {
            self.pieces.lock().unwrap().as_ref().and_then(|p| p.kept(b)).map(|p| Arc::new(p.to_vec()))
        });
        if let Some(p) = had {
            let mut g = Get(&p);
            let read = (|| -> Result<T> {
                anyhow::ensure!(g.bytes()? == key.as_bytes(), "another key's");
                let v = dec(&mut g)?;
                g.end()?;
                Ok(v)
            })();
            match read {
                Ok(v) => return Some(v),
                Err(e) => eprintln!("regions: a kept piece not taken ({e:#}): made again"),
            }
        }
        let v = make();
        let mut p = Put::default();
        p.bytes(key.as_bytes());
        enc(&v, &mut p);
        let p = Arc::new(p.0);
        if let Some(pieces) = self.pieces.lock().unwrap().as_mut() {
            if let Err(e) = pieces.keep(b, &p) {
                self.stop.lock().unwrap().get_or_insert(e);
            }
            #[cfg(test)]
            {
                let mut n = self.stop_after.lock().unwrap();
                match *n {
                    Some(k) if k <= 1 => {
                        *n = None;
                        self.stop.lock().unwrap().get_or_insert(anyhow::Error::new(crate::control::Paused));
                    }
                    Some(k) => *n = Some(k - 1),
                    None => {}
                }
            }
        }
        self.made.lock().unwrap().insert(b, p);
        Some(v)
    }

    /// The pause a piece kept met, if one did: the check stops with it.
    pub fn stopped(&self) -> Result<()> {
        match self.stop.lock().unwrap().take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The check done: the pieces deleted.
    pub fn finish(&self) {
        if let Some(p) = self.pieces.lock().unwrap().take() {
            p.finish();
        }
    }
}

/// A check name as `Problem` has it (static): the names read back are few, each kept once.
fn check_name(s: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut n = NAMES.lock().unwrap();
    if let Some(x) = n.iter().find(|x| **x == s) {
        return x;
    }
    let x: &'static str = Box::leak(s.to_string().into_boxed_str());
    n.push(x);
    x
}

fn strs(p: &mut Put, v: &[String]) {
    p.u64(v.len() as u64);
    for s in v {
        p.bytes(s.as_bytes());
    }
}

fn get_strs(g: &mut Get) -> Result<Vec<String>> {
    (0..g.count(8)?).map(|_| Ok(String::from_utf8(g.bytes()?)?)).collect()
}

pub fn problems(p: &mut Put, v: &[Problem]) {
    p.u64(v.len() as u64);
    for x in v {
        p.bytes(x.check.as_bytes());
        strs(p, &x.about);
        strs(p, &x.found);
        strs(p, &x.files);
        p.bytes(x.message.as_bytes());
    }
}

pub fn get_problems(g: &mut Get) -> Result<Vec<Problem>> {
    (0..g.count(40)?)
        .map(|_| {
            Ok(Problem {
                check: check_name(&String::from_utf8(g.bytes()?)?),
                about: get_strs(g)?,
                found: get_strs(g)?,
                files: get_strs(g)?,
                message: String::from_utf8(g.bytes()?)?,
            })
        })
        .collect()
}

pub fn area(p: &mut Put, a: &Area) {
    p.u64(a.polys.len() as u64);
    for poly in &a.polys {
        p.u64(poly.len() as u64);
        for ring in poly {
            p.u64(ring.len() as u64);
            for pt in ring {
                p.u64(pt[0] as u64);
                p.u64(pt[1] as u64);
            }
        }
    }
}

pub fn get_area(g: &mut Get) -> Result<Area> {
    let mut polys = Vec::new();
    for _ in 0..g.count(8)? {
        let mut poly = Vec::new();
        for _ in 0..g.count(8)? {
            let n = g.count(16)?;
            poly.push((0..n).map(|_| Ok([g.u64()? as i64, g.u64()? as i64])).collect::<Result<Vec<_>>>()?);
        }
        polys.push(poly);
    }
    Ok(Area { polys })
}

/// What it needed of the extract, as ids.
pub fn ids(p: &mut Put, v: &[super::extract::Id]) {
    strs(p, &v.iter().map(ToString::to_string).collect::<Vec<_>>());
}

pub fn get_ids(g: &mut Get) -> Result<Vec<super::extract::Id>> {
    get_strs(g)?.iter().map(|s| s.parse()).collect()
}

/// A resolution or its problems.
pub fn result<T>(p: &mut Put, r: &Result<T, Vec<Problem>>, ok: impl Fn(&mut Put, &T)) {
    match r {
        Ok(v) => {
            p.u8(1);
            ok(p, v);
        }
        Err(ps) => {
            p.u8(0);
            problems(p, ps);
        }
    }
}

pub fn get_result<T>(g: &mut Get, ok: impl Fn(&mut Get) -> Result<T>) -> Result<Result<T, Vec<Problem>>> {
    Ok(match g.u8()? {
        1 => Ok(ok(g)?),
        0 => Err(get_problems(g)?),
        t => anyhow::bail!("a result tagged {t}"),
    })
}

pub fn resolved(p: &mut Put, r: &Resolved) {
    p.bytes(&r.geometry.encode());
    p.bytes(&serde_json::to_vec(&r.drawn).unwrap_or_default());
    area(p, &r.lakes);
}

pub fn get_resolved(g: &mut Get) -> Result<Resolved> {
    Ok(Resolved {
        geometry: super::stored::Geometry::decode(&g.bytes()?)?,
        drawn: serde_json::from_slice(&g.bytes()?)?,
        lakes: get_area(g)?,
    })
}
