//! The OSM pass (docs/steps.md): the planet, fetched onto the NAS, turned on the lead into
//! what every later step reads: the filtered planet, extracts per z6 z6 tile (10 km buffer, ways whole,
//! multipolygons complete), the worldwide sets, the basemap, and worldwide road values.
//!
//! Each stage leaves a marker in the scratch folder, so a pass interrupted by sleep, a reboot or a
//! full disk resumes at the stage it was in; a stage that touched the NAS is redone from its inputs.

use crate::chain::{self, Interner, Link, RoadVal};
use crate::hipack::{grow, tile_bounds};
use crate::legacy::TileId;
use crate::out::Out;
use anyhow::{bail, ensure, Context, Result};
use rayon::prelude::*;
use roadcore::{class, flag, Ways};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the pipeline reads from OSM: the union of every step's tags, generous (a tag left out costs a
/// new planet download). Referenced nodes and members come along.
pub const FILTER_A: &[&str] = &[
    "nwr/highway",
    "nwr/railway",
    "r/route",
    "w/route=ferry",
    "nwr/public_transport",
    "nwr/amenity",
    "nwr/tourism",
    "nwr/historic",
    "nwr/heritage",
    "nwr/natural",
    "nwr/waterway",
    "nwr/water",
    "nwr/man_made",
    "nwr/leisure",
    "nwr/boundary",
    "nwr/building=train_station,church,cathedral,temple,shrine,mosque,synagogue,castle",
    "nwr/landuse=forest,reservoir,basin,salt_pond",
    "nwr/military",
    "nwr/place",
    "n/barrier",
    "nwr/aerialway",
    "nwr/mountain_pass",
    "nwr/wikidata",
];

/// The worldwide sets, each a filter of the filtered planet, with its filter's version: a changed
/// filter gets a new version, and so a new logical name (`set_name`), which the latest pass makes
/// from its kept filtered planet (`scenic-build pass-sets`); readers ask for the current one.
pub const SETS: &[(&str, u32, &[&str])] = &[
    (
        "rail",
        1,
        &["nwr/railway", "nwr/public_transport", "r/route=train,subway,tram,light_rail,monorail,funicular,railway"],
    ),
    ("ferries", 1, &["w/route=ferry", "r/route=ferry", "nwr/amenity=ferry_terminal"]),
    ("areas", 1, &["wr/boundary=national_park,protected_area,aboriginal_lands", "wr/leisure=nature_reserve"]),
    ("places", 1, &["n/place"]),
    ("outlines", 1, &["r/boundary=administrative", "r/ISO3166-1", "r/ISO3166-2"]),
    // The labels by importance (pipeline::labels): places, seas, bays and straits, water and parks.
    (
        "labels",
        1,
        &[
            "n/place",
            "n/natural=bay,strait",
            "wr/natural=water,bay,strait",
            "wr/boundary=national_park,protected_area",
            "wr/leisure=nature_reserve",
        ],
    ),
    // (The landmark candidates come from the z6 tiles' extracts, extract `--candidates`, not a set.)
    // Summits, worldwide, for prominence and isolation: peaks and volcanoes, nodes and ways (2: ways
    // too, as the peaks among the candidates are).
    ("summits", 2, &["nw/natural=peak,volcano"]),
    // Hiking and foot routes with their member ways, for the routes' ends (pipeline::trailends).
    ("hikes", 1, &["r/route=hiking,foot"]),
    // The heritage chain's named objects (heritage, museums, lighthouses, stations, protected areas
    // and parks), and World Heritage objects, for locating register records.
    (
        "named",
        1,
        &[
            "nwr/historic",
            "nwr/heritage",
            "nwr/tourism=museum,attraction,viewpoint",
            "nwr/man_made=lighthouse",
            "nwr/railway=station",
            "nwr/building=train_station,church,cathedral",
            "nwr/amenity=place_of_worship",
            "nwr/boundary=protected_area,national_park",
            "nwr/leisure=park",
            "nwr/military",
            "nwr/ref:whc",
            "nwr/heritage:operator=whc",
        ],
    ),
    // The World Heritage outlines' sources (crate::whsshapes' filter, its objects' references
    // with them), and every multipolygon, so the heritage job's clip of this to the cover (its smart
    // strategy completing the multipolygons that reach it) holds what crate::whsshapes reads of the
    // filtered planet's clip: about a fifth of the filtered planet, read in its place (task #496).
    ("whs", 1, &["nwr/wikidata", "nwr/ref:whc", "nwr/heritage:operator=whc", "r/type=multipolygon"]),
];

/// A set's logical name in its current version (version 1 is the plain name).
pub fn set_name(date: &str, name: &str) -> String {
    match SETS.iter().find(|s| s.0 == name).map(|s| s.1).unwrap_or(1) {
        1 => format!("sources/osm/{date}/sets/{name}"),
        v => format!("sources/osm/{date}/sets/{name}-v{v}"),
    }
}

/// The sets a pass lacks in their current versions.
pub fn missing_sets(out: &Out, date: &str) -> Vec<&'static (&'static str, u32, &'static [&'static str])> {
    SETS.iter().filter(|s| out.get(&set_name(date, s.0)).is_none()).collect()
}

/// Makes the sets a pass lacks (`missing_sets`) from its filtered planet `src`, reporting each.
pub fn make_missing_sets(out: &mut Out, date: &str, src: &Input, scratch: &Path) -> Result<usize> {
    let missing = missing_sets(out, date);
    for (k, (name, _, exprs)) in missing.iter().enumerate() {
        crate::agent::jobs::stage(k as u64, missing.len() as u64, &format!("sets made ({name} now)"));
        let o = scratch.join(format!("set-{name}.osm.pbf"));
        tags_filter(src, &o, exprs, scratch, &format!("osmium tags-filter (set {name})"), Some("sets filtered"))?;
        let p = crate::timings::phase("sets uploaded", crate::timings::Class::NasWrite);
        out.put_file(&set_name(date, name), "osm.pbf", &o)?;
        drop(p);
        out.save()?;
        std::fs::remove_file(&o).ok();
    }
    crate::agent::jobs::report(missing.len() as u64, missing.len() as u64, "sets made");
    Ok(missing.len())
}

/// What an osmium filter reads: a file here, or one on the NAS (the share's mount), streamed over
/// SSH (`streamed`).
pub enum Input {
    Local(PathBuf),
    Nas(PathBuf),
}

/// `osmium tags-filter` of `src` by `exprs` into `o` (`read_input`), in a phase of its own when
/// given one (`phase`: osmium's time for a local file, the NAS's read for one on the NAS).
fn tags_filter(src: &Input, o: &Path, exprs: &[&str], scratch: &Path, what: &str, phase: Option<&str>) -> Result<()> {
    use crate::timings::Class;
    let _p = phase.map(|ph| match src {
        Input::Local(_) => crate::timings::phase(&format!("{ph} (osmium)"), Class::Compute),
        Input::Nas(_) => crate::timings::phase(&format!("{ph} (osmium, over SSH)"), Class::NasRead),
    });
    // (Adding the objects the matches reference, osmium reads its input twice.)
    read_input(src, &scratch.join("stream"), 2, |input, local| {
        let mut c = osmium();
        c.args(["tags-filter", "--overwrite", "-F", "pbf", "-o"]).arg(o).arg(input).args(exprs);
        if local {
            run_osmium(c, what)
        } else {
            run(c, what)
        }
    })
}

/// Runs `run` on `src`'s path: a local file's own, or one streaming a file on the NAS over SSH
/// (`streamed`, `reads` reads of it expected, for progress; the bytes counted as the enclosing
/// phase's), with whether it's local.
fn read_input(src: &Input, dir: &Path, reads: u32, run: impl FnOnce(&Path, bool) -> Result<()>) -> Result<()> {
    match src {
        Input::Local(p) => run(p, true),
        #[cfg(target_os = "wasi")]
        Input::Nas(p) => {
            let _ = (dir, reads);
            bail!("{}: no streaming off the NAS here", p.display())
        }
        #[cfg(not(target_os = "wasi"))]
        Input::Nas(p) => {
            let remote =
                store::nas::on_nas(p).with_context(|| format!("{} isn't on the NAS's mounted share", p.display()))?;
            let n = streamed(&ssh, SSH_START, &remote, reads, dir, |input| run(input, false))?;
            crate::timings::count(n, 1);
            Ok(())
        }
    }
}

/// The ciphers `ssh` prefers for the planet's stream, in order: AES-GCM (hardware-accelerated on
/// both ends: 25 -> 59 MB/s for 1 GiB from the NAS on the Mac mini, task #554); ssh uses the first
/// the server also offers, so a server without it falls to the next.
#[cfg(not(target_os = "wasi"))]
const STREAM_CIPHERS: &str = "aes128-gcm@openssh.com,chacha20-poly1305@openssh.com,aes128-ctr";

/// How long an `ssh` to the NAS may take to start: to connect (ConnectTimeout, 20 s) and then to
/// authenticate, its key from the SSH agent, 1Password's, which waits for an approval while it's
/// locked, unbounded (ConnectTimeout doesn't cover it); a working one starts within seconds. The
/// call past it is stopped and fails `TimedOut`, as a NAS call past its deadline does
/// (`agent::nasread`): the job fails and is tried again later.
#[cfg(not(target_os = "wasi"))]
pub(crate) const SSH_START: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs `f` with `pid` (a child not yet waited for: the caller waits after) stopped (SIGTERM) if
/// `f` hasn't called its argument ("started") within `bound`, or returned; and whether it was.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn watched<T>(pid: u32, bound: std::time::Duration, f: impl FnOnce(&dyn Fn()) -> T) -> (T, bool) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|s| {
        let dog = s.spawn(move || {
            let fire = rx.recv_timeout(bound) == Err(std::sync::mpsc::RecvTimeoutError::Timeout);
            if fire {
                // SAFETY: a plain signal to the caller's unreaped child.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }
            fire
        });
        let got = f(&|| {
            let _ = tx.send(());
        });
        drop(tx);
        (got, dog.join().unwrap_or(false))
    })
}

/// `ssh` (a command line, its host last) runs `true` within `SSH_START`: authenticated, and its
/// shared connection opened when it makes one (ControlMaster), for the calls after to take.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn ssh_opened(ssh: &[&str]) -> Result<()> {
    let mut probe = store::spawn::command(ssh[0]);
    probe.args(&ssh[1..]).arg("true").stdin(std::process::Stdio::null());
    let mut child = store::spawn::spawn(&mut probe).context("ssh to the NAS")?;
    let pid = child.id();
    let ((), stalled) = watched(pid, SSH_START, |_| {
        // (Its end awaited, the child left to `wait` below: its PID stays its own till then.)
        // SAFETY: a zeroed siginfo_t, written by waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
    });
    let st = child.wait()?;
    // (One that ended well just as the bound fired stands: the signal met a finished child.)
    if stalled && !st.success() {
        return Err(ssh_stalled("its first call", SSH_START));
    }
    ensure!(st.success(), "ssh to the NAS failed: {st}");
    Ok(())
}

