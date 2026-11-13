//! The inputs' identities (docs/readiness.md §4): for each `Read::Input` a step declares
//! (crate::agent::steps), what a target's key names of it, as lines (`in <name> <line>`). Each is
//! the function the run's own reads are worked out by, so the key and the run can't name the input
//! differently. None while it can't be worked out now: the target's key is unknown and it waits.

use super::build::KeyCtx;
use codecs::legacy::TileId;

/// The spacing the extract reads (inputs::dem::spacing::key_line); None while it isn't known.
pub fn spacing(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![inputs::dem::spacing::key_line(x.inputs)?])
}

/// The heritage registers meeting the coverage, each by its keyed digest
/// (landmarks::heritage::parse_registers, from the records' accepted version in `inputs` "heritage");
/// a plan given no such line names the registers' accepted index whole. None while they have no
/// accepted version or it can't be read ("?").
pub fn registers(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let index = x.m.get(&inputs::inputs::logical(inputs::inputs::heritage::UNIT))?;
    match x.inputs.get("heritage").map(String::as_str) {
        Some("?") => None,
        Some(line) => {
            let cover = landmarks::heritage::cover_boxes(x.cov);
            Some(
                landmarks::heritage::parse_registers(line)
                    .into_iter()
                    .filter(|(_, _, b)| b.is_some_and(|b| landmarks::heritage::meets(&cover, b)))
                    .map(|(id, keyed, _)| format!("{id} {keyed}"))
                    .collect(),
            )
        }
        None => Some(vec![format!("index {index}")]),
    }
}

/// The World Heritage dots the last marks run wrote (landmarks::markconv::HERITAGE_DOTS), by content
/// ("-" before there are any: the overlays wait for them, `View::known`, and are keyed so after).
pub fn heritage_dots(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, [landmarks::markconv::HERITAGE_DOTS.to_string()]))
}

/// `logicals` by content as a key names them: `<logical>=<content>`, `<logical> -` for one the
/// manifest lacks.
fn named(x: &KeyCtx, logicals: impl IntoIterator<Item = String>) -> Vec<String> {
    logicals.into_iter().map(|l| x.m.get(&l).map_or_else(|| format!("{l} -"), |c| format!("{l}={c}"))).collect()
}

/// `logicals` by content, each required: None while one is missing.
fn required(x: &KeyCtx, logicals: impl IntoIterator<Item = String>) -> Option<Vec<String>> {
    logicals.into_iter().map(|l| x.m.get(&l).map(|c| format!("{l}={c}"))).collect()
}

/// The pass's extracts (`sources/osm/<date>/pieces/`) by content; None with none (the reach).
pub fn pieces(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let p = format!("sources/osm/{}/pieces/", x.date);
    let v: Vec<String> =
        x.m.range(p.clone()..).take_while(|(l, _)| l.starts_with(&p)).map(|(_, c)| c.clone()).collect();
    (!v.is_empty()).then_some(v)
}

/// The pass's outlines (the outlines step's, `sources/osm/<date>/outlines`), required.
pub fn pass_outlines(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [format!("sources/osm/{}/outlines", x.date)])
}

/// The languages input's accepted index (docs/inputs.md §6.2), required.
pub fn language_table(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [inputs::inputs::logical(inputs::inputs::languages::UNIT)])
}

/// The pass's basemap (`layers/basemap/world-<date>`), required.
pub fn basemap(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [format!("layers/basemap/world-{}", x.date)])
}

/// The pass's lakes filled from Canada's own data (inputs::lakefill::logical), when made.
pub fn lakefill(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, [inputs::lakefill::logical(x.date)]))
}

/// The rail and ferries extracts the timetables' to-do list reads, required.
pub fn transit_lines(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [inputs::inputs::gtfs::RAIL_LINES.to_string(), inputs::inputs::gtfs::FERRY_LINES.to_string()])
}

/// The cover (landmarks::heritage::cover_key: the z12 tiles within 20 km of the coverage's land
/// and the z6 tiles holding them), what the heritage jobs and rail clip their sets to; kept by the
/// coverage's version.
pub fn coverage_cover(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let v = x.cov.version();
    x.tiles.memo("coverage:cover", &v, || Ok(landmarks::heritage::cover_key(x.cov))).ok().map(|k| vec![k])
}

