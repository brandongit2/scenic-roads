//! A catalogue's coverage (docs/formats.md, Catalogue `coverage`) kept as JSON text, never as a
//! `serde_json::Value` tree. Its regions' outlines are most of the map's bytes (6.3 million points
//! on 10 Nov, 134 MB of JSON), and as a tree, a list of two numbers for each point, they were most of
//! the map server's memory and much of the lead's (task #1092: ~650 MB a catalogue, ~6.5 million
//! allocations). Each region is kept as its map entry (`r/<id>`, crate::map::Val): a catalogue made
//! of the map shares them with it. The rest of the coverage (a few small fields) is a `Value`. A
//! reader parses what it wants from each region's text (`regions`, `region_texts`).

use crate::map::Val;
use anyhow::{bail, ensure, Context, Result};
use serde::de::DeserializeOwned;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// The coverage a catalogue records: `{"regions": [...], ...}`, or anything else as it is (`Null`:
/// none). Serialized and compared as the JSON it is.
#[derive(Clone)]
pub struct Coverage(Kept);

#[derive(Clone)]
enum Kept {
    /// One that isn't an object whose `regions` are objects with distinct string ids, as it is.
    Whole(Value),
    /// An object's members but `regions`, and its regions in order.
    Split { rest: Map<String, Value>, regions: Vec<Region> },
}

/// A region: its id and its map entry, `{"i": its place, "region": it}` (Val::region).
#[derive(Clone)]
struct Region {
    id: String,
    entry: Val,
}

impl Region {
    /// Its JSON text: its entry's `region`.
    fn json(&self) -> &str {
        let e = self.entry.json();
        let at = e.find(REGION).expect("a region's entry names it") + REGION.len();
        &e[at..e.len() - 1]
    }
}

/// What precedes a region's text in its entry (`{"i":3,"region":{…}}`).
const REGION: &str = ",\"region\":";

impl Default for Coverage {
    fn default() -> Coverage {
        Coverage(Kept::Whole(Value::Null))
    }
}

impl Coverage {
    /// Whether it records nothing.
    pub fn is_null(&self) -> bool {
        matches!(&self.0, Kept::Whole(Value::Null))
    }

    /// Its regions, each parsed as a `T` (what `serde_json::from_value` of its `regions` gave): None
    /// without a list of them, or when one isn't a `T`.
    pub fn regions<T: DeserializeOwned>(&self) -> Option<Vec<T>> {
        match &self.0 {
            Kept::Whole(v) => v.get("regions").and_then(|r| Vec::<T>::deserialize(r).ok()),
            Kept::Split { regions, .. } => regions.iter().map(|r| serde_json::from_str(r.json()).ok()).collect(),
        }
    }

    /// Each region's JSON text, in order (none without a list of them): what a reader wanting a
    /// part of each parses.
    pub fn region_texts(&self) -> Vec<Cow<'_, str>> {
        match &self.0 {
            Kept::Whole(v) => v
                .get("regions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|r| Cow::Owned(r.to_string()))
                .collect(),
            Kept::Split { regions, .. } => regions.iter().map(|r| Cow::Borrowed(r.json())).collect(),
        }
    }

