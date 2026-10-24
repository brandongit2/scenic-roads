//! `scenic forecast-check` (docs/pool.md §7.9, task #621): the archived forecasts (crate::agent::fcarchive)
//! held to what happened (the history's jobs, crate::coord::history).
//!
//! A job done is a history's worker job (its "lease", then its "done" ok, by lease) or a lead's
//! ("end" ok: from its time less its `secs`): its Mac (the worker's name, a lead's slot's `#k`
//! cut), step, targets, start and end. A forecast
//! placed targets (step, target): each is held to the first job of that step and target that ended
//! after the forecast was made. The targets of one job are taken together: the forecast's start the
//! earliest of theirs, its end the latest, its Mac the first's lane's. So each (forecast, job) is a
//! sample: its end's error (forecast less actual: + late, − early) where the job ended after the
//! forecast, its start's where it started after it; its horizon the forecast's time until it
//! (from when it was made). Errors are said by step, by Mac (the Mac that ran it), by horizon and
//! by the forecast's code version: n, bias (the mean), the median and the 90th percentile of the
//! absolute error.

use super::fcarchive::Entry;
use crate::coord::history::Event;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

/// A job done, as the history says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Actual {
    pub mac: String,
    pub step: String,
    pub targets: Vec<String>,
    pub start: u64,
    pub end: u64,
}

/// The jobs done in `events`: workers' (lease to done) and the lead's (end, less its time), those
/// that ended well and name their targets; by end.
pub fn actuals(events: &[Event]) -> Vec<Actual> {
    let mut leased: BTreeMap<u64, u64> = BTreeMap::new();
    let mut out = Vec::new();
    for e in events {
        match e.kind.as_str() {
            "lease" => {
                if let Some(l) = e.lease {
                    leased.insert(l, e.t);
                }
            }
            "done" | "end" if e.ok == Some(true) && !e.targets.is_empty() => {
                let (Some(mac), Some(step)) = (&e.worker, &e.step) else { continue };
                let back = e.t.saturating_sub(e.secs.unwrap_or(0.0).max(0.0) as u64);
                let start = e.lease.and_then(|l| leased.get(&l).copied()).unwrap_or(back);
                // (A lead's slot past its first is named `<Mac> #k` in the history, as its lane.)
                let mac = mac.split(" #").next().unwrap_or(mac).to_string();
                out.push(Actual { mac, step: step.clone(), targets: e.targets.clone(), start, end: e.t });
            }
            _ => {}
        }
    }
    out.sort_by_key(|a| a.end);
    out
}

/// A forecast's error on a job's start or end.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub end: bool,
    pub step: String,
    pub mac: String,
    pub code: u32,
    /// How far ahead the forecast's time was (s from when it was made), and its error (s, forecast
    /// less actual).
    pub horizon_s: u64,
    pub err_s: f64,
}

/// n, the mean error, the median and 90th percentile of the absolute error (s).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stats {
    pub n: usize,
    pub bias_s: f64,
    pub median_abs_s: f64,
    pub p90_abs_s: f64,
}

impl Stats {
    fn of(errs: &[f64]) -> Stats {
        if errs.is_empty() {
            return Stats::default();
        }
        let mut a: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
        a.sort_by(f64::total_cmp);
        let q = |p: f64| a[((a.len() as f64 * p).ceil() as usize).clamp(1, a.len()) - 1];
        Stats {
            n: errs.len(),
            bias_s: (errs.iter().sum::<f64>() / errs.len() as f64).round(),
            median_abs_s: q(0.5).round(),
            p90_abs_s: q(0.9).round(),
        }
    }
}

/// The start's or the end's errors, overall and by group.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Errors {
    pub all: Stats,
    pub by_horizon: Vec<(String, Stats)>,
    pub by_step: BTreeMap<String, Stats>,
    pub by_mac: BTreeMap<String, Stats>,
    pub by_code: BTreeMap<u32, Stats>,
}

/// The horizons errors are grouped by: up to (s), and their names.
pub const HORIZONS: [(u64, &str); 5] =
    [(900, "< 15 min"), (3600, "15 min-1 h"), (4 * 3600, "1-4 h"), (12 * 3600, "4-12 h"), (u64::MAX, "12 h +")];

