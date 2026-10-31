//! The map server runs on the map's Mac alone (docs/macs.md, Devices; task #874): the Mac mini.
//! Every Mac ran one until #874; an installed server started on another Mac retires itself: its
//! LaunchAgent (`local.scenic.server`) and run file are removed and the job unloaded, which ends
//! it. The Mac's offline map (its mirror, downloads and followed map) is left where it is.
//!
//! The live map must never go down for a file gone missing, so a server retires only when it's
//! sure it isn't the map's Mac (`retires`): no name in `~/.config/scenic/names` (the file there
//! and without one, or not there; one that can't be read is no answer), and the pool's declared
//! lead (`state/build/lead-member.json`) read and naming another member than this Mac's own (its
//! member file read). Anything unknown keeps it serving.

use std::path::{Path, PathBuf};

pub const LABEL: &str = "local.scenic.server";

/// Whether the names file's read (`~/.config/scenic/names`) says this Mac serves no name: there
/// and without one, or not there. One that can't be read says nothing.
fn names_none(names: &std::io::Result<String>) -> bool {
    match names {
        Ok(t) => pipeline::net::names_in(t).is_empty(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Whether the server on this Mac retires: `names` is the names file's read, `lead` the declared
/// lead's member id (None: unread), `me` this Mac's member id (None: unread).
pub fn retires(names: &std::io::Result<String>, lead: Option<&str>, me: Option<&str>) -> bool {
    names_none(names) && matches!((lead, me), (Some(l), Some(m)) if l != m)
}

/// `retires` for this Mac: its names file in `user_home`, its member id in `home/agent`, the lead
/// declared on the NAS (no NAS, or no declaration read: it stays).
pub fn this_mac_retires(home: &Path, user_home: &Path) -> bool {
    let names = std::fs::read_to_string(user_home.join(pipeline::net::NAMES));
    if !names_none(&names) {
        return false;
    }
    let me = pipeline::pool::read_member_id(&home.join("agent"));
    let lead = me.as_ref().and_then(|_| {
        let r = store::nasfs::NasRoot::find(false)?;
        pipeline::pool::term::declared(&r).ok().flatten().map(|d| d.member)
    });
    retires(&names, lead.as_deref(), me.as_deref())
}

/// What retiring removes: the server's run file in the app's folder (`home`) and its LaunchAgent
/// in the user's (`user_home`).
/// The offline map (`mirror/`, `idx/`, `downloads.json`, `~/Library/Caches/scenic/map`) stays.
pub fn its_files(home: &Path, user_home: &Path) -> Vec<PathBuf> {
    vec![home.join("run/server"), user_home.join(format!("Library/LaunchAgents/{LABEL}.plist"))]
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

/// Retires this Mac's server (not the map's Mac): its service's files removed, its job unloaded
/// (which ends this process); if that fails, it exits, and the launcher, finding no run file,
/// starts nothing again.
pub fn retire(home: &Path, user_home: &Path) -> ! {
    eprintln!(
        "server: this Mac doesn't serve the map (only the Mac mini does, task #874): retiring, its offline map left"
    );
    for p in delete(&its_files(home, user_home)) {
        eprintln!("server: removed {}", p.display());
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
    fn only_a_mac_sure_it_isnt_the_lead_retires() {
        use std::io::{Error, ErrorKind};
        let gone = || Err(Error::from(ErrorKind::NotFound));
        let named: std::io::Result<String> = Ok("# Written by tools/caddy/install.sh\nmap.example.net\n".into());
        let empty: std::io::Result<String> = Ok("# Written by tools/caddy/install.sh\n\n".into());
        // A member that isn't the declared lead, without a name: it retires.
        assert!(retires(&gone(), Some("m-lead"), Some("m-other")));
        assert!(retires(&empty, Some("m-lead"), Some("m-other")), "comments alone");
        // A name: it serves, whatever the lead.
        assert!(!retires(&named, Some("m-lead"), Some("m-other")));
        // The declared lead serves with its names file gone.
        assert!(!retires(&gone(), Some("m-lead"), Some("m-lead")));
        // An unreadable names file, an unread lead or member id: no answer, it serves.
        assert!(!retires(&Err(Error::from(ErrorKind::PermissionDenied)), Some("m-lead"), Some("m-other")));
        assert!(!retires(&gone(), None, Some("m-other")));
        assert!(!retires(&gone(), Some("m-lead"), None));
    }

    #[test]
    fn retiring_removes_the_service_and_leaves_the_map() {
        let (home, u) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for d in ["mirror/ab", "idx", "agent/logs", "app/20261030-0000-aaaaaaa", "run", "catalog"] {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        let kept = ["mirror/ab/x.pack", "downloads.json", "reserve.json", "map-page", "run/agent", "agent/status.json"];
        for f in kept.iter().chain(&["run/server"]) {
            std::fs::write(home.path().join(f), b"x").unwrap();
        }
        std::fs::create_dir_all(u.path().join("Library/Caches/scenic/map")).unwrap();
        std::fs::create_dir_all(u.path().join("Library/LaunchAgents")).unwrap();
        std::fs::write(u.path().join(format!("Library/LaunchAgents/{LABEL}.plist")), b"x").unwrap();
        std::fs::write(u.path().join("Library/LaunchAgents/local.scenic.caddy.plist"), b"x").unwrap();
        let gone = delete(&its_files(home.path(), u.path()));
        assert_eq!(gone.len(), 2, "{gone:?}");
        assert!(!home.path().join("run/server").exists());
        assert!(!u.path().join(format!("Library/LaunchAgents/{LABEL}.plist")).exists());
        for p in kept.iter().chain(&["idx", "app/20261030-0000-aaaaaaa", "catalog"]) {
            assert!(home.path().join(p).exists(), "{p} kept");
        }
        assert!(u.path().join("Library/Caches/scenic/map").exists(), "the followed map kept");
        assert!(u.path().join("Library/LaunchAgents/local.scenic.caddy.plist").exists());
    }
}