    /// Its `regions` as JSON text (as `Value`'s `to_string` writes it); None without them.
    pub fn regions_json(&self) -> Option<String> {
        match &self.0 {
            Kept::Whole(v) => v.get("regions").map(Value::to_string),
            Kept::Split { regions, .. } => {
                let mut s = String::with_capacity(regions.iter().map(|r| r.json().len() + 1).sum::<usize>() + 2);
                s.push('[');
                for (i, r) in regions.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    s.push_str(r.json());
                }
                s.push(']');
                Some(s)
            }
        }
    }

    /// It as a `Value` (tests, and readers of a small one).
    pub fn to_value(&self) -> Value {
        match &self.0 {
            Kept::Whole(v) => v.clone(),
            Kept::Split { .. } => serde_json::to_value(self).expect("a coverage serializes"),
        }
    }

    /// The coverage whose canonical JSON `v` is (crate::map::Val): split when it's an object whose
    /// `regions` are objects with distinct string ids, else kept whole.
    fn of_val(v: Val) -> Coverage {
        Coverage::split(v.json()).unwrap_or_else(|| Coverage(Kept::Whole(v.value())))
    }

    fn split(json: &str) -> Option<Coverage> {
        if !json.starts_with('{') {
            return None;
        }
        let members: BTreeMap<String, &RawValue> = serde_json::from_str(json).ok()?;
        let list: Vec<&RawValue> = serde_json::from_str(members.get("regions")?.get()).ok()?;
        let mut ids = BTreeSet::new();
        let mut regions = Vec::with_capacity(list.len());
        for (i, r) in list.iter().enumerate() {
            let id = id_of(r.get())?;
            if !ids.insert(id.clone()) {
                return None;
            }
            regions.push(Region { id, entry: Val::of_canonical(format!("{{\"i\":{i}{REGION}{}}}", r.get())) });
        }
        let rest = members
            .into_iter()
            .filter(|(k, _)| k != "regions")
            .map(|(k, r)| Some((k, serde_json::from_str(r.get()).ok()?)))
            .collect::<Option<Map<String, Value>>>()?;
        Some(Coverage(Kept::Split { rest, regions }))
    }

    /// Its map entries (docs/maplog.md §4.1): each region `r/<id>` (its entry, shared), the rest in
    /// `m/coverage`, `m/regions` saying how many regions there are; one kept whole is `m/coverage`
    /// alone (none for none).
    pub(crate) fn each_entry(&self, f: &mut dyn FnMut(&str, Val) -> Result<()>) -> Result<()> {
        match &self.0 {
            Kept::Whole(Value::Null) => Ok(()),
            Kept::Whole(v) => f("m/coverage", v.into()),
            Kept::Split { rest, regions } => {
                for r in regions {
                    f(&format!("r/{}", r.id), r.entry.clone())?;
                }
                f("m/coverage", Value::Object(rest.clone()).into())?;
                f("m/regions", Value::from(regions.len()).into())
            }
        }
    }

    /// The coverage of the map's entries (`each_entry`'s inverse): `m/coverage` (`cov`), the
    /// regions by id with their entries, and `m/regions` (`count`). Each region's entry is kept as
    /// it is, shared with the map: when they're as `each_entry` writes them (else made again, as
    /// the regions' `Value`s were before).
    pub(crate) fn of_entries(cov: Option<&Val>, regions: Vec<(&str, &Val)>, count: Option<usize>) -> Result<Coverage> {
        let Some(count) = count else {
            ensure!(regions.is_empty(), "regions without their count");
            return Ok(cov.map_or_else(Coverage::default, |v| Coverage::of_val(v.clone())));
        };
        if let Some(c) = Coverage::of_entries_as_written(cov, &regions, count) {
            return Ok(c);
        }
        let mut placed: Vec<(u64, Value)> = Vec::with_capacity(regions.len());
        for (id, v) in regions {
            let mut r = v.value();
            let i = r.get("i").and_then(Value::as_u64).with_context(|| format!("entry \"r/{id}\": no place"))?;
            placed.push((i, r.get_mut("region").map(Value::take).unwrap_or_default()));
        }
        placed.sort_by_key(|r| r.0);
        ensure!(
            placed.len() == count && placed.iter().enumerate().all(|(i, r)| r.0 == i as u64),
            "{} regions, {count} counted, or their places aren't 0 to {count}",
            placed.len()
        );
        let mut o = match cov.map(Val::value) {
            Some(Value::Object(o)) => o,
            _ => bail!("regions without the coverage they're in"),
        };
        o.insert("regions".into(), Value::Array(placed.into_iter().map(|r| r.1).collect()));
        Ok(Coverage::from(Value::Object(o)))
    }

    /// `of_entries` when every entry is as `each_entry` writes it: `{"i", "region"}` alone, the
    /// places 0 to `count`, each region an object whose id is its key's, `m/coverage` an object
    /// without `regions`. None for anything else.
    fn of_entries_as_written(cov: Option<&Val>, regions: &[(&str, &Val)], count: usize) -> Option<Coverage> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Entry<'a> {
            i: u64,
            #[serde(borrow)]
            region: &'a RawValue,
        }
        let rest = match cov.map(Val::value) {
            Some(Value::Object(o)) if !o.contains_key("regions") => o,
            _ => return None,
        };
        let mut placed: Vec<(u64, Region)> = Vec::with_capacity(regions.len());
        for &(id, v) in regions {
            let e: Entry = serde_json::from_str(v.json()).ok()?;
            // (Its text where `Region::json` finds it: its entry starts `{"i":<i>,"region":`.)
            if !v.json().starts_with(&format!("{{\"i\":{}{REGION}", e.i)) || !has_id(e.region.get(), id) {
                return None;
            }
            placed.push((e.i, Region { id: id.to_string(), entry: v.clone() }));
        }
        placed.sort_by_key(|r| r.0);
        if placed.len() != count || !placed.iter().enumerate().all(|(i, r)| r.0 == i as u64) {
            return None;
        }
        Some(Coverage(Kept::Split { rest, regions: placed.into_iter().map(|r| r.1).collect() }))
    }
}

