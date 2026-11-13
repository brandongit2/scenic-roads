use super::*;
use axum::http::{header, HeaderValue};

const SCORE: &[u8] = include_bytes!("../../../../producers/extents/terrain/score.toml");

/// A box's ring, counter-clockwise.
fn square(w: f64, s: f64, e: f64, n: f64) -> Value {
    json!([[[w, s], [e, s], [e, n], [w, n], [w, s]]])
}

/// Source `id`'s declaration and pieces: (piece, its properties but its id, its polygons or none).
fn source(id: &str, availability: &str, ps: &[(&str, Value, Option<Value>)]) -> (String, Vec<u8>, String, Vec<u8>) {
    let decl = format!(
        "name = \"Source {id}\"\ncredit = \"Credit {id}\"\nlicence = \"CC-BY-4.0\"\nlicence_url = \"https://licence.test/\"\nsource = \"https://{id}.test/\"\ntheme = \"terrain\"\navailability = \"{availability}\"\n"
    );
    let feats: Vec<Value> = ps
        .iter()
        .map(|(p, props, geom)| {
            let mut pr = props.as_object().unwrap().clone();
            pr.insert("piece".into(), json!(p));
            pr.insert("availability".into(), json!(availability));
            pr.entry("extent").or_insert(json!(if geom.is_some() { "index" } else { "unknown" }));
            json!({"type": "Feature", "properties": pr,
                "geometry": geom.as_ref().map(|g| json!({"type": "MultiPolygon", "coordinates": [g]}))})
        })
        .collect();
    let fc = json!({"type": "FeatureCollection", "source": id, "theme": "terrain",
        "read": [{"piece": ps[0].0.split('/').next().unwrap(), "method": "file", "fetched": "2026-11-13", "tolerance_m": 50}],
        "features": feats});
    (format!("terrain/{id}.toml"), decl.into_bytes(), format!("terrain/{id}.geojson"), serde_json::to_vec(&fc).unwrap())
}

/// Open 1 m bare earth of 2021 over a box in France; 0.25 m to be had after a login over part of
/// it; one with no extent; a piece holding the north pole past the tiles.
fn fixture() -> Vec<(String, Vec<u8>)> {
    let a = source(
        "open-lidar",
        "open",
        &[
            ("dtm", json!({"spacing_m": 1, "surface": "ground", "year": 2021}), Some(square(2.0, 44.0, 2.4, 44.3))),
            (
                "pole",
                json!({"spacing_m": 2, "surface": "surface"}),
                Some(json!([[[-180.0, 86.0], [180.0, 86.0], [180.0, 90.0], [-180.0, 90.0], [-180.0, 86.0]]])),
            ),
        ],
    );
    let b = source(
        "fine",
        "login",
        &[
            ("dtm", json!({"spacing_m": 0.25, "surface": "ground"}), Some(square(2.2, 44.1, 2.6, 44.4))),
            ("later", json!({"spacing_m": 0.25}), None),
        ],
    );
    let mut files = vec![("terrain/score.toml".to_string(), SCORE.to_vec())];
    for (d, db, p, pb) in [a, b] {
        files.push((d, db));
        files.push((p, pb));
    }
    files
}

fn theme() -> Theme {
    theme_of(&fixture().into_iter().collect())
}

