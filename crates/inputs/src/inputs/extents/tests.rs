use super::*;

fn ids(fs: &[Finding]) -> Vec<(&str, Level)> {
    fs.iter().map(|f| (f.check(), f.level)).collect()
}

fn checks_of(path: &str, bytes: &[u8]) -> Vec<(String, Level)> {
    Extents.file(path, bytes).findings.iter().map(|f| (f.check().to_string(), f.level)).collect()
}

fn one(check: &str, level: Level) -> Vec<(String, Level)> {
    vec![(check.to_string(), level)]
}

const SCORE_TOML: &str = r#"theme = "terrain"
primary = "spacing_m"
within = 1.25
share = 0.02

[metric.spacing_m]
label = "Grid spacing"
unit = "m"
scale = "log"
worst = 1000
best = 0.1

[metric.surface]
label = "Bare earth or surface"
weight = 0.4
values = { ground = 1.0, surface = 0.0 }

[metric.year]
label = "Year surveyed"
weight = 0.2
scale = "linear"
worst = 2000
best = 2026
"#;

const DECL: &str = r#"name = "A lidar programme"
credit = "A ministry"
licence = "CC-BY-4.0"
theme = "terrain"
availability = "open"

[[piece]]
id = "dem"
method = "arcgis"
class = "index"
url = "https://example.org/FeatureServer/1"
by = ["year"]
[piece.metrics]
surface = "ground"
spacing_m = { field = "spacing", text = "metre" }
year = { field = "year", date = "year" }

[[piece]]
id = "rest"
method = "none"
why = "not published"
"#;

/// A square from (x, y), side `d`, counter-clockwise (`ccw`) or not.
fn square(x: f64, y: f64, d: f64, ccw: bool) -> String {
    let mut r = vec![[x, y], [x + d, y], [x + d, y + d], [x, y + d], [x, y]];
    if !ccw {
        r.reverse();
    }
    serde_json::to_string(&r).unwrap()
}

