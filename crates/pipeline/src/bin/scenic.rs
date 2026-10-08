//! `scenic`: the user's command (docs/plan.md §1, §8).
//!
//!   scenic status                       what the build Mac is doing, what waits and why, regions
//!   scenic add <id> "<name>" <outline>…  a region: outlines are osm:<relation>, geofabrik:<id>,
//!                                       poly:<file in inputs/outlines>, place:<lon>,<lat>,<km>
//!   scenic remove <id>                  remove a region (its recipe is kept as .removed)
//!   scenic agent [--once] [--dry-run] [--home <dir>] [--helper]  the build agent (the build Mac's
//!                                       login item; --helper: the M1's, the shared steps' jobs)
//!   scenic pool-shadow --home <dir> [--live <dir>] [--hours <h>] [--once]  the pool's shadow run
//!                                       (docs/pool.md §12): its driver beside this Mac's agent (its
//!                                       folder, `--live`, read only), writing only under the NAS's
//!                                       state/pool-shadow/, its log in <dir>/pool-shadow/
//!   scenic pause [--now] | resume       pause the whole build (every Mac's jobs stop at their next
//!                                       safe point; --now: frozen at once), or let it go on
//!   scenic clean [--yes]                clear this Mac's build caches (what later jobs copy back
//!                                       from the NAS or make again) once the build is done and no
//!                                       job runs here, after a y/N (--yes: none): its agent does it,
//!                                       and says what it freed
//!   scenic devices [accept|decline <code> | forget <id>]  on the build Mac: the devices asking to
//!                                       help through the build page, and those helping; an ask
//!                                       answered by the code its page shows, a device forgotten by
//!                                       its page's id (its page no longer helps)
//!   scenic gc [--dry-run] [--days 14]   remove replaced files from the NAS (the agent runs it daily)
//!   scenic backup [--local <dir>]       back up the user's folders (the agent runs it daily)
//!
//! `--root <dir>` points at the NAS project folder (default: the mounted share).

use anyhow::{bail, Context, Result};
use pipeline::agent::{self, backup, gc, jobs::now_s, recipes, Options};
use std::path::{Path, PathBuf};

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// `~/Library/Application Support/scenic`.
fn app_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/scenic")
}

fn root(args: &[String], mount: bool) -> Result<PathBuf> {
    match opt(args, "--root") {
        Some(r) => Ok(PathBuf::from(r)),
        None => agent::find_root(mount).context("the NAS isn't mounted (and couldn't be mounted)"),
    }
}

