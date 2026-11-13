use super::*;
use crate::maplog::MirrorLink;
use inputs::areafiles::{clip, lat_y, lon_x, ring_area};
use store::catalogue::{FileRef, Layer};
use store::naming::{write_atomic, Source};
use store::pmtiles::Compression;

/// A box as a closed ring.
fn rect(w: f64, s: f64, e: f64, n: f64) -> Vec<[f64; 2]> {
    vec![[w, s], [e, s], [e, n], [w, n], [w, s]]
}

#[test]
fn the_tiles_an_area_meets() {
    // A box west of Greenwich, in the z6 tile 31/21: a margin of 50 km reaches over the meridian
    // into 32/21, its tiles' (2 km) doesn't.
    let london = rect(-0.3, 51.4, -0.1, 51.6);
    assert_eq!(tiles_meeting(std::slice::from_ref(&london), NEAR_KM), [(31, 21)].into());
    assert_eq!(tiles_meeting(std::slice::from_ref(&london), 50.0), [(31, 21), (32, 21)].into());
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

/// A terrain tile as stored (lossless WebP), `h` m throughout.
fn terrain_tile(h: f32) -> Vec<u8> {
    codecs::terrain_codec::encode(&mut vec![h; 65536], codecs::terrain_codec::COARSER_M).unwrap()
}

/// The terrain's root (`root`) or lo pack's tiles (zooms 0–2 and 2/3/1 far east; the z3 tile 3/2's
/// over London's z6 tile and beside it), `h` m throughout.
fn terrain_tiles(root: bool, h: f32) -> Vec<(u8, u32, u32, Vec<u8>)> {
    let zxy: &[(u8, u32, u32)] = if root {
        &[(0, 0, 0), (1, 0, 0), (2, 1, 1), (2, 3, 1)]
    } else {
        &[(3, 3, 2), (4, 7, 5), (5, 15, 10), (5, 14, 10), (6, 31, 21), (6, 30, 21), (7, 62, 42), (8, 124, 84)]
    };
    zxy.iter().map(|&(z, x, y)| (z, x, y, terrain_tile(h))).collect()
}

/// Puts a pack of `tiles` (not gzip'd) on the "NAS" and lists it in `cat`.
fn put_pack(root: &store::nasfs::NasRoot, cat: &mut Catalogue, logical: &str, tiles: &[(u8, u32, u32, Vec<u8>)]) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("p");
    let mut w = store::pack::PackWriter::create(&p, json!({"layer": "terrain"}), false).unwrap();
    for (z, x, y, b) in tiles {
        w.add(*z, *x, *y, b, 0).unwrap();
    }
    w.finish().unwrap();
    put_bytes(root, cat, logical, "pack", &std::fs::read(&p).unwrap());
}

/// A catalogue of three z6 tiles' files (30/21, 31/21 and 32/21: the Atlantic off Ireland,
/// England, the Low Countries), a terrain layer's root and lo packs, a worldwide file, a basemap
/// (a tile at z0 and z10, and z11–14 tiles under each of the three), and one region, London,
/// recorded in its coverage.
fn catalogue(root: &store::nasfs::NasRoot) -> Catalogue {
    let mut c = Catalogue::new(1);
    let mut terrain = Layer { encoding: "terrarium-webp".into(), maxzoom: 12, ..Default::default() };
    let mut roads = Layer { encoding: "rt7".into(), minzoom: 4, maxzoom: 14, ..Default::default() };
    put_pack(root, &mut c, "layers/terrain/root", &terrain_tiles(true, 100.0));
    put_pack(root, &mut c, "layers/terrain/lo/3-3-2", &terrain_tiles(false, 100.0));
    terrain.root = Some("layers/terrain/root".into());
    terrain.lo.insert("3/3/2".into(), "layers/terrain/lo/3-3-2".into());
    let mut tiles: Vec<(u8, u32, u32, Vec<u8>)> =
        vec![(0, 0, 0, gz(b"world")), (10, 510, 340, gz(b"z10")), (10, 0, 340, gz(b"far"))];
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
    ]})
    .into();
    c.validate().unwrap();
    store::testkit::map_root(&root.unchecked_path(), &c);
    c
}

fn logicals(f: &Files) -> Vec<String> {
    let mut v: Vec<String> =
        f.names.iter().map(|(n, _)| store::naming::parse_content_name(n).unwrap().logical.to_string()).collect();
    v.sort();
    v
}

#[test]
fn an_areas_files_are_its_tiles_hi_packs_base_packs_and_hi_data() {
    let nas = tempfile::tempdir().unwrap();
    let cat = catalogue(&store::nasfs::NasRoot::local_for_test(nas.path()));
    let f = files_of(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);
    // Its tile's packs and hi data, the worldwide file (read zoomed in too); nothing of the tile
    // over the meridian, 25 km off (away from the NAS nothing past its tiles is read).
    assert_eq!(
        logicals(&f),
        [
            "base/6-31-21",
            "global/railfreq",
            "global/roads/6-31-21",
            "hidata/6-31-21",
            "layers/roads/hi/6-31-21",
            "layers/terrain/hi/6-31-21"
        ]
    );
    assert_eq!(f.bytes, 30_031 + 300 + 4_031 + 5_031 + 20_031 + 10_031);
    // And after its files, its pieces of the worldwide files, over its tile alone: of the terrain's
    // root (zooms 0–2) and its lo pack (3–8), and of the basemap (0–14).
    let p = area_part(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)], &BTreeSet::new());
    let c = |l: &str| cat.content(l).unwrap().to_string();
    let (root, lo, world) = (c("layers/terrain/root"), c("layers/terrain/lo/3-3-2"), c("layers/basemap/world"));
    let piece = |a: &String, piece| Item::Piece { archive: a.clone(), piece };
    let mut want = vec![
        piece(&root, Piece::Over(0, 0, 0)),
        piece(&root, Piece::Over(1, 0, 0)),
        piece(&root, Piece::Over(2, 1, 1)),
        piece(&lo, Piece::Over(3, 3, 2)),
        piece(&lo, Piece::Over(4, 7, 5)),
        piece(&lo, Piece::Over(5, 15, 10)),
        piece(&lo, Piece::Mid(31, 21)),
    ];
    want.extend((0..6).map(|z| piece(&world, Piece::Over(z, 31 >> (6 - z), 21 >> (6 - z)))));
    want.extend([piece(&world, Piece::Mid(31, 21)), piece(&world, Piece::Tile(31, 21))]);
    assert_eq!(p.items[6..], want);
    assert_eq!(p.items.len(), 6 + 15);
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

/// The size of a part's files.
fn file_bytes(p: &Part) -> u64 {
    p.items.iter().map(|i| if let Item::File { size, .. } = i { *size } else { 0 }).sum()
}

