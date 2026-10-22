//! Wikipedia pageviews (a measure of how much people read about a place, for ranking what the map
//! shows most prominently): the heritage chain's part (`heritage`: the heritage sites' items and the
//! World Heritage Sites' components', in the heritage step's stand-in root) and the items job's
//! (`items`: the landmark candidates' items). An item's Wikipedia articles in the languages of the
//! map's regions (`LANGS`) are counted and summed; its articles come from the chain's or the job's
//! article cache (wp.jsonl, crate::heritagewd), filled here for items not in it.
//!
//! Views come from Wikimedia's monthly pageview dumps (the declared `pageviews` source:
//! pageview_complete, user agents only). Wikimedia's pageviews API allows anonymous clients ten
//! requests a minute, too few for ~100,000 articles. One month per season is sampled
//! (`months_before`), so summer-heavy places aren't favoured. A month is streamed (~5 GB through
//! bzip2) once: every article of `LANGS` is counted, not just the ones wanted now, into the month's
//! index, kept on the NAS (`sources/pageviews/<month>.tsv.zst`: a `#langs<TAB>an,ast,…` line, then
//! `lang|Title<TAB>views` lines; zstd) and in this Mac's cache (`<cache>/items/months/`, through
//! store::cachefile: held while read, so the agent's room-making leaves it); any article of those
//! languages, of either job, any run, is then looked up there (a language added later streams the
//! month again). A dump's host's robots.txt is read first (crate::wikiask); the download goes
//! through the job's proxy (curl honours `HTTPS_PROXY`: the pool's budget), two months at once.
//!
//! Outputs: `heritage` writes data/pageviews/items.json, `items` the job's views.json and meta.json
//! (per item: its average monthly views over the sampled months, one decimal; Python's
//! `json.dumps` bytes, crate::pyjson; the heritage file's items by QID).

use crate::agent::jobs::report;
use crate::heritagewd::{read_lines, wikipedias, write_atomic, Ordered};
use crate::pyjson::{dumps_spaced, loads, Py};
use crate::timings::{phase, Class};
use crate::wikiask::Asker;
use anyhow::{bail, Context, Result};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::cachefile;

/// The map's languages, sorted (as an index's `#langs` line lists them).
pub const LANGS: [&str; 19] = [
    "an", "ast", "br", "ca", "co", "cy", "en", "es", "eu", "fr", "ga", "gd", "gl", "gv", "ja", "oc", "pt", "zh",
    "zh-yue",
];

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

/// The last November, February, May and August that ended at least 20 days before the epoch
/// (`YYYY-MM-DD`; Wikimedia publishes a month's dump in its first days), oldest first: a pass's
/// months, the same for the items job and the heritage chain.
pub fn months_before(epoch: &str) -> Result<Vec<String>> {
    let p: Vec<i64> = epoch.split('-').map(|s| s.parse()).collect::<Result<_, _>>().context("an epoch's date")?;
    let &[y0, m0, d0] = p.as_slice() else { bail!("{epoch}: not YYYY-MM-DD") };
    anyhow::ensure!((1..=12).contains(&m0) && (1..=31).contains(&d0), "{epoch}: not a date");
    let d = days(y0, m0, d0);
    let (mut out, mut y, mut m) = (Vec::new(), y0, m0);
    while out.len() < 4 {
        // (The day after the month before (y, m) ended.)
        let end = days(y, m, 1);
        m -= 1;
        if m == 0 {
            (y, m) = (y - 1, 12);
        }
        if matches!(m, 2 | 5 | 8 | 11) && d - end >= 20 {
            out.push(format!("{y:04}-{m:02}"));
        }
    }
    out.sort();
    Ok(out)
}

/// An article as the dumps name it: `lang|Title_with_underscores`.
fn norm(a: &str) -> String {
    let (l, t) = a.split_once('|').unwrap_or((a, ""));
    format!("{l}|{}", t.replace(' ', "_"))
}

/// An item's articles (its `arts`, `lang|Title`) in the map's languages.
fn arts_in_langs(wp: Option<&Py>) -> Vec<String> {
    let arts = wp.and_then(|r| r.get("arts")).and_then(Py::list);
    arts.into_iter()
        .flatten()
        .filter_map(Py::as_str)
        .filter(|a| a.split_once('|').is_some_and(|(l, _)| LANGS.contains(&l)))
        .map(str::to_string)
        .collect()
}

/// Average monthly views, as Python's `round(total / 4, 1)` (half to even; four months, so exact).
fn mean_views(total: u64, months: usize) -> Py {
    assert_eq!(months, 4, "four months a pass");
    let five = total as u128 * 5;
    let mut tenths = five / 2;
    if five % 2 == 1 && tenths % 2 == 1 {
        tenths += 1;
    }
    Py::Float(tenths as f64 / 10.0)
}

/// Per item with articles in the map's languages: its articles' views summed over the months,
/// averaged (`mean_views`), in `qids`' order.
fn item_views(qids: &[String], arts_of: &HashMap<String, Vec<String>>, per_month: &[HashMap<String, u64>]) -> Py {
    let mut out = Vec::new();
    for q in qids {
        let Some(arts) = arts_of.get(q).filter(|a| !a.is_empty()) else { continue };
        let total: u64 = arts
            .iter()
            .map(|a| norm(a))
            .map(|a| per_month.iter().map(|pm| pm.get(&a).copied().unwrap_or(0)).sum::<u64>())
            .sum();
        out.push((q.clone(), mean_views(total, per_month.len())));
    }
    Py::Dict(out)
}

