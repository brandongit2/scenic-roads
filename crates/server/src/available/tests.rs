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
            ("pole", json!({"spacing_m": 2, "surface": "surface"}), Some(json!([[[-180.0, 86.0], [180.0, 86.0], [180.0, 90.0], [-180.0, 90.0], [-180.0, 86.0]]]))),
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
    // Only what downloads freely: the open piece's; every one: the finer piece's score.
    assert_eq!(t.best(SCORE_LAYER, both, false), a.score);
    assert_eq!(t.best(SCORE_LAYER, both, true), b.score);
    assert!(b.score > a.score);
    // A metric's layer: the best value among those stating it (the year: the open piece alone).
    assert_eq!(t.best("year", both, true), a.values["year"]);
    assert_eq!(t.best("year", ["fine/dtm"], true), None, "a piece not stating it takes no part");
    assert_eq!(t.best("spacing_m", both, true), b.values["spacing_m"]);
    assert_eq!(t.best(SCORE_LAYER, ["fine/dtm"], false), None);
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
    // The spacing's stops run from its worst to its best.
    assert_eq!(d["layers"][1]["stops"][0]["label"], "1000");
    assert_eq!(d["layers"][1]["stops"][4]["label"], "0.1");
}

#[test]
fn the_ramp_runs_dark_to_light() {
    assert_eq!(colour(0.0), [68, 1, 84]);
    assert_eq!(colour(1.0), [253, 231, 37]);
    assert_eq!(colour(0.5), [33, 145, 140]);
    assert_eq!(colour(2.0), colour(1.0));
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
    let c = colour(t.pieces["open-lidar/dtm"].score.unwrap());
    assert_eq!(png_pixel(&open, 128, 128), [c[0], c[1], c[2], 255]);
    let all = body(get("score", 12, x, y, Some("avail=all"), headers(None)).await).await;
    let c = colour(t.pieces["fine/dtm"].score.unwrap());
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
    let c = colour(t.pieces["open-lidar/pole"].score.unwrap());
    assert_eq!(png_pixel(&body(r).await, 128, 128), [c[0], c[1], c[2], 255]);
    assert_eq!(polar(4, 0, 0).await.status(), StatusCode::NOT_FOUND);
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
    let ids = |v: &Value| v["pieces"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let v: Value = serde_json::from_slice(&body(click(2.3, 44.2, Some("all")).await).await).unwrap();
    assert_eq!(ids(&v), ["fine/dtm", "open-lidar/dtm"]);
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
