//! `scenic`: the user's command (docs/surface.md, docs/building.md §2).
//!
//!   scenic status                       what the lead is doing, what waits and why, regions
//!   scenic agent [--once] [--dry-run] [--home <dir>]  the agent (every Mac's login item: its
//!                                       member of the pool, leading when it's the declared lead;
//!                                       --helper, of before, accepted and ignored)
//!   scenic pool [status]                the pool as it stands (docs/pool.md): the terms, the agents
//!   scenic lead                         who leads the build (the declared lead, the Mac mini), its
//!                                       term, since when, and each member (docs/pool.md §11)
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
//!   scenic clean [--deep] [--yes]       clear this Mac's build caches now (what later jobs copy
//!                                       back cheaply from the NAS; --deep: also the heritage clip
//!                                       and the Wikidata and Wikipedia answers, costly to make
//!                                       again; jobs running keep what they use), after a y/N
//!                                       (--yes: none): its agent does it, and says what it freed
//!                                       and kept, and why
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
//!   scenic inputs check [<unit>] [--full]  the inputs folders listed now (the lead's agent checks what
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
//! `--root <dir>` points at the NAS project folder (default: the mounted share). The command line
//! is pipeline::cli::scenic's: `--help` (or `-h`) on any command prints its usage and does nothing;
//! an unknown command, flag or argument is refused before anything runs.

use anyhow::{bail, Context, Result};
use clap::Parser;
use pipeline::agent::{self, backup, gc, jobs::now_s, Options};
use pipeline::cli::scenic::{self as cli, Cmd};
use std::path::{Path, PathBuf};

/// `~/Library/Application Support/scenic`.
fn app_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/scenic")
}

/// This Mac's agent's folder: `--home`'s, else the app's.
fn home_or_default(home: &Option<PathBuf>) -> PathBuf {
    home.clone().unwrap_or_else(|| app_home().join("agent"))
}

