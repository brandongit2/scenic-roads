//! The timetable feeds (GTFS) where the coverage is, each fetched once: those that run passenger
//! rail (trains, metros, trams, funiculars) for the rail counter (crate::gtfs::railcount), and the
//! ferry feeds for the ferry counter (crate::gtfs::ferrycount, run by the ferries step). The
//! `rail-feeds` step runs it (docs/steps.md §10) on the accepted declarations of
//! inputs/timetables/feeds/ (docs/inputs.md §6.7.1: each as crate::inputs::feeds gives it).
//!
//! The feeds:
//!   - the Mobility Database catalogue's (the `mdb-catalogue` declaration; its pinned copy, else
//!     fetched from its URL into the out folder's catalogue.csv): active GTFS feeds of the countries
//!     the coverage is in (which the step works out from the pass's outlines; the declaration's
//!     `country` corrects the catalogue's country of a few) whose bounding box meets the coverage,
//!     with a download that needs no key, and not in its `exclude`. Of those, the ones with rail
//!     routes: only routes.txt is read, straight out of the zip by HTTP range requests (the
//!     catalogue's mirror, else the operator's URL), once per feed (`checked` keeps every feed's
//!     answer; one that got no answer is asked again next time);
//!   - the declared feeds (`gtfs`), in their `order`, then by id: for the rail list those whose
//!     `modes` have rail (or say none: then their routes.txt must have rail routes) where one of
//!     their territories is a country the coverage is in and their stops (the zip's stops.txt) meet
//!     the coverage; for the ferry list those whose `modes` have ferry (or say none), wherever they
//!     are (the ferries are mapped worldwide). A feed behind an API key (`[data.fetch] key`) is in
//!     either only when the keys hold its key (the NAS's inputs/keys.env, KEY=value lines: a key is
//!     read only to fetch its feed, and never written anywhere).
//! A feed another `replaces` (an operator's own feed over an aggregate republishing it, and the
//! catalogue declaration's `replaces` among its copies of one timetable) is left out while that one
//! has a zip (or is tried again), and read when it has none. The rail list has the declared feeds
//! first, so a train they share with a regional aggregate counts as theirs.
//!
//! Each feed is downloaded once: the NAS's zips (feed id → its path and the day it counts from)
//! are used, and only a feed not there is fetched, into the out folder's gtfs/<id>.zip (a zip left
//! there by a run cut short is used as it is). A zip on the NAS without service in its window (it
//! was out of date when fetched) is fetched again when this runs its declaration's `refresh` days
//! (REFRESH_DAYS for the catalogue's) or more after its day: the new zip replaces it, counting from
//! today (the NAS keeps one copy if it's the same file, which then isn't fetched again for another
//! while); a refusal or no answer leaves the old one, asked again the next time. How a feed is
//! fetched is its declaration's `[data.fetch] method`: `get` (default), `get-key-header` (the key in
//! a header), `get-key-json-link` (the key in a header, the answer JSON holding the download link at
//! `link`).
//!
//! What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): the operators' published feeds and the
//! catalogue (`data`), some through their keyed APIs (`api`): no robots.txt is read. Every request
//! carries crate::fetch::USER_AGENT, is refused offline first (crate::fetch::online), goes through
//! the job's proxy (`HTTPS_PROXY`), and a 429 or a `Retry-After` is told to it (crate::budget::seen)
//! and backed off from.
//!
//! A request that gets no answer (no connection, a 429 or a 5xx, an answer cut short) is tried three
//! times. A feed still without one (or whose file changed while it was read) fails the run, after
//! the checks and downloads that worked are written, so the step tries again later; once its server
//! has given none for NO_ANSWER_DAYS (unanswered.json keeps the first day from run to run, until the
//! step completes and removes the out folder), it's left out instead, its status naming the first
//! and the last day, and it's tried again the next time rail-feeds runs. A definite refusal (a 404,
//! a file that isn't a zip) leaves that feed out. A zip on the NAS that can't be read now fails the
//! run.
//!
//! Writes, in the out folder: feeds.json, {"feeds": […]}: the rail feeds in the order the rail
//! counter reads them, each with its facts, `zip` ("cache" for the NAS's, "new" for a download, none
//! when left out, with `status` saying why) and `fetched`; ferries.json likewise, the ferry feeds
//! (id, name, url, source, ferry_routes); and checked.json (the checks so far with this run's
//! answers, its bytes the Python's: crate::pyjson). Each is written by a temporary name, then
//! renamed.

use crate::coverage::Coverage;
use crate::gtfs::{self, ferrycount, railcount, Date, Kind, Lines};
use crate::pyjson::{self, Py};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A stale zip on the NAS is fetched again when this runs this many days or more after its day,
/// unless its declaration says.
const REFRESH_DAYS: i64 = 7;
/// Tries of a request that gets no answer, and the waits between them.
const TRIES: u64 = 3;
const WAIT_S: u64 = 20;
/// Days a feed's server may give no answer before the feed is left out, rather than failing the run.
const NO_ANSWER_DAYS: i64 = 3;
/// The rail route types (railcount::mode_of's).
fn rail_type(t: i64) -> bool {
    railcount::mode_of(t).is_some()
}

/// Why a request has no answer yet (asked again later), or a definite refusal (a status).
enum Why {
    NoAnswer(String),
    Refused(String),
}

/// An HTTP answer: its status (0: none), body and headers (lower-case names).
struct Answer {
    code: u16,
    body: Vec<u8>,
    headers: HashMap<String, String>,
}

fn transient(code: u16) -> bool {
    code == 0 || code == 429 || code >= 500
}

fn no_answer(code: u16) -> Why {
    Why::NoAnswer(if code != 0 { format!("http {code}") } else { "none".into() })
}

/// curl, as the requests are made (its TLS reaches servers that offer only the older suites, as
/// some operators' still do).
struct Net;

/// A program's exit code and what it printed, curl's `-D - -o -` (its stdin given `input`).
fn curl_run(args: &[std::ffi::OsString], input: Option<&[u8]>) -> (i32, Vec<u8>) {
    use std::io::Write;
    use std::process::Stdio;
    let mut c = store::spawn::command("curl");
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let Ok(mut child) = store::spawn::spawn(&mut c) else { return (-1, Vec::new()) };
    if let (Some(data), Some(mut w)) = (input, child.stdin.take()) {
        w.write_all(data).ok();
    }
    match child.wait_with_output() {
        Ok(o) => (o.status.code().unwrap_or(-1), o.stdout),
        Err(_) => (-1, Vec::new()),
    }
}

/// curl's `-D - -o -` output: the last header block's status and headers, and the body (status 0
/// when the headers were cut short).
fn answer(out: Vec<u8>) -> Answer {
    let (mut code, mut headers, mut body) = (0u16, HashMap::new(), &out[..]);
    while body.starts_with(b"HTTP/") {
        let Some(end) = body.windows(4).position(|w| w == b"\r\n\r\n") else {
            return Answer { code: 0, body: Vec::new(), headers: HashMap::new() };
        };
        // (Latin-1, as the Python decoded it.)
        let block: String = body[..end].iter().map(|&b| b as char).collect();
        let mut lines = block.split("\r\n");
        code = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
        headers = lines
            .map(|l| {
                let (k, v) = l.split_once(':').unwrap_or((l, ""));
                (crate::pytext::strip(k).to_lowercase(), crate::pytext::strip(v).to_string())
            })
            .collect();
        body = &body[end + 4..];
    }
    Answer { code, body: body.to_vec(), headers }
}

