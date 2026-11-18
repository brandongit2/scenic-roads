//! The OSM pass's planet (docs/inputs.md §6.9, task #1120): a source declaration of the
//! `osm-planet` role (`inputs/sources/osm-planet.toml`) pins it, its `version` the planet's date,
//! its `urls` where it's fetched from in turn (the mirrors, then the main site) and `params.md5`
//! its published MD5; a new planet is the owner's edit of it.
//!
//! The `planet-fetch` step (the lead's own job, a fetch the pool's downloads cap counts) brings
//! the declared planet once into `sources/osm/<date>/planet.osm.pbf`: by the build's fetcher
//! (buildstore::fetch::resume: its User-Agent, 429 and 5xx answers waited out), into a part
//! (`planet.osm.pbf.part`) that a stopped run, or the next URL, goes on from; checked against its
//! MD5, read back from the NAS, before it takes its name (a mismatch starts the part over from the
//! next URL). It's planned only when the NAS has room for it past the owner's reserve (`room`, by
//! the last planet's size; the fetch checks again by the length the server gives). The pass then
//! reads it (crate::osmpass): its date is the pass's, its path the pass's key, as they were before
//! the declaration, so declaring the planet the build had rebuilt nothing.

use crate::inputs::sources::{Data, Source, OSM_PLANET};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use store::nasfs::{NasPath, NasRoot};

/// A planet's file, in its date's folder.
pub const FILE: &str = "planet.osm.pbf";
/// Its part while it's fetched, beside it.
pub const PART: &str = "planet.osm.pbf.part";

/// Where the planet of `date` is kept on the NAS.
pub fn path(root: &NasRoot, date: &str) -> NasPath {
    root.join("sources/osm").join(date).join(FILE)
}

/// The declared planet.
#[derive(Clone, Debug, PartialEq)]
pub struct Planet {
    /// Its date (the declaration's `version`): the pass's.
    pub date: String,
    /// Where it's fetched from, in turn.
    pub urls: Vec<String>,
    /// Where its MD5 is published (`<hex>  <name>`).
    pub md5: String,
}

impl Planet {
    /// The planet accepted source `s` (of the `osm-planet` role) declares.
    pub fn of(s: &Source) -> Result<Planet> {
        let urls = if s.data.urls.is_empty() { vec![s.url()?.to_string()] } else { s.data.urls.clone() };
        Ok(Planet { date: s.version()?.to_string(), urls, md5: s.param_str("md5")?.to_string() })
    }
}

/// What an `osm-planet` declaration's `[data]` gets wrong past the shape every source has: its
/// `version` a date (YYYY-MM-DD, the pass's), its URLs named (`url` or `urls`, not an index), its
/// `params.md5` an http or https URL and no other parameter.
pub fn params_problems(d: &Data) -> Vec<String> {
    let mut why = Vec::new();
    match &d.version {
        Some(v) if crate::osmpass::is_date(v) => {}
        Some(v) => why.push(format!("`version` is \"{v}\": the planet's date, YYYY-MM-DD")),
        None => why.push("no `version`: the planet's date, YYYY-MM-DD".into()),
    }
    if d.index.is_some() {
        why.push("names an `index`: a planet's `url` or `urls`".into());
    }
    match d.params.get("md5").map(|v| v.as_str()) {
        Some(Some(u)) if u.starts_with("https://") || u.starts_with("http://") => {}
        Some(_) => why.push("`params.md5` isn't an http or https URL".into()),
        None => why.push("no `params.md5`: where the planet's MD5 is published".into()),
    }
    why.extend(
        d.params.keys().filter(|k| *k != "md5").map(|k| format!("`params.{k}` isn't a planet's parameter (md5)")),
    );
    why
}

/// The planet the accepted declarations the records `manifest` name pin: None while none
/// declares one (no version accepted yet, or none of the `osm-planet` role).
pub fn declared(root: &NasRoot, manifest: &BTreeMap<String, String>) -> Result<Option<Planet>> {
    let Some(s) = crate::inputs::sources::open(root, manifest)? else { return Ok(None) };
    s.role(OSM_PLANET).ok().map(Planet::of).transpose()
}

/// What the build does with declared planet `p`, the newest complete pass's date `have`.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Fetch it: the NAS lacks it.
    Fetch,
    /// Its pass, of the planet there.
    Pass(NasPath),
    /// Nothing: a newer planet's pass is complete (passes only go forward).
    Behind,
}

