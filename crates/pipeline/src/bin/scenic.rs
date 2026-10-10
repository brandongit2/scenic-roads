//! `scenic`: the user's command (docs/plan.md §1, §8).
//!
//!   scenic status                       what the build Mac is doing, what waits and why, regions
//!   scenic add <id> "<name>" <outline>…  a region: outlines are osm:<relation>, geofabrik:<id>,
//!                                       poly:<file in inputs/outlines>, place:<lon>,<lat>,<km>
//!   scenic remove <id>                  remove a region (its recipe is kept as .removed)
//!   scenic agent [--once] [--dry-run] [--home <dir>] [--helper]  the build agent (the build Mac's
//!                                       login item; --helper: the M1's, the shared steps' jobs)
//!   scenic pool on|off|status [--force]  the pool's switch (docs/pool.md §12): on, after its checks
//!                                       (no earlier pool's files, the writer named, every agent
//!                                       on this app); off, once its lead is caught up and no job
//!                                       runs, then (run again once the agents restarted) its files
//!                                       moved aside; how it stands
//!   scenic pool-shadow --home <dir> [--live <dir>] [--hours <h>] [--once]  the pool's shadow run
//!                                       (docs/pool.md §12): its driver beside this Mac's agent (its
//!                                       folder, `--live`, read only), writing only under the NAS's
//!                                       state/pool-shadow/, its log in <dir>/pool-shadow/
//!   scenic lead                         who leads the build, its term, since when, a handover
//!                                       under way, each member and whether it can lead (docs/pool.md
//!                                       §11)
//!   scenic lead give <member>           hand the lead to a member (its host name or member id)
//!   scenic lead take [--force] [--downgrade]  this Mac takes the lead over: once the lead is out
//!                                       of touch or stood down; --force with it in touch, past a
//!                                       term that can't be read, or over this Mac's own handover;
//!                                       --downgrade on an app older than the term's
//!   scenic lead auto on|off             the proactive offer taken by itself (the lead away or on
//!                                       battery five minutes, another Mac home on power)
//!   scenic pause [--now] | resume       pause the whole build (every Mac's jobs stop at their next
//!                                       safe point; --now: frozen at once), or let it go on
//!   scenic clean [--yes]                clear this Mac's build caches (what later jobs copy back
//!                                       from the NAS or make again) once the build is done (jobs
//!                                       running keep what they use), after a y/N (--yes: none): its
//!                                       agent does it, and says what it freed
//!   scenic room [<GB> | off]            this Mac's disk room target: the free space its agent
//!                                       keeps, freeing its caches to it as far as needed, jobs
//!                                       running or not (what they use stays), and starting no job
//!                                       that would cross it, until it's lowered or off; with none,
//!                                       how it stands
//!   scenic gc [--dry-run] [--days 14]   remove replaced files from the NAS (the agent runs it daily)
//!   scenic backup [--local <dir>]       back up the user's folders (the agent runs it daily)
//!   scenic inputs                       every gate unit (docs/inputs.md §4): its accepted version, its
//!                                       state, what holds it, its acceptances (those no check raises
//!                                       any more marked stale)
//!   scenic inputs check [<unit>] [--full]  the drop boxes listed now (the lead's agent checks what
//!                                       changed; --full: every file hashed)
//!   scenic inputs accept <unit> <finding id>|--all  a held warning accepted (or all the unit's),
//!                                       written from this Mac; errors can't be
//!   scenic inputs unaccept <unit> <finding id>|--stale  an acceptance undone (or every stale one)
//!   scenic inputs test on|off           the test unit `_gate-test` on the gate, or off it
//!   scenic timings [<kind>] [--last N] [--host <mac>] [--here | --file <f>] [--json]  the jobs'
//!                                       timings (pipeline::timings): each kind's phases over its
//!                                       last N runs (20), with their totals and shares; the
//!                                       build's (its coordinator's log, asked of it from another
//!                                       Mac), or with --here this Mac's own jobs' alone
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

