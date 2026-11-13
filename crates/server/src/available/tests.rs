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
/// it; one with no extent; a piece holding the north pole past the tiles; a paid 1 m of 2024 over
/// the box's west end (a tie with the open piece, of a better score).
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
    let c = source(
        "paid-twin",
        "paid",
        &[("dtm", json!({"spacing_m": 1, "surface": "ground", "year": 2024}), Some(square(2.0, 44.0, 2.15, 44.05)))],
    );
    let mut files = vec![("terrain/score.toml".to_string(), SCORE.to_vec())];
    for (d, db, p, pb) in [a, b, c] {
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
    assert_eq!(layers, ["score", "surface", "vertical_rmse_m", "year"], "the primary's layer is the score's");
    assert!(!t.has_layer("spacing_m") && t.has_layer(SCORE_LAYER) && t.has_layer("year"));
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
    // The piece each layer paints, as the tiles do: of every one, the score's the finer, the
    // year's the one stating it; of open data alone, the open one; none where nothing passes.
    let p = t.painted(&both, true);
    assert_eq!(p[SCORE_LAYER], "fine/dtm");
    assert!(p.get("spacing_m").is_none(), "no layer of the primary's own");
    assert_eq!(p["year"], "open-lidar/dtm");
    assert_eq!(t.painted(&both, false)[SCORE_LAYER], "open-lidar/dtm");
    assert!(t.painted(&["fine/dtm"], false).is_empty());
}

