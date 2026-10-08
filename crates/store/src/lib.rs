//! NAS storage for the map (docs/plan.md §3–4, docs/formats.md): content-named immutable files,
//! packs of tiles, sectioned files, catalogs, the bounded NAS I/O pool, finding and mounting the
//! share, the per-Mac mirror and PMTiles reading.

pub mod blobs;
pub mod catalog;
pub mod iopool;
pub mod mirror;
pub mod naming;
pub mod nas;
pub mod pack;
pub mod pieces;
pub mod pmtiles;
pub mod range;
pub mod sect;
pub mod sys;

pub use iopool::{IoError, IoPool};
pub use range::{MmapFile, PooledFile, RangeRead};