fn length(h: &HashMap<String, String>) -> Option<u64> {
    h.get("content-length").and_then(|l| gtfs::py_int(l)).and_then(|l| u64::try_from(l).ok())
}

fn os(s: impl AsRef<std::ffi::OsStr>) -> std::ffi::OsString {
    s.as_ref().to_os_string()
}

impl Net {
    /// One request: a range's bytes (`rng`, inclusive: an answer with more than the range keeps
    /// its status but no body, curl's --max-filesize), a HEAD, or the whole file; status 0 for no
    /// answer (no connection, a timeout, an answer cut short).
    fn once(&self, url: &str, rng: Option<(u64, u64)>, head: bool) -> Answer {
        let none = Answer { code: 0, body: Vec::new(), headers: HashMap::new() };
        if crate::fetch::online(url).is_err() {
            return none;
        }
        let mut args: Vec<std::ffi::OsString> =
            ["-sS", "-L", "-m", "90", "-A", crate::fetch::USER_AGENT, "-D", "-", "-o", "-"].iter().map(os).collect();
        if let Some((a, z)) = rng {
            args.extend([os("-r"), os(format!("{a}-{z}")), os("--max-filesize"), os((z - a + 1).to_string())]);
        }
        if head {
            args.push(os("-I"));
        }
        args.push(os(url));
        let (rc, out) = curl_run(&args, None);
        let mut a = answer(out);
        if rc == 63 && rng.is_some() {
            a.body.clear(); // (more than the range: the status stands)
        } else if rc != 0 || (!head && length(&a.headers).is_some_and(|l| l != a.body.len() as u64)) {
            a.code = 0;
        }
        crate::budget::seen(url, a.code, a.headers.get("retry-after").map(String::as_str));
        a
    }

    /// `once`, tried up to TRIES times while it gets no answer.
    fn ask(&self, url: &str, rng: Option<(u64, u64)>, head: bool) -> Answer {
        let mut a = self.once(url, rng, head);
        for attempt in 1..TRIES {
            if !transient(a.code) {
                break;
            }
            std::thread::sleep(Duration::from_secs(WAIT_S * attempt));
            a = self.once(url, rng, head);
        }
        a
    }

    /// Bytes start..=end of a remote file of `size` bytes, by a range request.
    fn ranged(&self, url: &str, start: u64, end: u64, size: u64) -> std::result::Result<Vec<u8>, Why> {
        let a = self.ask(url, Some((start, end)), false);
        if transient(a.code) {
            return Err(no_answer(a.code));
        }
        if a.code == 200 {
            return Err(Why::Refused("no range requests".into()));
        }
        if a.code != 206 {
            return Err(Why::Refused(format!("http {}", a.code)));
        }
        let total = a.headers.get("content-range").map_or("*", |c| c.rsplit('/').next().unwrap_or(""));
        if total != size.to_string() && total != "*" {
            return Err(Why::NoAnswer("the file changed while it was read".into()));
        }
        if a.body.len() as u64 != end - start + 1 {
            return Err(Why::NoAnswer("an answer cut short".into()));
        }
        Ok(a.body)
    }

    /// A member of a remote zip of `size` bytes, by range requests (its central directory, then the
    /// member).
    fn zip_member(&self, url: &str, name: &str, size: u64) -> std::result::Result<Vec<u8>, Why> {
        let start = size - size.min(1 << 20);
        let a = self.ask(url, Some((start, size - 1)), false);
        if a.code == 200 && a.body.len() as u64 == size {
            // The server ignores ranges, and the whole file came (it's no bigger than the range).
            return match crate::inputs::gtfs::unzip(&a.body) {
                Ok(files) => files.get(name).cloned().ok_or_else(|| Why::Refused(format!("no {name}"))),
                Err(e) if format!("{e:#}").contains("compression method") => {
                    Err(Why::Refused("an unknown compression".into()))
                }
                Err(_) => Err(Why::Refused("not a zip".into())),
            };
        }
        if transient(a.code) {
            return Err(no_answer(a.code));
        }
        if a.code == 200 {
            return Err(Why::Refused("no range requests".into()));
        }
        if a.code != 206 {
            return Err(Why::Refused(format!("http {}", a.code)));
        }
        let total = a.headers.get("content-range").map_or("*", |c| c.rsplit('/').next().unwrap_or(""));
        if total != size.to_string() && total != "*" {
            return Err(Why::NoAnswer("the file changed while it was read".into()));
        }
        let tail = a.body;
        if tail.len() as u64 != size - start {
            return Err(Why::NoAnswer("an answer cut short".into()));
        }
        let not_zip = || Why::Refused("not a zip".into());
        let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as u64;
        let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as u64;
        let u64_at = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let rfind = |b: &[u8], sig: &[u8], to: usize| {
            (0..=to.saturating_sub(sig.len())).rev().find(|&i| i + sig.len() <= to && &b[i..i + sig.len()] == sig)
        };
        let i = rfind(&tail, b"PK\x05\x06", tail.len()).ok_or_else(not_zip)?;
        if tail.len() - i < 22 {
            return Err(not_zip());
        }
        let (mut cd_size, mut cd_off) = (u32_at(&tail, i + 12), u32_at(&tail, i + 16));
        if cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
            let j = rfind(&tail, b"PK\x06\x06", i).ok_or_else(not_zip)?;
            if tail.len() - j < 56 {
                return Err(not_zip());
            }
            (cd_size, cd_off) = (u64_at(&tail, j + 40), u64_at(&tail, j + 48));
        }
        if cd_off + cd_size > size {
            return Err(not_zip());
        }
        let cd = if cd_off >= start {
            let a = (cd_off - start) as usize;
            tail[a.min(tail.len())..(a + cd_size as usize).min(tail.len())].to_vec()
        } else if cd_size > 0 {
            self.ranged(url, cd_off, cd_off + cd_size - 1, size)?
        } else {
            Vec::new()
        };
        let mut p = 0;
        while p + 46 <= cd.len() && &cd[p..p + 4] == b"PK\x01\x02" {
            let method = u16_at(&cd, p + 10);
            let crc = u32_at(&cd, p + 16) as u32;
            let (mut csz, mut usz) = (u32_at(&cd, p + 20), u32_at(&cd, p + 24));
            let (nl, el, cl) =
                (u16_at(&cd, p + 28) as usize, u16_at(&cd, p + 30) as usize, u16_at(&cd, p + 32) as usize);
            let mut off = u32_at(&cd, p + 42);
            let fname = String::from_utf8_lossy(&cd[(p + 46).min(cd.len())..(p + 46 + nl).min(cd.len())]).into_owned();
            let extra = &cd[(p + 46 + nl).min(cd.len())..(p + 46 + nl + el).min(cd.len())];
            let mut q = 0;
            while q + 4 <= extra.len() {
                let (hid, hl) = (u16_at(extra, q), u16_at(extra, q + 2) as usize);
                if hid == 1 {
                    let n = hl.min(extra.len() - q - 4) / 8;
                    let mut vals = (0..n).map(|k| u64_at(extra, q + 4 + 8 * k));
                    if usz == 0xFFFF_FFFF {
                        usz = vals.next().unwrap_or(usz);
                    }
                    if csz == 0xFFFF_FFFF {
                        csz = vals.next().unwrap_or(csz);
                    }
                    if off == 0xFFFF_FFFF {
                        off = vals.next().unwrap_or(off);
                    }
                }
                q += 4 + hl;
            }
            if fname.rsplit('/').next() == Some(name) {
                return self.member(url, size, method, crc, csz, usz, off);
            }
            p += 46 + nl + el + cl;
        }
        Err(Why::Refused(format!("no {name}")))
    }

    /// A zip member's bytes, from its local header (at `off`) and the central directory's facts:
    /// exactly as many as the zip says, with its CRC.
    #[allow(clippy::too_many_arguments)]
    fn member(
        &self,
        url: &str,
        size: u64,
        method: u64,
        crc: u32,
        csz: u64,
        usz: u64,
        off: u64,
    ) -> std::result::Result<Vec<u8>, Why> {
        if off + 30 > size {
            return Err(Why::Refused("not a zip".into()));
        }
        let lh = self.ranged(url, off, off + 29, size)?;
        if &lh[..4] != b"PK\x03\x04" {
            return Err(Why::NoAnswer("its local header came back damaged".into()));
        }
        let (n2, e2) = (u16::from_le_bytes([lh[26], lh[27]]) as u64, u16::from_le_bytes([lh[28], lh[29]]) as u64);
        let data_start = off + 30 + n2 + e2;
        if data_start + csz > size {
            return Err(Why::Refused("not a zip".into()));
        }
        let comp = if csz > 0 { self.ranged(url, data_start, data_start + csz - 1, size)? } else { Vec::new() };
        let data = match method {
            0 => comp,
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(&comp[..])
                    .read_to_end(&mut v)
                    .map_err(|_| Why::NoAnswer("its data came back damaged".into()))?;
                v
            }
            m => return Err(Why::Refused(format!("an unknown compression (method {m})"))),
        };
        if data.len() as u64 != usz || crc32fast::hash(&data) != crc {
            return Err(Why::NoAnswer("its data came back damaged".into()));
        }
        Ok(data)
    }
}

