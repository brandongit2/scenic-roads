//! A step's job opens its `Out` (buildstore::out, where build steps put their outputs) here,
//! held to its step's write set as the steps table has it (crate::agent::steps::writes).

use anyhow::Result;
use buildstore::out::Out;
use std::path::Path;
use store::nasfs::NasRoot;

/// A job of `step`'s outputs (buildstore::out::Out::open): it puts and removes only what the steps
/// table gives `step` (crate::agent::steps::writes, the function the lead's merge checks every
/// entry against, so the writer and the check can't disagree; docs/pool.md §7.3).
pub fn open(root: &NasRoot, scratch: &Path, step: &str) -> Result<Out> {
    Out::open(root, scratch, step, crate::agent::steps::writes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buildstore::records::read_manifest;

    /// `Out` opened here reads the pool's records and holds a job to its step's write set as the
    /// steps table has it (buildstore's tests check the layer itself with a table of their own).
    #[test]
    fn a_job_reads_the_records_and_writes_only_its_steps_outputs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(
            root.join("state/build/manifest.json"),
            r#"{"hidata/6-1-1": "hidata/6-1-1.1111111111111111.hidata"}"#,
        )
        .unwrap();
        let nr = NasRoot::local_for_test(&root);
        let mut out = open(&nr, &d.path().join("s"), "pack").unwrap();
        assert_eq!(out.get("hidata/6-1-1"), Some("hidata/6-1-1.1111111111111111.hidata"));
        assert_eq!(read_manifest(&nr).unwrap().len(), 1);
        out.put_bytes("layers/roads/hi/6-1-2", "pack", b"roads").unwrap();
        let e = format!("{:#}", out.put_bytes("layers/terrain/hi/6-1-2", "pack", b"t").unwrap_err());
        assert!(e.contains("writes layers/terrain/hi/6-1-2, outside pack's outputs"), "{e}");
        out.save().unwrap();
        assert!(read_manifest(&nr).unwrap().contains_key("layers/roads/hi/6-1-2"));
    }
}
