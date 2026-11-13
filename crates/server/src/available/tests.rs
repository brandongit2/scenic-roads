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

/// An open piece wins a tie: of the open 1 m of 2021 and the paid 1 m of 2024 (the better score),
/// the open one, on every layer that ranks them; the finer grid beyond the factor whatever its
/// availability.
#[test]
fn an_open_piece_wins_a_tie() {
    let t = theme();
    let (open, paid) = (&t.pieces["open-lidar/dtm"], &t.pieces["paid-twin/dtm"]);
    assert!(paid.score > open.score);
    let twins = ["paid-twin/dtm", "open-lidar/dtm"];
    for layer in [SCORE_LAYER, "spacing_m", "surface"] {
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
    let s = ex::score(SCORE).unwrap();
    let (a, b, pole) = (&t.pieces["open-lidar/dtm"], &t.pieces["fine/dtm"], &t.pieces["open-lidar/pole"]);
    let near = |x: Option<f64>, y: f64| assert!(x.is_some_and(|x| (x - y).abs() < 1e-9), "{x:?} against {y}");
    // The spacing as log10 of metres; the words' values; the year as it is.
    near(t.code("spacing_m", a), 0.0);
    near(t.code("spacing_m", b), 0.25f64.dlog10());
    near(t.code("surface", a), 1.0);
    near(t.code("surface", pole), 0.0);
    near(t.code("year", a), 2021.0);
    assert_eq!(t.code("year", b), None);
    assert_eq!(t.code("vertical_rmse_m", a), None);
    // The score on the spacing's axis: stating nothing but the spacing, the spacing itself; a
    // better tie-break, finer than it, a worse one coarser, both within the factor.
    let mut bare = a.clone();
    bare.props.retain(|k, _| k == "spacing_m");
    (bare.score, bare.values) = s.of(&bare.props);
    near(t.code(SCORE_LAYER, &bare), 0.0);
    let (good, poor) = (t.code(SCORE_LAYER, a).unwrap(), t.code(SCORE_LAYER, pole).unwrap());
    assert!(good < 0.0 && good > -s.within.dlog10(), "{good}");
    assert!(poor > 2f64.dlog10() && poor < (2.0 * s.within).dlog10(), "{poor}");
    // The layers: the score's first, on the primary's scale.
    let d = t.described("terrain");
    let l = d["layers"].as_array().unwrap();
    let ids: Vec<&str> = l.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["score", "spacing_m", "surface", "vertical_rmse_m", "year"]);
    assert_eq!(l[0]["label"], "Best data");
    assert_eq!(
        (&l[0]["scale"], &l[0]["of"], &l[0]["unit"], &l[0]["best"]),
        (&json!("log"), &json!("spacing_m"), &json!("m"), &json!("low"))
    );
    assert_eq!(l[1]["range"], json!([0.25, 2.0]), "the values present, of pieces with an extent");
    assert_eq!((&l[2]["scale"], &l[2]["best"]), (&json!("values"), &json!("high")));
    assert_eq!(l[2]["words"], json!([{"value": 0.0, "label": "surface"}, {"value": 1.0, "label": "ground"}]));
    assert!(l[3]["range"].is_null(), "no piece states its accuracy");
    assert_eq!(
        (&l[4]["scale"], &l[4]["best"], &l[4]["range"]),
        (&json!("linear"), &json!("high"), &json!([2021.0, 2024.0]))
    );
    assert_eq!(d["availability"], json!(["open", "login", "paid", "closed"]));
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
