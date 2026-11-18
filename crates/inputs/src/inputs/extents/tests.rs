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
    // (No availability order: availability ranks no tie.)
    assert!(s.availability.is_empty() && s.label.is_none() && s.ignored.is_empty());
    // `unstated`: UNSTATED where the file sets none, else its own, a value of the rest's range
    // (out of it the guarantee's bound wouldn't hold).
    assert_eq!(s.unstated, UNSTATED);
    let set = SCORE_TOML.replacen("share = 0.02\n", "share = 0.02\nunstated = 0.25\n", 1);
    assert!(checks_of("terrain/score.toml", set.as_bytes()).is_empty());
    let set = score(set.as_bytes()).unwrap();
    assert_eq!(set.unstated, 0.25);
    let bare = serde_json::json!({"spacing_m": 1});
    let lead = set.value("spacing_m", Some(&bare["spacing_m"])).unwrap();
    assert!(
        (set.of(bare.as_object().unwrap()).0.unwrap() - ((1.0 - set.share) * lead + set.share * 0.25)).abs() < 1e-12
    );
    for bad in ["-0.1", "1.5", "\"half\""] {
        let bad = SCORE_TOML.replacen("share = 0.02\n", &format!("share = 0.02\nunstated = {bad}\n"), 1);
        assert_eq!(checks_of("terrain/score.toml", bad.as_bytes()), one(SCORE_CHECK, Level::Error), "{bad}");
    }
}

/// A metric a piece doesn't state leaves both of the rest's sums, a metric of weight 0 takes no
/// part, and a piece stating no weighted metric has `unstated` (the owner, 13 Nov: the middle) for
/// its rest. At Tarn, France's LiDAR HD tiles of 2021 so outrank the claimed MNS Correl stating no
/// year; tiles of 2008 would not.
#[test]
fn a_piece_stating_no_weighted_metric_has_the_middle() {
    let s = score(REAL[0].1).unwrap();
    let of = |m: serde_json::Value| s.of(m.as_object().unwrap()).0.unwrap();
    let (share, lead) = (s.share, s.value("spacing_m", Some(&serde_json::json!(0.5))).unwrap());
    let (wy, wa) = (s.metrics["year"].weight.unwrap(), s.metrics["vertical_rmse_m"].weight.unwrap());
    let year = s.value("year", Some(&serde_json::json!(2021))).unwrap();
    // The year stated alone: the rest is its value, the accuracy's weight not diluting it.
    let dated = of(serde_json::json!({"spacing_m": 0.5, "year": 2021}));
    assert!((dated - ((1.0 - share) * lead + share * year)).abs() < 1e-12);
    let rmse = s.value("vertical_rmse_m", Some(&serde_json::json!(0.15))).unwrap();
    let both = of(serde_json::json!({"spacing_m": 0.5, "year": 2021, "vertical_rmse_m": 0.15}));
    assert!((both - ((1.0 - share) * lead + share * (wy * year + wa * rmse) / (wy + wa))).abs() < 1e-12);
    let none = of(serde_json::json!({"spacing_m": 0.5}));
    assert!((none - ((1.0 - share) * lead + share * UNSTATED)).abs() < 1e-12);
    assert_eq!(of(serde_json::json!({"spacing_m": 0.5, "surface": "surface"})), none);
    assert!(dated > none);
    assert!(of(serde_json::json!({"spacing_m": 0.5, "year": 2008})) < none);
    // Within the factor the year decides between two near grids, stated alone on both: a 1.2 m
    // grid of 2010 below a 1.25 m one of 2024.
    assert!(
        of(serde_json::json!({"spacing_m": 1.2, "year": 2010}))
            < of(serde_json::json!({"spacing_m": 1.25, "year": 2024}))
    );
    // France at Tarn: the claimed image-matched model (first, by id) against the LiDAR HD tiles.
    let rank = |year: i64| {
        let ps = [
            (of(serde_json::json!({"spacing_m": 0.5, "surface": "surface"})), "claimed"),
            (of(serde_json::json!({"spacing_m": 0.5, "surface": "ground", "year": year})), "index"),
        ];
        let rs: Vec<Ranked> = ps
            .iter()
            .map(|&(score, extent)| Ranked { lead, availability: Some("open"), score, extent: Some(extent) })
            .collect();
        s.best(&rs)
    };
    assert_eq!(rank(2021), Some(1));
    assert_eq!(rank(2008), Some(0));
    // No weighted metric in the file: the rest is `unstated` whatever is stated.
    let mut flat = s.clone();
    for m in flat.metrics.values_mut().filter(|m| m.weight.is_some()) {
        m.weight = Some(0.0);
    }
    let stated = serde_json::json!({"spacing_m": 0.5, "year": 2024, "vertical_rmse_m": 0.1});
    assert!((flat.of(stated.as_object().unwrap()).0.unwrap() - none).abs() < 1e-12);
}

/// The real score.toml: its layer's name and availability order, each mistake in the order refused.
#[test]
fn a_scores_availability_order() {
    let real = std::str::from_utf8(REAL[0].1).unwrap();
    let c = Extents.file("terrain/score.toml", real.as_bytes());
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    let s = score(real.as_bytes()).unwrap();
    assert_eq!(s.label.as_deref(), Some("Best data"));
    assert_eq!(s.availability, AVAILABILITY.map(|(k, _)| k), "open, login, paid, closed");
    assert_eq!(s.avail_rank(Some("open")), 0);
    assert_eq!(s.avail_rank(Some("closed")), 3);
    assert_eq!(s.avail_rank(None), 4);
    // Free data, the layers' default view: those of the order that cost nothing.
    assert_eq!(FREE.map(|a| s.avail_rank(Some(a))), [0, 1]);
    // The kind is a layer of its own and no part of the score (weight 0, the owner's, 13 Nov): a
    // ground and a surface model of one spacing and year score the same, and one stating its kind
    // alone scores as one stating nothing.
    assert_eq!(s.metrics["surface"].weight, Some(0.0));
    let of = |m: serde_json::Value| s.of(m.as_object().unwrap());
    let lead = of(serde_json::json!({"spacing_m": 1})).0;
    for kind in ["ground", "surface"] {
        assert_eq!(of(serde_json::json!({"spacing_m": 1, "surface": kind})).0, lead, "{kind}");
        assert_eq!(
            of(serde_json::json!({"spacing_m": 1, "surface": kind, "year": 2013})).0,
            of(serde_json::json!({"spacing_m": 1, "year": 2013})).0,
            "{kind}"
        );
    }
    for (from, to) in [
        ("\"closed\"]", "]"),
        ("\"closed\"]", "\"closed\", \"open\"]"),
        ("\"closed\"]", "\"gone\"]"),
        ("\"closed\"]", "3]"),
        ("availability = [", "availability = 3\nx = ["),
        ("label = \"Best data\"", "label = \"\""),
    ] {
        let bad = real.replacen(from, to, 1);
        assert_ne!(bad, real, "{from}");
        assert_eq!(checks_of("terrain/score.toml", bad.as_bytes()), one(SCORE_CHECK, Level::Error), "{from} → {to}");
    }
}

/// A score.toml as the NAS held it before the layers' colours were the app's (13 Nov): read as if
/// its classes, colours, ramp and shade weren't there, which the check says (a warning, so it's
/// accepted and the server keeps drawing until the new file replaces it).
#[test]
fn a_score_with_its_retired_colours_is_read_without_them() {
    let old = SCORE_TOML
        .replacen(
            "share = 0.02\n",
            "share = 0.02\nlabel = \"Best data\"\nramp = [\"#803041\", \"#e7fda7\"]\nshade = 0.4\n",
            1,
        )
        .replacen(
            "best = 0.1\n",
            "best = 0.1\nclasses = [{ upto = 0.625, label = \"0.5 m\" }, { label = \"coarser\" }]\n",
            1,
        )
        .replacen(
            "values = { ground = 1.0, surface = 0.0 }\n",
            "values = { ground = 1.0, surface = 0.0 }\ncolours = { ground = \"#56b4e9\", surface = \"#e69f00\" }\n",
            1,
        );
    let s = score(old.as_bytes()).unwrap();
    let mut plain = score(SCORE_TOML.as_bytes()).unwrap();
    plain.label = Some("Best data".into());
    plain.ignored =
        vec!["ramp".into(), "shade".into(), "metric.spacing_m.classes".into(), "metric.surface.colours".into()];
    assert_eq!(s, plain);
    assert_eq!(checks_of("terrain/score.toml", old.as_bytes()), one(RETIRED_CHECK, Level::Warning));
    // A retired field's own mistakes don't matter: it isn't read.
    let odd = old.replacen("shade = 0.4", "shade = \"some\"", 1);
    assert_eq!(checks_of("terrain/score.toml", odd.as_bytes()), one(RETIRED_CHECK, Level::Warning));
}

