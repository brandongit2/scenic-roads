//! NAS storage for the map (docs/storage.md–4, docs/formats.md): content-named immutable files,
//! packs of tiles, sectioned files, catalogues, the map and its log (docs/formats.md, The map and its log), the one
//! deadline every NAS call is made under (`deadline`), the NAS I/O pool, finding and mounting the share, the per-Mac mirror and PMTiles reading, and the
//! reads made ahead of the compute using them (`ahead`).

pub mod ahead;
pub mod aimd;
pub mod blobs;
pub mod cachefile;
pub mod catalogue;
pub mod clock;
pub mod deadline;
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
