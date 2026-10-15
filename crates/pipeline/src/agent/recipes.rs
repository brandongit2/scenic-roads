//! A region as the owner's tools write it (`scenic add`, the Regions panel): `inputs/regions/<id>.toml`
//! with `id`, `name` and `outline` (docs/inputs.md §6.1.3, the region file, whose check this is); a
//! region renamed to `<id>.toml.removed` is removed. The build reads the regions' accepted version
//! (crate::inputs::regions::read), never these files.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
}

/// A recipe's outline entries, hashed: what a catalog's `--ready` names a region built with
/// (`<id>=<digest>`), so one redrawn since isn't recorded as built with its new outline.
pub fn outline_digest(outline: &[String]) -> String {
    store::naming::hash16(outline.join("\n").as_bytes())
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !id.starts_with('-')
}

impl Recipe {
    /// Checked as a region file of these fields is (crate::inputs::regions::file::parse: its parts
    /// `osm:`, `shape:` or `region:`).
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_id(&self.id), "{:?}: ids are lower-case letters, digits and dashes", self.id);
        let text = toml::to_string(self)?;
        crate::inputs::regions::file::parse(&format!("{}.toml", self.id), &text).map_err(|fs| anyhow::anyhow!("{}: {}", self.id, fs.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")))?;
        Ok(())
    }
}

/// A recipe file's text, parsed and checked: it validates, and `name`, its file name, is its id's
/// (`<id>.toml`). (The map's server reads the files through its NAS I/O pool, then checks them here.)
pub fn parse(name: &str, text: &str) -> Result<Recipe> {
    let r: Recipe = toml::from_str(text)?;
    r.validate()?;
    ensure!(name.strip_suffix(".toml") == Some(r.id.as_str()), "the file is {name} but its id is {:?}", r.id);
    Ok(r)
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
    fn add_load_remove() {
        let d = tempfile::tempdir().unwrap();
        let r = Recipe { id: "borders".into(), name: "Scottish Borders".into(), outline: vec!["osm:1877178".into()] };
        add(d.path(), &r).unwrap();
        assert!(add(d.path(), &r).is_err(), "exclusive");
        assert!(add(d.path(), &Recipe { id: "p".into(), name: "P".into(), outline: vec!["place:1,2,3".into()] }).is_err(), "no longer a part");
        remove(d.path(), "borders").unwrap();
        assert!(!d.path().join("borders.toml").exists());
        assert!(d.path().join("borders.toml.removed").exists());
        // A file's text is checked against its name.
        let text = toml::to_string(&r).unwrap();
        assert_eq!(parse("borders.toml", &text).unwrap(), r);
        assert!(parse("other.toml", &text).is_err());
        assert!(parse("borders.toml", "id = \"borders\"").is_err());
    }
}