/// `p`'s next step (None: its pass is complete).
pub fn next(root: &NasRoot, p: &Planet, have: Option<&str>) -> Option<Next> {
    match have {
        Some(h) if h == p.date => None,
        Some(h) if h > p.date.as_str() => Some(Next::Behind),
        _ => {
            let at = path(root, &p.date);
            Some(if at.exists() { Next::Pass(at) } else { Next::Fetch })
        }
    }
}

/// The room the fetch of the planet of a date takes on the NAS, as the plan measures it before
/// planning the fetch (docs/inputs.md §6.9): the size of the planet of the newest complete pass
/// `have`, which a newer planet is about (a week's planet is a fraction of a GB bigger than the
/// last), less what the date's part holds already. None when the NAS holds no planet to measure by
/// (a first fetch): the fetch's own check stands then (`fetch`, by the length the server gives).
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    /// The size measured: the planet of `by`'s.
    pub size: u64,
    pub by: String,
    /// What the part holds.
    pub held: u64,
}

impl Room {
    /// The bytes still to fetch.
    pub fn left(&self) -> u64 {
        self.size.saturating_sub(self.held)
    }
}

/// `Room`'s measure.
pub fn room(root: &NasRoot, date: &str, have: Option<&str>) -> Option<Room> {
    let by = have?;
    let size = path(root, by).metadata().ok()?.len();
    let held = path(root, date).with_file_name(PART).metadata().map_or(0, |m| m.len());
    Some(Room { size, by: by.to_string(), held })
}

/// A size in GB, for the fetch's words.
fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
}

/// Planet `p` fetched into its place under `root` (`path`), once: nothing when it's there. Each
/// URL in turn goes on with the part (a mirror's file is the main site's); one the server hasn't,
/// or that fails past its retries, gives way to the next; the part whole, its MD5 is read back and
/// checked, and a mismatch removes it, the next URL starting over. Before each answer is written,
/// the bytes still to come (the length the server gives, less the part's) must fit in the NAS's
/// free space past the owner's reserve (`scenic nas-room`: buildstore::room::nas_room): short of
/// it, the fetch stops there, nothing written. An error when no URL brought it whole and right
/// (the part kept for the next run). `say` is told what happened.
pub fn fetch(root: &NasRoot, p: &Planet, say: &dyn Fn(&str)) -> Result<()> {
    let at = path(root, &p.date);
    if at.exists() {
        say(&format!("{} is there already", at.rel()));
        return Ok(());
    }
    let dir = at.parent().context("the planet's folder")?;
    dir.create_dir_all().with_context(|| dir.display().to_string())?;
    let want = md5_of(&buildstore::fetch::get(&p.md5)?.with_context(|| format!("{}: none there", p.md5))?)
        .with_context(|| format!("{}: not an MD5 file", p.md5))?;
    let part = dir.join(PART);
    // (Set once the room fell short: every URL's file is the same, so none is tried past it.)
    let short = std::cell::Cell::new(false);
    let fits = |left: u64| -> Result<()> {
        let room = buildstore::room::nas_room(root);
        match room {
            Ok(r) if left <= r => Ok(()),
            Ok(r) => {
                short.set(true);
                bail!(
                    "the NAS has {} free past its reserve (`scenic nas-room`): too little for the {} of the planet still to fetch",
                    gb(r),
                    gb(left)
                )
            }
            Err(e) => {
                short.set(true);
                Err(anyhow::Error::from(e).context("the NAS's free space"))
            }
        }
    };
    let mut failed = Vec::new();
    for u in &p.urls {
        let coming = |got: u64, of: u64| jobrt::jobs::report(got >> 20, of >> 20, "MB of the planet fetched");
        match buildstore::fetch::resume(u, &part, &fits, &coming) {
            Ok(true) => {}
            Ok(false) => {
                say(&format!("{u}: the server hasn't it"));
                failed.push(format!("{u}: none there"));
                continue;
            }
            Err(e) if short.get() => bail!("the planet of {} wasn't fetched: {e:#}", p.date),
            Err(e) => {
                say(&format!("{u}: {e:#}; the next URL goes on from the part"));
                failed.push(format!("{u}: {e:#}"));
                continue;
            }
        }
        let got = md5_file(&part)?;
        if got != want {
            say(&format!(
                "{u}: the planet's MD5 is {got}, published {want}: the part removed, the next URL starts over"
            ));
            failed.push(format!("{u}: MD5 {got}, not {want}"));
            part.remove_file().with_context(|| part.display().to_string())?;
            continue;
        }
        part.rename(&at).with_context(|| format!("{} → {}", part.display(), at.display()))?;
        say(&format!("{} fetched from {u}, its MD5 {want}", at.rel()));
        return Ok(());
    }
    bail!("the planet of {} wasn't fetched: {}", p.date, failed.join("; "))
}