impl Errors {
    fn of(s: &[&Sample]) -> Errors {
        let errs = |f: &dyn Fn(&Sample) -> bool| -> Vec<f64> { s.iter().filter(|x| f(x)).map(|x| x.err_s).collect() };
        let mut e = Errors { all: Stats::of(&errs(&|_| true)), ..Default::default() };
        let mut lo = 0;
        for (hi, name) in HORIZONS {
            let st = Stats::of(&errs(&|x| x.horizon_s >= lo && x.horizon_s < hi));
            if st.n > 0 {
                e.by_horizon.push((name.to_string(), st));
            }
            lo = hi;
        }
        let group = |key: &dyn Fn(&Sample) -> String| -> BTreeMap<String, Stats> {
            let mut m: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            for x in s {
                m.entry(key(x)).or_default().push(x.err_s);
            }
            m.into_iter().map(|(k, v)| (k, Stats::of(&v))).collect()
        };
        e.by_step = group(&|x| x.step.clone());
        e.by_mac = group(&|x| x.mac.clone());
        e.by_code = group(&|x| x.code.to_string()).into_iter().map(|(k, v)| (k.parse().unwrap_or(0), v)).collect();
        e
    }
}

/// The check's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub from: u64,
    pub to: u64,
    /// Forecasts archived in the span, and jobs the history says were done in it.
    pub forecasts: usize,
    pub jobs_done: usize,
    /// Of those jobs, how many some forecast placed, and the share of the (forecast, job) pairs
    /// whose forecast had the Mac that ran it.
    pub jobs_forecast: usize,
    pub same_mac: f64,
    /// Targets forecast to end before the history's last event that no job did by then (a forecast
    /// each): work forecast that didn't happen as placed (dropped, failed, or under another target).
    pub not_done: usize,
    pub not_done_by_step: BTreeMap<String, usize>,
    pub start: Errors,
    pub end: Errors,
}

/// The samples of `entries` held to `jobs` (`actuals`), and the targets forecast to end by `last`
/// (unix s) that no job did, and the jobs some forecast placed, and the pairs with the same Mac.
pub fn samples(entries: &[Entry], jobs: &[Actual], last: u64) -> (Vec<Sample>, BTreeMap<String, usize>, usize, usize) {
    let mut by_target: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (k, a) in jobs.iter().enumerate() {
        for t in &a.targets {
            by_target.entry((&a.step, t)).or_default().push(k);
        }
    }
    let (mut out, mut not_done, mut same) = (Vec::new(), BTreeMap::<String, usize>::new(), 0);
    let mut placed = vec![false; jobs.len()];
    for e in entries {
        // Job → (earliest start, latest end, its first target's lane), as this forecast placed it.
        let mut got: BTreeMap<usize, (u64, u64, usize)> = BTreeMap::new();
        for p in e.placed() {
            let (lane, from, until) = (&p.lane, &p.from, &p.until);
            let hit = by_target
                .get(&(p.step.as_str(), p.target.as_str()))
                .and_then(|ks| ks.iter().copied().find(|&k| jobs[k].end > e.at));
            match hit {
                Some(k) => {
                    let g = got.entry(k).or_insert((*from, *until, *lane));
                    g.0 = g.0.min(*from);
                    g.1 = g.1.max(*until);
                }
                None if *until < last => *not_done.entry(p.step.clone()).or_default() += 1,
                None => {}
            }
        }
        for (k, (from, until, lane)) in got {
            let a = &jobs[k];
            placed[k] = true;
            let mac = e.lanes.get(lane).map_or("", |l| l.mac.as_str());
            same += usize::from(mac == a.mac);
            let s = |end: bool, horizon_s: u64, err_s: f64| Sample {
                end,
                step: a.step.clone(),
                mac: a.mac.clone(),
                code: e.code,
                horizon_s,
                err_s,
            };
            out.push(s(true, until.saturating_sub(e.at), until as f64 - a.end as f64));
            if a.start > e.at {
                out.push(s(false, from.saturating_sub(e.at), from as f64 - a.start as f64));
            }
        }
    }
    let n = placed.iter().filter(|p| **p).count();
    (out, not_done, n, same)
}

/// The check of the forecasts `entries` against `events`, over `from`..`to` (unix s): the jobs that
/// ended in it.
pub fn check(entries: &[Entry], events: &[Event], from: u64, to: u64) -> Report {
    let jobs: Vec<Actual> = actuals(events).into_iter().filter(|a| a.end >= from && a.end < to).collect();
    let last = events.iter().map(|e| e.t).max().unwrap_or(0).min(to);
    let (s, not_done_by_step, jobs_forecast, same) = samples(entries, &jobs, last);
    let ends: Vec<&Sample> = s.iter().filter(|x| x.end).collect();
    let starts: Vec<&Sample> = s.iter().filter(|x| !x.end).collect();
    Report {
        from,
        to,
        forecasts: entries.len(),
        jobs_done: jobs.len(),
        jobs_forecast,
        same_mac: if ends.is_empty() { 0.0 } else { (same as f64 / ends.len() as f64 * 100.0).round() / 100.0 },
        not_done: not_done_by_step.values().sum(),
        not_done_by_step,
        start: Errors::of(&starts),
        end: Errors::of(&ends),
    }
}