/// The months' indexes: this Mac's (`dir`, the cache's `items/months`) and the NAS's (`store`), and
/// where a month's dump is (the declared source's URL, `{y}` and `{m}` in it).
pub struct Months {
    dir: PathBuf,
    store: Option<PathBuf>,
    url: String,
    ask: Asker,
    /// The bytes moved so far, by month and what (`stream`, `copied`, `read`: the dumps streamed,
    /// the indexes copied to or from the NAS and read): the progress line's.
    moved: Mutex<HashMap<(String, &'static str), Arc<AtomicU64>>>,
    /// The bytes the work's expected to move, all told (`plan`): worked out before the progress
    /// line's first, so its total doesn't grow as each month starts.
    planned: AtomicU64,
}

/// A damaged index (cut short, not zstd, not UTF-8, a count that isn't one), to copy or stream again.
struct Damaged(String);

/// A file read through, its bytes counted.
struct Counted<R> {
    r: R,
    n: Arc<AtomicU64>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let k = self.r.read(b)?;
        self.n.fetch_add(k as u64, Ordering::Relaxed);
        Ok(k)
    }
}

/// A file's size; None when it isn't there. (Any other error is one: a NAS that doesn't answer is
/// no reason to stream a month for an hour.)
fn size(p: &Path) -> Result<Option<u64>> {
    match std::fs::metadata(p) {
        Ok(m) => Ok(Some(m.len())),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("stat {}", p.display())),
    }
}

/// The languages an index counted (its first line, `#langs<TAB>…`; none without one). Its open's
/// error is an error; one reading it, damage.
fn langs_of_index(p: &Path) -> Result<Result<HashSet<String>, Damaged>> {
    let f = File::open(p).with_context(|| format!("open {}", p.display()))?;
    let first = (|| -> io::Result<Vec<u8>> {
        let mut r = BufReader::new(zstd::stream::read::Decoder::new(f)?);
        let mut line = Vec::new();
        r.read_until(b'\n', &mut line)?;
        Ok(line)
    })();
    Ok(match first.map(String::from_utf8) {
        Err(e) => Err(Damaged(e.to_string())),
        Ok(Err(e)) => Err(Damaged(e.to_string())),
        Ok(Ok(l)) => Ok(l
            .trim_end_matches('\n')
            .strip_prefix("#langs\t")
            .map(|r| r.split(',').map(str::to_string).collect())
            .unwrap_or_default()),
    })
}

/// The languages of articles `lang|Title`.
fn langs_of(wanted: &HashSet<String>) -> HashSet<String> {
    wanted.iter().map(|a| a.split('|').next().unwrap_or("").to_string()).collect()
}

fn host() -> String {
    store::sys::hostname().unwrap_or_default().split('.').next().unwrap_or("").to_string()
}

impl Months {
    /// The indexes in `dir` here (the cache's `items/months`) and `store` on the NAS (none: here
    /// only); a month's dump at `url` (the declared `pageviews` source's).
    pub fn new(dir: PathBuf, store: Option<PathBuf>, url: String) -> Months {
        Months { dir, store, url, ask: Asker::new(), moved: Mutex::new(HashMap::new()), planned: AtomicU64::new(0) }
    }

    fn counter(&self, month: &str, what: &'static str) -> Arc<AtomicU64> {
        self.moved.lock().unwrap().entry((month.to_string(), what)).or_default().clone()
    }

    fn local(&self, month: &str) -> PathBuf {
        self.dir.join(format!("{month}.tsv.zst"))
    }

    fn nas(&self, month: &str) -> Option<PathBuf> {
        self.store.as_ref().map(|s| s.join(format!("{month}.tsv.zst")))
    }

    /// The dump's URL for a month.
    fn dump_url(&self, month: &str) -> String {
        let (y, m) = month.split_once('-').unwrap_or((month, ""));
        self.url.replace("{y}", y).replace("{m}", m)
    }

