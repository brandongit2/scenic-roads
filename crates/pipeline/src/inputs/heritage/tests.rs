use super::*;
use crate::inputs::heritage_matches::{self as hm, Matches};

fn ids(fs: &[Finding]) -> Vec<(&str, Level)> {
    fs.iter().map(|f| (f.check(), f.level)).collect()
}

fn point(id: &str, lon: f64, lat: f64, extra: &str) -> String {
    format!(r#"{{"type":"Feature","geometry":{{"type":"Point","coordinates":[{lon},{lat}]}},"properties":{{"id":"{id}","name":"N {id}"{extra}}}}}"#)
}

fn fc(feats: &[String]) -> Vec<u8> {
    format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, feats.join(",")).into_bytes()
}

const TOML: &[u8] = b"name = \"Test register\"\ncredit = \"A ministry\"\nlicence = \"CC-BY-4.0\"\nclass = \"heritage\"\nterritory = [\"FR\"]\nwikidata_property = \"P380\"\nosm_ref_tags = [\"ref:mhs\"]\n";

#[test]
fn register_ids_and_files() {
    assert!(valid_id("fr-merimee") && valid_id("unesco-whc") && valid_id("2x"));
    assert!(!valid_id("FR") && !valid_id("-x") && !valid_id("a_b") && !valid_id(""));
    assert_eq!(split("fr-merimee.geojson"), Some(("fr-merimee", "geojson")));
    assert_eq!(split("fr-merimee.toml"), Some(("fr-merimee", "toml")));
    assert_eq!(split("fr-merimee.json"), None);
    assert_eq!(split("sub/x.toml"), None);
    let h = Heritage::default();
    assert_eq!(ids(&h.file("notes.txt", b"x").findings), [("he-shape", Level::Error)]);
}

#[test]
fn a_description_and_its_own_fields() {
    let h = Heritage::default();
    let c = h.file("fr-merimee.toml", TOML);
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    assert_eq!(c.facts["class"], "heritage");
    assert_eq!(c.facts["description"]["credit"], "A ministry");
    // A credit edited: the same keyed digest (no build reruns); a class changed: another.
    let credit = String::from_utf8(TOML.to_vec()).unwrap().replace("A ministry", "The ministry");
    assert_eq!(h.file("fr-merimee.toml", credit.as_bytes()).keyed, c.keyed);
    let class = String::from_utf8(TOML.to_vec()).unwrap().replace("\"heritage\"", "\"world-heritage\"");
    assert_ne!(h.file("fr-merimee.toml", class.as_bytes()).keyed, c.keyed);
    assert_eq!(ids(&h.file("a.toml", b"credit = \"x\"\nlicence = \"y\"\nclass = \"castle\"\nterritory = [\"FR\"]\n").findings), [("he-class", Level::Error)]);
    assert_eq!(ids(&h.file("a.toml", b"credit = \"x\"\nlicence = \"y\"\nclass = \"special\"\n").findings), [("he-desc", Level::Error)]);
    assert_eq!(ids(&h.file("a.toml", b"credit = \"x\"\nlicence = \"y\"\nclass = \"special\"\nterritory = \"world\"\nwikidata_property = \"380\"\n").findings), [("he-desc", Level::Error)]);
    assert_eq!(ids(&h.file("a.toml", b"credit = \"x\"\nlicence = \"y\"\nclass = \"special\"\nterritory = \"world\"\nosm_ref_tags = \"ref\"\n").findings), [("he-desc", Level::Error)]);
    assert_eq!(ids(&h.file("a.toml", b"licence = \"y\"\nclass = \"special\"\nterritory = \"world\"\n").findings), [("description", Level::Error)]);
}