/// `round(x, 1)` as a float.
fn round1(x: f64) -> f64 {
    crate::interest::py_round(x, 1)
}

/// A catalogue feed's rail routes, from its routes.txt alone: `status` "ok" with `rail_routes` and
/// `examples`, a definite answer why there are none, or "no answer (…)" (asked again next time).
fn check(net: &Net, mut feed: Py) -> Py {
    let url = feed.at("url").py_str();
    let res = (|| -> std::result::Result<(), Why> {
        let a = net.ask(&url, None, true);
        if transient(a.code) {
            return Err(no_answer(a.code));
        }
        if a.code != 200 {
            return Err(Why::Refused(format!("http {}", a.code)));
        }
        let size: u64 = a.headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
        feed.set("size_mb", Py::Float(round1(size as f64 / 1e6)));
        if size == 0 {
            return Err(Why::Refused("no size given".into()));
        }
        let routes = net.zip_member(&url, "routes.txt", size)?;
        let csv = gtfs::Csv::with_lines(Box::new(std::io::Cursor::new(routes)), true, Lines::LfOnly);
        let rows = (|| -> gtfs::R<Vec<String>> {
            let mut rows = gtfs::DictRows::new(Some(csv))?;
            let mut rail = Vec::new();
            while let Some(cells) = rows.next_cells()? {
                let r = rows.row(cells);
                let Some(t) = r.cell("route_type").get_or_empty().ok().and_then(gtfs::py_int) else { continue };
                if rail_type(t) {
                    let name = [r.cell("route_short_name").get(), r.cell("route_long_name").get()]
                        .into_iter()
                        .flatten()
                        .find(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| r.cell("route_id").get().map_or("None".into(), str::to_string));
                    rail.push(format!("{name} ({t})"));
                }
            }
            Ok(rail)
        })();
        match rows {
            Ok(rail) => {
                feed.set("rail_routes", Py::Int(rail.len().to_string()));
                feed.set("examples", Py::List(rail.into_iter().take(8).map(Py::Str).collect()));
                feed.set("status", Py::Str("ok".into()));
            }
            Err(e) if e.kind == Kind::Csv => feed.set("status", Py::Str(format!("routes.txt unreadable ({e})"))),
            Err(e) => feed.set("status", Py::Str(format!("no answer ({}: {e})", e.class()))),
        }
        Ok(())
    })();
    match res {
        Ok(()) => {}
        Err(Why::NoAnswer(e)) => feed.set("status", Py::Str(format!("no answer ({e})"))),
        Err(Why::Refused(e)) => feed.set("status", Py::Str(e)),
    }
    feed
}

/// A check that got no answer (asked again next time): "no answer (…)", or an earlier check's
/// "http <code>" with a code worth asking again for.
fn unanswered(c: &Py) -> bool {
    let s = c.at("status").as_str().unwrap_or("");
    if s.starts_with("no answer") {
        return true;
    }
    let w: Vec<&str> = s.split_whitespace().collect();
    w.len() == 2
        && w[0] == "http"
        && w[1].bytes().all(|b| b.is_ascii_digit())
        && w[1].parse::<u16>().map_or(true, transient)
}

/// Whether the box w, s, e, n (degrees) meets the coverage (crate::coverage's `meets_rect`).
fn meets(cov: &Coverage, b: [f64; 4]) -> bool {
    let e7 = |v: f64, lim: f64| (v.clamp(-lim, lim) * 1e7).round() as i32;
    cov.meets_rect([e7(b[0], 180.0), e7(b[1], 90.0), e7(b[2], 180.0), e7(b[3], 90.0)])
}

/// The catalogue's active GTFS feeds of `countries` (as `country_of`, its declaration's `country`,
/// corrects them) whose box meets the coverage, in its order.
fn catalogue_feeds(
    path: &Path,
    cov: &Coverage,
    countries: &HashSet<String>,
    country_of: &HashMap<String, String>,
) -> Result<Vec<Py>> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let csv = gtfs::Csv::with_lines(Box::new(std::io::BufReader::new(f)), false, Lines::Universal);
    let mut rows = gtfs::DictRows::new(Some(csv)).map_err(|e| anyhow::anyhow!("the catalogue: {e}"))?;
    let mut out = Vec::new();
    while let Some(cells) = rows.next_cells().map_err(|e| anyhow::anyhow!("the catalogue: {e}"))? {
        let r = rows.row(cells);
        let c = |k: &str| r.cell(k).req(k).map_err(|e| anyhow::anyhow!("the catalogue: {}: {e}", e.class()));
        let id = c("id")?;
        let country = country_of.get(id).map_or(c("location.country_code")?, String::as_str);
        if c("data_type")? != "gtfs"
            || !countries.contains(country)
            || matches!(c("status")?, "deprecated" | "inactive")
        {
            continue;
        }
        if !c("redirect.id")?.is_empty() {
            continue;
        }
        let mut bb = [0.0; 4];
        let mut ok = true;
        for (k, col) in
            ["minimum_longitude", "minimum_latitude", "maximum_longitude", "maximum_latitude"].iter().enumerate()
        {
            match gtfs::py_float(c(&format!("location.bounding_box.{col}"))?) {
                Some(v) => bb[k] = v,
                None => ok = false,
            }
            if !ok {
                break;
            }
        }
        if !ok || !meets(cov, bb) {
            continue;
        }
        let url = match c("urls.latest")? {
            "" if matches!(c("urls.authentication_type")?, "" | "0") => c("urls.direct_download")?,
            "" => "",
            u => u,
        };
        if url.is_empty() {
            continue;
        }
        let s = |x: &str| Py::Str(x.to_string());
        out.push(Py::Dict(vec![
            ("id".into(), s(id)),
            ("provider".into(), s(c("provider")?)),
            ("name".into(), s(c("name")?)),
            ("country".into(), s(country)),
            ("subdivision".into(), s(c("location.subdivision_name")?)),
            ("url".into(), s(url)),
            ("licence".into(), s(c("urls.license")?)),
        ]));
    }
    Ok(out)
}