/// The best piece and the order (`Score::best`, `ranked`): a finer grid beyond the factor wins
/// whatever its availability; within it, availability first, then the score, then the extent's
/// class; equals, the first.
#[test]
fn an_open_piece_wins_a_tie() {
    let s = score(REAL[0].1).unwrap();
    let piece = |spacing: f64, avail: &'static str, year: f64| {
        let m = serde_json::json!({"spacing_m": spacing, "surface": "ground", "year": year});
        let (score, vals) = s.of(m.as_object().unwrap());
        (vals["spacing_m"].unwrap(), avail, score.unwrap())
    };
    let ranked = |ps: &[(f64, &'static str, f64)]| {
        let rs: Vec<Ranked> = ps
            .iter()
            .map(|&(lead, a, score)| Ranked { lead, availability: Some(a), score, extent: Some("index") })
            .collect();
        (s.best(&rs), s.ranked(&rs))
    };
    // Berlin: the BKG's paid 1 m of 2022 against the Länder's open 1 m of 2021: the open one, though
    // its score is lower.
    let (paid, open) = (piece(1.0, "paid", 2022.0), piece(1.0, "open", 2021.0));
    assert!(paid.2 > open.2);
    assert_eq!(ranked(&[paid, open]), (Some(1), vec![1, 0]));
    // Within the factor (1.2 m against 1 m): still the open one; beyond it (1.3 m), the finer.
    assert_eq!(ranked(&[paid, piece(1.2, "open", 2021.0)]).0, Some(1));
    assert_eq!(ranked(&[paid, piece(1.3, "open", 2021.0)]).0, Some(0));
    // Login before paid before closed; of one availability, the score.
    let (login, closed) = (piece(1.0, "login", 2000.0), piece(1.0, "closed", 2026.0));
    assert_eq!(ranked(&[closed, paid, login]).1, vec![2, 1, 0]);
    assert_eq!(ranked(&[piece(1.0, "open", 2010.0), open]).1, vec![1, 0]);
    // The order: the best, then the best of the rest. A coarse open piece ties none of the fine
    // ones, so ranks after them; of two equal pieces, the first.
    let coarse = piece(30.0, "open", 2024.0);
    assert_eq!(ranked(&[coarse, paid, open]).1, vec![2, 1, 0]);
    assert_eq!(ranked(&[open, open]).1, vec![0, 1]);
    assert_eq!(ranked(&[]), (None, vec![]));
    // No order declared: availability ranks no tie (the score alone).
    let plain = score(SCORE_TOML.as_bytes()).unwrap();
    let rs = [
        Ranked { lead: paid.0, availability: Some("paid"), score: paid.2, extent: Some("index") },
        Ranked { lead: open.0, availability: Some("open"), score: open.2, extent: Some("index") },
    ];
    assert_eq!(plain.best(&rs), Some(0));
    // Of equal scores, the better-founded extent, whichever comes first: France's LiDAR HD (its
    // tile index) over an outline claimed over the same ground; a piece of no known class last.
    let at = |extent: Option<&'static str>| Ranked { lead: open.0, availability: Some("open"), score: open.2, extent };
    for (a, b) in [(Some("claimed"), Some("index")), (Some("service"), Some("files")), (None, Some("claimed"))] {
        assert_eq!(s.ranked(&[at(a), at(b)]), vec![1, 0], "{a:?} {b:?}");
        assert_eq!(s.ranked(&[at(b), at(a)]), vec![0, 1], "{a:?} {b:?}");
    }
    // The class breaks only an exact tie: a better score wins whatever its extent.
    let better = Ranked { score: open.2 + 1e-9, ..at(Some("claimed")) };
    assert_eq!(s.best(&[at(Some("mask")), better]), Some(1));
    assert_eq!(CLASSES.map(|(c, _)| class_rank(Some(c))), [0, 1, 2, 3, 4, 5]);
    // The layers: the score's, drawing the primary, then the other metrics; the primary none of
    // its own.
    assert_eq!(s.layers().collect::<Vec<_>>(), [SCORE_LAYER, "surface", "vertical_rmse_m", "year"]);
    assert_eq!(s.metric_of(SCORE_LAYER), Some(("spacing_m", &s.metrics["spacing_m"])));
    assert_eq!(s.metric_of("year"), Some(("year", &s.metrics["year"])));
    assert_eq!((s.metric_of("spacing_m"), s.metric_of("slope")), (None, None));
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
        ("url = \"https", "timeout_s = \"long\"\nurl = \"https"),
        ("url = \"https", "filter = { year = 2020 }\nurl = \"https"),
        ("url = \"https", "exclude = { year = 2020 }\nurl = \"https"),
        ("{ field = \"year\", date = \"year\" }", "[]"),
        ("{ field = \"year\", date = \"year\" }", "[2020, { field = \"year\", date = \"year\" }]"),
        ("{ field = \"year\", date = \"year\" }", "[{ field = \"year\", date = \"julian\" }]"),
        ("{ field = \"year\", date = \"year\" }", "{ field = \"year\", pattern = \"\", date = \"year\" }"),
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
    // Readings in turn, and the ATOM reader's class fixed as an index.
    let turns = DECL.replacen(
        "{ field = \"year\", date = \"year\" }",
        "[{ field = \"name\", date = \"text\" }, { field = \"datetime\", date = \"yymmdd\" }, 2020]",
        1,
    );
    assert!(checks_of("terrain/lidar.toml", turns.as_bytes()).is_empty());
    // A pattern picking the value out of a field.
    let picked = DECL.replacen(
        "{ field = \"year\", date = \"year\" }",
        "{ field = \"path\", pattern = 'MDT(\\d+)_', date = \"year\" }",
        1,
    );
    assert_ne!(picked, DECL);
    assert!(checks_of("terrain/lidar.toml", picked.as_bytes()).is_empty());
    let atom = DECL.replacen("method = \"arcgis\"\nclass = \"index\"", "method = \"atom\"", 1);
    assert!(checks_of("terrain/lidar.toml", atom.as_bytes()).is_empty());
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

/// A primary of words (the buildings' `detail`): no `within`, its pieces tying on the same word, and
/// the guarantee's bound the least gap between its values, k ÷ (1 + k).
#[test]
fn a_words_primary_and_its_guarantee() {
    const BUILDINGS: &str = include_str!("../../../../../producers/extents/buildings/score.toml");
    let c = Extents.file("buildings/score.toml", BUILDINGS.as_bytes());
    assert!(c.findings.is_empty(), "{:?}", c.findings);
    let s = score(BUILDINGS.as_bytes()).unwrap();
    assert_eq!((s.primary.as_str(), s.within), ("detail", None));
    let Scale::Values(words) = &s.metrics["detail"].scale else { panic!("detail: {:?}", s.metrics["detail"].scale) };
    // (The least gap, lod3's 0.9 to the mesh's 1.)
    assert!((Score::bound_values(words).unwrap() - 0.1 / 1.1).abs() < 1e-12);
    assert_eq!(Score::bound_values(&BTreeMap::from([("one".to_string(), 1.0)])), None);
    // A better word outscores a worse one whatever the rest; the same word ranks by its rest.
    let of = |m: serde_json::Value| s.of(m.as_object().unwrap()).0.unwrap();
    let best_lod2 = of(serde_json::json!({"detail": "lod2", "measured": "lidar", "completeness": 100, "year": 2026}));
    let worst_lod3 = of(serde_json::json!({"detail": "lod3", "measured": "gridded", "completeness": 0, "year": 2000}));
    assert!(worst_lod3 > best_lod2);
    assert!(best_lod2 > of(serde_json::json!({"detail": "lod2", "measured": "estimated"})));
    assert_eq!(s.tie(), 0.0);
    for (from, to) in [
        // A factor for words, a share over the bound, a single word.
        ("share = 0.05", "share = 0.05\nwithin = 1.25"),
        ("share = 0.05", "share = 0.1"),
        (
            "footprints = 0.0\nstoreys = 0.25\nheights = 0.5\nlod1 = 0.5\nlod2 = 0.75\nlod3 = 0.9\nmesh = 1.0",
            "mesh = 1.0",
        ),
    ] {
        let bad = BUILDINGS.replacen(from, to, 1);
        assert_ne!(bad, BUILDINGS);
        assert_eq!(checks_of("buildings/score.toml", bad.as_bytes()), one(SCORE_CHECK, Level::Error), "{from} → {to}");
    }
}

/// An `overture` block: its pieces of class mask, drawn by the build (no geometry in the file), each
/// naming its dataset and what it holds.
#[test]
fn an_overture_block() {
    const OVERTURE: &str = r#"name = "Overture buildings"
credit = "Overture"
licence = "ODbL-1.0"
theme = "buildings"
availability = "open"

[[piece]]
id = "osm-heights"
method = "overture"
dataset = "OpenStreetMap"
holds = "heights"
[piece.metrics]
detail = "heights"
"#;
    assert!(checks_of("buildings/overture.toml", OVERTURE.as_bytes()).is_empty());
    let d = declaration(OVERTURE.as_bytes()).unwrap();
    assert_eq!(d.blocks[0].extent, "mask");
    assert_eq!(d.blocks[0].overture, Some(MaskBlock { dataset: "OpenStreetMap".into(), holds: "heights".into() }));
    for (from, to) in [
        ("dataset = \"OpenStreetMap\"\n", ""),
        ("dataset = \"OpenStreetMap\"", "dataset = \" \""),
        ("holds = \"heights\"", "holds = \"roofs\""),
        ("method = \"overture\"", "method = \"overture\"\nclass = \"index\""),
    ] {
        let bad = OVERTURE.replacen(from, to, 1);
        assert_ne!(bad, OVERTURE);
        assert_eq!(
            checks_of("buildings/overture.toml", bad.as_bytes()),
            one(DECL_CHECK, Level::Error),
            "{from} → {to}"
        );
    }
    // Its file: the piece with no geometry, of class mask (another class with none is refused), its
    // reading the block's dataset and holds (another's isn't made from the block).
    let file = |extent: &str, holds: &str| {
        let f = feature(
            &format!(r#""piece":"osm-heights","extent":"{extent}","availability":"open","detail":"heights""#),
            "null",
        );
        format!(
            r#"{{"type":"FeatureCollection","source":"overture","theme":"buildings","read":[{{"piece":"osm-heights","method":"overture","dataset":"OpenStreetMap","holds":"{holds}"}}],"features":[{f}]}}"#
        )
    };
    assert!(checks_of("buildings/overture.geojson", file("mask", "heights").as_bytes()).is_empty());
    assert!(!checks_of("buildings/overture.geojson", file("index", "heights").as_bytes()).is_empty());
    let score = include_bytes!("../../../../../producers/extents/buildings/score.toml");
    let made = |holds: &str| {
        let g = file("mask", holds);
        let fs = whole_of(&[
            ("buildings/score.toml", score),
            ("buildings/overture.toml", OVERTURE.as_bytes()),
            ("buildings/overture.geojson", g.as_bytes()),
        ]);
        fs.iter().map(|f| f.check().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(made("heights"), Vec::<String>::new());
    assert_eq!(made("storeys"), [MADE]);
}

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
const REAL: [(&str, &[u8]); 583] = [
    ("score.toml", include_bytes!("../../../../../producers/extents/terrain/score.toml")),
    ("aad-station-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/aad-station-dem.geojson")),
    ("aad-station-dem.toml", include_bytes!("../../../../../producers/extents/terrain/aad-station-dem.toml")),
    ("ad-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/ad-mdt.geojson")),
    ("ad-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/ad-mdt.toml")),
    ("ae-dubai-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/ae-dubai-dtm.geojson")),
    ("ae-dubai-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/ae-dubai-dtm.toml")),
    ("africa-local-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/africa-local-dem.geojson")),
    ("africa-local-dem.toml", include_bytes!("../../../../../producers/extents/terrain/africa-local-dem.toml")),
    ("al-asig.geojson", include_bytes!("../../../../../producers/extents/terrain/al-asig.geojson")),
    ("al-asig.toml", include_bytes!("../../../../../producers/extents/terrain/al-asig.toml")),
    ("antigua-tcarta.geojson", include_bytes!("../../../../../producers/extents/terrain/antigua-tcarta.geojson")),
    ("antigua-tcarta.toml", include_bytes!("../../../../../producers/extents/terrain/antigua-tcarta.toml")),
    ("antilles-litto3d.geojson", include_bytes!("../../../../../producers/extents/terrain/antilles-litto3d.geojson")),
    ("antilles-litto3d.toml", include_bytes!("../../../../../producers/extents/terrain/antilles-litto3d.toml")),
    ("ar-cba-idecor.geojson", include_bytes!("../../../../../producers/extents/terrain/ar-cba-idecor.geojson")),
    ("ar-cba-idecor.toml", include_bytes!("../../../../../producers/extents/terrain/ar-cba-idecor.toml")),
    ("ar-ign-mde.geojson", include_bytes!("../../../../../producers/extents/terrain/ar-ign-mde.geojson")),
    ("ar-ign-mde.toml", include_bytes!("../../../../../producers/extents/terrain/ar-ign-mde.toml")),
    ("at-bev-als-dgm.geojson", include_bytes!("../../../../../producers/extents/terrain/at-bev-als-dgm.geojson")),
    ("at-bev-als-dgm.toml", include_bytes!("../../../../../producers/extents/terrain/at-bev-als-dgm.toml")),
    ("at-bgld-dgm.geojson", include_bytes!("../../../../../producers/extents/terrain/at-bgld-dgm.geojson")),
    ("at-bgld-dgm.toml", include_bytes!("../../../../../producers/extents/terrain/at-bgld-dgm.toml")),
    ("at-ktn-als.geojson", include_bytes!("../../../../../producers/extents/terrain/at-ktn-als.geojson")),
    ("at-ktn-als.toml", include_bytes!("../../../../../producers/extents/terrain/at-ktn-als.toml")),
    ("at-ooe-dgm.geojson", include_bytes!("../../../../../producers/extents/terrain/at-ooe-dgm.geojson")),
    ("at-ooe-dgm.toml", include_bytes!("../../../../../producers/extents/terrain/at-ooe-dgm.toml")),
    ("at-tirol-dgm.geojson", include_bytes!("../../../../../producers/extents/terrain/at-tirol-dgm.geojson")),
    ("at-tirol-dgm.toml", include_bytes!("../../../../../producers/extents/terrain/at-tirol-dgm.toml")),
    ("at-vbg-dgm.geojson", include_bytes!("../../../../../producers/extents/terrain/at-vbg-dgm.geojson")),
    ("at-vbg-dgm.toml", include_bytes!("../../../../../producers/extents/terrain/at-vbg-dgm.toml")),
    ("au-act-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/au-act-lidar.geojson")),
    ("au-act-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/au-act-lidar.toml")),
    ("au-ga-dem5m.geojson", include_bytes!("../../../../../producers/extents/terrain/au-ga-dem5m.geojson")),
    ("au-ga-dem5m.toml", include_bytes!("../../../../../producers/extents/terrain/au-ga-dem5m.toml")),
    ("au-ga-srtm1s.geojson", include_bytes!("../../../../../producers/extents/terrain/au-ga-srtm1s.geojson")),
    ("au-ga-srtm1s.toml", include_bytes!("../../../../../producers/extents/terrain/au-ga-srtm1s.toml")),
    ("au-ga-surveys.geojson", include_bytes!("../../../../../producers/extents/terrain/au-ga-surveys.geojson")),
    ("au-ga-surveys.toml", include_bytes!("../../../../../producers/extents/terrain/au-ga-surveys.toml")),
    ("au-iot-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/au-iot-dem.geojson")),
    ("au-iot-dem.toml", include_bytes!("../../../../../producers/extents/terrain/au-iot-dem.toml")),
    ("au-nsw-elevation.geojson", include_bytes!("../../../../../producers/extents/terrain/au-nsw-elevation.geojson")),
    ("au-nsw-elevation.toml", include_bytes!("../../../../../producers/extents/terrain/au-nsw-elevation.toml")),
    ("au-qld-elevation.geojson", include_bytes!("../../../../../producers/extents/terrain/au-qld-elevation.geojson")),
    ("au-qld-elevation.toml", include_bytes!("../../../../../producers/extents/terrain/au-qld-elevation.toml")),
    ("au-sa-dew.geojson", include_bytes!("../../../../../producers/extents/terrain/au-sa-dew.geojson")),
    ("au-sa-dew.toml", include_bytes!("../../../../../producers/extents/terrain/au-sa-dew.toml")),
    ("au-tas-elevation.geojson", include_bytes!("../../../../../producers/extents/terrain/au-tas-elevation.geojson")),
    ("au-tas-elevation.toml", include_bytes!("../../../../../producers/extents/terrain/au-tas-elevation.toml")),
    ("au-vic-elevation.geojson", include_bytes!("../../../../../producers/extents/terrain/au-vic-elevation.geojson")),
    ("au-vic-elevation.toml", include_bytes!("../../../../../producers/extents/terrain/au-vic-elevation.toml")),
    ("au-wa-landgate.geojson", include_bytes!("../../../../../producers/extents/terrain/au-wa-landgate.geojson")),
    ("au-wa-landgate.toml", include_bytes!("../../../../../producers/extents/terrain/au-wa-landgate.toml")),
    ("ba-fgu.geojson", include_bytes!("../../../../../producers/extents/terrain/ba-fgu.geojson")),
    ("ba-fgu.toml", include_bytes!("../../../../../producers/extents/terrain/ba-fgu.toml")),
    ("ba-rs-rgurs.geojson", include_bytes!("../../../../../producers/extents/terrain/ba-rs-rgurs.geojson")),
    ("ba-rs-rgurs.toml", include_bytes!("../../../../../producers/extents/terrain/ba-rs-rgurs.toml")),
    ("barbados-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/barbados-lidar.geojson")),
    ("barbados-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/barbados-lidar.toml")),
    ("be-bru-urbis.geojson", include_bytes!("../../../../../producers/extents/terrain/be-bru-urbis.geojson")),
    ("be-bru-urbis.toml", include_bytes!("../../../../../producers/extents/terrain/be-bru-urbis.toml")),
    ("be-vlg-dhmv.geojson", include_bytes!("../../../../../producers/extents/terrain/be-vlg-dhmv.geojson")),
    ("be-vlg-dhmv.toml", include_bytes!("../../../../../producers/extents/terrain/be-vlg-dhmv.toml")),
    ("be-wal-mnt.geojson", include_bytes!("../../../../../producers/extents/terrain/be-wal-mnt.geojson")),
    ("be-wal-mnt.toml", include_bytes!("../../../../../producers/extents/terrain/be-wal-mnt.toml")),
    ("belize-crip.geojson", include_bytes!("../../../../../producers/extents/terrain/belize-crip.geojson")),
    ("belize-crip.toml", include_bytes!("../../../../../producers/extents/terrain/belize-crip.toml")),
    ("bermuda-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/bermuda-lidar.geojson")),
    ("bermuda-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/bermuda-lidar.toml")),
    ("bg-mzh.geojson", include_bytes!("../../../../../producers/extents/terrain/bg-mzh.geojson")),
    ("bg-mzh.toml", include_bytes!("../../../../../producers/extents/terrain/bg-mzh.toml")),
    ("bg-sofiaplan.geojson", include_bytes!("../../../../../producers/extents/terrain/bg-sofiaplan.geojson")),
    ("bg-sofiaplan.toml", include_bytes!("../../../../../producers/extents/terrain/bg-sofiaplan.toml")),
    ("bh-slrb-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/bh-slrb-dtm.geojson")),
    ("bh-slrb-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/bh-slrb-dtm.toml")),
    ("br-ap-sema-insar.geojson", include_bytes!("../../../../../producers/extents/terrain/br-ap-sema-insar.geojson")),
    ("br-ap-sema-insar.toml", include_bytes!("../../../../../producers/extents/terrain/br-ap-sema-insar.toml")),
    (
        "br-ba-salvador-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/br-ba-salvador-lidar.geojson"),
    ),
    ("br-ba-salvador-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/br-ba-salvador-lidar.toml")),
    ("br-ba-sei.geojson", include_bytes!("../../../../../producers/extents/terrain/br-ba-sei.geojson")),
    ("br-ba-sei.toml", include_bytes!("../../../../../producers/extents/terrain/br-ba-sei.toml")),
    (
        "br-ce-fortaleza-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/br-ce-fortaleza-lidar.geojson"),
    ),
    (
        "br-ce-fortaleza-lidar.toml",
        include_bytes!("../../../../../producers/extents/terrain/br-ce-fortaleza-lidar.toml"),
    ),
    ("br-df-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/br-df-mdt.geojson")),
    ("br-df-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/br-df-mdt.toml")),
    (
        "br-dsg-radiografia.geojson",
        include_bytes!("../../../../../producers/extents/terrain/br-dsg-radiografia.geojson"),
    ),
    ("br-dsg-radiografia.toml", include_bytes!("../../../../../producers/extents/terrain/br-dsg-radiografia.toml")),
    ("br-es-geobases.geojson", include_bytes!("../../../../../producers/extents/terrain/br-es-geobases.geojson")),
    ("br-es-geobases.toml", include_bytes!("../../../../../producers/extents/terrain/br-es-geobases.toml")),
    ("br-ibge-mde.geojson", include_bytes!("../../../../../producers/extents/terrain/br-ibge-mde.geojson")),
    ("br-ibge-mde.toml", include_bytes!("../../../../../producers/extents/terrain/br-ibge-mde.toml")),
    ("br-mg-bh-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/br-mg-bh-lidar.geojson")),
    ("br-mg-bh-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/br-mg-bh-lidar.toml")),
    ("br-pe-pe3d.geojson", include_bytes!("../../../../../producers/extents/terrain/br-pe-pe3d.geojson")),
    ("br-pe-pe3d.toml", include_bytes!("../../../../../producers/extents/terrain/br-pe-pe3d.toml")),
    ("br-rj-rio-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/br-rj-rio-mdt.geojson")),
    ("br-rj-rio-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/br-rj-rio-mdt.toml")),
    ("br-sc-sigsc.geojson", include_bytes!("../../../../../producers/extents/terrain/br-sc-sigsc.geojson")),
    ("br-sc-sigsc.toml", include_bytes!("../../../../../producers/extents/terrain/br-sc-sigsc.toml")),
    ("br-sp-estado-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/br-sp-estado-mdt.geojson")),
    ("br-sp-estado-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/br-sp-estado-mdt.toml")),
    (
        "br-sp-geosampa-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/br-sp-geosampa-lidar.geojson"),
    ),
    ("br-sp-geosampa-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/br-sp-geosampa-lidar.toml")),
    (
        "caribbean-nl-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/caribbean-nl-lidar.geojson"),
    ),
    ("caribbean-nl-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/caribbean-nl-lidar.toml")),
    ("cayman-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/cayman-lidar.geojson")),
    ("cayman-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/cayman-lidar.toml")),
    ("ch-be-bern-dom.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-be-bern-dom.geojson")),
    ("ch-be-bern-dom.toml", include_bytes!("../../../../../producers/extents/terrain/ch-be-bern-dom.toml")),
    ("ch-bl-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-bl-dtm.geojson")),
    ("ch-bl-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/ch-bl-dtm.toml")),
    ("ch-fr-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-fr-lidar.geojson")),
    ("ch-fr-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/ch-fr-lidar.toml")),
    ("ch-ge-mnt.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-ge-mnt.geojson")),
    ("ch-ge-mnt.toml", include_bytes!("../../../../../producers/extents/terrain/ch-ge-mnt.toml")),
    ("ch-lu-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-lu-dtm.geojson")),
    ("ch-lu-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/ch-lu-dtm.toml")),
    ("ch-sh-dom.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-sh-dom.geojson")),
    ("ch-sh-dom.toml", include_bytes!("../../../../../producers/extents/terrain/ch-sh-dom.toml")),
    ("ch-swissalti3d.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-swissalti3d.geojson")),
    ("ch-swissalti3d.toml", include_bytes!("../../../../../producers/extents/terrain/ch-swissalti3d.toml")),
    ("ch-swisssurface3d.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-swisssurface3d.geojson")),
    ("ch-swisssurface3d.toml", include_bytes!("../../../../../producers/extents/terrain/ch-swisssurface3d.toml")),
    ("ch-zh-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/ch-zh-dtm.geojson")),
    ("ch-zh-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/ch-zh-dtm.toml")),
    ("cl-ciren-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/cl-ciren-dem.geojson")),
    ("cl-ciren-dem.toml", include_bytes!("../../../../../producers/extents/terrain/cl-ciren-dem.toml")),
    ("cl-saf-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/cl-saf-lidar.geojson")),
    ("cl-saf-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/cl-saf-lidar.toml")),
    ("clipperton-shom.geojson", include_bytes!("../../../../../producers/extents/terrain/clipperton-shom.geojson")),
    ("clipperton-shom.toml", include_bytes!("../../../../../producers/extents/terrain/clipperton-shom.toml")),
    ("co-igac-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/co-igac-mdt.geojson")),
    ("co-igac-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/co-igac-mdt.toml")),
    ("co-medellin-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/co-medellin-lidar.geojson")),
    ("co-medellin-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/co-medellin-lidar.toml")),
    ("connecticut-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/connecticut-lidar.geojson")),
    ("connecticut-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/connecticut-lidar.toml")),
    ("cop-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/cop-dem.geojson")),
    ("cop-dem.toml", include_bytes!("../../../../../producers/extents/terrain/cop-dem.toml")),
    ("cv-fogo-uav.geojson", include_bytes!("../../../../../producers/extents/terrain/cv-fogo-uav.geojson")),
    ("cv-fogo-uav.toml", include_bytes!("../../../../../producers/extents/terrain/cv-fogo-uav.toml")),
    ("cy-dls.geojson", include_bytes!("../../../../../producers/extents/terrain/cy-dls.geojson")),
    ("cy-dls.toml", include_bytes!("../../../../../producers/extents/terrain/cy-dls.toml")),
    ("cz-brno-dmt.geojson", include_bytes!("../../../../../producers/extents/terrain/cz-brno-dmt.geojson")),
    ("cz-brno-dmt.toml", include_bytes!("../../../../../producers/extents/terrain/cz-brno-dmt.toml")),
    ("cz-cuzk-dmr4g.geojson", include_bytes!("../../../../../producers/extents/terrain/cz-cuzk-dmr4g.geojson")),
    ("cz-cuzk-dmr4g.toml", include_bytes!("../../../../../producers/extents/terrain/cz-cuzk-dmr4g.toml")),
    ("cz-praha-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/cz-praha-dtm.geojson")),
    ("cz-praha-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/cz-praha-dtm.toml")),
    ("dc-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/dc-lidar.geojson")),
    ("dc-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/dc-lidar.toml")),
    ("de-bb-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-bb-dgm1.geojson")),
    ("de-bb-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-bb-dgm1.toml")),
    ("de-be-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-be-dgm1.geojson")),
    ("de-be-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-be-dgm1.toml")),
    ("de-bkg-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-bkg-dgm1.geojson")),
    ("de-bkg-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-bkg-dgm1.toml")),
    ("de-bw-dgm025.geojson", include_bytes!("../../../../../producers/extents/terrain/de-bw-dgm025.geojson")),
    ("de-bw-dgm025.toml", include_bytes!("../../../../../producers/extents/terrain/de-bw-dgm025.toml")),
    ("de-by-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-by-dgm1.geojson")),
    ("de-by-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-by-dgm1.toml")),
    ("de-dlr-tandemx.geojson", include_bytes!("../../../../../producers/extents/terrain/de-dlr-tandemx.geojson")),
    ("de-dlr-tandemx.toml", include_bytes!("../../../../../producers/extents/terrain/de-dlr-tandemx.toml")),
    ("de-hb-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-hb-dgm1.geojson")),
    ("de-hb-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-hb-dgm1.toml")),
    ("de-he-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-he-dgm1.geojson")),
    ("de-he-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-he-dgm1.toml")),
    ("de-hh-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-hh-dgm1.geojson")),
    ("de-hh-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-hh-dgm1.toml")),
    ("de-mv-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-mv-dgm1.geojson")),
    ("de-mv-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-mv-dgm1.toml")),
    ("de-ni-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-ni-dgm1.geojson")),
    ("de-ni-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-ni-dgm1.toml")),
    ("de-nw-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-nw-dgm1.geojson")),
    ("de-nw-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-nw-dgm1.toml")),
    ("de-rp-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-rp-dgm1.geojson")),
    ("de-rp-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-rp-dgm1.toml")),
    ("de-sh-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-sh-dgm1.geojson")),
    ("de-sh-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-sh-dgm1.toml")),
    ("de-sl-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-sl-dgm1.geojson")),
    ("de-sl-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-sl-dgm1.toml")),
    ("de-sn-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-sn-dgm1.geojson")),
    ("de-sn-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-sn-dgm1.toml")),
    ("de-st-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-st-dgm1.geojson")),
    ("de-st-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-st-dgm1.toml")),
    ("de-th-dgm1.geojson", include_bytes!("../../../../../producers/extents/terrain/de-th-dgm1.geojson")),
    ("de-th-dgm1.toml", include_bytes!("../../../../../producers/extents/terrain/de-th-dgm1.toml")),
    ("dk-dhm.geojson", include_bytes!("../../../../../producers/extents/terrain/dk-dhm.geojson")),
    ("dk-dhm.toml", include_bytes!("../../../../../producers/extents/terrain/dk-dhm.toml")),
    ("dlr-polardem.geojson", include_bytes!("../../../../../producers/extents/terrain/dlr-polardem.geojson")),
    ("dlr-polardem.toml", include_bytes!("../../../../../producers/extents/terrain/dlr-polardem.toml")),
    ("dominica-dvrp.geojson", include_bytes!("../../../../../producers/extents/terrain/dominica-dvrp.geojson")),
    ("dominica-dvrp.toml", include_bytes!("../../../../../producers/extents/terrain/dominica-dvrp.toml")),
    ("dominicanrep-sgn.geojson", include_bytes!("../../../../../producers/extents/terrain/dominicanrep-sgn.geojson")),
    ("dominicanrep-sgn.toml", include_bytes!("../../../../../producers/extents/terrain/dominicanrep-sgn.toml")),
    ("ec-sigtierras-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/ec-sigtierras-mdt.geojson")),
    ("ec-sigtierras-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/ec-sigtierras-mdt.toml")),
    ("ee-maaruum-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/ee-maaruum-dtm.geojson")),
    ("ee-maaruum-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/ee-maaruum-dtm.toml")),
    ("eg-sri-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/eg-sri-dem.geojson")),
    ("eg-sri-dem.toml", include_bytes!("../../../../../producers/extents/terrain/eg-sri-dem.toml")),
    ("elsalvador-cnr.geojson", include_bytes!("../../../../../producers/extents/terrain/elsalvador-cnr.geojson")),
    ("elsalvador-cnr.toml", include_bytes!("../../../../../producers/extents/terrain/elsalvador-cnr.toml")),
    ("es-an-rediam.geojson", include_bytes!("../../../../../producers/extents/terrain/es-an-rediam.geojson")),
    ("es-an-rediam.toml", include_bytes!("../../../../../producers/extents/terrain/es-an-rediam.toml")),
    ("es-cb-mds.geojson", include_bytes!("../../../../../producers/extents/terrain/es-cb-mds.geojson")),
    ("es-cb-mds.toml", include_bytes!("../../../../../producers/extents/terrain/es-cb-mds.toml")),
    ("es-cl-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/es-cl-mdt.geojson")),
    ("es-cl-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/es-cl-mdt.toml")),
    ("es-ct-icgc.geojson", include_bytes!("../../../../../producers/extents/terrain/es-ct-icgc.geojson")),
    ("es-ct-icgc.toml", include_bytes!("../../../../../producers/extents/terrain/es-ct-icgc.toml")),
    ("es-ib-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/es-ib-mdt.geojson")),
    ("es-ib-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/es-ib-mdt.toml")),
    ("es-md-madrid.geojson", include_bytes!("../../../../../producers/extents/terrain/es-md-madrid.geojson")),
    ("es-md-madrid.toml", include_bytes!("../../../../../producers/extents/terrain/es-md-madrid.toml")),
    ("es-na-mde.geojson", include_bytes!("../../../../../producers/extents/terrain/es-na-mde.geojson")),
    ("es-na-mde.toml", include_bytes!("../../../../../producers/extents/terrain/es-na-mde.toml")),
    ("es-pnoa.geojson", include_bytes!("../../../../../producers/extents/terrain/es-pnoa.geojson")),
    ("es-pnoa.toml", include_bytes!("../../../../../producers/extents/terrain/es-pnoa.toml")),
    ("es-pv-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/es-pv-mdt.geojson")),
    ("es-pv-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/es-pv-mdt.toml")),
    ("es-ri-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/es-ri-mdt.geojson")),
    ("es-ri-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/es-ri-mdt.toml")),
    ("es-vc-icv.geojson", include_bytes!("../../../../../producers/extents/terrain/es-vc-icv.geojson")),
    ("es-vc-icv.toml", include_bytes!("../../../../../producers/extents/terrain/es-vc-icv.toml")),
    ("fabdem.geojson", include_bytes!("../../../../../producers/extents/terrain/fabdem.geojson")),
    ("fabdem.toml", include_bytes!("../../../../../producers/extents/terrain/fabdem.toml")),
    (
        "fi-helsinki-korkeusmalli.geojson",
        include_bytes!("../../../../../producers/extents/terrain/fi-helsinki-korkeusmalli.geojson"),
    ),
    (
        "fi-helsinki-korkeusmalli.toml",
        include_bytes!("../../../../../producers/extents/terrain/fi-helsinki-korkeusmalli.toml"),
    ),
    ("fi-nls-km2.geojson", include_bytes!("../../../../../producers/extents/terrain/fi-nls-km2.geojson")),
    ("fi-nls-km2.toml", include_bytes!("../../../../../producers/extents/terrain/fi-nls-km2.toml")),
    ("fi-nls-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/fi-nls-lidar.geojson")),
    ("fi-nls-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/fi-nls-lidar.toml")),
    ("fo-us-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/fo-us-dem.geojson")),
    ("fo-us-dem.toml", include_bytes!("../../../../../producers/extents/terrain/fo-us-dem.toml")),
    ("fr-craig-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/fr-craig-lidar.geojson")),
    ("fr-craig-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/fr-craig-lidar.toml")),
    ("fr-litto3d.geojson", include_bytes!("../../../../../producers/extents/terrain/fr-litto3d.geojson")),
    ("fr-litto3d.toml", include_bytes!("../../../../../producers/extents/terrain/fr-litto3d.toml")),
    ("fr-lyon-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/fr-lyon-lidar.geojson")),
    ("fr-lyon-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/fr-lyon-lidar.toml")),
    ("fr-mns-correl.geojson", include_bytes!("../../../../../producers/extents/terrain/fr-mns-correl.geojson")),
    ("fr-mns-correl.toml", include_bytes!("../../../../../producers/extents/terrain/fr-mns-correl.toml")),
    ("fr-rgealti.geojson", include_bytes!("../../../../../producers/extents/terrain/fr-rgealti.geojson")),
    ("fr-rgealti.toml", include_bytes!("../../../../../producers/extents/terrain/fr-rgealti.toml")),
    (
        "fr-strasbourg-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/fr-strasbourg-lidar.geojson"),
    ),
    ("fr-strasbourg-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/fr-strasbourg-lidar.toml")),
    ("ga-afrisar-lvis.geojson", include_bytes!("../../../../../producers/extents/terrain/ga-afrisar-lvis.geojson")),
    ("ga-afrisar-lvis.toml", include_bytes!("../../../../../producers/extents/terrain/ga-afrisar-lvis.toml")),
    ("gb-eng-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/gb-eng-lidar.geojson")),
    ("gb-eng-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/gb-eng-lidar.toml")),
    (
        "gb-nir-daera-coastal.geojson",
        include_bytes!("../../../../../producers/extents/terrain/gb-nir-daera-coastal.geojson"),
    ),
    ("gb-nir-daera-coastal.toml", include_bytes!("../../../../../producers/extents/terrain/gb-nir-daera-coastal.toml")),
    ("gb-nir-hed-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/gb-nir-hed-lidar.geojson")),
    ("gb-nir-hed-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/gb-nir-hed-lidar.toml")),
    ("gb-nir-osni-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/gb-nir-osni-dtm.geojson")),
    ("gb-nir-osni-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/gb-nir-osni-dtm.toml")),
    (
        "gb-nir-river-basin-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/gb-nir-river-basin-lidar.geojson"),
    ),
    (
        "gb-nir-river-basin-lidar.toml",
        include_bytes!("../../../../../producers/extents/terrain/gb-nir-river-basin-lidar.toml"),
    ),
    (
        "gb-nir-roads-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/gb-nir-roads-lidar.geojson"),
    ),
    ("gb-nir-roads-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/gb-nir-roads-lidar.toml")),
    ("gb-sct-hes-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/gb-sct-hes-lidar.geojson")),
    ("gb-sct-hes-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/gb-sct-hes-lidar.toml")),
    ("gb-sct-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/gb-sct-lidar.geojson")),
    ("gb-sct-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/gb-sct-lidar.toml")),
    ("gedtm30.geojson", include_bytes!("../../../../../producers/extents/terrain/gedtm30.geojson")),
    ("gedtm30.toml", include_bytes!("../../../../../producers/extents/terrain/gedtm30.toml")),
    ("geobc-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/geobc-lidar.geojson")),
    ("geobc-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/geobc-lidar.toml")),
    ("geonb-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/geonb-lidar.geojson")),
    ("geonb-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/geonb-lidar.toml")),
    ("gh-accra-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/gh-accra-lidar.geojson")),
    ("gh-accra-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/gh-accra-lidar.toml")),
    ("gr-ktimatologio.geojson", include_bytes!("../../../../../producers/extents/terrain/gr-ktimatologio.geojson")),
    ("gr-ktimatologio.toml", include_bytes!("../../../../../producers/extents/terrain/gr-ktimatologio.toml")),
    ("greenland-ghm.geojson", include_bytes!("../../../../../producers/extents/terrain/greenland-ghm.geojson")),
    ("greenland-ghm.toml", include_bytes!("../../../../../producers/extents/terrain/greenland-ghm.toml")),
    ("grenada-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/grenada-lidar.geojson")),
    ("grenada-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/grenada-lidar.toml")),
    ("gs-sggis-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/gs-sggis-dem.geojson")),
    ("gs-sggis-dem.toml", include_bytes!("../../../../../producers/extents/terrain/gs-sggis-dem.toml")),
    ("guatemala-ign.geojson", include_bytes!("../../../../../producers/extents/terrain/guatemala-ign.geojson")),
    ("guatemala-ign.toml", include_bytes!("../../../../../producers/extents/terrain/guatemala-ign.toml")),
    ("haiti-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/haiti-dtm.geojson")),
    ("haiti-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/haiti-dtm.toml")),
    ("hk-cedd-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/hk-cedd-lidar.geojson")),
    ("hk-cedd-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/hk-cedd-lidar.toml")),
    ("hk-landsd-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/hk-landsd-dtm.geojson")),
    ("hk-landsd-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/hk-landsd-dtm.toml")),
    ("hm-aad-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/hm-aad-dem.geojson")),
    ("hm-aad-dem.toml", include_bytes!("../../../../../producers/extents/terrain/hm-aad-dem.toml")),
    ("honduras-sinit.geojson", include_bytes!("../../../../../producers/extents/terrain/honduras-sinit.geojson")),
    ("honduras-sinit.toml", include_bytes!("../../../../../producers/extents/terrain/honduras-sinit.toml")),
    ("hr-dgu.geojson", include_bytes!("../../../../../producers/extents/terrain/hr-dgu.geojson")),
    ("hr-dgu.toml", include_bytes!("../../../../../producers/extents/terrain/hr-dgu.toml")),
    ("hu-lechner-ddm.geojson", include_bytes!("../../../../../producers/extents/terrain/hu-lechner-ddm.geojson")),
    ("hu-lechner-ddm.toml", include_bytes!("../../../../../producers/extents/terrain/hu-lechner-ddm.toml")),
    ("id-big-demnas.geojson", include_bytes!("../../../../../producers/extents/terrain/id-big-demnas.geojson")),
    ("id-big-demnas.toml", include_bytes!("../../../../../producers/extents/terrain/id-big-demnas.toml")),
    ("ie-gsi-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/ie-gsi-lidar.geojson")),
    ("ie-gsi-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/ie-gsi-lidar.toml")),
    ("ie-tailte-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/ie-tailte-dtm.geojson")),
    ("ie-tailte-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/ie-tailte-dtm.toml")),
    ("ign-lidar-hd.geojson", include_bytes!("../../../../../producers/extents/terrain/ign-lidar-hd.geojson")),
    ("ign-lidar-hd.toml", include_bytes!("../../../../../producers/extents/terrain/ign-lidar-hd.toml")),
    ("il-soi-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/il-soi-dtm.geojson")),
    ("il-soi-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/il-soi-dtm.toml")),
    ("im-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/im-lidar.geojson")),
    ("im-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/im-lidar.toml")),
    ("in-nrsc-cartodem.geojson", include_bytes!("../../../../../producers/extents/terrain/in-nrsc-cartodem.geojson")),
    ("in-nrsc-cartodem.toml", include_bytes!("../../../../../producers/extents/terrain/in-nrsc-cartodem.toml")),
    ("inegi-mde.geojson", include_bytes!("../../../../../producers/extents/terrain/inegi-mde.geojson")),
    ("inegi-mde.toml", include_bytes!("../../../../../producers/extents/terrain/inegi-mde.toml")),
    ("iowa-geotree.geojson", include_bytes!("../../../../../producers/extents/terrain/iowa-geotree.geojson")),
    ("iowa-geotree.toml", include_bytes!("../../../../../producers/extents/terrain/iowa-geotree.toml")),
    ("ir-ncc-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/ir-ncc-dem.geojson")),
    ("ir-ncc-dem.toml", include_bytes!("../../../../../producers/extents/terrain/ir-ncc-dem.toml")),
    ("is-islandsdem.geojson", include_bytes!("../../../../../producers/extents/terrain/is-islandsdem.geojson")),
    ("is-islandsdem.toml", include_bytes!("../../../../../producers/extents/terrain/is-islandsdem.toml")),
    ("it-bz-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/it-bz-dtm.geojson")),
    ("it-bz-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/it-bz-dtm.toml")),
    ("it-campania-dtm5.geojson", include_bytes!("../../../../../producers/extents/terrain/it-campania-dtm5.geojson")),
    ("it-campania-dtm5.toml", include_bytes!("../../../../../producers/extents/terrain/it-campania-dtm5.toml")),
    ("it-er-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-er-lidar.geojson")),
    ("it-er-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-er-lidar.toml")),
    ("it-fi-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-fi-lidar.geojson")),
    ("it-fi-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-fi-lidar.toml")),
    ("it-fvg-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-fvg-lidar.geojson")),
    ("it-fvg-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-fvg-lidar.toml")),
    ("it-lazio-dtm5.geojson", include_bytes!("../../../../../producers/extents/terrain/it-lazio-dtm5.geojson")),
    ("it-lazio-dtm5.toml", include_bytes!("../../../../../producers/extents/terrain/it-lazio-dtm5.toml")),
    ("it-liguria-dtm5.geojson", include_bytes!("../../../../../producers/extents/terrain/it-liguria-dtm5.geojson")),
    ("it-liguria-dtm5.toml", include_bytes!("../../../../../producers/extents/terrain/it-liguria-dtm5.toml")),
    ("it-lombardia-dtm5.geojson", include_bytes!("../../../../../producers/extents/terrain/it-lombardia-dtm5.geojson")),
    ("it-lombardia-dtm5.toml", include_bytes!("../../../../../producers/extents/terrain/it-lombardia-dtm5.toml")),
    ("it-marche-dtm20.geojson", include_bytes!("../../../../../producers/extents/terrain/it-marche-dtm20.geojson")),
    ("it-marche-dtm20.toml", include_bytes!("../../../../../producers/extents/terrain/it-marche-dtm20.toml")),
    ("it-piemonte-dtm5.geojson", include_bytes!("../../../../../producers/extents/terrain/it-piemonte-dtm5.geojson")),
    ("it-piemonte-dtm5.toml", include_bytes!("../../../../../producers/extents/terrain/it-piemonte-dtm5.toml")),
    ("it-pst-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-pst-lidar.geojson")),
    ("it-pst-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-pst-lidar.toml")),
    ("it-puglia-dtm8.geojson", include_bytes!("../../../../../producers/extents/terrain/it-puglia-dtm8.geojson")),
    ("it-puglia-dtm8.toml", include_bytes!("../../../../../producers/extents/terrain/it-puglia-dtm8.toml")),
    ("it-sardegna-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/it-sardegna-dtm.geojson")),
    ("it-sardegna-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/it-sardegna-dtm.toml")),
    ("it-sicilia-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/it-sicilia-mdt.geojson")),
    ("it-sicilia-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/it-sicilia-mdt.toml")),
    ("it-tinitaly.geojson", include_bytes!("../../../../../producers/extents/terrain/it-tinitaly.geojson")),
    ("it-tinitaly.toml", include_bytes!("../../../../../producers/extents/terrain/it-tinitaly.toml")),
    ("it-tn-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-tn-lidar.geojson")),
    ("it-tn-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-tn-lidar.toml")),
    ("it-toscana-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-toscana-lidar.geojson")),
    ("it-toscana-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-toscana-lidar.toml")),
    ("it-vda-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/it-vda-dtm.geojson")),
    ("it-vda-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/it-vda-dtm.toml")),
    ("it-veneto-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/it-veneto-lidar.geojson")),
    ("it-veneto-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/it-veneto-lidar.toml")),
    ("jaxa-aw3d30.geojson", include_bytes!("../../../../../producers/extents/terrain/jaxa-aw3d30.geojson")),
    ("jaxa-aw3d30.toml", include_bytes!("../../../../../producers/extents/terrain/jaxa-aw3d30.toml")),
    ("jp-ehime-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-ehime-dem.geojson")),
    ("jp-ehime-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-ehime-dem.toml")),
    ("jp-gifu-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-gifu-dem.geojson")),
    ("jp-gifu-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-gifu-dem.toml")),
    ("jp-gsi-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-gsi-dem.geojson")),
    ("jp-gsi-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-gsi-dem.toml")),
    ("jp-gsi-pointcloud.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-gsi-pointcloud.geojson")),
    ("jp-gsi-pointcloud.toml", include_bytes!("../../../../../producers/extents/terrain/jp-gsi-pointcloud.toml")),
    (
        "jp-hiroshima-dobox.geojson",
        include_bytes!("../../../../../producers/extents/terrain/jp-hiroshima-dobox.geojson"),
    ),
    ("jp-hiroshima-dobox.toml", include_bytes!("../../../../../producers/extents/terrain/jp-hiroshima-dobox.toml")),
    ("jp-hyogo-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-hyogo-lidar.geojson")),
    ("jp-hyogo-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-hyogo-lidar.toml")),
    (
        "jp-ishikawa-noto-ground.geojson",
        include_bytes!("../../../../../producers/extents/terrain/jp-ishikawa-noto-ground.geojson"),
    ),
    (
        "jp-ishikawa-noto-ground.toml",
        include_bytes!("../../../../../producers/extents/terrain/jp-ishikawa-noto-ground.toml"),
    ),
    ("jp-kanagawa-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-kanagawa-lidar.geojson")),
    ("jp-kanagawa-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-kanagawa-lidar.toml")),
    ("jp-kochi-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-kochi-dem.geojson")),
    ("jp-kochi-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-kochi-dem.toml")),
    ("jp-kyoto-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-kyoto-dem.geojson")),
    ("jp-kyoto-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-kyoto-dem.toml")),
    ("jp-miyagi-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-miyagi-dem.geojson")),
    ("jp-miyagi-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-miyagi-dem.toml")),
    ("jp-nagano-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-nagano-dem.geojson")),
    ("jp-nagano-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-nagano-dem.toml")),
    ("jp-nagasaki-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-nagasaki-lidar.geojson")),
    ("jp-nagasaki-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-nagasaki-lidar.toml")),
    ("jp-osaka-ground.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-osaka-ground.geojson")),
    ("jp-osaka-ground.toml", include_bytes!("../../../../../producers/extents/terrain/jp-osaka-ground.toml")),
    ("jp-rinya-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-rinya-dem.geojson")),
    ("jp-rinya-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-rinya-dem.toml")),
    ("jp-saitama-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-saitama-lidar.geojson")),
    ("jp-saitama-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-saitama-lidar.toml")),
    (
        "jp-shimane-river-dem.geojson",
        include_bytes!("../../../../../producers/extents/terrain/jp-shimane-river-dem.geojson"),
    ),
    ("jp-shimane-river-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-shimane-river-dem.toml")),
    ("jp-shizuoka-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-shizuoka-lidar.geojson")),
    ("jp-shizuoka-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-shizuoka-lidar.toml")),
    ("jp-tochigi-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-tochigi-dem.geojson")),
    ("jp-tochigi-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-tochigi-dem.toml")),
    ("jp-tokyo-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-tokyo-lidar.geojson")),
    ("jp-tokyo-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-tokyo-lidar.toml")),
    ("jp-tottori-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-tottori-dem.geojson")),
    ("jp-tottori-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-tottori-dem.toml")),
    ("jp-toyama-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-toyama-dem.geojson")),
    ("jp-toyama-dem.toml", include_bytes!("../../../../../producers/extents/terrain/jp-toyama-dem.toml")),
    ("jp-wakayama-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/jp-wakayama-lidar.geojson")),
    ("jp-wakayama-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-wakayama-lidar.toml")),
    (
        "jp-yamanashi-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/jp-yamanashi-lidar.geojson"),
    ),
    ("jp-yamanashi-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/jp-yamanashi-lidar.toml")),
    ("kr-ngii-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/kr-ngii-dem.geojson")),
    ("kr-ngii-dem.toml", include_bytes!("../../../../../producers/extents/terrain/kr-ngii-dem.toml")),
    ("lk-survey-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/lk-survey-lidar.geojson")),
    ("lk-survey-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/lk-survey-lidar.toml")),
    ("lt-nzt-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/lt-nzt-lidar.geojson")),
    ("lt-nzt-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/lt-nzt-lidar.toml")),
    ("luxembourg-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/luxembourg-lidar.geojson")),
    ("luxembourg-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/luxembourg-lidar.toml")),
    ("lv-lgia-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/lv-lgia-lidar.geojson")),
    ("lv-lgia-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/lv-lgia-lidar.toml")),
    ("lv-riga-3d.geojson", include_bytes!("../../../../../producers/extents/terrain/lv-riga-3d.geojson")),
    ("lv-riga-3d.toml", include_bytes!("../../../../../producers/extents/terrain/lv-riga-3d.toml")),
    ("md-agcc.geojson", include_bytes!("../../../../../producers/extents/terrain/md-agcc.geojson")),
    ("md-agcc.toml", include_bytes!("../../../../../producers/extents/terrain/md-agcc.toml")),
    ("merit-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/merit-dem.geojson")),
    ("merit-dem.toml", include_bytes!("../../../../../producers/extents/terrain/merit-dem.toml")),
    ("mh-usgs-majuro.geojson", include_bytes!("../../../../../producers/extents/terrain/mh-usgs-majuro.geojson")),
    ("mh-usgs-majuro.toml", include_bytes!("../../../../../producers/extents/terrain/mh-usgs-majuro.toml")),
    ("mk-akn.geojson", include_bytes!("../../../../../producers/extents/terrain/mk-akn.geojson")),
    ("mk-akn.toml", include_bytes!("../../../../../producers/extents/terrain/mk-akn.toml")),
    ("mt-pa-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/mt-pa-dtm.geojson")),
    ("mt-pa-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/mt-pa-dtm.toml")),
    ("my-jupem-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/my-jupem-dtm.geojson")),
    ("my-jupem-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/my-jupem-dtm.toml")),
    ("mz-ara-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/mz-ara-lidar.geojson")),
    ("mz-ara-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/mz-ara-lidar.toml")),
    ("nasa-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/nasa-dem.geojson")),
    ("nasa-dem.toml", include_bytes!("../../../../../producers/extents/terrain/nasa-dem.toml")),
    ("nc-dittt-bdalti.geojson", include_bytes!("../../../../../producers/extents/terrain/nc-dittt-bdalti.geojson")),
    ("nc-dittt-bdalti.toml", include_bytes!("../../../../../producers/extents/terrain/nc-dittt-bdalti.toml")),
    ("nf-csiro-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/nf-csiro-lidar.geojson")),
    ("nf-csiro-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/nf-csiro-lidar.toml")),
    ("nl-ahn.geojson", include_bytes!("../../../../../producers/extents/terrain/nl-ahn.geojson")),
    ("nl-ahn.toml", include_bytes!("../../../../../producers/extents/terrain/nl-ahn.toml")),
    ("no-hoydedata.geojson", include_bytes!("../../../../../producers/extents/terrain/no-hoydedata.geojson")),
    ("no-hoydedata.toml", include_bytes!("../../../../../producers/extents/terrain/no-hoydedata.toml")),
    ("novascotia-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/novascotia-lidar.geojson")),
    ("novascotia-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/novascotia-lidar.toml")),
    ("np-survey-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/np-survey-lidar.geojson")),
    ("np-survey-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/np-survey-lidar.toml")),
    ("npi-antarctic-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/npi-antarctic-dtm.geojson")),
    ("npi-antarctic-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/npi-antarctic-dtm.toml")),
    ("nrcan-hrdem.geojson", include_bytes!("../../../../../producers/extents/terrain/nrcan-hrdem.geojson")),
    ("nrcan-hrdem.toml", include_bytes!("../../../../../producers/extents/terrain/nrcan-hrdem.toml")),
    ("nrcan-mrdem.geojson", include_bytes!("../../../../../producers/extents/terrain/nrcan-mrdem.geojson")),
    ("nrcan-mrdem.toml", include_bytes!("../../../../../producers/extents/terrain/nrcan-mrdem.toml")),
    ("nuevoleon-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/nuevoleon-lidar.geojson")),
    ("nuevoleon-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/nuevoleon-lidar.toml")),
    ("nz-linz-coastal.geojson", include_bytes!("../../../../../producers/extents/terrain/nz-linz-coastal.geojson")),
    ("nz-linz-coastal.toml", include_bytes!("../../../../../producers/extents/terrain/nz-linz-coastal.toml")),
    ("nz-linz-elevation.geojson", include_bytes!("../../../../../producers/extents/terrain/nz-linz-elevation.geojson")),
    ("nz-linz-elevation.toml", include_bytes!("../../../../../producers/extents/terrain/nz-linz-elevation.toml")),
    ("oi-litto3d.geojson", include_bytes!("../../../../../producers/extents/terrain/oi-litto3d.geojson")),
    ("oi-litto3d.toml", include_bytes!("../../../../../producers/extents/terrain/oi-litto3d.toml")),
    ("om-nsa-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/om-nsa-dem.geojson")),
    ("om-nsa-dem.toml", include_bytes!("../../../../../producers/extents/terrain/om-nsa-dem.toml")),
    ("ontario-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/ontario-lidar.geojson")),
    ("ontario-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/ontario-lidar.toml")),
    ("opentopography.geojson", include_bytes!("../../../../../producers/extents/terrain/opentopography.geojson")),
    ("opentopography.toml", include_bytes!("../../../../../producers/extents/terrain/opentopography.toml")),
    ("pa-igntg-sar.geojson", include_bytes!("../../../../../producers/extents/terrain/pa-igntg-sar.geojson")),
    ("pa-igntg-sar.toml", include_bytes!("../../../../../producers/extents/terrain/pa-igntg-sar.toml")),
    ("paccsap-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/paccsap-lidar.geojson")),
    ("paccsap-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/paccsap-lidar.toml")),
    (
        "pacific-island-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/pacific-island-lidar.geojson"),
    ),
    ("pacific-island-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/pacific-island-lidar.toml")),
    (
        "pacific-national-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/pacific-national-lidar.geojson"),
    ),
    (
        "pacific-national-lidar.toml",
        include_bytes!("../../../../../producers/extents/terrain/pacific-national-lidar.toml"),
    ),
    ("pe-anin-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/pe-anin-lidar.geojson")),
    ("pe-anin-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/pe-anin-lidar.toml")),
    ("pf-sau-mnt.geojson", include_bytes!("../../../../../producers/extents/terrain/pf-sau-mnt.geojson")),
    ("pf-sau-mnt.toml", include_bytes!("../../../../../producers/extents/terrain/pf-sau-mnt.toml")),
    ("pgc-arcticdem.geojson", include_bytes!("../../../../../producers/extents/terrain/pgc-arcticdem.geojson")),
    ("pgc-arcticdem.toml", include_bytes!("../../../../../producers/extents/terrain/pgc-arcticdem.toml")),
    ("pgc-rema.geojson", include_bytes!("../../../../../producers/extents/terrain/pgc-rema.geojson")),
    ("pgc-rema.toml", include_bytes!("../../../../../producers/extents/terrain/pgc-rema.toml")),
    (
        "ph-namria-elevation.geojson",
        include_bytes!("../../../../../producers/extents/terrain/ph-namria-elevation.geojson"),
    ),
    ("ph-namria-elevation.toml", include_bytes!("../../../../../producers/extents/terrain/ph-namria-elevation.toml")),
    ("pl-gugik-nmt.geojson", include_bytes!("../../../../../producers/extents/terrain/pl-gugik-nmt.geojson")),
    ("pl-gugik-nmt.toml", include_bytes!("../../../../../producers/extents/terrain/pl-gugik-nmt.toml")),
    ("ps-mopd-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/ps-mopd-dem.geojson")),
    ("ps-mopd-dem.toml", include_bytes!("../../../../../producers/extents/terrain/ps-mopd-dem.toml")),
    ("pt-ac-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/pt-ac-lidar.geojson")),
    ("pt-ac-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/pt-ac-lidar.toml")),
    ("pt-dgt-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/pt-dgt-lidar.geojson")),
    ("pt-dgt-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/pt-dgt-lidar.toml")),
    ("pt-ma-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/pt-ma-mdt.geojson")),
    ("pt-ma-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/pt-ma-mdt.toml")),
    ("qa-cgis-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/qa-cgis-dem.geojson")),
    ("qa-cgis-dem.toml", include_bytes!("../../../../../producers/extents/terrain/qa-cgis-dem.toml")),
    ("quebec-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/quebec-lidar.geojson")),
    ("quebec-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/quebec-lidar.toml")),
    ("ro-ancpi.geojson", include_bytes!("../../../../../producers/extents/terrain/ro-ancpi.geojson")),
    ("ro-ancpi.toml", include_bytes!("../../../../../producers/extents/terrain/ro-ancpi.toml")),
    ("rs-rgz.geojson", include_bytes!("../../../../../producers/extents/terrain/rs-rgz.geojson")),
    ("rs-rgz.toml", include_bytes!("../../../../../producers/extents/terrain/rs-rgz.toml")),
    ("ru-ffpd.geojson", include_bytes!("../../../../../producers/extents/terrain/ru-ffpd.geojson")),
    ("ru-ffpd.toml", include_bytes!("../../../../../producers/extents/terrain/ru-ffpd.toml")),
    ("rw-nla-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/rw-nla-dem.geojson")),
    ("rw-nla-dem.toml", include_bytes!("../../../../../producers/extents/terrain/rw-nla-dem.toml")),
    ("sa-geosa-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/sa-geosa-dtm.geojson")),
    ("sa-geosa-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/sa-geosa-dtm.toml")),
    (
        "saskatchewan-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/saskatchewan-lidar.geojson"),
    ),
    ("saskatchewan-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/saskatchewan-lidar.toml")),
    (
        "se-goteborg-hojdmodell.geojson",
        include_bytes!("../../../../../producers/extents/terrain/se-goteborg-hojdmodell.geojson"),
    ),
    (
        "se-goteborg-hojdmodell.toml",
        include_bytes!("../../../../../producers/extents/terrain/se-goteborg-hojdmodell.toml"),
    ),
    ("se-markhojdmodell.geojson", include_bytes!("../../../../../producers/extents/terrain/se-markhojdmodell.geojson")),
    ("se-markhojdmodell.toml", include_bytes!("../../../../../producers/extents/terrain/se-markhojdmodell.toml")),
    ("se-stockholm-sbk.geojson", include_bytes!("../../../../../producers/extents/terrain/se-stockholm-sbk.geojson")),
    ("se-stockholm-sbk.toml", include_bytes!("../../../../../producers/extents/terrain/se-stockholm-sbk.toml")),
    ("se-ythojdmodell.geojson", include_bytes!("../../../../../producers/extents/terrain/se-ythojdmodell.geojson")),
    ("se-ythojdmodell.toml", include_bytes!("../../../../../producers/extents/terrain/se-ythojdmodell.toml")),
    ("sh-saeri-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/sh-saeri-dem.geojson")),
    ("sh-saeri-dem.toml", include_bytes!("../../../../../producers/extents/terrain/sh-saeri-dem.toml")),
    ("si-clss.geojson", include_bytes!("../../../../../producers/extents/terrain/si-clss.geojson")),
    ("si-clss.toml", include_bytes!("../../../../../producers/extents/terrain/si-clss.toml")),
    ("si-lss.geojson", include_bytes!("../../../../../producers/extents/terrain/si-lss.geojson")),
    ("si-lss.toml", include_bytes!("../../../../../producers/extents/terrain/si-lss.toml")),
    ("sj-npi-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/sj-npi-dtm.geojson")),
    ("sj-npi-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/sj-npi-dtm.toml")),
    ("sk-lls.geojson", include_bytes!("../../../../../producers/extents/terrain/sk-lls.geojson")),
    ("sk-lls.toml", include_bytes!("../../../../../producers/extents/terrain/sk-lls.toml")),
    ("sn-pgiis-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/sn-pgiis-lidar.geojson")),
    ("sn-pgiis-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/sn-pgiis-lidar.toml")),
    ("spm-litto3d.geojson", include_bytes!("../../../../../producers/extents/terrain/spm-litto3d.geojson")),
    ("spm-litto3d.toml", include_bytes!("../../../../../producers/extents/terrain/spm-litto3d.toml")),
    ("stkitts-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/stkitts-lidar.geojson")),
    ("stkitts-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/stkitts-lidar.toml")),
    ("stlucia-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/stlucia-lidar.geojson")),
    ("stlucia-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/stlucia-lidar.toml")),
    ("stvincent-usgs.geojson", include_bytes!("../../../../../producers/extents/terrain/stvincent-usgs.geojson")),
    ("stvincent-usgs.toml", include_bytes!("../../../../../producers/extents/terrain/stvincent-usgs.toml")),
    ("tci-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/tci-dem.geojson")),
    ("tci-dem.toml", include_bytes!("../../../../../producers/extents/terrain/tci-dem.toml")),
    ("th-ldd-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/th-ldd-dem.geojson")),
    ("th-ldd-dem.toml", include_bytes!("../../../../../producers/extents/terrain/th-ldd-dem.toml")),
    ("tr-hgm.geojson", include_bytes!("../../../../../producers/extents/terrain/tr-hgm.geojson")),
    ("tr-hgm.toml", include_bytes!("../../../../../producers/extents/terrain/tr-hgm.toml")),
    ("trinidad-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/trinidad-lidar.geojson")),
    ("trinidad-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/trinidad-lidar.toml")),
    ("tw-moi-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/tw-moi-dtm.geojson")),
    ("tw-moi-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/tw-moi-dtm.toml")),
    ("tw-taipei-dtm.geojson", include_bytes!("../../../../../producers/extents/terrain/tw-taipei-dtm.geojson")),
    ("tw-taipei-dtm.toml", include_bytes!("../../../../../producers/extents/terrain/tw-taipei-dtm.toml")),
    ("us-hi-dsm.geojson", include_bytes!("../../../../../producers/extents/terrain/us-hi-dsm.geojson")),
    ("us-hi-dsm.toml", include_bytes!("../../../../../producers/extents/terrain/us-hi-dsm.toml")),
    ("us-il-dsm.geojson", include_bytes!("../../../../../producers/extents/terrain/us-il-dsm.geojson")),
    ("us-il-dsm.toml", include_bytes!("../../../../../producers/extents/terrain/us-il-dsm.toml")),
    ("us-mt-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/us-mt-lidar.geojson")),
    ("us-mt-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/us-mt-lidar.toml")),
    ("us-neon-aop.geojson", include_bytes!("../../../../../producers/extents/terrain/us-neon-aop.geojson")),
    ("us-neon-aop.toml", include_bytes!("../../../../../producers/extents/terrain/us-neon-aop.toml")),
    ("us-ut-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/us-ut-lidar.geojson")),
    ("us-ut-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/us-ut-lidar.toml")),
    ("usgs-3dep.geojson", include_bytes!("../../../../../producers/extents/terrain/usgs-3dep.geojson")),
    ("usgs-3dep.toml", include_bytes!("../../../../../producers/extents/terrain/usgs-3dep.toml")),
    ("usiei.geojson", include_bytes!("../../../../../producers/extents/terrain/usiei.geojson")),
    ("usiei.toml", include_bytes!("../../../../../producers/extents/terrain/usiei.toml")),
    ("uy-ide-mdt.geojson", include_bytes!("../../../../../producers/extents/terrain/uy-ide-mdt.geojson")),
    ("uy-ide-mdt.toml", include_bytes!("../../../../../producers/extents/terrain/uy-ide-mdt.toml")),
    (
        "uy-montevideo-lidar.geojson",
        include_bytes!("../../../../../producers/extents/terrain/uy-montevideo-lidar.geojson"),
    ),
    ("uy-montevideo-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/uy-montevideo-lidar.toml")),
    ("uy-mtop-vergara.geojson", include_bytes!("../../../../../producers/extents/terrain/uy-mtop-vergara.geojson")),
    ("uy-mtop-vergara.toml", include_bytes!("../../../../../producers/extents/terrain/uy-mtop-vergara.toml")),
    ("wales-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/wales-lidar.geojson")),
    ("wales-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/wales-lidar.toml")),
    ("yukon-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/yukon-lidar.geojson")),
    ("yukon-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/yukon-lidar.toml")),
    ("za-capetown-glm.geojson", include_bytes!("../../../../../producers/extents/terrain/za-capetown-glm.geojson")),
    ("za-capetown-glm.toml", include_bytes!("../../../../../producers/extents/terrain/za-capetown-glm.toml")),
    ("za-joburg-lidar.geojson", include_bytes!("../../../../../producers/extents/terrain/za-joburg-lidar.geojson")),
    ("za-joburg-lidar.toml", include_bytes!("../../../../../producers/extents/terrain/za-joburg-lidar.toml")),
    ("za-kruger-ceda.geojson", include_bytes!("../../../../../producers/extents/terrain/za-kruger-ceda.geojson")),
    ("za-kruger-ceda.toml", include_bytes!("../../../../../producers/extents/terrain/za-kruger-ceda.toml")),
    ("za-ngi-dem.geojson", include_bytes!("../../../../../producers/extents/terrain/za-ngi-dem.geojson")),
    ("za-ngi-dem.toml", include_bytes!("../../../../../producers/extents/terrain/za-ngi-dem.toml")),
];

