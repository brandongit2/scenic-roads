//! The NAS as the pool's protocol sees it (docs/pool.md §3): its operations, on the share and for
//! a reader (`Nas`, `ReadOnly`), are buildstore::records::nas's (task #916 step 6a, docs/crates.md
//! §6), kept at this path; what only the pool uses is here.

pub use buildstore::records::nas::*;

#[cfg(test)]
use anyhow::Result;

/// A NAS in memory without faults, for the modules' tests (the simulator has its own, with them).
#[cfg(test)]
#[derive(Default)]
pub struct Mem(pub std::cell::RefCell<std::collections::BTreeMap<String, Vec<u8>>>);

#[cfg(test)]
impl Nas for Mem {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        let mut m = self.0.borrow_mut();
        if m.contains_key(path) {
            return Ok(Created::There);
        }
        m.insert(path.to_string(), bytes.to_vec());
        Ok(Created::Made)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.0.borrow_mut().insert(path.to_string(), bytes.to_vec());
        Ok(())
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.borrow().get(path).cloned())
    }

    fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.0.borrow().contains_key(path))
    }
}