fn feature(props: &str, geometry: &str) -> String {
    format!(r#"{{"type":"Feature","properties":{{{props}}},"geometry":{geometry}}}"#)
}

fn mp(rings: &[String]) -> String {
    format!(r#"{{"type":"MultiPolygon","coordinates":[[{}]]}}"#, rings.join(","))
}

fn piece(id: &str, extra: &str, geometry: &str) -> String {
    feature(&format!(r#""piece":"{id}","extent":"index","availability":"open"{extra}"#), geometry)
}

fn fc(source: &str, read: &[&str], feats: &[String]) -> Vec<u8> {
    let read: Vec<String> = read.iter().map(|b| format!(r#"{{"piece":"{b}","method":"arcgis"}}"#)).collect();
    format!(
        r#"{{"type":"FeatureCollection","source":"{source}","theme":"terrain","read":[{}],"features":[{}]}}"#,
        read.join(","),
        feats.join(",")
    )
    .into_bytes()
}

/// The good pieces of `DECL`.
fn good() -> Vec<u8> {
    fc(
        "lidar",
        &["dem", "rest"],
        &[
            piece(
                "dem/2020",
                r#","name":"2020","surface":"ground","spacing_m":1.0,"year":2020"#,
                &mp(&[square(0.0, 0.0, 1.0, true)]),
            ),
            feature(r#""piece":"rest","extent":"unknown","availability":"open""#, "null"),
        ],
    )
}

fn version_of<'a>(files: &'a BTreeMap<String, Vec<u8>>, read: &'a dyn Fn(&str) -> Result<Arc<Vec<u8>>>) -> Version<'a> {
    Version { paths: files.keys().cloned().collect(), read }
}

fn whole_of(files: &[(&str, &[u8])]) -> Vec<Finding> {
    let files: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, b)| (p.to_string(), b.to_vec())).collect();
    let read = |p: &str| -> Result<Arc<Vec<u8>>> { Ok(Arc::new(files[p].clone())) };
    Extents.whole(&version_of(&files, &read)).unwrap()
}

#[test]
fn on_the_gate() {
    assert!(crate::inputs::UNITS.contains(&UNIT));
    let c = crate::inputs::checks(UNIT).unwrap();
    assert_eq!(c.unit(), UNIT);
    assert!(c.recursive());
}

#[test]
fn paths_and_shape() {
    assert_eq!(kind("terrain/score.toml"), Kind::Score("terrain"));
    assert_eq!(kind("terrain/cop-dem.toml"), Kind::Declaration("terrain", "cop-dem"));
    assert_eq!(kind("terrain/cop-dem.geojson"), Kind::Pieces("terrain", "cop-dem"));
    assert_eq!(kind("cop-dem.toml"), Kind::Other);
    assert_eq!(kind("terrain/sub/cop-dem.toml"), Kind::Other);
    assert_eq!(kind("terrain/Cop.toml"), Kind::Other);
    assert_eq!(kind("terrain/notes.txt"), Kind::Other);
    for p in ["cop-dem.toml", "terrain/sub/x.toml", "terrain/notes.txt", "terrain/A.geojson"] {
        assert_eq!(checks_of(p, b""), one(SHAPE, Level::Error), "{p}");
    }
    // A theme the app doesn't know.
    assert_eq!(checks_of("water/x.toml", DECL.as_bytes()), one(THEME, Level::Error));
}

#[test]
fn a_score_and_its_guarantee() {
    let c = Extents.file("terrain/score.toml", SCORE_TOML.as_bytes());
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    assert_eq!(c.facts["primary"], "spacing_m");
    assert_eq!(c.facts["metrics"], serde_json::json!(["spacing_m", "surface", "year"]));
    // The terrain's bound, as docs/inputs.md §6.11 states it.
    assert!((Score::bound(1.25, 1000.0, 0.1) - 0.0237).abs() < 5e-5);
    let s = score(SCORE_TOML.as_bytes()).unwrap();
    assert_eq!(s.metrics["surface"].scale.refuses(&serde_json::json!("ground")), None);
    assert!(s.metrics["surface"].scale.refuses(&serde_json::json!("rock")).is_some());
    assert!(s.metrics["spacing_m"].scale.refuses(&serde_json::json!(0)).is_some());
    assert!(s.metrics["year"].scale.refuses(&serde_json::json!("2020")).is_some());
    assert_eq!(s.metrics["year"].scale.refuses(&serde_json::json!(-3)), None);
    for (from, to) in [
        ("primary = \"spacing_m\"", ""),
        ("primary = \"spacing_m\"", "primary = \"depth\""),
        ("share = 0.02", "share = 0.03"),
        ("share = 0.02", ""),
        ("within = 1.25", "within = 1"),
        ("worst = 2000", "worst = 2026"),
        ("weight = 0.2", "weight = -0.2"),
        ("weight = 0.2\n", ""),
        ("unit = \"m\"\n", "unit = \"m\"\nweight = 1\n"),
        ("worst = 1000", "worst = 0"),
        ("ground = 1.0", "ground = 2.0"),
        ("scale = \"linear\"", "scale = \"cubic\""),
        ("theme = \"terrain\"", "theme = \"terrain\"\nweights = 1"),
        ("label = \"Year surveyed\"\n", ""),
        ("values = {", "scale = \"log\"\nvalues = {"),
        ("values = {", "scale = \"values\"\nvalues = {"),
    ] {
        let bad = SCORE_TOML.replacen(from, to, 1);
        assert_ne!(bad, SCORE_TOML);
        assert_eq!(checks_of("terrain/score.toml", bad.as_bytes()), one(SCORE_CHECK, Level::Error), "{from} → {to}");
    }
    // The primary on a scale that isn't a factor's.
    let linear =
        SCORE_TOML.replacen("primary = \"spacing_m\"", "primary = \"year\"", 1).replacen("weight = 0.2\n", "", 1);
    assert_eq!(checks_of("terrain/score.toml", linear.as_bytes()), one(SCORE_CHECK, Level::Error));
    assert_eq!(checks_of("terrain/score.toml", b"x = ["), one(SCORE_CHECK, Level::Error));
    let other = SCORE_TOML.replacen("theme = \"terrain\"", "theme = \"buildings\"", 1);
    assert_eq!(checks_of("terrain/score.toml", other.as_bytes()), one(THEME, Level::Error));
    // (A score with no colours declared, as above, is taken: the server draws it on its own ramp.)
    assert!(s.ramp.is_empty() && s.label.is_none() && s.shade == 0.0);
    assert!(s.metrics.values().all(|m| m.classes.is_empty() && m.colours.is_empty()));
}

/// The layers' colours in the real score.toml: its classes, ramp, shade and words' colours, and
/// each field's mistakes refused.
#[test]
fn a_scores_colours() {
    let real = std::str::from_utf8(REAL[0].1).unwrap();
    let s = score(real.as_bytes()).unwrap();
    let m = &s.metrics["spacing_m"];
    let labels: Vec<&str> = m.classes.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["0.5 m", "1 m", "2 m", "3–5 m", "10 m", "30 m", "90 m", "coarser"]);
    assert_eq!(s.ramp.len(), m.classes.len(), "a colour a class");
    assert_eq!(s.label.as_deref(), Some("Best data"));
    assert_eq!(colour_of("#56b4e9"), Some([0x56, 0xb4, 0xe9]));
    assert_eq!(s.metrics["surface"].colours["ground"], [0x56, 0xb4, 0xe9]);
    // Every spacing the real pieces state (and the units' near twins) in its class; the edges in
    // their class, the finest best.
    let class = |v: f64| m.classes[m.class_of(v).unwrap()].label.as_str();
    for (v, want) in [
        (0.5, "0.5 m"),
        (0.6096, "0.5 m"),
        (0.625, "0.5 m"),
        (0.9144, "1 m"),
        (1.0, "1 m"),
        (2.0, "2 m"),
        (3.0, "3–5 m"),
        (5.0, "3–5 m"),
        (10.3, "10 m"),
        (30.0, "30 m"),
        (90.0, "90 m"),
        (1000.0, "coarser"),
    ] {
        assert_eq!(class(v), want, "{v}");
    }
    assert_eq!(m.class_at(0), 1.0);
    assert_eq!(m.class_at(7), 0.0);
    assert_eq!(s.metrics["year"].class_of(2020.0), None);
    for (from, to) in [
        ("{ upto = 1.25, label = \"1 m\" }", "{ upto = 0.5, label = \"1 m\" }"),
        ("{ upto = 1.25, label = \"1 m\" }", "{ upto = 0.625, label = \"1 m\" }"),
        ("{ upto = 0.625, label = \"0.5 m\" }", "{ upto = -1, label = \"0.5 m\" }"),
        ("{ upto = 0.625, label = \"0.5 m\" }", "{ upto = 0, label = \"0.5 m\" }"),
        ("{ upto = 1.25, label = \"1 m\" }", "{ upto = \"x\", label = \"1 m\" }"),
        ("{ upto = 1.25, label = \"1 m\" }", "{ label = \"1 m\" }"),
        ("{ upto = 1.25, label = \"1 m\" }", "{ upto = 1.25 }"),
        ("{ upto = 1.25, label = \"1 m\" }", "{ upto = 1.25, label = \" \" }"),
        ("{ upto = 1.25, label = \"1 m\" }", "{ upto = 1.25, label = \"1 m\", colour = \"#ffffff\" }"),
        ("{ upto = 1.25, label = \"1 m\" }", "3"),
        ("{ label = \"coarser\" }", "{ upto = 500, label = \"coarser\" }"),
        ("classes = [", "classes = [{ label = \"x\" }]\nx = ["),
        ("\"#803041\",", "\"#80304\","),
        ("\"#803041\",", "\"803041\","),
        ("\"#803041\",", "\"#80304g\","),
        ("\"#803041\",", "3,"),
        ("ramp = [", "ramp = [\"#ffffff\"]\nx = ["),
        ("shade = 0.4", "shade = 1.5"),
        ("shade = 0.4", "shade = 1"),
        ("shade = 0.4", "shade = -0.1"),
        ("shade = 0.4", "shade = \"some\""),
        ("label = \"Best data\"", "label = \"\""),
        ("label = \"Best data\"", "label = 3"),
        ("surface = \"#e69f00\" }", "surface = \"#e69f00\", rock = \"#000000\" }"),
        ("ground = \"#56b4e9\", ", ""),
        ("ground = \"#56b4e9\"", "ground = \"blue\""),
        ("colours = {", "colours = 3\nx = {"),
        ("best = 2026\n", "best = 2026\ncolours = { a = \"#ffffff\" }\n"),
        ("colours = { ground", "classes = [{ upto = 1, label = \"a\" }, { label = \"b\" }]\nx = { ground"),
    ] {
        let bad = real.replacen(from, to, 1);
        assert_ne!(bad, real, "{from}");
        assert_eq!(checks_of("terrain/score.toml", bad.as_bytes()), one(SCORE_CHECK, Level::Error), "{from} → {to}");
    }
    // A shade with no classes to shade.
    let flat = SCORE_TOML.replacen("share = 0.02\n", "share = 0.02\nshade = 0.4\n", 1);
    assert_eq!(checks_of("terrain/score.toml", flat.as_bytes()), one(SCORE_CHECK, Level::Error));
    assert!(score(SCORE_TOML.replacen("share = 0.02\n", "share = 0.02\nshade = 0\n", 1).as_bytes()).is_ok());
}

#[test]
fn a_declaration() {
    let c = Extents.file("terrain/lidar.toml", DECL.as_bytes());
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    assert_eq!(c.facts["theme"], "terrain");
    assert_eq!(c.facts["availability"], "open");
    assert_eq!(c.facts["blocks"], serde_json::json!(["dem", "rest"]));
    for (from, to) in [
        ("method = \"arcgis\"", "method = \"ftp\""),
        ("class = \"index\"\n", ""),
        ("class = \"index\"", "class = \"files\""),
        ("method = \"none\"", "method = \"stac\"\nclass = \"service\""),
        ("method = \"none\"", "method = \"none\"\nclass = \"claimed\""),
        ("availability = \"open\"", "availability = \"free\""),
        ("availability = \"open\"\n", ""),
        ("id = \"rest\"", "id = \"dem\""),
        ("id = \"rest\"\n", ""),
        ("id = \"rest\"", "id = \"Rest\""),
        ("why = \"not published\"\n", ""),
        ("by = [\"year\"]", "by = [\"year\"]\nname = \"title\""),
        ("by = [\"year\"]", "by = \"year\""),
        ("date = \"year\"", "date = \"julian\""),
        ("text = \"metre\"", "text = \"metre\", unit = \"m\""),
        ("{ field = \"year\", date = \"year\" }", "{ date = \"year\" }"),
        ("{ field = \"spacing\",", "{ field = \"spacing\", index = -1,"),
        ("url = \"https", "tolerance_m = 0\nurl = \"https"),
        ("url = \"https", "filter = { year = 2020 }\nurl = \"https"),
    ] {
        let bad = DECL.replacen(from, to, 1);
        assert_ne!(bad, DECL);
        assert_eq!(checks_of("terrain/lidar.toml", bad.as_bytes()), one(DECL_CHECK, Level::Error), "{from} → {to}");
    }
    // No block at all.
    let bare = DECL.split("[[piece]]").next().unwrap();
    assert_eq!(checks_of("terrain/lidar.toml", bare.as_bytes()), one(DECL_CHECK, Level::Error));
    // The block's own availability is enough, and a claimed listing is a claim.
    let own = DECL
        .replacen("availability = \"open\"\n", "", 1)
        .replace("method = \"arcgis\"", "method = \"arcgis\"\navailability = \"paid\"")
        .replace("why =", "availability = \"open\"\nwhy =");
    assert!(checks_of("terrain/lidar.toml", own.as_bytes()).is_empty());
    let claimed =
        DECL.replacen("method = \"arcgis\"\nclass = \"index\"", "method = \"listing\"\nclass = \"claimed\"", 1);
    assert!(checks_of("terrain/lidar.toml", claimed.as_bytes()).is_empty());
    // The theme: missing, unknown, or not its folder's.
    assert_eq!(
        checks_of("terrain/lidar.toml", DECL.replacen("theme = \"terrain\"\n", "", 1).as_bytes()),
        one(THEME, Level::Error)
    );
    assert_eq!(
        checks_of("terrain/lidar.toml", DECL.replacen("\"terrain\"", "\"lava\"", 1).as_bytes()),
        one(THEME, Level::Error)
    );
    // The description: its credit.
    assert_eq!(
        checks_of("terrain/lidar.toml", DECL.replacen("credit = \"A ministry\"\n", "", 1).as_bytes()),
        one(DESC, Level::Error)
    );
    assert_eq!(
        checks_of("terrain/lidar.toml", b"x = ["),
        vec![(DESC.to_string(), Level::Error), (DECL_CHECK.to_string(), Level::Error)]
    );
}

/// The declarations' own check (`DECL` here is the fixture).
const DECL_CHECK: &str = super::DECL;

#[test]
fn pieces_and_their_facts() {
    let c = Extents.file("terrain/lidar.geojson", &good());
    assert_eq!(ids(&c.findings), [(UNKNOWN, Level::Warning)]);
    assert_eq!(c.findings[0].lines, [(2, "rest: extent unknown".to_string())]);
    assert_eq!(c.facts["pieces"], 2);
    assert_eq!(c.facts["classes"], serde_json::json!({"index": 1, "unknown": 1}));
    assert_eq!(c.facts["availability"], serde_json::json!({"open": 2}));
    assert_eq!(c.facts["box"], serde_json::json!([0.0, 0.0, 1.0, 1.0]));
    let g = mp(&[square(0.0, 0.0, 1.0, true)]);
    // Not a collection of pieces, or not its declaration's.
    for bad in [
        b"{".to_vec(),
        br#"{"type":"Feature"}"#.to_vec(),
        fc("other", &["dem"], &[piece("dem/a", "", &g)]),
        String::from_utf8(fc("lidar", &["dem"], &[piece("dem/a", "", &g)]))
            .unwrap()
            .replace(r#""read":[{"piece":"dem","method":"arcgis"}],"#, "")
            .into_bytes(),
        String::from_utf8(fc("lidar", &["dem"], &[piece("dem/a", "", &g)]))
            .unwrap()
            .replace(r#""method":"arcgis""#, r#""m":1"#)
            .into_bytes(),
    ] {
        assert_eq!(
            checks_of("terrain/lidar.geojson", &bad),
            one(PIECES, Level::Error),
            "{}",
            String::from_utf8_lossy(&bad)
        );
    }
    let other_theme = String::from_utf8(good()).unwrap().replace(r#""theme":"terrain""#, r#""theme":"buildings""#);
    assert!(checks_of("terrain/lidar.geojson", other_theme.as_bytes()).contains(&(THEME.to_string(), Level::Error)));
    // Pieces not in the shape: one finding, each listed.
    let feats = [
        feature(r#""extent":"index","availability":"open""#, &g),
        feature(r#""piece":"dem/b","extent":"guessed","availability":"open""#, &g),
        feature(r#""piece":"dem/c","extent":"index","availability":"free""#, &g),
        feature(r#""piece":"dem/d","extent":"index","availability":"open""#, "null"),
        feature(r#""piece":"dem/e","extent":"unknown","availability":"open""#, &g),
        feature(r#""piece":"dem/f","extent":"index","availability":"open","name":3"#, &g),
        feature(r#""piece":"dem/g","extent":"index","availability":"open","stated":"QL 2""#, &g),
        format!(r#"{{"type":"Thing","properties":{{"piece":"dem/h"}}}}"#),
        feature(r#""piece":"dem/i","extent":"index","availability":"open""#, r#"{"type":"Polygon","coordinates":[]}"#),
    ];
    let c = Extents.file("terrain/lidar.geojson", &fc("lidar", &["dem"], &feats));
    let mut want: Vec<(&str, Level)> = vec![(PIECE, Level::Error), (GEOM, Level::Error)];
    want.sort();
    let mut got = ids(&c.findings);
    got.sort();
    assert_eq!(got, want);
    let shape = c.findings.iter().find(|f| f.check() == PIECE).unwrap();
    assert_eq!(shape.lines.iter().map(|l| l.0).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6, 7, 8]);
    // A piece's id twice.
    let c = Extents
        .file("terrain/lidar.geojson", &fc("lidar", &["dem"], &[piece("dem/a", "", &g), piece("dem/a", "", &g)]));
    assert_eq!(ids(&c.findings), [(TWICE, Level::Error)]);
    assert_eq!(c.findings[0].lines[0].0, 2);
}

#[test]
fn geometry() {
    let at = |rings: &[String]| {
        checks_of("terrain/lidar.geojson", &fc("lidar", &["dem"], &[piece("dem/a", "", &mp(rings))]))
    };
    let ok = Vec::<(String, Level)>::new();
    // A polygon with a hole wound clockwise.
    assert_eq!(at(&[square(0.0, 0.0, 10.0, true), square(2.0, 2.0, 2.0, false)]), ok);
    // Cut at the antimeridian: one side up to 180°, the other from −180°.
    assert_eq!(at(&[square(170.0, 0.0, 10.0, true)]), ok);
    assert_eq!(at(&[square(-180.0, 0.0, 10.0, true)]), ok);
    // Closed along 90° round the pole (the step from 180° back to −180° along the pole).
    let cap = "[[-180,80],[180,80],[180,90],[-180,90],[-180,80]]".to_string();
    assert_eq!(at(&[cap]), ok);
    // A band round the turn, its edges from −180° to 180°.
    assert_eq!(at(&["[[-180,10],[180,10],[180,20],[-180,20],[-180,10]]".to_string()]), ok);
    // The same with a repeated position (allowed: it's no crossing).
    assert_eq!(at(&["[[0,0],[1,0],[1,0],[1,1],[0,1],[0,0]]".to_string()]), ok);
    let geom = one(GEOM, Level::Error);
    for bad in [
        "[[0,0],[1,0],[1,1],[0,1]]",                   // not closed
        "[[0,0],[1,0],[0,0]]",                         // fewer than four
        "[[0,0],[1,0],[2,0],[0,0]]",                   // no area
        "[[0,0],[2,2],[2,0],[0,2],[0,0]]",             // a bow tie
        "[[0,0],[0,1],[1,1],[1,0],[0,0]]",             // clockwise exterior
        "[[0,0],[181,0],[181,1],[0,1],[0,0]]",         // out of range
        "[[0,95],[1,95],[1,96],[0,96],[0,95]]",        // a latitude out of range
        "[[170,0],[-170,0],[-170,1],[170,1],[170,0]]", // across the antimeridian, uncut
        "[[180,0],[-170,0],[-170,1],[180,1],[180,0]]", // the same from 180°
        "[[-180,0],[180,10],[180,20],[-180,0]]",       // from −180° to 180° off a parallel
        "[[0,0],[1,0],[\"1\",1],[0,1],[0,0]]",         // a position not two numbers
    ] {
        assert_eq!(at(&[bad.to_string()]), geom, "{bad}");
    }
    // A hole wound counter-clockwise.
    assert_eq!(at(&[square(0.0, 0.0, 10.0, true), square(2.0, 2.0, 2.0, true)]), geom);
    // An empty MultiPolygon.
    let empty = checks_of(
        "terrain/lidar.geojson",
        &fc("lidar", &["dem"], &[piece("dem/a", "", r#"{"type":"MultiPolygon","coordinates":[]}"#)]),
    );
    assert_eq!(empty, geom);
}

#[test]
fn the_version_whole() {
    let s = SCORE_TOML.as_bytes();
    let (d, p) = (DECL.as_bytes(), good());
    assert_eq!(
        ids(&whole_of(&[("terrain/score.toml", s), ("terrain/lidar.toml", d), ("terrain/lidar.geojson", &p)])),
        []
    );
    // Pieces without their declaration; a declaration with none yet.
    let f = whole_of(&[("terrain/score.toml", s), ("terrain/lidar.geojson", &p)]);
    assert_eq!(ids(&f), [(PAIR, Level::Error)]);
    assert_eq!(f[0].files, ["terrain/lidar.geojson"]);
    let f = whole_of(&[("terrain/score.toml", s), ("terrain/lidar.toml", d)]);
    assert_eq!(ids(&f), [(NOT_YET, Level::Warning)]);
    // A theme without its score.
    let f = whole_of(&[("terrain/lidar.toml", d), ("terrain/lidar.geojson", &p)]);
    assert_eq!(ids(&f), [(NO_SCORE, Level::Error)]);
    assert_eq!(f[0].files, ["terrain/lidar.geojson", "terrain/lidar.toml"]);
    // A declaration's metric the score doesn't name, or a constant not of its kind: the score held
    // too (and the pieces, made before the constant changed, no longer as its block makes them).
    for (from, to) in [
        ("surface = \"ground\"", "depth = 3"),
        ("surface = \"ground\"", "surface = \"rock\""),
        ("spacing_m = { field = \"spacing\", text = \"metre\" }", "spacing_m = -1"),
    ] {
        let bad = DECL.replacen(from, to, 1);
        let f = whole_of(&[
            ("terrain/score.toml", s),
            ("terrain/lidar.toml", bad.as_bytes()),
            ("terrain/lidar.geojson", &p),
        ]);
        assert_eq!(ids(&f), [(METRIC, Level::Error), (MADE, Level::Error)], "{to}");
        assert_eq!(f[0].files, ["terrain/lidar.toml", "terrain/score.toml"]);
    }
    let mapped = DECL.replacen(
        "surface = \"ground\"",
        "surface = { field = \"kind\", map = { DTM = \"ground\", DSM = \"canopy\" } }",
        1,
    );
    let f = whole_of(&[
        ("terrain/score.toml", s),
        ("terrain/lidar.toml", mapped.as_bytes()),
        ("terrain/lidar.geojson", &p),
    ]);
    assert_eq!(ids(&f), [(METRIC, Level::Error)]);
    // A piece's metric the score doesn't name, of the wrong kind, or stated of no metric (of a
    // declaration with no constant, so the pieces are made from it).
    let g = mp(&[square(0.0, 0.0, 1.0, true)]);
    let free = DECL.replacen("surface = \"ground\"\n", "", 1);
    let dem = |extra: &str| {
        let surface = if extra.contains("\"surface\"") { "" } else { r#","surface":"ground""# };
        piece("dem/a", &format!("{surface}{extra}"), &g)
    };
    for extra in [
        r#","depth":3"#,
        r#","surface":"rock""#,
        r#","spacing_m":0"#,
        r#","year":"2020""#,
        r#","stated":{"depth":"QL 2"}"#,
    ] {
        let bad = fc("lidar", &["dem", "rest"], &[piece("dem/a", extra, &g)]);
        let f = whole_of(&[
            ("terrain/score.toml", s),
            ("terrain/lidar.toml", free.as_bytes()),
            ("terrain/lidar.geojson", &bad),
        ]);
        assert_eq!(ids(&f), [(METRIC, Level::Error)], "{extra}");
        assert_eq!(f[0].files, ["terrain/lidar.geojson", "terrain/score.toml"]);
        assert_eq!(f[0].lines[0].0, 1);
    }
    // A stated class beside its metric is fine, the value unknown.
    let stated = fc("lidar", &["dem", "rest"], &[dem(r#","stated":{"spacing_m":"2 feet"}"#)]);
    assert_eq!(
        ids(&whole_of(&[("terrain/score.toml", s), ("terrain/lidar.toml", d), ("terrain/lidar.geojson", &stated)])),
        []
    );
    // Not made from this declaration as it is: other blocks read, a piece of no block, or not of
    // its block's class, availability or constants.
    let ground = r#","surface":"ground""#;
    for bad in [
        fc("lidar", &["dem"], &[dem("")]),
        fc("lidar", &["dem", "rest", "old"], &[dem("")]),
        fc("lidar", &["dem", "rest"], &[piece("old/a", ground, &g)]),
        fc("lidar", &["dem", "rest"], &[piece("dem/a", "", &g)]),
        fc("lidar", &["dem", "rest"], &[dem(r#","surface":"surface""#)]),
        fc(
            "lidar",
            &["dem", "rest"],
            &[feature(r#""piece":"dem/a","extent":"service","availability":"open","surface":"ground""#, &g)],
        ),
        fc(
            "lidar",
            &["dem", "rest"],
            &[feature(r#""piece":"dem/a","extent":"index","availability":"paid","surface":"ground""#, &g)],
        ),
        fc(
            "lidar",
            &["dem", "rest"],
            &[feature(r#""piece":"rest","extent":"unknown","availability":"login""#, "null")],
        ),
    ] {
        let f = whole_of(&[("terrain/score.toml", s), ("terrain/lidar.toml", d), ("terrain/lidar.geojson", &bad)]);
        assert_eq!(ids(&f), [(MADE, Level::Error)], "{}", String::from_utf8_lossy(&bad));
        assert_eq!(f[0].files, ["terrain/lidar.geojson", "terrain/lidar.toml"]);
    }
    // A block that isn't open is claimed, whatever its method: its pieces say so.
    let paid = DECL.replacen("availability = \"open\"", "availability = \"paid\"", 1);
    let claimed = fc(
        "lidar",
        &["dem", "rest"],
        &[
            feature(r#""piece":"dem/a","extent":"claimed","availability":"paid","surface":"ground""#, &g),
            feature(r#""piece":"rest","extent":"unknown","availability":"paid""#, "null"),
        ],
    );
    assert_eq!(
        ids(&whole_of(&[
            ("terrain/score.toml", s),
            ("terrain/lidar.toml", paid.as_bytes()),
            ("terrain/lidar.geojson", &claimed)
        ])),
        []
    );
    // A theme the app doesn't know is the file check's alone.
    assert_eq!(ids(&whole_of(&[("lava/lidar.toml", d)])), []);
}

#[test]
fn partners_are_a_sources_two_files() {
    let all: BTreeSet<String> =
        ["terrain/a.toml", "terrain/a.geojson", "terrain/b.toml", "terrain/score.toml"].map(String::from).into();
    assert_eq!(Extents.partners("terrain/a.toml", &all), ["terrain/a.geojson"]);
    assert_eq!(Extents.partners("terrain/a.geojson", &all), ["terrain/a.toml"]);
    assert!(Extents.partners("terrain/b.toml", &all).is_empty());
    assert!(Extents.partners("terrain/score.toml", &all).is_empty());
}

/// The terrain's declarations, pieces and score as producers/extents/ made them.
const REAL: [(&str, &[u8]); 20] = [
    ("score.toml", include_bytes!("../../../../../producers/extents/terrain/score.toml")),
    ("cop-dem.toml", include_bytes!("../../../../../producers/extents/terrain/cop-dem.toml")),
    ("cop-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/cop-dem.geojson")),
    ("geobc-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/geobc-lidar.toml")),
    ("geobc-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/geobc-lidar.geojson")),
    ("ign-lidar-hd.toml", include_bytes!("../../../../../producers/extents/terrain/ign-lidar-hd.toml")),
    ("ign-lidar-hd.geojson", include_bytes!("../../../../../producers/extents/terrain/ign-lidar-hd.geojson")),
    ("jaxa-aw3d30.toml", include_bytes!("../../../../../producers/extents/terrain/jaxa-aw3d30.toml")),
    ("jaxa-aw3d30.geojson", include_bytes!("../../../../../producers/extents/terrain/jaxa-aw3d30.geojson")),
    ("luxembourg-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/luxembourg-lidar.toml")),
    ("luxembourg-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/luxembourg-lidar.geojson")),
    ("nrcan-hrdem.toml", include_bytes!("../../../../../producers/extents/terrain/nrcan-hrdem.toml")),
    ("nrcan-hrdem.geojson", include_bytes!("../../../../../producers/extents/terrain/nrcan-hrdem.geojson")),
    ("pgc-arcticdem.toml", include_bytes!("../../../../../producers/extents/terrain/pgc-arcticdem.toml")),
    ("pgc-arcticdem.geojson", include_bytes!("../../../../../producers/extents/terrain/pgc-arcticdem.geojson")),
    ("pgc-rema.toml", include_bytes!("../../../../../producers/extents/terrain/pgc-rema.toml")),
    ("pgc-rema.geojson", include_bytes!("../../../../../producers/extents/terrain/pgc-rema.geojson")),
    ("usgs-3dep.toml", include_bytes!("../../../../../producers/extents/terrain/usgs-3dep.toml")),
    ("wales-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/wales-lidar.toml")),
    ("wales-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/wales-lidar.geojson")),
];

#[test]
fn the_real_terrain_files_are_taken() {
    // (Every file there is here: one added to the folder fails this until it's listed.)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../producers/extents/terrain");
    let mut on_disk: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    on_disk.sort();
    let mut listed: Vec<String> = REAL.iter().map(|(n, _)| n.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
    let files: BTreeMap<String, Vec<u8>> = REAL.iter().map(|(n, b)| (format!("terrain/{n}"), b.to_vec())).collect();
    let mut warnings = Vec::new();
    for (p, b) in &files {
        let c = Extents.file(p, b);
        let errors: Vec<&Finding> = c.findings.iter().filter(|f| f.level == Level::Error).collect();
        assert!(errors.is_empty(), "{p}: {errors:#?}");
        warnings.extend(c.findings.iter().filter(|f| f.level == Level::Warning).map(|f| f.message.clone()));
        if p.ends_with(".geojson") {
            assert!(c.facts["pieces"].as_u64().unwrap() > 0, "{p}");
            assert!(c.facts.contains_key("box"), "{p}");
        }
    }
    let read = |p: &str| -> Result<Arc<Vec<u8>>> { Ok(Arc::new(files[p].clone())) };
    let whole = Extents.whole(&version_of(&files, &read)).unwrap();
    let errors: Vec<&Finding> = whole.iter().filter(|f| f.level == Level::Error).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    warnings.extend(whole.iter().map(|f| f.message.clone()));
    // (3DEP's index refused every query: declared, no pieces yet.)
    assert_eq!(warnings, ["terrain/usgs-3dep.toml: no terrain/usgs-3dep.geojson yet (made by producers/extents/extents.py): the source has no pieces"]);
    // NRCan's 950 projects and its mosaic, its box the country's north.
    let c = Extents.file("terrain/nrcan-hrdem.geojson", &files["terrain/nrcan-hrdem.geojson"]);
    assert_eq!(c.facts["pieces"], 951);
    assert_eq!(c.facts["classes"], serde_json::json!({"files": 1, "index": 950}));
}

/// Malformed files are findings, never a panic: each kind's fixture cut short at every few bytes,
/// and values of the wrong type in its fields, through the file check and the whole version's.
#[test]
fn malformed_files_are_findings() {
    let pieces = good();
    let wrong = [
        ("terrain/score.toml", SCORE_TOML.replace("0.1\n", "\"x\"\n").replace("[metric.year]", "[metric.year.x]")),
        ("terrain/lidar.toml", DECL.replace("\"year\"]", "1]").replace("metrics]", "metrics.x]").replace("[[piece]]\nid = \"rest\"", "piece = 3\n[[x]]")),
        ("terrain/lidar.geojson", String::from_utf8(pieces.clone()).unwrap().replace("[0.0,0.0]", "[null]").replace("\"read\":[", "\"read\":[3,")),
        ("terrain/lidar.geojson", r#"{"type":"FeatureCollection","features":[3,{"type":"Feature","properties":{"piece":"a","extent":"index","availability":"open"},"geometry":{"type":"MultiPolygon","coordinates":[3,[[]],[[[0,0],[1,0]]]]}}]}"#.to_string()),
    ];
    let mut cases: Vec<(&str, Vec<u8>)> = wrong.iter().map(|(p, b)| (*p, b.clone().into_bytes())).collect();
    for (p, b) in [
        ("terrain/score.toml", SCORE_TOML.as_bytes()),
        ("terrain/lidar.toml", DECL.as_bytes()),
        ("terrain/lidar.geojson", &pieces[..]),
    ] {
        cases.extend((0..b.len() - 2).step_by(7).map(|n| (p, b[..n].to_vec())));
    }
    for (k, (p, b)) in cases.iter().enumerate() {
        let c = Extents.file(p, b);
        // (The wrong values are errors; a file cut short may still parse, so it need only not panic.)
        if k < wrong.len() {
            assert!(c.findings.iter().any(|f| f.level == Level::Error), "{p}: {}", String::from_utf8_lossy(b));
        }
        let mut files = vec![
            ("terrain/score.toml", SCORE_TOML.as_bytes()),
            ("terrain/lidar.toml", DECL.as_bytes()),
            ("terrain/lidar.geojson", &pieces[..]),
        ];
        files.retain(|(q, _)| q != p);
        files.push((p, b));
        whole_of(&files);
    }
}

/// The server's score (`Score::of`) is score.py's: on score.toml's own examples and the pieces in
/// score_pieces.json, each score and metric value as producers/extents/score.py made them
/// (score_expected.json: the command in score.py's usage; producers/extents/test_extents.py makes
/// it again and checks it's this file), to the last digit it shows (three) and well past it.
#[test]
fn the_score_is_score_py_s() {
    let s = score(REAL[0].1).unwrap();
    let expected: Vec<Value> = serde_json::from_str(include_str!("score_expected.json")).unwrap();
    assert!(expected.len() > 15, "the examples and the pieces");
    for e in &expected {
        let metrics = e["metrics"].as_object().unwrap();
        let (got, vals) = s.of(metrics);
        match e["score"].as_f64() {
            Some(want) => {
                let got = got.unwrap_or_else(|| panic!("{metrics:?}: no score, score.py's {want}"));
                assert!((got - want).abs() < 1e-12, "{metrics:?}: {got} against score.py's {want}");
                assert_eq!(format!("{got:.3}"), format!("{want:.3}"), "{metrics:?}");
            }
            None => assert_eq!(got, None, "{metrics:?}"),
        }
        for (k, want) in e["values"].as_object().unwrap() {
            match (vals[k], want.as_f64()) {
                (Some(g), Some(w)) => assert!((g - w).abs() < 1e-12, "{metrics:?} {k}: {g} against {w}"),
                (g, w) => assert_eq!(g, w, "{metrics:?} {k}"),
            }
        }
    }
}