/// The value at `path` (`value[0].link`: keys and list indexes) in JSON `v`; a string on the way
/// that holds JSON (or a Python literal's simple JSON-like form, as LTA's does) is read as such.
/// None: the path isn't there (an IndexError: no link); Err: an answer it can't read.
fn json_at(v: &Py, path: &str) -> std::result::Result<Option<Py>, ()> {
    let mut v = v.clone();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            '[' => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
                let mut d = String::from("[");
                for c in chars.by_ref() {
                    d.push(c);
                    if c == ']' {
                        break;
                    }
                }
                parts.push(d);
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    for part in parts {
        if let Py::Str(s) = &v {
            v = pyjson::loads(s).or_else(|_| pyjson::loads(&s.replace('\'', "\""))).map_err(|_| ())?;
        }
        v = if let Some(i) = part.strip_prefix('[').and_then(|p| p.strip_suffix(']')) {
            let i: usize = i.parse().map_err(|_| ())?;
            match v.list() {
                Some(l) => match l.get(i) {
                    Some(x) => x.clone(),
                    None => return Ok(None),
                },
                None => return Err(()),
            }
        } else {
            match &v {
                Py::Dict(_) => v.get(&part).cloned().ok_or(())?,
                _ => return Err(()),
            }
        };
    }
    Ok(Some(v))
}

/// Whether a file is a zip (`zipfile.is_zipfile`: an end of central directory record in its last
/// 64 KiB and 22 bytes).
fn is_zipfile(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
    use std::io::{Seek, SeekFrom};
    let from = len.saturating_sub(22 + (1 << 16));
    if f.seek(SeekFrom::Start(from)).is_err() {
        return false;
    }
    let mut tail = Vec::new();
    if f.read_to_end(&mut tail).is_err() {
        return false;
    }
    (0..tail.len().saturating_sub(3))
        .rev()
        .find(|&i| &tail[i..i + 4] == b"PK\x05\x06")
        .is_some_and(|i| tail.len() - i >= 22)
}

/// What a keyed request's failure is called: its HTTP status (an error answer), or the class of
/// error the Python's urllib named for no answer (never the key, which travels on curl's stdin).
fn no_answer_class(rc: i32) -> &'static str {
    match rc {
        28 => "TimeoutError",
        52 => "RemoteDisconnected",
        56 => "ConnectionResetError",
        _ => "URLError",
    }
}

impl Net {
    /// A GET of a keyed feed's URL, its key in its header (given on curl's stdin, so no other
    /// process sees it), into `to` (the body: `-` for stdout): the status, or the failure.
    fn keyed(
        &self,
        feed: &Py,
        key: &str,
        json: bool,
        to: &Path,
    ) -> std::result::Result<(u16, Vec<u8>), (u16, &'static str)> {
        let url = feed.at("url").py_str();
        crate::fetch::online(&url).map_err(|_| (0, "URLError"))?;
        let f = feed.at("fetch");
        let headers =
            format!("{}: {key}\naccept: {}\n", f.at("header").py_str(), if json { "application/json" } else { "*/*" });
        let args: Vec<std::ffi::OsString> = vec![
            os("-sS"),
            os("-L"),
            os("-m"),
            os(if json { "60" } else { "3600" }),
            os("-A"),
            os(crate::fetch::USER_AGENT),
            os("-H"),
            os("@-"),
            os("-D"),
            os("-"),
            os("-o"),
            os(to),
            os(&url),
        ];
        let (rc, out) = curl_run(&args, Some(headers.as_bytes()));
        let a = answer(out);
        crate::budget::seen(&url, a.code, a.headers.get("retry-after").map(String::as_str));
        if rc != 0 || a.code == 0 {
            return Err((0, no_answer_class(rc)));
        }
        if a.code >= 400 {
            return Err((a.code, "HTTPError"));
        }
        Ok((a.code, a.body))
    }

    /// Downloads a feed's zip to `dest`: Ok(true), Ok(false) with why not, or Err for a request
    /// that got no answer after its tries.
    fn fetch(&self, feed: &Py, dest: &Path, key: Option<&str>) -> std::result::Result<(bool, String), String> {
        if dest.exists() && is_zipfile(dest) {
            return Ok((true, "ok".into()));
        }
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = dest.with_extension("part");
        let method = feed.at("fetch").get("method").map_or("get".to_string(), Py::py_str);
        let key = key.unwrap_or("");
        for attempt in 0..TRIES {
            let last = attempt + 1 == TRIES;
            let wait = || std::thread::sleep(Duration::from_secs(WAIT_S * (attempt + 1)));
            let failed = |code: u16, class: &str| -> std::result::Result<Option<(bool, String)>, String> {
                if code != 0 && !transient(code) {
                    return Ok(Some((false, format!("its download link: http {code}"))));
                }
                if last {
                    return Err(if code != 0 {
                        format!("its download link: http {code}")
                    } else {
                        format!("its download link: {class}")
                    });
                }
                Ok(None)
            };
            let url = match method.as_str() {
                "get-key-header" => {
                    // (The zip itself, the key in a header.)
                    match self.keyed(feed, key, false, &tmp) {
                        Ok(_) if is_zipfile(&tmp) => {
                            std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
                            return Ok((true, "ok".into()));
                        }
                        Ok(_) => {
                            std::fs::remove_file(&tmp).ok();
                            return Ok((false, "download failed: not a zip".into()));
                        }
                        Err((code, class)) => {
                            std::fs::remove_file(&tmp).ok();
                            if let Some(r) = failed(code, class)? {
                                return Ok(r);
                            }
                            wait();
                            continue;
                        }
                    }
                }
                "get-key-json-link" => match self.keyed(feed, key, true, Path::new("-")) {
                    Ok((_, body)) => {
                        let link = std::str::from_utf8(&body)
                            .ok()
                            .and_then(|t| pyjson::loads(t).ok())
                            .ok_or(())
                            .and_then(|v| json_at(&v, &feed.at("fetch").at("link").py_str()));
                        match link {
                            Ok(Some(l)) if l.truthy() => l.py_str(),
                            Ok(_) => return Ok((false, "no download link".into())),
                            Err(()) => {
                                std::fs::remove_file(&tmp).ok();
                                return Ok((false, "its download link: an answer it can't read".into()));
                            }
                        }
                    }
                    Err((code, class)) => {
                        if let Some(r) = failed(code, class)? {
                            return Ok(r);
                        }
                        wait();
                        continue;
                    }
                },
                _ => feed.at("url").py_str(),
            };
            if url.is_empty() {
                return Ok((false, "no download link".into()));
            }
            let (code, rc) = self.download(&url, &tmp);
            if rc == 0 && code == 200 && is_zipfile(&tmp) {
                std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
                return Ok((true, "ok".into()));
            }
            std::fs::remove_file(&tmp).ok();
            if rc == 0 && !transient(code) {
                return Ok((
                    false,
                    if code != 200 {
                        format!("download failed: http {code}")
                    } else {
                        "download failed: not a zip".into()
                    },
                ));
            }
            if last {
                return Err(format!("http {code}, curl exit {rc}"));
            }
            wait();
        }
        Ok((false, "download failed".into()))
    }