/// The error for an `ssh` to the NAS stopped by `watched`.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn ssh_stalled(what: &str, bound: std::time::Duration) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "ssh {} ({what}) didn't start within {} s: stopped (connecting, or its SSH agent waiting for an approval: 1Password locked?)",
            store::nas::SSH,
            bound.as_secs()
        ),
    ))
}

/// `ssh` to the NAS in batch mode (no prompts), through the job's proxy when it has one (a job
/// under the budgets' sandbox, crate::budget::SANDBOX, connects only to this Mac: the proxy
/// tunnels the connection, `CONNECT`, unmetered).
#[cfg(not(target_os = "wasi"))]
fn ssh() -> Command {
    let mut c = store::spawn::command("ssh");
    c.args(["-o", "BatchMode=yes", "-o", "ServerAliveInterval=30", "-o", "ConnectTimeout=20"]);
    c.args(["-c", STREAM_CIPHERS]);
    if let Some(p) = std::env::var(crate::budget::PROXY_ENV).ok().and_then(|u| proxy_addr(&u)) {
        c.arg("-o").arg(format!("ProxyCommand=/usr/bin/nc -X connect -x {p} %h %p"));
    }
    c
}

/// The proxy's `host:port` from its URL (`http://127.0.0.1:<port>`).
#[cfg(not(target_os = "wasi"))]
pub(crate) fn proxy_addr(url: &str) -> Option<String> {
    let a = url.strip_prefix("http://")?.trim_end_matches('/');
    a.contains(':').then(|| a.to_string())
}

/// Runs `run` on a path that reads as `remote` (a file on the NAS) streamed over SSH
/// (`ssh`'s command, `cat` on the NAS), never through the SMB mount: a soft mount times out on
/// osmium's hour-long reads ("Close failed: Operation timed out", 2026-10-21, four times). osmium
/// reads its input twice to add the objects the matches reference (and refuses stdin for that), so
/// the path is a link to a named pipe, fed one `cat` each time osmium opens it: once it's opened,
/// the link is pointed at a fresh pipe for the next open. Each read must carry the file's whole
/// length (its size taken over SSH first): a short `cat` that exits 0 fails. `reads`: how many
/// reads `run` makes, for progress (0: none said, the caller says its own). Each `ssh` is bounded
/// by `start` (`SSH_START`): the size's call whole, each `cat` to its first byte. Returns the bytes read off the NAS. In `dir`, made afresh.
#[cfg(not(target_os = "wasi"))]
pub fn streamed(
    ssh: &(dyn Fn() -> Command + Sync),
    start: std::time::Duration,
    remote: &str,
    reads: u32,
    dir: &Path,
    run: impl FnOnce(&Path) -> Result<()>,
) -> Result<u64> {
    let quoted = format!("'{}'", remote.replace('\'', "'\\''"));
    let mut check = ssh();
    // (Its size by `ls`, as producers/depth/repack.py reads sizes on the NAS: `-L` through a link.)
    check.arg(store::nas::SSH).arg(format!("test -r {quoted} && ls -lnL -- {quoted}"));
    check.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = store::spawn::spawn(&mut check).context("run ssh")?;
    let (mut out, mut err) =
        (child.stdout.take().context("ssh's output")?, child.stderr.take().context("ssh's errors")?);
    let (read, stalled) = watched(child.id(), start, |_| -> std::io::Result<(Vec<u8>, Vec<u8>)> {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        std::io::Read::read_to_end(&mut out, &mut o)?;
        std::io::Read::read_to_end(&mut err, &mut e)?;
        Ok((o, e))
    });
    let status = child.wait()?;
    if stalled && !status.success() {
        return Err(ssh_stalled(&format!("the size of {remote}"), start));
    }
    let (stdout, stderr) = read.context("read ssh's output")?;
    let ok = std::process::Output { status, stdout, stderr };
    ensure!(
        ok.status.success(),
        "can't read {remote} on the NAS over SSH ({}: {}): set up SSH keys to the NAS (docs/macs.md)",
        store::nas::SSH,
        String::from_utf8_lossy(&ok.stderr).trim()
    );
    let len: u64 = String::from_utf8_lossy(&ok.stdout)
        .split_whitespace()
        .nth(4)
        .and_then(|n| n.parse().ok())
        .with_context(|| format!("{remote}'s size on the NAS ({})", String::from_utf8_lossy(&ok.stdout).trim()))?;
    std::fs::remove_dir_all(dir).ok();
    std::fs::create_dir_all(dir)?;
    let link = dir.join("input.osm.pbf");
    let pipe = |k: u32| dir.join(format!("pass-{k}"));
    let point = |k: u32| -> Result<()> {
        let p = pipe(k);
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes())?;
        // SAFETY: a NUL-terminated path.
        ensure!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0, "mkfifo: {}", std::io::Error::last_os_error());
        let tmp = dir.join("input.tmp");
        std::os::unix::fs::symlink(&p, &tmp)?;
        std::fs::rename(&tmp, &link)?;
        Ok(())
    };
    point(0)?;
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (ran, fed) = std::thread::scope(|s| {
        let feeder = s.spawn(|| -> Result<u64> {
            use std::os::unix::fs::OpenOptionsExt;
            let (mut total, mut said) = (0u64, 0.0f64);
            for k in 0.. {
                // (Waits for osmium to open the pipe, or to be done: a writer's open without a
                // reader fails at once, non-blocking.)
                let mut w = loop {
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return Ok(total);
                    }
                    match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(pipe(k)) {
                        Ok(w) => break w,
                        Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                            std::thread::sleep(std::time::Duration::from_millis(50))
                        }
                        Err(e) => return Err(e.into()),
                    }
                };
                // (Blocking writes from here.)
                // SAFETY: the open file's descriptor.
                unsafe {
                    let fd = std::os::fd::AsRawFd::as_raw_fd(&w);
                    libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) & !libc::O_NONBLOCK);
                }
                point(k + 1)?;
                let mut c = ssh();
                c.arg(store::nas::SSH).arg(format!("cat {quoted}")).stdout(std::process::Stdio::piped());
                let mut child = store::spawn::spawn(&mut c).context("run ssh")?;
                let mut from = child.stdout.take().context("ssh's output")?;
                let mut buf = vec![0u8; 1 << 20];
                let before = total;
                let (copied, stalled) = watched(child.id(), start, |started| -> std::io::Result<()> {
                    loop {
                        let n = std::io::Read::read(&mut from, &mut buf)?;
                        started();
                        if n == 0 {
                            return Ok(());
                        }
                        std::io::Write::write_all(&mut w, &buf[..n])?;
                        total += n as u64;
                        // (Said each 0.1 %.)
                        let f = (total as f64 / (u64::from(reads.max(1)) * len.max(1)) as f64).min(0.99);
                        if reads > 0 && f - said >= 0.001 {
                            crate::agent::jobs::within(f);
                            said = f;
                        }
                    }
                });
                if copied.is_err() {
                    child.kill().ok();
                }
                let st = child.wait()?;
                if stalled {
                    return Err(ssh_stalled(&format!("cat {remote}"), start));
                }
                copied.context("stream the NAS's file to osmium")?;
                ensure!(st.success(), "ssh {} cat {remote} failed: {st}", store::nas::SSH);
                ensure!(total - before == len, "a short read of {remote}: {} of its {len} bytes", total - before);
            }
            Ok(total)
        });
        let ran = run(&link);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        (ran, feeder.join().unwrap_or_else(|_| Err(anyhow::anyhow!("the feeder panicked"))))
    });
    std::fs::remove_dir_all(dir).ok();
    match (ran, fed) {
        (Ok(()), Ok(n)) => Ok(n),
        (Ok(()), Err(e)) => Err(e),
        (Err(e), Ok(_)) => Err(e),
        (Err(e), Err(f)) => Err(anyhow::anyhow!("{e:#} (the NAS's stream: {f:#})")),
    }
}

/// The basemap's input (Planetiler's OpenMapTiles layers water, waterway, boundary, place,
/// water_name, park).
pub const FILTER_BASEMAP: &[&str] = &[
    "nwr/natural=water,bay,strait,wetland,glacier",
    "nwr/water",
    "nwr/waterway",
    "nwr/landuse=reservoir,basin,salt_pond",
    "nwr/leisure=nature_reserve,park",
    "nwr/boundary=administrative,national_park,protected_area,disputed",
    "nwr/place",
];

/// Room needed to copy the planet before filtering it: the copy, the filtered file it's deleted
/// after (allowed up to 75 % of the planet: 68 % measured on 2026-09-28's, 60.6 of 88.6 GB) and
/// 10 GB. Streaming the planet off the NAS instead (`streamed`) reads it twice over an hour or
/// more, and one broken connection ends the filter, whereas the copy resumes after any
/// interruption.
pub fn copy_room(planet_len: u64) -> u64 {
    planet_len + planet_len / 4 * 3 + (10 << 30)
}

/// Room the basemap's filter needs (its output, ~16 GB for the planet); short of it, the local
/// filtered file goes and the filter reads the NAS's copy.
const BASEMAP_ROOM: u64 = 50 << 30;

/// Room Planetiler needs for the basemap, from its input: it asked for 81 GB (54 GB of feature
/// storage) for the planet's 16.3 GB input, so six times the input. On 2026-10-03 it had 84 GB
/// beside the 60 GB local filtered file and ran out three times, hours in.
pub fn planetiler_room(input_len: u64) -> u64 {
    input_len * 6
}

/// Extract buffer around a z6 tile, km.
pub const BUFFER_KM: f64 = 10.0;

pub fn osmium() -> Command {
    let path = format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default());
    // (Looked up by the PATH it's given: store::spawn.)
    let mut c = store::spawn::command_in("osmium", std::ffi::OsStr::new(&path));
    c.env("PATH", path);
    c
}

