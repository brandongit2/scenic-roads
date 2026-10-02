//! Region recipes (docs/plan.md §5): `inputs/regions/<id>.toml` with `id`, `name` and `outline`,
//! a list whose union is the region. Entries: `osm:<relation>`, `geofabrik:<id>`, `poly:<file>`
//! (in `inputs/outlines/`), or `place:<lon>,<lat>,<radius km>`. A recipe renamed to
//! `<id>.toml.removed` is removed.

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
}

/// An outline entry, parsed.
#[derive(Clone, Debug, PartialEq)]
pub enum Outline {
    Osm(u64),
    Geofabrik(String),
    Poly(String),
    Place { lon: f64, lat: f64, km: f64 },
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !id.starts_with('-')
}

pub fn parse_outline(s: &str) -> Result<Outline> {
    let (kind, v) = s.split_once(':').with_context(|| format!("{s:?}: an outline is osm:<relation>, geofabrik:<id>, poly:<file> or place:<lon>,<lat>,<km>"))?;
    Ok(match kind {
        "osm" => Outline::Osm(v.parse().with_context(|| format!("{s:?}: not a relation id"))?),
        "geofabrik" => {
            ensure!(!v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'/'), "{s:?}: not a Geofabrik id");
            Outline::Geofabrik(v.into())
        }
        "poly" => {
            ensure!(!v.is_empty() && !v.contains("..") && !v.starts_with('/'), "{s:?}: a file name in inputs/outlines/");
            Outline::Poly(v.into())
        }
        "place" => {
            let n: Vec<f64> = v.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().with_context(|| format!("{s:?}: place:<lon>,<lat>,<km>"))?;
            ensure!(n.len() == 3 && (-180.0..=180.0).contains(&n[0]) && (-90.0..=90.0).contains(&n[1]) && n[2] > 0.0 && n[2] <= 500.0, "{s:?}: place:<lon>,<lat>,<km> (0–500 km)");
            Outline::Place { lon: n[0], lat: n[1], km: n[2] }
        }
        _ => bail!("{s:?}: unknown outline kind {kind:?}"),
    })
}

impl Recipe {
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_id(&self.id), "{:?}: ids are lower-case letters, digits and dashes", self.id);
        ensure!(!self.name.trim().is_empty(), "{}: no name", self.id);
        ensure!(!self.outline.is_empty(), "{}: no outline", self.id);
        for o in &self.outline {
            parse_outline(o).with_context(|| format!("region {}", self.id))?;
        }
        Ok(())
    }
}

/// Every recipe in `dir` (`inputs/regions`), sorted by id; a file that doesn't parse or validate is
/// reported in the second list (file name, problem), not dropped silently.
pub fn load(dir: &Path) -> (Vec<Recipe>, Vec<(String, String)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return (ok, bad) };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".toml") else { continue };
        let r = std::fs::read_to_string(e.path()).map_err(anyhow::Error::from).and_then(|s| toml::from_str::<Recipe>(&s).map_err(anyhow::Error::from)).and_then(|r| {
            r.validate()?;
            ensure!(r.id == stem, "the file is {name} but its id is {:?}", r.id);
            Ok(r)
        });
        match r {
            Ok(r) => ok.push(r),
            Err(e) => bad.push((name, format!("{e:#}"))),
        }
    }
    ok.sort_by(|a, b| a.id.cmp(&b.id));
    bad.sort();
    (ok, bad)
}

/// Writes a new recipe; fails if one with its id exists (created exclusively, so two Macs can't
/// both create it).
pub fn add(dir: &Path, r: &Recipe) -> Result<()> {
    r.validate()?;
    std::fs::create_dir_all(dir)?;
    let p = dir.join(format!("{}.toml", r.id));
    let mut f = std::fs::File::options().write(true).create_new(true).open(&p).with_context(|| format!("{} exists already (or can't be created)", p.display()))?;
    f.write_all(toml::to_string(r)?.as_bytes())?;
    Ok(())
}

/// Marks a recipe removed (renamed to `.toml.removed`, which keeps it for undoing).
pub fn remove(dir: &Path, id: &str) -> Result<()> {
    ensure!(valid_id(id), "{id:?} isn't a region id");
    let p = dir.join(format!("{id}.toml"));
    ensure!(p.exists(), "no region {id}");
    std::fs::rename(&p, dir.join(format!("{id}.toml.removed")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines() {
        assert_eq!(parse_outline("osm:88066").unwrap(), Outline::Osm(88066));
        assert_eq!(parse_outline("geofabrik:asia/japan/kanto").unwrap(), Outline::Geofabrik("asia/japan/kanto".into()));
        assert!(matches!(parse_outline("place:-2.1,55.2,40").unwrap(), Outline::Place { km, .. } if km == 40.0));
        assert!(parse_outline("poly:../x").is_err());
        assert!(parse_outline("osm:x").is_err());
        assert!(parse_outline("place:1,2").is_err());
        assert!(parse_outline("relation:1").is_err());
    }

    #[test]
    fn add_load_remove() {
        let d = tempfile::tempdir().unwrap();
        let r = Recipe { id: "borders".into(), name: "Scottish Borders".into(), outline: vec!["osm:1877178".into()] };
        add(d.path(), &r).unwrap();
        assert!(add(d.path(), &r).is_err(), "exclusive");
        std::fs::write(d.path().join("bad.toml"), "id = \"other\"\nname = \"x\"\noutline = [\"osm:1\"]").unwrap();
        let (ok, bad) = load(d.path());
        assert_eq!(ok, vec![r]);
        assert_eq!(bad.len(), 1);
        remove(d.path(), "borders").unwrap();
        assert!(load(d.path()).0.is_empty());
        assert!(d.path().join("borders.toml.removed").exists());
    }
}