/// A theme's real files (producers/extents/<theme>/, `real`) checked: every file there listed (one
/// added to the folder fails this until it's listed), none with an error, each `.geojson` with
/// pieces and a box; the warnings. The files, by their paths.
fn real_files(theme: &str, real: &[(&str, &[u8])]) -> (BTreeMap<String, Vec<u8>>, Vec<String>) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../producers/extents").join(theme);
    let mut on_disk: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    on_disk.sort();
    let mut listed: Vec<String> = real.iter().map(|(n, _)| n.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
    let files: BTreeMap<String, Vec<u8>> = real.iter().map(|(n, b)| (format!("{theme}/{n}"), b.to_vec())).collect();
    let mut warnings = Vec::new();
    for (p, b) in &files {
        let c = Extents.file(p, b);
        let errors: Vec<&Finding> = c.findings.iter().filter(|f| f.level == Level::Error).collect();
        assert!(errors.is_empty(), "{p}: {errors:#?}");
        warnings.extend(c.findings.iter().filter(|f| f.level == Level::Warning).map(|f| f.message.clone()));
        if p.ends_with(".geojson") {
            assert!(c.facts["pieces"].as_u64().unwrap() > 0, "{p}");
            // (A source whose only pieces have no geometry in the file, of unknown extent (Belize's and
            // four more) or masks the build draws (Overture's buildings), has no box.)
            let no_geometry =
                c.facts["classes"].as_object().is_some_and(|k| k.keys().all(|k| k == "unknown" || k == "mask"));
            assert!(c.facts.contains_key("box") || no_geometry, "{p}");
        }
    }
    let read = |p: &str| -> Result<Arc<Vec<u8>>> { Ok(Arc::new(files[p].clone())) };
    let whole = Extents.whole(&version_of(&files, &read)).unwrap();
    let errors: Vec<&Finding> = whole.iter().filter(|f| f.level == Level::Error).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    warnings.extend(whole.iter().map(|f| f.message.clone()));
    (files, warnings)
}

