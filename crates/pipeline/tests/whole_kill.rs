//! A writer killed mid-write (SIGKILL) never leaves a torn file: a child process (this test binary
//! again, `WHOLE_KILL_DIR` set) runs `store::whole::write` in a loop while this one reads the file; each
//! read is a whole version, and after the kill the file still is. On this Mac's disk, not over SMB:
//! it checks the write's own steps (a temporary name, flushed, checked, renamed), not the NAS.

use std::path::Path;
use std::time::{Duration, Instant};

/// A version: its length (8 bytes) and then one byte, repeated, so a torn file can't pass.
fn version(i: u64) -> Vec<u8> {
    let n = 64 << 10 | (i * 7919) % (512 << 10);
    let mut b = (n as u64).to_le_bytes().to_vec();
    b.resize(8 + n as usize, (i % 251) as u8);
    b
}

fn whole(b: &[u8]) -> bool {
    b.len() >= 8 && {
        let n = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize;
        b.len() == 8 + n && b[8..].iter().all(|&c| c == b[8])
    }
}

#[test]
fn writer() {
    let Some(dir) = std::env::var_os("WHOLE_KILL_DIR") else { return };
    let path = Path::new(&dir).join("f.json");
    for i in 0.. {
        store::whole::write(&path, &version(i)).unwrap();
        if i == 0 {
            // (The parent's kill clock starts once this one is writing.)
            std::fs::write(Path::new(&dir).join("started"), b"").unwrap();
        }
    }
}

#[test]
fn killed_writer_leaves_whole_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.json");
    store::whole::write(&path, &version(0)).unwrap();
    let exe = std::env::current_exe().unwrap();
    for round in 0..5 {
        let mut child = store::spawn::spawn(
            store::spawn::command(&exe)
                .args(["--exact", "writer", "--nocapture", "--test-threads=1"])
                .env("WHOLE_KILL_DIR", dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null()),
        )
        .unwrap();
        let started = dir.path().join("started");
        let wait = Instant::now() + store::testkit::WATCHDOG;
        while !started.exists() {
            assert!(Instant::now() < wait, "round {round}: the writer never started");
            let b = store::whole::read(&path).unwrap();
            assert!(whole(&b), "round {round}: a torn read ({} bytes)", b.len());
        }
        std::fs::remove_file(&started).unwrap();
        // (bound ok: how long it reads before the kill, not a wait)
        let until = Instant::now() + Duration::from_millis(300 + 97 * round);
        let mut reads = 0;
        while Instant::now() < until {
            let b = store::whole::read(&path).unwrap();
            assert!(whole(&b), "round {round}: a torn read ({} bytes)", b.len());
            reads += 1;
        }
        child.kill().unwrap(); // SIGKILL
        child.wait().unwrap();
        let b = std::fs::read(&path).unwrap();
        assert!(whole(&b), "round {round}: torn after the kill ({} bytes)", b.len());
        assert!(reads > 0);
    }
    // The killed writers' temporary files are left beside it, never under its name.
    for e in std::fs::read_dir(dir.path()).unwrap().flatten() {
        let p = e.path();
        assert!(p == path || store::whole::is_tmp(&p), "{}", p.display());
    }
}
