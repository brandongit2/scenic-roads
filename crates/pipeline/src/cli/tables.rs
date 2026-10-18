//! The programs' tables of commands, which their dispatchers match (crate::cli). A command added
//! to a dispatcher and not here is refused as unknown; one here is run through by the tests
//! (tests/cli.rs) with `--help` and an unknown flag.

use super::{Cmd, Table, ANY};

const fn c(
    name: &'static str,
    args: &'static str,
    about: &'static str,
    flags: &'static [&'static str],
    opts: &'static [&'static str],
    pos: usize,
) -> Cmd {
    Cmd { name, args, about, flags, opts, pos }
}

const H: &[&str] = &["--home"];
const R: &[&str] = &["--root"];
const HR: &[&str] = &["--home", "--root"];

/// `scenic`, the user's command (src/bin/scenic.rs).
pub const SCENIC: Table = Table {
    prog: "scenic",
    about: "the build and its pool, from this Mac (docs/plan.md §1, §8). --root <dir>: the NAS project folder (default: the mounted share); --home <dir>: this Mac's agent's folder",
    default: Some("status"),
    common: &[],
    cmds: &[
        c("status", "[--root <dir>] [--home <dir>]", "what the lead is doing, what waits and why, the build to the end by stage and step, regions", &[], HR, 0),
        c("steps", "[--json]", "the steps table (docs/pipeline.md): every step's title, what it does, its kind, scope, stage and where it runs; --json as the build page reads it", &["--json"], &[], 0),
        c("agent", "[--once] [--dry-run] [--home <dir>] [--root <dir>]", "the build agent (every Mac's login item, a member of the pool)", &["--once", "--dry-run", "--helper"], HR, 0),
        c("pool", "[--root <dir>]", "how the pool stands (docs/pool.md)", &[], R, 0),
        c("pool status", "[--root <dir>]", "how the pool stands", &[], R, 0),
        c("pool release", "<member> \"<job>\" [--root <dir>]", "a job's progress on a member's Mac given up: the lead runs it wherever it fits", &[], R, 2),
        c("pool floors", "[--clear [<step> [<target>]]] [--root <dir>]", "the targets' memory floors the lead's coordinator keeps; --clear clears them (all, a step's or one target's)", &["--clear"], R, 2),
        c("pool nas-probe", "[--dir <folder>] [--root <dir>]", "the NAS budget's probe once (pipeline::budget::nas): small files made, a larger one written and read back, in a folder of its own (made, then removed), printing MB/s and creates/s", &[], &["--dir", "--root"], 0),
        c("pool needs", "--fit [--costs <costs.json | costs.jsonl>] [--root <dir>]", "each step's model of what its jobs need (pipeline::needs): its fits' errors on held-out runs (memory, time, disk) and the steps that don't fit; read only, this Mac's coordinator's costs unless --costs", &["--fit"], &["--costs", "--root"], 0),
        c("run-task", "--spec <file> --lease <id> --dir <folder> --result <file> [--root <dir>]", "a task a member's agent leased, run here (the agent's)", &[], &["--spec", "--lease", "--dir", "--result", "--root"], 0),
        c("lead", "[--home <dir>]", "who leads the build, its term, since when, a handover under way, each member and whether it can lead (docs/pool.md §11)", &[], HR, 0),
        c("lead status", "[--home <dir>]", "the same", &[], HR, 0),
        c("lead give", "<member> [--home <dir>]", "hand the lead to a member (its host name or member id)", &[], HR, 1),
        c("lead take", "[--force] [--downgrade] [--home <dir>]", "this Mac takes the lead over: once the lead is out of touch or stood down; --force with it in touch; --downgrade on an app older than the term's", &["--force", "--downgrade"], HR, 0),
        c("lead auto", "[on|off] [--root <dir>]", "the proactive offer taken by itself (the lead away or on battery five minutes, another Mac home on power), or how it stands", &[], HR, 1),
        c("pause", "[--now] [--home <dir>]", "pause the whole build: every Mac's jobs stop at their next safe point; --now: frozen at once", &["--now"], H, 0),
        c("resume", "[--home <dir>]", "let the build go on after `scenic pause`", &[], H, 0),
        c("job cancel", "<target> [--step <step>] [--rerun-after-update] [--home <dir>]", "stop the running job building <target> (on this Mac or, through the lead, another) and give it back, not failed: no retry delay, nothing kept from its Mac, its scratch for the target cleared, so it runs again from the start; --rerun-after-update: held until the lead runs a newer app than this one", &["--rerun-after-update"], &["--step", "--home", "--root"], 1),
        c("clean", "[--yes] [--home <dir>]", "clear this Mac's build caches once the build is done, after a y/N (--yes: none): its agent does it, and says what it freed", &["--yes"], H, 0),
        c("room", "[<GB> | off] [--home <dir>]", "this Mac's disk room target: the free space its agent keeps, freeing its caches to it; with none, how it stands", &[], HR, 1),
        c("in-use", "[--cores <n>] [--memory <GB>] | off [--home <dir>]", "this Mac's in-use share: the cores and memory its agent keeps free of its jobs while its owner uses it (by default 2 cores and five eighths of its memory); off: the defaults; with none, how it stands", &[], &["--cores", "--memory", "--home", "--root"], 1),
        c("gc", "[--dry-run] [--days 14] [--root <dir>]", "remove replaced files from the NAS (the agent runs it daily)", &["--dry-run"], &["--days", "--root"], 0),
        c("backup", "[--local <dir>] [--root <dir>]", "back up the user's folders (the agent runs it daily)", &[], &["--local", "--root"], 0),
        c("timings", "[<kind>] [--last N] [--host <mac>] [--here | --file <f>] [--json]", "the jobs' timings: each kind's phases over its last N runs (20), with their totals and shares; the build's, or with --here this Mac's own jobs' alone", &["--here", "--json"], &["--last", "--host", "--file", "--home", "--root"], 1),
        c("timings publish", "[--last N] [--file <f>] [--json] [--root <dir>] [--home <dir>]", "publishing end to end over the last N catalogs (10): each unit from its job's end to its hand-off merged, its map tiles planned, leased and packed, the catalog (delta or whole, the files it stat'ed), each Mac's server adopting it and its mirror through it; the map tiles each unit made stale, the steps that ran per catalog, and the medians", &["--json"], &["--last", "--file", "--home", "--root"], 0),
        c("inputs", "[--root <dir>] [--home <dir>]", "every gate unit (docs/inputs.md §4): its accepted version, its state, what holds it, its acceptances", &[], HR, 0),
        c("inputs list", "[--root <dir>] [--home <dir>]", "the same", &[], HR, 0),
        c("inputs check", "[<unit>] [--full]", "the drop boxes listed now (the lead's agent checks what changed; --full: every file hashed)", &["--full"], HR, 1),
        c("inputs accept", "<unit> <finding id>|--all", "a held warning accepted (or all the unit's), written from this Mac; errors can't be", &["--all"], HR, 2),
        c("inputs unaccept", "<unit> <finding id>|--stale", "an acceptance undone (or every stale one)", &["--stale"], HR, 2),
        c("inputs test", "[on|off] [--forget]", "the test unit `_gate-test` on the gate, or off it (--forget: its acceptances and records removed), or how it stands", &["--forget"], HR, 1),
    ],
};