#[test]
fn the_real_terrain_files_are_taken() {
    let (files, warnings) = real_files("terrain", &REAL);
    // (Where they lie isn't stated, or their area would overstate them: Belize's 17 %, Greater
    // Santo Domingo's 380 km², Dominica's, Guatemala's and Saint Kitts and Nevis's coverage; in
    // Europe the partial surveys with no outline and the restricted products whose coverage isn't
    // stated, docs/inputs.md §6.11's list.)
    let unknown = |(s, n): (&str, usize)| {
        format!(
            "terrain/{s}.geojson: {} of class unknown: listed in the sources, drawn nowhere",
            super::plural(n, "piece", "pieces")
        )
    };
    let expected = [
        ("aad-station-dem", 4),
        ("africa-local-dem", 5),
        ("at-ktn-als", 1),
        ("au-ga-surveys", 2),
        ("au-nsw-elevation", 1),
        ("au-vic-elevation", 1),
        ("au-wa-landgate", 1),
        ("ba-fgu", 1),
        ("ba-rs-rgurs", 1),
        ("belize-crip", 1),
        ("br-df-mdt", 1),
        ("br-dsg-radiografia", 2),
        ("br-ibge-mde", 2),
        ("br-mg-bh-lidar", 3),
        ("br-pe-pe3d", 1),
        ("br-rj-rio-mdt", 1),
        ("cl-ciren-dem", 1),
        ("cl-saf-lidar", 1),
        ("co-medellin-lidar", 2),
        ("cv-fogo-uav", 1),
        ("cz-brno-dmt", 1),
        ("de-hb-dgm1", 1),
        ("de-th-dgm1", 1),
        ("dominica-dvrp", 1),
        ("dominicanrep-sgn", 1),
        ("eg-sri-dem", 1),
        ("es-na-mde", 2),
        ("es-pnoa", 2),
        ("fi-helsinki-korkeusmalli", 1),
        ("gb-nir-daera-coastal", 6),
        ("gb-nir-hed-lidar", 1),
        ("gb-nir-roads-lidar", 3),
        ("gh-accra-lidar", 2),
        ("gr-ktimatologio", 1),
        ("gs-sggis-dem", 1),
        ("guatemala-ign", 1),
        ("ie-tailte-dtm", 1),
        ("in-nrsc-cartodem", 2),
        ("ir-ncc-dem", 1),
        ("it-bz-dtm", 5),
        ("it-sardegna-dtm", 2),
        ("it-toscana-lidar", 2),
        ("it-vda-dtm", 2),
        ("jp-gifu-dem", 1),
        ("jp-nagano-dem", 1),
        ("jp-nagasaki-lidar", 1),
        ("jp-osaka-ground", 1),
        ("jp-wakayama-lidar", 1),
        ("kr-ngii-dem", 1),
        ("lk-survey-lidar", 1),
        ("lv-lgia-lidar", 2),
        ("lv-riga-3d", 1),
        ("mh-usgs-majuro", 1),
        ("mk-akn", 2),
        ("my-jupem-dtm", 1),
        ("mz-ara-lidar", 2),
        ("nc-dittt-bdalti", 1),
        ("np-survey-lidar", 1),
        ("npi-antarctic-dtm", 1),
        ("om-nsa-dem", 2),
        ("pa-igntg-sar", 2),
        ("paccsap-lidar", 4),
        ("pacific-island-lidar", 5),
        ("pe-anin-lidar", 2),
        ("pf-sau-mnt", 3),
        ("ph-namria-elevation", 6),
        ("pl-gugik-nmt", 1),
        ("ps-mopd-dem", 1),
        ("qa-cgis-dem", 1),
        ("ro-ancpi", 2),
        ("rs-rgz", 2),
        ("ru-ffpd", 1),
        ("sa-geosa-dtm", 1),
        ("se-stockholm-sbk", 2),
        ("sj-npi-dtm", 2),
        ("sn-pgiis-lidar", 1),
        ("stkitts-lidar", 1),
        ("th-ldd-dem", 2),
        ("tr-hgm", 1),
        ("za-joburg-lidar", 1),
    ];
    assert_eq!(warnings, expected.map(unknown));
    // NRCan's 950 projects, each a DTM and a DSM, its mosaic and the CDSM.
    let c = Extents.file("terrain/nrcan-hrdem.geojson", &files["terrain/nrcan-hrdem.geojson"]);
    assert_eq!(c.facts["pieces"], 1902);
    assert_eq!(c.facts["classes"], serde_json::json!({"files": 2, "index": 1900}));
}

