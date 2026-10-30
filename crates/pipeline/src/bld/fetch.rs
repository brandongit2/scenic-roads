//! `bld-fetch` (docs/buildings3d.md §2): the 3D buildings' sources onto the NAS, each file whole, as
//! the source has it, each fetched once:
//!
//! - Overture's building and building-part files of the declared release (the `buildings` source's
//!   `params.types`: GeoParquet on Overture's public S3 bucket, read anonymously over HTTPS) that
//!   hold a building within `MARGIN_KM` of the coverage: a file is taken when one of its row
//!   groups' boxes (its footer's bbox statistics) comes within the margin of the coverage. Into
//!   `sources/overture/<release>/theme=buildings/type=<type>/<file>` (the release's dot a dash),
//!   with `buildings.json` beside them (the files here, each with its size, S3 ETag, rows and row
//!   groups' boxes, and the coverage they were chosen for) and `footers.json.gz` (every file of the
//!   release's row-group boxes, read once: a release never changes).
//! - GHSL's building-height tiles (the `building-heights` source's index: JRC's 10° zips) within
//!   the margin of the coverage, into `sources/ghsl/<version>/<file>.zip`, with `index.json`.
//!
//! Adding a region and running again fetches only the files the new coverage needs.
//!
//! Each file is downloaded to `<file>.<host>.tmp` beside its place, resumed from where a run cut
//! short left it (an HTTP range, and for S3 only while the object is the one listed: If-Match its
//! ETag), in requests of `PIECE` bytes at most, flushed, checked, then renamed into place: an
//! Overture file's length and its ETag (the MD5 of its parts as S3 computes a multipart ETag,
//! hashed as it comes in; what a resumed file had is read back from the NAS); a GHSL zip's length
//! and every member's CRC-32 (crate::bld::ghsl::zip_bad). A file already in place at its listed
//! size is skipped: only checked files are put there. A file that fails its check is fetched again
//! once from the start, then fails the run.
//!
//! `TRANSFERS` at once; no speed cap. A request that gets no answer (no connection, a timeout, a
//! 429 or a 5xx, an answer cut short) is tried again with a growing wait (Retry-After honoured; a
//! 429 or a Retry-After told to the job's proxy, crate::budget::seen); a refusal (403, 404) fails
//! that file. When the NAS goes away, the run waits for it (up to `NAS_WAIT_H` hours), then goes
//! on from the temporary files. One run at a time per Mac (a lock in the temporary folder). Every
//! request says who asks (crate::fetch::USER_AGENT) and asks crate::fetch::online first; these are
//! published data and an API's listings (crate::fetch::Kind::Data, Api): no robots.txt.
//!
//! The indexes are written as Python's `json` wrote them (`json.dumps(…, indent=1, sort_keys=True)`,
//! `footers.json.gz` on one line, gzip'd at level 9 with no time), so files the Python made read on.

use super::ghsl::zip_bad;
use crate::coverage::Coverage;
use crate::pyjson::{self, Py};
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use store::nasfs::NasPath;

/// How near the coverage a file's row group (or a GHSL tile) must come to be fetched (km).
pub const MARGIN_KM: f64 = 20.0;
/// Transfers at once.
const TRANSFERS: usize = 3;
/// How long the run waits for the NAS to come back.
const NAS_WAIT_H: f64 = 6.0;
/// The most a download's request asks for: a request that stalls is cut by its time (`PIECE_S`)
/// and the next goes on from the bytes it got.
const PIECE: u64 = 16 << 20;
const PIECE_S: u64 = 180;
/// Part sizes S3 uploaders use, for a multipart ETag ("<md5 of the parts' md5s>-<parts>").
const PART_SIZES: [u64; 9] =
    [5 << 20, 8 << 20, 16 << 20, 32 << 20, 64 << 20, 128 << 20, 256 << 20, 512 << 20, 100_000_000];

fn gb(n: f64) -> String {
    format!("{:.2} GB", n / 1e9)
}

// ---- HTTP ------------------------------------------------------------------------------------------

/// Why a request failed.
#[derive(Debug)]
enum Fail {
    /// A definite answer: the file isn't there for us (403, 404, 410), changed (412), or 416.
    Refused(u16),
    /// Worth asking again (no answer, a timeout, 408, 429, a 5xx, an answer cut short), after the
    /// server's Retry-After when it gave one.
    Again(String, Option<f64>),
    /// Any other answer.
    Other(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Fail::Refused(c) => write!(f, "HTTP {c}"),
            Fail::Again(w, _) | Fail::Other(w) => f.write_str(w),
        }
    }
}

/// How long to wait before asking again after the k-th failure in a row: the server's Retry-After
/// (at most an hour), else 5 s doubling to 10 min.
fn wait_s(retry_after: Option<f64>, k: u32) -> f64 {
    retry_after.map_or_else(|| (5.0 * 2f64.powi(k.min(7) as i32)).min(600.0), |r| r.min(3600.0))
}

/// Requests to the sources' hosts (Overture's bucket, JRC's tiles).
pub struct Http {
    agent: ureq::Agent,
}

/// A response's status, sorted.
fn status(url: &str, r: &ureq::http::Response<ureq::Body>) -> Result<(), Fail> {
    let code = r.status().as_u16();
    let ra = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    crate::budget::seen(url, code, ra.as_deref());
    match code {
        200..=299 => Ok(()),
        403 | 404 | 410 | 412 | 416 => Err(Fail::Refused(code)),
        408 | 429 | 500 | 502 | 503 | 504 => Err(Fail::Again(
            format!("HTTP {code}"),
            ra.filter(|v| !v.trim().is_empty() && v.trim().bytes().all(|b| b.is_ascii_digit()))
                .and_then(|v| v.trim().parse().ok()),
        )),
        c => Err(Fail::Other(format!("HTTP {c}"))),
    }
}

impl Http {
    pub fn new() -> Http {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .user_agent(crate::fetch::USER_AGENT)
            .accept_encoding("identity")
            .http_status_as_error(false)
            .build()
            .into();
        Http { agent }
    }

    /// A GET (or HEAD) of `url` with `headers`, its body held to `body_s` seconds.
    fn call(
        &self,
        url: &str,
        head: bool,
        headers: &[(&str, String)],
        body_s: u64,
    ) -> Result<ureq::http::Response<ureq::Body>, Fail> {
        crate::fetch::online(url).map_err(|e| Fail::Other(format!("{e:#}")))?;
        let r = if head {
            let mut q = self.agent.head(url);
            for (k, v) in headers {
                q = q.header(*k, v);
            }
            q.call()
        } else {
            let mut q = self.agent.get(url).config().timeout_recv_body(Some(Duration::from_secs(body_s))).build();
            for (k, v) in headers {
                q = q.header(*k, v);
            }
            q.call()
        };
        let r = r.map_err(|e| Fail::Again(e.to_string(), None))?;
        status(url, &r)?;
        Ok(r)
    }