/// What world step `t`'s run asked of the coverage, as the coverage answers it now
/// (buildstore::asked::line: its record's line while the coverage is the version it asked, or
/// answers all it asked the same); asked again once a coverage version and record. None while the
/// record, or the outlines its questions name, can't be read now.
pub fn coverage_asked(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let Some(c) = x.m.get(&buildstore::asked::logical(t)) else {
        return Some(vec![buildstore::asked::line(x.cov, None, &|_| None)?]);
    };
    let r = x.tiles.asked(c)?;
    let v = x.cov.version();
    if r.version == v {
        return Some(vec![r.line.clone()]);
    }
    let reask = |q: &str| -> Option<String> {
        let (o, _) = inputs::rail::parse_countries_question(q)?;
        inputs::rail::reask_countries(x.cov, q, x.tiles.outlines(x.m, &o)?.as_ref())
    };
    // (And whether the outlines each question names are named still: once they aren't, its answer is GONE.)
    let named: Vec<bool> = r
        .also
        .iter()
        .filter_map(|(q, _)| inputs::rail::parse_countries_question(q))
        .map(|(o, _)| x.m.values().any(|v| *v == o))
        .collect();
    let l = x.tiles.memo(&format!("coverage:asked {t}"), &format!("{v} {c} {named:?}"), || {
        buildstore::asked::line(x.cov, Some((c, &r)), &reask)
            .ok_or_else(|| super::tiles::Unread(format!("{c}'s questions")))
    });
    l.ok().map(|l| vec![l])
}

/// The heritage matches' accepted version, when there is one.
pub fn matches(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, [inputs::inputs::logical(inputs::inputs::heritage_matches::UNIT)]))
}

/// A candidates' z6 tile's extract by content (`build::pois_units`: one meeting the coverage).
pub fn extract(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    required(x, [format!("sources/osm/{}/pieces/{}", x.date, u.dash())])
}

/// The coverage a candidates' z6 tile clips to: its rings over the tile and 10 km around it.
pub fn coverage_clip(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(vec![x.cov.fingerprint(roadcore::geo::grown_e7(u.z, u.x, u.y, 10.0))])
}

/// The pass's hiking-route ends (the trailends step's), when made.
pub fn pass_trailends(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, [format!("work/trailends/{}", x.date)]))
}

/// The pass's summits (the summits step's), when made.
pub fn pass_summits(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, [format!("work/summits/{}", x.date)]))
}

/// The candidates of the coverage's z6 tiles now (`build::pois_units`), by content, those made;
/// None with none (the items wait).
pub fn pois_now(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let v: Vec<String> = super::build::pois_units(x.cov, x.date, x.m)
        .into_iter()
        .filter_map(|(u, _, _)| {
            let l = format!("work/pois/{}", u.dash());
            x.m.get(&l).map(|c| format!("{l}={c}"))
        })
        .collect();
    (!v.is_empty()).then_some(v)
}

/// Every candidates' z6 tile's `step` file (`work/pois/`, `work/peaks/`), made or not; None while
/// the spacing their extract reads isn't known (no candidates' targets then).
fn per_unit(step: &str, x: &KeyCtx) -> Option<Vec<String>> {
    inputs::dem::spacing::key_line(x.inputs)?;
    Some(named(
        x,
        super::build::pois_units(x.cov, x.date, x.m).into_iter().map(|(u, _, _)| format!("work/{step}/{}", u.dash())),
    ))
}

/// The landmark points' candidates (`per_unit`).
pub fn units_pois(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    per_unit("pois", x)
}

/// The landmark points' peaks (`per_unit`).
pub fn units_peaks(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    per_unit("peaks", x)
}

/// The pass's heritage sites' files the rest of the heritage chain reads
/// (landmarks::heritage::base_logical); None before its heritage-sources are made.
pub fn heritage_sites_out(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [landmarks::heritage::base_logical(x.date, "heritage-sources")])?;
    Some(named(
        x,
        ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"]
            .into_iter()
            .map(|s| landmarks::heritage::base_logical(x.date, s)),
    ))
}

/// The pass's named and areas sets (when made) and its World Heritage set (required) the heritage
/// chain reads.
pub fn heritage_sets(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let whs = required(x, [inputs::osmpass::set_name(x.date, "whs")])?;
    let mut v = named(x, ["named", "areas"].into_iter().map(|k| inputs::osmpass::set_name(x.date, k)));
    v.extend(whs);
    Some(v)
}

/// The items' facts and pageviews of the pass (the items step's).
pub fn items_out(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, ["facts", "views"].into_iter().map(|f| format!("sources/items/{}/{f}", x.date))))
}

/// The heritage chain's landmark files the marks read (landmarks::markconv::heritage_source).
pub fn heritage_marks(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let src = landmarks::markconv::heritage_source(x.date);
    Some(named(x, ["layer-heritage", "details-heritage", "props-heritage"].into_iter().map(|s| format!("{src}/{s}"))))
}

