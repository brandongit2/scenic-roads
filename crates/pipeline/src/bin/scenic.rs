//! `scenic`: the user's command (docs/surface.md, docs/building.md §2).
//!
//!   scenic status                       what the lead is doing, what waits and why, regions
//!   scenic agent [--once] [--dry-run] [--home <dir>]  the agent (every Mac's login item: its
//!                                       member of the pool, leading or not as the terms say;
//!                                       --helper, of before, accepted and ignored)
//!   scenic pool [status]                the pool as it stands (docs/pool.md): the terms, the agents
//!   scenic lead                         who leads the build, its term, since when, a handover
//!                                       under way, each member and whether it can lead (docs/pool.md
//!                                       §11)
//!   scenic lead give <member>           hand the lead to a member (its host name or member id)
//!   scenic lead take [--force] [--downgrade]  this Mac takes the lead over: once the lead is out
//!                                       of touch or stood down; --force with it in touch, past a
//!                                       term that can't be read, or over this Mac's own handover;
//!                                       --downgrade on an app older than the term's
//!   scenic pause [--now] | resume       pause the whole build (every Mac's jobs stop at their next
//!                                       safe point; --now: frozen at once), or let it go on
//!   scenic pause --here | <member>      pause one Mac alone (this one, or a member by host name or
//!   scenic resume --here | <member>     id): its jobs stop at their next safe point and are given
//!                                       back, it takes no new work (leading, it keeps the
//!                                       lead); the rest of the pool goes on; kept over restarts until resumed
//!   scenic job cancel <target> [--step <step>] [--rerun-after-update]
//!                                       stop the running job building <target> (here, or through
//!                                       the lead on another Mac) and give it back, not failed, its
//!                                       scratch for the target cleared; --rerun-after-update: held
//!                                       until the lead runs a newer app
//!   scenic clean [--yes]                clear this Mac's build caches (what later jobs copy back
//!                                       from the NAS or make again) once the build is done (jobs
//!                                       running keep what they use), after a y/N (--yes: none): its
//!                                       agent does it, and says what it freed
//!   scenic room [<GB> | off]            this Mac's disk room target: the free space its agent
//!                                       keeps, freeing its caches to it as far as needed, jobs
//!                                       running or not (what they use stays), and starting no job
//!                                       that would cross it, until it's lowered or off; with none,
//!                                       how it stands
//!   scenic memory [<GB> | off]          the memory this Mac gives the build's jobs together
//!                                       (within the memory guard's limit; off: the limit); with
//!                                       none, how it stands
//!   scenic in-use [--cores <n>] | off   the in-use share: the cores this Mac's agent keeps free
//!                                       of its jobs while its owner uses it (by default 2); off:
//!                                       the default; with none, how it stands
//!   scenic nas-target [<percent> | off] the pool's NAS load target: the busiest disk's busy share
//!                                       the NAS budget grants up to (by default 85, 30 to 95);
//!                                       off: the default; with none, how it stands
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
//!   scenic inputs test on|off [--forget]  the test unit `_gate-test` on the gate, or off it
//!                                       (--forget: its acceptances and records removed)
//!   scenic timings [<kind>] [--last N] [--host <mac>] [--here | --file <f>] [--json]  the jobs'
//!                                       timings (pipeline::timings): each kind's phases over its
//!                                       last N runs (20), with their totals and shares; the
//!                                       build's (its coordinator's log, asked of it from another
//!                                       Mac), or with --here this Mac's own jobs' alone
//!
//! `--root <dir>` points at the NAS project folder (default: the mounted share). `--help` (or `-h`)
//! on any command prints its usage and does nothing; an unknown flag or an argument too many is
//! refused before anything runs (pipeline::cli::tables::SCENIC, the commands' table).

use anyhow::{bail, Context, Result};
use pipeline::agent::{self, backup, gc, jobs::now_s, Options};
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
    if args.get(2).map(String::as_str) == Some("publish") {
        return timings_publish(args);
    }
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
            let c = pipeline::coord::client::Client::from_nas(&root, &agent::cond::host_name())?
                .context("no coordinator to ask")?;
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

/// `scenic timings publish [--last N]`: publishing end to end (pipeline::pubtimes), from the
/// build's history (the NAS's, else this Mac's coordinator's; `--file` a history file) and the map
/// servers' log on the NAS: each catalog of the last N (10) and its z6 tiles, then the medians.
fn timings_publish(args: &[String]) -> Result<()> {
    use pipeline::pubtimes;
    let last: usize = opt(args, "--last").map(|n| n.parse()).transpose()?.unwrap_or(10);
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let root = root(args, false).ok();
    let days = 8;
    let events = match opt(args, "--file") {
        Some(f) => pubtimes::load_history(None, Some(Path::new(&f)), days),
        None => pubtimes::load_history(root.as_deref(), Some(&home.join("coord/history.jsonl")), days),
    };
    let servers =
        root.as_deref().map(|r| pubtimes::read_servers(r, now_s().saturating_sub(days * 86400))).unwrap_or_default();
    let (cats, units) = pubtimes::chains(&events, &servers);
    if flag(args, "--json") {
        let from = cats.len().saturating_sub(last);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "catalogs": &cats[from..], "units": units }))?
        );
        return Ok(());
    }
    if cats.is_empty() {
        println!("no catalogs in the build's history yet ({} events read)", events.len());
        return Ok(());
    }
    println!("Publishing, the last {last} catalogs: each unit's hand-off, its map tiles planned, leased and packed, the catalog, each Mac's server and mirror\n");
    print!("{}", pubtimes::text(&cats, &units, last));
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
        Some("give") => {
            let to =
                pos.get(1).map(|s| s.to_string()).context("scenic lead give <member: its host name or member id>")?;
            // (What the agent would say, said at once when it says so already.)
            if let Some(m) =
                view().and_then(|v| v.members.into_iter().find(|m| m.member == to || m.host.eq_ignore_ascii_case(&to)))
            {
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
        Some(x) => bail!("scenic lead [give <member> | take [--force] [--downgrade]], not {x}"),
    };
    let r = pipeline::control::request_lead(&home, ask, &format!("scenic lead on {host}"))?;
    println!("asked this Mac's agent; its pool's next step takes the ask up, within half a minute");
    // Followed as the agent's status says, two minutes at most.
    let start = std::time::Instant::now();
    let mut last = String::new();
    while start.elapsed() < std::time::Duration::from_secs(150) {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let Some(a) = view().and_then(|v| v.asked).filter(|a| a.ask == r.ask && a.at >= r.at.saturating_sub(1)) else {
            continue;
        };
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
    let manifest: std::collections::BTreeMap<String, String> =
        pipeline::out::read_record(&r.join("state/build/manifest.json"))?;
    let units = gi::units(&r);
    // (Checking, as the lead's status says.)
    let checking: std::collections::BTreeSet<String> = agent::read_status(Some(&r), &home)
        .map(|s| s.inputs.into_iter().filter(|v| v.state == view::State::Checking).map(|v| v.unit).collect())
        .unwrap_or_default();
    let views = view::of(&r, &manifest, &units, &checking, &Default::default(), &mut view::Cache::default());
    let by = format!("scenic inputs on {}", agent::cond::host_name());
    let unit_arg = |i: usize| -> Result<String> {
        let u = pos.get(i).map(|s| s.to_string()).context("which unit?")?;
        anyhow::ensure!(
            gi::checks(&u).is_some() || views.iter().any(|v| v.unit == u),
            "{u} isn't a gate unit (scenic inputs lists them)"
        );
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
                    if flag(args, "--forget") {
                        // Its acceptances removed here; its records by the lead (a job of the
                        // lead's: the records are its to write), asked through this Mac's agent.
                        let acc = r.join(gi::ACCEPTED).join(gi::TEST_UNIT);
                        if let Err(e) = std::fs::remove_dir_all(&acc) {
                            anyhow::ensure!(e.kind() == std::io::ErrorKind::NotFound, "remove {}: {e}", acc.display());
                        }
                        gi::ask(&home, &gi::Ask { unit: gi::TEST_UNIT.into(), forget: true, by: by.clone(), at: now_s(), ..Default::default() })?;
                        println!("off: {} is off the gate, its acceptances removed; the lead removes its records from the build's (its copies go with GC's 14 days; its drop box is yours)", gi::TEST_UNIT);
                    } else {
                        println!("off: {} is off the gate and no longer shown (its records stay: `scenic inputs test off --forget` removes them)", gi::TEST_UNIT);
                    }
                }
                _ => println!("{}", if p.exists() { "on" } else { "off" }),
            }
            Ok(())
        }
        Some(x) => bail!("scenic inputs [check [<unit>] [--full] | accept <unit> <id>|--all | unaccept <unit> <id>|--stale | test on|off [--forget]], not {x}"),
    }
}