#[test]
fn the_layers_values_are_the_best_of_the_pieces_that_pass_the_switch() {
    let t = theme();
    let s = ex::score(SCORE).unwrap();
    let a = t.pieces["open-lidar/dtm"].clone();
    let b = t.pieces["fine/dtm"].clone();
    assert_eq!(a.score, s.of(&a.props).0);
    let both = ["open-lidar/dtm", "fine/dtm"];
    let best = |layer: &str, ids: &[&str], all: bool| {
        t.best(layer, ids.iter().copied(), all).map(|p| p.props["piece"].clone())
    };
    // Only what downloads freely: the open piece; every one: the finer piece, of the better score.
    assert_eq!(t.best(SCORE_LAYER, both, false).unwrap().source, "open-lidar");
    assert_eq!(t.best(SCORE_LAYER, both, true).unwrap().source, "fine");
    assert!(b.score > a.score);
    // A metric's layer: the best value among those stating it (the year: the open piece alone).
    assert_eq!(t.best("year", both, true).unwrap().source, "open-lidar");
    assert_eq!(best("year", &["fine/dtm"], true), None, "a piece not stating it takes no part");
    assert_eq!(t.best("spacing_m", both, true).unwrap().source, "fine");
    assert_eq!(best(SCORE_LAYER, &["fine/dtm"], false), None);
    // The click: best first, each with its source's description, metrics, values and reading.
    let l = t.listed(both, true);
    assert_eq!(l.iter().map(|p| p["id"].as_str().unwrap()).collect::<Vec<_>>(), ["fine/dtm", "open-lidar/dtm"]);
    assert_eq!(l[0]["source_name"], "Source fine");
    assert_eq!(l[0]["link"], "https://fine.test/");
    assert_eq!(l[0]["availability"], "login");
    assert_eq!(l[1]["metrics"]["year"], 2021);
    assert_eq!(l[1]["read"]["method"], "file");
    assert!(l[1]["values"]["vertical_rmse_m"].is_null());
    assert_eq!(t.listed(both, false).len(), 1);
    // The theme: its score, layers, sources (a piece with no extent named).
    let d = t.described("terrain");
    assert_eq!(d["primary"], "spacing_m");
    let layers: Vec<&str> = d["layers"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(layers, ["score", "spacing_m", "surface", "vertical_rmse_m", "year"]);
    let fine = d["sources"].as_array().unwrap().iter().find(|s| s["id"] == "fine").unwrap();
    assert_eq!(fine["pieces"], 2);
    assert_eq!(fine["classes"]["unknown"], 1);
    assert_eq!(fine["unknown"], json!(["later"]));
    // The words the app shows for the ids: the sets', each availability's and extent class's.
    assert_eq!(d["sets"], json!(["best"]));
    assert_eq!(d["labels"]["sets"]["best"], "Best available anywhere");
    for (k, _) in ex::AVAILABILITY {
        assert!(d["labels"]["availability"][k].as_str().is_some_and(|w| !w.is_empty()), "{k}");
    }
    for (k, _) in ex::CLASSES {
        assert!(d["labels"]["extent"][k].as_str().is_some_and(|w| !w.is_empty()), "{k}");
    }
    // The piece each layer paints, as the tiles do: of every one, the score's and the grid
    // spacing's the finer, the year's the one stating it; of open data alone, the open one; none
    // where nothing passes.
    let p = t.painted(&both, true);
    assert_eq!(p[SCORE_LAYER], "fine/dtm");
    assert_eq!(p["spacing_m"], "fine/dtm");
    assert_eq!(p["year"], "open-lidar/dtm");
    assert_eq!(t.painted(&both, false)[SCORE_LAYER], "open-lidar/dtm");
    assert!(t.painted(&["fine/dtm"], false).is_empty());
}

/// score.toml's colours, as hex.
fn ramp_hex(k: usize) -> String {
    hex(ex::score(SCORE).unwrap().ramp[k])
}

#[test]
fn each_layer_is_drawn_as_score_toml_says() {
    let t = theme();
    let s = ex::score(SCORE).unwrap();
    let (r, n) = (&s.ramp, s.ramp.len());
    let (a, b, pole) = (&t.pieces["open-lidar/dtm"], &t.pieces["fine/dtm"], &t.pieces["open-lidar/pole"]);
    // The spacing's classes take the ramp's colours, the finest its best end: 0.25 m in "0.5 m",
    // 1 m in "1 m", 2 m in "2 m".
    assert_eq!(n, s.metrics["spacing_m"].classes.len());
    assert_eq!(t.colour("spacing_m", b), Some(r[n - 1]));
    assert_eq!(t.colour("spacing_m", a), Some(r[n - 2]));
    assert_eq!(t.colour("spacing_m", pole), Some(r[n - 3]));
    // The score: the spacing's class, shaded by the tie-break. Bare earth, nothing else stated: the
    // best tie-break, the class's own colour; a surface model with no year, the worst: `shade` of
    // the way to the next coarser class; 1 m of 2021, a little of the way.
    assert_eq!(t.colour(SCORE_LAYER, b), Some(r[n - 1]));
    assert_eq!(t.colour(SCORE_LAYER, pole), Some(mix(r[n - 3], r[n - 4], s.shade)));
    let rest = (0.4 + 0.2 * 21.0 / 26.0) / 0.6;
    assert_eq!(t.colour(SCORE_LAYER, a), Some(mix(r[n - 2], r[n - 3], s.shade * (1.0 - rest))));
    assert_ne!(t.colour(SCORE_LAYER, a), Some(r[n - 2]));
    // Stating none of the tie-break's metrics: the class's own colour, as the best (not shaded by
    // the primary's own value, which would darken every coarser class more).
    let mut bare = a.clone();
    bare.props.retain(|k, _| k == "spacing_m");
    (bare.score, bare.values) = s.of(&bare.props);
    assert_eq!(t.colour(SCORE_LAYER, &bare), Some(r[n - 2]));
    let lead = bare.values["spacing_m"].unwrap();
    assert!((bare.score.unwrap() - lead).abs() < 1e-12, "the score: the primary's value alone");
    // Two words, two colours; the year along the ramp.
    let surface = &s.metrics["surface"].colours;
    assert_eq!(t.colour("surface", a), Some(surface["ground"]));
    assert_eq!(t.colour("surface", pole), Some(surface["surface"]));
    assert_eq!(t.colour("year", a), Some(along(r, 21.0 / 26.0)));
    assert_eq!(t.colour("year", b), None);
    // The legend: every layer's, from score.toml.
    let d = t.described("terrain");
    let l = &d["layers"];
    assert_eq!(l[0]["label"], "Best data");
    assert_eq!(l[0]["legend"], "classes");
    assert_eq!(l[0]["unit"], "m");
    assert_eq!(l[0]["shade"], s.shade);
    assert_eq!(l[0]["shaded_by"], json!(["surface", "vertical_rmse_m", "year"]));
    assert_eq!(
        l[0]["stops"][0],
        json!({"label": "0.5 m", "upto": 0.625, "colour": ramp_hex(n - 1),
        "shaded": hex(mix(r[n - 1], r[n - 2], s.shade))})
    );
    assert_eq!(l[0]["stops"][n - 1]["label"], "coarser");
    assert!(l[0]["stops"][n - 1]["upto"].is_null());
    assert_eq!(l[1]["legend"], "classes");
    assert_eq!(l[1]["stops"][1], json!({"label": "1 m", "upto": 1.25, "colour": ramp_hex(n - 2)}));
    assert_eq!(l[2]["legend"], "values");
    assert_eq!(
        l[2]["stops"],
        json!([{"value": 0.0, "label": "surface", "colour": "#e69f00"},
        {"value": 1.0, "label": "ground", "colour": "#56b4e9"}])
    );
    for (k, (from, to)) in [(3, ("10", "0.05")), (4, ("2000", "2026"))] {
        assert_eq!(l[k]["legend"], "ramp");
        assert_eq!(l[k]["stops"][0]["label"], from);
        assert_eq!(l[k]["stops"][4]["label"], to);
        assert_eq!(l[k]["stops"][4]["colour"], ramp_hex(n - 1));
        assert_eq!(l[k]["ramp"][0], json!([0.0, ramp_hex(0)]));
    }
}

/// A score.toml of before the layers' colours (as the NAS held on 13 Nov): taken, each layer on
/// the built-in ramp, the score's name the server's.
#[test]
fn a_score_toml_with_no_colours_draws_every_layer_on_the_built_in_ramp() {
    const OLD: &str = "theme = \"terrain\"\nprimary = \"spacing_m\"\nwithin = 1.25\nshare = 0.02\n\
        [metric.spacing_m]\nlabel = \"Grid spacing\"\nunit = \"m\"\nscale = \"log\"\nworst = 1000\nbest = 0.1\n\
        [metric.surface]\nlabel = \"Bare earth or surface\"\nweight = 0.4\nvalues = { ground = 1.0, surface = 0.0 }\n\
        [metric.year]\nlabel = \"Year surveyed\"\nweight = 0.2\nscale = \"linear\"\nworst = 2000\nbest = 2026\n";
    let mut files: BTreeMap<String, Vec<u8>> = fixture().into_iter().collect();
    files.insert("terrain/score.toml".into(), OLD.as_bytes().to_vec());
    let t = theme_of(&files);
    let a = &t.pieces["open-lidar/dtm"];
    assert_eq!(t.colour(SCORE_LAYER, a), Some(along(&DEFAULT_RAMP, a.score.unwrap())));
    assert_eq!(t.colour("spacing_m", a), Some(along(&DEFAULT_RAMP, a.values["spacing_m"].unwrap())));
    assert_eq!(t.colour("surface", a), Some(DEFAULT_RAMP[4]));
    let d = t.described("terrain");
    assert_eq!(d["layers"][0]["label"], DEFAULT_LABEL);
    for l in d["layers"].as_array().unwrap() {
        assert_eq!(l["legend"], if l["id"] == "surface" { "values" } else { "ramp" }, "{l}");
    }
    assert_eq!(d["layers"][1]["stops"][0]["label"], "1000");
    assert_eq!(d["layers"][1]["stops"][4]["label"], "0.1");
}

#[test]
fn a_ramp_runs_through_its_colours_evenly() {
    let r = [[0, 0, 0], [100, 50, 0], [200, 250, 10]];
    assert_eq!(along(&r, 0.0), r[0]);
    assert_eq!(along(&r, 0.5), r[1]);
    assert_eq!(along(&r, 1.0), r[2]);
    assert_eq!(along(&r, 0.25), [50, 25, 0]);
    assert_eq!(along(&r, 0.75), [150, 150, 5]);
    assert_eq!(along(&r, 2.0), r[2]);
    assert_eq!(along(&r, -1.0), r[0]);
    assert_eq!(along(&DEFAULT_RAMP, 0.0), [68, 1, 84]);
}

/// A NAS root with the step's packs of the fixture's pieces and a catalogue naming them, and a
/// server whose extents are the fixture's.
fn served() -> (tempfile::TempDir, tempfile::TempDir, S) {
    let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    let mut out = buildstore::out::Out::unbound(&root, &nas.path().join("scratch")).unwrap();
    let mut pieces = Vec::new();
    for (p, b) in fixture() {
        if let ex::Kind::Pieces(_, id) = ex::kind(&p) {
            pieces.extend(terrain::available::pieces(id, &b).unwrap());
        }
    }
    terrain::available::build(&mut out, "terrain", &pieces, &|_, _| {}).unwrap();
    out.save().unwrap();
    let mut cat = store::catalogue::Catalogue::new(1);
    for (l, c) in &out.manifest {
        let parts: Vec<&str> = l.split('/').collect();
        let ["layers", layer, scope, key] = parts[..] else { continue };
        let e = cat.layers.entry(layer.into()).or_insert_with(|| store::catalogue::Layer {
            encoding: fmt::ENCODING.into(),
            maxzoom: fmt::maxz(fmt::of_layer(layer).unwrap().1),
            ..Default::default()
        });
        match scope {
            "root" => e.root = Some(l.clone()),
            _ => {
                e.hi.insert(key.replace('-', "/"), l.clone());
            }
        }
        let size = std::fs::metadata(nas.path().join(c)).unwrap().len();
        cat.files.insert(l.clone(), store::catalogue::FileRef { file: c.clone(), size, ..Default::default() });
    }
    assert!(cat.layers.contains_key("available-terrain") && cat.layers.contains_key("available-terrain-n"));
    store::testkit::map_root(nas.path(), &cat);
    let s = crate::test_state(home.path(), &root);
    let files = fixture();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
    s.available.set("sources/inputs/extents/@index.0123456789abcdef.json", &refs);
    (nas, home, s)
}

fn headers(etag: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(e) = etag {
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_str(e).unwrap());
    }
    h
}

