use super::*;
use pipeline::areafiles::{clip, lat_y, lon_x, ring_area, LISTS_KM};
use store::catalogue::{FileRef, Layer};
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
fn put(root: &store::nasfs::NasRoot, cat: &mut Catalogue, logical: &str, size: usize) -> String {
    let body: Vec<u8> = (0..size).map(|i| (i as u8).wrapping_mul(7).wrapping_add(logical.len() as u8)).collect();
    put_bytes(root, cat, logical, "bin", &body)
}

fn put_bytes(root: &store::nasfs::NasRoot, cat: &mut Catalogue, logical: &str, ext: &str, body: &[u8]) -> String {
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

/// A catalogue of three z6 tiles' files (30/21, 31/21 and 32/21: the Atlantic off Ireland,
/// England, the Low Countries), a terrain layer's root and lo packs, a worldwide file, a basemap
/// (a tile at z0 and z10, and z11–14 tiles under each of the three), and one region, London,
/// recorded in its coverage.
fn catalogue(root: &store::nasfs::NasRoot) -> Catalogue {
    let mut c = Catalogue::new(1);
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
        {"id": "east", "name": "East", "outline": ["osm:1"], "shapes": {"osm:1": [[rect(-0.06, 51.4, -0.03, 51.6)]]}},
    ]});
    c.validate().unwrap();
    store::catalogue::write(&root.unchecked_path().join("catalog"), &c).unwrap();
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
    let cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
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
    let p = area_part(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)], &BTreeSet::new());
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
    let cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
    let london = area_part(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)], &BTreeSet::new());

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

    // A region the catalogue doesn't have (not built yet): listed, nothing to copy.
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
    catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    // A reserve no disk has: nothing fits.
    let data = Data::open(crate::data::Options {
        home: home.path().to_owned(),
        nas_root: Some(store::nasfs::NasRoot::local_for_test(nas.path().to_owned())),
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
    let cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    // Before views went: London and a view over Rotterdam downloaded, with the World, all here.
    let view = rect(4.0, 51.7, 4.6, 52.1);
    std::fs::write(
        home.path().join(FILE),
        serde_json::to_vec(&json!({"fmt": 2, "world": 1, "regions": [{"id": "london", "name": "London", "at": 1}],
            "views": [{"id": "v1", "name": "Rotterdam", "outline": view, "at": 1}]}))
        .unwrap(),
    )
    .unwrap();
    let rotterdam = area_part(&cat, std::slice::from_ref(&view), &BTreeSet::new());
    {
        let m = Mirror::open(home.path().to_owned(), 0).unwrap();
        let all =
            Wanted { items: cat.files.values().map(|f| Item::File { name: f.file.clone(), size: f.size }).collect() };
        let pool = store::IoPool::new(2, Duration::from_secs(60), nas.path().to_owned());
        m.sync(&cat, &all, &store::nasfs::NasRoot::local_for_test(nas.path()), &pool, &Control::FREE).unwrap();
    }
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
    let asked = s.downloads.asked();
    assert!(asked.world.is_some() && asked.regions.len() == 1);
    let on_disk: Value = serde_json::from_slice(&std::fs::read(home.path().join(FILE)).unwrap()).unwrap();
    assert_eq!((on_disk["fmt"].as_u64(), on_disk.get("views")), (Some(4), None), "written again without them");
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
        // (Base packs and road values stay: this catalogue's hidata can't be read for its owners.)
        .filter(|n| !n.starts_with("base/") && !n.starts_with("global/roads/"))
        .collect();
    assert!(!gone.is_empty() && gone.iter().all(|n| !m.has(n)), "{gone:?}");
    // Besides what's wanted, only the base packs and road values kept (as above).
    let kept: Vec<&FileRef> = cat
        .base
        .values()
        .chain(cat.roads.values())
        .map(|l| &cat.files[l])
        .filter(|f| m.has(&f.file) && !plan.wanted.items.iter().any(|i| store::mirror::copy_name(i) == f.file))
        .collect();
    let v = get(&s).await;
    assert_eq!(
        v["here"].as_u64().unwrap(),
        v["wanted"]["here"].as_u64().unwrap() + kept.iter().map(|f| f.size).sum::<u64>()
    );
    assert_eq!(m.usage().0, plan.wanted.items.len() + kept.len());
}

#[test]
fn a_damaged_downloads_file_is_set_aside() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(FILE), b"{not json").unwrap();
    let d = Downloads::load(home.path());
    assert_eq!(d.asked(), Asked::default());
    assert!(home.path().join("downloads.json.bad").exists() && !home.path().join(FILE).exists());
}

