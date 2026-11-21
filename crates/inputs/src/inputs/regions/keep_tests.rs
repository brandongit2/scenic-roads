//! Task #1548: the resolution's pieces in a lasting folder (kept::Lasting, `scenic-build
//! regions-check --keep`). Each piece is keyed by what it reads: a check after an edit makes again
//! exactly the pieces reading it and takes the rest as kept; another code or `RESOLUTION` makes
//! every piece again; a piece cut short or changed is made again; the folder is held to its cap.
//! Every check here comes to what a check keeping nothing does.

use super::kept::{self, Kept, Lasting};
use super::water_tests::{bx, world_more, World, PASS, PINS};
use super::*;
use std::path::Path;

/// A folder of two regions sharing a remainder (`p/`), a region with water far from them and a
/// region without water.
fn files() -> BTreeMap<String, String> {
    [
        PINS,
        ("p/l.toml", "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,21.1\"]\nwater = []\n"),
        ("p/r.toml", "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\"]\nwater = []\n"),
        ("p/_remainder.toml", "of = \"box:30.0,20.0,33.0,21.0\"\n"),
        (
            "far.toml",
            "id = \"far\"\nname = \"Far\"\noutline = [\"osm:4\"]\nwater = [{ buffer = \"osm:4\", by = \"12 nmi\" }]\n",
        ),
        ("q.toml", "id = \"q\"\nname = \"Q\"\noutline = [\"box:40.0,20.0,41.0,21.0\"]\n"),
    ]
    .into_iter()
    .map(|(p, t)| (p.to_string(), t.to_string()))
    .collect()
}

fn the_world() -> World {
    world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)])
}

/// `files` resolved against `w`, its pieces kept in `kept` (none: kept nowhere).
fn resolve_with(w: &World, files: &BTreeMap<String, String>, kept: Option<&Kept>) -> resolve::Outcome {
    let folder = resolve::Folder::read(files.keys().cloned(), &|p| Ok(files[p].as_bytes().to_vec())).unwrap();
    let others = BTreeMap::new();
    let ctx = resolve::Context {
        pass: Some(PASS),
        outlines: Some(&w.o),
        extract: None,
        others: &others,
        memo: None,
        kept,
        reuse: None,
        names: String::new(),
        data: Some(&w.data),
        progress: Default::default(),
        window: None,
    };
    resolve::resolve(&folder, &ctx)
}

/// `files` checked as a new process would, its pieces kept in the lasting folder `dir` as code
/// `code` makes them: the pieces it made (each its stage and name: a region's id, a remainder's
/// file), and what it took; its outcome the same as a check keeping nothing.
fn check(w: &World, files: &BTreeMap<String, String>, dir: &Path, code: &str) -> (BTreeSet<(String, String)>, usize) {
    let kept = Kept::default();
    kept.open_lasting(Lasting::open_as(dir, "context", 1_000, code).unwrap()).unwrap();
    let out = resolve_with(w, files, Some(&kept));
    kept.stopped().unwrap();
    let fresh = resolve_with(w, files, None);
    assert!(fresh.problems.is_empty(), "{:?}", fresh.problems);
    assert_eq!(out.resolved, fresh.resolved, "as a check keeping nothing");
    assert_eq!(out.problems, fresh.problems);
    assert_eq!(out.needs, fresh.needs);
    let made: BTreeSet<(String, String)> = kept
        .made_keys()
        .into_iter()
        .map(|(s, k)| (kept::stage_name(s).to_string(), k.split(' ').next().unwrap().to_string()))
        .collect();
    let taken = kept.counts().values().map(|c| c.0).sum();
    (made, taken)
}