/// The report as text: a table each for the end and the start, in minutes.
pub fn text(r: &Report) -> String {
    let day = |t: u64| {
        let d = super::backup::format_day(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t));
        format!("{d} {:02}:{:02}", t % 86_400 / 3600, t % 3600 / 60)
    };
    let mut o = String::new();
    let _ = writeln!(o, "Forecasts made {} to {} UTC: {} archived.", day(r.from), day(r.to), r.forecasts);
    let _ = writeln!(
        o,
        "Jobs done then: {}, {} of them forecast; the forecast had the Mac that ran it {:.0} % of the time; {} targets forecast to be done by the history's end weren't.",
        r.jobs_done,
        r.jobs_forecast,
        r.same_mac * 100.0,
        r.not_done
    );
    if !r.not_done_by_step.is_empty() {
        let by: Vec<String> = r.not_done_by_step.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let _ = writeln!(o, "  (by step: {})", by.join(", "));
    }
    let _ = writeln!(
        o,
        "Error = forecast - actual, in minutes (+: the forecast was late; -: early); a sample a forecast and job."
    );
    let m = |s: f64| s / 60.0;
    let row = |o: &mut String, name: &str, s: &Stats| {
        let _ = writeln!(
            o,
            "  {name:<26} {:>7} {:>+8.1} {:>8.1} {:>8.1}",
            s.n,
            m(s.bias_s),
            m(s.median_abs_s),
            m(s.p90_abs_s)
        );
    };
    for (what, e) in [("End", &r.end), ("Start", &r.start)] {
        let _ = writeln!(o, "\n{what:<28} {:>7} {:>8} {:>8} {:>8}", "n", "bias", "median", "p90");
        row(&mut o, "all", &e.all);
        let _ = writeln!(o, "  by horizon");
        for (h, s) in &e.by_horizon {
            row(&mut o, &format!("  {h}"), s);
        }
        let _ = writeln!(o, "  by step");
        for (k, s) in &e.by_step {
            row(&mut o, &format!("  {k}"), s);
        }
        let _ = writeln!(o, "  by Mac");
        for (k, s) in &e.by_mac {
            row(&mut o, &format!("  {k}"), s);
        }
        if e.by_code.len() > 1 {
            let _ = writeln!(o, "  by forecast code");
            for (k, s) in &e.by_code {
                row(&mut o, &format!("  v{k}"), s);
            }
        }
    }
    o
}