fn ago(t: u64) -> String {
    let s = now_s().saturating_sub(t);
    match s {
        0..=89 => format!("{s} s ago"),
        90..=5399 => format!("{} min ago", s / 60),
        5400..=129599 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

fn status(args: &[String]) -> Result<()> {
    let root = root(args, false).ok();
    let Some(st) = agent::read_status(root.as_deref(), &app_home().join("agent")) else {
        bail!("no status yet: the build agent hasn't run (or the NAS isn't mounted)");
    };
    let c = &st.conditions;
    println!("Build Mac {} (app {}), seen {}", st.host, st.app, ago(st.beat));
    println!(
        "  {} · {} · {}",
        if c.ac { "on power" } else { "on battery" },
        if !c.nas { "NAS not reachable" } else if c.home { "NAS reachable" } else { "NAS reachable through Tailscale (away from home)" },
        if c.user_active() { "in use" } else { "idle" }
    );
    if now_s().saturating_sub(st.beat) > 600 {
        println!("  (not seen for a while: asleep, away or off; work waits for it)");
    }
    match &st.job {
        Some(j) => {
            println!("Running: {} (since {}){}", j.what, ago(j.started), j.paused.as_ref().map(|p| format!(", paused: {p}")).unwrap_or_default());
            // Its parts, done, under way and to come.
            for (i, p) in j.parts.iter().enumerate() {
                let mark = match j.part {
                    Some(c) if i < c => "✓",
                    Some(c) if i == c => "▸",
                    _ => "○",
                };
                println!("  {mark} {p}");
            }
            for l in j.tail.lines() {
                println!("    {l}");
            }
        }
        None => println!("Running: nothing"),
    }
    if let Some(j) = &st.beside {
        println!("Beside it: {} (since {}){}", j.what, ago(j.started), j.paused.as_ref().map(|p| format!(", paused: {p}")).unwrap_or_default());
    }
    for w in &st.waiting {
        println!("Waiting: {} — {}", w.what, w.why);
    }
    for d in st.recent.iter().take(5) {
        println!("{} {} ({}, {} s)", if d.ok { "Done:" } else { "Failed:" }, d.what, ago(d.ended), d.secs);
        if !d.ok {
            for l in d.note.lines().take(8) {
                println!("    {l}");
            }
        }
    }
    println!("Regions: {}", if st.regions.is_empty() { "none yet".to_string() } else { st.regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ") });
    for (f, e) in &st.bad_recipes {
        println!("  {f} isn't a valid region: {e}");
    }
    // Each Mac's build caches (agent::room): what a clear would free, the last trim and clear.
    for (host, c) in std::iter::once((&st.host, &st.caches)).chain(st.helpers.iter().map(|h| (&h.host, &h.caches))) {
        if let Some(c) = c {
            println!("Caches on {host}: {}", caches_line(c));
        }
    }
    if let Some(r) = &root {
        if let Ok(Some(cat)) = store::catalog::latest(&r.join("catalog")) {
            println!("Map data: catalog {} of {}, {} units", cat.n, cat.created, cat.units.len());
        }
    }
    // Other workers, and where a device's browser joins in (docs/workers.md §7).
    for w in &st.workers {
        println!("Worker: {} — {}{}{}", w.label, w.what, if w.done > 0 { format!(", {} done", w.done) } else { String::new() }, if w.bad { ", stopped: a result differed" } else { "" });
    }
    // (The build Mac's own record of it is over HTTPS once `tailscale serve` proxies the coordinator.)
    let page = std::fs::read_to_string(app_home().join("agent/coord/page")).ok().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let contact = root.as_ref().and_then(|r| std::fs::read(pipeline::coord::contact_path(r)).ok()).and_then(|b| serde_json::from_slice::<pipeline::coord::Contact>(&b).ok());
    let page = match (page, contact) {
        (Some(p), _) => Some(p),
        (None, Some(c)) => c.urls.first().map(|u| format!("{u}/work/")),
        _ => None,
    };
    // The build's page (its dashboard; a device that's to help asks the build Mac from there).
    if let Some(p) = page {
        // (An address from before devices asked carried a key: not shown.)
        println!("Build page: {} (open it on a device on the tailnet)", p.split("#k=").next().unwrap_or(&p));
    }
    // Devices asking to help, answered here on the build Mac (its menu bar, or `scenic devices`):
    // its coordinator says, to its owner alone.
    if let Some(d) = coordinator().ok().and_then(|c| c.post_json("/work/devices", &serde_json::json!({})).ok()).filter(|r| r.0 == 200).and_then(|r| serde_json::from_value::<pipeline::coord::devices::View>(r.1).ok()) {
        for a in &d.asking {
            println!("Asking to help: {} from {}, its page shows code {}: `scenic devices accept {}` or `decline {}`", a.label, a.from, a.code, a.code, a.code);
        }
        if !d.accepted.is_empty() {
            println!("Devices helping: {}", d.accepted.iter().map(|a| format!("{} {}", a.label, a.id)).collect::<Vec<_>>().join(", "));
        }
    }
    // The map on an iPhone or an iPad (docs/plan.md §4, Devices): its address with its key, which this
    // Mac's server writes.
    if let Some(m) = std::fs::read_to_string(app_home().join("map-page")).ok().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        println!("Map on a device: {m} (open it once on an iPhone or an iPad on the tailnet)");
    }
    Ok(())
}

/// A Mac's build caches in a line: what a clear would free (`scenic clean` there) or why not now,
/// and the last trim after the build and the last clear.
fn caches_line(c: &agent::room::Caches) -> String {
    let mut s = c.clearable.map_or_else(|| "not counted yet".to_string(), |n| format!("{} to clear", agent::room::size(n)));
    s += &match &c.why_not {
        Some(w) => format!(" (not now: {w})"),
        None => " (`scenic clean` there)".to_string(),
    };
    if let Some(f) = &c.trimmed {
        s += &format!("; trimmed after the build {}: {}", ago(f.at), f.say());
    }
    if let Some(f) = &c.cleared {
        s += &format!("; cleared {}: {}", ago(f.at), f.say());
    }
    if let Some(f) = c.declined.as_ref().filter(|d| c.cleared.as_ref().is_none_or(|c| d.at > c.at)) {
        s += &format!("; not cleared {}: {}", ago(f.at), f.why_not.as_deref().unwrap_or(""));
    }
    s
}

/// This Mac's own agent's status, as it writes it in its folder `home`: the build Mac's
/// `status.json` or a helper's `helper.json`, whichever is fresher.
fn own_status(home: &Path) -> Option<agent::Status> {
    ["status.json", "helper.json"].iter().filter_map(|f| serde_json::from_slice::<agent::Status>(&std::fs::read(home.join(f)).ok()?).ok()).max_by_key(|s| s.beat)
}

/// `scenic clean`: an ask to this Mac's agent to clear its build caches (agent::room::clear), and
/// what it says it freed, once it has.
fn clean(args: &[String]) -> Result<()> {
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let st = own_status(&home).context("this Mac's agent has no status: is it installed here?")?;
    // (Not written while it makes room for a job, minutes at most.)
    anyhow::ensure!(now_s().saturating_sub(st.beat) < 6 * 60, "this Mac's agent hasn't written its status since {}: is it running?", ago(st.beat));
    let caches = st.caches.as_ref().context("this Mac's agent runs an older app, which doesn't clear its caches on an ask")?;
    if let Some(why) = &caches.why_not {
        bail!("not now: {why}");
    }
    // What goes and how it comes back, and a y/N: a clear is a hundred GB on the build Mac.
    let n = caches.clearable.unwrap_or(0);
    println!("This Mac's build caches, {} in all:", agent::room::size(n));
    for g in &caches.each {
        println!("  {} {}: {}", g.what, agent::room::size(g.bytes), g.back);
    }
    println!("Kept: the Wikidata and Wikipedia answers (the NAS has them too; they free little), a pageview month the NAS lacks, and the map's offline copy.");
    if !flag(args, "--yes") {
        use std::io::Write;
        print!("Clear them? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("not cleared");
            return Ok(());
        }
    }
    let r = agent::room::request_clear(&home, &format!("scenic clean on {}", agent::cond::host_name()))?;
    println!("asked this Mac's agent to clear them; waiting for it…");
    let t = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let answered = own_status(&home).and_then(|s| s.caches).and_then(|c| [c.cleared, c.declined].into_iter().flatten().find(|f| f.asked == Some(r.at)));
        match answered {
            Some(f) => {
                if let Some(why) = f.why_not {
                    bail!("not cleared: {why}");
                }
                println!("{}", f.say());
                return Ok(());
            }
            None if t.elapsed() > std::time::Duration::from_secs(3 * 3600) => bail!("no word from the agent in three hours; it has the ask still (`scenic status` says when it's done)"),
            None => {}
        }
    }
}

/// This Mac's coordinator, as its owner reaches it: here, with the build's key its agent keeps
/// (the build Mac's alone answers).
fn coordinator() -> Result<pipeline::coord::client::Client> {
    let token = std::fs::read_to_string(app_home().join("agent/coord/workers-token")).context("this Mac's coordinator's key (agent/coord/workers-token): devices are answered on the build Mac")?;
    Ok(pipeline::coord::client::Client::at(vec![format!("http://127.0.0.1:{}", pipeline::coord::PORT)], token.trim().to_string(), "scenic devices"))
}

/// The devices asking to help through the build page and those helping, from this Mac's
/// coordinator (the build Mac's: crate::coord::devices); an ask accepted or declined by the code
/// its page shows, a device forgotten by its page's id.
fn devices(args: &[String]) -> Result<()> {
    let c = coordinator()?;
    match (args.get(2).map(String::as_str), args.get(3)) {
        (Some(verb @ ("accept" | "decline")), Some(code)) => {
            let r = c.post_json(&format!("/work/devices/{verb}"), &serde_json::json!({ "code": code }))?;
            anyhow::ensure!(r.0 == 200, "{}", r.1["error"].as_str().unwrap_or("no ask waits with that code"));
            let what = format!("{} {}", r.1["label"].as_str().unwrap_or(""), r.1["id"].as_str().unwrap_or(""));
            println!("{what}: {}", if verb == "accept" { "accepted: it may help and pause the build from its page now" } else { "declined" });
            Ok(())
        }
        (Some("forget"), Some(id)) => {
            let r = c.post_json("/work/devices/forget", &serde_json::json!({ "which": id }))?;
            anyhow::ensure!(r.0 == 200, "{}", r.1["error"].as_str().unwrap_or("no such device"));
            println!("forgotten: its page no longer helps (it may ask again)");
            Ok(())
        }
        (None, _) => {
            let v: pipeline::coord::devices::View = serde_json::from_value(c.post_json("/work/devices", &serde_json::json!({}))?.1)?;
            if v.asking.is_empty() && v.accepted.is_empty() {
                println!("No device asks to help, and none helps through the build page.");
            }
            for a in &v.asking {
                println!("asking   code {}  {} from {}, {}: `scenic devices accept {}`", a.code, a.label, a.from, ago(a.at), a.code);
            }
            for a in &v.accepted {
                println!("helping  {} {}  from {}, accepted {}: `scenic devices forget {}`", a.label, a.id, a.from, ago(a.at), a.id);
            }
            Ok(())
        }
        _ => bail!("scenic devices [accept|decline <code> | forget <id>]"),
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).cloned().unwrap_or_else(|| "status".into());
    match cmd.as_str() {
        "status" => status(&args),
        "add" => {
            let (Some(id), Some(name)) = (args.get(2), args.get(3)) else { bail!("scenic add <id> \"<name>\" <outline>…") };
            // (The outline is what follows the name, but for --root and the folder after it.)
            let mut outline: Vec<String> = Vec::new();
            let mut rest = args[4..].iter();
            while let Some(a) = rest.next() {
                if a == "--root" {
                    rest.next();
                } else if !a.starts_with("--") {
                    outline.push(a.clone());
                }
            }
            let r = recipes::Recipe { id: id.clone(), name: name.clone(), outline };
            recipes::add(&root(&args, true)?.join("inputs/regions"), &r)?;
            println!("added {} ({}); the build Mac builds it when it can (scenic status)", r.name, r.id);
            Ok(())
        }
        "remove" => {
            let id = args.get(2).context("scenic remove <id>")?;
            recipes::remove(&root(&args, true)?.join("inputs/regions"), id)?;
            println!("removed {id}; the build Mac takes what only it covered off the map with its next build");
            Ok(())
        }
        "agent" => {
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            let home = opt(&args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
            let o = Options { root: opt(&args, "--root").map(PathBuf::from), home, bin, dry_run: flag(&args, "--dry-run"), once: flag(&args, "--once"), helper: flag(&args, "--helper") };
            eprintln!("agent: started (app {}, root {})", o.bin.display(), o.root.as_ref().map(|r| r.display().to_string()).unwrap_or_else(|| "the NAS share".into()));
            pipeline::sys::raise_open_files();
            agent::Agent::new(o)?.run()
        }
        "pool-shadow" => {
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            let home = PathBuf::from(opt(&args, "--home").context("--home <its own folder>")?);
            let live = opt(&args, "--live").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
            anyhow::ensure!(home.canonicalize().ok() != live.canonicalize().ok() && home != app_home().join("agent"), "the shadow's folder can't be an agent's");
            let stop_after = opt(&args, "--hours").map(|h| h.parse::<f64>()).transpose()?.map(|h| std::time::Duration::from_secs_f64(h * 3600.0));
            let o = agent::shadow::Options { root: opt(&args, "--root").map(PathBuf::from), home, live, app: agent::app_version(&bin), once: flag(&args, "--once"), stop_after };
            pipeline::sys::raise_open_files();
            agent::shadow::run(o)
        }
        "run-task" => {
            // A task the helper's agent leased (pipeline::offload::run_task), run here; its result
            // written for the agent to hand back.
            let spec: serde_json::Value = serde_json::from_slice(&std::fs::read(opt(&args, "--spec").context("--spec <file>")?)?)?;
            let lease: u64 = opt(&args, "--lease").context("--lease <id>")?.parse()?;
            let (dir, result) = (PathBuf::from(opt(&args, "--dir").context("--dir <folder>")?), PathBuf::from(opt(&args, "--result").context("--result <file>")?));
            let urls: Vec<String> = std::env::var("SCENIC_COORD_URLS").unwrap_or_default().split(',').filter(|u| !u.is_empty()).map(str::to_string).collect();
            let client = pipeline::coord::client::Client::at(urls, std::env::var("SCENIC_COORD_TOKEN").unwrap_or_default(), &std::env::var("SCENIC_WORKER").unwrap_or_default());
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            let root = opt(&args, "--root").map(PathBuf::from);
            let r = pipeline::offload::run_task(&client, lease, &spec, &dir, &bin, root.as_deref());
            std::fs::remove_dir_all(&dir).ok();
            let v = match &r {
                Ok(t) => serde_json::json!({ "ok": true, "task": t }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            pipeline::whole::write(&result, v.to_string().as_bytes())?;
            r.map(|_| ())
        }
        "pause" | "resume" => {
            // An ask to this Mac's agent, which passes it on to the build Mac's (crate::control).
            use pipeline::control::{Mode, Pause};
            let home = opt(&args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
            let host = pipeline::agent::cond::host_name();
            let pause = (cmd == "pause").then(|| Pause::new(if flag(&args, "--now") { Mode::Freeze } else { Mode::Drain }, &format!("scenic pause on {host}")));
            pipeline::control::request(&home, pause)?;
            println!(
                "{}",
                match (cmd.as_str(), flag(&args, "--now")) {
                    ("pause", false) => "pausing: each Mac's running job stops at its next safe point (within minutes), and nothing new starts until `scenic resume`",
                    ("pause", true) => "pausing now: each Mac's running job is frozen where it is, until `scenic resume`",
                    _ => "going on: the build picks up where it stopped",
                }
            );
            Ok(())
        }
        "devices" => devices(&args),
        "clean" => clean(&args),
        "gc" => {
            let days: u64 = opt(&args, "--days").map(|d| d.parse()).transpose()?.unwrap_or(14);
            let r = gc::run(&root(&args, true)?, days, flag(&args, "--dry-run"))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        "backup" => {
            let local = opt(&args, "--local").map(PathBuf::from);
            let today = backup::format_day(std::time::SystemTime::now());
            let r = backup::run(&root(&args, true)?, local.as_deref(), &today, 30)?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        c => bail!("unknown command {c:?}: status, add, remove, agent, pause, resume, devices, clean, gc, backup"),
    }
}