/// The heritage chain's area files the overlays draw (landmarks::markconv::heritage_source); None
/// before its landmark files are made.
pub fn heritage_areas(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let src = landmarks::markconv::heritage_source(x.date);
    for s in ["layer-heritage", "details-heritage", "props-heritage"] {
        x.m.get(&format!("{src}/{s}"))?;
    }
    Some(named(
        x,
        [
            "layer-heritage-areas",
            "layer-indigenous",
            "layer-special",
            "layer-whs-shapes",
            "details-harea",
            "details-indigenous",
            "details-special",
            "details-park",
            "layer-summary",
            "heritage-sources",
        ]
        .into_iter()
        .map(|s| format!("{src}/{s}")),
    ))
}

/// The built z6 tiles (`base/`), by name: where the overlays' hi tiles are.
pub fn base_tiles(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(x.m.range("base/".to_string()..).take_while(|(l, _)| l.starts_with("base/")).map(|(l, _)| l.clone()).collect())
}

/// The 3D buildings' declared release (`inputs` "bld-key": bld::bld::sources::declared), "-"
/// without one.
pub fn bld_key(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.inputs.get("bld-key").map_or("-", String::as_str).to_string()])
}

/// Whether the release's sources are downloaded (`inputs` "bld-release",
/// bld::bld::sources::digests); None when that can't be read now, or the sources aren't declared.
pub fn bld_sources(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    match x.inputs.get("bld-release").map(String::as_str) {
        None | Some("?") => None,
        Some(_) if !x.inputs.contains_key("bld-key") => None,
        Some(r) => Some(vec![if r.is_empty() { "none" } else { "downloaded" }.to_string()]),
    }
}

/// What a terrain z6 tile's run reads of the coverage (task #832, `build::piece_coverage`: the
/// tiles of its levels it makes); a terrain-deep z10 tile's, its levels' (terrain::terrain_deep::Deep::coverage).
/// None while the declared datasets can't be read.
pub fn coverage_piece(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(vec![match u.z {
        6 => super::build::piece_coverage(x.cov, &x.tiles.zoom_sets(x.m)?, (u.x, u.y)),
        z if z == terrain::terrain_deep::PACK_Z => x.tiles.deep(x.m, x.cov)?.coverage((u.x, u.y)),
        _ => return None,
    }])
}

/// The pass's filtered planet (`sources/osm/<date>/filtered`) by content, required.
pub fn filtered(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [format!("sources/osm/{}/filtered", x.date)])
}

/// The pass the target is of (its planet's date).
pub fn pass(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.date.to_string()])
}

/// The pass's set `kind` (inputs::osmpass::set_name) by content; None before pass-sets makes it.
fn set(kind: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.m.get(&inputs::osmpass::set_name(x.date, kind))?.clone()])
}

/// One `Read::Input` per pass set a step reads, `set_<kind>`.
macro_rules! sets {
    ($($f:ident $kind:literal),* $(,)?) => {$(
        #[doc = concat!("The pass's `", $kind, "` set by content (`set`).")]
        pub fn $f(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
            set($kind, x)
        }
    )*};
}
sets!(set_outlines "outlines", set_coastline "coastline", set_hikes "hikes", set_labels "labels", set_summits "summits", set_areas "areas");

/// The previous pass's outlines the outlines step may keep a country from
/// (inputs::osmpass::previous_outlines): its date and content, "-" with none.
pub fn previous_outlines(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![inputs::osmpass::previous_outlines(x.m, x.date).map_or("-".to_string(), |(d, c)| format!("{d} {c}"))])
}

/// Nothing read of a step waited for whole, its reads named otherwise: the overlays wait for the
/// marks' reads (so the dots they read aren't about to change), not the marks, which are gated by
/// time and rarely current while z6 tiles arrive (the dots are an input, `heritage_dots`); a z6
/// tile's roads for the pass's reaches, read as its ways meet the coverage (`ways`).
pub fn nothing(_t: &str, _x: &KeyCtx) -> Option<Vec<String>> {
    Some(Vec::new())
}

/// A z6 tile of the roads (`unit`): the tile, its reach (one the coverage builds at the spacing
/// the declared datasets give, `build::builds`) and that spacing.
fn unit_of<'a>(
    t: &str,
    x: &KeyCtx<'a>,
) -> Option<(TileId, &'a roads::reach::Reach, std::sync::Arc<inputs::dem::spacing::Fine>)> {
    let u = TileId::parse(t).filter(|u| u.z == 6)?;
    let fine = x.tiles.spacing(x.m);
    let r = x.reach?.get(u).filter(|r| r.builds(x.cov, &*fine))?;
    Some((u, r, fine))
}

/// The box a z6 tile's roads stage over (its tile + roads::stage::MARGIN_KM).
fn unit_box(u: TileId) -> [f64; 4] {
    roadcore::geo::tile_box_grown(u.z, u.x, u.y, roads::stage::MARGIN_KM)
}