/// `scenic timings`: the jobs' phases, summed by kind (pipeline::timings::summary).
fn timings(args: &[String]) -> Result<()> {
    use pipeline::timings::{read_log, summary, summary_text, RunRec};
    let kind = args.get(2).filter(|a| !a.starts_with("--")).map(String::as_str);
    let last: usize = opt(args, "--last").map(|n| n.parse()).transpose()?.unwrap_or(20);
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    // The build's log: this Mac's coordinator's, else the coordinator's answer; --here, this Mac's
    // own jobs'.
    let (runs, from): (Vec<RunRec>, String) = if let Some(f) = opt(args, "--file") {
        (read_log(Path::new(&f)), f)
    } else if flag(args, "--here") {
        (read_log(&home.join("timings.jsonl")), "this Mac's jobs".into())
    } else if home.join("coord/timings.jsonl").exists() {
        (read_log(&home.join("coord/timings.jsonl")), "the build's jobs (this Mac's coordinator)".into())
    } else {
        let asked = (|| -> Result<Vec<RunRec>> {
            let root = root(args, false)?;
            let c = pipeline::coord::client::Client::from_nas(&root, &agent::cond::host_name())?.context("no coordinator to ask")?;
            let (code, v) = c.post_json("/work/timings", &serde_json::json!({ "kind": kind, "last": last }))?;
            anyhow::ensure!(code == 200, "the coordinator answered {code} (an older app?)");
            Ok(serde_json::from_value(v["runs"].clone())?)
        })();
        match asked {
            Ok(r) => (r, "the build's jobs (asked of its coordinator)".into()),
            Err(e) => {
                eprintln!("(the build's coordinator can't be asked: {e:#}; this Mac's jobs alone)");
                (read_log(&home.join("timings.jsonl")), "this Mac's jobs".into())
            }
        }
    };
    let sums = summary(&runs, kind, last, opt(args, "--host").as_deref());
    if flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&sums)?);
        return Ok(());
    }
    if sums.is_empty() {
        println!("no timings yet ({from}{})", kind.map(|k| format!(", kind {k}")).unwrap_or_default());
        return Ok(());
    }
    println!("Timings of {from}, the last {last} runs of each kind (CPU/wall marked ~: approximate, the phase overlapped another thread's work)\n");
    print!("{}", summary_text(&sums));
    Ok(())
}

/// `scenic lead`: the pool's lead as this Mac's agent sees it; an ask of it, followed until it's
/// done or refused (docs/pool.md §11).
fn lead(args: &[String]) -> Result<()> {
    use pipeline::agent::lead::{self as l, State};
    use pipeline::control::LeadAsk;
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let host = agent::cond::host_name();
    let view = || l::own_status(&home).and_then(|s| s.pool).and_then(|p| p.lead);
    // (Its words, the options and their values aside.)
    let mut pos: Vec<&str> = Vec::new();
    let mut it = args.iter().skip(2);
    while let Some(a) = it.next() {
        if a == "--home" || a == "--root" {
            it.next();
        } else if !a.starts_with("--") {
            pos.push(a);
        }
    }
    let ask = match pos.first().copied() {
        None | Some("status") => {
            let v = view().context("this Mac's agent isn't in the pool (`scenic pool status`), or hasn't said yet")?;
            for line in l::said(&v, now_s()) {
                println!("{line}");
            }
            return Ok(());
        }
        Some("auto") => {
            let r = root(args, false)?;
            let p = r.join(l::AUTO);
            match pos.get(1).copied() {
                Some("on") => {
                    pipeline::whole::write(&p, format!("turned on by scenic lead on {host}\n").as_bytes())?;
                    println!("on: when the lead is away or on battery and another Mac is home on power for five minutes, the lead hands the build to it by itself (never within half an hour of the last change of lead)");
                }
                Some("off") => {
                    if let Err(e) = std::fs::remove_file(&p) {
                        anyhow::ensure!(e.kind() == std::io::ErrorKind::NotFound, "{e}");
                    }
                    println!("off: the offer waits for a click");
                }
                _ => println!("{}", if p.exists() { "on" } else { "off" }),
            }
            return Ok(());
        }
        Some("give") => {
            let to = pos.get(1).map(|s| s.to_string()).context("scenic lead give <member: its host name or member id>")?;
            // (What the agent would say, said at once when it says so already.)
            if let Some(m) = view().and_then(|v| v.members.into_iter().find(|m| m.member == to || m.host.eq_ignore_ascii_case(&to))) {
                if !m.can_lead {
                    bail!("the lead can't be handed to {}: {}", m.host, m.why_not.unwrap_or_default());
                }
            }
            LeadAsk::Give { to: to.clone() }
        }
        Some("take") => {
            let (force, downgrade) = (flag(args, "--force"), flag(args, "--downgrade"));
            if let Some(t) = view().and_then(|v| v.takeover) {
                if let Some(w) = t.refused {
                    bail!("this Mac can't take the lead over: {w}");
                }
                if let (Some(w), false) = (&t.force, force) {
                    bail!("taking the lead over needs --force: {w}");
                }
                if let (Some(w), false) = (&t.downgrade, downgrade) {
                    bail!("taking the lead over needs --downgrade: {w}");
                }
            }
            LeadAsk::Take { force, downgrade }
        }
        Some(x) => bail!("scenic lead [give <member> | take [--force] [--downgrade] | auto on|off], not {x}"),
    };
    let r = pipeline::control::request_lead(&home, ask, &format!("scenic lead on {host}"))?;
    println!("asked this Mac's agent; it takes the ask up within seconds");
    // Followed as the agent's status says, two minutes at most.
    let start = std::time::Instant::now();
    let mut last = String::new();
    while start.elapsed() < std::time::Duration::from_secs(150) {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let Some(a) = view().and_then(|v| v.asked).filter(|a| a.ask == r.ask && a.at >= r.at.saturating_sub(1)) else { continue };
        let now = format!("{:?}: {}", a.state, a.said);
        if now != last {
            println!("{}", a.said);
            last = now;
        }
        match a.state {
            State::Done => return Ok(()),
            State::Refused | State::Failed => bail!("{}", a.said),
            _ => {}
        }
    }
    println!("still under way: `scenic lead` says how it stands");
    Ok(())
}

