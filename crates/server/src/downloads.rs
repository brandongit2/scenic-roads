//! Downloads (docs/macs.md, "Mirror, per Mac"; docs/surface.md, the Regions panel): what the owner has
//! downloaded to this Mac for when it's away from the NAS. Nothing else is copied here, and nothing
//! downloaded goes until the owner removes it (store::mirror).
//!
//!   GET    /api/downloads                 this Mac's room, the World download, every region the
//!                                         catalogue has with its size, how much of it is here,
//!                                         whether it's downloaded and whether it's behind the
//!                                         catalogue (what changed, the bytes an update copies),
//!                                         the copy under way
//!   PUT    /api/downloads/world           {on}: download the World, zoomed out, or remove it
//!   PUT    /api/downloads/regions/{id}    {on}: download a region, or remove it (with its
//!                                         terrain detail)
//!   PUT    /api/downloads/regions/{id}/detail {on}: download a downloaded region's terrain detail,
//!                                         or remove it
//!   POST   /api/downloads/world/update, /api/downloads/regions/{id}/update,
//!          /api/downloads/regions/{id}/detail/update: update a download to the catalogue served
//!   PUT    /api/downloads/reserve         {gb}: the free space downloads leave on the disk
//!                                         (crate::reserve)
//!
//! A download or update that wouldn't fit (all that's downloaded, less what's here, more than the
//! free space above the reserve) is refused, with the numbers. What's downloaded is this Mac's alone:
//! `<home>/downloads.json` (docs/formats.md), never the NAS.
//!
//! **Updates only when asked.** Each download (the World, a region, a region's terrain detail) stays
//! at the catalogue it was downloaded or last updated at (its `Pin`): a new catalogue copies nothing.
//! The map reads a download's own older files while it's behind (Data::content_in), the NAS for
//! what it lacks (away from the NAS, nothing: the map is the downloads alone). An Update copies the catalogue served's files for it alongside the old ones; once all
//! are here, the download moves to that catalogue and the files only the old one named go. A
//! download moves on by itself only when that copies nothing: all an Update would copy is here (the
//! catalogue's files for it are the same, or other downloads brought them), or its own catalogue had
//! none of it (downloaded before it was built); or when it has
//! nothing whole to keep: not all here at its own catalogue (still copying when the new one came),
//! whose older files the map never reads (they'd mix with the NAS's, `older`).
//!
//! **Its own list** (docs/macs.md, Mirror). Each download keeps what it is at its catalogue: its
//! files' content names (each names its logical name) and sizes, and its pieces, in
//! `downloads.json`'s `lists`, by catalogue number (`seq`). The lists of the catalogue served are
//! written as the mirror plans, for every download (`Downloads::keep_lists`), and those of each
//! catalogue a download is at or updating to are kept, however old: what it keeps, what changed and
//! what an Update copies are its list against the catalogue served, one pass over its own items,
//! however far behind it is. No catalogue is kept on this Mac: a
//! download whose list wasn't kept (a file of format 4, from before lists) reads at the map served,
//! as one whose catalogue is lost (`Plan::eff`). A region behind whose list lacks some file of no z6
//! tile it now holds (one listed before a region held what's read over it zoomed out) takes it at
//! the served catalogue, at the mirror's next round with the NAS (`unlisted`): no Update, and the
//! rest stays at its own catalogue. What a download holds here in the form it had then (a whole
//! pack, the basemap's zooms 0–10) stays, and is what's read, until all that replaces it is here
//! (`bridge`): cut off at any point, away from the NAS it's one form or the other, whole.
//!
//! **Whole tiles.** No z6 tile is read from two catalogues (a list's ways index their owner's base
//! pack). A download at the served catalogue moves every tile it has files of there (`Plan::moved`):
//! a download behind keeps only its other tiles at its own catalogue, and takes its files of those at
//! the served one (`Plan::own`). An Update copies, besides its own files, every other download's of
//! its tiles (`Plan::target`), so a shared tile moves whole. A road from a tile kept behind into a
//! moved one may lack its info at the crossing until that download is updated too (not found, never
//! wrong). The World moves no tile and is moved by none (`Which::tiled`): its per-tile files index
//! no base pack.
//!
//! - **The World, zoomed out** (`world_part`): the worldwide files, every layer's root and lo
//!   packs, the landmark points and area details (store::mirror::essentials), and the basemap's
//!   zooms 0–10 (its `lo` piece, store::pieces): away from the NAS, the whole world to zoom 8 (the
//!   basemap to 10), and the place search's places that show by zoom 8, everywhere. It's downloaded
//!   on its own: a region doesn't bring it.
//! - **A region** (`area_part`): what the map reads over it at every zoom, so away from the NAS
//!   every feature works inside it without the World. Every layer's hi pack (zooms 9–14), the base
//!   pack and road values, the hi data, landmark points and area details, of each z6 tile within
//!   2 km of it (its outlines are simplified), and the base packs and road values of the owners of the ways those tiles' hidata list (`Downloads::learn_owners`), and
//!   the basemap's zooms 11–14 over those tiles; zoomed out, its pieces over those tiles of every
//!   layer's root and lo packs (what a terrain or shading tile without its own is made from, and
//!   the labels the place search reads there) and of the basemap's zooms 0–10 (store::pieces;
//!   inputs::areafiles::pieces_of); and what the map reads of the World's files
//!   (inputs::areafiles::files_of). A tile of zooms 0–5 reaches past its z6 tiles: away from
//!   the NAS one is cut to them as it's served (Data::reaches and Data::cut, set each round:
//!   `reach`).
//! - **A region's terrain detail** (`detail_of`), an add-on to its download: the terrain's deep
//!   packs (terrain::terrain_deep, z13 and finer, by z10 tile) within 2 km of it, and their
//!   shading's (the slope layer's deep packs: terrain::slope_pack). They cover only parts of a
//!   region; its terrain to z12 (the hi packs) is the rest and every zoom to z12, so it needs the
//!   region: downloaded only while the region is. Copied after every other download.

use crate::data::{Data, Reach};
use crate::S;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use inputs::areafiles::{box_meets, detail_of, files_of, outer_rings, pieces_of, tiles_meeting, Files, NEAR_KM};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::catalogue::Catalogue;
use store::mirror::{Control, Item, Mirror, SyncEnd, Wanted};
use store::pieces::Piece;

const FILE: &str = "downloads.json";
/// `downloads.json`'s format: 5 since each download keeps its own list (`lists`); 4 since downloads
/// update only when asked (each its `Pin`); 3 had `world` as a time and `detail` as a flag.
const FMT: u32 = 5;

/// The catalogue a download is at, and the one an Update is taking it to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// The number of the catalogue whose files it keeps (0: none yet: the mirror's next round
    /// pins it to the one served).
    #[serde(default)]
    pub cat: u64,
    /// An Update under way: the catalogue whose files it's copying; it moves there once they're all
    /// here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
}

/// What this Mac has downloaded (`downloads.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub fmt: u32,
    /// The World, zoomed out (None: it isn't downloaded).
    #[serde(default)]
    pub world: Option<AskedWorld>,
    /// Downloaded regions, by id, as they were named then (for one the map no longer has).
    #[serde(default)]
    pub regions: Vec<AskedRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskedWorld {
    /// When it was downloaded (seconds since 1970).
    pub at: u64,
    #[serde(flatten)]
    pub pin: Pin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskedRegion {
    pub id: String,
    pub name: String,
    /// When it was downloaded (seconds since 1970).
    pub at: u64,
    #[serde(flatten)]
    pub pin: Pin,
    /// Its terrain detail, when downloaded too (`detail_of`): its own catalogue.
    #[serde(default)]
    pub detail: Option<Pin>,
}

/// A download: the World, a region, or a region's terrain detail.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Which {
    World,
    Region(String),
    Detail(String),
}

impl Which {
    /// Whether its z6 tiles move whole with other downloads' (module doc, "Whole tiles"): not the
    /// World's, whose per-tile files (roads' English names, road ids) index no base pack: updating
    /// it copies none of a region's.
    fn tiled(&self) -> bool {
        !matches!(self, Which::World)
    }
}

impl Asked {
    /// Every download and its pin, in copy order: the World, the regions as they were asked, then
    /// their terrain detail.
    fn pins(&self) -> Vec<(Which, Pin)> {
        let mut v: Vec<(Which, Pin)> = self.world.iter().map(|w| (Which::World, w.pin)).collect();
        v.extend(self.regions.iter().map(|r| (Which::Region(r.id.clone()), r.pin)));
        v.extend(self.regions.iter().filter_map(|r| Some((Which::Detail(r.id.clone()), r.detail?))));
        v
    }

    fn pin_mut(&mut self, w: &Which) -> Option<&mut Pin> {
        match w {
            Which::World => self.world.as_mut().map(|w| &mut w.pin),
            Which::Region(id) => self.regions.iter_mut().find(|r| r.id == *id).map(|r| &mut r.pin),
            Which::Detail(id) => self.regions.iter_mut().find(|r| r.id == *id).and_then(|r| r.detail.as_mut()),
        }
    }
}

/// A downloads file of an earlier format, as format 4: `world` a time → {at}; a region's `detail`
/// (or `fine`, before one terrain) a flag → {} or nothing; the views of format 2 dropped. Each
/// download pinned to the catalogue served at the mirror's next round.
fn upgrade(mut v: Value) -> Value {
    if let Some(at) = v["world"].as_u64() {
        v["world"] = json!({ "at": at });
    }
    if let Some(rs) = v["regions"].as_array_mut() {
        for r in rs.iter_mut().filter_map(Value::as_object_mut) {
            let on = r.remove("fine").or_else(|| r.remove("detail")).and_then(|d| d.as_bool()).unwrap_or(false);
            r.insert("detail".into(), if on { json!({}) } else { Value::Null });
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("views");
    }
    v
}

/// What a download copies: files of a catalogue and pieces of its basemap's archives and of its
/// layers' packs (store::pieces), in copy order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub items: Vec<Item>,
}