/// A z6 tile's road values (`sources/osm/<date>/roads/`), when made.
pub fn roads(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(named(x, [format!("sources/osm/{}/roads/{}", x.date, u.dash())]))
}

/// What a z6 tile's roads read of the reach and the coverage: the coverage as its ways meet it
/// (`Reach::coverage_key`), the location rules over its extent (inputs::rules), which of the cells
/// its ways start in a fine dataset covers (`Fine::key`).
pub fn ways(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let (u, r, fine) = unit_of(t, x)?;
    Some(vec![
        format!("coverage {}", r.coverage_key(x.cov, u, &*fine)),
        format!("rules {}", inputs::rules::versions_meeting(r.extent(u))),
        format!("spacing {}", fine.key(&r.cells)),
    ])
}

/// The declared elevation datasets meeting a z6 tile's ways' extent, by their keyed digests
/// (inputs::dem::declared::meeting); None while they can't be read (the z6 tile waits).
pub fn elevation(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let (u, r, _) = unit_of(t, x)?;
    let e = x.tiles.elevation(x.m).ok().flatten()?;
    x.tiles.keyed(x.m)?;
    let ext = r.extent(u);
    let deg = |v: i32| v as f64 * 1e-7;
    Some(vec![inputs::dem::declared::meeting(&e, [deg(ext[0]), deg(ext[1]), deg(ext[2]), deg(ext[3])]).join(",")])
}

/// The land cover and canopy declarations a z6 tile reads where they aren't as staged
/// (inputs::inputs::sources::unit_extra), none where they are; None while the sources' keys can't be
/// read.
pub fn unit_sources(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(x.tiles.keyed(x.m)?.unit.iter().cloned().collect())
}

/// The roadside buildings' index of the declared release a z6 tile reads (bld::buildtiles).
pub fn roadside(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let k = x.tiles.keyed(x.m)?;
    Some(named(x, [bld::buildtiles::index_logical(&k.release)]))
}

/// The terrain tiles a z6 tile's roads stage (`build::unit_terrain`: their XXH3s, digested); None
/// while a pack's index can't be read.
pub fn unit_tiles(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let (u, r, _) = unit_of(t, x)?;
    Some(vec![super::build::unit_terrain(u, r, x.m, x.tiles).ok()?])
}

/// The deep tiles a z6 tile's roads' heights can read, by content (`build::unit_deep_tiles`);
/// None while a pack's index can't be read.
pub fn unit_deep_tiles(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let (u, r, _) = unit_of(t, x)?;
    super::build::unit_deep_tiles(u, r, x.m, x.tiles).ok()
}

/// The pass's heritage slices (sites and areas) of the z6 tiles over a z6 tile's box, as the
/// manifest has them (what the roads stage).
pub fn heritage_slices(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(named(
        x,
        roadcore::geo::tiles_in(6, unit_box(u)).into_iter().flat_map(|(a, b)| {
            [landmarks::heritage::pos_logical(x.date, a, b), landmarks::heritage::areas_logical(x.date, a, b)]
        }),
    ))
}

/// A multi-line identity (a dataset's lines, its deep levels') as key lines, each after `head`.
fn lines_of(head: &str, s: &str) -> Vec<String> {
    s.lines().map(|l| if head.is_empty() { l.to_string() } else { format!("{head} {l}") }).collect()
}

/// The depth's water source (terrain::terrain_pack::water_source_pin: the latest pass's basemap's
/// digests' file), required: the terrain-water job's.
pub fn water_pin(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![terrain::terrain_pack::water_source_pin(x.m)?])
}

/// What the depth's targets read of the water (`build::depth_water_wants`): what the terrain-water
/// job digests, "<target> <from>" each; None while the depth's sources can't be read.
pub fn water_wants(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let d = x.tiles.depth(x.m)?;
    Some(super::build::depth_water_wants(x.cov, &d).into_iter().map(|(t, f)| format!("{t} {f}")).collect())
}

/// The elevation pool's datasets meeting a terrain target's tile (`TerrainTiles::terrain_datasets`:
/// their lines as the roads' keys name them); None while they can't be told.
pub fn datasets_terrain(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(lines_of("", &x.tiles.terrain_datasets(x.m, roadcore::geo::tile_box(u.z, u.x, u.y))?))
}

/// What a terrain target reads of the declared datasets' zooms (terrain::terrain_zoom::key_lines over
/// its tile: each meeting it, the zoom it reaches there), and the deep band's code where it has
/// some (terrain::terrain_deep::target_key_lines); None while they can't be read.
pub fn zoom_lines(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    let sets = x.tiles.zoom_sets(x.m)?;
    Some(match u.z {
        z if z == terrain::terrain_deep::PACK_Z => {
            lines_of("", &terrain::terrain_deep::target_key_lines(&sets, (u.x, u.y)))
        }
        _ => terrain::terrain_zoom::key_lines(&sets, u.z, u.x, u.y),
    })
}

