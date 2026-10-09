//! The basemap's pieces (docs/plan.md §4, "Mirror, per Mac"): the parts of Planetiler's worldwide
//! PMTiles archive a Mac downloads, each a PMTiles archive of its own.
//!
//! - **lo**: zooms 0–10 of the whole world (the World download's; ~1.7 GB of the 28.5 GB).
//! - **6-x-y**: zooms 11–14 under the z6 tile x/y (a region's download takes those of the z6 tiles
//!   it meets).
//!
//! On PMTiles' Hilbert curve the tiles under a z6 tile, at each zoom, are one run of tile ids, and
//! the archive is clustered (tile data in tile id order, repeated bytes pointing back), so a piece
//! is four runs of the directory and mostly sequential reads of the data: large ranged reads from
//! the NAS, never the whole file. Each piece is a whole archive: the header, a root directory
//! (and leaves when it'd be over 16 kB), the source's metadata, and the tiles' bytes as stored. Its
//! layout is fixed by the source's directory before any data is read (`plan`), so its size is
//! known ahead, a build cut short resumes where it stopped (`write`), and the same source gives
//! the same bytes. A gzipped tile's checksum is checked as it's copied.

use crate::iopool::IoError;
use crate::pmtiles::{encode_directory, zxy_to_tile_id, Compression, DirEntry, Header, PmTiles, HEADER_LEN};
use anyhow::{bail, ensure, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The World download's piece: zooms 0 to this.
pub const LO_MAX_ZOOM: u8 = 10;
/// A z6 tile's piece: these zooms.
pub const HI_ZOOMS: std::ops::RangeInclusive<u8> = 11..=14;
/// The header and root directory fit in the first 16 kB (the spec's rule: readers fetch that much
/// first).
const ROOT_ROOM: usize = 16384 - HEADER_LEN;
/// Largest NAS read while copying a piece's tiles.
const SPAN: u64 = 4 << 20;
/// Bytes between two tiles' data read over rather than read apart.
const GAP: u64 = 256 << 10;

/// A piece of the basemap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Piece {
    /// Zooms 0–10, worldwide.
    Lo,
    /// Zooms 11–14 under the z6 tile (x, y).
    Tile(u32, u32),
}

/// The first tile id of zoom `z`.
fn zoom_base(z: u8) -> u64 {
    ((1u64 << (2 * u32::from(z))) - 1) / 3
}

impl Piece {
    /// Its file's stem: `lo`, `6-x-y`.
    pub fn name(&self) -> String {
        match self {
            Piece::Lo => "lo".into(),
            Piece::Tile(x, y) => format!("6-{x}-{y}"),
        }
    }

    pub fn parse(s: &str) -> Option<Piece> {
        if s == "lo" {
            return Some(Piece::Lo);
        }
        let mut it = s.strip_prefix("6-")?.split('-');
        let (x, y) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
        (it.next().is_none() && x < 64 && y < 64).then_some(Piece::Tile(x, y))
    }

    /// The piece holding tile z/x/y; None past zoom 14.
    pub fn of(z: u8, x: u32, y: u32) -> Option<Piece> {
        match z {
            0..=LO_MAX_ZOOM => Some(Piece::Lo),
            _ if HI_ZOOMS.contains(&z) => Some(Piece::Tile(x >> (z - 6), y >> (z - 6))),
            _ => None,
        }
    }

    /// Its tiles, as runs of tile ids (half open, ascending).
    pub fn ranges(&self) -> Vec<(u64, u64)> {
        match *self {
            Piece::Lo => vec![(0, zoom_base(LO_MAX_ZOOM + 1))],
            Piece::Tile(x, y) => {
                // (A z6 tile's position along its zoom's curve: the leading digits of its
                // descendants' at every deeper zoom.)
                let d6 = zxy_to_tile_id(6, x, y).map_or(0, |id| id - zoom_base(6));
                HI_ZOOMS
                    .map(|z| {
                        let n = 1u64 << (2 * u32::from(z - 6));
                        let start = zoom_base(z) + d6 * n;
                        (start, start + n)
                    })
                    .collect()
            }
        }
    }
}

