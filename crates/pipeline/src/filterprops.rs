//! The numbers the heritage layers' filters work on, stamped onto their features in the heritage
//! chain's stand-in root (`data/build`, after heritagedetails.py and areadetails.py; the details
//! stay on the server, these few fields are what MapLibre filters and the in-view counts need):
//!
//!   heritage.json   by (year built), dy (year designated), sl (Wikipedia/Wikimedia sitelinks),
//!                   wp (has a Wikipedia article), t (kind of designation: the register's `tier`,
//!                   else by level: `tier`)
//!   heritage-areas.json, special.json, indigenous.json   a (area km²)
//!
//! The stops & sights' values are the marks job's (`interest::poi_filter_props`). The files are
//! read and written as Python's `json.load`/`json.dumps(ensure_ascii=False, separators=(",",":"))`
//! do (the chain's scripts after it read them, and the outputs keep their bytes): keys in their
//! order, numbers as Python writes them (crate::pyjson).

use crate::pyjson::{dumps, loads, Py};
use crate::timings::{phase, Class};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// The first run of 3–4 digits (a minus sign before it kept) in `str(v or "")`.
fn year(v: &Py) -> Option<Py> {
    let s = if v.truthy() { v.py_str() } else { String::new() };
    let b = s.as_bytes();
    for i in 0..b.len() {
        let j0 = if b[i] == b'-' { i + 1 } else { i };
        let n = b[j0.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n >= 3 {
            let t: i64 = s[i..j0 + n.min(4)].parse().ok()?;
            return Some(Py::Int(t.to_string()));
        }
    }
    None
}

/// A site's kind of designation for the Heritage sites toggles (web/src/basemap.ts
/// HERITAGE_TIERS): its register's (`t`, from the entry's `tier`: docs/inputs.md §6.5), else by
/// its level, World Heritage by its category.
fn tier(p: &Py) -> Py {
    if p.at("t").truthy() {
        return p.at("t").clone();
    }
    let lv = if p.at("level").truthy() { p.at("level").number() } else { Some(5.0) };
    let t = match lv {
        Some(l) if l == 1.0 => {
            if matches!(p.at("category"), Py::Str(c) if c == "Natural" || c == "Mixed") {
                "w.n"
            } else {
                "w.c"
            }
        }
        Some(l) if l == 2.0 => "n.top",
        Some(l) if l == 3.0 => "n.second",
        Some(l) if l == 4.0 => "p.des",
        _ => "m.des",
    };
    Py::Str(t.into())
}

/// A details file's records by their `i` (none when it's missing).
fn jsonl(b: &Path, name: &str) -> Result<HashMap<String, Py>> {
    let mut out = HashMap::new();
    let p = b.join(name);
    if p.exists() {
        for line in std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?.lines() {
            let r = loads(line).with_context(|| format!("a line of {name}"))?;
            out.insert(r.get("i").context("a record without `i`")?.key(), r);
        }
    }
    Ok(out)
}

fn read(b: &Path, name: &str) -> Result<Py> {
    let p = b.join(name);
    loads(&std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?)
        .with_context(|| format!("parse {name}"))
}

fn write(b: &Path, name: &str, fc: &Py) -> Result<()> {
    let tmp = b.join(format!("{name}.tmp"));
    std::fs::write(&tmp, dumps(fc))?;
    std::fs::rename(&tmp, b.join(name))?;
    Ok(())
}

fn features(fc: &mut Py) -> Result<&mut Vec<Py>> {
    match fc {
        Py::Dict(d) => match d.iter_mut().find(|(k, _)| k == "features") {
            Some((_, Py::List(l))) => Ok(l),
            _ => bail!("a FeatureCollection without features"),
        },
        _ => bail!("not a FeatureCollection"),
    }
}

fn props(f: &mut Py) -> Option<&mut Py> {
    match f {
        Py::Dict(d) => d.iter_mut().find(|(k, _)| k == "properties").map(|(_, v)| v),
        _ => None,
    }
}

/// The heritage sites' values (heritage.json, from details-heritage.jsonl).
fn heritage(b: &Path) -> Result<()> {
    let p = phase("the layers and details read", Class::Disk);
    let mut fc = read(b, "heritage.json")?;
    let det = jsonl(b, "details-heritage.jsonl")?;
    drop(p);
    let ph = phase("the filter values set", Class::Compute);
    let empty = Py::Dict(vec![]);
    let mut n = 0;
    for f in features(&mut fc)? {
        let p = props(f).context("a feature without properties")?;
        let d = det.get(&p.at("i").key()).unwrap_or(&empty);
        for key in ["by", "dy", "sl", "wp"] {
            p.pop(key);
        }
        if let Some(by) = year(d.at("inception")) {
            p.set("by", by);
        }
        if let Some(dy) = year(p.at("date")) {
            p.set("dy", dy);
        }
        if d.at("sl").truthy() {
            p.set("sl", d.at("sl").clone());
        }
        if d.at("wiki").truthy() {
            p.set("wp", Py::Int("1".into()));
        }
        let t = tier(p);
        p.set("t", t);
        n += (p.get("by").is_some() || p.get("wp").is_some()) as usize;
    }
    drop(ph);
    let _p = phase("the layers written", Class::Disk);
    write(b, "heritage.json", &fc)?;
    eprintln!("heritage.json: {n} sites with a build year or article");
    Ok(())
}

/// The areas' sizes (`a`, km²) from their details.
fn areas(b: &Path) -> Result<()> {
    for (name, layer) in
        [("heritage-areas.json", "harea"), ("special.json", "special"), ("indigenous.json", "indigenous")]
    {
        let p = phase("the layers and details read", Class::Disk);
        let mut fc = read(b, name)?;
        let det = jsonl(b, &format!("details-{layer}.jsonl"))?;
        drop(p);
        let p = phase("the filter values set", Class::Compute);
        for f in features(&mut fc)? {
            let Some(p) = props(f) else { bail!("a feature of {name} without properties") };
            let a = det.get(&p.at("i").key()).map(|r| r.at("area_km2").clone()).unwrap_or(Py::Null);
            if a.truthy() {
                p.set("a", a);
            }
        }
        drop(p);
        let _p = phase("the layers written", Class::Disk);
        write(b, name, &fc)?;
    }
    eprintln!("areas: a (km²) on heritage districts, biospheres & co., Indigenous lands");
    Ok(())
}

/// Stamps the filters' values onto the heritage layers in `b` (the stand-in root's data/build).
pub fn run(b: &Path) -> Result<()> {
    heritage(b)?;
    areas(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heritage_values() {
        let p = loads(r#"{"i":3,"level":1,"category":"Mixed","date":"designated 1985-05-01"}"#).unwrap();
        assert_eq!(tier(&p), Py::Str("w.n".into()));
        assert_eq!(year(p.at("date")), Some(Py::Int("1985".into())));
        assert_eq!(year(&Py::Float(-123.5)), Some(Py::Int("-123".into())));
        assert_eq!(year(&Py::Int("12".into())), None);
        assert_eq!(tier(&loads(r#"{"level":0}"#).unwrap()), Py::Str("m.des".into()));
        assert_eq!(tier(&loads(r#"{"t":"x","level":2}"#).unwrap()), Py::Str("x".into()));
    }
}