impl Part {
    /// The files (`names`, content names; put in copy order), then the pieces (of the files whose
    /// content names they give).
    fn new(cat: &Catalogue, files: Files, pieces: impl IntoIterator<Item = (String, Piece)>) -> Part {
        let by_content: HashMap<&str, &str> = cat.files.iter().map(|(l, f)| (f.file.as_str(), l.as_str())).collect();
        let mut logicals: Vec<&str> =
            files.names.iter().filter_map(|(n, _)| by_content.get(n.as_str()).copied()).collect();
        store::mirror::copy_order(cat, &mut logicals);
        let mut items: Vec<Item> = logicals
            .iter()
            .filter_map(|l| cat.files.get(*l))
            .map(|f| Item::File { name: f.file.clone(), size: f.size })
            .collect();
        items.extend(pieces.into_iter().map(|(archive, piece)| Item::Piece { archive, piece }));
        Part { items }
    }

    fn pieces(&self) -> impl Iterator<Item = (&str, Piece)> {
        self.items.iter().filter_map(|i| match i {
            Item::Piece { archive, piece } => Some((archive.as_str(), *piece)),
            Item::File { .. } => None,
        })
    }
}

/// These items' size as far as it's known, how much of them is here, and how many pieces aren't
/// sized yet.
fn tally<'a>(items: impl Iterator<Item = &'a Item>, m: &Mirror) -> (u64, u64, u32) {
    let (mut bytes, mut here, mut unknown) = (0, 0, 0);
    for i in items {
        match m.item_size(i) {
            Some(s) => {
                bytes += s;
                here += m.item_here(i).min(s);
            }
            None => unknown += 1,
        }
    }
    (bytes, here, unknown)
}

/// Whether all these items are here.
fn complete<'a>(items: impl Iterator<Item = &'a Item>, m: &Mirror) -> bool {
    let (bytes, here, unknown) = tally(items, m);
    unknown == 0 && here >= bytes
}

/// What a region's downloads copy: the region, and its terrain detail (an add-on), with the share
/// of the region the detail covers (per cent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionParts {
    pub coarse: Part,
    pub detail: Part,
    pub cover: f64,
}

/// By (region id, a z6 tile's hidata content name): the owner units (tile keys) of the ways it
/// lists (`here`) whose boxes meet the region's outline grown by `NEAR_KM`.
type Owners = HashMap<(String, String), Arc<BTreeSet<u64>>>;

/// Every region a catalogue records, what downloading it copies, by id.
type Regions = BTreeMap<String, RegionParts>;

/// What downloads of one catalogue copy: made from the catalogue (`cat`), the World and every region
/// it records; or kept (`Downloads::keep_lists`), the World when downloaded and the regions
/// downloaded.
pub struct At {
    pub cat: Option<Arc<Catalogue>>,
    pub world: Option<Part>,
    pub regions: Regions,
    /// A list kept while some of its ways' owners weren't known (their hidata unread): it may lack
    /// a base pack, so it's made again from its map while this server holds it, and while it's
    /// read no base pack goes (`once`).
    pub partial: bool,
}