/// One round of the mirror thread, until it has nothing more to do.
fn settle(s: &S) {
    let m = s.data.mirror.clone().unwrap();
    while once(s, &m, None) {}
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
    let packs: u64 = ["layers/terrain/root", "layers/terrain/lo/3-3-2"].iter().map(|l| cat.files[*l].size).sum();
    let pieces: u64 = london
        .items
        .iter()
        .map(|i| match i {
            Item::Piece { archive, piece } => m.piece_size(archive, *piece).unwrap(),
            Item::File { .. } => 0,
        })
        .sum();
    assert_eq!(
        v["world"],
        json!({"bytes": 300 + packs + lo, "here": 0, "unknown": 0, "on": false, "at": null, "state": null})
    );
    assert_eq!(
        v["regions"]["london"],
        json!({"name": "London", "bytes": file_bytes(&london) + pieces, "here": 0, "unknown": 0, "on": false, "state": null})
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
        "London keeps the worldwide file (its own pieces of the root and lo packs and the basemap)"
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
    assert_eq!((on_disk["fmt"].as_u64(), on_disk.get("views")), (Some(5), None), "written again without them");
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    // The World's and London's stay; the view's files no region needs went.
    let plan = s.downloads.plan(&s.data);
    let gone: Vec<&String> = rotterdam
        .items
        .iter()
        .filter_map(|i| match i {
            Item::File { name, .. } => Some(name),
            Item::Piece { .. } => None,
        })
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
/// written again as format 5: the World's time and each region's terrain detail kept, every download
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
    assert_eq!(back["fmt"], 5);
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
    agentcore::agent::settings::set_room(&home.path().join("agent"), "40", "t").unwrap();
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
    std::fs::remove_dir_all(nas.path().join(store::map::ROOT_DIR)).unwrap();
    let se = rect(-0.3, 51.4, 0.3, 51.6);
    let coverage = |built: u32| {
        json!({"recorded": true, "regions": [
            {"id": "se", "name": "South East", "outline": ["osm:1"], "shapes": {"osm:1": [[se.clone()]]},
             "tiles": {"built": built, "total": 2}},
        ]})
        .into()
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
    store::testkit::map_root(nas.path(), &first);
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(s.data.catalogue().n, 1);
    let m = s.data.mirror.clone().unwrap();

    // Downloaded while building: catalogue 1's files copied.
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
    store::testkit::map_root(nas.path(), &second);
    assert!(tokio::task::block_in_place(|| s.data.serve_root_map()));
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
    let on_disk: Value = serde_json::from_slice(&std::fs::read(home.path().join(FILE)).unwrap()).unwrap();
    assert!(on_disk["lists"]["1"]["regions"]["se"].is_object(), "its own list at 1");

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
    // Not behind: refused.
    let (code, v) = call(post_region_update(State(s.clone()), Path("se".into())).await).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");

    // Removed: its files go.
    assert_eq!(region(&s, "se", false).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(m.usage(), (0, 0));
}

/// A region downloaded before it held its pieces of the worldwide files (its list naming the root
/// and lo packs over it whole and the basemap's worldwide zooms 0–10, and those here), behind the
/// catalogue served: at its next round with the NAS it takes its pieces at the served catalogue and
/// lets the whole files go, and stays at its own for the rest (no Update).
#[tokio::test(flavor = "multi_thread")]
async fn a_region_listed_in_an_older_form_takes_its_pieces_and_stays_behind() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    let cat = catalogue(&root);
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let (lo, roads) = ("layers/terrain/lo/3-3-2", "layers/roads/hi/6-31-21");
    // Its list as one made before: the whole root and lo pack, the basemap's lo piece; and the
    // root and lo pack here.
    let path = home.path().join(FILE);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let coarse = v["lists"]["1"]["regions"]["london"]["coarse"].as_array_mut().unwrap();
    coarse.retain(|i| {
        !i["piece"].as_str().and_then(Piece::parse).is_some_and(|p| matches!(p, Piece::Over(..) | Piece::Mid(..)))
    });
    let whole = ["layers/terrain/root", lo];
    for l in whole {
        let f = &cat.files[l];
        coarse.push(json!({"file": f.file, "size": f.size}));
        let p = home.path().join("mirror").join(&f.file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::copy(nas.path().join(&f.file), p).unwrap();
    }
    coarse.push(json!({"archive": cat.content("layers/basemap/world").unwrap(), "piece": "lo"}));
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    // (Its pieces of zooms 0–10 not here, as then.)
    for d in [".packs", ".basemap"] {
        for e in std::fs::read_dir(home.path().join("mirror").join(d)).unwrap().flatten() {
            for f in std::fs::read_dir(e.path()).unwrap().flatten() {
                if f.file_name().to_str().is_some_and(|n| !n.starts_with("6-") && n != "sizes.json") {
                    std::fs::remove_file(f.path()).unwrap();
                }
            }
        }
    }
    // Catalogue 2: London's roads and the lo pack made again.
    drop(s);
    let s = crate::test_state_with(home.path(), &root, true);
    let m = s.data.mirror.clone().unwrap();
    assert!(whole.iter().all(|l| m.has(cat.content(l).unwrap())));
    let two = next_catalogue(&s, nas.path(), &cat, 2, &[roads, lo]);
    tokio::task::block_in_place(|| settle(&s));
    let world = two.content("layers/basemap/world").unwrap();
    assert!(
        m.has_piece(two.content(lo).unwrap(), Piece::Mid(31, 21)),
        "the lo pack's pieces taken at the served catalogue"
    );
    assert!(m.has_piece(two.content("layers/terrain/root").unwrap(), Piece::Over(2, 1, 1)));
    assert!(m.has_piece(world, Piece::Over(0, 0, 0)) && m.has_piece(world, Piece::Mid(31, 21)));
    assert!(!whole.iter().any(|l| m.has(cat.content(l).unwrap())), "the whole files gone");
    assert!(!m.has_piece(world, Piece::Lo));
    assert!(m.has(cat.content(roads).unwrap()) && !m.has(two.content(roads).unwrap()), "its roads as they were");
    assert_eq!(Downloads::load(home.path()).asked().regions[0].pin, Pin { cat: 1, to: None }, "behind, as it was");
}

/// Copies folder `from` into `to`, as it is.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

/// The NAS away: its folder `root` moved to `gone`, the pool told.
fn nas_away(s: &S, root: &std::path::Path, gone: &std::path::Path) {
    std::fs::rename(root, gone).unwrap();
    s.data.pool().unwrap().mark_offline("test");
}

/// London downloaded in the form a region had before its pieces, in `home`, over the NAS at
/// `root`: the whole terrain root and lo pack, the basemap's zooms 0–10 piece (the World's,
/// downloaded with it, then removed before the mirror's next round), its tiles' files and
/// pieces; none of its pieces of zooms 0–10. `listed`: its list says so too (one written then),
/// and catalogue 2 makes the terrain's root and lo pack again (200 m), London behind it at 1;
/// else it's at the served catalogue, the files on this Mac all that's of the old form. The
/// catalogue it was downloaded at.
async fn old_form(home: &std::path::Path, root: &std::path::Path, listed: bool) -> Catalogue {
    let r = store::nasfs::NasRoot::local_for_test(root);
    let cat = catalogue(&r);
    let s = crate::test_state_with(home, &r, true);
    assert_eq!(call(put_world(State(s.clone()), Json(On { on: true })).await).await.0, StatusCode::OK);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(call(put_world(State(s.clone()), Json(On { on: false })).await).await.0, StatusCode::OK);
    drop(s);
    let zoomed_out = |p: Option<Piece>| p.is_some_and(|p| matches!(p, Piece::Over(..) | Piece::Mid(..)));
    for d in [".packs", ".basemap"] {
        for e in std::fs::read_dir(home.join("mirror").join(d)).unwrap().flatten() {
            for f in std::fs::read_dir(e.path()).unwrap().flatten() {
                let name = f.file_name().into_string().unwrap();
                if zoomed_out(name.split('.').next().and_then(Piece::parse)) {
                    std::fs::remove_file(f.path()).unwrap();
                }
            }
        }
    }
    if !listed {
        return cat;
    }
    let path = home.join(FILE);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let coarse = v["lists"]["1"]["regions"]["london"]["coarse"].as_array_mut().unwrap();
    coarse.retain(|i| !zoomed_out(i["piece"].as_str().and_then(Piece::parse)));
    for l in ["layers/terrain/root", "layers/terrain/lo/3-3-2"] {
        let f = &cat.files[l];
        coarse.push(json!({"file": f.file, "size": f.size}));
    }
    coarse.push(json!({"archive": cat.content("layers/basemap/world").unwrap(), "piece": "lo"}));
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let s = crate::test_state_with(home, &r, true);
    next_catalogue(&s, root, &cat, 2, &["layers/terrain/root", "layers/terrain/lo/3-3-2"]);
    cat
}

/// What's served of London zoomed out, away from the NAS: its terrain (the root's and lo pack's
/// tiles over it, each at one height, the old form's or the new's) and the basemap's zooms 0–10
/// over it.
fn zoomed_out_heights(s: &S, world: &str) -> BTreeSet<u32> {
    let mut heights = BTreeSet::new();
    for (z, x, y) in [(0, 0, 0), (1, 0, 0), (2, 1, 1), (3, 3, 2), (4, 7, 5), (5, 15, 10), (6, 31, 21), (7, 62, 42)] {
        // (A pixel inside London's z6 tile 31/21.)
        let at = |t: u32| {
            if z <= 6 {
                (((t as f64 + 0.5) / f64::from(1u32 << (6 - z))).fract() * 256.0) as usize
            } else {
                128
            }
        };
        let (b, _) = s.data.tile("terrain", z, x, y).unwrap().unwrap_or_else(|| panic!("terrain {z}/{x}/{y}: none"));
        let t = roadcore::grid::decode_terrain(b.bytes()).unwrap();
        heights.insert(t[at(21) * 256 + at(31)] as u32);
    }
    let basemap = |z: u8, x: u32, y: u32| s.basemap.tiles(&s.data, &[world.to_string()], z, x, y).unwrap();
    assert_eq!(basemap(0, 0, 0), [gz(b"world")]);
    assert_eq!(basemap(10, 510, 340), [gz(b"z10")]);
    assert_eq!(basemap(12, 31 << 6, 21 << 6), [gz(b"12/31")]);
    heights
}

/// Switching a download made before its pieces (task #985) to them: the new pieces are copied and
/// checked first, the old whole files and zooms 0–10 piece kept; once they're all here the old form
/// goes in one step. Interrupted at each point (the NAS blocked between two pieces and in the middle
/// of one, then the server stopped and started again away from it), London away from the NAS is
/// its old form or its new one, whole: every tile zoomed out over it there, from one or the other.
/// Both ways a region had it: its list made before (behind the catalogue, whose root and lo pack
/// differ), and a list of the new form with the old files here (at the served catalogue).
#[tokio::test(flavor = "multi_thread")]
async fn a_region_switches_to_its_pieces_whole_whenever_its_interrupted() {
    for listed in [true, false] {
        let (tpl, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, gone) = (nas.path().join("project"), nas.path().join("gone"));
        std::fs::create_dir_all(&root).unwrap();
        let r = store::nasfs::NasRoot::local_for_test(&root);
        let cat = old_form(tpl.path(), &root, listed).await;
        let world = cat.content("layers/basemap/world").unwrap().to_string();
        let old: Vec<Item> = ["layers/terrain/root", "layers/terrain/lo/3-3-2"]
            .iter()
            .map(|l| Item::File { name: cat.files[*l].file.clone(), size: cat.files[*l].size })
            .chain([Item::Piece { archive: world.clone(), piece: Piece::Lo }])
            .collect();
        let (h_old, h_new) = (100, if listed { 200 } else { 100 });
        let new: Vec<Item> = {
            let s = crate::test_state_with(tpl.path(), &r, true);
            let m = s.data.mirror.clone().unwrap();
            assert!(
                old.iter().all(|i| m.is_here(i)),
                "the old form here: {:?}",
                old.iter().filter(|i| !m.is_here(i)).collect::<Vec<_>>()
            );
            let w = s.downloads.plan(&s.data).wanted.items.clone();
            w.into_iter().filter(|i| !m.is_here(i)).collect()
        };
        assert!(new.len() >= 10 && new.iter().all(|i| piece_of(i).is_some()), "{new:?}");
        let check = |s: &S, at: &str| {
            let m = s.data.mirror.clone().unwrap();
            let (o, n) = (old.iter().all(|i| m.is_here(i)), new.iter().all(|i| m.is_here(i)));
            assert!(o != n, "{at}: the old form here {o}, the new {n}");
            assert!(o || old.iter().all(|i| !m.is_here(i)), "{at}: the old form gone whole");
            assert_eq!(zoomed_out_heights(s, &world), [if o { h_old } else { h_new }].into(), "{at}");
        };
        for k in 0..=new.len() {
            for mid in [false, true].into_iter().filter(|&mid| !mid || k < new.len()) {
                let at =
                    format!("listed {listed}, {k} of {} pieces{}", new.len(), if mid { ", in the next" } else { "" });
                let home = tempfile::tempdir().unwrap();
                copy_dir(tpl.path(), home.path());
                // One round with the NAS, blocked once `k` of the new pieces are here.
                let s = crate::test_state_with(home.path(), &r, true);
                let m = s.data.mirror.clone().unwrap();
                let cat = s.data.catalogue();
                // (Planned again while a pin moves, as a round goes again.)
                let wanted = std::iter::repeat_with(|| round_plan(&s, &m, &cat)).take(5).flatten().next().unwrap().1;
                let pin = s.downloads.asked().regions[0].pin;
                assert_eq!(pin, Pin { cat: 1, to: None }, "{at}: at its own catalogue");
                let block = || {
                    let n = new.iter().filter(|i| m.is_here(i)).count();
                    // (In the middle of a piece: as it's made, before its first read; one with
                    // nothing to read is made whole, and the NAS goes before the next.)
                    if root.exists() && n >= k && (!mid || n > k || m.copying().is_some()) {
                        nas_away(&s, &root, &gone);
                    }
                };
                let stop = || {
                    block();
                    false
                };
                let ctl = Control { stop: &stop, slow: &|| false };
                m.sync(&cat, &wanted, &r, &s.data.pool().unwrap(), &ctl).unwrap();
                if root.exists() {
                    nas_away(&s, &root, &gone);
                }
                let n = new.iter().filter(|i| m.is_here(i)).count();
                assert!(n == k || mid && n == k + 1, "{at}: {n} here");
                tokio::task::block_in_place(|| settle(&s));
                check(&s, &format!("{at}, the NAS blocked"));
                // The server stopped, and started again away.
                drop((s, m));
                std::fs::rename(&gone, &root).unwrap();
                let s = crate::test_state_with(home.path(), &r, true);
                nas_away(&s, &root, &gone);
                tokio::task::block_in_place(|| settle(&s));
                check(&s, &format!("{at}, started again away"));
                // Started again at home: the rest copied, the old form gone; away again, the new
                // form.
                drop(s);
                std::fs::rename(&gone, &root).unwrap();
                let s = crate::test_state_with(home.path(), &r, true);
                tokio::task::block_in_place(|| settle(&s));
                let m = s.data.mirror.clone().unwrap();
                assert!(new.iter().all(|i| m.is_here(i)) && !old.iter().any(|i| m.is_here(i)), "{at}, home");
                nas_away(&s, &root, &gone);
                tokio::task::block_in_place(|| settle(&s));
                check(&s, &format!("{at}, home then away"));
                std::fs::rename(&gone, &root).unwrap();
            }
        }
    }
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
    let mut cov = two.coverage.to_value();
    cov["regions"].as_array_mut().unwrap().push(
        json!({"id": "kanto", "name": "Kanto", "outline": ["osm:2"], "shapes": {"osm:2": [[rect(-0.06, 51.45, -0.05, 51.5)]]}}),
    );
    two.coverage = cov.into();
    two.validate().unwrap();
    store::testkit::map_root(&nas.path(), &two);
    assert!(tokio::task::block_in_place(|| s.data.serve_root_map()));
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
    store::testkit::map_root(&nas.path(), &two);
    assert!(tokio::task::block_in_place(|| s.data.serve_root_map()));
    assert_eq!(s.data.content(roads), Some(two.files[roads].file.clone()), "nothing older read");
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 2, to: None });
    assert_eq!(s.data.content(roads), Some(two.files[roads].file.clone()));
    assert_eq!(s.data.older_token(), "");
}

/// A deep terrain pack (terrain::terrain_deep) of one z13 tile `h` metres high, under z10 tile
/// (x, y), put on the "NAS" and named in `cat`'s terrain.
fn put_deep(root: &store::nasfs::NasRoot, cat: &mut Catalogue, x: u32, y: u32, h: f32) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("p.pack");
    let mut w = store::pack::PackWriter::create(&path, json!({"layer": "terrain"}), false).unwrap();
    let b = codecs::terrain_codec::encode(&mut vec![h; 65536], codecs::terrain_codec::NATIVE_M).unwrap();
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
    store::testkit::map_root(&nas.path(), &cat);
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
    store::testkit::map_root(&nas.path(), &cat);
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    assert!(s.downloads.asked().world.is_none());
    tokio::task::block_in_place(|| settle(&s));
    let m = s.data.mirror.clone().unwrap();
    for l in ["markdata/6-31-21", "ovdata/3-3-2", "global/roaden/6-31-21", "global/railfreq"] {
        assert!(m.has(cat.content(l).unwrap()), "{l} here with London alone");
    }
    assert!(!m.has(cat.content("global/roaden/6-10-10").unwrap()), "not a far tile's");
    // Zoomed out over it too: the pieces over its tile of the root and lo packs and the basemap.
    let c = |l: &str| cat.content(l).unwrap();
    assert!(m.has_piece(c("layers/terrain/root"), Piece::Over(2, 1, 1)));
    assert!(m.has_piece(c("layers/terrain/lo/3-3-2"), Piece::Mid(31, 21)));
    assert!(m.has_piece(c("layers/basemap/world"), Piece::Over(0, 0, 0)));
    assert!(!m.has(c("layers/terrain/lo/3-3-2")) && !m.has_piece(c("layers/basemap/world"), Piece::Lo), "not whole");
    let v = get(&s).await;
    assert_eq!(v["regions"]["london"]["here"], v["regions"]["london"]["bytes"], "counted in its size");
}

