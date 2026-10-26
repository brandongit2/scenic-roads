//! `scenic-build`, the build's steps (src/bin/scenic-build.rs; its doc comment says more of each):
//! a table of the steps, each with the flags and arguments it takes, as clap's command. The steps
//! read their flags by name from the line clap checked (`opt` in scenic-build.rs), and their
//! arguments through `targets`; `--root` and `--scratch` every step takes.

use clap::{Arg, ArgAction, Command};

/// A step: its name, its usage after it, what it does, its flags that take no value, its flags
/// followed by a value, and how many arguments it takes at most (`ANY`: no limit).
struct Step {
    name: &'static str,
    usage: &'static str,
    about: &'static str,
    flags: &'static [&'static str],
    opts: &'static [&'static str],
    pos: usize,
}

/// No limit on a step's arguments.
const ANY: usize = usize::MAX;

/// Flags given as often as there are values (`--clear a --clear b`); every other one once.
const REPEATED: &[&str] = &["--clear"];

const fn s(
    name: &'static str,
    usage: &'static str,
    about: &'static str,
    flags: &'static [&'static str],
    opts: &'static [&'static str],
    pos: usize,
) -> Step {
    Step { name, usage, about, flags, opts, pos }
}

const PASS: &[&str] = &["--pass"];
const PASS_REGIONS: &[&str] = &["--pass", "--regions"];

