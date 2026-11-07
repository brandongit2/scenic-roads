//! This Mac's member id (`member_id`, kept in the agent's folder with the Mac it's of) and a
//! process's hold on its member (`MemberLock`): the pool's (docs/pool.md), here since task #916
//! step 7a so the map server reads the id without the runtime; agentcore::pool re-exports them.

use super::is_member_id;
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

/// The file in the agent's local folder that keeps this Mac's member id, and the Mac it's of.
const ID_FILE: &str = "member";

/// This Mac's member id (`m-<16 hex>`), kept in `home` (the agent's local folder) with the Mac's
/// hardware UUID (IOPlatformUUID), and made the first time, or again when the file names another
/// Mac: a copy Migration Assistant brought to a new Mac would make the two Macs one member. An
/// error when the file is there but can't be read now (a new id would make this Mac another
/// member).
pub fn member_id(home: &Path) -> Result<String> {
    member_id_on(home, platform_uuid())
}

/// This Mac's member id as its file in `home` (the agent's folder) says, made by none: None when
/// there's none yet, or it can't be read, or it names another Mac.
pub fn read_member_id(home: &Path) -> Option<String> {
    let s = std::fs::read_to_string(home.join(ID_FILE)).ok()?;
    let mut lines = s.lines().map(str::trim);
    let (id, of) = (lines.next().unwrap_or(""), lines.next().filter(|u| !u.is_empty()));
    let ours = match (of, platform_uuid()) {
        (Some(of), Some(mac)) => of == mac,
        _ => true,
    };
    (is_member_id(id) && ours).then(|| id.to_string())
}