/// A time as the command takes it (UTC): unix seconds, YYYY-MM-DD, YYYY-MM-DDTHH:MM (or with a
/// space), or a span back from `now` (30m, 12h, 3d).
pub fn parse_time(s: &str, now: u64) -> Option<u64> {
    let s = s.trim();
    if let Ok(t) = s.parse::<u64>() {
        return Some(t);
    }
    if let Some((n, unit)) = s.char_indices().last().map(|(i, c)| (&s[..i], c)) {
        let by = match unit {
            'm' => Some(60),
            'h' => Some(3600),
            'd' => Some(86_400),
            _ => None,
        };
        if let (Some(by), Ok(n)) = (by, n.parse::<u64>()) {
            return Some(now.saturating_sub(n * by));
        }
    }
    let day = super::backup::parse_day(s.get(..10)?)?;
    match s.get(10..) {
        Some("") => Some(day),
        Some(rest) => {
            let hm = rest.strip_prefix(['T', ' '])?;
            let (h, m) = hm.split_once(':')?;
            Some(day + h.parse::<u64>().ok()? * 3600 + m.get(..2)?.parse::<u64>().ok()? * 60)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fcarchive::Lane;
    use super::*;

    fn ev(
        t: u64,
        kind: &str,
        worker: &str,
        lease: Option<u64>,
        step: &str,
        targets: &[&str],
        secs: Option<f64>,
    ) -> Event {
        Event {
            t,
            kind: kind.into(),
            worker: Some(worker.into()),
            lease,
            step: Some(step.into()),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            secs,
            ok: (kind == "done" || kind == "end").then_some(true),
            ..Default::default()
        }
    }

    fn entry(at: u64, jobs: Vec<(usize, usize, &str, u64, u64)>) -> Entry {
        let steps = ["unit", "end-step"];
        let placed: Vec<super::super::forecast::Placed> = jobs
            .into_iter()
            .map(|j| super::super::forecast::Placed {
                lane: j.0,
                step: steps[j.1].into(),
                target: j.2.into(),
                from: at + j.3,
                until: at + j.4,
            })
            .collect();
        let mut e = Entry {
            at,
            code: 1,
            lanes: vec![
                Lane { name: "lead".into(), mac: "lead".into(), ..Default::default() },
                Lane { name: "m".into(), mac: "m".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        e.set_placed(&placed);
        e
    }

    #[test]
    fn jobs_are_the_leases_done_and_the_leads_ends() {
        let es = vec![
            ev(1000, "lease", "m", Some(7), "unit", &["a", "b"], None),
            ev(1600, "done", "m", Some(7), "unit", &["a", "b"], Some(590.0)),
            ev(2000, "end", "lead", None, "end-step", &["x"], Some(300.0)),
            ev(2100, "fail", "m", Some(8), "unit", &["c"], None),
        ];
        let a = actuals(&es);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].start, a[0].end, a[0].mac.as_str()), (1000, 1600, "m"));
        assert_eq!((a[1].start, a[1].end), (1700, 2000));
        // A lead's slot past its first: its Mac.
        assert_eq!(actuals(&[ev(9, "end", "lead #3", None, "unit", &["z"], Some(1.0))])[0].mac, "lead");
    }

    #[test]
    fn a_jobs_targets_are_held_together_and_errors_signed_forecast_less_actual() {
        let es = vec![
            ev(1000, "lease", "m", Some(7), "unit", &["a", "b"], None),
            ev(1600, "done", "m", Some(7), "unit", &["a", "b"], None),
            ev(2000, "end", "lead", None, "end-step", &["x"], Some(300.0)),
            ev(5000, "agent", "lead", None, "", &[], None),
        ];
        // Made at 400: a and b on m from 700 (300 s on) to 1300 and 1900; x on the lead, 1500-2100;
        // y never done.
        let e = entry(
            400,
            vec![(1, 0, "a", 300, 900), (1, 0, "b", 900, 1500), (0, 1, "x", 1100, 1700), (0, 0, "y", 100, 200)],
        );
        let r = check(&[e], &es, 0, 10_000);
        assert_eq!((r.jobs_done, r.jobs_forecast, r.not_done, r.same_mac), (2, 2, 1, 1.0));
        assert_eq!(r.not_done_by_step, [("unit".to_string(), 1)].into());
        // Ends: a+b forecast 1900 against 1600 (+300); x 2100 against 2000 (+100).
        assert_eq!(r.end.all.n, 2);
        assert_eq!(r.end.all.bias_s, 200.0);
        assert_eq!(r.end.by_step["unit"].bias_s, 300.0);
        assert_eq!(r.end.by_mac["lead"].median_abs_s, 100.0);
        // Starts: a+b 700 against 1000 (-300); x 1500 against 1700 (-200).
        assert_eq!(r.start.all.bias_s, -250.0);
        assert_eq!(r.start.all.p90_abs_s, 300.0);
        // Horizons: the ends 1500 s and 1700 s out: 15 min-1 h.
        assert_eq!(r.end.by_horizon, [("15 min-1 h".to_string(), r.end.all.clone())]);
        let t = text(&r);
        assert!(t.contains("by horizon") && t.contains("unit"), "{t}");
    }

    #[test]
    fn a_forecast_made_once_a_job_ran_holds_to_its_end_alone_and_the_next_run_after_it() {
        let es = vec![
            ev(1000, "lease", "m", Some(7), "unit", &["a"], None),
            ev(1600, "done", "m", Some(7), "unit", &["a"], None),
            ev(3000, "lease", "m", Some(9), "unit", &["a"], None),
            ev(3600, "done", "m", Some(9), "unit", &["a"], None),
        ];
        let r = check(
            &[entry(1200, vec![(1, 0, "a", 0, 500)]), entry(2000, vec![(1, 0, "a", 1000, 1500)])],
            &es,
            0,
            10_000,
        );
        // The first: running, its end alone, 1700 against 1600 (+100); the second: the rerun, 3000-3500
        // against 3000-3600 (0, -100).
        assert_eq!(r.end.all.n, 2);
        assert_eq!(r.start.all.n, 1);
        assert_eq!(r.start.all.bias_s, 0.0);
        assert_eq!(r.end.all.bias_s, 0.0);
    }

    #[test]
    fn times_parse() {
        let now = 1_792_800_000;
        assert_eq!(parse_time("12h", now), Some(now - 43_200));
        assert_eq!(parse_time("2026-10-23", now), Some(1_792_713_600));
        assert_eq!(parse_time("2026-10-23T01:30", now), Some(1_792_713_600 + 5400));
        assert_eq!(parse_time("2026-10-23 01:30", now), Some(1_792_713_600 + 5400));
        assert_eq!(parse_time("17", now), Some(17));
        assert_eq!(parse_time("soon", now), None);
    }
}
