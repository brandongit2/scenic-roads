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
        ("far.toml", "id = \"far\"\nname = \"Far\"\noutline = [\"osm:4\"]\nwater = [{ buffer = \"osm:4\", by = \"12 nmi\" }]\n"),
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
    let pieces: BTreeMap<(String, String), std::path::PathBuf> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| (piece_of(&p), p))
        .collect();
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
        let p = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| !files.contains(p))
            .unwrap();
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