/// How a piece is laid out, worked out from the source's directory alone.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Everything before the tile data: header, root directory, metadata, leaf directories.
    prefix: Vec<u8>,
    /// The tiles' bytes to copy, in the order they're written: (offset in the source file,
    /// length).
    reads: Vec<(u64, u32)>,
    gzipped: bool,
    /// The piece's size in bytes.
    pub size: u64,
    /// Its tile entries and distinct tiles.
    pub entries: u64,
    pub contents: u64,
}

fn gzip(b: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(b).expect("in memory");
    e.finish().expect("in memory")
}

/// The root directory and leaves for `entries`: the root alone while it fits the first 16 kB, else
/// leaves of 4,096 entries (doubled until the root fits).
fn directories(entries: &[DirEntry]) -> (Vec<u8>, Vec<u8>) {
    let root = gzip(&encode_directory(entries));
    if root.len() <= ROOT_ROOM {
        return (root, Vec::new());
    }
    let mut per = 4096;
    loop {
        let mut leaves = Vec::new();
        let mut top = Vec::new();
        for chunk in entries.chunks(per) {
            let leaf = gzip(&encode_directory(chunk));
            top.push(DirEntry { tile_id: chunk[0].tile_id, offset: leaves.len() as u64, length: leaf.len() as u32, run_length: 0 });
            leaves.extend_from_slice(&leaf);
        }
        let root = gzip(&encode_directory(&top));
        if root.len() <= ROOT_ROOM {
            return (root, leaves);
        }
        per *= 2;
    }
}

/// An archive of `entries` (tile ids ascending; offsets into `data`), the rest of its header from
/// `like`, its metadata `meta` (JSON, gzipped here), and its zooms and bounds as given. Its bytes
/// before the data, and the header.
fn prefix(like: &Header, entries: &[DirEntry], contents: u64, data_len: u64, meta: &[u8], zooms: (u8, u8), bounds: [i32; 4]) -> Vec<u8> {
    let (root, leaves) = directories(entries);
    let meta = gzip(meta);
    let root_offset = HEADER_LEN as u64;
    let metadata_offset = root_offset + root.len() as u64;
    let leaf_offset = metadata_offset + meta.len() as u64;
    let data_offset = leaf_offset + leaves.len() as u64;
    let h = Header {
        version: 3,
        root_offset,
        root_length: root.len() as u64,
        metadata_offset,
        metadata_length: meta.len() as u64,
        leaf_offset,
        leaf_length: leaves.len() as u64,
        data_offset,
        data_length: data_len,
        addressed_tiles: entries.iter().map(|e| u64::from(e.run_length)).sum(),
        tile_entries: entries.len() as u64,
        tile_contents: contents,
        clustered: true,
        internal_compression: Compression::Gzip,
        tile_compression: like.tile_compression,
        tile_type: like.tile_type,
        min_zoom: zooms.0,
        max_zoom: zooms.1,
        min_lon_e7: bounds[0],
        min_lat_e7: bounds[1],
        max_lon_e7: bounds[2],
        max_lat_e7: bounds[3],
        center_zoom: zooms.0,
        center_lon_e7: ((i64::from(bounds[0]) + i64::from(bounds[2])) / 2) as i32,
        center_lat_e7: ((i64::from(bounds[1]) + i64::from(bounds[3])) / 2) as i32,
    };
    let mut out = h.to_bytes();
    out.extend_from_slice(&root);
    out.extend_from_slice(&meta);
    out.extend_from_slice(&leaves);
    out
}

/// A z6 tile's box in 1e-7 degrees (west, south, east, north).
fn tile_bounds(x: u32, y: u32) -> [i32; 4] {
    let lon = |x: u32| f64::from(x) / 64.0 * 360.0 - 180.0;
    let lat = |y: u32| (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / 64.0)).sinh().atan().to_degrees();
    [lon(x), lat(y + 1), lon(x + 1), lat(y)].map(|d| (d * 1e7).round() as i32)
}