    /// `f`, asked again after the failures that may pass, 10 tries in all.
    fn retried<T>(&self, what: &str, mut f: impl FnMut() -> Result<T, Fail>) -> Result<T> {
        let mut k = 0;
        loop {
            match f() {
                Ok(v) => return Ok(v),
                Err(Fail::Again(why, ra)) if k < 9 => {
                    let w = wait_s(ra, k);
                    eprintln!("bld-fetch: {what}: {why}; asking again in {w:.0} s");
                    std::thread::sleep(Duration::from_secs_f64(w));
                    k += 1;
                }
                Err(e) => bail!("{what}: {e}"),
            }
        }
    }

    /// `url`'s bytes (`rng`: first and last), whole.
    pub fn get_bytes(&self, url: &str, rng: Option<(u64, u64)>) -> Result<Vec<u8>> {
        let what = url.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(url);
        let what: String = what.chars().take(60).collect();
        self.retried(&what, || {
            let h: Vec<(&str, String)> = rng.map(|(s, e)| ("Range", format!("bytes={s}-{e}"))).into_iter().collect();
            let mut r = self.call(url, false, &h, 120)?;
            let d = r
                .body_mut()
                .with_config()
                .limit(1 << 30)
                .read_to_vec()
                .map_err(|e| Fail::Again(e.to_string(), None))?;
            if let Some((s, e)) = rng {
                if d.len() as u64 != e - s + 1 {
                    return Err(Fail::Again(format!("{} of {} bytes", d.len(), e - s + 1), None));
                }
            }
            Ok(d)
        })
    }
}

// ---- the coverage ----------------------------------------------------------------------------------

/// Whether a box comes within the margin of the coverage: a distance query on the coverage's
/// polygons as they are (GEOS, an STR tree), never the coverage grown. Growing it (a buffer of every
/// region, then their union) took GEOS past 60 GB on the regions' 244 MB outlines (126 regions, one
/// of 98 k polygons: each polygon's 20 km ring overlapping thousands of others), task #493.
pub struct Near {
    geoms: Vec<geos::Geometry>,
    tree: geos::STRtree<usize>,
    margin_km: f64,
}

/// shapely's `box(w, s, e, n)`: its ring from the south-east corner, anticlockwise.
fn rect(w: f64, s: f64, e: f64, n: f64) -> Result<geos::Geometry> {
    let ring = geos::CoordSeq::new_from_vec(&[[e, s], [e, n], [w, n], [w, s], [e, s]])?;
    Ok(geos::Geometry::create_polygon(geos::Geometry::create_linear_ring(ring)?, Vec::new())?)
}

impl Near {
    /// Every ring of the coverage's outlines as a polygon of its own, in degrees (the outlines'
    /// even–odd rings, their holes too: near one is near the outline). An invalid polygon is made
    /// valid first, every part kept: the lobes, and the lines and points of a zero-area outline,
    /// which still answer distances (GEOS leaves its predicates on invalid input undefined).
    pub fn new(cov: &Coverage, margin_km: f64) -> Result<Near> {
        use geos::{Geom, SpatialIndex};
        let mut geoms = Vec::new();
        for s in &cov.shapes {
            for r in s.rings.iter() {
                let mut pts: Vec<[f64; 2]> = r.iter().map(|p| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7]).collect();
                if pts.first() != pts.last() {
                    pts.push(pts[0]);
                }
                let ring = geos::Geometry::create_linear_ring(geos::CoordSeq::new_from_vec(&pts)?)?;
                let g = geos::Geometry::create_polygon(ring, Vec::new())?;
                if g.is_empty()? {
                    continue;
                }
                let g = if g.is_valid()? { g } else { g.make_valid()? };
                let multi = matches!(
                    g.geometry_type()?,
                    geos::GeometryTypes::MultiPolygon
                        | geos::GeometryTypes::GeometryCollection
                        | geos::GeometryTypes::MultiLineString
                        | geos::GeometryTypes::MultiPoint
                );
                if !multi {
                    if !g.is_empty()? {
                        geoms.push(g);
                    }
                    continue;
                }
                for k in 0..g.get_num_geometries()? {
                    let x = geos::Geom::clone(&g.get_geometry_n(k)?)?;
                    if !x.is_empty()? {
                        geoms.push(x);
                    }
                }
            }
        }
        ensure!(!geoms.is_empty(), "no coverage");
        let mut tree = geos::STRtree::with_capacity(10)?;
        for (i, g) in geoms.iter().enumerate() {
            tree.insert(g, i);
        }
        tree.build()?;
        Ok(Near { geoms, tree, margin_km })
    }

    /// Whether box (w, s, e, n) comes within the margin. Degrees of longitude shrink with
    /// latitude: the margin in degrees at the highest latitude within the margin of the box, so
    /// it's at least the margin everywhere near it. Near a pole (that latitude's circle shorter than
    /// the margin's reach) every longitude is near.
    pub fn meets(&self, w: f64, s: f64, e: f64, n: f64) -> Result<bool> {
        use geos::{Geom, SpatialIndex};
        let dlat = self.margin_km / 111.32;
        let lat = s.abs().max(n.abs()) + dlat;
        let c = lat.min(90.0).to_radians().cos();
        let dlon = if c > 1e-9 { self.margin_km / (111.32 * c) } else { f64::INFINITY };
        if dlon >= 180.0 {
            // (Any outline whose envelope meets the band, as the tree answers it.)
            let mut hit = false;
            self.tree.query(&rect(-180.0, s - dlat, 180.0, n + dlat)?, |_| hit = true);
            return Ok(hit);
        }
        // (No wrap in the coordinates: the box also a turn east or west when its margin crosses
        // ±180°, so a polygon at 179.9° meets a box at -179.9°.)
        let mut ks = vec![0.0];
        if w - dlon < -180.0 {
            ks.push(360.0);
        }
        if e + dlon > 180.0 {
            ks.push(-360.0);
        }
        for k in ks {
            let b = rect(w + k, s, e + k, n)?;
            let grown = rect(w + k - dlon, s - dlon, e + k + dlon, n + dlon)?;
            let mut cand = Vec::new();
            self.tree.query(&grown, |&i| cand.push(i));
            for i in cand {
                if b.dwithin(&self.geoms[i], dlon)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

// ---- the indexes, as Python's json wrote them -------------------------------------------------------

/// `v` with every object's keys sorted (`sort_keys=True`).
fn sorted(v: Py) -> Py {
    match v {
        Py::Dict(mut d) => {
            d.sort_by(|a, b| a.0.cmp(&b.0));
            Py::Dict(d.into_iter().map(|(k, x)| (k, sorted(x))).collect())
        }
        Py::List(l) => Py::List(l.into_iter().map(sorted).collect()),
        x => x,
    }
}

/// JSON text with every character past ASCII escaped (`ensure_ascii`: `\uXXXX`, surrogate pairs).
fn ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut u = [0u16; 2];
            for x in c.encode_utf16(&mut u) {
                out.push_str(&format!("\\u{x:04x}"));
            }
        }
    }
    out
}

