use super::*;
use crate::pyjson::Py;

/// The fixture's zips (fixture.json) and what the Python counters made of them (fixture_py.json:
/// dem/railgtfs.py, dem/ferrygtfs.py and dem/staticgtfs.py as they were, CPython 3.14).
const FIXTURE: &str = include_str!("fixture.json");
const PY: &str = include_str!("fixture_py.json");

/// A zip of `files`, stored.
fn write_zip(path: &Path, files: &[(String, Vec<u8>)]) {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let at = out.len() as u32;
        let crc = crc32fast::hash(data);
        let common = |v: &mut Vec<u8>| {
            v.extend(0u16.to_le_bytes()); // flags
            v.extend(0u16.to_le_bytes()); // stored
            v.extend([0u8; 4]); // time, date
            v.extend(crc.to_le_bytes());
            v.extend((data.len() as u32).to_le_bytes());
            v.extend((data.len() as u32).to_le_bytes());
            v.extend((name.len() as u16).to_le_bytes());
            v.extend(0u16.to_le_bytes());
        };
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        common(&mut out);
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        common(&mut central);
        central.extend([0u8; 10]);
        central.extend(at.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_at = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    std::fs::write(path, out).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    zips: std::collections::BTreeMap<String, PathBuf>,
    lines: Vec<(String, [[f64; 2]; 2])>,
    py: serde_json::Value,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let f: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let mut zips = std::collections::BTreeMap::new();
    for (name, tables) in f["zips"].as_object().unwrap() {
        let files: Vec<(String, Vec<u8>)> = tables
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().as_bytes().to_vec()))
            .collect();
        let p = dir.path().join(format!("{name}.zip"));
        write_zip(&p, &files);
        zips.insert(name.clone(), p);
    }
    let pt = |v: &serde_json::Value| [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()];
    let lines = f["lines"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), [pt(&v[0]), pt(&v[1])])).collect();
    Fixture { _dir: dir, zips, lines, py: serde_json::from_str(PY).unwrap() }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn the_rail_counter_makes_the_pythons_bytes() {
    let fx = fixture();
    let p = |k: &str| fx.zips[k].to_string_lossy().into_owned();
    let feeds = serde_json::json!([
        {"id": "a", "provider": "P", "url": "u", "fetched": "2026-10-01", "path": p("rail_a")},
        {"id": "b", "fetched": "2026-10-01", "path": p("rail_b"), "licence": "L"},
        {"id": "c", "status": "no answer"},
        {"id": "old", "fetched": "2026-10-01", "path": p("rail_b")},
        {"id": "new", "fetched": "2026-10-01", "path": p("rail_b"), "replaces": ["old"]}
    ]);
    let r = railcount::run(feeds.as_array().unwrap(), &[("st.zip".to_string(), fx.zips["static"].clone())]).unwrap();
    assert_eq!(hex(&r.pairs), fx.py["pairs"].as_str().unwrap());
    assert_eq!(hex(&r.counts), fx.py["counts"].as_str().unwrap());
    assert_eq!(r.used, fx.py["used"].as_str().unwrap());
    assert_eq!(r.routes, fx.py["routes"].as_str().unwrap());
    let day = Date::parse_iso("2026-10-01").unwrap();
    assert!(railcount::has_service(&fx.zips["rail_a"], day).unwrap());
    assert!(!railcount::has_service(&fx.zips["rail_a"], Date::parse_iso("2030-01-01").unwrap()).unwrap());
}