/// Away from the NAS the map is what's downloaded (crate::data): a file of a region downloaded is
/// read from this Mac, at every zoom over it; one outside it, and everything with nothing
/// downloaded, is none, at once (never a NAS read, never an error); the downloads reach the
/// region's tiles. Back at home, the NAS's again.
#[tokio::test(flavor = "multi_thread")]
async fn away_from_the_nas_the_map_is_its_downloads() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = nas.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let cat = catalogue(&store::nasfs::NasRoot::local_for_test(&root));
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    let world = cat.content("layers/basemap/world").unwrap().to_string();
    let basemap = |z: u8, x: u32, y: u32| s.basemap.tiles(&s.data, std::slice::from_ref(&world), z, x, y);
    let (far, here) = ("layers/terrain/hi/6-30-21", "layers/terrain/hi/6-31-21");
    assert_eq!(s.data.content(far).as_deref(), cat.content(far), "at home: the NAS's");
    assert_eq!(s.data.reach(), None);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let root_token = || crate::meta_json(&s)["packs"]["terrain"]["root"].as_str().unwrap().to_string();
    let home_root = root_token();
    // The basemap's tiles as the app asks for them (through the browser's caching): the status and
    // Cache-Control.
    let base = |z: u8, x: u32, y: u32| {
        let s = s.clone();
        async move {
            let v = crate::meta_json(&s)["versions"]["basemap"].as_str().unwrap().to_string();
            let app = axum::Router::new()
                .route("/tiles/base/{z}/{x}/{y}", axum::routing::get(crate::tiles::base_tile))
                .layer(axum::middleware::from_fn_with_state(s.clone(), crate::versioned_caching))
                .with_state(s.clone());
            let req = axum::http::Request::builder()
                .uri(format!("/tiles/base/{z}/{x}/{y}?v={v}"))
                .body(axum::body::Body::empty());
            let r = tower::ServiceExt::oneshot(app, req.unwrap()).await.unwrap();
            let cc = r.headers().get(axum::http::header::CACHE_CONTROL).map(|c| c.to_str().unwrap().to_string());
            (r.status(), cc)
        }
    };
    // (Made at home, from the NAS: kept in memory.)
    assert_eq!(base(10, 0, 340).await.0, StatusCode::OK);

    // Away.
    std::fs::rename(&root, nas.path().join("gone")).unwrap();
    let g = s.data.generation.load(Ordering::SeqCst);
    s.data.pool().unwrap().mark_offline("test");
    assert!(s.data.generation.load(Ordering::SeqCst) > g, "what's read changed");
    for l in [here, "base/6-31-21"] {
        assert_eq!(s.data.content(l).as_deref(), cat.content(l), "{l}: downloaded, read here");
    }
    for l in [far, "base/6-30-21", "layers/roads/hi/6-30-21"] {
        assert_eq!(s.data.content(l), None, "{l}: not downloaded, none");
    }
    assert!(s.data.tile("roads", 12, 30 << 6, 21 << 6).unwrap().is_none(), "none, not an error");
    assert_eq!(basemap(0, 0, 0).unwrap(), [gz(b"world")], "zoomed out over it");
    assert_eq!(basemap(10, 510, 340).unwrap(), [gz(b"z10")]);
    assert_eq!(basemap(12, 31 << 6, 21 << 6).unwrap(), [gz(b"12/31")]);
    assert!(basemap(12, 30 << 6, 21 << 6).unwrap().is_empty(), "outside it: none, not an error");
    assert_eq!(s.data.reach().map(|r| r.merc.set.clone()), Some([(31, 21)].into()));
    // What reaches past it (the basemap's zooms 0–10, the lo and root packs) is read over it alone:
    // their pieces over it (the packs not here whole), a tile only partly over it cut to it.
    assert!(basemap(10, 0, 340).unwrap().is_empty(), "the zooms 0–10 piece, far from it");
    assert_eq!(s.data.content("layers/terrain/lo/3-3-2"), None, "not here whole");
    let terrain = |z: u8, x: u32, y: u32| {
        s.data.tile("terrain", z, x, y).unwrap().map(|(b, _)| roadcore::grid::decode_terrain(b.bytes()).unwrap())
    };
    assert_eq!(terrain(6, 31, 21).unwrap()[0], 100.0, "its tile, whole");
    assert!(terrain(6, 30, 21).is_none() && terrain(5, 14, 10).is_none(), "its lo pack, beside it");
    // (z5 15/10 is four z6 tiles: London's 31/21, its bottom right; z2 1/1 is 16/16 to 31/31, 31/21
    // its pixels 240–255 across, 80–95 down.)
    for (z, x, y, inside) in [(5, 15, 10, 200 * 256 + 200), (2, 1, 1, 85 * 256 + 250)] {
        let t = terrain(z, x, y).unwrap();
        assert_eq!((t[inside], t[0]), (100.0, 0.0), "{z}/{x}/{y}: cut to it, flat outside");
    }
    assert!(terrain(2, 3, 1).is_none() && terrain(5, 14, 10).is_none());
    let stored = |z, x, y| {
        let ix = store::pack::PackIndex::parse_bytes(
            &std::fs::read(nas.path().join("gone").join(cat.content("layers/terrain/lo/3-3-2").unwrap())).unwrap(),
        )
        .unwrap();
        ix.find(z, x, y).unwrap().hash
    };
    assert_ne!(s.data.tile_hash("terrain", 5, 15, 10).unwrap(), Some(stored(5, 15, 10)), "a cut tile's own ETag");
    assert_eq!(s.data.tile_hash("terrain", 6, 31, 21).unwrap(), Some(stored(6, 31, 21)));
    // Every version the app's URLs carry names the reach (Data::reach_token), so what's answered
    // for it isn't reused at home or for another reach; and a 204 away isn't cached for good (it
    // says "not downloaded"), nor is the tile made at home served.
    let r = s.data.reach_token().expect("kept to the region");
    assert_eq!(root_token(), format!("{home_root}-{r}"));
    assert_eq!(base(10, 0, 340).await, (StatusCode::NO_CONTENT, Some("no-store".to_string())));
    // A file here of a tile past where the downloads reach (a reach other than the files', as an
    // older download's) is read at no zoom: its hi pack's tiles and its hi data none.
    s.data.set_reach(crate::data::Reach::Tiles(Arc::new(crate::data::Footprint::of([(33, 21)], []))));
    assert!(s.data.content("layers/terrain/hi/6-31-21").is_some());
    assert!(terrain(12, 31 << 6, 21 << 6).is_none() && terrain(9, 31 << 3, 21 << 3).is_none());
    assert!(s.data.hidata("6/31/21").unwrap().is_none());

    // Nothing downloaded: nothing at all.
    assert_eq!(region(&s, "london", false).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    for l in [here, "layers/terrain/lo/3-3-2", "global/railfreq"] {
        assert_eq!(s.data.content(l), None, "{l}");
    }
    assert!(basemap(0, 0, 0).unwrap().is_empty());
    assert_eq!(s.data.reach(), Some(Arc::new(crate::data::Footprint::default())));
    assert!(s.data.tile("terrain", 0, 0, 0).unwrap().is_none(), "no pieces, none");

    // Home again: the NAS's.
    std::fs::rename(nas.path().join("gone"), &root).unwrap();
    store::testkit::eventually("the NAS read again", || s.data.online());
    assert_eq!(s.data.content(far).as_deref(), cat.content(far));
    assert_eq!(s.data.reach(), None);
    assert_eq!(basemap(10, 0, 340).unwrap(), [gz(b"far")]);
    assert_eq!((s.data.reach_token(), root_token()), (None, home_root));
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
    let tile_of = |lon: f64| (roadcore::geo::merc_x(lon) * 64.0).floor() as u32;
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
    store::testkit::map_root(&root, &cat);
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

    // Started again away from the NAS, on the map this Mac kept (crate::maplog's, as a start reads
    // it): the owners read from this Mac's hidata, nothing goes.
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    if let Some(p) = s.data.pool() {
        p.mark_offline("test");
    }
    assert_eq!(s.data.catalogue().n, 0, "no map on a NAS that isn't there");
    let cache = tempfile::tempdir().unwrap();
    let kept = store::map::Snapshot::of(&store::map::Map::of(&cat).unwrap(), &Default::default()).encode().unwrap();
    std::fs::write(cache.path().join("current.json.zst"), kept).unwrap();
    crate::maplog::Follower::new(
        s.data.clone(),
        crate::maplog::Config {
            cache: Some(cache.path().to_owned()),
            lead: crate::maplog::Lead::At { urls: Vec::new(), token: String::new() },
            wait_s: 0,
            retry: std::time::Duration::from_millis(50),
            client: "server/test".into(),
            save_every: crate::maplog::SAVE_EVERY,
            stop: Default::default(),
        },
    )
    .start();
    assert_eq!(s.data.catalogue().n, 2);
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
    store::testkit::map_root(&root, &cat);
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
    store::testkit::map_root(&root, &cat);
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
    store::testkit::map_root(&root, &cat);
    let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(&root), true);
    tokio::task::block_in_place(|| settle(&s));
    assert!(s.data.mirror.clone().unwrap().has(&far), "kept while its hidata is unread");
    // (Its list at 3 is partial while its owners can't be told: made again from map 3 once they
    // are; until then no base pack goes.)
    let j: Value = serde_json::from_slice(&std::fs::read(home.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(j["lists"]["3"]["partial"], json!(true), "{}", j["lists"]);
}

/// Two regions sharing z6 tiles (task #796), on the roads fixture at catalogue 2: London (its tile
/// 31/21, way 100's owner 30/21, the lists and terrain of 32/21) and East, stretched to 33/21 (its
/// own tile: roads and terrain there), keeping 32/21's base pack (way 103's owner), which London
/// doesn't. Both downloaded and all here. The NAS's root, the catalogue, and the state.
async fn two_regions(home: &std::path::Path, nas: &std::path::Path) -> (std::path::PathBuf, Catalogue, S) {
    let root = nas.join("project");
    std::fs::create_dir_all(&root).unwrap();
    let r = store::nasfs::NasRoot::local_for_test(&root);
    let mut cat = catalogue(&r);
    put_roads(&root, &mut cat);
    for l in ["terrain", "roads"] {
        put(&r, &mut cat, &format!("layers/{l}/hi/6-33-21"), 7_000);
        cat.layers.get_mut(l).unwrap().hi.insert("6/33/21".into(), format!("layers/{l}/hi/6-33-21"));
    }
    cat.coverage = json!({"recorded": true, "regions": [
        {"id": "london", "name": "London", "outline": ["osm:175342"], "shapes": {"osm:175342": [[rect(-0.3, 51.4, -0.1, 51.6)]]}},
        {"id": "east", "name": "East", "outline": ["osm:1"], "shapes": {"osm:1": [[rect(-0.06, 51.4, 5.7, 51.6)]]}},
    ]})
    .into();
    cat.n = 2;
    cat.validate().unwrap();
    store::testkit::map_root(&root, &cat);
    let s = crate::test_state_with(home, &r, true);
    for id in ["london", "east"] {
        assert_eq!(region(&s, id, true).await.0, StatusCode::OK);
    }
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    for id in ["london", "east"] {
        assert_eq!(v["regions"][id]["state"].as_str(), Some("done"), "{v}");
    }
    let m = s.data.mirror.clone().unwrap();
    assert!(m.has(cat.content("base/6-32-21").unwrap()) && m.has(cat.content("layers/roads/hi/6-33-21").unwrap()));
    (root, cat, s)
}

/// Catalogue `n`: `cat` with these files made again (other bytes), written to the NAS and served.
fn next_catalogue(s: &S, root: &std::path::Path, cat: &Catalogue, n: u64, remade: &[&str]) -> Catalogue {
    let mut c = cat.clone();
    c.n = n;
    let at = store::nasfs::NasRoot::local_for_test(root);
    for l in remade {
        match l.strip_prefix("layers/terrain/") {
            Some(p) if p == "root" || p.starts_with("lo/") => {
                put_pack(&at, &mut c, l, &terrain_tiles(p == "root", 200.0))
            }
            _ => {
                put(&at, &mut c, l, 1_234);
            }
        }
    }
    c.validate().unwrap();
    store::testkit::map_root(&root, &c);
    assert!(tokio::task::block_in_place(|| s.data.serve_root_map()));
    c
}

/// The catalogues (of `cats`) whose file the map reads, for each logical name of z6 tile `t` that
/// one of them lists (a file the same in both: both).
fn tile_cats(s: &S, cats: &[&Catalogue], t: &str) -> BTreeMap<String, BTreeSet<u64>> {
    let logicals: BTreeSet<&String> =
        cats.iter().flat_map(|c| c.files.keys()).filter(|l| l.ends_with(&format!("/{t}"))).collect();
    logicals
        .into_iter()
        .filter_map(|l| {
            let read = s.data.content(l)?;
            Some((l.clone(), cats.iter().filter(|c| c.content(l) == Some(read.as_str())).map(|c| c.n).collect()))
        })
        .collect()
}

/// Updating London moves every tile it has to the new catalogue whole, East's files of them too
/// (31/21, the tile they share): no tile mixes catalogues. East keeps its own tiles (32/21 and
/// 33/21) at its catalogue, out of date for those alone.
#[tokio::test(flavor = "multi_thread")]
async fn updating_a_region_moves_the_tiles_it_shares_whole() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, two, s) = two_regions(home.path(), nas.path()).await;
    let m = s.data.mirror.clone().unwrap();
    let remade = ["base/6-32-21", "layers/terrain/hi/6-31-21", "layers/roads/hi/6-33-21"];
    let three = next_catalogue(&s, &root, &two, 3, &remade);
    tokio::task::block_in_place(|| settle(&s));
    // Both behind, read at 2.
    for t in ["6-31-21", "6-32-21", "6-33-21"] {
        assert!(tile_cats(&s, &[&two, &three], t).values().all(|n| n.contains(&2)), "{t}");
    }

    let (code, v) = call(post_region_update(State(s.clone()), Path("london".into())).await).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    tokio::task::block_in_place(|| settle(&s));
    let pins: BTreeMap<String, Pin> = s.downloads.asked().regions.iter().map(|r| (r.id.clone(), r.pin)).collect();
    assert_eq!(pins["london"], Pin { cat: 3, to: None });
    assert_eq!(pins["east"], Pin { cat: 2, to: None }, "East stays at its own");
    // Whole tiles: London's of 30/21 (an owner's base pack) and 31/21 (East's too) at 3; East's
    // own 32/21 and 33/21 at 2.
    for t in ["6-30-21", "6-31-21"] {
        let c = tile_cats(&s, &[&two, &three], t);
        assert!(c.values().all(|n| n.contains(&3)), "{t}: {c:?}");
    }
    for t in ["6-32-21", "6-33-21"] {
        let c = tile_cats(&s, &[&two, &three], t);
        assert!(c.values().all(|n| n.contains(&2)), "{t}: {c:?}");
    }
    assert!(m.has(two.content("base/6-32-21").unwrap()) && m.has(two.content("layers/roads/hi/6-33-21").unwrap()));
    // East: all here, out of date for its own two tiles' files alone.
    let v = get(&s).await;
    let e = &v["regions"]["east"];
    assert_eq!((e["state"].as_str(), e["cat"].as_u64()), (Some("done"), Some(2)), "{v}");
    assert_eq!(e["here"], e["bytes"]);
    assert_eq!(e["behind"]["files"], 2, "{e}");
    assert_eq!(e["behind"]["bytes"], 2 * 1_234);
    assert!(v["regions"]["london"].get("behind").is_none(), "{v}");

    // East updated in its turn: all at 3, the old 33/21 gone.
    let (code, v) = call(post_region_update(State(s.clone()), Path("east".into())).await).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    tokio::task::block_in_place(|| settle(&s));
    assert!(s.downloads.asked().regions.iter().all(|r| r.pin == Pin { cat: 3, to: None }));
    assert!(!m.has(two.content("layers/roads/hi/6-33-21").unwrap()) && !m.has(two.content("base/6-32-21").unwrap()));
}