/// A member's capacity in a line: "memory 8.8 GB free of the build's 10.0 GB (1.2 GB held, the
/// limit 12.0 GB) · 6 of 8 cores free · disk 120 GB free · 100 GB for jobs (20 GB room target) · on
/// power · in use · NAS reachable · leases 12, 14 · 20 s ago".
fn capacity_line(c: &pipeline::coord::Capacity) -> String {
    let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
    let mut v = vec![
        format!(
            "memory {} free of the build's {} ({} held{}{})",
            gb(c.mem_mb),
            gb(if c.build_mb > 0 { c.build_mb } else { c.limit_mb }),
            gb(c.held_mb),
            if c.build_mb > 0 && c.build_mb < c.limit_mb {
                format!(", the limit {}", gb(c.limit_mb))
            } else {
                String::new()
            },
            if c.guard { "" } else { ", not guarded" }
        ),
        format!("{} of {} cores free", c.cores_free, c.cores),
        if c.free_mb > 0 {
            format!("disk {}", pipeline::coord::disk_words(c, c.disk_mb))
        } else {
            format!(
                "disk {} free{}",
                gb(c.disk_mb),
                if c.reserved_mb > 0 { format!(" ({} reserved)", gb(c.reserved_mb)) } else { String::new() }
            )
        },
        match (c.ac, c.battery) {
            (true, _) => "on power".to_string(),
            (false, Some(b)) => format!("on battery ({b} %)"),
            (false, None) => "on battery".to_string(),
        },
        (if c.in_use { "in use" } else { "idle" }).to_string(),
        (if !c.nas {
            "NAS not reachable"
        } else if c.home {
            "NAS reachable"
        } else {
            "NAS through Tailscale"
        })
        .to_string(),
    ];
    if !c.leases.is_empty() {
        v.push(format!(
            "lease{} {}",
            if c.leases.len() == 1 { "" } else { "s" },
            c.leases.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
        ));
    }
    v.push(ago(c.at));
    v.join(" · ")
}

/// A Mac's jobs, every slot's in order, each with its slot (`Status::jobs`: an older agent's by
/// their place).
fn jobs_of(st: &agent::Status) -> Vec<agent::JobView> {
    st.jobs()
}

/// The ramp in words: "3 of 6 slots (reached 3) · clean since 2 h ago, grows ≈ in 4.0 h", or
/// "· held: …" (its slots held at the start), "· not clean: …" (growth stopped alone).
fn ramp_words(r: &agent::RampView) -> String {
    let mut s = if r.most > 0 {
        format!("{} of {} slots (ramp reached {})", r.slots, r.most, r.reached)
    } else {
        format!("{} slots (ramp reached {})", r.slots, r.reached)
    };
    if let Some(h) = &r.held {
        s += &format!(" · held: {h}");
    } else if let Some(n) = &r.not_clean {
        s += &format!(" · not clean: {n}");
    } else if r.clean_since > 0 {
        s += &format!(" · clean since {}", ago(r.clean_since));
        if let Some(g) = r.grows_at {
            let left = g.saturating_sub(now_s());
            s += &format!(", grows in {:.1} h if it keeps its slots busy", left as f64 / 3600.0);
        }
    }
    s
}

/// A job in a line: "slot 2: Terrain: 4 tiles · 40% · about 12 min left · since 20 min ago".
fn job_line(j: &agent::JobView) -> String {
    let mut v = vec![format!("slot {}: {}", j.slot.unwrap_or(0) + 1, j.what)];
    if let Some(p) = j.progress.as_ref().filter(|p| p.total > 0.0) {
        v.push(format!("{}%", (p.done / p.total * 100.0).floor()));
        if let Some(e) = p.eta_s {
            v.push(format!("about {} min left", e.div_ceil(60)));
        }
    }
    if let Some(t) = j.threads {
        v.push(format!("{t} threads"));
    }
    if let Some(m) = j.mem_mb {
        v.push(format!("{:.1} GB", m as f64 / 1024.0));
    }
    v.push(format!("since {}", ago(j.started)));
    if let Some(p) = &j.paused {
        v.push(format!("paused: {p}"));
    } else if j.pausing.is_some() {
        v.push("stopping at its next safe point".into());
    }
    v.join(" · ")
}

/// Each Mac's lines (`scenic status`): this Mac's first, marked with its part in the pool
/// (`Status::role`: "lead, coordinator", "member", or out of the pool and why), with its ramp; then
/// each member building (`Status::helpers`); then any other the coordinator holds a capacity of.
/// Under each: its capacity (coord::Capacity) and NAS share, a line per running job, and why
/// nothing more fits (the packing's refusal, `Status::packing`).
fn macs_lines(st: &agent::Status) -> Vec<String> {
    let ramp = st.ramp.as_ref().map(|r| format!(" · {}", ramp_words(r))).unwrap_or_default();
    let mut macs: Vec<(&str, String, Option<&agent::Status>)> = vec![(
        &st.host,
        format!("{} · {} · {} running{ramp}", st.host, st.role(), plural(jobs_of(st).len(), "job")),
        Some(st),
    )];
    for h in &st.helpers {
        // (A member out of the pool says so, and why.)
        let part = if h.out_of_pool.is_some() { h.role() } else { "member".into() };
        let head = format!("{} · {part} · {} running · seen {}", h.host, plural(jobs_of(h).len(), "job"), ago(h.beat));
        macs.push((&h.host, head, Some(h)));
    }
    for (name, _) in &st.capacity {
        if !macs.iter().any(|(h, _, _)| agent::is_slot_of(name, h)) {
            macs.push((name, format!("{name} · member · its status not on the NAS lately"), None));
        }
    }
    let mut out = Vec::new();
    for (host, head, s) in macs {
        // (Paused alone, task #224: its jobs given back at their next safe point, nothing new.)
        match s.and_then(|s| s.held.as_ref()) {
            Some(h) => out.push(format!("{head} · paused (from {})", h.by)),
            None => out.push(head),
        }
        let cap = st.capacity.iter().find(|(n, _)| agent::is_slot_of(n, host)).map(|(_, c)| c);
        let nas: u32 = st.nas_share.iter().filter(|(n, _)| agent::is_slot_of(n, host)).map(|(_, w)| *w).sum();
        let nas =
            if st.nas_share.is_empty() { String::new() } else { format!(" · NAS share {:.0} %", nas as f64 / 10.0) };
        match cap {
            Some(c) => out.push(format!("    capacity: {}{nas}", capacity_line(c))),
            None if !nas.is_empty() => out.push(format!("    capacity: not said lately{nas}")),
            None => {}
        }
        let jobs = s.map(jobs_of).unwrap_or_default();
        for j in &jobs {
            out.push(format!("    {}", job_line(j)));
            if let Some(w) = &j.waiting {
                out.push(format!("      {w}"));
            }
            for (i, p) in j.parts.iter().enumerate() {
                let mark = match j.part {
                    Some(c) if i < c => "✓",
                    Some(c) if i == c => "▸",
                    _ => "○",
                };
                out.push(format!("      {mark} {p}"));
            }
            for l in j.tail.lines() {
                out.push(format!("        {l}"));
            }
        }
        if jobs.is_empty() {
            out.push("    running nothing".into());
        }
        for (_, why) in st.packing.iter().filter(|(n, _)| agent::is_slot_of(n, host)) {
            out.push(format!("    nothing more fits: {why}"));
        }
    }
    out
}