fn set(v: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// Every piece of `files()`.
fn all_pieces() -> BTreeSet<(String, String)> {
    set(&[
        ("resolved", "q"),
        ("land", "l"),
        ("land", "r"),
        ("land", "far"),
        ("remainder", "p/_remainder.toml"),
        ("remainder's cells", "p/_remainder.toml"),
        ("water", "far"),
        ("stored", "l"),
        ("stored", "r"),
        ("stored", "far"),
    ])
}

const CODE: &str = "the code";

/// An edit makes again exactly the pieces reading it: its region's, and those reading that
/// region's area where they're made; nothing changed, none.
#[test]
fn an_edit_makes_again_exactly_the_pieces_reading_it() {
    let w = the_world();
    let d = tempfile::tempdir().unwrap();
    let mut f = files();
    assert_eq!(check(&w, &f, d.path(), CODE).0, all_pieces());
    assert_eq!(check(&w, &f, d.path(), CODE), (set(&[]), all_pieces().len()), "nothing changed: every piece taken");
    // Far's water alone, and its area stored: its land reads its outline, not its water.
    f.insert("far.toml".into(), f["far.toml"].replace("12 nmi", "6 nmi"));
    assert_eq!(check(&w, &f, d.path(), CODE).0, set(&[("water", "far"), ("stored", "far")]));
    // R's outline: its land, and the remainder (R takes it, and its outline is in its cells); not
    // L's land, nor far's water. L's and R's areas stored (the median between them moved).
    f.insert("p/r.toml".into(), f["p/r.toml"].replace("32.0,19.9", "32.1,19.9"));
    assert_eq!(
        check(&w, &f, d.path(), CODE).0,
        set(&[("land", "r"), ("remainder", "p/_remainder.toml"), ("stored", "l"), ("stored", "r")])
    );
    // A region without water moved elsewhere: its own resolution alone.
    f.insert("q.toml".into(), f["q.toml"].replace("box:40.0,20.0,41.0,21.0", "box:44.0,20.0,45.0,21.0"));
    assert_eq!(check(&w, &f, d.path(), CODE).0, set(&[("resolved", "q")]));
    // Moved into a cell far's water is made in (none of it at sea there): far's water too, which
    // reads every region without water in its cells; its area as it was, taken.
    f.insert("q.toml".into(), f["q.toml"].replace("box:44.0,20.0,45.0,21.0", "box:22.5,20.0,23.0,20.5"));
    assert_eq!(check(&w, &f, d.path(), CODE).0, set(&[("resolved", "q"), ("water", "far")]));
    // Onto far's sea: far's water now less it, and its area stored.
    f.insert("q.toml".into(), f["q.toml"].replace("box:22.5,20.0,23.0,20.5", "box:21.05,20.4,21.1,20.6"));
    assert_eq!(check(&w, &f, d.path(), CODE).0, set(&[("resolved", "q"), ("water", "far"), ("stored", "far")]));
    // Back as it was: every piece as kept.
    assert_eq!(check(&w, &files(), d.path(), CODE).0, set(&[]));
}

/// A water written otherwise to the same area (12 nmi as metres): its water made again (it reads
/// its parts as written), its area as it was, but what its stored shapes are said to be from
/// changed, so stored again: a check keeping nothing says the new words.
#[test]
fn a_water_written_otherwise_is_stored_again() {
    let w = the_world();
    let d = tempfile::tempdir().unwrap();
    let mut f = files();
    check(&w, &f, d.path(), CODE);
    f.insert("far.toml".into(), f["far.toml"].replace("12 nmi", "22224 m"));
    assert_eq!(check(&w, &f, d.path(), CODE).0, set(&[("water", "far"), ("stored", "far")]));
}

/// A remainder reads the regions in its cells: a region without water there (by its piece), and a
/// region with water there not one of its own (by its outline), each moved, makes it again.
#[test]
fn a_remainder_is_made_again_when_a_region_in_its_cells_moves() {
    let w = the_world();
    let d = tempfile::tempdir().unwrap();
    let mut f = files();
    f.insert("q.toml".into(), f["q.toml"].replace("box:40.0,20.0,41.0,21.0", "box:31.4,20.4,31.6,20.6"));
    f.insert("n.toml".into(), "id = \"n\"\nname = \"N\"\noutline = [\"box:31.4,20.2,31.6,20.3\"]\nwater = []\n".into());
    check(&w, &f, d.path(), CODE);
    f.insert("q.toml".into(), f["q.toml"].replace("box:31.4,20.4,31.6,20.6", "box:31.3,20.4,31.6,20.6"));
    assert_eq!(
        check(&w, &f, d.path(), CODE).0,
        set(&[("resolved", "q"), ("remainder", "p/_remainder.toml"), ("stored", "l")]),
        "q, without water: the remainder L takes less"
    );
    f.insert("n.toml".into(), f["n.toml"].replace("box:31.4,20.2", "box:31.3,20.2"));
    assert_eq!(
        check(&w, &f, d.path(), CODE).0,
        set(&[("land", "n"), ("remainder", "p/_remainder.toml"), ("stored", "l"), ("stored", "n")]),
        "n, with water: the remainder L takes less"
    );
}

/// A region's land keeps its pieces in the cells the remainders read: the remainder grown into
/// more of L's cells (its cells made again), L's land made again to keep them.
#[test]
fn a_land_is_made_again_when_the_cells_kept_of_it_change() {
    let w = the_world();
    let d = tempfile::tempdir().unwrap();
    let mut f = files();
    check(&w, &f, d.path(), CODE);
    f.insert("p/_remainder.toml".into(), "of = \"box:29.0,20.0,33.0,21.0\"\n".into());
    assert_eq!(
        check(&w, &f, d.path(), CODE).0,
        set(&[("land", "l"), ("remainder", "p/_remainder.toml"), ("remainder's cells", "p/_remainder.toml")])
    );
}

/// Another code, or another `RESOLUTION`, makes every piece again (its pieces' names are
/// others'), and the first's are still taken after.
#[test]
fn another_code_or_resolution_makes_every_piece_again() {
    let w = the_world();
    let d = tempfile::tempdir().unwrap();
    let f = files();
    let now = kept::code_key(RESOLUTION, crate::inputs::code_of(module_path!()));
    assert_eq!(check(&w, &f, d.path(), &now).0, all_pieces());
    let code = kept::code_key(RESOLUTION, "another digest");
    assert_eq!(check(&w, &f, d.path(), &code).0, all_pieces(), "another code");
    let resolution = kept::code_key(RESOLUTION + 1, crate::inputs::code_of(module_path!()));
    assert_eq!(check(&w, &f, d.path(), &resolution).0, all_pieces(), "another RESOLUTION");
    assert_eq!(check(&w, &f, d.path(), &now).0, set(&[]));
}

/// Task #1572: the cells round water_tests' small world (its regions in the cells from 0° to 6°,
/// the disputed zone's from 4°): the cover check's window here.
fn cover_window() -> BTreeSet<tiles::Cell> {
    (-3..=4).flat_map(|x| (-2..=3).map(move |y| (x, y))).collect()
}

/// `files` (with `cover`) checked in the cells of `cover_window` as a new process would, its
/// pieces kept in the lasting folder `dir` as code `code` makes them: the cells whose cover check it
/// made, and how many it took; its outcome (the regions, the findings, how they fit, to the bit)
/// the same as a check keeping nothing.
fn cover_check(w: &World, files: &[(&str, String)], dir: &Path, code: &str) -> (BTreeSet<tiles::Cell>, usize) {
    let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    let folder = resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap();
    let others = BTreeMap::new();
    let window = cover_window();
    let run = |kept: Option<&Kept>| {
        let ctx = resolve::Context {
            pass: Some(PASS),
            outlines: Some(&w.o),
            extract: None,
            others: &others,
            memo: None,
            kept,
            reuse: None,
            names: String::new(),
            data: Some(&w.data),
            progress: Default::default(),
            window: Some(&window),
        };
        resolve::resolve(&folder, &ctx)
    };
    let kept = Kept::default();
    kept.open_lasting(Lasting::open_as(dir, "context", 1_000, code).unwrap()).unwrap();
    let out = run(Some(&kept));
    kept.stopped().unwrap();
    let fresh = run(None);
    assert_eq!(out.resolved, fresh.resolved, "as a check keeping nothing");
    assert_eq!(out.problems, fresh.problems);
    assert_eq!(format!("{:?}", out.fit), format!("{:?}", fresh.fit), "how they fit, to the bit");
    assert!(!fresh.fit.gaps.is_empty(), "the open sea's stretches measured");
    let made = kept
        .made_keys()
        .into_iter()
        .filter(|(s, _)| *s == kept::COVER)
        .map(|(_, k)| {
            let (x, y) = k.split(' ').next().unwrap().split_once(',').unwrap();
            (x.parse().unwrap(), y.parse().unwrap())
        })
        .collect();
    (made, kept.counts().get(&kept::COVER).map_or(0, |c| c.0))
}

fn cover_files() -> Vec<(&'static str, String)> {
    super::water_tests::all().into_iter().map(|(p, t)| (p, t.to_string())).collect()
}

/// The cells within one of `c` (the 3 × 3 round it).
fn round(c: tiles::Cell) -> BTreeSet<tiles::Cell> {
    (c.0 - 1..=c.0 + 1).flat_map(|x| (c.1 - 1..=c.1 + 1).map(move |y| (x, y))).collect()
}

/// Task #1572: a cell's cover check is kept as a piece, keyed by everything it reads (its pieces
/// and those round it, its land, its zones' water): nothing changed, every cell's is taken; an
/// edit changing pieces in one cell makes again that cell's and its eight neighbours' (whose own
/// pieces are as they were, but read the edited cell's), and no other.
#[test]
fn a_cells_cover_check_is_made_again_only_where_it_or_a_neighbour_changed() {
    let w = super::water_tests::world();
    let d = tempfile::tempdir().unwrap();
    let mut f = cover_files();
    let window = cover_window();
    assert_eq!(cover_check(&w, &f, d.path(), CODE), (window.clone(), 0), "every cell made");
    assert_eq!(cover_check(&w, &f, d.path(), CODE), (BTreeSet::new(), window.len()), "nothing changed: all taken");
    // The island's sea narrowed: its piece, and A's and B's (less the island's), in the cell
    // from 0°, 0° alone.
    let isl = f.iter().position(|x| x.0 == "w/isl.toml").unwrap();
    f[isl].1 = f[isl].1.replace("12 nmi", "6 nmi");
    let (made, taken) = cover_check(&w, &f, d.path(), CODE);
    assert_eq!(made, round((0, 0)), "the edited cell and its neighbours");
    assert_eq!(taken, window.len() - 9);
    assert_eq!(cover_check(&w, &f, d.path(), CODE).0, BTreeSet::new(), "as kept after");
    // Back as it was: every cell as kept before.
    assert_eq!(cover_check(&w, &cover_files(), d.path(), CODE).0, BTreeSet::new());
}

/// Task #1572: another code (or `RESOLUTION`) makes every cell's cover check again; the first's are
/// still taken after.
#[test]
fn another_code_makes_every_cells_cover_check_again() {
    let w = super::water_tests::world();
    let d = tempfile::tempdir().unwrap();
    let f = cover_files();
    let all = cover_window();
    let now = kept::code_key(RESOLUTION, crate::inputs::code_of(module_path!()));
    assert_eq!(cover_check(&w, &f, d.path(), &now).0, all);
    assert_eq!(cover_check(&w, &f, d.path(), &kept::code_key(RESOLUTION, "another digest")).0, all, "another code");
    assert_eq!(cover_check(&w, &f, d.path(), &now), (BTreeSet::new(), all.len()));
}

/// Task #1572: a check paused midway through the cover check (its cells kept in the job's scratch
/// as they're made, each a safe point) resumes to the same outcome, to the bit, each cell's check
/// made once over the two runs.
#[test]
fn a_check_paused_in_the_cover_check_resumes_to_the_same() {
    use jobrt::resume_log::STOP_AFTER;
    let w = super::water_tests::world();
    let f = cover_files();
    let m: BTreeMap<String, Vec<u8>> = f.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    let folder = resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap();
    let others = BTreeMap::new();
    let window = cover_window();
    let run = |kept: Option<&Kept>| {
        let ctx = resolve::Context {
            pass: Some(PASS),
            outlines: Some(&w.o),
            extract: None,
            others: &others,
            memo: None,
            kept,
            reuse: None,
            names: String::new(),
            data: Some(&w.data),
            progress: Default::default(),
            window: Some(&window),
        };
        resolve::resolve(&folder, &ctx)
    };
    let fresh = run(None);
    let cover = |made: Vec<(u8, String)>| made.iter().filter(|(s, _)| *s == kept::COVER).count();
    // (The pieces kept before the cover check: all but the cells' and the stored shapes, made after.)
    let (first, d0) = (Kept::default(), tempfile::tempdir().unwrap());
    first.open(d0.path(), "a", true);
    run(Some(&first));
    let before = first.made_keys().iter().filter(|(s, _)| *s != kept::COVER && *s != kept::STORED).count();
    let d = tempfile::tempdir().unwrap();
    let paused = Kept::default();
    STOP_AFTER.set(Some(before + 5));
    paused.open(d.path(), "a", true);
    run(Some(&paused));
    assert!(jobrt::control::is_paused(&paused.stopped().unwrap_err()));
    let made = cover(paused.made_keys());
    assert!((5..window.len()).contains(&made), "paused in the cover check, after {made} cells");
    let resumed = Kept::default();
    resumed.open(d.path(), "a", true);
    let out = run(Some(&resumed));
    resumed.stopped().unwrap();
    assert_eq!(made + cover(resumed.made_keys()), window.len(), "each cell made once");
    assert_eq!(out.resolved, fresh.resolved);
    assert_eq!(out.problems, fresh.problems);
    assert_eq!(format!("{:?}", out.fit), format!("{:?}", fresh.fit), "how they fit, to the bit");
}

/// The stage and name of the piece a lasting file holds (its whole key's last line).
fn piece_of(file: &Path) -> (String, String) {
    let b = std::fs::read(file).unwrap();
    let mut g = jobrt::resume_log::Get(&b[16 + 32..]);
    let whole = String::from_utf8(g.bytes().unwrap()).unwrap();
    let last = whole.lines().last().unwrap();
    let mut it = last.split(' ');
    let stage: u8 = it.next().unwrap().parse().unwrap();
    (kept::stage_name(stage).to_string(), it.next().unwrap().to_string())
}

/// A lasting piece changed (a point moved, which reads back as another area) or cut short (a
/// check stopped as it wrote, a disk full) is found, and made again; a write a stopped check left
/// is never read. Every other piece is taken.
#[test]
fn a_changed_or_cut_short_piece_is_made_again() {
    let w = the_world();
    let d = tempfile::tempdir().unwrap();
    let f = files();
    check(&w, &f, d.path(), CODE);
    let pieces: BTreeMap<(String, String), std::path::PathBuf> =
        std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().path()).map(|p| (piece_of(&p), p)).collect();
    assert_eq!(pieces.keys().cloned().collect::<BTreeSet<_>>(), all_pieces());
    // Far's land: a corner of its outline (21°) a unit east.
    let land = &pieces[&("land".to_string(), "far".to_string())];
    let mut b = std::fs::read(land).unwrap();
    let at = b.windows(8).position(|x| x == 210_000_000u64.to_le_bytes()).expect("a corner at 21°");
    b[at..at + 8].copy_from_slice(&210_000_001u64.to_le_bytes());
    std::fs::write(land, &b).unwrap();
    // The remainder, cut short.
    let rem = &pieces[&("remainder".to_string(), "p/_remainder.toml".to_string())];
    let b = std::fs::read(rem).unwrap();
    std::fs::write(rem, &b[..b.len() / 2]).unwrap();
    // Far's water, its piece's write left half done beside it (another piece under way).
    let water = &pieces[&("water".to_string(), "far".to_string())];
    let mut tmp = water.clone().into_os_string();
    tmp.push(".tmp-1-0");
    std::fs::write(&tmp, &std::fs::read(water).unwrap()[..40]).unwrap();
    std::fs::remove_file(water).unwrap();
    let (made, taken) = check(&w, &f, d.path(), CODE);
    assert_eq!(
        made,
        set(&[("land", "far"), ("remainder", "p/_remainder.toml"), ("water", "far")]),
        "the changed, the cut short and the one never written whole"
    );
    assert_eq!(taken, all_pieces().len() - 3);
    // Each made again kept whole: none made after.
    assert_eq!(check(&w, &f, d.path(), CODE).0, set(&[]));
}