/// A download whose tiles another's update moved, with nothing else changed, has nothing left to
/// copy: it moves on by itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_another_update_covered_moves_on_by_itself() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, two, s) = two_regions(home.path(), nas.path()).await;
    let three = next_catalogue(&s, &root, &two, 3, &["layers/terrain/hi/6-31-21"]);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    assert_eq!(v["regions"]["east"]["behind"]["files"], 1, "{v}");
    assert_eq!(call(post_region_update(State(s.clone()), Path("london".into())).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert!(s.downloads.asked().regions.iter().all(|r| r.pin == Pin { cat: 3, to: None }));
    let v = get(&s).await;
    assert!(v["regions"]["east"].get("behind").is_none(), "{v}");
    assert_eq!(v["regions"]["east"]["state"].as_str(), Some("done"));
    assert!(tile_cats(&s, &[&two, &three], "6-31-21").values().all(|n| n.contains(&3)));
}

#[test]
fn sizes_under_a_megabyte_are_in_kilobytes() {
    assert_eq!(
        [0, 1, 400, 1_234, 999_000, 4_200_000, 2_400_000_000].map(size),
        ["0 MB", "1 KB", "1 KB", "1 KB", "999 KB", "4 MB", "2.4 GB"]
    );
}

/// The World's per-tile records (roads' English names by z6 tile, global/roaden/6-x-y) move no
/// tile: updating the World copies none of a region's files, and a region behind stays behind.
#[tokio::test(flavor = "multi_thread")]
async fn updating_the_world_moves_no_region_tile() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, two, s) = two_regions(home.path(), nas.path()).await;
    assert_eq!(call(put_world(State(s.clone()), Json(On { on: true })).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let mut with = two.clone();
    put(&store::nasfs::NasRoot::local_for_test(&root), &mut with, "global/roaden/6-33-21", 90);
    with.global.insert("roaden/6-33-21".into(), "global/roaden/6-33-21".into());
    let three = next_catalogue(&s, &root, &with, 3, &["layers/roads/hi/6-33-21", "layers/terrain/hi/6-31-21"]);
    tokio::task::block_in_place(|| settle(&s));
    let v = get(&s).await;
    let world = v["world"]["behind"]["bytes"].as_u64().unwrap();
    assert!(world < 1_000, "the World's update copies its own files alone: {v}");

    assert_eq!(call(post_world_update(State(s.clone()),).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let pins: BTreeMap<String, Pin> = s.downloads.asked().regions.iter().map(|r| (r.id.clone(), r.pin)).collect();
    assert_eq!(pins["east"], Pin { cat: 2, to: None }, "East stays behind");
    assert_eq!(pins["london"], Pin { cat: 2, to: None }, "London stays behind");
    let m = s.data.mirror.clone().unwrap();
    assert!(!m.has(three.content("layers/roads/hi/6-33-21").unwrap()), "nothing of East's copied");
    assert!(!m.has(three.content("layers/terrain/hi/6-31-21").unwrap()), "nothing of London's copied");
    assert!(tile_cats(&s, &[&two, &three], "6-33-21")
        .iter()
        .filter(|(l, _)| l.starts_with("layers/"))
        .all(|(_, n)| n.contains(&2)));
}

/// A download whose Update would copy nothing beyond what it keeps already moves on at once, while
/// the files of a tile another download moved are still being copied: never "out of date" with
/// nothing to copy.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_with_nothing_left_to_copy_moves_on_before_the_copies() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, two, s) = two_regions(home.path(), nas.path()).await;
    let three = next_catalogue(&s, &root, &two, 3, &["layers/terrain/hi/6-31-21"]);
    // London at 3 (as downloaded again there): East's 31/21 moved, its new terrain not here yet.
    s.downloads
        .edit(|a| {
            *a.pin_mut(&Which::Region("london".into())).unwrap() = Pin { cat: 3, to: None };
            Ok(())
        })
        .unwrap();
    let m = s.data.mirror.clone().unwrap();
    assert!(!m.has(three.content("layers/terrain/hi/6-31-21").unwrap()));
    let v = get(&s).await;
    assert_eq!(v["regions"]["east"]["behind"]["files"], 0, "{v}");
    s.downloads.advance(&s.downloads.plan(&s.data), &m);
    let east = s.downloads.asked().regions.iter().find(|r| r.id == "east").unwrap().pin;
    assert_eq!(east, Pin { cat: 3, to: None });
    let v = get(&s).await;
    assert!(v["regions"]["east"].get("behind").is_none(), "{v}");
    assert_eq!(v["regions"]["east"]["state"].as_str(), Some("queued"), "{v}");
}

/// A small box at the middle of z6 tile (x, y).
fn in_tile(x: u32, y: u32) -> Vec<[f64; 2]> {
    let lon = (f64::from(x) + 0.5) / 64.0 * 360.0 - 180.0;
    let lat = roadcore::geo::merc_lat((f64::from(y) + 0.5) / 64.0);
    rect(lon - 0.1, lat - 0.1, lon + 0.1, lat + 0.1)
}

/// On the recorded chain (store's src/map/testdata: real catalogues 798 to 809, trimmed), for each
/// catalogue and the next: what an Update of the World, and of a region over each z6 tile the next
/// one changed, copies and lets go, by the download's own list as downloads.json keeps it, is the
/// two catalogues' difference.
#[test]
fn an_update_s_size_from_its_list_equals_the_two_catalogues_diff() {
    let cats: Vec<Catalogue> = store::testkit::recorded_chain();
    assert!(cats.len() > 2);
    let bytes = |items: &[&Item]| -> u64 {
        items.iter().map(|i| if let Item::File { size, .. } = i { *size } else { 0 }).sum()
    };
    let (mut changed, mut parts) = (0, 0);
    for w in cats.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let tiles: BTreeSet<(u32, u32)> = b
            .files
            .iter()
            .filter(|(l, f)| a.content(l) != Some(f.file.as_str()))
            .filter_map(|(_, f)| tile_of(&Item::File { name: f.file.clone(), size: 0 }))
            .collect();
        let at = |c: &Catalogue| At {
            cat: None,
            partial: false,
            world: Some(world_part(c)),
            regions: tiles
                .iter()
                .map(|&(x, y)| (format!("{x}-{y}"), region_parts(c, "", &[in_tile(x, y)], &Owners::new())))
                .collect(),
        };
        let (old, new) = (at(a), at(b));
        let kept: ListedAt = serde_json::from_slice(&serde_json::to_vec(&ListedAt::of(&old)).unwrap()).unwrap();
        let list = kept.at().unwrap();
        assert!(list.same(&old), "{}: the list as kept", a.n);
        let pairs = |x: &At| -> Vec<Part> {
            let mut v = vec![x.world.clone().unwrap()];
            v.extend(x.regions.values().flat_map(|r| [r.coarse.clone(), r.detail.clone()]));
            v
        };
        for ((l, o), n) in pairs(&list).iter().zip(pairs(&old)).zip(pairs(&new)) {
            parts += 1;
            let had: HashSet<&Item> = l.items.iter().collect();
            let copies: Vec<&Item> = n.items.iter().filter(|i| !had.contains(i)).collect();
            let goes = l.items.iter().filter(|i| !n.items.contains(i)).count();
            let diff: Vec<&Item> = n.items.iter().filter(|i| !o.items.contains(i)).collect();
            assert_eq!(copies, diff, "{} → {}", a.n, b.n);
            assert_eq!(bytes(&copies), bytes(&diff));
            assert_eq!(goes, o.items.iter().filter(|i| !n.items.contains(i)).count());
            changed += usize::from(!copies.is_empty());
        }
    }
    assert!(changed > 0 && parts > changed, "{changed} of {parts} parts changed");
}

/// A download kept far behind (its map long gone from this Mac, as from any log) says what
/// changed from its own list, keeps reading its own files, and updates.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_behind_the_log_compares_its_list() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let nas_root = nas.path().join("project");
    std::fs::create_dir_all(&nas_root).unwrap();
    let root = store::nasfs::NasRoot::local_for_test(&nas_root);
    // (Its hidata real: its owners known, its list whole.)
    let mut cat = catalogue(&root);
    put_roads(&nas_root, &mut cat);
    cat.n = 2;
    cat.validate().unwrap();
    store::testkit::map_root(&nas_root, &cat);
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(region(&s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    let roads = "layers/roads/hi/6-31-21";
    let old = cat.files[roads].file.clone();
    let mut c = cat.clone();
    for n in 3..=6 {
        let remade = if n == 3 { roads } else { "layers/roads/hi/6-30-21" };
        c = next_catalogue(&s, &nas_root, &c, n, &[remade]);
        tokio::task::block_in_place(|| settle(&s));
    }
    for s in [s.clone(), crate::test_state_with(home.path(), &root, true)] {
        tokio::task::block_in_place(|| settle(&s));
        assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 2, to: None });
        let v = get(&s).await;
        let r = &v["regions"]["london"];
        assert_eq!((r["state"].as_str(), r["cat"].as_u64()), (Some("done"), Some(2)), "{v}");
        assert_eq!(
            (r["behind"]["cat"].as_u64(), r["behind"]["files"].as_u64(), r["behind"]["gone"].as_u64()),
            (Some(6), Some(1), Some(1)),
            "{r}"
        );
        assert_eq!((r["behind"]["bytes"].as_u64(), &r["behind"]["what"]), (Some(1_234), &json!(["roads"])));
        assert_eq!(s.data.content(roads), Some(old.clone()), "its own older file");
    }
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(call(post_region_update(State(s.clone()), Path("london".into())).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| settle(&s));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 6, to: None });
    assert_eq!(s.data.content(roads), Some(c.files[roads].file.clone()));
    assert!(!s.data.mirror.as_ref().unwrap().has(&old));
}