const PASS: &[&str] = &["--pass"];
const PASS_DEM: &[&str] = &["--pass", "--dem"];
const PASS_REGIONS: &[&str] = &["--pass", "--regions"];

/// `scenic-build`, the build's steps (src/bin/scenic-build.rs; its doc comment says more of each).
pub const SCENIC_BUILD: Table = Table {
    prog: "scenic-build",
    about: "the build's steps, writing to the NAS project folder (docs/plan.md §6). Every step takes --root <nas project folder> and --scratch <local dir>",
    default: None,
    common: &["--root", "--scratch"],
    cmds: &[
        c("pack", "[--cache dir] [--mirror dir] [--pass date] [T …]", "road and rail hi packs (z9–14) and hidata of z6 tiles T (default: every tile with ways), opening only T's owner units (by the pass's reaches)", &[], &["--cache", "--mirror", "--pass"], ANY),
        c("lo", "[--cache dir] [--mirror dir] [--pass date] [Q …]", "lo packs (z4–8 road and rail tiles) of z3 tiles Q (default: all), opening only Q's owner units (by the pass's reaches)", &[], &["--cache", "--mirror", "--pass"], ANY),
        c("osm-pass", "--planet <p> --date <d> [--extract f] [--planetiler f] [--clear dir] …", "the OSM pass (pieces, sets, basemap, road values); resumable. --clear: a cache folder cleared first (one per --clear, repeated for more)", &[], &["--planet", "--date", "--extract", "--planetiler", "--clear"], 0),
        c("outlines", "[--pass d]", "the pass's administrative and ISO 3166 outlines from its outline set", &[], PASS, 0),
        c("patch-ferries", "[--pass d] [--near-coverage] [--regions dir]", "the pass's pieces given the standalone ferry ways its filtered planet lacked; run by hand", &["--near-coverage"], PASS_REGIONS, 0),
        c("pass-sets", "[--pass d]", "the pass's sets it lacks in their current filters", &[], PASS, 0),
        c("trailends", "[--pass d]", "the pass's hiking routes' ends", &[], PASS, 0),
        c("unit", "[U …] [--pass d] [--layers-root r] [--regions dir] [--dem dir] [--cache-dir dir] [--buildings dir]", "base(U) from the pass's pieces: default every unit whose piece meets the coverage", &[], &["--pass", "--layers-root", "--regions", "--dem", "--cache-dir", "--buildings"], ANY),
        c("unit-snap", "U --out d --cache c [--carry] [--pass d] [--regions dir] [--dem dir]", "unit U's folder built from the records into d, nothing written to the NAS, snapshotted around each step's program", &["--carry"], &["--out", "--cache", "--pass", "--regions", "--dem"], 1),
        c("tail-spec", "U", "unit U's tail as a task gives it a worker (tools/check/tail.mjs); nothing read or written", &[], &[], 1),
        c("pois", "[U …] [--pass d] [--regions dir]", "the units' landmark candidates", &[], PASS_REGIONS, ANY),
        c("summits", "[--pass d] [--cache dir]", "the world's summits", &[], &["--pass", "--cache"], 0),
        c("peaks", "[U …] [--pass d] [--cache dir] [--raw dir] [--coarse-threads n]", "the units' peaks", &[], &["--pass", "--cache", "--raw", "--coarse-threads"], ANY),
        c("marks", "[--pass d] [--regions dir] [--facts f] [--views f]", "the landmarks", &[], &["--pass", "--regions", "--facts", "--views"], 0),
        c("items", "[--pass d] [--regions dir] [--dem dir] [--cache dir]", "the landmarks' Wikidata items", &[], &["--pass", "--regions", "--dem", "--cache"], 0),
        c("heritage", "[--pass d] [--regions dir] [--dem dir] [--cache dir]", "the heritage chain", &[], &["--pass", "--regions", "--dem", "--cache"], 0),
        c("heritage-sites", "[--pass d] [--regions dir] [--dem dir] [--cache dir] [--heritage-index i] [--heritage-matches-index i]", "the heritage sites", &[], &["--pass", "--regions", "--dem", "--cache", "--heritage-index", "--heritage-matches-index"], 0),
        c("overlays", "[--pass d]", "the overlays", &[], PASS, 0),
        c("roadunits", "", "the road → units index from every unit's road values", &[], &[], 0),
        c("terrain", "[T …] [--regions dir] [--pass d] [--raw dir] [--expect-same T,…] [--pieces-of Q,…]", "the terrain of z6 tiles T near the coverage (pieces), or of z3 tiles' whole (default: all of them)", &[], &["--regions", "--pass", "--raw", "--expect-same", "--pieces-of"], ANY),
        c("terrain-lo", "<Q …> [--raw dir] [--regions dir] [--pass d]", "z3 tiles Q's zoomed-out terrain (lo packs, z3–8) from their pieces' mids", &[], &["--raw", "--regions", "--pass"], ANY),
        c("terrain-z8", "[--raw dir]", "coarse terrain for the whole world", &[], &["--raw"], 0),
        c("terrain-root", "[--raw dir]", "the terrain's z0–2 root packs, from the lo packs' z3 tiles", &[], &["--raw"], 0),
        c("slope", "[T …] [--regions dir] [--pass d] [--expect-same T,…] [--pieces-of Q,…]", "the slope from the terrain packs: of z6 tiles T, or of z3 tiles' whole (default: all of them)", &[], &["--regions", "--pass", "--expect-same", "--pieces-of"], ANY),
        c("slope-lo", "<Q …> [--regions dir] [--pass d]", "z3 tiles Q's zoomed-out slope (lo packs, z3–8)", &[], PASS_REGIONS, ANY),
        c("slope-root", "", "the slope's z0–2 root packs", &[], &[], 0),
        c("raw-pack", "[--from-tar - [--expect n]] [--cache dir] | --order | --check [--every n]", "AWS's raw tiles packed into the NAS's archives: a tar of them on stdin, else those waiting in the cache", &["--order", "--check"], &["--from-tar", "--expect", "--cache", "--every"], 0),
        c("prune", "<target …>", "drops from the manifest what the coverage no longer builds (\"unit U\", \"pois U\", \"pack T\", \"lo Q\", \"bldprep T\", \"bldtiles T\")", &[], &[], ANY),
        c("buildings", "[--dem dir] [--workers n]", "the world's roadside buildings: the declared release, in z8 tiles, onto the NAS with their index", &[], &["--dem", "--workers"], 0),
        c("bld-fetch", "[--pass d] [--regions dir] [--dem dir]", "the 3D buildings' sources onto the NAS (dem/bldfetch.py)", &[], &["--pass", "--regions", "--dem"], 0),
        c("bldprep", "<T …> [--dem dir]", "the 3D buildings' normalized files of z6 tiles T", &[], &["--dem"], ANY),
        c("bldtiles", "<T …> [--pass d] [--regions dir]", "the 3D buildings' tiles of z6 tiles T", &[], PASS_REGIONS, ANY),
        c("bldfield-lo", "<Q …>", "z3 tiles Q's zoomed-out buildings field", &[], &[], ANY),
        c("depth-lo", "<Q …>", "z3 tiles Q's zoomed-out depth (lo packs, z3–8) from the declared depth sources", &[], &[], ANY),
        c("depth", "<T …>", "z6 tiles T's depth (hi packs, z9–16) where a depth source is finer than z8", &[], &[], ANY),
        c("bldtile-task", "<8/x/y | 6/x/y> --out dir [--pass d] [--regions dir]", "a z8 area's task folder as a bldtiles job cuts it", &[], &["--out", "--pass", "--regions"], 1),
        c("trees", "<T …> [--pass d] [--regions dir] [--dem dir] [--chm dir] [--expect-same T,…]", "the tree cover layers, clipped to the coverage: of z6 tiles T, or a z3 tile's whole", &[], &["--pass", "--regions", "--dem", "--chm", "--expect-same"], ANY),
        c("trees-lo", "<Q …> [--pass d] [--regions dir]", "z3 tiles Q's zoomed-out tree cover (lo packs) from their pieces' mids", &[], PASS_REGIONS, ANY),
        c("trees-coverage", "<Q> --out <file> [--pass d] [--regions dir]", "the coverage the trees program reads for z3 tile Q; nothing written to the NAS", &[], &["--out", "--pass", "--regions"], 1),
        c("treeblock-task", "<6/x/y> --row y --out dir [--pass d] [--regions dir]", "a row of tree cover blocks' task folder as a piece's run cuts it; nothing written to the NAS", &[], &["--row", "--out", "--pass", "--regions"], 1),
        c("terrainsub-task", "<6/x/y> --out dir [--group 8/x/y] [--raw dir] [--pass d] [--regions dir]", "a terrain piece's z8 subtrees' task files; nothing written to the NAS", &[], &["--out", "--group", "--raw", "--pass", "--regions"], 1),
        c("reach", "[--pass d] [U …]", "every unit's reach (units named: printed, nothing written)", &[], PASS, ANY),
        c("labels", "[--pass d] [--dem dir]", "the labels by importance, worldwide, as the labels layer's packs", &[], PASS_DEM, 0),
        c("spoken", "[--pass d] [--languages-index i]", "the languages spoken where, for names", &[], &["--pass", "--languages-index"], 0),
        c("lakefill", "[--pass d]", "Canada's lakes missing from OpenStreetMap filled in", &[], PASS, 0),
        c("water", "[--pass d] [--basemap f] [--only x/y,…] [--archive f] [--lakefill f]", "the water layer, z0–9 (--archive: into that local file, the NAS untouched)", &[], &["--pass", "--basemap", "--only", "--archive", "--lakefill"], 0),
        c("terrain-water", "[--pass d] [--regions dir]", "the terrain's water digests", &[], PASS_REGIONS, 0),
        c("water-key-check", "[--pass d] [--compute [--save f] | --digests f] [--fill n]", "what keying terrain on its water's digests does with the records now. Reads only", &["--compute"], &["--pass", "--save", "--digests", "--fill"], 0),
        c("stations", "--pass d [--geojson f]", "the rail stops of the pass's rail set within the coverage, as the stations' tiles", &[], &["--pass", "--geojson"], 0),
        c("ferries", "--pass d [--dem dir] [--gtfs-index i]", "the pass's ferries set through ferries.py (with the ferry feeds and the static timetables of index i, else the accepted one), as the ferries' blocks", &[], &["--pass", "--dem", "--gtfs-index"], 0),
        c("rail-feeds", "[--pass d] [--regions dir] [--dem dir] [--feeds-index i]", "the timetable feeds (the declarations of index i, else the accepted one): the coverage's rail feeds and the ferry feeds, each fetched once into the rail sources", &[], &["--pass", "--regions", "--dem", "--feeds-index"], 0),
        c("rail", "[--pass d] [--regions dir] [--dem dir] [--cache dir] [--gtfs-index i]", "trains a day on the coverage's rail ways (the feeds and the static timetables of index i, else the accepted one), as global/railfreq", &[], &["--pass", "--regions", "--dem", "--cache", "--gtfs-index"], 0),
        c("regions-extract", "--pass d --ids r<id>,w<id>,…", "regions' outlines cut from the pass", &[], &["--pass", "--ids"], 0),
        c("regions-fixture", "<id> <lon,lat,km>", "an accepted version of one region, a circle, in a scratch folder (never the project folder)", &[], &[], 2),
        c("names-todo", "--translations-index i --descriptions-index i [--out dir]", "the translations' and descriptions' to-do lists from the newest catalog", &[], &["--translations-index", "--descriptions-index", "--out"], 0),
        c("timetables-todo", "[--pass d] [--regions dir] [--out dir]", "the timetables' to-do list (into the static timetables' todo/ under the root, or --out) and the report of routes matching nothing on the map, from the rail and ferries jobs' pieces", &[], &["--pass", "--regions", "--out"], 0),
        c("inputs", "<unit> [--full] [--listing f] [--dem dir] [--forget]", "a gate unit's drop box checked: its clean changes taken in, the held ones reported", &["--full", "--forget"], &["--listing", "--dem"], 1),
        c("put", "<logical> <ext> <file>", "upload a file under a logical name", &[], &[], 3),
        c("verify", "", "check every unverified upload on the NAS (SHA-256 over SSH)", &[], &[], 0),
        c("catalog", "[--held] [--ready <ids>] [--delta] [--whole]", "publish a catalog of the build manifest (--held: into catalog-held/)", &["--held", "--delta", "--whole"], &["--ready"], 0),
        c("rekey-check", "[--pass d]", "what re-keying the records for the units' new keys would do now. Reads only", &[], PASS, 0),
        c("fabdem-fetch", "--mirror <HF dataset URL> --work <local dir> [--first <square>,…]", "every FABDEM tile the store (sources/fetched/fabdem/) lacks, from the mirror, one at a time, the 10° squares named first; restartable", &[], &["--mirror", "--work", "--first"], 0),
        c("p5-check", "terrain|trees [--pass d] [--costs f]", "what switching terrain and slope, or tree cover, to pieces and assemblies does now. Reads only", &[], &["--pass", "--costs"], 1),
    ],
};