/// `gzip.compress(data, 9, mtime=0)`: zlib's deflate at level 9 (its zlib stream's body), in a gzip
/// header with no time, level 9's flag and OS 255, as Python writes it.
fn py_gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut z = vec![0u8; data.len() + data.len() / 1000 + 64];
    let mut n = z.len() as libz_sys::uLongf;
    // SAFETY: zlib's one-shot compress over buffers that outlive the call, `n` holding `z`'s length.
    let r = unsafe { libz_sys::compress2(z.as_mut_ptr(), &mut n, data.as_ptr(), data.len() as libz_sys::uLong, 9) };
    ensure!(r == libz_sys::Z_OK && n >= 6, "zlib: {r}");
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 255];
    out.extend_from_slice(&z[2..n as usize - 4]);
    out.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    Ok(out)
}

/// An index written whole: a temporary name, flushed, renamed; `gz`: on one line, gzip'd, else an
/// item a line.
fn write_json(path: &NasPath, v: Py, gz: bool) -> Result<()> {
    let v = sorted(v);
    let text = ascii(&if gz { pyjson::dumps_spaced(&v) } else { pyjson::dumps_indent(&v, 1) });
    let bytes = if gz { py_gzip(text.as_bytes())? } else { text.into_bytes() };
    let name = path.file_name().context("an index's name")?;
    let tmp = path.with_file_name(format!("{name}.{}.{}.tmp", store::sys::host(), std::process::id()));
    let mut f = tmp.create().with_context(|| format!("write {}", tmp.display()))?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);
    tmp.rename(path)?;
    Ok(())
}

/// A JSON file as Python read it, or None when it isn't there or doesn't read.
fn read_json(path: &NasPath) -> Option<Py> {
    let b = path.read().ok()?;
    pyjson::loads(std::str::from_utf8(&b).ok()?).ok()
}

fn float_list(v: &[f64]) -> Py {
    Py::List(v.iter().map(|&x| Py::Float(x)).collect())
}

fn int(n: impl std::fmt::Display) -> Py {
    Py::Int(n.to_string())
}

fn dict(kv: Vec<(&str, Py)>) -> Py {
    Py::Dict(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// Now as `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
fn now_iso() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00", rem / 3600, rem / 60 % 60, rem % 60)
}

// ---- Overture --------------------------------------------------------------------------------------

/// An object of the bucket's listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub key: String,
    pub size: u64,
    pub etag: String,
}

/// XML's escapes undone.
fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// The text of each `<tag>…</tag>` in `x`, in order.
fn texts<'a>(x: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = x[at..].find(&open) {
        let s = at + i + open.len();
        let Some(j) = x[s..].find(&close) else { break };
        out.push(&x[s..s + j]);
        at = s + j + close.len();
    }
    out
}

/// A query's value, percent-encoded as `urllib.parse.urlencode` encodes it.
fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The `.parquet` objects under `prefix` in the bucket at `bucket` (S3's ListObjectsV2, page by
/// page), in S3's order (by key).
pub fn s3_list(http: &Http, bucket: &str, prefix: &str) -> Result<Vec<Object>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut q = format!("list-type=2&prefix={}", quote(prefix));
        if let Some(t) = &token {
            q.push_str(&format!("&continuation-token={}", quote(t)));
        }
        let x = String::from_utf8(http.get_bytes(&format!("{bucket}?{q}"), None)?).context("S3's listing")?;
        ensure!(x.contains("<ListBucketResult"), "S3's listing: not a ListBucketResult");
        for c in texts(&x, "Contents") {
            let one = |t: &str| texts(c, t).first().map(|v| unescape(v));
            let key = one("Key").context("S3's listing: a key")?;
            if key.ends_with(".parquet") {
                let size = one("Size").and_then(|v| v.parse().ok()).context("S3's listing: a size")?;
                let etag = one("ETag").context("S3's listing: an ETag")?.trim_matches('"').to_string();
                out.push(Object { key, size, etag });
            }
        }
        if texts(&x, "IsTruncated").first() != Some(&"true") {
            return Ok(out);
        }
        token = texts(&x, "NextContinuationToken").first().map(|v| unescape(v));
        ensure!(token.is_some(), "S3's listing: truncated without a continuation token");
    }
}

/// A parquet file's rows and its row groups' boxes `[w, s, e, n, rows]`, from its footer (its last
/// 64 KiB read, then the whole footer when that's longer).
fn row_groups(http: &Http, url: &str, size: u64) -> Result<(i64, Vec<([f64; 4], i64)>)> {
    let mut tail = http.get_bytes(url, Some((size.saturating_sub(65536), size - 1)))?;
    if let Some(need) = super::overture::footer_need(&tail).with_context(|| url.to_string())? {
        tail = http.get_bytes(url, Some((size - need, size - 1)))?;
    }
    let md = super::overture::footer_meta(&tail).with_context(|| format!("{url}: its footer"))?;
    super::overture::row_group_boxes(&md)
}

/// A file to fetch.
#[derive(Clone, Debug)]
struct Job {
    ghsl: bool,
    url: String,
    dest: NasPath,
    size: u64,
    etag: Option<String>,
    name: String,
    rows: i64,
    row_groups: usize,
    rows_near: i64,
    bbox: [f64; 4],
}