    /// A file's download into `to`: its status (0: none) and curl's exit.
    fn download(&self, url: &str, to: &Path) -> (u16, i32) {
        if crate::fetch::online(url).is_err() {
            return (0, 7);
        }
        let args: Vec<std::ffi::OsString> = vec![
            os("-sSL"),
            os("-m"),
            os("3600"),
            os("-A"),
            os(crate::fetch::USER_AGENT),
            os("-o"),
            os(to),
            os("-w"),
            os("%{http_code}"),
            os(url),
        ];
        let (rc, out) = curl_run(&args, None);
        let text = String::from_utf8_lossy(&out);
        let code = crate::pytext::strip(&text).parse().unwrap_or(0);
        crate::budget::seen(url, code, None);
        (code, rc)
    }
}

/// inputs/keys.env's keys (KEY=value lines; # comments).
fn keys(path: Option<&Path>) -> HashMap<String, String> {
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else { return HashMap::new() };
    let mut out = HashMap::new();
    for l in text.lines() {
        let (k, v) = l.split_once('=').unwrap_or((l, ""));
        let (k, v) = (crate::pytext::strip(k), crate::pytext::strip(v));
        if !v.is_empty() && !k.starts_with('#') {
            out.insert(k.to_string(), v.to_string());
        }
    }
    out
}

/// A declaration's `facts.feed` (serde) as Python's json read it.
fn declared(d: &Py, countries: &HashSet<String>) -> Py {
    let terr = d.at("territory").list().cloned().unwrap_or_default();
    let country = terr.iter().filter_map(Py::as_str).find(|c| countries.contains(*c)).unwrap_or("");
    let or = |k: &str, dflt: Py| d.get(k).filter(|v| v.truthy()).cloned().unwrap_or(dflt);
    let fetch = or("fetch", Py::Dict(vec![("method".into(), Py::Str("get".into()))]));
    let mut f = vec![
        ("id".to_string(), d.at("id").clone()),
        ("provider".into(), d.at("name").clone()),
        ("url".into(), d.at("url").clone()),
        ("licence".into(), d.at("licence").clone()),
        ("country".into(), Py::Str(country.into())),
        ("replaces".into(), or("replaces", Py::List(Vec::new()))),
        ("fetch".into(), fetch.clone()),
        ("refresh_days".into(), d.get("refresh_days").cloned().unwrap_or(Py::Int(REFRESH_DAYS.to_string()))),
        ("modes".into(), or("modes", Py::List(Vec::new()))),
        ("name_".into(), d.at("name").clone()),
        ("source".into(), d.get("source").cloned().unwrap_or(Py::Str(String::new()))),
        ("ferry_routes".into(), or("ferry_routes", Py::List(Vec::new()))),
    ];
    if let Some(k) = fetch.get("key").filter(|k| k.truthy()) {
        f.push(("key".into(), k.clone()));
    }
    Py::Dict(f)
}

/// Whether a declaration's `modes` take it to `mode`'s counter (none: by its routes).
fn serves(d: &Py, mode: &str) -> bool {
    let m = d.at("modes");
    !m.truthy() || m.list().is_some_and(|l| l.iter().any(|x| x.as_str() == Some(mode)))
}

/// The box of a zip's stops (stops.txt), w, s, e, n, or None when it has none.
fn stops_box(path: &Path) -> gtfs::R<Option<[f64; 4]>> {
    let z = gtfs::Zip::open(path)?;
    let mut rows = z.stripped_rows("stops.txt")?;
    let mut b: Option<[f64; 4]> = None;
    while let Some(cells) = rows.next_cells()? {
        let s = rows.row(cells);
        let (Some(x), Some(y)) =
            (gtfs::py_float(s.cell("stop_lon").or_empty()), gtfs::py_float(s.cell("stop_lat").or_empty()))
        else {
            continue;
        };
        if (x, y) != (0.0, 0.0) && (-180.0..=180.0).contains(&x) && (-90.0..=90.0).contains(&y) {
            b = Some(match b {
                None => [x, y, x, y],
                Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
            });
        }
    }
    Ok(b)
}

/// The ferry counter's view of a feed (its `id`, `name_`, `url`, `source`, `ferry_routes`).
fn ferry_feed(f: &Py) -> ferrycount::Feed {
    let s = |k: &str| f.get(k).filter(|v| v.truthy()).map(Py::py_str);
    ferrycount::Feed {
        id: f.at("id").py_str(),
        name: s("name_").or_else(|| s("name")),
        url: f.at("url").py_str(),
        source: s("source"),
        ferry_routes: f.at("ferry_routes").list().map(|l| l.iter().map(Py::py_str).collect()).unwrap_or_default(),
    }
}

/// What the step gives the run.
pub struct Args<'a> {
    /// The accepted declarations (crate::inputs::feeds::of_accepted).
    pub declarations: &'a [serde_json::Value],
    /// The catalogue's pinned copy; none: fetched from its declaration's URL.
    pub catalogue: Option<PathBuf>,
    pub coverage: &'a Coverage,
    /// ISO 3166-1 codes.
    pub countries: &'a [String],
    /// The catalogue feeds checked so far (a JSON list), when there's a copy.
    pub checked: Option<PathBuf>,
    /// The NAS's zips: {id: {path, fetched}}.
    pub zips: &'a serde_json::Value,
    /// KEY=value lines (inputs/keys.env).
    pub keys: Option<PathBuf>,
    /// A folder: feeds.json, ferries.json, checked.json, gtfs/, unanswered.json, catalogue.csv.
    pub out: PathBuf,
}

/// Writes `path` by a temporary name, then renames it: a run cut short leaves the old file or the
/// new one, never part of one.
fn write_json(path: &Path, v: &Py) -> Result<()> {
    let tmp = path.with_file_name(format!("{}.tmp", path.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, pyjson::dumps_indent(v, 1))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Where a feed is: it has a zip, it's tried again later (the run fails), or it's left out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Zip,
    Trying,
    Out,
}

struct Run<'a> {
    net: Net,
    today: Date,
    out: PathBuf,
    by_id: HashMap<String, Py>,
    replacers: HashMap<String, Vec<String>>,
    exclude: Py,
    left_out: HashMap<String, String>,
    have: HashMap<String, String>,
    zips: &'a serde_json::Value,
    silent: BTreeMap<String, String>,
    waiting: Vec<String>,
    failed: Vec<String>,
    placed: HashMap<String, (State, Py)>,
}

fn pick(f: &Py, keys: &[&str]) -> Py {
    Py::Dict(keys.iter().filter_map(|&k| f.get(k).filter(|v| v.truthy()).map(|v| (k.to_string(), v.clone()))).collect())
}

fn with(mut d: Py, kv: &[(&str, Py)]) -> Py {
    for (k, v) in kv {
        d.set(k, v.clone());
    }
    d
}