/// The folder held to its cap: the most recently used pieces (written, or read) kept while they
/// fit, the first that doesn't and every one used before it deleted, a stopped check's write too.
#[test]
fn the_folder_is_held_to_its_cap_the_least_recently_used_deleted() {
    let d = tempfile::tempdir().unwrap();
    let l = Lasting::open_as(d.path(), "context", 1, CODE).unwrap();
    let piece = vec![7u8; 300_000];
    let at = |s: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + s);
    let mut files = Vec::new();
    for (i, name) in ["a", "b", "c", "d"].iter().enumerate() {
        l.write(kept::LAND, name, &piece).unwrap();
        let p = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().path()).find(|p| !files.contains(p)).unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(at(i as u64)).unwrap();
        files.push(p);
    }
    let tmp = d.path().join("x.piece.tmp-1-0");
    std::fs::write(&tmp, [0u8; 10]).unwrap();
    std::fs::File::options().write(true).open(&tmp).unwrap().set_modified(at(0)).unwrap();
    // "a", the oldest written, read now: the most recently used.
    assert_eq!(l.read(kept::LAND, "a").as_deref(), Some(&piece[..]));
    let s = l.sweep().unwrap();
    let left: BTreeSet<std::path::PathBuf> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(left, BTreeSet::from([files[0].clone(), files[3].clone(), files[2].clone()]), "a, d, c");
    assert_eq!((s.files, s.deleted), (3, 2), "{s:?}");
    assert!(s.bytes <= 1_000_000, "{s:?}");
    assert_eq!(l.read(kept::LAND, "b"), None, "deleted");
    assert_eq!(l.read(kept::LAND, "d").as_deref(), Some(&piece[..]));
}