const REAL_BATHYMETRY: [(&str, &[u8]); 49] = [
    ("score.toml", include_bytes!("../../../../../producers/extents/bathymetry/score.toml")),
    ("au-ausbathytopo.geojson", include_bytes!("../../../../../producers/extents/bathymetry/au-ausbathytopo.geojson")),
    ("au-ausbathytopo.toml", include_bytes!("../../../../../producers/extents/bathymetry/au-ausbathytopo.toml")),
    ("au-ausseabed.geojson", include_bytes!("../../../../../producers/extents/bathymetry/au-ausseabed.geojson")),
    ("au-ausseabed.toml", include_bytes!("../../../../../producers/extents/bathymetry/au-ausseabed.toml")),
    ("ca-bc-lakes.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ca-bc-lakes.geojson")),
    ("ca-bc-lakes.toml", include_bytes!("../../../../../producers/extents/bathymetry/ca-bc-lakes.toml")),
    ("ca-chs-nonna.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ca-chs-nonna.geojson")),
    ("ca-chs-nonna.toml", include_bytes!("../../../../../producers/extents/bathymetry/ca-chs-nonna.toml")),
    ("ca-on-lakes.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ca-on-lakes.geojson")),
    ("ca-on-lakes.toml", include_bytes!("../../../../../producers/extents/bathymetry/ca-on-lakes.toml")),
    ("ca-qc-gblq.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ca-qc-gblq.geojson")),
    ("ca-qc-gblq.toml", include_bytes!("../../../../../producers/extents/bathymetry/ca-qc-gblq.toml")),
    ("de-bsh.geojson", include_bytes!("../../../../../producers/extents/bathymetry/de-bsh.geojson")),
    ("de-bsh.toml", include_bytes!("../../../../../producers/extents/bathymetry/de-bsh.toml")),
    ("dk-ddm.geojson", include_bytes!("../../../../../producers/extents/bathymetry/dk-ddm.geojson")),
    ("dk-ddm.toml", include_bytes!("../../../../../producers/extents/bathymetry/dk-ddm.toml")),
    ("emodnet-dtm.geojson", include_bytes!("../../../../../producers/extents/bathymetry/emodnet-dtm.geojson")),
    ("emodnet-dtm.toml", include_bytes!("../../../../../producers/extents/bathymetry/emodnet-dtm.toml")),
    ("emodnet-hr.geojson", include_bytes!("../../../../../producers/extents/bathymetry/emodnet-hr.geojson")),
    ("emodnet-hr.toml", include_bytes!("../../../../../producers/extents/bathymetry/emodnet-hr.toml")),
    ("fo-fodm.geojson", include_bytes!("../../../../../producers/extents/bathymetry/fo-fodm.geojson")),
    ("fo-fodm.toml", include_bytes!("../../../../../producers/extents/bathymetry/fo-fodm.toml")),
    ("fr-shom-homonim.geojson", include_bytes!("../../../../../producers/extents/bathymetry/fr-shom-homonim.geojson")),
    ("fr-shom-homonim.toml", include_bytes!("../../../../../producers/extents/bathymetry/fr-shom-homonim.toml")),
    ("fr-shom-litto3d.geojson", include_bytes!("../../../../../producers/extents/bathymetry/fr-shom-litto3d.geojson")),
    ("fr-shom-litto3d.toml", include_bytes!("../../../../../producers/extents/bathymetry/fr-shom-litto3d.toml")),
    ("gebco-2026.geojson", include_bytes!("../../../../../producers/extents/bathymetry/gebco-2026.geojson")),
    ("gebco-2026.toml", include_bytes!("../../../../../producers/extents/bathymetry/gebco-2026.toml")),
    ("gmrt.geojson", include_bytes!("../../../../../producers/extents/bathymetry/gmrt.geojson")),
    ("gmrt.toml", include_bytes!("../../../../../producers/extents/bathymetry/gmrt.toml")),
    ("ibcao-v5.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ibcao-v5.geojson")),
    ("ibcao-v5.toml", include_bytes!("../../../../../producers/extents/bathymetry/ibcao-v5.toml")),
    ("ibcso.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ibcso.geojson")),
    ("ibcso.toml", include_bytes!("../../../../../producers/extents/bathymetry/ibcso.toml")),
    ("ie-infomar.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ie-infomar.geojson")),
    ("ie-infomar.toml", include_bytes!("../../../../../producers/extents/bathymetry/ie-infomar.toml")),
    ("ncei-dems.geojson", include_bytes!("../../../../../producers/extents/bathymetry/ncei-dems.geojson")),
    ("ncei-dems.toml", include_bytes!("../../../../../producers/extents/bathymetry/ncei-dems.toml")),
    (
        "nl-rijkswaterstaat.geojson",
        include_bytes!("../../../../../producers/extents/bathymetry/nl-rijkswaterstaat.geojson"),
    ),
    ("nl-rijkswaterstaat.toml", include_bytes!("../../../../../producers/extents/bathymetry/nl-rijkswaterstaat.toml")),
    ("no-kartverket.geojson", include_bytes!("../../../../../producers/extents/bathymetry/no-kartverket.geojson")),
    ("no-kartverket.toml", include_bytes!("../../../../../producers/extents/bathymetry/no-kartverket.toml")),
    ("noaa-bluetopo.geojson", include_bytes!("../../../../../producers/extents/bathymetry/noaa-bluetopo.geojson")),
    ("noaa-bluetopo.toml", include_bytes!("../../../../../producers/extents/bathymetry/noaa-bluetopo.toml")),
    ("nz-linz.geojson", include_bytes!("../../../../../producers/extents/bathymetry/nz-linz.geojson")),
    ("nz-linz.toml", include_bytes!("../../../../../producers/extents/bathymetry/nz-linz.toml")),
    ("us-mn-dnr-lakes.geojson", include_bytes!("../../../../../producers/extents/bathymetry/us-mn-dnr-lakes.geojson")),
    ("us-mn-dnr-lakes.toml", include_bytes!("../../../../../producers/extents/bathymetry/us-mn-dnr-lakes.toml")),
];

