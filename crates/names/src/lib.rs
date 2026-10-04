//! Display names (docs/plan.md §7): each name gets a main label and an optional sub line, from the
//! user's translation files (the server's local copy of the NAS folder), else the thing's own
//! English.
//! `mvt` attaches them to vector tiles as they're served.
//!
//! - [`area`]: which area's table a name is read by, from where the thing is ([`area_at`]).
//! - [`display`]: the tables ([`Names`]: loading, refreshing, versions for ETags), places' and
//!   roads' kept apart ([`Kind`]), and the display rule ([`Names::display`], [`same_name`]).
//! - [`mvt`]: vector tiles: decoding and encoding, [`mvt::attach`] and [`mvt::merge`].

pub mod area;
pub mod display;
pub mod mvt;
mod pbf;
mod table;

pub use area::{area_at, areas_in, is_area, AREAS};
pub use display::{in_parts, same_name, AreaSummary, Display, DisplayRef, Kind, Names, Translation, STABLE};

/// A temporary folder of translation files for tests.
#[cfg(test)]
pub(crate) mod testdir {
    use std::fs::{self, File, FileTimes};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, SystemTime};

    /// A folder under the system's temporary directory, removed when dropped.
    pub(crate) struct Dir(pub PathBuf);

    impl Dir {
        pub fn new() -> Dir {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!("names-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("temp dir");
            Dir(p)
        }

        /// Writes a file last modified an hour ago (long settled).
        pub fn write(&self, rel: &str, text: &str) {
            self.write_aged(rel, text, Duration::from_secs(3600));
        }

        /// Writes a file last modified `age` ago.
        pub fn write_aged(&self, rel: &str, text: &str, age: Duration) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(&p, text).expect("write");
            let f = File::options().write(true).open(&p).expect("open");
            f.set_times(FileTimes::new().set_modified(SystemTime::now() - age)).expect("set mtime");
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