/// Task #1566: the pool's check keeps its pieces in its Mac's lasting folder, in the cache and to
/// the cap its job is given (`kept::pool_lasting_of`; neither given, none kept), takes them in the
/// next check, and holds the folder to its cap once its outcome is saved (`kept_done`).
#[test]
fn the_pools_check_keeps_its_pieces_in_its_macs_folder() {
    use std::path::PathBuf;
    let c = |s: &str| Some(std::ffi::OsString::from(s));
    assert_eq!(kept::pool_lasting_of(None, Some("5".into())), None, "no cache");
    assert_eq!(kept::pool_lasting_of(c(""), Some("5".into())), None, "no cache");
    assert_eq!(kept::pool_lasting_of(c("/c"), None), None, "no cap: a job run by hand");
    assert_eq!(kept::pool_lasting_of(c("/c"), Some("x".into())), None);
    assert_eq!(kept::pool_lasting_of(c("/c"), Some(" 7 ".into())), Some((PathBuf::from("/c").join(kept::POOL_DIR), 7)));
    let d = tempfile::tempdir().unwrap();
    let cache = || Some(d.path().as_os_str().to_os_string());
    let dir = d.path().join(kept::POOL_DIR);
    let (w, f) = (the_world(), files());
    let fresh = resolve_with(&w, &f, None);
    let count = || std::fs::read_dir(&dir).map_or(0, |r| r.count());
    // None given: nothing kept there.
    let none = Regions::new(None);
    none.keep_pool(None);
    resolve_with(&w, &f, Some(&none.kept));
    none.kept_done();
    assert_eq!(count(), 0);
    // The first check makes every piece and keeps it there; the next takes them all.
    let first = Regions::new(None);
    first.keep_pool(kept::pool_lasting_of(cache(), Some("1000".into())));
    assert_eq!(resolve_with(&w, &f, Some(&first.kept)).resolved, fresh.resolved);
    assert_eq!(first.kept.made_keys().len(), all_pieces().len());
    first.kept_done();
    assert_eq!(count(), all_pieces().len(), "held: under the cap");
    let next = Regions::new(None);
    next.keep_pool(kept::pool_lasting_of(cache(), Some("1000".into())));
    assert_eq!(resolve_with(&w, &f, Some(&next.kept)).resolved, fresh.resolved);
    assert!(next.kept.made_keys().is_empty(), "every piece taken");
    // A cap of nothing: the folder emptied once the check is done.
    next.kept_done();
    let last = Regions::new(None);
    last.keep_pool(kept::pool_lasting_of(cache(), Some("0".into())));
    resolve_with(&w, &f, Some(&last.kept));
    last.kept_done();
    assert_eq!(count(), 0);
}
