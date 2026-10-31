//! NAS storage for the map (docs/storage.md–4, docs/formats.md): content-named immutable files,
//! packs of tiles, sectioned files, catalogues, the map and its log (docs/maplog.md), the bounded
//! NAS I/O pool, finding and mounting the share, the per-Mac mirror and PMTiles reading.

pub mod blobs;
pub mod cachefile;
pub mod catalogue;
pub mod clock;
pub mod iopool;
pub mod map;
pub mod mirror;
pub mod naming;
pub mod nas;
pub mod nasfs;
pub mod nasio;
pub mod pack;
pub mod pieces;
pub mod pmtiles;
pub mod range;
pub mod retry;
pub mod sect;
pub mod seglog;
pub mod spawn;
pub mod sys;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod waits;
pub mod whole;

pub use iopool::{IoError, IoPool};
pub use range::{MmapFile, PooledFile, RangeRead};