fn run(mut c: Command, what: &str) -> Result<()> {
    eprintln!("$ {what}");
    let st = store::spawn::status(&mut c).with_context(|| format!("run {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// Runs an osmium command (`run`), how far it is (its `--progress`) said as the stage's under way
/// (crate::agent::jobs::within).
fn run_osmium(mut c: Command, what: &str) -> Result<()> {
    eprintln!("$ {what}");
    c.arg("--progress");
    let st =
        crate::agent::jobs::run_watched(&mut c, crate::agent::jobs::within).with_context(|| format!("run {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// The `name:<language>` tags the basemap keeps: the coverage's languages and its neighbours',
/// romanisations and kana readings (names::own reads them).
pub const BASEMAP_LANGUAGES: &str = "en,fr,es,pt,ca,eu,gl,cy,ga,gd,br,co,oc,de,it,nl,ja,ja-Latn,ja_rm,ja-Hira,ja_kana,zh,zh-Hant,zh-Hans,zh-Latn-pinyin,yue,ko,ko-Latn,ms,ta,iu";

/// Runs Planetiler (`run`), how far it is (`planetiler_fraction` of each line of its log, which
/// goes on to this job's) said as the part's.
fn run_planetiler(mut c: Command, what: &str) -> Result<()> {
    eprintln!("$ {what}");
    let mut child =
        store::spawn::spawn(&mut c.stdout(std::process::Stdio::piped())).with_context(|| format!("run {what}"))?;
    let mut far = 0.0f64;
    if let Some(o) = child.stdout.take() {
        crate::agent::jobs::each_line(o, |l| {
            eprintln!("{l}");
            if let Some(f) = planetiler_fraction(l).filter(|f| *f > far) {
                far = f;
                crate::agent::jobs::within(f);
            }
        });
    }
    let st = child.wait().with_context(|| format!("wait for {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// How far Planetiler is (0–1) by a line of its log: its phases weighted by their times on the
/// M4 (2026-09-28's pass: its sources 4.5 min, the first pass over the planet 3, the second
/// 20, the sort 3, the tiles 15 of 46), the second pass and the tiles each through by the share
/// they say they're done.
pub fn planetiler_fraction(line: &str) -> Option<f64> {
    // (Its log is coloured: the escapes go.)
    let mut l = String::with_capacity(line.len());
    let mut esc = false;
    for c in line.chars() {
        match (esc, c) {
            (false, '\u{1b}') => esc = true,
            (true, c) if c.is_ascii_alphabetic() => esc = false,
            (true, _) => {}
            (false, c) => l.push(c),
        }
    }
    let phase = l.split("INF [").nth(1)?.split([']', ':']).next()?;
    let pct = |of: &str| -> Option<f64> {
        let inner = l.split(of).nth(1)?.split(']').next()?;
        inner
            .split_whitespace()
            .find_map(|w| w.strip_suffix('%')?.parse::<f64>().ok())
            .map(|p| (p / 100.0).clamp(0.0, 1.0))
    };
    let (from, to, within) = match phase {
        "lake_centerlines" | "water_polygons" | "natural_earth" | "ne_lakes" => (0.0, 0.1, None),
        "osm_pass1" => (0.1, 0.16, None),
        "osm_pass2" => (0.16, 0.6, pct("blocks: ")),
        "boundaries" => (0.6, 0.61, None),
        "sort" => (0.61, 0.67, None),
        "archive" => (0.67, 1.0, pct("features: ")),
        _ => return None,
    };
    Some(from + within.unwrap_or(0.0) * (to - from))
}

/// A stage's completion marker.
fn done(scratch: &Path, stage: &str) -> PathBuf {
    scratch.join(format!("{stage}.done"))
}

fn mark(scratch: &Path, stage: &str) -> Result<()> {
    std::fs::write(done(scratch, stage), b"")?;
    Ok(())
}

/// Copy a (large) file, resuming a partial copy: what's there already is kept when its size is
/// shorter than the source's, and the rest appended.
pub fn copy_resume(src: &Path, dst: &Path) -> Result<()> {
    copy_resume_with(src, dst, &mut |_, _| {})
}

/// `copy_resume`, telling `progress` the bytes copied and the total as it goes (each 1 %).
pub fn copy_resume_with<'a>(
    src: impl Into<store::nasfs::Loc<'a>>,
    dst: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let src = src.into();
    let total = src.metadata()?.len();
    let have = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    if have == total {
        return Ok(());
    }
    ensure!(have < total, "{} is larger than its source", dst.display());
    let mut s = src.open()?;
    s.seek(SeekFrom::Start(have))?;
    let mut d = std::fs::OpenOptions::new().create(true).append(true).open(dst)?;
    let mut buf = vec![0u8; 16 << 20];
    let mut done = have;
    let t0 = std::time::Instant::now();
    loop {
        let n = s.read(&mut buf)?;
        if n == 0 {
            break;
        }
        d.write_all(&buf[..n])?;
        let step = (total / 100).max(1);
        if (done + n as u64) / step != done / step {
            progress(done + n as u64, total);
        }
        done += n as u64;
        if done % (2 << 30) < n as u64 {
            eprintln!(
                "copy {}: {:.1}/{:.1} GB ({:.0} MB/s)",
                src.display(),
                done as f64 / 1e9,
                total as f64 / 1e9,
                (done - have) as f64 / 1e6 / t0.elapsed().as_secs_f64()
            );
        }
    }
    d.sync_all()?;
    ensure!(done == total, "short copy of {}", src.display());
    Ok(())
}

/// Every z3 tile, with its extracts' bounds (the tile grown by the buffer).
/// The pass's filtered planet on the NAS.
fn filtered_nas(out: &Out, date: &str) -> Result<PathBuf> {
    // (osmium's input, read where it lies: tests/nas_access.rs.)
    Ok(out
        .path(out.get(&format!("sources/osm/{date}/filtered")).context("the filtered planet on the NAS")?)
        .unchecked_path())
}

/// Cuts `input` (the data of tile `t` and its buffer) down to the z6 extracts below `t`, a quarter at
/// a time and depth first, calling `each` on every non-empty z6 extract. osmium keeps id sets per
/// output that span the whole id range (about 4 GB each on a planet-sized input, measured), so a
/// run makes four. Each extract goes as soon as its quarters are cut, so the tree's files on disk add
/// up to about one copy of the input (plus the buffers' overlap), however uneven the data.
/// Resumable: `<tile>.cut` marks a tile whose quarters were cut, `<tile>.done` one whose whole
/// subtree is done.
fn cut_tree(
    input: &Input,
    t: TileId,
    consume: bool,
    work: &Path,
    each: &mut dyn FnMut(TileId, &Path) -> Result<()>,
) -> Result<()> {
    let kids: Vec<TileId> =
        (0..2u32).flat_map(|i| (0..2u32).map(move |j| TileId { z: t.z + 1, x: t.x * 2 + i, y: t.y * 2 + j })).collect();
    let dir = work.join(format!("z{}", t.z + 1));
    let marker = work.join(format!("{}.cut", t.dash()));
    if !marker.exists() {
        // (osmium extract with the smart strategy reads its input four times (1.19), each open a stream of its own when
        // it's on the NAS; the cut says its progress by the extracts made.)
        read_input(input, &work.join("stream"), 0, |p, _| cut(p, &kids, &dir, 4))?;
        std::fs::write(&marker, b"")?;
    }
    // Its quarters are on disk: the input goes (`consume`: a z6 tile of this tree, or the root's
    // local copy).
    if let (true, Input::Local(p)) = (consume, input) {
        std::fs::remove_file(p).ok();
    }
    for k in kids {
        let done = work.join(format!("{}.done", k.dash()));
        if done.exists() {
            continue;
        }
        let f = dir.join(format!("{}.osm.pbf", k.dash()));
        let cut_already = work.join(format!("{}.cut", k.dash())).exists();
        if f.exists() && is_empty_piece(&f) {
            std::fs::remove_file(&f)?;
        } else if k.z == 6 {
            // Gone without its mark: done just before an interruption (uploaded, links kept).
            if f.exists() {
                each(k, &f)?;
                std::fs::remove_file(&f)?;
            }
        } else if f.exists() || cut_already {
            cut_tree(&Input::Local(f), k, true, work, each)?;
        }
        std::fs::write(&done, b"")?;
    }
    Ok(())
}

/// An osmium extract config: one bbox per tile (grown by the buffer) → `dir/<z-x-y>.osm.pbf`.
fn extract_config(tiles: &[TileId], dir: &Path) -> serde_json::Value {
    let ex: Vec<serde_json::Value> = tiles
        .iter()
        .map(|u| {
            let b = grow(tile_bounds(u.z, u.x, u.y), BUFFER_KM);
            let d = |v: i32| v as f64 * 1e-7;
            serde_json::json!({"output": format!("{}.osm.pbf", u.dash()), "bbox": [d(b[0]), d(b[1]), d(b[2]), d(b[3])]})
        })
        .collect();
    serde_json::json!({"directory": dir, "extracts": ex})
}

/// Cut `input` into one extract per tile, in batches (osmium keeps an id set per output).
fn cut(input: &Path, tiles: &[TileId], dir: &Path, batch: usize) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (i, chunk) in tiles.chunks(batch).enumerate() {
        let cfg = dir.join(format!("extract-{i}.json"));
        std::fs::write(&cfg, serde_json::to_vec_pretty(&extract_config(chunk, dir))?)?;
        let mut c = osmium();
        c.args(["extract", "--strategy", "smart", "-S", "types=multipolygon", "--overwrite", "-c"])
            .arg(&cfg)
            .arg(input);
        run(c, &format!("osmium extract {} tiles of {}", chunk.len(), input.display()))?;
        std::fs::remove_file(&cfg).ok();
    }
    Ok(())
}

/// An extract with nothing in it (osmium writes a header-only file for an empty bbox).
fn is_empty_piece(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() < 1024).unwrap_or(true)
}

/// The z6 tile extracts of one pass, as the build manifest records them. (Stored under the names
/// `pieces` and `sources/osm/<date>/pieces/<z6 tile>`, from when an extract was a "piece": kept, as
/// every pass's records and its files on the NAS carry them; the pass summary's `units` is its count.)
#[derive(serde::Serialize, serde::Deserialize, Default, Debug)]
pub struct Pieces {
    pub date: String,
    /// Z6 tile "6/x/y" → logical name of its extract.
    pub pieces: BTreeMap<String, String>,
}

/// One z6 tile's chaining inputs: its owned ways (OSM id, length) and the pairs made at its nodes.
#[derive(Default, Debug, PartialEq)]
pub struct UnitLinks {
    pub ways: Vec<(u64, f32)>,
    /// (way a << 1 | end a, way b << 1 | end b) by OSM id.
    pub pairs: Vec<(u64, u64)>,
}

const LINKS_MAGIC: &[u8; 8] = b"RDLINK01";

impl UnitLinks {
    /// Kept between the cut and the walk: the counts, then 16 bytes per way (id, length as f64
    /// bits) and per pair.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut b = Vec::with_capacity(24 + 16 * (self.ways.len() + self.pairs.len()));
        b.extend_from_slice(LINKS_MAGIC);
        b.extend_from_slice(&(self.ways.len() as u64).to_le_bytes());
        b.extend_from_slice(&(self.pairs.len() as u64).to_le_bytes());
        for &(id, len) in &self.ways {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&(len as f64).to_bits().to_le_bytes());
        }
        for &(a, c) in &self.pairs {
            b.extend_from_slice(&a.to_le_bytes());
            b.extend_from_slice(&c.to_le_bytes());
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &b)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<UnitLinks> {
        let b = std::fs::read(path)?;
        anyhow::ensure!(b.len() >= 24 && &b[..8] == LINKS_MAGIC, "{}: not a links file", path.display());
        let u = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let (nw, np) = (u(8) as usize, u(16) as usize);
        anyhow::ensure!(b.len() == 24 + 16 * (nw + np), "{}: truncated", path.display());
        let ways = (0..nw).map(|k| (u(24 + 16 * k), f64::from_bits(u(32 + 16 * k)) as f32)).collect();
        let at = 24 + 16 * nw;
        let pairs = (0..np).map(|k| (u(at + 16 * k), u(at + 8 + 16 * k))).collect();
        Ok(UnitLinks { ways, pairs })
    }
}

/// Run `extract` on a extract and work out its z6 tile's chaining inputs.
/// `spacing`: the declared datasets' spacing file (crate::dem::spacing), as the z6 tiles' extract has it.
pub fn unit_links(extract: &Path, spacing: &Path, piece: &Path, unit: TileId, work: &Path) -> Result<UnitLinks> {
    std::fs::create_dir_all(work)?;
    let mut c = store::spawn::command(extract);
    c.arg(work).arg(spacing).arg(piece);
    c.stderr(std::process::Stdio::null());
    run(c, &format!("extract {}", piece.display()))?;
    let wv = Ways::open(work)?;
    let strings = roadcore::read_strings(work)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let tb = tile_bounds(unit.z, unit.x, unit.y);
    let inside = |p: [i32; 2]| p[0] >= tb[0] && p[0] < tb[2] && p[1] >= tb[1] && p[1] < tb[3];
    let mut names = Interner::default();
    let mut refs = Interner::default();
    let mut pool: Vec<u32> = Vec::new();
    let mut links: Vec<Link> = Vec::with_capacity(ways.len());
    for w in ways {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let (ends, heading, len) = chain::shape(&verts[r]);
        let kind = if w.vcount < 2 || w.class == class::FERRY {
            chain::KIND_NONE
        } else if class::is_rail(w.class) {
            chain::KIND_RAIL
        } else {
            chain::KIND_ROAD
        };
        let (name, (refs_at, refs_len)) = if kind == chain::KIND_RAIL {
            let raw = if w.name != 0 {
                strings[w.name as usize].as_str()
            } else {
                strings[w.route as usize].split(" · ").next().unwrap_or("")
            };
            (names.id(raw.split(':').next().unwrap_or("").trim()), (pool.len() as u32, 0))
        } else {
            (names.id(&strings[w.name as usize]), chain::intern_refs(&strings[w.ref_ as usize], &mut refs, &mut pool))
        };
        links.push(Link {
            id: w.id as u64,
            kind,
            class: w.class,
            oneway: w.flags & flag::ONEWAY != 0,
            name,
            refs_at,
            refs_len,
            ends,
            heading,
            len,
        });
    }
    // Pairs at this z6 tile's nodes only: the extract holds every way through them.
    let partner = chain::pair(&links, &pool, &inside);
    let mut out = UnitLinks::default();
    for (i, l) in links.iter().enumerate() {
        if inside(l.ends[0]) {
            out.ways.push((l.id, l.len));
        }
        for e in 0..2 {
            let p = partner[i][e];
            if p != chain::NONE && inside(l.ends[e]) {
                let (j, f) = ((p >> 1) as usize, (p & 1) as u64);
                let a = l.id << 1 | e as u64;
                let b = links[j].id << 1 | f;
                if a < b {
                    out.pairs.push((a, b));
                }
            }
        }
    }
    std::fs::remove_dir_all(work).ok();
    Ok(out)
}

/// The worldwide walk over every z6 tile's chaining inputs: road values by OSM way id.
/// The worldwide walk over every z6 tile's chaining inputs: the ways (OSM id, length), sorted by id,
/// and each one's road value in the same order. Each z6 tile's pairs are dropped once used: at planet
/// scale (some 250 M ways) the walk then stays near 20 GB.
pub fn walk_all(units: &mut [UnitLinks]) -> (Vec<(u64, f32)>, Vec<RoadVal>) {
    let mut ways: Vec<(u64, f32)> = units.iter().flat_map(|u| u.ways.iter().copied()).collect();
    ways.par_sort_unstable_by_key(|w| w.0);
    ways.dedup_by_key(|w| w.0);
    let index = |id: u64| ways.binary_search_by_key(&id, |w| w.0).ok();
    let mut partner = vec![[chain::NONE; 2]; ways.len()];
    for u in units.iter_mut() {
        for &(a, b) in &std::mem::take(&mut u.pairs) {
            let (Some(ia), Some(ib)) = (index(a >> 1), index(b >> 1)) else { continue };
            partner[ia][(a & 1) as usize] = (ib as u32) << 1 | (b & 1) as u32;
            partner[ib][(b & 1) as usize] = (ia as u32) << 1 | (a & 1) as u32;
        }
    }
    // Every way gets a road, of its own when it pairs with nothing.
    let vals = chain::walk_by(ways.len(), |i| ways[i].0, |i| ways[i].1, |_| true, &partner);
    (ways, vals)
}

/// Stage list, in order.
pub const STAGES: &[&str] = &["copy", "filter", "sets", "basemap", "cut", "roads"];

/// The pass's parts, for the status (crate::agent::jobs::part): each says how far it is.
const PARTS: [&str; 9] = [
    "Copying the planet here from the NAS",
    "Filtering the planet to the build's tags (osmium)",
    "Uploading the filtered planet to the NAS",
    "Making the worldwide sets (osmium)",
    "Filtering the basemap's input (osmium)",
    "Drawing the worldwide basemap (Planetiler)",
    "Cutting the planet into z6 tiles",
    "Reading the z6 tiles' road links",
    "Walking the world's roads and writing their values",
];

/// What each part of the pass mostly waits on (crate::timings: its phase's class; a mixed part's
/// sub-phases say).
const PART_CLASSES: [crate::timings::Class; 9] = {
    use crate::timings::Class::*;
    [NasRead, Compute, NasWrite, Mixed, Compute, Compute, Mixed, Disk, Mixed]
};

/// The part under way's phase (crate::timings), ended as the next begins (`part`) or the pass ends
/// (`end_part`).
static PART: std::sync::Mutex<Option<crate::timings::Phase>> = std::sync::Mutex::new(None);

/// Part `i` of the pass begins (`PARTS`), its progress one item's (`one`) until it says its own.
fn part(i: usize, one: &str) {
    end_part();
    *PART.lock().unwrap() = Some(crate::timings::phase(one, PART_CLASSES[i]));
    crate::agent::jobs::part(i, &PARTS);
    crate::agent::jobs::stage(0, 1, one);
}

/// The part under way's phase ended.
fn end_part() {
    let p = PART.lock().unwrap().take();
    drop(p);
}

/// The newest pass before `date`'s with outlines in `manifest`: its date and their content name.
pub fn previous_outlines<'a>(
    manifest: &'a std::collections::BTreeMap<String, String>,
    date: &str,
) -> Option<(&'a str, &'a str)> {
    manifest
        .iter()
        .filter_map(|(k, v)| match k.split('/').collect::<Vec<_>>()[..] {
            ["sources", "osm", d, "outlines"] if d < date => Some((d, v.as_str())),
            _ => None,
        })
        .max_by_key(|(d, _)| *d)
}

/// The pass's administrative and ISO 3166 outlines (crate::outlines::assemble) from its outline
/// set, as `sources/osm/<date>/outlines`: the outlines step's work (crate::agent::build::
/// outlines_work, keyed on the set, the previous outlines and `OUTLINES_V`). The set is copied
/// into `scratch` first (osmium reads it twice); the previous pass's outlines
/// (`previous_outlines`) are what a country this planet can't assemble keeps.
pub fn make_outlines(out: &mut Out, date: &str, scratch: &Path) -> Result<crate::outlines::Summary> {
    use crate::timings::{phase, Class};
    std::fs::create_dir_all(scratch)?;
    let nas = out.path(out.get(&set_name(date, "outlines")).context("the pass's outline set")?);
    let set = scratch.join("set-outlines.osm.pbf");
    let p = phase("the outline set copied here", Class::NasRead);
    p.count(nas.copy_data_to_local(&set).with_context(|| format!("copy {}", nas.display()))?, 1);
    drop(p);
    let file = scratch.join("outlines.sect");
    let previous = previous_outlines(&out.manifest, date).map(|(d, c)| (out.path(c), d.to_string()));
    let p = phase("assembled", Class::Compute);
    let s = crate::outlines::assemble(
        &set,
        &scratch.join("outlines-work"),
        &file,
        previous.as_ref().map(|(p, d)| (p.into(), d.as_str())),
    )?;
    drop(p);
    std::fs::remove_file(&set).ok();
    eprintln!(
        "outlines: {} ({} points, {} simplified); by level {:?}",
        s.outlines, s.points, s.simplified_points, s.by_level
    );
    for (id, iso, gap) in &s.repaired {
        eprintln!("outlines: relation {id} ({iso}) left out by osmium's export, assembled here (widest gap closed {gap:.0} m)");
    }
    for (id, iso, why, from) in &s.stale {
        eprintln!("outlines: WARNING: relation {id} ({iso}) can't be assembled from this planet ({why}): kept as the {from} pass's outline (stale)");
    }
    if !s.unassembled.is_empty() {
        eprintln!(
            "outlines: warning: {} relations below country level left out (their rings don't close in OSM): {:?}",
            s.unassembled.len(),
            s.unassembled
        );
    }
    let p = phase("uploaded", Class::NasWrite);
    out.put_file(&format!("sources/osm/{date}/outlines"), "sect", &file)?;
    drop(p);
    out.save()?;
    std::fs::remove_file(&file).ok();
    Ok(s)
}

/// Run (or resume) the pass for `date` from `planet` (on the NAS).
pub fn run_pass(
    out: &mut Out,
    planet: &Path,
    date: &str,
    scratch: &Path,
    extract_bin: &Path,
    planetiler: &Path,
) -> Result<()> {
    use crate::agent::jobs::{report, stage};
    std::fs::create_dir_all(scratch)?;
    let local_planet = scratch.join("planet.osm.pbf");
    let filtered = scratch.join("filtered.osm.pbf");
    // The planet is read by the filter (twice): copied first when there's room for it (resumable),
    // else streamed off the NAS over SSH (`streamed`; an interruption then repeats the filter).
    let planet_len = std::fs::metadata(planet).map(|m| m.len()).unwrap_or(u64::MAX);
    // (Every room it reckons is past the owner's room target, which stays free: the agent tells
    // it, room::TARGET_ENV; run by hand, none.)
    let target = crate::agent::room::job_target();
    let room = crate::agent::room::free_past(scratch, target);
    let copy_first = done(scratch, "copy").exists() || room > copy_room(planet_len);
    if copy_first && !done(scratch, "copy").exists() && !done(scratch, "filter").exists() {
        part(0, "the planet copied");
        copy_resume_with(planet, &local_planet, &mut |d, t| report(d >> 20, t >> 20, "MB of the planet copied"))?;
        mark(scratch, "copy")?;
    }
    if !done(scratch, "filter").exists() {
        part(1, "the planet filtered (osmium)");
        let src = if copy_first { Input::Local(local_planet.clone()) } else { Input::Nas(planet.to_path_buf()) };
        tags_filter(&src, &filtered, FILTER_A, scratch, "osmium tags-filter (the pipeline's tags)", None)?;
        part(2, "the filtered planet uploaded");
        out.put_file_with(
            &format!("sources/osm/{date}/filtered"),
            "osm.pbf",
            &copy_keep(&filtered, scratch)?,
            &|d, t| report(d >> 20, t >> 20, "MB of the filtered planet moved (read, sent, checked)"),
        )?;
        out.save()?;
        std::fs::remove_file(&local_planet).ok();
        mark(scratch, "filter")?;
    }
    if !done(scratch, "sets").exists() {
        part(3, "sets made");
        for (k, (name, _, exprs)) in SETS.iter().enumerate() {
            stage(k as u64, SETS.len() as u64, &format!("sets made ({name} now)"));
            let o = scratch.join(format!("set-{name}.osm.pbf"));
            let src = Input::Local(filtered.clone());
            tags_filter(&src, &o, exprs, scratch, &format!("osmium tags-filter (set {name})"), Some("sets filtered"))?;
            let _p = crate::timings::sub("sets uploaded", crate::timings::Class::NasWrite);
            out.put_file(&set_name(date, name), "osm.pbf", &o)?;
            std::fs::remove_file(&o).ok();
        }
        report(SETS.len() as u64, SETS.len() as u64, "sets made");
        out.save()?;
        mark(scratch, "sets")?;
    }
    // Sets added or changed since this pass began (a pass resumed by a newer app): made now, from
    // the same filtered file (the local copy while it's there).
    if !done(scratch, "sets-added").exists() {
        if !missing_sets(out, date).is_empty() {
            part(3, "sets made");
            let src =
                if filtered.exists() { Input::Local(filtered.clone()) } else { Input::Nas(filtered_nas(out, date)?) };
            make_missing_sets(out, date, &src, scratch)?;
        }
        mark(scratch, "sets-added")?;
    }
    // (The outlines are their own step, from the outline set uploaded above: `make_outlines`,
    // crate::agent::build::outlines_work.)
    // From here the filtered file is read twice more (the basemap's filter, the first cut): from
    // the local copy while there's room, else from the NAS's.
    let free = |p: &Path| crate::agent::room::free_past(p, target);
    if !done(scratch, "basemap").exists() {
        check_planetiler(planetiler)?;
        part(4, "the basemap's input filtered (osmium)");
        if filtered.exists() && free(scratch) < BASEMAP_ROOM {
            eprintln!(
                "basemap: {} GB free past the room target; reading the filtered planet from the NAS",
                free(scratch) >> 30
            );
            std::fs::remove_file(&filtered)?;
        }
        let b = scratch.join("basemap-input.osm.pbf");
        // (Kept until the basemap is made: a failed Planetiler run doesn't filter the planet again.)
        if !(b.exists() && done(scratch, "basemap-input").exists()) {
            let src =
                if filtered.exists() { Input::Local(filtered.clone()) } else { Input::Nas(filtered_nas(out, date)?) };
            tags_filter(&src, &b, FILTER_BASEMAP, scratch, "osmium tags-filter (basemap)", None)?;
            mark(scratch, "basemap-input")?;
        }
        part(5, "the basemap drawn (Planetiler)");
        // Planetiler's room: the local filtered file goes first (the cut reads the NAS's copy then).
        let need = planetiler_room(std::fs::metadata(&b)?.len());
        if free(scratch) < need && filtered.exists() {
            eprintln!(
                "basemap: {} GB free past the room target, Planetiler needs ~{} GB; the cut will read the filtered planet from the NAS",
                free(scratch) >> 30,
                need >> 30
            );
            std::fs::remove_file(&filtered)?;
        }
        // (A run that failed may have left its temporary files.)
        std::fs::remove_dir_all(scratch.join("planetiler-tmp")).ok();
        ensure!(
            free(scratch) >= need,
            "basemap: Planetiler needs ~{} GB free on this Mac past its room target, {} GB are",
            need >> 30,
            free(scratch) >> 30
        );
        let pm = scratch.join("basemap.pmtiles");
        let downloads = out.root().join("sources/basemap");
        downloads.create_dir_all()?;
        // Run in the scratch folder, its files named relative to it: Planetiler reads `--output` as
        // a URI, which a space in the path ("Application Support") breaks.
        let mut j = store::spawn::command("/opt/homebrew/opt/openjdk@21/bin/java");
        j.current_dir(scratch);
        j.args(["-Xmx24g", "-jar"]).arg(planetiler).args([
            "--download",
            "--storage=mmap",
            "--force",
            "--nodemap-type=sparsearray",
            "--only-layers=water,waterway,boundary,place,water_name,park",
            // The names' languages and their romanised forms and kana readings, for the thing's own
            // English and the languages OSM gives its name (docs/names.md); the 2026-09-28 pass's
            // basemap has en and fr alone.
            &format!("--languages={}", BASEMAP_LANGUAGES),
            "--maxzoom=14",
        ]);
        j.arg(format!("--download-dir={}", downloads.display()));
        j.arg("--tmpdir=planetiler-tmp");
        j.arg("--osm-path=basemap-input.osm.pbf");
        j.arg("--output=basemap.pmtiles");
        run_planetiler(j, "planetiler (basemap)")?;
        end_part();
        let _p = crate::timings::phase("the basemap uploaded", crate::timings::Class::NasWrite);
        out.put_file(&format!("layers/basemap/world-{date}"), "pmtiles", &pm)?;
        out.save()?;
        std::fs::remove_file(&b).ok();
        std::fs::remove_file(done(scratch, "basemap-input")).ok();
        std::fs::remove_dir_all(scratch.join("planetiler-tmp")).ok();
        mark(scratch, "basemap")?;
    }
    let tree = scratch.join("cut");
    let links = scratch.join("links");
    // (The road links spaced as the z6 tiles' extract spaces them: crate::dem::spacing.)
    let spacing = scratch.join(crate::dem::spacing::FILE);
    crate::dem::spacing::Fine::of_records(out.root(), &out.manifest)?.write(&spacing)?;
    if !done(scratch, "cut").exists() {
        // The first cut writes about the filtered file again (its four quarters); below it each
        // extract goes as soon as it's cut, so the tree stays near that size. The local filtered
        // file is kept for the first cut only when there's room for both, and goes after it.
        let flen = std::fs::metadata(&filtered)
            .or_else(|_| std::fs::metadata(filtered_nas(out, date)?).map_err(anyhow::Error::from))?
            .len();
        if filtered.exists() && !tree.join("0-0-0.cut").exists() && free(scratch) < flen / 8 * 10 + (20 << 30) {
            eprintln!(
                "cut: {} GB free past the room target; reading the filtered planet from the NAS",
                free(scratch) >> 30
            );
            std::fs::remove_file(&filtered)?;
        }
        let local = filtered.exists();
        let path = if local { filtered.clone() } else { filtered_nas(out, date)? };
        let work = scratch.join("extract-work");
        // Progress by data: the extracts' sizes against the filtered planet's (their buffers overlap a
        // little, so it's capped just short of the end), the extracts uploaded before a restart too.
        let total_mb = std::fs::metadata(&path)?.len() >> 20;
        let prefix = format!("sources/osm/{date}/pieces/");
        let mut done_mb: u64 = out
            .manifest
            .range(prefix.clone()..)
            .take_while(|(l, _)| l.starts_with(&prefix))
            .filter_map(|(_, c)| out.path(c).metadata().ok())
            .map(|m| m.len() >> 20)
            .sum();
        part(6, "the planet cut into z6 tiles");
        let cut_report = |mb: u64| report(mb.min(total_mb.saturating_sub(1)), total_mb, "MB cut into z6 tiles");
        cut_report(done_mb);
        let src = if local { Input::Local(path) } else { Input::Nas(path) };
        cut_tree(&src, TileId { z: 0, x: 0, y: 0 }, local, &tree, &mut |u, f| {
            let logical = format!("sources/osm/{date}/pieces/{}", u.dash());
            if out.get(&logical).is_none() {
                let mb = std::fs::metadata(f).map(|m| m.len() >> 20).unwrap_or(0);
                let _p = crate::timings::sub("pieces uploaded", crate::timings::Class::NasWrite);
                out.put_file(&logical, "osm.pbf", &copy_keep(f, scratch)?)?;
                out.save()?;
                done_mb += mb;
                cut_report(done_mb);
            }
            let lf = links.join(format!("{}.bin", u.dash()));
            if !lf.exists() {
                let _p = crate::timings::sub("their road links worked out (extract)", crate::timings::Class::Compute);
                let ul = unit_links(extract_bin, &spacing, f, u, &work)?;
                eprintln!("roads: {} {} ways, {} pairs", u.slash(), ul.ways.len(), ul.pairs.len());
                ul.save(&lf)?;
            }
            Ok(())
        })?;
        let mut pieces = Pieces { date: date.to_string(), ..Default::default() };
        let prefix = format!("sources/osm/{date}/pieces/");
        for k in out.manifest.keys().filter(|k| k.starts_with(&prefix)) {
            if let Some(u) = TileId::parse(&k[prefix.len()..]) {
                pieces.pieces.insert(u.slash(), k.clone());
            }
        }
        out.put_bytes(&format!("sources/osm/{date}/pieces"), "json", &serde_json::to_vec_pretty(&pieces)?)?;
        out.save()?;
        // Marked first: the clean-up below is safe to repeat, the cut isn't cheap to.
        mark(scratch, "cut")?;
    }
    end_part();
    std::fs::remove_dir_all(&tree).ok();
    std::fs::remove_file(&filtered).ok();
    let name = out.get(&format!("sources/osm/{date}/pieces")).context("pieces list")?.to_string();
    let pieces: Pieces = serde_json::from_slice(&out.path(&name).read()?)?;
    if !done(scratch, "roads").exists() {
        // Every z6 tile's chaining inputs (kept by the cut, else worked out from its extract), then the
        // worldwide walk; values sliced by owner z6 tile.
        let mut all: Vec<(TileId, UnitLinks)> = Vec::new();
        let n = pieces.pieces.len() as u64;
        part(7, "z6 tiles' road links read");
        for (k, (u, _)) in pieces.pieces.iter().enumerate() {
            report(k as u64, n, "z6 tiles' road links read");
            let unit = TileId::parse(u).context("unit")?;
            let lf = links.join(format!("{}.bin", unit.dash()));
            let ul = match UnitLinks::load(&lf) {
                Ok(ul) => ul,
                Err(_) => {
                    let src =
                        out.path(out.get(&format!("sources/osm/{date}/pieces/{}", unit.dash())).context("piece")?);
                    // (The extract program's input, read where it lies: tests/nas_access.rs.)
                    unit_links(extract_bin, &spacing, &src.unchecked_path(), unit, &scratch.join("extract-work"))?
                }
            };
            all.push((unit, ul));
        }
        report(n, n, "z6 tiles' road links read");
        let (units, mut lists): (Vec<TileId>, Vec<UnitLinks>) = all.into_iter().unzip();
        part(8, "the roads walked");
        let p = crate::timings::sub("walked", crate::timings::Class::Compute);
        let (ways, vals) = walk_all(&mut lists);
        drop(p);
        let _p = crate::timings::sub("their values uploaded", crate::timings::Class::NasWrite);
        for (k, (unit, ul)) in units.iter().zip(&lists).enumerate() {
            report(k as u64, n, "z6 tiles' road values written");
            let mut recs: Vec<u8> = Vec::with_capacity(ul.ways.len() * 32);
            let mut ids: Vec<u64> = ul.ways.iter().map(|w| w.0).collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let Ok(k) = ways.binary_search_by_key(&id, |w| w.0) else { continue };
                let v = vals[k];
                // (u64 way id, then the RoadRec layout)
                recs.extend_from_slice(&id.to_le_bytes());
                recs.extend_from_slice(bytemuck::bytes_of(&roadcore::packs::RoadRec {
                    road: v.road,
                    len: v.len,
                    offset: v.offset,
                    dir: v.dir,
                    _pad: [0; 7],
                }));
            }
            out.put_bytes(&format!("sources/osm/{date}/roads/{}", unit.dash()), "bin", &recs)?;
        }
        report(n, n, "z6 tiles' road values written");
        out.save()?;
        mark(scratch, "roads")?;
    }
    end_part();
    if done(scratch, "roads").exists() {
        std::fs::remove_dir_all(&links).ok();
    }
    // The pass is complete: its summary marks it so (the agent's `pass_done`).
    if out.get(&format!("sources/osm/{date}/pass")).is_none() {
        // (`units`: the pass summary's stored key for its count of z6 tile extracts: kept.)
        let summary = serde_json::json!({ "date": date, "units": pieces.pieces.len() });
        out.put_bytes(&format!("sources/osm/{date}/pass"), "json", &serde_json::to_vec_pretty(&summary)?)?;
        out.save()?;
    }
    let n = retire_older(out, date);
    if n > 0 {
        out.save()?;
        eprintln!("osm-pass: {n} entries of older passes retired");
    }
    Ok(())
}

/// A pass date (YYYY-MM-DD).
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// Removes the manifest's entries of passes older than `date` (their planet's extracts, sets, road
/// values and outlines; the summits, route ends, items and heritage made from them) once `date`'s
/// pass is complete: every job reads the newest pass, so nothing reads them again. (Their files
/// go with GC: its sweep of retired passes' sources, agent::gc.) How many went.
pub fn retire_older(out: &mut Out, date: &str) -> usize {
    // (The newest older pass's outlines stay: what `date`'s outlines step reads and is keyed on,
    // the outlines a country this planet can't assemble keeps. They go with the next pass.)
    let previous = previous_outlines(&out.manifest, date).map(|(d, _)| format!("sources/osm/{d}/outlines"));
    let old: Vec<String> = out
        .manifest
        .keys()
        .filter(|l| Some(l.as_str()) != previous.as_deref())
        .filter(|l| {
            let s: Vec<&str> = l.split('/').collect();
            match s.as_slice() {
                ["sources", "osm", d, _, ..]
                | ["sources", "items", d, ..]
                | ["work", "heritage", d, ..]
                | ["work", "summits", d]
                | ["work", "trailends", d] => is_date(d) && *d < date,
                _ => false,
            }
        })
        .cloned()
        .collect();
    for l in &old {
        out.remove(l);
    }
    old.len()
}

/// Whether the pass from the planet of `date` is complete on the NAS (its `pass.<hash>.json`).
pub fn pass_done(root: &store::nasfs::NasRoot, date: &str) -> bool {
    root.join("sources/osm")
        .join(date)
        .read_dir()
        .map(|rd| rd.iter().any(|e| e.name.starts_with("pass.") && e.name.ends_with(".json")))
        .unwrap_or(false)
}

/// A job's environment: a pass is complete when its records say so (`latest_pass`; the agent sets
/// it with `state/pool/resume` on: crate::agent::resume).
pub const RESUME_ENV: &str = "SCENIC_RESUME";

/// The date of the newest complete pass: by the records when the job's agent says so
/// (`RESUME_ENV`), else by the `pass.*.json` files on the NAS (`latest_pass_by`).
pub fn latest_pass(root: &store::nasfs::NasRoot) -> Option<String> {
    latest_pass_by(root, std::env::var_os(RESUME_ENV).is_some())
}

/// The date of the newest complete pass: with `records`, the newest whose `sources/osm/<date>/pass`
/// the merged records name (today's manifest, which the lead writes from them: docs/pool.md §7.4),
/// none when they can't be read now; else the newest with a `pass.*.json` file on the NAS.
pub fn latest_pass_by(root: &store::nasfs::NasRoot, records: bool) -> Option<String> {
    if records {
        let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).ok()?;
        return latest_pass_in(&m);
    }
    latest_pass_on_nas(root)
}

/// The date of the newest pass manifest `m` names complete (`sources/osm/<date>/pass`).
pub fn latest_pass_in(m: &BTreeMap<String, String>) -> Option<String> {
    m.range("sources/osm/".to_string()..)
        .take_while(|(l, _)| l.starts_with("sources/osm/"))
        .filter_map(|(l, _)| l.strip_prefix("sources/osm/")?.strip_suffix("/pass").filter(|d| is_date(d)))
        .max()
        .map(str::to_string)
}

/// The date of the newest complete pass on the NAS, by its `pass.*.json` file.
fn latest_pass_on_nas(root: &store::nasfs::NasRoot) -> Option<String> {
    let dir = root.join("sources/osm");
    let mut dates: Vec<String> = dir
        .read_dir()
        .ok()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.len() == 10 && n.as_bytes()[4] == b'-' && pass_done(root, n))
        .collect();
    dates.sort();
    dates.pop()
}

/// A copy of a scratch file for `put_file`, which consumes its input.
fn copy_keep(p: &Path, scratch: &Path) -> Result<PathBuf> {
    let d = scratch.join(format!("upload-{}", p.file_name().unwrap().to_string_lossy()));
    // A clone on APFS: instant, no extra space.
    let st = store::spawn::status(store::spawn::command("cp").arg("-c").arg(p).arg(&d))?;
    if !st.success() {
        store::sys::copy_data(p, &d)?;
    }
    Ok(d)
}

/// Whether the NAS has a planet newer than `have` (a pass date), and its path and date.
pub fn newer_planet(
    root: &store::nasfs::NasRoot,
    have: Option<&str>,
) -> Result<Option<(store::nasfs::NasPath, String)>> {
    let dir = root.join("sources/osm");
    let mut dates: Vec<String> = dir
        .read_dir()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.len() == 10 && n.as_bytes()[4] == b'-' && dir.join(n).join("planet.osm.pbf").exists())
        .collect();
    dates.sort();
    let Some(last) = dates.pop() else { return Ok(None) };
    if have.is_some_and(|h| h >= last.as_str()) {
        return Ok(None);
    }
    Ok(Some((dir.join(&last).join("planet.osm.pbf"), last)))
}

