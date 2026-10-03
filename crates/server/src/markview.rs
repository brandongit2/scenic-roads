//! A z6 tile's landmark points (markdata, docs/phase5.md) as the server reads them: per kind, its
//! rows (ids, points, the filters' values), their lean properties and popup records (zstd blocks,
//! decompressed on use and kept under a budget), and the tile's named peaks.

use crate::pages::Lru;
use crate::views::{Blob, Sect, SectView};
use anyhow::{ensure, Context, Result};
use pipeline::marks::{self, MarkPt, SummitRec, KINDS};
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex};

/// Decompressed blocks of `props` and `info`, by (file, section, block).
static BLOCKS: LazyLock<Mutex<Lru<(String, u8, u32), Arc<Vec<u8>>>>> = LazyLock::new(|| Mutex::new(Lru::new(96 << 20)));

pub struct MarkView {
    pub tile: (u32, u32),
    /// The file's content name (cache keys, ETags).
    pub content: String,
    kinds: Vec<[u32; 2]>,
    /// Per kind, where its values start in `fvals`, and its fields.
    fval_at: Vec<usize>,
    pub fields: Vec<Vec<&'static str>>,
    pub ids: Sect<u64>,
    pub pts: Sect<MarkPt>,
    pub fvals: Sect<f64>,
    props_idx: Vec<[u64; 2]>,
    info_idx: Vec<[u64; 2]>,
    pub summits: Vec<SummitRec>,
    summit_names: Vec<u8>,
    s: SectView,
}

impl MarkView {
    pub fn new(s: SectView, content: String) -> Result<MarkView> {
        let tile = s.meta["tile"].as_str().and_then(|t| {
            let mut p = t.split('/').skip(1).map(|v| v.parse::<u32>().ok());
            Some((p.next()??, p.next()??))
        });
        let tile = tile.context("markdata without its tile")?;
        let kinds: Vec<[u32; 2]> = s.sect::<[u32; 2]>("kinds")?.all()?.cast::<[u32; 2]>().to_vec();
        ensure!(kinds.len() == KINDS.len(), "markdata kinds: {}", kinds.len());
        let fields: Vec<Vec<&'static str>> = KINDS.iter().map(|k| marks::fields(k)).collect();
        // The file's own fields must be today's (a new filter is a format change).
        for (k, f) in KINDS.iter().zip(&fields) {
            let theirs: Vec<&str> = s.meta["fields"][k].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            ensure!(theirs == *f, "markdata {content}: {k}'s fields {theirs:?}, not {f:?}");
        }
        let mut fval_at = Vec::with_capacity(KINDS.len());
        let mut at = 0usize;
        for (k, f) in kinds.iter().zip(&fields) {
            fval_at.push(at);
            at += f.len() * k[1] as usize;
        }
        let ids = s.sect::<u64>("ids")?;
        let pts = s.sect::<MarkPt>("pts")?;
        let fvals = s.sect::<f64>("fvals")?;
        let n: usize = kinds.iter().map(|k| k[1] as usize).sum();
        ensure!(ids.len() == n && pts.len() == n && fvals.len() == at, "markdata {content}: sections disagree");
        let props_idx = s.sect::<[u64; 2]>("props_idx")?.all()?.cast::<[u64; 2]>().to_vec();
        let info_idx = s.sect::<[u64; 2]>("info_idx")?.all()?.cast::<[u64; 2]>().to_vec();
        let summits = s.sect::<SummitRec>("summits")?.all()?.cast::<SummitRec>().to_vec();
        let summit_names = s.get("summit_names")?.bytes().to_vec();
        Ok(MarkView { tile, content, kinds, fval_at, fields, ids, pts, fvals, props_idx, info_idx, summits, summit_names, s })
    }

    pub fn is_remote(&self) -> bool {
        self.s.is_remote()
    }

    /// Rows of kind `k`.
    pub fn range(&self, k: usize) -> Range<usize> {
        let [a, n] = self.kinds[k];
        a as usize..(a + n) as usize
    }

    /// Kind `k`'s values of field `f`, for its rows in order.
    pub fn fval_range(&self, k: usize, f: usize) -> Range<usize> {
        let n = self.kinds[k][1] as usize;
        let a = self.fval_at[k] + f * n;
        a..a + n
    }

    pub fn summit_name(&self, i: usize) -> Result<&str> {
        Ok(std::str::from_utf8(marks::object(&self.summit_names, i)?)?)
    }

    fn block(&self, sect: u8, b: usize) -> Result<Arc<Vec<u8>>> {
        let key = (self.content.clone(), sect, b as u32);
        if let Some(v) = BLOCKS.lock().unwrap().get(&key) {
            return Ok(v);
        }
        let (name, idx) = if sect == 0 { ("props", &self.props_idx) } else { ("info", &self.info_idx) };
        let [off, len] = *idx.get(b).context("no such block")?;
        let z = self.s.get_part(name, off, len as usize)?;
        let v = Arc::new(zstd::bulk::decompress(&z, 64 << 20)?);
        BLOCKS.lock().unwrap().put(key, v.clone(), v.len() as u64);
        Ok(v)
    }

    /// Row `i`'s lean properties (a JSON object's bytes).
    pub fn props(&self, i: usize) -> Result<Vec<u8>> {
        let b = self.block(0, i / marks::BLOCK)?;
        Ok(marks::object(&b, i % marks::BLOCK)?.to_vec())
    }

    /// Row `i`'s popup record (empty: none).
    pub fn info(&self, i: usize) -> Result<Vec<u8>> {
        let b = self.block(1, i / marks::BLOCK)?;
        Ok(marks::object(&b, i % marks::BLOCK)?.to_vec())
    }

    /// The row of kind `k` with this id.
    pub fn find(&self, k: usize, id: u64) -> Result<Option<usize>> {
        let r = self.range(k);
        let ids = self.ids.range(r.clone())?;
        Ok(ids.binary_search(&id).ok().map(|j| r.start + j))
    }

    /// Whole sections for a scan (from the NAS: read once, kept while memory allows).
    pub fn scan(&self) -> Result<(Blob, Blob, Blob)> {
        Ok((self.ids.all()?, self.pts.all()?, self.fvals.all()?))
    }
}