#[test]
fn the_entries_are_checked() {
    let h = Heritage::default();
    let ok = fc(&[point("PA1", 2.3, 48.8, r#","level":2,"designation":"MH classé","wikidata":"Q1","tier":"n.top","dot":true,"municipality":null,"bylaw":"7"}"#.trim_end_matches('}')), point("PA2", 3.0, 45.0, r#","level":3,"designation":"MH inscrit""#)]);
    let c = h.file("fr-merimee.geojson", &ok);
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    assert_eq!(c.facts["entries"], 2);
    assert_eq!(c.facts["box"], serde_json::json!([2.3, 45.0, 3.0, 48.8]));
    let bad = fc(&[
        point("A", 200.0, 48.0, ""),
        point("B", 2.0, 48.0, r#","level":6"#),
        point("C", 2.0, 48.0, r#","wikidata":"X1""#),
        point("D", 2.0, 48.0, r#","tier":"z.z""#),
        point("E", 2.0, 48.0, r#","kind":"castle""#),
        r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[2,48]},"properties":{"name":"no id"}}"#.into(),
        r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[0,0]]]},"properties":{"id":"F"}}"#.into(),
        point("G", 2.0, 48.0, ""),
        point("G", 2.1, 48.0, ""),
    ]);
    let c = h.file("x.geojson", &bad);
    assert_eq!(ids(&c.findings), [("he-entry", Level::Error), ("he-twice", Level::Error)]);
    assert_eq!(c.findings[0].lines.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(c.findings[1].lines, [(9, "G: an id an earlier entry has".to_string())]);
    assert_eq!(ids(&h.file("x.geojson", b"{\"type\":\"Feature\"}").findings), [("he-json", Level::Error)]);
    assert_eq!(ids(&h.file("x.geojson", b"{").findings), [("he-json", Level::Error)]);
}

#[test]
fn warnings_name_what_they_found() {
    let unnamed = fc(&[r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[2,48]},"properties":{"id":"A","level":2,"approx":true}}"#.into(), point("B", 2.0, 48.0, r#","approx":true"#), point("C", 2.0, 48.0, "")]);
    let h = Heritage::default();
    let c = h.file("x.geojson", &unnamed);
    assert_eq!(ids(&c.findings), [("he-unnamed", Level::Warning), ("he-approx", Level::Warning)]);
    assert_eq!(c.findings[0].lines, [(1, "A: no name".to_string())]);
    // A loss over a tenth since the accepted version: a warning naming the counts and versions.
    let mut prev = BTreeMap::new();
    prev.insert("x.geojson".to_string(), (4u64, "sources/inputs/heritage/x.1111111111111111.geojson".to_string()));
    let h = Heritage { outlines: None, prev };
    let lost = h.file("x.geojson", &fc(&[point("A", 2.0, 48.0, ""), point("B", 2.0, 48.0, "")]));
    assert_eq!(ids(&lost.findings), [("he-loss", Level::Warning)]);
    assert!(lost.findings[0].message.contains("4 → 2 entries (a loss of 50 %)"), "{}", lost.findings[0].message);
    let again = h.file("x.geojson", &fc(&[point("A", 2.0, 48.0, ""), point("C", 2.0, 48.0, "")]));
    assert_ne!(again.findings[0].id, lost.findings[0].id, "another version, another finding");
    assert!(h.file("x.geojson", &fc(&[point("A", 2.0, 48.0, ""), point("B", 2.0, 48.0, ""), point("C", 2.0, 48.0, ""), point("D", 2.0, 48.0, "")])).findings.is_empty());
    let was = FileEntry { file: "sources/inputs/heritage/x.1111111111111111.geojson".into(), size: 1, keyed: "k".into(), facts: serde_json::Map::from_iter([("entries".to_string(), serde_json::json!(4))]) };
    let gone = h.removed("x.geojson", &was);
    assert_eq!(ids(&gone), [("he-removed", Level::Warning)]);
    assert!(h.removed("x.toml", &was).is_empty());
}

fn version<'a>(files: &'a BTreeMap<String, Vec<u8>>, read: &'a dyn Fn(&str) -> Result<Arc<Vec<u8>>>) -> Version<'a> {
    Version { paths: files.keys().cloned().collect(), read }
}

#[test]
fn the_version_whole_and_whats_derived() {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("fr-merimee.toml".into(), TOML.to_vec());
    files.insert("fr-merimee.geojson".into(), fc(&[point("A", 2.0, 48.0, r#","level":2,"designation":"MH""#), point("B", 3.0, 49.0, r#","level":2"#)]));
    files.insert("lonely.geojson".into(), fc(&[]));
    files.insert("parks.toml".into(), b"credit = \"x\"\nlicence = \"y\"\nclass = \"special\"\nterritory = \"world\"\n".to_vec());
    files.insert("parks.geojson".into(), fc(&[point("P", 2.0, 48.0, r#","kind":"geopark""#), point("Q", 2.0, 48.0, "")]));
    let read = |p: &str| -> Result<Arc<Vec<u8>>> { Ok(Arc::new(files[p].clone())) };
    let h = Heritage::default();
    let w = h.whole(&version(&files, &read)).unwrap();
    assert_eq!(ids(&w), [("he-needs", Level::Error), ("he-pair", Level::Error), ("he-needs", Level::Error)]);
    assert_eq!(w[0].lines, [(2, "B: no level or designation (a heritage register's entries need both)".to_string())]);
    assert_eq!(w[1].files, ["lonely.geojson"]);
    assert_eq!(w[2].lines.len(), 1);
    let d = h.derive(&version(&files, &read), &BTreeSet::new()).unwrap();
    assert_eq!(d.facts["fr-merimee.toml"].0["extent"], serde_json::json!([2.0, 48.0, 3.0, 49.0]));
    assert!(!d.facts.contains_key("fr-merimee.geojson"));
    let kept = h.derive(&version(&files, &read), &BTreeSet::from(["fr-merimee.toml".to_string()])).unwrap();
    assert!(!kept.facts.contains_key("fr-merimee.toml"));
    let all: BTreeSet<String> = files.keys().cloned().collect();
    assert_eq!(h.partners("fr-merimee.toml", &all), ["fr-merimee.geojson"]);
    assert_eq!(h.partners("lonely.geojson", &all), Vec::<String>::new());
}

#[test]
fn entries_far_outside_their_territory() {
    // A territory one degree square at (2, 48): an entry inside, one 10 km out, one 100 km out.
    let ring = vec![[2.0, 48.0], [3.0, 48.0], [3.0, 49.0], [2.0, 49.0], [2.0, 48.0]];
    let e7: Vec<Vec<[i32; 2]>> = vec![ring.iter().map(|p: &[f64; 2]| [(p[0] * 1e7) as i32, (p[1] * 1e7) as i32]).collect()];
    let shapes: Shapes = vec![([2.0, 48.0, 3.0, 49.0], vec![ring], e7)];
    let e = |id: &str, at: [f64; 2]| Entry { id: id.into(), at, level: Some(2), designation: true, kind: false, approx: false, named: true };
    let entries = [e("in", [2.5, 48.5]), e("near", [2.5, 49.09]), e("far", [2.5, 49.9]), e("east", [4.5, 48.5])];
    let far: Vec<&str> = Heritage::far(&shapes, &entries).iter().map(|e| e.id.as_str()).collect();
    assert_eq!(far, ["far", "east"]);
}

#[test]
fn the_accepted_registers() {
    let mut files = BTreeMap::new();
    let entry = |file: &str, facts: serde_json::Value| FileEntry { file: file.into(), size: 1, keyed: format!("k-{file}"), facts: facts.as_object().cloned().unwrap_or_default() };
    files.insert("b.geojson".to_string(), entry("sources/inputs/heritage/b.2222222222222222.geojson", serde_json::json!({"box": [1.0, 2.0, 3.0, 4.0], "entries": 3})));
    files.insert("b.toml".to_string(), entry("sources/inputs/heritage/b.3333333333333333.toml", serde_json::json!({})));
    files.insert("a.geojson".to_string(), entry("sources/inputs/heritage/a.4444444444444444.geojson", serde_json::json!({"entries": 0})));
    files.insert("a.toml".to_string(), entry("sources/inputs/heritage/a.5555555555555555.toml", serde_json::json!({})));
    files.insert("c.geojson".to_string(), entry("sources/inputs/heritage/c.6666666666666666.geojson", serde_json::json!({})));
    let a = crate::inputs::Accepted { root: std::path::PathBuf::from("/nowhere"), version: "v".into(), index: crate::inputs::Index { fmt: 1, unit: UNIT.into(), checks: "heritage 1".into(), files, accepted: BTreeMap::new() } };
    let r = registers(&a);
    assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(r[1].bbox, Some([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(r[0].bbox, None);
    assert_eq!(r[1].keyed, "k-sources/inputs/heritage/b.2222222222222222.geojson k-sources/inputs/heritage/b.3333333333333333.toml");
}

#[test]
fn matches_lines_and_contradictions() {
    assert_eq!(hm::parse(r#"{"a":"ca-dfhd:1","b":"ca-qc-rpcq:2","same":true,"note":"x"}"#), Ok(hm::Match { a: "ca-dfhd:1".into(), b: "ca-qc-rpcq:2".into(), same: true }));
    assert!(hm::parse(r#"{"a":"ca-dfhd:1","b":"ca-dfhd:1","same":true}"#).is_err());
    assert!(hm::parse(r#"{"a":"CA:1","b":"x:2","same":true}"#).is_err());
    assert!(hm::parse(r#"{"a":"a:1","b":"b:2","same":"yes"}"#).is_err());
    assert!(hm::parse(r#"{"a":"a:1","b":"b:2","same":false,"note":3}"#).is_err());
    let m = Matches { entries: Some(["a:1", "b:2", "c:3"].map(String::from).into()) };
    let c = m.file("m.jsonl", b"{\"a\":\"a:1\",\"b\":\"b:2\",\"same\":true}\nnot json\n{\"a\":\"a:1\",\"b\":\"z:9\",\"same\":false}\n");
    assert_eq!(ids(&c.findings), [("hm-line", Level::Error), ("hm-nowhere", Level::Warning)]);
    assert_eq!(c.findings[1].lines, [(3, "no accepted register has z:9".to_string())]);
    assert_eq!(ids(&m.file("m.txt", b"").findings), [("hm-shape", Level::Error)]);
    // a = b = c by same pairs, across files, and a ≠ c: a contradiction.
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("1.jsonl".into(), b"{\"a\":\"a:1\",\"b\":\"b:2\",\"same\":true}\n".to_vec());
    files.insert("2.jsonl".into(), b"{\"a\":\"c:3\",\"b\":\"b:2\",\"same\":true}\n{\"a\":\"a:1\",\"b\":\"c:3\",\"same\":false}\n".to_vec());
    let read = |p: &str| -> Result<Arc<Vec<u8>>> { Ok(Arc::new(files[p].clone())) };
    let w = m.whole(&version(&files, &read)).unwrap();
    assert_eq!(ids(&w), [("hm-contra", Level::Error)]);
    assert_eq!(w[0].lines, [(2, "a:1 and c:3 are joined by same pairs".to_string())]);
    files.insert("1.jsonl".into(), b"{\"a\":\"a:1\",\"b\":\"b:2\",\"same\":false}\n".to_vec());
    let read = |p: &str| -> Result<Arc<Vec<u8>>> { Ok(Arc::new(files[p].clone())) };
    assert!(m.whole(&version(&files, &read)).unwrap().is_empty());
    let manifest = BTreeMap::from([(crate::inputs::logical(UNIT), "sources/inputs/heritage/@index.7777777777777777.json".to_string())]);
    assert_eq!(m.context(&manifest), ["heritage sources/inputs/heritage/@index.7777777777777777.json"]);
}