/// The MD5 an MD5 file gives (its first word: 32 hex digits), in lower case.
fn md5_of(b: &[u8]) -> Option<String> {
    let w = std::str::from_utf8(b).ok()?.split_whitespace().next()?;
    (w.len() == 32 && w.bytes().all(|c| c.is_ascii_hexdigit())).then(|| w.to_ascii_lowercase())
}

/// File `p`'s MD5 (lower-case hex), read whole from the NAS, saying how far it is.
fn md5_file(p: &NasPath) -> Result<String> {
    use md5::Digest;
    use std::io::Read;
    let total = p.metadata().with_context(|| p.display().to_string())?.len();
    let mut f = p.open().with_context(|| p.display().to_string())?;
    let mut h = md5::Md5::new();
    let mut b = vec![0u8; 16 << 20];
    let (mut done, mut told) = (0u64, std::time::Instant::now());
    loop {
        let n = f.read(&mut b).with_context(|| p.display().to_string())?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
        done += n as u64;
        if told.elapsed() >= std::time::Duration::from_secs(1) {
            told = std::time::Instant::now();
            jobrt::jobs::report(done >> 20, total >> 20, "MB of the planet checked");
        }
    }
    Ok(h.finalize().iter().map(|x| format!("{x:02x}")).collect())
}

