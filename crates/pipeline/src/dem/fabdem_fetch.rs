//! FABDEM V1-2 kept whole in the store, once (`scenic-build fabdem-fetch`, docs/inputs.md §6.8):
//! every tile of a Hugging Face mirror's listing (`--mirror`, its dataset's URL; the docs name it)
//! not yet in `sources/fetched/fabdem/`, one at a time, the squares named first. Area builds never fetch FABDEM: a tile the index names and
//! the store lacks is an error naming it (crate::dem::generic).
//!
//! Each tile: downloaded into a work folder off the store (`<name>.part`, resumed with a range
//! request, retried with backoff), its size checked against the listing's, then decoded and written
//! to the store as every stored tile is (crate::dem::kept: rewritten, read back and compared under a
//! temporary name, then renamed). A tile the store has under its name is skipped, so a run stopped
//! anywhere is started again as it is.

use anyhow::{bail, ensure, Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A Hugging Face dataset's file list, with sizes, from its URL (`<host>/datasets/<owner>/<name>`).
pub fn listing_url(mirror: &str) -> Result<String> {
    let m = mirror.trim_end_matches('/');
    let (host, rest) = m.split_once("/datasets/").context("--mirror: a Hugging Face dataset's URL (<host>/datasets/<owner>/<name>)")?;
    Ok(format!("{host}/api/datasets/{rest}?blobs=true"))
}

/// A tile of the listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Its path in the mirror (`tiles/<square>_FABDEM_V1-2/<tile>_FABDEM_V1-2.tif`).
    pub path: String,
    /// Its 10° square (`N20W110-N30W100`).
    pub square: String,
    /// Its file name, which is its name in the store.
    pub name: String,
    pub size: u64,
}

/// The tiles of the listing's JSON (`siblings`: `rfilename`, `size`), the squares in `first` first,
/// then the rest, each part by path.
pub fn tiles(listing: &[u8], first: &[String]) -> Result<Vec<Tile>> {
    let v: serde_json::Value = serde_json::from_slice(listing).context("the listing isn't JSON")?;
    let sib = v["siblings"].as_array().context("the listing has no `siblings`")?;
    let mut out = Vec::new();
    for s in sib {
        let Some(p) = s["rfilename"].as_str() else { continue };
        let parts: Vec<&str> = p.split('/').collect();
        let [ "tiles", dir, name ] = parts[..] else { continue };
        let (Some(square), true) = (dir.strip_suffix("_FABDEM_V1-2"), name.ends_with("_FABDEM_V1-2.tif")) else { continue };
        let size = s["size"].as_u64().with_context(|| format!("{p}: no size in the listing"))?;
        out.push(Tile { path: p.to_string(), square: square.to_string(), name: name.to_string(), size });
    }
    ensure!(!out.is_empty(), "the listing names no tile");
    out.sort_by(|a, b| (!first.contains(&a.square), &a.path).cmp(&(!first.contains(&b.square), &b.path)));
    Ok(out)
}

/// An answer to a GET from byte `from`: its status and body.
pub struct Answer {
    pub status: u16,
    pub body: Box<dyn Read>,
}

/// What GETs a URL from a byte on.
pub trait Get {
    fn get(&self, url: &str, from: u64) -> Result<Answer>;
}

/// The network, as the project asks (crate::fetch::USER_AGENT); redirects followed (the mirror's
/// files are served from its CDN).
pub struct Net(ureq::Agent);

impl Net {
    pub fn new() -> Net {
        Net(ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .user_agent(crate::fetch::USER_AGENT)
            .accept_encoding("identity")
            .http_status_as_error(false)
            .build()
            .into())
    }
}

impl Default for Net {
    fn default() -> Net {
        Net::new()
    }
}