impl At {
    /// What the downloads of `asked` keep of it (`cat` none).
    fn listed(&self, asked: &Asked, partial: bool) -> At {
        let ids: HashSet<&str> = asked.regions.iter().map(|r| r.id.as_str()).collect();
        At {
            cat: None,
            partial,
            world: self.world.clone().filter(|_| asked.world.is_some()),
            regions: self
                .regions
                .iter()
                .filter(|(id, _)| ids.contains(id.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// The same lists.
    fn same(&self, o: &At) -> bool {
        self.world == o.world && self.regions == o.regions && self.partial == o.partial
    }
}

/// An item of a download's list as `downloads.json` keeps it: a file (its content name, which names
/// its logical name, and size), or a piece of a basemap archive (the archive's content name).
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Listed {
    File { file: String, size: u64 },
    Piece { archive: String, piece: String },
}

#[derive(Serialize, Deserialize)]
struct ListedRegion {
    coarse: Vec<Listed>,
    detail: Vec<Listed>,
    cover: f64,
}

/// The downloads' lists at one catalogue, as `downloads.json` keeps them.
#[derive(Serialize, Deserialize)]
struct ListedAt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    world: Option<Vec<Listed>>,
    #[serde(default)]
    regions: BTreeMap<String, ListedRegion>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    partial: bool,
}

fn to_listed(p: &Part) -> Vec<Listed> {
    p.items
        .iter()
        .map(|i| match i {
            Item::File { name, size } => Listed::File { file: name.clone(), size: *size },
            Item::Piece { archive, piece } => Listed::Piece { archive: archive.clone(), piece: piece.name() },
        })
        .collect()
}

fn from_listed(v: Vec<Listed>) -> Result<Part> {
    let items = v
        .into_iter()
        .map(|l| match l {
            Listed::File { file, size } => Ok(Item::File { name: file, size }),
            Listed::Piece { archive, piece } => match Piece::parse(&piece) {
                Some(piece) => Ok(Item::Piece { archive, piece }),
                None => bail!("{archive}: no piece {piece}"),
            },
        })
        .collect::<Result<_>>()?;
    Ok(Part { items })
}

impl ListedAt {
    fn of(a: &At) -> ListedAt {
        ListedAt {
            partial: a.partial,
            world: a.world.as_ref().map(to_listed),
            regions: a
                .regions
                .iter()
                .map(|(id, r)| {
                    (
                        id.clone(),
                        ListedRegion { coarse: to_listed(&r.coarse), detail: to_listed(&r.detail), cover: r.cover },
                    )
                })
                .collect(),
        }
    }

    fn at(self) -> Result<At> {
        let regions = self
            .regions
            .into_iter()
            .map(|(id, r)| {
                Ok((id, RegionParts { coarse: from_listed(r.coarse)?, detail: from_listed(r.detail)?, cover: r.cover }))
            })
            .collect::<Result<_>>()?;
        Ok(At { cat: None, partial: self.partial, world: self.world.map(from_listed).transpose()?, regions })
    }
}

/// The lists kept, by catalogue number.
type Lists = BTreeMap<u64, Arc<At>>;

/// What the mirror copies for the catalogues served and kept, and one set of downloads.
pub struct Plan {
    /// The catalogue generation and the downloads' version it was made for.
    generation: u64,
    version: u64,
    /// What's downloaded, in copy order: the World, then the regions as they were asked, then their
    /// terrain detail; each at its catalogue, then (updating) at the one it's updating to.
    pub wanted: Wanted,
    /// The catalogue served's number.
    pub current: u64,
    /// By catalogue number: the one served, and each a download is at or updating to that this Mac
    /// has.
    pub at: BTreeMap<u64, Arc<At>>,
    pub asked: Asked,
    /// The z6 tiles a download at the catalogue served has files of: a download behind it takes
    /// those tiles' files at the served one too (`own`), so no tile mixes catalogues.
    moved: HashSet<(u32, u32)>,
    /// By download, what it held here in a form it no longer has, kept while what replaces it isn't
    /// all here (`bridge`).
    bridges: HashMap<Which, Vec<Item>>,
    /// With bridges, what was here when it was made (Mirror::usage): it's made again once that
    /// changes, so they go as soon as what replaces them is here, whatever else changed.
    here: Option<(usize, u64)>,
}

impl Plan {
    /// The catalogue a pin's `n` reads: its own when this Mac has it, else the one served (none
    /// pinned yet, or its catalogue lost).
    fn eff(&self, n: u64) -> u64 {
        if self.at.contains_key(&n) {
            n
        } else {
            self.current
        }
    }

    /// What `w` copies at catalogue `n` (None: that catalogue doesn't have it).
    fn part(&self, w: &Which, n: u64) -> Option<&Part> {
        let at = self.at.get(&self.eff(n))?;
        match w {
            Which::World => at.world.as_ref(),
            Which::Region(id) => at.regions.get(id).map(|p| &p.coarse),
            Which::Detail(id) => at.regions.get(id).map(|p| &p.detail),
        }
    }

    /// What `w`, pinned at `pin`, keeps: at its own catalogue, its files there of tiles no download
    /// at the served one has (`kept`, with every item of no tile, a region's that its part at the
    /// served one still names), and at the served one (`taken`)
    /// its files of the tiles one has (moved, with that download's, by its download or Update)
    /// and, a region's, the items of no tile its list doesn't name at all (`unlisted`).
    fn own(&self, w: &Which, pin: Pin) -> Option<Own<'_>> {
        let c = self.eff(pin.cat);
        let old = self.part(w, c)?;
        let bridge = self.bridges.get(w).map_or(Vec::new(), |b| b.iter().collect());
        if c == self.current || !w.tiled() {
            return Some(Own { kept: old.items.iter().collect(), taken: Vec::new(), bridge });
        }
        let moved = |i: &&Item| tile_of(i).is_some_and(|t| self.moved.contains(&t));
        let now = self.part(w, self.current);
        // (A region's items of no tile are what its part at the served catalogue names: one its
        // list has that the region no longer holds goes, as a whole lo pack or the basemap's
        // worldwide zooms 0–10 where it now holds their pieces over it, `unlisted`.)
        let named: Option<HashSet<String>> = match (w, now) {
            (Which::Region(_), Some(now)) => Some(now.items.iter().map(logical_of).collect()),
            _ => None,
        };
        let held = |i: &&Item| tile_of(i).is_some() || named.as_ref().is_none_or(|n| n.contains(&logical_of(i)));
        let kept = old.items.iter().filter(|i| !moved(i) && held(i)).collect();
        let mut taken: Vec<&Item> = now.map_or(Vec::new(), |p| p.items.iter().filter(moved).collect());
        if let (Which::Region(_), Some(now)) = (w, now) {
            taken.extend(unlisted(old, now));
        }
        Some(Own { kept, taken, bridge })
    }

    /// What an Update of `w` to catalogue `n` copies: its files there, and every other download's
    /// files there of the same z6 tiles (a tile moves whole: module doc).
    fn target(&self, w: &Which, n: u64) -> Option<Vec<&Item>> {
        let part = self.part(w, n)?;
        let mut out: Vec<&Item> = part.items.iter().collect();
        if !w.tiled() {
            return Some(out);
        }
        let tiles: HashSet<(u32, u32)> = part.items.iter().filter_map(tile_of).collect();
        let mut seen: HashSet<&Item> = part.items.iter().collect();
        for (o, _) in self.asked.pins().iter().filter(|(o, _)| o != w && o.tiled()) {
            for i in self.part(o, n).into_iter().flat_map(|p| p.items.iter()) {
                if tile_of(i).is_some_and(|t| tiles.contains(&t)) && seen.insert(i) {
                    out.push(i);
                }
            }
        }
        Some(out)
    }

    /// The oldest map whose files the mirror still copies from the NAS (docs/macs.md, Mirror): the
    /// one served, an Update's older one while its files aren't all here, or a download's own while
    /// what it keeps there isn't (`Downloads::advance` moves that on). The mirror's cursor in the
    /// map's log (crate::maplog::MirrorLink): never newer than what it lacks.
    pub fn copies_from(&self, m: &Mirror) -> u64 {
        let mut n = self.current;
        for (w, p) in self.asked.pins() {
            if let Some(t) = p.to.map(|t| self.eff(t)) {
                if self.target(&w, t).is_some_and(|items| !complete(items.into_iter(), m)) {
                    n = n.min(t);
                }
            }
            let c = self.eff(p.cat);
            if c != self.current && self.own(&w, p).is_some_and(|o| !o.kept.iter().all(|i| m.is_here(i))) {
                n = n.min(c);
            }
        }
        n
    }

    /// The catalogue served's.
    pub fn now(&self) -> &At {
        &self.at[&self.current]
    }

    /// The catalogue served.
    fn served(&self) -> &Catalogue {
        self.now().cat.as_deref().expect("the served catalogue's is made from it")
    }
}

/// What a download keeps (`Plan::own`).
struct Own<'a> {
    kept: Vec<&'a Item>,
    taken: Vec<&'a Item>,
    /// What it held in a form it no longer has, kept until what replaces it is all here
    /// (`bridge`): not its own (`all`), but wanted, and read while it's kept.
    bridge: Vec<&'a Item>,
}

impl<'a> Own<'a> {
    /// What it is now: at its catalogue, and at the served one.
    fn all(&self) -> impl Iterator<Item = &'a Item> + '_ {
        self.kept.iter().chain(&self.taken).copied()
    }
}

/// What download `w` (`own`, without a bridge) held here in a form it no longer has, while what
/// replaces it isn't all here: a whole pack or the basemap's zooms 0–10 (`Piece::Lo`) where it now
/// takes their pieces (a region downloaded before it held pieces, its list naming them whole, or
/// this Mac's files from then). Of what's here (`here`), its list's items and the served
/// catalogue's whole files of its pieces, those holding a piece of its that isn't here, all of
/// them until every such piece is: so away from the NAS it reads its old form whole until the new
/// one is, then the new one, and the old goes in one step (Mirror::let_go), never half of each.
fn bridge(plan: &Plan, w: &Which, pin: Pin, own: &Own, here: &dyn Fn(&Item) -> bool) -> Vec<Item> {
    let logical = |n: &str| store::naming::parse_content_name(n).map(|c| c.logical.to_string());
    // Its pieces (but the World's zooms 0–10), by their file's logical name: each, and whether here.
    let mut pieces: HashMap<String, Vec<(Piece, bool)>> = HashMap::new();
    let mut archives: BTreeSet<&str> = BTreeSet::new();
    for i in own.all() {
        if let Some((a, p)) = piece_of(i).filter(|(_, p)| *p != Piece::Lo) {
            let Some(l) = logical(a) else { continue };
            pieces.entry(l).or_default().push((p, here(i)));
            archives.insert(a);
        }
    }
    if pieces.is_empty() {
        return Vec::new();
    }
    let ours: HashSet<&Item> = own.all().collect();
    let sizes: HashMap<&str, u64> = plan.served().files.values().map(|f| (f.file.as_str(), f.size)).collect();
    let list = plan.part(w, plan.eff(pin.cat)).map_or(&[][..], |p| &p.items[..]);
    let cands = list.iter().cloned().chain(archives.iter().flat_map(|&a| {
        let whole = sizes.get(a).map(|&size| Item::File { name: a.to_string(), size });
        whole.into_iter().chain([Item::Piece { archive: a.to_string(), piece: Piece::Lo }])
    }));
    let (mut old, mut seen, mut waiting) = (Vec::new(), HashSet::new(), false);
    for c in cands {
        // (A whole file holds every piece of its pack; zooms 0–10 the pieces of zooms 0–10.)
        let (name, lo) = match &c {
            Item::File { name, .. } => (name, false),
            Item::Piece { archive, piece: Piece::Lo } => (archive, true),
            Item::Piece { .. } => continue,
        };
        let Some(held) = logical(name).and_then(|l| pieces.get(&l)) else { continue };
        let held = held.iter().filter(|(p, _)| !lo || matches!(p, Piece::Over(..) | Piece::Mid(..)));
        let mut held = held.peekable();
        if held.peek().is_none() || ours.contains(&c) || seen.contains(&c) || !here(&c) {
            continue;
        }
        waiting |= held.any(|(_, h)| !h);
        seen.insert(c.clone());
        old.push(c);
    }
    if waiting {
        old
    } else {
        Vec::new()
    }
}

/// The items of no z6 tile in `now` (a region's part at the catalogue served) whose logical names
/// `old` (its list at its own) doesn't have: what a region holds that its list, made before it did,
/// lacks (one made before a region held what's read over it zoomed out: the roots and lo packs, the
/// basemap's zooms 0–10). A download behind takes them at the served catalogue, as copies of what it
/// never had, at its next round with the NAS: no Update, and what it keeps stays as it is.
fn unlisted<'a>(old: &Part, now: &'a Part) -> impl Iterator<Item = &'a Item> {
    let named: HashSet<String> = old.items.iter().map(logical_of).collect();
    now.items.iter().filter(move |i| tile_of(i).is_none() && !named.contains(&logical_of(i)))
}

/// What an item is of in any catalogue: a file's logical name, a basemap piece's archive's and its
/// own.
fn logical_of(i: &Item) -> String {
    let logical =
        |n: &str| store::naming::parse_content_name(n).map_or_else(|| n.to_string(), |c| c.logical.to_string());
    match i {
        Item::File { name, .. } => logical(name),
        Item::Piece { archive, piece } => format!("{}#{}", logical(archive), piece.name()),
    }
}

/// The z6 tile a file or basemap piece is of (its logical name ending in "6-x-y"), if any.
fn tile_of(i: &Item) -> Option<(u32, u32)> {
    match i {
        Item::Piece { piece: Piece::Tile(x, y), .. } => Some((*x, *y)),
        Item::Piece { .. } => None,
        Item::File { name, .. } => {
            let logical = store::naming::parse_content_name(name)?.logical;
            let mut t = logical.rsplit('/').next()?.split('-');
            match (t.next(), t.next(), t.next(), t.next()) {
                (Some("6"), Some(x), Some(y), None) => Some((x.parse().ok()?, y.parse().ok()?)),
                _ => None,
            }
        }
    }
}

/// The downloads: what's asked, the plan made of it, and the mirror thread's alarm.
pub struct Downloads {
    path: std::path::PathBuf,
    asked: Mutex<Asked>,
    /// Each download's list at each catalogue one is at or updating to, and at the one served
    /// (`keep_lists`); written with `asked`, locked after it.
    lists: Mutex<Lists>,
    /// Bumped at every change of what's asked, and when owners are learned.
    version: AtomicU64,
    plan: Mutex<Option<Arc<Plan>>>,
    /// What each catalogue's downloads copy, by its number, with the owners' change it was made
    /// at (they don't change with the downloads otherwise).
    ats: Mutex<HashMap<u64, (u64, Arc<At>)>>,
    /// The downloaded regions' hidata's owners, as far as they're read, and a count of their
    /// changes (what `ats` is keyed on: their number alone stays when one region's swap for
    /// another's).
    owners: Mutex<(u64, Arc<Owners>)>,
    /// Set when the downloads change, to wake the mirror thread.
    wake: (Mutex<bool>, Condvar),
}