#[test]
fn the_real_bathymetry_files_are_taken() {
    let (files, warnings) = real_files("bathymetry", &REAL_BATHYMETRY);
    assert_eq!(warnings, Vec::<String>::new());
    // GEBCO's pieces a kind of source its type identifier grid holds, each from the file's cells.
    let c = Extents.file("bathymetry/gebco-2026.geojson", &files["bathymetry/gebco-2026.geojson"]);
    assert_eq!(c.facts["classes"].as_object().unwrap().keys().collect::<Vec<_>>(), ["files"]);
    // Each national or local grid by what its extent stands on: the Faroes' and Denmark's source
    // grids, the German model's coverages, AusBathyTopo's type identifier grid and Shom's WMS layers
    // are the data's own (`files`); LINZ's depth models, a layer's bounding box, are claimed.
    let classes = |f: &str| {
        Extents.file(&format!("bathymetry/{f}.geojson"), &files[&format!("bathymetry/{f}.geojson")]).facts["classes"]
            .clone()
    };
    assert_eq!(classes("dk-ddm"), serde_json::json!({"files": 8}));
    assert_eq!(classes("de-bsh"), serde_json::json!({"files": 1}));
    assert_eq!(classes("fr-shom-homonim"), serde_json::json!({"files": 4}));
    assert_eq!(classes("nz-linz"), serde_json::json!({"claimed": 35, "index": 1}));
    assert_eq!(classes("nl-rijkswaterstaat"), serde_json::json!({"index": 1, "service": 1}));
    // The bathymetry's score: its guarantee holds, its primary the grid's spacing.
    let s = score(REAL_BATHYMETRY[0].1).unwrap();
    assert_eq!(s.primary, "spacing_m");
}

