//! A step keyed on the coverage as its run asked it (docs/readiness.md §4, `in coverage:asked`): the
//! worldwide steps that test their own features' points and boxes against the coverage (rail,
//! rail-feeds, timetables-todo, bld-fetch) rerun on a region change only where it changes an
//! answer they read.
//!
//! The run asks through `coverage::Asking` and, as it ends, puts a record (`logical`): what it asked
//! (`coverage::Asked`: its points, its boxes), its other questions with their answers
//! (`Record::also`: rail-feeds' countries), a digest of all the answers (`Record::answers`), and the
//! key line it ran under (`Record::line`). The key's line (`line`) is the record's own line while
//! the coverage still answers those questions the same; otherwise a new one. The run works its line
//! out as the plan did, from the record before it, so the key it is recorded done under is the one
//! the plan works out after it: one run, never a second to settle the key (a record's places are
//! only known once it has run).
//!
//! Sound because a run is a function of its other reads (their own key lines) and of the answers
//! it got: the same answers to the same questions, the same run. A line is either one a record
//! carried or a new one naming the record it replaces, so a record's line never stands for answers
//! it didn't get.

use crate::coverage::{Asked, Coverage};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// What a run asked of the coverage, as it put it (`logical`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub asked: Asked,
    /// Questions of the coverage asked whole, each answered by a function the plan asks again
    /// (`line`'s `reask`): (question, answer).
    pub also: Vec<(String, String)>,
    /// A digest of every answer (`answers`).
    pub answers: String,
    /// The key line the run ran under.
    pub line: String,
}

/// Where step `step`'s run puts its record.
pub fn logical(step: &str) -> String {
    format!("work/asked/{step}")
}

/// The digest of the coverage's answers to `asked` and of the answers `also`.
fn answers(cov: &Coverage, asked: &Asked, also: impl Iterator<Item = (String, String)>) -> String {
    let mut text = cov.answers(asked);
    for (q, a) in also {
        text.push_str(&format!("\n{q}\t{a}"));
    }
    store::naming::hash16(text.as_bytes())
}

impl Record {
    /// A run's record: what it asked and was answered, under key line `line`.
    pub fn new(cov: &Coverage, asked: Asked, also: Vec<(String, String)>, line: String) -> Record {
        let a = answers(cov, &asked, also.iter().cloned());
        Record { asked, also, answers: a, line }
    }

    /// As put: zstd'd JSON (rail's asks a point of each of the world's rail ways).
    pub fn to_bytes(&self) -> Vec<u8> {
        zstd::encode_all(&serde_json::to_vec(self).expect("a record serialises")[..], 9).expect("zstd in memory")
    }

    pub fn from_bytes(b: &[u8]) -> Result<Record> {
        serde_json::from_slice(&zstd::decode_all(b).context("not zstd")?).context("not a coverage record")
    }
}

/// The key line of a step keyed on what it asked: with no record yet, a new one of the coverage's
/// version; with record `prev` (put as content `content`), its line while `cov` answers its
/// questions as they were answered (`reask` answering its `also` questions now), else a new one
/// naming it. None while a question can't be answered now (`reask` None).
pub fn line(cov: &Coverage, prev: Option<(&str, &Record)>, reask: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let v = cov.version();
    let Some((content, r)) = prev else { return Some(store::naming::hash16(format!("none {v}").as_bytes())) };
    let also: Vec<(String, String)> =
        r.also.iter().map(|(q, _)| Some((q.clone(), reask(q)?))).collect::<Option<_>>()?;
    Some(if answers(cov, &r.asked, also.into_iter()) == r.answers {
        r.line.clone()
    } else {
        store::naming::hash16(format!("changed {v} {content}").as_bytes())
    })
}

/// The record before this run, as `out` names it: (content name, record), None before any.
pub fn prev(out: &crate::out::Out, step: &str) -> Result<Option<(String, Record)>> {
    let Some(c) = out.get(&logical(step)) else { return Ok(None) };
    let b = out.path(c).read().with_context(|| format!("read {c}"))?;
    Ok(Some((c.to_string(), Record::from_bytes(&b).with_context(|| c.to_string())?)))
}

/// The line this run ran under: the one the plan worked out from `prev`, the record before it
/// (`prev`), `reask` answering its questions as the plan does.
pub fn run_line(
    cov: &Coverage,
    prev: Option<&(String, Record)>,
    reask: &dyn Fn(&str) -> Option<String>,
) -> Result<String> {
    line(cov, prev.map(|(c, r)| (c.as_str(), r)), reask)
        .context("the record before this run asks what can't be answered now")
}