/// The NAS project folder: `--root`'s, else the share (mounted first if `mount`).
fn root(root: &Option<PathBuf>, mount: bool) -> Result<store::nasfs::NasRoot> {
    match root {
        Some(r) => Ok(store::nasfs::NasRoot::from_arg(r.clone())),
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
fn timings(a: &cli::Timings) -> Result<()> {
    use pipeline::timings::{read_log, summary, summary_text, RunRec};
    if let Some(cli::TimingsCmd::Publish { last, file, json, places }) = &a.cmd {
        return timings_publish(*last, file.as_deref(), *json, places);
    }
    let (kind, last) = (a.kind.as_deref(), a.last);
    let home = home_or_default(&a.places.home);
    // The build's log: this Mac's coordinator's, else the coordinator's answer; --here, this Mac's
    // own jobs'.
    let (runs, from): (Vec<RunRec>, String) = if let Some(f) = &a.file {
        (read_log(f), f.display().to_string())
    } else if a.here {
        (read_log(&home.join("timings.jsonl")), "this Mac's jobs".into())
    } else if home.join("coord/timings.jsonl").exists() {
        (read_log(&home.join("coord/timings.jsonl")), "the build's jobs (this Mac's coordinator)".into())
    } else {
        let asked = (|| -> Result<Vec<RunRec>> {
            let root = root(&a.places.root, false)?;
            let c = pipeline::coord::client::Client::from_nas(&root, &store::sys::host_name())?
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
    let sums = summary(&runs, kind, last, a.host.as_deref());
    if a.json {
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
/// servers' log on the NAS: each catalogue of the last N (10) and its z6 tiles, then the medians.
fn timings_publish(last: usize, file: Option<&Path>, json: bool, places: &cli::Places) -> Result<()> {
    use pipeline::pubtimes;
    let home = home_or_default(&places.home);
    let root = root(&places.root, false).ok();
    let days = 8;
    let events = match file {
        Some(f) => pubtimes::load_history(None, Some(f), days),
        None => pubtimes::load_history(root.as_ref(), Some(&home.join("coord/history.jsonl")), days),
    };
    let servers =
        root.as_ref().map(|r| pubtimes::read_servers(r, now_s().saturating_sub(days * 86400))).unwrap_or_default();
    let (cats, units) = pubtimes::chains(&events, &servers);
    if json {
        let from = cats.len().saturating_sub(last);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "catalogs": &cats[from..], "units": units }))?
        );
        return Ok(());
    }
    if cats.is_empty() {
        println!("no catalogues in the build's history yet ({} events read)", events.len());
        return Ok(());
    }
    println!("Publishing, the last {last} catalogues: each unit's hand-off, its map tiles planned, leased and packed, the catalogue, each Mac's server and mirror\n");
    print!("{}", pubtimes::text(&cats, &units, last));
    Ok(())
}

/// `scenic lead`: the pool's lead as this Mac's agent sees it (docs/pool.md §11): the lead is
/// fixed (the member `state/build/lead-member.json` declares), so there's nothing to ask of it.
fn lead(a: &cli::Lead) -> Result<()> {
    use pipeline::agent::lead as l;
    let home = home_or_default(&a.home);
    let v = l::own_status(&home)
        .and_then(|s| s.pool)
        .and_then(|p| p.lead)
        .context("this Mac's agent isn't in the pool (`scenic pool status`), or hasn't said yet")?;
    for line in l::said(&v, now_s()) {
        println!("{line}");
    }
    Ok(())
}

/// `scenic inputs`: the gate (docs/inputs.md §4.5, §4.7), from the records on the NAS.
fn inputs(a: &cli::Inputs) -> Result<()> {
    use cli::{InputsCmd, OnOff};
    use pipeline::inputs::{self as gi, view};
    let home = home_or_default(&a.home);
    let r = root(&a.root, false)?;
    let manifest = pipeline::out::read_manifest(&r)?;
    let units = gi::units(&r);
    // (Checking, as the lead's status says.)
    let status = agent::read_status(Some(&r), &home);
    let checking: std::collections::BTreeSet<String> = status
        .as_ref()
        .map(|s| s.inputs.iter().filter(|v| v.checking).map(|v| v.unit.clone()).collect())
        .unwrap_or_default();
    // (A running check's job on the lead, for its progress: "Checking the <unit> inputs", or from
    // a lead older than task #502, "Checking the inputs dropped in <unit>".)
    let running: Vec<(String, agent::JobView)> = status
        .iter()
        .flat_map(|s| s.jobs())
        .filter_map(|j| {
            let unit = j.what.strip_prefix("Checking the ")?;
            let unit = unit.strip_suffix(", every file").unwrap_or(unit);
            let unit = unit.strip_prefix("inputs dropped in ").or_else(|| unit.strip_suffix(" inputs"))?;
            Some((unit.to_string(), j))
        })
        .collect();
    let views = view::of(&r, &manifest, &units, &checking, &Default::default(), &mut view::Cache::default());
    let by = format!("scenic inputs on {}", store::sys::host_name());
    let unit_arg = |u: &str| -> Result<String> {
        anyhow::ensure!(
            gi::checks(u).is_some() || views.iter().any(|v| v.unit == u),
            "{u} isn't a gate unit (scenic inputs lists them)"
        );
        Ok(u.to_string())
    };
    match &a.cmd {
        None | Some(InputsCmd::List) => {
            if views.is_empty() {
                println!("no unit is on the gate (`scenic inputs test on` puts the test unit on it)");
            }
            for v in &views {
                println!(
                    "{}: {}{}",
                    v.unit,
                    match v.state {
                        view::State::Ok => "taken in",
                        view::State::Checking => "checking",
                        view::State::Held => "held",
                    },
                    v.checked.map(|t| format!(", checked {}", ago(t))).unwrap_or_default()
                );
                println!("  version: {}", v.version.as_deref().unwrap_or("none yet"));
                if v.checking {
                    match running.iter().find(|(u, _)| *u == v.unit) {
                        Some((_, j)) => println!("  {}", check_line(j)),
                        None => println!("  check: planned, not running yet"),
                    }
                }
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
                    println!(
                        "  {} {}: {}",
                        if f.level == gi::Level::Error { "error" } else { "warning" },
                        f.id,
                        f.message
                    );
                    for (n, l) in f.lines.iter().take(10) {
                        println!("      line {n}: {l}");
                    }
                    let more = f.lines.len().saturating_sub(10) + s.more;
                    if more > 0 {
                        println!("      … and {more} more (in its report)");
                    }
                }
                for id in &v.accepted {
                    println!(
                        "  accepted: {id}{}",
                        if v.stale.contains(id) { " (stale: no check raises it any more)" } else { "" }
                    );
                }
            }
            Ok(())
        }
        Some(InputsCmd::Check { unit, full }) => {
            let full = *full;
            let which: Vec<String> = match unit {
                Some(u) => vec![unit_arg(u)?],
                None => units.iter().map(|u| u.to_string()).collect(),
            };
            for u in &which {
                gi::ask(
                    &home,
                    &gi::Ask { unit: u.clone(), check: true, full, by: by.clone(), at: now_s(), ..Default::default() },
                )?;
            }
            println!("asked this Mac's agent to list the inputs folders now{}: the lead checks what changed (`scenic inputs` says how it stands)", if full { ", hashing every file" } else { "" });
            Ok(())
        }
        Some(InputsCmd::Accept { unit, id, all }) => {
            let u = unit_arg(unit)?;
            let v = views.iter().find(|v| v.unit == u).context("nothing held")?;
            let report =
                gi::Report { findings: v.findings.iter().map(|s| s.finding.clone()).collect(), ..Default::default() };
            let ask = match (all, id) {
                (true, _) => gi::Ask { unit: u.clone(), all: true, by: by.clone(), at: now_s(), ..Default::default() },
                (false, Some(id)) => gi::Ask {
                    unit: u.clone(),
                    accept: vec![id.clone()],
                    by: by.clone(),
                    at: now_s(),
                    ..Default::default()
                },
                (false, None) => bail!("scenic inputs accept <unit> <finding id>|--all"),
            };
            let said = gi::apply_ask(&r, &ask, Some(&report), &gi::member_of(&home))?;
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
        Some(InputsCmd::Unaccept { unit, id, stale }) => {
            let u = unit_arg(unit)?;
            let ids: Vec<String> = match (stale, id) {
                (true, _) => views.iter().find(|v| v.unit == u).map(|v| v.stale.clone()).unwrap_or_default(),
                (false, Some(id)) => vec![id.clone()],
                (false, None) => bail!("scenic inputs unaccept <unit> <finding id>|--stale"),
            };
            for id in &ids {
                println!(
                    "{}",
                    if gi::unaccept(&r, &u, id)? {
                        format!("unaccepted {id}")
                    } else {
                        format!("{id} wasn't accepted")
                    }
                );
            }
            if ids.is_empty() {
                println!("no stale acceptance");
            }
            gi::ask(&home, &gi::Ask { unit: u, check: true, by, at: now_s(), ..Default::default() })?;
            Ok(())
        }
        Some(InputsCmd::Test { state, forget }) => {
            let p = r.join(gi::TEST_FLAG);
            match state {
                Some(OnOff::On) => {
                    p.parent().unwrap().create_dir_all()?;
                    p.write_whole(format!("turned on by {by}\n"))?;
                    println!(
                        "on: {} is on the gate (its inputs folder inputs/{}/, .jsonl files of {{\"k\": …, \"v\": …}} lines)",
                        gi::TEST_UNIT,
                        gi::TEST_UNIT
                    );
                }
                Some(OnOff::Off) => {
                    if let Err(e) = p.remove_file() {
                        anyhow::ensure!(e.kind() == std::io::ErrorKind::NotFound, "{e}");
                    }
                    if *forget {
                        // Its acceptances removed here; its records by the lead (a job of the
                        // lead's: the records are its to write), asked through this Mac's agent.
                        let acc = r.join(gi::ACCEPTED).join(gi::TEST_UNIT);
                        if let Err(e) = acc.remove_dir_all() {
                            anyhow::ensure!(e.kind() == std::io::ErrorKind::NotFound, "remove {}: {e}", acc.display());
                        }
                        gi::ask(
                            &home,
                            &gi::Ask {
                                unit: gi::TEST_UNIT.into(),
                                forget: true,
                                by: by.clone(),
                                at: now_s(),
                                ..Default::default()
                            },
                        )?;
                        println!("off: {} is off the gate, its acceptances removed; the lead removes its records from the build's (its copies go with GC's 14 days; its inputs folder is yours)", gi::TEST_UNIT);
                    } else {
                        println!("off: {} is off the gate and no longer shown (its records stay: `scenic inputs test off --forget` removes them)", gi::TEST_UNIT);
                    }
                }
                None => println!("{}", if p.exists() { "on" } else { "off" }),
            }
            Ok(())
        }
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

/// A running input check in a line: "check: running since 40 min ago, stage 1 of 5: reading and
/// checking the files (33%), about 9 min left, 6 threads at its start". Its progress is the stage
/// it is in (`inputs::gate::decide`'s `STAGES`) and how far within it.
fn check_line(j: &agent::JobView) -> String {
    let mut v = vec![format!("check: running since {}", ago(j.started))];
    if let Some(p) = j.progress.as_ref().filter(|p| p.total > 0.0) {
        let stage = p.unit.split_once(" (").map_or(p.unit.as_str(), |(_, w)| w.trim_end_matches(')'));
        v.push(format!(
            "stage {} of {}: {stage} ({}%)",
            p.done.floor() as u64,
            p.total,
            (p.done.fract() * 100.0).floor()
        ));
        if let Some(e) = p.eta_s {
            v.push(format!("about {} min left", e.div_ceil(60)));
        }
    } else {
        v.push("no progress said yet".into());
    }
    if let Some(t) = j.threads {
        v.push(format!("{t} threads at its start (more as cores free up)"));
    }
    v.join(", ")
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

/// Publishing as it stands (the build page's map KPI says its last catalogue): what it does and
/// why (agent::build::PublishLine), the last catalogue, the map tiles to draw and by which Mac, the
/// cadenced publishes next (agent::forecast), each Mac's catalogue adopted (agent::Published).
fn publishing_lines(st: &agent::Status) -> Vec<String> {
    let mut out = Vec::new();
    let Some(u) = &st.publishing else { return out };
    out.push(format!("Publishing: {}", u.why));
    let p = st.published.clone().unwrap_or_default();
    if let Some(c) = p.last {
        let kind = if p.low { "with the low zooms" } else { "map tiles only" };
        out.push(format!("  last catalogue: {}, {}, {kind}", c.n, ago(c.at)));
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
        let a: Vec<String> = p.adopted.iter().map(|(h, c)| format!("{h}: catalogue {} ({})", c.n, ago(c.at))).collect();
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

fn status(places: &cli::Places) -> Result<()> {
    let root = root(&places.root, false).ok();
    // (This Mac's own agent's folder, for what its own status adds (its memory, its view of the
    // pool): `--home`'s; with a `--root` of another build (a scratch one) and no `--home`, none:
    // that build's status alone, never this Mac's real agent's.)
    let own_home = places.home.clone().or_else(|| places.root.is_none().then(|| app_home().join("agent")));
    let Some(st) = agent::read_status(root.as_ref(), own_home.as_deref().unwrap_or(Path::new("/nonexistent"))) else {
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
    // Publishing as it stands (the build page's map KPI has its last catalogue).
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
                    agent::steps::current(&i.step),
                    count_line(i.done, i.total, i.left, &noun)
                        + &if i.empty > 0 { format!(" ({} with nothing to publish)", i.empty) } else { String::new() }
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
    // Each Mac's build caches (agent::room): what a clear would free, the last clear.
    for (host, c) in std::iter::once((&st.host, &st.caches)).chain(st.helpers.iter().map(|h| (&h.host, &h.caches))) {
        if let Some(c) = c {
            println!("Caches on {host}: {}", caches_line(c));
        }
    }
    if let Some(r) = &root {
        if let Ok(Some(cat)) = store::catalogue::latest(&r.join(store::catalogue::NAME)) {
            println!("Map data: catalogue {} of {}, {} units", cat.n, cat.created, cat.units.len());
        }
    }
    // Other workers.
    for w in &st.workers {
        println!(
            "Worker: {} — {}{}",
            w.label,
            w.what,
            if w.done > 0 { format!(", {} done", w.done) } else { String::new() }
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
        .and_then(|r| pipeline::coord::contact_path(r).read().ok())
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
fn steps(json: bool) -> Result<()> {
    use agent::steps::{self, Kind, Place, Scope};
    let rows = steps::table();
    if json {
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
        (Some(of), _) => format!("a task of {of}"),
        (None, Scope::World) => "the world".to_string(),
        (None, s) => format!("{} {}", if s == Scope::Input { "an" } else { "a" }, s.noun()),
    };
    for st in steps::STAGES {
        println!("{}: {}", st.id, st.does);
        for r in rows.iter().filter(|r| r.stage == st.id) {
            println!(
                "  {:<16} {:<8} {:<22} {:<8} {}",
                r.id,
                kind(r.kind),
                each(r),
                if r.place == Place::Any { "any Mac" } else { "the lead" },
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
/// and the last clear.
fn caches_line(c: &agent::room::Caches) -> String {
    let mut s = c.clearable.map_or_else(
        || "not counted yet".to_string(),
        |n| {
            let deep = c.clearable_deep.map(|d| format!(", {} deeply", agent::room::size(d))).unwrap_or_default();
            format!("{} to clear{deep}", agent::room::size(n))
        },
    );
    s += &match &c.why_not {
        Some(w) => format!(" (not now: {w})"),
        None => " (`scenic clean [--deep]` there)".to_string(),
    };
    if let Some(f) = &c.cleared {
        s += &format!("; cleared {}: {}", ago(f.at), f.say());
    }
    if let Some(f) = &c.cleared_deep {
        s += &format!("; cleared deeply {}: {}", ago(f.at), f.say());
    }
    let last = [&c.cleared, &c.cleared_deep].into_iter().flatten().map(|f| f.at).max();
    if let Some(f) = c.declined.as_ref().filter(|d| last.is_none_or(|at| d.at > at)) {
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
fn in_use(cores_asked: Option<u32>, off: bool, home: &Option<PathBuf>) -> Result<()> {
    use agent::share;
    let home = home_or_default(home);
    let cores = agent::settings::cores();
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
    if off {
        say(agent::settings::set_in_use(&home, None, "")?.as_ref());
        return Ok(());
    }
    let Some(n) = cores_asked else {
        match share::read_checked(&home) {
            Ok(s) => say(s.as_ref()),
            Err(e) => {
                println!("{e:#}: the agent keeps the default");
                say(None);
            }
        }
        return Ok(());
    };
    let by = format!("scenic in-use on {}", store::sys::host_name());
    let s = agent::settings::set_in_use(&home, Some(n), &by)?;
    say(s.as_ref());
    Ok(())
}

/// `scenic nas-target [<percent> | off]`: the pool's NAS load target (pipeline::budget::nas::Target),
/// set in the project folder on the NAS (`state/pool/nas-target.json`), which the lead reads each
/// 10 s, whichever Mac set it; off: the default; with none, how it stands.
fn nas_target(what: Option<&str>, root_: &Option<PathBuf>) -> Result<()> {
    use pipeline::budget::nas;
    let r = root(root_, true)?;
    let by = format!("scenic nas-target on {}", store::sys::host_name());
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

/// `scenic nas-room [<GB> | off]`: the NAS reserve (agent::room::NAS_RESERVE), set in the project
/// folder on the NAS, which every job reads; off: none; with none, how it stands.
fn nas_room(what: Option<&str>, root_: &Option<PathBuf>) -> Result<()> {
    use agent::room::{nas_reserve, size};
    let r = root(root_, true)?;
    let by = format!("scenic nas-room on {}", store::sys::host_name());
    if let Some(what) = what {
        agent::settings::set_nas_room(&r, what, &by)?;
    }
    match nas_reserve(&r) {
        Some(t) => println!("NAS reserve: {} (set {} by {})", size(t.bytes), ago(t.at), t.by),
        None => println!("NAS reserve: off (`scenic nas-room <GB>` sets one)"),
    }
    if let Ok(f) = pipeline::agent::room::nas_free(&r) {
        println!("Free on the NAS now: {}", size(f));
    }
    Ok(())
}

/// `scenic memory [<GB> | off]`: the memory this Mac gives the build's jobs together
/// (agent::buildmem), set in its agent's folder, which the agent reads each loop; within the memory
/// guard's limit; off: the limit alone; with none, how it stands.
fn memory(what: Option<&str>, home: &Option<PathBuf>) -> Result<()> {
    use agent::buildmem;
    let home = home_or_default(home);
    let by = format!("scenic memory on {}", store::sys::host_name());
    let total_mb = pipeline::sys::memsize().map_or(0, |b| b >> 20);
    let limit = agent::memguard::limit_mb(total_mb);
    let set = match what {
        Some(what) => agent::settings::set_memory(&home, what, &by)?,
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
fn room(what: Option<&str>, home: &Option<PathBuf>) -> Result<()> {
    use agent::room::{size, target};
    let home = home_or_default(home);
    let by = format!("scenic room on {}", store::sys::host_name());
    match what {
        Some("off") => {
            agent::settings::set_room(&home, "off", &by)?;
            println!("the disk room target is off: this Mac's agent fills its caches again as its jobs need");
        }
        Some(gb) => {
            let free = agent::room::disk_free(&home).ok();
            let bytes = agent::settings::set_room(&home, gb, &by)?.map_or(0, |t| t.bytes);
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

/// This Mac's own agent's status, as it writes it in its folder `home`: the lead's `status.json`
/// or a member's `helper.json`, whichever is fresher.
fn own_status(home: &Path) -> Option<agent::Status> {
    ["status.json", "helper.json"]
        .iter()
        .filter_map(|f| serde_json::from_slice::<agent::Status>(&std::fs::read(home.join(f)).ok()?).ok())
        .max_by_key(|s| s.beat)
}

/// `scenic clean [--deep]`: an ask to this Mac's agent to clear its build caches, as far as the level
/// (agent::room::clear, Level), and what it says it freed and kept, once it has.
fn clean(yes: bool, deep: bool, home: &Option<PathBuf>) -> Result<()> {
    use agent::room::Level;
    let level = if deep { Level::Deep } else { Level::Caches };
    let home = home_or_default(home);
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
    let n = if deep { caches.clearable_deep } else { caches.clearable }.unwrap_or(0);
    println!("This Mac's build caches{}, {} in all:", if deep { ", deeply" } else { "" }, agent::room::size(n));
    for g in caches.each.iter().filter(|g| deep || !g.deep) {
        println!("  {} {}: {}", g.what, agent::room::size(g.bytes), g.back);
    }
    println!("{}", agent::room::kept_words(level));
    if !yes {
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
    let r = agent::room::request_clear(&home, &format!("scenic clean on {}", store::sys::host_name()), level)?;
    println!("asked this Mac's agent to clear them; waiting for it…");
    let t = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let answered = own_status(&home).and_then(|s| s.caches).and_then(|c| {
            [c.cleared, c.cleared_deep, c.declined].into_iter().flatten().find(|f| f.asked == Some(r.at))
        });
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
fn job_cancel(target: &str, step: Option<&str>, rerun_after_update: bool, home: &Option<PathBuf>) -> Result<()> {
    let home = home_or_default(home);
    let host = store::sys::host_name();
    let c = pipeline::control::Cancel::new(target, step, rerun_after_update, &format!("scenic job cancel on {host}"));
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

/// `scenic redo <step> <target>… [--run]` (pipeline::agent::redo): what the newest records hold for
/// the targets; with --run, an ask of the lead's coordinator (`/work/redo`), followed until its merge
/// lands in the records.
fn redo(step: &str, targets: &[String], run: bool, root_: &Option<PathBuf>) -> Result<()> {
    use pipeline::agent::redo;
    use pipeline::pool::{records::Records, term};
    let (step, targets) = redo::checked(step, targets)?;
    let r = root(root_, false)?;
    let mut records = || -> Result<Records> {
        let cur = term::current(&r).context("the current term")?;
        Records::newest(&r, cur.term, true)?.context("no records")
    };
    let rec = records()?;
    for l in redo::said(&rec.keys, &rec.manifest, &step, &targets) {
        println!("{l}");
    }
    let todo: Vec<String> = redo::clears(&rec.keys, &step, &targets)
        .into_iter()
        .filter(|(_, k)| k.is_some())
        .map(|(t, _)| t.to_string())
        .collect();
    if todo.is_empty() {
        println!("nothing to clear");
        return Ok(());
    }
    if !run {
        println!("a dry run: `scenic redo {step} {} --run` asks the lead to clear them", todo.join(" "));
        return Ok(());
    }
    let c = pipeline::coord::client::Client::from_nas(&r, &store::sys::host_name())?
        .context("no lead's coordinator to ask")?;
    let by = format!("scenic redo on {}", store::sys::host_name());
    let (code, v) = c.post_json("/work/redo", &serde_json::json!({ "step": step, "targets": todo, "by": by }))?;
    anyhow::ensure!(
        code == 200,
        "the lead's coordinator answered {code} ({v}): a lead on an app from before `scenic redo` can't take it; nothing asked"
    );
    println!("asked the lead (redo {}); its job clears them, merged within minutes", v["n"]);
    // The redo's job is leased after `after`; an older lead's answer lacks it: each target's mark.
    let floor = match serde_json::from_value::<(u64, u64)>(serde_json::json!([v["after"]["term"], v["after"]["n"]])) {
        Ok((term, n)) => vec![Some(pipeline::pool::journal::LeaseId { term, n }); todo.len()],
        Err(_) => redo::marks(&rec, &step, &todo),
    };
    let landed = redo::follow(
        &mut records,
        &step,
        &todo,
        &floor,
        std::time::Duration::from_secs(15 * 60),
        std::time::Duration::from_secs(15),
        &mut |e| eprintln!("the records can't be read now: {e:#}"),
    );
    if landed {
        println!("merged: the plan offers the {} target(s) again, as it builds them (`scenic status`)", todo.len());
        return Ok(());
    }
    bail!("the redo wasn't merged into the records in 15 minutes: `scenic status` shows its job; a lead that changed meanwhile may have lost the ask (ask again: `scenic redo {step} {} --run`)", todo.join(" "))
}

/// `scenic redo <step> --restore <file> [--run]` (task #832, pipeline::agent::restore): the
/// restores `file` lists (`scenic-build terrain-restore-check --ask`), those whose record in the
/// newest records is still the one checked; with --run, asked of the lead's coordinator
/// (`/work/redo`), `redo::MAX_TARGETS` at a time: its jobs record each under its key now while its
/// record is still that one (`build::Keys::record`), so a target built since keeps its own.
fn restore(step: &str, file: &Path, run: bool, root_: &Option<PathBuf>) -> Result<()> {
    use pipeline::agent::redo;
    use pipeline::pool::term;
    let all: Vec<redo::Restore> =
        serde_json::from_slice(&std::fs::read(file).with_context(|| format!("read {}", file.display()))?)
            .with_context(|| format!("parse {}", file.display()))?;
    let r = root(root_, false)?;
    let cur = term::current(&r).context("the current term")?;
    let rec = pipeline::pool::records::Records::newest(&r, cur.term, true)?.context("no records")?;
    let step = pipeline::agent::steps::current(step).to_string();
    let (todo, moved): (Vec<redo::Restore>, Vec<redo::Restore>) =
        all.into_iter().partition(|x| rec.keys.recorded(&step, &x.target) == Some(x.from.as_str()));
    println!(
        "{} {step} target(s) to restore; {} recorded otherwise since the check (left as they are)",
        todo.len(),
        moved.len()
    );
    if todo.is_empty() || !run {
        if !todo.is_empty() {
            println!("a dry run: `scenic redo {step} --restore {} --run` asks the lead", file.display());
        }
        return Ok(());
    }
    let c = pipeline::coord::client::Client::from_nas(&r, &store::sys::host_name())?
        .context("no lead's coordinator to ask")?;
    let by = format!("scenic redo --restore on {}", store::sys::host_name());
    for part in todo.chunks(redo::MAX_TARGETS) {
        let (code, v) = c.post_json("/work/redo", &serde_json::json!({ "step": step, "restore": part, "by": by }))?;
        anyhow::ensure!(code == 200, "the lead's coordinator answered {code} ({v}): nothing more asked");
        println!("asked the lead (redo {}): {} target(s), recorded as its job merges", v["n"], part.len());
    }
    Ok(())
}

fn main() -> Result<()> {
    // (Its command line checked first: `--help` or a flag it doesn't know runs nothing.)
    let cli = cli::Cli::parse();
    // (A catalogue read through deltas kept on this Mac for the next read: store::catalogue::keep_read_in.)
    if let Some(d) = store::catalogue::kept_dir() {
        store::catalogue::keep_read_in(d);
    }
    match cli.cmd {
        None => status(&cli.status),
        Some(Cmd::Status(places)) => status(&places),
        Some(Cmd::Menu(places)) => {
            // (`--home` here is the app's folder's agent folder, as everywhere: its parent read.)
            let home = places.home.as_ref().and_then(|h| h.parent().map(Path::to_path_buf)).unwrap_or_else(app_home);
            let root = root(&places.root, false).ok().map(|r| r.short_retry());
            println!("{}", pipeline::menu::status(&home, pipeline::menu::lead_heartbeat(root.as_ref()), now_s()));
            Ok(())
        }
        Some(Cmd::Lead(a)) => lead(&a),
        Some(Cmd::Agent { once, dry_run, helper: _, places }) => {
            let bin = std::env::current_exe()?.parent().map(Path::to_path_buf).context("the agent's folder")?;
            let o = Options {
                home: home_or_default(&places.home),
                root: places.root.map(store::nasfs::NasRoot::from_arg),
                bin,
                dry_run,
                once,
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
            // (Every Mac's app kept current from here, the map server's Mac too: pipeline::appupdate.)
            if places.home.is_none() {
                pipeline::appupdate::spawn(app_home(), o.root.clone());
            }
            let mut a = agent::Agent::new(o)?;
            a.run()?;
            // (Quit as Scenic asked: the launcher ends with it rather than starting it again.)
            if a.quit_done() {
                drop(a);
                std::process::exit(pipeline::control::QUIT_EXIT);
            }
            Ok(())
        }
        Some(Cmd::Pool(pool)) => {
            use cli::PoolCmd;
            // The pool (docs/pool.md): how it stands, and the owner's asks of its lead.
            let r = root(&pool.root, false)?;
            match pool.cmd {
                Some(PoolCmd::Status) | None => println!("{}", agent::pool::status(&r)),
                Some(PoolCmd::Release { member, job: Some(job) }) => {
                    // A job's progress on a member's Mac given up (crate::agent::resume): the lead
                    // runs it wherever it fits from then.
                    println!("{}", agent::resume::release(&r, &member, &job)?);
                }
                Some(PoolCmd::Release { member, job: None }) => {
                    // A member's leases released by the owner (docs/coordination.md §6.1,
                    // crate::coord::held): its targets go to others.
                    let c = pipeline::coord::client::Client::from_nas(&r, &store::sys::host_name())?
                        .context("no lead's coordinator to ask")?;
                    let (code, v) = c.post_json("/pool/release", &serde_json::json!({ "member": member }))?;
                    anyhow::ensure!(code == 200, "the lead's coordinator answered {code} (an older app?)");
                    let gone = v["released"].as_array().cloned().unwrap_or_default();
                    println!("{member}: {} lease(s) released", gone.len());
                    for l in gone {
                        println!("  {}", l.as_str().unwrap_or(""));
                    }
                }
                Some(PoolCmd::Needs { fit: _, costs }) => {
                    // Each step's model of what its jobs need (pipeline::needs): `--fit` prints its
                    // fits' errors on held-out runs and the steps that don't fit. Read only: the
                    // coordinator's costs (`--costs <costs.json | costs.jsonl>`, else this Mac's
                    // agent's), its disk floors beside them.
                    let p = costs.unwrap_or_else(|| app_home().join("agent/coord/costs.json"));
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
                Some(PoolCmd::JournalGc { repair }) => {
                    // The journal GC's marker against the newest records (crate::pool::journal_gc::check).
                    let nas = r.clone();
                    println!(
                        "{}",
                        pipeline::pool::journal_gc::check(&nas, now_s(), repair, &|| {
                            std::thread::sleep(std::time::Duration::from_secs(pipeline::pool::journal_gc::REREAD_S))
                        })?
                    );
                }
                Some(PoolCmd::NasProbe { dir }) => {
                    // The NAS budget's probe once (pipeline::budget::nas), in `--dir <folder>` (made,
                    // then removed with what it holds), else a folder of its own under the project
                    // folder's `state/pool/nas-probe`.
                    let dir = dir.unwrap_or_else(|| pipeline::budget::nas::probe_dir(&r, &store::sys::host_name()));
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
                Some(PoolCmd::Floors { clear, step, target }) => {
                    // The targets' floors the lead's coordinator keeps (crate::agent::memguard):
                    // `--clear` clears them all, `--clear <step>` a step's, `--clear <step> <target>`
                    // one target's (a step fixed, or a floor learned wrong).
                    let c = pipeline::coord::client::Client::from_nas(&r, &store::sys::host_name())?
                        .context("no lead's coordinator to ask")?;
                    let (code, v) = c.post_json(
                        "/work/floors",
                        &serde_json::json!({ "clear": clear, "step": step, "target": target }),
                    )?;
                    anyhow::ensure!(code == 200, "the lead's coordinator answered {code} (an older app?)");
                    if clear {
                        // (What the running coordinator took out of its memory: the clear reached it.)
                        println!(
                            "the lead's coordinator cleared {} floor(s) and {} disk floor(s)",
                            v["cleared"]["floors"].as_u64().unwrap_or(0),
                            v["cleared"]["disks"].as_u64().unwrap_or(0)
                        );
                    }
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
            }
            Ok(())
        }
        Some(Cmd::RunTask { spec, lease, dir, result, root: cli::Root { root } }) => {
            // A task a member's agent leased (pipeline::offload::run_task), run here; its result
            // written for the agent to hand back.
            let spec: serde_json::Value = serde_json::from_slice(&std::fs::read(spec)?)?;
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
            // (Timed as a task of its kind: a tail's, a 3D buildings' z3 tile's, tree cover's blocks'.)
            let kind = format!("task {}", pipeline::offload::task_kind(&spec));
            let r = pipeline::timings::job(&kind, || {
                pipeline::offload::run_task(
                    &client,
                    lease,
                    &spec,
                    &dir,
                    &bin,
                    root.map(store::nasfs::NasRoot::from_arg).as_ref(),
                )
            });
            std::fs::remove_dir_all(&dir).ok();
            let v = match &r {
                Ok(t) => serde_json::json!({ "ok": true, "task": t }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            store::whole::write(&result, v.to_string().as_bytes())?;
            // (A step stopped as the build pauses: the task exits as paused, given back by its
            // agent, not failed.)
            if r.as_ref().is_err_and(pipeline::control::is_paused) {
                std::process::exit(pipeline::control::PAUSED_EXIT);
            }
            r.map(|_| ())
        }
        Some(Cmd::Pause { now, here, member, home }) => pause_resume(true, now, here, member.as_deref(), &home.home),
        Some(Cmd::Resume { here, member, home }) => pause_resume(false, false, here, member.as_deref(), &home.home),
        Some(Cmd::Job(cli::Job::Cancel { target, step, rerun_after_update, home })) => {
            job_cancel(&target, step.as_deref(), rerun_after_update, &home.home)
        }
        Some(Cmd::Clean { yes, deep, home }) => clean(yes, deep, &home.home),
        Some(Cmd::Room { what, places }) => room(what.as_deref(), &places.home),
        Some(Cmd::InUse { cores, off, places }) => in_use(cores, off.is_some(), &places.home),
        Some(Cmd::NasTarget { what, root }) => nas_target(what.as_deref(), &root.root),
        Some(Cmd::NasRoom { what, root }) => nas_room(what.as_deref(), &root.root),
        Some(Cmd::Memory { what, places }) => memory(what.as_deref(), &places.home),
        Some(Cmd::Gc { dry_run, days, depth_keep, root: r }) => {
            // (The depth's targets not read: said, and no depth name dropped this run.)
            let keep = depth_keep.as_deref().and_then(|p| {
                gc::DepthKeep::read(p, now_s())
                    .map_err(|e| eprintln!("gc: no depth pack dropped: {e:#}"))
                    .ok()
                    .flatten()
            });
            let r = pipeline::timings::job("gc", || gc::run(&root(&r.root, true)?, days, dry_run, keep.as_ref()))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Some(Cmd::Redo { step, run, restore: Some(f), root: r, .. }) => restore(&step, &f, run, &r.root),
        Some(Cmd::Redo { step, targets, run, root: r, .. }) => redo(&step, &targets, run, &r.root),
        Some(Cmd::Backup { local, root: r }) => {
            let today = backup::format_day(std::time::SystemTime::now());
            let r =
                pipeline::timings::job("backup", || backup::run(&root(&r.root, true)?, local.as_deref(), &today, 30))?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Some(Cmd::Timings(a)) => timings(&a),
        Some(Cmd::Steps { json }) => steps(json),
        Some(Cmd::Inputs(a)) => inputs(&a),
        Some(Cmd::ForecastCheck { from, to, replay, split, json, root: r }) => {
            forecast_check(from, to, replay.then_some(split), json, &r.root)
        }
        Some(Cmd::Catalogues { after, home }) => {
            use pipeline::agent::cataloguesview::{CataloguesView, KEPT};
            let kept = home_or_default(&home.home).join(KEPT);
            println!("{}", serde_json::to_string(&CataloguesView::answer_kept(&kept, after))?);
            Ok(())
        }
    }
}

/// `scenic pause` (`pause`) or `scenic resume`: an ask to this Mac's agent, which passes it on to
/// the lead's (crate::control); `--here` or a member: that Mac alone (task #224).
fn pause_resume(pause: bool, now: bool, here: bool, member: Option<&str>, home: &Option<PathBuf>) -> Result<()> {
    use pipeline::control::{Mode, Pause};
    let home = home_or_default(home);
    let host = store::sys::host_name();
    if here || member.is_some() {
        let held = pause.then(|| pipeline::control::Hold::new(&format!("scenic pause on {host}")));
        let r = pipeline::control::request_hold(&home, member, held)?;
        let waited = std::time::Instant::now();
        while waited.elapsed() < std::time::Duration::from_secs(60) {
            if let Some(a) = pipeline::control::hold_answer(&home, &r) {
                anyhow::ensure!(a.ok, "{}", a.said);
                println!(
                    "{}{}",
                    a.said,
                    if pause {
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
    let p = pause.then(|| Pause::new(if now { Mode::Freeze } else { Mode::Drain }, &format!("scenic pause on {host}")));
    pipeline::control::request(&home, p)?;
    println!(
        "{}",
        match (pause, now) {
            (true, false) => "pausing: each Mac's running job stops at its next safe point (within minutes), and nothing new starts until `scenic resume`",
            (true, true) => "pausing now: each Mac's running job is frozen where it is, until `scenic resume`",
            (false, _) => "going on: the build picks up where it stopped",
        }
    );
    Ok(())
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
    /// the last catalogue and its kind, the map tiles to draw by Mac, the cadenced publishes next,
    /// each Mac's catalogue adopted.
    #[test]
    fn publishing_says_the_last_catalogue_the_tiles_by_mac_the_next_and_each_macs_catalogue() {
        let now = now_s();
        let seen = |n: u64, ago: u64| agent::CatalogueSeen { n, at: now - ago };
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
                "  last catalogue: 812, 2 min ago, map tiles only",
                "  map tiles to draw: 18 (being drawn: m1: 6 tiles, m4: 12 tiles)",
                "  next: the low zooms in 2.0 h, the landmarks in 50 min, the area overlays now",
                "  adopted: m1: catalogue 811 (3 min ago), m4: catalogue 812 (1 min ago)",
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
            live: None,
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

/// `scenic forecast-check`: the archived forecasts made from `from` to `to` held to the history's
/// jobs that ended then (agent::fccheck); with `replay` (its test set's start, if said), the
/// backtest of the terms' forecast inputs made then.
fn forecast_check(
    from: Option<String>,
    to: Option<String>,
    replay: Option<Option<String>>,
    json: bool,
    r: &Option<PathBuf>,
) -> Result<()> {
    use pipeline::agent::{fcarchive, fccheck};
    let now = now_s();
    let at = |s: &Option<String>, or: u64| -> Result<u64> {
        match s {
            Some(s) => fccheck::parse_time(s, now)
                .with_context(|| format!("{s}: not a time (unix s, YYYY-MM-DD[THH:MM], or 12h, 3d back)")),
            None => Ok(or),
        }
    };
    let (from, to) = (at(&from, now.saturating_sub(7 * 86_400))?, at(&to, now)?);
    anyhow::ensure!(from < to, "--from is not before --to");
    let root = root(r, false)?;
    // (A day before `from` too: what each step's pace reads.)
    let days = now.saturating_sub(from) / 86_400 + 2;
    let events = fccheck::dedup(pipeline::pubtimes::load_history(Some(&root), None, days));
    if let Some(split) = replay {
        let records = fccheck::records(&root, from, to);
        anyhow::ensure!(!records.is_empty(), "no forecast input recorded in the span");
        let last = records.iter().map(|r| r.input.now).max().unwrap_or(to);
        let split = at(&split, last.saturating_sub(86_400))?;
        let b = fccheck::backtest(&records, &events, from, to, split);
        if json {
            println!("{}", serde_json::to_string_pretty(&b)?);
        } else {
            print!("{}", fccheck::backtest_text(&b));
        }
        return Ok(());
    }
    let entries = fcarchive::read(&root, from, to);
    let rep = fccheck::check(&entries, &events, from, to);
    if json {
        println!("{}", serde_json::to_string_pretty(&rep)?);
    } else {
        print!("{}", fccheck::text(&rep));
    }
    Ok(())
}