/// "no hi tiles" for a terrain z6 tile that makes none (terrain::terrain_pack::makes_hi): its run
/// drops any hi pack an earlier one left. None while the declared datasets can't be read.
pub fn makes_hi(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    let sets = x.tiles.zoom_sets(x.m)?;
    Some(if terrain::terrain_pack::makes_hi(x.cov, &sets, (u.x, u.y)) {
        Vec::new()
    } else {
        vec!["no hi tiles".into()]
    })
}

/// The deep mids a terrain z6 tile takes in (terrain::terrain_deep::mids_under), by content: its z12
/// tiles' quarters there.
pub fn terrain_deep_mids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(
        terrain::terrain_deep::mids_under(x.m, (u.x, u.y))
            .into_iter()
            .map(|(f, c)| format!("10/{}/{} {c}", f.0, f.1))
            .collect(),
    )
}

/// The declared pyramids read tile by tile (not terrarium) meeting a z6 tile, their lines as the
/// terrain's keys name them (inputs::dem::declared::meeting); None while the datasets can't be read.
pub fn pyramids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    use inputs::dem::declared::Data;
    let u = TileId::parse(t)?;
    let e = x.tiles.elevation(x.m).ok()??;
    let pyr: Vec<_> = e
        .iter()
        .filter(|(a, _)| matches!(&a.declaration.data, Data::Tiles { every_zoom: false, .. }))
        .cloned()
        .collect();
    Some(inputs::dem::declared::meeting(&pyr, roadcore::geo::tile_box(6, u.x, u.y)))
}

/// The terrain-deep targets over a z6 tile (terrain::terrain_deep::targets_in: whose base tiles the
/// pyramids' fill takes whole); None while the fine datasets can't be read.
pub fn deep_targets(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    let z = terrain::terrain_deep::PACK_Z;
    Some(x.tiles.deep(x.m, x.cov)?.targets_in((u.x, u.y)).iter().map(|(a, b)| format!("{z}/{a}/{b}")).collect())
}

/// The water source (terrain::terrain_pack::water_source_pin), "-" without one (no lake read): what
/// lake-depth reads of the water, whole.
pub fn water_source(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![terrain::terrain_pack::water_source_pin(x.m).unwrap_or_else(|| "-".into())])
}

/// The mids of a z3 tile's z6 tiles near the coverage (`buildstore::coverage::coverage_tiles`) of the terrain or
/// slope (`mid`: its mid's logical name), "6/x/y=<content>" ("-" for one not made).
fn near_mids(t: &str, x: &KeyCtx, mid: fn(u32, u32) -> String) -> Option<Vec<String>> {
    let q = TileId::parse(t).filter(|q| q.z == 3)?;
    let ts = buildstore::coverage::coverage_tiles(x.cov).get(&(q.x, q.y)).cloned().unwrap_or_default();
    Some(
        ts.into_iter().map(|(a, b)| format!("6/{a}/{b}={}", x.m.get(&mid(a, b)).map_or("-", String::as_str))).collect(),
    )
}

/// A terrain assembly's mids (`near_mids`).
pub fn terrain_mids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    near_mids(t, x, terrain::terrain_pack::mid_logical)
}

/// A slope assembly's mids (`near_mids`).
pub fn slope_mids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    near_mids(t, x, terrain::slope_pack::mid_logical)
}

/// A tree cover assembly's mids: those of its z6 tiles the coverage meets, "6/x/y=<content>" ("-"
/// for one not made).
pub fn trees_mids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let q = TileId::parse(t).filter(|q| q.z == 3)?;
    Some(
        (q.x * 8..(q.x + 1) * 8)
            .flat_map(|a| (q.y * 8..(q.y + 1) * 8).map(move |b| (a, b)))
            .filter(|&(a, b)| x.cov.meets_rect(codecs::hipack::tile_bounds(6, a, b)))
            .map(|(a, b)| {
                format!("6/{a}/{b}={}", x.m.get(&trees::treepacks::mid_logical(a, b)).map_or("-", String::as_str))
            })
            .collect(),
    )
}

/// The coverage inside a tree cover z6 tile (`Coverage::fingerprint` over it).
pub fn coverage_inside(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(vec![x.cov.fingerprint(codecs::hipack::tile_bounds(u.z, u.x, u.y))])
}

/// The canopy and leaf-type declarations a tree cover z6 tile reads where they aren't as staged
/// (inputs::inputs::sources::trees_extra), none where they are.
pub fn trees_sources(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(x.tiles.keyed(x.m).and_then(|k| k.trees.clone()).into_iter().collect())
}

