//! NAS storage for the map (docs/storage.md–4, docs/formats.md): content-named immutable files,
//! packs of tiles, sectioned files, catalogs, the bounded NAS I/O pool, finding and mounting the
//! share, the per-Mac mirror and PMTiles reading.

pub mod blobs;
pub mod cachefile;
pub mod catalog;
pub mod clock;
pub mod iopool;
pub mod mirror;
pub mod naming;
pub mod nas;
pub mod nasio;
pub mod pack;
pub mod pieces;
pub mod pmtiles;
pub mod range;
pub mod retry;
pub mod sect;
pub mod spawn;
pub mod sys;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use iopool::{IoError, IoPool};
pub use range::{MmapFile, PooledFile, RangeRead};