const STEPS: &[Step] = &[
    s("pack", "[--cache dir] [--mirror dir] [--pass date] [T …]", "road and rail hi packs (z9–14) and hidata of z6 tiles T (default: every tile with ways), opening only T's owner units (by the pass's reaches)", &[], &["--cache", "--mirror", "--pass"], ANY),
    s("lo", "[--cache dir] [--mirror dir] [--pass date] [Q …]", "lo packs (z4–8 road and rail tiles) of z3 tiles Q (default: all), opening only Q's owner units (by the pass's reaches)", &[], &["--cache", "--mirror", "--pass"], ANY),
    s("osm-pass", "--planet <p> --date <d> [--extract f] [--planetiler f] [--clear dir] …", "the OSM pass (z6 tile extracts, sets, basemap, road values); resumable. --clear: a cache folder cleared first (one per --clear, repeated for more). Keeps free the room target SCENIC_ROOM_TARGET_MB says (the agent sets it); unset, as run by hand, no target", &[], &["--planet", "--date", "--extract", "--planetiler", "--clear"], 0),
    s("outlines", "[--pass d]", "the pass's administrative and ISO 3166 outlines from its outline set", &[], PASS, 0),
    s("patch-ferries", "[--pass d] [--near-coverage] [--regions dir]", "the pass's extracts given the standalone ferry ways its filtered planet lacked; run by hand", &["--near-coverage"], PASS_REGIONS, 0),
    s("pass-sets", "[--pass d]", "the pass's sets it lacks in their current filters; copies the filtered planet here only with room past the room target SCENIC_ROOM_TARGET_MB says (the agent sets it); unset, as run by hand, no target", &[], PASS, 0),
    s("trailends", "[--pass d]", "the pass's hiking routes' ends", &[], PASS, 0),
    s("unit", "[U …] [--pass d] [--layers-root r] [--regions dir] [--cache-dir dir] [--buildings dir]", "base(U) from the pass's extracts: default every z6 tile whose extract meets the coverage", &[], &["--pass", "--layers-root", "--regions", "--cache-dir", "--buildings"], ANY),
    s("unit-snap", "U --out d --cache c [--carry] [--pass d] [--regions dir]", "unit U's folder built from the records into d, nothing written to the NAS, snapshotted around each step's program", &["--carry"], &["--out", "--cache", "--pass", "--regions"], 1),
    s("tail-spec", "U --root <nas project folder>", "unit U's tail as a task gives it a worker (tools/check/tail.mjs); the accepted declarations read for its data servers, nothing written", &[], &[], 1),
    s("pois", "[U …] [--pass d] [--regions dir]", "the units' landmark candidates", &[], PASS_REGIONS, ANY),
    s("summits", "[--pass d] [--cache dir]", "the world's summits", &[], &["--pass", "--cache"], 0),
    s("peaks", "[U …] [--pass d] [--cache dir] [--coarse-threads n]", "the units' peaks", &[], &["--pass", "--cache", "--coarse-threads"], ANY),
    s("marks", "[--pass d] [--regions dir] [--facts f] [--views f]", "the landmarks", &[], &["--pass", "--regions", "--facts", "--views"], 0),
    s("items", "[--pass d] [--regions dir] [--cache dir]", "the landmarks' Wikidata items", &[], &["--pass", "--regions", "--cache"], 0),
    s("heritage", "[--pass d] [--regions dir] [--dem dir] [--cache dir]", "the heritage chain", &[], &["--pass", "--regions", "--dem", "--cache"], 0),
    s("heritage-sites", "[--pass d] [--regions dir] [--dem dir] [--cache dir] [--heritage-index i] [--heritage-matches-index i]", "the heritage sites", &[], &["--pass", "--regions", "--dem", "--cache", "--heritage-index", "--heritage-matches-index"], 0),
    s("overlays", "[--pass d]", "the overlays", &[], PASS, 0),
    s("roadunits", "", "the road → units index from the units' road ids", &[], &[], 0),
    s("terrain-hi", "[T …] [--regions dir] [--pass d] [--raw dir] [--expect-same T,…] [--pieces-of Q,…]", "the terrain hi packs (z9–12) of z6 tiles T near the coverage, or of z3 tiles' whole (default: all of them)", &[], &["--regions", "--pass", "--raw", "--expect-same", "--pieces-of"], ANY),
    s("terrain-deep", "<10/x/y …> [--regions dir] [--pass d] [--raw dir]", "z10 tiles' terrain past the base's zoom (deep packs, z13–16) where a declared elevation dataset has the detail, from the base made from the elevation pool", &[], &["--regions", "--pass", "--raw"], ANY),
    s("terrain-lo", "<Q …> [--raw dir] [--regions dir] [--pass d]", "z3 tiles Q's zoomed-out terrain (lo packs, z3–8) from their z6 tiles' mids", &[], &["--raw", "--regions", "--pass"], ANY),
    s("terrain-root", "[--raw dir]", "the terrain's z0–2 root packs, from the lo packs' z3 tiles", &[], &["--raw"], 0),
    s("slope-hi", "[T …] [--regions dir] [--pass d] [--expect-same T,…] [--pieces-of Q,…]", "the shading from the terrain packs: of z6 tiles T (hi packs, z9–12, their z10 tiles' deep mids taken in), or of z3 tiles' whole, their deep levels, z6 tiles then assembly (default: all of them)", &[], &["--regions", "--pass", "--expect-same", "--pieces-of"], ANY),
    s("slope-deep", "<10/x/y …> [--regions dir] [--pass d]", "z10 tiles' shading down to the terrain's deep levels (deep packs, z13–16, and deep mids)", &[], PASS_REGIONS, ANY),
    s("slope-lo", "<Q …> [--regions dir] [--pass d]", "z3 tiles Q's zoomed-out slope (lo packs, z3–8)", &[], PASS_REGIONS, ANY),
    s("slope-root", "", "the slope's z0–2 root packs", &[], &[], 0),
    s("polar-terrain", "<s/4/x/y …>", "polar blocks' terrain (z4–9) from the elevation pool", &[], &[], ANY),
    s("polar-slope", "<s/4/x/y …>", "polar blocks' shading from their and their neighbours' terrain packs", &[], &[], ANY),
    s("polar-root", "<s …> [--regions dir] [--pass d]", "a pole's zoomed-out terrain and shading (z0–3) from its blocks; those no longer built dropped", &[], PASS_REGIONS, ANY),
    s("raw-pack", "[--from-tar - [--expect n]] [--cache dir] | --order | --check [--every n]", "AWS's raw tiles packed into the NAS's archives: a tar of them on stdin, else those waiting in the cache", &["--order", "--check"], &["--from-tar", "--expect", "--cache", "--every"], 0),
    s("prune", "<target …>", "drops from the manifest what the coverage no longer builds (\"unit U\", \"pois U\", \"pack T\", \"lo Q\", \"bldprep T\", \"bldtiles T\")", &[], &[], ANY),
    s("buildings", "[--dem dir] [--workers n]", "the world's roadside buildings: the declared release, in z8 tiles, onto the NAS with their index", &[], &["--dem", "--workers"], 0),
    s("bld-fetch", "[--pass d] [--regions dir]", "the 3D buildings' sources onto the NAS (pipeline::bld::fetch)", &[], &["--pass", "--regions"], 0),
    s("bldprep", "<T …> [--dem dir]", "the 3D buildings' normalized files of z6 tiles T", &[], &["--dem"], ANY),
    s("bldtiles", "<T …> [--pass d] [--regions dir]", "the 3D buildings' tiles of z6 tiles T", &[], PASS_REGIONS, ANY),
    s("bldfield-lo", "<Q …>", "z3 tiles Q's zoomed-out buildings field", &[], &[], ANY),
    s("depth-lo", "<Q …>", "z3 tiles Q's zoomed-out depth (lo packs, z3–8) from the declared depth sources", &[], &[], ANY),
    s("depth", "<T …>", "z6 tiles T's depth (hi packs, z9–16) where a depth source is finer than z8", &[], &[], ANY),
    s("depth-shade", "<Q …>", "z3 tiles Q's zoomed-out shading of the sea floor (lo packs, z3–8) from their depth and their z6 tiles' shading", &[], &[], ANY),
    s("bldtile-task", "<8/x/y | 6/x/y> --out dir [--pass d] [--regions dir]", "a z8 tile's task folder as a bldtiles job cuts it", &[], &["--out", "--pass", "--regions"], 1),
    s("trees", "<T …> [--pass d] [--regions dir] [--dem dir] [--chm dir] [--expect-same T,…]", "the tree cover layers, clipped to the coverage: of z6 tiles T, or a z3 tile's whole", &[], &["--pass", "--regions", "--dem", "--chm", "--expect-same"], ANY),
    s("trees-lo", "<Q …> [--pass d] [--regions dir]", "z3 tiles Q's zoomed-out tree cover (lo packs) from their z6 tiles' mids", &[], PASS_REGIONS, ANY),
    s("trees-coverage", "<Q> --out <file> [--pass d] [--regions dir]", "the coverage the trees program reads for z3 tile Q; nothing written to the NAS", &[], &["--out", "--pass", "--regions"], 1),
    s("treeblock-task", "<6/x/y> --row y --out dir [--pass d] [--regions dir]", "a row of tree cover blocks' task folder as a z6 tile's run cuts it; nothing written to the NAS", &[], &["--row", "--out", "--pass", "--regions"], 1),
    s("terrain-same", "<6/x/y | 3/x/y> [--whole] [--raw dir] [--pass d] [--regions dir]", "a terrain z6 tile made by subtree (or whole), its pack and mid's content names beside the manifest's, or a z3 tile's lo pack's; nothing written to the NAS", &["--whole"], &["--raw", "--pass", "--regions"], 1),
    s("terrain-deep-same", "[10/x/y …] [--of 6/x/y] [--list] [--raw dir] [--pass d] [--regions dir] [--hashes file]", "terrain-deep targets (named, or every one of a z6 tile) made as their jobs make them, their deep packs' content names beside the manifest's and their times (--list: the targets alone); nothing written to the NAS", &["--list"], &["--of", "--raw", "--pass", "--regions", "--hashes"], ANY),
    s("terrainsub-task", "<6/x/y> --out dir [--group 8/x/y] [--raw dir] [--pass d] [--regions dir]", "a terrain z6 tile's z8 subtrees' task files; nothing written to the NAS", &[], &["--out", "--group", "--raw", "--pass", "--regions"], 1),
    s("reach", "[--pass d] [U …]", "every unit's reach (units named: printed, nothing written)", &[], PASS, ANY),
    s("labels", "[--pass d]", "the labels by importance, worldwide, as the labels layer's packs", &[], PASS, 0),
    s("spoken", "[--pass d] [--languages-index i]", "the languages spoken where, for names", &[], &["--pass", "--languages-index"], 0),
    s("lakefill", "[--pass d]", "Canada's lakes missing from OpenStreetMap filled in", &[], PASS, 0),
    s("water", "[--pass d] [--basemap f] [--only x/y,…] [--archive f] [--lakefill f]", "the water layer, z0–9 (--archive: into that local file, the NAS untouched)", &[], &["--pass", "--basemap", "--only", "--archive", "--lakefill"], 0),
    s("terrain-water", "[--pass d] [--regions dir]", "the terrain's water digests", &[], PASS_REGIONS, 0),
    s("stations", "--pass d [--geojson f]", "the rail stops of the pass's rail set within the coverage, as the stations' tiles", &[], &["--pass", "--geojson"], 0),
    s("ferries", "--pass d [--gtfs-index i]", "the pass's ferries set (with the ferry feeds and the static timetables of index i, else the accepted one), as the ferries' blocks", &[], &["--pass", "--gtfs-index"], 0),
    s("rail-feeds", "[--pass d] [--regions dir] [--feeds-index i]", "the timetable feeds (the declarations of index i, else the accepted one): the coverage's rail feeds and the ferry feeds, each fetched once into the rail sources", &[], &["--pass", "--regions", "--feeds-index"], 0),
    s("rail", "[--pass d] [--regions dir] [--cache dir] [--gtfs-index i]", "trains a day on the coverage's rail ways (the feeds and the static timetables of index i, else the accepted one), as global/railfreq", &[], &["--pass", "--regions", "--cache", "--gtfs-index"], 0),
    s("regions-extract", "--pass d --ids r<id>,w<id>,…", "regions' outlines cut from the pass", &[], &["--pass", "--ids"], 0),
    s("regions-check", "--dir <folder> [--declarations <folder>] [--report f] [--max-mb n]", "the regions' check run on a scratch folder of region files in memory, against the current pass and the datasets the declarations name: its findings and each region's size. Reads only", &[], &["--dir", "--declarations", "--report", "--max-mb"], 0),
    s("regions-fixture", "<id> <lon,lat,km>", "an accepted version of one region, a circle, in a scratch folder (never the project folder)", &[], &[], 2),
    s("names-todo", "--translations-index i --descriptions-index i [--out dir]", "the translations' and descriptions' to-do lists from the newest catalogue", &[], &["--translations-index", "--descriptions-index", "--out"], 0),
    s("timetables-todo", "[--pass d] [--regions dir] [--out dir]", "the timetables' to-do list (into the static timetables' todo/ under the root, or --out) and the report of routes matching nothing on the map, from the rail and ferries jobs' pieces", &[], &["--pass", "--regions", "--out"], 0),
    s("inputs", "<unit> [--full] [--listing f] [--dem dir] [--forget]", "a gate unit's inputs folder checked: its clean changes taken in, the held ones reported", &["--full", "--forget"], &["--listing", "--dem"], 1),
    s("put", "<logical> <ext> <file>", "upload a file under a logical name", &[], &[], 3),
    s(store::catalogue::NAME, "[--held] [--ready <ids>] [--building <id>=<built>/<total>,…] [--delta]", "publish a catalogue of the build manifest (--held: into catalog-held/)", &["--held", "--delta"], &["--ready", "--building"], 0),
    s("store-fetch", "<id> --index <file.geojson>", "every file and fill mask of a dataset's tile index its store (sources/fetched/<id>/) lacks, as published; restartable", &[], &["--index"], 1),
    s("pyenv", "[--dem dir]", "the Python environment of dem/'s lock file (beside the app's versions, shared by those with the same lock), made if it's missing; its folder printed", &[], &["--dem"], 0),
    s("geoid-offsets", "--sources <dir> --from <reference> --at <lon,lat>[;…] [--store <dir>]", "the offsets the elevation step adds to heights in a reference to bring them to EGM2008, by the geoid declarations in a folder. Reads only", &[], &["--sources", "--from", "--at", "--store"], 0),
    s("p5-check", "trees [--pass d] [--costs f]", "what switching tree cover to z6 tiles and assemblies does now. Reads only", &[], &["--pass", "--costs"], 1),
];