impl Downloads {
    /// The downloads in `home` (none yet: none). A file that doesn't read is set aside as
    /// `downloads.json.bad`. One of an earlier format is written again as this one (`upgrade`;
    /// format 4's pins kept, its lists made by the mirror's next rounds: `keep_lists`).
    pub fn load(home: &std::path::Path) -> Arc<Downloads> {
        let path = home.join(FILE);
        let read = |b: &[u8]| -> Result<(Asked, Lists, bool)> {
            let mut v: Value = serde_json::from_slice(b)?;
            let fmt = v["fmt"].as_u64().unwrap_or(0);
            let listed: BTreeMap<u64, ListedAt> = match v.as_object_mut().and_then(|o| o.remove("lists")) {
                Some(l) => serde_json::from_value(l)?,
                None => BTreeMap::new(),
            };
            let lists = listed.into_iter().map(|(n, l)| Ok((n, Arc::new(l.at()?)))).collect::<Result<_>>()?;
            let v = if fmt < 4 { upgrade(v) } else { v };
            Ok((serde_json::from_value(v)?, lists, fmt < u64::from(FMT)))
        };
        let (asked, lists, old) = match std::fs::read(&path) {
            Ok(b) => match read(&b) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("downloads: {} doesn't read ({e}); set aside as {FILE}.bad", path.display());
                    let _ = std::fs::rename(&path, home.join(format!("{FILE}.bad")));
                    (Asked::default(), Lists::new(), false)
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("downloads: can't read {}: {e}", path.display());
                }
                (Asked::default(), Lists::new(), false)
            }
        };
        let d = Arc::new(Downloads {
            path,
            asked: Mutex::new(asked.clone()),
            lists: Mutex::new(lists),
            version: AtomicU64::new(1),
            plan: Mutex::new(None),
            ats: Default::default(),
            owners: Default::default(),
            wake: (Mutex::new(false), Condvar::new()),
        });
        if old {
            if let Err(e) = d.edit(|_| Ok(())) {
                eprintln!("downloads: {e:#}");
            }
        }
        d
    }

    pub fn asked(&self) -> Asked {
        self.asked.lock().unwrap().clone()
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Changes what's asked with `f` and writes it out (through a temporary file); the mirror
    /// thread is woken. Nothing changes when `f` fails or the file can't be written.
    fn edit<T>(&self, f: impl FnOnce(&mut Asked) -> Result<T>) -> Result<T> {
        let out = self.write(f, None)?;
        self.version.fetch_add(1, Ordering::SeqCst);
        self.poke();
        Ok(out)
    }

    /// Writes what's asked, changed by `f`, and the lists (`lists`, else those kept), through a
    /// temporary file; nothing changes when `f` fails or the file can't be written.
    fn write<T>(&self, f: impl FnOnce(&mut Asked) -> Result<T>, lists: Option<Lists>) -> Result<T> {
        let mut g = self.asked.lock().unwrap();
        let mut l = self.lists.lock().unwrap();
        let mut next = g.clone();
        let out = f(&mut next)?;
        next.fmt = FMT;
        let lists = lists.unwrap_or_else(|| l.clone());
        let mut v = serde_json::to_value(&next)?;
        let listed: BTreeMap<u64, ListedAt> = lists.iter().map(|(n, a)| (*n, ListedAt::of(a))).collect();
        v["lists"] = serde_json::to_value(listed)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&v)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("rename {}", tmp.display()))?;
        *g = next;
        *l = lists;
        Ok(out)
    }

    /// Keeps each download's list at the catalogue served and at each one a download is at or
    /// updating to (those it has; the others go). The served one's are made again at each plan
    /// while it's served (a download asked, owners learned: their base packs), marked `partial`
    /// while some owners aren't known, and made again from its map once they are, while this server
    /// holds it (`Plan::at`).
    fn keep_lists(&self, plan: &Plan, owners_known: bool) {
        let held = self.lists.lock().unwrap().clone();
        let pinned: BTreeSet<u64> =
            plan.asked.pins().iter().flat_map(|(_, p)| [Some(p.cat), p.to]).flatten().filter(|&n| n > 0).collect();
        let mut next = Lists::new();
        for &n in pinned.iter().chain([&plan.current]) {
            let made = plan.at.get(&n).filter(|a| a.cat.is_some());
            let remake = n == plan.current || owners_known;
            match (made, held.get(&n)) {
                (Some(a), h) if remake => {
                    let a = a.listed(&plan.asked, !owners_known);
                    next.insert(n, h.filter(|h| h.same(&a)).cloned().unwrap_or_else(|| Arc::new(a)));
                }
                (_, Some(h)) => {
                    next.insert(n, h.clone());
                }
                _ => {}
            }
        }
        let same = next.len() == held.len() && next.iter().all(|(n, a)| held.get(n).is_some_and(|h| Arc::ptr_eq(a, h)));
        if !same {
            if let Err(e) = self.write(|_| Ok(()), Some(next)) {
                eprintln!("downloads: keeping their lists: {e:#}");
            }
        }
    }

    /// Wakes the mirror thread for another round (the reserve changed).
    pub fn poke(&self) {
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Waits until the downloads change, or `timeout`.
    fn wait(&self, timeout: Duration) {
        let (lock, cv) = &self.wake;
        let g = lock.lock().unwrap();
        let (mut g, _) = cv.wait_timeout_while(g, timeout, |woken| !*woken).unwrap();
        *g = false;
    }

    /// The plan for the catalogue being served and the downloads as they are; the map told which
    /// older files the downloads behind it keep (Data::set_older).
    pub fn plan(&self, data: &Data) -> Arc<Plan> {
        let generation = data.generation.load(Ordering::SeqCst);
        let version = self.version();
        let usage = data.mirror.as_ref().map(|m| m.usage());
        if let Some(p) = self.plan.lock().unwrap().as_ref() {
            if p.generation == generation && p.version == version && p.here.is_none_or(|h| Some(h) == usage) {
                return p.clone();
            }
        }
        let cur = data.catalogue();
        let asked = self.asked();
        let (changes, owners) = self.owners.lock().unwrap().clone();
        let mut ns: BTreeSet<u64> = asked.pins().iter().flat_map(|(_, p)| [Some(p.cat), p.to]).flatten().collect();
        ns.insert(cur.n);
        let lists = self.lists.lock().unwrap().clone();
        let at: BTreeMap<u64, Arc<At>> = {
            let mut g = self.ats.lock().unwrap();
            g.retain(|n, _| ns.contains(n) && (*n == cur.n || lists.get(n).is_none_or(|a| a.partial)));
            ns.iter()
                .filter_map(|&n| {
                    // (Each download's own lists, but at the map served; else what this server made
                    // of a map while it served it: one the map moved past before the mirror's round
                    // listed it, a partial list made whole.)
                    if n != cur.n && !g.contains_key(&n) {
                        if let Some(a) = lists.get(&n) {
                            return Some((n, a.clone()));
                        }
                    }
                    if let Some((c, a)) = g.get(&n) {
                        // (The served catalogue's again when it was swapped under the same number:
                        // a test's, or a fresh NAS.)
                        if *c == changes && (n != cur.n || a.cat.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cur))) {
                            return Some((n, a.clone()));
                        }
                    }
                    if n != cur.n {
                        return lists.get(&n).map(|a| (n, a.clone()));
                    }
                    let a = Arc::new(at_of(cur.clone(), &owners));
                    g.insert(n, (changes, a.clone()));
                    Some((n, a))
                })
                .collect()
        };
        let here = |i: &Item| data.mirror.as_ref().is_some_and(|m| m.is_here(i));
        let mut plan = make_plan(generation, version, asked, cur.n, at, &here);
        if !plan.bridges.is_empty() {
            plan.here = usage;
        }
        // (A change moves the generation on: the plan is for the one it leaves.)
        let older = data.mirror.as_ref().map(|m| older(&plan, m)).unwrap_or_default();
        if data.set_older(older) {
            plan.generation = data.generation.load(Ordering::SeqCst);
        }
        let plan = Arc::new(plan);
        *self.plan.lock().unwrap() = Some(plan.clone());
        plan
    }
}

/// Where the downloads of `asked` reach (Data::reach): everywhere with the World, else the z6 tiles
/// of the regions downloaded as `cat` records them (their pieces', `area_part`), and the polar
/// caps' blocks they meet (their polar blocks', inputs::areafiles). Only those regions' outlines
/// are read: known at once, not after a plan of every region.
fn reach(asked: &Asked, cat: &Catalogue) -> Reach {
    if asked.world.is_some() {
        return Reach::World;
    }
    let ids: HashSet<&str> = asked.regions.iter().map(|r| r.id.as_str()).collect();
    // (An object's id, read from its text: the regions asked are parsed whole.)
    #[derive(serde::Deserialize)]
    struct Id {
        #[serde(default)]
        id: Value,
    }
    let recorded = cat.coverage.region_texts();
    let rings: Vec<Vec<Vec<[f64; 2]>>> = recorded
        .iter()
        .filter(|r| r.starts_with('{'))
        .filter(|r| serde_json::from_str::<Id>(r).is_ok_and(|r| r.id.as_str().is_some_and(|id| ids.contains(id))))
        .filter_map(|r| serde_json::from_str::<buildstore::coverage::DrawnRegion>(r).ok())
        .map(|r| outer_rings(&r))
        .collect();
    let tiles = rings.iter().flat_map(|r| tiles_meeting(r, NEAR_KM));
    let blocks = rings.iter().flat_map(|r| {
        [roadcore::polar::Pole::North, roadcore::polar::Pole::South].into_iter().flat_map(move |p| {
            inputs::areafiles::polar_blocks_meeting(r, p, NEAR_KM).into_iter().map(move |(x, y)| (p.letter(), x, y))
        })
    });
    Reach::Tiles(Arc::new(crate::data::Footprint::of(tiles, blocks)))
}