/// Puts into the pass's extracts the ferry ways its filtered planet lacks. The 2026-09-28 pass's was
/// filtered without `w/route=ferry`: it has a ferry way only where another of the filter's tags
/// kept it (a route relation's member, a `wikidata` tag, …), so its extracts lack the rest
/// (Kobe–Miyazaki, Tanger–Tarifa, …). Its ferries set is the planet's own, with them. Each extract
/// gets the set's ferry ways it meets as the cut kept ways (its tile and buffer, the smart
/// strategy: whole, with their nodes) and lacks, merged in (an object both have, at the one
/// version the planet has, is written once).
/// An extract that lacks none stays as it is, so only the z6 tiles whose extracts change are built again,
/// and a run after it changes nothing. Only the extracts of the z6 tiles `only` takes. The extracts changed
/// and the ways they gained. The records are saved once, at the end: a extract changed goes stale the
/// roads' reach, and with it the z6 tiles' planning on each Mac, so saving each as it changed would
/// hold the z6 tiles up for the whole run (a run stopped part way saves none, and is run again).
pub fn patch_ferries(
    out: &mut Out,
    date: &str,
    scratch: &Path,
    only: &dyn Fn(TileId) -> bool,
) -> Result<(usize, usize)> {
    let set = out.path(out.get(&set_name(date, "ferries")).context("the pass's ferries set")?);
    let work = scratch.join("patch-ferries");
    std::fs::create_dir_all(&work)?;
    let local_set = work.join("ferries.osm.pbf");
    set.copy_data_to_local(&local_set)?;
    let prefix = format!("sources/osm/{date}/pieces/");
    let pieces: Vec<(TileId, String, String)> = out
        .manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .filter_map(|(l, c)| Some((TileId::parse(&l[prefix.len()..])?, l.clone(), c.clone())))
        .filter(|(u, _, _)| only(*u))
        .collect();
    let (mut changed, mut gained) = (0, 0);
    for (k, (u, logical, content)) in pieces.iter().enumerate() {
        crate::agent::jobs::report(k as u64, pieces.len() as u64, "pieces checked for ferries");
        let near = work.join("near.osm.pbf");
        let b = grow(tile_bounds(u.z, u.x, u.y), BUFFER_KM);
        let d = |v: i32| v as f64 * 1e-7;
        let mut c = osmium();
        c.args(["extract", "--no-progress", "--strategy", "smart", "-S", "types=multipolygon", "--overwrite", "-b"])
            .arg(format!("{},{},{},{}", d(b[0]), d(b[1]), d(b[2]), d(b[3])))
            .arg(&local_set)
            .arg("-o")
            .arg(&near);
        quiet(c, "osmium extract (the ferries near a piece)")?;
        let ids = way_ids(&near, None)?;
        if ids.is_empty() {
            continue;
        }
        // (Read where it is: copied only when it lacks some.)
        // (osmium's input, read where it lies: tests/nas_access.rs.)
        let lacks = ids.len() - way_ids(&out.path(content).unchecked_path(), Some(&ids))?.len();
        if lacks == 0 {
            continue;
        }
        let piece = work.join("piece.osm.pbf");
        out.path(content).copy_data_to_local(&piece)?;
        let merged = work.join(format!("{}.osm.pbf", u.dash()));
        let mut c = osmium();
        c.args(["merge", "--no-progress", "--overwrite", "--output-header", "sorting=Type_then_ID", "-o"])
            .arg(&merged)
            .arg(&piece)
            .arg(&near);
        quiet(c, "osmium merge (a piece and its ferries)")?;
        out.put_file(logical, "osm.pbf", &merged)?;
        eprintln!("ferries: {} lacked {lacks} of the {} ferry ways it meets", u.slash(), ids.len());
        (changed, gained) = (changed + 1, gained + lacks);
    }
    out.save()?;
    std::fs::remove_dir_all(&work).ok();
    Ok((changed, gained))
}

