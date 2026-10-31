//! Python's behaviour, ported: the pieces the pipeline's steps ported from Python use so their
//! outputs stay the bytes the Python made. No dependency on the pipeline (docs/crates.md, step 1);
//! the pipeline re-exports each module under its old path (`pipeline::pyjson`, …).

pub mod pyjson;
pub mod pynumpy;
pub mod pytext;
pub mod tpsimplify;
