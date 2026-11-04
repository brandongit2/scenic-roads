//! Where build steps put their outputs: buildstore::out (task #916 step 4, docs/crates.md), its
//! `Out` reading what's above that crate through `Pipeline`: the pool's records
//! (crate::pool::records) and each step's write set (crate::agent::steps::writes). Every other
//! item is buildstore's, at this path.

pub use buildstore::out::*;

use anyhow::Result;
use std::collections::BTreeMap;
use store::nasfs::NasRoot;

/// The outputs of a run, as every step and tool here opens them.
pub type Out = buildstore::out::Out<Pipeline>;

/// The records and write sets buildstore's `Out` reads: the pool's and the agent's.
pub enum Pipeline {}

impl Above for Pipeline {
    /// The build's records under `root` as a reader has them now (crate::pool::records::Records::now),
    /// the parts an `Out` reads.
    fn records(root: &NasRoot) -> Result<Recorded> {
        let r = crate::pool::records::Records::now(root)?;
        Ok(Recorded { manifest: r.manifest, pending: r.pending, sizes: r.sizes })
    }

    fn writes(step: &str, logical: &str, removal: bool) -> Option<bool> {
        crate::agent::steps::writes(step, logical, removal)
    }
}

/// The records' manifest under `root` (crate::pool::records::Brief::now: the current term's brief,
/// a quarter of its snapshot's bytes), read only: the accepted versions, for a reader that needs no
/// `Out`.
pub fn read_manifest(root: &NasRoot) -> Result<BTreeMap<String, String>> {
    Ok(crate::pool::records::Brief::now(root)?.manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut out = Out::open(&nr, &d.path().join("s"), "pack").unwrap();
        assert_eq!(out.get("hidata/6-1-1"), Some("hidata/6-1-1.1111111111111111.hidata"));
        assert_eq!(read_manifest(&nr).unwrap().len(), 1);
        out.put_bytes("layers/roads/hi/6-1-2", "pack", b"roads").unwrap();
        let e = format!("{:#}", out.put_bytes("layers/terrain/hi/6-1-2", "pack", b"t").unwrap_err());
        assert!(e.contains("writes layers/terrain/hi/6-1-2, outside pack's outputs"), "{e}");
        out.save().unwrap();
        assert!(read_manifest(&nr).unwrap().contains_key("layers/roads/hi/6-1-2"));
    }
}