/// A region still building, as the Regions panel and the menu bar say it: and when all of it is
/// on the map, as the forecast has it (`map_at`); every z6 tile built but not yet recorded built,
/// finishing.
fn building_line(built: usize, total: usize, map_at: Option<u64>) -> String {
    if built >= total {
        return format!("finishing: {built} of {total} tiles, the last map tiles and layers being made");
    }
    let all = map_at.map(|t| format!("; all ≈ {}", when(t))).unwrap_or_default();
    format!("{built} of {total} tiles, on the map as they come{all}")
}

/// A time to come, from now: "now", "in 40 min", "in 2.5 h".
fn when(t: u64) -> String {
    let s = t.saturating_sub(now_s());
    match s {
        0..=59 => "now".into(),
        60..=5399 => format!("in {} min", s.div_ceil(60)),
        _ => format!("in {:.1} h", s as f64 / 3600.0),
    }
}

/// The cadenced publishes' names (agent::forecast::PublishFc), as the build page says them.
fn publish_name(what: &str) -> &str {
    match what {
        "low" => "the low zooms",
        "marks" => "the landmarks",
        "overlays" => "the area overlays",
        x => x,
    }
}

/// Publishing as it stands (the build page's Publishing card, in the same words): what it does and
/// why (agent::build::PublishLine), the last catalog, the map tiles to draw and by which Mac, the
/// cadenced publishes next (agent::forecast), each Mac's catalog adopted (agent::Published).
fn publishing_lines(st: &agent::Status) -> Vec<String> {
    let mut out = Vec::new();
    let Some(u) = &st.publishing else { return out };
    out.push(format!("Publishing: {}", u.why));
    let p = st.published.clone().unwrap_or_default();
    if let Some(c) = p.last {
        let kind = if p.low { "with the low zooms" } else { "map tiles only" };
        out.push(format!("  last catalog: {}, {}, {kind}", c.n, ago(c.at)));
    }
    if u.tiles > 0 || !p.drawing.is_empty() {
        let by: Vec<String> = p.drawing.iter().map(|(h, k)| format!("{h}: {}", plural(*k, "tile"))).collect();
        let by = if by.is_empty() { String::new() } else { format!(" (being drawn: {})", by.join(", ")) };
        out.push(format!("  map tiles to draw: {}{by}", u.tiles));
    }
    let next: Vec<String> = st
        .forecast
        .iter()
        .flat_map(|f| &f.publishes)
        .map(|x| {
            let t = if x.what == "low" && u.low { "now".to_string() } else { when(x.at) };
            format!("{} {t}", publish_name(&x.what))
        })
        .collect();
    if !next.is_empty() {
        out.push(format!("  next: {}", next.join(", ")));
    }
    if !p.adopted.is_empty() {
        let a: Vec<String> = p.adopted.iter().map(|(h, c)| format!("{h}: catalog {} ({})", c.n, ago(c.at))).collect();
        out.push(format!("  adopted: {}", a.join(", ")));
    }
    for r in &u.regions {
        out.push(format!("  done, not on the map as it is: {} ({})", r.id, r.kinds.join(", ")));
    }
    out
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn status(args: &[String]) -> Result<()> {
    let root = root(args, false).ok();
    // (This Mac's own agent's folder, for what its own status adds (its memory, its view of the
    // pool): `--home`'s; with a `--root` of another build (a scratch one) and no `--home`, none:
    // that build's status alone, never this Mac's real agent's.)
    let own_home = opt(args, "--home")
        .map(PathBuf::from)
        .or_else(|| opt(args, "--root").is_none().then(|| app_home().join("agent")));
    let Some(st) = agent::read_status(root.as_deref(), own_home.as_deref().unwrap_or(Path::new("/nonexistent"))) else {
        bail!("no status yet: the build agent hasn't run (or the NAS isn't mounted)");
    };
    let c = &st.conditions;
    println!("Lead {} (app {}), seen {}", st.host, st.app, ago(st.beat));
    println!(
        "  {} · {} · {}",
        if c.ac { "on power" } else { "on battery" },
        if !c.nas {
            "NAS not reachable"
        } else if c.home {
            "NAS reachable"
        } else {
            "NAS reachable through Tailscale (away from home)"
        },
        if c.user_active() { "in use" } else { "idle" }
    );
    if now_s().saturating_sub(st.beat) > 600 {
        println!("  (not seen for a while: asleep, away or off; work waits for it)");
    }
    // This Mac's agent's loop stalled, as its watchdog says (agent::nasread::watch).
    if let Some(s) = own_home.as_deref().and_then(agent::nasread::stall) {
        println!("  this Mac's agent loop is stalled: no round finished since {}", ago(s.round_at));
    }
    // The memory guard as its thread last sampled (every 5 s, whatever the agent's loop waits on).
    if let Some(m) = own_home.as_deref().and_then(agent::memguard::live).filter(|m| now_s().saturating_sub(m.at) < 60) {
        let held: u64 = m.held_mb.iter().flatten().sum();
        let frozen = if m.frozen.iter().any(|f| *f) { ", a job frozen past it" } else { "" };
        println!(
            "  memory: the jobs hold {:.1} GB of this Mac's limit of {:.1} GB ({}{frozen}, {})",
            held as f64 / 1024.0,
            m.limit_mb as f64 / 1024.0,
            if m.on { "guarded" } else { "not guarded" },
            ago(m.at)
        );
    }
    // Each Mac, the lead (the coordinator) first: its slots, its capacity, its jobs, and why
    // nothing more fits (the same layout as the build page's machines).
    for l in macs_lines(&st) {
        println!("{l}");
    }
    for w in &st.waiting {
        println!("Waiting: {} — {}", w.what, w.why);
    }
    for n in &st.notices {
        println!("note: {} — {}", n.what, n.why);
    }
    // Publishing as it stands (the build page's Publishing card).
    for l in publishing_lines(&st) {
        println!("{l}");
    }
    // The timetables' to-do list and the routes matching nothing on the map (docs/inputs.md §6.7.3).
    if let Some(t) = &st.timetables {
        for l in t.lines() {
            println!("{l}");
        }
    }
    // The build to the end (the status's checklist, as the build page and the menu bar have it): a
    // line per stage, done, under way or to come, and under it a line per step, each the same:
    // its title, its targets done of all (or left) in its noun, and where it runs.
    if !st.checklist.is_empty() {
        let now: Vec<String> = jobs_of(&st).iter().filter_map(|j| agent::step_of(&j.id)).collect();
        let finished = st.checklist.iter().filter(|l| l.finished()).count();
        println!("To the end: {finished} of {} stages done (⇄: any Mac may take it)", st.checklist.len());
        for l in &st.checklist {
            let running = now.iter().any(|n| l.steps.contains(n));
            let mark = if l.finished() {
                "✓"
            } else if running {
                "▸"
            } else {
                "○"
            };
            println!(
                "  {mark} {}: {}{}",
                l.what,
                l.counts().join(" · "),
                l.note.as_deref().map(|n| format!(" — {n}")).unwrap_or_default()
            );
            if l.finished() {
                continue;
            }
            for i in &l.items {
                let noun = format!("{}s", agent::steps::noun(&i.step));
                let here = now.iter().any(|n| n == &i.step);
                let place = if agent::steps::place(&i.step) == agent::steps::Place::Any { " ⇄" } else { "" };
                println!(
                    "      {} {}: {}{place}",
                    if i.finished() {
                        "✓"
                    } else if here {
                        "▸"
                    } else {
                        "○"
                    },
                    agent::steps::title(&i.step),
                    count_line(i.done, i.total, i.left, &noun)
                );
            }
            for (k, j) in l.next.iter().take(4).enumerate() {
                println!(
                    "      {}: {j}",
                    if k > 0 {
                        "then"
                    } else if running {
                        "now"
                    } else {
                        "next"
                    }
                );
            }
        }
    }
    for d in st.recent.iter().take(5) {
        let how = match (d.ok, d.cancelled) {
            (true, _) => "Done:",
            (false, true) => "Cancelled:",
            (false, false) => "Failed:",
        };
        println!("{how} {} ({}, {} s)", d.what, ago(d.ended), d.secs);
        if d.cancelled {
            println!("    {}", d.note);
        } else if !d.ok {
            for l in d.note.lines().take(8) {
                println!("    {l}");
            }
        }
    }
    // The gate (docs/inputs.md §4.7): a line per unit held or checking.
    for v in st.inputs.iter().filter(|v| v.state != pipeline::inputs::view::State::Ok) {
        println!(
            "Inputs: {}{}",
            v.line(),
            if v.state == pipeline::inputs::view::State::Held { " (`scenic inputs` says what holds it)" } else { "" }
        );
    }
    println!(
        "Regions: {}",
        if st.regions.is_empty() {
            "none yet".to_string()
        } else {
            st.regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")
        }
    );
    // (Those still building: their z6 tiles built of their total, on the map as they come, and when
    // all of it is, as the forecast has it; and those with every z6 tile built but not yet recorded
    // built (publishing's held regions): finishing.)
    let held = |id: &str| {
        st.publishing.as_ref().is_some_and(|p| {
            p.regions.iter().any(|r| r.id == id && r.kinds.iter().any(|k| k != "new region" && k != "rebuilt"))
        })
    };
    let map_at = |id: &str| st.forecast.as_ref()?.regions.iter().find(|r| r.id == id)?.map_at;
    for r in &st.regions {
        if let Some(b) = st.built.get(&r.id).filter(|b| b.built < b.total || held(&r.id)) {
            println!("  {}: {}", r.name, building_line(b.built, b.total, map_at(&r.id)));
        }
    }
    for (f, e) in &st.bad_recipes {
        println!("  {f} isn't a valid region: {e}");
    }
    // The pool (docs/pool.md §10): who leads, each member, as this Mac's agent sees it (else as the
    // status read says).
    let own = own_home.as_deref().and_then(agent::lead::own_status).and_then(|s| s.pool).and_then(|p| p.lead);
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
        println!(
            "Worker: {} — {}{}{}",
            w.label,
            w.what,
            if w.done > 0 { format!(", {} done", w.done) } else { String::new() },
            if w.bad { ", stopped: a result differed" } else { "" }
        );
    }
    // (The lead's own record of it is over HTTPS once `tailscale serve` proxies the coordinator.)
    let page = own_home
        .as_ref()
        .and_then(|h| std::fs::read_to_string(h.join("coord/page")).ok())
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    let contact = root
        .as_ref()
        .and_then(|r| std::fs::read(pipeline::coord::contact_path(r)).ok())
        .and_then(|b| serde_json::from_slice::<pipeline::coord::Contact>(&b).ok());
    let page = match (page, contact) {
        (Some(p), _) => Some(p),
        (None, Some(c)) => c.urls.first().map(|u| format!("{u}/work/")),
        _ => None,
    };
    // The build's page (its dashboard, and where a device helps).
    if let Some(p) = page {
        println!("Build page: {p} (open it on a device on the LAN or the tailnet)");
    }
    // The map on an iPhone or an iPad (docs/macs.md, Devices): its address, which this Mac's
    // server writes.
    if let Some(m) = std::fs::read_to_string(app_home().join("map-page"))
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    {
        println!("Map on a device: {m} (open it once on an iPhone or an iPad on the tailnet)");
    }
    Ok(())
}