/// An open piece wins a tie: of the open 1 m of 2021 and the paid 1 m of 2024 (the better score),
/// the open one, on every layer that ranks them; the finer grid beyond the factor whatever its
/// availability.
#[test]
fn an_open_piece_wins_a_tie() {
    let t = theme();
    let (open, paid) = (&t.pieces["open-lidar/dtm"], &t.pieces["paid-twin/dtm"]);
    assert!(paid.score > open.score);
    let twins = ["paid-twin/dtm", "open-lidar/dtm"];
    for layer in [SCORE_LAYER, "surface"] {
        assert_eq!(t.best(layer, twins, true).unwrap().source, "open-lidar", "{layer}");
    }
    // The year isn't tied: the paid one's is newer.
    assert_eq!(t.best("year", twins, true).unwrap().source, "paid-twin");
    let all = ["paid-twin/dtm", "open-lidar/dtm", "fine/dtm"];
    assert_eq!(t.best(SCORE_LAYER, all, true).unwrap().source, "fine");
    let ids = |v: Vec<Value>| v.iter().map(|p| p["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(t.listed(all, true)), ["fine/dtm", "open-lidar/dtm", "paid-twin/dtm"]);
}

/// What a tile carries: each layer's value of the piece drawn, in its scale's code, and the
/// layers' scales as described.
#[test]
fn each_layer_carries_its_value() {
    let t = theme();
    let (a, b, pole) = (&t.pieces["open-lidar/dtm"], &t.pieces["fine/dtm"], &t.pieces["open-lidar/pole"]);
    let near = |x: Option<f64>, y: f64| assert!(x.is_some_and(|x| (x - y).abs() < 1e-9), "{x:?} against {y}");
    // The score's: the piece's own grid spacing, as log10 of metres, whatever its tie-break (the
    // 1 m of 2021 bare earth at 1 m, the 2 m surface at 2 m); the words' values; the year as it is.
    assert!(a.score.is_some() && pole.score.is_some());
    near(t.code(SCORE_LAYER, a), 0.0);
    near(t.code(SCORE_LAYER, b), 0.25f64.dlog10());
    near(t.code(SCORE_LAYER, pole), 2f64.dlog10());
    assert_eq!(t.code("spacing_m", a), None, "not a layer");
    near(t.code("surface", a), 1.0);
    near(t.code("surface", pole), 0.0);
    near(t.code("year", a), 2021.0);
    assert_eq!(t.code("year", b), None);
    assert_eq!(t.code("vertical_rmse_m", a), None);
    // The layers: the score's first, on the primary's scale and in its unit, under score.toml's
    // label.
    let d = t.described("terrain");
    let l = d["layers"].as_array().unwrap();
    let ids: Vec<&str> = l.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["score", "surface", "vertical_rmse_m", "year"]);
    assert_eq!(
        (&l[0]["label"], &l[0]["scale"], &l[0]["unit"], &l[0]["best"]),
        (&json!("Best data"), &json!("log"), &json!("m"), &json!("low"))
    );
    assert_eq!(l[0]["range"], json!([0.25, 2.0]), "the spacings present, of pieces with an extent");
    assert_eq!((&l[1]["scale"], &l[1]["best"]), (&json!("values"), &json!("high")));
    assert_eq!(l[1]["words"], json!([{"value": 0.0, "label": "surface"}, {"value": 1.0, "label": "ground"}]));
    assert!(l[2]["range"].is_null(), "no piece states its accuracy");
    assert_eq!(
        (&l[3]["scale"], &l[3]["best"], &l[3]["range"]),
        (&json!("linear"), &json!("high"), &json!([2021.0, 2024.0]))
    );
    assert_eq!(d["availability"], json!(["open", "login", "paid", "closed"]));
    // The metrics as declared, the primary among them (the click's rows).
    let m: Vec<&str> = d["metrics"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(m, ["spacing_m", "surface", "vertical_rmse_m", "year"]);
    // A score.toml naming no label: the server's own.
    let mut files: BTreeMap<String, Vec<u8>> = fixture().into_iter().collect();
    let unnamed = std::str::from_utf8(SCORE).unwrap().replacen("label = \"Best data\"\n", "", 1);
    assert_ne!(unnamed.as_bytes(), SCORE);
    files.insert("terrain/score.toml".into(), unnamed.into_bytes());
    assert_eq!(theme_of(&files).described("terrain")["layers"][0]["label"], "Best data");
}

/// A score.toml as the NAS holds it before the new one is placed (its classes, colours, ramp and
/// shade): served as the new one, those fields not read.
#[test]
fn a_score_toml_with_its_retired_colours_is_served() {
    let old = std::str::from_utf8(SCORE)
        .unwrap()
        .replacen("share = 0.02\n", "share = 0.02\nramp = [\"#803041\", \"#e7fda7\"]\nshade = 0.4\n", 1)
        .replacen(
            "best = 0.1\n",
            "best = 0.1\nclasses = [{ upto = 0.625, label = \"0.5 m\" }, { label = \"coarser\" }]\n",
            1,
        );
    let mut files: BTreeMap<String, Vec<u8>> = fixture().into_iter().collect();
    files.insert("terrain/score.toml".into(), old.into_bytes());
    let t = theme_of(&files);
    let new = theme();
    assert_eq!(t.described("terrain")["layers"], new.described("terrain")["layers"]);
    let a = &t.pieces["open-lidar/dtm"];
    assert_eq!(t.code(SCORE_LAYER, a), new.code(SCORE_LAYER, a));
}

/// A Terrarium pixel's value.
fn terrarium(px: [u8; 4]) -> f64 {
    f64::from(px[0]) * 256.0 + f64::from(px[1]) + f64::from(px[2]) / 256.0 - 32768.0
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
    // (To its 1/256.)
    let near = |px: [u8; 4], want: f64| {
        assert_eq!(px[3], 255);
        assert!((terrarium(px) - want).abs() <= 0.5 / 256.0, "{} against {want}", terrarium(px));
    };
    near(png_pixel(&open, 128, 128), t.code(SCORE_LAYER, &t.pieces["open-lidar/dtm"]).unwrap());
    near(png_pixel(&open, 128, 128), 0.0);
    let all = body(get("score", 12, x, y, Some("avail=all"), headers(None)).await).await;
    near(png_pixel(&all, 128, 128), t.code(SCORE_LAYER, &t.pieces["fine/dtm"]).unwrap());
    let year = body(get("year", 12, x, y, Some("avail=all"), headers(None)).await).await;
    near(png_pixel(&year, 128, 128), 2021.0);
    // Where the open piece ends inside a tile: its pixels inside, transparent (all 0) outside.
    let (lng, lat) = (2.4, 44.3);
    let (xe, ye) = z12(lng, lat);
    let edge = body(get("score", 12, xe, ye, None, headers(None)).await).await;
    let w = 4096.0 * SIDE as f64;
    let (px, py) = roadcore::geo::merc(lng, lat);
    let (i, j) = ((px * w) as usize % SIDE, (py * w) as usize % SIDE);
    assert!(i > 2 && i < SIDE - 3 && j > 2 && j < SIDE - 3, "{i} {j}");
    assert_eq!(png_pixel(&edge, i - 2, j + 2)[3], 255, "inside");
    assert_eq!(png_pixel(&edge, i + 2, j - 2), [0, 0, 0, 0], "outside");
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
    assert_eq!(get("spacing_m", 0, 0, 0, None, headers(None)).await.status(), StatusCode::NOT_FOUND, "the score's");
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
    near(png_pixel(&body(r).await, 128, 128), t.code(SCORE_LAYER, &t.pieces["open-lidar/pole"]).unwrap());
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
    assert!(v["painted"].get("spacing_m").is_none());
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
    assert_eq!(v["sources"].as_array().unwrap().len(), 3);
    assert_eq!(about(State(s.clone()), Path("bathymetry".into())).await.status(), StatusCode::NOT_FOUND);
    // The app's version of the layer names the extents' too.
    let meta = crate::meta_json(&s);
    let v = meta["versions"]["available-terrain"].as_str().unwrap();
    assert!(v.ends_with("-0123456789abcdef"), "{v}");
    assert!(meta["packs"].get("available-terrain").is_none());
}

/// The real pieces (producers/extents/terrain/) built and their score layer's values drawn as PNGs
/// over the map's land colour, each value in its spacing's class (0.5, 1, 2, 5, 10, 30, 90 m and
/// coarser, a class's edge a factor of 1.25 over its spacing, finer brighter): the world at z2,
/// Europe at z4, France at z5, Germany at z6, Vancouver at z8, each under both settings of the
/// switch, written to `$SCENIC_RENDER_DIR`; a look by hand at what the app's colour map will show
/// (the step's whole build, ~1 min); nothing without the variable. It prints what the step made:
/// its time, its peak resident set, its packs' bytes and its tiles' sets (over 256, the most).
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
    let t = std::time::Instant::now();
    terrain::available::build(&mut out, "terrain", &pieces, &|_, _| {}).unwrap();
    let took = t.elapsed();
    // SAFETY: getrusage fills the struct it's given (ru_maxrss: bytes on macOS).
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    let peak = if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) } == 0 { r.ru_maxrss } else { 0 };
    out.save().unwrap();
    let layer = fmt::layer("terrain");
    let (mut bytes, mut tiles, mut over, mut most) = (0, 0, 0, 0);
    for (l, c) in &out.manifest {
        if !l.starts_with(&format!("layers/{layer}/")) {
            continue;
        }
        let b = out.path(c).read().unwrap();
        bytes += b.len();
        for e in store::pack::PackIndex::parse_bytes(&b).unwrap().entries {
            let n = Tile::decode(&b[e.offset as usize..][..e.len as usize]).unwrap().sets.len();
            tiles += 1;
            over += usize::from(n > 256);
            most = most.max(n);
        }
    }
    eprintln!(
        "{} pieces built in {took:.1?}, a peak resident set of {peak} bytes: {bytes} bytes of packs, {tiles} tiles, \
         {over} of over 256 sets, {most} the most",
        pieces.len()
    );
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
    // The classes' upper edges (metres) and their colours, finest first.
    let edges = [0.625, 1.25, 2.5, 6.25, 12.5, 37.5, 112.5];
    let ramp: [[u8; 3]; 8] = [
        [0xe7, 0xfd, 0xa7],
        [0x9a, 0xef, 0xba],
        [0x58, 0xd7, 0xd2],
        [0x4f, 0xb6, 0xdf],
        [0x6d, 0x90, 0xd5],
        [0x84, 0x6a, 0xb3],
        [0x8a, 0x4a, 0x7e],
        [0x80, 0x30, 0x41],
    ];
    // (place, zoom, first column and row, tiles a side)
    let places: [(&str, u8, u32, u32, u32); 5] = [
        ("world", 2, 0, 0, 4),
        ("europe", 4, 6, 3, 4),
        ("france", 5, 15, 10, 2),
        ("germany", 6, 32, 20, 3),
        ("vancouver", 8, 39, 87, 3),
    ];
    for (place, z, x0, y0, n) in places {
        let tiles: Vec<Option<Tile>> =
            (0..n * n).map(|k| fmt::tile_at(Grid::Mercator, z, x0 + k % n, y0 + k / n, &mut get).unwrap()).collect();
        let n = n as usize;
        for all in [true, false] {
            let w = n * SIDE;
            let mut rgb = [land].repeat(w * w).concat();
            for (k, t) in tiles.iter().enumerate() {
                let Some(b) = t.as_ref().and_then(|t| values(t, &th, SCORE_LAYER, all).unwrap()) else { continue };
                let mut d = png::Decoder::new(std::io::Cursor::new(b)).read_info().unwrap();
                let mut v = vec![0; d.output_buffer_size().unwrap()];
                d.next_frame(&mut v).unwrap();
                let (ox, oy) = ((k % n) * SIDE, (k / n) * SIDE);
                for j in 0..SIDE {
                    for i in 0..SIDE {
                        let q = (j * SIDE + i) * 4;
                        if v[q + 3] > 0 {
                            let m = 10f64.dpowf(terrarium([v[q], v[q + 1], v[q + 2], v[q + 3]]));
                            let class = edges.iter().position(|&e| m <= e).unwrap_or(edges.len());
                            let o = ((oy + j) * w + ox + i) * 3;
                            rgb[o..o + 3].copy_from_slice(&ramp[class]);
                        }
                    }
                }
            }
            let name = format!("{place}-score-{}.png", if all { "all" } else { "open" });
            let f = std::fs::File::create(dir.join(name)).unwrap();
            let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, w as u32);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
    }
}