impl Run<'_> {
    /// Feed `i` got no answer today: None while it's tried again (the run fails), else the first
    /// day it got none, NO_ANSWER_DAYS or more ago (it's left out).
    fn no_answer_since(&mut self, i: &str) -> Option<String> {
        let since = self.silent.entry(i.to_string()).or_insert_with(|| self.today.iso()).clone();
        let d = Date::parse_iso(&since).map_or(0, |d| (self.today.0 - d.0) as i64);
        (d >= NO_ANSWER_DAYS).then_some(since)
    }

    fn place(&mut self, f: &Py) -> (State, Py) {
        let i = f.at("id").py_str();
        if !self.placed.contains_key(&i) {
            // (Meanwhile, as one tried again: a cycle of replacements, or a failure, holds back
            // the feeds it replaces.)
            self.placed.insert(i.clone(), (State::Trying, Py::Dict(Vec::new())));
            match self.settle(f) {
                Ok(p) => {
                    self.placed.insert(i.clone(), p);
                }
                Err(e) => {
                    // Unforeseen: this feed fails the run, the others go on. (A keyed feed's
                    // error is named by its class alone, never its text.)
                    let keyed = f.get("key").is_some_and(Py::truthy);
                    self.failed.push(if keyed { format!("{i}: {}", e.0) } else { format!("{i}: {}: {}", e.0, e.1) });
                }
            }
        }
        self.placed[&i].clone()
    }

    fn settle(&mut self, f: &Py) -> std::result::Result<(State, Py), (String, String)> {
        let i = f.at("id").py_str();
        let rec = pick(f, &["id", "provider", "name", "country", "url", "licence", "replaces", "rail_routes"]);
        let st = |s: String| Py::Str(s);
        // Left out while a feed replacing it has a zip, or is tried again (not fetched for nothing).
        for g in self.replacers.get(&i).cloned().unwrap_or_default() {
            let by = self.by_id[&g].clone();
            if self.place(&by).0 != State::Out {
                return Ok((State::Out, with(rec, &[("status", st(format!("replaced by {g}")))])));
            }
        }
        if let Some(by) = self.exclude.get(&i) {
            return Ok((State::Out, with(rec, &[("status", st(format!("replaced by {}", by.py_str())))])));
        }
        if let Some(why) = self.left_out.get(&i) {
            eprintln!("{i}: left out ({why})");
            return Ok((State::Out, with(rec, &[("status", st(why.clone()))])));
        }
        let key = f.get("key").filter(|k| k.truthy()).and_then(|k| self.have.get(&k.py_str()).cloned());
        let dest = self.out.join("gtfs").join(format!("{i}.zip"));
        if let Some(c) = self.zips.get(&i) {
            let fetched = c["fetched"].as_str().unwrap_or("").to_string();
            let cached = with(rec.clone(), &[("zip", st("cache".into())), ("fetched", st(fetched.clone()))]);
            let day = Date::parse_iso(&fetched)
                .ok_or_else(|| ("ValueError".to_string(), format!("Invalid isoformat string: {fetched:?}")))?;
            let path = PathBuf::from(c["path"].as_str().unwrap_or(""));
            let fresh = {
                let _p = crate::timings::phase("checking the NAS's zips for service", crate::timings::Class::NasRead);
                let rail = if serves(f, "rail") { railcount::has_service(&path, day) } else { Ok(false) };
                let r = match rail {
                    Ok(true) => Ok(true),
                    Ok(false) if f.get("name_").is_some() && serves(f, "ferry") => {
                        ferrycount::has_service(&path, &ferry_feed(f), day)
                    }
                    other => other,
                };
                match r {
                    Ok(b) => b,
                    Err(e) if e.kind == Kind::Os => {
                        // Not read now (the NAS): the run fails, and is tried again, rather than
                        // fetching it.
                        self.failed.push(format!("{i}: its zip on the NAS can't be read now (OSError: {e})"));
                        return Ok((State::Trying, rec));
                    }
                    Err(e) if e.unreadable() => {
                        // Its tables can't be read (the rail counter can't count it either): as
                        // one out of date.
                        eprintln!("{i}: its zip can't be read ({}: {e})", e.class());
                        false
                    }
                    Err(e) => return Err((e.class().to_string(), e.msg)),
                }
            };
            let refresh = f.get("refresh_days").and_then(Py::number).unwrap_or(REFRESH_DAYS as f64);
            if fresh || ((self.today.0 - day.0) as f64) < refresh {
                return Ok((State::Zip, cached));
            }
            let got = {
                let _p = crate::timings::phase("downloading the feeds' zips", crate::timings::Class::Net);
                self.net.fetch(f, &dest, key.as_deref())
            };
            let (ok, why) = got.unwrap_or_else(|e| (false, format!("no answer ({e})")));
            if !ok {
                eprintln!("{i}: out of date in the cache, and {why} upstream; the cached copy stays");
                return Ok((State::Zip, cached));
            }
            // (A file the same as the cached one is kept on the NAS once; it then counts from today.)
            eprintln!("{i}: out of date in the cache; fetched again");
            return Ok((State::Zip, with(rec, &[("zip", st("new".into())), ("fetched", st(self.today.iso()))])));
        }
        let got = {
            let _p = crate::timings::phase("downloading the feeds' zips", crate::timings::Class::Net);
            self.net.fetch(f, &dest, key.as_deref())
        };
        let (ok, why) = match got {
            Ok(x) => x,
            Err(e) => {
                if let Some(since) = self.no_answer_since(&i) {
                    let status = format!("no answer since {since} (last tried {}): {e}", self.today.iso());
                    eprintln!("{i}: left out ({status})");
                    return Ok((State::Out, with(rec, &[("status", st(status))])));
                }
                self.waiting.push(format!("{i}: no answer ({e})"));
                return Ok((State::Trying, rec));
            }
        };
        self.silent.remove(&i);
        if ok {
            eprintln!("{i}: fetched");
            return Ok((State::Zip, with(rec, &[("zip", st("new".into())), ("fetched", st(self.today.iso()))])));
        }
        eprintln!("{i}: left out ({why})");
        Ok((State::Out, with(rec, &[("status", st(why))])))
    }

    fn zip_path(&self, i: &str, rec: &Py) -> PathBuf {
        if rec.at("zip").as_str() == Some("cache") {
            PathBuf::from(self.zips[i]["path"].as_str().unwrap_or(""))
        } else {
            self.out.join("gtfs").join(format!("{i}.zip"))
        }
    }

    /// Why a declared feed with a zip is left out of the rail list: its stops all beyond the
    /// coverage, or (its modes not said) no rail routes. None: it's in.
    fn beyond(&self, f: &Py, rec: &Py, cov: &Coverage) -> Result<Option<String>> {
        let _p = crate::timings::phase("reading the declared feeds' stops", crate::timings::Class::NasRead);
        let i = f.at("id").py_str();
        let path = self.zip_path(&i, rec);
        let r = (|| -> gtfs::R<Option<String>> {
            let b = stops_box(&path)?;
            if b.is_none_or(|b| !meets(cov, b)) {
                return Ok(Some("its stops are beyond the coverage".into()));
            }
            if !f.at("modes").truthy() && railcount::rail_routes(&gtfs::Zip::open(&path)?)?.is_empty() {
                return Ok(Some("no rail routes".into()));
            }
            Ok(None)
        })();
        match r {
            Ok(w) => Ok(w),
            Err(e) if e.unreadable() => {
                eprintln!("{i}: its stops can't be read ({}: {e})", e.class());
                Ok(None)
            }
            Err(e) => anyhow::bail!("{i}: its stops: {}: {e}", e.class()),
        }
    }
}