/// A downloads file of format 3 (and one from before one terrain, task #633, with `fine`) is
/// written again as format 4: the World's time and each region's terrain detail kept, every download
/// pinned to the catalogue served at the mirror's next round.
#[tokio::test(flavor = "multi_thread")]
async fn a_downloads_file_of_format_3_is_upgraded_and_pinned_to_the_catalogue_served() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    let old = r#"{"fmt":3,"world":5,"regions":[{"id":"london","name":"London","at":1,"fine":true},
        {"id":"east","name":"East","at":2,"detail":false}]}"#;
    std::fs::write(home.path().join(FILE), old).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
    let a = s.downloads.asked();
    assert_eq!(a.world, Some(AskedWorld { at: 5, pin: Pin::default() }));
    let detail: Vec<Option<Pin>> = a.regions.iter().map(|r| r.detail).collect();
    assert_eq!(detail, [Some(Pin::default()), None]);
    let back: Value = serde_json::from_slice(&std::fs::read(home.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(back["fmt"], 4);
    assert!(back["regions"][0].get("fine").is_none());
    tokio::task::block_in_place(|| settle(&s));
    let a = Downloads::load(home.path()).asked();
    let pins: Vec<Pin> = a.pins().into_iter().map(|(_, p)| p).collect();
    assert_eq!(pins, [Pin { cat: 1, to: None }; 4], "the World, both regions, London's detail");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reserve_is_set_kept_and_bounded_and_the_mirror_takes_it_at_once() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    let data = Data::open(crate::data::Options {
        home: home.path().to_owned(),
        nas_root: Some(store::nasfs::NasRoot::local_for_test(nas.path().to_owned())),
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
    catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    // This Mac's agent keeps 40 GiB free (`scenic room 40`), above the 30 GB reserve.
    std::fs::create_dir_all(home.path().join("agent")).unwrap();
    pipeline::agent::settings::set_room(&home.path().join("agent"), "40", "t").unwrap();
    let data = Data::open(crate::data::Options {
        home: home.path().to_owned(),
        nas_root: Some(store::nasfs::NasRoot::local_for_test(nas.path().to_owned())),
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

/// Downloads update only when asked: a new catalogue copies nothing; the download stays at its own,
/// read from this Mac (its older files first), and says what an update brings; Update copies those
/// beside the old ones (the region works from them meanwhile), then moves the download on and lets
/// the old ones go.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_catalogue_copies_nothing_until_the_download_is_updated() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    // A region over 31/21 and 32/21 (London to Kent), still building: catalogue 1 has 31/21's files
    // alone; catalogue 2 32/21's too, and 31/21's roads made again.
    let full = catalogue(&root);
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
    store::catalogue::write(&dir, &first).unwrap();
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(s.data.catalogue().n, 1);
    let m = s.data.mirror.clone().unwrap();

    // Downloaded while building: listed with its tiles, catalogue 1's files copied.
    assert_eq!(get(&s).await["regions"]["se"]["tiles"], json!({"built": 1, "total": 2}));
    assert_eq!(region(&s, "se", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let one = area_part(&first, std::slice::from_ref(&se), &BTreeSet::new());
    let v = get(&s).await;
    assert_eq!(
        (v["regions"]["se"]["state"].as_str(), v["regions"]["se"]["cat"].as_u64()),
        (Some("done"), Some(1)),
        "{v}"
    );
    assert!(v["regions"]["se"].get("behind").is_none(), "{v}");
    let roads = "layers/roads/hi/6-31-21";
    let old_roads = first.files[roads].file.clone();

    // Catalogue 2: nothing copied; the region stays at 1, behind, read from its own files here.
    let mut second = full.clone();
    second.n = 2;
    second.coverage = coverage(2);
    put(&root, &mut second, roads, 25_000);
    second.validate().unwrap();
    store::catalogue::write(&dir, &second).unwrap();
    assert!(tokio::task::block_in_place(|| s.data.refresh_catalogue()));
    assert_eq!(s.data.catalogue().n, 2);
    tokio::task::block_in_place(|| settle(&s));
    let (st, _) = m.last().unwrap();
    assert_eq!((st.copied, st.removed), (0, 0), "{st:?}");
    let two = area_part(&second, std::slice::from_ref(&se), &BTreeSet::new());
    let new: Vec<&Item> = two.items.iter().filter(|i| !one.items.contains(i)).collect();
    let new_bytes: u64 = new.iter().map(|i| m.item_size(i).unwrap()).sum();
    let v = get(&s).await;
    let r = &v["regions"]["se"];
    assert_eq!(
        (r["state"].as_str(), r["cat"].as_u64(), r["here"].as_u64()),
        (Some("done"), Some(1), Some(r["bytes"].as_u64().unwrap())),
        "{v}"
    );
    assert_eq!(r["behind"]["cat"], 2);
    assert_eq!(r["behind"]["files"].as_u64(), Some(new.len() as u64));
    assert_eq!((r["behind"]["gone"].as_u64(), r["behind"]["bytes"].as_u64()), (Some(1), Some(new_bytes)), "{r}");
    assert_eq!(r["behind"]["what"], json!(["lists", "road data", "roads", "terrain"]));
    assert_eq!(s.data.content(roads), Some(old_roads.clone()), "its own older file, here");
    assert!(s.data.mirror.as_ref().unwrap().catalogue(1).is_some(), "catalogue 1 kept");

    // Update, with one new file missing on the NAS: the rest copied beside the old ones; still at 1,
    // read as before.
    let new_roads = second.files[roads].file.clone();
    let aside = nas.path().join("aside");
    std::fs::rename(nas.path().join(&new_roads), &aside).unwrap();
    let (code, v) = call(post_region_update(State(s.clone()), Path("se".into())).await).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    tokio::task::block_in_place(|| settle(&s));
    let a = s.downloads.asked();
    assert_eq!(a.regions[0].pin, Pin { cat: 1, to: Some(2) });
    let v = get(&s).await;
    let r = &v["regions"]["se"];
    assert_eq!(r["state"].as_str(), Some("updating"), "{v}");
    let u = &r["update"];
    assert!(u["here"].as_u64() < u["bytes"].as_u64() && u["here"].as_u64() > Some(0), "{u}");
    assert!(m.has(&old_roads) && !m.has(&new_roads));
    assert_eq!(s.data.content(roads), Some(old_roads.clone()), "mid-update: the old file");
    // (Asked again meanwhile: nothing changes.)
    assert_eq!(call(post_region_update(State(s.clone()), Path("se".into())).await).await.0, StatusCode::OK);

    // All here: moved on to 2; the old file goes; the new one read.
    std::fs::rename(&aside, nas.path().join(&new_roads)).unwrap();
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 2, to: None });
    assert!(!m.has(&old_roads) && m.has(&new_roads));
    assert_eq!(s.data.content(roads), Some(new_roads));
    let v = get(&s).await;
    let r = &v["regions"]["se"];
    assert_eq!((r["state"].as_str(), r["cat"].as_u64()), (Some("done"), Some(2)), "{v}");
    assert!(r.get("behind").is_none() && r.get("update").is_none(), "{v}");
    assert_eq!(r["here"], r["bytes"]);
    assert_eq!(r["tiles"], json!({"built": 2, "total": 2}));
    // Not behind: refused.
    let (code, v) = call(post_region_update(State(s.clone()), Path("se".into())).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");

    // Removed: its files go.
    assert_eq!(region(&s, "se", false).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(m.usage(), (0, 0));
}

/// A download whose catalogue's files for it are the same as the served one's moves on by itself
/// (nothing to copy), as does one its catalogue didn't have yet (nothing to keep).
#[tokio::test(flavor = "multi_thread")]
async fn a_download_with_nothing_to_copy_moves_on_by_itself() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    let cat = catalogue(&root);
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    region(&s, "kanto", true).await;
    tokio::task::block_in_place(|| settle(&s));
    // Catalogue 2: a far tile changed, and Kanto built.
    let mut two = cat.clone();
    two.n = 2;
    put(&root, &mut two, "layers/roads/hi/6-30-21", 99);
    let mut cov = two.coverage.clone();
    cov["regions"].as_array_mut().unwrap().push(
        json!({"id": "kanto", "name": "Kanto", "outline": ["osm:2"], "shapes": {"osm:2": [[rect(-0.06, 51.45, -0.05, 51.5)]]}}),
    );
    two.coverage = cov;
    two.validate().unwrap();
    store::catalogue::write(&nas.path().join("catalog"), &two).unwrap();
    assert!(tokio::task::block_in_place(|| s.data.refresh_catalogue()));
    tokio::task::block_in_place(|| settle(&s));
    let pins: Vec<Pin> = s.downloads.asked().regions.iter().map(|r| r.pin).collect();
    assert_eq!(pins, [Pin { cat: 2, to: None }; 2]);
    let v = get(&s).await;
    assert_eq!(v["regions"]["kanto"]["state"].as_str(), Some("done"), "{v}");
}

/// A download not all here when a new catalogue comes has nothing whole to keep: it moves on to
/// the new one, and the map reads none of its older files meanwhile (they'd mix with the NAS's).
#[tokio::test(flavor = "multi_thread")]
async fn a_download_partly_here_moves_on_and_mixes_nothing() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    let cat = catalogue(&root);
    let s = crate::test_state_with(home.path(), &root, true);
    let m = s.data.mirror.clone().unwrap();
    // One of London's files missing on the NAS: the rest copied.
    let missing = cat.files[&cat.layers["terrain"].hi["6/31/21"]].file.clone();
    std::fs::rename(nas.path().join(&missing), nas.path().join("aside")).unwrap();
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let roads = "layers/roads/hi/6-31-21";
    let old_roads = cat.files[roads].file.clone();
    assert!(m.has(&old_roads) && !m.has(&missing));
    // Catalogue 2, London's roads made again: London moves on to it, read from the NAS's.
    let mut two = cat.clone();
    two.n = 2;
    put(&root, &mut two, roads, 25_000);
    two.validate().unwrap();
    store::catalogue::write(&nas.path().join("catalog"), &two).unwrap();
    assert!(tokio::task::block_in_place(|| s.data.refresh_catalogue()));
    assert_eq!(s.data.content(roads), Some(two.files[roads].file.clone()), "nothing older read");
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 2, to: None });
    assert_eq!(s.data.content(roads), Some(two.files[roads].file.clone()));
    assert_eq!(s.data.older_token(), "");
}

/// A deep terrain pack (pipeline::terrain_deep) of one z13 tile `h` metres high, under z10 tile
/// (x, y), put on the "NAS" and named in `cat`'s terrain.
fn put_deep(root: &store::nasfs::NasRoot, cat: &mut Catalogue, x: u32, y: u32, h: f32) {
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
    // (Its shading's deep pack, the slope layer's: its bytes don't matter here.)
    let shading = format!("layers/slope/deep/10-{x}-{y}");
    put_bytes(root, cat, &shading, "pack", shading.as_bytes());
    cat.layers.entry("slope".into()).or_default().deep.insert(format!("10/{x}/{y}"), shading);
}

#[test]
fn a_regions_terrain_detail_is_the_deep_packs_near_it_and_the_share_they_cover() {
    let nas = tempfile::tempdir().unwrap();
    let mut cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    // A box from London to Sussex, in z10 tiles x 511, y 340 and 341. One deep pack under its
    // north (row 340), one an ocean away.
    put_deep(&store::nasfs::NasRoot::local_for_test(nas.path()), &mut cat, 511, 340, 100.0);
    put_deep(&store::nasfs::NasRoot::local_for_test(nas.path()), &mut cat, 600, 340, 1.0);
    let (f, cover) = detail_of(&cat, &[rect(-0.3, 51.2, -0.1, 51.6)]);
    assert_eq!(logicals(&f), ["layers/slope/deep/10-511-340", "layers/terrain/deep/10-511-340"]);
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
    let mut cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    put_deep(&store::nasfs::NasRoot::local_for_test(nas.path()), &mut cat, 511, 340, 100.0);
    cat.n = 2;
    cat.validate().unwrap();
    store::catalogue::write(&nas.path().join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
    let detail = |s: &S, on: bool| {
        let s = s.clone();
        async move { call(put_region_detail(State(s), Path("london".into()), Json(On { on })).await).await }
    };
    let pack = cat.files["layers/terrain/deep/10-511-340"].clone();
    let shading = cat.files["layers/slope/deep/10-511-340"].clone();

    // Before: listed under London with its size (the terrain's deep pack and its shading's) and
    // cover, not downloaded.
    let v = get(&s).await;
    let f = &v["regions"]["london"]["detail"];
    assert_eq!(
        (f["bytes"].as_u64(), f["on"].as_bool(), f["state"].as_str()),
        (Some(pack.size + shading.size), Some(false), None),
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
    assert!(asked.world.is_none() && asked.regions.len() == 1 && asked.regions[0].detail.is_some());
    let plan = s.downloads.plan(&s.data);
    let n = plan.wanted.items.len();
    let mut last = plan.wanted.items[n - 2..].to_vec();
    last.sort_by_key(|i| format!("{i:?}"));
    let mut want = vec![
        Item::File { name: pack.file.clone(), size: pack.size },
        Item::File { name: shading.file.clone(), size: shading.size },
    ];
    want.sort_by_key(|i| format!("{i:?}"));
    assert_eq!(last, want);
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
    assert!(s.downloads.asked().regions[0].detail.is_some());
    assert_eq!(detail(&s, false).await.0, StatusCode::OK);
    let asked = s.downloads.asked();
    assert!(asked.regions.len() == 1 && asked.regions[0].detail.is_none());
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
    let mut cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    // The World's per-tile records over London (landmarks, area details, roads' English names), and
    // one far away.
    put(&store::nasfs::NasRoot::local_for_test(nas.path()), &mut cat, "markdata/6-31-21", 700);
    cat.markdata.insert("6/31/21".into(), "markdata/6-31-21".into());
    put(&store::nasfs::NasRoot::local_for_test(nas.path()), &mut cat, "ovdata/3-3-2", 800);
    cat.ovdata.insert("3/3/2".into(), "ovdata/3-3-2".into());
    for t in ["6-31-21", "6-10-10"] {
        put(&store::nasfs::NasRoot::local_for_test(nas.path()), &mut cat, &format!("global/roaden/{t}"), 90);
        cat.global.insert(format!("roaden/{t}"), format!("global/roaden/{t}"));
    }
    cat.n = 2;
    cat.validate().unwrap();
    store::catalogue::write(&nas.path().join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
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

/// One way of the roads fixture: its id, owner (z6 x at y 21), and its two ends' longitudes (at
/// 51.5° N).
struct FixWay {
    id: i64,
    owner: u32,
    lons: [f64; 2],
}

/// One road (id 100) of three ways across 30/21, 31/21 and 32/21, written as real files: each
/// tile's base pack and road values, its hidata (the ways drawn in it, with their owners) and the
/// road → units index. Way 100 starts far west of London (owned by 30/21) and runs into it; 101
/// runs on through London into 32/21; 102 lies in 32/21 alone; 103, owned by 32/21, starts in
/// 31/21 3.5 km east of London (beyond its 2 km), in East.
fn put_roads(root: &std::path::Path, cat: &mut Catalogue) {
    use roadcore::packs::{Here, RoadRec};
    use roadcore::WayRec;
    let ways = [
        FixWay { id: 100, owner: 30, lons: [-5.8, -0.2] },
        FixWay { id: 101, owner: 31, lons: [-0.2, 0.1] },
        FixWay { id: 102, owner: 32, lons: [0.1, 1.0] },
        FixWay { id: 103, owner: 32, lons: [-0.05, 1.0] },
    ];
    let e7 = |d: f64| (d * 1e7).round() as i32;
    let lat = e7(51.5);
    let tile_of = |lon: f64| ((lon + 180.0) / 360.0 * 64.0).floor() as u32;
    let mut file = |logical: &str, write: &dyn Fn(&mut store::sect::SectWriter)| {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let mut w =
            store::sect::SectWriter::create(&p, json!({"unit": logical_unit(logical), "tile": logical_unit(logical)}))
                .unwrap();
        write(&mut w);
        w.finish().unwrap();
        let name = write_atomic(
            &store::nasfs::NasRoot::local_for_test(root),
            logical,
            "sect",
            Source::Bytes(&std::fs::read(&p).unwrap()),
        )
        .unwrap();
        let size = std::fs::metadata(root.join(&name)).unwrap().len();
        cat.files.insert(logical.into(), FileRef { file: name, size, fmt: 1, extra: Default::default() });
    };
    let mut offset = 0f32;
    let mut offsets = Vec::new();
    for w in &ways {
        offsets.push(offset);
        offset += ((w.lons[1] - w.lons[0]) * 69_000.0) as f32;
    }
    for x in [30u32, 31, 32] {
        let t = format!("6-{x}-21");
        let mine: Vec<usize> = (0..ways.len()).filter(|&k| ways[k].owner == x).collect();
        let recs: Vec<WayRec> = mine
            .iter()
            .enumerate()
            .map(|(i, &k)| WayRec { id: ways[k].id, vstart: 2 * i as u64, vcount: 2, ..bytemuck::Zeroable::zeroed() })
            .collect();
        let verts: Vec<[i32; 2]> = mine.iter().flat_map(|&k| ways[k].lons.map(|l| [e7(l), lat])).collect();
        let n = verts.len();
        file(&format!("base/{t}"), &|w| {
            w.add("strings", b"").unwrap();
            w.add_pod("ways", &recs).unwrap();
            w.add_pod("verts", &verts).unwrap();
            w.add_pod("elevu", &vec![6_000u16; n]).unwrap();
            w.add_pod("grade", &vec![0u8; n]).unwrap();
            w.add_pod("src", &vec![1u8; n]).unwrap();
        });
        let vals: Vec<RoadRec> = mine
            .iter()
            .map(|&k| RoadRec { road: 100, len: offset, offset: offsets[k], dir: 0, _pad: [0; 7] })
            .collect();
        file(&format!("global/roads/{t}"), &|w| w.add_pod("roads", &vals).unwrap());
        // The ways drawn in the tile, with their owners.
        let here: Vec<Here> = (0..ways.len())
            .filter(|&k| (tile_of(ways[k].lons[0])..=tile_of(ways[k].lons[1] - 1e-9)).contains(&x))
            .map(|k| {
                let o = ways[k].owner;
                let index = (0..k).filter(|&j| ways[j].owner == o).count() as u32;
                Here {
                    id: ways[k].id as u64,
                    owner: roadcore::archive::tile_key(6, o, 21),
                    index,
                    bbox: [e7(ways[k].lons[0]), lat, e7(ways[k].lons[1]), lat],
                    ..bytemuck::Zeroable::zeroed()
                }
            })
            .collect();
        file(&format!("hidata/{t}"), &|w| w.add_pod("here", &here).unwrap());
    }
    let pairs: Vec<[u64; 2]> = [30u32, 31, 32].iter().map(|&x| [100, roadcore::archive::tile_key(6, x, 21)]).collect();
    file("global/roadunits", &|w| w.add_pod("pairs", &pairs).unwrap());
    cat.global.insert("roadunits".into(), "global/roadunits".into());
}

fn logical_unit(logical: &str) -> String {
    let last = logical.rsplit('/').next().unwrap_or("");
    last.replace('-', "/")
}

async fn way_call(s: &S, what: &str, id: u64, at: [f64; 2]) -> (StatusCode, Value) {
    let q = axum::extract::Query(serde_json::from_value(json!({"at": format!("{},{}", at[0], at[1])})).unwrap());
    let (s, p) = (State(s.clone()), Path(id));
    call(match what {
        "way" => crate::ways::way_h(s, p, q).await,
        "road" => crate::ways::road_h(s, p, q).await,
        _ => crate::ways::profile_h(s, p, q).await,
    })
    .await
}

/// Task #457: away from the NAS, a downloaded region's ways have their info (their owners' base
/// packs come with it, though they start far outside), and a road leaving it is cut where this
/// Mac's files end.
#[tokio::test(flavor = "multi_thread")]
async fn away_a_regions_ways_have_their_info_and_its_roads_are_cut_at_its_edge() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = nas.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let mut cat = catalogue(&store::nasfs::NasRoot::local_for_test(&root));
    put_roads(&root, &mut cat);
    cat.n = 2;
    cat.validate().unwrap();
    store::catalogue::write(&root.join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    let in_london = [-0.15, 51.5];

    // At home: the whole road.
    let (code, v) = way_call(&s, "road", 101, in_london).await;
    assert_eq!((code, v), (StatusCode::OK, json!([100, 101, 102, 103])));

    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    assert!(m.has(cat.content("base/6-30-21").unwrap()), "way 100's owner, listed in London's hidata");
    assert!(m.has(cat.content("global/roads/6-30-21").unwrap()));
    assert!(!m.has(cat.content("base/6-32-21").unwrap()), "way 103's owner: its box misses London's 2 km");
    let v = get(&s).await;
    assert_eq!(v["regions"]["london"]["here"], v["regions"]["london"]["bytes"], "counted in its size");

    // Away: the NAS gone.
    std::fs::rename(&root, nas.path().join("gone")).unwrap();
    s.data.pool().unwrap().mark_offline("test");
    tokio::task::block_in_place(|| settle(&s));
    assert!(m.has(cat.content("base/6-30-21").unwrap()), "kept away from the NAS");

    // Way 100 starts 390 km outside London: its info all the same.
    let (code, v) = way_call(&s, "way", 100, [-0.25, 51.5]).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["osm_id"], json!(100));
    // The road through London, cut where this Mac's files end (32/21 isn't here).
    let (code, v) = way_call(&s, "road", 101, in_london).await;
    assert_eq!((code, v), (StatusCode::OK, json!([100, 101])));
    let (code, v) = way_call(&s, "profile", 101, in_london).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["ways"].clone(), v["truncated"].clone()), (json!([100, 101]), json!(true)));

    // Started again away from the NAS: the owners read from this Mac's hidata, nothing goes.
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    if let Some(p) = s.data.pool() {
        p.mark_offline("test");
    }
    tokio::task::block_in_place(|| settle(&s));
    assert!(s.data.mirror.clone().unwrap().has(cat.content("base/6-30-21").unwrap()), "kept after a start away");
    let (code, v) = way_call(&s, "way", 100, [-0.25, 51.5]).await;
    assert_eq!((code, v["osm_id"].clone()), (StatusCode::OK, json!(100)));
}

/// A downloaded region swapped for another with as many hidata: the second's owners come, the
/// first's go.
#[tokio::test(flavor = "multi_thread")]
async fn a_region_swapped_for_another_keeps_the_others_owners() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = nas.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let mut cat = catalogue(&store::nasfs::NasRoot::local_for_test(&root));
    put_roads(&root, &mut cat);
    cat.n = 2;
    cat.validate().unwrap();
    store::catalogue::write(&root.join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    let m = s.data.mirror.clone().unwrap();
    let has = |l: &str| m.has(cat.content(l).unwrap());
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert!(has("base/6-30-21") && !has("base/6-32-21"));
    assert_eq!(region(&s, "london", false).await.0, StatusCode::OK);
    assert_eq!(region(&s, "east", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert!(has("base/6-32-21"), "way 103's owner, for East");
    assert!(!has("base/6-30-21"), "London's far owner went");
}

/// At home, a downloaded region's hidata that can't be read (started again, the NAS's copy gone)
/// lets no base pack go: its owners can't be told.
#[tokio::test(flavor = "multi_thread")]
async fn at_home_an_unread_hidata_lets_no_owner_go() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = nas.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let mut cat = catalogue(&store::nasfs::NasRoot::local_for_test(&root));
    put_roads(&root, &mut cat);
    cat.n = 2;
    cat.validate().unwrap();
    store::catalogue::write(&root.join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let far = cat.content("base/6-30-21").unwrap().to_string();
    assert!(s.data.mirror.clone().unwrap().has(&far));
    drop(s);

    // A new catalogue whose London hidata isn't on the NAS.
    let gone = "hidata/6-31-21.0123456789abcdef.sect".to_string();
    cat.files.get_mut("hidata/6-31-21").unwrap().file = gone;
    cat.n = 3;
    store::catalogue::write(&root.join("catalog"), &cat).unwrap();
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    tokio::task::block_in_place(|| settle(&s));
    assert!(s.data.mirror.clone().unwrap().has(&far), "kept while its hidata is unread");
}