/// A region's id: its `id` when it's an object whose `id` is a string.
fn id_of(region: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Id {
        #[serde(default)]
        id: Value,
    }
    if !region.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Id>(region).ok()?.id {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// Whether region `json` is an object whose `id` is `id`: from its start when `id` is its first
/// member (as a region's is: canonical JSON's members are sorted), else parsed.
fn has_id(json: &str, id: &str) -> bool {
    let head = format!("{{\"id\":{}", Value::from(id));
    match json.strip_prefix(&head) {
        Some(after) => after.starts_with([',', '}']),
        None => id_of(json).as_deref() == Some(id),
    }
}

impl From<Value> for Coverage {
    fn from(v: Value) -> Coverage {
        Coverage::of_val(Val::from(&v))
    }
}

impl PartialEq for Coverage {
    fn eq(&self, o: &Coverage) -> bool {
        match (&self.0, &o.0) {
            (Kept::Whole(a), Kept::Whole(b)) => a == b,
            (Kept::Split { rest: a, regions: x }, Kept::Split { rest: b, regions: y }) => {
                a == b && x.len() == y.len() && x.iter().zip(y).all(|(r, s)| r.json() == s.json())
            }
            _ => false,
        }
    }
}

impl std::fmt::Debug for Coverage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&serde_json::to_string(self).map_err(|_| std::fmt::Error)?)
    }
}

impl Serialize for Coverage {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (rest, regions) = match &self.0 {
            Kept::Whole(v) => return v.serialize(s),
            Kept::Split { rest, regions } => (rest, regions),
        };
        // (Its members in order, as a `Value`'s object writes them: `regions` among the rest.)
        let mut m = s.serialize_map(Some(rest.len() + 1))?;
        let mut put = Some(Texts(regions));
        for (k, v) in rest {
            if k.as_str() > "regions" {
                if let Some(t) = put.take() {
                    m.serialize_entry("regions", &t)?;
                }
            }
            m.serialize_entry(k, v)?;
        }
        if let Some(t) = put {
            m.serialize_entry("regions", &t)?;
        }
        m.end()
    }
}

/// The regions as a list of their texts.
struct Texts<'a>(&'a [Region]);

impl Serialize for Texts<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut q = s.serialize_seq(Some(self.0.len()))?;
        for r in self.0 {
            let raw: &RawValue = serde_json::from_str(r.json()).map_err(serde::ser::Error::custom)?;
            q.serialize_element(raw)?;
        }
        q.end()
    }
}