/// The staged declaration with its planet's date `date` in place of its own, accepted (as the only
/// source), its index named in records `m`: a planner's tests declare a planet so.
#[cfg(any(test, feature = "testkit"))]
pub fn accept_for_test(root: &NasRoot, m: &mut BTreeMap<String, String>, date: &str) {
    let staged = include_str!("../../../producers/sources/osm-planet.toml");
    let decl = staged.replace("version = \"2026-09-28\"", &format!("version = \"{date}\""));
    crate::inputs::sources::accept_files(root, m, &[("osm-planet.toml", decl.as_bytes())]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn planet(urls: &[String], md5: &str) -> Planet {
        Planet { date: "2026-09-28".into(), urls: urls.to_vec(), md5: md5.into() }
    }

    /// A local server of `files` (path → bytes): a range `bytes=a-` answered 206, else 200; any other
    /// path 404. Its base URL, and the requests it had (path, range start).
    fn server(files: BTreeMap<String, Vec<u8>>) -> (String, std::sync::Arc<std::sync::Mutex<Vec<(String, u64)>>>) {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let a = asked.clone();
        // (store::spawn::accept takes a listener that doesn't block; the connection then does.)
        l.set_nonblocking(true).unwrap();
        std::thread::spawn(move || loop {
            let Ok((mut s, _)) = store::spawn::accept(&l) else {
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            };
            s.set_nonblocking(false).ok();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while s.read(&mut b).is_ok_and(|n| n == 1) {
                req.push(b[0]);
                if req.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&req).to_string();
            let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
            let from = text
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string))
                .and_then(|r| r.trim().trim_end_matches('-').parse::<u64>().ok());
            a.lock().unwrap().push((path.clone(), from.unwrap_or(0)));
            let head = match (files.get(&path), from) {
                (None, _) => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                (Some(d), Some(f)) if f as usize >= d.len() => format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    d.len()
                ),
                (Some(d), Some(f)) => format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {f}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    d.len() - 1,
                    d.len(),
                    d.len() - f as usize
                ),
                (Some(d), None) => {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", d.len())
                }
            };
            s.write_all(head.as_bytes()).ok();
            if let Some(d) = files.get(&path) {
                let f = from.unwrap_or(0) as usize;
                if f < d.len() {
                    s.write_all(&d[f..]).ok();
                }
            }
        });
        (base, asked)
    }

    fn md5_hex(b: &[u8]) -> String {
        use md5::Digest;
        md5::Md5::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    /// The fetch by the build's fetcher against a local server: a URL the server hasn't gives way
    /// to the next; a file not its MD5 is removed and the next URL starts over; the right one takes
    /// its name; a part a stopped run left is gone on from; once there, nothing is asked.
    #[test]
    fn the_declared_planet_is_fetched_once_checked_against_its_md5() {
        let d = tempfile::tempdir().unwrap();
        let root = NasRoot::local_for_test(d.path());
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
        let mut bad = data.clone();
        bad[500_000] ^= 1;
        let md5 = format!("{}  planet-260928.osm.pbf\n", md5_hex(&data));
        let (base, asked) = server(BTreeMap::from([
            ("/bad/planet.pbf".to_string(), bad),
            ("/good/planet.pbf".to_string(), data.clone()),
            ("/planet.pbf.md5".to_string(), md5.into_bytes()),
        ]));
        let urls: Vec<String> =
            ["/gone/planet.pbf", "/bad/planet.pbf", "/good/planet.pbf"].iter().map(|p| format!("{base}{p}")).collect();
        let p = planet(&urls, &format!("{base}/planet.pbf.md5"));
        let said = std::sync::Mutex::new(Vec::<String>::new());
        let say = |s: &str| said.lock().unwrap().push(s.to_string());
        assert_eq!(next(&root, &p, None), Some(Next::Fetch));
        fetch(&root, &p, &say).unwrap();
        let at = path(&root, "2026-09-28");
        assert_eq!(std::fs::read(at.unchecked_path()).unwrap(), data);
        assert!(!at.with_file_name(PART).exists());
        let said = said.into_inner().unwrap();
        assert!(said[0].contains("/gone/planet.pbf: the server hasn't it"), "{said:?}");
        assert!(said[1].contains("/bad/planet.pbf: the planet's MD5 is"), "{said:?}");
        assert!(said[2].contains("fetched from") && said[2].contains("/good/"), "{said:?}");
        // (The good URL started over: the bad one's part was removed.)
        assert!(asked.lock().unwrap().contains(&("/good/planet.pbf".to_string(), 0)));
        assert_eq!(next(&root, &p, None), Some(Next::Pass(at.clone())));
        assert_eq!(next(&root, &p, Some("2026-09-28")), None);
        assert_eq!(next(&root, &p, Some("2026-10-05")), Some(Next::Behind));
        // There already: nothing asked.
        let n = asked.lock().unwrap().len();
        fetch(&root, &p, &|_| {}).unwrap();
        assert_eq!(asked.lock().unwrap().len(), n);
        // A part a stopped run left: the rest alone asked.
        std::fs::remove_file(at.unchecked_path()).unwrap();
        std::fs::write(at.with_file_name(PART).unchecked_path(), &data[..400_000]).unwrap();
        let p = planet(&urls[2..], &format!("{base}/planet.pbf.md5"));
        fetch(&root, &p, &|_| {}).unwrap();
        assert_eq!(std::fs::read(at.unchecked_path()).unwrap(), data);
        assert_eq!(asked.lock().unwrap().last(), Some(&("/good/planet.pbf".to_string(), 400_000)));
        // The NAS short of room for the rest, past its reserve (the test's disk: 700 kB free, a
        // reserve of 100 kB): stopped at the first answer, nothing written, no other URL tried.
        std::fs::remove_file(at.unchecked_path()).unwrap();
        std::fs::write(at.with_file_name(PART).unchecked_path(), &data[..400_000]).unwrap();
        buildstore::room::testkit::TEST_FREE.with(|c| c.set(Some(700_000)));
        buildstore::room::set_nas_reserve(&root, Some(100_001), "a test").unwrap();
        let n = asked.lock().unwrap().len();
        let e = fetch(&root, &planet(&urls[1..], &p.md5), &|_| {}).unwrap_err().to_string();
        assert!(e.contains("wasn't fetched") && e.contains("0.0 GB free past its reserve"), "{e}");
        assert_eq!(
            asked.lock().unwrap()[n..],
            [("/planet.pbf.md5".to_string(), 0), ("/bad/planet.pbf".to_string(), 400_000)]
        );
        assert_eq!(std::fs::metadata(at.with_file_name(PART).unchecked_path()).unwrap().len(), 400_000);
        // Room for it, just: fetched.
        buildstore::room::set_nas_reserve(&root, Some(100_000), "a test").unwrap();
        fetch(&root, &p, &|_| {}).unwrap();
        assert_eq!(std::fs::read(at.unchecked_path()).unwrap(), data);
        buildstore::room::set_nas_reserve(&root, None, "a test").unwrap();
        // No URL brings it: an error naming each, the part kept.
        std::fs::remove_file(at.unchecked_path()).unwrap();
        let p = planet(&[format!("{base}/gone/planet.pbf")], &format!("{base}/planet.pbf.md5"));
        let e = fetch(&root, &p, &|_| {}).unwrap_err().to_string();
        assert!(e.contains("wasn't fetched") && e.contains("/gone/planet.pbf: none there"), "{e}");
    }

    /// The fetch's room as the plan measures it: the newest complete pass's planet's size, less the
    /// part's; none with no pass, or its planet gone.
    #[test]
    fn the_fetchs_room_is_the_last_planets_size_less_the_part() {
        let d = tempfile::tempdir().unwrap();
        let root = NasRoot::local_for_test(d.path());
        assert_eq!(room(&root, "2026-10-05", None), None);
        assert_eq!(room(&root, "2026-10-05", Some("2026-09-28")), None, "its planet not there");
        for date in ["2026-09-28", "2026-10-05"] {
            std::fs::create_dir_all(d.path().join("sources/osm").join(date)).unwrap();
        }
        std::fs::write(path(&root, "2026-09-28").unchecked_path(), vec![0u8; 5000]).unwrap();
        let r = room(&root, "2026-10-05", Some("2026-09-28")).unwrap();
        assert_eq!((r.size, r.by.as_str(), r.held, r.left()), (5000, "2026-09-28", 0, 5000));
        std::fs::write(path(&root, "2026-10-05").with_file_name(PART).unchecked_path(), vec![0u8; 1200]).unwrap();
        assert_eq!(room(&root, "2026-10-05", Some("2026-09-28")).unwrap().left(), 3800);
    }

    /// The declaration staged (producers/sources/osm-planet.toml) pins the planet the build has
    /// read since 2 Oct (sources/osm/2026-09-28/), its URLs and MD5 the 28 September file's; a
    /// declaration's planet fields are checked.
    #[test]
    fn the_staged_planet_is_the_one_the_build_has() {
        let s = crate::inputs::sources::staged();
        let p = Planet::of(s.role(OSM_PLANET).unwrap()).unwrap();
        assert_eq!(p.date, "2026-09-28");
        assert!(p.urls.iter().all(|u| u.ends_with("/planet-260928.osm.pbf")) && p.urls.len() == 4, "{:?}", p.urls);
        assert!(p.urls.last().unwrap().starts_with("https://planet.openstreetmap.org/"), "the main site last");
        assert_eq!(p.md5, "https://planet.openstreetmap.org/pbf/planet-260928.osm.pbf.md5");
        let decl = |data: &str| {
            format!("name = \"P\"\ncredit = \"C\"\nlicence = \"ODbL-1.0\"\nterritory = \"world\"\n[data]\nrole = \"osm-planet\"\nformat = \"osm-pbf\"\nkind = \"data\"\nurl = \"https://a.example/p.pbf\"\n{data}")
        };
        let parse = |data: &str| crate::inputs::sources::parse(decl(data).as_bytes());
        assert!(parse("version = \"2026-09-28\"\n[data.params]\nmd5 = \"https://a.example/p.md5\"\n").is_ok());
        for (data, why) in [
            ("version = \"260928\"\n[data.params]\nmd5 = \"https://a.example/p.md5\"\n", "the planet's date"),
            ("[data.params]\nmd5 = \"https://a.example/p.md5\"\n", "no `version`"),
            ("version = \"2026-09-28\"\n", "no `params.md5`"),
            ("version = \"2026-09-28\"\n[data.params]\nmd5 = \"p.md5\"\n", "isn't an http"),
            ("version = \"2026-09-28\"\n[data.params]\nmd5 = \"https://a.example/p.md5\"\nsize = 1\n", "`params.size`"),
        ] {
            let e = parse(data).unwrap_err().join("; ");
            assert!(e.contains(why), "{data}: {e}");
        }
        assert_eq!(
            md5_of(b"3B704522E01272F5A6078EC93B275B64  planet.osm.pbf\n").as_deref(),
            Some("3b704522e01272f5a6078ec93b275b64")
        );
        assert_eq!(md5_of(b"<html>"), None);
    }
}