/// The terrain tiles a slope tile reads by content (`build::slope_tile_lines`); None while a pack's
/// index can't be read.
pub fn slope_tiles(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    super::build::slope_tile_lines((u.z, u.x, u.y), x.m, x.tiles).ok()
}

/// A slope-hi z6 tile's z10 tiles' deep mids, by content (terrain::slope_pack::deep_mids_under: the
/// slope-deep runs' parts it takes in).
pub fn deep_mids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t).filter(|u| u.z == 6)?;
    let z = terrain::terrain_deep::PACK_Z;
    Some(
        terrain::slope_pack::deep_mids_under(x.m, (u.x, u.y))
            .into_iter()
            .map(|((a, b), c)| format!("{z}/{a}/{b}={c}"))
            .collect(),
    )
}

/// The terrain's root (`layers/terrain/root/0-0-0`) by content: the ancestor of every terrain tile
/// slope reads past its packs, and the slope's root's z0–2 where no z3 tile is.
pub fn terrain_root(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, ["layers/terrain/root/0-0-0".to_string()]))
}

/// The depth sources meeting boxes `bs` (their pins, in the sources' order); None while the
/// sources can't be read.
fn depth_pins(x: &KeyCtx, bs: &[[f64; 4]]) -> Option<Vec<String>> {
    let sources = x.tiles.depth(x.m)?;
    Some(vec![sources
        .iter()
        .filter(|s| bs.iter().any(|&b| s.meeting(b).next().is_some()))
        .map(|s| s.pin.as_str())
        .collect::<Vec<_>>()
        .join(",")])
}

/// The depth sources a depth target reads: a z6 tile's over its coarse levels' areas
/// (`terrain::depth::build::depth_hi_box_read`), an assembly's over its z3 tile with its blocks' margin
/// (`terrain::depth::build::depth_lo_box`).
pub fn depth_sources(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    match u.z {
        6 => depth_pins(x, &terrain::depth::build::depth_hi_box_read((u.x, u.y))),
        3 => depth_pins(x, &[terrain::depth::build::depth_lo_box((u.x, u.y))]),
        _ => None,
    }
}

/// The sources the lake-depth step makes (terrain::depth::lakes: the `lake-estimate` declarations),
/// their pins: what a lake-depth target reads of the declarations (their methods and versions,
/// which pin the files in their folders); None while they can't be read.
pub fn lake_sources(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let sources = x.tiles.depth(x.m)?;
    Some(sources.iter().filter(|s| s.made.is_some()).map(|s| s.pin.clone()).collect())
}

/// The lake-depth packs a depth target reads (terrain::depth::lakes::read_by), "<logical>=<content>"
/// each, "-" for none.
pub fn lake_packs(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    let tiles: std::collections::BTreeSet<(u32, u32)> =
        terrain::depth::lakes::read_by(u.z, u.x, u.y).into_iter().collect();
    let mut v: Vec<String> = terrain::depth::lakes::named(x.m)
        .filter(|(_, t, _)| tiles.contains(t))
        .map(|(id, t, c)| format!("{}={c}", terrain::depth::lakes::logical(id, t)))
        .collect();
    if v.is_empty() {
        v.push("-".into());
    }
    Some(v)
}

/// Depth target `t` and the enclosed seas the records name where its tiles meet one
/// (terrain::depth::surface::meets), Some(None) else; None while those can't be read (its key can't
/// be worked out then: it waits).
fn depth_on_enclosed(t: &str, x: &KeyCtx) -> Option<Option<((u8, u32, u32), std::sync::Arc<inputs::water::Enclosed>)>> {
    let u = TileId::parse(t)?;
    let t = (u.z, u.x, u.y);
    Some(x.tiles.enclosed(x.m)?.filter(|e| terrain::depth::surface::meets(e, t)).map(|e| (t, e)))
}

/// The enclosed seas' file a depth target reads, by content, where its tiles meet one; none else.
pub fn depth_enclosed(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let Some((t, e)) = depth_on_enclosed(t, x)? else { return Some(Vec::new()) };
    Some(terrain::depth::surface::reads(x.m, Some(&e), t).enclosed)
}

/// The terrain tiles a depth target reads an enclosed sea's surface from where its tiles meet one
/// (terrain::depth::surface::tiles_read), by their XXH3 in the lo packs' indexes ("-": not stored);
/// none else; None while an index can't be read.
pub fn depth_surface_terrain(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let Some((t, e)) = depth_on_enclosed(t, x)? else { return Some(Vec::new()) };
    let mut r = x.tiles.reader(x.m);
    terrain::depth::surface::tiles_read(&e, t)
        .into_iter()
        .map(|(z, a, b)| {
            Ok(format!("{z}/{a}/{b} {}", r.hash(z, a, b)?.map_or("-".to_string(), |h| format!("{h:016x}"))))
        })
        .collect::<Result<_, super::tiles::Unread>>()
        .ok()
}