/// The run: what it checked and fetched is written in `out` even when it fails (Err).
pub fn run(a: Args<'_>) -> Result<()> {
    use crate::timings::{phase, Class};
    let out = a.out.clone();
    std::fs::create_dir_all(&out)?;
    let today = gtfs::today();
    let p = phase("reading the checks, keys and the NAS's list", Class::Disk);
    let countries: HashSet<String> = a.countries.iter().filter(|c| !c.is_empty()).cloned().collect();
    let decls: Vec<Py> = a.declarations.iter().map(pyjson::from_serde).collect();
    let cat = decls
        .iter()
        .find(|d| d.at("format").as_str() == Some("mdb-catalogue"))
        .cloned()
        .unwrap_or(Py::Dict(Vec::new()));
    let mut gtfs_decls: Vec<&Py> = decls.iter().filter(|d| d.at("format").as_str() == Some("gtfs")).collect();
    gtfs_decls.sort_by(|x, y| {
        let o = |d: &Py| d.get("order").and_then(Py::number).unwrap_or(0.0);
        o(x).total_cmp(&o(y)).then_with(|| x.at("id").py_str().cmp(&y.at("id").py_str()))
    });
    let exclude = cat.get("exclude").filter(|v| v.truthy()).cloned().unwrap_or(Py::Dict(Vec::new()));
    let cat_replaces = cat.get("replaces").filter(|v| v.truthy()).cloned().unwrap_or(Py::Dict(Vec::new()));
    let have = keys(a.keys.as_deref());
    let mut checked: Vec<(String, Py)> = Vec::new();
    if let Some(p) = a.checked.as_ref().filter(|p| p.exists()) {
        let v = pyjson::loads(&std::fs::read_to_string(p)?).context("the checks so far")?;
        for c in v.list().cloned().unwrap_or_default() {
            let id = c.at("id").py_str();
            match checked.iter_mut().find(|(k, _)| *k == id) {
                Some(e) => e.1 = c,
                None => checked.push((id, c)),
            }
        }
    }
    let mut checked: HashMap<String, Py> = checked.into_iter().collect();
    // The first day each feed's server gave no answer, kept from run to run until the step
    // completes.
    let silent_path = out.join("unanswered.json");
    let silent: BTreeMap<String, String> = if silent_path.exists() {
        let v = pyjson::loads(&std::fs::read_to_string(&silent_path)?)?;
        match v {
            Py::Dict(d) => d.into_iter().map(|(k, v)| (k, v.py_str())).collect(),
            _ => BTreeMap::new(),
        }
    } else {
        BTreeMap::new()
    };
    drop(p);
    let net = Net;
    let mut waiting: Vec<String> = Vec::new();
    // The catalogue: its pinned copy, else its declaration's URL fetched.
    let mut catalogue = a.catalogue.clone();
    if catalogue.is_none() && cat.at("url").truthy() {
        let _p = phase("fetching the catalogue", Class::Net);
        let url = cat.at("url").py_str();
        let dest = out.join("catalogue.csv");
        let r = net.ask(&url, None, false);
        if r.code != 200 {
            anyhow::bail!(
                "the catalogue, {url}: {}",
                if transient(r.code) { "no answer".to_string() } else { format!("http {}", r.code) }
            );
        }
        if !r.body.starts_with(b"id,") {
            anyhow::bail!("the catalogue, {url}: not its CSV");
        }
        std::fs::write(&dest, &r.body)?;
        catalogue = Some(dest);
    }
    // The catalogue's feeds here, each checked once (again while one gets no answer).
    let p = phase("listing the catalogue's feeds", Class::Compute);
    let country_of: HashMap<String, String> = match cat.get("country") {
        Some(Py::Dict(d)) => d.iter().map(|(k, v)| (k.clone(), v.py_str())).collect(),
        _ => HashMap::new(),
    };
    let cands = match &catalogue {
        Some(c) => catalogue_feeds(c, a.coverage, &countries, &country_of)?,
        None => Vec::new(),
    };
    let todo: Vec<Py> =
        cands.iter().filter(|c| checked.get(&c.at("id").py_str()).is_none_or(unanswered)).cloned().collect();
    drop(p);
    let mut sorted_countries: Vec<&String> = countries.iter().collect();
    sorted_countries.sort();
    eprintln!(
        "{} catalogue feeds where the coverage is ({}); {} to check",
        cands.len(),
        sorted_countries.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        todo.len()
    );
    let done: Vec<Py> = {
        let _p = phase("checking the catalogue's feeds", Class::Net);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let n_done = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<std::sync::Mutex<Option<Py>>> = todo.iter().map(|_| std::sync::Mutex::new(None)).collect();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if k >= todo.len() {
                        break;
                    }
                    *results[k].lock().unwrap() = Some(check(&net, todo[k].clone()));
                    let d = n_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    crate::agent::jobs::report(d as u64, todo.len() as u64, "feeds checked");
                });
            }
        });
        results.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
    };
    {
        let _p = phase("writing the lists", Class::Disk);
        for c in &done {
            checked.insert(c.at("id").py_str(), c.clone());
        }
        let mut all: Vec<(&String, &Py)> = checked.iter().collect();
        all.sort_by(|x, y| x.0.cmp(y.0));
        write_json(&out.join("checked.json"), &Py::List(all.into_iter().map(|(_, v)| v.clone()).collect()))?;
    }
    let mut run = Run {
        net,
        today,
        out: out.clone(),
        by_id: HashMap::new(),
        replacers: HashMap::new(),
        exclude,
        left_out: HashMap::new(),
        have,
        zips: a.zips,
        silent,
        waiting: Vec::new(),
        failed: Vec::new(),
        placed: HashMap::new(),
    };
    for c in &done {
        let id = c.at("id").py_str();
        if !unanswered(c) {
            run.silent.remove(&id);
        } else if let Some(since) = run.no_answer_since(&id) {
            run.left_out.insert(id, format!("its check: no answer since {since} (last tried {})", today.iso()));
        } else {
            waiting.push(format!("{id}: its check got {}", c.at("status").py_str()));
        }
    }
    let here = |d: &Py| -> bool {
        let terr = d.at("territory").list().cloned().unwrap_or_default();
        let key = d.at("fetch").get("key").filter(|k| k.truthy()).map(Py::py_str);
        terr.iter().any(|c| c.as_str().is_some_and(|c| countries.contains(c)))
            && key.is_none_or(|k| run.have.get(&k).is_some_and(|v| !v.is_empty()))
    };
    let mut listed: Vec<Py> = cands
        .iter()
        .map(|c| checked[&c.at("id").py_str()].clone())
        .filter(|c| c.at("rail_routes").truthy() || run.left_out.contains_key(&c.at("id").py_str()))
        .collect();
    listed.sort_by(|x, y| {
        (x.at("country").py_str(), x.at("provider").py_str())
            .cmp(&(y.at("country").py_str(), y.at("provider").py_str()))
    });
    let mut own: Vec<Py> = gtfs_decls.iter().filter(|d| here(d)).map(|d| declared(d, &countries)).collect();
    for f in own.iter_mut() {
        if let Some(r) = cat_replaces.get(&f.at("id").py_str()) {
            if serves(f, "rail") {
                f.set("replaces", r.clone());
            }
        }
    }
    // (The ferries are mapped worldwide: every ferry feed whose key is held, wherever it is.)
    let ferry_feeds: Vec<Py> = gtfs_decls
        .iter()
        .filter(|d| {
            let key = d.at("fetch").get("key").filter(|k| k.truthy()).map(Py::py_str);
            serves(d, "ferry") && key.is_none_or(|k| run.have.get(&k).is_some_and(|v| !v.is_empty()))
        })
        .map(|d| own.iter().find(|f| f.at("id") == d.at("id")).cloned().unwrap_or_else(|| declared(d, &countries)))
        .collect();
    let mut order: Vec<Py> = own.iter().filter(|f| serves(f, "rail")).cloned().collect();
    for f in listed {
        let mut f = f;
        if let Some(r) = cat_replaces.get(&f.at("id").py_str()) {
            f.set("replaces", r.clone());
        }
        order.push(f);
    }
    for f in &order {
        run.by_id.insert(f.at("id").py_str(), f.clone());
        for r in f.at("replaces").list().cloned().unwrap_or_default() {
            run.replacers.entry(r.py_str()).or_default().push(f.at("id").py_str());
        }
    }

    // Each feed's zip: the NAS's, else fetched; a stale one again once it's its `refresh` days old.
    let mine: HashSet<String> = own.iter().map(|f| f.at("id").py_str()).collect();
    let p = phase("placing the feeds' zips", Class::Mixed);
    let mut feeds = Vec::new();
    for (k, f) in order.iter().enumerate() {
        crate::agent::jobs::report(k as u64, order.len() as u64, "feeds' zips");
        let (state, mut rec) = run.place(f);
        if state == State::Zip && mine.contains(&f.at("id").py_str()) {
            if let Some(why) = run.beyond(f, &rec, a.coverage)? {
                rec.pop("zip");
                rec.pop("fetched");
                rec.set("status", Py::Str(why));
            }
        }
        if state != State::Trying {
            feeds.push(rec);
        }
    }
    crate::agent::jobs::report(order.len() as u64, order.len() as u64, "feeds' zips");
    // The ferry list: the declared feeds that serve ferries, wherever they are (zips already placed
    // for the rail list are named as the NAS's: they're uploaded with it).
    let new_in_rail: HashSet<String> =
        feeds.iter().filter(|r| r.at("zip").as_str() == Some("new")).map(|r| r.at("id").py_str()).collect();
    let mut ferries = Vec::new();
    for f in &ferry_feeds {
        let (state, rec) = run.place(f);
        if state == State::Trying {
            continue;
        }
        let mut fr = vec![
            ("id".to_string(), f.at("id").clone()),
            ("name".into(), f.at("name_").clone()),
            ("url".into(), f.at("url").clone()),
            ("source".into(), f.at("source").clone()),
            ("ferry_routes".into(), f.at("ferry_routes").clone()),
        ];
        if state == State::Zip {
            let zip = if new_in_rail.contains(&f.at("id").py_str()) {
                Py::Str("cache".into())
            } else {
                rec.at("zip").clone()
            };
            fr.push(("zip".into(), zip));
            fr.push(("fetched".into(), rec.at("fetched").clone()));
        } else {
            fr.push(("status".into(), rec.get("status").cloned().unwrap_or(Py::Str(String::new()))));
        }
        ferries.push(Py::Dict(fr));
    }
    drop(p);
    write_json(&silent_path, &Py::Dict(run.silent.iter().map(|(k, v)| (k.clone(), Py::Str(v.clone()))).collect()))?;
    let n_new = feeds.iter().filter(|f| f.at("zip").as_str() == Some("new")).count();
    let n_cache = feeds.iter().filter(|f| f.at("zip").as_str() == Some("cache")).count();
    eprintln!(
        "{} feeds: {n_cache} from the cache, {n_new} fetched, {} left out, {} to try again",
        order.len(),
        feeds.len() - n_cache - n_new,
        order.len() - feeds.len()
    );
    waiting.extend(std::mem::take(&mut run.waiting));
    if !waiting.is_empty() || !run.failed.is_empty() {
        // What worked is written (checked.json, gtfs/, unanswered.json) for the step to keep; the
        // lists aren't.
        let all: Vec<String> = run.failed.iter().chain(&waiting).cloned().collect();
        anyhow::bail!("{} feeds to try again: {}", all.len(), all.join("; "));
    }
    write_json(&out.join("feeds.json"), &Py::Dict(vec![("feeds".into(), Py::List(feeds))]))?;
    write_json(&out.join("ferries.json"), &Py::Dict(vec![("feeds".into(), Py::List(ferries))]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, d: f64) -> Vec<[i32; 2]> {
        let e = |v: f64| (v * 1e7).round() as i32;
        vec![[e(x), e(y)], [e(x + d), e(y)], [e(x + d), e(y + d)], [e(x), e(y + d)]]
    }

    /// The cases the Python's coverage was tested on (task #498), as crate::coverage
    /// answers them.
    #[test]
    fn boxes_meet_the_coverage_as_before() {
        let cov = |rings: Vec<Vec<[i32; 2]>>, buffer: f64| Coverage {
            shapes: vec![crate::coverage::Shape::new("t".into(), rings, buffer)],
            ..Default::default()
        };
        let c = cov(vec![square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)], 0.0);
        assert!(meets(&c, [1.0, 1.0, 2.0, 2.0]));
        assert!(!meets(&c, [4.5, 4.5, 5.5, 5.5])); // in the hole, no edge met
        assert!(meets(&c, [3.0, 3.0, 4.5, 4.5])); // the hole's edge met
        assert!(!meets(&c, [11.0, 11.0, 12.0, 12.0]));
        assert!(meets(&c, [-5.0, -5.0, 20.0, 20.0])); // the box around it all
        let km = 1.0 / 111.32;
        let c = cov(vec![square(10.0, 45.0, 0.1)], 1000.0);
        assert!(meets(&c, [10.1 + 0.5 * km, 45.0, 10.2, 45.05]));
        assert!(!meets(&c, [10.1 + 2.0 * km, 45.0, 10.2, 45.05]));
        // (The catalogue has boxes past ±90°: with no margin, not every longitude.)
        let c = cov(vec![square(3.0, 49.0, 0.5)], 0.0);
        assert!(meets(&c, [3.0, 48.9, 4.3, 2457338.0]));
        assert!(!meets(&c, [120.0, 48.9, 121.0, 2457338.0]));
    }

    #[test]
    fn curls_answer_read_as_before() {
        let a = answer(
            b"HTTP/1.1 302 Found\r\nLocation: x\r\n\r\nHTTP/2 206 \r\nContent-Range: bytes 0-3/10\r\ncontent-length: 4\r\n\r\nabcd"
                .to_vec(),
        );
        assert_eq!((a.code, a.body.as_slice()), (206, &b"abcd"[..]));
        assert_eq!(a.headers["content-range"], "bytes 0-3/10");
        assert_eq!(length(&a.headers), Some(4));
        assert_eq!(answer(b"HTTP/1.1 200 OK\r\nContent-Le".to_vec()).code, 0);
        let s = |x: &str| Py::Dict(vec![("status".into(), Py::Str(x.into()))]);
        assert!(unanswered(&s("no answer (none)")) && unanswered(&s("http 503")) && unanswered(&s("http 429")));
        assert!(!unanswered(&s("http 404")) && !unanswered(&s("ok")) && !unanswered(&s("http x")));
    }

    #[test]
    fn a_json_link_is_found_as_before() {
        let v = pyjson::loads(r#"{"value": "[{'link': 'https://x/y.zip'}]"}"#).unwrap();
        assert_eq!(json_at(&v, "value[0].link"), Ok(Some(Py::Str("https://x/y.zip".into()))));
        let v = pyjson::loads(r#"{"value": []}"#).unwrap();
        assert_eq!(json_at(&v, "value[0].link"), Ok(None));
        assert_eq!(json_at(&v, "other"), Err(()));
    }
}