async fn body(r: Response) -> Vec<u8> {
    axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
}

/// The tile of z12 over a point.
fn z12(lng: f64, lat: f64) -> (u32, u32) {
    let (x, y) = roadcore::geo::merc(lng, lat);
    ((x * 4096.0) as u32, (y * 4096.0) as u32)
}

fn png_pixel(png: &[u8], i: usize, j: usize) -> [u8; 4] {
    let mut d = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
    let mut px = vec![0u8; d.output_buffer_size().unwrap()];
    d.next_frame(&mut px).unwrap();
    let k = (j * SIDE + i) * 4;
    [px[k], px[k + 1], px[k + 2], px[k + 3]]
}

#[tokio::test]
async fn tiles_are_coloured_by_the_layer_and_the_switch() {
    let (_nas, _home, s) = served();
    let t = theme();
    let get = |layer: &str, z: u8, x: u32, y: u32, q: Option<&str>, h: HeaderMap| {
        mercator_tile(
            State(s.clone()),
            Path(("terrain".into(), "best".into(), layer.into(), z, x, y)),
            RawQuery(q.map(str::to_string)),
            h,
        )
    };
    // Inside both boxes: open alone, the open piece's score; all, the finer one's.
    let (x, y) = z12(2.3, 44.2);
    let r = get("score", 12, x, y, None, headers(None)).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache");
    let etag = r.headers()[header::ETAG].to_str().unwrap().to_string();
    let open = body(r).await;
    let c = t.colour(SCORE_LAYER, &t.pieces["open-lidar/dtm"]).unwrap();
    assert_eq!(png_pixel(&open, 128, 128), [c[0], c[1], c[2], 255]);
    let all = body(get("score", 12, x, y, Some("avail=all"), headers(None)).await).await;
    let c = t.colour(SCORE_LAYER, &t.pieces["fine/dtm"]).unwrap();
    assert_eq!(png_pixel(&all, 128, 128), [c[0], c[1], c[2], 255]);
    // Its ETag: 304; versioned: kept for good.
    assert_eq!(get("score", 12, x, y, None, headers(Some(&etag))).await.status(), StatusCode::NOT_MODIFIED);
    let r = get("score", 12, x, y, Some("v=1"), headers(None)).await;
    assert_eq!(r.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
    // Where only the login piece is, open: none there.
    let (x, y) = z12(2.55, 44.35);
    assert_eq!(get("score", 12, x, y, None, headers(None)).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(get("score", 12, x, y, Some("avail=all"), headers(None)).await.status(), StatusCode::OK);
    // No piece: 204; a layer, set or zoom that isn't: 404.
    assert_eq!(get("score", 12, 0, 0, None, headers(None)).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(get("year", 0, 0, 0, None, headers(None)).await.status(), StatusCode::OK);
    assert_eq!(get("slope", 0, 0, 0, None, headers(None)).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(get("score", 13, 0, 0, None, headers(None)).await.status(), StatusCode::NOT_FOUND);
    let built = mercator_tile(
        State(s.clone()),
        Path(("terrain".into(), "built".into(), "score".into(), 0, 0, 0)),
        RawQuery(None),
        headers(None),
    );
    assert_eq!(built.await.status(), StatusCode::NOT_FOUND);
    // Past the tiles, the pole's cap.
    let polar = |z, x, y| {
        polar_tile(
            State(s.clone()),
            Path(("n".into(), "terrain".into(), "best".into(), "score".into(), z, x, y)),
            RawQuery(None),
            headers(None),
        )
    };
    let r = polar(0, 0, 0).await;
    assert_eq!(r.status(), StatusCode::OK);
    let c = t.colour(SCORE_LAYER, &t.pieces["open-lidar/pole"]).unwrap();
    assert_eq!(png_pixel(&body(r).await, 128, 128), [c[0], c[1], c[2], 255]);
    assert_eq!(polar(4, 0, 0).await.status(), StatusCode::NOT_FOUND);
}

/// A tile's headers say what its body is: a PNG painted here goes out with no Content-Encoding
/// (the live server sent `gzip` over a plain PNG: through Caddy over HTTP/2, an empty body),
/// made or kept, on both grids.
#[tokio::test]
async fn a_tiles_headers_describe_its_body() {
    let (_nas, _home, s) = served();
    let (x, y) = z12(2.3, 44.2);
    let mercator = || {
        mercator_tile(
            State(s.clone()),
            Path(("terrain".into(), "best".into(), "score".into(), 12, x, y)),
            RawQuery(None),
            headers(None),
        )
    };
    let polar = || {
        polar_tile(
            State(s.clone()),
            Path(("n".into(), "terrain".into(), "best".into(), "surface".into(), 0, 0, 0)),
            RawQuery(Some("avail=all".into())),
            headers(None),
        )
    };
    // (Each twice: made, then from the tiles kept.)
    for r in [mercator().await, mercator().await, polar().await, polar().await] {
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "image/png");
        assert!(r.headers().get(header::CONTENT_ENCODING).is_none(), "{:?}", r.headers());
        let b = body(r).await;
        assert!(b.starts_with(b"\x89PNG\r\n\x1a\n"), "a PNG");
    }
}

#[tokio::test]
async fn a_click_answers_the_pieces_there_best_first() {
    let (_nas, _home, s) = served();
    let click = |lng: f64, lat: f64, avail: Option<&str>| {
        at(
            State(s.clone()),
            Path(("terrain".into(), "best".into())),
            Query(At { lng, lat, avail: avail.map(str::to_string) }),
        )
    };
    let ids = |v: &Value| {
        v["pieces"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };
    let v: Value = serde_json::from_slice(&body(click(2.3, 44.2, Some("all")).await).await).unwrap();
    assert_eq!(ids(&v), ["fine/dtm", "open-lidar/dtm"]);
    assert_eq!(v["painted"]["score"], "fine/dtm");
    assert!(v["pieces"][0]["score"].as_f64().unwrap() > v["pieces"][1]["score"].as_f64().unwrap());
    let v: Value = serde_json::from_slice(&body(click(2.3, 44.2, None).await).await).unwrap();
    assert_eq!(ids(&v), ["open-lidar/dtm"]);
    let v: Value = serde_json::from_slice(&body(click(10.0, 10.0, None).await).await).unwrap();
    assert!(ids(&v).is_empty());
    let v: Value = serde_json::from_slice(&body(click(30.0, 89.0, None).await).await).unwrap();
    assert_eq!(ids(&v), ["open-lidar/pole"], "past the tiles, the pole's cap");
    // The theme.
    let r = about(State(s.clone()), Path("terrain".into())).await;
    let v: Value = serde_json::from_slice(&body(r).await).unwrap();
    assert_eq!(v["sources"].as_array().unwrap().len(), 2);
    assert_eq!(about(State(s.clone()), Path("bathymetry".into())).await.status(), StatusCode::NOT_FOUND);
    // The app's version of the layer names the extents' too.
    let meta = crate::meta_json(&s);
    let v = meta["versions"]["available-terrain"].as_str().unwrap();
    assert!(v.ends_with("-0123456789abcdef"), "{v}");
    assert!(meta["packs"].get("available-terrain").is_none());
}

/// The real pieces (producers/extents/terrain/) built and painted, each layer as a PNG over the
/// map's land colour: the world at z2, France at z5, Vancouver at z8 (every piece, `avail=all`),
/// written to `$SCENIC_RENDER_DIR`: a look at the colours, run by hand (the step's whole build, ~1
/// min); nothing without the variable.
#[test]
fn renders_the_real_pieces() {
    let Some(dir) = std::env::var_os("SCENIC_RENDER_DIR").map(std::path::PathBuf::from) else { return };
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../producers/extents/terrain");
    let mut files = BTreeMap::new();
    let mut pieces = Vec::new();
    for e in std::fs::read_dir(&src).unwrap() {
        let p = e.unwrap().path();
        let name = format!("terrain/{}", p.file_name().unwrap().to_string_lossy());
        let b = std::fs::read(&p).unwrap();
        if let ex::Kind::Pieces(_, id) = ex::kind(&name) {
            pieces.extend(terrain::available::pieces(id, &b).unwrap());
        }
        files.insert(name, b);
    }
    let th = theme_of(&files);
    let nas = tempfile::tempdir().unwrap();
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    let mut out = buildstore::out::Out::unbound(&root, &nas.path().join("scratch")).unwrap();
    terrain::available::build(&mut out, "terrain", &pieces, &|_, _| {}).unwrap();
    out.save().unwrap();
    let layer = fmt::layer("terrain");
    let mut get = |z: u8, x: u32, y: u32| -> anyhow::Result<Option<Tile>> {
        let Some((scope, key)) = fmt::pack_of(Grid::Mercator, z, x, y) else { return Ok(None) };
        let Some(c) = out.get(&format!("layers/{layer}/{scope}/{}", key.replace('/', "-"))) else { return Ok(None) };
        let b = out.path(c).read()?;
        let idx = store::pack::PackIndex::parse_bytes(&b)?;
        Ok(match idx.find(z, x, y) {
            Some(e) => Some(Tile::decode(&b[e.offset as usize..][..e.len as usize])?),
            None => None,
        })
    };
    let land = [0x0b, 0x0e, 0x13];
    // (place, zoom, first column and row, tiles a side)
    let places: [(&str, u8, u32, u32, u32); 3] =
        [("world", 2, 0, 0, 4), ("france", 5, 15, 10, 2), ("vancouver", 8, 39, 87, 3)];
    let mut layers = vec![SCORE_LAYER.to_string()];
    layers.extend(th.score.as_ref().unwrap().metrics.keys().cloned());
    for (place, z, x0, y0, n) in places {
        let tiles: Vec<Option<Tile>> =
            (0..n * n).map(|k| fmt::tile_at(Grid::Mercator, z, x0 + k % n, y0 + k / n, &mut get).unwrap()).collect();
        let n = n as usize;
        for l in &layers {
            let w = n * SIDE;
            let mut rgb = [land].repeat(w * w).concat();
            for (k, t) in tiles.iter().enumerate() {
                let Some(b) = t.as_ref().and_then(|t| paint(t, &th, l, true).unwrap()) else { continue };
                let mut d = png::Decoder::new(std::io::Cursor::new(b)).read_info().unwrap();
                let mut v = vec![0; d.output_buffer_size().unwrap()];
                d.next_frame(&mut v).unwrap();
                let (ox, oy) = ((k % n) * SIDE, (k / n) * SIDE);
                for j in 0..SIDE {
                    for i in 0..SIDE {
                        let q = (j * SIDE + i) * 4;
                        if v[q + 3] > 0 {
                            let o = ((oy + j) * w + ox + i) * 3;
                            rgb[o..o + 3].copy_from_slice(&v[q..q + 3]);
                        }
                    }
                }
            }
            let f = std::fs::File::create(dir.join(format!("{place}-{l}.png"))).unwrap();
            let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, w as u32);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
    }
}