impl Get for Net {
    fn get(&self, url: &str, from: u64) -> Result<Answer> {
        crate::fetch::online(url)?;
        let mut req = self.0.get(url);
        if from > 0 {
            req = req.header("Range", format!("bytes={from}-"));
        }
        let r = req.call()?;
        let status = r.status().as_u16();
        let body = r.into_body().into_with_config().limit(u64::MAX).reader();
        Ok(Answer { status, body: Box::new(body) })
    }
}

/// Waits before attempt `n` (from 1): 2 s, twice as long each time, at most two minutes.
fn backoff(n: u32) -> Duration {
    Duration::from_secs((2u64 << (n - 1).min(10)).min(120))
}

/// `url` into `part` until it holds `size` bytes: a part already there goes on from its end (a
/// range request); a transfer that breaks off goes on from what it got. Attempts that move nothing
/// count against `tries`; a server answering a range whole starts the part again.
pub fn download(get: &dyn Get, url: &str, part: &Path, size: u64, tries: u32, wait: &dyn Fn(Duration)) -> Result<()> {
    let mut idle = 0u32;
    let mut last = String::new();
    loop {
        let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        if have == size {
            return Ok(());
        }
        if have > size {
            std::fs::remove_file(part)?;
            continue;
        }
        if idle > 0 {
            ensure!(idle <= tries, "{url}: no progress in {tries} tries ({last})");
            wait(backoff(idle));
        }
        // (The bytes it got, and whether it went on from `have`: a range answered whole starts the
        // part again, which isn't progress.)
        let r = (|| -> Result<(u64, bool)> {
            let mut a = get.get(url, have)?;
            let mut f = match (a.status, have) {
                (206, _) => std::fs::OpenOptions::new().append(true).open(part)?,
                (200, _) => std::fs::File::create(part)?,
                (404 | 410, _) => bail!("{url}: not on the mirror (status {})", a.status),
                (s, _) => return Err(anyhow::anyhow!("status {s}")),
            };
            let start = if a.status == 206 { have } else { 0 };
            let mut got = 0u64;
            let mut buf = vec![0u8; 1 << 20];
            let r = loop {
                match a.body.read(&mut buf) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        if start + got + n as u64 > size {
                            break Err(anyhow::anyhow!("more than the listing's {size} bytes"));
                        }
                        f.write_all(&buf[..n])?;
                        got += n as u64;
                    }
                    Err(e) => break Err(e.into()),
                }
            };
            f.flush()?;
            let on = a.status == 206 || have == 0;
            r.map(|_| (got, on)).or_else(|e| if got > 0 { Ok((got, on)) } else { Err(e) })
        })();
        match r {
            Ok((n, true)) if n > 0 => idle = 0,
            Ok((_, on)) => {
                idle += 1;
                last = if on { "an empty answer" } else { "a range answered whole" }.into();
            }
            Err(e) => {
                if format!("{e:#}").contains("not on the mirror") {
                    return Err(e);
                }
                if format!("{e:#}").contains("more than the listing") {
                    std::fs::remove_file(part).ok();
                }
                idle += 1;
                last = format!("{e:#}");
            }
        }
    }
}

/// The names in the store folder (`.tif`s, as stored: a temporary file never has its final name).
pub fn stored(store: &Path) -> Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    match std::fs::read_dir(store) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| store.display().to_string()),
        Ok(d) => {
            for e in d {
                let n = e?.file_name().to_string_lossy().into_owned();
                if n.ends_with(".tif") {
                    out.insert(n);
                }
            }
        }
    }
    Ok(out)
}

/// How a run went.
#[derive(Debug, Default)]
pub struct Done {
    pub stored: usize,
    pub skipped: usize,
    pub failed: Vec<(String, String)>,
    pub bytes: u64,
}