/// The ids of the ways in an OSM file, or of those among `among` (osmium getid).
fn way_ids(file: &Path, among: Option<&std::collections::BTreeSet<i64>>) -> Result<std::collections::BTreeSet<i64>> {
    let mut c = osmium();
    match among {
        None => {
            c.args(["cat", "--no-progress", "-t", "way", "-f", "opl"]).arg(file);
        }
        Some(ids) => {
            let list = std::env::temp_dir().join(format!("scenic-way-ids-{}", std::process::id()));
            std::fs::write(&list, ids.iter().map(|i| format!("w{i}\n")).collect::<String>())?;
            c.args(["getid", "--no-progress", "-f", "opl", "-i"]).arg(&list).arg(file);
        }
    }
    let o = store::spawn::output(&mut c).context("run osmium");
    if among.is_some() {
        std::fs::remove_file(std::env::temp_dir().join(format!("scenic-way-ids-{}", std::process::id()))).ok();
    }
    let o = o?;
    // (getid exits 1, saying nothing, when some of the ids aren't there: the answer, not a failure;
    // a file it can't read says why.)
    let quiet_miss = among.is_some() && o.status.code() == Some(1) && o.stderr.iter().all(u8::is_ascii_whitespace);
    ensure!(
        o.status.success() || quiet_miss,
        "osmium on {} failed: {}: {}",
        file.display(),
        o.status,
        String::from_utf8_lossy(&o.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix('w')?.split(' ').next()?.parse().ok())
        .collect())
}

