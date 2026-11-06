//! The build's files (docs/crates.md, task #916 step 4): the layer above `store` that every step
//! writes and reads through. Its outputs on the NAS and the manifest naming them (out, handoff),
//! the raw tiles' and pyramids' archives (rawpack), this Mac's copies of NAS files (localread), a
//! job's cost inputs (costin), the coverage and the outlines it is drawn from, a region as the
//! build reads it (recipes), the inputs folders a step never reads (inputs_folder), the network's
//! fetches (fetch), GeoTIFF reading and writing (geotiff), and a bounded `ssh` to the NAS
//! (nasssh), the build's records as a reader finds them (records: the NAS's operations, the
//! current term, the snapshot read now), and the free space a step checks before it writes (room:
//! a disk's, the NAS's past the owner's reserve). Pipeline re-exports the moved modules at their old paths;
//! a step's write set, the steps table's above, is handed to `out::Out::open` (pipeline::out::open).

pub mod costin;
pub mod coverage;
pub mod fetch;
pub mod geotiff;
pub mod handoff;
pub mod inputs_folder;
pub mod localread;
pub mod nasssh;
pub mod out;
pub mod outlines;
pub mod rawpack;
pub mod recipes;
pub mod records;
pub mod room;