/// Every tile of `tiles` not in `store` (by name), downloaded through `work` and stored, in order;
/// a tile that fails is noted and the run goes on. `say` hears each line of progress.
pub fn run(get: &dyn Get, mirror: &str, tiles: &[Tile], store: &Path, work: &Path, tries: u32, wait: &dyn Fn(Duration), say: &mut dyn FnMut(&str)) -> Result<Done> {
    std::fs::create_dir_all(work)?;
    std::fs::create_dir_all(store)?;
    let have = stored(store)?;
    let mut done = Done::default();
    let todo: Vec<&Tile> = tiles.iter().filter(|t| !have.contains(&t.name)).collect();
    done.skipped = tiles.len() - todo.len();
    let total: u64 = todo.iter().map(|t| t.size).sum();
    say(&format!("fabdem-fetch: {} tiles listed, {} stored already, {} to fetch ({:.1} GiB)", tiles.len(), done.skipped, todo.len(), total as f64 / (1u64 << 30) as f64));
    let t0 = Instant::now();
    for (i, t) in todo.iter().enumerate() {
        let part = work.join(format!("{}.part", t.name));
        let url = format!("{}/resolve/main/{}", mirror.trim_end_matches('/'), t.path);
        let r = (|| -> Result<()> {
            let mut again = false;
            loop {
                download(get, &url, &part, t.size, tries, wait)?;
                match super::kept::store(store, &t.name, &part) {
                    Ok(_) => break,
                    // (Bytes of the right length that don't decode: taken once more.)
                    Err(e) if !again => {
                        say(&format!("  {}: {e:#}; downloading it again", t.name));
                        std::fs::remove_file(&part).ok();
                        again = true;
                    }
                    Err(e) => return Err(e),
                }
            }
            std::fs::remove_file(&part).ok();
            Ok(())
        })();
        match r {
            Ok(()) => {
                done.stored += 1;
                done.bytes += t.size;
            }
            Err(e) => {
                say(&format!("  {}: FAILED: {e:#}", t.name));
                done.failed.push((t.name.clone(), format!("{e:#}")));
            }
        }
        let s = t0.elapsed().as_secs_f64().max(1e-3);
        let rate = done.bytes as f64 / s;
        let left = total.saturating_sub(todo[..=i].iter().map(|t| t.size).sum());
        say(&format!(
            "[{}/{}] {} {} ({:.1} MB) — stored {} this run, {} in all, {:.1} MB/s, {:.1} GiB left, ~{:.1} h",
            i + 1,
            todo.len(),
            t.square,
            t.name,
            t.size as f64 / 1e6,
            done.stored,
            done.skipped + done.stored,
            rate / 1e6,
            left as f64 / (1u64 << 30) as f64,
            if rate > 0.0 { left as f64 / rate / 3600.0 } else { f64::NAN }
        ));
    }
    Ok(done)
}