/// What downloads of `cat` copy, with the owners learned.
fn at_of(cat: Arc<Catalogue>, owners: &Owners) -> At {
    let regions = crate::regions::recorded(&cat)
        .iter()
        .map(|r| (r.id.clone(), region_parts(&cat, &r.id, &outer_rings(r), owners)))
        .collect();
    At { world: Some(world_part(&cat)), regions, cat: Some(cat), partial: false }
}

/// The older files the downloads behind the catalogue served keep, by logical name, for the map to
/// read (Data::content): those each keeps of its own catalogue (`Plan::own`: none of a tile a
/// download at the served one has) that the served one replaced, unless a download at the served
/// catalogue has its file (a file of no tile shared by both: the newer). Only a download with all it
/// keeps here counts: one partly here would mix its catalogue's files with the served one's (a list's
/// ways index their owner's base pack), and what it reads would change as each file came, under the
/// same URLs (`advance` moves it on).
fn older(plan: &Plan, m: &Mirror) -> HashMap<String, String> {
    let current: HashSet<String> = plan
        .asked
        .pins()
        .iter()
        .filter(|(_, p)| plan.eff(p.cat) == plan.current)
        .filter_map(|(w, p)| plan.part(w, p.cat))
        .flat_map(|p| p.items.iter().map(store::mirror::copy_name))
        .collect();
    let cat = plan.served();
    let mut out = HashMap::new();
    for (w, p) in plan.asked.pins() {
        if plan.eff(p.cat) == plan.current {
            continue;
        }
        let Some(own) = plan.own(&w, p).filter(|o| o.kept.iter().all(|i| m.is_here(i))) else {
            continue;
        };
        for i in own.kept.into_iter().chain(own.bridge) {
            let name = match i {
                Item::File { name, .. } => name,
                Item::Piece { archive, .. } => archive,
            };
            let Some(logical) = store::naming::parse_content_name(name).map(|c| c.logical.to_string()) else {
                continue;
            };
            let now = cat.content(&logical);
            if now != Some(name.as_str()) && !now.is_some_and(|c| current.contains(c)) {
                out.insert(logical, name.clone());
            }
        }
    }
    out
}

impl Downloads {
    /// Reads the owners of the ways the downloaded regions' hidata list, where not known yet, at
    /// each catalogue they're at or updating to and the one served: from this Mac's copy, else the
    /// NAS. A region keeps the base packs and road values of the owners of those ways whose boxes
    /// meet its outline grown by `NEAR_KM` (`area_part`), so a way drawn in it that starts far
    /// outside has its info away from the NAS. Whether every one is known: a hidata that can't be
    /// read is tried again next round. A change changes the plan.
    fn learn_owners(&self, data: &Data, plan: &Plan) -> bool {
        let known = self.owners.lock().unwrap().1.clone();
        let mut next = Owners::new();
        let mut complete = true;
        for asked in &plan.asked.regions {
            let ns: BTreeSet<u64> =
                [plan.eff(asked.pin.cat), plan.current].into_iter().chain(asked.pin.to.map(|t| plan.eff(t))).collect();
            for n in ns {
                // (A download's own list needs none: its owners were known when it was made.)
                let Some(cat) = plan.at.get(&n).and_then(|a| a.cat.as_ref()) else { continue };
                let Some(r) = crate::regions::recorded(cat).into_iter().find(|d| d.id == asked.id) else { continue };
                let rings = outer_rings(&r);
                for t in near(cat, &rings) {
                    let Some(content) = cat.hidata.get(&t).and_then(|l| cat.content(l)) else { continue };
                    let key = (asked.id.clone(), content.to_string());
                    if next.contains_key(&key) {
                        continue;
                    }
                    if let Some(o) = known.get(&key) {
                        next.insert(key, o.clone());
                        continue;
                    }
                    match owners_near(data, content, &rings) {
                        Ok(o) => {
                            next.insert(key, Arc::new(o));
                        }
                        Err(e) => {
                            eprintln!("mirror: the owners of hidata {t}'s ways: {e:#}");
                            complete = false;
                        }
                    }
                }
            }
        }
        let same =
            next.len() == known.len() && next.iter().all(|(k, v)| known.get(k).is_some_and(|o| Arc::ptr_eq(o, v)));
        if !same {
            let mut g = self.owners.lock().unwrap();
            *g = (g.0 + 1, Arc::new(next));
            self.version.fetch_add(1, Ordering::SeqCst);
        }
        complete
    }

    /// The lead answered the mirror's cursor a reset (docs/macs.md, Mirror: outside its log, or not
    /// held by GC: new, or unheard or at one map `CURSOR_TTL_S`, 6 h), and the map served is the
    /// head it answered or past it (`once`): each Update of a map older than the one served goes
    /// on at the one served, whose files the NAS has (an older map's may have gone with GC
    /// meanwhile). What a download keeps is here and stays. Whether any moved.
    fn resync(&self, plan: &Plan) -> bool {
        let cur = plan.current;
        let stale = |p: &Pin| p.to.is_some_and(|t| plan.eff(t) < cur);
        if !plan.asked.pins().iter().any(|(_, p)| stale(p)) {
            return false;
        }
        eprintln!("mirror: its cursor is outside the map's log: the Updates go on at map {cur}");
        match self.edit(|a| {
            for (w, p) in a.pins() {
                if stale(&p) {
                    a.pin_mut(&w).expect("a pin listed").to = Some(cur);
                }
            }
            Ok(())
        }) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("downloads: {e:#}");
                false
            }
        }
    }

    /// Moves each download's pin on where the plan says (`Plan::eff`, module doc): pinned at last
    /// (none yet, or its catalogue lost: the one served); an Update all here (`Plan::target`): to its
    /// catalogue; behind, with nothing to copy (all an Update would copy here already), or with
    /// nothing whole to keep (not all it keeps here at its own catalogue): to the one served.
    /// Whether anything moved.
    fn advance(&self, plan: &Plan, m: &Mirror) -> bool {
        let cur = plan.current;
        let mut next = plan.asked.clone();
        for (w, pin) in plan.asked.pins() {
            let mut p = pin;
            p.cat = plan.eff(p.cat);
            if let Some(t) = p.to.map(|t| plan.eff(t)) {
                p.to = Some(t);
                if t == p.cat {
                    p.to = None;
                } else if plan.target(&w, t).is_none_or(|items| complete(items.into_iter(), m)) {
                    p = Pin { cat: t, to: None };
                }
            }
            if p.cat != cur {
                let whole = plan.own(&w, p).is_some_and(|o| o.kept.iter().all(|i| m.is_here(i)));
                if !whole {
                    p = Pin { cat: cur, to: None };
                }
            }
            // (Nothing to copy: its catalogue had none of it, or each file an Update would copy is
            // here, through another download, or one it keeps already (the same file, or one of a
            // tile another download moved, being copied): what the panel says is behind. Not while
            // it holds an older form (`bridge`), read at its own catalogue until the new is here.)
            if p.to.is_none() && p.cat != cur && !plan.bridges.contains_key(&w) {
                let none = plan.part(&w, p.cat).is_none();
                let had: HashSet<&Item> = plan.own(&w, p).iter().flat_map(Own::all).collect();
                let left = |items: Vec<&Item>| items.into_iter().any(|i| !had.contains(i) && !m.is_here(i));
                if none || plan.target(&w, cur).is_some_and(|items| !left(items)) {
                    p.cat = cur;
                }
            }
            if p != pin {
                *next.pin_mut(&w).expect("a pin listed") = p;
            }
        }
        if next == plan.asked {
            return false;
        }
        match self.edit(|a| {
            // (Only the pins move: what was asked meanwhile stays.)
            for (w, p) in next.pins() {
                if let Some(q) = a.pin_mut(&w) {
                    *q = p;
                }
            }
            Ok(())
        }) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("downloads: {e:#}");
                false
            }
        }
    }
}

/// The z6 tiles ("6/x/y") within `NEAR_KM` of `rings` with hidata in `cat`.
fn near(cat: &Catalogue, rings: &[Vec<[f64; 2]>]) -> Vec<String> {
    tiles_meeting(rings, NEAR_KM)
        .into_iter()
        .map(|(x, y)| format!("6/{x}/{y}"))
        .filter(|k| cat.hidata.contains_key(k))
        .collect()
}

/// The owners of the ways a hidata file (`content`) lists whose boxes meet `rings` grown by
/// `NEAR_KM`. An owner kept isn't tested again; the box's own test first.
fn owners_near(data: &Data, content: &str, rings: &[Vec<[f64; 2]>]) -> Result<BTreeSet<u64>> {
    let hv = data.hidata_of(content)?;
    let all = hv.here.all()?;
    let mut out = BTreeSet::new();
    for h in all.cast::<roadcore::records::Here>() {
        if !out.contains(&h.owner) && box_meets(rings, h.bbox.map(|v| f64::from(v) / 1e7), NEAR_KM) {
            out.insert(h.owner);
        }
    }
    Ok(out)
}

/// The owner units a region's download at `cat` keeps, as far as they're known.
fn units_of(owners: &Owners, cat: &Catalogue, region: &str, rings: &[Vec<[f64; 2]>]) -> BTreeSet<u64> {
    near(cat, rings)
        .iter()
        .filter_map(|t| cat.hidata.get(t).and_then(|l| cat.content(l)))
        .filter_map(|c| owners.get(&(region.to_string(), c.to_string())))
        .flat_map(|o| o.iter().copied())
        .collect()
}