    /// Each month's views of the wanted articles (`month_views`), two months at once (the most the
    /// dumps' server asks for), with a progress line every half minute: the bytes moved of all
    /// that's planned (dumps streamed, indexes copied and read), so the build's status shows how far
    /// it is, not the step before's last line for half an hour.
    pub fn views(&self, months: &[String], wanted: &HashSet<String>) -> Result<Vec<HashMap<String, u64>>> {
        self.moved.lock().unwrap().clear();
        {
            let _p = phase("the work planned", Class::Mixed);
            let mut total = 0;
            for m in months {
                total += self.plan(m, wanted)?;
            }
            self.planned.store(total, Ordering::Relaxed);
        }
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let _p = phase("the months' views counted", Class::Mixed);
        let next = AtomicUsize::new(0);
        let got: Vec<Mutex<Option<Result<HashMap<String, u64>>>>> = months.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|s| {
            s.spawn(move || {
                while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    stopped.recv_timeout(Duration::from_secs(30))
                {
                    self.report(false);
                }
            });
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    s.spawn(|| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(m) = months.get(i) else { break };
                        *got[i].lock().unwrap() = Some(self.month_views(m, wanted));
                    })
                })
                .collect();
            for w in workers {
                w.join().ok();
            }
            drop(stop);
        });
        self.report(true);
        got.into_iter()
            .zip(months)
            .map(|(g, m)| g.into_inner().unwrap().with_context(|| format!("{m}: not counted"))?)
            .collect()
    }

    /// The progress line: of the bytes planned, until the end short of full (the work may move
    /// more than planned: a dump larger than said, an index copied again).
    fn report(&self, last: bool) {
        let total = self.planned.load(Ordering::Relaxed);
        if total == 0 {
            return;
        }
        let (done, n) = {
            let m = self.moved.lock().unwrap();
            let done: u64 = m.values().map(|c| c.load(Ordering::Relaxed)).sum();
            (done, m.keys().map(|k| &k.0).collect::<HashSet<_>>().len())
        };
        let done = if last { total } else { done.min(total * 99 / 100) };
        report(
            done >> 20,
            total >> 20,
            &format!("MB of the pageview dumps streamed ({n} month{})", if n == 1 { "" } else { "s" }),
        );
    }

    /// The bytes a month's work is expected to move, as far as can be told before it starts: its
    /// index read here (copied from the NAS first when only the NAS has one), else its dump
    /// streamed (5 GB when its server doesn't say).
    fn plan(&self, month: &str, wanted: &HashSet<String>) -> Result<u64> {
        let langs = langs_of(wanted);
        let covers = |p: &Path| matches!(langs_of_index(p), Ok(Ok(c)) if langs.is_subset(&c));
        let local = self.local(month);
        if let Some(n) = size(&local)? {
            if covers(&local) {
                return Ok(n);
            }
        }
        if let Some(nas) = self.nas(month) {
            if let Some(n) = size(&nas)? {
                if covers(&nas) {
                    return Ok(2 * n);
                }
            }
        }
        Ok(match self.ask.content_length(&self.dump_url(month)) {
            0 => 5 << 30,
            n => n,
        })
    }

    /// Views in one month of each wanted article (`lang|Title_with_underscores`): looked up in the
    /// month's index; the month streamed once into its index when there's none.
    fn month_views(&self, month: &str, wanted: &HashSet<String>) -> Result<HashMap<String, u64>> {
        let langs = langs_of(wanted);
        let mut index = match self.index(month, &langs, false)? {
            Some(i) => i,
            None => self.stream_index(month)?,
        };
        // (A damaged index: the NAS's copied again, then the month streamed again.)
        for again in ["nas", "stream", ""] {
            match self.look_up(month, &index, wanted)? {
                Ok(v) => return Ok(v),
                Err(Damaged(e)) if again.is_empty() => bail!("{month}: its index is damaged ({e})"),
                Err(Damaged(e)) => {
                    let how = if again == "nas" { "copying the NAS’s again" } else { "streaming the month again" };
                    eprintln!("  {month}: its index is damaged ({e}); {how}");
                    let fresh = if again == "nas" { self.index(month, &langs, true)? } else { None };
                    index = match fresh {
                        Some(i) => i,
                        None => self.stream_index(month)?,
                    };
                }
            }
        }
        unreachable!()
    }

    /// The month's index here, held, copied whole from the NAS the first time (or again: fresh), and
    /// put on the NAS if it has none (an upload that failed before); None when neither has one
    /// counting every language in `langs`.
    fn index(&self, month: &str, langs: &HashSet<String>, fresh: bool) -> Result<Option<PathBuf>> {
        let local = self.local(month);
        let nas = self.nas(month);
        self.sweep_here();
        if let Some(s) = &self.store {
            sweep_nas(s);
        }
        if fresh {
            cachefile::discard(&local);
        }
        let nas_size = match &nas {
            Some(n) => size(n)?,
            None => None,
        };
        if let (Some(n), Some(_)) = (&nas, nas_size) {
            cachefile::hold(&local, &mut |t| self.copy_into(month, n, t))?;
        } else if cachefile::hold_existing(&local)?.is_none() {
            return Ok(None);
        }
        let covered = match langs_of_index(&local)? {
            Ok(c) => c,
            Err(Damaged(e)) => {
                // (The NAS's copied again; that one damaged too, or none: the month streamed again.)
                eprintln!("  {month}: its index is damaged ({e})");
                if !fresh && nas_size.is_some() {
                    return self.index(month, langs, true);
                }
                cachefile::discard(&local);
                return Ok(None);
            }
        };
        if !langs.is_subset(&covered) {
            // (The NAS's may count them: another Mac's, made since a language was added.)
            if !fresh && nas_size.is_some() && nas_size != size(&local)? {
                return self.index(month, langs, true);
            }
            return Ok(None);
        }
        // (Only when the NAS has none: one that differs may be the good one, this copy damaged.)
        if let (Some(n), None) = (&nas, nas_size) {
            if let Err(e) = self.copy_whole(month, &local, n) {
                eprintln!("  {month}: its index isn't on the NAS yet ({e:#}); tried again next time");
            }
        }
        Ok(Some(local))
    }

    /// Temporary files here of copies and streams whose process is gone (killed midway): those no
    /// process holds (store::cachefile).
    fn sweep_here(&self) {
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".tmp") && n.contains(".tsv.zst.") {
                cachefile::try_remove(&e.path());
            }
        }
    }

    /// Copies `src` (the NAS's) into `dst` in place (a scratch file of store::cachefile), its length
    /// checked.
    fn copy_into(&self, month: &str, src: &Path, dst: &Path) -> io::Result<()> {
        let p = phase("the months' indexes copied from the NAS", Class::NasRead);
        let mut a = Counted { r: File::open(src)?, n: self.counter(month, "copied") };
        let mut b = File::options().write(true).truncate(true).open(dst)?;
        io::copy(&mut a, &mut b)?;
        b.sync_all()?;
        let (got, want) = (std::fs::metadata(dst)?.len(), std::fs::metadata(src)?.len());
        if got != want {
            return Err(io::Error::other(format!("{}: {got} of {want} bytes copied", dst.display())));
        }
        p.count(got, 1);
        Ok(())
    }

    /// Copies `src` to the NAS's `dst` by a temporary name (this Mac's and process's), flushed, its
    /// length checked.
    fn copy_whole(&self, month: &str, src: &Path, dst: &Path) -> Result<()> {
        let p = phase("the months' indexes copied to the NAS", Class::NasWrite);
        std::fs::create_dir_all(dst.parent().unwrap_or(Path::new(".")))?;
        let name = dst.file_name().unwrap_or_default().to_string_lossy();
        let tmp = dst.with_file_name(format!("{name}.{}.{}.tmp", host(), std::process::id()));
        let r = (|| -> Result<()> {
            let mut a = Counted { r: File::open(src)?, n: self.counter(month, "copied") };
            let mut b = File::create(&tmp)?;
            io::copy(&mut a, &mut b)?;
            b.sync_all()?;
            let (got, want) = (std::fs::metadata(&tmp)?.len(), std::fs::metadata(src)?.len());
            anyhow::ensure!(got == want, "{}: {got} of {want} bytes copied", dst.display());
            std::fs::rename(&tmp, dst)?;
            p.count(want, 1);
            Ok(())
        })();
        std::fs::remove_file(&tmp).ok();
        r
    }

    /// The month's views of the wanted articles in its index (a page's lines summed: its access
    /// kinds); the index's open failing an error, its reading damage.
    fn look_up(
        &self,
        month: &str,
        index: &Path,
        wanted: &HashSet<String>,
    ) -> Result<Result<HashMap<String, u64>, Damaged>> {
        let _p = phase("the months' indexes read", Class::Disk);
        let t0 = Instant::now();
        // (Marked used: room-making lets the least recently used copies go first.)
        cachefile::touch(index);
        let raw = File::open(index).with_context(|| format!("open {}", index.display()))?;
        let mut views: HashMap<String, u64> = HashMap::new();
        let read = (|| -> Result<(), Damaged> {
            let dmg = |e: io::Error| Damaged(e.to_string());
            let raw = Counted { r: raw, n: self.counter(month, "read") };
            let mut r = BufReader::with_capacity(1 << 20, zstd::stream::read::Decoder::new(raw).map_err(dmg)?);
            let mut line = Vec::new();
            loop {
                line.clear();
                if r.read_until(b'\n', &mut line).map_err(dmg)? == 0 {
                    return Ok(());
                }
                let l = std::str::from_utf8(&line).map_err(|e| Damaged(e.to_string()))?;
                let l = l.strip_suffix('\n').unwrap_or(l);
                let (key, n) = l.split_once('\t').unwrap_or((l, ""));
                if let Some(k) = wanted.get(key) {
                    let n: u64 = n.parse().map_err(|_| Damaged(format!("not a count: {n:?}")))?;
                    *views.entry(k.clone()).or_insert(0) += n;
                }
            }
        })();
        if let Err(d) = read {
            return Ok(Err(d));
        }
        eprintln!(
            "  {month}: {} of {} articles with views, from its index ({:.0} s)",
            views.len(),
            wanted.len(),
            t0.elapsed().as_secs_f64()
        );
        Ok(Ok(views))
    }

    /// Streams a month's dump into its index, here then on the NAS: every article of the map's
    /// languages, its views summed over the dump's adjacent lines for it (its access kinds).
    fn stream_index(&self, month: &str) -> Result<PathBuf> {
        let local = self.local(month);
        let t0 = Instant::now();
        let rows = {
            let _p = phase("the months' dumps streamed into indexes", Class::Net);
            cachefile::make_dir(&self.dir)?;
            eprintln!("  {month}: streaming its dump, every article of the map's languages counted");
            let url = self.dump_url(month);
            anyhow::ensure!(self.ask.may(&url)?, "{url}: its robots.txt disallows us");
            // (A held scratch file of store::cachefile: room-making leaves it while it's written.)
            let (tmp, held) = cachefile::scratch(&local)?;
            let r = self.stream_into(month, &url, &tmp);
            let r = r.and_then(|rows| {
                File::open(&tmp)?.sync_all()?;
                // (One here that didn't count every language goes: never renamed over.)
                cachefile::discard(&local);
                cachefile::publish(held, &tmp, &local)?;
                anyhow::ensure!(
                    cachefile::hold_existing(&local)?.is_some(),
                    "{}: gone as it was made",
                    local.display()
                );
                Ok(rows)
            });
            std::fs::remove_file(&tmp).ok();
            r?
        };
        if let Some(n) = self.nas(month) {
            // (One that fails is tried again the next time the month is looked up: `index`.)
            if let Err(e) = self.copy_whole(month, &local, &n) {
                eprintln!("  {month}: its index isn't on the NAS yet ({e:#})");
            }
        }
        eprintln!(
            "  {month}: {rows} articles in its index, {} MB ({:.0} s)",
            std::fs::metadata(&local).map_or(0, |m| m.len()) >> 20,
            t0.elapsed().as_secs_f64()
        );
        Ok(local)
    }

    /// curl | bzip2 -dc into `tmp` (zstd), the map's languages' lines kept; each program's exit
    /// checked, so a download cut short (curl's error, bzip2's truncated stream) fails the month
    /// instead of keeping what arrived. curl's bytes reach bzip2 through here, counted (the progress
    /// line's). A download that stalls (under 10 kB/s for five minutes) is given up. The rows written.
    fn stream_into(&self, month: &str, url: &str, tmp: &Path) -> Result<u64> {
        use std::process::Stdio;
        let mut curl = store::spawn::spawn(
            store::spawn::command("curl")
                .args([
                    "-sSL",
                    "--fail",
                    "--connect-timeout",
                    "30",
                    "--speed-limit",
                    "10000",
                    "--speed-time",
                    "300",
                    "-A",
                ])
                .arg(crate::fetch::USER_AGENT)
                .arg(url)
                .stdout(Stdio::piped()),
        )
        .context("run curl")?;
        let mut bz = match store::spawn::spawn(
            store::spawn::command("bzip2").arg("-dc").stdin(Stdio::piped()).stdout(Stdio::piped()),
        ) {
            Ok(b) => b,
            Err(e) => {
                curl.kill().ok();
                curl.wait().ok();
                return Err(e).context("run bzip2");
            }
        };
        let (mut from, mut to) = (curl.stdout.take().unwrap(), bz.stdin.take().unwrap());
        let streamed = self.counter(month, "stream");
        let curl = Arc::new(Mutex::new(curl));
        let pumper = {
            let curl = curl.clone();
            std::thread::spawn(move || {
                let mut b = vec![0u8; 1 << 20];
                loop {
                    match from.read(&mut b) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => {
                            if to.write_all(&b[..k]).is_err() {
                                // (bzip2 stopped: its exit says why. curl, with no one left to read
                                // it, would wait on its full pipe for good.)
                                curl.lock().unwrap().kill().ok();
                                break;
                            }
                            streamed.fetch_add(k as u64, Ordering::Relaxed);
                        }
                    }
                }
            })
        };
        let out = bz.stdout.take().unwrap();
        let written = (|| -> Result<u64> {
            let f = File::options().write(true).truncate(true).open(tmp)?;
            let mut z = zstd::stream::write::Encoder::new(io::BufWriter::new(f), 3)?;
            writeln!(z, "#langs\t{}", LANGS.join(","))?;
            let prefixes: Vec<String> = LANGS.iter().map(|l| format!("{l}.wikipedia ")).collect();
            let mut r = BufReader::with_capacity(1 << 20, out);
            let (mut line, mut last, mut total, mut rows) = (Vec::new(), None::<String>, 0u64, 0u64);
            loop {
                line.clear();
                if r.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                if !prefixes.iter().any(|p| line.starts_with(p.as_bytes())) {
                    continue;
                }
                // wiki title page_id access monthly_total hourly
                let l = String::from_utf8_lossy(&line);
                let f: Vec<&str> = l.splitn(6, ' ').collect();
                if f.len() < 5 || f[4].is_empty() || !f[4].bytes().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let key = format!("{}|{}", &f[0][..f[0].len() - ".wikipedia".len()], f[1]);
                if last.as_deref() != Some(key.as_str()) {
                    if let Some(k) = last.take() {
                        writeln!(z, "{k}\t{total}")?;
                        rows += 1;
                    }
                    (last, total) = (Some(key), 0);
                }
                total += f[4].parse::<u64>()?;
            }
            if let Some(k) = last {
                writeln!(z, "{k}\t{total}")?;
                rows += 1;
            }
            z.finish()?.flush()?;
            Ok(rows)
        })();
        if written.is_err() {
            curl.lock().unwrap().kill().ok();
            bz.kill().ok();
        }
        pumper.join().ok();
        let rc = (curl.lock().unwrap().wait()?, bz.wait()?);
        let rows = written?;
        anyhow::ensure!(rc.0.success() && rc.1.success(), "{month}: download failed (curl {}, bzip2 {})", rc.0, rc.1);
        Ok(rows)
    }
}