/// Malformed files are findings, never a panic: each kind's fixture cut short at every few bytes,
/// and values of the wrong type in its fields, through the file check and the whole version's.
const REAL_BUILDINGS: [(&str, &[u8]); 335] = [
    ("score.toml", include_bytes!("../../../../../producers/extents/buildings/score.toml")),
    ("3d-globfp.geojson", include_bytes!("../../../../../producers/extents/buildings/3d-globfp.geojson")),
    ("3d-globfp.toml", include_bytes!("../../../../../producers/extents/buildings/3d-globfp.toml")),
    (
        "ae-abudhabi-adsdi.geojson",
        include_bytes!("../../../../../producers/extents/buildings/ae-abudhabi-adsdi.geojson"),
    ),
    ("ae-abudhabi-adsdi.toml", include_bytes!("../../../../../producers/extents/buildings/ae-abudhabi-adsdi.toml")),
    ("ae-dubai-twin.geojson", include_bytes!("../../../../../producers/extents/buildings/ae-dubai-twin.geojson")),
    ("ae-dubai-twin.toml", include_bytes!("../../../../../producers/extents/buildings/ae-dubai-twin.toml")),
    ("africa-grid3.geojson", include_bytes!("../../../../../producers/extents/buildings/africa-grid3.geojson")),
    ("africa-grid3.toml", include_bytes!("../../../../../producers/extents/buildings/africa-grid3.toml")),
    ("ar-caba-tejido.geojson", include_bytes!("../../../../../producers/extents/buildings/ar-caba-tejido.geojson")),
    ("ar-caba-tejido.toml", include_bytes!("../../../../../producers/extents/buildings/ar-caba-tejido.toml")),
    ("at-bev-dlm.geojson", include_bytes!("../../../../../producers/extents/buildings/at-bev-dlm.geojson")),
    ("at-bev-dlm.toml", include_bytes!("../../../../../producers/extents/buildings/at-bev-dlm.toml")),
    ("at-graz-a106.geojson", include_bytes!("../../../../../producers/extents/buildings/at-graz-a106.geojson")),
    ("at-graz-a106.toml", include_bytes!("../../../../../producers/extents/buildings/at-graz-a106.toml")),
    (
        "at-tirol-gebaeude.geojson",
        include_bytes!("../../../../../producers/extents/buildings/at-tirol-gebaeude.geojson"),
    ),
    ("at-tirol-gebaeude.toml", include_bytes!("../../../../../producers/extents/buildings/at-tirol-gebaeude.toml")),
    (
        "at-wien-baukoerper.geojson",
        include_bytes!("../../../../../producers/extents/buildings/at-wien-baukoerper.geojson"),
    ),
    ("at-wien-baukoerper.toml", include_bytes!("../../../../../producers/extents/buildings/at-wien-baukoerper.toml")),
    ("au-adelaide-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/au-adelaide-3d.geojson")),
    ("au-adelaide-3d.toml", include_bytes!("../../../../../producers/extents/buildings/au-adelaide-3d.toml")),
    ("au-ballarat.geojson", include_bytes!("../../../../../producers/extents/buildings/au-ballarat.geojson")),
    ("au-ballarat.toml", include_bytes!("../../../../../producers/extents/buildings/au-ballarat.toml")),
    ("au-geelong.geojson", include_bytes!("../../../../../producers/extents/buildings/au-geelong.geojson")),
    ("au-geelong.toml", include_bytes!("../../../../../producers/extents/buildings/au-geelong.toml")),
    ("au-geoscape.geojson", include_bytes!("../../../../../producers/extents/buildings/au-geoscape.geojson")),
    ("au-geoscape.toml", include_bytes!("../../../../../producers/extents/buildings/au-geoscape.toml")),
    ("au-iot-outlines.geojson", include_bytes!("../../../../../producers/extents/buildings/au-iot-outlines.geojson")),
    ("au-iot-outlines.toml", include_bytes!("../../../../../producers/extents/buildings/au-iot-outlines.toml")),
    ("au-launceston.geojson", include_bytes!("../../../../../producers/extents/buildings/au-launceston.geojson")),
    ("au-launceston.toml", include_bytes!("../../../../../producers/extents/buildings/au-launceston.toml")),
    (
        "au-melbourne-footprints.geojson",
        include_bytes!("../../../../../producers/extents/buildings/au-melbourne-footprints.geojson"),
    ),
    (
        "au-melbourne-footprints.toml",
        include_bytes!("../../../../../producers/extents/buildings/au-melbourne-footprints.toml"),
    ),
    (
        "au-qld-lidar-outlines.geojson",
        include_bytes!("../../../../../producers/extents/buildings/au-qld-lidar-outlines.geojson"),
    ),
    (
        "au-qld-lidar-outlines.toml",
        include_bytes!("../../../../../producers/extents/buildings/au-qld-lidar-outlines.toml"),
    ),
    ("au-vicmap-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/au-vicmap-3d.geojson")),
    ("au-vicmap-3d.toml", include_bytes!("../../../../../producers/extents/buildings/au-vicmap-3d.toml")),
    ("be-antwerp-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/be-antwerp-3d.geojson")),
    ("be-antwerp-3d.toml", include_bytes!("../../../../../producers/extents/buildings/be-antwerp-3d.toml")),
    ("be-ghent-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/be-ghent-3d.geojson")),
    ("be-ghent-3d.toml", include_bytes!("../../../../../producers/extents/buildings/be-ghent-3d.toml")),
    ("be-namur-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/be-namur-3d.geojson")),
    ("be-namur-3d.toml", include_bytes!("../../../../../producers/extents/buildings/be-namur-3d.toml")),
    ("be-urbis-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/be-urbis-3d.geojson")),
    ("be-urbis-3d.toml", include_bytes!("../../../../../producers/extents/buildings/be-urbis-3d.toml")),
    ("be-vlg-3dgrb.geojson", include_bytes!("../../../../../producers/extents/buildings/be-vlg-3dgrb.geojson")),
    ("be-vlg-3dgrb.toml", include_bytes!("../../../../../producers/extents/buildings/be-vlg-3dgrb.toml")),
    ("be-wal-lod1.geojson", include_bytes!("../../../../../producers/extents/buildings/be-wal-lod1.geojson")),
    ("be-wal-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/be-wal-lod1.toml")),
    ("bg-cadastre.geojson", include_bytes!("../../../../../producers/extents/buildings/bg-cadastre.geojson")),
    ("bg-cadastre.toml", include_bytes!("../../../../../producers/extents/buildings/bg-cadastre.toml")),
    ("bh-slrb.geojson", include_bytes!("../../../../../producers/extents/buildings/bh-slrb.geojson")),
    ("bh-slrb.toml", include_bytes!("../../../../../producers/extents/buildings/bh-slrb.toml")),
    (
        "br-bh-edificacao-3d.geojson",
        include_bytes!("../../../../../producers/extents/buildings/br-bh-edificacao-3d.geojson"),
    ),
    ("br-bh-edificacao-3d.toml", include_bytes!("../../../../../producers/extents/buildings/br-bh-edificacao-3d.toml")),
    ("br-df-edificacao.geojson", include_bytes!("../../../../../producers/extents/buildings/br-df-edificacao.geojson")),
    ("br-df-edificacao.toml", include_bytes!("../../../../../producers/extents/buildings/br-df-edificacao.toml")),
    (
        "br-rio-edificacoes.geojson",
        include_bytes!("../../../../../producers/extents/buildings/br-rio-edificacoes.geojson"),
    ),
    ("br-rio-edificacoes.toml", include_bytes!("../../../../../producers/extents/buildings/br-rio-edificacoes.toml")),
    ("br-sp-geosampa.geojson", include_bytes!("../../../../../producers/extents/buildings/br-sp-geosampa.geojson")),
    ("br-sp-geosampa.toml", include_bytes!("../../../../../producers/extents/buildings/br-sp-geosampa.toml")),
    ("ca-burnaby.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-burnaby.geojson")),
    ("ca-burnaby.toml", include_bytes!("../../../../../producers/extents/buildings/ca-burnaby.toml")),
    ("ca-calgary-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-calgary-3d.geojson")),
    ("ca-calgary-3d.toml", include_bytes!("../../../../../producers/extents/buildings/ca-calgary-3d.toml")),
    ("ca-edmonton-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-edmonton-3d.geojson")),
    ("ca-edmonton-3d.toml", include_bytes!("../../../../../producers/extents/buildings/ca-edmonton-3d.toml")),
    (
        "ca-geonb-buildings.geojson",
        include_bytes!("../../../../../producers/extents/buildings/ca-geonb-buildings.geojson"),
    ),
    ("ca-geonb-buildings.toml", include_bytes!("../../../../../producers/extents/buildings/ca-geonb-buildings.toml")),
    ("ca-kelowna.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-kelowna.geojson")),
    ("ca-kelowna.toml", include_bytes!("../../../../../producers/extents/buildings/ca-kelowna.toml")),
    ("ca-montreal-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-montreal-3d.geojson")),
    ("ca-montreal-3d.toml", include_bytes!("../../../../../producers/extents/buildings/ca-montreal-3d.toml")),
    ("ca-nrcan-aeb.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-nrcan-aeb.geojson")),
    ("ca-nrcan-aeb.toml", include_bytes!("../../../../../producers/extents/buildings/ca-nrcan-aeb.toml")),
    ("ca-ottawa-lod1.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-ottawa-lod1.geojson")),
    ("ca-ottawa-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/ca-ottawa-lod1.toml")),
    ("ca-qc-batiments.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-qc-batiments.geojson")),
    ("ca-qc-batiments.toml", include_bytes!("../../../../../producers/extents/buildings/ca-qc-batiments.toml")),
    ("ca-surrey.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-surrey.geojson")),
    ("ca-surrey.toml", include_bytes!("../../../../../producers/extents/buildings/ca-surrey.toml")),
    ("ca-toronto-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-toronto-3d.geojson")),
    ("ca-toronto-3d.toml", include_bytes!("../../../../../producers/extents/buildings/ca-toronto-3d.toml")),
    ("ca-vancouver.geojson", include_bytes!("../../../../../producers/extents/buildings/ca-vancouver.geojson")),
    ("ca-vancouver.toml", include_bytes!("../../../../../producers/extents/buildings/ca-vancouver.toml")),
    ("ch-be-hoehe.geojson", include_bytes!("../../../../../producers/extents/buildings/ch-be-hoehe.geojson")),
    ("ch-be-hoehe.toml", include_bytes!("../../../../../producers/extents/buildings/ch-be-hoehe.toml")),
    ("ch-ge-buildings.geojson", include_bytes!("../../../../../producers/extents/buildings/ch-ge-buildings.geojson")),
    ("ch-ge-buildings.toml", include_bytes!("../../../../../producers/extents/buildings/ch-ge-buildings.toml")),
    ("ch-gwr.geojson", include_bytes!("../../../../../producers/extents/buildings/ch-gwr.geojson")),
    ("ch-gwr.toml", include_bytes!("../../../../../producers/extents/buildings/ch-gwr.toml")),
    (
        "ch-swissbuildings3d.geojson",
        include_bytes!("../../../../../producers/extents/buildings/ch-swissbuildings3d.geojson"),
    ),
    ("ch-swissbuildings3d.toml", include_bytes!("../../../../../producers/extents/buildings/ch-swissbuildings3d.toml")),
    (
        "ch-zurich-dachmodell.geojson",
        include_bytes!("../../../../../producers/extents/buildings/ch-zurich-dachmodell.geojson"),
    ),
    (
        "ch-zurich-dachmodell.toml",
        include_bytes!("../../../../../producers/extents/buildings/ch-zurich-dachmodell.toml"),
    ),
    ("cn-cmab.geojson", include_bytes!("../../../../../producers/extents/buildings/cn-cmab.geojson")),
    ("cn-cmab.toml", include_bytes!("../../../../../producers/extents/buildings/cn-cmab.toml")),
    (
        "co-bogota-construccion.geojson",
        include_bytes!("../../../../../producers/extents/buildings/co-bogota-construccion.geojson"),
    ),
    (
        "co-bogota-construccion.toml",
        include_bytes!("../../../../../producers/extents/buildings/co-bogota-construccion.toml"),
    ),
    ("cy-dls-bu3.geojson", include_bytes!("../../../../../producers/extents/buildings/cy-dls-bu3.geojson")),
    ("cy-dls-bu3.toml", include_bytes!("../../../../../producers/extents/buildings/cy-dls-bu3.toml")),
    ("cz-brno-vyska.geojson", include_bytes!("../../../../../producers/extents/buildings/cz-brno-vyska.geojson")),
    ("cz-brno-vyska.toml", include_bytes!("../../../../../producers/extents/buildings/cz-brno-vyska.toml")),
    (
        "cz-praha-podlaznosti.geojson",
        include_bytes!("../../../../../producers/extents/buildings/cz-praha-podlaznosti.geojson"),
    ),
    (
        "cz-praha-podlaznosti.toml",
        include_bytes!("../../../../../producers/extents/buildings/cz-praha-podlaznosti.toml"),
    ),
    ("cz-ruian.geojson", include_bytes!("../../../../../producers/extents/buildings/cz-ruian.geojson")),
    ("cz-ruian.toml", include_bytes!("../../../../../producers/extents/buildings/cz-ruian.toml")),
    ("de-bb-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-bb-lod2.geojson")),
    ("de-bb-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-bb-lod2.toml")),
    ("de-be-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-be-lod2.geojson")),
    ("de-be-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-be-lod2.toml")),
    ("de-bkg-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-bkg-lod2.geojson")),
    ("de-bkg-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-bkg-lod2.toml")),
    ("de-bw-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-bw-lod2.geojson")),
    ("de-bw-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-bw-lod2.toml")),
    ("de-by-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-by-lod2.geojson")),
    ("de-by-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-by-lod2.toml")),
    ("de-ethos-builda.geojson", include_bytes!("../../../../../producers/extents/buildings/de-ethos-builda.geojson")),
    ("de-ethos-builda.toml", include_bytes!("../../../../../producers/extents/buildings/de-ethos-builda.toml")),
    ("de-hb-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-hb-lod2.geojson")),
    ("de-hb-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-hb-lod2.toml")),
    ("de-he-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-he-lod2.geojson")),
    ("de-he-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-he-lod2.toml")),
    ("de-hh-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-hh-lod2.geojson")),
    ("de-hh-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-hh-lod2.toml")),
    ("de-mv-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-mv-lod2.geojson")),
    ("de-mv-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-mv-lod2.toml")),
    ("de-ni-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-ni-lod2.geojson")),
    ("de-ni-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-ni-lod2.toml")),
    ("de-nw-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-nw-lod2.geojson")),
    ("de-nw-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-nw-lod2.toml")),
    ("de-rp-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-rp-lod2.geojson")),
    ("de-rp-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-rp-lod2.toml")),
    ("de-sh-gebaeude.geojson", include_bytes!("../../../../../producers/extents/buildings/de-sh-gebaeude.geojson")),
    ("de-sh-gebaeude.toml", include_bytes!("../../../../../producers/extents/buildings/de-sh-gebaeude.toml")),
    ("de-sl-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-sl-lod2.geojson")),
    ("de-sl-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-sl-lod2.toml")),
    ("de-sn-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-sn-lod2.geojson")),
    ("de-sn-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-sn-lod2.toml")),
    ("de-st-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-st-lod2.geojson")),
    ("de-st-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-st-lod2.toml")),
    ("de-th-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/de-th-lod2.geojson")),
    ("de-th-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/de-th-lod2.toml")),
    ("dk-bbr.geojson", include_bytes!("../../../../../producers/extents/buildings/dk-bbr.geojson")),
    ("dk-bbr.toml", include_bytes!("../../../../../producers/extents/buildings/dk-bbr.toml")),
    ("ee-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/ee-lod2.geojson")),
    ("ee-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/ee-lod2.toml")),
    ("es-bizkaia.geojson", include_bytes!("../../../../../producers/extents/buildings/es-bizkaia.geojson")),
    ("es-bizkaia.toml", include_bytes!("../../../../../producers/extents/buildings/es-bizkaia.toml")),
    ("es-catastro.geojson", include_bytes!("../../../../../producers/extents/buildings/es-catastro.geojson")),
    ("es-catastro.toml", include_bytes!("../../../../../producers/extents/buildings/es-catastro.toml")),
    ("es-ct-rtt.geojson", include_bytes!("../../../../../producers/extents/buildings/es-ct-rtt.geojson")),
    ("es-ct-rtt.toml", include_bytes!("../../../../../producers/extents/buildings/es-ct-rtt.toml")),
    ("es-gipuzkoa.geojson", include_bytes!("../../../../../producers/extents/buildings/es-gipuzkoa.geojson")),
    ("es-gipuzkoa.toml", include_bytes!("../../../../../producers/extents/buildings/es-gipuzkoa.toml")),
    ("es-madrid-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/es-madrid-3d.geojson")),
    ("es-madrid-3d.toml", include_bytes!("../../../../../producers/extents/buildings/es-madrid-3d.toml")),
    ("es-navarra.geojson", include_bytes!("../../../../../producers/extents/buildings/es-navarra.geojson")),
    ("es-navarra.toml", include_bytes!("../../../../../producers/extents/buildings/es-navarra.toml")),
    ("fi-espoo-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/fi-espoo-3d.geojson")),
    ("fi-espoo-3d.toml", include_bytes!("../../../../../producers/extents/buildings/fi-espoo-3d.toml")),
    ("fi-helsinki-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/fi-helsinki-3d.geojson")),
    ("fi-helsinki-3d.toml", include_bytes!("../../../../../producers/extents/buildings/fi-helsinki-3d.toml")),
    (
        "fi-maastotietokanta.geojson",
        include_bytes!("../../../../../producers/extents/buildings/fi-maastotietokanta.geojson"),
    ),
    ("fi-maastotietokanta.toml", include_bytes!("../../../../../producers/extents/buildings/fi-maastotietokanta.toml")),
    (
        "fr-bdtopo-antilles-guyane.geojson",
        include_bytes!("../../../../../producers/extents/buildings/fr-bdtopo-antilles-guyane.geojson"),
    ),
    (
        "fr-bdtopo-antilles-guyane.toml",
        include_bytes!("../../../../../producers/extents/buildings/fr-bdtopo-antilles-guyane.toml"),
    ),
    (
        "fr-bdtopo-reunion-mayotte.geojson",
        include_bytes!("../../../../../producers/extents/buildings/fr-bdtopo-reunion-mayotte.geojson"),
    ),
    (
        "fr-bdtopo-reunion-mayotte.toml",
        include_bytes!("../../../../../producers/extents/buildings/fr-bdtopo-reunion-mayotte.toml"),
    ),
    ("fr-bdtopo.geojson", include_bytes!("../../../../../producers/extents/buildings/fr-bdtopo.geojson")),
    ("fr-bdtopo.toml", include_bytes!("../../../../../producers/extents/buildings/fr-bdtopo.toml")),
    ("fr-city-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/fr-city-3d.geojson")),
    ("fr-city-3d.toml", include_bytes!("../../../../../producers/extents/buildings/fr-city-3d.toml")),
    ("fr-paris-volumes.geojson", include_bytes!("../../../../../producers/extents/buildings/fr-paris-volumes.geojson")),
    ("fr-paris-volumes.toml", include_bytes!("../../../../../producers/extents/buildings/fr-paris-volumes.toml")),
    ("gb-glasgow-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/gb-glasgow-3d.geojson")),
    ("gb-glasgow-3d.toml", include_bytes!("../../../../../producers/extents/buildings/gb-glasgow-3d.toml")),
    ("gb-london-bsm.geojson", include_bytes!("../../../../../producers/extents/buildings/gb-london-bsm.geojson")),
    ("gb-london-bsm.toml", include_bytes!("../../../../../producers/extents/buildings/gb-london-bsm.toml")),
    ("gb-os-bha.geojson", include_bytes!("../../../../../producers/extents/buildings/gb-os-bha.geojson")),
    ("gb-os-bha.toml", include_bytes!("../../../../../producers/extents/buildings/gb-os-bha.toml")),
    ("ghs-obat.geojson", include_bytes!("../../../../../producers/extents/buildings/ghs-obat.geojson")),
    ("ghs-obat.toml", include_bytes!("../../../../../producers/extents/buildings/ghs-obat.toml")),
    (
        "globalbuildingatlas.geojson",
        include_bytes!("../../../../../producers/extents/buildings/globalbuildingatlas.geojson"),
    ),
    ("globalbuildingatlas.toml", include_bytes!("../../../../../producers/extents/buildings/globalbuildingatlas.toml")),
    (
        "hk-landsd-buildings.geojson",
        include_bytes!("../../../../../producers/extents/buildings/hk-landsd-buildings.geojson"),
    ),
    ("hk-landsd-buildings.toml", include_bytes!("../../../../../producers/extents/buildings/hk-landsd-buildings.toml")),
    ("hr-zagreb-zg3d.geojson", include_bytes!("../../../../../producers/extents/buildings/hr-zagreb-zg3d.geojson")),
    ("hr-zagreb-zg3d.toml", include_bytes!("../../../../../producers/extents/buildings/hr-zagreb-zg3d.toml")),
    ("id-balikpapan.geojson", include_bytes!("../../../../../producers/extents/buildings/id-balikpapan.geojson")),
    ("id-balikpapan.toml", include_bytes!("../../../../../producers/extents/buildings/id-balikpapan.toml")),
    ("id-big-lod1.geojson", include_bytes!("../../../../../producers/extents/buildings/id-big-lod1.geojson")),
    ("id-big-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/id-big-lod1.toml")),
    ("id-jakarta.geojson", include_bytes!("../../../../../producers/extents/buildings/id-jakarta.geojson")),
    ("id-jakarta.toml", include_bytes!("../../../../../producers/extents/buildings/id-jakarta.toml")),
    ("ie-geodirectory.geojson", include_bytes!("../../../../../producers/extents/buildings/ie-geodirectory.geojson")),
    ("ie-geodirectory.toml", include_bytes!("../../../../../producers/extents/buildings/ie-geodirectory.toml")),
    ("il-beer-sheva.geojson", include_bytes!("../../../../../producers/extents/buildings/il-beer-sheva.geojson")),
    ("il-beer-sheva.toml", include_bytes!("../../../../../producers/extents/buildings/il-beer-sheva.toml")),
    ("in-gobs.geojson", include_bytes!("../../../../../producers/extents/buildings/in-gobs.geojson")),
    ("in-gobs.toml", include_bytes!("../../../../../producers/extents/buildings/in-gobs.toml")),
    ("it-bologna.geojson", include_bytes!("../../../../../producers/extents/buildings/it-bologna.geojson")),
    ("it-bologna.toml", include_bytes!("../../../../../producers/extents/buildings/it-bologna.toml")),
    ("it-ferrara.geojson", include_bytes!("../../../../../producers/extents/buildings/it-ferrara.geojson")),
    ("it-ferrara.toml", include_bytes!("../../../../../producers/extents/buildings/it-ferrara.toml")),
    ("it-milano-dbt.geojson", include_bytes!("../../../../../producers/extents/buildings/it-milano-dbt.geojson")),
    ("it-milano-dbt.toml", include_bytes!("../../../../../producers/extents/buildings/it-milano-dbt.toml")),
    ("it-sardegna-dbgt.geojson", include_bytes!("../../../../../producers/extents/buildings/it-sardegna-dbgt.geojson")),
    ("it-sardegna-dbgt.toml", include_bytes!("../../../../../producers/extents/buildings/it-sardegna-dbgt.toml")),
    ("it-sondrio-lecco.geojson", include_bytes!("../../../../../producers/extents/buildings/it-sondrio-lecco.geojson")),
    ("it-sondrio-lecco.toml", include_bytes!("../../../../../producers/extents/buildings/it-sondrio-lecco.toml")),
    ("jp-plateau.geojson", include_bytes!("../../../../../producers/extents/buildings/jp-plateau.geojson")),
    ("jp-plateau.toml", include_bytes!("../../../../../producers/extents/buildings/jp-plateau.toml")),
    ("kr-gwacheon.geojson", include_bytes!("../../../../../producers/extents/buildings/kr-gwacheon.geojson")),
    ("kr-gwacheon.toml", include_bytes!("../../../../../producers/extents/buildings/kr-gwacheon.toml")),
    (
        "kr-national-buildings.geojson",
        include_bytes!("../../../../../producers/extents/buildings/kr-national-buildings.geojson"),
    ),
    (
        "kr-national-buildings.toml",
        include_bytes!("../../../../../producers/extents/buildings/kr-national-buildings.toml"),
    ),
    ("kr-seoul-solar.geojson", include_bytes!("../../../../../producers/extents/buildings/kr-seoul-solar.geojson")),
    ("kr-seoul-solar.toml", include_bytes!("../../../../../producers/extents/buildings/kr-seoul-solar.toml")),
    ("lu-act-bati3d.geojson", include_bytes!("../../../../../producers/extents/buildings/lu-act-bati3d.geojson")),
    ("lu-act-bati3d.toml", include_bytes!("../../../../../producers/extents/buildings/lu-act-bati3d.toml")),
    ("lv-ekas.geojson", include_bytes!("../../../../../producers/extents/buildings/lv-ekas.geojson")),
    ("lv-ekas.toml", include_bytes!("../../../../../producers/extents/buildings/lv-ekas.toml")),
    ("lv-riga-lod1.geojson", include_bytes!("../../../../../producers/extents/buildings/lv-riga-lod1.geojson")),
    ("lv-riga-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/lv-riga-lod1.toml")),
    ("mx-cdmx-niveles.geojson", include_bytes!("../../../../../producers/extents/buildings/mx-cdmx-niveles.geojson")),
    ("mx-cdmx-niveles.toml", include_bytes!("../../../../../producers/extents/buildings/mx-cdmx-niveles.toml")),
    ("mx-guadalajara.geojson", include_bytes!("../../../../../producers/extents/buildings/mx-guadalajara.geojson")),
    ("mx-guadalajara.toml", include_bytes!("../../../../../producers/extents/buildings/mx-guadalajara.toml")),
    ("nl-3dbag.geojson", include_bytes!("../../../../../producers/extents/buildings/nl-3dbag.geojson")),
    ("nl-3dbag.toml", include_bytes!("../../../../../producers/extents/buildings/nl-3dbag.toml")),
    ("nl-rotterdam-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/nl-rotterdam-3d.geojson")),
    ("nl-rotterdam-3d.toml", include_bytes!("../../../../../producers/extents/buildings/nl-rotterdam-3d.toml")),
    ("no-matrikkelen.geojson", include_bytes!("../../../../../producers/extents/buildings/no-matrikkelen.geojson")),
    ("no-matrikkelen.toml", include_bytes!("../../../../../producers/extents/buildings/no-matrikkelen.toml")),
    ("nz-christchurch.geojson", include_bytes!("../../../../../producers/extents/buildings/nz-christchurch.geojson")),
    ("nz-christchurch.toml", include_bytes!("../../../../../producers/extents/buildings/nz-christchurch.toml")),
    ("nz-linz-outlines.geojson", include_bytes!("../../../../../producers/extents/buildings/nz-linz-outlines.geojson")),
    ("nz-linz-outlines.toml", include_bytes!("../../../../../producers/extents/buildings/nz-linz-outlines.toml")),
    ("nz-queenstown-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/nz-queenstown-3d.geojson")),
    ("nz-queenstown-3d.toml", include_bytes!("../../../../../producers/extents/buildings/nz-queenstown-3d.toml")),
    ("nz-wellington-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/nz-wellington-3d.geojson")),
    ("nz-wellington-3d.toml", include_bytes!("../../../../../producers/extents/buildings/nz-wellington-3d.toml")),
    (
        "open-buildings-25d.geojson",
        include_bytes!("../../../../../producers/extents/buildings/open-buildings-25d.geojson"),
    ),
    ("open-buildings-25d.toml", include_bytes!("../../../../../producers/extents/buildings/open-buildings-25d.toml")),
    ("overture.geojson", include_bytes!("../../../../../producers/extents/buildings/overture.geojson")),
    ("overture.toml", include_bytes!("../../../../../producers/extents/buildings/overture.toml")),
    (
        "pacific-footprints.geojson",
        include_bytes!("../../../../../producers/extents/buildings/pacific-footprints.geojson"),
    ),
    ("pacific-footprints.toml", include_bytes!("../../../../../producers/extents/buildings/pacific-footprints.toml")),
    ("pl-gugik-lod2.geojson", include_bytes!("../../../../../producers/extents/buildings/pl-gugik-lod2.geojson")),
    ("pl-gugik-lod2.toml", include_bytes!("../../../../../producers/extents/buildings/pl-gugik-lod2.toml")),
    ("rw-eo4kigali.geojson", include_bytes!("../../../../../producers/extents/buildings/rw-eo4kigali.geojson")),
    ("rw-eo4kigali.toml", include_bytes!("../../../../../producers/extents/buildings/rw-eo4kigali.toml")),
    ("se-byggnad.geojson", include_bytes!("../../../../../producers/extents/buildings/se-byggnad.geojson")),
    ("se-byggnad.toml", include_bytes!("../../../../../producers/extents/buildings/se-byggnad.toml")),
    (
        "se-stockholm-lod1.geojson",
        include_bytes!("../../../../../producers/extents/buildings/se-stockholm-lod1.geojson"),
    ),
    ("se-stockholm-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/se-stockholm-lod1.toml")),
    ("se-umea-lod1.geojson", include_bytes!("../../../../../producers/extents/buildings/se-umea-lod1.geojson")),
    ("se-umea-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/se-umea-lod1.toml")),
    ("sg-hdb.geojson", include_bytes!("../../../../../producers/extents/buildings/sg-hdb.geojson")),
    ("sg-hdb.toml", include_bytes!("../../../../../producers/extents/buildings/sg-hdb.toml")),
    (
        "si-kataster-stavbe.geojson",
        include_bytes!("../../../../../producers/extents/buildings/si-kataster-stavbe.geojson"),
    ),
    ("si-kataster-stavbe.toml", include_bytes!("../../../../../producers/extents/buildings/si-kataster-stavbe.toml")),
    ("sk-zbgis.geojson", include_bytes!("../../../../../producers/extents/buildings/sk-zbgis.geojson")),
    ("sk-zbgis.toml", include_bytes!("../../../../../producers/extents/buildings/sk-zbgis.toml")),
    ("tw-nlsc-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/tw-nlsc-3d.geojson")),
    ("tw-nlsc-3d.toml", include_bytes!("../../../../../producers/extents/buildings/tw-nlsc-3d.toml")),
    ("tw-taichung.geojson", include_bytes!("../../../../../producers/extents/buildings/tw-taichung.geojson")),
    ("tw-taichung.toml", include_bytes!("../../../../../producers/extents/buildings/tw-taichung.toml")),
    (
        "tw-taipei-permits.geojson",
        include_bytes!("../../../../../producers/extents/buildings/tw-taipei-permits.geojson"),
    ),
    ("tw-taipei-permits.toml", include_bytes!("../../../../../producers/extents/buildings/tw-taipei-permits.toml")),
    ("us-boston-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/us-boston-3d.geojson")),
    ("us-boston-3d.toml", include_bytes!("../../../../../producers/extents/buildings/us-boston-3d.toml")),
    ("us-cambridge-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/us-cambridge-3d.geojson")),
    ("us-cambridge-3d.toml", include_bytes!("../../../../../producers/extents/buildings/us-cambridge-3d.toml")),
    ("us-chattanooga.geojson", include_bytes!("../../../../../producers/extents/buildings/us-chattanooga.geojson")),
    ("us-chattanooga.toml", include_bytes!("../../../../../producers/extents/buildings/us-chattanooga.toml")),
    ("us-ct-footprints.geojson", include_bytes!("../../../../../producers/extents/buildings/us-ct-footprints.geojson")),
    ("us-ct-footprints.toml", include_bytes!("../../../../../producers/extents/buildings/us-ct-footprints.toml")),
    ("us-dallas.geojson", include_bytes!("../../../../../producers/extents/buildings/us-dallas.geojson")),
    ("us-dallas.toml", include_bytes!("../../../../../producers/extents/buildings/us-dallas.toml")),
    ("us-dc-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/us-dc-3d.geojson")),
    ("us-dc-3d.toml", include_bytes!("../../../../../producers/extents/buildings/us-dc-3d.toml")),
    ("us-denver.geojson", include_bytes!("../../../../../producers/extents/buildings/us-denver.geojson")),
    ("us-denver.toml", include_bytes!("../../../../../producers/extents/buildings/us-denver.toml")),
    ("us-detroit.geojson", include_bytes!("../../../../../producers/extents/buildings/us-detroit.geojson")),
    ("us-detroit.toml", include_bytes!("../../../../../producers/extents/buildings/us-detroit.toml")),
    ("us-hamilton-oh.geojson", include_bytes!("../../../../../producers/extents/buildings/us-hamilton-oh.geojson")),
    ("us-hamilton-oh.toml", include_bytes!("../../../../../producers/extents/buildings/us-hamilton-oh.toml")),
    ("us-houston.geojson", include_bytes!("../../../../../producers/extents/buildings/us-houston.geojson")),
    ("us-houston.toml", include_bytes!("../../../../../producers/extents/buildings/us-houston.toml")),
    ("us-jefferson-co.geojson", include_bytes!("../../../../../producers/extents/buildings/us-jefferson-co.geojson")),
    ("us-jefferson-co.toml", include_bytes!("../../../../../producers/extents/buildings/us-jefferson-co.toml")),
    (
        "us-la-county-lariac.geojson",
        include_bytes!("../../../../../producers/extents/buildings/us-la-county-lariac.geojson"),
    ),
    ("us-la-county-lariac.toml", include_bytes!("../../../../../producers/extents/buildings/us-la-county-lariac.toml")),
    ("us-louisville.geojson", include_bytes!("../../../../../producers/extents/buildings/us-louisville.geojson")),
    ("us-louisville.toml", include_bytes!("../../../../../producers/extents/buildings/us-louisville.toml")),
    ("us-miami-dade.geojson", include_bytes!("../../../../../producers/extents/buildings/us-miami-dade.geojson")),
    ("us-miami-dade.toml", include_bytes!("../../../../../producers/extents/buildings/us-miami-dade.toml")),
    ("us-nashville.geojson", include_bytes!("../../../../../producers/extents/buildings/us-nashville.geojson")),
    ("us-nashville.toml", include_bytes!("../../../../../producers/extents/buildings/us-nashville.toml")),
    (
        "us-nga-133-cities.geojson",
        include_bytes!("../../../../../producers/extents/buildings/us-nga-133-cities.geojson"),
    ),
    ("us-nga-133-cities.toml", include_bytes!("../../../../../producers/extents/buildings/us-nga-133-cities.toml")),
    ("us-nyc-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/us-nyc-3d.geojson")),
    ("us-nyc-3d.toml", include_bytes!("../../../../../producers/extents/buildings/us-nyc-3d.toml")),
    (
        "us-nyc-footprints.geojson",
        include_bytes!("../../../../../producers/extents/buildings/us-nyc-footprints.geojson"),
    ),
    ("us-nyc-footprints.toml", include_bytes!("../../../../../producers/extents/buildings/us-nyc-footprints.toml")),
    ("us-or-rlis-sbfo.geojson", include_bytes!("../../../../../producers/extents/buildings/us-or-rlis-sbfo.geojson")),
    ("us-or-rlis-sbfo.toml", include_bytes!("../../../../../producers/extents/buildings/us-or-rlis-sbfo.toml")),
    ("us-philadelphia.geojson", include_bytes!("../../../../../producers/extents/buildings/us-philadelphia.geojson")),
    ("us-philadelphia.toml", include_bytes!("../../../../../producers/extents/buildings/us-philadelphia.toml")),
    ("us-ri-lod1.geojson", include_bytes!("../../../../../producers/extents/buildings/us-ri-lod1.geojson")),
    ("us-ri-lod1.toml", include_bytes!("../../../../../producers/extents/buildings/us-ri-lod1.toml")),
    ("us-san-francisco.geojson", include_bytes!("../../../../../producers/extents/buildings/us-san-francisco.geojson")),
    ("us-san-francisco.toml", include_bytes!("../../../../../producers/extents/buildings/us-san-francisco.toml")),
    ("us-santa-clara.geojson", include_bytes!("../../../../../producers/extents/buildings/us-santa-clara.geojson")),
    ("us-santa-clara.toml", include_bytes!("../../../../../producers/extents/buildings/us-santa-clara.toml")),
    ("us-seattle.geojson", include_bytes!("../../../../../producers/extents/buildings/us-seattle.geojson")),
    ("us-seattle.toml", include_bytes!("../../../../../producers/extents/buildings/us-seattle.toml")),
    ("us-semcog.geojson", include_bytes!("../../../../../producers/extents/buildings/us-semcog.geojson")),
    ("us-semcog.toml", include_bytes!("../../../../../producers/extents/buildings/us-semcog.toml")),
    ("us-tampa.geojson", include_bytes!("../../../../../producers/extents/buildings/us-tampa.geojson")),
    ("us-tampa.toml", include_bytes!("../../../../../producers/extents/buildings/us-tampa.toml")),
    ("us-tn-3d.geojson", include_bytes!("../../../../../producers/extents/buildings/us-tn-3d.geojson")),
    ("us-tn-3d.toml", include_bytes!("../../../../../producers/extents/buildings/us-tn-3d.toml")),
    ("us-tx-stratmap.geojson", include_bytes!("../../../../../producers/extents/buildings/us-tx-stratmap.geojson")),
    ("us-tx-stratmap.toml", include_bytes!("../../../../../producers/extents/buildings/us-tx-stratmap.toml")),
    (
        "us-usa-structures.geojson",
        include_bytes!("../../../../../producers/extents/buildings/us-usa-structures.geojson"),
    ),
    ("us-usa-structures.toml", include_bytes!("../../../../../producers/extents/buildings/us-usa-structures.toml")),
    ("us-vt-structures.geojson", include_bytes!("../../../../../producers/extents/buildings/us-vt-structures.geojson")),
    ("us-vt-structures.toml", include_bytes!("../../../../../producers/extents/buildings/us-vt-structures.toml")),
    ("za-capetown.geojson", include_bytes!("../../../../../producers/extents/buildings/za-capetown.geojson")),
    ("za-capetown.toml", include_bytes!("../../../../../producers/extents/buildings/za-capetown.toml")),
    ("za-ethekwini.geojson", include_bytes!("../../../../../producers/extents/buildings/za-ethekwini.geojson")),
    ("za-ethekwini.toml", include_bytes!("../../../../../producers/extents/buildings/za-ethekwini.toml")),
];