/// The water step's target a depth target waits for where its tiles meet an enclosed sea (the
/// enclosed seas' file it reads); none else (steps::Read::Built).
pub fn depth_surface_water(t: &str, x: &KeyCtx) -> Vec<String> {
    depth_on_enclosed(t, x).flatten().map(|_| vec!["water".to_string()]).unwrap_or_default()
}

/// The terrain-lo targets a depth target waits for where its tiles meet an enclosed sea: the lo
/// packs holding the tiles its surface is read from (terrain::depth::surface::lo_packs); none else
/// (steps::Read::Built).
pub fn depth_surface_lo(t: &str, x: &KeyCtx) -> Vec<String> {
    depth_on_enclosed(t, x).flatten().map_or_else(Vec::new, |(t, e)| {
        terrain::depth::surface::lo_packs(&e, t).into_iter().map(|(a, b)| format!("3/{a}/{b}")).collect()
    })
}

/// The water a depth target reads (`build::water_key` of "depth <target>": a z6 tile's from
/// `build::DEPTH_WATER_FROM`, an assembly's `DEPTH_LO_WATER_FROM`); None without a water source or
/// while its digest isn't there.
pub fn water_depth(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    let pin = terrain::terrain_pack::water_source_pin(x.m)?;
    let idx = x.tiles.water_idx(x.m, &pin);
    let from = if u.z == 6 { super::build::DEPTH_WATER_FROM } else { super::build::DEPTH_LO_WATER_FROM };
    Some(vec![super::build::water_key(Some(&pin), idx, &format!("depth {t}"), from)?])
}

/// A polar block's (or a pole's root's) elevation datasets: those meeting its box
/// (`TerrainTiles::terrain_datasets`); None while they can't be told.
pub fn datasets_polar(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    use terrain::polar as p;
    let b = match p::parse_target(t) {
        Some((pole, b)) => roadcore::polar::lat_box(pole, p::BLOCK_Z, b.0, b.1),
        None => roadcore::polar::lat_box(roadcore::polar::Pole::of_letter(t)?, 0, 0, 0),
    };
    Some(lines_of("", &x.tiles.terrain_datasets(x.m, b)?))
}

/// A polar block's built neighbours' terrain packs (terrain::polar::around), by content ("-" for
/// one not made): the edges its shading reads.
pub fn polar_neighbours(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    use terrain::polar as p;
    let (pole, b) = p::parse_target(t)?;
    let built: std::collections::BTreeSet<(u32, u32)> = p::built(x.cov, pole).into_iter().collect();
    Some(named(
        x,
        p::around(pole, b).into_iter().filter(|n| built.contains(n)).map(|n| p::block_logical("terrain", pole, n)),
    ))
}

/// A pole's blocks the coverage builds (terrain::polar::built), as targets: what its root composes.
pub fn polar_blocks(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let pole = roadcore::polar::Pole::of_letter(t)?;
    Some(terrain::polar::built(x.cov, pole).into_iter().map(|b| terrain::polar::target(pole, b)).collect())
}

/// A pole's `kind` packs (`layers/<layer>/polar/`), by content.
fn polar_packs(kind: &str, t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let pole = roadcore::polar::Pole::of_letter(t)?;
    let pre = format!("layers/{}/polar/", terrain::polar::layer(kind, pole));
    Some(x.m.range(pre.clone()..).take_while(|(l, _)| l.starts_with(&pre)).map(|(l, c)| format!("{l}={c}")).collect())
}

/// A pole's terrain packs (`polar_packs`).
pub fn polar_terrain_packs(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    polar_packs("terrain", t, x)
}

/// A pole's shading packs (`polar_packs`).
pub fn polar_slope_packs(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    polar_packs("slope", t, x)
}

/// What a 3D buildings z6 tile's normalized file reads of the sources (`inputs` "bldprep 6/x/y":
/// its row groups and GHSL tile, bld::bld::sources); None before the sources are downloaded.
pub fn bld_rows(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    bld_sources(t, x).filter(|v| v[0] == "downloaded")?;
    Some(vec![x.inputs.get(&format!("bldprep {t}"))?.clone()])
}

/// The normalized files of a 3D buildings z6 tile and its 8 neighbours (`build::bld_around`), by
/// content.
pub fn bld_prepared(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(named(
        x,
        super::build::bld_around(u.x, u.y).into_iter().filter_map(|n| match n {
            (Some(a), Some(b)) => Some(bld::bld::work_logical(a, b)),
            _ => None,
        }),
    ))
}