/// The plan for `asked` at the catalogues `at` (`current` served), with what's here (`here`, for
/// what a download held in an older form: `bridge`).
fn make_plan(
    generation: u64,
    version: u64,
    asked: Asked,
    current: u64,
    at: BTreeMap<u64, Arc<At>>,
    here: &dyn Fn(&Item) -> bool,
) -> Plan {
    let mut plan = Plan {
        generation,
        version,
        wanted: Wanted::default(),
        current,
        at,
        asked,
        moved: HashSet::new(),
        bridges: HashMap::new(),
        here: None,
    };
    let moved: HashSet<(u32, u32)> = plan
        .asked
        .pins()
        .iter()
        .filter(|(w, p)| w.tiled() && plan.eff(p.cat) == current)
        .filter_map(|(w, _)| plan.part(w, current))
        .flat_map(|p| p.items.iter().filter_map(tile_of))
        .collect();
    plan.moved = moved;
    let bridges: HashMap<Which, Vec<Item>> = plan
        .asked
        .pins()
        .into_iter()
        .filter_map(|(w, p)| Some((w.clone(), bridge(&plan, &w, p, &plan.own(&w, p)?, here))))
        .filter(|(_, b)| !b.is_empty())
        .collect();
    plan.bridges = bridges;
    let mut wanted = Wanted::default();
    let mut seen = HashSet::new();
    for (w, p) in plan.asked.pins() {
        let own = plan.own(&w, p);
        let to = p.to.and_then(|t| plan.target(&w, t));
        let bridge = own.iter().flat_map(|o| o.bridge.iter().copied());
        for i in own.iter().flat_map(Own::all).chain(bridge).chain(to.into_iter().flatten()) {
            wanted.push(i.clone(), &mut seen);
        }
    }
    plan.wanted = wanted;
    plan
}

/// The World, zoomed out: the essentials and the basemap's zooms 0–10.
fn world_part(cat: &Catalogue) -> Part {
    let ess = store::mirror::essentials(cat);
    let files = of_contents(cat, ess.iter().map(String::as_str));
    Part::new(cat, files, basemap_archives(cat).into_iter().map(|a| (a, Piece::Lo)))
}

/// A region's download (`rings`, each read as a filled area): its files, with the base packs and
/// road values of `units` (the owners of its ways, `Downloads::learn_owners`), then its pieces of
/// worldwide files, those over its z6 tiles alone (store::pieces): of the layers' roots and lo packs
/// (inputs::areafiles::pieces_of), and of the basemap at every zoom.
fn area_part(cat: &Catalogue, rings: &[Vec<[f64; 2]>], units: &BTreeSet<u64>) -> Part {
    let tiles = tiles_meeting(rings, NEAR_KM);
    let mut files: BTreeMap<String, u64> = files_of(cat, rings).names.into_iter().collect();
    for &u in units {
        let unit = crate::ways::unit_str(u);
        for l in [cat.base.get(&unit), cat.roads.get(&unit)].into_iter().flatten() {
            if let Some(f) = cat.files.get(l) {
                files.insert(f.file.clone(), f.size);
            }
        }
    }
    let basemap = Piece::over(&tiles, 0..=*store::pieces::HI_ZOOMS.end());
    let pieces = pieces_of(cat, rings)
        .into_iter()
        .chain(basemap_archives(cat).into_iter().flat_map(|a| basemap.iter().map(move |&p| (a.clone(), p))));
    Part::new(cat, Files::from_map(files), pieces)
}

/// A region's downloads (`id`, `rings` its outer rings): the region, and its terrain detail.
fn region_parts(cat: &Catalogue, id: &str, rings: &[Vec<[f64; 2]>], owners: &Owners) -> RegionParts {
    let (files, cover) = detail_of(cat, rings);
    let units = units_of(owners, cat, id, rings);
    RegionParts { coarse: area_part(cat, rings, &units), detail: Part::new(cat, files, []), cover }
}

fn basemap_archives(cat: &Catalogue) -> Vec<String> {
    cat.basemap.iter().filter_map(|l| cat.content(l)).map(str::to_string).collect()
}

/// The catalogue's files of these content names.
fn of_contents<'a>(cat: &Catalogue, contents: impl IntoIterator<Item = &'a str>) -> Files {
    let sizes: std::collections::HashMap<&str, u64> = cat.files.values().map(|f| (f.file.as_str(), f.size)).collect();
    Files::from_map(contents.into_iter().filter_map(|c| sizes.get(c).map(|&s| (c.to_string(), s))).collect())
}

// ---- the mirror thread --------------------------------------------------------------------------

/// Keeps the mirror (store::mirror) on its own thread: every minute, or as soon as the downloads
/// change, it lets go of what no download wants and copies what they do, while the NAS is reachable
/// (from home or not); a new catalogue or a change of the downloads has it plan again at once. A new
/// catalogue copies nothing by itself (module doc). While the build runs (the lead's heartbeat on the
/// NAS says a job runs), copies keep to 20 MB/s; a file this Mac's pack or lo job holds isn't
/// deleted (store::mirror). Between copies it works out the pieces' sizes (for the panel's
/// sizes), those of what's downloaded first. Each round with the NAS it tells the lead its cursor
/// (`lead`, crate::maplog::MirrorLink: none for a server of a `--root`).
pub fn spawn_mirror(s: S, mut lead: Option<crate::maplog::MirrorLink>) {
    let Some(m) = s.data.mirror.clone() else { return };
    let spawned = std::thread::Builder::new().name("mirror".into()).spawn(move || loop {
        if !once(&s, &m, lead.as_mut()) {
            // (No map yet: its first round as soon as one comes, so where the downloads reach,
            // what's read away from the NAS, is known at once: Data::reach.)
            s.downloads.wait(Duration::from_secs(if s.data.catalogue().n == 0 { 1 } else { 60 }));
        }
    });
    if let Err(e) = spawned {
        eprintln!("mirror: can't start its thread: {e}");
    }
}

/// A round's start, at catalogue `cat`: where the downloads reach, the owners of their ways learned,
/// their lists kept, their pins moved on (None: one moved, plan again); the plan, and what the round
/// keeps here.
fn round_plan(s: &crate::AppState, m: &Mirror, cat: &Catalogue) -> Option<(Arc<Plan>, Wanted)> {
    s.data.set_reach(reach(&s.downloads.asked(), cat));
    let owners_known = s.downloads.learn_owners(&s.data, &s.downloads.plan(&s.data));
    s.downloads.keep_lists(&s.downloads.plan(&s.data), owners_known);
    // (A partial list read, its map gone: its owners are as unknown.)
    let owners_known = owners_known && s.downloads.plan(&s.data).at.values().all(|a| !a.partial);
    if s.downloads.advance(&s.downloads.plan(&s.data), m) {
        return None;
    }
    let plan = s.downloads.plan(&s.data);
    // While a downloaded region's hidata is unread, its owners can't be told: every base pack and
    // road values here stay (a NAS hiccup mustn't let them go).
    let mut wanted = plan.wanted.clone();
    if !owners_known {
        let mut seen: HashSet<Item> = wanted.items.iter().cloned().collect();
        for c in plan.at.values().filter_map(|a| a.cat.as_ref()) {
            for f in c.base.values().chain(c.roads.values()).filter_map(|l| c.files.get(l)) {
                if m.has(&f.file) {
                    wanted.push(Item::File { name: f.file.clone(), size: f.size }, &mut seen);
                }
            }
        }
    }
    Some((plan, wanted))
}

/// One round of the mirror; whether to go again at once (the catalogue or the downloads changed
/// meanwhile).
fn once(s: &crate::AppState, m: &Mirror, mut lead: Option<&mut crate::maplog::MirrorLink>) -> bool {
    let cat = s.data.catalogue();
    // (Nothing known of the map yet: what's here can't be told from what isn't wanted.)
    if cat.n == 0 {
        return false;
    }
    let Some((plan, wanted)) = round_plan(s, m, &cat) else { return true };
    let wanted = &wanted;
    let changed =
        || s.data.generation.load(Ordering::SeqCst) != plan.generation || s.downloads.version() != plan.version;
    let Some((root, pool)) = s.data.nas_root().zip(s.data.pool()) else {
        // No NAS mounted: what no download wants goes all the same; nothing can be copied.
        match m.let_go(&cat, wanted) {
            Ok(st) if st.removed > 0 => eprintln!("mirror (no NAS): {st:?}"),
            Ok(_) => {}
            Err(e) => eprintln!("mirror: {e:#}"),
        }
        return changed();
    };
    if pool.is_online() {
        if let Err(e) = size_pieces(m, plan.wanted.items.iter().filter_map(piece_of), &root, &pool, &changed) {
            eprintln!("mirror: sizing the pieces: {e:#}");
        }
    }
    let slow = || s.data.agent_busy();
    // (Its cursor said to the lead before it copies: the files it's after are kept on the NAS. One
    // the lead resets: the Updates of an older map go on at the one served, once that's the lead's
    // head or past it. Before, it's this server's map kept on this Mac, a start or a wake not yet
    // caught up (crate::maplog), whose files GC may have removed as well: the reset waits.)
    if let Some(l) = lead.as_deref_mut() {
        let since = plan.copies_from(m);
        if let Some(head) = l.tell(since, plan.current).filter(|&h| plan.current >= h) {
            l.reset_done();
            if since < head && s.downloads.resync(&plan) {
                return true;
            }
        }
    }
    let mut bridged = false;
    match m.sync(&cat, wanted, &root, &pool, &Control { stop: &changed, slow: &slow }) {
        Ok(st) => {
            let through = st.end == SyncEnd::Done && st.waiting == 0 && st.failed == 0 && st.pending == 0;
            if let Some(l) = lead.as_deref_mut().filter(|_| through) {
                l.through(plan.current, plan.copies_from(m));
            }
            if st.copied > 0 || st.removed > 0 || st.waiting > 0 || st.failed > 0 {
                eprintln!("mirror: {st:?}");
            }
            if st.copied > 0 {
                s.data.forget_remote();
            }
            // (What replaces a download's older form came: planned again at once, so the old
            // form goes as soon as the new one is all here, `bridge`.)
            bridged = st.copied > 0 && !plan.bridges.is_empty();
        }
        Err(e) => eprintln!("mirror: {e:#}"),
    }
    // (An update all here now moves on at once.)
    if bridged || changed() || s.downloads.advance(&plan, m) {
        return true;
    }
    if pool.is_online() {
        let all = plan
            .now()
            .world
            .iter()
            .flat_map(Part::pieces)
            .chain(plan.now().regions.values().flat_map(|r| r.coarse.pieces()));
        if let Err(e) = size_pieces(m, all, &root, &pool, &changed) {
            eprintln!("mirror: sizing the pieces: {e:#}");
        }
    }
    changed()
}

