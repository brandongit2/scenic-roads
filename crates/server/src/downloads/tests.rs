use super::*;
use store::catalog::{FileRef, Layer};
use store::naming::{write_atomic, Source};
use store::pmtiles::Compression;

/// A box as a closed ring.
fn rect(w: f64, s: f64, e: f64, n: f64) -> Vec<[f64; 2]> {
    vec![[w, s], [e, s], [e, n], [w, n], [w, s]]
}

#[test]
fn the_tiles_an_area_meets() {
    // A box west of Greenwich, in the z6 tile 31/21: its hi data's margin (50 km) reaches over
    // the meridian into 32/21, its tiles' (2 km) doesn't.
    let london = rect(-0.3, 51.4, -0.1, 51.6);
    assert_eq!(tiles_meeting(std::slice::from_ref(&london), NEAR_KM), [(31, 21)].into());
    assert_eq!(tiles_meeting(std::slice::from_ref(&london), LISTS_KM), [(31, 21), (32, 21)].into());
    // A tile wholly inside an area is met, though no edge or vertex of it is in the tile.
    let big = rect(-40.0, 0.0, 40.0, 60.0);
    assert!(tiles_meeting(&[big], 0.0).contains(&(32, 26)));
    // Across the antimeridian (longitudes past 180): both sides.
    let fiji = rect(179.5, -16.5, 180.5, -15.5);
    assert_eq!(tiles_meeting(&[fiji], 0.0), [(0, 34), (63, 34)].into());
    // A ring only near a tile, beyond the margin, doesn't meet it; a broken ring meets none.
    assert!(!tiles_meeting(&[rect(5.0, 45.0, 5.1, 45.1)], NEAR_KM).contains(&(31, 22)));
    assert!(tiles_meeting(&[vec![[0.0, 0.0], [f64::NAN, 1.0], [1.0, 1.0]]], 1.0).is_empty());
}

/// Puts a file of `size` bytes on the "NAS" and lists it in `cat`.
fn put(root: &std::path::Path, cat: &mut Catalog, logical: &str, size: usize) -> String {
    let body: Vec<u8> = (0..size).map(|i| (i as u8).wrapping_mul(7).wrapping_add(logical.len() as u8)).collect();
    put_bytes(root, cat, logical, "bin", &body)
}

fn put_bytes(root: &std::path::Path, cat: &mut Catalog, logical: &str, ext: &str, body: &[u8]) -> String {
    let name = write_atomic(root, logical, ext, Source::Bytes(body)).unwrap();
    cat.files.insert(logical.into(), FileRef { file: name.clone(), size: body.len() as u64, fmt: 1, extra: Default::default() });
    name
}

fn gz(b: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
    e.write_all(b).unwrap();
    e.finish().unwrap()
}

