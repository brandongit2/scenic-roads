//! `scenic`: the user's command (docs/plan.md §1, §8).
//!
//!   scenic status                       what the build Mac is doing, what waits and why, regions
//!   scenic add <id> "<name>" <outline>…  a region: outlines are osm:<relation>, geofabrik:<id>,
//!                                       poly:<file in inputs/outlines>, place:<lon>,<lat>,<km>
//!   scenic remove <id>                  remove a region (its recipe is kept as .removed)
//!   scenic agent [--once] [--dry-run] [--home <dir>] [--helper]  the build agent (the build Mac's
//!                                       login item; --helper: the M1's, units only)
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
            for l in j.tail.lines() {
                println!("    {l}");
            }
        }
        None => println!("Running: nothing"),
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
    if let Some(r) = &root {
        if let Ok(Some(cat)) = store::catalog::latest(&r.join("catalog")) {
            println!("Map data: catalog {} of {}, {} units", cat.n, cat.created, cat.units.len());
        }
    }
    // Other workers, and where a device's browser joins in (docs/workers.md §7).
    for w in &st.workers {
        println!("Worker: {} — {}{}{}", w.label, w.what, if w.done > 0 { format!(", {} done", w.done) } else { String::new() }, if w.bad { ", stopped: a result differed" } else { "" });
    }
    let contact = root.as_ref().and_then(|r| std::fs::read(pipeline::coord::contact_path(r)).ok()).and_then(|b| serde_json::from_slice::<pipeline::coord::Contact>(&b).ok());
    if let Some(c) = contact {
        if let Some(u) = c.urls.first() {
            println!("Worker page: {u}/work/#k={} (open it on a device on the tailnet)", c.token);
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).cloned().unwrap_or_else(|| "status".into());
    match cmd.as_str() {
        "status" => status(&args),
        "add" => {
            let (Some(id), Some(name)) = (args.get(2), args.get(3)) else { bail!("scenic add <id> \"<name>\" <outline>…") };
            let outline: Vec<String> = args[4..].iter().filter(|a| !a.starts_with("--")).cloned().collect();
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
            agent::Agent::new(o)?.run()
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
            let r = pipeline::offload::run_task(&client, lease, &spec, &dir, &bin);
            std::fs::remove_dir_all(&dir).ok();
            let v = match &r {
                Ok(t) => serde_json::json!({ "ok": true, "task": t }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            pipeline::whole::write(&result, v.to_string().as_bytes())?;
            r.map(|_| ())
        }
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
        c => bail!("unknown command {c:?}: status, add, remove, agent, gc, backup"),
    }
}
