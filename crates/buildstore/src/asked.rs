//! A step keyed on the coverage as its run asked it (docs/readiness.md §4, `in coverage:asked`): the
//! worldwide steps that test their own features' points and boxes against the coverage (rail,
//! rail-feeds, timetables-todo, bld-fetch), and the z6 tiles' steps that test their own points
//! against it (pois, unit, bldtiles; trees by its mask), rerun on a region change only where it
//! changes an answer they read. A record carries other answers too: a lake-depth z6 tile's asks
//! nothing of the coverage, only, as `Record::also` questions, the terrain's z12 tiles it read,
//! each answered by its XXH3 (terrain::depth::lakes::tile_question), so it reruns on a terrain
//! change only where a tile it read changed.
//!
//! The run asks through `Run::asking` (`coverage::Asking`) and, as it ends, puts a record
//! (`logical`, a world step's; `target_logical`, a target's): what it asked (`coverage::Asked`: its
//! points, cells and boxes), its other questions with their answers (`Record::also`: rail-feeds'
//! countries, a tree cover tile's mask), a digest of all the answers (`Record::answers`), what it
//! read of the coverage by a cheap test (`Record::version`: the coverage's version for a world
//! step; for a target, what its key named of the coverage before it asked, its geometry over the
//! target's box), with the version of the code that answered it (`Record::answering`,
//! `coverage::ANSWERING`), and the key line it ran under (`Record::line`). The key's line (`line`,
//! `target_line`) is the record's own line while that cheap test reads the same (`as_read`: the
//! same version, answered by this build's code) or the coverage answers those questions the same;
//! otherwise a new one. So a change to how answers are worked out moves no identity: the bump asks
//! every record again once, and only a record answered differently moves its key. A target with
//! no record keys by the cheap test's own line, as before it asked: its first run under this
//! keying moves no key. The run works its line out as the plan did, from the record before it
//! (`Run::start`,
//! `Run::start_target`), so the key it is recorded done under is the one the plan works out after
//! it: one run, never a second to settle the key (a record's places are only known once it has
//! run).
//!
//! Sound because a run is a function of its other reads (their own key lines) and of the answers
//! it got: the same answers to the same questions, the same run. A line is either one a record
//! carried, a target's cheap line (the same geometry, the same answers), or a new one naming the
//! record it replaces, so a record's line never stands for answers it didn't get.

use crate::coverage::{Asked, Asking, Coverage};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// What a run asked of the coverage, as it put it (`logical`, `target_logical`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub asked: Asked,
    /// Questions of the coverage asked whole, each answered by a function the plan asks again
    /// (`line`'s `reask`): (question, answer).
    pub also: Vec<(String, String)>,
    /// A digest of every answer (`answers`).
    pub answers: String,
    /// What it read of the coverage by the cheap test (a world step's: `Coverage::version`; a
    /// target's: its key's geometric line): the same, answered by the same code (`answering`), the
    /// same answers.
    pub version: String,
    /// The coverage's answering code it was answered by (`coverage::ANSWERING`), the cheap test's
    /// other half; a record put before it was carried, 1.
    #[serde(default = "first_answering")]
    pub answering: u32,
    /// The key line the run ran under.
    pub line: String,
}

/// The answering code of a record put before it was carried (`Record::answering`).
pub fn first_answering() -> u32 {
    1
}

/// Whether a record that read `version` of the coverage by the cheap test, answered by code
/// `answering`, answers as it did under cheap test `now` without asking again: the same version,
/// answered by this build's code (`coverage::ANSWERING`).
pub fn as_read(version: &str, answering: u32, now: &str) -> bool {
    version == now && answering == crate::coverage::ANSWERING
}

/// Where world step `step`'s run puts its record.
pub fn logical(step: &str) -> String {
    format!("work/asked/{step}")
}

/// Where step `step`'s run of target `target` (a tile, "z/x/y") puts its record.
pub fn target_logical(step: &str, target: &str) -> String {
    format!("work/asked/{step}/{}", target.replace('/', "-"))
}

