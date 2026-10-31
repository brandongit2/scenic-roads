//! The map server runs on the map's Mac alone (docs/macs.md, Devices; task #874): the Mac mini, the
//! one that serves the map's own name (`~/.config/scenic/names`, written by tools/caddy/install.sh).
//! Every Mac ran one until #874; an installed server started on another Mac retires itself: what
//! only the map server kept on that Mac is deleted (its offline copy, its downloads and settings,
//! the map it followed), then its LaunchAgent (`local.scenic.server`) and run file are removed and
//! the job unloaded, which ends it.

use std::path::{Path, PathBuf};

pub const LABEL: &str = "local.scenic.server";

/// Whether this Mac serves the map: its names file (pipeline::net::names) names one.
pub fn the_maps_mac(user_home: &Path) -> bool {
    std::fs::read_to_string(user_home.join(".config/scenic/names"))
        .is_ok_and(|s| s.lines().any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')))
}

/// What only the map server keeps in the app's folder (`home`) and the user's (`user_home`).
pub fn its_files(home: &Path, user_home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "mirror",
        "idx",
        "downloads.json",
        "downloads.json.bad",
        "reserve.json",
        "map-page",
        "accepted",
        "names",
        "region-areas.json",
        "run/server",
        pipeline::appupdate::SERVER_APP,
    ]
    .iter()
    .map(|p| home.join(p))
    .collect();
    v.push(user_home.join("Library/Caches/scenic/map"));
    v.push(user_home.join(format!("Library/LaunchAgents/{LABEL}.plist")));
    v
}

/// Deletes what `its_files` names (each that's there): what was deleted.
pub fn delete(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut gone = Vec::new();
    for p in files {
        let r = match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
            Ok(_) => std::fs::remove_file(p),
            Err(_) => continue,
        };
        match r {
            Ok(()) => gone.push(p.clone()),
            Err(e) => eprintln!("server: retiring: {}: {e}", p.display()),
        }
    }
    gone
}

/// Retires this Mac's server (not the map's Mac): its files deleted, its job unloaded (which
/// ends this process); if that fails, it exits, and the launcher, finding no run file, starts
/// nothing again.
pub fn retire(home: &Path, user_home: &Path) -> ! {
    eprintln!("server: this Mac doesn't serve the map (only the Mac mini does, task #874): retiring");
    for p in delete(&its_files(home, user_home)) {
        eprintln!("server: deleted {}", p.display());
    }
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let r =
        store::spawn::status(store::spawn::command("/bin/launchctl").args(["bootout", &format!("gui/{uid}/{LABEL}")]));
    eprintln!("server: launchctl bootout: {r:?}");
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_mac_with_a_name_serves_the_map() {
        let u = tempfile::tempdir().unwrap();
        assert!(!the_maps_mac(u.path()));
        std::fs::create_dir_all(u.path().join(".config/scenic")).unwrap();
        std::fs::write(u.path().join(".config/scenic/names"), "# Written by tools/caddy/install.sh\n\n").unwrap();
        assert!(!the_maps_mac(u.path()), "comments alone");
        std::fs::write(u.path().join(".config/scenic/names"), "# Written by tools/caddy/install.sh\nmap.example.net\n")
            .unwrap();
        assert!(the_maps_mac(u.path()));
    }

    #[test]
    fn retiring_deletes_the_servers_files_alone() {
        let (home, u) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for d in ["mirror/ab", "idx", "agent/logs", "app/20261030-0000-aaaaaaa", "run", "catalog"] {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        for f in [
            "mirror/ab/x.pack",
            "downloads.json",
            "reserve.json",
            "map-page",
            "run/server",
            "run/agent",
            "agent/status.json",
        ] {
            std::fs::write(home.path().join(f), b"x").unwrap();
        }
        std::fs::create_dir_all(u.path().join("Library/Caches/scenic/map")).unwrap();
        std::fs::create_dir_all(u.path().join("Library/LaunchAgents")).unwrap();
        std::fs::write(u.path().join(format!("Library/LaunchAgents/{LABEL}.plist")), b"x").unwrap();
        std::fs::write(u.path().join("Library/LaunchAgents/local.scenic.caddy.plist"), b"x").unwrap();
        let gone = delete(&its_files(home.path(), u.path()));
        assert_eq!(gone.len(), 8, "{gone:?}");
        for p in ["mirror", "idx", "downloads.json", "run/server"] {
            assert!(!home.path().join(p).exists(), "{p}");
        }
        for p in ["agent/status.json", "run/agent", "app/20261030-0000-aaaaaaa", "catalog"] {
            assert!(home.path().join(p).exists(), "{p} kept");
        }
        assert!(!u.path().join("Library/Caches/scenic/map").exists());
        assert!(u.path().join("Library/LaunchAgents/local.scenic.caddy.plist").exists());
    }
}