/// The listing read from the mirror (retried with backoff).
pub fn listing(net: &Net, mirror: &str) -> Result<Vec<u8>> {
    let url = listing_url(mirror)?;
    let mut last = String::new();
    for n in 0..8u32 {
        if n > 0 {
            std::thread::sleep(backoff(n));
        }
        match net.get(&url, 0) {
            Ok(mut a) if a.status == 200 => {
                let mut b = Vec::new();
                match a.body.read_to_end(&mut b) {
                    Ok(_) => return Ok(b),
                    Err(e) => last = e.to_string(),
                }
            }
            Ok(a) => last = format!("status {}", a.status),
            Err(e) => last = format!("{e:#}"),
        }
    }
    bail!("{url}: {last}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geotiff::write_f32;
    use std::sync::Mutex;

    const MIRROR: &str = "https://mirror.example/datasets/o/fabdem";

    /// A mirror in memory: `breaks` answers cut off after that many bytes, `whole` answers ranges
    /// with the whole file (200).
    struct Mem {
        files: std::collections::HashMap<String, Vec<u8>>,
        breaks: Mutex<Vec<usize>>,
        whole: bool,
        asked: Mutex<Vec<(String, u64)>>,
    }

    impl Get for Mem {
        fn get(&self, url: &str, from: u64) -> Result<Answer> {
            self.asked.lock().unwrap().push((url.to_string(), from));
            let Some(b) = self.files.get(url) else { return Ok(Answer { status: 404, body: Box::new(std::io::empty()) }) };
            let (status, mut body) = if from > 0 && !self.whole { (206, b[from as usize..].to_vec()) } else { (200, b.clone()) };
            if let Some(n) = self.breaks.lock().unwrap().pop() {
                body.truncate(n);
                struct Broken(std::io::Cursor<Vec<u8>>);
                impl Read for Broken {
                    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                        match self.0.read(buf)? {
                            0 => Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset")),
                            n => Ok(n),
                        }
                    }
                }
                return Ok(Answer { status, body: Box::new(Broken(std::io::Cursor::new(body))) });
            }
            Ok(Answer { status, body: Box::new(std::io::Cursor::new(body)) })
        }
    }

    fn tif(k: u32) -> Vec<u8> {
        let data: Vec<f32> = (0..300 * 200).map(|i| (i % 911) as f32 + k as f32).collect();
        write_f32(300, 200, &data, 512, [k as f64, 1.0 / 3600.0, 0.0, 10.0, 0.0, -1.0 / 3600.0], 4326, Some(-9999.0)).unwrap()
    }

    fn listing_of(files: &[(&str, usize)]) -> Vec<u8> {
        let sib: Vec<_> = files.iter().map(|(p, n)| serde_json::json!({"rfilename": p, "size": n})).collect();
        serde_json::to_vec(&serde_json::json!({"siblings": sib})).unwrap()
    }

    #[test]
    fn the_listing_gives_tiles_the_blocked_squares_first() {
        let l = listing_of(&[("license.txt", 9), ("FABDEM_v1-2_tiles.geojson", 5), ("tiles/N40E000-N50E010_FABDEM_V1-2/N48E002_FABDEM_V1-2.tif", 7), ("tiles/N20W110-N30W100_FABDEM_V1-2/N25W105_FABDEM_V1-2.tif", 8), ("tiles/N00E000-N10E010_FABDEM_V1-2/N00E006_FABDEM_V1-2.tif", 6)]);
        let t = tiles(&l, &["N20W110-N30W100".into()]).unwrap();
        let names: Vec<&str> = t.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["N25W105_FABDEM_V1-2.tif", "N00E006_FABDEM_V1-2.tif", "N48E002_FABDEM_V1-2.tif"]);
        assert_eq!((t[0].square.as_str(), t[0].size), ("N20W110-N30W100", 8));
    }

    /// Two tiles: one stored already (skipped, never asked for), one downloaded through breaks
    /// (resumed by range from where it stopped) and stored as kept.rs stores (decoding to the same
    /// pixels); nothing left but the stored files; a second run fetches nothing.
    #[test]
    fn tiles_are_resumed_checked_and_stored_and_a_run_again_skips_them() {
        let d = tempfile::tempdir().unwrap();
        let (store, work) = (d.path().join("fabdem"), d.path().join("work"));
        std::fs::create_dir_all(&store).unwrap();
        let (a, b) = (tif(1), tif(2));
        std::fs::write(store.join("N01E001_FABDEM_V1-2.tif"), &a).unwrap();
        // (Another dataset's file and a `.none` stay as they are.)
        std::fs::write(store.join("N34E140.none"), b"").unwrap();
        let pa = "tiles/N00E000-N10E010_FABDEM_V1-2/N01E001_FABDEM_V1-2.tif";
        let pb = "tiles/N00E000-N10E010_FABDEM_V1-2/N02E002_FABDEM_V1-2.tif";
        let t = tiles(&listing_of(&[(pa, a.len()), (pb, b.len())]), &[]).unwrap();
        let ub = format!("{MIRROR}/resolve/main/{pb}");
        let mem = Mem { files: [(ub.clone(), b.clone())].into(), breaks: Mutex::new(vec![b.len() / 2, 1000]), whole: false, asked: Mutex::default() };
        let mut lines = Vec::new();
        let done = run(&mem, MIRROR, &t, &store, &work, 5, &|_| {}, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!((done.stored, done.skipped, done.failed.len()), (1, 1, 0));
        let asked = mem.asked.lock().unwrap().clone();
        assert_eq!(asked, vec![(ub.clone(), 0), (ub.clone(), 1000), (ub.clone(), 1000 + (b.len() / 2) as u64)]);
        let got = crate::geotiff::Tiff::open(Arc::new(std::fs::read(store.join("N02E002_FABDEM_V1-2.tif")).unwrap())).unwrap();
        let want = crate::geotiff::Tiff::open(Arc::new(b.clone())).unwrap();
        assert_eq!(got.read_window(0, 0, 0, 300, 200).unwrap(), want.read_window(0, 0, 0, 300, 200).unwrap());
        assert_eq!((got.transform(0).unwrap(), got.nodata()), (want.transform(0).unwrap(), want.nodata()));
        let mut names: Vec<String> = std::fs::read_dir(&store).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["N01E001_FABDEM_V1-2.tif", "N02E002_FABDEM_V1-2.tif", "N34E140.none"]);
        assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0, "no part left");
        assert!(lines.iter().any(|l| l.starts_with("[1/1]")));
        mem.asked.lock().unwrap().clear();
        let again = run(&mem, MIRROR, &t, &store, &work, 5, &|_| {}, &mut |_| {}).unwrap();
        assert_eq!((again.stored, again.skipped), (0, 2));
        assert!(mem.asked.lock().unwrap().is_empty());
    }

    /// Bytes of the listing's length that don't decode never take the tile's name: downloaded
    /// once more, then the tile fails and the run goes on; a size that isn't the listing's
    /// neither; a server answering a range whole starts the part again.
    #[test]
    fn a_tile_that_wont_decode_or_differs_in_size_is_never_stored() {
        let d = tempfile::tempdir().unwrap();
        let (store, work) = (d.path().join("fabdem"), d.path().join("work"));
        let good = tif(3);
        let bad = vec![7u8; 5000];
        let pb2 = "tiles/S_FABDEM_V1-2/N03E003_FABDEM_V1-2.tif";
        let pg2 = "tiles/S_FABDEM_V1-2/N04E004_FABDEM_V1-2.tif";
        let pl2 = "tiles/S_FABDEM_V1-2/N05E005_FABDEM_V1-2.tif";
        let u = |p: &str| format!("{MIRROR}/resolve/main/{p}");
        // (N05's listing says it's longer than the mirror's file: it never completes.)
        let t = tiles(&listing_of(&[(pb2, bad.len()), (pg2, good.len()), (pl2, good.len() + 10)]), &[]).unwrap();
        let mem = Mem { files: [(u(pb2), bad.clone()), (u(pg2), good.clone()), (u(pl2), good.clone())].into(), breaks: Mutex::new(vec![100]), whole: true, asked: Mutex::default() };
        let done = run(&mem, MIRROR, &t, &store, &work, 2, &|_| {}, &mut |_| {}).unwrap();
        let failed: Vec<&str> = done.failed.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(failed, ["N03E003_FABDEM_V1-2.tif", "N05E005_FABDEM_V1-2.tif"]);
        assert_eq!(done.stored, 1);
        let names: Vec<String> = std::fs::read_dir(&store).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(listing_url(&format!("{MIRROR}/")).unwrap(), "https://mirror.example/api/datasets/o/fabdem?blobs=true");
        assert_eq!(names, ["N04E004_FABDEM_V1-2.tif"], "nothing else in the store, no temporary file");
        assert_eq!(mem.asked.lock().unwrap().iter().filter(|(x, _)| *x == u(pb2)).count(), 3, "a range answered whole started again, then downloaded once more");
    }
}