/// `scenic-metrics <unit_dir> <step>`: checked as `<step> <unit_dir>` (src/bin/scenic-metrics.rs).
pub const SCENIC_METRICS: Table = Table {
    prog: "scenic-metrics",
    about: "scenic analysis of a unit's roads (pipeline::unit); usage: scenic-metrics <unit_dir> <step>",
    default: None,
    common: &[],
    cmds: &[
        c("prep", "<unit_dir>", "road sample points with observer eye heights; per-vertex drape heights", &[], &[], 1),
        c("canopy", "<unit_dir>", "canopy heights: grid layers, near-field horizons, roadside trees", &[], &[], 1),
        c("view", "<unit_dir>", "far-field viewshed and landscape metrics", &[], &[], 1),
        c(
            "buildings",
            "<unit_dir> <dir>",
            "roadside buildings per sample from the footprints (the folder given)",
            &[],
            &[],
            2,
        ),
        c("flags", "<unit_dir>", "the flag channels again on the view's metrics", &[], &[], 1),
    ],
};

/// `lakefill` (src/bin/lakefill.rs).
pub const LAKEFILL: Table = Table {
    prog: "lakefill",
    about: "Canada's lakes missing from OSM, sheet by sheet: the lakefill step's parts, and checks of a fill (scratch folders only)",
    default: None,
    common: &[],
    cmds: &[
        c("survey", "<basemap.pmtiles> <outlines.sect> <out.csv>", "every NTS 1:50,000 sheet across the Canadian regions, with the basemap's z12 water in it", &[], &[], 3),
        c("gaps", "<sheets.csv> <out.csv>", "the survey's gap sheets", &[], &[], 2),
        c("fill", "--basemap <pmtiles> --sheets <gap-sheets.csv> --source <dir|zip>… [--band w,e] [--no-complete-halves] [--tol <m>] [--confirm <share>] --out <dir>", "the fill of those sheets from the lake-fill source's shapefiles", &["--no-complete-halves"], &["--basemap", "--sheets", "--source", "--band", "--tol", "--confirm", "--out"], 0),
        c("merge", "--out <dir> <run dir>…", "bands' fills as one", &[], &["--out"], ANY),
        c("all", "--basemap <pmtiles> --outlines <sect> --source <dir> --work <dir>", "all of it, as the step runs it", &[], &["--basemap", "--outlines", "--source", "--work"], 0),
        c("view", "<basemap.pmtiles> <w,s,e,n> <z> <out.png>", "a box of the basemap's water with the sheets' edges", &[], &[], 4),
        c("catalog-add", "--root <test root> --fill <f>", "a test root's newest catalog again, one newer, with the fill. Never the NAS's", &[], &["--root", "--fill"], 0),
        c("stats", "--fill <f> --sheets <gap-sheets.csv>", "the fill's lakes and their area by region, as JSON", &[], &["--fill", "--sheets"], 0),
        c("seamcheck", "--fill <f> --basemap <pmtiles> [--max n]", "where the fill's lakes were cut at a sheet's edge and OSM's water lies across it, as JSON", &[], &["--fill", "--basemap", "--max"], 0),
    ],
};