/// `member_id` on the Mac whose hardware UUID is `mac` (None: it can't be read now).
fn member_id_on(home: &Path, mac: Option<&str>) -> Result<String> {
    let p = home.join(ID_FILE);
    match std::fs::read_to_string(&p) {
        Ok(s) => {
            let mut lines = s.lines().map(str::trim);
            let (id, of) = (lines.next().unwrap_or(""), lines.next().filter(|u| !u.is_empty()));
            match (of, mac) {
                // (Empty or cut short: its first write didn't finish, so nothing was named by it.)
                _ if !is_member_id(id) => {}
                // Another Mac's: this one is a member of its own.
                (Some(of), Some(mac)) if of != mac => {}
                // This Mac's; or this Mac's UUID can't be read now, and a new id would make it
                // another member.
                (Some(_), _) | (None, None) => return Ok(id.to_string()),
                // Made before its Mac could be read: bound now.
                (None, Some(mac)) => {
                    ::store::whole::write(&p, format!("{id}\n{mac}\n").as_bytes())?;
                    return Ok(id.to_string());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
    }
    std::fs::create_dir_all(home).with_context(|| format!("make {}", home.display()))?;
    let id = new_id();
    ::store::whole::write(&p, format!("{id}\n{}\n", mac.unwrap_or("")).as_bytes())?;
    Ok(id)
}

/// This Mac's hardware UUID (IOPlatformUUID: the same across reinstalls, another on another Mac);
/// None when it can't be read.
fn platform_uuid() -> Option<&'static str> {
    static UUID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    UUID.get_or_init(|| {
        let o = ::store::spawn::output(::store::spawn::command("/usr/sbin/ioreg").args([
            "-rd1",
            "-c",
            "IOPlatformExpertDevice",
        ]))
        .ok()?;
        parse_uuid(&String::from_utf8_lossy(&o.stdout))
    })
    .as_deref()
}

/// The IOPlatformUUID in `ioreg`'s output.
fn parse_uuid(ioreg: &str) -> Option<String> {
    ioreg
        .lines()
        .find_map(|l| l.split_once("\"IOPlatformUUID\" = "))
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .filter(|u| u.len() >= 32 && u.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
}

/// This process's hold on its member (agentcore::pool::driver's contract: one process per member): an
/// exclusive flock on `pool-<id>.lock` in the folder it's given: the app's, which the agent passes
/// (`~/Library/Application Support/scenic`: above every copy of the agent's folder on this Mac, and
/// not the temporary folder, which macOS empties of files three days old), held while the process
/// runs, so a second process of the member (a second agent, or one started from a copy of the
/// agent's folder) can't take it. The driver checks it at every step (`check`). Let go when
/// dropped.
#[derive(Debug)]
pub struct MemberLock {
    lock: jobrt::sys::PathLock,
}

impl MemberLock {
    /// Member `id`'s lock, in folder `dir` (the app's); None when another process holds it.
    pub fn take(dir: &Path, id: &str) -> Result<Option<MemberLock>> {
        anyhow::ensure!(is_member_id(id), "{id:?} isn't a member id");
        std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
        let path = dir.join(format!("pool-{id}.lock"));
        Ok(lock_file(&path)?.map(|lock| MemberLock { lock }))
    }

    /// Whether this process still holds it: true when its path still names the file it holds, or
    /// one removed or replaced (by hand, or a cleanup) was taken again, no other process holding
    /// it; false when another process holds it (this one isn't the member's only one, and stops);
    /// an error when it can't tell (its folder unreadable a moment: checked again).
    pub fn check(&mut self) -> Result<bool> {
        match std::fs::metadata(self.lock.path()) {
            Ok(_) if self.lock.holds() => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("stat {}", self.lock.path().display())),
        }
        match lock_file(self.lock.path())? {
            Some(lock) => {
                self.lock = lock;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Its file.
    pub fn path(&self) -> &Path {
        self.lock.path()
    }
}

/// The file at `path`, made if it isn't there, locked (jobrt::sys::PathLock: its file removed as
/// it's let go); None when another process holds its lock.
fn lock_file(path: &Path) -> Result<Option<jobrt::sys::PathLock>> {
    jobrt::sys::PathLock::take(path, false).with_context(|| format!("lock {}", path.display()))
}

/// A new member id: the system's random bytes, with the time, the process and the host name in
/// case they can't be read.
fn new_id() -> String {
    let mut seed = Vec::new();
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok() {
        seed.extend_from_slice(&b);
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    seed.extend_from_slice(&t.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(::store::sys::host().as_bytes());
    format!("m-{}", ::store::naming::hash16(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_id_is_made_once_and_kept() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let id = member_id(&home).unwrap();
        assert!(is_member_id(&id), "{id}");
        assert_eq!(member_id(&home).unwrap(), id);
        // Another Mac (another home) is another member.
        assert_ne!(member_id(&d.path().join("other")).unwrap(), id);
        // A file left empty by a first write cut short: made again.
        std::fs::write(home.join(ID_FILE), b"").unwrap();
        let again = member_id(&home).unwrap();
        assert!(is_member_id(&again) && again != id);
        // One that can't be read now is an error, not a new member.
        std::fs::remove_file(home.join(ID_FILE)).unwrap();
        std::fs::create_dir(home.join(ID_FILE)).unwrap();
        assert!(member_id(&home).is_err());
        assert!(!is_member_id("m-3F9C000000000000") && !is_member_id("m-3f9c") && !is_member_id("3f9c000000000000"));
    }

    #[test]
    fn a_member_is_one_process() {
        // (Review N7: nothing kept a second process of a member from stepping a driver.)
        let dir = tempfile::tempdir().unwrap();
        let id = new_id();
        let held = MemberLock::take(dir.path(), &id).unwrap().expect("free");
        assert!(MemberLock::take(dir.path(), &id).unwrap().is_none(), "held by another");
        assert!(MemberLock::take(dir.path(), &new_id()).unwrap().is_some(), "another member's is its own");
        drop(held);
        assert!(MemberLock::take(dir.path(), &id).unwrap().is_some(), "let go");
        assert!(MemberLock::take(dir.path(), "Mac-mini").is_err());
    }

    #[test]
    fn a_member_lock_whose_file_is_removed_is_taken_again_or_stops_its_process() {
        // macOS empties the temporary folder of files three days old, and the agent holds its lock
        // for longer: removed, a second process could take a lock of its own. (Re-review F2.)
        let dir = tempfile::tempdir().unwrap();
        let id = new_id();
        let path = dir.path().join(format!("pool-{id}.lock"));
        let mut held = MemberLock::take(dir.path(), &id).unwrap().expect("free");
        assert!(held.check().unwrap());
        std::fs::remove_file(&path).unwrap();
        assert!(held.check().unwrap(), "no other process: taken again");
        assert!(MemberLock::take(dir.path(), &id).unwrap().is_none(), "held again");
        std::fs::remove_file(&path).unwrap();
        let second = MemberLock::take(dir.path(), &id).unwrap().expect("a second process takes the new file");
        assert!(!held.check().unwrap(), "another process is the member: this one stops");
        drop(second);
    }

    #[test]
    fn a_member_id_copied_to_another_mac_is_another_member() {
        // Migration Assistant copies the agent's folder to a new Mac. (Review L3: the id was a
        // plain file, and the two Macs were one member.)
        const OLD: &str = "6A1F0C2E-0000-4000-8000-00000000000A";
        const NEW: &str = "6A1F0C2E-0000-4000-8000-00000000000B";
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let id = member_id_on(&home, Some(OLD)).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(ID_FILE)).unwrap(), format!("{id}\n{OLD}\n"));
        assert_eq!(member_id_on(&home, Some(OLD)).unwrap(), id);
        // Its UUID not read now: kept.
        assert_eq!(member_id_on(&home, None).unwrap(), id);
        // On the new Mac: a new member, bound to it.
        let copy = d.path().join("copy");
        std::fs::create_dir(&copy).unwrap();
        std::fs::copy(home.join(ID_FILE), copy.join(ID_FILE)).unwrap();
        let other = member_id_on(&copy, Some(NEW)).unwrap();
        assert!(is_member_id(&other) && other != id);
        assert_eq!(member_id_on(&copy, Some(NEW)).unwrap(), other);
        assert_eq!(member_id_on(&home, Some(OLD)).unwrap(), id, "the old Mac keeps its own");
        // One made while no UUID could be read: bound when one can.
        let early = d.path().join("early");
        let e = member_id_on(&early, None).unwrap();
        assert_eq!(member_id_on(&early, Some(NEW)).unwrap(), e);
        assert_eq!(std::fs::read_to_string(early.join(ID_FILE)).unwrap(), format!("{e}\n{NEW}\n"));
        assert_eq!(parse_uuid("    \"IOPlatformSerialNumber\" = \"X\"\n    \"IOPlatformUUID\" = \"6A1F0C2E-0000-4000-8000-00000000000A\"\n").as_deref(), Some(OLD));
        assert_eq!(parse_uuid("nothing"), None);
    }
}
