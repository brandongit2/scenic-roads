//! The proof of #143's first step (docs/inputs.md §6.8): the units' vertices sampled from the
//! declared datasets (crate::dem::generic) beside the heights the DEM rules gave them, kept in the
//! NAS's `cache/dem-units/` (each unit's last run's samples). Every vertex whose height differs must
//! lie in a blend band, with the same source; one that doesn't is a failure, not an explanation.
//! Reads the NAS only; writes the report to a scratch folder, and the proof the input check reads
//! (`state/inputs/elevation-proof.json`, crate::inputs::elevation::PROOF) only when asked
//! (`write`, which needs the stores, `store`): over every unit the records list, each with its kept
//! samples, and every one passed. The proof names the sampler's version (generic::SAMPLER_V): the
//! switch refuses a proof of another.
//!
//! The declarations are the accepted version the records name, or (before any is accepted) a folder
//! of them checked here as the input check would (`declarations`), their copies and index made in
//! the scratch folder.

use super::declared;
use crate::inputs::elevation::{self, Elevation};
use crate::inputs::{Checks, Version};
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct Opts {
    /// The NAS project folder (read only).
    pub root: PathBuf,
    /// A folder of declarations (the drop box's shape) to prove before any is accepted.
    pub declarations: Option<PathBuf>,
    /// Where the report (and the declarations' copies) go.
    pub scratch: PathBuf,
    /// The units to prove (None: every unit with kept samples).
    pub units: Option<Vec<String>>,
    /// The stores' folder datasets declared `store` read from (read only).
    pub store: Option<PathBuf>,
    /// Write the proof for the input check (every unit recorded, all passed; needs `store`).
    pub write: bool,
    pub workers: usize,
}

/// One unit's comparison.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct UnitProof {
    pub unit: String,
    pub vertices: usize,
    /// Bit for bit as before.
    pub same: usize,
    /// Different, and in a blend band.
    pub blended: usize,
    /// Different outside any blend band, or of another source: (lon, lat, before, now, source
    /// before, now), the first 20.
    pub failures: Vec<(f64, f64, f32, f32, u8, u8)>,
    pub failed: usize,
    /// The largest change in a blend band (m).
    pub max_blend_m: f32,
    /// The kept samples today's code doesn't reproduce now (older runs', for the record: the
    /// comparison is with today's code run now).
    pub kept_drift: usize,
}