/// `coastcheck` (src/bin/coastcheck.rs).
pub const COASTCHECK: Table = Table {
    prog: "coastcheck",
    about: "the shoreline check's reference (tools/coastcheck/README.md)",
    default: None,
    common: &[],
    cmds: &[
        c(
            "build",
            "--set <water set .osm.pbf> --water-polygons <zip> --out <store folder> [--index <type>]",
            "the water at full detail into a store",
            &[],
            &["--set", "--water-polygons", "--out", "--index"],
            0,
        ),
        c(
            "serve",
            "--store <store folder> [--port 18095]",
            "the store's coverage served as raster tiles",
            &[],
            &["--store", "--port"],
            0,
        ),
        c(
            "tile",
            "--store <store folder> <z> <x> <y> [--size 512]",
            "one coverage tile as a PNG on stdout",
            &[],
            &["--store", "--size"],
            3,
        ),
        c(
            "stats",
            "--store <store folder> --z <z> [--sample n] [--size n]",
            "how big a coverage layer would be",
            &[],
            &["--store", "--z", "--sample", "--size"],
            0,
        ),
        c(
            "water-packs",
            "--tiles <dir> --catalog <n.json.zst> --root <dir>",
            "water-build's tiles as the water layer's packs in a test root. Never the NAS's",
            &[],
            &["--tiles", "--catalog", "--root"],
            0,
        ),
        c(
            "water-build",
            "--basemap <pmtiles> [--only x/y,…] [--lakefill f] --out <dir>",
            "pipeline::water's build into a folder",
            &[],
            &["--basemap", "--only", "--lakefill", "--out"],
            0,
        ),
        c(
            "mark-sea",
            "--store <store folder> --water-polygons <zip>",
            "the sea's rings marked in a store",
            &[],
            &["--store", "--water-polygons"],
            0,
        ),
    ],
};

/// A command line as a job runs it (env settings and wrappers first, the program by its path):
/// None when it runs none of these programs, else its check (the program's table's).
pub fn check_command(cmd: &[String]) -> Option<Result<(), String>> {
    let is = |a: &str, t: &Table| std::path::Path::new(a).file_name().is_some_and(|f| f == t.prog);
    let at = cmd.iter().position(|a| ALL.iter().any(|t| is(a, t)))?;
    let t = ALL.iter().find(|t| is(&cmd[at], t))?;
    let mut rest: Vec<String> = cmd[at + 1..].to_vec();
    // (scenic-metrics: its unit folder first, its step second.)
    if t.prog == "scenic-metrics" && rest.len() >= 2 {
        rest.swap(0, 1);
    }
    Some(match t.check(&rest) {
        Ok(super::Checked::Run(_)) => Ok(()),
        Ok(super::Checked::Help(_)) => Err(format!("{cmd:?}: asks for help")),
        Err(e) => Err(format!("{cmd:?}: {e}")),
    })
}

/// Every program's table, for the tests.
pub const ALL: [&Table; 5] = [&SCENIC, &SCENIC_BUILD, &SCENIC_METRICS, &LAKEFILL, &COASTCHECK];