/// `scenic steps [--json]`: the steps table (crate::agent::steps, docs/pipeline.md), a stage at a
/// time, each step one line of the same columns; `--json` as `/work/steps` serves it.
fn steps(args: &[String]) -> Result<()> {
    use agent::steps::{self, Kind, Place, Scope};
    let rows = steps::table();
    if flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "steps": rows, "stages": steps::STAGES }))?);
        return Ok(());
    }
    let kind = |k: Kind| match k {
        Kind::Fetch => "fetch",
        Kind::Derive => "derive",
        Kind::Pack => "pack",
        Kind::Publish => "publish",
        Kind::Keep => "keep",
    };
    let each = |r: &steps::Info| match (&r.of, r.scope) {
        (Some(of), _) => format!("a task of {}", steps::title(of)),
        (None, Scope::World) => "the world".to_string(),
        (None, s) => format!("{} {}", if s == Scope::Input { "an" } else { "a" }, s.noun()),
    };
    for st in steps::STAGES {
        println!("{}: {}", st.title, st.does);
        for r in rows.iter().filter(|r| r.stage == st.id) {
            println!(
                "  {:<26} {:<8} {:<22} {:<8} {:<16} {}",
                r.title,
                kind(r.kind),
                each(r),
                if r.place == Place::Any { "any Mac" } else { "the lead" },
                format!("id {}", r.id),
                r.does
            );
        }
    }
    Ok(())
}

/// A step's line's count: "12 of 480 tiles", "3 runs left", "done", or "to come" (not sized yet).
fn count_line(done: usize, total: Option<usize>, left: Option<usize>, noun: &str) -> String {
    match (total, left) {
        (Some(t), _) if t > 0 => format!("{done} of {t} {noun}"),
        (_, Some(0)) => "done".into(),
        (_, Some(l)) => format!("{l} {} left", if l == 1 { noun.trim_end_matches('s') } else { noun }),
        _ => "to come".into(),
    }
}

/// A Mac's build caches in a line: what a clear would free (`scenic clean` there) or why not now,
/// and the last trim after the build and the last clear.
fn caches_line(c: &agent::room::Caches) -> String {
    let mut s =
        c.clearable.map_or_else(|| "not counted yet".to_string(), |n| format!("{} to clear", agent::room::size(n)));
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
            s += &format!(
                "; disk room target {} ({} free{})",
                agent::room::size(t.bytes),
                agent::room::size(r.free),
                if r.short.is_some() { ", short of it" } else { "" }
            );
        }
    }
    s
}