/// Values that may start with `-`: a west or south coordinate (`--at -73.5,45.5`, regions-fixture's
/// `-73.5,45.5,20`).
const SIGNED: &[&str] = &["--at"];

/// The value's name in the usage, as the step's usage line has it (`[--pass d]`: d), else VALUE.
fn value_name(usage: &str, flag: &str) -> String {
    let after = usage.split(&format!("{flag} ")).nth(1).unwrap_or("");
    let w: String = after.chars().take_while(|c| !c.is_whitespace() && !"[]|".contains(*c)).collect();
    if w.is_empty() || w.starts_with('-') {
        "VALUE".into()
    } else {
        w.trim_matches(|c| c == '<' || c == '>').to_string()
    }
}

/// scenic-build's command line: its steps as subcommands.
pub fn command() -> Command {
    let mut c = Command::new("scenic-build")
        .about("The build's steps, writing to the NAS project folder (docs/steps.md)")
        .subcommand_required(true)
        .arg(Arg::new("root").long("root").value_name("NAS PROJECT FOLDER").global(true).help("The NAS project folder"))
        .arg(
            Arg::new("scratch")
                .long("scratch")
                .value_name("LOCAL DIR")
                .global(true)
                .help("The step's local scratch folder (default /tmp/scenic-build)"),
        );
    for st in STEPS {
        let mut sc = Command::new(st.name).about(st.about).override_usage(format!(
            "scenic-build {} {} [--root <nas project folder>] [--scratch <local dir>]",
            st.name, st.usage
        ));
        for f in st.flags {
            sc = sc.arg(Arg::new(&f[2..]).long(&f[2..]).action(ArgAction::SetTrue));
        }
        for o in st.opts {
            sc = sc.arg(
                Arg::new(&o[2..])
                    .long(&o[2..])
                    .value_name(value_name(st.usage, o))
                    .allow_hyphen_values(SIGNED.contains(o))
                    .action(if REPEATED.contains(o) { ArgAction::Append } else { ArgAction::Set }),
            );
        }
        sc = match (st.name, st.pos) {
            (_, 0) => sc,
            // (Its circle's longitude may be west: a value, not a flag.)
            ("regions-fixture", _) => sc
                .arg(Arg::new("id").required(true))
                .arg(Arg::new("circle").value_name("LON,LAT,KM").required(true).allow_hyphen_values(true)),
            (_, ANY) => sc.arg(Arg::new("targets").value_name("TARGET").num_args(0..)),
            (_, n) => sc.arg(Arg::new("targets").value_name("TARGET").num_args(0..=n)),
        };
        c = c.subcommand(sc);
    }
    c
}