/// The coverage's shapes over a 3D buildings z6 tile grown by 1 km, with their countries
/// (`Coverage::shapes_key`, as `build::bld_tiles` keeps them); None for a tile not meeting it.
pub fn coverage_shapes(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    super::build::bld_tiles(x.cov).iter().find(|(a, b, _)| (*a, *b) == (u.x, u.y))?.2.clone().map(|s| vec![s])
}

/// The newest pass's heritage slices of a 3D buildings z6 tile: its sites and its World Heritage
/// components (for the field's heritage lists).
pub fn bld_heritage(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(match landmarks::heritage::latest_date(x.m) {
        Some(d) => {
            named(x, [landmarks::heritage::pos_logical(&d, u.x, u.y), landmarks::heritage::parts_logical(&d, u.x, u.y)])
        }
        None => vec!["-".into()],
    })
}

/// The 3D buildings' fields' mids of a z3 tile's z6 tiles, by content.
pub fn bld_mids(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let q = TileId::parse(t).filter(|q| q.z == 3)?;
    Some(named(
        x,
        (q.y * 8..q.y * 8 + 8).flat_map(|b| (q.x * 8..q.x * 8 + 8).map(move |a| bld::bld::field::mid_logical(a, b))),
    ))
}

/// The rail feeds declared (`inputs` "feeds"), "-" without; None while they can't be read ("?").
pub fn feeds_declared(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let d = x.inputs.get("feeds").map_or("-", String::as_str);
    (d != "?").then(|| vec![d.to_string()])
}

/// Of the keys inputs/keys.env holds (`inputs` "keys"), those the feeds' declarations fetch with
/// ("feed-keys"); None while either can't be read ("?"). Requires the feeds input accepted.
pub fn feed_keys(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    x.m.get(&inputs::inputs::logical(inputs::inputs::feeds::UNIT))?;
    let keys = x.inputs.get("keys").map_or("", String::as_str);
    let used = x.inputs.get("feed-keys").map_or("", String::as_str);
    if keys == "?" || used == "?" {
        return None;
    }
    let used: Vec<&str> = used.split(',').filter(|k| !k.is_empty()).collect();
    Some(vec![keys.split(',').filter(|k| used.contains(k)).collect::<Vec<_>>().join(",")])
}

/// The feeds' catalogue (inputs::rail::CATALOGUE) and the pass's outlines, as made.
pub fn feeds_reads(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(named(x, [inputs::rail::CATALOGUE.to_string(), format!("sources/osm/{}/outlines", x.date)]))
}

/// The rail feeds fetched (inputs::rail::FEEDS), required.
pub fn rail_feeds(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [inputs::rail::FEEDS.to_string()])
}

/// The accepted static timetables (docs/inputs.md §6.7.2) and the pass's rail set, required.
pub fn rail_reads(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    required(x, [inputs::inputs::logical(inputs::inputs::gtfs::UNIT), inputs::osmpass::set_name(x.date, "rail")])
}

/// What the road → z6 tiles index reads (`build::roadunits_reads`: each z6 tile's road ids, or
/// its road values built before them); None before any is made.
pub fn road_ids(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let v: Vec<String> = super::build::roadunits_reads(x.m).into_iter().map(|(_, l, c)| format!("{l}={c}")).collect();
    (!v.is_empty()).then_some(v)
}

/// The z6 tiles built (landmarks::ovconv::cover_of their base packs), as rail stops keep them; None
/// before any.
pub fn built_tiles(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let v: Vec<String> = landmarks::ovconv::cover_of(
        x.m.range("base/".to_string()..).take_while(|(l, _)| l.starts_with("base/")).map(|(l, _)| l),
    )
    .iter()
    .map(|(a, b)| format!("6/{a}/{b}"))
    .collect();
    (!v.is_empty()).then_some(v)
}

/// The pass's rail set, required.
pub fn set_rail(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    set("rail", x)
}

/// What the ferries read: the pass's ferries set, the ferry feeds' list (inputs::rail::FERRY_FEEDS)
/// and the static timetables' accepted index, each required, once a z6 tile is built.
pub fn ferries_reads(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    built_tiles(t, x)?;
    let mut v = set("ferries", x)?;
    v.extend(required(
        x,
        [inputs::rail::FERRY_FEEDS.to_string(), inputs::inputs::logical(inputs::inputs::gtfs::UNIT)],
    )?);
    Some(v)
}

/// The tiles of the terrain's root (z0–2) a slope tile reads by content (`build::root_tile_lines`:
/// an edge neighbour's ancestor where its z3 tile has no lo pack); none for most. Those reading
/// one wait while the root is stale (the planner's `Left::unknown_for`), the rest not at all.
pub fn root_tiles(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    Some(super::build::root_tile_lines((u.z, u.x, u.y), x.m, x.tiles))
}