/// `scenic in-use [--cores <n>] | off`: this Mac's in-use share (agent::share), set in its agent's
/// folder, which the agent reads each loop; without --cores, how it stands.
fn in_use(args: &[String]) -> Result<()> {
    use agent::share;
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
    let say = |s: Option<&share::Share>| {
        println!(
            "In-use share: {} of {cores} cores kept free of this Mac's jobs while its owner uses it ({})",
            share::cores(s, cores),
            match s {
                Some(s) => format!("set {} by {}", ago(s.at), s.by),
                None => "the default".into(),
            }
        )
    };
    if args.iter().skip(2).any(|a| a == "off") {
        share::set(&home, None, "")?;
        say(None);
        return Ok(());
    }
    let n: Option<u32> = opt(args, "--cores")
        .map(|c| {
            c.parse().ok().filter(|&c: &u32| c < cores).with_context(|| format!("--cores: 0 to {}, not {c}", cores - 1))
        })
        .transpose()?;
    let Some(n) = n else {
        match share::read_checked(&home) {
            Ok(s) => say(s.as_ref()),
            Err(e) => {
                println!("{e:#}: the agent keeps the default");
                say(None);
            }
        }
        return Ok(());
    };
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    let by = format!("scenic in-use on {}", agent::cond::host_name());
    let s = share::set(&home, Some(n), &by)?;
    say(s.as_ref());
    Ok(())
}

/// `scenic nas-target [<percent> | off]`: the pool's NAS load target (pipeline::budget::nas::Target),
/// set in the project folder on the NAS (`state/pool/nas-target.json`), which the lead reads each
/// 10 s, whichever Mac set it; off: the default; with none, how it stands.
fn nas_target(args: &[String]) -> Result<()> {
    use pipeline::budget::nas;
    let r = root(args, true)?;
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
    let by = format!("scenic nas-target on {}", agent::cond::host_name());
    let t = match what {
        Some("off") => nas::set_target(&r, None, &by)?,
        Some(p) => {
            let pct: u32 = p
                .trim_end_matches('%')
                .parse()
                .with_context(|| format!("scenic nas-target <percent> | off, not {p}"))?;
            nas::set_target(&r, Some(pct), &by)?
        }
        None => nas::read_target(&r),
    };
    println!(
        "NAS target: the budget grants while the NAS's busiest disk is under {} % busy ({})",
        nas::target_pct(t.as_ref()),
        match &t {
            Some(t) => format!("set {} by {}", ago(t.at), t.by),
            None => "the default".into(),
        }
    );
    Ok(())
}

/// `scenic memory [<GB> | off]`: the memory this Mac gives the build's jobs together
/// (agent::buildmem), set in its agent's folder, which the agent reads each loop; within the memory
/// guard's limit; off: the limit alone; with none, how it stands.
fn memory(args: &[String]) -> Result<()> {
    use agent::buildmem;
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let by = format!("scenic memory on {}", agent::cond::host_name());
    let total_mb = pipeline::sys::memsize().map_or(0, |b| b >> 20);
    let limit = agent::memguard::limit_mb(total_mb);
    // (The first word after `memory` that isn't an option or an option's value: `--home X 10`.)
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
    let set = match what {
        Some("off") => {
            buildmem::set(&home, None, &by)?;
            None
        }
        Some(gb) => {
            let gb: f64 = gb
                .trim_end_matches("GB")
                .trim_end_matches("gb")
                .parse()
                .ok()
                .filter(|g: &f64| g.is_finite() && *g >= 1.0)
                .with_context(|| format!("scenic memory <GB> | off, not {gb}: 1 GB or more"))?;
            let mb = (gb * 1024.0) as u64;
            anyhow::ensure!(
                total_mb == 0 || mb <= total_mb,
                "{gb} GB is more than this Mac has ({:.0} GB)",
                total_mb as f64 / 1024.0
            );
            anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
            buildmem::set(&home, Some(mb), &by)?
        }
        None => buildmem::read(&home),
    };
    println!("{}", memory_line(set.as_ref(), limit, |t| ago(t)));
    Ok(())
}

/// The build's memory in a line: "Build memory: 10.0 GB for the build's jobs together, set 2 h ago
/// by scenic memory on m1 (the memory guard's limit, 12.0 GB, stays the ceiling)"; not set, the
/// limit's.
fn memory_line(set: Option<&agent::buildmem::Memory>, limit_mb: u64, ago: impl Fn(u64) -> String) -> String {
    let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
    let room = agent::buildmem::room_mb(set, limit_mb);
    match set {
        Some(m) => format!(
            "Build memory: {} for the build's jobs together, set {} by {}{}",
            gb(room),
            ago(m.at),
            m.by,
            match limit_mb {
                0 => String::new(),
                l if m.mb > l => format!(" ({} asked, held to the memory guard's limit)", gb(m.mb)),
                l => format!(" (the memory guard's limit, {}, stays the ceiling)", gb(l)),
            }
        ),
        None => format!(
            "Build memory: {} for the build's jobs together, the memory guard's limit (`scenic memory <GB>` sets less)",
            gb(room)
        ),
    }
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
            let gb: f64 = gb
                .trim_end_matches("GB")
                .trim_end_matches("gb")
                .parse()
                .ok()
                .filter(|g: &f64| g.is_finite() && *g >= 1.0)
                .with_context(|| format!("scenic room <GB> | off, not {gb}: a target of 1 GB or more"))?;
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
                    if let Some(f) = r.toward.as_ref().filter(|f| t.as_ref().is_some_and(|t| f.target == Some(t.bytes)))
                    {
                        let past = f
                            .goal
                            .map(|g| {
                                format!(
                                    " (and the {} a job waiting for it needs past it)",
                                    size(g.saturating_sub(f.target.unwrap_or(0)))
                                )
                            })
                            .unwrap_or_default();
                        println!("Freed toward it{past} {}: {}", ago(f.at), f.say());
                    }
                    if let Some(why) = &r.short {
                        println!("Short of it: {why}");
                    }
                    for w in st.iter().flat_map(|s| &s.waiting).filter(|w| w.why.contains("disk room target")) {
                        println!("Waiting: {} — {}", w.what, w.why);
                    }
                }
                None if t.is_some() => println!(
                    "(this Mac's agent isn't running, or runs an older app: it keeps the target once it runs this one)"
                ),
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

/// This Mac's own agent's status, as it writes it in its folder `home`: the lead's `status.json`
/// or a member's `helper.json`, whichever is fresher.
fn own_status(home: &Path) -> Option<agent::Status> {
    ["status.json", "helper.json"]
        .iter()
        .filter_map(|f| serde_json::from_slice::<agent::Status>(&std::fs::read(home.join(f)).ok()?).ok())
        .max_by_key(|s| s.beat)
}