/// An accepted version made from a folder of declarations, checked as the input check checks them:
/// its copies and index under `scratch` (an error naming the findings that would hold any).
pub fn accept_folder(dir: &Path, scratch: &Path, root: &Path, m: &BTreeMap<String, String>) -> Result<crate::inputs::Accepted> {
    let unit = elevation::UNIT;
    let outlines = elevation::outlines_of(m).map(|o| crate::outlines::Outlines::open(&root.join(o))).transpose()?;
    let checks = Elevation::new(Arc::new(crate::fetch::Fetcher::from_env()), outlines);
    let mut files: BTreeMap<String, Arc<Vec<u8>>> = BTreeMap::new();
    fn walk(base: &Path, d: &Path, out: &mut BTreeMap<String, Arc<Vec<u8>>>) -> Result<()> {
        for e in std::fs::read_dir(d)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if crate::inputs::ignored(&name) || name == "how" || name == "todo" {
                continue;
            }
            if e.file_type()?.is_dir() {
                walk(base, &e.path(), out)?;
            } else {
                let rel = e.path().strip_prefix(base)?.to_string_lossy().into_owned();
                out.insert(rel, Arc::new(std::fs::read(e.path())?));
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut files)?;
    let rd = |p: &str| files.get(p).cloned().with_context(|| format!("{p}: none"));
    let v = Version { paths: files.keys().cloned().collect(), read: &rd };
    let mut findings = Vec::new();
    let mut checked = BTreeMap::new();
    for (p, b) in &files {
        let c = checks.file(p, b);
        findings.extend(c.findings.clone());
        checked.insert(p.clone(), c);
    }
    findings.extend(checks.whole(&v)?);
    for f in &findings {
        eprintln!("elevation-proof: {:?} {}: {}", f.level, f.id, f.message);
    }
    ensure!(findings.is_empty(), "the declarations have {} finding(s): the input check would hold them", findings.len());
    let derived = checks.derive(&v, &Default::default())?;
    let store = scratch.join(crate::inputs::store_dir(unit));
    let mut entries = BTreeMap::new();
    for (p, b) in &files {
        let c = &checked[p];
        let mut facts = c.facts.clone();
        let mut keyed = c.keyed.clone();
        if let Some((more, k)) = derived.facts.get(p) {
            facts.extend(more.clone());
            keyed = k.clone().or(keyed);
        }
        let h = store::naming::hash16(b);
        let copy = crate::inputs::copy_name(unit, p, &h).with_context(|| format!("{p}: no copy name"))?;
        std::fs::create_dir_all(scratch.join(&copy).parent().unwrap())?;
        std::fs::write(scratch.join(&copy), &**b)?;
        entries.insert(p.clone(), crate::inputs::FileEntry { file: copy, size: b.len() as u64, keyed: keyed.unwrap_or_else(|| h.clone()), facts });
    }
    let index = crate::inputs::Index { fmt: 1, unit: unit.into(), checks: format!("{unit} {}", checks.version()), files: entries, accepted: BTreeMap::new() };
    let body = serde_json::to_vec_pretty(&index)?;
    let name = format!("{}/@index.{}.json", crate::inputs::store_dir(unit), store::naming::hash16(&body));
    std::fs::create_dir_all(&store)?;
    std::fs::write(scratch.join(&name), body)?;
    let mm = BTreeMap::from([(crate::inputs::logical(unit), name)]);
    crate::inputs::open(scratch, &mm, unit)?.context("the index just made")
}

/// The proof over the units: each's comparison; the report written to the scratch folder.
/// Asked to `write`, it refuses first what couldn't be proved: no stores given, units or declarations
/// of its own chosen, a unit the records list with roads but without kept samples (one without
/// roads, no `base/` or `global/roads/` in the manifest, is proved as having no road vertices).
pub fn run(o: &Opts) -> Result<Vec<UnitProof>> {
    // (The proof the input check reads is of every unit the records list, each with kept samples or
    // roadless: no `base/<u>` or `global/roads/<u>` in the manifest, the unit run's sign of no road
    // vertices in it (it samples none, keeps none). One with roads and no samples is a unit unproved,
    // and nothing is written. Known before any is sampled.)
    let m: BTreeMap<String, String> = crate::out::read_record(&o.root.join("state/build/manifest.json"))?;
    let (recorded, roadless): (Vec<String>, Vec<String>) = if o.write {
        ensure!(o.units.is_none() && o.declarations.is_none(), "the proof the input check reads is of every unit and of the accepted version");
        ensure!(o.store.is_some(), "--write needs --store <nas>/sources: the datasets kept whole are proved from the stores the units read");
        let keys = crate::agent::build::Keys::load_strict(&o.root)?;
        let recorded: Vec<String> = keys.unit.keys().filter_map(|t| crate::legacy::Unit::parse(t)).map(|u| u.dash()).collect();
        let files = crate::unit::dem_samples_files(&o.root.join("cache").join(crate::unit::DEM_UNITS));
        let have: std::collections::BTreeSet<&str> = files.iter().map(|(u, ..)| u.as_str()).collect();
        let (roadless, lacking): (Vec<&String>, Vec<&String>) = recorded.iter().filter(|u| !have.contains(u.as_str())).partition(|u| !m.contains_key(&format!("base/{u}")) && !m.contains_key(&format!("global/roads/{u}")));
        ensure!(lacking.is_empty(), "{} of the {} units recorded have roads but no kept samples ({}{}): they'd be unproved, so no proof is written", lacking.len(), recorded.len(), lacking.iter().take(5).map(|u| u.as_str()).collect::<Vec<_>>().join(", "), if lacking.len() > 5 { ", …" } else { "" });
        if !roadless.is_empty() {
            eprintln!("elevation-proof: {} of the {} units recorded have no road vertices (no base/ or global/roads/ in the manifest), proved as such: {}", roadless.len(), recorded.len(), roadless.iter().map(|u| u.as_str()).collect::<Vec<_>>().join(", "));
        }
        let roadless = roadless.into_iter().cloned().collect();
        (recorded, roadless)
    } else {
        (Vec::new(), Vec::new())
    };
    std::fs::create_dir_all(&o.scratch)?;
    let accepted = match &o.declarations {
        Some(d) => accept_folder(d, &o.scratch, &o.root, &m)?,
        None => crate::inputs::open(&o.root, &m, elevation::UNIT)?.context("the records name no accepted version of the elevation datasets yet (--declarations <folder>)")?,
    };
    let files = crate::unit::dem_samples_files(&o.root.join("cache").join(crate::unit::DEM_UNITS));
    let chosen: Vec<_> = files.into_iter().filter(|(u, ..)| o.units.as_ref().is_none_or(|w| w.iter().any(|x| x.replace('/', "-") == *u))).collect();
    if let Some(w) = &o.units {
        ensure!(chosen.len() == w.len(), "kept samples for {} of the {} units asked for", chosen.len(), w.len());
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(o.workers.max(1)).build().ok();
    let fetch = crate::fetch::Fetcher::from_env();
    // (`store` is the sources' folder: the stores in its `fetched/`, FABDEM's earlier in its
    // `fabdem/`, as the units read them.)
    let go = super::generic::Opts { fetch: &fetch, store: o.store.as_ref().map(|s| s.join("fetched")), read_only: true, earlier: o.store.clone(), pool: pool.as_ref() };
    let mut out = Vec::new();
    for (u, _, p) in &chosen {
        let t = std::time::Instant::now();
        let Some((pts, before, src, made)) = crate::unit::dem_samples_read(p)? else { bail!("{}: not whole", p.display()) };
        if made[0] == u32::MAX {
            eprintln!("elevation-proof: {u}: its samples are the declared datasets' already: passed over");
            continue;
        }
        let lon: Vec<f64> = pts.iter().map(|q| q[0] as f64 * 1e-7).collect();
        let lat: Vec<f64> = pts.iter().map(|q| q[1] as f64 * 1e-7).collect();
        let bx = pts.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            let (x, y) = (q[0] as f64 * 1e-7, q[1] as f64 * 1e-7);
            [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
        });
        let plan = declared::plan(&accepted, bx)?;
        let s = super::generic::sample(&plan, &lon, &lat, &go, &|_| {})?;
        // Today's code on the same vertices, now (the kept samples are older runs': a few differ from
        // today's code in their last bit, the seed's made by numpy).
        let dir = o.scratch.join("units").join(u);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("verts.bin"), bytemuck::cast_slice::<[i32; 2], u8>(&pts))?;
        let cfg = super::Config { build: dir.clone(), workers: o.workers, cache: dir.join("cache"), no_cache: true, fabdem_store: o.store.as_ref().map(|s| s.join("fetched/fabdem")), fabdem_read_only: true, store: None, earlier_store: o.store.clone() };
        super::run(&cfg, &fetch)?;
        let today: Vec<f32> = bytemuck::pod_collect_to_vec(&std::fs::read(dir.join("elev.f32"))?);
        let today_src = std::fs::read(dir.join("src.u8"))?;
        let drift = (0..pts.len()).filter(|&i| !(today[i].to_bits() == before[i].to_bits() || (today[i].is_nan() && before[i].is_nan())) || today_src[i] != src[i]).count();
        std::fs::remove_dir_all(&dir).ok();
        let (before, src) = (today, today_src);
        let mut r = UnitProof { unit: u.clone(), vertices: pts.len(), kept_drift: drift, ..Default::default() };
        for i in 0..pts.len() {
            let same = before[i].to_bits() == s.elev[i].to_bits() || (before[i].is_nan() && s.elev[i].is_nan());
            if same && src[i] == s.src[i] {
                r.same += 1;
            } else if src[i] == s.src[i] && s.blended[i] {
                r.blended += 1;
                r.max_blend_m = r.max_blend_m.max((before[i] - s.elev[i]).abs());
            } else {
                r.failed += 1;
                if r.failures.len() < 20 {
                    r.failures.push((lon[i], lat[i], before[i], s.elev[i], src[i], s.src[i]));
                }
            }
        }
        eprintln!("elevation-proof: {u}: {} vertices, {} the same, {} in blend bands (at most {:.2} m), {} failed ({} kept samples not today's), in {:.0?}", r.vertices, r.same, r.blended, r.max_blend_m, r.failed, r.kept_drift, t.elapsed());
        out.push(r);
    }
    let failed: usize = out.iter().map(|r| r.failed).sum();
    let report = serde_json::json!({ "index": accepted.version, "units": out, "failed": failed });
    std::fs::write(o.scratch.join("elevation-proof-report.json"), serde_json::to_vec_pretty(&report)?)?;
    if o.write {
        ensure!(failed == 0, "{failed} vertices differ outside the blend bands: no proof written");
        let proved: std::collections::BTreeSet<&str> = out.iter().map(|r| r.unit.as_str()).chain(roadless.iter().map(|u| u.as_str())).collect();
        let unproved: Vec<&String> = recorded.iter().filter(|u| !proved.contains(u.as_str())).collect();
        ensure!(unproved.is_empty(), "{} of the {} units recorded weren't proved ({}{}): no proof written", unproved.len(), recorded.len(), unproved.iter().take(5).map(|u| u.as_str()).collect::<Vec<_>>().join(", "), if unproved.len() > 5 { ", …" } else { "" });
        let me = crate::agent::app_version(std::env::current_exe()?.parent().context("the program's folder")?);
        let proof = serde_json::json!({ "index": accepted.version, "ok": true, "app": me, "sampler": super::generic::SAMPLER_V, "units": out.len() + roadless.len(), "roadless": roadless, "units_recorded": recorded.len(), "vertices": out.iter().map(|r| r.vertices).sum::<usize>(), "blended": out.iter().map(|r| r.blended).sum::<usize>() });
        crate::whole::write(&o.root.join(elevation::PROOF), &serde_json::to_vec_pretty(&proof)?)?;
    }
    Ok(out)
}

