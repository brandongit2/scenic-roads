//! The map server, run as the built program, serves a map: a small one laid out as the NAS's
//! (`--root`), its tiles made with the codecs the build stores them with, the app's page files beside
//! it. Started on a free port (`--port 0`, the port it took read from its "listening on" line), with
//! homes of its own; no NAS, no fixed port, no wait on anything but the program itself. Each route
//! publish's live smoke test asked (task #1114, which replaced it) answers 200 with a body that
//! decodes to what the map holds:
//! - /api/ping, /api/meta, /api/catalog and the page (/);
//! - a terrain tile (terrarium-webp) and a road tile (rt7, gzip'd as the build stores it), as stored;
//! - slope tiles at z5 and z12 as stored, and at z16 made from its nearest ancestor that is shading:
//!   its z12 ancestor is a tile from before the shading's encoding (a PNG, as the live NAS's slope
//!   packs were), read as none, so it comes from z8 (be0d2b0a: every slope tile answered 503, the
//!   PNG decoded as WebP);
//! - a depth-shade tile at z16 made from its z12 ancestor, past an undecodable one at z14 (f0fb9b5d:
//!   a shading tile that won't decode was a 503 for every tile below it).

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The catalogue's number.
const N: u64 = 7;

/// A pack at `root`/`logical`.<hash>.pack of `layer`, each tile `(z, x, y, bytes, length before
/// gzip)` stored as given (gzip'd already when `gzip`), and its catalogue entry: the file's reference.
fn pack(
    root: &Path,
    logical: &str,
    layer: &str,
    gzip: bool,
    tiles: &[(u8, u32, u32, Vec<u8>, u32)],
) -> store::catalogue::FileRef {
    let file = format!("{logical}.{:016x}.pack", logical.len());
    let path = root.join(&file);
    std::fs::create_dir_all(root.join(logical.rsplit_once('/').unwrap().0)).unwrap();
    let mut w = store::pack::PackWriter::create(&path, serde_json::json!({"layer": layer}), gzip).unwrap();
    for (z, x, y, b, raw) in tiles {
        w.add(*z, *x, *y, b, *raw).unwrap();
    }
    w.finish().unwrap();
    store::catalogue::FileRef { file, size: std::fs::metadata(&path).unwrap().len(), ..Default::default() }
}

/// A shading tile, every pixel `s` % steep facing east, as the slope and depth steps store it.
fn shade(s: f32) -> Vec<u8> {
    codecs::shade_codec::encode(&vec![roadcore::shade::Px { s, u: [1.0, 0.0] }; 65536]).unwrap()
}

/// What a shading tile of `s` % decodes to (a pixel's code).
fn shade_code(s: f32) -> [u8; 3] {
    roadcore::shade::code(roadcore::shade::Px { s, u: [1.0, 0.0] })
}

/// A slope tile from before the shading's encoding: a PNG (`slope4-png`).
fn old_slope() -> Vec<u8> {
    let mut b = Vec::new();
    let mut e = png::Encoder::new(&mut b, 256, 256);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&vec![128u8; 256 * 256 * 3]).unwrap();
    b
}

/// The road tile's one line.
fn road() -> roadcore::tile::TileLine {
    roadcore::tile::TileLine {
        style: 3,
        way: 4242,
        true_len_dm: 1000,
        road_m: 100,
        pts: vec![[10, 20], [300, 400]],
        elev: vec![12_340, 12_400],
        grade: vec![4, 6],
        drape: vec![1234, 1240],
        sc: vec![[7; roadcore::tile::NCH]; 2],
        ..Default::default()
    }
}