/// `scenic clean`: an ask to this Mac's agent to clear its build caches (agent::room::clear), and
/// what it says it freed, once it has.
fn clean(args: &[String]) -> Result<()> {
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    let st = own_status(&home).context("this Mac's agent has no status: is it installed here?")?;
    // (Not written while it makes room for a job, minutes at most.)
    anyhow::ensure!(
        now_s().saturating_sub(st.beat) < 6 * 60,
        "this Mac's agent hasn't written its status since {}: is it running?",
        ago(st.beat)
    );
    let caches =
        st.caches.as_ref().context("this Mac's agent runs an older app, which doesn't clear its caches on an ask")?;
    if let Some(why) = &caches.why_not {
        bail!("not now: {why}");
    }
    // What goes and how it comes back, and a y/N: a clear is a hundred GB on the M4.
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
        let answered = own_status(&home)
            .and_then(|s| s.caches)
            .and_then(|c| [c.cleared, c.declined].into_iter().flatten().find(|f| f.asked == Some(r.at)));
        match answered {
            Some(f) => {
                if let Some(why) = f.why_not {
                    bail!("not cleared: {why}");
                }
                println!("{}", f.say());
                return Ok(());
            }
            None if t.elapsed() > std::time::Duration::from_secs(3 * 3600) => bail!(
                "no word from the agent in three hours; it has the ask still (`scenic status` says when it's done)"
            ),
            None => {}
        }
    }
}