// ---- the mirror's cursor in the map's log (docs/macs.md, Mirror) ---------------------------------

type Feed = Arc<store::map::feed::Feed>;

/// A lead over `feed` and the mirror's link to it (its cursor `mirror/test`).
fn mirror_link(s: &S, home: &std::path::Path, feed: &Feed) -> (crate::maplog::tests::FakeLead, MirrorLink) {
    let lead = crate::maplog::tests::FakeLead::on(0, feed.clone());
    let cfg = crate::maplog::tests::config(lead.url(), &home.join("cache"), 0);
    let link = MirrorLink::new(s.data.clone(), &cfg);
    (lead, link)
}

/// The mirror's rounds, each telling the lead, until it has nothing more to do.
fn settle_told(s: &S, link: &mut MirrorLink) {
    let m = s.data.mirror.clone().unwrap();
    while once(s, &m, Some(&mut *link)) {}
}

/// `cat` served (on the NAS's map and by this server), and appended to the lead's log at `map`.
fn serve(s: &S, nas: &std::path::Path, cat: &Catalogue, lead: Option<(&Feed, &mut store::map::Map)>) {
    store::testkit::map_root(nas, cat);
    assert!(s.data.serve_root_map());
    assert_eq!(s.data.catalogue().n, cat.n);
    if let Some((feed, map)) = lead {
        let b = map.batch_of(cat, "", "").unwrap();
        *map = map.apply(&b).unwrap();
        feed.push(Arc::new(map.clone()), &b);
    }
}

