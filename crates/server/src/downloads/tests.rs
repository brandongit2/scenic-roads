use super::*;
use pipeline::areafiles::{clip, lat_y, lon_x, ring_area, LISTS_KM};
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
    cat.files.insert(
        logical.into(),
        FileRef { file: name.clone(), size: body.len() as u64, fmt: 1, extra: Default::default() },
    );
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
    let mut v: Vec<String> =
        f.names.iter().map(|(n, _)| store::naming::parse_content_name(n).unwrap().logical.to_string()).collect();
    v.sort();
    v
}

#[test]
fn an_areas_files_are_its_tiles_hi_packs_and_base_packs_and_the_hi_data_around() {
    let nas = tempfile::tempdir().unwrap();
    let cat = catalog(nas.path());
    let f = files_of(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);
    // Its tile's packs, the terrain of the tile within 25 km across the meridian (for the
    // viewshed), the hi data within 50 km, the worldwide file (read zoomed in too).
    assert_eq!(
        logicals(&f),
        [
            "base/6-31-21",
            "global/railfreq",
            "global/roads/6-31-21",
            "hidata/6-31-21",
            "hidata/6-32-21",
            "layers/roads/hi/6-31-21",
            "layers/terrain/hi/6-31-21",
            "layers/terrain/hi/6-32-21"
        ]
    );
    assert_eq!(f.bytes, 30_031 + 300 + 4_031 + 5_031 + 5_032 + 20_031 + 10_031 + 10_032);
    // And the basemap's zooms 11–14 over its tile, after its files.
    let p = area_part(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);
    assert_eq!(
        p.items.last(),
        Some(&Item::Piece { archive: cat.content("layers/basemap/world").unwrap().into(), piece: Piece::Tile(31, 21) })
    );
    assert_eq!(p.items.len(), 9);
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
async fn downloading_the_world_and_regions() {
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
    assert_eq!(
        v["world"],
        json!({"bytes": 300 + 1_000 + 2_000 + lo, "here": 0, "unknown": 0, "on": false, "at": null, "state": null})
    );
    assert_eq!(
        v["regions"]["london"],
        json!({"name": "London", "bytes": london.files.bytes + piece, "here": 0, "unknown": 0, "on": false, "state": null})
    );

    // London downloaded: not the World with it; written to downloads.json; copied by the thread.
    let (code, _) = region(&s, "london", true).await;
    assert_eq!(code, StatusCode::OK);
    let asked = Downloads::load(home.path()).asked();
    assert!(asked.world.is_none() && asked.regions.iter().map(|r| r.name.as_str()).eq(["London"]));
    let v = get(&s).await;
    assert_eq!((v["world"]["on"].as_bool(), v["regions"]["london"]["state"].as_str()), (Some(false), Some("queued")));
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!(v["regions"]["london"]["state"].as_str(), Some("done"));
    assert_eq!(v["here"].as_u64(), v["regions"]["london"]["bytes"].as_u64(), "London alone");
    assert_eq!(call(put_world(State(s.clone()), Json(On { on: true })).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!((v["world"]["state"].as_str(), v["regions"]["london"]["state"].as_str()), (Some("done"), Some("done")));
    assert_eq!(v["regions"]["london"]["here"], v["regions"]["london"]["bytes"]);
    assert_eq!(v["wanted"]["here"], v["wanted"]["bytes"]);
    assert_eq!(v["here"].as_u64(), v["wanted"]["bytes"].as_u64(), "nothing else");

    // Either goes on its own: the World while London stays, then London.
    assert_eq!(call(put_world(State(s.clone()), Json(On { on: false })).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!(
        (v["regions"]["london"]["state"].as_str(), v["world"]["here"].as_u64()),
        (Some("done"), Some(300)),
        "London keeps the worldwide file"
    );
    assert_eq!(v["here"].as_u64(), v["regions"]["london"]["bytes"].as_u64(), "the rest of the World went");
    assert_eq!(region(&s, "london", false).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(m.usage(), (0, 0), "removed: nothing here");

    // A region the catalog doesn't have (not built yet): listed, nothing to copy.
    region(&s, "kanto", true).await;
    let v = get(&s).await;
    assert_eq!(
        v["regions"]["kanto"],
        json!({"name": "kanto", "bytes": 0, "here": 0, "unknown": 0, "on": true, "state": "missing"})
    );
    let (code, v) = region(&s, "../x", true).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
    region(&s, "kanto", false).await;
    assert!(get(&s).await.get("views").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_that_wouldnt_fit_is_refused_with_the_numbers() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    catalog(nas.path());
    // A reserve no disk has: nothing fits.
    let data = Data::open(crate::data::Options {
        home: home.path().to_owned(),
        nas_root: Some(nas.path().to_owned()),
        mirror: true,
        reserve: u64::MAX / 4,
    })
    .unwrap();
    let s = crate::test_state_from(home.path(), data);
    let (code, v) = region(&s, "london", true).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let e = v["error"].as_str().unwrap();
    assert!(e.starts_with("Downloading “London” takes ") && e.contains("it has 0 MB free above the reserve"), "{e}");
    assert_eq!(s.downloads.asked(), Asked::default(), "not downloaded");
    let (code, v) = call(put_world(State(s.clone()), Json(On { on: true })).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_macs_downloaded_views_are_removed_and_their_files_go() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let cat = catalog(nas.path());
    // Before views went: London and a view over Rotterdam downloaded, with the World, all here.
    let view = rect(4.0, 51.7, 4.6, 52.1);
    std::fs::write(
        home.path().join(FILE),
        serde_json::to_vec(&json!({"fmt": 2, "world": 1, "regions": [{"id": "london", "name": "London", "at": 1}],
            "views": [{"id": "v1", "name": "Rotterdam", "outline": view, "at": 1}]}))
        .unwrap(),
    )
    .unwrap();
    let rotterdam = area_part(&cat, std::slice::from_ref(&view));
    {
        let m = Mirror::open(home.path().to_owned(), 0).unwrap();
        let all =
            Wanted { items: cat.files.values().map(|f| Item::File { name: f.file.clone(), size: f.size }).collect() };
        let pool = store::IoPool::new(2, Duration::from_secs(60), nas.path().to_owned());
        m.sync(&cat, &all, nas.path(), &pool, &Control::FREE).unwrap();
    }
    let s = crate::test_state_with(home.path(), nas.path(), true);
    let asked = s.downloads.asked();
    assert!(asked.world.is_some() && asked.regions.len() == 1);
    let on_disk: Value = serde_json::from_slice(&std::fs::read(home.path().join(FILE)).unwrap()).unwrap();
    assert_eq!((on_disk["fmt"].as_u64(), on_disk.get("views")), (Some(3), None), "written again without them");
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    // The World's and London's stay; the view's files no region needs went.
    let plan = s.downloads.plan(&s.data);
    let gone: Vec<&String> = rotterdam
        .files
        .names
        .iter()
        .map(|(n, _)| n)
        .filter(|n| !plan.wanted.items.iter().any(|i| store::mirror::copy_name(i) == n.as_str()))
        .collect();
    assert!(!gone.is_empty() && gone.iter().all(|n| !m.has(n)), "{gone:?}");
    let v = get(&s).await;
    assert_eq!(v["here"], v["wanted"]["here"]);
    assert_eq!(m.usage().0, plan.wanted.items.len());
}

#[test]
fn a_damaged_downloads_file_is_set_aside() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(FILE), b"{not json").unwrap();
    let d = Downloads::load(home.path());
    assert_eq!(d.asked(), Asked::default());
    assert!(home.path().join("downloads.json.bad").exists() && !home.path().join(FILE).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reserve_is_set_kept_and_bounded_and_the_mirror_takes_it_at_once() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    catalog(nas.path());
    let data = Data::open(crate::data::Options {
        home: home.path().to_owned(),
        nas_root: Some(nas.path().to_owned()),
        mirror: true,
        reserve: crate::reserve::load(home.path()) * 1_000_000_000,
    })
    .unwrap();
    let s = crate::test_state_from(home.path(), data);
    let m = s.data.mirror.clone().unwrap();
    assert_eq!(m.reserve(), 30_000_000_000, "unset: 30 GB");
    let disk = 500_000_000_000;
    for bad in [0, 4, 500, 900] {
        assert!(tokio::task::block_in_place(|| set_reserve(&s, bad, disk)).is_err(), "{bad} GB");
    }
    assert_eq!(m.reserve(), 30_000_000_000, "refused: unchanged");
    assert!(!home.path().join(crate::reserve::FILE).exists());
    let v = tokio::task::block_in_place(|| set_reserve(&s, 12, disk)).unwrap();
    assert_eq!(v["reserve"].as_u64(), Some(12_000_000_000));
    assert_eq!(m.reserve(), 12_000_000_000, "the mirror's, without a restart");
    assert_eq!(get(&s).await["reserve"].as_u64(), Some(12_000_000_000));
    assert_eq!(crate::reserve::load(home.path()), 12, "kept for the next start");
    // Raised: taken as it is.
    tokio::task::block_in_place(|| set_reserve(&s, 200, disk)).unwrap();
    assert_eq!(m.reserve(), 200_000_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agents_room_target_above_the_reserve_is_the_mirrors_reserve() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    catalog(nas.path());
    // This Mac's agent keeps 40 GiB free (`scenic room 40`), above the 30 GB reserve.
    std::fs::create_dir_all(home.path().join("agent")).unwrap();
    pipeline::agent::settings::set_room(&home.path().join("agent"), "40", "t").unwrap();
    let data = Data::open(crate::data::Options {
        home: home.path().to_owned(),
        nas_root: Some(nas.path().to_owned()),
        mirror: true,
        reserve: 30_000_000_000,
    })
    .unwrap();
    let s = crate::test_state_from(home.path(), data);
    let m = s.data.mirror.clone().unwrap();
    assert_eq!(m.reserve(), 40 << 30);
    let v = get(&s).await;
    assert_eq!((v["reserve"].as_u64(), v["reserve_set"].as_u64()), (Some(40 << 30), Some(30_000_000_000)));
    // Set above the target: the setting.
    tokio::task::block_in_place(|| set_reserve(&s, 100, 500_000_000_000)).unwrap();
    assert_eq!(m.reserve(), 100_000_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_region_still_building_downloads_as_its_tiles_come() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // A region over 31/21 and 32/21 (London to Kent), still building: catalog 1 has 31/21's files
    // alone, catalog 2 32/21's too.
    let full = catalog(nas.path());
    let dir = nas.path().join("catalog");
    std::fs::remove_dir_all(&dir).unwrap();
    let se = rect(-0.3, 51.4, 0.3, 51.6);
    let coverage = |built: u32| {
        json!({"recorded": true, "regions": [
            {"id": "se", "name": "South East", "outline": ["osm:1"], "shapes": {"osm:1": [[se.clone()]]},
             "tiles": {"built": built, "total": 2}},
        ]})
    };
    let mut first = full.clone();
    first.files.retain(|l, _| !l.contains("6-32-21"));
    for l in first.layers.values_mut() {
        l.hi.remove("6/32/21");
    }
    for m in [&mut first.base, &mut first.roads, &mut first.hidata] {
        m.remove("6/32/21");
    }
    first.coverage = coverage(1);
    first.validate().unwrap();
    store::catalog::write(&dir, &first).unwrap();
    let s = crate::test_state_with(home.path(), nas.path(), true);
    assert_eq!(s.data.catalog().n, 1);

    // Downloaded while building: listed with its tiles, catalog 1's files copied.
    let v = get(&s).await;
    assert_eq!(v["regions"]["se"]["tiles"], json!({"built": 1, "total": 2}));
    assert_eq!(region(&s, "se", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    let one = area_part(&first, std::slice::from_ref(&se));
    let v = get(&s).await;
    assert_eq!(v["regions"]["se"]["state"].as_str(), Some("done"), "{v}");
    assert!(logicals(&one.files).iter().all(|l| !l.contains("6-32-21")));

    // Catalog 2: the region's other tile built. The wanted set grows by its files, and only those
    // are copied: nothing here already is fetched again.
    let mut second = full.clone();
    second.n = 2;
    second.coverage = coverage(2);
    store::catalog::write(&dir, &second).unwrap();
    assert!(tokio::task::block_in_place(|| s.data.refresh_catalog()));
    assert_eq!(s.data.catalog().n, 2);
    let two = area_part(&second, std::slice::from_ref(&se));
    let added: Vec<(String, u64)> = two.files.names.iter().filter(|(n, _)| !one.files.contains(n)).cloned().collect();
    assert!(!added.is_empty() && one.files.names.iter().all(|(n, _)| two.files.contains(n)), "the wanted set grows");
    tokio::task::block_in_place(|| settle(&s));
    let (st, _) = m.last().unwrap();
    assert_eq!(
        (st.copied as usize, st.copied_bytes),
        (added.len(), added.iter().map(|(_, b)| b).sum::<u64>()),
        "only the new tile's files copied: {st:?}"
    );
    let v = get(&s).await;
    assert_eq!(v["regions"]["se"]["state"].as_str(), Some("done"), "{v}");
    assert_eq!(v["regions"]["se"]["tiles"], json!({"built": 2, "total": 2}));
    assert_eq!(v["regions"]["se"]["here"], v["regions"]["se"]["bytes"]);
}

/// A deep terrain pack (pipeline::terrain_deep) of one z13 tile `h` metres high, under z10 tile
/// (x, y), put on the "NAS" and named in `cat`'s terrain.
fn put_deep(root: &std::path::Path, cat: &mut Catalog, x: u32, y: u32, h: f32) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("p.pack");
    let mut w = store::pack::PackWriter::create(&path, json!({"layer": "terrain"}), false).unwrap();
    let b = pipeline::terrain_deep::encode(&mut vec![h; 65536]).unwrap();
    w.add(13, x * 8, y * 8, &b, b.len() as u32).unwrap();
    w.finish().unwrap();
    let logical = format!("layers/terrain/deep/10-{x}-{y}");
    put_bytes(root, cat, &logical, "pack", &std::fs::read(&path).unwrap());
    let t = cat.layers.get_mut("terrain").unwrap();
    t.deep.insert(format!("10/{x}/{y}"), logical);
    t.native_zoom.insert(format!("10/{x}/{y}"), 13);
}

#[test]
fn a_regions_terrain_detail_is_the_deep_packs_near_it_and_the_share_they_cover() {
    let nas = tempfile::tempdir().unwrap();
    let mut cat = catalog(nas.path());
    // A box from London to Sussex, in z10 tiles x 511, y 340 and 341. One deep pack under its
    // north (row 340), one an ocean away.
    put_deep(nas.path(), &mut cat, 511, 340, 100.0);
    put_deep(nas.path(), &mut cat, 600, 340, 1.0);
    let (f, cover) = detail_of(&cat, &[rect(-0.3, 51.2, -0.1, 51.6)]);
    assert_eq!(logicals(&f), ["layers/terrain/deep/10-511-340"]);
    // Its share: the part of the box north of row 341's edge (in Web Mercator).
    let (y0, y1) = (lat_y(51.6) * 16.0, lat_y(51.2) * 16.0);
    assert!(y0 < 341.0 && y1 > 341.0 && lon_x(-0.3) * 16.0 > 511.0 && lon_x(-0.1) * 16.0 < 512.0);
    let want = (341.0 - y0) / (y1 - y0) * 100.0;
    assert!((cover - want).abs() < 1e-6, "{cover} {want}");
    // London, wholly in that deep tile: all of it; a region with none: nothing.
    assert!((detail_of(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]).1 - 100.0).abs() < 1e-6);
    assert_eq!(detail_of(&cat, &[rect(5.0, 45.0, 5.1, 45.1)]).0, Files::default());
    // Clipping keeps a ring's area within the box, a ring in it whole.
    assert!((ring_area(&clip(&rect(0.0, 0.0, 2.0, 2.0), [1.0, 1.0, 3.0, 3.0])) - 1.0).abs() < 1e-12);
    assert!((ring_area(&clip(&rect(0.2, 0.2, 0.4, 0.4), [0.0, 0.0, 1.0, 1.0])) - 0.04).abs() < 1e-12);
}

#[tokio::test(flavor = "multi_thread")]
async fn terrain_detail_downloads_with_its_region_and_is_read_from_this_mac() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut cat = catalog(nas.path());
    put_deep(nas.path(), &mut cat, 511, 340, 100.0);
    cat.n = 2;
    cat.validate().unwrap();
    store::catalog::write(&nas.path().join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), nas.path(), true);
    let detail = |s: &S, on: bool| {
        let s = s.clone();
        async move { call(put_region_detail(State(s), Path("london".into()), Json(On { on })).await).await }
    };
    let pack = cat.files["layers/terrain/deep/10-511-340"].clone();

    // Before: listed under London with its size and cover, not downloaded.
    let v = get(&s).await;
    let f = &v["regions"]["london"]["detail"];
    assert_eq!(
        (f["bytes"].as_u64(), f["on"].as_bool(), f["state"].as_str()),
        (Some(pack.size), Some(false), None),
        "{v}"
    );
    assert!(f["cover"].as_f64().unwrap() > 50.0, "{f}");

    // Refused until London is downloaded.
    let (code, v) = detail(&s, true).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Download the region first"), "{v}");
    assert_eq!(s.downloads.asked(), Asked::default());

    // Downloaded, after London: copied after it; the server reads it here.
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    assert_eq!(detail(&s, true).await.0, StatusCode::OK);
    let asked = s.downloads.asked();
    assert!(asked.world.is_none() && asked.regions.len() == 1 && asked.regions[0].detail);
    let plan = s.downloads.plan(&s.data);
    assert_eq!(plan.wanted.items.last(), Some(&Item::File { name: pack.file.clone(), size: pack.size }));
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!(v["regions"]["london"]["detail"]["state"].as_str(), Some("done"), "{v}");
    assert_eq!(v["wanted"]["here"], v["wanted"]["bytes"], "counted with the rest");
    let (on_nas, aside) = (nas.path().join(&pack.file), nas.path().join("aside"));
    std::fs::rename(&on_nas, &aside).unwrap();
    let h = tokio::task::block_in_place(|| crate::terrain::terrain_f32(&s, 13, 511 * 8, 340 * 8)).unwrap();
    assert_eq!(h.map(|e| e[0]), Some(100.0), "the deep tile, from this Mac");
    std::fs::rename(&aside, &on_nas).unwrap();

    // London downloaded again keeps it; its terrain detail removed, London stays; London removed,
    // both go.
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    assert!(s.downloads.asked().regions[0].detail);
    assert_eq!(detail(&s, false).await.0, StatusCode::OK);
    let asked = s.downloads.asked();
    assert!(asked.regions.len() == 1 && !asked.regions[0].detail);
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    assert!(!m.has(&pack.file) && m.has(cat.content("layers/terrain/hi/6-31-21").unwrap()));
    assert_eq!(detail(&s, true).await.0, StatusCode::OK);
    assert_eq!(region(&s, "london", false).await.0, StatusCode::OK);
    assert!(s.downloads.asked().regions.is_empty());
    // A downloaded region with no terrain detail: refused.
    region(&s, "kanto", true).await;
    let (code, v) = call(put_region_detail(State(s.clone()), Path("kanto".into()), Json(On { on: true })).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("no terrain detail"), "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_region_alone_has_what_the_map_reads_zoomed_in() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut cat = catalog(nas.path());
    // The World's per-tile records over London (landmarks, area details, roads' English names), and
    // one far away.
    put(nas.path(), &mut cat, "markdata/6-31-21", 700);
    cat.markdata.insert("6/31/21".into(), "markdata/6-31-21".into());
    put(nas.path(), &mut cat, "ovdata/3-3-2", 800);
    cat.ovdata.insert("3/3/2".into(), "ovdata/3-3-2".into());
    for t in ["6-31-21", "6-10-10"] {
        put(nas.path(), &mut cat, &format!("global/roaden/{t}"), 90);
        cat.global.insert(format!("roaden/{t}"), format!("global/roaden/{t}"));
    }
    cat.n = 2;
    cat.validate().unwrap();
    store::catalog::write(&nas.path().join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), nas.path(), true);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    assert!(s.downloads.asked().world.is_none());
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    for l in ["markdata/6-31-21", "ovdata/3-3-2", "global/roaden/6-31-21", "global/railfreq"] {
        assert!(m.has(cat.content(l).unwrap()), "{l} here with London alone");
    }
    assert!(!m.has(cat.content("global/roaden/6-10-10").unwrap()), "not a far tile's");
    assert!(!m.has(cat.content("layers/terrain/lo/3-3-2").unwrap()), "nor the World's zoomed-out packs");
    let v = get(&s).await;
    assert_eq!(v["regions"]["london"]["here"], v["regions"]["london"]["bytes"], "counted in its size");
}