#[test]
fn the_ferry_counter_makes_the_pythons_records() {
    let fx = fixture();
    let dumps = |v: Vec<Py>| crate::pyjson::dumps(&Py::List(v));
    let feed = ferrycount::Feed {
        id: "f".into(),
        name: Some("Ferry Co".into()),
        url: "fu".into(),
        source: Some("fs".into()),
        ferry_routes: Vec::new(),
    };
    let mut un = Vec::new();
    let day = Date::parse_iso("2026-10-01").unwrap();
    let recs = ferrycount::process(&feed, &fx.zips["ferry"], &fx.lines, Some(day), Some(&mut un)).unwrap();
    assert_eq!(dumps(recs), fx.py["ferry"].as_str().unwrap());
    assert_eq!(dumps(un), fx.py["ferry_unmatched"].as_str().unwrap());
    let z = Zip::open(&fx.zips["static"]).unwrap();
    assert_eq!(dumps(statics::ferry_counts(&z).unwrap()), fx.py["static_counts"].as_str().unwrap());
    let st = ferrycount::Feed { id: "st.zip".into(), ..Default::default() };
    let mut un = Vec::new();
    let recs = ferrycount::process(&st, &fx.zips["static"], &fx.lines, None, Some(&mut un)).unwrap();
    assert_eq!(dumps(recs), fx.py["static_ferry"].as_str().unwrap());
    assert_eq!(dumps(un), fx.py["static_unmatched"].as_str().unwrap());
    assert!(ferrycount::has_service(&fx.zips["ferry"], &feed, day).unwrap());
}

/// Python's csv module's reading: quotes, line ends, empty lines, a byte-order mark, the field
/// limit.
#[test]
fn csv_as_pythons() {
    let rd = |s: &str, lines: Lines| {
        let mut c = Csv::with_lines(Box::new(std::io::Cursor::new(s.as_bytes().to_vec())), false, lines);
        let mut v = Vec::new();
        while let Some(r) = c.record().unwrap() {
            v.push(r);
        }
        v
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        rd("\u{feff}a,\"b,c\"\r\n\r\nd,\"e\r\nf\"\rg\"h\",\"i\"\"j\"x\n\"open", Lines::Raw),
        vec![s(&["a", "b,c"]), s(&[]), s(&["d", "e\r\nf"]), s(&["g\"h\"", "i\"jx"]), s(&["open"])]
    );
    assert_eq!(rd("a,\"x\r\ny\"\r\nb\rc", Lines::Universal), vec![s(&["a", "x\ny"]), s(&["b"]), s(&["c"])]);
    let mut c = Csv::with_lines(Box::new(std::io::Cursor::new(b"a\rb\n".to_vec())), false, Lines::LfOnly);
    assert_eq!(c.record().unwrap_err().kind, Kind::Csv);
    let big = format!("{}\n", "x".repeat(131_073));
    let mut c = Csv::new(Box::new(std::io::Cursor::new(big.into_bytes())), false);
    assert_eq!(c.record().unwrap_err().msg, "field larger than field limit (131072)");
    let mut c = Csv::new(Box::new(std::io::Cursor::new(b"a\xff,b\n".to_vec())), true);
    assert_eq!(c.record().unwrap(), Some(s(&["a\u{fffd}", "b"])));
}

#[test]
fn numbers_and_days_as_pythons() {
    assert_eq!(py_int(" 1_000 "), Some(1000));
    assert_eq!(py_int("+7"), Some(7));
    assert_eq!(py_int("١٢"), Some(12));
    assert_eq!(py_int("1__0"), None);
    assert_eq!(py_int("_1"), None);
    assert_eq!(py_int(""), None);
    let d = Date::new(2026, 10, 23).unwrap();
    assert_eq!((d.weekday(), d.iso(), d.iso_week()), (4, "2026-10-23".to_string(), (2026, 43)));
    assert_eq!(Date::new(2027, 1, 1).unwrap().iso_week(), (2026, 53));
    assert_eq!(Date::new(1, 1, 1).unwrap().0, 1);
    assert!(Date::new(2026, 2, 29).is_none());
    assert_eq!(ymd("20240229").unwrap().iso(), "2024-02-29");
    assert_eq!(ymd("2024x229").unwrap_err().kind, Kind::Value);
    assert_eq!(statics::month_day("1229").unwrap().iso(), "2001-12-29");
    assert_eq!(statics::month_day("20000229").unwrap().iso(), "2001-02-28");
    assert_eq!(statics::number("10"), Some(Py::Int("10".into())));
    assert_eq!(statics::number("-1.5e2"), Some(Py::Float(-150.0)));
    assert_eq!(statics::number(".5"), Some(Py::Float(0.5)));
    assert_eq!(statics::number("5."), Some(Py::Float(5.0)));
    assert_eq!(statics::number("1.2.3"), None);
    assert_eq!(statics::number("about"), None);
}