/// A catalog of three z6 tiles' files (30/21, 31/21 and 32/21: the Atlantic off Ireland,
/// England, the Low Countries), a terrain layer's root and lo packs, a worldwide file, a basemap
/// (a tile at z0 and z10, and z11–14 tiles under each of the three), and one region, London,
/// recorded in its coverage.
fn catalog(root: &std::path::Path) -> Catalog {
    let mut c = Catalog::new(1);
    let mut terrain = Layer { encoding: "terrarium-png".into(), maxzoom: 12, ..Default::default() };
    let mut roads = Layer { encoding: "rt7".into(), minzoom: 4, maxzoom: 14, ..Default::default() };
    put(root, &mut c, "layers/terrain/root", 1_000);
    put(root, &mut c, "layers/terrain/lo/3-3-2", 2_000);
    terrain.root = Some("layers/terrain/root".into());
    terrain.lo.insert("3/3/2".into(), "layers/terrain/lo/3-3-2".into());
    let mut tiles: Vec<(u8, u32, u32, Vec<u8>)> = vec![(0, 0, 0, gz(b"world")), (10, 510, 340, gz(b"z10"))];
    for x in [30u32, 31, 32] {
        let (t, key) = (format!("6-{x}-21"), format!("6/{x}/21"));
        put(root, &mut c, &format!("layers/terrain/hi/{t}"), 10_000 + x as usize);
        put(root, &mut c, &format!("layers/roads/hi/{t}"), 20_000 + x as usize);
        put(root, &mut c, &format!("base/{t}"), 30_000 + x as usize);
        put(root, &mut c, &format!("global/roads/{t}"), 4_000 + x as usize);
        put(root, &mut c, &format!("hidata/{t}"), 5_000 + x as usize);
        terrain.hi.insert(key.clone(), format!("layers/terrain/hi/{t}"));
        roads.hi.insert(key.clone(), format!("layers/roads/hi/{t}"));
        c.base.insert(key.clone(), format!("base/{t}"));
        c.roads.insert(key.clone(), format!("global/roads/{t}"));
        c.hidata.insert(key, format!("hidata/{t}"));
        for z in 11..=14u8 {
            let s = z - 6;
            tiles.push((z, x << s, 21 << s, gz(format!("{z}/{x}").as_bytes())));
        }
    }
    c.layers.insert("terrain".into(), terrain);
    c.layers.insert("roads".into(), roads);
    put(root, &mut c, "global/railfreq", 300);
    c.global.insert("railfreq".into(), "global/railfreq".into());
    put_bytes(root, &mut c, "layers/basemap/world", "pmtiles", &store::pieces::archive(&tiles, Compression::Gzip));
    c.basemap = vec!["layers/basemap/world".into()];
    c.coverage = json!({"recorded": true, "regions": [
        {"id": "london", "name": "London", "outline": ["osm:175342"], "shapes": {"osm:175342": [[rect(-0.3, 51.4, -0.1, 51.6)]]}},
    ]});
    c.validate().unwrap();
    store::catalog::write(&root.join("catalog"), &c).unwrap();
    c
}

fn logicals(f: &Files) -> Vec<String> {
    let mut v: Vec<String> = f.names.iter().map(|(n, _)| store::naming::parse_content_name(n).unwrap().logical.to_string()).collect();
    v.sort();
    v
}

#[test]
fn an_areas_files_are_its_tiles_hi_packs_and_base_packs_and_the_hi_data_around() {
    let nas = tempfile::tempdir().unwrap();
    let cat = catalog(nas.path());
    let f = files_of(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);
    // Its tile's packs, the terrain of the tile within 25 km across the meridian (for the
    // viewshed), the hi data within 50 km.
    assert_eq!(logicals(&f), ["base/6-31-21", "global/roads/6-31-21", "hidata/6-31-21", "hidata/6-32-21", "layers/roads/hi/6-31-21", "layers/terrain/hi/6-31-21", "layers/terrain/hi/6-32-21"]);
    assert_eq!(f.bytes, 30_031 + 4_031 + 5_031 + 5_032 + 20_031 + 10_031 + 10_032);
    // And the basemap's zooms 11–14 over its tile, after its files.
    let p = area_part(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);
    assert_eq!(p.items.last(), Some(&Item::Piece { archive: cat.content("layers/basemap/world").unwrap().into(), piece: Piece::Tile(31, 21) }));
    assert_eq!(p.items.len(), 8);
}