/// The mirror's cursor as the lead heard it: its `seq`, and the maps it said it copied and was
/// through at.
fn mirror_cursor(feed: &Feed) -> (u64, Option<u64>, Option<u64>) {
    let k = &feed.cursors()["mirror/test"];
    (k.seq, k.copying.map(|h| h.seq), k.synced.map(|h| h.seq))
}

/// London downloaded at map 1, then map 2 served with its roads made again, and its Update to 2
/// asked with the new roads file missing on the NAS (moved to the path returned): rounds that
/// can't get through. `rounds` runs the mirror; `on_map` serves a map.
async fn an_update_held_up(
    s: &S,
    root: &store::nasfs::NasRoot,
    nas: &std::path::Path,
    rounds: &mut dyn FnMut(&S),
    on_map: &mut dyn FnMut(&Catalogue),
) -> (Catalogue, String, std::path::PathBuf) {
    assert_eq!(region(s, "london", true).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| rounds(s));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 1, to: None });
    let mut second = s.data.catalogue().as_ref().clone();
    second.n = 2;
    let new_roads = put(root, &mut second, "layers/roads/hi/6-31-21", 25_000);
    second.validate().unwrap();
    tokio::task::block_in_place(|| on_map(&second));
    let aside = nas.join("aside");
    std::fs::rename(nas.join(&new_roads), &aside).unwrap();
    assert_eq!(call(post_region_update(State(s.clone()), Path("london".into())).await).await.0, StatusCode::OK);
    tokio::task::block_in_place(|| rounds(s));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 1, to: Some(2) });
    (second, new_roads, aside)
}