/// How many units the switch rebuilds: of the units recorded, those whose ways more than one
/// dataset meets (each built again), and those one alone meets (re-keyed: agent::rekey).
pub fn units_affected(root: &Path, accepted: &crate::inputs::Accepted) -> Result<(usize, usize, usize)> {
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json"))?;
    let date = crate::osmpass::latest_pass_in(&m).context("no complete pass")?;
    let reach = crate::reach::Reaches::load(root, &m, &date).map_err(|e| anyhow::anyhow!("the pass's reaches: {e:?}"))?.context("no reaches")?;
    let keys = crate::agent::build::Keys::load_strict(root)?;
    let ds: Vec<(declared::Accepted, declared::Cover)> = declared::accepted(accepted)?.into_iter().map(|d| {
        let c = declared::Cover::of(d.measured.cover.as_deref());
        (d, c)
    }).collect();
    let (mut rebuilt, mut rekeyed, mut other) = (0, 0, 0);
    for t in keys.unit.keys() {
        let Some(u) = crate::legacy::Unit::parse(t) else { continue };
        let Some(r) = reach.get(u) else {
            other += 1;
            continue;
        };
        let e = r.extent(u);
        let deg = |v: i32| v as f64 * 1e-7;
        match declared::meeting(&ds, [deg(e[0]), deg(e[1]), deg(e[2]), deg(e[3])]).len() {
            0 | 1 => rekeyed += 1,
            _ => rebuilt += 1,
        }
    }
    Ok((rebuilt, rekeyed, other))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(root: &Path, store: Option<PathBuf>) -> Opts {
        Opts { root: root.to_path_buf(), declarations: None, scratch: root.join("scratch"), units: None, store, write: true, workers: 1 }
    }

    #[test]
    fn the_proof_is_written_only_with_the_stores_and_every_unit_recorded_kept() {
        let d = tempfile::tempdir().unwrap();
        let e = run(&opts(d.path(), None)).unwrap_err();
        assert!(e.to_string().contains("--write needs --store"), "{e:#}");
        std::fs::create_dir_all(d.path().join("state/build")).unwrap();
        std::fs::write(d.path().join("state/build/jobs.json"), br#"{"unit": {"6/1/1": "k1", "6/1/2": "k2"}}"#).unwrap();
        std::fs::write(d.path().join("state/build/manifest.json"), br#"{"base/6-1-1": "b", "global/roads/6-1-2": "r"}"#).unwrap();
        let e = run(&opts(d.path(), Some(d.path().join("sources")))).unwrap_err();
        assert!(e.to_string().contains("2 of the 2 units recorded have roads but no kept samples (6-1-1, 6-1-2)"), "{e:#}");
    }

    #[test]
    fn a_roadless_unit_is_proved_and_one_with_roads_but_no_samples_refuses() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state/build")).unwrap();
        std::fs::write(d.path().join("state/build/jobs.json"), br#"{"unit": {"6/1/1": "k1", "6/1/2": "k2"}}"#).unwrap();
        // 6-1-1 has a base, 6-1-2 none: only 6-1-1 refuses.
        std::fs::write(d.path().join("state/build/manifest.json"), br#"{"base/6-1-1": "b"}"#).unwrap();
        let e = run(&opts(d.path(), Some(d.path().join("sources")))).unwrap_err();
        assert!(e.to_string().contains("1 of the 2 units recorded have roads but no kept samples (6-1-1)"), "{e:#}");
        // Both roadless: past the refusal (on to the accepted index, of which there is none here).
        std::fs::write(d.path().join("state/build/manifest.json"), b"{}").unwrap();
        let e = run(&opts(d.path(), Some(d.path().join("sources")))).unwrap_err();
        assert!(!e.to_string().contains("no kept samples"), "{e:#}");
        assert!(e.to_string().contains("no accepted version"), "{e:#}");
    }
}