/// The map: a catalogue naming terrain, roads, slope and depth-shade packs around the tiles asked
/// below, written as the NAS's newest map snapshot in `root`.
fn map(root: &Path) {
    use store::catalogue::Layer;
    let mut cat = store::catalogue::Catalogue::new(N);
    let mut layer =
        |name: &str, encoding: &str, minzoom: u8, maxzoom: u8, packs: Vec<(&str, &str, store::catalogue::FileRef)>| {
            let mut l = Layer { encoding: encoding.into(), minzoom, maxzoom, ..Default::default() };
            for (scope, key, f) in packs {
                let logical = format!("layers/{name}/{scope}/{}", key.replace('/', "-"));
                match scope {
                    "lo" => l.lo.insert(key.into(), logical.clone()),
                    _ => l.hi.insert(key.into(), logical.clone()),
                };
                cat.files.insert(logical, f);
            }
            cat.layers.insert(name.into(), l);
        };
    let heights = codecs::terrain_codec::encode(&mut vec![1234.0; 65536], codecs::terrain_codec::COARSER_M).unwrap();
    layer(
        "terrain",
        codecs::terrain_codec::ENCODING,
        0,
        16,
        vec![("lo", "3/2/2", pack(root, "layers/terrain/lo/3-2-2", "terrain", false, &[(5, 9, 11, heights, 0)]))],
    );
    let raw = roadcore::tile::encode(&[road()], 12);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&raw).unwrap();
    let gz = gz.finish().unwrap();
    layer(
        "roads",
        "rt7",
        4,
        14,
        vec![(
            "hi",
            "6/18/23",
            pack(root, "layers/roads/hi/6-18-23", "roads", true, &[(10, 300, 380, gz, raw.len() as u32)]),
        )],
    );
    layer(
        "slope",
        codecs::shade_codec::ENCODING,
        0,
        16,
        vec![
            (
                "lo",
                "3/2/2",
                pack(
                    root,
                    "layers/slope/lo/3-2-2",
                    "slope",
                    false,
                    &[(5, 9, 11, shade(20.0), 0), (8, 73, 89, shade(40.0), 0)],
                ),
            ),
            (
                "hi",
                "6/18/22",
                pack(
                    root,
                    "layers/slope/hi/6-18-22",
                    "slope",
                    false,
                    &[(12, 1202, 1463, shade(30.0), 0), (12, 1170, 1433, old_slope(), 0)],
                ),
            ),
        ],
    );
    let ds = terrain::depth::SHADE_LAYER;
    layer(
        ds,
        codecs::shade_codec::ENCODING,
        0,
        16,
        vec![(
            "hi",
            "6/4/16",
            // (Its z14, between them, WebP that won't decode.)
            pack(
                root,
                &format!("layers/{ds}/hi/6-4-16"),
                ds,
                false,
                &[
                    (12, 288, 1056, shade(60.0), 0),
                    (14, 1152, 4224, b"RIFF\x10\0\0\0WEBPVP8L\x04\0\0\0junk".to_vec(), 0),
                ],
            ),
        )],
    );
    store::testkit::map_root(root, &cat);
}

/// The server started, killed and reaped when dropped; what it printed so far in `log`.
struct Server {
    child: std::process::Child,
    log: Arc<Mutex<String>>,
    base: String,
}

impl Server {
    /// The built program over the map in `root`, the page files in `web`, homes of its own; once
    /// it says the port it took.
    fn start(root: &Path, web: &Path, home: &Path) -> Server {
        let fonts = home.join("fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        // (The program as cargo and nextest name it when the test runs: never `env!`'s compiled-in
        // path, tools/dev/testing.md.)
        let mut cmd = store::spawn::command(std::env::var("CARGO_BIN_EXE_server").expect("CARGO_BIN_EXE_server"));
        cmd.arg("--root")
            .arg(root)
            .arg("--home")
            .arg(home.join("app"))
            .arg("--web")
            .arg(web)
            .arg("--fonts")
            .arg(&fonts)
            .args(["--listen", "127.0.0.1", "--port", "0"])
            .current_dir(home)
            .env("HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let mut child = store::spawn::spawn(&mut cmd).unwrap();
        let log = Arc::new(Mutex::new(String::new()));
        let (err, l2) = (child.stderr.take().unwrap(), log.clone());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                let mut l = l2.lock().unwrap();
                l.push_str(&line);
                l.push('\n');
            }
        });
        let mut s = Server { child, log, base: String::new() };
        let port = store::testkit::wait_for("the server's listening line", || {
            if let Some(st) = s.child.try_wait().unwrap() {
                panic!("the server exited ({st}):\n{}", s.log());
            }
            let log = s.log();
            let at = log.find("listening on http://127.0.0.1:")? + "listening on http://127.0.0.1:".len();
            log[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse::<u16>().ok()
        });
        assert_ne!(port, 0, "{}", s.log());
        s.base = format!("http://127.0.0.1:{port}");
        s
    }

    fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }

    /// `path` asked: its content type and body, once it answered 200.
    fn get(&self, agent: &ureq::Agent, path: &str) -> (String, Vec<u8>) {
        let mut r =
            agent.get(format!("{}{path}", self.base)).call().unwrap_or_else(|e| panic!("{path}: {e}\n{}", self.log()));
        assert_eq!(r.status().as_u16(), 200, "{path}\n{}", self.log());
        let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        (ct, r.body_mut().read_to_vec().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

#[test]
fn the_server_serves_every_route_from_a_map_made_by_the_builds_codecs() {
    let (root, web, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    map(root.path());
    std::fs::write(web.path().join("index.html"), "<!doctype html><title>Scenic Roads</title>").unwrap();
    let s = Server::start(root.path(), web.path(), home.path());
    // (gzip'd answers unpacked, as a browser does: ureq's default.)
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .timeout_global(Some(store::testkit::WATCHDOG))
        .build()
        .into();
    let json = |path: &str| serde_json::from_slice::<serde_json::Value>(&s.get(&agent, path).1).unwrap();

    assert_eq!(s.get(&agent, "/api/ping").1, b"ok");
    let meta = json("/api/meta");
    assert_eq!(meta["catalog"], N, "{meta}");
    for l in ["terrain", "roads", "slope", terrain::depth::SHADE_LAYER] {
        assert!(meta["layers"].get(l).is_some(), "{l}: {meta}");
    }
    assert!(meta["versions"]["slope.tiles"].as_str().is_some_and(|v| v.ends_with("-shade-webp")), "{meta}");
    let cat = json("/api/catalog");
    assert_eq!(cat["n"], N, "{cat}");
    assert_eq!(cat["layers"]["roads"]["encoding"], "rt7", "{cat}");
    let (ct, page) = s.get(&agent, "/");
    assert!(ct.starts_with("text/html") && String::from_utf8_lossy(&page).contains("Scenic Roads"), "{ct}");

    let (ct, b) = s.get(&agent, "/tiles/terrain/5/9/11");
    assert_eq!(ct, "image/webp");
    let h = roadcore::grid::decode_terrain(&b).unwrap();
    assert!(h.len() == 65536 && h.iter().all(|&e| (e - 1234.0).abs() < 0.01), "{:?}", &h[..4]);
    let (_, b) = s.get(&agent, "/tiles/roads/10/300/380");
    let (lines, ext) = roadcore::tile::decode(&b).unwrap();
    assert_eq!(ext, 12);
    assert!(
        lines.len() == 1 && lines[0].way == road().way && lines[0].pts == road().pts && lines[0].elev == road().elev
    );

    let shading = |path: &str| {
        let (ct, b) = s.get(&agent, path);
        assert_eq!(ct, "image/webp", "{path}");
        let px = roadcore::shade::decode(&b).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(px.len(), 65536, "{path}");
        roadcore::shade::code(px[1000])
    };
    assert_eq!(shading("/tiles/slope/5/9/11"), shade_code(20.0), "z5 as stored");
    assert_eq!(shading("/tiles/slope/12/1202/1463"), shade_code(30.0), "z12 as stored");
    assert_eq!(shading("/tiles/slope/16/18732/22928"), shade_code(40.0), "z16 from z8, its z12 ancestor a PNG");
    assert_eq!(
        shading(&format!("/tiles/{}/16/4608/16896", terrain::depth::SHADE_LAYER)),
        shade_code(60.0),
        "z16 from z12, its z14 ancestor undecodable"
    );
}