/// The footers' cache: every file of the release by key, `{etag, size, rows, rgs}`.
fn footers_of(p: Py) -> BTreeMap<String, Py> {
    match p {
        Py::Dict(d) => d.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

fn num(p: &Py) -> f64 {
    p.number().unwrap_or(f64::NAN)
}

/// The release's files meeting the coverage, its folder, and the footers' cache.
fn overture_jobs(
    http: &Http,
    root: &store::nasfs::NasRoot,
    release: &str,
    bucket: &str,
    types: &[String],
    near: &Near,
) -> Result<(Vec<Job>, NasPath)> {
    use crate::timings::{phase, Class};
    let base = root.join(super::sources::release_dir(release));
    base.create_dir_all()?;
    // Every file's row groups (kept: a release's files never change).
    let p = phase("reading the footers' kept copy", Class::NasRead);
    let foot_path = base.join("footers.json.gz");
    let mut footers: BTreeMap<String, Py> = BTreeMap::new();
    if foot_path.exists() {
        let read = foot_path.read().map_err(anyhow::Error::from).and_then(|gz| {
            let mut s = String::new();
            flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut s)?;
            pyjson::loads(&s)
        });
        match read {
            Ok(v) => footers = footers_of(v),
            Err(e) => eprintln!("bld-fetch: {}: unreadable ({e:#}); reading the footers again", foot_path.display()),
        }
    }
    drop(p);
    let p = phase("listing the release's files on S3", Class::Net);
    let mut listed = Vec::new();
    for t in types {
        let prefix = format!("release/{release}/theme=buildings/type={t}/");
        let mut files = s3_list(http, bucket, &prefix)?;
        if files.is_empty() && !footers.is_empty() {
            // The release gone from S3 (Overture keeps about two months): the footers read before
            // list its files, so the coverage's files here are known whole and skipped; one the
            // coverage needs and that isn't here fails (it waits for the next pinned release).
            files = footers
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, v)| Object {
                    key: k.clone(),
                    size: num(v.at("size")) as u64,
                    etag: v.at("etag").as_str().unwrap_or("").to_string(),
                })
                .collect();
            eprintln!(
                "bld-fetch: overture {release}: no {t} files on S3 now; going by the {} footers read before",
                files.len()
            );
        }
        ensure!(!files.is_empty(), "Overture's release {release} has no {t} files on S3 (gone, or another name?)");
        listed.extend(files);
    }
    eprintln!(
        "bld-fetch: overture {release}: {} files listed, {}",
        listed.len(),
        gb(listed.iter().map(|f| f.size as f64).sum())
    );
    drop(p);
    let todo: Vec<&Object> = listed
        .iter()
        .filter(|f| footers.get(&f.key).and_then(|c| c.get("etag")).and_then(Py::as_str) != Some(f.etag.as_str()))
        .collect();
    if !todo.is_empty() {
        eprintln!("bld-fetch: reading {} files' footers", todo.len());
        let p = phase("reading the files' footers from S3", Class::Net);
        let done = AtomicU64::new(0);
        let read: Vec<Result<(String, Py)>> = {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(TRANSFERS + 1).build()?;
            pool.install(|| {
                use rayon::prelude::*;
                todo.par_iter()
                    .map(|f| {
                        let (rows, rgs) = row_groups(http, &format!("{bucket}{}", f.key), f.size)?;
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n.is_multiple_of(50) {
                            eprintln!("bld-fetch: footers: {n}/{}", todo.len());
                        }
                        let rgs = rgs
                            .iter()
                            .map(|(b, r)| {
                                let mut v: Vec<Py> = b.iter().map(|&x| Py::Float(x)).collect();
                                v.push(int(r));
                                Py::List(v)
                            })
                            .collect();
                        let entry = dict(vec![
                            ("etag", Py::Str(f.etag.clone())),
                            ("size", int(f.size)),
                            ("rows", int(rows)),
                            ("rgs", Py::List(rgs)),
                        ]);
                        Ok((f.key.clone(), entry))
                    })
                    .collect()
            })
        };
        for r in read {
            let (k, v) = r?;
            footers.insert(k, v);
        }
        drop(p);
        let _p = phase("writing the footers' kept copy", Class::NasWrite);
        write_json(&foot_path, Py::Dict(footers.clone().into_iter().collect()), true)?;
    }
    let _p = phase("picking the files meeting the coverage", Class::Compute);
    let mut jobs = Vec::new();
    for f in &listed {
        let c = &footers[&f.key];
        let rgs: Vec<[f64; 5]> = c
            .at("rgs")
            .list()
            .map(|l| {
                l.iter()
                    .filter_map(|g| {
                        let g = g.list()?;
                        (g.len() >= 5).then(|| [num(&g[0]), num(&g[1]), num(&g[2]), num(&g[3]), num(&g[4])])
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut hit = Vec::new();
        for g in &rgs {
            if near.meets(g[0], g[1], g[2], g[3])? {
                hit.push(*g);
            }
        }
        if hit.is_empty() {
            continue;
        }
        let name = f.key.split_once(&format!("release/{release}/")).map_or(f.key.as_str(), |x| x.1).to_string();
        let fold = |i: usize, min: bool| {
            rgs.iter().map(|g| g[i]).fold(if min { f64::INFINITY } else { f64::NEG_INFINITY }, |a, b| {
                if min {
                    a.min(b)
                } else {
                    a.max(b)
                }
            })
        };
        jobs.push(Job {
            ghsl: false,
            url: format!("{bucket}{}", f.key),
            dest: base.join(&name),
            size: f.size,
            etag: Some(f.etag.clone()),
            name,
            rows: num(c.at("rows")) as i64,
            row_groups: rgs.len(),
            rows_near: hit.iter().map(|g| g[4] as i64).sum(),
            bbox: [fold(0, true), fold(1, true), fold(2, false), fold(3, false)],
        });
    }
    Ok((jobs, base))
}

// ---- GHSL ------------------------------------------------------------------------------------------

/// GHSL's tiles meeting the coverage (the declared index's, by URL), each one's size asked (the
/// listing rounds them, so one already here is known whole).
fn ghsl_jobs(
    http: &Http,
    root: &store::nasfs::NasRoot,
    src: &crate::inputs::sources::Source,
    near: &Near,
) -> Result<(Vec<Job>, NasPath)> {
    use crate::timings::{phase, Class};
    let base = root.join(super::sources::ghsl_dir(src.version()?));
    base.create_dir_all()?;
    ensure!(!src.index.is_empty(), "GHSL: its declaration's index lists no tiles");
    let p = phase("picking the files meeting the coverage", Class::Compute);
    let mut index: Vec<&crate::inputs::sources::Feature> = src.index.iter().collect();
    index.sort_by(|a, b| a.url.cmp(&b.url));
    let mut jobs = Vec::new();
    for f in index {
        let name = f.url.rsplit('/').next().unwrap_or("").to_string();
        let b = f.bbox;
        if near.meets(b[0], b[1], b[2], b[3])? {
            jobs.push(Job {
                ghsl: true,
                url: f.url.clone(),
                dest: base.join(&name),
                size: 0,
                etag: None,
                name,
                rows: 0,
                row_groups: 0,
                rows_near: 0,
                bbox: b,
            });
        }
    }
    drop(p);
    let _p = phase("listing the GHSL tiles", Class::Net);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(TRANSFERS).build()?;
    let sizes: Vec<Result<u64>> = pool.install(|| {
        use rayon::prelude::*;
        jobs.par_iter()
            .map(|j| {
                http.retried(&j.name, || {
                    let r = http.call(&j.url, true, &[], 60)?;
                    r.headers()
                        .get("content-length")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| Fail::Other("no Content-Length".into()))
                })
            })
            .collect()
    });
    for (j, s) in jobs.iter_mut().zip(sizes) {
        j.size = s?;
    }
    Ok((jobs, base))
}

// ---- downloading -----------------------------------------------------------------------------------

/// The run's progress: files and bytes.
struct Progress {
    files: usize,
    total: u64,
    done_files: AtomicU64,
    done: AtomicU64,
    t0: Instant,
    window: Mutex<Vec<(Instant, u64)>>,
}

impl Progress {
    fn add(&self, n: i64) {
        if n >= 0 {
            self.done.fetch_add(n as u64, Ordering::Relaxed);
        } else {
            self.done.fetch_sub(n.unsigned_abs(), Ordering::Relaxed);
        }
    }

    fn file_done(&self) {
        let d = self.done_files.fetch_add(1, Ordering::Relaxed) + 1;
        crate::agent::jobs::report(d, self.files as u64, "files");
    }

    fn line(&self) -> String {
        let now = Instant::now();
        let done = self.done.load(Ordering::Relaxed);
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        w.push((now, done));
        while w.len() > 2 && now.duration_since(w[0].0).as_secs_f64() > 300.0 {
            w.remove(0);
        }
        let (t, d) = w[0];
        let span = now.duration_since(t).as_secs_f64().max(1e-9);
        let rate = done.saturating_sub(d) as f64 / span;
        let eta = if rate > 0.0 {
            format!("~{:.0} min left", self.total.saturating_sub(done) as f64 / rate / 60.0)
        } else {
            "time left unknown".into()
        };
        format!(
            "progress: {}/{} files, {} of {} ({:.1} MB/s over the last {:.0} s, {eta})",
            self.done_files.load(Ordering::Relaxed),
            self.files,
            gb(done as f64),
            gb(self.total as f64),
            rate / 1e6,
            span.min(300.0).min(now.duration_since(self.t0).as_secs_f64())
        )
    }
}

/// The S3 ETag of a file as it comes in: its MD5, or for a multipart upload ("…-N") the MD5 of its
/// parts' MD5s, for each part size that makes N parts of the file's size.
struct PartHasher {
    etag: String,
    state: Vec<(u64, md5::Md5, u64, Vec<[u8; 16]>)>,
}

impl PartHasher {
    fn new(size: u64, etag: &str) -> PartHasher {
        use md5::Digest;
        let sizes: Vec<u64> = match etag.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()) {
            Some(n) if n > 0 => {
                let fit: Vec<u64> = PART_SIZES.iter().copied().filter(|&p| size.div_ceil(p) == n).collect();
                if fit.is_empty() {
                    vec![size.div_ceil(n)]
                } else {
                    fit
                }
            }
            _ => vec![0],
        };
        PartHasher {
            etag: etag.to_string(),
            state: sizes.into_iter().map(|p| (p, md5::Md5::new(), 0, Vec::new())).collect(),
        }
    }

    fn update(&mut self, b: &[u8]) {
        use md5::Digest;
        for (p, h, inn, parts) in &mut self.state {
            if *p == 0 {
                h.update(b);
                continue;
            }
            let mut b = b;
            while !b.is_empty() {
                let k = (b.len() as u64).min(*p - *inn) as usize;
                h.update(&b[..k]);
                *inn += k as u64;
                b = &b[k..];
                if *inn == *p {
                    parts.push(std::mem::take(h).finalize().into());
                    *inn = 0;
                }
            }
        }
    }

    fn ok(self) -> bool {
        use md5::Digest;
        let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        self.state.into_iter().any(|(p, h, inn, mut parts)| {
            if p == 0 {
                return hex(&h.finalize()) == self.etag;
            }
            if inn > 0 {
                parts.push(h.finalize().into());
            }
            let all: Vec<u8> = parts.concat();
            format!("{}-{}", hex(&md5::Md5::digest(&all)), parts.len()) == self.etag
        })
    }
}

/// Waits for the NAS's folder `root` (up to `NAS_WAIT_H`).
fn wait_for_nas(root: &store::nasfs::NasRoot) -> Result<()> {
    let t0 = Instant::now();
    while !root.path().is_dir() {
        ensure!(
            t0.elapsed().as_secs_f64() <= NAS_WAIT_H * 3600.0,
            "the NAS ({}) has been away {NAS_WAIT_H} h",
            root.display()
        );
        eprintln!("bld-fetch: the NAS ({}) isn't there; waiting", root.display());
        std::thread::sleep(Duration::from_secs(60));
    }
    Ok(())
}

/// How a download's attempt ended.
enum Attempt {
    /// The file whole in its temporary file.
    Whole,
    /// A failure worth going on from (after a wait).
    Again(String, Option<f64>),
    /// A definite one.
    Fatal(anyhow::Error),
    /// The NAS's (a file operation).
    Nas(std::io::Error),
}

/// One file into place (see the module's docs): whether it was fetched (else it was here), and
/// the bytes this run fetched of it (the rest were here from a run cut short).
fn fetch(http: &Http, job: &Job, root: &store::nasfs::NasRoot, prog: &Progress) -> Result<(bool, u64)> {
    let size = job.size;
    if job.dest.metadata().is_ok_and(|m| m.len() == size) {
        prog.add(size as i64);
        prog.file_done();
        return Ok((false, 0));
    }
    job.dest.parent().context("a file's folder")?.create_dir_all()?;
    let name = job.dest.file_name().context("a file's name")?;
    let tmp = job.dest.with_file_name(format!("{name}.{}.tmp", store::sys::host()));
    let (mut fresh_starts, mut k) = (0, 0u32);
    let mut kept: Option<u64> = None; // the bytes a run before left in the temporary file
    loop {
        let mut counted: i64 = 0; // what this attempt added to the progress (taken off if it fails)
        let mut hasher: Option<PartHasher> = None;
        let attempt = (|| -> Attempt {
            let io = |e: std::io::Error| Attempt::Nas(e);
            let mut have = match tmp.metadata() {
                Ok(m) => m.len(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                Err(e) => return io(e),
            };
            if have > size {
                if let Err(e) = tmp.remove_file() {
                    return io(e);
                }
                have = 0;
            }
            kept.get_or_insert(have);
            hasher = job.etag.as_deref().map(|e| PartHasher::new(size, e));
            if let (true, Some(h)) = (have > 0, hasher.as_mut()) {
                // The bytes already here, hashed again (read back from the NAS).
                let mut f = match tmp.open() {
                    Ok(f) => f,
                    Err(e) => return io(e),
                };
                let mut buf = vec![0u8; 8 << 20];
                loop {
                    match f.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => h.update(&buf[..n]),
                        Err(e) => return io(e),
                    }
                }
            }
            prog.add(have as i64);
            counted += have as i64;
            let mut got = have;
            let mut f = match tmp.open_with(std::fs::OpenOptions::new().create(true).append(true)) {
                Ok(f) => f,
                Err(e) => return io(e),
            };
            while got < size {
                let end = (got + PIECE).min(size) - 1;
                let mut h: Vec<(&str, String)> = vec![("Range", format!("bytes={got}-{end}"))];
                if let Some(e) = &job.etag {
                    h.push(("If-Match", format!("\"{e}\"")));
                }
                let mut r = match http.call(&job.url, false, &h, PIECE_S) {
                    Ok(r) => r,
                    Err(Fail::Refused(412)) => {
                        // The object isn't the one listed (replaced since?): from the start, next run.
                        tmp.remove_file().ok();
                        return Attempt::Fatal(anyhow::anyhow!(
                            "{}: changed on the server since it was listed (412); run again",
                            job.name
                        ));
                    }
                    Err(Fail::Refused(c)) => return Attempt::Fatal(anyhow::anyhow!("{}: HTTP {c}", job.name)),
                    Err(Fail::Again(w, ra)) => return Attempt::Again(w, ra),
                    Err(Fail::Other(w)) => return Attempt::Fatal(anyhow::anyhow!("{}: {w}", job.name)),
                };
                let want = if r.status().as_u16() == 206 {
                    end + 1
                } else {
                    // The range wasn't honoured (the whole file came): from the start.
                    if got > 0 {
                        if let Err(e) = f.set_len(0) {
                            return io(e);
                        }
                        prog.add(-counted);
                        counted = 0;
                        got = 0;
                        kept = Some(0);
                        hasher = job.etag.as_deref().map(|e| PartHasher::new(size, e));
                    }
                    size
                };
                let mut rd = r.body_mut().with_config().limit(want - got + 1).reader();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = match rd.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) => {
                            f.flush().ok();
                            return Attempt::Again(e.to_string(), None);
                        }
                    };
                    if got + n as u64 > want {
                        return Attempt::Fatal(anyhow::anyhow!("{}: more bytes than asked for", job.name));
                    }
                    if let Err(e) = f.write_all(&buf[..n]) {
                        return io(e);
                    }
                    if let Some(h) = hasher.as_mut() {
                        h.update(&buf[..n]);
                    }
                    got += n as u64;
                    prog.add(n as i64);
                    counted += n as i64;
                }
                if got < want {
                    if let Err(e) = f.flush() {
                        return io(e);
                    }
                    return Attempt::Again(format!("{} of {size} bytes", got), None);
                }
                // (Each piece whole moves the file on: a later failure goes on from here.)
                k = 0;
            }
            if let Err(e) = f.sync_all() {
                return io(e);
            }
            Attempt::Whole
        })();
        match attempt {
            Attempt::Whole => {
                // Checked before it's put in place.
                let why = if hasher.take().is_some_and(|h| !h.ok()) {
                    Some("its ETag doesn't match".to_string())
                } else if job.ghsl {
                    zip_bad(&tmp)
                } else {
                    None
                };
                if let Some(why) = why {
                    tmp.remove_file().ok();
                    prog.add(-counted);
                    kept = Some(0);
                    fresh_starts += 1;
                    ensure!(fresh_starts <= 1, "{}: {why}, twice", job.name);
                    eprintln!("bld-fetch: {}: {why}; fetching it again from the start", job.name);
                    continue;
                }
                tmp.rename(&job.dest)?;
                prog.file_done();
                return Ok((true, size - kept.unwrap_or(0)));
            }
            Attempt::Again(why, ra) => {
                prog.add(-counted);
                ensure!(k < 12, "{}: {why}", job.name);
                let w = wait_s(ra, k);
                k += 1;
                eprintln!("bld-fetch: {}: {why}; going on from where it is in {w:.0} s", job.name);
                std::thread::sleep(Duration::from_secs_f64(w));
            }
            Attempt::Fatal(e) => {
                prog.add(-counted);
                return Err(e);
            }
            Attempt::Nas(e) => {
                prog.add(-counted);
                eprintln!("bld-fetch: {}: {e}; checking the NAS", job.name);
                std::thread::sleep(Duration::from_secs(10));
                wait_for_nas(root)?;
                k += 1;
                ensure!(k <= 12, "{}: {e}", job.name);
            }
        }
    }
}