/// The mirror's cursor is the oldest map whose files it still copies: a download all here at an
/// older map holds nothing; an Update of map 2 holds it at 2 while the map served moves on to 3,
/// through rounds that can't get through and a restart of the server (made again from the
/// downloads' pins and the files here); once the Update's files are here it catches up to the map
/// served. The maps its rounds copy and get through at are said beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_mirror_copying_an_older_seq_holds_its_cursor() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    let first = catalogue(&root);
    let s = crate::test_state_with(home.path(), &root, true);
    let feed = store::map::feed::Feed::new();
    let mut map = store::map::Map::of(&first).unwrap();
    feed.start(Arc::new(map.clone()), 1, &[], Default::default());
    let (_lead, mut link) = mirror_link(&s, home.path(), &feed);
    let mut seen = Vec::new();
    let (_, new_roads, aside) = an_update_held_up(
        &s,
        &root,
        nas.path(),
        &mut |s| {
            settle_told(s, &mut link);
            seen.push(mirror_cursor(&feed));
        },
        &mut |c| serve(&s, nas.path(), c, Some((&feed, &mut map))),
    )
    .await;
    // (At 1, through. At 2, London behind but all here at 1, so the cursor at 2; the Update's round
    // not through: a file missing.)
    assert_eq!(seen, [(1, Some(1), Some(1)), (2, Some(2), Some(1))]);

    // Map 3, London's terrain made again: the Update stays at 2, and so does the cursor.
    let mut third = s.data.catalogue().as_ref().clone();
    third.n = 3;
    put(&root, &mut third, "layers/terrain/hi/6-31-21", 11_500);
    third.validate().unwrap();
    tokio::task::block_in_place(|| serve(&s, nas.path(), &third, Some((&feed, &mut map))));
    tokio::task::block_in_place(|| settle_told(&s, &mut link));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 1, to: Some(2) });
    assert_eq!(mirror_cursor(&feed), (2, Some(3), Some(1)));

    // The server started again: the same cursor, from what's on disk.
    drop(link);
    drop(s);
    let s = crate::test_state_with(home.path(), &root, true);
    assert_eq!(s.data.catalogue().n, 3);
    let (_lead2, mut link) = mirror_link(&s, home.path(), &feed);
    tokio::task::block_in_place(|| settle_told(&s, &mut link));
    assert_eq!(mirror_cursor(&feed).0, 2);

    // The file back: the Update moves London to 2, all here, and the cursor catches up to 3, its
    // round through there.
    std::fs::rename(&aside, nas.path().join(&new_roads)).unwrap();
    tokio::task::block_in_place(|| settle_told(&s, &mut link));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 2, to: None });
    assert!(s.data.mirror.as_ref().unwrap().has(&new_roads));
    assert_eq!(mirror_cursor(&feed), (3, Some(3), Some(3)));
}