/// The buildings' real files: the declarations of the research (claimed areas, or none where it
/// isn't stated) and Overture's mask pieces, drawn by the build (no geometry in the file).
#[test]
fn the_real_buildings_files_are_taken() {
    let (files, warnings) = real_files("buildings", &REAL_BUILDINGS);
    // (Only the sources whose extent isn't stated warn: of class unknown.)
    assert!(warnings.iter().all(|w| w.contains("of class unknown")), "{warnings:#?}");
    let c = Extents.file("buildings/overture.geojson", &files["buildings/overture.geojson"]);
    assert_eq!(c.facts["classes"], serde_json::json!({"mask": 15}));
    let s = score(REAL_BUILDINGS[0].1).unwrap();
    assert_eq!((s.primary.as_str(), s.within), ("detail", None));
}

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

/// The server's score and order (`Score::of`, `Score::ranked`) are score.py's: on score.toml's own
/// examples and the pieces in score_pieces.json, each score and metric value as
/// producers/extents/score.py made them (score_expected.json: the command in score.py's usage;
/// producers/extents/test_extents.py makes it again and checks it's this file), to the last digit
/// it shows (three) and well past it; and each of its groups of pieces in score.py's order.
#[test]
fn the_score_is_score_py_s() {
    let s = score(REAL[0].1).unwrap();
    let file: Value = serde_json::from_str(include_str!("score_expected.json")).unwrap();
    let expected = file["scored"].as_array().unwrap();
    assert!(expected.len() > 15, "the examples and the pieces");
    let groups = file["ranked"].as_array().unwrap();
    assert!(groups.len() >= 4);
    for g in groups {
        let ps: Vec<(Option<f64>, BTreeMap<String, Option<f64>>, Option<&str>, Option<&str>)> = g["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let (score, vals) = s.of(p.as_object().unwrap());
                (score, vals, p["availability"].as_str(), p["extent"].as_str())
            })
            .collect();
        let at: Vec<usize> = (0..ps.len()).filter(|&k| ps[k].0.is_some()).collect();
        let rs: Vec<Ranked> = at
            .iter()
            .map(|&k| Ranked {
                lead: ps[k].1["spacing_m"].unwrap(),
                availability: ps[k].2,
                score: ps[k].0.unwrap(),
                extent: ps[k].3,
            })
            .collect();
        let got: Vec<usize> = s.ranked(&rs).into_iter().map(|k| at[k]).collect();
        let want: Vec<usize> = g["order"].as_array().unwrap().iter().map(|k| k.as_u64().unwrap() as usize).collect();
        assert_eq!(got, want, "{}", g["pieces"]);
    }
    for e in expected {
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