impl<'de> Deserialize<'de> for Coverage {
    /// Read as canonical JSON (crate::map::Val), never a `Value` tree of its regions.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Coverage, D::Error> {
        Ok(Coverage::of_val(Val::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn region(id: &str) -> Value {
        json!({"id": id, "name": id.to_uppercase(), "outline": ["osm:1"],
            "shapes": {"osm:1": [[[[1.5, 50.0], [2.0, 50.25], [2.0, 51.0], [1.5, 50.0]]]]}})
    }

    /// Whatever it holds, it's the JSON it was made of: serialized, compared, read back, its
    /// regions and their text; split only where its regions can be told apart by id.
    #[test]
    fn a_coverage_is_the_json_it_was_made_of() {
        let cases = [
            json!(null),
            json!({"recorded": true, "regions": [region("a"), region("b")], "zz": [1]}),
            json!({"regions": []}),
            json!({"regions": [{"name": "no id"}]}),
            json!({"regions": [region("a"), region("a")]}),
            json!({"regions": [[1, 2]]}),
            json!({"regions": 5}),
            json!([1, 2]),
            json!({"a": {"b": 1}, "regions": [{"area": 1, "id": "x"}]}),
        ];
        for v in cases {
            let c = Coverage::from(v.clone());
            assert_eq!(serde_json::to_value(&c).unwrap(), v, "{v}");
            assert_eq!(serde_json::to_string(&c).unwrap(), serde_json::to_string(&v).unwrap());
            assert_eq!(c.to_value(), v);
            assert_eq!(c.is_null(), v.is_null());
            let back: Coverage = serde_json::from_str(&v.to_string()).unwrap();
            assert_eq!(back, c);
            assert_eq!(c.regions_json(), v.get("regions").map(Value::to_string));
            let texts: Vec<String> = c.region_texts().into_iter().map(Cow::into_owned).collect();
            let want: Vec<String> =
                v.get("regions").and_then(Value::as_array).into_iter().flatten().map(Value::to_string).collect();
            assert_eq!(texts, want);
            assert_eq!(
                c.regions::<Value>(),
                v.get("regions").and_then(|r| serde_json::from_value::<Vec<Value>>(r.clone()).ok())
            );
            let split = matches!(c.0, Kept::Split { .. });
            let distinct = v.get("regions").and_then(Value::as_array).is_some_and(|rs| {
                let ids: BTreeSet<&str> = rs.iter().filter_map(|r| r.get("id")?.as_str()).collect();
                ids.len() == rs.len()
            });
            assert_eq!(split, distinct, "{v}");
        }
        assert_ne!(
            Coverage::from(json!({"regions": [region("a")]})),
            Coverage::from(json!({"regions": [region("b")]}))
        );
    }

    /// A catalogue made of the map holds the map's regions, not copies; one whose entries aren't as
    /// the map writes them is made as before.
    #[test]
    fn a_catalogue_of_the_map_shares_its_regions() {
        let mut c = crate::catalogue::Catalogue::new(4);
        c.coverage = json!({"recorded": true, "regions": [region("b"), region("a")]}).into();
        let m = crate::map::Map::of(&c).unwrap();
        let back = m.catalogue().unwrap();
        assert_eq!(back, c);
        let Kept::Split { regions, .. } = &back.coverage.0 else { panic!("split") };
        for r in regions {
            assert!(r.entry.shares(&m.entries()[&format!("r/{}", r.id)]), "{}", r.id);
        }
        // (An entry with a member past `region`, and one whose region's id isn't its key's.)
        let mut e = m.entries().clone();
        e.insert("r/a".into(), json!({"i": 1, "region": region("a"), "z": 1}).into());
        let odd = crate::map::catalogue(&e, 4, "").unwrap();
        assert_eq!(odd.coverage, c.coverage);
        let mut e = m.entries().clone();
        e.insert("r/a".into(), json!({"i": 1, "region": region("x")}).into());
        let odd = crate::map::catalogue(&e, 4, "").unwrap();
        assert_eq!(odd.coverage.to_value(), json!({"recorded": true, "regions": [region("b"), region("x")]}));
        assert!(crate::map::entries(&odd).unwrap().contains_key("r/x"));
    }
}