// ---- the run ----------------------------------------------------------------------------------------

/// What `run` is given.
pub struct Args<'a> {
    /// The NAS's project folder (the sources' home).
    pub root: &'a store::nasfs::NasRoot,
    /// The declared sources (crate::inputs::sources): `buildings` (its release, bucket and
    /// types) and `building-heights` (its version and index).
    pub sources: &'a crate::inputs::sources::Sources,
    pub cov: &'a Coverage,
}

/// The coverage's regions, by id, in their order.
fn regions(cov: &Coverage) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in &cov.shapes {
        let id = s.source.split_once(": ").map_or(s.source.as_str(), |(id, _)| id);
        if !out.iter().any(|r| r == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// Fetches what the coverage needs and writes the indexes (see the module's docs).
pub fn run(a: Args<'_>) -> Result<()> {
    use crate::inputs::sources::{BUILDINGS, BUILDING_HEIGHTS};
    use crate::timings::{phase, Class};
    let root = a.root;
    ensure!(root.join("sources").is_dir(), "{}: not the NAS's project folder (no sources/)", root.display());
    let lock = crate::sys::PathLock::take(&std::env::temp_dir().join("bldfetch.lock"), false)?;
    ensure!(lock.is_some(), "another bld-fetch runs on this Mac");
    let (b, g) = (a.sources.role(BUILDINGS)?, a.sources.role(BUILDING_HEIGHTS)?);
    let release = b.version()?.to_string();
    let bucket = b.url()?.to_string();
    let types = b.param_strs("types")?;
    let http = Http::new();
    eprintln!(
        "bld-fetch: pid {} on {}, root {}, {TRANSFERS} transfers at once",
        std::process::id(),
        store::sys::host(),
        root.display()
    );
    let p = phase("reading the coverage", Class::Compute);
    let near = Near::new(a.cov, MARGIN_KM)?;
    let what = regions(a.cov);
    drop(p);
    eprintln!("bld-fetch: coverage: {} regions, files within {MARGIN_KM} km of it", what.len());
    let (ov, ov_base) = overture_jobs(&http, root, &release, &bucket, &types, &near)?;
    eprintln!(
        "bld-fetch: overture: {} files meet the coverage, {}, {:.1} M buildings in their row groups that meet it",
        ov.len(),
        gb(ov.iter().map(|j| j.size as f64).sum()),
        ov.iter().map(|j| j.rows_near as f64).sum::<f64>() / 1e6
    );
    let (gh, gh_base) = ghsl_jobs(&http, root, g, &near)?;
    eprintln!("bld-fetch: ghsl: {} tiles meet the coverage, {}", gh.len(), gb(gh.iter().map(|j| j.size as f64).sum()));
    let jobs: Vec<Job> = ov.into_iter().chain(gh).collect();
    let p = phase("checking the files here", Class::NasRead);
    let whole = |j: &Job| j.dest.metadata().is_ok_and(|m| m.len() == j.size);
    let known: u64 = jobs.iter().map(|j| j.size).sum();
    let left: u64 = jobs.iter().filter(|j| !whole(j)).map(|j| j.size).sum();
    eprintln!(
        "bld-fetch: {} files, {} already here; {} to fetch",
        jobs.len(),
        jobs.iter().filter(|j| whole(j)).count(),
        gb(left as f64)
    );
    drop(p);
    // (What's to fetch must fit in the NAS's free space past the owner's reserve, `scenic nas-room`.)
    let room = crate::agent::room::nas_room(root)?;
    ensure!(left <= room, "the NAS has {} free past its reserve: too little for {}", gb(room as f64), gb(left as f64));
    let prog = Progress {
        files: jobs.len(),
        total: known,
        done_files: AtomicU64::new(0),
        done: AtomicU64::new(0),
        t0: Instant::now(),
        window: Mutex::new(vec![(Instant::now(), 0)]),
    };
    let checked: Mutex<BTreeMap<String, &'static str>> = Mutex::new(BTreeMap::new());
    let failed: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let save = |checked: &BTreeMap<String, &'static str>| {
        if let Err(e) = save_indexes(&jobs, &ov_base, &gh_base, &release, &bucket, g, &what, checked) {
            eprintln!("bld-fetch: the indexes weren't written now ({e:#}); the next save writes them");
        }
    };
    // GHSL's small tiles first, then Overture's parts, then its buildings.
    let mut order: Vec<&Job> = jobs.iter().collect();
    order.sort_by_key(|j| (!j.ghsl, !j.name.contains("type=building_part/"), j.name.clone()));
    let next = AtomicU64::new(0);
    let stop_report = AtomicBool::new(false);
    let p = phase("downloading the files to the NAS", Class::Net);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut last = Instant::now();
            while !stop_report.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if last.elapsed() >= Duration::from_secs(60) {
                    eprintln!("bld-fetch: {}", prog.line());
                    last = Instant::now();
                }
            }
        });
        let workers: Vec<_> = (0..TRANSFERS)
            .map(|_| {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                    let Some(job) = order.get(i) else { return };
                    let t0 = Instant::now();
                    match fetch(&http, job, root, &prog) {
                        Ok((true, new)) => {
                            let dt = t0.elapsed().as_secs_f64();
                            let check = if job.ghsl { "zip CRC-32" } else { "ETag (MD5 of its parts)" };
                            let kept = if new < job.size {
                                format!(", {:.1} MB of it kept from before", (job.size - new) as f64 / 1e6)
                            } else {
                                String::new()
                            };
                            eprintln!(
                                "bld-fetch: {}: {:.1} MB in {dt:.0} s ({:.1} MB/s){kept}; {check} checked",
                                job.name,
                                new as f64 / 1e6,
                                new as f64 / 1e6 / dt.max(1e-9)
                            );
                            let mut c = checked.lock().unwrap_or_else(|e| e.into_inner());
                            c.insert(job.name.clone(), check);
                            if c.len().is_multiple_of(10) {
                                save(&c);
                            }
                        }
                        Ok((false, _)) => {}
                        Err(e) => {
                            eprintln!("bld-fetch: {}: FAILED: {e:#}", job.name);
                            failed.lock().unwrap_or_else(|e| e.into_inner()).push(job.name.clone());
                        }
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().ok();
        }
        stop_report.store(true, Ordering::Relaxed);
    });
    drop(p);
    eprintln!("bld-fetch: {}", prog.line());
    let _p = phase("writing the indexes", Class::NasWrite);
    save(&checked.lock().unwrap_or_else(|e| e.into_inner()));
    let failed = failed.into_inner().unwrap_or_default();
    if !failed.is_empty() {
        let more = if failed.len() > 5 { " …" } else { "" };
        bail!("{} files failed: {}{more} (run again to retry)", failed.len(), failed[..failed.len().min(5)].join(", "));
    }
    eprintln!("bld-fetch: done: {} files in place", jobs.len());
    Ok(())
}