/// How `piece` of the archive `pm` is laid out: its directory's runs are read (the leaves that
/// reach into them), none of its tiles.
pub fn plan(pm: &PmTiles, piece: Piece) -> Result<Plan> {
    let h = pm.header();
    let mut entries: Vec<DirEntry> = Vec::new();
    for (lo, hi) in piece.ranges() {
        pm.entries_in(lo, hi, &mut |e| entries.push(e))?;
    }
    // The distinct tiles, in the source's order (a clustered source's order: tile id order, as
    // each first appears), placed one after another.
    let mut at: BTreeMap<u64, (u32, u64)> = BTreeMap::new();
    for e in &entries {
        ensure!(e.offset.checked_add(u64::from(e.length)).is_some_and(|end| end <= h.data_length), "a tile points outside the tile data");
        at.entry(e.offset).or_insert((e.length, 0));
    }
    let mut data_len = 0u64;
    let mut reads = Vec::with_capacity(at.len());
    for (off, (len, new)) in at.iter_mut() {
        *new = data_len;
        data_len += u64::from(*len);
        reads.push((h.data_offset + *off, *len));
    }
    for e in &mut entries {
        let (len, new) = at[&e.offset];
        ensure!(len == e.length, "two tiles share an offset with different lengths");
        e.offset = new;
    }
    let meta = serde_json::to_vec(&pm.metadata()?)?;
    let (zooms, bounds) = match piece {
        Piece::Lo => ((h.min_zoom, h.max_zoom.min(LO_MAX_ZOOM)), [h.min_lon_e7, h.min_lat_e7, h.max_lon_e7, h.max_lat_e7]),
        Piece::Tile(x, y) => ((*HI_ZOOMS.start(), *HI_ZOOMS.end()), tile_bounds(x, y)),
    };
    let prefix = prefix(h, &entries, at.len() as u64, data_len, &meta, zooms, bounds);
    Ok(Plan { size: prefix.len() as u64 + data_len, entries: entries.len() as u64, contents: at.len() as u64, prefix, reads, gzipped: h.tile_compression == Compression::Gzip })
}

/// How a `write` ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrote {
    /// The piece is whole at its path (synced): ready to be renamed into place.
    Done,
    /// `stop()` said stop; it resumes from where it is.
    Stopped,
}

/// Writes the piece `plan` lays out to `part`, resuming what's there (when its prefix is this
/// plan's: else from the start), reading the source's tile bytes with `read` (offset, length),
/// in reads of up to 4 MB. `stop()` is asked before each read; `progress` told the bytes written
/// after each. A gzipped tile whose checksum fails is an error (the NAS's copy is damaged, or the
/// read was).
pub fn write(plan: &Plan, part: &Path, read: &dyn Fn(u64, usize) -> Result<Vec<u8>, IoError>, stop: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64)) -> Result<Wrote> {
    let mut f = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(part).with_context(|| format!("open {}", part.display()))?;
    let len = f.metadata()?.len();
    let p = plan.prefix.len() as u64;
    let resumes = len >= p && len <= plan.size && {
        let mut have = vec![0u8; p as usize];
        f.read_exact(&mut have).is_ok() && have == plan.prefix
    };
    let mut have = if resumes {
        len
    } else {
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&plan.prefix)?;
        p
    };
    f.seek(SeekFrom::Start(have))?;
    progress(have);
    // The first read not yet written whole, and where it starts in the piece.
    let mut i = 0;
    let mut out = p;
    while i < plan.reads.len() && out + u64::from(plan.reads[i].1) <= have {
        out += u64::from(plan.reads[i].1);
        i += 1;
    }
    while i < plan.reads.len() {
        if stop() {
            f.sync_all()?;
            return Ok(Wrote::Stopped);
        }
        // A run of reads near each other in the source, read at once.
        let first = plan.reads[i].0;
        let mut j = i + 1;
        let mut end = first + u64::from(plan.reads[i].1);
        while j < plan.reads.len() {
            let (o, l) = plan.reads[j];
            let e = o + u64::from(l);
            if o < end || o - end > GAP || e - first > SPAN {
                break;
            }
            end = e;
            j += 1;
        }
        let span = read(first, (end - first) as usize).map_err(anyhow::Error::from)?;
        ensure!(span.len() as u64 == end - first, "a short read of the basemap");
        for &(o, l) in &plan.reads[i..j] {
            let tile = &span[(o - first) as usize..(o - first) as usize + l as usize];
            if plan.gzipped {
                check_gzip(tile).with_context(|| format!("the basemap's tile at byte {o} doesn't read: the NAS's copy is damaged"))?;
            }
            // (Resuming inside a tile: its first bytes are here already.)
            let skip = have.saturating_sub(out).min(u64::from(l)) as usize;
            f.write_all(&tile[skip..])?;
            out += u64::from(l);
            have = out;
        }
        progress(have);
        i = j;
    }
    f.sync_all()?;
    let len = f.metadata()?.len();
    if len != plan.size {
        bail!("{} is {len} bytes once written, not {}", part.display(), plan.size);
    }
    Ok(Wrote::Done)
}