/// `scenic inputs`: the gate (docs/inputs.md §4.5, §4.7), from the records on the NAS.
fn inputs(args: &[String]) -> Result<()> {
    use pipeline::inputs::{self as gi, view};
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let r = root(args, false)?;
    // (Its words, the options and their values aside.)
    let mut pos: Vec<&str> = Vec::new();
    let mut it = args.iter().skip(2);
    while let Some(a) = it.next() {
        if a == "--home" || a == "--root" {
            it.next();
        } else if !a.starts_with("--") {
            pos.push(a);
        }
    }
    let manifest: std::collections::BTreeMap<String, String> = pipeline::out::read_record(&r.join("state/build/manifest.json"))?;
    let units = gi::units(&r);
    // (Checking, as the lead's status says.)
    let checking: std::collections::BTreeSet<String> = agent::read_status(Some(&r), &home).map(|s| s.inputs.into_iter().filter(|v| v.state == view::State::Checking).map(|v| v.unit).collect()).unwrap_or_default();
    let views = view::of(&r, &manifest, &units, &checking, &Default::default(), &mut view::Cache::default());
    let by = format!("scenic inputs on {}", agent::cond::host_name());
    let unit_arg = |i: usize| -> Result<String> {
        let u = pos.get(i).map(|s| s.to_string()).context("which unit?")?;
        anyhow::ensure!(gi::checks(&u).is_some() || views.iter().any(|v| v.unit == u), "{u} isn't a gate unit (scenic inputs lists them)");
        Ok(u)
    };
    match pos.first().copied() {
        None | Some("list") => {
            if views.is_empty() {
                println!("no unit is on the gate (`scenic inputs test on` puts the test unit on it)");
            }
            for v in &views {
                println!("{}: {}{}", v.unit, match v.state {
                    view::State::Ok => "taken in",
                    view::State::Checking => "checking",
                    view::State::Held => "held",
                }, v.checked.map(|t| format!(", checked {}", ago(t))).unwrap_or_default());
                println!("  version: {}", v.version.as_deref().unwrap_or("none yet"));
                if let Some(u) = &v.unread {
                    println!("  can't be read now: {u}");
                }
                if !v.held.is_empty() {
                    println!("  held: {}", v.held.join(", "));
                }
                if let Some(t) = &v.together {
                    println!("  {t}");
                }
                for s in &v.findings {
                    let f = &s.finding;
                    println!("  {} {}: {}", if f.level == gi::Level::Error { "error" } else { "warning" }, f.id, f.message);
                    for (n, l) in f.lines.iter().take(10) {
                        println!("      line {n}: {l}");
                    }
                    let more = f.lines.len().saturating_sub(10) + s.more;
                    if more > 0 {
                        println!("      … and {more} more (in its report)");
                    }
                }
                for id in &v.accepted {
                    println!("  accepted: {id}{}", if v.stale.contains(id) { " (stale: no check raises it any more)" } else { "" });
                }
            }
            Ok(())
        }
        Some("check") => {
            let full = flag(args, "--full");
            let which: Vec<String> = match pos.get(1) {
                Some(_) => vec![unit_arg(1)?],
                None => units.iter().map(|u| u.to_string()).collect(),
            };
            for u in &which {
                gi::ask(&home, &gi::Ask { unit: u.clone(), check: true, full, by: by.clone(), at: now_s(), ..Default::default() })?;
            }
            println!("asked this Mac's agent to list the drop boxes now{}: the lead checks what changed (`scenic inputs` says how it stands)", if full { ", hashing every file" } else { "" });
            Ok(())
        }
        Some("accept") => {
            let u = unit_arg(1)?;
            let v = views.iter().find(|v| v.unit == u).context("nothing held")?;
            let report = gi::Report { findings: v.findings.iter().map(|s| s.finding.clone()).collect(), ..Default::default() };
            let a = match (flag(args, "--all"), pos.get(2)) {
                (true, _) => gi::Ask { unit: u.clone(), all: true, by: by.clone(), at: now_s(), ..Default::default() },
                (false, Some(id)) => gi::Ask { unit: u.clone(), accept: vec![id.to_string()], by: by.clone(), at: now_s(), ..Default::default() },
                _ => bail!("scenic inputs accept <unit> <finding id>|--all"),
            };
            let said = gi::apply_ask(&r, &a, Some(&report), &gi::member_of(&home))?;
            if said.is_empty() {
                println!("nothing new to accept");
            }
            for s in said {
                println!("{s}");
            }
            // (Listed now, should this Mac lead: else at the lead's next listing, two minutes at most.)
            gi::ask(&home, &gi::Ask { unit: u, check: true, by, at: now_s(), ..Default::default() })?;
            Ok(())
        }
        Some("unaccept") => {
            let u = unit_arg(1)?;
            let ids: Vec<String> = match (flag(args, "--stale"), pos.get(2)) {
                (true, _) => views.iter().find(|v| v.unit == u).map(|v| v.stale.clone()).unwrap_or_default(),
                (false, Some(id)) => vec![id.to_string()],
                _ => bail!("scenic inputs unaccept <unit> <finding id>|--stale"),
            };
            for id in &ids {
                println!("{}", if gi::unaccept(&r, &u, id)? { format!("unaccepted {id}") } else { format!("{id} wasn't accepted") });
            }
            if ids.is_empty() {
                println!("no stale acceptance");
            }
            gi::ask(&home, &gi::Ask { unit: u, check: true, by, at: now_s(), ..Default::default() })?;
            Ok(())
        }
        Some("test") => {
            let p = r.join(gi::TEST_FLAG);
            match pos.get(1).copied() {
                Some("on") => {
                    std::fs::create_dir_all(p.parent().unwrap())?;
                    pipeline::whole::write(&p, format!("turned on by {by}\n").as_bytes())?;
                    println!("on: {} is on the gate (its drop box inputs/{}/, .jsonl files of {{\"k\": …, \"v\": …}} lines)", gi::TEST_UNIT, gi::TEST_UNIT);
                }
                Some("off") => {
                    if let Err(e) = std::fs::remove_file(&p) {
                        anyhow::ensure!(e.kind() == std::io::ErrorKind::NotFound, "{e}");
                    }
                    println!("off: {} is off the gate (its records and copies stay until removed)", gi::TEST_UNIT);
                }
                _ => println!("{}", if p.exists() { "on" } else { "off" }),
            }
            Ok(())
        }
        Some(x) => bail!("scenic inputs [check [<unit>] [--full] | accept <unit> <id>|--all | unaccept <unit> <id>|--stale | test on|off], not {x}"),
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
    // The memory guard as its thread last sampled (every 5 s, whatever the agent's loop waits on).
    if let Some(m) = agent::memguard::live(&app_home().join("agent")).filter(|m| now_s().saturating_sub(m.at) < 60) {
        let held: u64 = m.held_mb.iter().flatten().sum();
        let frozen = if m.frozen.iter().any(|f| *f) { ", a job frozen past it" } else { "" };
        println!("  memory: the jobs hold {:.1} GB of this Mac's limit of {:.1} GB ({}{frozen}, {})", held as f64 / 1024.0, m.limit_mb as f64 / 1024.0, if m.on { "guarded" } else { "not guarded" }, ago(m.at));
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
    // The gate (docs/inputs.md §4.7): a line per unit held or checking.
    for v in st.inputs.iter().filter(|v| v.state != pipeline::inputs::view::State::Ok) {
        println!("Inputs: {}{}", v.line(), if v.state == pipeline::inputs::view::State::Held { " (`scenic inputs` says what holds it)" } else { "" });
    }
    println!("Regions: {}", if st.regions.is_empty() { "none yet".to_string() } else { st.regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ") });
    for (f, e) in &st.bad_recipes {
        println!("  {f} isn't a valid region: {e}");
    }
    // The pool (docs/pool.md §10): who leads, each member, as this Mac's agent sees it (else as the
    // status read says).
    let own = agent::lead::own_status(&app_home().join("agent")).and_then(|s| s.pool).and_then(|p| p.lead);
    if let Some(v) = own.or_else(|| st.pool.as_ref().and_then(|p| p.lead.clone())) {
        println!("The pool:");
        for l in agent::lead::said(&v, now_s()) {
            println!("  {l}");
        }
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
    // The build's page (its dashboard, and where a device helps).
    if let Some(p) = page {
        println!("Build page: {p} (open it on a device on the LAN or the tailnet)");
    }
    // The map on an iPhone or an iPad (docs/plan.md §4, Devices): its address, which this Mac's
    // server writes.
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
    if let Some(r) = &c.room {
        if let Some(t) = &r.target {
            s += &format!("; disk room target {} ({} free{})", agent::room::size(t.bytes), agent::room::size(r.free), if r.short.is_some() { ", short of it" } else { "" });
        }
    }
    s
}

/// `scenic room [<GB> | off]`: this Mac's disk room target (agent::room::Target), set in its agent's
/// folder, which the agent reads each loop; and how it stands, as the agent's status says.
fn room(args: &[String]) -> Result<()> {
    use agent::room::{set_target, size, target};
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let by = format!("scenic room on {}", agent::cond::host_name());
    // (The first word after `room` that isn't an option or an option's value: `--home X 50`.)
    let mut words = args.iter().skip(2);
    let mut what = None;
    while let Some(a) = words.next() {
        if a == "--home" || a == "--root" {
            words.next();
        } else if !a.starts_with("--") {
            what = Some(a.as_str());
            break;
        }
    }
    match what {
        Some("off") => {
            set_target(&home, None, &by)?;
            println!("the disk room target is off: this Mac's agent fills its caches again as its jobs need");
        }
        Some(gb) => {
            let gb: f64 = gb.trim_end_matches("GB").trim_end_matches("gb").parse().ok().filter(|g: &f64| g.is_finite() && *g >= 1.0).with_context(|| format!("scenic room <GB> | off, not {gb}: a target of 1 GB or more"))?;
            let bytes = (gb * (1u64 << 30) as f64) as u64;
            anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
            if let Some(n) = disk_size(&home) {
                anyhow::ensure!(bytes < n, "a target of {} is more than this disk holds ({})", size(bytes), size(n));
            }
            let free = agent::room::disk_free(&home).ok();
            set_target(&home, Some(bytes), &by)?;
            let short = free.map(|f| bytes.saturating_sub(f)).unwrap_or(0);
            if short > 0 {
                println!("the disk room target is {}: this Mac's agent frees {} of its caches as far as needed, now, jobs running or not (what they use stays), and starts no job that would cross it (`scenic room` says how it stands)", size(bytes), size(short));
            } else {
                println!("the disk room target is {}: the disk has that free; this Mac's agent starts no job that would cross it", size(bytes));
            }
        }
        None => {
            let t = target(&home);
            let free = agent::room::disk_free(&home).ok();
            match &t {
                Some(t) => println!("Disk room target: {} (set {} by {})", size(t.bytes), ago(t.at), t.by),
                None => println!("Disk room target: off (`scenic room <GB>` sets one)"),
            }
            if let Some(f) = free {
                println!("Free now: {}", size(f));
            }
            let st = own_status(&home).filter(|s| now_s().saturating_sub(s.beat) < 6 * 60);
            match st.as_ref().and_then(|s| s.caches.as_ref()).and_then(|c| c.room.as_ref()) {
                Some(r) => {
                    if let Some(f) = r.toward.as_ref().filter(|f| t.as_ref().is_some_and(|t| f.target == Some(t.bytes))) {
                        let past = f.goal.map(|g| format!(" (and the {} a job waiting for it needs past it)", size(g.saturating_sub(f.target.unwrap_or(0))))).unwrap_or_default();
                        println!("Freed toward it{past} {}: {}", ago(f.at), f.say());
                    }
                    if let Some(why) = &r.short {
                        println!("Short of it: {why}");
                    }
                    for w in st.iter().flat_map(|s| &s.waiting).filter(|w| w.why.contains("disk room target")) {
                        println!("Waiting: {} — {}", w.what, w.why);
                    }
                }
                None if t.is_some() => println!("(this Mac's agent isn't running, or runs an older app: it keeps the target once it runs this one)"),
                None => {}
            }
        }
    }
    Ok(())
}

/// The size of the disk holding `p` (bytes).
fn disk_size(p: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_blocks as u64 * s.f_frsize as u64)
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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).cloned().unwrap_or_else(|| "status".into());
    match cmd.as_str() {
        "status" => status(&args),
        "lead" => lead(&args),
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
        "pool" => {
            // The pool's switch (docs/pool.md §12): on, off, or how it stands.
            let r = root(&args, false)?;
            let force = flag(&args, "--force");
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            match args.get(2).map(String::as_str) {
                Some("on") => println!("{}", agent::pool::switch_on(&r, &agent::app_version(&bin), force)?),
                Some("off") => println!("{}", agent::pool::switch_off(&r, force)?),
                Some("status") | None => println!("{}", agent::pool::status(&r)),
                Some("floors") => {
                    // The targets' floors the lead's coordinator keeps (crate::agent::memguard):
                    // `--clear` clears them all, `--clear <step>` a step's, `--clear <step> <target>`
                    // one target's (a step fixed, or a floor learned wrong).
                    let clear = flag(&args, "--clear");
                    let rest: Vec<&String> = args.iter().skip(3).filter(|a| !a.starts_with("--") && Some(*a) != opt(&args, "--root").as_ref()).collect();
                    let c = pipeline::coord::client::Client::from_nas(&r, &agent::cond::host_name())?.context("no lead's coordinator to ask")?;
                    let (code, v) = c.post_json("/work/floors", &serde_json::json!({ "clear": clear, "step": rest.first(), "target": rest.get(1) }))?;
                    anyhow::ensure!(code == 200, "the lead's coordinator answered {code} (an older app?)");
                    let floors: std::collections::BTreeMap<String, pipeline::agent::memguard::Floor> = serde_json::from_value(v["floors"].clone())?;
                    if floors.is_empty() {
                        println!("no floors");
                    }
                    for (k, f) in floors {
                        println!("{k}: {:.1} GB{}{}", f.mb as f64 / 1024.0, if f.alone { ", held alone" } else { ", in a batch" }, if f.v > 0 { format!(", v{}", f.v) } else { String::new() });
                    }
                }
                Some(x) => bail!("scenic pool on|off|status [--force] | floors [--clear [<step> [<target>]]], not {x}"),
            }
            Ok(())
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
            // (Timed as a task of its kind: a tail's, a 3D buildings' area's, tree cover's blocks'.)
            let kind = format!("task {}", pipeline::offload::task_kind(&spec));
            let r = pipeline::timings::job(&kind, || pipeline::offload::run_task(&client, lease, &spec, &dir, &bin, root.as_deref()));
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
        "clean" => clean(&args),
        "room" => room(&args),
        "gc" => {
            let days: u64 = opt(&args, "--days").map(|d| d.parse()).transpose()?.unwrap_or(14);
            let r = pipeline::timings::job("gc", || gc::run(&root(&args, true)?, days, flag(&args, "--dry-run")))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        "backup" => {
            let local = opt(&args, "--local").map(PathBuf::from);
            let today = backup::format_day(std::time::SystemTime::now());
            let r = pipeline::timings::job("backup", || backup::run(&root(&args, true)?, local.as_deref(), &today, 30))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        "timings" => timings(&args),
        "inputs" => inputs(&args),
        c => bail!("unknown command {c:?}: status, add, remove, agent, pause, resume, clean, room, gc, backup, timings, inputs"),
    }
}