/// Runs a command whose output isn't wanted, failing with its name.
fn quiet(mut c: Command, what: &str) -> Result<()> {
    let o = store::spawn::output(&mut c).with_context(|| format!("run {what}"))?;
    ensure!(o.status.success(), "{what} failed: {}: {}", o.status, String::from_utf8_lossy(&o.stderr).trim());
    Ok(())
}

/// The `version=` of a Planetiler jar's `buildinfo.properties`.
fn buildinfo_version(props: &str) -> Option<&str> {
    props.lines().find_map(|l| l.trim().strip_prefix("version=")).map(str::trim)
}

/// Stops on a Planetiler jar other than the one the water layer was checked against
/// (pipeline::water::PLANETILER_VERSION: its own `buildinfo.properties` says which it is), so a
/// new one's basemap isn't drawn until its z14 water is checked again as full detail.
pub fn check_planetiler(jar: &Path) -> Result<()> {
    let o =
        store::spawn::output(store::spawn::command("/usr/bin/unzip").arg("-p").arg(jar).arg("buildinfo.properties"))
            .with_context(|| format!("read {}", jar.display()))?;
    ensure!(o.status.success(), "no buildinfo.properties in {}", jar.display());
    let props = String::from_utf8_lossy(&o.stdout);
    let v =
        buildinfo_version(&props).with_context(|| format!("no version in {}'s buildinfo.properties", jar.display()))?;
    let pinned = crate::water::PLANETILER_VERSION;
    ensure!(v == pinned, "{} is Planetiler {v}, but the water layer was checked against Planetiler {pinned}'s z14 water (pipeline::water, docs/steps.md): check {v}'s with tools/coastcheck, then pin it", jar.display());
    Ok(())
}

