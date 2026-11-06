//! The static files' checks (docs/inputs.md §6.7.2), on zips made here.

use super::*;

/// A zip of `files` (stored, as a tool may write them; one entry deflated, as most do).
fn zip(files: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut dir = Vec::new();
    for (i, (name, text)) in files.iter().enumerate() {
        let raw = text.as_bytes();
        let (method, data): (u16, Vec<u8>) = if i == 0 {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut e, raw).unwrap();
            (8, e.finish().unwrap())
        } else {
            (0, raw.to_vec())
        };
        let crc = crc32fast::hash(raw);
        let at = out.len() as u32;
        let head = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(method.to_le_bytes());
            h.extend([0u8; 4]);
            h.extend(crc.to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((raw.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                h.extend([0u8; 6]);
                h.extend(0u32.to_le_bytes());
                h.extend(at.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(head(0x0403_4b50, false));
        out.extend(&data);
        dir.extend(head(0x0201_4b50, true));
    }
    let start = out.len() as u32;
    out.extend(&dir);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((dir.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

const AGENCY: &str = "agency_id,agency_name,agency_url,agency_timezone\na,Ferries,,\n";
const FEED: &str = "feed_publisher_name,feed_publisher_url,feed_lang\nlooked up by hand,,en\n";
const CAL_HEAD: &str = "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n";

/// A timed rail route (a trip on a frequency, May to October) and a counted one, and a counted
/// ferry pinned to OSM.
fn good() -> Vec<(&'static str, String)> {
    vec![
        ("agency.txt", AGENCY.into()),
        ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\na,A,46.5,7.5\nb,B,46.6,7.6\n".into()),
        ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type,route_url,checked,months,osm\nr1,a,,Valley line,2,https://example.org/r1,2026-09,May–Oct,\nr2,a,,\"Hill line, upper\",2,https://example.org/r2,2026-09,,\nf1,a,,Lake ferry,4,https://example.org/f1,2026-09,,w123\n".into()),
        ("trips.txt", "route_id,service_id,trip_id\nr1,summer,t1\n".into()),
        ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt1,06:00:00,06:00:00,a,1\nt1,06:20:00,06:20:00,b,2\n".into()),
        ("frequencies.txt", "trip_id,start_time,end_time,headway_secs\nt1,06:00:00,23:00:00,600\n".into()),
        ("calendar.txt", format!("{CAL_HEAD}summer,1,1,1,1,1,1,1,20000501,20001031\n")),
        ("feed_info.txt", FEED.into()),
        ("counts.txt", "route_id,from_stop_id,to_stop_id,at_least,per_day,per_week,per_day_low,headway,days,months,season,overnight,note\nr2,a,b,1,40,,,,,,,,\"at least, by the headway\"\nf1,,,,null,,,30 minutes,daily,,year,0,\n".into()),
    ]
}

fn with(files: &[(&'static str, String)]) -> Vec<u8> {
    let v: Vec<(&str, &str)> = files.iter().map(|(n, t)| (*n, t.as_str())).collect();
    zip(&v)
}

fn set(files: &mut Vec<(&'static str, String)>, name: &'static str, text: &str) {
    files.retain(|(n, _)| *n != name);
    files.push((name, text.to_string()));
}

fn checks_of(c: &FileCheck) -> Vec<(String, Level)> {
    c.findings.iter().map(|f| (f.check().to_string(), f.level)).collect()
}

#[test]
fn a_good_file_has_no_finding_and_says_what_it_holds() {
    let c = Gtfs.file("lake.zip", &with(&good()));
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    assert_eq!(c.facts["routes"], 3);
    assert_eq!(c.facts["trips"], 1);
    assert_eq!(c.facts["count_rows"], 2);
    assert_eq!(c.keyed, None);
}

#[test]
fn the_tables_read_as_written() {
    let files = unzip(&with(&good())).unwrap();
    let (head, rows) = csv(files["routes.txt"].as_slice()).unwrap();
    assert_eq!(head[4], "route_type");
    assert_eq!(rows[1][3], "Hill line, upper");
    let (_, rows) = csv(b"\xef\xbb\xbfa,b\r\n\"x \"\"y\"\"\",\r\n").unwrap();
    assert_eq!(rows, vec![vec!["x \"y\"".to_string(), String::new()]]);
    assert!(csv(b"a\n\"open\n").is_err());
}

#[test]
fn errors_name_what_is_wrong() {
    let cases: Vec<(&'static str, &str, &str)> = vec![
        ("feed_info.txt", "", "tg-table"),
        ("trips.txt", "route_id,service_id,trip_id\nnone,summer,t1\n", "tg-ref"),
        ("trips.txt", "route_id,service_id,trip_id\nr1,winter,t1\n", "tg-ref"),
        ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt1,6h,06:00:00,a,1\nt1,06:20:00,06:20:00,b,2\n", "tg-value"),
        ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt1,06:00:00,06:00:00,z,1\n", "tg-ref"),
        ("calendar.txt", "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nsummer,1,1,1,1,1,1,1,20000501,20001331\n", "tg-value"),
        ("frequencies.txt", "trip_id,start_time,end_time,headway_secs\nt1,06:00:00,23:00:00,0\n", "tg-value"),
        ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\na,A,95,7.5\nb,B,46.6,7.6\n", "tg-value"),
        ("counts.txt", "route_id,from_stop_id,to_stop_id,per_day\nr2,a,,40\n", "tg-ref"),
        ("counts.txt", "route_id,per_day\nf1,about ten\n", "tg-value"),
        ("counts.txt", "route_id,per_day\nr1,40\n", "tg-ref"),
    ];
    for (name, text, want) in cases {
        let mut f = good();
        if text.is_empty() {
            f.retain(|(n, _)| *n != name);
        } else {
            set(&mut f, name, text);
        }
        let c = Gtfs.file("lake.zip", &with(&f));
        assert!(checks_of(&c).iter().any(|(k, l)| k == want && *l == Level::Error), "{name}: {:?}", c.findings);
    }
    // A route without its page, of a type GTFS hasn't, a ferry counted without its pin, a route both
    // timed and counted.
    let mut f = good();
    set(&mut f, "routes.txt", "route_id,agency_id,route_long_name,route_type,route_url,osm\nr1,a,Valley,2,https://e.org,\nr2,a,Hill,2,,\nf1,a,Lake,4,https://e.org,x9\n");
    let c = Gtfs.file("lake.zip", &with(&f));
    let k = checks_of(&c);
    assert!(k.contains(&("tg-route".into(), Level::Error)) && k.contains(&("tg-value".into(), Level::Error)), "{k:?}");
    set(&mut f, "routes.txt", "route_id,agency_id,route_long_name,route_type,route_url,osm\nr1,a,Valley,13,https://e.org,\nr2,a,Hill,2,https://e.org,\nf1,a,Lake,4,https://e.org,\n");
    set(&mut f, "counts.txt", "route_id,from_stop_id,to_stop_id,per_day\nr2,a,b,40\nf1,,,3\nr1,a,b,2\n");
    let c = Gtfs.file("lake.zip", &with(&f));
    let msgs: Vec<&str> = c.findings.iter().map(|f| f.message.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("route_type \"13\"")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("no osm pin")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("both trips and count rows")), "{msgs:?}");
    // Not a zip, not the inputs folder's shape.
    assert_eq!(checks_of(&Gtfs.file("lake.zip", b"PK nothing")), vec![("tg-zip".to_string(), Level::Error)]);
    assert_eq!(checks_of(&Gtfs.file("lake.csv", b"")), vec![("tg-shape".to_string(), Level::Error)]);
    assert_eq!(
        checks_of(&Gtfs.file("lake.toml", b"name = \"x\"\n")).iter().filter(|(_, l)| *l == Level::Error).count(),
        2
    );
}

#[test]
fn calendar_dates_are_months_and_days() {
    for (s, ok) in [
        ("20000229", true),
        ("19990229", true),
        ("00000101", true),
        ("20001231", true),
        ("20000230", false),
        ("2000011", false),
        ("20001301", false),
        ("2000010a", false),
    ] {
        assert_eq!(valid_month_day(s), ok, "{s}");
    }
    for (s, ok) in
        [("06:00:00", true), ("25:10:00", true), ("6:00:00", true), ("06:60:00", false), ("06:00", false), ("", false)]
    {
        assert_eq!(valid_time(s), ok, "{s}");
    }
}

#[test]
fn warnings_a_route_with_nothing_and_one_route_in_two_files() {
    let mut f = good();
    set(&mut f, "routes.txt", "route_id,agency_id,route_long_name,route_type,route_url,osm\nr1,a,Valley,2,https://e.org,\nr2,a,Hill,2,https://e.org,\nf1,a,Lake,4,https://e.org,w1\nidle,a,Idle,2,https://e.org,\n");
    let c = Gtfs.file("lake.zip", &with(&f));
    assert_eq!(checks_of(&c), vec![("tg-empty".to_string(), Level::Warning)]);
    let a = with(&good());
    let read = |_: &str| Ok(std::sync::Arc::new(a.clone()));
    let v = Version { paths: ["a.zip".to_string(), "b.zip".to_string(), "b.toml".to_string()].into(), read: &read };
    let w = Gtfs.whole(&v).unwrap();
    assert_eq!(w.len(), 3, "{w:?}");
    assert!(w
        .iter()
        .all(|f| f.check() == "tg-same-route" && f.level == Level::Warning && f.files == ["a.zip", "b.zip"]));
    // A zip and its description are held together.
    assert_eq!(Gtfs.partners("b.zip", &v.paths), vec!["b.toml".to_string()]);
    assert_eq!(Gtfs.partners("a.zip", &v.paths), Vec::<String>::new());
}