/// Puts this run's record (at its end, after its outputs): what it asked under `line` (`run_line`),
/// its `also` answers.
pub fn put(
    out: &mut crate::out::Out,
    step: &str,
    cov: &Coverage,
    line: String,
    asked: Asked,
    also: Vec<(String, String)>,
) -> Result<()> {
    let r = Record::new(cov, asked, also, line);
    out.put_bytes(&logical(step), "json.zst", &r.to_bytes())?;
    out.save()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{fixture_circle, Asking, Shape};

    fn cov(circles: &[(f64, f64, f64)]) -> Coverage {
        Coverage {
            shapes: circles
                .iter()
                .enumerate()
                .map(|(i, &(lon, lat, km))| Shape::new(format!("r{i}: c"), vec![fixture_circle(lon, lat, km)], 1000.0))
                .collect(),
            ..Default::default()
        }
    }

    fn e7(lon: f64, lat: f64) -> [i32; 2] {
        [(lon * 1e7) as i32, (lat * 1e7) as i32]
    }

    /// A run asking of points near Lyon, under the line `line` works out from `prev`: its record.
    fn run(c: &Coverage, prev: Option<&(String, Record)>) -> (String, Record) {
        let none = |_: &str| None;
        let l = line(c, prev.map(|(n, r)| (n.as_str(), r)), &none).unwrap();
        let a = Asking::new(c);
        a.contains(e7(4.85, 45.75));
        a.contains(e7(5.7, 45.75));
        a.meets_rect([e7(4.0, 45.0)[0], e7(4.0, 45.0)[1], e7(4.2, 45.2)[0], e7(4.2, 45.2)[1]]);
        let r = Record::new(c, a.asked(), Vec::new(), l);
        (store::naming::hash16(&r.to_bytes()), r)
    }

    #[test]
    fn a_change_far_from_what_was_asked_keeps_the_line_and_one_under_it_changes_it() {
        let none = |_: &str| None;
        let lyon_and_oslo = cov(&[(4.85, 45.75, 60.0), (10.75, 59.9, 40.0)]);
        let first = run(&lyon_and_oslo, None);
        let line_of = |c: &Coverage, r: &(String, Record)| line(c, Some((r.0.as_str(), &r.1)), &none).unwrap();
        // The key the run was recorded done under is the one worked out after it.
        assert_eq!(line(&lyon_and_oslo, None, &none).unwrap(), first.1.line);
        assert_eq!(line_of(&lyon_and_oslo, &first), first.1.line);
        // Oslo's region moved and grown, a region added in Greenland, Lyon's split in two (a region
        // inside it): nothing asked is answered differently.
        let far = cov(&[(4.85, 45.75, 60.0), (10.9, 60.1, 55.0), (-45.0, 70.0, 300.0), (4.85, 45.75, 10.0)]);
        assert_ne!(lyon_and_oslo.version(), far.version());
        assert_eq!(line_of(&far, &first), first.1.line);
        // Lyon's region grown past a point asked (5.7° E is 66 km east: out at 60 km, in at 80 km).
        let near = cov(&[(4.85, 45.75, 80.0), (10.75, 59.9, 40.0)]);
        let changed = line_of(&near, &first);
        assert_ne!(changed, first.1.line);
        // Its run puts that line, and the plan works out the same after it: one run.
        let second = run(&near, Some(&first));
        assert_eq!(second.1.line, changed);
        assert_eq!(line_of(&near, &second), changed);
        // Back as it was: the answers are the first run's, but the line names the second's record.
        assert_ne!(line_of(&lyon_and_oslo, &second), second.1.line);
        assert_ne!(line_of(&lyon_and_oslo, &second), first.1.line);
    }

    #[test]
    fn a_question_asked_again_decides_with_the_rest() {
        let c = cov(&[(4.85, 45.75, 60.0)]);
        let a = Asking::new(&c);
        a.contains(e7(4.85, 45.75));
        let r = Record::new(&c, a.asked(), vec![("countries x".into(), "FR".into())], "l".into());
        assert_eq!(line(&c, Some(("n", &r)), &|_| Some("FR".into())).unwrap(), "l");
        assert_ne!(line(&c, Some(("n", &r)), &|_| Some("FR,IT".into())).unwrap(), "l");
        assert_eq!(line(&c, Some(("n", &r)), &|_| None), None);
    }
}