/// Whether a gzipped tile decompresses whole, its checksum and length right.
fn check_gzip(b: &[u8]) -> Result<()> {
    if b.is_empty() {
        return Ok(());
    }
    std::io::copy(&mut GzDecoder::new(b), &mut std::io::sink())?;
    Ok(())
}

/// A small archive of these tiles ((z, x, y, bytes as stored)), each stored once and in tile id
/// order, its tiles compressed as `tile_compression` says: for tests, here and in the server.
pub fn archive(tiles: &[(u8, u32, u32, Vec<u8>)], tile_compression: Compression) -> Vec<u8> {
    let mut t: Vec<(u64, &Vec<u8>, u8)> = tiles.iter().map(|(z, x, y, b)| (zxy_to_tile_id(*z, *x, *y).expect("a tile"), b, *z)).collect();
    t.sort_by_key(|t| t.0);
    let mut data = Vec::new();
    let mut seen: BTreeMap<&[u8], u64> = BTreeMap::new();
    let mut entries: Vec<DirEntry> = Vec::new();
    for (id, b, _) in &t {
        let off = *seen.entry(b.as_slice()).or_insert_with(|| {
            let o = data.len() as u64;
            data.extend_from_slice(b);
            o
        });
        match entries.last_mut() {
            Some(l) if l.tile_id + u64::from(l.run_length) == *id && l.offset == off => l.run_length += 1,
            _ => entries.push(DirEntry { tile_id: *id, offset: off, length: b.len() as u32, run_length: 1 }),
        }
    }
    let zooms = (t.iter().map(|t| t.2).min().unwrap_or(0), t.iter().map(|t| t.2).max().unwrap_or(0));
    let like = Header {
        version: 3,
        root_offset: 0,
        root_length: 0,
        metadata_offset: 0,
        metadata_length: 0,
        leaf_offset: 0,
        leaf_length: 0,
        data_offset: 0,
        data_length: 0,
        addressed_tiles: 0,
        tile_entries: 0,
        tile_contents: 0,
        clustered: true,
        internal_compression: Compression::Gzip,
        tile_compression,
        tile_type: crate::pmtiles::TileType::Mvt,
        min_zoom: 0,
        max_zoom: 0,
        min_lon_e7: 0,
        min_lat_e7: 0,
        max_lon_e7: 0,
        max_lat_e7: 0,
        center_zoom: 0,
        center_lon_e7: 0,
        center_lat_e7: 0,
    };
    let mut out = prefix(&like, &entries, seen.len() as u64, data.len() as u64, br#"{"name":"test"}"#, zooms, [-1_800_000_000, -850_511_287, 1_800_000_000, 850_511_287]);
    out.extend_from_slice(&data);
    out
}

/// Reads `path` whole as an archive: for checks and tests.
pub fn open_file(path: &Path) -> Result<PmTiles> {
    let mut b = Vec::new();
    File::open(path)?.read_to_end(&mut b)?;
    PmTiles::open(Box::new(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmtiles::tile_id_to_zxy;
    use std::cell::Cell;

    fn gz(b: &[u8]) -> Vec<u8> {
        gzip(b)
    }

    #[test]
    fn a_z6_tiles_descendants_are_one_run_of_ids_at_each_zoom() {
        for (x, y) in [(0, 0), (31, 21), (63, 63), (10, 50)] {
            let ranges = Piece::Tile(x, y).ranges();
            assert_eq!(ranges.len(), 4);
            for (k, (lo, hi)) in ranges.into_iter().enumerate() {
                let z = 11 + k as u8;
                for id in [lo, lo + 1, (lo + hi) / 2, hi - 1] {
                    let (tz, tx, ty) = tile_id_to_zxy(id).unwrap();
                    assert_eq!((tz, tx >> (z - 6), ty >> (z - 6)), (z, x, y), "id {id}");
                }
                // And every one of them: as many ids as tiles.
                assert_eq!(hi - lo, 1 << (2 * (z - 6)));
            }
        }
        assert_eq!(Piece::of(10, 1000, 3), Some(Piece::Lo));
        assert_eq!(Piece::of(14, 12917, 8130), Some(Piece::Tile(50, 31)));
        assert_eq!(Piece::of(15, 0, 0), None);
        for p in [Piece::Lo, Piece::Tile(50, 31)] {
            assert_eq!(Piece::parse(&p.name()), Some(p));
        }
        assert_eq!(Piece::parse("6-64-0"), None);
    }

    /// A world of tiles: z0–10 a few, and under the z6 tiles 32/21 and 33/21 some of z11–14; the
    /// sea, the same bytes, all over z14 (stored once).
    fn world() -> (Vec<u8>, Tiles) {
        let mut tiles = Vec::new();
        for (z, x, y) in [(0, 0, 0), (5, 16, 10), (8, 128, 85), (10, 512, 340), (10, 530, 341)] {
            tiles.push((z, x, y, gz(format!("{z}/{x}/{y}").as_bytes())));
        }
        for (x6, y6) in [(32u32, 21u32), (33, 21)] {
            for z in 11..=14u8 {
                let s = z - 6;
                for k in 0..6u32 {
                    let (x, y) = ((x6 << s) + k * 3, (y6 << s) + k);
                    let body = if z == 14 && k % 2 == 0 { gz(b"sea") } else { gz(format!("{z}/{x}/{y}").as_bytes()) };
                    tiles.push((z, x, y, body));
                }
            }
        }
        (archive(&tiles, Compression::Gzip), tiles)
    }

    type Tiles = Vec<(u8, u32, u32, Vec<u8>)>;

    fn pieces_of(tiles: &[(u8, u32, u32, Vec<u8>)], p: Piece) -> Vec<&(u8, u32, u32, Vec<u8>)> {
        tiles.iter().filter(|t| Piece::of(t.0, t.1, t.2) == Some(p)).collect()
    }

    fn build(src: &[u8], p: Piece, dir: &Path) -> (Plan, std::path::PathBuf) {
        let pm = PmTiles::open(Box::new(src.to_vec())).unwrap();
        let plan = plan(&pm, p).unwrap();
        let part = dir.join(format!("{}.pmtiles", p.name()));
        let read = |o: u64, l: usize| Ok(src[o as usize..o as usize + l].to_vec());
        assert_eq!(write(&plan, &part, &read, &|| false, &mut |_| {}).unwrap(), Wrote::Done);
        (plan, part)
    }

    #[test]
    fn a_piece_holds_its_tiles_and_no_others() {
        let (src, tiles) = world();
        let d = tempfile::tempdir().unwrap();
        for p in [Piece::Lo, Piece::Tile(32, 21), Piece::Tile(33, 21), Piece::Tile(0, 0)] {
            let (plan, path) = build(&src, p, d.path());
            assert_eq!(std::fs::metadata(&path).unwrap().len(), plan.size, "its size known ahead");
            let a = open_file(&path).unwrap();
            for t in &tiles {
                let got = a.get(t.0, t.1, t.2).unwrap();
                if Piece::of(t.0, t.1, t.2) == Some(p) {
                    assert_eq!(got.as_ref(), Some(&t.3), "{p:?} {}/{}/{}", t.0, t.1, t.2);
                } else {
                    assert_eq!(got, None, "{p:?} has {}/{}/{}", t.0, t.1, t.2);
                }
            }
            let n = pieces_of(&tiles, p).len() as u64;
            assert_eq!(a.header().addressed_tiles, n);
            if p == Piece::Tile(32, 21) {
                // The sea's three z14 tiles stored once.
                assert_eq!(plan.contents, n - 2);
            }
        }
    }

    #[test]
    fn a_big_piece_gets_leaf_directories() {
        let mut tiles = Vec::new();
        // (Tiles here and there, of every length: a directory that doesn't compress to nothing.)
        for x in 0..256u32 {
            for y in 0..256u32 {
                let h = blake3::hash(&[x as u8, y as u8]).as_bytes()[0];
                if !h.is_multiple_of(3) {
                    tiles.push((14, (32 << 8) + x, (21 << 8) + y, gz(&vec![h; usize::from(h) * 7 + (x as usize % 13)])));
                }
            }
        }
        let src = archive(&tiles, Compression::Gzip);
        let d = tempfile::tempdir().unwrap();
        let (_, path) = build(&src, Piece::Tile(32, 21), d.path());
        let a = open_file(&path).unwrap();
        assert!(a.header().leaf_length > 0);
        for t in tiles.iter().step_by(97) {
            assert_eq!(a.get(t.0, t.1, t.2).unwrap().as_ref(), Some(&t.3));
        }
    }

    #[test]
    fn a_piece_cut_short_resumes_and_comes_out_the_same() {
        let (src, _) = world();
        let d = tempfile::tempdir().unwrap();
        let (plan, whole) = build(&src, Piece::Tile(32, 21), d.path());
        let want = std::fs::read(&whole).unwrap();
        let part = d.path().join("part");
        let read = |o: u64, l: usize| Ok(src[o as usize..o as usize + l].to_vec());
        // Stopped at once: the prefix alone.
        assert_eq!(write(&plan, &part, &read, &|| true, &mut |_| {}).unwrap(), Wrote::Stopped);
        // Cut inside a tile (as a crash would): resumed from there.
        let cut = std::fs::metadata(&part).unwrap().len() + 7;
        std::fs::write(&part, &want[..cut as usize]).unwrap();
        let first = Cell::new(0);
        assert_eq!(write(&plan, &part, &read, &|| false, &mut |h| if first.get() == 0 { first.set(h) }).unwrap(), Wrote::Done);
        assert_eq!(first.get(), cut);
        assert_eq!(std::fs::read(&part).unwrap(), want);
        // Something else there (another plan's prefix): started again.
        std::fs::write(&part, b"PMTiles junk").unwrap();
        write(&plan, &part, &read, &|| false, &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), want);
    }

    #[test]
    fn a_damaged_tile_is_caught() {
        let (mut src, tiles) = world();
        let pm = PmTiles::open(Box::new(src.clone())).unwrap();
        let t = pieces_of(&tiles, Piece::Lo)[1];
        let (off, len) = pm.locate(t.0, t.1, t.2).unwrap().unwrap();
        src[(off + u64::from(len) - 5) as usize] ^= 0x40;
        let plan = plan(&pm, Piece::Lo).unwrap();
        let d = tempfile::tempdir().unwrap();
        let read = |o: u64, l: usize| Ok(src[o as usize..o as usize + l].to_vec());
        let e = write(&plan, &d.path().join("lo"), &read, &|| false, &mut |_| {}).unwrap_err();
        assert!(format!("{e:#}").contains("damaged"), "{e:#}");
    }

    /// The live basemap, when the NAS is mounted: a piece of it holds the very tiles the archive
    /// has there.
    #[test]
    #[ignore = "real data: reads the live basemap on the NAS (run with --ignored)"]
    fn the_live_basemaps_pieces() {
        let dir = Path::new("/Volumes/personal/projects/scenic-roads/layers/basemap");
        let Some(path) = std::fs::read_dir(dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("world-")))) else { return };
        let src = crate::range::PlainFile::open(&path).unwrap();
        let pm = PmTiles::open(Box::new(crate::range::PlainFile::open(&path).unwrap())).unwrap();
        // Andorra's z6 tile (32/23): small.
        let p = Piece::Tile(32, 23);
        let plan = plan(&pm, p).unwrap();
        let d = tempfile::tempdir().unwrap();
        let part = d.path().join("p.pmtiles");
        let read = |o: u64, l: usize| crate::range::RangeRead::read_at(&src, o, l);
        write(&plan, &part, &read, &|| false, &mut |_| {}).unwrap();
        let a = open_file(&part).unwrap();
        for (z, x, y) in [(11, 1036, 760), (12, 2073, 1520), (14, 8290, 6083)] {
            assert_eq!(a.get(z, x, y).unwrap(), pm.get(z, x, y).unwrap(), "{z}/{x}/{y}");
        }
        eprintln!("piece 6-32-23: {} bytes, {} entries", plan.size, plan.entries);
    }
}