/// The NAS's temporary files of this Mac's copies whose process is gone (killed midway):
/// `<month>.tsv.zst.<host>.<pid>.tmp`.
fn sweep_nas(d: &Path) {
    let host = host();
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let parts: Vec<&str> = n.split('.').collect();
        // (Only a name this Mac's copy gave: one it can't tell, or another Mac's, isn't judged by a pid here.)
        if host.is_empty()
            || !n.ends_with(".tmp")
            || !n.contains(".tsv.zst.")
            || parts.len() < 6
            || parts[parts.len() - 3] != host
        {
            continue;
        }
        let Ok(pid) = parts[parts.len() - 2].parse::<i32>() else { continue };
        // SAFETY: kill with signal 0 only asks whether the process is there.
        let gone = unsafe { libc::kill(pid, 0) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

/// The heritage chain's pageviews, in its stand-in `root`: the items of data/heritage/wd/items.jsonl
/// with an article in `LANGS` (`wiki`) and the World Heritage Sites' components' (data/build/whs-sites.json:
/// a site is as well known as its best known part; the Rideau Canal's own item has no articles, the
/// canal's has), their articles from data/heritage/wd/wp.jsonl (those not in it asked of the
/// declared `wikidata` source and added), over the epoch's months (`months_before`). Writes
/// data/pageviews/items.json.
pub fn heritage(root: &Path, epoch: &str, wikidata: &str, months: &Months) -> Result<()> {
    let ms = months_before(epoch)?;
    let w = root.join("data/heritage/wd");
    let out = root.join("data/pageviews");
    let p = phase("the item lists and article cache read", Class::Disk);
    std::fs::create_dir_all(&out)?;
    let mut qids: BTreeSet<String> = BTreeSet::new();
    let items = std::fs::read_to_string(w.join("items.jsonl")).context("read items.jsonl")?;
    for line in items.lines() {
        let r = loads(line).context("a line of items.jsonl")?;
        if r.at("wiki").truthy() {
            qids.insert(r.at("qid").as_str().context("items.jsonl: a qid not a string")?.to_string());
        }
    }
    let whs = root.join("data/build/whs-sites.json");
    if whs.exists() {
        if let Py::Dict(sites) = loads(&std::fs::read_to_string(&whs)?).context("parse whs-sites.json")? {
            for (_, s) in sites {
                qids.extend(s.at("q").list().into_iter().flatten().filter_map(Py::as_str).map(str::to_string));
            }
        }
    }
    // Every item's Wikipedia articles (the cache shared with crate::heritagewd).
    let wp_path = w.join("wp.jsonl");
    let mut wp: Ordered<Py> = read_lines(&wp_path, |r| Ok(r.at("qid").py_str()))?;
    let need: Vec<String> = qids.iter().filter(|q| !wp.has(q)).cloned().collect();
    drop(p);
    eprintln!("{} items; articles to look up for {}", qids.len(), need.len());
    if !need.is_empty() {
        for (q, r) in wikipedias(&months.ask, wikidata, &need)? {
            let mut d = vec![("qid".to_string(), Py::Str(q.clone()))];
            if let Py::Dict(e) = r {
                d.extend(e);
            }
            wp.set(&q, Py::Dict(d));
        }
        let _p = phase("the article cache written", Class::Disk);
        write_atomic(&wp_path, &wp.iter().map(|(_, r)| dumps_spaced(r) + "\n").collect::<String>())?;
    }
    let qids: Vec<String> = qids.into_iter().collect();
    let arts_of: HashMap<String, Vec<String>> = qids.iter().map(|q| (q.clone(), arts_in_langs(wp.get(q)))).collect();
    let wanted: HashSet<String> = arts_of.values().flatten().map(|a| norm(a)).collect();
    eprintln!("{} articles; months {}", wanted.len(), ms.join(", "));
    let per_month = months.views(&ms, &wanted)?;
    let per_item = item_views(&qids, &arts_of, &per_month);
    {
        let _p = phase("the views written", Class::Disk);
        write_atomic(&out.join("items.json"), &dumps_spaced(&per_item))?;
    }
    if let Py::Dict(d) = &per_item {
        let mut top: Vec<&(String, Py)> = d.iter().collect();
        top.sort_by(|a, b| b.1.number().unwrap_or(0.0).total_cmp(&a.1.number().unwrap_or(0.0)));
        let top: Vec<String> = top.iter().take(10).map(|(q, v)| format!("({q}, {})", v.py_repr())).collect();
        eprintln!("items.json: {} items; most read: [{}]", d.len(), top.join(", "));
    }
    Ok(())
}

/// The items job's pageviews: per item of `views_q` (the candidates' first QIDs), its articles from
/// the epoch's article cache (`<cache>/wp-<epoch>.jsonl`, crate::items fills it), over the
/// epoch's months. Writes `<out>/views.json` and `<out>/meta.json` (the epoch, its months, the days
/// anything was fetched for it, `<cache>/fetched-<epoch>.json`, and the counts: `facts`, the items
/// whose facts were asked); then this Mac's indexes of months before the epoch's go (the NAS keeps
/// its own).
pub fn items(
    cache: &Path,
    epoch: &str,
    views_q: &BTreeSet<String>,
    facts: usize,
    months: &Months,
    out: &Path,
) -> Result<()> {
    let ms = months_before(epoch)?;
    let p = phase("the article cache read", Class::Disk);
    let wp = read_lines(&cache.join(format!("wp-{epoch}.jsonl")), |r| Ok(r.at("qid").py_str()))?;
    let qids: Vec<String> = views_q.iter().cloned().collect();
    let arts_of: HashMap<String, Vec<String>> = qids.iter().map(|q| (q.clone(), arts_in_langs(wp.get(q)))).collect();
    let wanted: HashSet<String> = arts_of.values().flatten().map(|a| norm(a)).collect();
    drop(p);
    eprintln!("views: {} articles over {}", wanted.len(), ms.join(", "));
    let per_month = months.views(&ms, &wanted)?;
    let views = item_views(&qids, &arts_of, &per_month);
    let n = if let Py::Dict(d) = &views { d.len() } else { 0 };
    let p = phase("the outputs written", Class::Disk);
    let fetched = match std::fs::read_to_string(cache.join(format!("fetched-{epoch}.json"))) {
        Ok(t) => loads(&t).context("parse fetched.json")?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Py::Dict(vec![]),
        Err(e) => return Err(e.into()),
    };
    let day = |k: &str| fetched.get(k).cloned().unwrap_or(Py::Null);
    let meta = Py::Dict(vec![
        ("epoch".into(), Py::Str(epoch.into())),
        ("months".into(), Py::List(ms.iter().map(|m| Py::Str(m.clone())).collect())),
        ("fetched".into(), Py::List(vec![day("first"), day("last")])),
        ("facts".into(), Py::Int(facts.to_string())),
        ("views".into(), Py::Int(n.to_string())),
    ]);
    std::fs::create_dir_all(out)?;
    write_atomic(&out.join("views.json"), &dumps_spaced(&views))?;
    write_atomic(&out.join("meta.json"), &dumps_spaced(&meta))?;
    drop(p);
    // (Later epochs' months are later.)
    let _p = phase("the old months deleted", Class::Disk);
    for e in std::fs::read_dir(&months.dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.get(..7).is_some_and(|m| m < ms[0].as_str()) {
            cachefile::try_remove(&e.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_months() {
        assert_eq!(months_before("2026-09-28").unwrap(), ["2025-11", "2026-02", "2026-05", "2026-08"]);
        // (August ended 1 September: 19 days isn't enough.)
        assert_eq!(months_before("2026-09-20").unwrap(), ["2025-08", "2025-11", "2026-02", "2026-05"]);
        assert_eq!(months_before("2026-09-21").unwrap(), ["2025-11", "2026-02", "2026-05", "2026-08"]);
        assert_eq!(months_before("2027-01-01").unwrap(), ["2026-02", "2026-05", "2026-08", "2026-11"]);
        assert!(months_before("2026-13-01").is_err());
    }

    #[test]
    fn views_rounded_as_python() {
        let s = |t| crate::pyjson::dumps(&mean_views(t, 4));
        // (round(x, 1) halves to even: 0.25 → 0.2, 0.75 → 0.8.)
        assert_eq!(
            [s(0), s(1), s(3), s(5), s(7), s(10), s(123457)],
            ["0.0", "0.2", "0.8", "1.2", "1.8", "2.5", "30864.2"]
        );
    }

    fn index(p: &Path, lines: &str) {
        let mut z = zstd::stream::write::Encoder::new(File::create(p).unwrap(), 3).unwrap();
        z.write_all(lines.as_bytes()).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn indexes_looked_up_copied_and_checked() {
        let d = tempfile::tempdir().unwrap();
        let (here, nas) = (d.path().join("months"), d.path().join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let head = format!("#langs\t{}\n", LANGS.join(","));
        index(&nas.join("2026-02.tsv.zst"), &format!("{head}en|A\t3\nen|B\t5\nen|A\t4\nfr|C\t1\n"));
        // (Offline: a month with no index anywhere is an error, never a download.)
        crate::fetch::go_offline();
        let m = Months::new(here.clone(), Some(nas.clone()), "https://example.org/{y}/{m}".into());
        let wanted: HashSet<String> = ["en|A", "fr|C", "en|Z"].iter().map(|s| s.to_string()).collect();
        let got = m.month_views("2026-02", &wanted).unwrap();
        assert_eq!(got, HashMap::from([("en|A".into(), 7), ("fr|C".into(), 1)]));
        assert!(here.join("2026-02.tsv.zst").exists());
        // An index here only goes up to a NAS without one.
        index(&here.join("2026-05.tsv.zst"), &format!("{head}en|A\t2\n"));
        assert_eq!(m.month_views("2026-05", &wanted).unwrap()["en|A"], 2);
        assert!(nas.join("2026-05.tsv.zst").exists());
        // A damaged copy here: the NAS's copied again.
        std::fs::remove_file(here.join("2026-02.tsv.zst")).unwrap();
        std::fs::write(here.join("2026-02.tsv.zst"), b"not zstd").unwrap();
        cachefile::release(&here.join("2026-02.tsv.zst"));
        assert_eq!(m.month_views("2026-02", &wanted).unwrap()["en|A"], 7);
        // One cut short, and the NAS's too: the month streamed again (offline: an error).
        let full = std::fs::read(nas.join("2026-02.tsv.zst")).unwrap();
        for p in [here.join("2026-02.tsv.zst"), nas.join("2026-02.tsv.zst")] {
            cachefile::release(&p);
            std::fs::remove_file(&p).unwrap();
            std::fs::write(&p, &full[..full.len() - 4]).unwrap();
        }
        assert!(m.month_views("2026-02", &wanted).is_err());
        // An index not counting a language asked: none.
        index(&here.join("2025-11.tsv.zst"), "#langs\ten\nen|A\t1\n");
        let fr: HashSet<String> = HashSet::from(["fr|C".to_string()]);
        let no_nas = Months::new(here.clone(), None, String::new());
        assert!(no_nas.index("2025-11", &langs_of(&fr), false).unwrap().is_none());
        let en: HashSet<String> = HashSet::from(["en|A".to_string()]);
        assert!(no_nas.index("2025-11", &langs_of(&en), false).unwrap().is_some());
    }

    /// Room-making mid-job (docs/building.md §2): the months' indexes looked up again and again
    /// (each copied here from the NAS when it isn't here), while every cache file no job holds is
    /// deleted as fast as it can be; every lookup gives what the first did.
    #[test]
    fn lookups_survive_room_making() {
        let d = tempfile::tempdir().unwrap();
        let (here, nas) = (d.path().join("months"), d.path().join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let months = ["2025-11", "2026-02", "2026-05"];
        for (i, m) in months.iter().enumerate() {
            let mut s = format!("#langs\t{}\n", LANGS.join(","));
            for k in 0..20000 {
                s += &format!("en|Page_{k}\t{}\n", (k * (i + 3)) % 997);
            }
            index(&nas.join(format!("{m}.tsv.zst")), &s);
        }
        // (Offline: a month whose index can't be had from the NAS fails at once, never downloads.)
        crate::fetch::go_offline();
        let m = Months::new(here.clone(), Some(nas), String::new());
        let wanted: HashSet<String> = (0..20000).step_by(7).map(|k| format!("en|Page_{k}")).collect();
        let look = || {
            let got: Vec<_> = months.iter().map(|mo| m.month_views(mo, &wanted).unwrap()).collect();
            // (The job's holds let go, as a job's end would: the deleter may take them now.)
            for mo in months {
                cachefile::release(&here.join(format!("{mo}.tsv.zst")));
            }
            got
        };
        let calm = look();
        let stop = std::sync::atomic::AtomicBool::new(false);
        let gone = std::thread::scope(|s| {
            let deleter = s.spawn(|| {
                let mut gone = 0;
                while !stop.load(Ordering::Relaxed) {
                    for e in std::fs::read_dir(&here).into_iter().flatten().flatten() {
                        gone += matches!(cachefile::try_remove(&e.path()), cachefile::Removed::Freed(_)) as u32;
                    }
                }
                gone
            });
            for _ in 0..40 {
                assert!(look() == calm, "a lookup with the deleter under way differs");
            }
            stop.store(true, Ordering::Relaxed);
            deleter.join().unwrap()
        });
        assert!(gone > 0, "the deleter took nothing");
    }

    #[test]
    fn items_and_meta_written() {
        let d = tempfile::tempdir().unwrap();
        let (cache, out) = (d.path().join("cache"), d.path().join("out"));
        let here = cache.join("months");
        std::fs::create_dir_all(&here).unwrap();
        let head = format!("#langs\t{}\n", LANGS.join(","));
        for (k, m) in ["2025-11", "2026-02", "2026-05", "2026-08"].iter().enumerate() {
            index(&here.join(format!("{m}.tsv.zst")), &format!("{head}en|St_Paul's\t{}\ngd|Eilean\t1\n", k + 1));
        }
        index(&here.join("2025-08.tsv.zst"), &head);
        std::fs::write(
            cache.join("wp-2026-09-28.jsonl"),
            "{\"qid\": \"Q2\", \"n\": 2, \"arts\": [\"en|St Paul's\", \"de|X\"]}\n{\"qid\": \"Q1\", \"n\": 1, \"arts\": [\"gd|Eilean\"]}\n{\"qid\": \"Q3\", \"n\": 1, \"arts\": [\"de|Y\"]}\n",
        )
        .unwrap();
        std::fs::write(cache.join("fetched-2026-09-28.json"), "{\"first\": \"2026-10-05\", \"last\": \"2026-10-22\"}")
            .unwrap();
        crate::fetch::go_offline();
        let m = Months::new(here.clone(), None, String::new());
        let q: BTreeSet<String> = ["Q1", "Q2", "Q3", "Q4"].iter().map(|s| s.to_string()).collect();
        items(&cache, "2026-09-28", &q, 7, &m, &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("views.json")).unwrap(), "{\"Q1\": 1.0, \"Q2\": 2.5}");
        assert_eq!(
            std::fs::read_to_string(out.join("meta.json")).unwrap(),
            "{\"epoch\": \"2026-09-28\", \"months\": [\"2025-11\", \"2026-02\", \"2026-05\", \"2026-08\"], \"fetched\": [\"2026-10-05\", \"2026-10-22\"], \"facts\": 7, \"views\": 2}"
        );
        assert!(!here.join("2025-08.tsv.zst").exists() && here.join("2025-11.tsv.zst").exists());
    }
}
