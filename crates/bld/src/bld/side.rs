//! A z6 tile's normalized buildings' side file (`work/bldsrc/6-<x>-<y>.<h>.sect`, RDSECT v1;
//! docs/formats.md): the columns only the data-available masks read (crate::bld::extents), kept
//! beside the normalized file (crate::bld::work) and not in it, so a column the masks need never
//! changes the normalized file, which `bldtiles` is keyed on (docs/steps.md §14, the masks).
//!
//! Meta: `{"fmt": 1, "tile": "6/x/y", "work": <the normalized file's content name>}`. Section
//! `fsrc`: each record's footprint's dataset (`u8`, a code into the normalized file's `srcs`; 0
//! none), the records in the normalized file's order (its blocks in index order, each block's
//! records in order), zstd'd.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use store::sect::SectReader;

/// The side file's meta.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SideMeta {
    pub fmt: u32,
    pub tile: String,
    /// The content name of the normalized file whose records it follows.
    pub work: String,
}

/// A side file read whole.
#[derive(Debug)]
pub struct SideFile {
    pub meta: SideMeta,
    /// Each record's footprint's dataset, in the normalized file's order.
    pub fsrc: Vec<u8>,
}

/// Writes the side file of tile `tile` to `path`, for the normalized file `work` (its content
/// name).
pub fn write(path: &Path, tile: &str, work: &str, fsrc: &[u8]) -> Result<()> {
    let meta = SideMeta { fmt: 1, tile: tile.to_string(), work: work.to_string() };
    let mut w = store::sect::SectWriter::create(path, serde_json::to_value(&meta)?)?;
    w.add_pod("fsrc", &zstd::bulk::compress(fsrc, super::work::ZSTD_LEVEL)?)?;
    w.finish()?;
    Ok(())
}

impl SideFile {
    pub fn open<'a>(path: impl Into<store::nasfs::Loc<'a>>) -> Result<SideFile> {
        let path = path.into();
        let f = path.open_plain().with_context(|| format!("open {}", path.display()))?;
        let r = SectReader::open(f).with_context(|| format!("read {}", path.display()))?;
        let meta: SideMeta =
            serde_json::from_value(r.meta().clone()).with_context(|| format!("{}: its meta", path.display()))?;
        ensure!(meta.fmt == 1, "{}: a buildings side file fmt {} (this reads 1)", path.display(), meta.fmt);
        let z: Vec<u8> = r.read_pod("fsrc")?;
        let fsrc = zstd::decode_all(&z[..]).with_context(|| format!("{}: its fsrc's zstd", path.display()))?;
        Ok(SideFile { meta, fsrc })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_side_file_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.sect");
        write(&p, "6/1/2", "work/bld/6-1-2.0123456789abcdef.sect", &[1, 0, 2, 2]).unwrap();
        let s = SideFile::open(&p).unwrap();
        assert_eq!(
            s.meta,
            SideMeta { fmt: 1, tile: "6/1/2".into(), work: "work/bld/6-1-2.0123456789abcdef.sect".into() }
        );
        assert_eq!(s.fsrc, [1, 0, 2, 2]);
    }
}