/// Whether `l` is a world step's record (`logical`), not a target's.
pub fn is_world(l: &str) -> bool {
    l.strip_prefix("work/asked/").is_some_and(|s| !s.is_empty() && !s.contains('/'))
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
    /// A world step's record: what it asked and was answered, under key line `line`.
    pub fn new(cov: &Coverage, asked: Asked, also: Vec<(String, String)>, line: String) -> Record {
        Record::read_as(cov, cov.version(), asked, also, line)
    }

    /// A record of a run that read `version` of the coverage by the cheap test.
    pub fn read_as(cov: &Coverage, version: String, asked: Asked, also: Vec<(String, String)>, line: String) -> Record {
        let a = answers(cov, &asked, also.iter().cloned());
        Record { asked, also, answers: a, version, answering: crate::coverage::ANSWERING, line }
    }

    /// As put: zstd'd JSON (rail's asks a point of each of the world's rail ways), one frame whose
    /// header holds its size (`record_cost` reads it).
    pub fn to_bytes(&self) -> Vec<u8> {
        zstd::bulk::compress(&serde_json::to_vec(self).expect("a record serializes"), 9).expect("zstd in memory")
    }

    pub fn from_bytes(b: &[u8]) -> Result<Record> {
        serde_json::from_slice(&zstd::decode_all(b).context("not zstd")?).context("not a coverage record")
    }
}

/// What reading record `p` and asking it again holds in memory (bytes): the file, its JSON and the
/// record parsed from it (no larger than its JSON), its JSON's size from its zstd frame's header
/// (`Record::to_bytes`), else, for a record put before that held it, by decoding the frame once
/// into nothing (a file that isn't zstd: its own size). An error only when it can't be read.
pub fn record_cost(p: &store::nasfs::NasPath) -> std::io::Result<u64> {
    use std::io::Read;
    let file = p.metadata()?.len();
    let mut head = Vec::with_capacity(18);
    p.open()?.take(18).read_to_end(&mut head)?;
    let json = match zstd::zstd_safe::get_frame_content_size(&head) {
        Ok(Some(n)) => n,
        _ => {
            struct Count(u64);
            impl std::io::Write for Count {
                fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                    self.0 += b.len() as u64;
                    Ok(b.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            // (One that isn't zstd costs its file: it's read, and doesn't parse.)
            let mut n = Count(0);
            zstd::stream::copy_decode(p.open()?, &mut n).map_or(0, |()| n.0)
        }
    };
    Ok(file + 2 * json)
}

/// A question of a record's `also` answered again now (`line`'s and `target_line`'s `reask`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Now(String),
    /// A question of a form this build doesn't ask (a form since changed): taken for an answer
    /// flipped, so its step re-runs once.
    OfAnotherForm,
    /// One that can't be answered now (what it names can't be read: I/O): the key waits.
    NotNow,
}

/// The key line of a run that reads `now` of the coverage by the cheap test: with no record, `none`;
/// with record `prev` (put as content `content`), its line while `now` is what it read and this
/// build's code answers (`as_read`: nothing asked again) or `cov` answers its questions as they
/// were answered (`reask` answering its
/// `also` questions now), else a new one naming it; a record that doesn't parse, or a question of
/// another form, a new one too. None only while a question can't be answered now (`Answer::NotNow`).
fn line_of(
    cov: &Coverage,
    now: &str,
    none: impl FnOnce() -> String,
    prev: Option<(&str, Prev)>,
    reask: &dyn Fn(&str) -> Answer,
) -> Option<String> {
    let (content, r) = match prev {
        None => return Some(none()),
        Some((content, Prev::Unparsed)) => return Some(changed(now, content)),
        Some((content, Prev::Record(r))) => (content, r),
    };
    if as_read(&r.version, r.answering, now) {
        return Some(r.line.clone());
    }
    let also: Vec<(String, String)> = r
        .also
        .iter()
        .map(|(q, _)| match reask(q) {
            Answer::Now(a) => Some((q.clone(), a)),
            Answer::OfAnotherForm => Some((q.clone(), UNANSWERED.to_string())),
            Answer::NotNow => None,
        })
        .collect::<Option<_>>()?;
    Some(if answers(cov, &r.asked, also.into_iter()) == r.answers { r.line.clone() } else { changed(now, content) })
}

/// The line of a run reading `now` by the cheap test whose record `content` answers differently now.
fn changed(now: &str, content: &str) -> String {
    store::naming::hash16(format!("changed {now} {content}").as_bytes())
}

/// A record as the plan and the run read it: the record, or one that reads but doesn't parse (a
/// format since changed): taken for answers flipped.
pub enum Prev<'a> {
    Record(&'a Record),
    Unparsed,
}

/// A record's format: bumped with any change to `Record` or `coverage::Asked` that changes what
/// parses, so what's kept of a record by it (the plan's heads, agent::tiles) is read again.
pub const RECORD_FMT: u32 = 2;

/// The `reask` of a step that asks no question whole: any it finds is of another form.
pub fn no_questions(_: &str) -> Answer {
    Answer::OfAnotherForm
}

/// An answer no run gets: a question of another form answers this, so its answers differ from the
/// record's.
const UNANSWERED: &str = "\u{0}unanswered";

/// The key line of a world step keyed on what it asked: with no record yet, a new one of the
/// coverage's version; with record `prev`, `line_of`'s, the coverage's version its cheap test.
pub fn line(cov: &Coverage, prev: Option<(&str, Prev)>, reask: &dyn Fn(&str) -> Answer) -> Option<String> {
    let v = cov.version();
    line_of(cov, &v, || store::naming::hash16(format!("none {v}").as_bytes()), prev, reask)
}

/// The key line of a target keyed on what it asked, `geo` what its key reads of the coverage by
/// geometry (its cheap test): with no record yet, `geo` itself, the line it keyed by before it
/// asked; with record `prev`, `line_of`'s, a question `reask` can't answer (a target's questions
/// read nothing that can fail: of another form) taken for an answer flipped. Never none: a record
/// of another form re-runs its target once, never wedges it.
pub fn target_line(
    cov: &Coverage,
    geo: &str,
    prev: Option<(&str, Prev)>,
    reask: &dyn Fn(&str) -> Option<String>,
) -> String {
    let flips = |q: &str| reask(q).map_or(Answer::OfAnotherForm, Answer::Now);
    line_of(cov, geo, || geo.to_string(), prev, &flips).expect("every question answered")
}

/// A run of a step keyed on what it asks: the coverage asked through (`asking`), where its record
/// goes, what it read by the cheap test and the line it runs under (the plan's, worked out from the
/// record before it).
pub struct Run<'a> {
    logical: String,
    cov: &'a Coverage,
    version: String,
    line: String,
    asking: Asking<'a>,
}