pub fn check_tools(extract_bin: &Path, planetiler: &Path) -> Result<()> {
    if !extract_bin.exists() {
        bail!("no extract binary at {}", extract_bin.display());
    }
    if !planetiler.exists() {
        bail!("no Planetiler at {}", planetiler.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_previous_outlines_are_the_newest_before() {
        let m: std::collections::BTreeMap<String, String> = [
            ("sources/osm/2026-08-01/outlines", "a"),
            ("sources/osm/2026-09-01/outlines", "b"),
            ("sources/osm/2026-09-28/outlines", "c"),
            ("sources/osm/2026-09-15/sets/outlines", "x"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(previous_outlines(&m, "2026-09-28"), Some(("2026-09-01", "b")));
        assert_eq!(previous_outlines(&m, "2026-10-26"), Some(("2026-09-28", "c")));
        assert_eq!(previous_outlines(&m, "2026-08-01"), None);
    }

    /// A pass complete by its records (docs/pool.md §7.4): the newest the manifest names complete,
    /// whatever `pass.*.json` files the NAS holds; by the files, as before.
    #[test]
    fn a_pass_is_complete_by_its_records() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("state/build")).unwrap();
        for (date, file) in [("2026-09-01", true), ("2026-10-01", true), ("2026-11-01", false)] {
            std::fs::create_dir_all(r.join("sources/osm").join(date)).unwrap();
            if file {
                std::fs::write(r.join(format!("sources/osm/{date}/pass.1111111111111111.json")), "{}").unwrap();
            }
        }
        // (2026-10-01's pass file is there, its entry not merged yet; 2026-09-01's in the records.)
        std::fs::write(r.join("state/build/manifest.json"), r#"{"sources/osm/2026-09-01/pass": "sources/osm/2026-09-01/pass.1111111111111111.json", "sources/osm/2026-09-01/pieces/6-1-1": "p", "sources/osm/2026-12-01/sets/rail": "s", "sources/osm/x/pass": "y"}"#).unwrap();
        assert_eq!(latest_pass_by(&store::nasfs::NasRoot::local_for_test(r), false).as_deref(), Some("2026-10-01"));
        assert_eq!(latest_pass_by(&store::nasfs::NasRoot::local_for_test(r), true).as_deref(), Some("2026-09-01"));
        assert_eq!(latest_pass_in(&BTreeMap::new()), None);
        // When the records say what the files do, the same.
        std::fs::write(
            r.join("state/build/manifest.json"),
            r#"{"sources/osm/2026-09-01/pass": "a", "sources/osm/2026-10-01/pass": "b"}"#,
        )
        .unwrap();
        assert_eq!(
            latest_pass_by(&store::nasfs::NasRoot::local_for_test(r), true),
            latest_pass_by(&store::nasfs::NasRoot::local_for_test(r), false)
        );
    }

    /// A pass stopped after its filter, resumed by its Mac before the lead merged that stage's
    /// entry: its marker says the stage is done, and the snapshot with this Mac's unmerged
    /// hand-offs laid over it (crate::agent::resume) names the filtered planet the next stages read.
    #[test]
    fn a_pass_resumed_finds_its_earlier_stages_unmerged() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), "{}").unwrap();
        mark(&scratch, "filter").unwrap();
        let date = "2026-10-01";
        let earlier = crate::handoff::Handoff {
            changes: [(
                format!("sources/osm/{date}/filtered"),
                Some(format!("sources/osm/{date}/filtered.2222222222222222.osm.pbf")),
            )]
            .into(),
            ..Default::default()
        };
        let f = d.path().join("unmerged.json");
        std::fs::write(&f, serde_json::to_vec(&earlier).unwrap()).unwrap();
        let fresh = Out::open_with(&store::nasfs::NasRoot::local_for_test(&root), &scratch, None).unwrap();
        assert!(filtered_nas(&fresh, date).is_err(), "without them, the stage's output isn't there to read");
        let resumed = Out::open_with(&store::nasfs::NasRoot::local_for_test(&root), &scratch, Some(&f)).unwrap();
        assert!(done(&scratch, "filter").exists() && !done(&scratch, "sets").exists());
        assert_eq!(
            filtered_nas(&resumed, date).unwrap(),
            root.join(format!("sources/osm/{date}/filtered.2222222222222222.osm.pbf"))
        );
    }

    #[test]
    fn planetilers_version_is_read_from_its_buildinfo() {
        let props = "githash=0e5588c4a6e8c29a270a33afe8df62027d889604\ntimestamp=1774708536815\nversion=0.10.2\n";
        assert_eq!(buildinfo_version(props), Some("0.10.2"));
        assert_eq!(buildinfo_version(props), Some(crate::water::PLANETILER_VERSION));
        assert_eq!(buildinfo_version("githash=x\n"), None);
    }

    #[test]
    fn planetilers_log_says_how_far_it_is() {
        let l = "\u{1b}[m\u{1b}[0m0:07:29 INF [osm_pass2] -  nodes: \u{1b}[32m[ 228M  11% 9.9M/s ]\u{1b}[0m 31G   ways: [    0   0%    0/s ] blocks: [  28k  10% 1.2k/s ]";
        assert!((planetiler_fraction(l).unwrap() - (0.16 + 0.1 * 0.44)).abs() < 1e-9);
        let a = "0:30:43 INF [archive] -  features: \u{1b}[32m[ 3.5M  50% 359k/s ]\u{1b}[0m 71G   tiles: [ 6.6k  668/s ] 42M";
        assert!((planetiler_fraction(a).unwrap() - (0.67 + 0.5 * 0.33)).abs() < 1e-9);
        assert_eq!(planetiler_fraction("0:00:34 INF [lake_centerlines] - Starting..."), Some(0.0));
        assert_eq!(planetiler_fraction("0:04:26 INF [osm_pass1:process] - "), Some(0.1));
        assert_eq!(planetiler_fraction("some other line"), None);
    }

    /// An OSM file from OPL lines.
    fn osm(dir: &Path, name: &str, opl: &str) -> PathBuf {
        let (src, dest) = (dir.join(format!("{name}.opl")), dir.join(format!("{name}.osm.pbf")));
        std::fs::write(&src, opl).unwrap();
        let st =
            store::spawn::status(osmium().args(["cat", "--no-progress", "--overwrite", "-o"]).arg(&dest).arg(&src))
                .unwrap();
        assert!(st.success());
        dest
    }

    /// A fake `ssh` (in the returned folder) that runs its command here: its last argument; with
    /// FAIL set it can't connect, with HANG it never returns (as while 1Password waits for an
    /// approval), with HANG_CAT only its `cat`s don't, with SHORT set a `cat` stops after 50 bytes and exits 0.
    fn fake_ssh(dir: &Path) -> impl Fn(&str) -> Box<dyn Fn() -> Command + Sync> {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("ssh");
        std::fs::write(
            &fake,
            "#!/bin/sh\n[ -n \"$FAIL\" ] && { echo 'Permission denied' >&2; exit 255; }\n[ -n \"$HANG\" ] && exec sleep 600\n\
             for a; do last=$a; done\n\
             case \"$last\" in cat*) [ -n \"$HANG_CAT\" ] && exec sleep 600; [ -n \"$SHORT\" ] && { sh -c \"$last\" | head -c 50; exit 0; };; esac\n\
             exec sh -c \"$last\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        move |mode: &str| {
            let (bin, mode) = (bin.clone(), mode.to_string());
            Box::new(move || {
                let mut c = store::spawn::command_in("ssh", bin.as_os_str());
                if !mode.is_empty() {
                    c.env(&mode, "1");
                }
                c
            })
        }
    }

    /// The stream's `ssh` asks for the AES-GCM-first cipher list.
    #[test]
    fn the_stream_ssh_prefers_aes_gcm() {
        let a: Vec<_> = ssh().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let i = a.iter().position(|x| x == "-c").expect("-c");
        assert!(a[i + 1].starts_with("aes128-gcm@openssh.com,"), "{a:?}");
    }

    /// A filter of a file on the NAS streamed over SSH (`streamed`): osmium reads it twice (the
    /// matches' nodes added), each read counted; an SSH that can't reach the NAS fails clearly,
    /// before osmium runs; a short read fails though `cat` exits 0; an osmium that fails leaves no
    /// feeder waiting.
    #[test]
    fn a_filter_streams_the_nas_file_over_ssh() {
        let d = tempfile::tempdir().unwrap();
        let src = osm(
            d.path(),
            "planet",
            "n1 v1 x1 y1\nn2 v1 x2 y1\nn3 v1 x3 y1\nw10 v1 Twikidata=Q1 Nn1,n2\nw11 v1 Thighway=road Nn2,n3\n",
        );
        let ssh = fake_ssh(d.path());
        let o = d.path().join("out.osm.pbf");
        let len = std::fs::metadata(&src).unwrap().len();
        let remote = src.to_str().unwrap();
        let stream = d.path().join("stream");
        let filter = |input: &Path| {
            let mut c = osmium();
            c.args(["tags-filter", "--no-progress", "--overwrite", "-F", "pbf", "-o"]).arg(&o).arg(input);
            c.arg("w/wikidata");
            run(c, "osmium tags-filter")
        };
        let n = streamed(&*ssh(""), SSH_START, remote, 2, &stream, filter).unwrap();
        assert_eq!(n, 2 * len, "read twice, each read counted");
        let opl = store::spawn::output(osmium().args(["cat", "--no-progress", "-f", "opl"]).arg(&o)).unwrap().stdout;
        let ids: Vec<String> =
            String::from_utf8(opl).unwrap().lines().map(|l| l.split(' ').next().unwrap().to_string()).collect();
        assert_eq!(ids, ["n1", "n2", "w10"]);
        assert!(!stream.exists(), "its pipes gone");
        let e = streamed(&*ssh("FAIL"), SSH_START, remote, 2, &stream, |_| panic!("osmium ran")).unwrap_err();
        assert!(format!("{e:#}").contains("SSH keys"), "{e:#}");
        let e = streamed(&*ssh("SHORT"), SSH_START, remote, 2, &stream, filter).unwrap_err();
        assert!(format!("{e:#}").contains("short read"), "{e:#}");
        let e = streamed(&*ssh(""), SSH_START, remote, 2, &stream, |p| {
            drop(std::fs::File::open(p).unwrap());
            anyhow::bail!("osmium failed")
        })
        .unwrap_err();
        assert!(format!("{e:#}").contains("osmium failed"), "{e:#}");
    }

    /// An `ssh` to the NAS that never starts (its agent waiting for an approval, #638) is stopped
    /// past its bound and fails `TimedOut`, saying why: the size's call, and a `cat` (osmium
    /// waiting on its pipe let go).
    #[test]
    fn a_hung_ssh_to_the_nas_fails_in_time() {
        let d = tempfile::tempdir().unwrap();
        let src = osm(d.path(), "planet", "n1 v1 x1 y1\n");
        let ssh = fake_ssh(d.path());
        let bound = std::time::Duration::from_secs(1);
        for mode in ["HANG", "HANG_CAT"] {
            let t = std::time::Instant::now();
            let e = streamed(&*ssh(mode), bound, src.to_str().unwrap(), 1, &d.path().join("stream"), |p| {
                drop(std::fs::read(p));
                Ok(())
            })
            .unwrap_err();
            store::testkit::ended_before(t, std::time::Duration::from_secs(20), mode);
            assert!(crate::agent::nasread::timed_out(&e), "{mode}: {e:#}");
            assert!(format!("{e:#}").contains("didn't start within 1 s"), "{mode}: {e:#}");
        }
    }

    /// The first cut (`cut`, osmium extract's smart strategy) of a file streamed off the NAS makes
    /// the extracts the local file's cut makes, each of its reads whole.
    #[test]
    fn the_cut_streams_the_nas_file_over_ssh() {
        let d = tempfile::tempdir().unwrap();
        let src = osm(
            d.path(),
            "planet",
            "n1 v1 x1.0 y50.0\nn2 v1 x1.1 y50.0\nn3 v1 x-100 y-40\nn4 v1 x-100.1 y-40\n\
             w10 v1 Thighway=road Nn1,n2\nw11 v1 Thighway=road Nn3,n4\n\
             r20 v1 Ttype=multipolygon Mw10@outer\n",
        );
        let ssh = fake_ssh(d.path());
        let len = std::fs::metadata(&src).unwrap().len();
        let kids: Vec<TileId> = (0..2).flat_map(|x| (0..2).map(move |y| TileId { z: 1, x, y })).collect();
        let (here, there) = (d.path().join("here"), d.path().join("there"));
        cut(&src, &kids, &here, 4).unwrap();
        let n = streamed(&*ssh(""), SSH_START, src.to_str().unwrap(), 0, &d.path().join("stream"), |p| {
            cut(p, &kids, &there, 4)
        })
        .unwrap();
        assert_eq!(n, 4 * len, "four whole reads (osmium 1.19)");
        let opl = |p: &Path| {
            store::spawn::output(osmium().args(["cat", "--no-progress", "-f", "opl"]).arg(p)).unwrap().stdout
        };
        for k in &kids {
            let f = format!("{}.osm.pbf", k.dash());
            assert_eq!(here.join(&f).exists(), there.join(&f).exists(), "{f}");
            if here.join(&f).exists() {
                assert_eq!(opl(&here.join(&f)), opl(&there.join(&f)), "{f}");
            }
        }
    }

    #[test]
    fn pieces_gain_the_ferries_they_lack() {
        let d = tempfile::tempdir().unwrap();
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(d.path()), &d.path().join("scratch")).unwrap();
        // Z6 tile 6/32/21 (lon 0 to 5.6, lat 48.9 to 52.5): a road and a ferry the filter kept (a route
        // relation's member) in its extract; z6 tile 6/40/20's extract has a road only.
        let shared = "n1 v1 x1.0 y50.0\nn2 v1 x1.1 y50.0\nn3 v1 x1.2 y50.1\nn4 v1 x1.5 y50.5\n";
        let piece = osm(
            d.path(),
            "piece",
            &format!("{shared}w10 v1 Thighway=primary Nn1,n2\nw20 v1 Troute=ferry,motor_vehicle=yes Nn2,n3\n"),
        );
        let other = osm(d.path(), "other", "n9 v1 x45.0 y45.0\nn8 v1 x45.1 y45.0\nw90 v1 Thighway=primary Nn9,n8\n");
        // The ferries set: that ferry, and a standalone one from the extract's coast far out to sea.
        let set = osm(d.path(), "set", &format!("{shared}n5 v1 x-3.0 y52.0\nw20 v1 Troute=ferry,motor_vehicle=yes Nn2,n3\nw30 v1 Troute=ferry,motor_vehicle=yes Nn3,n4,n5\n"));
        out.put_file("sources/osm/2026-09-28/pieces/6-32-21", "osm.pbf", &piece).unwrap();
        out.put_file("sources/osm/2026-09-28/pieces/6-40-20", "osm.pbf", &other).unwrap();
        out.put_file("sources/osm/2026-09-28/sets/ferries", "osm.pbf", &set).unwrap();
        let (before, untouched) = (
            out.get("sources/osm/2026-09-28/pieces/6-32-21").unwrap().to_string(),
            out.get("sources/osm/2026-09-28/pieces/6-40-20").unwrap().to_string(),
        );
        assert_eq!(patch_ferries(&mut out, "2026-09-28", &d.path().join("scratch"), &|_| true).unwrap(), (1, 1));
        // The extract gained the standalone ferry, whole (its node at sea too), and kept the rest once.
        let after = out.path(out.get("sources/osm/2026-09-28/pieces/6-32-21").unwrap());
        assert_ne!(out.get("sources/osm/2026-09-28/pieces/6-32-21").unwrap(), before);
        let after = after.unchecked_path();
        assert_eq!(way_ids(&after, None).unwrap().into_iter().collect::<Vec<_>>(), [10, 20, 30]);
        let opl = String::from_utf8(
            store::spawn::output(osmium().args(["cat", "--no-progress", "-f", "opl"]).arg(&after)).unwrap().stdout,
        )
        .unwrap();
        assert_eq!(opl.lines().filter(|l| l.starts_with('n')).count(), 5);
        // An extract with no ferries near it stays as it was; a second run changes nothing.
        assert_eq!(out.get("sources/osm/2026-09-28/pieces/6-40-20").unwrap(), untouched);
        assert_eq!(patch_ferries(&mut out, "2026-09-28", &d.path().join("scratch"), &|_| true).unwrap(), (0, 0));
        // Z6 tiles left out stay as they are.
        assert_eq!(patch_ferries(&mut out, "2026-09-28", &d.path().join("scratch"), &|u| u.x == 40).unwrap(), (0, 0));
    }

    #[test]
    fn older_passes_retire() {
        let d = tempfile::tempdir().unwrap();
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(d.path()), &d.path().join("scratch")).unwrap();
        for l in [
            "sources/osm/2026-03-28/pieces/6-1-2",
            "sources/osm/2026-03-28/sets/summits",
            "sources/osm/2026-01-01/outlines",
            "sources/osm/2026-03-28/outlines",
            "sources/osm/2026-09-28/pieces/6-1-2",
            "sources/osm/2026-09-28/outlines",
            "work/summits/2026-03-28",
            "work/summits/2026-09-28",
            "work/trailends/2026-03-28",
            "sources/items/2026-03-28/facts",
            "work/heritage/2026-03-28/layer-heritage",
            "work/pois/6-1-2",
            "base/6-1-2",
            "global/roads/6-1-2",
        ] {
            out.put_bytes(l, "json", b"{}").unwrap();
        }
        assert_eq!(retire_older(&mut out, "2026-09-28"), 7);
        let left: Vec<&str> = out.manifest.keys().map(String::as_str).collect();
        // (The newest older pass's outlines stay, for the outlines step: the older ones go.)
        assert_eq!(
            left,
            [
                "base/6-1-2",
                "global/roads/6-1-2",
                "sources/osm/2026-03-28/outlines",
                "sources/osm/2026-09-28/outlines",
                "sources/osm/2026-09-28/pieces/6-1-2",
                "work/pois/6-1-2",
                "work/summits/2026-09-28"
            ]
        );
        assert!(is_date("2026-09-28") && !is_date("2026-9-28") && !is_date("6-1-2"));
    }

    #[test]
    fn links_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let ul = UnitLinks { ways: vec![(1, 12.5), (u64::MAX - 3, 0.25)], pairs: vec![(2, 3), (7 << 1 | 1, 9 << 1)] };
        let p = d.path().join("l/6-1-2.bin");
        ul.save(&p).unwrap();
        assert_eq!(UnitLinks::load(&p).unwrap(), ul);
        std::fs::write(&p, &std::fs::read(&p).unwrap()[..30]).unwrap();
        assert!(UnitLinks::load(&p).is_err());
    }
}