async fn call(r: Response) -> (StatusCode, Value) {
    let code = r.status();
    let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
    (code, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn get(s: &S) -> Value {
    let (code, v) = call(get_status(State(s.clone())).await).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    v
}

async fn region(s: &S, id: &str, on: bool) -> (StatusCode, Value) {
    call(put_region(State(s.clone()), Path(id.into()), Json(On { on })).await).await
}

/// One round of the mirror thread, until it has nothing more to do.
fn settle(s: &S) {
    let m = s.data.mirror.clone().unwrap();
    while once(s, &m) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn downloading_the_world_regions_and_views() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let cat = catalog(nas.path());
    let s = crate::test_state_with(home.path(), nas.path(), true);
    let london = area_part(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);

    // Nothing downloaded: nothing copied; the sizes worked out, the basemap's pieces too.
    let v = get(&s).await;
    assert_eq!(v["world"]["on"], json!(false));
    assert!(v["world"]["unknown"].as_u64().unwrap() > 0 || v["world"]["bytes"].as_u64().unwrap() > 3_300);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    let m = s.data.mirror.clone().unwrap();
    assert_eq!(m.usage(), (0, 0), "nothing downloaded, nothing copied");
    let lo = m.piece_size(cat.content("layers/basemap/world").unwrap(), Piece::Lo).unwrap();
    let piece = m.piece_size(cat.content("layers/basemap/world").unwrap(), Piece::Tile(31, 21)).unwrap();
    assert_eq!(v["world"], json!({"bytes": 300 + 1_000 + 2_000 + lo, "here": 0, "unknown": 0, "on": false, "at": null, "state": null}));
    assert_eq!(v["regions"]["london"], json!({"name": "London", "bytes": london.files.bytes + piece, "here": 0, "unknown": 0, "on": false, "state": null}));

    // London downloaded: the World with it; written to downloads.json; copied by the thread.
    let (code, _) = region(&s, "london", true).await;
    assert_eq!(code, StatusCode::OK);
    let asked = Downloads::load(home.path()).asked();
    assert!(asked.world.is_some() && asked.regions.iter().map(|r| r.name.as_str()).eq(["London"]));
    let v = get(&s).await;
    assert_eq!((v["world"]["state"].as_str(), v["regions"]["london"]["state"].as_str()), (Some("queued"), Some("queued")));
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!((v["world"]["state"].as_str(), v["regions"]["london"]["state"].as_str()), (Some("done"), Some("done")));
    assert_eq!(v["regions"]["london"]["here"], v["regions"]["london"]["bytes"]);
    assert_eq!(v["wanted"]["here"], v["wanted"]["bytes"]);
    assert_eq!(v["here"].as_u64(), v["wanted"]["bytes"].as_u64(), "nothing else");

    // The World can't go while London needs it; London can.
    let (code, e) = call(put_world(State(s.clone()), Json(On { on: false })).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("Remove the downloaded regions"), "{e}");
    assert_eq!(region(&s, "london", false).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!((v["regions"]["london"]["here"].as_u64(), v["world"]["state"].as_str()), (Some(0), Some("done")));
    assert_eq!(call(put_world(State(s.clone()), Json(On { on: false })).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(m.usage(), (0, 0), "removed: nothing here");

    // A region the catalog doesn't have (not built yet): listed, nothing to copy.
    region(&s, "kanto", true).await;
    let v = get(&s).await;
    assert_eq!(v["regions"]["kanto"], json!({"name": "kanto", "bytes": 0, "here": 0, "unknown": 0, "on": true, "state": "missing"}));
    let (code, v) = region(&s, "../x", true).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
    region(&s, "kanto", false).await;

    // A view: its size first, then downloaded, renamed, removed. Named by where it is when the
    // place search knows no place in it (this catalog has no labels).
    let outline = rect(4.0, 51.7, 4.6, 52.1);
    let (code, v) = call(post_view_size(State(s.clone()), Json(Outline { outline: outline.clone() })).await).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["with_world"].as_u64(), v["fits"].as_bool()), (Some(0), Some(true)), "the World's here already");
    let (code, v) = call(post_view(State(s.clone()), Json(NewView { outline: outline.clone(), name: None })).await).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["name"].as_str(), Some("51.90° N, 4.30° E"));
    let id = v["id"].as_str().unwrap().to_string();
    let (code, _) = call(put_view(State(s.clone()), Path(id.clone()), Json(Rename { name: "  Rotterdam\n ".into() })).await).await;
    assert_eq!(code, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    let view = &v["views"][0];
    assert_eq!((view["id"].as_str(), view["name"].as_str(), view["state"].as_str()), (Some(id.as_str()), Some("Rotterdam"), Some("done")));
    for bad in [NewView { outline: outline[..2].to_vec(), name: None }, NewView { outline: vec![[0.0, 95.0]; 4], name: None }, NewView { outline: outline.clone(), name: Some(" ".into()) }] {
        assert_eq!(call(post_view(State(s.clone()), Json(bad)).await).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(call(delete_view(State(s.clone()), Path(id.clone())).await).await.0, StatusCode::OK);
    assert_eq!(call(delete_view(State(s.clone()), Path(id)).await).await.0, StatusCode::BAD_REQUEST);
    assert!(get(&s).await["views"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_that_wouldnt_fit_is_refused_with_the_numbers() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    catalog(nas.path());
    // A reserve no disk has: nothing fits.
    let data = Data::open(crate::data::Options { home: home.path().to_owned(), nas_root: Some(nas.path().to_owned()), mirror: true, reserve: u64::MAX / 4 }).unwrap();
    let s = crate::test_state_from(home.path(), data);
    let (code, v) = region(&s, "london", true).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let e = v["error"].as_str().unwrap();
    assert!(e.starts_with("Downloading “London”, with the World, zoomed out, takes ") && e.contains("it has 0 MB free above the reserve"), "{e}");
    assert_eq!(s.downloads.asked(), Asked::default(), "not downloaded");
    let (code, v) = call(put_world(State(s.clone()), Json(On { on: true })).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
    let (_, v) = call(post_view_size(State(s.clone()), Json(Outline { outline: rect(-0.3, 51.4, -0.1, 51.6) })).await).await;
    assert_eq!((v["room"].as_u64(), v["fits"].as_bool()), (Some(0), Some(false)));
    assert!(v["with_world"].as_u64().unwrap() > 3_300);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_mac_kept_becomes_downloads_and_the_rest_goes() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let cat = catalog(nas.path());
    // Before downloads: London kept, and the mirror holding all of the map (the old essentials,
    // London's, the rest, the whole basemap).
    std::fs::write(home.path().join(OLD_FILE), serde_json::to_vec(&json!({"fmt": 1, "regions": [{"id": "london", "name": "London", "at": 1}], "views": []})).unwrap()).unwrap();
    {
        let m = Mirror::open(home.path().to_owned(), 0).unwrap();
        let all = Wanted { items: cat.files.values().map(|f| Item::File { name: f.file.clone(), size: f.size }).collect() };
        let pool = store::IoPool::new(2, Duration::from_secs(5), nas.path().to_owned());
        m.sync(&cat, &all, nas.path(), &pool, &Control::FREE).unwrap();
    }
    let s = crate::test_state_with(home.path(), nas.path(), true);
    let asked = s.downloads.asked();
    assert!(asked.world.is_some() && asked.regions.len() == 1);
    assert!(!home.path().join(OLD_FILE).exists() && home.path().join(FILE).exists());
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    // The World's and London's stay, the rest went (the whole basemap among them); only the
    // pieces were copied.
    let v = get(&s).await;
    assert_eq!(v["wanted"]["here"], v["wanted"]["bytes"]);
    assert_eq!(v["here"], v["wanted"]["here"]);
    assert!(!m.has(cat.content("base/6-30-21").unwrap()) && !m.has(cat.content("layers/basemap/world").unwrap()));
    assert_eq!(m.usage().0, s.downloads.plan(&s.data).wanted.items.len());
}

#[test]
fn a_damaged_downloads_file_is_set_aside() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(FILE), b"{not json").unwrap();
    let d = Downloads::load(home.path());
    assert_eq!(d.asked(), Asked::default());
    assert!(home.path().join("downloads.json.bad").exists() && !home.path().join(FILE).exists());
}