/// Whether `step` is one of scenic-build's, and whether it takes arguments (targets).
pub fn takes_targets(step: &str) -> Option<bool> {
    STEPS.iter().find(|s| s.name == step).map(|s| s.pos > 0)
}

/// The step's matches, from a command line clap has checked (`args`: the program's name first);
/// the process's own line read once.
fn step_matches(args: &[String]) -> (String, clap::ArgMatches) {
    static FIRST: std::sync::OnceLock<(Vec<String>, String, clap::ArgMatches)> = std::sync::OnceLock::new();
    let read = |args: &[String]| {
        let m = command().try_get_matches_from(args).expect("a command line checked in main");
        let (step, sm) = m.subcommand().expect("a step");
        (args.to_vec(), step.to_string(), sm.clone())
    };
    let (a, step, sm) = FIRST.get_or_init(|| read(args));
    if a == args {
        return (step.clone(), sm.clone());
    }
    let (_, step, sm) = read(args);
    (step, sm)
}

/// A flag's values on a checked command line (`--pass`: its one; `--clear`: each; `--pass=d` too),
/// none when it isn't given or the step doesn't take it.
pub fn values(args: &[String], flag: &str) -> Vec<String> {
    let (_, sm) = step_matches(args);
    match sm.try_get_many::<String>(flag.trim_start_matches("--")) {
        Ok(Some(v)) => v.cloned().collect(),
        _ => Vec::new(),
    }
}

/// Whether a flag that takes no value (`--full`) is given on a checked command line; false when the
/// step doesn't take it.
pub fn flag(args: &[String], flag: &str) -> bool {
    let (_, sm) = step_matches(args);
    matches!(sm.try_get_one::<bool>(flag.trim_start_matches("--")), Ok(Some(true)))
}

/// The step named and its arguments (not flags or their values), in order, from a command line
/// clap has checked (`args`: the program's name first).
pub fn targets(args: &[String]) -> (String, Vec<String>) {
    let (step, sm) = step_matches(args);
    let step = step.as_str();
    let ids: Vec<String> = command()
        .find_subcommand(step)
        .map(|c| c.get_positionals().map(|a| a.get_id().to_string()).collect())
        .unwrap_or_default();
    let vals = ids.iter().flat_map(|id| sm.get_many::<String>(id).into_iter().flatten().cloned()).collect();
    (step.to_string(), vals)
}