fn piece_of(i: &Item) -> Option<(&str, Piece)> {
    match i {
        Item::Piece { archive, piece } => Some((archive.as_str(), *piece)),
        Item::File { .. } => None,
    }
}

/// Works out the sizes of these pieces not known yet, from their archives and packs on the NAS (their
/// directories and indexes alone), until `stop()`.
fn size_pieces<'a>(
    m: &Mirror,
    pieces: impl Iterator<Item = (&'a str, Piece)>,
    root: &store::nasfs::NasRoot,
    pool: &Arc<store::IoPool>,
    stop: &dyn Fn() -> bool,
) -> Result<()> {
    let mut by: BTreeMap<&str, BTreeSet<Piece>> = BTreeMap::new();
    for (a, p) in pieces.filter(|(a, p)| m.piece_size(a, *p).is_none()) {
        by.entry(a).or_default().insert(p);
    }
    for (archive, ps) in by {
        if stop() {
            break;
        }
        let f = store::PooledFile::open(pool, &root.join(archive)).with_context(|| format!("open {archive}"))?;
        let src = m.piece_source(archive, Box::new(f))?;
        let ps: Vec<Piece> = ps.into_iter().collect();
        let t = Instant::now();
        let n = m.size_pieces(archive, &ps, &src, stop)?;
        if n > 10 {
            eprintln!("mirror: sized {n} pieces of {archive} in {:.1} s", t.elapsed().as_secs_f64());
        }
    }
    Ok(())
}

// ---- the state, for the panel ------------------------------------------------------------------

/// A download's state: all here, being copied, waiting its turn, waiting for room, or the NAS
/// unreachable; for these items (its own, or an update's).
fn items_state(items: &[&Item], m: &Mirror, copying: Option<&str>, more: u64, online: bool) -> &'static str {
    let (bytes, here, unknown) = tally(items.iter().copied(), m);
    if unknown == 0 && here >= bytes {
        "done"
    } else if !online {
        "away"
    } else if copying.is_some_and(|c| items.iter().any(|i| store::mirror::copy_name(i) == c)) {
        "copying"
    } else if more > 0 {
        "room"
    } else {
        "queued"
    }
}