/// The record `out` names at `logical`, with its content name: None when it names none, its
/// record None when it doesn't parse (`Prev::Unparsed`). An error only when it can't be read.
fn record_at(out: &crate::out::Out, logical: &str) -> Result<Option<(String, Option<Record>)>> {
    let Some(c) = out.get(logical) else { return Ok(None) };
    let b = out.path(c).read().with_context(|| format!("read {c}"))?;
    Ok(Some((c.to_string(), Record::from_bytes(&b).ok())))
}

/// `record_at`'s as `line_of` takes it.
fn prev_of(p: &Option<(String, Option<Record>)>) -> Option<(&str, Prev<'_>)> {
    p.as_ref().map(|(c, r)| (c.as_str(), r.as_ref().map_or(Prev::Unparsed, Prev::Record)))
}

impl<'a> Run<'a> {
    /// World step `step`'s run over `cov`: its line from the record `out` names (`reask` answering
    /// its questions asked whole, as the plan's does: one that doesn't parse, or a question of
    /// another form, a new line). An error when that record can't be read, or asks what can't be
    /// answered now (`Answer::NotNow`).
    pub fn start(
        out: &crate::out::Out,
        step: &str,
        cov: &'a Coverage,
        reask: &dyn Fn(&str) -> Answer,
    ) -> Result<Run<'a>> {
        let logical = logical(step);
        let prev = record_at(out, &logical)?;
        let line =
            line(cov, prev_of(&prev), reask).context("the record before this run asks what can't be answered now")?;
        Ok(Run { logical, cov, version: cov.version(), line, asking: Asking::new(cov) })
    }

    /// Step `step`'s run of target `target` over `cov`, reading `geo` of it by geometry (its key's
    /// cheap line, as the plan works it out): its line from the record `out` names (`target_line`:
    /// one that doesn't parse, or asks what can't be answered now, as the plan takes it). An error
    /// only when that record can't be read.
    pub fn start_target(
        out: &crate::out::Out,
        step: &str,
        target: &str,
        cov: &'a Coverage,
        geo: String,
        reask: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Run<'a>> {
        let logical = target_logical(step, target);
        let prev = record_at(out, &logical)?;
        let line = target_line(cov, &geo, prev_of(&prev), reask);
        Ok(Run { logical, cov, version: geo, line, asking: Asking::new(cov) })
    }

    /// The run with its line as its key spells it (`f` of the line worked out): a key that names a
    /// line by what it stands for (pipeline's agent::idents::peaks_lo: no record yet, its cheap
    /// test's lines whole) records that, so the plan finds it as it keyed the run.
    pub fn spelled(self, f: impl FnOnce(String) -> String) -> Run<'a> {
        Run { line: f(self.line), ..self }
    }

    /// The coverage, every question kept.
    pub fn asking(&self) -> &Asking<'a> {
        &self.asking
    }

    /// Its record: what it asked, and `also`, its questions asked whole with their answers.
    pub fn record(self, also: Vec<(String, String)>) -> (String, Record) {
        let r = Record::read_as(self.cov, self.version, self.asking.asked(), also, self.line);
        (self.logical, r)
    }

    /// Puts its record (at its end, with its outputs; saved by the run's own save).
    pub fn put(self, out: &mut crate::out::Out, also: Vec<(String, String)>) -> Result<()> {
        let (l, r) = self.record(also);
        out.put_bytes(&l, "json.zst", &r.to_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{fixture_circle, Shape};

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
        let none = |_: &str| Answer::NotNow;
        let l = line(c, prev.map(|(n, r)| (n.as_str(), Prev::Record(r))), &none).unwrap();
        let a = Asking::new(c);
        a.contains(e7(4.85, 45.75));
        a.contains(e7(5.7, 45.75));
        a.meets_rect([e7(4.0, 45.0)[0], e7(4.0, 45.0)[1], e7(4.2, 45.2)[0], e7(4.2, 45.2)[1]]);
        let r = Record::new(c, a.asked(), Vec::new(), l);
        (store::naming::hash16(&r.to_bytes()), r)
    }

    #[test]
    fn a_change_far_from_what_was_asked_keeps_the_line_and_one_under_it_changes_it() {
        let none = |_: &str| Answer::NotNow;
        let lyon_and_oslo = cov(&[(4.85, 45.75, 60.0), (10.75, 59.9, 40.0)]);
        let first = run(&lyon_and_oslo, None);
        let line_of =
            |c: &Coverage, r: &(String, Record)| line(c, Some((r.0.as_str(), Prev::Record(&r.1))), &none).unwrap();
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

    /// Task #1590: a record answered by other answering code (`coverage::ANSWERING`), the coverage's
    /// version the same, is asked again: answered the same by this build's code, its line; answered
    /// differently, a new one (its step re-runs). A record put before it carried one is code 1's.
    #[test]
    fn an_answering_change_asks_again_and_moves_only_a_line_answered_differently() {
        let none = |_: &str| Answer::NotNow;
        let c = cov(&[(4.85, 45.75, 60.0)]);
        let a = Asking::new(&c);
        assert!(a.contains(e7(4.85, 45.75)));
        let kept = Record::new(&c, a.asked(), Vec::new(), "kept".into());
        assert_eq!(kept.answering, crate::coverage::ANSWERING);
        let rec = |r: &Record| line(&c, Some(("n", Prev::Record(r))), &none).unwrap();
        assert_eq!(rec(&kept), "kept");
        // Answered by code 1, the same answers: asked again (nothing else could tell), its line.
        let same = Record { answering: 1, line: "same".into(), ..kept.clone() };
        assert!(!as_read(&same.version, same.answering, &c.version()));
        assert_eq!(rec(&same), "same");
        // Code 1 answered the point out (its digest that of a cell out): asked again, answered in
        // now, a new line.
        let out = Asking::new(&c);
        assert!(!out.contains(e7(5.7, 45.75)));
        let differs = Record {
            answering: 1,
            line: "differs".into(),
            answers: Record::new(&c, out.asked(), Vec::new(), String::new()).answers,
            ..kept.clone()
        };
        assert_ne!(rec(&differs), "differs");
        // The same record under this build's code: its line, nothing asked again.
        let now = Record { answering: crate::coverage::ANSWERING, ..differs.clone() };
        assert_eq!(rec(&now), "differs");
        // A record as put before it carried one: code 1's.
        let json = serde_json::to_value(&kept).unwrap();
        let mut old = json.as_object().unwrap().clone();
        old.remove("answering");
        let parsed: Record = serde_json::from_value(serde_json::Value::Object(old)).unwrap();
        assert_eq!(parsed.answering, 1);
    }

    /// Task #1590 review: what asking a record again holds in memory, its JSON's size read from its
    /// frame's header as put now, and by decoding one put before that held it.
    #[test]
    fn a_records_cost_is_its_file_and_twice_its_json() {
        let c = cov(&[(4.85, 45.75, 60.0)]);
        let a = Asking::new(&c);
        for k in 0..2000 {
            a.contains(e7(4.0 + k as f64 * 0.001, 45.75));
        }
        let r = Record::new(&c, a.asked(), vec![("q".into(), "a".into())], "l".into());
        let json = serde_json::to_vec(&r).unwrap().len() as u64;
        let d = tempfile::tempdir().unwrap();
        for (name, b) in
            [("now", r.to_bytes()), ("before", zstd::encode_all(&serde_json::to_vec(&r).unwrap()[..], 9).unwrap())]
        {
            let p = d.path().join(name);
            std::fs::write(&p, &b).unwrap();
            assert_eq!(Record::from_bytes(&b).unwrap(), r);
            let held = zstd::zstd_safe::get_frame_content_size(&b).ok().flatten();
            assert_eq!(held, (name == "now").then_some(json), "{name}");
            let cost = record_cost(&store::nasfs::NasPath::local_for_test(&p)).unwrap();
            assert_eq!(cost, b.len() as u64 + 2 * json, "{name}");
        }
        // One that isn't zstd: its file (it's read, and doesn't parse); one that can't be read, none.
        let p = d.path().join("not");
        std::fs::write(&p, b"not a record").unwrap();
        assert_eq!(record_cost(&store::nasfs::NasPath::local_for_test(&p)).unwrap(), 12);
        assert!(record_cost(&store::nasfs::NasPath::local_for_test(d.path().join("gone"))).is_err());
    }

    #[test]
    fn a_question_asked_again_decides_with_the_rest() {
        let c = cov(&[(4.85, 45.75, 60.0)]);
        let a = Asking::new(&c);
        a.contains(e7(4.85, 45.75));
        let r = Record::new(&c, a.asked(), vec![("countries x".into(), "FR".into())], "l".into());
        // (Another version, the point answered the same: the question decides.)
        let other = cov(&[(4.85, 45.75, 61.0)]);
        assert_ne!(other.version(), c.version());
        let rec = || Some(("n", Prev::Record(&r)));
        assert_eq!(line(&other, rec(), &|_| Answer::Now("FR".into())).unwrap(), "l");
        let flipped = line(&other, rec(), &|_| Answer::Now("FR,IT".into())).unwrap();
        assert_ne!(flipped, "l");
        // Task #1473 re-review: what it names can't be read now, the key waits; a question of
        // another form, or a record that doesn't parse, is a new line (its step re-runs once),
        // never a wait for good.
        assert_eq!(line(&other, rec(), &|_| Answer::NotNow), None);
        let other_form = line(&other, rec(), &|_| Answer::OfAnotherForm).unwrap();
        assert_eq!(other_form, flipped);
        let unparsed = line(&other, Some(("n", Prev::Unparsed)), &|_| Answer::NotNow).unwrap();
        assert_eq!(unparsed, flipped);
    }

    /// Task #1473 re-review: a world step's run over a record that doesn't parse goes ahead under
    /// the line the plan works out, and the plan's line after it is its own record's.
    #[test]
    fn a_world_run_over_a_record_of_another_form_goes_ahead() {
        let none = |_: &str| Answer::NotNow;
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(&d.path().join("nas"));
        let mut out = crate::out::Out::unbound(&root, &d.path().join("s")).unwrap();
        let c = cov(&[(4.85, 45.75, 60.0)]);
        out.put_bytes(&logical("rail"), "json.zst", b"not a record").unwrap();
        out.save().unwrap();
        let content = out.get(&logical("rail")).unwrap().to_string();
        let plan = line(&c, Some((&content, Prev::Unparsed)), &none).unwrap();
        let run = Run::start(&out, "rail", &c, &none).unwrap();
        run.asking().contains(e7(4.85, 45.75));
        run.put(&mut out, Vec::new()).unwrap();
        out.save().unwrap();
        let content = out.get(&logical("rail")).unwrap().to_string();
        let r = Record::from_bytes(&out.path(&content).read().unwrap()).unwrap();
        assert_eq!(r.line, plan);
        assert_eq!(line(&c, Some((&content, Prev::Record(&r))), &none).unwrap(), plan);
    }

    /// The version a record asked answers every question as it was answered: its line, nothing
    /// asked again (rail's 4.3 M points, rail-feeds' countries: not on each lead's start).
    #[test]
    fn the_version_asked_keeps_the_line_without_asking_again() {
        let c = cov(&[(4.85, 45.75, 60.0)]);
        let r = Record::new(&c, Asked::default(), vec![("countries x".into(), "FR".into())], "l".into());
        assert_eq!(r.version, c.version());
        let asked_again = std::cell::Cell::new(false);
        let reask = |_: &str| {
            asked_again.set(true);
            Answer::NotNow
        };
        assert_eq!(line(&c, Some(("n", Prev::Record(&r))), &reask).unwrap(), "l");
        assert!(!asked_again.get());
    }

    /// The box around Lyon a target's key reads by geometry in these tests.
    fn geo(c: &Coverage) -> String {
        let (sw, ne) = (e7(4.0, 45.0), e7(6.0, 46.5));
        c.fingerprint([sw[0], sw[1], ne[0], ne[1]])
    }

    /// Task #1473: a target's line is its geometry's while it has no record, the line it keyed by
    /// before it asked (so no target's key moves until its own run records what it asked); after
    /// its run, the line it ran under while the coverage answers what it asked the same, however
    /// the geometry moved; a new one once an answer flips.
    #[test]
    fn a_targets_line_is_its_geometry_until_it_asked_then_moves_only_with_an_answer() {
        let none = |_: &str| None;
        let lyon = cov(&[(4.85, 45.75, 60.0)]);
        let l = target_line(&lyon, &geo(&lyon), None, &none);
        assert_eq!(l, geo(&lyon));
        // Its run asks of a point in Lyon and one 66 km east (out).
        let a = Asking::new(&lyon);
        assert!(a.contains(e7(4.85, 45.75)));
        assert!(!a.contains(e7(5.7, 45.75)));
        let r = Record::read_as(&lyon, geo(&lyon), a.asked(), Vec::new(), l.clone());
        // (Both points far from an edge: each asked as its cell.)
        assert_eq!((r.asked.points.len(), r.asked.cells.len()), (0, 2), "{:?}", r.asked);
        let c = store::naming::hash16(&r.to_bytes());
        let now = |cv: &Coverage| target_line(cv, &geo(cv), Some((&c, Prev::Record(&r))), &none);
        assert_eq!(now(&lyon), l);
        // Lyon's region redrawn a kilometre wider and with a region inside it: its geometry over
        // the box moved, no answer did.
        let redrawn = cov(&[(4.85, 45.75, 61.0), (4.85, 45.75, 10.0)]);
        assert_ne!(geo(&redrawn), geo(&lyon));
        assert_eq!(now(&redrawn), l);
        // Grown past the point east (80 km): an answer flips, and so does the line.
        assert_ne!(now(&cov(&[(4.85, 45.75, 80.0)])), l);
        // Grown to just short of its cell's neighbourhood (64 km): the cell isn't one answer now,
        // so it's taken for one that flipped (its points weren't kept).
        assert_ne!(now(&cov(&[(4.85, 45.75, 64.5)])), l);
        // A point by an edge is kept as a point: an edge moved past others near it keeps its answer.
        let a = Asking::new(&lyon);
        assert!(a.contains(e7(5.6, 45.75)));
        let r = Record::read_as(&lyon, geo(&lyon), a.asked(), Vec::new(), l.clone());
        assert_eq!((r.asked.points.len(), r.asked.cells.len()), (1, 0));
        let c = store::naming::hash16(&r.to_bytes());
        let now = |cv: &Coverage| target_line(cv, &geo(cv), Some((&c, Prev::Record(&r))), &none);
        assert_eq!(now(&cov(&[(4.85, 45.75, 59.0)])), l);
        assert_ne!(now(&cov(&[(4.85, 45.75, 55.0)])), l);
    }

    /// A target's run works its line out from the record before it as the plan does, so the key it's
    /// recorded done under is the one worked out after it (`Run::start_target`).
    #[test]
    fn a_targets_run_puts_the_line_the_plan_works_out() {
        let none = |_: &str| None;
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(&d.path().join("nas"));
        let mut out = crate::out::Out::unbound(&root, &d.path().join("s")).unwrap();
        let lyon = cov(&[(4.85, 45.75, 60.0)]);
        let near = cov(&[(4.85, 45.75, 80.0)]);
        for c in [&lyon, &lyon, &near, &near] {
            let run = Run::start_target(&out, "pois", "6/33/22", c, geo(c), &none).unwrap();
            run.asking().contains(e7(5.7, 45.75));
            run.put(&mut out, Vec::new()).unwrap();
            out.save().unwrap();
            let l = target_logical("pois", "6/33/22");
            let content = out.get(&l).unwrap().to_string();
            let r = Record::from_bytes(&out.path(&content).read().unwrap()).unwrap();
            // (The line it ran under, as its record holds it.)
            assert_eq!(target_line(c, &geo(c), Some((&content, Prev::Record(&r))), &none), r.line);
        }
    }

    /// Task #1473 review: a target's record that doesn't parse (a format since changed), or asks a
    /// question that can't be answered now, re-keys its target once (a new line naming the record),
    /// never leaves it unkeyed; its run goes ahead under the line the plan works out, and the line
    /// after it is its own record's.
    #[test]
    fn a_record_of_another_form_re_runs_its_target_once() {
        let none = |_: &str| None;
        let lyon = cov(&[(4.85, 45.75, 60.0)]);
        let g = geo(&lyon);
        // A question that can't be answered now: an answer flipped.
        let r = Record::read_as(
            &lyon,
            "before".into(),
            Asked::default(),
            vec![("old form".into(), "a".into())],
            "l".into(),
        );
        let flipped = target_line(&lyon, &g, Some(("n", Prev::Record(&r))), &none);
        assert_ne!(flipped, "l");
        assert_eq!(flipped, target_line(&lyon, &g, Some(("n", Prev::Record(&r))), &|_| Some("b".into())));
        assert_eq!(target_line(&lyon, &g, Some(("n", Prev::Record(&r))), &|_| Some("a".into())), "l");
        // A record that doesn't parse, put where the plan and the run read it.
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(&d.path().join("nas"));
        let mut out = crate::out::Out::unbound(&root, &d.path().join("s")).unwrap();
        let l = target_logical("pois", "6/33/22");
        out.put_bytes(&l, "json.zst", b"not a record").unwrap();
        out.save().unwrap();
        let content = out.get(&l).unwrap().to_string();
        let plan = target_line(&lyon, &g, Some((&content, Prev::Unparsed)), &none);
        assert_ne!(plan, g);
        assert_eq!(plan, target_line(&lyon, &g, Some((&content, Prev::Unparsed)), &none));
        let run = Run::start_target(&out, "pois", "6/33/22", &lyon, g.clone(), &none).unwrap();
        run.asking().contains(e7(4.85, 45.75));
        run.put(&mut out, Vec::new()).unwrap();
        out.save().unwrap();
        let content = out.get(&l).unwrap().to_string();
        let r = Record::from_bytes(&out.path(&content).read().unwrap()).unwrap();
        assert_eq!(r.line, plan);
        assert_eq!(target_line(&lyon, &g, Some((&content, Prev::Record(&r))), &none), plan);
    }

    /// The 3D buildings' questions (task #1473): a point's country and a cell's whole answer as
    /// `shape_at` gives each point, and the digest moves only with them.
    #[test]
    fn countries_by_point_and_by_cell_answer_as_shape_at_does() {
        let countries = |c: &mut Coverage, cs: &[&str]| {
            for (s, cc) in c.shapes.iter_mut().zip(cs) {
                s.country = cc.to_string();
            }
        };
        let mut c = cov(&[(4.85, 45.75, 60.0), (5.6, 45.75, 30.0)]);
        countries(&mut c, &["FR", "CH"]);
        let a = Asking::new(&c);
        // Deep in the first: its cell's one answer, every point near it the same.
        let deep = crate::coverage::cell_of(e7(4.5, 45.75));
        assert_eq!(a.country_cell(deep), Some(Some("FR")));
        for k in 0..50 {
            let p = e7(4.5 + (k as f64 - 25.0) * 0.0006, 45.75 + (k % 7) as f64 * 0.001);
            assert_eq!(c.country_at(p), Some("FR"), "{p:?}");
        }
        // In both (the first's: outlines in order), near the first's edge: no whole answer, asked alone.
        assert_eq!(a.country_cell(crate::coverage::cell_of(e7(5.6, 45.75))), None);
        assert_eq!(a.country_at(e7(5.6, 45.75)), Some("FR"));
        // In neither, far from both: the cell's, none.
        assert_eq!(a.country_cell(crate::coverage::cell_of(e7(7.5, 45.75))), Some(None));
        let asked = a.asked();
        let was = c.answers(&asked);
        // The second's country changed, or a region added inside the first: no answer moved.
        let mut other = c.clone();
        countries(&mut other, &["FR", "IT"]);
        assert_eq!(other.answers(&asked), was);
        let mut inner = cov(&[(4.85, 45.75, 60.0), (5.6, 45.75, 30.0), (4.85, 45.75, 10.0)]);
        countries(&mut inner, &["FR", "CH", "FR"]);
        assert_eq!(inner.answers(&asked), was);
        // The order swapped (the point by both now the second's), the first's country, or the
        // first grown over the point far east: each moves it.
        let mut swapped = cov(&[(5.6, 45.75, 30.0), (4.85, 45.75, 60.0)]);
        countries(&mut swapped, &["CH", "FR"]);
        assert_ne!(swapped.answers(&asked), was);
        let mut de = c.clone();
        countries(&mut de, &["DE", "CH"]);
        assert_ne!(de.answers(&asked), was);
        let mut grown = cov(&[(4.85, 45.75, 260.0), (5.6, 45.75, 30.0)]);
        countries(&mut grown, &["FR", "CH"]);
        assert_ne!(grown.answers(&asked), was);
    }

    /// Task #1473 review: a record's cells are answered at the zoom it asked them at (`cell_z`), not
    /// `CELL_Z` now; one put without it is of zoom 14.
    #[test]
    fn cells_are_answered_at_the_zoom_they_were_asked_at() {
        let lyon = cov(&[(4.85, 45.75, 60.0)]);
        let (x, y) = roadcore::geo::tile_of(roadcore::geo::merc_clamped(4.85, 45.75), 13);
        let old = Asked { cells: vec![[x, y]], cell_z: 13, ..Default::default() };
        // Lyon's region shrunk to a kilometre: the z13 cell at its centre is no longer one answer.
        // Read at z14, the same x and y are a cell over the Atlantic, out either way.
        let small = cov(&[(4.85, 45.75, 1.0)]);
        assert_ne!(lyon.answers(&old), small.answers(&old));
        let misread = Asked { cell_z: 14, ..old.clone() };
        assert_eq!(lyon.answers(&misread), small.answers(&misread));
        let r: Asked = serde_json::from_str(r#"{"points": [], "land": [], "all": [], "cells": [[1, 2]]}"#).unwrap();
        assert_eq!(r.cell_z, 14);
    }

    /// A record from before cells and countries were asked (none of them in it) answers with the
    /// digest it was put with: the world steps' records keep their lines.
    #[test]
    fn a_record_with_no_cell_or_country_answers_as_before() {
        let c = cov(&[(4.85, 45.75, 60.0)]);
        let asked = Asked { points: vec![e7(4.85, 45.75), e7(5.7, 45.75)], ..Default::default() };
        let bits = |v: &[bool]| v.iter().enumerate().fold(0u8, |b, (i, &x)| b | (u8::from(x) << i));
        let mut h = blake3::Hasher::new();
        for (n, b) in [(2u64, vec![bits(&[true, false])]), (0, vec![]), (0, vec![])] {
            h.update(&n.to_le_bytes());
            h.update(&b);
        }
        assert_eq!(c.answers(&asked), store::naming::hex16(&h.finalize()));
    }
}