/// The indexes beside the files: `buildings.json` (the files here, those listed before kept) and
/// GHSL's `index.json` (likewise).
#[allow(clippy::too_many_arguments)]
fn save_indexes(
    jobs: &[Job],
    ov_base: &NasPath,
    gh_base: &NasPath,
    release: &str,
    bucket: &str,
    ghsl: &crate::inputs::sources::Source,
    what: &[String],
    checked: &BTreeMap<String, &'static str>,
) -> Result<()> {
    let whole = |j: &Job| j.dest.metadata().is_ok_and(|m| m.len() == j.size);
    let idx_path = ov_base.join("buildings.json");
    let idx = read_json(&idx_path);
    let mut files: BTreeMap<String, Py> = match idx.as_ref().and_then(|i| i.get("files")) {
        Some(Py::Dict(d)) => d.iter().cloned().collect(),
        _ => BTreeMap::new(),
    };
    for j in jobs.iter().filter(|j| !j.ghsl && whole(j)) {
        let f = files.remove(&j.name).unwrap_or(Py::Dict(Vec::new()));
        let was = f.get("checked").filter(|c| c.truthy()).cloned();
        let mut d: Vec<(String, Py)> = match f {
            Py::Dict(d) => d,
            _ => Vec::new(),
        };
        let set = |d: &mut Vec<(String, Py)>, k: &str, v: Py| match d.iter_mut().find(|x| x.0 == k) {
            Some(x) => x.1 = v,
            None => d.push((k.to_string(), v)),
        };
        set(&mut d, "size", int(j.size));
        set(&mut d, "etag", Py::Str(j.etag.clone().unwrap_or_default()));
        set(&mut d, "rows", int(j.rows));
        set(&mut d, "row_groups", int(j.row_groups));
        set(&mut d, "rows_near", int(j.rows_near));
        set(&mut d, "bbox", float_list(&j.bbox));
        let c = was.unwrap_or_else(|| Py::Str(checked.get(&j.name).copied().unwrap_or("size").to_string()));
        set(&mut d, "checked", c);
        files.insert(j.name.clone(), Py::Dict(d));
    }
    let coverage = dict(vec![
        ("regions", Py::List(what.iter().map(|r| Py::Str(r.clone())).collect())),
        ("margin_km", Py::Float(MARGIN_KM)),
        ("at", Py::Str(now_iso())),
    ]);
    write_json(
        &idx_path,
        dict(vec![
            ("fmt", int(1)),
            ("release", Py::Str(release.to_string())),
            ("source", Py::Str(format!("{bucket}release/{release}/theme=buildings/"))),
            ("terms", Py::Str("ODbL 1.0 (the theme; its sources' licences per row in `sources`)".into())),
            ("coverage", coverage),
            ("files", Py::Dict(files.into_iter().collect())),
        ]),
        false,
    )?;
    let idx_path = gh_base.join("index.json");
    let mut tiles: BTreeMap<String, Py> = match read_json(&idx_path).as_ref().and_then(|i| i.get("tiles")) {
        Some(Py::Dict(d)) => d.iter().cloned().collect(),
        _ => BTreeMap::new(),
    };
    for j in jobs.iter().filter(|j| j.ghsl && whole(j)) {
        tiles.insert(
            j.name.clone(),
            dict(vec![("size", int(j.size)), ("bbox", float_list(&j.bbox)), ("checked", Py::Str("zip CRC-32".into()))]),
        );
    }
    write_json(
        &idx_path,
        dict(vec![
            ("fmt", int(1)),
            ("product", Py::Str(format!("{} {}", ghsl.id, ghsl.version()?))),
            ("source", Py::Str("its declaration's index".into())),
            ("terms", Py::Str("© European Union, 1995-2026; CC BY 4.0".into())),
            ("tiles", Py::Dict(tiles.into_iter().collect())),
        ]),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_etag_is_the_md5_of_the_file_or_of_its_parts() {
        use md5::Digest;
        let data: Vec<u8> = (0..(11u32 << 20)).map(|i| (i * 7 + i / 3) as u8).collect();
        let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let whole = hex(&md5::Md5::digest(&data));
        let mut h = PartHasher::new(data.len() as u64, &whole);
        for c in data.chunks(1 << 20) {
            h.update(c);
        }
        assert!(h.ok());
        // Two parts of 8 MiB and 3 MiB: the MD5 of their MD5s, "-2".
        let parts: Vec<u8> = data.chunks(8 << 20).flat_map(|c| md5::Md5::digest(c).to_vec()).collect();
        let multi = format!("{}-2", hex(&md5::Md5::digest(&parts)));
        let mut h = PartHasher::new(data.len() as u64, &multi);
        for c in data.chunks(3 << 20) {
            h.update(c);
        }
        assert!(h.ok());
        let mut h = PartHasher::new(data.len() as u64, &multi);
        h.update(&data[1..]);
        assert!(!h.ok());
    }

    #[test]
    fn the_indexes_are_written_as_pythons_json_wrote_them() {
        let v = dict(vec![("b", Py::Float(20.0)), ("a", Py::Str("© x".into())), ("c", Py::List(vec![int(1)]))]);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("i.json");
        write_json(&store::nasfs::NasPath::local_for_test(&p), v.clone(), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "{\n \"a\": \"\\u00a9 x\",\n \"b\": 20.0,\n \"c\": [\n  1\n ]\n}"
        );
        write_json(&store::nasfs::NasPath::local_for_test(&p), v, true).unwrap();
        let gz = std::fs::read(&p).unwrap();
        // gzip.compress(…, mtime=0): no time, level 9's flag, OS 255.
        assert_eq!(&gz[..10], &[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 255]);
        let mut s = String::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut s).unwrap();
        assert_eq!(s, "{\"a\": \"\\u00a9 x\", \"b\": 20.0, \"c\": [1]}");
    }

    #[test]
    fn a_listing_reads_as_s3_writes_it() {
        let x = "<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>release/a/x.parquet</Key><Size>5</Size><ETag>&quot;ab-2&quot;</ETag></Contents><Contents><Key>release/a/_SUCCESS</Key><Size>0</Size><ETag>&quot;e&quot;</ETag></Contents><NextContinuationToken>t/1=</NextContinuationToken></ListBucketResult>";
        let c = texts(x, "Contents");
        assert_eq!(c.len(), 2);
        assert_eq!(unescape(texts(c[0], "ETag")[0]).trim_matches('"'), "ab-2");
        assert_eq!(quote("release/2026-09-23.1/theme=buildings/"), "release%2F2026-09-23.1%2Ftheme%3Dbuildings%2F");
    }

    #[test]
    fn near_is_within_the_margin_of_an_outline() {
        use crate::coverage::Shape;
        let sq = |x: f64, y: f64, d: f64| -> Vec<[i32; 2]> {
            let e = |v: f64| (v * 1e7) as i32;
            vec![[e(x), e(y)], [e(x + d), e(y)], [e(x + d), e(y + d)], [e(x), e(y + d)]]
        };
        let cov = Coverage {
            shapes: vec![Shape::new("r: osm:1".into(), vec![sq(10.0, 45.0, 1.0), sq(179.8, 0.0, 0.1)], 0.0)],
            ..Default::default()
        };
        let n = Near::new(&cov, 20.0).unwrap();
        // 0.2° east at 45°N is ~15.7 km: near; 0.4° is ~31 km: not.
        assert!(n.meets(11.2, 45.4, 11.3, 45.5).unwrap());
        assert!(!n.meets(11.4, 45.4, 11.5, 45.5).unwrap());
        // Across ±180°.
        assert!(n.meets(-179.95, 0.0, -179.9, 0.05).unwrap());
        // Near the pole any longitude is near an outline in the latitudes within the margin: none here.
        assert!(!n.meets(-50.0, 89.9, -49.0, 90.0).unwrap());
    }
}