/// What a file or piece is of, in a word, for what changed: a layer's name, "basemap", "road data",
/// "lists" (hidata), or a worldwide file's kind.
fn kind(i: &Item) -> String {
    let (Item::File { name, .. } | Item::Piece { archive: name, .. }) = i;
    let logical = store::naming::parse_content_name(name).map_or("", |c| c.logical);
    let mut parts = logical.split('/');
    match (parts.next(), parts.next()) {
        (Some("layers"), Some(l)) => l.to_string(),
        (Some("base"), _) | (Some("global"), Some("roads")) => "road data".into(),
        (Some("hidata"), _) => "lists".into(),
        (Some("global"), Some(g)) => g.to_string(),
        (Some(o), _) => o.to_string(),
        _ => String::new(),
    }
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn now() -> u64 {
    secs(SystemTime::now())
}

/// The downloads' state for the panel (`GET /api/downloads`, docs/formats.md).
pub fn status(s: &crate::AppState) -> Result<Value> {
    let Some(m) = s.data.mirror.as_ref() else {
        return Ok(json!({ "mirror": false, "online": s.data.online(), "regions": {} }));
    };
    let plan = s.downloads.plan(&s.data);
    let room = m.room(&plan.wanted)?;
    let copying = m.copying();
    let cname = copying.as_ref().map(|c| c.name.as_str());
    let online = s.data.online();
    let slow = online && s.data.agent_busy();
    let tally_of = |items: &[&Item]| {
        let (bytes, here, unknown) = tally(items.iter().copied(), m);
        json!({ "bytes": bytes, "here": here, "unknown": unknown })
    };
    // A download's own: its size and how much is here (at its catalogue), its state, and how it's
    // behind the catalogue served or updating to it; not downloaded, the served catalogue's size.
    let dl = |w: &Which, pin: Option<Pin>| -> Option<Value> {
        let Some(pin) = pin else {
            let p = plan.part(w, plan.current)?;
            let mut v = tally_of(&p.items.iter().collect::<Vec<_>>());
            v["on"] = json!(false);
            v["state"] = Value::Null;
            return Some(v);
        };
        let cat = plan.eff(pin.cat);
        let own: Vec<&Item> = plan.own(w, pin)?.all().collect();
        let mut v = tally_of(&own);
        v["on"] = json!(true);
        v["cat"] = json!(cat);
        v["state"] = json!(items_state(&own, m, cname, room.more, online));
        let had: HashSet<&Item> = own.iter().copied().collect();
        let to = pin.to.map(|t| plan.eff(t)).unwrap_or(plan.current);
        if to != cat {
            if let (Some(p), Some(target)) = (plan.part(w, to), plan.target(w, to)) {
                let new: Vec<&Item> = target.into_iter().filter(|i| !had.contains(i)).collect();
                let gone = own.len() - p.items.iter().filter(|i| had.contains(i)).count();
                let mut kinds: Vec<String> = new.iter().map(|i| kind(i)).collect::<BTreeSet<_>>().into_iter().collect();
                kinds.retain(|k| !k.is_empty());
                let (bytes, here, unknown) = tally(new.iter().copied(), m);
                let mut b = json!({ "cat": to, "files": new.len(), "gone": gone, "what": kinds,
                    "bytes": bytes.saturating_sub(here), "unknown": unknown });
                if pin.to.is_some() {
                    v["update"] = tally_of(&new);
                    v["state"] = json!("updating");
                    b["state"] = json!(items_state(&new, m, cname, room.more, online));
                }
                v["behind"] = b;
            }
        }
        Some(v)
    };
    let asked: HashMap<&str, &AskedRegion> = plan.asked.regions.iter().map(|r| (r.id.as_str(), r)).collect();
    let listed: BTreeMap<String, Value> = crate::regions::recorded_list_kept(&s.data.catalogue())
        .iter()
        .filter_map(|r| Some((r["id"].as_str()?.to_string(), r.clone())))
        .collect();
    let mut regions = serde_json::Map::new();
    let ids: BTreeSet<&str> = plan.now().regions.keys().map(String::as_str).chain(asked.keys().copied()).collect();
    for id in ids {
        let a = asked.get(id);
        let (rw, dw) = (Which::Region(id.to_string()), Which::Detail(id.to_string()));
        let Some(mut v) = dl(&rw, a.map(|a| a.pin)) else { continue };
        // (Its terrain detail, where it has any: its size, the share of the region it covers.)
        let detail_pin = a.and_then(|a| a.detail);
        let rp = plan.at.get(&plan.eff(detail_pin.map_or(plan.current, |p| p.cat))).and_then(|at| at.regions.get(id));
        if rp.is_some_and(|rp| !rp.detail.items.is_empty()) || detail_pin.is_some() {
            if let Some(mut f) = dl(&dw, detail_pin) {
                let cover = rp.map_or(0.0, |rp| rp.cover);
                f["cover"] = json!((cover * 10.0).round() / 10.0);
                v["detail"] = f;
            }
        }
        let r = listed.get(id);
        v["name"] = json!(r.and_then(|r| r["name"].as_str()).or(a.map(|a| a.name.as_str())).unwrap_or_default());
        regions.insert(id.to_string(), v);
    }
    // Downloaded regions no catalogue here has (not built yet, or removed since).
    for r in &plan.asked.regions {
        regions.entry(r.id.clone()).or_insert_with(
            || json!({ "name": r.name, "bytes": 0, "here": 0, "unknown": 0, "on": true, "state": "missing" }),
        );
    }
    let mut world = dl(&Which::World, plan.asked.world.as_ref().map(|w| w.pin)).unwrap_or_default();
    world["at"] = json!(plan.asked.world.as_ref().map(|w| w.at));
    let wanted_here: u64 = plan.wanted.items.iter().map(|i| m.item_here(i)).sum();
    let last = m.last().map(|(st, at)| {
        json!({
            "at": secs(at), "copied": st.copied, "copied_bytes": st.copied_bytes, "removed": st.removed, "removed_bytes": st.removed_bytes,
            "waiting": st.waiting, "waiting_bytes": st.waiting_bytes, "failed": st.failed, "pending": st.pending,
            "end": match st.end { SyncEnd::Done => "done", SyncEnd::Paused => "paused", SyncEnd::Offline => "offline" },
        })
    });
    let what = |c: &store::mirror::Copying| -> String {
        if let Some(rest) = c.name.strip_prefix("basemap ") {
            return format!(
                "the basemap's {}",
                rest.split(' ').nth(1).map_or("piece".into(), |p| if p == "lo" {
                    "zooms 0–10".to_string()
                } else {
                    format!("zooms 11–14 of {}", p.replacen('-', "/", 2))
                })
            );
        }
        store::naming::parse_content_name(&c.name).map(|c| c.logical.to_string()).unwrap_or_default()
    };
    Ok(json!({
        "mirror": true,
        "online": online,
        "slow": slow,
        "rate": store::mirror::SLOW_RATE,
        "free": room.free,
        "reserve": room.reserve,
        "reserve_set": m.reserve_set(),
        "reserve_min": crate::reserve::MIN_GB * 1_000_000_000,
        "disk": store::sys::disk_size(m.root()).ok(),
        "here": m.usage().1,
        "world": world,
        "wanted": { "bytes": room.missing + wanted_here, "here": wanted_here, "more": room.more, "unknown": room.unknown },
        "copying": copying.as_ref().map(|c| json!({ "what": what(c), "bytes": c.size, "have": c.have, "slow": c.slow })),
        "last": last,
        "regions": regions,
    }))
}

// ---- the API ----------------------------------------------------------------------------------

fn err(code: StatusCode, msg: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": msg.to_string() }))).into_response()
}

fn no_store(v: Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
}

/// Runs `f` off the async threads: its answer, or a 400 with why (500 if it panicked).
async fn blocking(s: S, f: impl FnOnce(&crate::AppState) -> Result<Value> + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(move || f(&s)).await {
        Ok(Ok(v)) => no_store(v),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_status(State(s): State<S>) -> Response {
    match tokio::task::spawn_blocking(move || status(&s)).await {
        Ok(Ok(v)) => no_store(v),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Bytes in metric units, as the panel says them: 12 KB, 840 MB, 2.4 GB (under 1 MB in KB, at least 1).
fn size(b: u64) -> String {
    if b > 0 && b < 1_000_000 {
        format!("{} KB", (b as f64 / 1e3).round().max(1.0))
    } else if b < 1_000_000_000 {
        format!("{} MB", (b as f64 / 1e6).round())
    } else {
        format!("{:.1} GB", b as f64 / 1e9)
    }
}

/// What all that `next` downloads would still need on this Mac, and whether that fits the free
/// space above the reserve: (need, room above the reserve). The pieces' sizes are worked out from
/// the NAS first where they aren't known.
fn need(s: &crate::AppState, m: &Mirror, next: &Asked) -> Result<(u64, u64)> {
    let cur = s.downloads.plan(&s.data);
    let plan = make_plan(cur.generation, cur.version, next.clone(), cur.current, cur.at.clone(), &|i| m.is_here(i));
    if plan.wanted.items.iter().any(|i| m.item_size(i).is_none()) {
        let (Some(root), Some(pool)) = (s.data.nas_root(), s.data.pool().filter(|p| p.is_online())) else {
            bail!("The NAS isn't reachable, so this Mac can't download from it now")
        };
        size_pieces(m, plan.wanted.items.iter().filter_map(piece_of), &root, &pool, &|| false)?;
    }
    let room = m.room(&plan.wanted)?;
    Ok((room.missing, room.free.saturating_sub(room.reserve)))
}

/// Refuses `next` when it wouldn't fit, saying why with the numbers (`what`: "Downloading “London”").
fn check_fits(s: &crate::AppState, next: &Asked, what: &str) -> Result<()> {
    let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
    let (need, room) = need(s, m, next)?;
    let already = if s.downloads.asked() != Asked::default() { ", with what's downloaded already" } else { "" };
    ensure!(
        need <= room,
        "{what} takes {} more on this Mac{already}, and it has {} free above the reserve ({}): free some space, or remove a download",
        size(need),
        size(room),
        size(m.reserve())
    );
    Ok(())
}

/// A new download's pin: the catalogue served.
fn pin_now(s: &crate::AppState) -> Pin {
    Pin { cat: s.data.catalogue().n, to: None }
}

#[derive(Deserialize)]
pub struct Reserve {
    gb: u64,
}

/// Sets the reserve to `gb` on a disk of `disk_bytes`: kept in the home, the mirror told, and its
/// thread woken (downloads waiting for room may start; raised, nothing is deleted).
fn set_reserve(s: &crate::AppState, gb: u64, disk_bytes: u64) -> Result<Value> {
    let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
    s.reserve.set(gb, disk_bytes, m)?;
    s.downloads.poke();
    Ok(json!({ "ok": true, "reserve": m.reserve(), "reserve_set": m.reserve_set() }))
}

/// Sets the reserve (crate::reserve), bounded by the size of the mirror's disk.
pub async fn put_reserve(State(s): State<S>, Json(r): Json<Reserve>) -> Response {
    blocking(s, move |s| {
        let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
        let disk = store::sys::disk_size(m.root()).context("the disk's size")?;
        set_reserve(s, r.gb, disk)
    })
    .await
}

#[derive(Deserialize)]
pub struct On {
    on: bool,
}

/// Downloads the World, zoomed out, or removes it.
pub async fn put_world(State(s): State<S>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        let mut next = s.downloads.asked();
        if q.on {
            if next.world.is_none() {
                next.world = Some(AskedWorld { at: now(), pin: pin_now(s) });
                check_fits(s, &next, "Downloading the World, zoomed out,")?;
            }
        } else {
            next.world = None;
        }
        s.downloads.edit(|a| {
            *a = next;
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

/// Downloads a region, at the catalogue served, or removes it (with its terrain detail). A region the
/// catalogue doesn't have yet can be downloaded: its files come once a catalogue has it (its download
/// moves on to it: it has nothing to keep). One still building has the files of the
/// catalogue served; an Update brings those of the tiles built since.
pub async fn put_region(State(s): State<S>, Path(id): Path<String>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(buildstore::recipes::valid_id(&id), "{id:?} isn't a region id");
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        ensure!(
            !(q.on && crate::regions::is_disabled(s, &id)),
            "{id:?} is disabled in its file (disabled = true): nothing is built for it to download"
        );
        let cat = s.data.catalogue();
        let name = crate::regions::recorded_list_kept(&cat)
            .iter()
            .find(|r| r["id"] == id.as_str())
            .and_then(|r| r["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| id.clone());
        let mut next = s.downloads.asked();
        let was = next.regions.iter().any(|r| r.id == id);
        if q.on {
            if !was {
                next.regions.push(AskedRegion {
                    id: id.clone(),
                    name: name.clone(),
                    at: now(),
                    pin: pin_now(s),
                    detail: None,
                });
                check_fits(s, &next, &format!("Downloading “{name}”"))?;
            }
        } else {
            next.regions.retain(|r| r.id != id);
        }
        s.downloads.edit(|a| {
            *a = next;
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

/// Downloads a region's terrain detail (refused unless the region is downloaded: it needs it), or
/// removes it (the region stays).
pub async fn put_region_detail(State(s): State<S>, Path(id): Path<String>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(buildstore::recipes::valid_id(&id), "{id:?} isn't a region id");
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        let mut next = s.downloads.asked();
        let at = next.regions.iter().position(|r| r.id == id);
        if q.on {
            let Some(i) = at else { bail!("Download the region first: its terrain detail needs it") };
            let plan = s.downloads.plan(&s.data);
            let has = plan.now().regions.get(&id).is_some_and(|p| !p.detail.items.is_empty());
            ensure!(has, "{id:?} has no terrain detail on the map");
            if next.regions[i].detail.is_none() {
                next.regions[i].detail = Some(pin_now(s));
                let name = next.regions[i].name.clone();
                check_fits(s, &next, &format!("Downloading the terrain detail of “{name}”"))?;
            }
        } else if let Some(i) = at {
            next.regions[i].detail = None;
        }
        s.downloads.edit(|a| {
            *a = next;
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

/// Updates a download to the catalogue served (`w`; `what` names it): its files copied beside its
/// old ones, the download moved there once they're all here (the mirror thread). Refused when it
/// isn't downloaded, isn't behind, or wouldn't fit.
fn update(s: &crate::AppState, w: Which, what: &str) -> Result<Value> {
    ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
    let plan = s.downloads.plan(&s.data);
    let mut next = s.downloads.asked();
    let Some(pin) = next.pin_mut(&w) else { bail!("{what} isn't downloaded") };
    ensure!(plan.eff(pin.cat) != plan.current, "{what} is at the map served already");
    if pin.to == Some(plan.current) {
        return Ok(json!({ "ok": true }));
    }
    pin.to = Some(plan.current);
    check_fits(s, &next, &format!("Updating {what}"))?;
    s.downloads.edit(|a| {
        if let Some(p) = a.pin_mut(&w) {
            p.to = Some(plan.current);
        }
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// The name a region's download goes by in what's said.
fn region_name(s: &crate::AppState, id: &str) -> String {
    s.downloads.asked().regions.iter().find(|r| r.id == id).map_or_else(|| id.to_string(), |r| format!("“{}”", r.name))
}

pub async fn post_world_update(State(s): State<S>) -> Response {
    blocking(s, |s| update(s, Which::World, "the World, zoomed out,")).await
}

pub async fn post_region_update(State(s): State<S>, Path(id): Path<String>) -> Response {
    blocking(s, move |s| {
        ensure!(buildstore::recipes::valid_id(&id), "{id:?} isn't a region id");
        let what = region_name(s, &id);
        update(s, Which::Region(id), &what)
    })
    .await
}

pub async fn post_detail_update(State(s): State<S>, Path(id): Path<String>) -> Response {
    blocking(s, move |s| {
        ensure!(buildstore::recipes::valid_id(&id), "{id:?} isn't a region id");
        let what = format!("the terrain detail of {}", region_name(s, &id));
        update(s, Which::Detail(id), &what)
    })
    .await
}

#[cfg(test)]
mod tests;