/// `scenic job cancel <target>`: an ask to this Mac's agent (pipeline::control::Cancel), followed
/// until it says what it stopped.
fn job_cancel(args: &[String]) -> Result<()> {
    let home = opt(args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
    // (Its words, the options and their values aside: `job cancel <target>`.)
    let mut pos: Vec<&str> = Vec::new();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if matches!(a.as_str(), "--home" | "--step") {
            it.next();
        } else if !a.starts_with("--") {
            pos.push(a);
        }
    }
    let target = match pos.as_slice() {
        ["job", "cancel", t] => *t,
        _ => bail!("scenic job cancel <target> [--step <step>] [--rerun-after-update]"),
    };
    let host = agent::cond::host_name();
    let c = pipeline::control::Cancel::new(
        target,
        opt(args, "--step").as_deref(),
        flag(args, "--rerun-after-update"),
        &format!("scenic job cancel on {host}"),
    );
    pipeline::control::request_cancel(&home, &c)?;
    let t = std::time::Instant::now();
    loop {
        if let Some(a) = pipeline::control::cancel_answer(&home, &c) {
            println!("{}", a.said);
            anyhow::ensure!(a.hit, "nothing cancelled");
            return Ok(());
        }
        if t.elapsed() > std::time::Duration::from_secs(90) {
            bail!("no word from this Mac's agent in 90 s; it has the ask still (`scenic status` says what it did)");
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // (Its flags and arguments checked first: `--help` or a flag it doesn't know runs nothing.)
    let cmd = pipeline::cli::tables::SCENIC.gate(&args).name.split(' ').next().unwrap_or_default().to_string();
    match cmd.as_str() {
        "status" => status(&args),
        "lead" => lead(&args),
        "agent" => {
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            let home = opt(&args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
            let o = Options {
                root: opt(&args, "--root").map(PathBuf::from),
                home,
                bin,
                dry_run: flag(&args, "--dry-run"),
                once: flag(&args, "--once"),
                helper: false,
            };
            eprintln!(
                "agent: started (app {}, root {})",
                o.bin.display(),
                o.root.as_ref().map(|r| r.display().to_string()).unwrap_or_else(|| "the NAS share".into())
            );
            let extra = agent::cond::test_disk_extra_mb();
            if extra > 0 {
                eprintln!("agent: WARNING: SCENIC_TEST_DISK_EXTRA_MB: this agent counts {extra} MB more free on its disk than there is (the two-Mac test's alone, never a real agent's)");
            }
            pipeline::sys::raise_open_files();
            let mut a = agent::Agent::new(o)?;
            a.run()?;
            // (Quit as Scenic asked: the launcher ends with it rather than starting it again.)
            if a.quit_done() {
                drop(a);
                std::process::exit(pipeline::control::QUIT_EXIT);
            }
            Ok(())
        }
        "pool" => {
            // The pool (docs/pool.md): how it stands, and the owner's asks of its lead.
            let r = root(&args, false)?;
            match args.get(2).map(String::as_str) {
                Some("status") | None => println!("{}", agent::pool::status(&r)),
                Some("release") => {
                    // A job's progress on a member's Mac given up (crate::agent::resume): the lead
                    // runs it wherever it fits from then.
                    let (Some(m), Some(job)) = (args.get(3), args.get(4)) else {
                        anyhow::bail!("scenic pool release <member> \"<job>\" (e.g. \"osm-pass 2026-10-01\")")
                    };
                    println!("{}", agent::resume::release(&r, m, job)?);
                }
                Some("needs") => {
                    // Each step's model of what its jobs need (pipeline::needs): `--fit` prints its
                    // fits' errors on held-out runs and the steps that don't fit. Read only: the
                    // coordinator's costs (`--costs <costs.json | costs.jsonl>`, else this Mac's
                    // agent's), its disk floors beside them.
                    anyhow::ensure!(
                        flag(&args, "--fit"),
                        "scenic pool needs --fit [--costs <costs.json | costs.jsonl>]"
                    );
                    let p = opt(&args, "--costs")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| app_home().join("agent/coord/costs.json"));
                    let (costs, mut disks) =
                        pipeline::needs::read_costs(&p).with_context(|| format!("reading {}", p.display()))?;
                    let kept: std::collections::BTreeMap<String, u64> = std::fs::read(p.with_file_name("disks.json"))
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default();
                    disks.extend(kept);
                    println!("{} ({} costs, {} disk measures)\n", p.display(), costs.len(), disks.len());
                    print!("{}", pipeline::needs::fit_table(&pipeline::needs::Book::with(&costs, None, Some(&disks))));
                    // Where each step's memory peaks: the phase (a program it runs) that held the
                    // most over its last runs, from the build's timings beside the costs
                    // (pipeline::timings::peak_holders).
                    let runs = pipeline::timings::read_log(&p.with_file_name("timings.jsonl"));
                    let holders = pipeline::timings::peak_holders(&runs, 20);
                    if !holders.is_empty() {
                        println!(
                            "\nwhere each step's memory peaks (its phase holding the most, its last 20 timed runs):"
                        );
                        print!("{}", pipeline::timings::peak_text(&holders));
                    }
                }
                Some("nas-probe") => {
                    // The NAS budget's probe once (pipeline::budget::nas), in `--dir <folder>` (made,
                    // then removed with what it holds), else a folder of its own under the project
                    // folder's `state/pool/nas-probe`.
                    let dir = opt(&args, "--dir")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| pipeline::budget::nas::probe_dir(&r, &agent::cond::host_name()));
                    anyhow::ensure!(!dir.exists(), "{} exists: the probe makes its own folder", dir.display());
                    let p =
                        pipeline::budget::nas::probe(&dir).with_context(|| format!("probing in {}", dir.display()))?;
                    println!(
                        "{}: {:.1} MB/s (written and read back), {:.1} creates/s, {:.0} entries listed/s, {:.1} stats/s, {:.1} deletes/s",
                        dir.display(),
                        p.bytes_s / 1e6,
                        p.creates_s,
                        p.listed_s,
                        p.stats_s,
                        p.deletes_s
                    );
                }
                Some("floors") => {
                    // The targets' floors the lead's coordinator keeps (crate::agent::memguard):
                    // `--clear` clears them all, `--clear <step>` a step's, `--clear <step> <target>`
                    // one target's (a step fixed, or a floor learned wrong).
                    let clear = flag(&args, "--clear");
                    let rest: Vec<&String> = args
                        .iter()
                        .skip(3)
                        .filter(|a| !a.starts_with("--") && Some(*a) != opt(&args, "--root").as_ref())
                        .collect();
                    let c = pipeline::coord::client::Client::from_nas(&r, &agent::cond::host_name())?
                        .context("no lead's coordinator to ask")?;
                    let (code, v) = c.post_json(
                        "/work/floors",
                        &serde_json::json!({ "clear": clear, "step": rest.first(), "target": rest.get(1) }),
                    )?;
                    anyhow::ensure!(code == 200, "the lead's coordinator answered {code} (an older app?)");
                    let floors: std::collections::BTreeMap<String, pipeline::agent::memguard::Floor> =
                        serde_json::from_value(v["floors"].clone())?;
                    if floors.is_empty() {
                        println!("no floors");
                    }
                    for (k, f) in floors {
                        println!(
                            "{k}: {:.1} GB{}{}",
                            f.mb as f64 / 1024.0,
                            if f.alone { ", held alone" } else { ", in a batch" },
                            if f.v > 0 { format!(", v{}", f.v) } else { String::new() }
                        );
                    }
                    // (The disk floors: what each target was seen to take on the disk at least.)
                    let disks: std::collections::BTreeMap<String, u64> =
                        serde_json::from_value(v["disks"].clone()).unwrap_or_default();
                    for (k, mb) in disks {
                        println!("{k}: disk {:.1} GB", mb as f64 / 1024.0);
                    }
                }
                Some(x) => bail!(
                    "scenic pool status | release <member> \"<job>\" | floors [--clear [<step> [<target>]]], not {x}"
                ),
            }
            Ok(())
        }
        "run-task" => {
            // A task a member's agent leased (pipeline::offload::run_task), run here; its result
            // written for the agent to hand back.
            let spec: serde_json::Value =
                serde_json::from_slice(&std::fs::read(opt(&args, "--spec").context("--spec <file>")?)?)?;
            let lease: u64 = opt(&args, "--lease").context("--lease <id>")?.parse()?;
            let (dir, result) = (
                PathBuf::from(opt(&args, "--dir").context("--dir <folder>")?),
                PathBuf::from(opt(&args, "--result").context("--result <file>")?),
            );
            let urls: Vec<String> = std::env::var("SCENIC_COORD_URLS")
                .unwrap_or_default()
                .split(',')
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .collect();
            let client = pipeline::coord::client::Client::at(
                urls,
                std::env::var("SCENIC_COORD_TOKEN").unwrap_or_default(),
                &std::env::var("SCENIC_WORKER").unwrap_or_default(),
            );
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            let root = opt(&args, "--root").map(PathBuf::from);
            // (Timed as a task of its kind: a tail's, a 3D buildings' z3 tile's, tree cover's blocks'.)
            let kind = format!("task {}", pipeline::offload::task_kind(&spec));
            let r = pipeline::timings::job(&kind, || {
                pipeline::offload::run_task(&client, lease, &spec, &dir, &bin, root.as_deref())
            });
            std::fs::remove_dir_all(&dir).ok();
            let v = match &r {
                Ok(t) => serde_json::json!({ "ok": true, "task": t }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            pipeline::whole::write(&result, v.to_string().as_bytes())?;
            r.map(|_| ())
        }
        "pause" | "resume" => {
            // An ask to this Mac's agent, which passes it on to the lead's (crate::control).
            use pipeline::control::{Mode, Pause};
            let home = opt(&args, "--home").map(PathBuf::from).unwrap_or_else(|| app_home().join("agent"));
            let host = pipeline::agent::cond::host_name();
            // One Mac alone (task #224): this one (`--here`) or a member named.
            let named: Option<&String> = {
                let mut it = args.iter().skip_while(|a| **a != cmd).skip(1);
                let mut found = None;
                while let Some(a) = it.next() {
                    if a == "--home" {
                        it.next();
                    } else if !a.starts_with("--") {
                        found = Some(a);
                        break;
                    }
                }
                found
            };
            if flag(&args, "--here") || named.is_some() {
                anyhow::ensure!(
                    !flag(&args, "--now"),
                    "--now pauses the whole build: one Mac alone stops at its next safe point"
                );
                let held = (cmd == "pause").then(|| pipeline::control::Hold::new(&format!("scenic pause on {host}")));
                let r = pipeline::control::request_hold(&home, named.map(String::as_str), held)?;
                let waited = std::time::Instant::now();
                while waited.elapsed() < std::time::Duration::from_secs(60) {
                    if let Some(a) = pipeline::control::hold_answer(&home, &r) {
                        anyhow::ensure!(a.ok, "{}", a.said);
                        println!(
                            "{}{}",
                            a.said,
                            if cmd == "pause" {
                                ": its running jobs stop at their next safe point and are given back, it takes no new work (leading, it keeps the lead and grants the others' work), until `scenic resume`; the rest of the pool goes on"
                            } else {
                                ""
                            }
                        );
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                bail!("this Mac's agent hasn't taken the ask up in a minute (is it running?); it stays asked");
            }
            let pause = (cmd == "pause").then(|| {
                Pause::new(
                    if flag(&args, "--now") { Mode::Freeze } else { Mode::Drain },
                    &format!("scenic pause on {host}"),
                )
            });
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
        "job" => job_cancel(&args),
        "clean" => clean(&args),
        "room" => room(&args),
        "in-use" => in_use(&args),
        "nas-target" => nas_target(&args),
        "memory" => memory(&args),
        "gc" => {
            let days: u64 = opt(&args, "--days").map(|d| d.parse()).transpose()?.unwrap_or(14);
            let r = pipeline::timings::job("gc", || gc::run(&root(&args, true)?, days, flag(&args, "--dry-run")))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        "backup" => {
            let local = opt(&args, "--local").map(PathBuf::from);
            let today = backup::format_day(std::time::SystemTime::now());
            let r =
                pipeline::timings::job("backup", || backup::run(&root(&args, true)?, local.as_deref(), &today, 30))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        "timings" => timings(&args),
        "steps" => steps(&args),
        "inputs" => inputs(&args),
        c => bail!("{c}: in the table (pipeline::cli::tables::SCENIC) but not run here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A member's capacity line shows its jobs' disk reservations (task #242), and none when it has
    /// none.
    #[test]
    fn the_capacity_line_shows_the_disk_reserved() {
        let c = pipeline::coord::Capacity { disk_mb: 12 << 10, reserved_mb: 8 << 10, ..Default::default() };
        assert!(capacity_line(&c).contains("disk 12.0 GB free (8.0 GB reserved)"), "{}", capacity_line(&c));
        let c = pipeline::coord::Capacity { disk_mb: 12 << 10, ..Default::default() };
        assert!(!capacity_line(&c).contains("reserved"));
    }

    /// A region still building in words, as the Regions panel and the menu bar have it (task #258).
    #[test]
    fn a_region_still_building_says_its_tiles() {
        assert_eq!(building_line(3, 8, None), "3 of 8 tiles, on the map as they come");
        assert_eq!(building_line(8, 8, None), "finishing: 8 of 8 tiles, the last map tiles and layers being made");
        assert_eq!(
            building_line(3, 8, Some(now_s() + 40 * 60)),
            "3 of 8 tiles, on the map as they come; all ≈ in 40 min"
        );
    }

    /// Publishing in `scenic status` as the build page's card says it (task #259): what it does,
    /// the last catalog and its kind, the map tiles to draw by Mac, the cadenced publishes next,
    /// each Mac's catalog adopted.
    #[test]
    fn publishing_says_the_last_catalog_the_tiles_by_mac_the_next_and_each_macs_catalog() {
        let now = now_s();
        let seen = |n: u64, ago: u64| agent::CatalogSeen { n, at: now - ago };
        let fc = |what: &str, at: u64| agent::forecast::PublishFc { what: what.into(), at };
        let st = agent::Status {
            publishing: Some(agent::build::PublishLine {
                why: "drawing the map tiles of the units built".into(),
                tiles: 18,
                ..Default::default()
            }),
            published: Some(agent::Published {
                last: Some(seen(812, 120)),
                low: false,
                drawing: [("m1".to_string(), 6), ("m4".to_string(), 12)].into(),
                adopted: [("m1".to_string(), seen(811, 200)), ("m4".to_string(), seen(812, 100))].into(),
            }),
            forecast: Some(agent::forecast::Forecast {
                publishes: vec![fc("low", now + 7200), fc("marks", now + 3000), fc("overlays", now)],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            publishing_lines(&st),
            [
                "Publishing: drawing the map tiles of the units built",
                "  last catalog: 812, 2 min ago, map tiles only",
                "  map tiles to draw: 18 (being drawn: m1: 6 tiles, m4: 12 tiles)",
                "  next: the low zooms in 2.0 h, the landmarks in 50 min, the area overlays now",
                "  adopted: m1: catalog 811 (3 min ago), m4: catalog 812 (1 min ago)",
            ]
        );
    }

    /// `scenic status` shows each Mac as the build page does (task #246): the lead marked
    /// coordinator with its ramp, each Mac's capacity, NAS share, a line per job (0 to 4 of them)
    /// and why nothing more fits.
    #[test]
    fn status_shows_each_mac_its_jobs_and_why_nothing_more_fits() {
        let job = |k: usize| agent::JobView {
            id: format!("unit 6/{k}/1"),
            what: format!("job {k}"),
            started: now_s(),
            paused: None,
            pausing: None,
            tail: String::new(),
            parts: Vec::new(),
            part: None,
            progress: None,
            mem_mb: Some(2048),
            threads: Some(3),
            waiting: None,
            slot: Some(k),
        };
        // (The lead on the new wire; its member on the old one, its jobs not saying their slots.)
        let with = |host: &str, n: usize, all_new: bool| {
            let mut st = agent::Status { host: host.into(), ..Default::default() };
            st.set_jobs((0..n).map(job).collect(), all_new);
            if !all_new {
                st.jobs.clear();
                for j in st.job.iter_mut().chain(st.beside.iter_mut()).chain(st.more.iter_mut()) {
                    j.slot = None;
                }
            }
            st
        };
        for n in 0..=4 {
            let mut st = with("m4", n, true);
            assert!(st.job.is_none() && st.beside.is_none() && st.more.is_empty(), "the old fields not written");
            st.helpers = vec![with("m1", 4 - n, false)];
            st.capacity = vec![("m4".into(), pipeline::coord::Capacity { limit_mb: 20 << 10, ..Default::default() })];
            st.packing = [("m1".to_string(), "cores: 0 of 8 left".to_string())].into();
            st.nas_share = [("m4".to_string(), 250), ("m1 #2".to_string(), 100)].into();
            st.ramp = Some(agent::RampView {
                slots: 2,
                reached: 2,
                most: 12,
                not_clean: Some("a stall".into()),
                ..Default::default()
            });
            let l = macs_lines(&st).join("\n");
            assert!(l.starts_with(&format!("m4 · lead, coordinator · {n} job")), "{l}");
            assert!(l.contains("2 of 12 slots (ramp reached 2) · not clean: a stall"), "{l}");
            assert!(!l.contains("held:"), "{l}");
            assert!(
                l.contains("capacity: memory") && l.contains("NAS share 25 %") && l.contains("NAS share 10 %"),
                "{l}"
            );
            assert_eq!(l.matches("    slot ").count(), 4, "{l}");
            assert_eq!(l.contains("slot 4: job 3"), n == 4 || n == 0, "{l}");
            assert_eq!(l.matches("running nothing").count(), usize::from(n == 0 || n == 4), "{l}");
            assert!(l.contains("m1 · member") && l.contains("nothing more fits: cores: 0 of 8 left"), "{l}");
        }
        // Sparse slots: the first and the fourth running, numbered by their slots, a job's tail
        // lines under it.
        let mut fourth = job(3);
        fourth.tail = "tail line".into();
        let mut st = agent::Status { host: "m4".into(), ..Default::default() };
        st.set_jobs(vec![job(0), fourth], true);
        let l = macs_lines(&st).join("\n");
        assert!(l.contains("slot 1: job 0") && l.contains("slot 4: job 3") && !l.contains("slot 2:"), "{l}");
        assert!(l.contains("        tail line"), "{l}");
        // Task #423: this Mac's real part. A member says so; one out of the pool says why; a member
        // building out of the pool, too.
        let pooled = |role: &str| -> agent::PoolView {
            serde_json::from_value(serde_json::json!({
                "member": "m-0000000000000001", "role": role, "gates": {}, "members": [], "unacked": 0
            }))
            .unwrap()
        };
        let mut st = agent::Status { host: "m1".into(), pool: Some(pooled("member")), ..Default::default() };
        assert!(macs_lines(&st)[0].starts_with("m1 · member · 0 jobs running"), "{:?}", macs_lines(&st));
        st.pool = Some(pooled("lead"));
        assert!(macs_lines(&st)[0].starts_with("m1 · lead, coordinator · "), "{:?}", macs_lines(&st));
        st.pool = None;
        st.out_of_pool = Some("its member's lock is held".into());
        assert!(
            macs_lines(&st)[0].starts_with("m1 · not in the pool: waiting to join (its member's lock is held) · "),
            "{:?}",
            macs_lines(&st)
        );
        let mut lead = agent::Status { host: "m4".into(), pool: Some(pooled("lead")), ..Default::default() };
        lead.helpers = vec![st];
        let l = macs_lines(&lead).join("\n");
        assert!(l.contains("m1 · not in the pool: waiting to join (its member's lock is held) · 0 jobs"), "{l}");
    }

    /// The build's memory in words (task #390), and a Mac's capacity with it and its disk said
    /// plainly (task #391).
    #[test]
    fn the_build_memory_and_the_disk_are_said_plainly() {
        let m = agent::buildmem::Memory { mb: 10 << 10, by: "scenic memory on m1".into(), at: 1 };
        let ago = |_| "2 h ago".to_string();
        assert_eq!(
            memory_line(Some(&m), 12 << 10, ago),
            "Build memory: 10.0 GB for the build's jobs together, set 2 h ago by scenic memory on m1 (the memory guard's limit, 12.0 GB, stays the ceiling)"
        );
        assert_eq!(
            memory_line(Some(&m), 8 << 10, ago),
            "Build memory: 8.0 GB for the build's jobs together, set 2 h ago by scenic memory on m1 (10.0 GB asked, held to the memory guard's limit)"
        );
        assert!(memory_line(None, 12 << 10, ago)
            .starts_with("Build memory: 12.0 GB for the build's jobs together, the memory guard's limit"));
        let c = pipeline::coord::Capacity {
            limit_mb: 12 << 10,
            build_mb: 10 << 10,
            mem_mb: 6 << 10,
            free_mb: 39_014,
            disk_mb: 18 << 10,
            target_mb: 20 << 10,
            ..Default::default()
        };
        let l = capacity_line(&c);
        assert!(
            l.starts_with("memory 6.0 GB free of the build's 10.0 GB (0.0 GB held, the limit 12.0 GB, not guarded)"),
            "{l}"
        );
        assert!(l.contains("disk 38 GB free · 18 GB for jobs (20 GB room target)"), "{l}");
    }
}