/// A mirror whose cursor is older than the lead's log (no batch from there held: this Mac away a
/// long time) resyncs whole: the Update of the older map goes on at the one served, whose files
/// the NAS has, and gets through; what the download keeps stays here meanwhile.
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_older_than_the_log_resyncs_the_updates() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    catalogue(&root);
    let s = crate::test_state_with(home.path(), &root, true);
    let (second, new_roads, _aside) =
        an_update_held_up(&s, &root, nas.path(), &mut |s| settle(s), &mut |c| serve(&s, nas.path(), c, None)).await;
    let roads = "layers/roads/hi/6-31-21";
    let old_roads = s.data.content(roads).unwrap();
    // Map 3, the roads made again; the lead's log opened at 3.
    let mut third = second.clone();
    third.n = 3;
    let roads3 = put(&root, &mut third, roads, 26_000);
    third.validate().unwrap();
    tokio::task::block_in_place(|| serve(&s, nas.path(), &third, None));
    let feed = store::map::feed::Feed::new();
    feed.start(Arc::new(store::map::Map::of(&third).unwrap()), 3, &[], Default::default());
    let (_lead, mut link) = mirror_link(&s, home.path(), &feed);
    let m = s.data.mirror.clone().unwrap();
    assert!(m.has(&old_roads), "kept meanwhile");
    tokio::task::block_in_place(|| settle_told(&s, &mut link));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 3, to: None });
    assert!(m.has(&roads3) && !m.has(&new_roads) && !m.has(&old_roads));
    assert_eq!(mirror_cursor(&feed), (3, Some(3), Some(3)));
}

/// Task #1134 review: a reset answered while the map served is older than the lead's head (a
/// laptop's server started, or woken, from the map it kept, not yet caught up) waits for it: the
/// Update isn't moved on to that map, whose files GC may have removed too (here map 3's roads,
/// gone from the NAS), but to the head once it's served. The lead answers the reset once (the
/// mirror never heard), so the mirror's link keeps it till then.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_waits_for_the_map_served_to_reach_the_leads_head() {
    let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path());
    catalogue(&root);
    let s = crate::test_state_with(home.path(), &root, true);
    let (second, _new_roads, _aside) =
        an_update_held_up(&s, &root, nas.path(), &mut |s| settle(s), &mut |c| serve(&s, nas.path(), c, None)).await;
    let roads = "layers/roads/hi/6-31-21";
    // Map 3 served, its roads made again; the lead's head is 4, the roads made again since, and
    // GC has removed map 3's.
    let mut third = second.clone();
    third.n = 3;
    let roads3 = put(&root, &mut third, roads, 26_000);
    third.validate().unwrap();
    let mut fourth = third.clone();
    fourth.n = 4;
    let roads4 = put(&root, &mut fourth, roads, 27_000);
    fourth.validate().unwrap();
    tokio::task::block_in_place(|| serve(&s, nas.path(), &third, None));
    std::fs::remove_file(nas.path().join(&roads3)).unwrap();
    let feed = store::map::feed::Feed::new();
    feed.start(Arc::new(store::map::Map::of(&fourth).unwrap()), 0, &[], Default::default());
    let (_lead, mut link) = mirror_link(&s, home.path(), &feed);
    tokio::task::block_in_place(|| settle_told(&s, &mut link));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 1, to: Some(2) }, "not on to map 3");
    // Map 4 served: the reset acted on, the Update goes on at 4 and gets through.
    tokio::task::block_in_place(|| serve(&s, nas.path(), &fourth, None));
    tokio::task::block_in_place(|| settle_told(&s, &mut link));
    assert_eq!(s.downloads.asked().regions[0].pin, Pin { cat: 4, to: None });
    assert!(s.data.mirror.as_ref().unwrap().has(&roads4));
    assert_eq!(mirror_cursor(&feed), (4, Some(4), Some(4)));
}
