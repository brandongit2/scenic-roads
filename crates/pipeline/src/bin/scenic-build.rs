//! The build steps of the new pipeline (docs/plan.md §6), writing to the NAS project folder.
//!
//! usage: scenic-build <step> --root <nas project folder> --scratch <local dir> [options]
//!
//!   pack [--cache dir] [T …]     pack(T) for z6 tiles T (default: every tile with ways): road and
//!                                rail hi packs (z9–14) and hidata
//!   lo [--cache dir] [Q …]       lo packs (z4–8 road and rail tiles) for z3 tiles Q (default: all)
//!   osm-pass --planet <p> --date <d>  the OSM pass (pieces, sets, basemap, road values); resumable
//!   outlines [--pass d]          the pass's administrative and ISO 3166 outlines from its outline
//!                                set (pipeline::osmpass::make_outlines)
//!   patch-ferries [--pass d] [--near-coverage]  the pass's pieces given the standalone ferry ways
//!                                its filtered planet lacked (pipeline::osmpass::patch_ferries), all
//!                                or those meeting the regions (10 km round); run by hand
//!   unit [U …] [--pass d] [--layers-root r] [--regions dir] [--dem dir] [--cache-dir dir] [--buildings dir]
//!                                base(U) from the pass's pieces (the unit's programs on a unit folder):
//!                                default every unit whose piece meets the coverage
//!   unit-snap U --out d --cache c [--carry]  unit U's folder built from the records into d, nothing
//!                                written to the NAS, snapshotted around each step's program (for
//!                                tools/check/same.py and tail.mjs)
//!   tail-spec U --root R         unit U's tail as a task gives it a worker, its data servers by --root's declarations (tools/check/tail.mjs)
//!   roadunits                    the road → units index from the units' road ids
//!   terrain [T …] [--regions dir] [--pass d] [--raw dir] [--expect-same T,…]  the terrain from AWS's
//!                                raw tiles (cached in --raw), GLO-30 and the basemap's water: of z6
//!                                tiles T near the coverage (pieces: each one's hi pack, z9–12, and
//!                                its mid; --expect-same, those made again as the manifest has them,
//!                                else it fails, uploading nothing), or of z3 tiles' whole (default:
//!                                all of them; by hand, and a lease of the scheme before pieces);
//!                                --pieces-of Q,…: every piece of z3 tiles Q
//!   terrain-lo <Q …> [--raw dir]  z3 tiles Q's zoomed-out terrain (lo packs, z3–8) from their
//!                                pieces' mids
//!   slope [T …] [--regions dir] [--expect-same T,…]  the slope from the terrain packs: of z6 tiles
//!                                T (pieces: each one's hi pack, z9–11, and its mid), or of z3 tiles'
//!                                whole (default: all of them)
//!   slope-lo <Q …>               z3 tiles Q's zoomed-out slope (lo packs, z3–8) from their pieces'
//!                                mids and the lo pack as it is
//!   terrain-root, slope-root     their z0–2 root packs, from the lo packs' z3 tiles
//!   raw-pack [--from-tar -] [--cache dir]  AWS's raw tiles packed into the NAS's archives
//!                                (pipeline::rawpack): a tar of them on stdin (tools/nas/raw-pack.sh),
//!                                else those waiting in the cache
//!   prune <target …>             drops from the manifest what the coverage no longer builds:
//!                                "unit U" (its base pack, road values, English), "pois U" (its
//!                                candidates, peaks), "pack T" (hidata, road and rail hi packs),
//!                                "lo Q" (road and rail lo packs), "bldprep T" (its normalized
//!                                buildings), "bldtiles T" (its 3D buildings' hi pack)
//!   buildings [--dem dir] [--workers n]  the world's roadside buildings (pipeline::buildtiles):
//!                                the declared release, in z8 tiles, onto the NAS with their index
//!   bld-fetch [--pass d] [--dem dir]  the 3D buildings' sources onto the NAS (dem/bldfetch.py):
//!                                the pinned Overture release's files and GHSL's tiles meeting the
//!                                coverage grown by 20 km; what's there already skipped
//!   bldprep <T …> [--dem dir]    the 3D buildings' normalized files of z6 tiles T (pipeline::bld::prep:
//!                                dem/bldprep.py reads the downloaded Overture row groups and the
//!                                GHSL windows under T), as work/bld/6-x-y
//!   bldtiles <T …> [--pass d] [--regions dir]  the 3D buildings' tiles of z6 tiles T
//!                                (pipeline::bld::job): the buildings touching the coverage, their
//!                                heights filled, z14, as the hi pack layers/buildings/hi/6-x-y;
//!                                under the agent, some z8 areas offered to workers as tasks
//!                                with the field (levels 14–9) as layers/buildings-field/hi/6-x-y and
//!                                its level-8 planes as work/bldfield-mid/6-x-y
//!   bldfield-lo <Q …>            z3 tiles Q's zoomed-out buildings field (levels 8–4, the lo pack
//!                                layers/buildings-field/lo/3-x-y) from their z6 tiles' mids
//!   bldtile-task <8/x/y | 6/x/y> --out dir [--pass d] [--regions dir]  a z8 area's task folder
//!                                as a bldtiles job cuts it (pipeline::bld::task; a z6 tile: its
//!                                densest area), for the `bldtile` program and the checks
//!   trees <T …> [--pass d] [--dem dir] [--chm dir] [--expect-same T,…]  the tree cover layers
//!                                (pipeline::treepacks), clipped to the coverage: of z6 tiles T (a
//!                                piece: its hi packs and its mid; --expect-same, those made again
//!                                as the manifest has them, else it fails, uploading nothing), or a
//!                                z3 tile's whole (by hand, and a lease of the old scheme)
//!   trees-lo <Q …> [--pass d]    z3 tiles Q's zoomed-out tree cover (lo packs) from their pieces'
//!                                mids
//!   trees-coverage <Q> --out <file> [--pass d]  the coverage the trees program reads for z3 tile
//!                                Q (its cov.json), to run it by hand; nothing written to the NAS
//!   treeblock-task <6/x/y> --row y --out dir [--pass d]  a row of tree cover blocks' task folder as a
//!                                piece's run cuts it (pipeline::trees::task: out/task/u/), its
//!                                arguments (out/task.json: the blocks, the canopy squares the NAS
//!                                has there) and the blocks' tiles as the manifest's packs and mid
//!                                have them (out/packs/8-x-y/, as `trees --blocks` writes them), for
//!                                the checks; nothing written to the NAS
//!   terrainsub-task <6/x/y> --out dir [--group 8/x/y] [--pass d]  a terrain piece's z8 subtrees
//!                                in their groups, each group's task file as the piece's run cuts
//!                                it (pipeline::terrain_task: out/<x>-<y>/u/in.sect), for the
//!                                checks; nothing written to the NAS
//!   reach [--pass d] [U …]       every unit's reach (pipeline::reach): the boxes of its piece's
//!                                roads, rail and ferries, owned and all (units named: printed,
//!                                nothing written)
//!   labels [--pass d] [--dem dir]  the labels by importance, worldwide, from the pass's labels set
//!                                (dem/labels.py), as the labels layer's packs
//!   water [--pass d] [--basemap f] [--only x/y,…] [--archive f]  the water layer
//!                                (pipeline::water): each pixel's share of sea and inland water,
//!                                z0–9, drawn from the pass's basemap's z14 water, as the water
//!                                layer's packs (`--basemap`: a local archive; `--only`: those z5
//!                                tiles; `--archive`: into that local file, the NAS untouched)
//!   stations --pass d [--geojson f]  the rail stops of the pass's rail set within the coverage,
//!                                as the stations' tiles
//!   ferries --pass d [--dem dir] [--gtfs-index i]  the pass's ferries set through ferries.py
//!                                (with the ferry feeds and the static timetables of index i,
//!                                else the records' accepted one), as the ferries' blocks
//!   rail-feeds [--pass d] [--dem dir] [--feeds-index i]  the timetable feeds (the declarations of
//!                                index i, else the records' accepted one; dem/railfeeds.py), each
//!                                fetched once into the rail sources
//!   rail [--pass d] [--dem dir] [--cache dir] [--gtfs-index i]  trains a day on the coverage's
//!                                rail ways (dem/railgtfs.py on the feeds and the static
//!                                timetables of index i, else the records' accepted one, then
//!                                railfreq on the pass's rail set), as global/railfreq
//!   terrain-water [--pass d]     the terrain's water digests (pipeline::terrain_water::WaterIdx):
//!                                what each terrain piece and assembly near the coverage reads of
//!                                the latest basemap's water, as work/water-idx/<hash16 of its
//!                                pin>; those made before for the same reads kept
//!   water-key-check [--pass d] [--compute [--save f] | --digests f] [--fill n]  what keying
//!                                terrain on its water's digests does with the records now
//!                                (pipeline::agent::rekey::derive): the terrain left to make, then
//!                                as if a new basemap came with the same water, and with n pieces'
//!                                water changed; without the manifest's digests, --compute works
//!                                them out here, in memory, timed. Reads only
//!   inputs <unit> [--full] [--listing f]  a gate unit's drop box checked (pipeline::inputs::gate): its clean
//!                                changes taken in as its next accepted version, the held ones
//!                                reported (`--full`: every file hashed, not only those whose size
//!                                or time changed)
//!   put <logical> <ext> <file>   upload a file under a logical name
//!   verify                       check every unverified upload on the NAS (SHA-256 over SSH)
//!   catalog                      publish a catalog of the build manifest
//!   rekey-check [--pass d]       what re-keying the records for the units' new keys
//!                                (pipeline::agent::rekey) would do now: the units re-keyed, and
//!                                those left to build again, each with why; and the times that show
//!                                its one assumption holds. Reads only: writes nothing, keeps no index
//!   p5-check terrain [--pass d] [--costs f]  what switching terrain and slope to pieces and
//!                                assemblies (pipeline::agent::rekey::derive) does now: each z3
//!                                tile read as its pieces and assembly or not, slope's pieces left
//!                                to make and why, the work after it and its time, what deriving
//!                                costs a plan. Reads only (the terrain packs' indexes into memory)
//!   p5-check trees [--pass d] [--costs f]  what switching tree cover to pieces and assemblies
//!                                (pipeline::agent::rekey) would do now: each z3 tile re-keyed, or
//!                                made again as pieces and why, the work left and its time by the
//!                                last runs (`f`: the coordinator's costs.json). Reads only
//!
//! `--cache` is where base packs are kept locally (copied from the NAS when missing). `--help` (or
//! `-h`) on any step prints its usage and does nothing; a flag the step doesn't know or an argument
//! too many is refused before anything runs (pipeline::cli::tables::SCENIC_BUILD, the steps' table).

use anyhow::{bail, Context, Result};
use pipeline::basepack::BasePack;
use pipeline::hipack::{self, b, meets, tile_bounds};
use pipeline::layers::{self, LayerOut};
use pipeline::legacy::{self, Legacy, Unit};
use pipeline::out::Out;
use pipeline::timings::{phase, Class};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SSH: [&str; 8] = [
    "ssh",
    "-o",
    "ControlMaster=auto",
    "-o",
    "ControlPath=~/.ssh/cm-%r@%h:%p",
    "-o",
    "ControlPersist=12h",
    "brandontsang@fishandchips.local",
];
const NAS_ROOT: &str = "/volume1/personal/projects/scenic-roads";

/// Stage `k` (from 0) of a step's `n` starting: its progress line, which the agent's status shows
/// ("2 of 4 steps (what's being done)"). Within a part (`Parts`), its steps.
fn stage(k: u64, n: u64, what: &str) {
    pipeline::agent::jobs::stage(k, n, &format!("steps ({what})"));
}

/// A step's parts, in order: each marked as it begins (pipeline::agent::jobs::part: the status lists
/// them under the job, done, under way and to come), with its progress line ("2 of 4 parts").
struct Parts(&'static [&'static str]);

impl Parts {
    fn start(&self, i: usize) {
        pipeline::agent::jobs::part(i, self.0);
        pipeline::agent::jobs::stage(i as u64, self.0.len() as u64, &format!("parts ({})", self.0[i]));
    }
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

/// Positional arguments after the step (not options or their values).
fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 2;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
        } else {
            out.push(args[i].clone());
            i += 1;
        }
    }
    out
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // (Its flags and arguments checked first: `--help` or a flag the step doesn't know runs nothing.)
    let step = pipeline::cli::tables::SCENIC_BUILD.gate(&args).name.to_string();
    // Its phases' times (pipeline::timings: the log's table, the agent's record), however it ends.
    pipeline::timings::job(&step, || step_main(&args, &step))
}

/// inputs <unit> [--full] [--listing f]: a gate unit's drop box checked against its accepted version
/// (pipeline::inputs::gate; docs/inputs.md §4.3): the clean changes taken in, the held ones
/// reported, both handed off as records changes. `--listing <file>`: the listing and acceptances
/// the agent's key was made from, so the key recorded is the one of what was checked; by hand, the
/// drop box listed now. `--forget`: a unit off the gate, its records removed.
fn inputs_step(out: &mut Out, args: &[String]) -> Result<()> {
    let unit =
        positional(args).first().map(|u| u.trim_start_matches("inputs/").to_string()).context("inputs <unit>")?;
    if args.iter().any(|a| a == "--forget") {
        eprintln!("inputs: {}", pipeline::inputs::gate::forget(out, &unit)?);
        return Ok(());
    }
    let planned = opt(args, "--listing")
        .map(|f| -> Result<pipeline::inputs::Planned> {
            Ok(serde_json::from_slice(&std::fs::read(&f).with_context(|| format!("read {f}"))?)?)
        })
        .transpose()?;
    // (Where the Python the translations' checker runs in is: the app's copy of dem/.)
    if let Some(dem) = opt(args, "--dem") {
        pipeline::inputs::lint::set(pipeline::inputs::lint::Python::uv(
            &std::fs::canonicalize(&dem).with_context(|| format!("--dem {dem}"))?,
            &out.scratch_file("lint"),
        ));
    }
    let said = pipeline::inputs::gate::run(out, &unit, args.iter().any(|a| a == "--full"), planned)?;
    eprintln!("inputs: {said}");
    Ok(())
}

fn step_main(args: &[String], step: &str) -> Result<()> {
    let args: Vec<String> = args.to_vec();
    let step = step.to_string();
    // tail-spec U --root R: unit U's tail as a task gives it a worker (its steps, where its places lie, the
    // data servers it may read), for tools/check/tail.mjs; nothing written.
    if step == "tail-spec" {
        let u = positional(&args).first().and_then(|s| Unit::parse(s)).context("tail-spec U")?;
        // (The data servers as the accepted declarations under --root name them: crate::coord::hosts.)
        let root = PathBuf::from(opt(&args, "--root").context("tail-spec U --root <nas project folder>")?);
        let spec = serde_json::json!({ "runs": pipeline::unit::tail(u, true, true), "places": pipeline::offload::places(), "web_hosts": pipeline::coord::hosts::now(&root)? });
        println!("{spec}");
        return Ok(());
    }
    let root = PathBuf::from(opt(&args, "--root").context("--root <nas project folder>")?);
    // (Before the scratch folder: it writes nothing.)
    if step == "rekey-check" {
        return rekey_check(&root, &args);
    }
    if step == "regions-fixture" {
        // regions-fixture --root <scratch folder> <id> <lon,lat,km>: an accepted version of one
        // region, a circle, its index named in the folder's records (a scratch run's: the pool's
        // two-Mac procedure, tools/check/pool-two-macs.sh); never the project folder.
        anyhow::ensure!(!root.to_string_lossy().contains("/projects/scenic-roads"), "not the project folder");
        let pos = positional(&args);
        let (Some(id), Some(at)) = (pos.first(), pos.get(1)) else {
            bail!("regions-fixture --root <folder> <id> <lon,lat,km>")
        };
        let path = root.join("state/build/manifest.json");
        let mut m: BTreeMap<String, String> = pipeline::out::read_record(&path).unwrap_or_default();
        let r =
            pipeline::agent::recipes::Recipe { id: id.clone(), name: id.clone(), outline: vec![format!("place:{at}")] };
        let name = pipeline::inputs::regions::accept_fixture(&root, &root, &[r], &mut m);
        pipeline::whole::write(&path, &serde_json::to_vec(&m)?)?;
        println!("regions-fixture: {name}");
        return Ok(());
    }
    if step == "fabdem-fetch" {
        // fabdem-fetch --root <nas project folder> --mirror <HF dataset URL> --work <local folder>
        // [--first <square>,…]: every FABDEM tile the store (sources/fetched/fabdem/) lacks, from the
        // mirror (docs/inputs.md §6.8 names it), one at a time, the squares named first
        // (pipeline::dem::fabdem_fetch). Restartable; writes nothing else.
        use pipeline::dem::fabdem_fetch as ff;
        let work = PathBuf::from(opt(&args, "--work").context("--work <local folder for partial downloads>")?);
        let first: Vec<String> =
            opt(&args, "--first").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default();
        let mirror = opt(&args, "--mirror").context("--mirror <the Hugging Face dataset's URL>")?;
        let net = ff::Net::new();
        let tiles = ff::tiles(&ff::listing(&net, &mirror)?, &first)?;
        let done = ff::run(
            &net,
            &mirror,
            &tiles,
            &root.join("sources/fetched/fabdem"),
            &work,
            12,
            &std::thread::sleep,
            &mut |l| eprintln!("{l}"),
        )?;
        eprintln!(
            "fabdem-fetch: stored {} ({:.1} GiB), {} already there, {} failed",
            done.stored,
            done.bytes as f64 / (1u64 << 30) as f64,
            done.skipped,
            done.failed.len()
        );
        for (n, why) in &done.failed {
            eprintln!("  failed: {n}: {why}");
        }
        anyhow::ensure!(done.failed.is_empty(), "{} tiles failed: run it again", done.failed.len());
        return Ok(());
    }
    if step == "p5-check" {
        return p5_check(&root, &args);
    }
    if step == "water-key-check" {
        return water_key_check(&root, &args);
    }
    if step == "names-todo" {
        // names-todo --translations-index <index> --descriptions-index <index> [--out <dir>]: the
        // translations' and descriptions' to-do lists from the newest catalog (pipeline::namestodo)
        // and those inputs' accepted versions the agent names (their indexes' content names), into
        // the drop boxes' todo/ under the root (or --out); it writes nothing else.
        let out = opt(&args, "--out").map_or_else(|| root.clone(), PathBuf::from);
        let accepted = |unit: &str| -> Result<pipeline::inputs::Accepted> {
            let index = opt(&args, &format!("--{unit}-index"))
                .with_context(|| format!("--{unit}-index <its accepted index's content name>"))?;
            let m = std::collections::BTreeMap::from([(pipeline::inputs::logical(unit), index)]);
            pipeline::inputs::open(&root, &m, unit)?.with_context(|| format!("no accepted version of {unit}"))
        };
        let (tr, de) =
            (accepted(pipeline::inputs::translations::UNIT)?, accepted(pipeline::inputs::descriptions::UNIT)?);
        let t0 = std::time::Instant::now();
        let r = pipeline::namestodo::run(&root, &out, &tr, &de, &|what, done, total| {
            pipeline::agent::jobs::report(done, total, what)
        })?;
        eprintln!(
            "names-todo: {} in {:.0} s",
            serde_json::json!({"langs": r.langs, "read": r.read, "own": r.own, "lined": r.lined, "settlements": r.settlements, "english": r.english, "nowhere": r.nowhere, "landmarks": r.landmarks, "areas": r.areas}),
            t0.elapsed().as_secs_f64()
        );
        return Ok(());
    }
    let scratch = PathBuf::from(opt(&args, "--scratch").unwrap_or_else(|| "/tmp/scenic-build".into()));
    // (It puts and removes only its step's outputs: the function the lead's merge checks by.)
    let mut out = Out::open(&root, &scratch, &step)?;
    let t0 = std::time::Instant::now();
    match step.as_str() {
        "pack" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            let mirror = opt(&args, "--mirror").map(PathBuf::from);
            pack(&mut out, &cache, mirror.as_deref(), opt(&args, "--pass"), &positional(&args))?
        }
        "lo" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            let mirror = opt(&args, "--mirror").map(PathBuf::from);
            lo(&mut out, &cache, mirror.as_deref(), opt(&args, "--pass"), &positional(&args))?
        }
        "osm-pass" => {
            let planet = PathBuf::from(opt(&args, "--planet").context("--planet <path>")?);
            let date = opt(&args, "--date").context("--date YYYY-MM-DD")?;
            let extract = PathBuf::from(opt(&args, "--extract").unwrap_or_else(|| "target/release/extract".into()));
            let planetiler = PathBuf::from(opt(&args, "--planetiler").unwrap_or_else(|| "tools/planetiler.jar".into()));
            pipeline::osmpass::check_tools(&extract, &planetiler)?;
            // Room first: caches the pass makes worthless or can refill (the pack cache), through
            // their accessor, which passes over every file another job holds (store::cachefile).
            for dir in args.windows(2).filter(|w| w[0] == "--clear").map(|w| PathBuf::from(&w[1])) {
                if dir.exists() {
                    let t = store::cachefile::remove_tree(&dir);
                    eprintln!(
                        "osm-pass: cleared {}: {} MB freed, {} MB left (held by another job)",
                        dir.display(),
                        t.freed >> 20,
                        t.left >> 20
                    );
                }
            }
            pipeline::osmpass::run_pass(&mut out, &planet, &date, &scratch, &extract, &planetiler)?
        }
        "patch-ferries" => {
            let date = opt(&args, "--pass")
                .or_else(|| pipeline::osmpass::latest_pass(out.root()))
                .context("no complete OSM pass")?;
            // --near-coverage: the pieces meeting the regions, 10 km round (as their candidates read them).
            let cov = if args.iter().any(|a| a == "--near-coverage") { Some(coverage_of(&out, &args)?) } else { None };
            let near = |u: Unit| {
                cov.as_ref().is_none_or(|c| {
                    let b = pipeline::stage::tile_box_grown(u.z, u.x, u.y, 10.0);
                    let e7 = |v: f64| (v * 1e7).round() as i32;
                    c.meets_rect([e7(b[0]), e7(b[1]), e7(b[2]), e7(b[3])])
                })
            };
            let (pieces, ways) = pipeline::osmpass::patch_ferries(&mut out, &date, &scratch, &near)?;
            eprintln!("patch-ferries: {pieces} pieces of the {date} pass gained {ways} ferry ways");
        }
        "verify" => {
            let n = out.verify(&SSH, NAS_ROOT)?;
            eprintln!("verified {n} uploads");
        }
        // catalog [--held] [--ready <ids>] [--delta] [--whole]: held, it goes to catalog-held/, which
        // no server reads (a build to compare before it's switched to: inputs/hold-catalog);
        // `--ready`, the regions it records as built (comma-separated; the agent's plan says which),
        // the others as the last catalog had them (without it: every region as its recipe is now).
        // `--delta` (the agent's, with state/pool/delta-catalog on): the served catalog written as
        // what changed over the last (docs/formats.md, Delta catalogs), each file's size as the
        // records or the last catalog have it, not looked at on the NAS; `--whole` (the agent's in
        // idle time, when store::catalog::whole_due) writes it whole, every file looked at.
        "catalog" => {
            // (`<id>=<outline digest>` each, or a bare id: a region built with the outline it has now.)
            let ready: Option<BTreeMap<String, Option<String>>> = opt(&args, "--ready").map(|v| {
                v.split(',')
                    .filter(|s| !s.is_empty())
                    .map(|e| match e.split_once('=') {
                        Some((id, d)) => (id.to_string(), Some(d.to_string())),
                        None => (e.to_string(), None),
                    })
                    .collect()
            });
            let has = |f: &str| args.iter().any(|a| a == f);
            catalog(
                &mut out,
                has("--held"),
                ready.as_ref(),
                CatalogWay { delta: has("--delta"), whole: has("--whole") },
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
            )
            .map(|st| eprintln!("{}", st.line()))?
        }
        "unit" => unit_step(&mut out, &args, &scratch)?,
        "unit-snap" => unit_snap(&out, &args)?,
        "pois" => pois_step(&mut out, &args, &scratch)?,
        "roadunits" => roadunits(&mut out)?,
        "terrain" => terrain_step(&mut out, &args)?,
        "raw-pack" => {
            // raw-pack [--from-tar - [--expect n]] [--cache dir] | --order | --check [--every n]:
            // AWS's raw tiles packed into the NAS's archives (pipeline::rawpack): a tar stream of
            // them on stdin (the NAS's own: tools/nas/raw-pack.sh), the files listed for it `n`,
            // else the cache's tiles waiting to go. --order: tile paths on stdin, those the archives
            // lack put in their areas' order on stdout (for the stream; a run again streams only
            // what's left). --check: every archive the index names read
            // and matched against its name, and one tile in n of each against the NAS's loose copy.
            let store = out.root().join("sources/aws-terrarium");
            let dir = PathBuf::from(
                opt(&args, "--cache")
                    .unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
            );
            if args.iter().any(|a| a == "--order") {
                let mut paths = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut paths)?;
                let have = pipeline::rawpack::packed(&store)?;
                let list = pipeline::rawpack::order(&paths, |a, k| have.get(a).is_some_and(|s| s.contains(&k)));
                eprintln!(
                    "raw-pack: {} of {} files already in the archives",
                    paths.lines().count() - list.lines().count(),
                    paths.lines().count()
                );
                print!("{list}");
            } else if args.iter().any(|a| a == "--check") {
                let every = opt(&args, "--every").map(|n| n.parse()).transpose()?.unwrap_or(0);
                let c = pipeline::rawpack::check(&store, every)?;
                eprintln!(
                    "raw-pack: {} archives of {} tiles; {} bad; {} tiles matched against the loose ones, {} differ",
                    c.archives,
                    c.tiles,
                    c.bad.len(),
                    c.sampled,
                    c.differ.len()
                );
                for b in c.bad.iter().chain(&c.differ) {
                    eprintln!("  {b}");
                }
                anyhow::ensure!(c.bad.is_empty() && c.differ.is_empty(), "the archives aren't right");
            } else if opt(&args, "--from-tar").as_deref() == Some("-") {
                let expect = opt(&args, "--expect").map(|n| n.parse()).transpose()?;
                let r = pipeline::rawpack::pack_tar(std::io::stdin().lock(), &dir, &store, out.root(), expect)?;
                eprintln!(
                    "raw-pack: {} files in the stream, {} tiles, {} new to their archives",
                    r.files, r.tiles, r.added
                );
            } else {
                let n = pipeline::rawpack::pack_local(&dir, &store, out.root(), true)?;
                eprintln!("raw-pack: {n} tiles from {} packed", dir.display());
            }
        }
        "terrain-z8" => {
            // terrain-z8 [--raw dir]: AWS's z8 worldwide, repaired (pipeline::terrain_z8), once.
            if out.get(&pipeline::terrain_z8::logical()).is_some() {
                eprintln!("terrain-z8: already made ({})", pipeline::terrain_z8::logical());
            } else {
                let raw_dir = PathBuf::from(
                    opt(&args, "--raw")
                        .unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
                );
                let raw = raw_tiles(&out, &raw_dir)?;
                let parts =
                    Parts(&["Fetching and repairing the world's z8 terrain", "Packing the new raw tiles onto the NAS"]);
                parts.start(0);
                let (n, none) = pipeline::terrain_z8::build(&mut out, &raw)?;
                eprintln!("terrain-z8: {n} tiles ({none} of open sea)");
                parts.start(1);
                pack_raw_with(&out, &raw_dir, &|what, done, total| pipeline::agent::jobs::report(done, total, what));
            }
        }
        "inputs" => inputs_step(&mut out, &args)?,
        "regions-extract" => {
            // regions-extract --pass <date> --ids r1,w2,…: the regions' extract of those ids (and
            // those the pass's extract has) from the pass's planet (docs/inputs.md §6.1.6).
            let date = opt(&args, "--pass").context("--pass <date>")?;
            let ids: std::collections::BTreeSet<pipeline::inputs::regions::extract::Id> = opt(&args, "--ids")
                .context("--ids r<id>,w<id>,…")?
                .split(',')
                .map(str::parse)
                .collect::<Result<_>>()?;
            eprintln!("{}", pipeline::inputs::regions::extract::run(&mut out, &date, &ids)?);
        }
        "summits" => summits_step(&mut out, &args, &scratch)?,
        "peaks" => peaks_step(&mut out, &args, &scratch)?,
        "marks" => marks_step(&mut out, &args)?,
        "items" => items_step(&mut out, &args, &scratch)?,
        "heritage" => heritage_step(&mut out, &args, &scratch)?,
        "heritage-sites" => heritage_sites_step(&mut out, &args, &scratch)?,
        "overlays" => {
            // overlays [--pass <date>]: the area overlays and parks from the pass's heritage
            // outputs, the World Heritage outlines with the marks job's dots (ovconv::overlays).
            let date = opt(&args, "--pass")
                .or_else(|| pipeline::osmpass::latest_pass(out.root()))
                .context("no complete OSM pass")?;
            let src = pipeline::markconv::heritage_source(&date);
            stage(0, 1, "making the area overlays' tiles");
            let p = phase("the marks' heritage dots read", Class::NasRead);
            let dots = pipeline::ovconv::marks_dots(&out)?;
            drop(p);
            pipeline::ovconv::overlays(&mut out, &src, &dots)?;
            out.save()?;
        }
        "slope" => slope_step(&mut out, &args)?,
        "terrain-lo" => terrain_lo_step(&mut out, &args)?,
        "slope-lo" => slope_lo_step(&mut out, &args)?,
        "labels" => labels_step(&mut out, &args, &scratch)?,
        "spoken" => {
            // spoken [--pass <date>] --languages-index <index>: the languages spoken where
            // (names::spoken), from the pass's outlines and the languages input's accepted version
            // the agent names (its index's content name), as global/spoken, which the servers read.
            let date = opt(&args, "--pass")
                .or_else(|| pipeline::osmpass::latest_pass(out.root()))
                .context("no complete OSM pass")?;
            let table = {
                let unit = pipeline::inputs::languages::UNIT;
                let index = opt(&args, "--languages-index")
                    .context("--languages-index <the languages' accepted index's content name>")?;
                let m = std::collections::BTreeMap::from([(pipeline::inputs::logical(unit), index)]);
                let a = pipeline::inputs::open(out.root(), &m, unit)?
                    .with_context(|| format!("no accepted version of {unit}"))?;
                pipeline::inputs::languages::table_of(&a)?
            };
            let outlines = out.path(out.get(&format!("sources/osm/{date}/outlines")).context("the pass's outlines")?);
            // (The outlines read from the NAS as it works through them: one phase.)
            let p = phase("the languages spoken where worked out from the outlines", Class::NasRead);
            let s = pipeline::outlines::Outlines::open(&outlines)?.spoken(&table)?;
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("spoken.bin");
            std::fs::write(&local, s.to_bytes())?;
            drop(p);
            let p = phase("uploaded", Class::NasWrite);
            let name = out.put_file("global/spoken", "bin", &local)?;
            drop(p);
            std::fs::remove_file(&local).ok();
            out.save()?;
            eprintln!("spoken: {} regions, {name}", s.regions().count());
        }
        "lakefill" => lakefill_step(&mut out, &args, &scratch)?,
        "water" => water_step(&mut out, &args, &scratch)?,
        "terrain-water" => terrain_water_step(&mut out, &args)?,
        "pass-sets" => {
            // pass-sets [--pass <date>]: the sets the pass lacks in their current filters
            // (osmpass::SETS versions), from its kept filtered planet: copied here first when
            // there's room, since osmium reads it two or three times a set (from the NAS over
            // Wi-Fi that took hours a set; over the LAN, 65 min for the water set).
            let date = opt(&args, "--pass")
                .or_else(|| pipeline::osmpass::latest_pass(out.root()))
                .context("no complete OSM pass")?;
            let nas = out.path(out.get(&format!("sources/osm/{date}/filtered")).context("the pass's filtered planet")?);
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("filtered.osm.pbf");
            let size = std::fs::metadata(&nas)?.len();
            // (Room for the copy, the sets made from it, and the agent's reserve left free.)
            let room = pipeline::agent::cond::free_bytes(&scratch).unwrap_or(0)
                + std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0)
                > size + (20 << 30) + pipeline::agent::room::RESERVE;
            let src = if room && !pipeline::osmpass::missing_sets(&out, &date).is_empty() {
                let _p = phase("the filtered planet copied here", Class::NasRead);
                pipeline::osmpass::copy_resume_with(&nas, &local, &mut |d, t| {
                    pipeline::agent::jobs::report(
                        d >> 20,
                        t >> 20,
                        "MB of the filtered planet copied here (then the sets)",
                    )
                })?;
                pipeline::timings::count(std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0), 1);
                local.clone()
            } else {
                nas
            };
            let n = pipeline::osmpass::make_missing_sets(&mut out, &date, &src, &scratch)?;
            let p = phase("the copy removed", Class::Disk);
            std::fs::remove_file(&local).ok();
            drop(p);
            eprintln!("pass-sets: {n} made");
        }
        "outlines" => {
            // outlines [--pass <date>]: the pass's administrative and ISO 3166 outlines from its
            // outline set (pipeline::osmpass::make_outlines), as sources/osm/<date>/outlines.
            let date = opt(&args, "--pass")
                .or_else(|| pipeline::osmpass::latest_pass(out.root()))
                .context("no complete OSM pass")?;
            let s = pipeline::osmpass::make_outlines(&mut out, &date, &scratch)?;
            eprintln!("outlines: {} made ({} repaired, {} stale)", s.outlines, s.repaired.len(), s.stale.len());
        }
        "trailends" => {
            // trailends [--pass <date>]: every hiking route's ends, worldwide, from the hikes set.
            let date = opt(&args, "--pass")
                .or_else(|| pipeline::osmpass::latest_pass(out.root()))
                .context("no complete OSM pass")?;
            let set = out.path(
                out.get(&pipeline::osmpass::set_name(&date, "hikes"))
                    .context("the pass has no hikes set (pass-sets makes it)")?,
            );
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("set-hikes.osm.pbf");
            let parts = Parts(&["Copying the hiking routes", "Finding their ends", "Uploading"]);
            parts.start(0);
            let p = phase("the hikes set copied here", Class::NasRead);
            p.count(store::sys::copy_data(&set, &local).with_context(|| format!("copy {}", set.display()))?, 1);
            drop(p);
            parts.start(1);
            let p = phase("their ends found", Class::Compute);
            let ends = pipeline::trailends::ends(&local)?;
            drop(p);
            parts.start(2);
            let file = scratch.join("trailends.jsonl.zst");
            let p = phase("written", Class::Disk);
            pipeline::trailends::write(&file, &ends)?;
            drop(p);
            let p = phase("uploaded", Class::NasWrite);
            out.put_file(&format!("work/trailends/{date}"), "jsonl.zst", &file)?;
            drop(p);
            out.save()?;
            std::fs::remove_file(&local).ok();
            eprintln!("trailends: {} ends", ends.len());
        }
        "reach" => reach_step(&mut out, &args, &scratch)?,
        "buildings" => {
            let dem = std::fs::canonicalize(opt(&args, "--dem").unwrap_or_else(|| "dem".into()))?;
            // Two scans a thread the agent allows (they mostly wait on S3), 4 to 32.
            let threads: usize = std::env::var("RAYON_NUM_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(12);
            let workers = opt(&args, "--workers").map(|w| w.parse()).transpose()?.unwrap_or((threads * 2).clamp(4, 32));
            pipeline::buildtiles::build(&mut out, &dem, &scratch, workers)?;
        }
        "bld-fetch" => bld_fetch_step(&mut out, &args, &scratch)?,
        "bldprep" => bldprep_step(&mut out, &args)?,
        "bldtiles" => bldtiles_step(&mut out, &args)?,
        "bldfield-lo" => {
            // bldfield-lo <Q …>: each z3 tile's zoomed-out field of the 3D buildings (levels 8–4,
            // its lo pack) from its z6 tiles' mids (pipeline::bld::job::build_lo).
            let qs: Vec<Unit> = positional(&args)
                .iter()
                .map(|t| Unit::parse(t).filter(|u| u.z == 3).with_context(|| format!("not a z3 tile: {t}")))
                .collect::<Result<_>>()?;
            for (k, &q) in qs.iter().enumerate() {
                pipeline::control::safe_point("bldfield-lo");
                pipeline::agent::jobs::report(k as u64, qs.len() as u64, "areas");
                let t = cost_start("bldfield-lo", &q.slash());
                pipeline::bld::job::build_lo(&mut out, q)?;
                pipeline::control::done("bldfield-lo", &q.slash());
                note_cost("bldfield-lo", &q.slash(), t);
            }
            pipeline::agent::jobs::report(qs.len() as u64, qs.len() as u64, "areas");
        }
        "bldtile-task" => bldtile_task_step(&mut out, &args)?,
        "depth-lo" | "depth" => depth_step(&mut out, &args, step.as_str())?,
        "trees" => {
            // trees <tile …> [--pass d] [--dem dir] [--chm dir] [--expect-same T,…]: the tree cover
            // of z6 tiles (pieces: their hi packs and mids; pipeline::treepacks::build_piece), or a
            // z3 tile's whole (by hand, and a lease of the old scheme: treepacks::build_with).
            // `--expect-same`: those z6 tiles made again as they are (their mids backfilled), each
            // pack as the manifest has it, else the job fails, uploading nothing for it.
            let cov = coverage_of(&out, &args)?;
            let dem = std::fs::canonicalize(opt(&args, "--dem").unwrap_or_else(|| "dem".into()))?;
            let chm = PathBuf::from(opt(&args, "--chm").unwrap_or_else(|| "data/cache/chm10".into()));
            let workers: usize = std::env::var("RAYON_NUM_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(8);
            let ts: Vec<Unit> = positional(&args)
                .iter()
                .map(|t| {
                    Unit::parse(t).filter(|u| u.z == 3 || u.z == 6).with_context(|| format!("not a z3 or z6 tile: {t}"))
                })
                .collect::<Result<_>>()?;
            let same: BTreeSet<String> = opt(&args, "--expect-same")
                .map(|v| v.split(',').filter(|t| !t.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            let named: BTreeSet<String> = ts.iter().filter(|u| u.z == 6).map(|u| u.slash()).collect();
            anyhow::ensure!(
                same.is_subset(&named),
                "--expect-same names z6 tiles not given: {:?}",
                same.difference(&named).collect::<Vec<_>>()
            );
            // Its parts, for the status (as terrain's): each tile's tree cover mapped (the trees
            // program, which says how far it is), then written.
            let n = ts.len();
            let of = |k: usize| if n > 1 { format!(" ({} of {n})", k + 1) } else { String::new() };
            let mut names: Vec<String> = Vec::new();
            for (k, u) in ts.iter().enumerate() {
                let what = if u.z == 3 { "the area's".to_string() } else { format!("{}'s", u.slash()) };
                names.push(format!("Mapping {what} tree cover{}", of(k)));
                names.push(format!("Writing {what} tree cover to the NAS{}", of(k)));
            }
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            for (k, &u) in ts.iter().enumerate() {
                pipeline::control::safe_point("trees");
                pipeline::agent::jobs::part(2 * k, &names);
                let t = cost_start("trees", &u.slash());
                let writing = || pipeline::agent::jobs::part(2 * k + 1, &names);
                if u.z == 3 {
                    pipeline::treepacks::build_with(&mut out, &cov, u, &dem, &chm, &scratch, workers, &writing)?;
                } else {
                    pipeline::treepacks::build_piece(
                        &mut out,
                        &cov,
                        u,
                        &dem,
                        &chm,
                        &scratch,
                        workers,
                        same.contains(&u.slash()),
                        &writing,
                    )?;
                }
                pipeline::control::done("trees", &u.slash());
                note_cost("trees", &u.slash(), t);
            }
        }
        "trees-lo" => {
            // trees-lo <z3 tile …> [--pass d]: each z3 tile's zoomed-out tree cover (its lo packs)
            // assembled from its pieces' mids (pipeline::treepacks::build_lo).
            let cov = coverage_of(&out, &args)?;
            let workers: usize = std::env::var("RAYON_NUM_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(8);
            let qs: Vec<Unit> = positional(&args)
                .iter()
                .map(|t| Unit::parse(t).filter(|u| u.z == 3).with_context(|| format!("not a z3 tile: {t}")))
                .collect::<Result<_>>()?;
            for (k, &q) in qs.iter().enumerate() {
                pipeline::control::safe_point("trees-lo");
                pipeline::agent::jobs::report(k as u64, qs.len() as u64, "areas");
                let t = cost_start("trees-lo", &q.slash());
                pipeline::treepacks::build_lo(&mut out, &cov, q, &scratch, workers)?;
                pipeline::control::done("trees-lo", &q.slash());
                note_cost("trees-lo", &q.slash(), t);
            }
            pipeline::agent::jobs::report(qs.len() as u64, qs.len() as u64, "areas");
        }
        "trees-coverage" => {
            // trees-coverage <z3 tile> --out <file> [--pass d]: the coverage the trees program reads
            // there (its cov.json), to run it by hand. It only reads: nothing is saved.
            let cov = coverage_of(&out, &args)?;
            let t = positional(&args).first().cloned().context("a z3 tile")?;
            let q = Unit::parse(&t).filter(|u| u.z == 3).with_context(|| format!("not a z3 tile: {t}"))?;
            let f = PathBuf::from(opt(&args, "--out").context("--out <file>")?);
            std::fs::write(&f, serde_json::to_vec(&pipeline::treepacks::coverage_json(&cov, q))?)?;
            return Ok(());
        }
        "treeblock-task" => {
            // treeblock-task <6/x/y> --row y --out dir [--pass d]: a row's task folder (z6 tile
            // 6/x/y's blocks in zoom-8 row y that the coverage meets), its arguments and its blocks'
            // tiles as the NAS has them (for tools/check/treeblock-same.mjs). It only reads.
            let cov = coverage_of(&out, &args)?;
            let t = positional(&args)
                .first()
                .and_then(|t| Unit::parse(t))
                .filter(|u| u.z == 6)
                .context("treeblock-task <6/x/y> --row y")?;
            let y: u32 = opt(&args, "--row").context("--row y")?.parse()?;
            let shapes = pipeline::trees::mask::Shapes::parse(&serde_json::to_string(
                &pipeline::treepacks::coverage_json(&cov, t),
            )?)?;
            let row: Vec<(u32, u32)> =
                pipeline::trees::task::rows_of(&pipeline::trees::blocks_of(&shapes, 6, t.x, t.y))
                    .into_iter()
                    .flatten()
                    .filter(|b| b.1 == y)
                    .collect();
            anyhow::ensure!(!row.is_empty(), "{}: no block of row {y} meets the coverage", t.slash());
            let dir = PathBuf::from(opt(&args, "--out").context("--out dir")?);
            treeblock_task(&out, &cov, &row, &dir)?;
            return Ok(());
        }
        "terrainsub-task" => {
            // terrainsub-task <6/x/y> --out dir [--group 8/x/y] [--pass d]: piece 6/x/y's z8 subtrees
            // in their groups (pipeline::terrain_task::groups), each group's task file as the
            // piece's run cuts it (out/<x>-<y>/u/in.sect, by its first subtree; `--group`: that
            // group's alone), a line each said (its subtrees, tiles, bytes, predicted memory). It
            // only reads (AWS's raw tiles copied to this Mac's cache, as a terrain run does).
            let cov = coverage_of(&out, &args)?;
            let t = positional(&args)
                .first()
                .and_then(|t| Unit::parse(t))
                .filter(|u| u.z == 6)
                .context("terrainsub-task <6/x/y> --out dir")?;
            let dir = PathBuf::from(opt(&args, "--out").context("--out dir")?);
            let only = opt(&args, "--group")
                .map(|g| pipeline::terrain_task::parse_subtrees(&g))
                .transpose()?
                .and_then(|v| v.first().copied());
            let raw_dir = PathBuf::from(
                opt(&args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
            );
            let raw = raw_tiles(&out, &raw_dir)?;
            let opened = pipeline::terrain_pack::SourceFiles::open(&out, false)?;
            let coarse = pipeline::terrain_pack::Coarse::new(&raw);
            let src = opened.sources(Some(&coarse));
            let levels = pipeline::terrain_pack::piece_levels(&cov, (t.x, t.y));
            let t0 = std::time::Instant::now();
            let groups = pipeline::terrain_task::groups(&levels, src.water)?;
            println!(
                "{}",
                serde_json::json!({ "piece": t.slash(), "tiles": levels.iter().map(|(_, v)| v.len()).sum::<usize>(), "groups": groups.iter().map(|g| pipeline::terrain_task::subtrees_arg(g)).collect::<Vec<_>>(), "secs": t0.elapsed().as_secs_f64() })
            );
            for g in &groups {
                if only.is_some_and(|f| !g.contains(&f)) {
                    continue;
                }
                let lv = pipeline::terrain_task::levels_of(&levels, &g.iter().copied().collect(), true);
                let path = dir.join(format!("{}-{}", g[0].0, g[0].1)).join("u").join(pipeline::terrain_task::INPUT);
                let t0 = std::time::Instant::now();
                let c = pipeline::terrain_task::cut((t.x, t.y), g, &lv, &raw, &src, &path)?;
                println!(
                    "{}",
                    serde_json::json!({ "group": pipeline::terrain_task::subtrees_arg(g), "tiles": c.tiles, "made": c.made.len(), "raw": c.raw, "cells": c.cells, "bytes": c.bytes, "mem_mb": pipeline::terrain_task::mem_mb(c.bytes, &lv), "secs": t0.elapsed().as_secs_f64() })
                );
            }
            return Ok(());
        }
        "prune" => prune_step(&mut out, &args)?,
        "stations" => {
            // stations --pass <date> [--geojson file]: the pass's rail set's stops as the stations' tiles.
            let date = opt(&args, "--pass").context("--pass <date>")?;
            let gj = opt(&args, "--geojson").map(PathBuf::from);
            stage(0, 1, "making the rail stops' tiles");
            let (n, tiles) = pipeline::ovconv::stations_job(&mut out, &date, gj.as_deref())?;
            eprintln!("stations: {n} stops in {tiles} tiles");
        }
        "ferries" => {
            // ferries --pass <date> [--dem dir] [--gtfs-index <index>]: the pass's ferries set
            // through ferries.py, as blocks.
            let date = opt(&args, "--pass").context("--pass <date>")?;
            let dem = PathBuf::from(opt(&args, "--dem").unwrap_or_else(|| "dem".into()));
            let statics = static_timetables(&out, &args)?;
            stage(0, 1, "finding the ferries (ferries.py) and their blocks");
            let n = pipeline::ovconv::ferries_job(&mut out, &date, &dem, &statics)?;
            eprintln!("ferries: {n} blocks");
        }
        "rail-feeds" => rail_feeds_step(&mut out, &args, &scratch)?,
        "rail" => rail_step(&mut out, &args, &scratch)?,
        "timetables-todo" => {
            // timetables-todo [--pass d] [--regions dir] [--out dir]: the timetables' to-do list
            // into the static timetables' todo/ under the root (or --out), and the report of the
            // routes matching nothing on the map (pipeline::timetables), from the rail and ferries
            // jobs' pieces.
            let cov = coverage_of(&out, &args)?;
            let to = opt(&args, "--out").map_or_else(|| out.root().to_path_buf(), PathBuf::from);
            let r = pipeline::timetables::run(&mut out, &cov, &to)?;
            eprintln!("timetables-todo: {} rail and {} ferry lines with no timetable ({:.0} km), {} routes matching nothing on the map", r.rail, r.ferry, r.km, r.unmatched);
        }
        "terrain-root" => {
            let raw_dir = PathBuf::from(
                opt(&args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
            );
            let raw = raw_tiles(&out, &raw_dir)?;
            let opened = pipeline::terrain_pack::SourceFiles::open(&out, true)?;
            let n = pipeline::terrain_pack::build_root(&mut out, &raw, &opened.sources(None))?;
            eprintln!("terrain root: {n} tiles");
            pack_raw(&out, &raw_dir);
        }
        "slope-root" => {
            let n = pipeline::slope_pack::build_root(&mut out)?;
            eprintln!("slope root: {n} tiles");
        }
        "put" => {
            // put <logical> <ext> <file>: upload a file under a logical name (manual operations).
            let p = positional(&args);
            let (Some(l), Some(ext), Some(f)) = (p.first(), p.get(1), p.get(2)) else {
                bail!("put <logical> <ext> <file>")
            };
            let tmp = scratch.join(format!("put-{}", std::path::Path::new(f).file_name().unwrap().to_string_lossy()));
            std::fs::create_dir_all(&scratch)?;
            store::sys::copy_data(f, &tmp)?;
            let name = out.put_file(l, ext, &tmp)?;
            eprintln!("{l} -> {name}");
        }
        s => bail!("unknown step {s:?} (see the usage at the top of scenic-build.rs)"),
    }
    out.save()?;
    eprintln!("{step}: done in {:.0?}", t0.elapsed());
    Ok(())
}

// ---- road values ---------------------------------------------------------------------------

/// A unit's road values file: the values per way, and the ways by road (sorted (road, way index)
/// pairs, for the server's whole-road lookups).
fn put_roads(out: &mut Out, u: Unit, recs: &[pipeline::legacy::RoadRec], keep: Option<&Path>) -> Result<String> {
    let mut byroad: Vec<[u64; 2]> = recs.iter().enumerate().map(|(i, r)| [r.road, i as u64]).collect();
    byroad.sort_unstable();
    put_sect_kept(
        out,
        &format!("global/roads/{}", u.dash()),
        serde_json::json!({"fmt": 1, "unit": u.slash()}),
        &[("roads", b(recs)), ("byroad", b(&byroad))],
        keep,
    )
}

/// A unit's road ids (`pipeline::out::ROAD_IDS`): its road values' roads, sorted, each once, as
/// section "ids" (u64); what the road → units index reads of it (`roadunits`).
fn put_road_ids(out: &mut Out, u: Unit, recs: &[pipeline::legacy::RoadRec]) -> Result<String> {
    let ids = road_ids(recs);
    put_sect(
        out,
        &format!("{}{}", pipeline::out::ROAD_IDS, u.dash()),
        serde_json::json!({"fmt": 1, "unit": u.slash()}),
        &[("ids", b(&ids))],
    )
}

/// Road values' roads, sorted, each once.
fn road_ids(recs: &[pipeline::legacy::RoadRec]) -> Vec<u64> {
    let mut ids: Vec<u64> = recs.iter().map(|r| r.road).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn put_sect(out: &mut Out, logical: &str, meta: serde_json::Value, sections: &[(&str, &[u8])]) -> Result<String> {
    put_sect_kept(out, logical, meta, sections, None)
}

/// `put_sect`, a copy kept in cache `keep` too (by its content name, as `open_units` looks for it)
/// when there's one.
fn put_sect_kept(
    out: &mut Out,
    logical: &str,
    meta: serde_json::Value,
    sections: &[(&str, &[u8])],
    keep: Option<&Path>,
) -> Result<String> {
    let local = out.scratch_file(&format!("{logical}.sect"));
    let mut w = store::sect::SectWriter::create(&local, meta)?;
    for (n, bytes) in sections {
        w.add(n, bytes)?;
    }
    w.finish()?;
    if let Some(cache) = keep {
        keep_for_packs(cache, logical, &local);
    }
    out.put_file(logical, "sect", &local)
}

/// The base packs' cache of this Mac's map tiles' jobs (`open_units`: `<cache>/<content name>`),
/// given unit file `local` (logical `logical`) as it's uploaded, so the next pack job here needn't
/// copy back from the NAS what this Mac just wrote. Only where that cache is (a Mac that makes the
/// map tiles), not held after (room-making may take it), and only a cache: failing costs that copy.
fn keep_for_packs(cache: &Path, logical: &str, local: &Path) {
    if !cache.is_dir() {
        return;
    }
    let kept = (|| -> Result<()> {
        let name = store::naming::content_name(logical, &store::naming::hash16_file(local)?, "sect");
        let at = cache.join(&name);
        store::cachefile::create(&at, &mut |tmp| std::fs::copy(local, tmp).map(|_| ()))?;
        store::cachefile::release(&at);
        Ok(())
    })();
    if let Err(e) = kept {
        eprintln!("{logical}: no copy kept for the map tiles ({e:#})");
    }
}

// ---- base packs, locally ------------------------------------------------------------------

/// Every unit's base pack and road values, copied into `cache` when missing, opened.
/// Every unit's base pack and road values, opened from local copies: this Mac's mirror's when it
/// has them (`mirror`, the server's: same content names, so no second copy), else the cache's,
/// copied from the NAS when missing. Cached files no unit uses any more go first, so the cache
/// holds at most one copy of what the mirror lacks.
/// The units of the manifest owning tiles `only` (z`z`: `build::owners_of`, the owners their keys
/// read), by the reaches of pass `pass` (else the latest) and every base pack's extent
/// (crate::extents: the lead's, those it lacks read from their headers): None (every unit)
/// when no tiles are named, the reaches or extents can't be read, or a unit isn't in the reaches
/// (said).
fn owners_here(out: &Out, pass: Option<String>, z: u8, only: &[Unit]) -> Option<BTreeSet<Unit>> {
    if only.is_empty() {
        return None;
    }
    let date = pass.or_else(|| pipeline::osmpass::latest_pass(out.root()))?;
    let mut reach = match pipeline::reach::Reaches::load(out.root(), &out.manifest, &date) {
        Ok(Some(r)) => r,
        Ok(None) => {
            eprintln!("no reaches for the pass of {date}: every unit's base pack opened");
            return None;
        }
        Err(e) => {
            eprintln!("the pass's reaches ({e:?}): every unit's base pack opened");
            return None;
        }
    };
    // (A unit the reaches don't know: where its ways lie isn't known, so neither are the owners.)
    if let Some(u) = out
        .manifest
        .keys()
        .filter_map(|k| k.strip_prefix("base/").and_then(Unit::parse))
        .find(|&u| reach.get(u).is_none())
    {
        eprintln!("unit {} isn't in the reaches of the pass of {date}: every unit's base pack opened", u.slash());
        return None;
    }
    let p = phase("the base packs' extents", Class::NasRead);
    // (The lead's kept extents, those it lacks read here from this job's packs' headers: in memory
    // only, the lead keeps the file.)
    reach.extents = match pipeline::extents::fill(out.root(), &out.manifest, std::mem::take(&mut reach.extents)) {
        Ok((e, read)) => {
            if read > 0 {
                eprintln!("extents: {read} base packs' headers read (not kept yet by the lead)");
            }
            e
        }
        Err(e) => {
            eprintln!("the base packs' extents ({e:#}): every unit's base pack opened");
            return None;
        }
    };
    drop(p);
    let bad = outside_their_ways(&out.manifest, &reach);
    if !bad.is_empty() {
        eprintln!(
            "WARNING: base packs of {} lie outside where the reaches put their ways (built by other reaches?): their own extents taken in, as the tiles' keys take them in",
            bad.iter().map(|u| u.slash()).collect::<Vec<_>>().join(", ")
        );
    }
    let tiles: Vec<(u8, u32, u32)> = only.iter().map(|t| (z, t.x, t.y)).collect();
    debug_assert!(only.iter().all(|t| t.z == z));
    Some(pipeline::agent::build::owners_of(Some(&reach), &out.manifest, &tiles))
}

/// The manifest's units whose base pack's extent (kept with `reach`) isn't all inside where
/// `reach` puts their ways (`build::unit_ways`): units built by other reaches.
fn outside_their_ways(m: &BTreeMap<String, String>, reach: &pipeline::reach::Reaches) -> Vec<Unit> {
    m.iter()
        .filter_map(|(l, c)| Some((l.strip_prefix("base/").and_then(Unit::parse)?, reach.extents.get(c)?)))
        .filter(|(u, e)| {
            let w = pipeline::agent::build::unit_ways(Some(reach), *u);
            !(e[0] >= w[0] && e[1] >= w[1] && e[2] <= w[2] && e[3] <= w[3])
        })
        .map(|(u, _)| u)
        .collect()
}

/// The base packs a pack or lo job of tiles `only` (z`z`) reads: its tiles' owners
/// (`owners_here`), every unit when they can't be told.
fn open_for_tiles(
    out: &Out,
    cache: &Path,
    mirror: Option<&Path>,
    pass: Option<String>,
    z: u8,
    only: &[Unit],
) -> Result<Vec<BasePack>> {
    open_units(out, cache, mirror, owners_here(out, pass, z, only).as_ref())
}

/// The base packs (with their road values) of the manifest's units, of only those in `owning`
/// when given: each held while the job runs (store::cachefile), from this Mac's mirror when it
/// has it, else from the cache, copied there from the NAS first when it isn't. (Past this
/// process's cap of held files, store::cachefile lets the earliest go, as a run of every unit,
/// about 528 files, may: harmless, since each pack is mmapped once opened, and a mapped file's
/// bytes stay until it's unmapped though the mirror or room-making unlinks it; it only frees its
/// room later.)
fn open_units(
    out: &Out,
    cache: &Path,
    mirror: Option<&Path>,
    owning: Option<&BTreeSet<Unit>>,
) -> Result<Vec<BasePack>> {
    // (From this Mac's mirror, else copied from the NAS: the copies counted.)
    let _p = phase("the areas' base packs here", Class::NasRead);
    std::fs::create_dir_all(cache)?;
    let mut all: Vec<(String, String, String)> = Vec::new();
    for (k, v) in &out.manifest {
        if let Some(u) = k.strip_prefix("base/") {
            let roads = out.get(&format!("global/roads/{u}")).with_context(|| format!("no road values for {u}"))?;
            all.push((u.to_string(), v.clone(), roads.to_string()));
        }
    }
    // (Every unit's copies kept: another job's tiles may need them. One a running job holds stays
    // anyway: prune_cache.)
    let needed: BTreeSet<&str> = all.iter().flat_map(|(_, b, r)| [b.as_str(), r.as_str()]).collect();
    prune_cache(cache, &needed)?;
    let units: Vec<&(String, String, String)> =
        all.iter().filter(|(u, _, _)| owning.is_none_or(|o| Unit::parse(u).is_some_and(|u| o.contains(&u)))).collect();
    let mut packs = Vec::with_capacity(units.len());
    let mut said = std::time::Instant::now();
    for (k, (u, base, roads)) in units.iter().enumerate() {
        // (The first job after a pass copies most of them: minutes.)
        if k == 0 || said.elapsed() >= std::time::Duration::from_secs(1) {
            said = std::time::Instant::now();
            pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas' base packs here");
        }
        let mut local = Vec::new();
        for name in [base, roads] {
            // (The mirror renames a file into place only once it's copied and checked, and deletes
            // one only under an exclusive lock: held here under a shared one, it stays.)
            if let Some(m) = mirror.map(|m| m.join(name)) {
                if let Some(m) =
                    store::cachefile::hold_untouched(&m).with_context(|| format!("hold {}", m.display()))?
                {
                    local.push(m);
                    continue;
                }
            }
            // (Held for the job, copied from the NAS first when it isn't here: store::cachefile.)
            let p = store::cachefile::hold(&cache.join(name), &mut |tmp| {
                pipeline::timings::count(store::sys::copy_data(out.path(name), tmp)?, 1);
                if store::naming::hash16_file(tmp).map_err(std::io::Error::other)?
                    != name.rsplit('.').nth(1).unwrap_or("")
                {
                    return Err(std::io::Error::other("hash mismatch after copy"));
                }
                Ok(())
            })
            .with_context(|| format!("copy {name} from the NAS"))?;
            local.push(p);
        }
        packs.push(BasePack::open(&local[0], &local[1]).with_context(|| format!("unit {u}"))?);
    }
    pipeline::agent::jobs::report(units.len() as u64, units.len() as u64, "areas' base packs here");
    Ok(packs)
}

/// Removes from `cache` every file not in `keep` (content names relative to it), and leftovers of
/// interrupted copies, but those a running job holds (store::cachefile::try_remove).
fn prune_cache(cache: &Path, keep: &BTreeSet<&str>) -> Result<()> {
    let mut stack = vec![cache.to_path_buf()];
    let mut freed = 0u64;
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)?.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(cache).unwrap_or(&p).to_string_lossy().into_owned();
            // (Not one another job uses: store::cachefile.)
            if !keep.contains(rel.as_str()) {
                if let store::cachefile::Removed::Freed(n) = store::cachefile::try_remove(&p) {
                    freed += n;
                }
            }
        }
    }
    if freed > 0 {
        eprintln!("cache: {} MB of replaced base packs removed", freed >> 20);
    }
    Ok(())
}

/// The z-level tiles any way of `packs` touches.
fn tiles_touched(packs: &[BasePack], z: u8) -> Result<BTreeSet<Unit>> {
    let mut set = BTreeSet::new();
    for bp in packs {
        for bb in bp.bboxes()? {
            let a = Unit::of_point(z, [bb[0], bb[3]]);
            let c = Unit::of_point(z, [bb[2], bb[1]]);
            for x in a.x..=c.x {
                for y in a.y..=c.y {
                    set.insert(Unit { z, x, y });
                }
            }
        }
    }
    Ok(set)
}

fn parse_tiles(list: &[String], z: u8) -> Result<Vec<Unit>> {
    list.iter().map(|s| Unit::parse(s).filter(|u| u.z == z).with_context(|| format!("not a z{z} tile: {s}"))).collect()
}

// ---- pack(T) --------------------------------------------------------------------------------

fn pack(out: &mut Out, cache: &Path, mirror: Option<&Path>, pass: Option<String>, only: &[String]) -> Result<()> {
    let parts = Parts(&["Getting the areas' base packs here", "Drawing the map tiles"]);
    parts.start(0);
    let only = parse_tiles(only, 6)?;
    // (Only the tiles' owners: no other unit's ways come into them.)
    let packs = open_for_tiles(out, cache, mirror, pass, 6, &only)?;
    let refs: Vec<&BasePack> = packs.iter().collect();
    let p = phase("the tiles they touch found", Class::Compute);
    let ts: Vec<Unit> = if only.is_empty() { tiles_touched(&packs, 6)?.into_iter().collect() } else { only };
    drop(p);
    eprintln!("pack: {} tiles from {} units", ts.len(), packs.len());
    parts.start(1);
    let t0 = std::time::Instant::now();
    let n = ts.len() as u64;
    // (Within a tile: its ways read, then its tiles drawn, then written.)
    let at = |k: usize, f: f64| pipeline::agent::jobs::report_f(k as f64 + f, n, "map tiles");
    for (k, t) in ts.iter().enumerate() {
        // (A safe point before each tile: with the build pausing, the job ends here.)
        pipeline::control::safe_point("pack");
        // (What each tile takes, for the lead's offers by the measures: docs/pool.md §7.7.)
        let c = cost_start("pack", &t.slash());
        at(k, 0.0);
        let tb = tile_bounds(t.z, t.x, t.y);
        // (Each stage of a tile's: one phase over the tiles.)
        let p = phase("ways gathered from the base packs", Class::Compute);
        let near: Vec<&BasePack> = refs.iter().copied().filter(|bp| meets(bp.extent, tb)).collect();
        let in_t = hipack::ways_in(&near, tb)?;
        drop(p);
        // (Its inputs' measures: the base packs it meets, their bytes, its ways.)
        pipeline::costin::set("units", near.len() as u64);
        pipeline::costin::add("bytes", near.iter().map(|bp| bp.bytes()).sum());
        pipeline::costin::set("ways", in_t.len() as u64);
        if in_t.is_empty() {
            // No ways here (any more): what an earlier build made for the tile goes.
            drop_entries(
                out,
                &[
                    format!("hidata/{}", t.dash()),
                    format!("layers/roads/hi/{}", t.dash()),
                    format!("layers/rails/hi/{}", t.dash()),
                ],
            )?;
            note_cost("pack", &t.slash(), c);
            pipeline::control::done("pack", &t.slash());
            continue;
        }
        let p = phase("road and rail tiles drawn", Class::Compute);
        let win = hipack::way_inputs(&near, &in_t)?;
        at(k, 0.25);
        let (roads, rails) = hipack::tiles(&win, 6, t.x, t.y, 9..=14, false);
        drop(p);
        at(k, 0.6);
        let p = phase("road and rail packs written to the NAS", Class::NasWrite);
        for (layer, enc) in [("roads", &roads), ("rails", &rails)] {
            let mut it = enc.iter().map(|e| {
                let (z, x, y) =
                    ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                (z, x, y, e.gz.clone(), e.raw_len as u32)
            });
            if layers::write_pack(out, layer, "rt7", true, "hi", (6, t.x, t.y), &mut it)?.is_none() {
                drop_entries(out, &[format!("layers/{layer}/hi/{}", t.dash())])?;
            }
        }
        drop(p);
        at(k, 0.75);
        let p = phase("hidata made", Class::Compute);
        let hd = hipack::hidata(*t, &near, &in_t)?;
        // The zoomed-out summaries (docs/phase5.md), from the same parts.
        let ls = roadcore::lsum::build(&hd.parts, &hd.psamples, &hd.pch, &hd.here, &hd.railinfo);
        drop(p);
        let p = phase("hidata written to the NAS", Class::NasWrite);
        put_sect(
            out,
            &format!("hidata/{}", t.dash()),
            serde_json::json!({"fmt": 1, "tile": t.slash(), "here": hd.here.len(), "parts": hd.parts.len(), "lsum": roadcore::lsum::LSUM_V}),
            &[
                ("here", b(&hd.here)),
                ("ends", b(&hd.ends)),
                ("parts", b(&hd.parts)),
                ("psamples", b(&hd.psamples)),
                ("pch", b(&hd.pch)),
                ("railinfo", b(&hd.railinfo)),
                ("railstr", &hd.railstr),
                ("lparts", b(&ls.lparts)),
                ("lbins", b(&ls.lbins)),
                ("lrparts", b(&ls.lrparts)),
                ("lrbins", b(&ls.lrbins)),
            ],
        )?;
        drop(p);
        out.save()?;
        note_cost("pack", &t.slash(), c);
        pipeline::control::done("pack", &t.slash());
        eprintln!(
            "pack {} ({}/{}): {} ways, {} road tiles, {} parts ({:.0?})",
            t.slash(),
            k + 1,
            ts.len(),
            in_t.len(),
            roads.len(),
            hd.parts.len(),
            t0.elapsed()
        );
    }
    pipeline::agent::jobs::report(n, n, "map tiles");
    Ok(())
}

/// Drops logical names from the manifest (outputs a step no longer makes), saving when any went.
fn drop_entries(out: &mut Out, logicals: &[String]) -> Result<()> {
    let gone: Vec<&String> = logicals.iter().filter(|l| out.get(l).is_some()).collect();
    for l in &gone {
        out.remove(l);
    }
    if !gone.is_empty() {
        out.save()?;
    }
    Ok(())
}

// ---- lo packs -------------------------------------------------------------------------------

fn lo(out: &mut Out, cache: &Path, mirror: Option<&Path>, pass: Option<String>, only: &[String]) -> Result<()> {
    let parts = Parts(&["Getting the areas' base packs here", "Drawing the zoomed-out tiles"]);
    parts.start(0);
    let only = parse_tiles(only, 3)?;
    let packs = open_for_tiles(out, cache, mirror, pass, 3, &only)?;
    let refs: Vec<&BasePack> = packs.iter().collect();
    let p = phase("the tiles they touch found", Class::Compute);
    let qs: Vec<Unit> = if only.is_empty() { tiles_touched(&packs, 3)?.into_iter().collect() } else { only };
    drop(p);
    eprintln!("lo: {} tiles from {} units", qs.len(), packs.len());
    parts.start(1);
    let t0 = std::time::Instant::now();
    let n = qs.len() as u64;
    let at = |k: usize, f: f64| pipeline::agent::jobs::report_f(k as f64 + f, n, "zoomed-out tiles");
    for (k, q) in qs.iter().enumerate() {
        pipeline::control::safe_point("lo");
        let c = cost_start("lo", &q.slash());
        at(k, 0.0);
        let qb = tile_bounds(q.z, q.x, q.y);
        let p = phase("ways gathered from the base packs", Class::Compute);
        let near: Vec<&BasePack> = refs.iter().copied().filter(|bp| meets(bp.extent, qb)).collect();
        let staged = hipack::ways_in(&near, qb)?;
        drop(p);
        pipeline::costin::set("units", near.len() as u64);
        pipeline::costin::add("bytes", near.iter().map(|bp| bp.bytes()).sum());
        pipeline::costin::set("ways", staged.len() as u64);
        if staged.is_empty() {
            drop_entries(out, &[format!("layers/roads/lo/{}", q.dash()), format!("layers/rails/lo/{}", q.dash())])?;
            note_cost("lo", &q.slash(), c);
            pipeline::control::done("lo", &q.slash());
            continue;
        }
        let p = phase("road and rail tiles drawn", Class::Compute);
        let win = hipack::way_inputs(&near, &staged)?;
        at(k, 0.3);
        let (roads, rails) = hipack::tiles(&win, 3, q.x, q.y, 4..=8, true);
        drop(p);
        at(k, 0.8);
        let p = phase("road and rail packs written to the NAS", Class::NasWrite);
        for (layer, enc) in [("roads", &roads), ("rails", &rails)] {
            let mut it = enc.iter().map(|e| {
                let (z, x, y) =
                    ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                (z, x, y, e.gz.clone(), e.raw_len as u32)
            });
            if layers::write_pack(out, layer, "rt7", true, "lo", (3, q.x, q.y), &mut it)?.is_none() {
                drop_entries(out, &[format!("layers/{layer}/lo/{}", q.dash())])?;
            }
        }
        drop(p);
        out.save()?;
        note_cost("lo", &q.slash(), c);
        pipeline::control::done("lo", &q.slash());
        eprintln!(
            "lo {} ({}/{}): {} ways, {} road tiles ({:.0?})",
            q.slash(),
            k + 1,
            qs.len(),
            staged.len(),
            roads.len(),
            t0.elapsed()
        );
    }
    pipeline::agent::jobs::report(n, n, "zoomed-out tiles");
    Ok(())
}

// ---- catalog --------------------------------------------------------------------------------

/// The map's meta, added up from the units' summaries: each base pack's own, else worked out from
/// its sections once (packs without it) and kept in `state/build/summaries.json` by content
/// name (a content name never changes).
/// `now`: the catalog's moment (seconds since the epoch), its `built`.
fn units_meta(out: &Out, base: &BTreeMap<String, String>, now: u64) -> Result<serde_json::Value> {
    use pipeline::summary::Summary;
    let path = out.root().join("state/build/summaries.json");
    let mut known: BTreeMap<String, Summary> =
        std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut total = Summary::default();
    let mut used: std::collections::BTreeSet<String> = Default::default();
    let mut made = 0;
    for logical in base.values() {
        let content = out.get(logical).with_context(|| format!("no file for {logical}"))?.to_string();
        if !known.contains_key(&content) {
            let r = store::sect::SectReader::open(store::range::PlainFile::open(&out.path(&content))?)?;
            let s = match r.meta().get("summary").and_then(|v| serde_json::from_value::<Summary>(v.clone()).ok()) {
                Some(s) => s,
                None => {
                    let (ways, verts) = (r.read_pod::<roadcore::WayRec>("ways")?, r.read_pod::<[i32; 2]>("verts")?);
                    if r.section("elevu").is_some() {
                        Summary::of(&ways, &verts, roadcore::elev::Elevs::U16(&r.read_pod::<u16>("elevu")?))
                    } else {
                        Summary::of(&ways, &verts, roadcore::elev::Elevs::I16(&r.read_pod::<i16>("elev")?))
                    }
                }
            };
            known.insert(content.clone(), s);
            made += 1;
        }
        total.add(&known[&content]);
        used.insert(content);
    }
    if made > 0 || known.len() != used.len() {
        known.retain(|c, _| used.contains(c));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&known)?)?;
        std::fs::rename(&tmp, &path)?;
        eprintln!("catalog: {made} unit summaries made from their packs");
    }
    let mut m = total.meta();
    m["built"] = serde_json::json!(now);
    Ok(m)
}

/// A layer's zoom range as its builders make it.
fn layer_zooms(layer: &str) -> Option<(u8, u8)> {
    Some(match layer {
        "roads" | "rails" => (4, 14),
        "terrain" | "labels" => (0, 12),
        // Stored to z9; the server makes deeper ones on demand (pipeline::water).
        "water" => (0, pipeline::water::STORED_MAXZ),
        // Stored to z11; the server makes z12 on demand.
        "slope" => (0, 11),
        l if l.starts_with("trees-") => (4, 12),
        // Analysis grids, not served.
        l if l.starts_with("grid-") => (11, 11),
        // Thinned tiles; z6 blocks come from markdata.
        l if l.starts_with("marks-") => (0, 5),
        l if l.starts_with("ov-") || l == "stations" => (0, pipeline::ovconv::MAXZ),
        // Blocks at zooms 0, 3 and 6 (the app picks one by the view's zoom).
        "ferries" => (0, 6),
        // The 3D buildings: z14 hi packs (pipeline::bld).
        "buildings" => (pipeline::bld::ZOOM, pipeline::bld::ZOOM),
        // Their field: levels 9–14 in the hi packs, 4–8 in the lo (pipeline::bld::field).
        "buildings-field" => (pipeline::bld::field::LEVEL_MIN, pipeline::bld::field::LEVEL_MAX),
        // Depth: z3–8 in the lo packs, z9–16 in the hi where a source has that detail; the server
        // makes the others from their ancestors (pipeline::depth).
        pipeline::depth::LAYER | pipeline::depth::CLASS_LAYER => pipeline::depth::ZOOMS,
        _ => return None,
    })
}

/// The catalog's `global` names besides `global/…` files: the latest pass's outlines, for the
/// Regions panel and the modules (exactly `sources/osm/<date>/outlines`: the pass's outline set,
/// sets/outlines, is its input), and the lakes filled from Canada's own data made from the latest
/// pass's basemap, which the server draws into the water's deeper zooms
/// (pipeline::lakefill::current).
fn worldwide_globals(m: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut g = BTreeMap::new();
    if let Some(l) =
        m.keys().filter(|k| matches!(k.split('/').collect::<Vec<_>>()[..], ["sources", "osm", _, "outlines"])).max()
    {
        g.insert("outlines".to_string(), l.clone());
    }
    if let Some((l, _)) = pipeline::lakefill::current(m) {
        g.insert("lakefill".to_string(), l);
    }
    g
}

/// Whether the catalog lists logical `l` among its files: only what the map reads (build sources,
/// the planet's pieces, sets and road values, stay out, or every Mac's mirror would copy them).
fn served_file(l: &str, basemap: &[String], global: &BTreeMap<String, String>) -> bool {
    (l.starts_with("layers/") && (!l.starts_with("layers/basemap/") || basemap.iter().any(|b| b == l)))
        || l.starts_with("base/")
        || l.starts_with("hidata/")
        || l.starts_with("markdata/")
        || l.starts_with("ovdata/")
        || l.starts_with("global/")
        || global.values().any(|g| g == l)
}

/// How a catalog is written (`--delta`, `--whole`).
#[derive(Clone, Copy, Default)]
struct CatalogWay {
    /// Served (not held): as a delta over the last, unless asked whole, there's no whole one to
    /// start from or the run of deltas nears where readers stop (store::catalog::chain_full); each
    /// file's size from the records (crate::out::SIZES) or the last catalog (by content name: the
    /// same name, the same bytes), the NAS looked at only for a file neither has.
    delta: bool,
    whole: bool,
}

/// `now`: the catalog's moment (seconds since the epoch), its `created` and its meta's `built`.
fn catalog(
    out: &mut Out,
    held: bool,
    ready: Option<&BTreeMap<String, Option<String>>>,
    way: CatalogWay,
    now: u64,
) -> Result<pipeline::pubtimes::CatalogStats> {
    let delta = way.delta && !held;
    let p = phase("the manifest's files sorted into the catalog's", Class::Compute);
    let mut layers: BTreeMap<String, LayerOut> = BTreeMap::new();
    let (mut base, mut roads, mut hidata, mut global, mut basemap) =
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), Vec::new());
    let mut markdata = BTreeMap::new();
    let mut ovdata = BTreeMap::new();
    // The basemap: the newest pass's worldwide archive.
    let world = out.manifest.keys().filter(|k| k.starts_with("layers/basemap/world-")).max().cloned();
    for logical in out.manifest.keys() {
        let parts: Vec<&str> = logical.split('/').collect();
        match parts.as_slice() {
            ["layers", "basemap", _] => {
                if world.as_deref() == Some(logical.as_str()) {
                    basemap.push(logical.clone());
                }
            }
            ["layers", layer, scope, key] => {
                let u = Unit::parse(key).context("pack key")?;
                let enc = match *layer {
                    "roads" | "rails" => "rt7",
                    "terrain" => "terrarium-png",
                    "slope" => "slope4-png",
                    "labels" => "mvt",
                    "water" => "water-png",
                    l if l.starts_with("trees-") => "terrarium-webp",
                    l if l.starts_with("grid-") => "u8-zstd",
                    l if l.starts_with("marks-") => "rdmt",
                    l if l.starts_with("ov-") || l == "stations" || l == "buildings" => "mvt",
                    "ferries" => "geojson-gz",
                    "buildings-field" => pipeline::bld::field::ENCODING,
                    pipeline::depth::LAYER => pipeline::depth::ENCODING,
                    pipeline::depth::CLASS_LAYER => pipeline::depth::CLASS_ENCODING,
                    _ => "unknown",
                };
                let zs = match *scope {
                    "root" => (0, 2),
                    "lo" => (3, 8),
                    _ => (9, 14),
                };
                layers.entry(layer.to_string()).or_insert_with(|| LayerOut::new(enc)).add(
                    scope,
                    (u.z, u.x, u.y),
                    logical.clone(),
                    zs,
                );
            }
            ["base", u] => {
                base.insert(Unit::parse(u).context("unit")?.slash(), logical.clone());
            }
            ["global", "roads", u] => {
                roads.insert(Unit::parse(u).context("unit")?.slash(), logical.clone());
            }
            ["hidata", t] => {
                hidata.insert(Unit::parse(t).context("tile")?.slash(), logical.clone());
            }
            ["markdata", t] => {
                markdata.insert(Unit::parse(t).context("tile")?.slash(), logical.clone());
            }
            ["ovdata", t] => {
                ovdata.insert(Unit::parse(t).context("tile")?.slash(), logical.clone());
            }
            ["global", ..] => {
                global.insert(logical.trim_start_matches("global/").to_string(), logical.clone());
            }
            _ => {}
        }
    }
    // Zoom ranges as each layer is defined (whichever packs exist yet), else the packs' scopes.
    for (l, x) in layers.iter_mut() {
        if let Some(zs) = layer_zooms(l) {
            (x.minzoom, x.maxzoom) = zs;
        }
    }
    global.extend(worldwide_globals(&out.manifest));
    drop(p);
    let p = phase("the units' summaries read", Class::NasRead);
    let mut meta = units_meta(out, &base, now)?;
    // (The depth's sources as the map says them: each by the code its class tiles carry, with its
    // label, datum and the notice its licence asks for.)
    if layers.contains_key(pipeline::depth::CLASS_LAYER) {
        let d = pipeline::depth::grids::declared(out.root(), &out.manifest)?;
        let list: Vec<serde_json::Value> = d.iter().map(|s| serde_json::json!({"code": s.code, "id": s.id, "label": s.label, "datum": s.datum, "water": if s.lake { "lake" } else { "sea" }, "notice": s.notice})).collect();
        if let Some(o) = meta.as_object_mut() {
            o.insert("depth".into(), serde_json::json!({"sources": list}));
        }
    }
    drop(p);
    let p = phase("the coverage drawn and the credits found", Class::NasRead);
    let units: Vec<String> = base.keys().cloned().collect();
    // The coverage it's built for, drawn from the outlines it lists, and the credits of the sources
    // its data comes from: where the coverage is, and where the units' ways are.
    let dir = out.root().join(if held { "catalog-held" } else { "catalog" });
    // The last catalog there, whole (the first held one goes by the served one).
    let mut last = store::catalog::latest(&dir).with_context(|| format!("the last catalog in {}", dir.display()))?;
    if last.is_none() && held {
        last = store::catalog::latest(&out.root().join("catalog")).context("the last catalog")?;
    }
    let regions = catalog_coverage(out, global.get("outlines").map(String::as_str), ready, last.as_ref())?;
    // (The inputs' accepted descriptions' credits beside the table's: pipeline::inputs::credits;
    // one that can't be read now fails the catalog, as the agent's plan waits then, rather than
    // publish credits without it.)
    let described = pipeline::inputs::credits::described(out.root(), &out.manifest)?;
    // (The depth's sources whose grids meet the depth's packs: credited wherever they are.)
    let depth_credited: Vec<(String, String)> = if layers.contains_key(pipeline::depth::LAYER) {
        let packs: Vec<[f64; 4]> = out
            .manifest
            .keys()
            .filter_map(|l| l.strip_prefix(&format!("layers/{}/lo/", pipeline::depth::LAYER)))
            .filter_map(Unit::parse)
            .map(|u| {
                let b = pipeline::hipack::tile_bounds(u.z, u.x, u.y);
                [f64::from(b[0]) * 1e-7, f64::from(b[1]) * 1e-7, f64::from(b[2]) * 1e-7, f64::from(b[3]) * 1e-7]
            })
            .collect();
        pipeline::depth::grids::declared(out.root(), &out.manifest)?
            .iter()
            .filter(|s| packs.iter().any(|b| s.meeting(*b).next().is_some()))
            .map(|s| (pipeline::inputs::sources::UNIT.to_string(), format!("{}.toml", s.id)))
            .collect()
    } else {
        Vec::new()
    };
    let credits = pipeline::inputs::credits::catalog_credits_with(
        &regions,
        &unit_extents(out, &base),
        &described,
        &depth_credited,
    );
    eprintln!(
        "catalog: {} regions, {} credits (of the table's {} and {} description{})",
        regions.len(),
        credits.len(),
        pipeline::rules::CREDITS.len(),
        described.len(),
        if described.len() == 1 { "" } else { "s" }
    );
    // Only what the map reads: build sources (the planet's pieces, sets and road values) stay out,
    // or every Mac's mirror would copy them. A file missing on the NAS stops a whole catalog (a
    // delta takes its new files' sizes from the records and doesn't look).
    let served = |l: &str| served_file(l, &basemap, &global);
    drop(p);
    // (A delta (`--delta`, served, not asked whole, with a whole catalog to start from and its run
    // of deltas short of where readers stop: store::catalog::chain_full) lists each file's size as
    // the records have it (crate::out::SIZES, from its job's hand-off) or the last catalog does (by
    // content name: the same name, the same bytes), looking at the NAS only for a file neither has
    // (one written before the records kept sizes: once, the next catalog has it from this one). A
    // whole catalog looks at every file, its check that each is there and of its recorded size:
    // made in idle time (`--whole`, the agent's when store::catalog::whole_due), or when a delta
    // can't be.)
    let p = phase("the last catalogs listed and the records' sizes read", Class::NasRead);
    let chain = if delta && !way.whole { store::catalog::deltas_since_whole(&dir)? } else { None };
    let as_delta = last.is_some() && chain.is_some_and(|c| !store::catalog::chain_full(c));
    if chain.is_some_and(store::catalog::chain_full) {
        eprintln!("catalog: {} deltas since the last whole catalog: this one whole", chain.unwrap_or(0));
    }
    let recorded = out.recorded_sizes()?;
    let mut known: BTreeMap<&str, u64> =
        last.iter().flat_map(|c| c.files.values().map(|f| (f.file.as_str(), f.size))).collect();
    known.extend(recorded.iter().map(|(c, n)| (c.as_str(), *n)));
    let all: Vec<(&String, &String)> = out.manifest.iter().filter(|(l, _)| served(l)).collect();
    drop(p);
    let p = phase(if as_delta { "the new files' sizes" } else { "every file checked on the NAS" }, Class::NasRead);
    let listed: Vec<(&String, &String)> =
        all.iter().copied().filter(|(_, n)| !as_delta || !known.contains_key(n.as_str())).collect();
    if as_delta {
        let from_records = all.iter().filter(|(_, n)| recorded.contains_key(n.as_str())).count();
        eprintln!(
            "catalog: {} of {} files' sizes from the records, {} more from catalog {}, {} looked at on the NAS",
            from_records,
            all.len(),
            all.len() - listed.len() - from_records,
            last.as_ref().map_or(0, |c| c.n),
            listed.len()
        );
    }
    // (Many at once: over SMB each look waits a round trip, 5 to 17 ms, and the share answers
    // thirty-odd together about as fast as one; the first missing, in the manifest's order, stops it.)
    let total = listed.len() as u64;
    let done = std::sync::atomic::AtomicU64::new(0);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let sizes: Vec<std::sync::OnceLock<std::io::Result<u64>>> =
        (0..listed.len()).map(|_| std::sync::OnceLock::new()).collect();
    std::thread::scope(|s| {
        for _ in 0..32 {
            s.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((_, n)) = listed.get(k) else { break };
                sizes[k].set(std::fs::metadata(out.path(n)).map(|m| m.len())).ok();
                let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if d % (total / 100).max(1) == 0 {
                    pipeline::agent::jobs::report(d, total, "files checked on the NAS");
                }
            });
        }
    });
    let mut files: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for ((l, n), size) in listed.into_iter().zip(sizes) {
        let size =
            size.into_inner().context("not checked")?.with_context(|| format!("{l}: {n} is missing on the NAS"))?;
        // (Whole: a file of another size than the records or the last catalog say is an error.)
        if let Some(&k) = known.get(n.as_str()) {
            anyhow::ensure!(size == k, "{l}: {n} is {size} bytes on the NAS, {k} as recorded");
        }
        files.insert(l.clone(), serde_json::json!({"file": n, "size": size, "fmt": 1}));
        p.count(0, 1);
    }
    let rest: Vec<(&String, &String)> = all.iter().copied().filter(|(l, _)| !files.contains_key(*l)).collect();
    for (l, n) in rest {
        files.insert(l.clone(), serde_json::json!({"file": n, "size": known[n.as_str()], "fmt": 1}));
    }
    drop(p);
    let _p = phase("the catalog written", Class::NasWrite);
    std::fs::create_dir_all(&dir)?;
    let n = store::catalog::next_n(&dir)?;
    let cat = serde_json::json!({
        "fmt": 1,
        "n": n,
        "created": utc(now as i64),
        // The published app that made it (its folder's name), or "development".
        "app": std::env::current_exe().ok().and_then(|e| e.parent().map(pipeline::agent::app_version)).unwrap_or_else(|| "development".into()),
        "files": files,
        "units": units,
        "layers": layers,
        "basemap": basemap,
        "base": base,
        "roads": roads,
        "hidata": hidata,
        "markdata": markdata,
        "ovdata": ovdata,
        "global": global,
        "meta": meta,
        "credits": credits,
        "dem_labels": pipeline::inputs::credits::dem_labels(&described),
        // (`recorded`: this catalog says which regions it's built for, even none of them yet; one
        // made before catalogs did has none, and the server draws the recipes for it.)
        "coverage": {"regions": regions, "recorded": true},
    });
    let catalog: store::catalog::Catalog = serde_json::from_value(cat)?;
    // (As a delta over the last whole-read catalog, however large: the whole one comes in idle
    // time, store::catalog::whole_due.)
    let as_delta = match last.as_ref().filter(|_| as_delta) {
        Some(base) => {
            let d = store::catalog::Delta::between(base, &catalog)?;
            let size = d.encode()?.len() as u64;
            Some((base, d, size))
        }
        None => None,
    };
    let delta_written = as_delta.is_some();
    let path = match as_delta {
        Some((base, d, size)) => {
            eprintln!(
                "catalog: {n} as a delta over {} ({size} bytes; changed: {})",
                base.n,
                store::catalog::delta::changed_keys(&d.patch).into_iter().collect::<Vec<_>>().join(", ")
            );
            store::catalog::write_delta_of(&dir, base, &catalog, &d)?
        }
        None => store::catalog::write(&dir, &catalog)?,
    };
    eprintln!("published {}", path.display());
    // (What it did, for publishing's times (pipeline::pubtimes): its phases once written, the
    // write's whole.)
    drop(_p);
    Ok(pipeline::pubtimes::CatalogStats::now(n, delta_written, all.len() as u64, total))
}

/// The coverage a catalog records: the regions built as their recipes are now (`ready`; without it
/// every recipe), each outline entry simplified for drawing (pipeline::coverage::drawn), `osm:`
/// ones from `outlines` (the catalog's own); and a region not built yet as the last catalog in
/// `catalogs` had it, if it had it (on the map as it was: its old outline, its units not yet rebuilt),
/// in the recipes' order. So the regions are those the catalog's data is built for; the Regions
/// panel shows a recipe it lacks, or has with another outline, as still to come.
/// A read that fails (the NAS) fails the catalog, to be tried again, rather than record a region
/// without its outline or leave a region out.
fn catalog_coverage(
    out: &Out,
    outlines: Option<&str>,
    ready: Option<&BTreeMap<String, Option<String>>>,
    last: Option<&store::catalog::Catalog>,
) -> Result<Vec<pipeline::coverage::DrawnRegion>> {
    use pipeline::inputs::regions::read;
    let _ = outlines;
    let src = read::source(out.root(), &out.manifest)?;
    let recipes = read::recipes(&src)?;
    let Some(ready) = ready else { return read::drawn(out.root(), &src, &recipes) };
    // (Built with the outline its recipe has now: one redrawn since the plan said so isn't.)
    let is_ready = |r: &pipeline::agent::recipes::Recipe| {
        ready
            .get(&r.id)
            .is_some_and(|d| d.as_ref().is_none_or(|d| *d == pipeline::agent::recipes::outline_digest(&r.outline)))
    };
    let built: Vec<_> = recipes.iter().filter(|r| is_ready(r)).cloned().collect();
    let mut drawn: BTreeMap<String, pipeline::coverage::DrawnRegion> =
        read::drawn(out.root(), &src, &built)?.into_iter().map(|d| (d.id.clone(), d)).collect();
    // (The first held catalog keeps the regions as the served one has them: `last`, the caller's.)
    let had: BTreeMap<String, pipeline::coverage::DrawnRegion> = last
        .and_then(|c| {
            c.coverage
                .get("regions")
                .and_then(|v| serde_json::from_value::<Vec<pipeline::coverage::DrawnRegion>>(v.clone()).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect();
    let kept = recipes.iter().filter(|r| !is_ready(r) && had.contains_key(&r.id)).count();
    eprintln!("catalog: {} regions built as they are now, {kept} as the last catalog had them", drawn.len());
    Ok(recipes.iter().filter_map(|r| drawn.remove(&r.id).or_else(|| had.get(&r.id).cloned())).collect())
}

/// Where each built unit's ways are (E7): its summary's extent, as units_meta left the summaries in
/// state/build/summaries.json by content name, else its tile.
fn unit_extents(out: &Out, base: &BTreeMap<String, String>) -> Vec<[i32; 4]> {
    let known: BTreeMap<String, pipeline::summary::Summary> =
        std::fs::read(out.root().join("state/build/summaries.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    base.iter()
        .filter_map(|(u, logical)| match out.get(logical).and_then(|c| known.get(c)) {
            // (An empty unit's extent is inverted: nothing of it is anywhere.)
            Some(s) => (s.extent[0] <= s.extent[2]).then_some(s.extent),
            None => Unit::parse(u).map(|u| tile_bounds(u.z, u.x, u.y)),
        })
        .collect()
}

/// Target `target` of `step` begins, for `note_cost`: its time, and the job's peak memory started
/// again; noted for the agent (`SCENIC_COSTS`, a `started` line), which keeps what the job holds
/// while a target is under way as what that target takes at least (crate::agent::memguard).
fn cost_start(step: &str, target: &str) -> std::time::Instant {
    pipeline::sys::reset_group_peak();
    note_started(&pipeline::coord::cost_key(step, target));
    // (A tile's area, among its inputs' measures: pipeline::costin.)
    if let Some(u) = Unit::parse(target) {
        pipeline::costin::set("km2", pipeline::costin::tile_km2(u.z, u.x, u.y));
    }
    std::time::Instant::now()
}

/// The bytes this process had added to the caches as its last target began (store::cachefile::
/// written), for the disk its next cost line notes.
static WRITTEN_AT_START: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The disk the target under way has taken so far (MB): the bytes this process added to the caches
/// since it began (store::cachefile counts what its accessors write: docs/pool.md §7.7).
fn disk_since_start() -> u64 {
    store::cachefile::written().saturating_sub(WRITTEN_AT_START.load(std::sync::atomic::Ordering::Relaxed)) >> 20
}

/// Notes that the target whose cost key is `key` begins (`SCENIC_COSTS`).
fn note_started(key: &str) {
    WRITTEN_AT_START.store(store::cachefile::written(), std::sync::atomic::Ordering::Relaxed);
    pipeline::costin::begin();
    let Some(p) = std::env::var_os("SCENIC_COSTS") else { return };
    let line = serde_json::json!({ "unit": key, "started": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) });
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("{key}: noting its start: {e}");
    }
}

/// What a job of `step` took for `target` (since its `cost_start`): the most memory the job's
/// processes held together meanwhile (sampled: a pool's workers summed, an earlier target's peak
/// not counted) and its time, noted for the coordinator (`SCENIC_COSTS`), which gives a helper only
/// work that fits its memory (crate::coord::job_peak); with its inputs' measures (pipeline::costin).
fn note_cost(step: &str, target: &str, t0: std::time::Instant) {
    let line = done_line(step, target, t0);
    let Some(p) = std::env::var_os("SCENIC_COSTS") else { return };
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("{step} {target}: noting what it cost: {e}");
    }
}

/// `note_cost`'s line (its inputs' measures taken: pipeline::costin).
fn done_line(step: &str, target: &str, t0: std::time::Instant) -> serde_json::Value {
    let sizes = pipeline::costin::take();
    serde_json::json!({ "unit": pipeline::coord::cost_key(step, target), "peak_mb": pipeline::sys::group_peak() >> 20, "secs": t0.elapsed().as_secs(), "v": pipeline::coord::cost_version(step), "disk_mb": disk_since_start(), "sizes": sizes })
}

/// The raw tiles a job fetched, packed onto the NAS (pipeline::rawpack): an archive of their own an
/// area, not a file a tile; kept here too, for the next jobs. Not packed now (the NAS away), they
/// wait in the cache for the next job or room-making.
fn pack_raw(out: &Out, dir: &Path) {
    pack_raw_with(out, dir, &|_, _, _| {})
}

/// `pack_raw`, saying how far it is (`progress` lines for the status: the tiles packed, then the
/// areas whose archives were merged).
fn pack_raw_with(out: &Out, dir: &Path, progress: pipeline::rawpack::Progress) {
    let _p = phase("new raw tiles packed onto the NAS", Class::NasWrite);
    match pipeline::rawpack::pack_local_with(dir, &out.root().join("sources/aws-terrarium"), out.root(), true, progress)
    {
        Ok(_) => {}
        Err(e) => eprintln!("raw tiles: not packed now ({e:#}); they wait in the cache"),
    }
}

/// The terrain's raw tiles: the local cache `dir`, where each is downloaded once (from the declared
/// base, pipeline::terrain_sources), filled from the NAS's archives (`sources/aws-terrarium/packs/`)
/// when it lacks one.
fn raw_tiles(out: &Out, dir: &Path) -> Result<pipeline::terrain_pack::RawTiles> {
    let base = pipeline::terrain_sources::read(out)?.base;
    Ok(pipeline::terrain_pack::RawTiles::with_store(dir, &out.root().join("sources/aws-terrarium"), &base))
}

/// Seconds since the epoch as RFC 3339 UTC (`2026-10-05T03:55:11Z`).
fn utc(s: i64) -> String {
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// RFC 3339 UTC as `utc` writes it (a catalog's `created`), as seconds since the epoch.
fn epoch_of(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86400 + hh * 3600 + mm * 60 + ss)
}

// ---- rekey-check -------------------------------------------------------------------------------

/// `rekey-check`: what re-keying the records for the units' new keys (pipeline::agent::rekey) would
/// do now, read from the NAS with nothing written (no scratch folder, no index kept): the units
/// re-keyed, and those left to build again, each with why. And the times that show its one
/// assumption: a unit re-keyed that reads an area's zoomed-out terrain (z8–z4) was built after the
/// build first made that area's (before, it may have staged the converted legacy tiles, which its
/// old key couldn't tell from the build's; docs/plan.md §8, A new key scheme): its base pack written
/// over an hour after that area's first lo pack after the conversion's (the first catalog's), by
/// that file's time or, if earlier, the catalog's that first listed it.
fn rekey_check(root: &Path, args: &[String]) -> Result<()> {
    use pipeline::agent::{build, rekey, tiles::TerrainTiles};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(root)).context("no complete OSM pass")?;
    let m: BTreeMap<String, String> = pipeline::out::read_record(&root.join("state/build/manifest.json"))?;
    let keys = build::Keys::load_strict(root)?;
    let src = pipeline::inputs::regions::read::source(root, &m)?;
    let (recipes, bad): (Vec<_>, Vec<(String, String)>) = (pipeline::inputs::regions::read::recipes(&src)?, Vec::new());
    anyhow::ensure!(bad.is_empty(), "regions that can't be read now (the agent re-keys nothing meanwhile): {bad:?}");
    let cov = pipeline::coverage::Coverage::accepted(root, &src, None)?;
    if build::reach_work(&date, &m, &keys).is_some() {
        println!("(the pass's reaches are to be made again: the agent re-keys nothing until they are)");
    }
    let reach = match pipeline::reach::Reaches::load(root, &m, &date) {
        Ok(Some(r)) => r,
        Ok(None) => bail!("the pass {date} has no reaches"),
        Err(e) => bail!("the pass's reaches: {e:?}"),
    };
    let digests = pipeline::agent::input_digests(root);
    let t0 = std::time::Instant::now();
    let mut tiles = TerrainTiles::new(None);
    let n = tiles.load(root, &m);
    println!(
        "pass {date}, {} regions; the terrain packs' indexes: {n} read in {:.1} s, {} not",
        recipes.len(),
        t0.elapsed().as_secs_f64(),
        tiles.unread().count()
    );
    for (c, why) in tiles.unread() {
        println!("  {c}: {why}");
    }
    let t1 = std::time::Instant::now();
    let old = rekey::v1::unit_keys(
        &cov,
        &date,
        &m,
        Some(&reach),
        &digests,
        tiles.buildings_release(&m).context("the declared buildings source can't be read")?,
    );
    let t_old = t1.elapsed().as_secs_f64();
    let new: BTreeMap<String, Option<String>> =
        build::unit_keys(&cov, &date, &m, Some(&reach), &tiles).into_iter().map(|(u, k)| (u.slash(), k)).collect();
    let t_new = t1.elapsed().as_secs_f64() - t_old;
    let times = rekey::FileTimes::new(root);
    let older = |x: u32, y: u32| times.hi_older(&m, x, y);
    let mut after = keys.clone();
    let r = rekey::rekey(&mut after, &cov, &date, &m, Some(&reach), &digests, &tiles, &times);
    println!(
        "(the old keys took {t_old:.1} s, the new {t_new:.1} s, the re-keying {:.1} s)",
        t1.elapsed().as_secs_f64() - t_old - t_new
    );
    // (Again on what it made: nothing to do, as each plan's re-keying finds once it's done.)
    let t2 = std::time::Instant::now();
    let mut twice = after.clone();
    let again = rekey::rekey(&mut twice, &cov, &date, &m, Some(&reach), &digests, &tiles, &times);
    println!(
        "a second pass: {} (in {:.1} s)",
        if again.changed() || twice != after { "it changed the records again" } else { "nothing to do" },
        t2.elapsed().as_secs_f64()
    );
    let rec = |u: Unit| keys.unit.get(&u.slash());
    let new_of = |u: Unit| new.get(&u.slash()).and_then(Option::as_ref);
    let current: Vec<Unit> = old.iter().filter(|(u, k)| rec(*u) == Some(k)).map(|(u, _)| *u).collect();
    let already = old.iter().filter(|(u, _)| rec(*u).is_some() && rec(*u) == new_of(*u)).count();
    let stale = old.iter().filter(|(u, k)| rec(*u).is_some_and(|x| x != k && Some(x) != new_of(*u))).count();
    let never = old.iter().filter(|(u, _)| rec(*u).is_none()).count();
    let built: BTreeSet<String> = old.iter().map(|(u, _)| u.slash()).collect();
    let words = |v: &[String]| if v.is_empty() { String::new() } else { format!(": {}", v.join(", ")) };
    println!(
        "units (worked out in {:.1} s): {} recorded, {} the coverage builds",
        t1.elapsed().as_secs_f64(),
        keys.unit.len(),
        old.len()
    );
    println!("  current under the old keys: {}", current.len());
    println!("    re-keyed: {} ({} of them without outputs{})", r.moved.len(), r.empty.len(), words(&r.empty));
    println!("    left to build again: {}", r.left.len());
    println!("    unknown now (an index unread): {}", r.unknown.len());
    println!("  under the new keys already: {already}");
    println!("  stale under the old keys (built again either way): {stale}");
    println!("  never built: {never}");
    println!(
        "  re-keyed to the declared datasets: {} (elevations), {} (spacing: their ways spaced as before)",
        r.elevation_moved.len(),
        r.spacing_moved.len()
    );
    println!(
        "  recorded, but not built by the coverage now (pruned in time): {}",
        keys.unit.keys().filter(|u| !built.contains(*u)).count()
    );
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for (_, why) in &r.left {
        for w in why {
            *kinds.entry(w.split(':').next().unwrap_or_default().to_string()).or_default() += 1;
        }
    }
    println!("left to build again ({} units), by why:", r.left.len());
    for (k, n) in &kinds {
        println!("  {k}: {n}");
    }
    for (u, why) in &r.left {
        println!("  {u}  {}", why.join("; "));
    }
    for (u, why) in &r.unknown {
        println!("  {u}  unknown now: {why}");
    }

    // The build's first zoomed-out terrain of each area it makes terrain for.
    let targets: BTreeSet<String> = build::coverage_tiles(&cov).keys().map(|q| format!("3/{}/{}", q.0, q.1)).collect();
    // The stale terrain hi packs: their z6 tile no longer near the coverage, or left by an earlier
    // run (older than the area's lo pack: its last run made no hi tiles for the piece).
    let pieces: BTreeSet<(u32, u32)> = build::coverage_tiles(&cov).into_values().flatten().collect();
    let hi: Vec<(u32, u32)> = m
        .keys()
        .filter_map(|l| l.strip_prefix("layers/terrain/hi/"))
        .filter_map(Unit::parse)
        .map(|t| (t.x, t.y))
        .collect();
    let left: Vec<String> = hi
        .iter()
        .filter(|t| pieces.contains(*t) && older(t.0, t.1) != Some(false))
        .map(|t| {
            format!("6/{}/{}{}", t.0, t.1, if older(t.0, t.1).is_none() { " (its files' times unread)" } else { "" })
        })
        .collect();
    let gone = hi.iter().filter(|t| !pieces.contains(*t)).count();
    println!("terrain hi packs: {}, {} of them stale: {gone} of z6 tiles the coverage left, {} an earlier run left (older than their area's lo pack){}", hi.len(), gone + left.len(), left.len(), words(&left));
    // (The served catalogs and those held for review, as they were made.)
    let mut cats: Vec<(i64, String, store::catalog::Catalog)> = Vec::new();
    for dir in ["catalog", "catalog-held"] {
        for n in store::catalog::list(&root.join(dir))? {
            match store::catalog::read_n(&root.join(dir), n) {
                Ok(c) => cats.push((epoch_of(&c.created).unwrap_or(i64::MAX), format!("{dir} {n}"), c)),
                Err(e) => println!("  ({dir}/{n} not read: {e:#})"),
            }
        }
    }
    cats.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let first = store::catalog::list(&root.join("catalog"))?
        .into_iter()
        .min()
        .map(|n| store::catalog::read_n(&root.join("catalog"), n))
        .transpose()?
        .context("no catalog")?;
    let lo_of = |c: &store::catalog::Catalog, q: &str| -> Option<String> {
        Some(c.files.get(c.layers.get("terrain")?.lo.get(q)?)?.file.clone())
    };
    let mtime = |content: &str| {
        std::fs::metadata(root.join(content))
            .and_then(|md| md.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
    };
    let mut first_new: BTreeMap<String, (i64, String)> = BTreeMap::new();
    for (at, name, c) in &cats {
        for q in c.layers.get("terrain").map(|l| l.lo.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
            let Some(content) = lo_of(c, &q).filter(|x| lo_of(&first, &q).as_ref() != Some(x)) else { continue };
            let t = mtime(&content).map_or(*at, |f| f.min(*at));
            if first_new.get(&q).is_none_or(|(x, _)| t < *x) {
                first_new.insert(q, (t, name.clone()));
            }
        }
    }
    println!("the build's first zoomed-out terrain of each area it makes terrain for (after catalog {}'s, the conversion's), written:", first.n);
    for q in &targets {
        match first_new.get(q) {
            Some((t, name)) => println!("  {q}  {} (first listed by {name})", utc(*t)),
            None => println!("  {q}  none yet"),
        }
    }
    let (mut reading, mut shown, mut moved) = (0, 0, 0);
    let mut not: Vec<String> = Vec::new();
    for &u in &current {
        let (Some(base), Some(ru)) = (m.get(&format!("base/{}", u.dash())), reach.get(u)) else { continue };
        let Ok(read) = build::unit_terrain_tiles(u, ru, &m, &tiles) else { continue };
        let areas: BTreeSet<String> = read
            .iter()
            .filter(|t| (4..=8).contains(&t.0))
            .map(|&(z, x, y, _)| format!("3/{}/{}", x >> (z - 3), y >> (z - 3)))
            .filter(|q| targets.contains(q))
            .collect();
        if areas.is_empty() {
            continue;
        }
        reading += 1;
        let is_moved = r.moved.contains(&u.slash());
        moved += is_moved as usize;
        let at = mtime(base);
        if at.is_some_and(|b| areas.iter().all(|q| first_new.get(q).is_some_and(|(t, _)| b - 3600 > *t))) {
            shown += 1;
        } else {
            let each: Vec<String> = areas
                .iter()
                .map(|q| format!("{q} {}", first_new.get(q).map_or("none".to_string(), |(t, _)| utc(*t))))
                .collect();
            not.push(format!(
                "  {} ({}), built {}; first of {}",
                u.slash(),
                if is_moved { "re-keyed" } else { "left to build" },
                at.map_or("?".to_string(), utc),
                each.join(", ")
            ));
        }
    }
    println!("units current under the old keys, with outputs, that read zoomed-out terrain (z8–z4) of those areas: {reading} ({moved} of them re-keyed)");
    println!("  built over an hour after the build first made each such area's: {shown}");
    println!("  not shown so: {}", not.len());
    for l in &not {
        println!("{l}");
    }
    Ok(())
}

// ---- p5-check ----------------------------------------------------------------------------------

/// `p5-check trees`: what switching tree cover from a z3 tile's whole run to pieces and assemblies
/// (pipeline::agent::rekey's trees rules) does now, read from the NAS with nothing written (no
/// scratch folder): each z3 tile's record, current or stale under the old scheme, whose packs made
/// it (the trees program, or trees.py before it, by their files' times), and so re-keyed (its
/// pieces and assembly recorded, their mids made in idle time, expected the same) or made again as
/// pieces; then the work the plan has (agent::build::tree_work), and its time by the z3 tiles' last
/// runs (`--costs`: the coordinator's costs.json; at this Mac's pace, another Mac's time
/// halved) and the packs made again (their size now).
/// `p5-check terrain`: what switching terrain and slope from a z3 tile's whole run to pieces and
/// assemblies (pipeline::agent::rekey::derive) does now: each z3 tile's records read as its pieces
/// and assembly, or not and why; slope's pieces left to make (reading another area's terrain); the
/// work after the switch, and its time by the areas' last runs (`--costs`: the coordinator's
/// costs.json); and what deriving costs a plan. Reads only: the terrain packs' indexes into memory.
fn p5_check_terrain(root: &Path, args: &[String]) -> Result<()> {
    use pipeline::agent::{build, rekey, tiles::TerrainTiles};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(root)).context("no complete OSM pass")?;
    let m: BTreeMap<String, String> = pipeline::out::read_record(&root.join("state/build/manifest.json"))?;
    let keys = build::Keys::load_strict(root)?;
    let src = pipeline::inputs::regions::read::source(root, &m)?;
    let (recipes, bad): (Vec<_>, Vec<(String, String)>) = (pipeline::inputs::regions::read::recipes(&src)?, Vec::new());
    anyhow::ensure!(bad.is_empty(), "regions that can't be read now: {bad:?}");
    let cov = pipeline::coverage::Coverage::accepted(root, &src, None)?;
    let costs: BTreeMap<String, pipeline::coord::Cost> = match opt(args, "--costs") {
        Some(p) => pipeline::out::read_record(Path::new(&p))?,
        None => BTreeMap::new(),
    };
    let me = pipeline::agent::cond::host();
    // (An area's last whole run of `step`, at this Mac's pace.)
    let last_run = |step: &str, q: &str| {
        costs.get(&pipeline::coord::cost_key(step, q)).map(|c| {
            if c.worker.as_deref().is_none_or(|w| w == me) {
                c.secs as f64
            } else {
                c.secs as f64 / 2.0
            }
        })
    };
    let t0 = std::time::Instant::now();
    let mut tiles = TerrainTiles::new(None);
    let n = tiles.load(root, &m);
    anyhow::ensure!(
        tiles.unread().next().is_none(),
        "terrain packs' indexes unread: {:?}",
        tiles.unread().collect::<Vec<_>>()
    );
    println!(
        "pass {date}, {} regions; the terrain packs' {n} indexes read into memory ({:.1} s)",
        recipes.len(),
        t0.elapsed().as_secs_f64()
    );
    let z3 = |map: &BTreeMap<String, String>| map.keys().filter(|t| Unit::parse(t).is_some_and(|u| u.z == 3)).count();
    println!(
        "records: terrain {} z3 tiles' whole runs (the old scheme), {} pieces, {} assemblies; slope {} z3, {} pieces, {} assemblies",
        z3(&keys.terrain),
        keys.terrain.len() - z3(&keys.terrain),
        keys.terrain_lo.len(),
        z3(&keys.slope),
        keys.slope.len() - z3(&keys.slope),
        keys.slope_lo.len()
    );
    // Deriving, as a plan does: the first time (the slope pieces' reads worked out), then again (as
    // each plan after: kept with the packs they read).
    let t1 = std::time::Instant::now();
    let mut read = keys.clone();
    let d = rekey::derive(&mut read, &cov, &m, &tiles);
    let first = t1.elapsed();
    let t2 = std::time::Instant::now();
    let mut again = keys.clone();
    let d2 = rekey::derive(&mut again, &cov, &m, &tiles);
    let next = t2.elapsed();
    let mut twice = read.clone();
    let d3 = rekey::derive(&mut twice, &cov, &m, &tiles);
    // (What a plan works out anyway: the pieces' and assemblies' keys.)
    let t3 = std::time::Instant::now();
    let _ = build::terrain_slope_targets(&cov, &m, &tiles);
    let targets = t3.elapsed();
    println!(
        "derived: terrain {} z3 tiles as their pieces and assembly, slope {}; {} slope pieces left to make; passed over (stale under the old scheme): {}; unknown now: {}",
        d.terrain.len(),
        d.slope.len(),
        d.slope_left.len(),
        d.stale.len(),
        d.unknown.len()
    );
    println!(
        "deriving took {:.0} ms the first time, {:.0} ms again (a plan's; of which the targets' keys, which the plan works out anyway, {:.0} ms); again from the records: {}; on what it read: {}",
        first.as_secs_f64() * 1e3,
        next.as_secs_f64() * 1e3,
        targets.as_secs_f64() * 1e3,
        if d2 == d && again == read { "the same" } else { "DIFFERENT" },
        if d3 == rekey::Derived::default() && twice == read { "nothing to do" } else { "IT CHANGED THEM AGAIN" }
    );
    for (t, why) in &d.stale {
        println!("  {t}: {why}");
    }
    for (q, why) in &d.unknown {
        println!("  slope {q}: kept for the next pass: {why}");
    }
    // Each area.
    let by_q = build::coverage_tiles(&cov);
    let (mut piece_s, mut slope_left_s, mut backfill_t, mut backfill_s, mut mids_needed) =
        (0.0f64, 0.0f64, 0.0f64, 0.0f64, Vec::new());
    for (q, ts) in &by_q {
        let qs = format!("3/{}/{}", q.0, q.1);
        let left: Vec<&(String, String)> =
            d.slope_left.iter().filter(|(t, _)| build::area_of(t).as_deref() == Some(qs.as_str())).collect();
        let (tr, sr) = (last_run("terrain", &qs), last_run("slope", &qs));
        let n = ts.len() as f64;
        if let Some(s) = tr {
            piece_s += s / n;
            backfill_t += s;
        }
        if let Some(s) = sr {
            slope_left_s += s / n * left.len() as f64;
            backfill_s += s / n * (n - left.len() as f64);
        }
        mids_needed.push(ts.len() - 1);
        println!(
            "  {qs}: {} pieces; terrain {}; slope {}{}; last runs: terrain {}, slope {}",
            ts.len(),
            if d.terrain.contains(&qs) { "derived" } else { "not derived" },
            if d.slope.contains(&qs) {
                format!("derived, {} of its pieces left to make", left.len())
            } else {
                "not derived".into()
            },
            left.first().map_or(String::new(), |(t, why)| format!(" (first: {t}, {why})")),
            tr.map_or("unknown".into(), |s| format!("{s:.0} s")),
            sr.map_or("unknown".into(), |s| format!("{s:.0} s"))
        );
    }
    // The work the plan has after the switch.
    let tt = build::terrain_slope_targets(&cov, &m, &tiles);
    let w = build::terrain_work(&tt, &m, &read);
    let idle = |step: &str| w.backfill.iter().filter(|b| b.step == step).map(|b| b.targets.len()).sum::<usize>();
    println!(
        "after the switch: terrain {} pieces and {} assemblies to make now; slope {} pieces runnable now ({} stale), {} assemblies runnable ({} stale); mids to make in idle time: terrain {}, slope {}",
        w.terrain.len(),
        w.terrain_lo.len(),
        w.slope.len(),
        w.slope_pieces_left.len(),
        w.slope_lo.len(),
        w.slope_lo_left.len(),
        idle("terrain"),
        idle("slope")
    );
    let mean = mids_needed.iter().sum::<usize>() as f64 / mids_needed.len().max(1) as f64;
    println!(
        "a stale terrain piece before its area's mids are made: its area's other pieces made again for their mids, {mean:.1} on average (at most {}); after the backfill, none",
        mids_needed.iter().max().unwrap_or(&0)
    );
    println!(
        "its time by the areas' last runs (at this Mac's pace, a piece its area's share): slope's pieces left {:.2} h; the backfill: terrain {:.1} h, slope {:.1} h; a terrain piece {:.0} s on average; the assemblies a minute each",
        slope_left_s / 3600.0,
        backfill_t / 3600.0,
        backfill_s / 3600.0,
        piece_s / by_q.len().max(1) as f64
    );
    Ok(())
}

// ---- water-key-check -----------------------------------------------------------------------------

/// `water-key-check [--pass d] [--compute [--save f] | --digests f] [--fill n]`: what keying the
/// terrain on its water's digests (agent::build::water_key) does with the records as they are,
/// reading only: the records read as every reader reads them (agent::rekey::derive) and the
/// terrain left to make; then, as if a new basemap came whose water is the same, and again with
/// `n` pieces' water changed (a lake fill's), what's left to make. The digests are the manifest's;
/// without them, `--compute` works them out here, in memory, timed (`--save f` keeps them in a
/// file, which `--digests f` reads instead).
fn water_key_check(root: &Path, args: &[String]) -> Result<()> {
    use pipeline::agent::{build, rekey, tiles::TerrainTiles};
    use pipeline::terrain_water::{idx_logical, WaterIdx};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(root)).context("no complete OSM pass")?;
    let mut m: BTreeMap<String, String> = pipeline::out::read_record(&root.join("state/build/manifest.json"))?;
    let keys = build::Keys::load_strict(root)?;
    let src = pipeline::inputs::regions::read::source(root, &m)?;
    let (recipes, bad): (Vec<_>, Vec<(String, String)>) = (pipeline::inputs::regions::read::recipes(&src)?, Vec::new());
    anyhow::ensure!(bad.is_empty(), "regions that can't be read now: {bad:?}");
    let cov = pipeline::coverage::Coverage::accepted(root, &src, None)?;
    let t0 = std::time::Instant::now();
    let mut tiles = TerrainTiles::new(None);
    let n = tiles.load(root, &m);
    anyhow::ensure!(
        tiles.unread().next().is_none(),
        "terrain packs' indexes or water digests unread: {:?}",
        tiles.unread().collect::<Vec<_>>()
    );
    println!(
        "pass {date}, {} regions; the terrain packs' {n} indexes read into memory ({:.1} s)",
        recipes.len(),
        t0.elapsed().as_secs_f64()
    );
    let z3 = |map: &BTreeMap<String, String>| map.keys().filter(|t| Unit::parse(t).is_some_and(|u| u.z == 3)).count();
    println!(
        "records: terrain {} z3 tiles' whole runs, {} pieces, {} assemblies",
        z3(&keys.terrain),
        keys.terrain.len() - z3(&keys.terrain),
        keys.terrain_lo.len()
    );
    let pin = pipeline::terrain_pack::water_source_pin(&m).context("no basemap")?;
    println!("the water's source: {pin}");
    // The digests: the manifest's, or worked out here.
    let held = |m: &mut BTreeMap<String, String>, tiles: &mut TerrainTiles, idx: WaterIdx| {
        let content = format!("{}.check.json", idx_logical(&idx.pin));
        m.insert(idx_logical(&idx.pin), content.clone());
        tiles.hold_water(&content, idx);
    };
    let idx: WaterIdx = match (tiles.water_idx(&m, &pin).cloned(), opt(args, "--digests")) {
        (Some(w), _) => {
            println!("its digests: the manifest's ({} targets)", w.digests.len());
            w
        }
        (None, Some(f)) => {
            let w = WaterIdx::from_bytes(&std::fs::read(&f)?)?;
            anyhow::ensure!(w.pin == pin, "{f} is {}'s digests", w.pin);
            println!("its digests: {f}'s ({} targets)", w.digests.len());
            held(&mut m, &mut tiles, w.clone());
            w
        }
        (None, None) => {
            anyhow::ensure!(
                args.iter().any(|a| a == "--compute"),
                "the manifest has no water digests for {pin}: --compute works them out here, in memory"
            );
            let src = pipeline::terrain_water::BasemapWater::open(&root.join(&pin), &pin)?;
            let t = std::time::Instant::now();
            let reads = build::water_reads(&cov);
            let wanted = t.elapsed();
            let ntiles: usize = reads.iter().map(|r| r.2.len()).sum();
            let pieces = reads.iter().filter(|r| r.0.starts_with("6/")).count();
            println!(
                "what the targets read worked out: {} pieces, {} assemblies, {ntiles} tiles ({:.1} s)",
                pieces,
                reads.len() - pieces,
                wanted.as_secs_f64()
            );
            let t = std::time::Instant::now();
            let w = pipeline::terrain_water::make_idx(&src, None, &reads, &|d, n| {
                if d % 50 == 0 || d == n {
                    eprintln!("  {d} of {n} targets ({:.0} s)", t.elapsed().as_secs_f64());
                }
            })?;
            let secs = t.elapsed().as_secs_f64();
            let none = w.digests.values().filter(|(_, d)| d == "-").count();
            println!(
                "its digests worked out here: {} targets ({none} without water) in {secs:.1} s, {:.0} tiles/s",
                w.digests.len(),
                ntiles as f64 / secs
            );
            // (A second time: the basemap's directory and this Mac's file cache warm.)
            let t = std::time::Instant::now();
            let again = pipeline::terrain_water::make_idx(&src, None, &reads, &|_, _| {})?;
            println!(
                "again: {:.1} s, {}",
                t.elapsed().as_secs_f64(),
                if again == w { "the same digests" } else { "DIFFERENT DIGESTS" }
            );
            if let Some(f) = opt(args, "--save") {
                std::fs::write(&f, serde_json::to_vec(&w)?)?;
                println!("kept in {f}");
            }
            held(&mut m, &mut tiles, w.clone());
            w
        }
    };
    // What's left to make, the records read as every reader reads them.
    let left = |m: &BTreeMap<String, String>,
                tiles: &TerrainTiles,
                what: &str|
     -> Result<(rekey::Derived, build::TerrainWork)> {
        let t = std::time::Instant::now();
        let mut read = keys.clone();
        let d = rekey::derive(&mut read, &cov, m, tiles);
        let first = t.elapsed();
        // (Again, as each plan after the first: kept with what it's worked out from.)
        let t = std::time::Instant::now();
        let mut again = keys.clone();
        let d2 = rekey::derive(&mut again, &cov, m, tiles);
        let next = t.elapsed();
        anyhow::ensure!(d2 == d && again == read, "deriving again gave other records");
        let tt = build::terrain_slope_targets(&cov, m, tiles);
        let w = build::terrain_work(&tt, m, &read);
        let unknown = tt.terrain.iter().chain(&tt.terrain_lo).filter(|(_, k)| k == build::UNKNOWN).count();
        println!("{what}:");
        println!(
            "  derived: {} z3 tiles as their pieces and assembly ({} from an earlier basemap), {} pieces and assemblies of the keys that named the basemap, {} pieces of the keys that named their sources in code; stale under the old scheme: {}; kept, their water unknown: {}; deriving {:.0} ms the first time, {:.0} ms again",
            d.terrain.len(),
            d.older.len(),
            d.water.len(),
            d.sources.len(),
            d.stale.len(),
            d.unknown.len(),
            first.as_secs_f64() * 1e3,
            next.as_secs_f64() * 1e3
        );
        for (t, why) in d.stale.iter().chain(&d.unknown) {
            println!("    {t}: {why}");
        }
        println!(
            "  terrain left to make: {} of {} pieces, {} of {} assemblies ({} keys unknown); areas whose terrain is to be made: {}",
            w.terrain_pieces_left.len(),
            tt.terrain.len(),
            w.terrain_lo_left.len(),
            tt.terrain_lo.len(),
            unknown,
            w.terrain_left.len()
        );
        if !w.terrain_pieces_left.is_empty() {
            println!("    pieces: {}", w.terrain_pieces_left.iter().cloned().collect::<Vec<_>>().join(" "));
        }
        if !w.terrain_lo_left.is_empty() {
            println!("    assemblies: {}", w.terrain_lo_left.iter().cloned().collect::<Vec<_>>().join(" "));
        }
        Ok((d, w))
    };
    let (_, now) = left(&m, &tiles, "now, the terrain keyed on its water's digests")?;
    // A new basemap, its water the same as this one's under every target: nothing to make.
    let mut m2 = m.clone();
    let next = "layers/basemap/world-2099-01-01";
    m2.insert(next.into(), format!("{next}.0123456789abcdef.pmtiles"));
    let pin2 = pipeline::terrain_pack::water_source_pin(&m2).unwrap();
    let mut tiles2 = TerrainTiles::new(None);
    tiles2.load(root, &m);
    held(&mut m2, &mut tiles2, idx.clone());
    held(&mut m2, &mut tiles2, WaterIdx { pin: pin2.clone(), ..idx.clone() });
    let (_, same) = left(&m2, &tiles2, "a new basemap whose water is the same")?;
    // And with some pieces' water changed (a lake fill's): those pieces, their areas' assemblies.
    let k: usize = opt(args, "--fill").map(|v| v.parse()).transpose()?.unwrap_or(10);
    let mut changed = WaterIdx { pin: pin2, ..idx.clone() };
    let touched: Vec<String> = changed
        .digests
        .keys()
        .filter(|t| t.starts_with("6/"))
        .step_by((idx.digests.len() / k.max(1)).max(1))
        .take(k)
        .cloned()
        .collect();
    for t in &touched {
        let e = changed.digests.get_mut(t).unwrap();
        e.1 = format!("{}+fill", e.1);
    }
    let mut tiles3 = TerrainTiles::new(None);
    tiles3.load(root, &m);
    let mut m3 = m2.clone();
    held(&mut m3, &mut tiles3, idx.clone());
    held(&mut m3, &mut tiles3, changed);
    let (_, fill) = left(
        &m3,
        &tiles3,
        &format!("a new basemap with {} pieces' water changed ({})", touched.len(), touched.join(" ")),
    )?;
    let want: BTreeSet<String> = touched.iter().cloned().chain(now.terrain_pieces_left.iter().cloned()).collect();
    println!(
        "the switch: {}; a new basemap with the same water: {}; with {} pieces' water changed: {}",
        if now.terrain_pieces_left.is_empty() && now.terrain_lo_left.is_empty() {
            "nothing to make".to_string()
        } else {
            format!("{} pieces and {} assemblies to make", now.terrain_pieces_left.len(), now.terrain_lo_left.len())
        },
        if same.terrain_pieces_left == now.terrain_pieces_left && same.terrain_lo_left == now.terrain_lo_left {
            "the same, nothing more"
        } else {
            "MORE TO MAKE"
        },
        touched.len(),
        if fill.terrain_pieces_left == want { "those pieces alone" } else { "OTHER PIECES TOO" }
    );
    Ok(())
}

fn p5_check(root: &Path, args: &[String]) -> Result<()> {
    use pipeline::agent::{build, rekey, tiles::TerrainTiles};
    match positional(args).first().map(String::as_str) {
        Some("trees") => {}
        Some("terrain") => return p5_check_terrain(root, args),
        _ => anyhow::bail!("p5-check <trees | terrain>"),
    }
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(root)).context("no complete OSM pass")?;
    let m: BTreeMap<String, String> = pipeline::out::read_record(&root.join("state/build/manifest.json"))?;
    let keys = build::Keys::load_strict(root)?;
    let src = pipeline::inputs::regions::read::source(root, &m)?;
    let (recipes, bad): (Vec<_>, Vec<(String, String)>) = (pipeline::inputs::regions::read::recipes(&src)?, Vec::new());
    anyhow::ensure!(bad.is_empty(), "regions that can't be read now (the agent re-keys nothing meanwhile): {bad:?}");
    let cov = pipeline::coverage::Coverage::accepted(root, &src, None)?;
    let costs: BTreeMap<String, pipeline::coord::Cost> = match opt(args, "--costs") {
        Some(p) => pipeline::out::read_record(Path::new(&p))?,
        None => BTreeMap::new(),
    };
    let me = pipeline::agent::cond::host();
    // (A z3 tile's last run, at this Mac's pace.)
    let last_run = |q: &str| {
        costs.get(&pipeline::coord::cost_key("trees", q)).map(|c| {
            if c.worker.as_deref().is_none_or(|w| w == me) {
                c.secs as f64
            } else {
                c.secs as f64 / 2.0
            }
        })
    };
    let t0 = std::time::Instant::now();
    let times = rekey::FileTimes::new(root);
    let z3: Vec<(String, String)> = keys
        .trees
        .iter()
        .filter(|(t, _)| Unit::parse(t).is_some_and(|u| u.z == 3))
        .map(|(t, k)| (t.clone(), k.clone()))
        .collect();
    println!("pass {date}, {} regions; tree cover's records: {} z3 tiles' whole runs (the old scheme), {} pieces, {} assemblies", recipes.len(), z3.len(), keys.trees.len() - z3.len(), keys.trees_lo.len());
    // The re-keying, its tree cover rules alone (no reaches: the units' are rekey-check's).
    let mut after = keys.clone();
    let r = rekey::rekey(&mut after, &cov, &date, &m, None, &BTreeMap::new(), &TerrainTiles::new(None), &times);
    let mut twice = after.clone();
    let again = rekey::rekey(&mut twice, &cov, &date, &m, None, &BTreeMap::new(), &TerrainTiles::new(None), &times);
    println!("re-keyed: {} z3 tiles as their pieces and assemblies; dropped: {}; unknown now: {}; a second pass: {} ({:.1} s)", r.trees_moved.len(), r.trees_dropped.len(), r.trees_unknown.len(), if again.changed() || twice != after { "it changed the records again" } else { "nothing to do" }, t0.elapsed().as_secs_f64());
    // Each z3 tile recorded: what it was, whose its packs are, what the switch does.
    let old: BTreeMap<String, String> = rekey::v1::trees_targets(&cov, &m).into_iter().collect();
    let trees_extra = pipeline::inputs::sources::open(root, &m)?
        .map(|s| pipeline::inputs::sources::trees_extra(&s))
        .transpose()?
        .flatten();
    let tt = pipeline::treepacks::targets_with(&cov, &m, trees_extra.as_deref());
    let mtime = |c: &str| {
        std::fs::metadata(root.join(c))
            .and_then(|md| md.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
    };
    let size = |c: &str| std::fs::metadata(root.join(c)).map(|md| md.len()).unwrap_or(0);
    let packs_of = |q: Unit| -> Vec<&String> {
        m.iter()
            .filter(|(l, _)| {
                pipeline::treepacks::LAYERS.iter().any(|layer| l.starts_with(&format!("layers/{layer}/")))
                    && (l.ends_with(&format!("/lo/{}", q.dash()))
                        || l.rsplit('/')
                            .next()
                            .and_then(Unit::parse)
                            .is_some_and(|u| u.z == 6 && (u.x >> 3, u.y >> 3) == (q.x, q.y)))
            })
            .map(|(_, c)| c)
            .collect()
    };
    let (mut rebuild, mut backfill_s, mut bytes, mut unknown_s) = (0.0f64, 0.0f64, 0u64, 0usize);
    for (q, k) in &z3 {
        let Some(u) = Unit::parse(q) else { continue };
        let state = match old.get(q) {
            Some(now) if now == k && *now == rekey::v1::trees_none(q) => "current, \"none\"",
            Some(now) if now == k => "current",
            Some(_) => "stale",
            None => "no longer a target",
        };
        let packs = packs_of(u);
        let lo = pipeline::treepacks::LAYERS.iter().find_map(|l| m.get(&format!("layers/{l}/lo/{}", u.dash())));
        let made = lo.and_then(|c| mtime(c.as_str())).map_or("?".to_string(), utc);
        let by = match times.trees_by_program(&m, u.x, u.y) {
            Some(true) => "the trees program",
            Some(false) => "trees.py",
            None => "? (its times unread)",
        };
        let n = tt.pieces_of(q).count();
        let mb = packs.iter().map(|c| size(c.as_str())).sum::<u64>();
        let run = last_run(q.as_str());
        let what = if r.trees_moved.contains(q) {
            backfill_s += run.unwrap_or(0.0);
            "re-keyed: its pieces and assembly recorded, their mids made in idle time".to_string()
        } else if let Some((_, why)) = r.trees_dropped.iter().find(|(t, _)| t == q) {
            if !why.contains("nothing left") {
                rebuild += run.unwrap_or(0.0);
                unknown_s += run.is_none() as usize;
                bytes += mb;
            }
            format!("dropped: {why}")
        } else {
            "kept for the next pass".to_string()
        };
        println!("  {q}: {state} under the old scheme; packs by {by} (lo pack written {made}); {n} pieces, {} packs, {:.0} MB; last run {}: {what}", packs.len(), mb as f64 / 1e6, run.map_or("unknown".to_string(), |s| format!("{s:.0} s")));
    }
    // The work the plan has after the switch.
    let w = build::tree_work(&tt, &m, &after);
    let areas: BTreeSet<String> = w.pieces.iter().filter_map(|(t, _)| pipeline::treepacks::area_of(t)).collect();
    println!(
        "after the switch, tree cover's work: {} pieces to make, in {} z3 tiles ({} of them current, their mids for an assembly), {} assemblies runnable now of {} stale, {} mids to backfill in idle time (expected the same)",
        w.pieces.len(),
        areas.len(),
        w.pieces.iter().filter(|(t, k)| after.trees.get(t) == Some(k)).count(),
        w.lo.len(),
        w.stale_lo.len(),
        w.backfill.len()
    );
    println!(
        "its time by the z3 tiles' last runs (at this Mac's pace): made again {:.1} h{}, the packs uploaded again {:.2} GB; the mids backfilled {:.1} h; the assemblies seconds each",
        rebuild / 3600.0,
        if unknown_s > 0 { format!(" ({unknown_s} z3 tiles' runs unknown)") } else { String::new() },
        bytes as f64 / 1e9,
        backfill_s / 3600.0
    );
    Ok(())
}

// ---- base(U) from the pass ------------------------------------------------------------------

/// The pass's road values of a unit: (OSM way id, record), sorted by id.
fn pass_roads(out: &Out, date: &str, u: Unit) -> Result<Vec<(u64, pipeline::legacy::RoadRec)>> {
    let Some(name) = out.get(&format!("sources/osm/{date}/roads/{}", u.dash())) else { return Ok(Vec::new()) };
    let b = std::fs::read(out.path(name))?;
    Ok(b.chunks_exact(32)
        .map(|c| (u64::from_le_bytes(c[..8].try_into().unwrap()), bytemuck::pod_read_unaligned(&c[8..32])))
        .collect())
}

/// pois <units…> [--pass <date>]: each unit's landmark candidates (pipeline::candidates), from
/// its piece through `extract --candidates` with the pass's hiking-route ends, as `work/pois/<u>`.
fn pois_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass on the NAS (--pass)")?,
    };
    let setup = phase("the regions and their outlines read", Class::NasRead);
    let (cov, _) = regions_coverage(out, args, &date, None)?;
    let trailends = out
        .get(&format!("work/trailends/{date}"))
        .map(|c| out.path(c))
        .context("no hiking-route ends for the pass (the trailends step)")?;
    let extract = std::env::current_exe()?.parent().context("bin")?.join("extract");
    std::fs::create_dir_all(scratch)?;
    let units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    drop(setup);
    for (k, &u) in units.iter().enumerate() {
        pipeline::control::safe_point("pois");
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas");
        let t = cost_start("pois", &u.slash());
        let Some(piece) = out.get(&format!("sources/osm/{date}/pieces/{}", u.dash())).map(|n| out.path(n)) else {
            eprintln!("pois {}: no piece", u.slash());
            continue;
        };
        let local = scratch.join(format!("piece-{}.osm.pbf", u.dash()));
        let p = phase("pieces copied from the NAS", Class::NasRead);
        let n = store::sys::copy_data(&piece, &local).with_context(|| format!("copy {}", piece.display()))?;
        p.count(n, 1);
        // (Its input's measure: the piece's bytes.)
        pipeline::costin::add("bytes", n);
        drop(p);
        let dir = scratch.join(format!("pois-{}", u.dash()));
        let p = phase("candidates extracted (extract)", Class::Compute);
        let mut c = store::spawn::command(&extract);
        c.arg(&dir).arg("8").arg("--candidates").arg("--trailends").arg(&trailends).arg(&local);
        let o = store::spawn::output(&mut c).with_context(|| format!("run {}", extract.display()))?;
        anyhow::ensure!(
            o.status.success(),
            "extract --candidates for {}: {}\n{}",
            u.slash(),
            o.status,
            String::from_utf8_lossy(&o.stderr).lines().rev().take(8).collect::<Vec<_>>().join("\n")
        );
        drop(p);
        let p = phase("candidates kept in the coverage and written", Class::Compute);
        let cands = pipeline::candidates::from_pois(&std::fs::read(dir.join("pois.json"))?, u, &cov)?;
        let file = scratch.join(format!("pois-{}.jsonl.zst", u.dash()));
        pipeline::candidates::write(&file, &cands)?;
        drop(p);
        let p = phase("candidates uploaded", Class::NasWrite);
        out.put_file(&format!("work/pois/{}", u.dash()), "jsonl.zst", &file)?;
        drop(p);
        out.save()?;
        pipeline::control::done("pois", &u.slash());
        let p = phase("scratch files removed", Class::Disk);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(&local).ok();
        std::fs::remove_file(&file).ok();
        drop(p);
        eprintln!("pois {}: {} candidates ({:.0?})", u.slash(), cands.len(), t.elapsed());
        note_cost("pois", &u.slash(), t);
    }
    Ok(())
}

/// A local copy of a NAS file by logical name, made once under `cache/<logical>/` (content-named,
/// so never stale; older copies go).
fn local_copy(out: &Out, logical: &str, cache: &Path) -> Result<PathBuf> {
    let content = out.get(logical).with_context(|| format!("no {logical} in the manifest"))?;
    let src = out.path(content);
    let name = Path::new(content).file_name().context("file name")?.to_string_lossy().into_owned();
    let dir = cache.join(logical.replace('/', "-"));
    let local = dir.join(&name);
    let size = std::fs::metadata(&src)?.len();
    // (Held for the job, copied first when it isn't here whole: store::cachefile. Content-named:
    // one here of another length was cut short.)
    let copy = &mut |tmp: &Path| {
        let n = store::sys::copy_data(&src, tmp)?;
        pipeline::timings::count(n, 1);
        if n != size {
            return Err(std::io::Error::other(format!("{n} of {size} bytes copied")));
        }
        Ok(())
    };
    store::cachefile::hold(&local, copy).with_context(|| format!("copy {}", src.display()))?;
    if std::fs::metadata(&local).map(|m| m.len()).ok() != Some(size) {
        store::cachefile::discard(&local);
        store::cachefile::hold(&local, copy).with_context(|| format!("copy {}", src.display()))?;
    }
    // (Other copies go, unless a job uses them.)
    for e in std::fs::read_dir(&dir)?.flatten() {
        if e.file_name().to_string_lossy() != name {
            store::cachefile::try_remove(&e.path());
        }
    }
    // Older passes' copies of the same file go too.
    if let Some(d) = logical.split('/').find(|s| pipeline::osmpass::is_date(s)) {
        let me = logical.replace('/', "-");
        let (pre, post) = me.split_once(d).context("date")?;
        for e in std::fs::read_dir(cache)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n != me
                && n.len() == me.len()
                && n.starts_with(pre)
                && n.ends_with(post)
                && pipeline::osmpass::is_date(&n[pre.len()..pre.len() + 10])
            {
                store::cachefile::remove_tree(&e.path());
            }
        }
    }
    Ok(local)
}

/// The z8 artifact, from local copies under `cache`.
fn open_z8(out: &Out, cache: &Path) -> Result<pipeline::terrain_z8::Z8> {
    let pack = local_copy(out, &pipeline::terrain_z8::logical(), cache)?;
    let maxes = local_copy(out, &pipeline::terrain_z8::max_logical(), cache)?;
    pipeline::terrain_z8::Z8::open(&pack, &maxes, 4096)
}

/// summits [--pass <date>] [--cache dir]: every summit worldwide with its z8 height (pipeline::summits).
fn summits_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cache =
        PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t = std::time::Instant::now();
    let parts = Parts(&[
        "Copying the summits",
        "Reading them",
        "Taking their heights from the worldwide z8 terrain",
        "Uploading",
    ]);
    parts.start(0);
    let p = phase("the summits set copied here", Class::NasRead);
    let set = local_copy(out, &pipeline::osmpass::set_name(&date, "summits"), &cache)?;
    drop(p);
    parts.start(1);
    let p = phase("the summits read", Class::Compute);
    let mut summits = pipeline::summits::read_set(&set)?;
    drop(p);
    parts.start(2);
    let p = phase("the worldwide z8 terrain copied here", Class::NasRead);
    let z8 = open_z8(out, &cache)?;
    drop(p);
    let p = phase("their z8 heights taken", Class::Compute);
    let raised = pipeline::summits::add_z8(&mut summits, &z8)?;
    drop(p);
    parts.start(3);
    let p = phase("written", Class::Disk);
    std::fs::create_dir_all(scratch)?;
    let file = scratch.join("summits.jsonl.zst");
    pipeline::summits::write(&file, &summits)?;
    drop(p);
    let p = phase("uploaded", Class::NasWrite);
    out.put_file(&format!("work/summits/{date}"), "jsonl.zst", &file)?;
    drop(p);
    out.save()?;
    std::fs::remove_file(&file).ok();
    eprintln!("summits: {} ({} with a z8 height) ({:.0?})", summits.len(), raised, t.elapsed());
    Ok(())
}

/// peaks <units…> [--pass <date>] [--raw dir] [--cache dir] [--coarse-threads n]: prominence and
/// isolation of each unit's peak candidates (pipeline::peaks::unit), as `work/peaks/<u>`.
fn peaks_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::peaks::unit;
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cache =
        PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let raw_dir =
        PathBuf::from(opt(args, "--raw").unwrap_or_else(|| cache.join("aws-terrarium").to_string_lossy().into_owned()));
    let raw = raw_tiles(out, &raw_dir)?;
    let coarse_threads: usize = opt(args, "--coarse-threads").map(|s| s.parse()).transpose()?.unwrap_or(4);
    let p = phase("the summits copied here and read", Class::NasRead);
    let summits = pipeline::summits::read(&local_copy(out, &format!("work/summits/{date}"), &cache)?)?;
    drop(p);
    let p = phase("the summits' z8 base made", Class::Compute);
    let base8 = unit::Z8Base::new(&summits);
    drop(p);
    let p = phase("the worldwide z8 terrain copied here", Class::NasRead);
    let z8 = open_z8(out, &cache)?;
    drop(p);
    std::fs::create_dir_all(scratch)?;
    let units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    // Its parts: the areas' peaks, then the raw tiles AWS gave packed onto the NAS.
    let parts = Parts(&["Measuring the areas' peaks", "Packing the new raw tiles onto the NAS"]);
    parts.start(0);
    for (k, &u) in units.iter().enumerate() {
        // (A safe point before each area: the raw tiles fetched wait in the cache for the next job.)
        pipeline::control::safe_point("peaks");
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas");
        let t = cost_start("peaks", &u.slash());
        let p = phase("candidates read", Class::NasRead);
        let pois = out
            .get(&format!("work/pois/{}", u.dash()))
            .map(|c| out.path(c))
            .with_context(|| format!("no candidates for {} (the pois step)", u.slash()))?;
        let peaks: Vec<unit::UnitPeak> = pipeline::candidates::read(&pois)?
            .into_iter()
            .filter(|c| c.kind == "peak")
            .filter_map(|c| Some(unit::UnitPeak { id: c.osm.clone()?, key: c.key, lon: c.lon, lat: c.lat, ele: c.ele }))
            .collect();
        drop(p);
        let want = unit::tiles_wanted(
            &peaks.iter().map(|p| (p.lon as f64 * 1e-7, p.lat as f64 * 1e-7)).collect::<Vec<_>>(),
            29.5,
        );
        let z12 = unit::UnitZ12::load(out, &raw, &want)?;
        let p = phase("prominence and isolation measured", Class::Compute);
        let res = unit::run(&peaks, &summits, &base8, &z12, &z8, coarse_threads)?;
        drop(p);
        let file = scratch.join(format!("peaks-{}.jsonl.zst", u.dash()));
        let p = phase("results written", Class::Disk);
        {
            use std::io::Write;
            let mut w = zstd::Encoder::new(std::fs::File::create(&file)?, 9)?.auto_finish();
            for (key, o) in &res {
                let mut v = o.json();
                v.as_object_mut().unwrap().remove("i");
                v["key"] = key.clone().into();
                serde_json::to_writer(&mut w, &v)?;
                w.write_all(b"\n")?;
            }
        }
        drop(p);
        let p = phase("results uploaded", Class::NasWrite);
        out.put_file(&format!("work/peaks/{}", u.dash()), "jsonl.zst", &file)?;
        drop(p);
        out.save()?;
        pipeline::control::done("peaks", &u.slash());
        std::fs::remove_file(&file).ok();
        eprintln!(
            "peaks {}: {} peaks; z12 tiles {} from the packs, {} from AWS, {} sea ({:.0?})",
            u.slash(),
            res.len(),
            z12.from.0,
            z12.from.1,
            z12.from.2,
            t.elapsed()
        );
        // (Its inputs' measures: the peaks, the z12 tiles read.)
        pipeline::costin::set("peaks", peaks.len() as u64);
        pipeline::costin::set("z12_tiles", (z12.from.0 + z12.from.1) as u64);
        note_cost("peaks", &u.slash(), t);
    }
    pipeline::agent::jobs::report(units.len() as u64, units.len() as u64, "areas");
    parts.start(1);
    pack_raw_with(out, &raw_dir, &|what, done, total| pipeline::agent::jobs::report(done, total, what));
    Ok(())
}

/// marks [--pass <date>] [--facts file] [--views file]: the landmark points from the current units'
/// candidates and peaks (pipeline::marksjob), with the heritage sites, as markdata and the
/// marks packs. Facts (Wikidata, by QID) and monthly pageviews: the files given, else the items
/// job's for the pass.
fn marks_step(out: &mut Out, args: &[String]) -> Result<()> {
    use serde_json::Value;
    use std::collections::HashMap;
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let read_json = |p: PathBuf| -> Result<Value> {
        Ok(serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)?)
    };
    // The items job's for this pass.
    let items = |name: &str| -> Result<PathBuf> {
        let l = format!("sources/items/{date}/{name}");
        Ok(out.path(out.get(&l).with_context(|| format!("no {l} (the items step)"))?))
    };
    let facts_file = match opt(args, "--facts") {
        Some(f) => PathBuf::from(f),
        None => items("facts")?,
    };
    let views_file = match opt(args, "--views") {
        Some(f) => PathBuf::from(f),
        None => items("views")?,
    };
    let p = phase("the items' facts and pageviews read", Class::NasRead);
    let facts: HashMap<String, Value> = serde_json::from_value(read_json(facts_file)?)?;
    let views: HashMap<String, f64> = serde_json::from_value(read_json(views_file)?)?;
    drop(p);
    let units = pipeline::agent::build::pois_keys(&cov, &date, &out.manifest);
    let parts = Parts(&["Reading every area's candidates and peaks", "Ranking the landmarks", "Writing their tiles"]);
    parts.start(0);
    let p = phase("every area's candidates and peaks read", Class::NasRead);
    let mut cands: Vec<(String, pipeline::marksjob::Candidate)> = Vec::new();
    let (mut with_peaks, mut missing_peaks) = (0, 0);
    for (u, _) in &units {
        let Some(pc) = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)) else { continue };
        let mut peaks: HashMap<String, Value> = HashMap::new();
        if let Some(pk) = out.get(&format!("work/peaks/{}", u.dash())).map(|c| out.path(c)) {
            use std::io::BufRead;
            for line in std::io::BufReader::new(zstd::Decoder::new(std::fs::File::open(&pk)?)?).lines() {
                let mut v: Value = serde_json::from_str(&line?)?;
                let key = v
                    .as_object_mut()
                    .and_then(|o| o.remove("key"))
                    .and_then(|k| k.as_str().map(str::to_string))
                    .context("a peak without its key")?;
                peaks.insert(key, v);
            }
        }
        for c in pipeline::candidates::read(&pc)? {
            let pk = peaks.get(&c.key).cloned();
            if c.kind == "peak" {
                if pk.is_some() {
                    with_peaks += 1
                } else {
                    missing_peaks += 1
                }
            }
            cands.push((c.key.clone(), pipeline::marksjob::from_unit(&c, pk, &facts)));
        }
    }
    drop(p);
    anyhow::ensure!(missing_peaks == 0, "marks: {missing_peaks} peaks have no prominence yet (the peaks step)");
    // One order for every candidate, whichever unit it came from.
    cands.sort_by(|a, b| a.0.cmp(&b.0));
    let cands: Vec<pipeline::marksjob::Candidate> = cands.into_iter().map(|c| c.1).collect();
    eprintln!("marks: {} candidates from {} units ({with_peaks} peaks)", cands.len(), units.len());
    parts.start(1);
    let p = phase("the landmarks ranked", Class::Compute);
    let pts = pipeline::marksjob::poi_points(&cands, &views);
    let summits = pipeline::marksjob::summits_list(&pts);
    let mut all = pts;
    drop(p);
    // The pass's heritage (the heritage job's outputs).
    let p = phase("the heritage sites read", Class::NasRead);
    let src = pipeline::markconv::heritage_source(&date);
    eprintln!("marks: heritage from {src}");
    all.extend(pipeline::markconv::heritage_marks(out, &src)?);
    drop(p);
    parts.start(2);
    let c = pipeline::markconv::write(out, all, summits)?;
    eprintln!("marks: {} points, {} markdata tiles, {} thinned tiles", c.points, c.tiles, c.thinned);
    Ok(())
}

/// items [--pass <date>] [--dem dir] [--cache dir]: facts and pageviews for the current units'
/// candidates' Wikidata items (dem/items.py, per pass epoch), as sources/items/<date>/{facts,views}.
/// What items.py fetched is kept on the NAS too (pipeline::answers:
/// sources/items/<date>/answers.tar.zst), made one with its cache here as it starts, and sent there
/// as it ends, finished or not, or at the next start when it was stopped (the agent's SIGTERM ends
/// it at once).
fn items_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into()));
    let cache =
        PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()))
            .join("items");
    let (mut facts, mut views) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    let is_qid = |q: &str| q.len() > 1 && q.starts_with('Q') && q[1..].bytes().all(|b| b.is_ascii_digit());
    // (items.py marks its three: SCENIC_PARTS names them.)
    const PARTS: &[&str] = &[
        "Gathering the landmark candidates' Wikidata items",
        "Fetching their facts from Wikidata",
        "Looking up their Wikipedia articles",
        "Counting their pageviews in four months of Wikipedia's dumps",
        "Uploading",
    ];
    let parts = Parts(PARTS);
    parts.start(0);
    let p = phase("the candidates' items gathered", Class::NasRead);
    for (u, _) in pipeline::agent::build::pois_keys(&cov, &date, &out.manifest) {
        let Some(pc) = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)) else { continue };
        for c in pipeline::candidates::read(&pc)? {
            let Some(q) = c.qid.as_deref() else { continue };
            if is_qid(q) {
                facts.insert(q.to_string());
            }
            let first = q.split(';').next().unwrap_or("").trim();
            if is_qid(first) {
                views.insert(first.to_string());
            }
        }
    }
    std::fs::create_dir_all(scratch)?;
    let qfile = scratch.join("qids.json");
    std::fs::write(&qfile, serde_json::to_vec(&serde_json::json!({"facts": facts, "views": views}))?)?;
    drop(p);
    // The pass's answers: this Mac's and the NAS's made one.
    // (With the budgets on: this Mac's answers held while the step runs, after the agent sending
    // them if it is, and each sync and keep under the answers' lease on the NAS.)
    let _hold = pipeline::answers::Local::for_job(cache.parent().unwrap_or(&cache))?;
    let p = phase("the pass's answers made one with the NAS's", Class::NasRead);
    let kept =
        pipeline::answers::items(out.root(), &cache, &date).leased(out.root(), pipeline::answers::budgets_on_for_job());
    let answers = || pipeline::answers::items_files(&cache, &date);
    eprintln!("items: the pass's answers {}", kept.sync(&answers(), &scratch.join("answers"))?.words());
    drop(p);
    let dir = scratch.join("items-out");
    let mut c = store::spawn::command("uv");
    let declared = pipeline::inputs::sources::write_for_python(out.root(), &out.manifest, scratch)?;
    c.current_dir(&dem)
        .env("SCENIC_PARTS", serde_json::to_string(PARTS)?)
        .env("SCENIC_PAGEVIEWS_STORE", out.root().join("sources/pageviews"))
        .env(pipeline::inputs::sources::PYTHON_ENV, &declared)
        .args(["run", "python", "items.py", "--qids"])
        .arg(&qfile)
        .arg("--epoch")
        .arg(&date)
        .arg("--cache")
        .arg(&cache)
        .arg("--out")
        .arg(&dir);
    let p = phase("items.py", Class::Mixed);
    pipeline::timings::child(&mut c);
    let st = store::spawn::status(&mut c).context("run items.py")?;
    drop(p);
    // What it fetched, on the NAS whether or not it finished (stopped, at the next start).
    let p = phase("the pass's answers sent to the NAS", Class::NasWrite);
    let sent = kept.keep(&answers(), &scratch.join("answers"));
    drop(p);
    match sent {
        Ok(true) => eprintln!("items: the pass's answers sent to the NAS"),
        Ok(false) => {}
        Err(e) => eprintln!("items: the pass's answers not sent to the NAS now ({e:#}); the next start sends them"),
    }
    anyhow::ensure!(st.success(), "items.py failed: {st}");
    parts.start(4);
    let _p = phase("uploaded", Class::NasWrite);
    for name in ["facts", "views", "meta"] {
        out.put_file(&format!("sources/items/{date}/{name}"), "json", &dir.join(format!("{name}.json")))?;
    }
    out.save()?;
    eprintln!("items: {} items with facts asked, {} for views", facts.len(), views.len());
    Ok(())
}

/// heritage [--pass <date>] [--dem dir] [--cache dir]: the rest of the heritage chain
/// (heritagewd, heritagedetails, areadetails, whsshapes, filterprops' and interest's heritage parts,
/// pageviews, layers) in the stand-in root on the heritage-sites job's outputs (docs/phase5.md
/// "Heritage and area flags"), over the same cover: the pass's areas and named objects within it
/// (named with the chain's filter), and for the World Heritage parts the kept filtered planet within
/// it (one clip per pass and cover, kept in the cache). The park facts are the pass's answers'
/// (`areas-wikidata.json`: areadetails.py asks Wikidata for those it lacks); the pageview months are
/// the items job's cache, the epoch's months; the layers' English is the things' own (the map's
/// server adds the translations' as it serves them: docs/inputs.md §6.5.3).
/// No stops & sights (the marks job's). Its outputs go to `work/heritage/<date>/<file>`. Its
/// caches are the pass's answers (`Epoch`: this Mac's working copy, made one with the NAS's
/// archive); what the chain fetched goes to the NAS as it ends, finished or not, or at the next
/// start when it was stopped.
fn heritage_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::heritage::{base_logical, cover_tiles, tiles_geojson, COVER_Z};
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache =
        PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(scratch)?;
    // Its parts; then the chain, each script in its part.
    let parts = Parts(&[
        "Getting ready: the coverage's protected areas and named places (osmium)",
        "Looking up details: Wikidata facts, descriptions, the areas' sizes",
        "Tracing the World Heritage Sites' lines and areas; stamping the filters' numbers",
        "Weighing fame: counting Wikipedia pageviews, finding what's rare nearby",
        "Writing the map's layers and uploading them",
    ]);
    let chain = [
        (1, "heritagewd.py", vec![]),
        (1, "heritagedetails.py", vec![]),
        (1, "areadetails.py", vec![]),
        (2, "whsshapes.py", vec![]),
        (2, "filterprops.py", vec![]),
        (3, "pageviews.py", vec!["--epoch", date.as_str()]),
        (3, "interest.py", vec![]),
        (4, "layers.py", vec![]),
    ];
    parts.start(0);
    stage(0, 5, "reading the pass's answers and the heritage sites");
    let epoch = heritage_epoch(out, &date, &cache, scratch)?;
    // As it ends, finished or failed (stopped by a signal, it doesn't run: the next start sends what
    // it fetched): the named places' export gone (made again each run, 3 GB), and what the chain
    // fetched kept on the NAS.
    let _end = OnEnd(|| {
        std::fs::remove_file(epoch.dir.join("osm/named.geojsonseq")).ok();
        epoch.keep(scratch);
    });
    // (Extracts of the registers' seeds, which the job read until #139: gone.)
    for e in std::fs::read_dir(&cache)?.flatten() {
        if e.file_name().to_string_lossy().starts_with("registers-") {
            store::cachefile::remove_tree(&e.path());
        }
    }
    let p = phase("the heritage sites copied here", Class::NasRead);
    let root = heritage_root(scratch, &dem, Some(&epoch.dir))?;
    let b = root.join("data/build");
    // The heritage-sites job's outputs, as heritage.py left them.
    for stem in ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"] {
        let c = out
            .get(&base_logical(&date, stem))
            .with_context(|| format!("no {stem} for the pass of {date} (the heritage-sites step)"))?;
        p.count(store::sys::copy_data(out.path(c), b.join(format!("{stem}.json")))?, 1);
    }
    drop(p);
    // No stops & sights.
    std::fs::write(b.join("pois.json"), br#"{"type":"FeatureCollection","features":[]}"#)?;
    std::fs::write(b.join("details-poi.jsonl"), b"")?;
    std::fs::write(b.join("peaks.json"), b"[]")?;
    let inputs = ["pois.json", "details-poi.jsonl", "peaks.json"];
    // The cover, and the pass's sets within it.
    let tiles = cover_tiles(&cov);
    let poly = scratch.join("cover.geojson");
    std::fs::write(&poly, serde_json::to_vec(&tiles_geojson(COVER_Z, &tiles))?)?;
    stage(1, 5, "clipping the protected areas to the coverage (osmium)");
    areas_over_cover(out, &date, &poly, scratch, &root.join("data/areas/areas.geojsonseq"))?;
    stage(2, 5, "clipping the pass's named places to the coverage (osmium)");
    let p = phase("the named places clipped to the cover (osmium)", Class::NasRead);
    let named = osmium_clip(
        &out.path(out.get(&pipeline::osmpass::set_name(&date, "named")).context("the pass's named set")?),
        &poly,
        &scratch.join("named-cover.osm.pbf"),
    )?;
    drop(p);
    // The chain's filter of named objects (the set also keeps the World Heritage tags).
    let named_today = scratch.join("named.osm.pbf");
    let mut c = pipeline::osmpass::osmium();
    c.arg("tags-filter").arg(&named).args([
        "nwr/historic",
        "nwr/heritage",
        "nwr/tourism=museum,attraction,viewpoint",
        "nwr/man_made=lighthouse",
        "nwr/railway=station",
        "nwr/building=train_station,church,cathedral",
        "nwr/amenity=place_of_worship",
        "nwr/boundary=protected_area,national_park",
        "nwr/leisure=park",
        "nwr/military",
        "-o",
    ]);
    c.arg(&named_today).arg("--overwrite");
    stage(3, 5, "filtering them as the chain reads them (osmium)");
    let p = phase("the named places filtered and exported (osmium)", Class::Compute);
    osmium_run(c, "osmium tags-filter (named)")?;
    std::fs::create_dir_all(epoch.dir.join("osm"))?;
    let mut c = pipeline::osmpass::osmium();
    c.arg("export")
        .arg(&named_today)
        .args(["-f", "geojsonseq", "--overwrite", "-o"])
        .arg(epoch.dir.join("osm/named.geojsonseq"));
    osmium_quiet(c, "osmium export (named)")?;
    std::fs::remove_file(&named).ok();
    std::fs::remove_file(&named_today).ok();
    drop(p);
    // The kept filtered planet within the cover, once per pass and cover (whsshapes.py's merged.osm.pbf).
    stage(4, 5, "clipping the pass's filtered planet to the coverage (osmium, once a pass)");
    let p = phase("the filtered planet clipped to the cover (osmium, once a pass)", Class::NasRead);
    let merged = merged_over_cover(out, &date, &poly, &cache)?;
    drop(p);
    let p = phase("the caches linked", Class::Disk);
    pipeline::sys::symlink(&merged, &root.join("data/osm/merged.osm.pbf"))?;
    // The park facts: the pass's answers' (none yet: areadetails.py asks for them all).
    let facts = epoch.dir.join("areas-wikidata.json");
    if !facts.exists() {
        std::fs::write(&facts, b"{}")?;
    }
    pipeline::sys::symlink(&facts, &root.join("data/areas/wikidata.json"))?;
    // The pageview months: the items job's cache (the same files: its months' indexes).
    let pv = cache.join("items");
    std::fs::create_dir_all(pv.join("months"))?;
    pipeline::sys::symlink(&pv, &root.join("data/pageviews"))?;
    drop(p);
    // (The declared sources the chain asks: Wikidata, Wikipedia, the pageviews.)
    let declared = pipeline::inputs::sources::write_for_python(out.root(), &out.manifest, scratch)?;
    for (k, (part, script, sargs)) in chain.iter().enumerate() {
        if k == 0 || chain[k - 1].0 != *part {
            parts.start(*part);
        }
        let mine: Vec<&str> = chain.iter().filter(|c| c.0 == *part).map(|c| c.1).collect();
        stage(
            mine.iter().position(|s| s == script).unwrap_or(0) as u64,
            mine.len() as u64 + (*part == 4) as u64,
            script,
        );
        heritage_script(&root, &dem, out.root(), Some(&declared), script, sargs)?;
    }
    stage(1, 2, "uploading");
    let p = phase("uploaded", Class::NasWrite);
    // Outputs (not the stops & sights' stand-ins, nor the layers made from them), the pass's
    // earlier ones this run didn't make dropped.
    let mut wrote: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&b)?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    let n = files.len().max(1) as f64;
    for (k, p) in files.into_iter().enumerate() {
        pipeline::agent::jobs::within(k as f64 / n);
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let (stem, ext) = name.split_once('.').unwrap_or((name.as_str(), "bin"));
        if inputs.contains(&name.as_str())
            || name.ends_with(".tmp")
            || name.starts_with('.')
            || stem.starts_with("layer-pois")
            || stem == "layer-summits"
        {
            continue;
        }
        let l = format!("work/heritage/{date}/{stem}");
        out.put_file(&l, ext, &p)?;
        wrote.insert(l);
    }
    let prefix = format!("work/heritage/{date}/");
    let stale: Vec<String> = out
        .manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .map(|(l, _)| l.clone())
        .filter(|l| !l[prefix.len()..].contains('/') && !wrote.contains(l) && out.may_remove(l))
        .collect();
    for l in stale {
        out.remove(&l);
    }
    drop(p);
    out.save()?;
    eprintln!("heritage: {} files ({:.0?})", wrote.len(), t0.elapsed());
    Ok(())
}

/// Runs an osmium command, failing with its name; how far it is (its `--progress`) is said as the
/// stage's under way (pipeline::agent::jobs::within).
fn osmium_run(mut c: std::process::Command, what: &str) -> Result<()> {
    c.arg("--progress");
    let st = pipeline::agent::jobs::run_watched(&mut c, pipeline::agent::jobs::within)
        .with_context(|| format!("run {what}"))?;
    anyhow::ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// Runs an osmium command that's quick after the one before in its stage (an export of what was
/// just clipped), failing with its name: its progress not said (a second bar would set the stage's
/// back).
fn osmium_quiet(mut c: std::process::Command, what: &str) -> Result<()> {
    let st = store::spawn::status(&mut c).with_context(|| format!("run {what}"))?;
    anyhow::ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// `src` within the polygon file (whole relations: smart), into `dest`.
fn osmium_clip(src: &Path, poly: &Path, dest: &Path) -> Result<PathBuf> {
    let mut c = pipeline::osmpass::osmium();
    c.args(["extract", "--strategy", "smart", "--overwrite", "-p"]).arg(poly).arg(src).arg("-o").arg(dest);
    osmium_run(
        c,
        &format!("osmium extract ({})", src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
    )?;
    Ok(dest.to_path_buf())
}

/// The pass's protected areas and Indigenous lands within the cover, as heritage.py's areas.geojsonseq.
fn areas_over_cover(out: &Out, date: &str, poly: &Path, scratch: &Path, dest: &Path) -> Result<()> {
    let set = out.path(out.get(&pipeline::osmpass::set_name(date, "areas")).context("the pass's areas set")?);
    let p = phase("the protected areas clipped to the cover (osmium)", Class::NasRead);
    let clip = osmium_clip(&set, poly, &scratch.join("areas-cover.osm.pbf"))?;
    drop(p);
    let _p = phase("the protected areas exported (osmium)", Class::Compute);
    let mut c = pipeline::osmpass::osmium();
    c.args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "-a", "type,id", "--overwrite", "-o"])
        .arg(dest)
        .arg(&clip);
    osmium_quiet(c, "osmium export (areas)")?;
    std::fs::remove_file(&clip).ok();
    Ok(())
}

/// The pass's kept filtered planet within the cover (whole relations), kept in the cache for the
/// pass and cover (`heritage-merged-<date>-<cover>.osm.pbf`; others go): whsshapes.py's
/// merged.osm.pbf. Read from the NAS (an hour or so for the 60 GB file: osmium reads
/// it twice), once per pass and cover.
fn merged_over_cover(out: &Out, date: &str, poly: &Path, cache: &Path) -> Result<PathBuf> {
    let id = store::naming::hash16(&std::fs::read(poly)?)[..12].to_string();
    let name = format!("heritage-merged-{date}-{id}.osm.pbf");
    let dest = cache.join(&name);
    // Held for the job (store::cachefile: osmium and the scripts read it by name); clipped first
    // when it isn't here, osmium writing into the held temporary file in place (--overwrite: the
    // same file, truncated), with a name osmium reads as a PBF.
    let src = out.get(&format!("sources/osm/{date}/filtered")).map(|c| out.path(c));
    if store::cachefile::hold_existing(&dest)?.is_none() {
        let src = src.context("the pass's filtered planet")?;
        let (tmp, held) = store::cachefile::scratch(&cache.join(format!("{name}.tmp.osm.pbf")))?;
        let pbf = tmp.with_extension("osm.pbf");
        std::fs::hard_link(&tmp, &pbf)?;
        let clipped = osmium_clip(&src, poly, &pbf);
        // (osmium writes in place, but one that made a file of its own there is followed:
        // store::cachefile::publish. The same file under both names: the rename does nothing.)
        std::fs::rename(&pbf, &tmp).ok();
        std::fs::remove_file(&pbf).ok();
        if let Err(e) = clipped {
            std::fs::remove_file(&tmp).ok();
            return Err(e);
        }
        store::cachefile::publish(held, &tmp, &dest)?;
    }
    // (Other passes' and covers' clips go, unless a job uses them.)
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("heritage-merged-") && n != name && !n.contains(".tmp") {
            store::cachefile::try_remove(&e.path());
        }
    }
    Ok(dest)
}

/// This pass's working copy of the heritage chain's answers (`dir`, `cache/heritage-<date>`), which
/// the chain's scripts add their caches to, and the NAS's archive of them (`kept`).
struct Epoch {
    dir: PathBuf,
    kept: pipeline::answers::Kept,
    /// This Mac's answers held while the step runs, with the budgets on (pipeline::answers::Local).
    _hold: Option<pipeline::answers::Local>,
}

impl Epoch {
    /// What the chain fetched sent to the NAS (as a step ends, finished or not; a step stopped by a
    /// signal doesn't run this, and the next start sends it).
    fn keep(&self, scratch: &Path) {
        let _p = phase("the pass's answers sent to the NAS", Class::NasWrite);
        match self.kept.keep(&pipeline::answers::heritage_files(&self.dir), &scratch.join("answers")) {
            Ok(true) => eprintln!("heritage: the pass's answers sent to the NAS"),
            Ok(false) => {}
            Err(e) => {
                eprintln!("heritage: the pass's answers not sent to the NAS now ({e:#}); the next start sends them")
            }
        }
    }
}

/// Runs its function when dropped: as a step ends, finished or failed (not when a signal stops
/// it: scenic-build has no handler).
struct OnEnd<F: FnMut()>(F);

impl<F: FnMut()> Drop for OnEnd<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}

/// This pass's working copy of the heritage chain's answers (`cache/heritage-<date>`), made one with
/// the NAS's (pipeline::answers: `sources/items/<date>/heritage.tar.zst`); a new pass's starts from
/// the last pass's answers (pipeline::answers::heritage_start). Other passes' copies go, and the
/// copies of the registers' snapshot the chain worked in before the registers became an input
/// (`heritage-<date>-<id>`).
fn heritage_epoch(out: &Out, date: &str, cache: &Path, scratch: &Path) -> Result<Epoch> {
    let hold = pipeline::answers::Local::for_job(cache)?;
    let p = phase("the pass's answers made ready", Class::Disk);
    let epoch = cache.join(format!("heritage-{date}"));
    std::fs::create_dir_all(&epoch)?;
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let other_epoch =
            n.strip_prefix("heritage-").is_some_and(|r| r.len() >= 10 && pipeline::osmpass::is_date(&r[..10]))
                && e.path() != epoch;
        if other_epoch {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    drop(p);
    let _p = phase("the pass's answers made one with the NAS's", Class::NasRead);
    // (The registers' snapshot's caches, for the first pass since the registers became an input.)
    let legacy = out.get("sources/registers/legacy").map(|c| out.path(c));
    if let Some(a) = pipeline::answers::heritage_start(out.root(), &epoch, date, legacy.as_deref())? {
        eprintln!("heritage: the pass's answers start from {}", a.display());
    }
    std::fs::write(epoch.join(".done"), b"")?;
    let e = Epoch {
        kept: pipeline::answers::heritage(out.root(), &epoch, date)
            .leased(out.root(), pipeline::answers::budgets_on_for_job()),
        dir: epoch,
        _hold: hold,
    };
    eprintln!(
        "heritage: the pass's answers {}",
        e.kept.sync(&pipeline::answers::heritage_files(&e.dir), &scratch.join("answers"))?.words()
    );
    Ok(e)
}

/// A stand-in root laid out as the repository for the heritage scripts: `dem/` the app's
/// scripts and their Python project, `data/heritage` this pass's working copy of the chain's
/// answers (`epoch`; an empty folder for heritage.py, which fetches nothing).
fn heritage_root(scratch: &Path, dem: &Path, epoch: Option<&Path>) -> Result<PathBuf> {
    let _p = phase("the scripts' stand-in root made", Class::Disk);
    let root = scratch.join("heritage-root");
    std::fs::remove_dir_all(&root).ok();
    for d in ["dem", "data/build", "data/areas", "data/osm"] {
        std::fs::create_dir_all(root.join(d))?;
    }
    for e in std::fs::read_dir(dem)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if e.path().is_file()
            && (n.ends_with(".py") || n == "pyproject.toml" || n == "uv.lock" || n == ".python-version")
        {
            store::sys::copy_data(e.path(), root.join("dem").join(&n))?;
        }
    }
    match epoch {
        Some(e) => pipeline::sys::symlink(&e, &root.join("data/heritage"))?,
        None => std::fs::create_dir_all(root.join("data/heritage"))?,
    }
    Ok(root)
}

/// Runs one of the heritage scripts in a stand-in root, in the Python environment of the app's `dem`
/// (`dem/.venv`, which uv makes from the app's lock file the first time a step runs on a Mac, as
/// for every other Python step; the lock file as is), given the declared sources' file `declared`
/// when it reads any (pipeline::inputs::sources::write_for_python).
fn heritage_script(
    root: &Path,
    dem: &Path,
    nas: &Path,
    declared: Option<&Path>,
    script: &str,
    args: &[&str],
) -> Result<()> {
    let t = std::time::Instant::now();
    let mut c = store::spawn::command("uv");
    if let Some(d) = declared {
        c.env(pipeline::inputs::sources::PYTHON_ENV, d);
    }
    // (The pageview months' indexes the items job keeps, on the NAS: pageviews.py.)
    c.current_dir(root.join("dem"))
        .env("UV_PROJECT_ENVIRONMENT", dem.join(".venv"))
        .env("SCENIC_PAGEVIEWS_STORE", nas.join("sources/pageviews"))
        .args(["run", "--frozen", "python", script])
        .args(args);
    // (Its phase, its own phases under it: dem/timings.py.)
    let _p = phase(script, Class::Mixed);
    pipeline::timings::child(&mut c);
    let st = store::spawn::status(&mut c).with_context(|| format!("run {script}"))?;
    anyhow::ensure!(st.success(), "{script} failed: {st}");
    eprintln!("heritage: {script} done ({:.0?})", t.elapsed());
    Ok(())
}

/// The accepted version of input `unit` the step reads: the index the agent names (`--<unit>-index`,
/// from the records it planned with), else the records' now (by hand).
fn accepted_input(out: &Out, args: &[String], unit: &str) -> Result<Option<pipeline::inputs::Accepted>> {
    match opt(args, &format!("--{unit}-index")) {
        Some(index) => {
            let m = std::collections::BTreeMap::from([(pipeline::inputs::logical(unit), index)]);
            pipeline::inputs::open(out.root(), &m, unit)
        }
        None => pipeline::inputs::open(out.root(), &out.manifest, unit),
    }
}

/// The accepted registers meeting the cover, laid out for heritage.py in `dir` (`<id>.geojson` and
/// `<id>.toml`, links to the checked copies), and the accepted matches in `matches` (None without
/// any). The registers, by id.
fn heritage_inputs(
    out: &Out,
    args: &[String],
    cov: &pipeline::coverage::Coverage,
    dir: &Path,
    matches: &Path,
) -> Result<(Vec<String>, Option<PathBuf>)> {
    use pipeline::inputs::{heritage, heritage_matches};
    let _p = phase("the accepted registers laid out", Class::Disk);
    let a = accepted_input(out, args, heritage::UNIT)?
        .context("no accepted heritage registers (the input check takes inputs/heritage/ in)")?;
    std::fs::remove_dir_all(dir).ok();
    std::fs::create_dir_all(dir)?;
    let regs = pipeline::heritage::registers_meeting(cov, &a);
    for r in &regs {
        pipeline::sys::symlink(&out.root().join(&r.geojson), &dir.join(format!("{}.geojson", r.id)))?;
        pipeline::sys::symlink(&out.root().join(&r.toml), &dir.join(format!("{}.toml", r.id)))?;
    }
    std::fs::remove_dir_all(matches).ok();
    let m = match accepted_input(out, args, heritage_matches::UNIT)? {
        Some(m) if !m.index.files.is_empty() => {
            std::fs::create_dir_all(matches)?;
            for (k, (p, copy)) in m.files().enumerate() {
                let name = format!("{k:04}-{}", p.replace('/', "+"));
                pipeline::sys::symlink(&copy, &matches.join(name))?;
            }
            Some(matches.to_path_buf())
        }
        _ => None,
    };
    Ok((regs.into_iter().map(|r| r.id).collect(), m))
}

/// heritage-sites [--pass <date>] [--dem dir] [--heritage-index i] [--heritage-matches-index i]:
/// the heritage sites and designated areas the units read (pipeline::heritage): heritage.py in a
/// stand-in root, over the tiles within 20 km of the coverage, on the accepted heritage registers
/// meeting them (docs/inputs.md §6.5: the indexes the agent names, from the records it planned
/// with; by hand, the records' now), the owner's matches, and the pass's protected areas (its
/// `areas` set within those tiles, as heritage.py's areas.geojsonseq). Its outputs go to
/// `work/heritage/<date>/base/<file>`, and per z6 tile the sites' positions and the area polygons.
fn heritage_sites_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::heritage::{
        base_logical, cover_tiles, put_parts, put_slices, slice_areas, slice_parts, slice_sites, tiles_bytes,
        tiles_geojson, COVER_Z,
    };
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(scratch)?;
    let parts = Parts(&[
        "Reading the heritage registers",
        "Clipping the protected areas to the coverage (osmium)",
        "Locating the registers' sites (heritage.py)",
        "Slicing them per area",
        "Uploading",
    ]);
    parts.start(0);
    let root = heritage_root(scratch, &dem, None)?;
    let b = root.join("data/build");
    let (regs, matches) = heritage_inputs(out, args, &cov, &scratch.join("registers"), &scratch.join("matches"))?;
    eprintln!("heritage-sites: {} registers meet the cover ({})", regs.len(), regs.join(", "));
    // The cover: its tiles for heritage.py, as rectangles for osmium.
    let p = phase("the cover worked out", Class::Compute);
    let tiles = cover_tiles(&cov);
    std::fs::write(b.join("cover.idx"), tiles_bytes(&tiles))?;
    let poly = scratch.join("cover.geojson");
    std::fs::write(&poly, serde_json::to_vec(&tiles_geojson(COVER_Z, &tiles))?)?;
    eprintln!("heritage-sites: {} z{COVER_Z} tiles within 20 km of the coverage ({:.0?})", tiles.len(), t0.elapsed());
    drop(p);
    // The pass's protected areas and Indigenous lands within them (whole relations: smart).
    parts.start(1);
    areas_over_cover(out, &date, &poly, scratch, &root.join("data/areas/areas.geojsonseq"))?;
    parts.start(2);
    let (reg_arg, match_arg, zoom) = (
        scratch.join("registers").to_string_lossy().into_owned(),
        matches.map(|m| m.to_string_lossy().into_owned()),
        COVER_Z.to_string(),
    );
    let mut hargs = vec![
        "../data/build",
        "--registers",
        reg_arg.as_str(),
        "--tiles",
        "../data/build/cover.idx",
        "--zoom",
        zoom.as_str(),
        "--date",
        &date,
    ];
    if let Some(m) = &match_arg {
        hargs.extend(["--matches", m.as_str()]);
    }
    heritage_script(&root, &dem, out.root(), None, "heritage.py", &hargs)?;
    parts.start(3);
    // The units' slices, then the whole files.
    let p = phase("the sites and areas sliced per area", Class::Compute);
    let her = std::fs::read(b.join("heritage.json"))?;
    let sites = slice_sites(&her)?;
    // (The World Heritage components apart: the 3D buildings' heritage lists leave them out.)
    let parts_of = slice_parts(&her)?;
    drop(her);
    let areas = slice_areas(&std::fs::read_to_string(b.join("area-shapes.geojsonseq"))?)?;
    drop(p);
    let p = phase("the slices uploaded", Class::NasWrite);
    let (ns, na) = put_slices(out, &date, &sites, &areas)?;
    put_parts(out, &date, &parts_of)?;
    drop(p);
    parts.start(4);
    let _p = phase("uploaded", Class::NasWrite);
    for (stem, ext) in [
        ("heritage", "json"),
        ("heritage-areas", "json"),
        ("special", "json"),
        ("indigenous", "json"),
        ("heritage-sources", "json"),
        ("area-shapes", "geojsonseq"),
    ] {
        out.put_file(&base_logical(&date, stem), ext, &b.join(format!("{stem}.{ext}")))?;
    }
    out.save()?;
    eprintln!("heritage-sites: {} sites' and {} areas' slices ({:.0?})", ns, na, t0.elapsed());
    Ok(())
}

/// The unit step's global-source layers: the manifest's, or a pilot's published catalog.
fn layers_source<'a>(
    out: &'a Out,
    pilot: &'a Option<(PathBuf, store::catalog::Catalog)>,
    blobs: Option<&'a store::blobs::Blobs>,
) -> pipeline::stage::Source<'a> {
    match pilot {
        Some((r, c)) => pipeline::stage::Source::Catalog(r, c),
        None => pipeline::stage::Source::Manifest(out, blobs),
    }
}

/// The pass the units build from (`--pass`, else the newest complete one) and the regions'
/// coverage (`--regions`, else the NAS's recipes), with the regions' count.
fn pass_and_coverage(out: &Out, args: &[String]) -> Result<(String, pipeline::coverage::Coverage, usize)> {
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass on the NAS (--pass)")?,
    };
    let (cov, n) = regions_coverage(out, args, &date, None)?;
    Ok((date, cov, n))
}

/// The canopy files the canopy step reads for box `b` (w, s, e, n, degrees): each 10° square's
/// median, p95 and cover (scenic-metrics names them by the square's top and left).
fn canopy_files(b: [f64; 4]) -> Vec<String> {
    let (lefts, tops) = (
        (b[0] / 10.0).floor() as i32..=(b[2] / 10.0).floor() as i32,
        (b[1] / 10.0).ceil() as i32..=(b[3] / 10.0).ceil() as i32,
    );
    tops.flat_map(|t| lefts.clone().map(move |l| (t * 10, l * 10)))
        .flat_map(pipeline::unit::canopy_square_files)
        .collect()
}

/// unit-snap U --out <dir> --cache <dir> [--carry]: unit U's folder built as the unit step builds
/// it, from the build's records, writing nothing to the NAS: its kept samples go under <dir>/shared
/// (seeded with its and its neighbours' from the NAS), and the folder is copied before and after
/// each of its steps' programs into <dir>/snap (pipeline::unit::Tools::snap) for
/// tools/check/same.py and tail.mjs. `--cache` holds the DEM seed (`dem-cache.*`) and the canopy
/// files (`chm10/`), as the agent's cache does; the step programs are this binary's neighbours.
/// `--carry`: its scenic results from its last run restored first (a copy of the NAS's), as a
/// rebuild's are.
fn unit_snap(out: &Out, args: &[String]) -> Result<()> {
    use pipeline::unit::{build_folder, Tools};
    let u = positional(args).first().and_then(|s| Unit::parse(s)).context("unit-snap U")?;
    let dir = PathBuf::from(opt(args, "--out").context("--out <dir>")?);
    let (date, cov, _) = pass_and_coverage(out, args)?;
    let reach = pipeline::reach::Reaches::load(out.root(), &out.manifest, &date)
        .map_err(|e| anyhow::anyhow!("the pass's reaches: {e:?}"))?
        .context("no reaches for the pass")?;
    let index = pipeline::buildtiles::Index::load(out, &pipeline::buildtiles::release_of(out.root(), &out.manifest)?)?
        .context("the roadside buildings aren't made")?;
    let shared = dir.join("shared");
    let kept = shared.join(pipeline::unit::DEM_UNITS);
    std::fs::create_dir_all(&kept)?;
    for e in std::fs::read_dir(out.root().join("cache").join(pipeline::unit::DEM_UNITS))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(n) = name.split('.').next().and_then(|s| Unit::parse(&s.replace('-', "/"))) else { continue };
        if n.x.abs_diff(u.x) <= 1 && n.y.abs_diff(u.y) <= 1 && !kept.join(&name).exists() {
            store::sys::copy_data(e.path(), kept.join(&name))?;
        }
    }
    let carry = args
        .iter()
        .any(|a| a == "--carry")
        .then(|| -> Result<pipeline::scache::Carry> {
            let (from, to) =
                (out.root().join("cache/scenic-units").join(u.dash()), shared.join("scenic-units").join(u.dash()));
            std::fs::remove_dir_all(&to).ok();
            std::fs::create_dir_all(&to)?;
            for e in
                std::fs::read_dir(&from).with_context(|| format!("no kept results in {}", from.display()))?.flatten()
            {
                store::sys::copy_data(e.path(), to.join(e.file_name()))?;
            }
            Ok(pipeline::scache::Carry { dir: to })
        })
        .transpose()?;
    let bdir = dir.join(format!("{}-buildings", u.dash()));
    let n = pipeline::buildtiles::stage(out.root(), &index, u, reach.get(u), &bdir, None)?;
    eprintln!("unit-snap {}: buildings from {n} tiles", u.slash());
    let tools = Tools {
        bin: std::env::current_exe()?.parent().context("bin")?.to_path_buf(),
        dem: PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into())),
        cache: PathBuf::from(opt(args, "--cache").context("--cache <dir>")?),
        buildings: Some(bdir),
        sources: Some(out.root().join("sources")),
        shared: Some(shared),
        chm: None,
        stores_read_only: false,
        snap: Some(dir.join("snap")),
        elevation: elevation_of(out)?,
        declared: Some(pipeline::inputs::sources::read(out.root(), &out.manifest)?),
    };
    let piece = out.path(out.get(&format!("sources/osm/{date}/pieces/{}", u.dash())).context("no piece")?);
    let local = dir.join(format!("piece-{}.osm.pbf", u.dash()));
    store::sys::copy_data(&piece, &local)?;
    let folder = dir.join("units").join(u.dash());
    std::fs::remove_dir_all(&folder).ok();
    std::fs::remove_dir_all(dir.join("snap")).ok();
    let heritage = |b: [f64; 4], d: &Path| pipeline::heritage::unit_inputs(out, &date, b, d);
    let t = std::time::Instant::now();
    let blobs = store::blobs::Blobs::new(tools.cache.join("blobs"));
    let rep = build_folder(
        u,
        &local,
        &folder,
        &cov,
        &pipeline::stage::Source::Manifest(out, Some(&blobs)),
        &tools,
        &heritage,
        carry.as_ref(),
    )?;
    eprintln!(
        "unit-snap {}: {} of {} ways kept, {} owned, in {:.1?}; snapshots in {}",
        u.slash(),
        rep.kept_ways,
        rep.piece_ways,
        rep.owned,
        t.elapsed(),
        dir.join("snap").display()
    );
    Ok(())
}

/// The declared elevation datasets' accepted version the units read (pipeline::inputs::elevation):
/// an error before the records say the build reads them (their switch, docs/inputs.md §6.8).
fn elevation_of(out: &Out) -> Result<Option<pipeline::inputs::Accepted>> {
    use pipeline::inputs::elevation;
    anyhow::ensure!(elevation::switched(&out.manifest), "the records don't say the build reads the declared elevation datasets yet (their switch, docs/inputs.md §6.8): an app from before it builds the units until then");
    Ok(Some(pipeline::inputs::open(out.root(), &out.manifest, elevation::UNIT)?.context(
        "the build reads the declared elevation datasets, but the records name no accepted version of them",
    )?))
}

/// The unit job's progress (pipeline::unit::Areas): each area under way by the stages it's
/// through, said as they finish.
static AREAS: std::sync::Mutex<Option<pipeline::unit::Areas>> = std::sync::Mutex::new(None);

fn areas(f: impl FnOnce(&mut pipeline::unit::Areas)) {
    if let Ok(mut a) = AREAS.lock() {
        if let Some(a) = a.as_mut() {
            f(a);
        }
    }
}

fn unit_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::unit::{prepare_folder, run_tail, stage as unit_stage, Tools};
    let setup = phase("the pass, coverage, pieces and reaches read", Class::NasRead);
    let (date, cov, regions) = pass_and_coverage(out, args)?;
    // Global-source layers: as this build's manifest has them now (what the unit keys hash: a
    // terrain job of the same plan is published only with its catalog, at the end), or another
    // root's published catalog (a pilot builds against the real one).
    let pilot = match opt(args, "--layers-root").map(PathBuf::from) {
        Some(r) => {
            let c = store::catalog::latest(&r.join("catalog"))?.context("no catalog for the global-source layers")?;
            Some((r, c))
        }
        None => None,
    };
    let bin = std::env::current_exe()?.parent().context("bin")?.to_path_buf();
    let tools = Tools {
        bin,
        dem: PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into())),
        cache: PathBuf::from(opt(args, "--cache-dir").unwrap_or_else(|| "data/cache".into())),
        buildings: opt(args, "--buildings").map(PathBuf::from),
        sources: Some(out.root().join("sources")),
        shared: Some(out.root().join("cache")),
        chm: None,
        stores_read_only: false,
        snap: None,
        elevation: elevation_of(out)?,
        declared: Some(pipeline::inputs::sources::read(out.root(), &out.manifest)?),
    };
    drop(setup);
    // (The DEM cache's seed, where the units' elevations start from, is copied here once a Mac by
    // the first unit that it can serve: unit::prepare_folder.)
    // What units kept in this Mac's own cache: moved to the shared one (both Macs' units read it).
    let _p = phase("kept results moved to the shared cache", Class::NasWrite);
    if let Some(shared) = &tools.shared {
        match pipeline::unit::move_kept_to_shared(&tools.cache, shared) {
            Ok(0) => {}
            Ok(n) => eprintln!("unit: moved {n} kept results to the shared cache ({})", shared.display()),
            Err(e) => eprintln!("unit: moving kept results to the shared cache: {e:#}"),
        }
    }
    drop(_p);
    let setup = phase("the pass, coverage, pieces and reaches read", Class::NasRead);
    // The pass's heritage sites and designated areas (the heritage-sites step), for every unit's
    // flags.
    anyhow::ensure!(
        out.get(&pipeline::heritage::base_logical(&date, "heritage-sources")).is_some(),
        "no heritage sites for the pass of {date} (the heritage-sites step)"
    );
    // The units: as asked, else every unit whose piece meets the coverage.
    let pieces: serde_json::Value = serde_json::from_slice(&std::fs::read(
        out.path(out.get(&format!("sources/osm/{date}/pieces")).context("the pass's pieces list")?),
    )?)?;
    let mut units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    // The pass's reaches: which units the coverage builds, and the roadside buildings' tiles each
    // reads (unless a folder of them is given), from the release's (the buildings step).
    let buildings = match tools.buildings {
        Some(_) => None,
        None => Some(
            pipeline::buildtiles::Index::load(out, &pipeline::buildtiles::release_of(out.root(), &out.manifest)?)?
                .context("the roadside buildings aren't made (the buildings step)")?,
        ),
    };
    let reach = if units.is_empty() || tools.buildings.is_none() {
        Some(
            pipeline::reach::Reaches::load(out.root(), &out.manifest, &date)
                .map_err(|e| anyhow::anyhow!("the pass's reaches: {e:?}"))?
                .context("no reaches for the pass (the reach step)")?,
        )
    } else {
        None
    };
    if units.is_empty() {
        let reach = reach.as_ref().context("the pass's reaches")?;
        let a = tools.elevation.as_ref().context("the declared elevation datasets' accepted version")?;
        let fine = pipeline::dem::spacing::Fine::of_accepted(&pipeline::dem::declared::accepted(a)?);
        for k in pieces["pieces"].as_object().context("pieces")?.keys() {
            let u = Unit::parse(k).context("unit")?;
            if pipeline::agent::build::builds(&cov, reach, u, &fine) {
                units.push(u);
            }
        }
    }
    eprintln!("unit: pass {date}, {regions} region(s), {} unit(s)", units.len());
    drop(setup);
    // A unit's folders go once it's built; what an earlier job left (a unit that failed) goes now.
    std::fs::remove_dir_all(scratch.join("units")).ok();
    for e in std::fs::read_dir(scratch).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("piece-") {
            std::fs::remove_file(e.path()).ok();
        }
    }
    // Its progress: area by area, each by its stages (their times on this Mac learned and kept with
    // its caches).
    *AREAS.lock().unwrap() = Some(pipeline::unit::Areas::new(units.len(), Some(tools.cache.join("unit-stages.json"))));
    pipeline::unit::on_stage(Some(Box::new(|stage, frac, took| areas(|a| a.stage(stage, frac, took)))));
    // This Mac's copies of the packs staging reads (pipeline::stage), kept with the agent's caches.
    let blobs = store::blobs::Blobs::new(tools.cache.join("blobs"));
    // The next unit's piece and packs, copied while this one builds (one stream: large sequential
    // reads, nothing the unit building now waits on).
    let mut ahead: Option<std::thread::JoinHandle<()>> = None;
    // Other workers, through the lead's coordinator (the agent's jobs there), and the units
    // whose tails' last steps are out with them.
    let offload = pipeline::offload::Offload::from_env(scratch);
    let mut out_now: std::collections::VecDeque<(Built, pipeline::offload::Offered)> = Default::default();
    let mut paused = false;
    // The cache files held for the units (store::cachefile): each unit's let go once the unit after
    // it is under way (what this unit and the next's copying ahead hold stays), so room-making may
    // take them meanwhile, and a job of hundreds of units doesn't hold them all.
    let mut held_since: Option<u64> = None;
    for (k, &u) in units.iter().enumerate() {
        if let Some(m) = held_since.replace(store::cachefile::mark()) {
            store::cachefile::release_before(m);
        }
        // A safe point before each area: with the build pausing, the areas whose last steps are out
        // are settled (below), and the job ends.
        if pipeline::control::draining() {
            paused = true;
            break;
        }
        areas(|a| {
            a.on(&u.slash());
            a.say(true);
        });
        // (Under way, for the agent, crate::agent::memguard; its processes' most held from here,
        // its measure.)
        pipeline::sys::reset_group_peak();
        note_started(&pipeline::coord::cost_key("unit", &u.slash()));
        // (What the caches' accessor had written as it began: its own disk, carried with it, as a
        // later unit begins before its tail ends.)
        let written_at = store::cachefile::written();
        let t = std::time::Instant::now();
        if let Some(h) = ahead.take() {
            // (The area before's copying ahead of this one's piece and packs, not done yet.)
            let _p = phase("waiting for the copying ahead", Class::Wait);
            h.join().ok();
        }
        if let Some(&next) = units.get(k + 1) {
            let o: &Out = out;
            let b = pipeline::stage::tile_box_grown(next.z, next.x, next.y, pipeline::stage::MARGIN_KM);
            let mut packs = layers_source(o, &pilot, Some(&blobs)).pack_contents(b);
            // (Its heritage slices too: pipeline::heritage::unit_inputs_with.)
            packs.extend(pipeline::heritage::unit_contents(o, &date, b));
            let piece = o
                .get(&format!("sources/osm/{date}/pieces/{}", next.dash()))
                .map(|c| (o.path(c), scratch.join(format!("piece-{}.osm.pbf", next.dash()))));
            // Its canopy squares' files, from the NAS's store into the cache the canopy step reads
            // (scenic-metrics: it takes what's there whole, else copies it itself).
            let squares: Vec<(PathBuf, PathBuf)> = match &tools.sources {
                Some(s) => canopy_files(b)
                    .into_iter()
                    .map(|n| (s.join("canopy").join(&n), tools.cache.join("chm10").join(&n)))
                    .collect(),
                None => Vec::new(),
            };
            // Its roadside buildings' tiles, into this Mac's copies of them.
            let tiles: Vec<(PathBuf, PathBuf, u64)> = match &buildings {
                Some(index) => pipeline::buildtiles::tiles_for(next, reach.as_ref().and_then(|r| r.get(next)))
                    .into_iter()
                    .filter_map(|t| pipeline::buildtiles::tile_copy(o.root(), index, t, &tools.cache))
                    .collect(),
                None => Vec::new(),
            };
            // Its scenic results kept from its last run, copied beside its folder.
            let carry = (
                pipeline::scache::Carry { dir: tools.scenic_kept(next) },
                pipeline::unit::ahead_carry(&scratch.join("units").join(next.dash())),
            );
            let (root, blobs) = (o.root().to_path_buf(), blobs.clone());
            ahead = Some(std::thread::spawn(move || {
                // (A phase beside the main thread's: crate::timings.)
                let _p = phase("next area's piece and packs copied ahead", Class::NasRead);
                // (Under this process's own temporary name, renamed whole: another job on this Mac
                // may be copying the same file.)
                let copy = |src: &Path, dst: &Path| {
                    if dst.exists() || !src.exists() {
                        return;
                    }
                    let tmp = pipeline::whole::tmp_name(dst);
                    let whole = || {
                        std::fs::metadata(src).ok().map(|m| m.len()) == std::fs::metadata(&tmp).ok().map(|m| m.len())
                    };
                    if dst.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
                        && store::sys::copy_data(src, &tmp).inspect(|n| pipeline::timings::count(*n, 1)).is_ok()
                        && whole()
                    {
                        std::fs::rename(&tmp, dst).ok();
                    } else {
                        std::fs::remove_file(&tmp).ok();
                    }
                };
                if let Some((src, dst)) = &piece {
                    copy(src, dst);
                }
                if let Err(e) = std::fs::create_dir_all(carry.1.parent().unwrap_or(Path::new(".")))
                    .map_err(anyhow::Error::from)
                    .and_then(|_| carry.0.fetch(&carry.1))
                {
                    eprintln!("unit: its kept scenic results not copied ahead ({e:#})");
                }
                for (src, local, want) in &tiles {
                    if let Err(e) = pipeline::buildtiles::fetch_tile(src, local, *want) {
                        eprintln!("unit: {} not copied ahead ({e:#})", local.display());
                    }
                }
                for c in packs {
                    if let Err(e) = blobs.get(&root, &c) {
                        eprintln!("unit: {c} not copied ahead ({e})");
                    }
                }
                // (The squares held, as they're copied: store::cachefile, the canopy step's own
                // hold reading them again.)
                for (src, dst) in &squares {
                    if !dst.exists() && !src.exists() {
                        continue;
                    }
                    let got = store::cachefile::hold(dst, &mut |tmp| {
                        let (n, want) = (store::sys::copy_data(src, tmp)?, std::fs::metadata(src)?.len());
                        if n != want {
                            return Err(std::io::Error::other(format!("{n} of {want} bytes copied")));
                        }
                        Ok(())
                    });
                    if let Err(e) = got {
                        eprintln!("unit: {} not copied ahead ({e})", dst.display());
                    }
                }
            }));
        }
        let dir = scratch.join("units").join(u.dash());
        let bdir = scratch.join("units").join(format!("{}-buildings", u.dash()));
        let clean = || {
            std::fs::remove_dir_all(&dir).ok();
            std::fs::remove_dir_all(&bdir).ok();
        };
        clean();
        let piece_logical = format!("sources/osm/{date}/pieces/{}", u.dash());
        let Some(piece) = out.get(&piece_logical).map(|n| out.path(n)) else {
            eprintln!("unit {}: no piece (nothing there)", u.slash());
            areas(|a| a.finished(&u.slash()));
            continue;
        };
        let local_piece = scratch.join(format!("piece-{}.osm.pbf", u.dash()));
        let s = unit_stage("piece copied from the NAS", Class::NasRead);
        // (Copied ahead, while the unit before it built, unless it's the job's first.)
        if std::fs::metadata(&local_piece).map(|m| m.len()).ok() != std::fs::metadata(&piece).map(|m| m.len()).ok() {
            let n = store::sys::copy_data(&piece, &local_piece).with_context(|| format!("copy {}", piece.display()))?;
            pipeline::timings::count(n, 1);
        }
        s.end();
        // Its scenic results from its last run, kept in the shared cache (pipeline::scache::Carry).
        let carry = pipeline::scache::Carry { dir: tools.scenic_kept(u) };
        pipeline::unit::take_peak();
        let (rep, tools) = {
            let o: &Out = out;
            // (Its heritage slices from this Mac's copies: copied ahead, or now.)
            let heritage = |b: [f64; 4], d: &Path| pipeline::heritage::unit_inputs_with(o, &date, b, d, Some(&blobs));
            // Its roadside buildings: the folder given by hand, else the release's tiles near its
            // roads.
            let mut tools = tools.clone();
            if let Some(index) = &buildings {
                let s = unit_stage("buildings staged", Class::NasRead);
                // (From this Mac's copies of the tiles: copied ahead, or now.)
                let n = pipeline::buildtiles::stage(
                    o.root(),
                    index,
                    u,
                    reach.as_ref().and_then(|r| r.get(u)),
                    &bdir,
                    Some(&tools.cache),
                )?;
                eprintln!("unit {}: buildings from {n} tiles", u.slash());
                s.end();
                tools.buildings = Some(bdir.clone());
            }
            (
                prepare_folder(
                    u,
                    &local_piece,
                    &dir,
                    &cov,
                    &layers_source(o, &pilot, Some(&blobs)),
                    &tools,
                    &heritage,
                    Some(&carry),
                )?,
                tools,
            )
        };
        std::fs::remove_file(&local_piece).ok();
        // Its tail: offered to another worker while this Mac prepares the next unit (when one's
        // around, and fewer than such are out), else run here too. The canopy squares a worker
        // reads where they lie, and can't download one: while the NAS's store lacks one, the
        // steps through the canopy stay here (where it's downloaded).
        let mut runs = pipeline::unit::tail(u, tools.buildings.is_some(), tools.sources.is_some());
        if !tools.sources.as_deref().is_some_and(|s| pipeline::unit::canopy_stored(&dir, s)) {
            pipeline::unit::keep_here(&mut runs, "scenic canopy");
        }
        let (here, anywhere) = pipeline::unit::split(&runs);
        if rep.kept_ways > 0 {
            run_tail(here, &dir, &tools)?;
        }
        // (Its inputs' measures: the piece's bytes beside what the store's readers read, its ways
        // and vertices kept, its elevation tiles and area.)
        // (Its tail's later steps run here add theirs: `costin::measured`.)
        pipeline::costin::set("km2", pipeline::costin::tile_km2(u.z, u.x, u.y));
        let mut sizes = pipeline::costin::take();
        *sizes.entry("bytes".into()).or_default() += std::fs::metadata(&piece).map_or(0, |m| m.len());
        sizes.extend([
            ("ways".into(), rep.kept_ways as u64),
            ("vertices".into(), rep.kept_verts as u64),
            ("piece_ways".into(), rep.piece_ways as u64),
            ("dem_tiles".into(), rep.dem_cache as u64),
        ]);
        let b = Built {
            u,
            dir: dir.clone(),
            bdir: bdir.clone(),
            rep,
            tools,
            carry,
            piece,
            t,
            peak: pipeline::unit::take_peak(),
            group: pipeline::sys::group_peak(),
            disk: store::cachefile::written().saturating_sub(written_at),
            anywhere: anywhere.to_vec(),
            sizes,
        };
        let offering = phase("tails offered to other workers", Class::Net);
        let offered = match &offload {
            Some(o) if b.rep.kept_ways > 0 && out_now.len() < o.depth() => {
                match o.offer(u, &b.dir, b.tools.buildings.as_deref(), &b.anywhere) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("unit {}: its last steps not offered ({e:#}); run here", u.slash());
                        None
                    }
                }
            }
            _ => None,
        };
        drop(offering);
        match offered {
            Some(task) => out_now.push_back((b, task)),
            None => {
                let mut b = b;
                if b.rep.kept_ways > 0 {
                    let (r, bytes, tiffs) = pipeline::costin::measured(|| run_tail(&b.anywhere, &b.dir, &b.tools));
                    r?;
                    pipeline::costin::add_to(&mut b.sizes, bytes, tiffs);
                    b.peak = b.peak.max(pipeline::unit::take_peak());
                }
                commit_unit(out, &date, b, "here")?;
            }
        }
        // Units whose last steps came back from other workers: committed before the next.
        settle_tails(out, &date, offload.as_ref(), &mut out_now, false)?;
    }
    // The rest: given a moment to be taken by a worker whose pace beats this Mac's (or isn't
    // measured, once an hour), and waited on while a worker holding one will be back with it
    // before this Mac's run would end; then taken back and run here where no one took them, raced
    // here where someone did.
    settle_tails(out, &date, offload.as_ref(), &mut out_now, true)?;
    if paused {
        if let Some(h) = ahead.take() {
            h.join().ok();
        }
        pipeline::control::stop_paused("unit");
    }
    Ok(())
}

/// A unit prepared (and its tail's first steps run), waiting to be finished and committed.
struct Built {
    u: Unit,
    dir: PathBuf,
    bdir: PathBuf,
    rep: pipeline::unit::Report,
    tools: pipeline::unit::Tools,
    carry: pipeline::scache::Carry,
    piece: PathBuf,
    t: std::time::Instant,
    peak: u64,
    /// The most the job's processes held together while it was built here (bytes, sampled: what
    /// the agent's memory guard measures too), its measure (`SCENIC_COSTS`).
    group: u64,
    /// What the caches' accessor added to the disk while it was built here (bytes: store::cachefile::
    /// written), its disk (`SCENIC_COSTS`' `disk_mb`).
    disk: u64,
    /// Its tail's steps any worker may run.
    anywhere: Vec<pipeline::unit::Run>,
    /// Its inputs' measures (pipeline::costin), for its cost line.
    sizes: BTreeMap<String, u64>,
}

/// A tail's time on the lead until it has timed its stages (seconds).
const TAIL_GUESS_S: f64 = 120.0;

/// Settles the units whose last steps are out with other workers (pipeline::offload), in order,
/// and commits them: with `wait` false only those a worker finished or failed; with it, each after
/// the patience its steps' time here and its worker's pace allow (pipeline::offload::Patience).
fn settle_tails(
    out: &mut Out,
    date: &str,
    offload: Option<&pipeline::offload::Offload>,
    waiting: &mut std::collections::VecDeque<(Built, pipeline::offload::Offered)>,
    wait: bool,
) -> Result<()> {
    let Some(o) = offload else { return Ok(()) };
    let mut i = 0;
    while i < waiting.len() {
        let (b, task) = &mut waiting[i];
        // (Its last steps run here, when they're taken back: its stages.)
        areas(|a| a.on(&b.u.slash()));
        let (dir, tools, anywhere) = (b.dir.clone(), b.tools.clone(), b.anywhere.clone());
        // (What its steps read when run here is added to its sizes: `costin::measured`. A tail a
        // worker settled adds none.)
        let mut read = (0, 0);
        let mut here = || {
            let (r, bytes, tiffs) = pipeline::costin::measured(|| pipeline::unit::run_tail(&anywhere, &dir, &tools));
            read = (read.0 + bytes, read.1 + tiffs);
            r
        };
        // (This Mac's own run of its steps, as its stages have taken here: a worker is waited on
        // only while its pace says it'll be back sooner, and measured against it.)
        let mut here_s = None;
        areas(|a| here_s = a.here_s(&anywhere.iter().map(|r| r.what.as_str()).collect::<Vec<_>>()));
        let patience = pipeline::offload::Patience { here_s: Some(here_s.unwrap_or(TAIL_GUESS_S)) };
        match o.settle(task, &b.dir, wait, patience, &mut here)? {
            None => i += 1,
            Some(how) => {
                let (mut b, _) = waiting.remove(i).unwrap();
                pipeline::costin::add_to(&mut b.sizes, read.0, read.1);
                b.peak = b.peak.max(pipeline::unit::take_peak());
                let how = match how {
                    pipeline::offload::Settled::Remote(w) => format!("by {w}"),
                    pipeline::offload::Settled::Here(None) => "here".into(),
                    pipeline::offload::Settled::Here(Some((w, true))) => format!("here, and by {w} the same"),
                    pipeline::offload::Settled::Here(Some((w, false))) => {
                        format!("here: {w}'s differed, and it gets no more work")
                    }
                };
                commit_unit(out, date, b, &how)?;
            }
        }
    }
    Ok(())
}

/// A unit's last part, its folder ready: the grids its packs lacked, its scenic results kept for
/// its next run, its base pack, road values, road ids and roads' English saved, its folders
/// removed, and what it cost noted. `how`: where its tail's last steps ran. (What it saves is
/// `pipeline::unit::saved_files`, all a helper's hand-off may change: keep the two together.)
fn commit_unit(out: &mut Out, date: &str, b: Built, how: &str) -> Result<()> {
    let Built { u, dir, bdir, rep, tools, carry, piece, t, peak, group, disk, sizes, .. } = b;
    use pipeline::timings::Class;
    use pipeline::unit::{owns, stage as unit_stage};
    areas(|a| a.on(&u.slash()));
    // The DEM samples its elevations made (its last steps' first, here or by a worker), kept.
    if rep.kept_ways > 0 {
        let s = unit_stage("DEM samples kept", Class::NasWrite);
        pipeline::unit::keep_dem_samples(u, &dir, &tools);
        s.end();
    }
    let clean = || {
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&bdir).ok();
        std::fs::remove_dir_all(pipeline::unit::ahead_carry(&dir)).ok();
    };
    // Grids its packs lacked (new coverage), made in the folder: the unit's own z6 tile's go up,
    // for later units and packs. (The canopy step made canopy and cover for every tile.)
    let s = unit_stage("missing grids written", Class::NasWrite);
    for var in ["class", "canopy", "cover"] {
        if rep.staged.missing.get(var).copied().unwrap_or(0) == 0 {
            continue;
        }
        let mut tiles = pipeline::stage::grid_tiles_in(&dir, var, u.x, u.y)?.into_iter();
        layers::write_pack(out, &format!("grid-{var}"), "u8-zstd", false, "hi", (6, u.x, u.y), &mut tiles)?;
    }
    s.end();
    // Its scenic results, for its next run. A cache: not keeping them only costs time later.
    let s = unit_stage("scenic results kept", Class::NasWrite);
    if let Err(e) = carry.save(&dir) {
        eprintln!("unit {}: its scenic results not kept: {e:#}", u.slash());
    }
    s.end();
    eprintln!("unit {}: {} of {} ways touch the coverage, {} owned; {} heritage sites, {} area polygons; its last steps run {how}", u.slash(), rep.kept_ways, rep.piece_ways, rep.owned, rep.heritage, rep.areas);
    if rep.kept_ways == 0 || rep.owned == 0 {
        // None of its ways in the coverage (any more): a base pack and road values from an
        // earlier coverage go, so the map and the map tiles stop showing them.
        let gone: Vec<String> = pipeline::out::UNIT_OUTPUTS
            .iter()
            .map(|p| format!("{p}{}", u.dash()))
            .filter(|l| out.get(l).is_some())
            .collect();
        if !gone.is_empty() {
            for l in &gone {
                out.remove(l);
            }
            eprintln!("unit {}: removed its earlier {}", u.slash(), gone.join(" and "));
        }
        // (Saved before it's noted done: its grids above too.)
        out.save()?;
        let s = unit_stage("its folders removed", Class::Disk);
        clean();
        s.end();
        pipeline::control::done("unit", &u.slash());
        areas(|a| a.finished(&u.slash()));
        return Ok(());
    }
    // The owned ways, in base-pack order, with the pass's road values.
    let s = unit_stage("base pack made", Class::Compute);
    let lg = Legacy::open(&dir)?;
    let tb = tile_bounds(u.z, u.x, u.y);
    let idx: Vec<u32> = lg
        .units()
        .remove(&u)
        .unwrap_or_default()
        .into_iter()
        .filter(|&i| owns(tb, lg.first_vertex(&lg.ways.ways()[i as usize])))
        .collect();
    let built = format!("pass:{date}");
    let bs = legacy::base_sections(&lg, u, &idx, &built);
    s.end();
    let s = unit_stage("base pack written to the NAS", Class::NasWrite);
    let secs: Vec<(&str, &[u8])> = bs.sections.iter().map(|(n, v)| (*n, v.as_slice())).collect();
    // (A copy kept for this Mac's map tiles, where it makes them: `keep_for_packs`.)
    put_sect_kept(out, &format!("base/{}", u.dash()), bs.meta, &secs, Some(&tools.base_packs()))?;
    s.end();
    // (The pass's road values read from the NAS; the unit's written back.)
    let s = unit_stage("road values made and written", Class::Mixed);
    let vals = {
        let _p = pipeline::timings::sub("the pass's road values read", Class::NasRead);
        pass_roads(out, date, u)?
    };
    let ways = lg.ways.ways();
    let verts = lg.ways.verts();
    let recs: Vec<pipeline::legacy::RoadRec> = idx
        .iter()
        .map(|&i| {
            let w = &ways[i as usize];
            match vals.binary_search_by_key(&(w.id as u64), |v| v.0) {
                Ok(k) => vals[k].1,
                Err(_) => {
                    // Not chained (a ferry, a one-vertex way): its own road.
                    let vs = &verts[Legacy::range(w)];
                    let len: f64 = vs
                        .windows(2)
                        .map(|p| {
                            roadcore::dist_m(
                                p[0][0] as f64 * 1e-7,
                                p[0][1] as f64 * 1e-7,
                                p[1][0] as f64 * 1e-7,
                                p[1][1] as f64 * 1e-7,
                            )
                        })
                        .sum();
                    pipeline::legacy::RoadRec { road: w.id as u64, len: len as f32, offset: 0.0, dir: 0, _pad: [0; 7] }
                }
            }
        })
        .collect();
    {
        let _p = pipeline::timings::sub("the unit's written", Class::NasWrite);
        put_roads(out, u, &recs, Some(&tools.base_packs()))?;
        put_road_ids(out, u, &recs)?;
    }
    // The roads' own English (OSM's name:en where it isn't the name), for the server to show
    // with them.
    let en: BTreeMap<String, String> =
        std::fs::read(dir.join("name-en.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mine: BTreeMap<String, String> = idx
        .iter()
        .filter_map(|&i| en.get(&ways[i as usize].id.to_string()).map(|e| (ways[i as usize].id.to_string(), e.clone())))
        .collect();
    let logical = format!("global/roaden/{}", u.dash());
    if !mine.is_empty() {
        let _p = pipeline::timings::sub("the unit's written", Class::NasWrite);
        out.put_bytes(&logical, "json", &serde_json::to_vec(&mine)?)?;
    } else if out.get(&logical).is_some() {
        out.remove(&logical);
    }
    s.end();
    let s = unit_stage("records saved", Class::NasWrite);
    out.save()?;
    pipeline::control::done("unit", &u.slash());
    s.end();
    areas(|a| a.finished(&u.slash()));
    let s = unit_stage("its folders removed", Class::Disk);
    drop(lg);
    clean();
    s.end();
    // The most memory the job's processes held together while it was built here (as the agent's
    // memory guard measures a job: the same kind of figure, so either takes the other's place),
    // noted for the coordinator (`SCENIC_COSTS`), which gives a worker only units that fit its
    // memory; its steps' programs' most, one at a time, logged against its piece's size.
    let piece_mb = std::fs::metadata(&piece).map(|m| m.len() >> 20).unwrap_or(0);
    let peak = peak.max(pipeline::unit::take_peak());
    eprintln!(
        "unit {}: base pack of {} ways in {:.0?}; piece {piece_mb} MB, its steps' programs' peak memory {:.1} GB",
        u.slash(),
        idx.len(),
        t.elapsed(),
        peak as f64 / 1e9
    );
    if let Some(p) = std::env::var_os("SCENIC_COSTS") {
        let line = serde_json::json!({ "unit": u.slash(), "peak_mb": group.max(peak) >> 20, "secs": t.elapsed().as_secs(), "v": pipeline::coord::cost_version("unit"), "disk_mb": disk >> 20, "sizes": sizes });
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
        if let Err(e) = r {
            eprintln!("unit {}: noting what it cost: {e}", u.slash());
        }
    }
    Ok(())
}

/// Drops the prune targets' entries from the manifest (agent::build's prune works): the next
/// catalog leaves them out, and GC frees their files once no catalog of the last 14 days lists them.
fn prune_step(out: &mut Out, args: &[String]) -> Result<()> {
    let p = phase("the targets' entries dropped", Class::Compute);
    let mut gone = 0;
    for t in positional(args) {
        let (kind, at) = t.split_once(' ').with_context(|| format!("{t:?}: a prune target is \"<kind> <z/x/y>\""))?;
        let d = Unit::parse(at).with_context(|| format!("{t:?}: not a tile"))?.dash();
        let logicals = match kind {
            "unit" => pipeline::out::UNIT_OUTPUTS.iter().map(|p| format!("{p}{d}")).collect(),
            "pois" => vec![format!("work/pois/{d}"), format!("work/peaks/{d}")],
            "pack" => vec![format!("hidata/{d}"), format!("layers/roads/hi/{d}"), format!("layers/rails/hi/{d}")],
            "lo" => vec![format!("layers/roads/lo/{d}"), format!("layers/rails/lo/{d}")],
            "bldprep" => vec![format!("work/bld/{d}")],
            "bldtiles" => vec![
                format!("layers/{}/hi/{d}", pipeline::bld::LAYER),
                format!("layers/{}/hi/{d}", pipeline::bld::field::LAYER),
                format!("work/bldfield-mid/{d}"),
            ],
            "bldfield-lo" => vec![format!("layers/{}/lo/{d}", pipeline::bld::field::LAYER)],
            k => bail!("{t:?}: unknown prune target {k:?}"),
        };
        for l in logicals {
            if out.get(&l).is_some() {
                out.remove(&l);
                gone += 1;
            }
        }
    }
    drop(p);
    out.save()?;
    eprintln!("prune: {gone} entries dropped from the manifest");
    Ok(())
}

/// Every unit's reach (pipeline::reach) from its piece, as `sources/osm/<date>/reach`: each piece
/// copied here and read twice (its ways, then their nodes).
fn reach_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::reach::{logical, of_piece, Reaches};
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let list = out.get(&format!("sources/osm/{date}/pieces")).context("the pass's pieces list")?.to_string();
    let p = phase("the pass's pieces list read", Class::NasRead);
    let pieces: pipeline::osmpass::Pieces = serde_json::from_slice(&std::fs::read(out.path(&list))?)?;
    drop(p);
    std::fs::create_dir_all(scratch)?;
    let local = scratch.join("reach-piece.osm.pbf");
    let mut all = Reaches { fmt: 1, date: date.clone(), ..Default::default() };
    let only: Vec<String> = positional(args).iter().filter_map(|s| Unit::parse(s)).map(|u| u.slash()).collect();
    let todo: Vec<(&String, &String)> =
        pieces.pieces.iter().filter(|(u, _)| only.is_empty() || only.contains(u)).collect();
    let n = todo.len() as u64;
    let t0 = std::time::Instant::now();
    for (k, (u, l)) in todo.into_iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, n, "areas");
        let unit = Unit::parse(u).with_context(|| format!("unit {u}"))?;
        let src = out.path(out.get(l).with_context(|| format!("{l} isn't in the manifest"))?);
        let p = phase("pieces copied from the NAS", Class::NasRead);
        p.count(store::sys::copy_data(&src, &local).with_context(|| format!("copy {}", src.display()))?, 1);
        drop(p);
        let t = std::time::Instant::now();
        let p = phase("their roads' reach worked out", Class::Compute);
        let r = of_piece(&local, unit).with_context(|| format!("piece {u}"))?;
        drop(p);
        if let Some(r) = r {
            if !only.is_empty() {
                let (owned, verts) =
                    (r.long.iter().filter(|w| w.owned).count(), r.long.iter().map(|w| w.verts.len()).sum::<usize>());
                eprintln!("reach {u}: owned box {:?}, {} long ways ({owned} owned, {verts} vertices), extent {:?} ({:.1?}, {} MB)", r.owned, r.long.len(), r.extent(unit), t.elapsed(), std::fs::metadata(&local).map(|m| m.len() >> 20).unwrap_or(0));
            }
            all.units.insert(unit.slash(), r);
        }
    }
    std::fs::remove_file(&local).ok();
    if !only.is_empty() {
        return Ok(());
    }
    let p = phase("uploaded", Class::NasWrite);
    out.put_bytes(&logical(&date), "json.zst", &all.encode()?)?;
    drop(p);
    out.save()?;
    eprintln!("reach: {} units with roads of {} pieces ({:.0?})", all.units.len(), n, t0.elapsed());
    Ok(())
}

/// The road → units index from every unit's road ids in the manifest (a unit built before them: its
/// road values; `pipeline::agent::build::roadunits_reads`). Only the lists changed since the index
/// before are read: it names each unit's list by content (section "srcs"), and a unit whose list
/// it names as now keeps its pairs from it. The same index as one made from every list.
fn roadunits(out: &mut Out) -> Result<()> {
    let reads = pipeline::agent::build::roadunits_reads(&out.manifest);
    let srcs: Vec<(u64, String)> = reads.iter().map(|(u, _, c)| (u.key(), c.clone())).collect();
    // (The index before, and the lists it was made from.)
    let p = phase("the index before read", Class::NasRead);
    let before: Option<(BTreeMap<u64, String>, Vec<u64>)> = out.get("global/roadunits").and_then(|c| {
        let r = store::sect::SectReader::open(store::range::PlainFile::open(&out.path(c)).ok()?).ok()?;
        // (Made by another version of the step: read afresh.)
        let (v, s): (u32, Vec<(u64, String)>) = serde_json::from_slice(&r.read("srcs").ok()?).ok()?;
        (v == pipeline::agent::build::ROADUNITS_V).then_some(())?;
        Some((s.into_iter().collect(), r.read_pod("pairs").ok()?))
    });
    drop(p);
    let (was, flat_before) = before.unwrap_or_default();
    let kept: std::collections::BTreeSet<u64> =
        srcs.iter().filter(|(u, c)| was.get(u) == Some(c)).map(|(u, _)| *u).collect();
    let mut pairs: Vec<(u64, u64)> =
        flat_before.chunks_exact(2).map(|p| (p[0], p[1])).filter(|(_, u)| kept.contains(u)).collect();
    let fresh: Vec<&(Unit, String, String)> = reads.iter().filter(|(u, _, _)| !kept.contains(&u.key())).collect();
    let p = phase("the units' road ids read", Class::NasRead);
    for (k, (u, l, content)) in fresh.iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, fresh.len() as u64, "units' road ids read");
        let r = store::sect::SectReader::open(store::range::PlainFile::open(&out.path(content))?)?;
        if l.starts_with(pipeline::out::ROAD_IDS) {
            let ids: Vec<u64> = r.read_pod("ids")?;
            pairs.extend(ids.iter().map(|&id| (id, u.key())));
        } else {
            let recs: Vec<pipeline::legacy::RoadRec> = r.read_pod("roads")?;
            pairs.extend(road_ids(&recs).into_iter().map(|id| (id, u.key())));
        }
        p.count(0, 1);
    }
    drop(p);
    let p = phase("the index sorted", Class::Compute);
    pairs.sort_unstable();
    pairs.dedup();
    let flat: Vec<u64> = pairs.iter().flat_map(|&(r, u)| [r, u]).collect();
    let srcs_json = serde_json::to_vec(&(pipeline::agent::build::ROADUNITS_V, &srcs))?;
    drop(p);
    let _p = phase("uploaded", Class::NasWrite);
    put_sect(
        out,
        "global/roadunits",
        serde_json::json!({"fmt": 1, "pairs": pairs.len()}),
        &[("pairs", b(&flat)), ("srcs", &srcs_json)],
    )?;
    eprintln!(
        "roadunits: {} roads×units over {} units ({} lists read, {} kept from the index before)",
        pairs.len(),
        reads.len(),
        fresh.len(),
        kept.len()
    );
    Ok(())
}

/// The coverage of the regions (recipes in --regions, else inputs/regions; outlines from --pass,
/// else the latest complete pass).
fn coverage_of(out: &Out, args: &[String]) -> Result<pipeline::coverage::Coverage> {
    let _p = phase("the regions and their outlines read", Class::NasRead);
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).unwrap_or_default();
    // (Kept in this Mac's cache, where the agent names one, while what it's made from is as it was:
    // pipeline::coverage::Coverage::load, Coverage::accepted.)
    let cache = std::env::var_os(pipeline::coverage::CACHE_ENV).map(PathBuf::from);
    Ok(regions_coverage(out, args, &date, cache.as_deref())?.0)
}

/// The regions' coverage as a step reads it (pipeline::inputs::regions::read), with the regions'
/// count: `--regions <dir>`, a scratch folder of region files, through the check's own code in
/// memory; else the accepted version.
fn regions_coverage(
    out: &Out,
    args: &[String],
    date: &str,
    cache: Option<&Path>,
) -> Result<(pipeline::coverage::Coverage, usize)> {
    use pipeline::inputs::regions::read;
    if let Some(dir) = opt(args, "--regions").map(PathBuf::from) {
        return read::coverage_of_folder(out.root(), &out.manifest, &dir);
    }
    let _ = date;
    let src = read::source(out.root(), &out.manifest)?;
    let cov = pipeline::coverage::Coverage::accepted(out.root(), &src, cache)?;
    let n = cov.by_region().len();
    Ok((cov, n))
}

/// The z6 tiles named (`6/x/y`), at least one.
fn z6_tiles(args: &[String], step: &str) -> Result<Vec<Unit>> {
    let ts: Vec<Unit> = positional(args)
        .iter()
        .map(|t| {
            Unit::parse(t).filter(|u| u.z == 6 && u.x < 64 && u.y < 64).with_context(|| format!("not a z6 tile: {t}"))
        })
        .collect::<Result<_>>()?;
    anyhow::ensure!(!ts.is_empty(), "{step} <6/x/y …>");
    Ok(ts)
}

/// bld-fetch [--pass d] [--dem dir]: the 3D buildings' sources onto the NAS (docs/buildings3d.md
/// §2.6): dem/bldfetch.py with the coverage written here (its outlines, rail::coverage_geojson),
/// the pinned release's files and GHSL's tiles that meet it grown by 20 km, each fetched once,
/// checked and put in place whole; what's there already skipped. Not through the manifest: the
/// sources are read where they are (`sources/overture/<release>/`, `sources/ghsl/R2023A/`), and
/// their indexes key bldprep (crate::bld::sources).
fn bld_fetch_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    std::fs::create_dir_all(scratch)?;
    let cover = scratch.join("bld-coverage.geojson");
    std::fs::write(&cover, serde_json::to_vec(&pipeline::rail::coverage_geojson(&cov))?)?;
    let _p = phase("bldfetch.py", Class::Mixed);
    let mut c = store::spawn::command("uv");
    // (The declared sources: the release, Overture's bucket, GHSL's tiles by its index.)
    let release = pipeline::buildtiles::release_of(out.root(), &out.manifest)?;
    let declared = pipeline::inputs::sources::write_for_python(out.root(), &out.manifest, scratch)?;
    c.current_dir(&dem)
        .env(pipeline::inputs::sources::PYTHON_ENV, &declared)
        .args(["run", "python", "bldfetch.py", "--root"])
        .arg(out.root())
        .args(["--release", &release, "--coverage"])
        .arg(&cover);
    pipeline::timings::child(&mut c);
    let st = store::spawn::status(&mut c).context("run bldfetch.py")?;
    anyhow::ensure!(st.success(), "bldfetch.py: {st}");
    Ok(())
}

/// bldprep <T …> [--dem dir]: the 3D buildings' normalized files of z6 tiles T (docs/buildings3d.md
/// §3.1), from the downloaded Overture files and GHSL tiles (`sources/overture/<release>/`,
/// `sources/ghsl/R2023A/`) through dem/bldprep.py.
fn bldprep_step(out: &mut Out, args: &[String]) -> Result<()> {
    let ts = z6_tiles(args, "bldprep")?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let names: Vec<String> = ts.iter().map(|t| format!("Reading the buildings of {}", t.slash())).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    for (k, &t) in ts.iter().enumerate() {
        pipeline::control::safe_point("bldprep");
        pipeline::agent::jobs::part(k, &names);
        let c = cost_start("bldprep", &t.slash());
        let st = pipeline::bld::prep::run(out, t, &dem)?;
        eprintln!("bldprep {}: {}", t.slash(), serde_json::to_string(&st)?);
        // (Its inputs' measures: the rows read, the buildings and parts among them.)
        pipeline::costin::set("rows", st.rows);
        pipeline::costin::set("buildings", st.buildings);
        pipeline::costin::set("parts", st.parts);
        pipeline::control::done("bldprep", &t.slash());
        note_cost("bldprep", &t.slash(), c);
    }
    Ok(())
}

/// bldtiles <T …> [--pass d] [--regions dir]: the 3D buildings' tiles of z6 tiles T (docs/buildings3d.md
/// §3.1): those touching the coverage, heights filled, as T's hi pack.
fn bldtiles_step(out: &mut Out, args: &[String]) -> Result<()> {
    let ts = z6_tiles(args, "bldtiles")?;
    let cov = coverage_of(out, args)?;
    let names: Vec<String> = ts.iter().map(|t| format!("Raising the 3D buildings of {}", t.slash())).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    // Other workers, through the lead's coordinator (the agent's jobs there): some of a tile's
    // z8 areas offered as tasks while it builds the others.
    let offload = pipeline::offload::Offload::from_env(&out.scratch);
    for (k, &t) in ts.iter().enumerate() {
        pipeline::control::safe_point("bldtiles");
        pipeline::agent::jobs::part(k, &names);
        let c = cost_start("bldtiles", &t.slash());
        let sum = pipeline::bld::job::build(out, &cov, t, offload.as_ref())?;
        eprintln!("bldtiles {}: {}", t.slash(), serde_json::to_string(&sum)?);
        // (Its inputs' measures: the buildings and parts read, drawn or not.)
        pipeline::costin::set("buildings", sum.buildings + sum.outside);
        pipeline::costin::set("parts", sum.parts);
        pipeline::control::done("bldtiles", &t.slash());
        note_cost("bldtiles", &t.slash(), c);
    }
    Ok(())
}

/// depth-lo <Q …> | depth <T …>: the depth's assemblies of z3 tiles Q (their lo packs, z3–8) or
/// pieces of z6 tiles T (their hi packs, z9–16), from the declared depth sources on the latest
/// pass's water (pipeline::depth::build).
fn depth_step(out: &mut Out, args: &[String], step: &str) -> Result<()> {
    let z = if step == "depth-lo" { 3 } else { 6 };
    let ts: Vec<Unit> = positional(args)
        .iter()
        .map(|t| Unit::parse(t).filter(|u| u.z == z).with_context(|| format!("not a z{z} tile: {t}")))
        .collect::<Result<_>>()?;
    anyhow::ensure!(!ts.is_empty(), "{step} <{z}/x/y …>");
    let p = phase("the depth sources and the basemap's water opened", Class::NasRead);
    let sources = pipeline::depth::grids::declared(out.root(), &out.manifest)?;
    anyhow::ensure!(!sources.is_empty(), "no depth source is declared (inputs/sources/, docs/inputs.md §6.10)");
    let water = pipeline::terrain_pack::open_water(out)?;
    drop(p);
    let names: Vec<String> = ts.iter().map(|t| format!("Composing the depth of {}", t.slash())).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    for (k, &t) in ts.iter().enumerate() {
        pipeline::control::safe_point(step);
        pipeline::agent::jobs::part(k, &names);
        let c = cost_start(step, &t.slash());
        let n = if z == 3 {
            pipeline::depth::build::build_lo(out, &sources, &water, (t.x, t.y))?
        } else {
            pipeline::depth::build::build_hi(out, &sources, &water, (t.x, t.y))?
        };
        eprintln!("{step} {}: {n} tiles", t.slash());
        pipeline::control::done(step, &t.slash());
        note_cost(step, &t.slash(), c);
    }
    Ok(())
}

/// treeblock-task's work: row `row`'s task folder in `dir/task` (cut from its piece's coverage as
/// the piece's run cuts it), `dir/task.json` (its blocks and the canopy squares the NAS's store
/// has whole there), and `dir/packs/8-x-y/` its blocks' tiles from the manifest's hi packs (zoom
/// 9–12) and its piece's mid (zoom 8, the values), written as the trees program writes a block.
fn treeblock_task(out: &Out, cov: &pipeline::coverage::Coverage, row: &[(u32, u32)], dir: &Path) -> Result<()> {
    use pipeline::trees::{self, task};
    anyhow::ensure!(
        !row.is_empty()
            && row.iter().all(|b| b.1 == row[0].1 && (b.0 >> 2, b.1 >> 2) == (row[0].0 >> 2, row[0].1 >> 2)),
        "a row: blocks of one z6 tile and one row"
    );
    let piece = Unit { z: 6, x: row[0].0 >> 2, y: row[0].1 >> 2 };
    std::fs::remove_dir_all(dir).ok();
    let cj = serde_json::to_string(&pipeline::treepacks::coverage_json(cov, piece))?;
    task::cut(&cj, row, &dir.join("task"))?;
    let store = out.root().join("sources/canopy");
    let whole = |top: i32, left: i32| {
        ["cover5m", "p95"]
            .iter()
            .all(|k| std::fs::metadata(store.join(trees::chm_name(top, left, k))).is_ok_and(|m| m.len() > 0))
    };
    let mut squares: Vec<(i32, i32)> = row
        .iter()
        .flat_map(|&(x, y)| trees::squares_of(trees::tile_bounds(trees::ZBLOCK, x, y)))
        .filter(|&(t, l)| whole(t, l))
        .collect();
    squares.sort_unstable();
    squares.dedup();
    std::fs::write(
        dir.join("task.json"),
        serde_json::to_vec(
            &serde_json::json!({ "blocks": task::blocks_arg(row), "squares": task::squares_arg(&squares), "piece": piece.slash() }),
        )?,
    )?;
    // The NAS's tiles of each block.
    let mid = out
        .get(&pipeline::treepacks::mid_logical(piece.x, piece.y))
        .with_context(|| format!("{} has no mid", piece.slash()))?;
    let mid = trees::read_mid(&out.path(mid))?;
    let mut packs = Vec::new();
    for l in pipeline::treepacks::LAYERS {
        packs.push(match out.get(&format!("layers/{l}/hi/{}", piece.dash())) {
            Some(c) => {
                let f = store::range::PlainFile::open(&out.path(c))?;
                let idx = store::pack::PackIndex::read_from(&f)?;
                Some((f, idx))
            }
            None => None,
        });
    }
    for &(bx, by) in row {
        let mut tiles: Vec<trees::pyramid::Tile> = mid.z8.iter().filter(|t| (t.x, t.y) == (bx, by)).cloned().collect();
        for (layer, p) in packs.iter().enumerate() {
            let Some((f, idx)) = p else { continue };
            for e in &idx.entries {
                let (z, x, y) = e.zxy();
                if z > trees::ZBLOCK && (x >> (z - trees::ZBLOCK), y >> (z - trees::ZBLOCK)) == (bx, by) {
                    tiles.push(trees::pyramid::Tile { layer: layer as u8, z, x, y, webp: idx.read_blob(f, e)? });
                }
            }
        }
        let tops = mid.tops.get(&(bx, by)).with_context(|| format!("the mid has no values of 8/{bx}/{by}"))?;
        let d = trees::block_dir(&dir.join("packs"), (bx, by));
        std::fs::create_dir_all(&d)?;
        let mut w = trees::Writers::create(&d)?;
        for t in &tiles {
            w.add(t)?;
        }
        w.finish()?;
        std::fs::write(d.join(trees::TOPS), tops)?;
    }
    println!(
        "{}",
        serde_json::json!({ "piece": piece.slash(), "blocks": task::blocks_arg(row), "squares": task::squares_arg(&squares), "coverage_bytes": std::fs::metadata(dir.join("task/u").join(task::COVERAGE))?.len() })
    );
    Ok(())
}

/// bldtile-task <8/x/y | 6/x/y> --out dir [--pass d] [--regions dir]: a z8 area's task folder, as a
/// `bldtiles` job cuts it for a worker (pipeline::bld::task::cut; for the checks: the `bldtile`
/// program then runs over it). A z6 tile: its area with the most buildings.
fn bldtile_task_step(out: &mut Out, args: &[String]) -> Result<()> {
    let u = positional(args)
        .first()
        .and_then(|s| Unit::parse(s))
        .filter(|u| u.z == 6 || u.z == 8)
        .context("bldtile-task <8/x/y | 6/x/y> --out dir")?;
    let dir = PathBuf::from(opt(args, "--out").context("--out dir")?);
    let cov = coverage_of(out, args)?;
    let t = pipeline::bld::task::tile_of(Unit {
        z: 8,
        x: if u.z == 6 { u.x << 2 } else { u.x },
        y: if u.z == 6 { u.y << 2 } else { u.y },
    });
    let files = pipeline::bld::job::work_files(out, t)?;
    let a = if u.z == 8 {
        (u.x, u.y)
    } else {
        // (The area whose blocks hold the most records.)
        let own = files[4].as_ref().context("no work file for that tile")?;
        let mut by: BTreeMap<(u32, u32), u64> = BTreeMap::new();
        for e in &own.index {
            let (_, x, y) = pipeline::bld::key_zxy(e.key);
            *by.entry((x >> 6, y >> 6)).or_default() += e.count as u64;
        }
        by.into_iter().max_by_key(|(a, n)| (*n, std::cmp::Reverse(*a))).context("no buildings in that tile")?.0
    };
    let t0 = std::time::Instant::now();
    let sites = pipeline::bld::job::sites(out, t)?;
    let c = pipeline::bld::task::cut(&files, &cov, t, a, &sites, &dir)?;
    println!(
        "{}",
        serde_json::json!({ "area": format!("8/{}/{}", a.0, a.1), "bytes": c.bytes, "records": c.records, "mem_mb": c.mem_mb(), "secs": t0.elapsed().as_secs_f64() })
    );
    Ok(())
}

/// The z6 tiles an area's whole terrain or slope run makes, by z3 pack: those near the coverage (as
/// the agent's keys list them, `build::coverage_tiles`) of each z3 pack named (`3/x/y`), or with
/// none named every z6 tile near the coverage. (Z6 tiles named are pieces: `pieces_named`.)
fn terrain_targets(
    cov: &pipeline::coverage::Coverage,
    args: &[String],
) -> Result<BTreeMap<(u32, u32), Vec<(u32, u32)>>> {
    let _p = phase("the areas' pieces near the coverage worked out", Class::Compute);
    let near = pipeline::agent::build::coverage_tiles(cov);
    let named: Vec<Unit> = positional(args)
        .iter()
        .map(|s| Unit::parse(s).with_context(|| format!("not a tile: {s}")))
        .collect::<Result<_>>()?;
    if named.is_empty() {
        return Ok(near);
    }
    let mut by_q: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for t in named {
        match t.z {
            3 => {
                let list = near
                    .get(&(t.x, t.y))
                    .with_context(|| format!("3/{}/{}: no tile of it is near the coverage", t.x, t.y))?;
                by_q.entry((t.x, t.y)).or_default().extend(list);
            }
            z => anyhow::bail!("{z}/{}/{}: an area's whole run takes z3 tiles", t.x, t.y),
        }
    }
    for list in by_q.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    Ok(by_q)
}

/// The z6 tiles named (pieces: `terrain` and `slope` of z6 tiles; one not near the coverage is a
/// "none" piece, whose run drops its hi pack and mid, if any); None when none is (an area's whole
/// run, of z3 tiles or of all); an error when z6 tiles are named with others. `--expect-same`'s,
/// those of them made again as they are (each near the coverage).
fn pieces_named(cov: &pipeline::coverage::Coverage, args: &[String]) -> Result<Option<(Vec<Unit>, BTreeSet<String>)>> {
    let _p = phase("the pieces near the coverage worked out", Class::Compute);
    let mut named: Vec<Unit> = positional(args)
        .iter()
        .map(|s| Unit::parse(s).with_context(|| format!("not a tile: {s}")))
        .collect::<Result<_>>()?;
    // (`--pieces-of Q,…`: every piece of those z3 tiles, by hand.)
    if let Some(qs) = opt(args, "--pieces-of") {
        let near = pipeline::agent::build::coverage_tiles(cov);
        for q in qs.split(',').filter(|q| !q.is_empty()) {
            let u = Unit::parse(q).filter(|u| u.z == 3).with_context(|| format!("not a z3 tile: {q}"))?;
            let ts = near.get(&(u.x, u.y)).with_context(|| format!("{q}: no tile of it is near the coverage"))?;
            named.extend(ts.iter().map(|&(x, y)| Unit { z: 6, x, y }));
        }
    }
    let same: BTreeSet<String> = opt(args, "--expect-same")
        .map(|v| v.split(',').filter(|t| !t.is_empty()).map(String::from).collect())
        .unwrap_or_default();
    if named.is_empty() || named.iter().all(|u| u.z != 6) {
        anyhow::ensure!(same.is_empty(), "--expect-same is for pieces (z6 tiles)");
        return Ok(None);
    }
    anyhow::ensure!(named.iter().all(|u| u.z == 6), "z6 tiles (pieces) or z3 tiles (areas' whole runs), not both");
    let near = pipeline::agent::build::coverage_tiles(cov);
    for u in &named {
        if near.get(&(u.x >> 3, u.y >> 3)).is_some_and(|ts| ts.contains(&(u.x, u.y))) {
            continue;
        }
        anyhow::ensure!(
            !same.contains(&u.slash()),
            "{}: not near the coverage, so it can't be made as it is",
            u.slash()
        );
    }
    let slashes: BTreeSet<String> = named.iter().map(Unit::slash).collect();
    anyhow::ensure!(
        same.is_subset(&slashes),
        "--expect-same names z6 tiles not given: {:?}",
        same.difference(&slashes).collect::<Vec<_>>()
    );
    Ok(Some((named, same)))
}

/// The z3 tiles named (assemblies: `terrain-lo`, `slope-lo`), each with its pieces (its z6 tiles
/// near the coverage, in column then row order; none for a "none" assembly, a z3 tile the coverage
/// has left, whose run drops its lo pack).
fn areas_named(
    cov: &pipeline::coverage::Coverage,
    args: &[String],
    step: &str,
) -> Result<Vec<(Unit, Vec<(u32, u32)>)>> {
    let _p = phase("the areas' pieces near the coverage worked out", Class::Compute);
    let near = pipeline::agent::build::coverage_tiles(cov);
    let qs: Vec<Unit> = positional(args)
        .iter()
        .map(|t| Unit::parse(t).filter(|u| u.z == 3).with_context(|| format!("not a z3 tile: {t}")))
        .collect::<Result<_>>()?;
    anyhow::ensure!(!qs.is_empty(), "{step} <3/x/y …>");
    Ok(qs.into_iter().map(|q| (q, near.get(&(q.x, q.y)).cloned().unwrap_or_default())).collect())
}

/// The "none" assemblies of `qs` (those without a piece: `areas_named`) run: each z3 tile's lo pack
/// of `step`'s layer dropped; the others returned. Nothing is read for them, so a job of them alone opens
/// no sources.
fn none_assemblies(
    out: &mut Out,
    step: &str,
    qs: Vec<(Unit, Vec<(u32, u32)>)>,
) -> Result<Vec<(Unit, Vec<(u32, u32)>)>> {
    let (gone, rest): (Vec<_>, Vec<_>) = qs.into_iter().partition(|(_, ts)| ts.is_empty());
    for (q, _) in &gone {
        pipeline::control::safe_point(step);
        let n = if step == "terrain-lo" {
            pipeline::terrain_pack::drop_lo(out, (q.x, q.y))?
        } else {
            pipeline::slope_pack::drop_lo(out, (q.x, q.y))?
        };
        eprintln!("{step} {}: the coverage has left it; {n} files dropped", q.slash());
        pipeline::control::done(step, &q.slash());
    }
    Ok(rest)
}

/// terrain <6/x/y …> [--expect-same T,…]: terrain's pieces (pipeline::terrain_pack::build_piece:
/// each z6 tile's hi pack and mid), then the raw tiles AWS gave packed onto the NAS.
fn terrain_pieces(
    out: &mut Out,
    args: &[String],
    cov: &pipeline::coverage::Coverage,
    ts: &[Unit],
    same: &BTreeSet<String>,
) -> Result<()> {
    // ("None" pieces, z6 tiles the coverage has left, first: their hi packs and mids dropped, nothing
    // read, no cost noted; a job of them alone opens no sources and packs nothing.)
    let (ts, gone): (Vec<Unit>, Vec<Unit>) =
        ts.iter().partition(|t| pipeline::terrain_pack::near_coverage(cov, 6, t.x, t.y, 20.0));
    for t in &gone {
        pipeline::control::safe_point("terrain");
        let n = pipeline::terrain_pack::drop_piece(out, (t.x, t.y))?;
        eprintln!("terrain {}: the coverage has left it; {n} files dropped", t.slash());
        pipeline::control::done("terrain", &t.slash());
    }
    if ts.is_empty() {
        return Ok(());
    }
    let ts = &ts[..];
    let raw_dir = PathBuf::from(
        opt(args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
    );
    let raw = raw_tiles(out, &raw_dir)?;
    let opened = pipeline::terrain_pack::SourceFiles::open(out, true)?;
    let coarse = pipeline::terrain_pack::Coarse::new(&raw);
    let src = opened.sources(Some(&coarse));
    let n = ts.len();
    let mut names: Vec<String> =
        ts.iter().map(|t| format!("Fetching, shading and writing {}'s terrain tiles", t.slash())).collect();
    names.push("Packing the new raw tiles onto the NAS".into());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let say = |what: &str, done: u64, total: u64| pipeline::agent::jobs::report(done, total, what);
    // Other workers, through the lead's coordinator: some of each piece's z8 subtrees
    // (pipeline::terrain_task).
    // (Each piece's offered as it begins; offering ahead (`offer_ahead`), the next's then too, its
    // head start the piece under way's tiles.)
    let offload = pipeline::offload::Offload::from_env(&out.scratch);
    let offer_after = |t: &Unit, before: usize| {
        pipeline::terrain_task::Offers::offer_after(
            offload.as_ref(),
            (t.x, t.y),
            &pipeline::terrain_pack::piece_levels(cov, (t.x, t.y)),
            &raw,
            &src,
            before,
        )
    };
    let offer = |t: &Unit| offer_after(t, 0);
    let offer_ahead = pipeline::terrain_task::offer_ahead();
    let mut ahead: Option<pipeline::terrain_task::Offers> = None;
    for (k, t) in ts.iter().enumerate() {
        pipeline::control::safe_point("terrain");
        pipeline::agent::jobs::part(k, &names);
        let c = cost_start("terrain", &t.slash());
        let offers = match ahead.take() {
            Some(o) => o,
            None => offer(t),
        };
        if offer_ahead {
            let before: usize =
                pipeline::terrain_pack::piece_levels(cov, (t.x, t.y)).iter().map(|(_, l)| l.len()).sum();
            ahead = ts.get(k + 1).map(|n| offer_after(n, before));
        }
        let r = pipeline::terrain_pack::build_piece_offered(
            out,
            &raw,
            (t.x, t.y),
            cov,
            &src,
            same.contains(&t.slash()),
            &say,
            offers,
        )?;
        eprintln!("terrain {} ({} of {n}): {r:?} ({:.0?})", t.slash(), k + 1, c.elapsed());
        pipeline::control::done("terrain", &t.slash());
        note_cost("terrain", &t.slash(), c);
    }
    pipeline::control::safe_point("terrain");
    pipeline::agent::jobs::part(names.len() - 1, &names);
    pack_raw_with(out, &raw_dir, &say);
    Ok(())
}

/// terrain-water [--pass d]: the terrain's water digests (`pipeline::terrain_water::make_idx`) of
/// what each terrain piece and assembly near the coverage reads (agent::build::water_wants), of
/// the latest basemap's water, under `work/water-idx/<hash16 of its pin>`: those already there for
/// the same reads kept, the others read from the basemap.
fn terrain_water_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let src = pipeline::terrain_pack::open_water(out)?;
    let idx = water_idx_of(out, &cov, &src, &|done, total| pipeline::agent::jobs::report(done, total, "targets"))?;
    let local = out.scratch_file("terrain-water.json");
    std::fs::write(&local, serde_json::to_vec(&idx)?)?;
    let name = out.put_file(&pipeline::terrain_water::idx_logical(&idx.pin), "json", &local)?;
    std::fs::remove_file(&local).ok();
    out.save()?;
    eprintln!("terrain-water: {} targets' digests, {name}", idx.digests.len());
    Ok(())
}

/// The water digests the coverage's terrain wants of `src` (agent::build::water_wants), those the
/// manifest has for its pin kept.
fn water_idx_of(
    out: &Out,
    cov: &pipeline::coverage::Coverage,
    src: &dyn pipeline::terrain_water::WaterSource,
    progress: &dyn Fn(u64, u64),
) -> Result<pipeline::terrain_water::WaterIdx> {
    let pin = src.pin();
    anyhow::ensure!(
        Some(&pin) == pipeline::terrain_pack::water_source_pin(&out.manifest).as_ref(),
        "the water source's pin ({pin}) isn't the one the plan names"
    );
    let prior = match out.get(&pipeline::terrain_water::idx_logical(&pin)) {
        Some(c) => Some(pipeline::terrain_water::WaterIdx::from_bytes(&std::fs::read(out.path(c))?)?),
        None => None,
    };
    // (The depth's targets' too, once its sources are declared: pipeline::depth. Declarations that
    // can't be read now leave the depth out, never the terrain's digests.)
    let mut reads = pipeline::agent::build::water_reads(cov);
    let depth = pipeline::depth::grids::declared(out.root(), &out.manifest).unwrap_or_else(|e| {
        eprintln!("terrain-water: the depth's targets left out: its declarations can't be read ({e:#})");
        Vec::new()
    });
    for r in pipeline::agent::build::depth_water_reads(cov, &depth) {
        if !reads.iter().any(|x| x.0 == r.0 && x.1 == r.1) {
            reads.push(r);
        }
    }
    pipeline::terrain_water::make_idx(src, prior.as_ref(), &reads, progress)
}

/// terrain-lo <3/x/y …>: terrain's assemblies (pipeline::terrain_pack::build_lo: each z3 tile's lo
/// pack from its pieces' mids), then the raw tiles AWS gave packed onto the NAS.
fn terrain_lo_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let qs = none_assemblies(out, "terrain-lo", areas_named(&cov, args, "terrain-lo")?)?;
    if qs.is_empty() {
        return Ok(());
    }
    let raw_dir = PathBuf::from(
        opt(args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
    );
    let raw = raw_tiles(out, &raw_dir)?;
    let opened = pipeline::terrain_pack::SourceFiles::open(out, true)?;
    let src = opened.sources(None);
    let mut names: Vec<String> =
        qs.iter().map(|(q, _)| format!("Assembling {}'s zoomed-out terrain", q.slash())).collect();
    names.push("Packing the new raw tiles onto the NAS".into());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let say = |what: &str, done: u64, total: u64| pipeline::agent::jobs::report(done, total, what);
    for (k, (q, ts)) in qs.iter().enumerate() {
        pipeline::control::safe_point("terrain-lo");
        pipeline::agent::jobs::part(k, &names);
        let c = cost_start("terrain-lo", &q.slash());
        let r = pipeline::terrain_pack::build_lo(out, &raw, (q.x, q.y), ts, &src, &say)?;
        eprintln!("terrain-lo {}: {r:?} ({:.0?})", q.slash(), c.elapsed());
        pipeline::control::done("terrain-lo", &q.slash());
        note_cost("terrain-lo", &q.slash(), c);
    }
    pipeline::control::safe_point("terrain-lo");
    pipeline::agent::jobs::part(names.len() - 1, &names);
    pack_raw_with(out, &raw_dir, &say);
    Ok(())
}

/// slope <6/x/y …> [--expect-same T,…]: slope's pieces (pipeline::slope_pack::build_piece); of a z6
/// tile the coverage has left, its hi pack and mid dropped (pipeline::slope_pack::drop_piece).
fn slope_pieces(out: &mut Out, cov: &pipeline::coverage::Coverage, ts: &[Unit], same: &BTreeSet<String>) -> Result<()> {
    let names: Vec<String> = ts.iter().map(|t| format!("Working out {}'s slope", t.slash())).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    for (k, t) in ts.iter().enumerate() {
        pipeline::control::safe_point("slope");
        pipeline::agent::jobs::part(k, &names);
        // (A "none" piece first: nothing read, neither a `started` nor a cost line.)
        if !pipeline::terrain_pack::near_coverage(cov, 6, t.x, t.y, 20.0) {
            let n = pipeline::slope_pack::drop_piece(out, (t.x, t.y))?;
            eprintln!("slope {}: the coverage has left it; {n} files dropped", t.slash());
            pipeline::control::done("slope", &t.slash());
            continue;
        }
        let c = cost_start("slope", &t.slash());
        let r = pipeline::slope_pack::build_piece(out, (t.x, t.y), same.contains(&t.slash()), &|what, done, total| {
            pipeline::agent::jobs::report(done, total, what)
        })?;
        eprintln!("slope {}: {r:?} ({:.0?})", t.slash(), c.elapsed());
        pipeline::control::done("slope", &t.slash());
        note_cost("slope", &t.slash(), c);
    }
    Ok(())
}

/// slope-lo <3/x/y …>: slope's assemblies (pipeline::slope_pack::build_lo).
fn slope_lo_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let qs = none_assemblies(out, "slope-lo", areas_named(&cov, args, "slope-lo")?)?;
    for (k, (q, ts)) in qs.iter().enumerate() {
        pipeline::control::safe_point("slope-lo");
        pipeline::agent::jobs::report(k as u64, qs.len() as u64, "areas");
        let c = cost_start("slope-lo", &q.slash());
        let r = pipeline::slope_pack::build_lo(out, (q.x, q.y), ts, &|_, _, _| {})?;
        eprintln!("slope-lo {}: {r:?} ({:.0?})", q.slash(), c.elapsed());
        pipeline::control::done("slope-lo", &q.slash());
        note_cost("slope-lo", &q.slash(), c);
    }
    pipeline::agent::jobs::report(qs.len() as u64, qs.len() as u64, "areas");
    Ok(())
}

fn terrain_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    if let Some((ts, same)) = pieces_named(&cov, args)? {
        return terrain_pieces(out, args, &cov, &ts, &same);
    }
    let by_q = terrain_targets(&cov, args)?;
    eprintln!("terrain: {} z6 tiles in {} z3 packs", by_q.values().map(Vec::len).sum::<usize>(), by_q.len());
    // AWS's raw tiles: this Mac's cache, filled from the NAS's store.
    let raw_dir = PathBuf::from(
        opt(args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()),
    );
    let raw = raw_tiles(out, &raw_dir)?;
    // GLO-30 north of 60°N and the pass's basemap's water (docs/plan.md §6, Terrain), and AWS's z9
    // tiles for the walled patches.
    let opened = pipeline::terrain_pack::SourceFiles::open(out, true)?;
    let coarse = pipeline::terrain_pack::Coarse::new(&raw);
    let src = opened.sources(Some(&coarse));
    // Its parts, for the status (agent::jobs::part): each area's tiles fetched and shaded, then its
    // terrain written; then the raw tiles AWS gave packed onto the NAS. Each says how far it is.
    let n = by_q.len();
    let of = |k: usize| if n > 1 { format!(" ({} of {n})", k + 1) } else { String::new() };
    let mut names: Vec<String> = Vec::new();
    for k in 0..n {
        names.push(format!("Fetching, shading and writing the area's terrain tiles{}", of(k)));
        names.push(format!("Writing the area's zoomed-out terrain to the NAS{}", of(k)));
    }
    names.push("Packing the new raw tiles onto the NAS".into());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let say = |what: &str, done: u64, total: u64| pipeline::agent::jobs::report(done, total, what);
    for (k, (q, list)) in by_q.into_iter().enumerate() {
        pipeline::control::safe_point("terrain");
        pipeline::agent::jobs::part(2 * k, &names);
        let writing = std::sync::atomic::AtomicBool::new(false);
        let t = cost_start("terrain", &format!("3/{}/{}", q.0, q.1));
        let r = pipeline::terrain_pack::build_q_with(out, &raw, q, &list, &cov, &src, &|what, done, total| {
            if what == "packs" && !writing.swap(true, std::sync::atomic::Ordering::Relaxed) {
                pipeline::agent::jobs::part(2 * k + 1, &names);
            }
            say(what, done, total);
        })?;
        eprintln!("terrain 3/{}/{}: {r:?} ({:.0?})", q.0, q.1, t.elapsed());
        pipeline::control::done("terrain", &format!("3/{}/{}", q.0, q.1));
        note_cost("terrain", &format!("3/{}/{}", q.0, q.1), t);
    }
    // (The areas are written: with the build pausing, the raw tiles wait in the cache for the next
    // job, which packs them.)
    pipeline::control::safe_point("terrain");
    pipeline::agent::jobs::part(names.len() - 1, &names);
    pack_raw_with(out, &raw_dir, &say);
    Ok(())
}

fn slope_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    if let Some((ts, same)) = pieces_named(&cov, args)? {
        return slope_pieces(out, &cov, &ts, &same);
    }
    let by_q = terrain_targets(&cov, args)?;
    eprintln!("slope: {} z6 tiles in {} z3 packs", by_q.values().map(Vec::len).sum::<usize>(), by_q.len());
    // Its parts, for the status (as terrain's): each area's slope worked out, then written. Each
    // says how far it is.
    let n = by_q.len();
    let of = |k: usize| if n > 1 { format!(" ({} of {n})", k + 1) } else { String::new() };
    let mut names: Vec<String> = Vec::new();
    for k in 0..n {
        names.push(format!("Working out the area's slope{}", of(k)));
        names.push(format!("Writing the area's slope to the NAS{}", of(k)));
    }
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    for (k, (q, list)) in by_q.into_iter().enumerate() {
        pipeline::control::safe_point("slope");
        pipeline::agent::jobs::part(2 * k, &names);
        let writing = std::sync::atomic::AtomicBool::new(false);
        let t = cost_start("slope", &format!("3/{}/{}", q.0, q.1));
        let r = pipeline::slope_pack::build_q_with(out, q, &list, &|what, done, total| {
            if what == "packs written" && !writing.swap(true, std::sync::atomic::Ordering::Relaxed) {
                pipeline::agent::jobs::part(2 * k + 1, &names);
            }
            pipeline::agent::jobs::report(done, total, what);
        })?;
        pipeline::control::done("slope", &format!("3/{}/{}", q.0, q.1));
        eprintln!("slope 3/{}/{}: {r:?} ({:.0?})", q.0, q.1, t.elapsed());
        note_cost("slope", &format!("3/{}/{}", q.0, q.1), t);
    }
    Ok(())
}

/// rail-feeds [--pass <date>] [--dem dir] [--feeds-index <index>]: the timetable feeds
/// (pipeline::rail): dem/railfeeds.py on the accepted declarations of the timetable feeds (the
/// index the agent names, else the records', pipeline::inputs::feeds) and the rail sources (the catalogue, the feeds
/// checked so far, the NAS's zips) with the countries the coverage is in (from the pass's outlines
/// and the catalogue declaration's `own_code`), each feed fetched once, as `sources/rail/feeds` (the
/// rail feeds) and `sources/rail/ferry-feeds`. What it checked and fetched is kept even when it
/// fails (a feed's server that doesn't answer, an upload that fails), so the next try starts from
/// there.
fn rail_feeds_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::rail;
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let decls = {
        let unit = pipeline::inputs::feeds::UNIT;
        let index = opt(args, "--feeds-index")
            .or_else(|| out.get(&pipeline::inputs::logical(unit)).map(str::to_string))
            .context("no accepted version of the timetable feeds yet (--feeds-index <its index's content name>)")?;
        let m = std::collections::BTreeMap::from([(pipeline::inputs::logical(unit), index)]);
        let a =
            pipeline::inputs::open(out.root(), &m, unit)?.with_context(|| format!("no accepted version of {unit}"))?;
        pipeline::inputs::feeds::of_accepted(&a)?
    };
    let catalogue = out.get(rail::CATALOGUE).map(|c| out.path(c));
    // (A catalogue without its declaration, held by the input check, would be read without its
    // corrections: the job waits for it instead.)
    anyhow::ensure!(catalogue.is_none() || decls.iter().any(|d| d["format"] == pipeline::inputs::feeds::CATALOGUE), "the catalogue's declaration isn't in the accepted version of the timetable feeds (held?): its corrections would be missing");
    let parts = Parts(&[
        "Finding the countries the coverage is in",
        "Finding and fetching the feeds (railfeeds.py)",
        "Uploading",
    ]);
    parts.start(0);
    let p = phase("the countries the coverage is in found", Class::NasRead);
    let outlines = pipeline::outlines::Outlines::open(
        &out.path(out.get(&format!("sources/osm/{date}/outlines")).context("the pass's outlines")?),
    )?;
    let countries = rail::countries(&cov, &outlines, &pipeline::inputs::feeds::own_code(&decls))?;
    drop(p);
    let p = phase("the feeds checked and fetched before read", Class::NasRead);
    eprintln!("rail-feeds: the coverage is in {}", countries.join(", "));
    std::fs::create_dir_all(scratch)?;
    let (cover, checked, cache, declared) = (
        scratch.join("coverage.geojson"),
        scratch.join("checked-in.json"),
        scratch.join("cache.json"),
        scratch.join("declarations.json"),
    );
    std::fs::write(&cover, serde_json::to_vec(&rail::coverage_geojson(&cov))?)?;
    std::fs::write(&declared, serde_json::to_vec(&decls)?)?;
    match out.get(rail::CHECKED) {
        Some(c) => store::sys::copy_data(out.path(c), &checked).map(|_| ())?,
        None => std::fs::write(&checked, b"[]")?,
    }
    // (The declared feeds' first pins, before the NAS's zips are listed: a feed pinned isn't fetched.)
    let pinned = rail::pin_seeds(out, &decls, &scratch.join("pins"))?;
    if pinned > 0 {
        eprintln!("rail-feeds: {pinned} zips pinned from their declarations");
    }
    std::fs::write(&cache, serde_json::to_vec(&rail::cache_index(out, &rail::read_fetched(out)?))?)?;
    // (Its downloads stay in `found` until they're on the NAS: a run cut short keeps them. So does
    // railfeeds.py's record of the servers that haven't answered, until the job completes.)
    let found = scratch.join("found");
    drop(p);
    parts.start(1);
    let p = phase("railfeeds.py", Class::Mixed);
    let mut c = store::spawn::command("uv");
    c.current_dir(&dem)
        .args(["run", "python", "railfeeds.py", "--declarations"])
        .arg(&declared)
        .args(catalogue.iter().flat_map(|c| [std::ffi::OsStr::new("--catalogue"), c.as_os_str()]))
        .arg("--coverage")
        .arg(&cover)
        .arg("--countries")
        .arg(countries.join(","))
        .arg("--checked")
        .arg(&checked)
        .arg("--cache")
        .arg(&cache)
        .arg("--keys")
        .arg(out.root().join("inputs/keys.env"))
        .arg("--out")
        .arg(&found);
    pipeline::timings::child(&mut c);
    let st = store::spawn::status(&mut c).context("run railfeeds.py")?;
    drop(p);
    parts.start(2);
    let p = phase("the feeds uploaded", Class::NasWrite);
    if found.join("checked.json").exists() {
        out.put_file(rail::CHECKED, "json", &found.join("checked.json"))?;
        out.save()?;
    }
    // (The catalogue fetched from its declaration's URL, there being no copy: its first pin.)
    if catalogue.is_none() && found.join("catalogue.csv").exists() {
        out.put_file(rail::CATALOGUE, "csv", &found.join("catalogue.csv"))?;
        out.save()?;
    }
    if !st.success() {
        let n = rail::keep_downloads(out, &found.join("gtfs"))?;
        out.save()?;
        bail!("railfeeds.py failed ({st}); the {n} zips it fetched are kept");
    }
    let new = rail::put_feeds(out, &found.join("feeds.json"), &found.join("gtfs"), rail::FEEDS)?
        + rail::put_feeds(out, &found.join("ferries.json"), &found.join("gtfs"), rail::FERRY_FEEDS)?;
    drop(p);
    out.save()?;
    std::fs::remove_dir_all(&found).ok();
    for f in [cover, checked, cache, declared] {
        std::fs::remove_file(f).ok();
    }
    eprintln!("rail-feeds: {new} zips fetched");
    Ok(())
}

/// The static timetables' accepted version a step reads (pipeline::inputs::gtfs): the index the
/// agent names (`--gtfs-index`), else the records' now (by hand).
fn static_timetables(out: &Out, args: &[String]) -> Result<pipeline::inputs::Accepted> {
    let unit = pipeline::inputs::gtfs::UNIT;
    let index = opt(args, "--gtfs-index")
        .or_else(|| out.get(&pipeline::inputs::logical(unit)).map(str::to_string))
        .context("no accepted version of the static timetables yet (--gtfs-index <its index's content name>)")?;
    let m = std::collections::BTreeMap::from([(pipeline::inputs::logical(unit), index)]);
    pipeline::inputs::open(out.root(), &m, unit)?.with_context(|| format!("no accepted version of {unit}"))
}

/// rail [--pass <date>] [--dem dir] [--cache dir] [--gtfs-index <index>]: trains a day on the
/// coverage's rail ways (pipeline::rail), as `global/railfreq`. The stop pairs of the feeds and of
/// the static timetables' accepted zips (dem/railgtfs.py, kept in the cache for the list of feeds
/// and the static files' version, which don't depend on the coverage): their timed trains, and
/// apart, the static files' count rows as given; their stops
/// beyond the coverage marked, matched by `railfreq` onto the rail ways of the pass's rail set that
/// touch the coverage: the set clipped to the tiles within 20 km of it (as the heritage jobs' cover),
/// then `extract` (8 m, as the units), then the ways touching it. The feeds' days and trips go to
/// `work/rail/used`.
fn rail_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::rail;
    let t0 = std::time::Instant::now();
    let date =
        opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache =
        PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()))
            .join("rail");
    let bin = std::env::current_exe()?.parent().context("bin")?.to_path_buf();
    let feeds = out.get(rail::FEEDS).context("no rail feeds yet (the rail-feeds step)")?.to_string();
    let statics = static_timetables(out, args)?;
    let set = out.path(out.get(&pipeline::osmpass::set_name(&date, "rail")).context("the pass has no rail set")?);
    std::fs::create_dir_all(scratch)?;
    // 1. The feeds' stop pairs, once per list of feeds (and the step's version).
    let parts = Parts(&[
        "Reading the feeds' trains (railgtfs.py)",
        "Adding the stops beyond the coverage",
        "Clipping the pass's rail to the coverage (osmium)",
        "Extracting its rail ways",
        "Matching the trains onto the tracks (railfreq)",
    ]);
    parts.start(0);
    let tag =
        store::naming::hash16(format!("{} {feeds} {}", pipeline::agent::build::RAIL_V, statics.version).as_bytes());
    let (raw, raw_counts) = (cache.join(format!("pairs-{tag}.bin")), cache.join(format!("pairs-{tag}.counts")));
    let raw_routes = cache.join(format!("pairs-{tag}.routes"));
    if !raw.exists() || !raw_counts.exists() || !raw_routes.exists() {
        let (list, pairs, used) =
            (scratch.join("feeds.json"), scratch.join("pairs-raw.bin"), scratch.join("used.json"));
        let (static_list, counts, routes) =
            (scratch.join("static.json"), scratch.join("counts-raw.bin"), scratch.join("routes.json"));
        let p = phase("the feeds and static files listed", Class::NasRead);
        std::fs::write(&list, serde_json::to_vec(&rail::feeds_for_counting(out, rail::FEEDS)?)?)?;
        std::fs::write(&static_list, serde_json::to_vec(&pipeline::inputs::gtfs::zips_json(&statics))?)?;
        drop(p);
        let p = phase("railgtfs.py", Class::Mixed);
        let mut c = store::spawn::command("uv");
        c.current_dir(&dem)
            .args(["run", "python", "railgtfs.py", "--feeds"])
            .arg(&list)
            .arg("--out")
            .arg(&pairs)
            .arg("--used")
            .arg(&used)
            .arg("--static")
            .arg(&static_list)
            .arg("--counts")
            .arg(&counts)
            .arg("--routes")
            .arg(&routes);
        pipeline::timings::child(&mut c);
        let st = store::spawn::status(&mut c).context("run railgtfs.py")?;
        anyhow::ensure!(st.success(), "railgtfs.py failed: {st}");
        drop(p);
        let p = phase("the pairs kept and the feeds' use uploaded", Class::NasWrite);
        // Only the current list's pairs are kept.
        std::fs::create_dir_all(&cache)?;
        for e in std::fs::read_dir(&cache)?.flatten() {
            if e.file_name().to_string_lossy().starts_with("pairs-") {
                std::fs::remove_file(e.path()).ok();
            }
        }
        // (Where it can't be moved, copied by a temporary name: a copy cut short is never taken for
        // the pairs. The count rows first: the trains' pairs there mean both are.)
        for (from, to) in [(&routes, &raw_routes), (&counts, &raw_counts), (&pairs, &raw)] {
            if std::fs::rename(from, to).is_err() {
                let tmp = to.with_extension("tmp");
                store::sys::copy_data(from, &tmp)?;
                std::fs::rename(&tmp, to)?;
                std::fs::remove_file(from).ok();
            }
        }
        std::fs::remove_file(&static_list).ok();
        out.put_file("work/rail/used", "json", &used)?;
        drop(p);
        out.save()?;
    }
    // 2. Stops beyond the coverage.
    parts.start(1);
    let p = phase("the stops beyond the coverage marked", Class::Compute);
    let (pairs, beyond) = rail::mark_beyond(&std::fs::read(&raw)?, &cov);
    let pairs_file = scratch.join("pairs.bin");
    std::fs::write(&pairs_file, &pairs)?;
    // (The count rows apart, after the trains: railfreq reads its inputs in their order.)
    let (counted, _) = rail::mark_beyond(&std::fs::read(&raw_counts)?, &cov);
    let counts_file = scratch.join("pairs-counts.bin");
    std::fs::write(&counts_file, &counted)?;
    let inputs = vec![pairs_file.clone(), counts_file];
    eprintln!("rail: {} stop pairs from the timetables' trains, {beyond} with a stop beyond the coverage, and {} from count rows ({:.0?})", pairs.len() / rail::PAIR, counted.len() / rail::PAIR, t0.elapsed());
    drop(p);
    // 3. The rail ways touching the coverage.
    parts.start(2);
    let local = scratch.join("rail-set.osm.pbf");
    let p = phase("the rail set copied here", Class::NasRead);
    p.count(store::sys::copy_data(&set, &local).with_context(|| format!("copy {}", set.display()))?, 1);
    drop(p);
    let p = phase("the rail set clipped to the cover (osmium)", Class::Compute);
    let poly = scratch.join("cover.geojson");
    std::fs::write(
        &poly,
        serde_json::to_vec(&pipeline::heritage::tiles_geojson(
            pipeline::heritage::COVER_Z,
            &pipeline::heritage::cover_tiles(&cov),
        ))?,
    )?;
    let clip = scratch.join("rail-cover.osm.pbf");
    let mut c = pipeline::osmpass::osmium();
    c.args(["extract", "--strategy", "complete_ways", "--overwrite", "-p"]).arg(&poly).arg(&local).arg("-o").arg(&clip);
    osmium_run(c, "osmium extract (the rail set over the coverage)")?;
    std::fs::remove_file(&local).ok();
    // (Its service relations, for the timetables' to-do list: pipeline::timetables.)
    let rels_pbf = scratch.join("rail-rels.osm.pbf");
    let mut c = pipeline::osmpass::osmium();
    c.args(["tags-filter", "--overwrite", "-R"])
        .arg(&clip)
        .arg("r/route=train,subway,tram,light_rail,monorail,funicular")
        .arg("-o")
        .arg(&rels_pbf);
    osmium_run(c, "osmium tags-filter (the rail set's service relations)")?;
    let rels_opl = scratch.join("rail-rels.opl");
    let mut c = pipeline::osmpass::osmium();
    c.args(["cat", "--overwrite", "-t", "relation", "-f", "opl"]).arg(&rels_pbf).arg("-o").arg(&rels_opl);
    osmium_run(c, "osmium cat (the rail set's service relations)")?;
    std::fs::remove_file(&rels_pbf).ok();
    drop(p);
    parts.start(3);
    let p = phase("its ways extracted and those in the coverage kept (extract)", Class::Compute);
    let dir = scratch.join("ways");
    std::fs::remove_dir_all(&dir).ok();
    let st = store::spawn::status(store::spawn::command(bin.join("extract")).arg(&dir).arg("8").arg(&clip))
        .context("run extract")?;
    anyhow::ensure!(st.success(), "extract failed: {st}");
    std::fs::remove_file(&clip).ok();
    let (kept, _) = pipeline::unit::subset(&dir, |_, vs| cov.touches(vs))?;
    eprintln!("rail: {kept} ways touch the coverage ({:.0?})", t0.elapsed());
    drop(p);
    // 4. The trains on them.
    parts.start(4);
    let p = phase("the trains matched onto the tracks (railfreq)", Class::Compute);
    let st = store::spawn::status(store::spawn::command(bin.join("railfreq")).arg(&dir).args(&inputs))
        .context("run railfreq")?;
    anyhow::ensure!(st.success(), "railfreq failed: {st}");
    let ways = roadcore::Ways::open(&dir)?;
    let freq = rail::by_way_id(ways.ways(), &std::fs::read(dir.join("rail-freq.bin"))?);
    drop(p);
    // The lines and the routes matching nothing, for the timetables' to-do list and report.
    let p = phase("the lines served and the routes matching nothing found", Class::Compute);
    let verts = ways.verts();
    let mut len: std::collections::HashMap<u64, f64> = std::collections::HashMap::new();
    for w in ways.ways() {
        let v = &verts[w.vstart as usize..(w.vstart + w.vcount as u64) as usize];
        let e7 = roadcore::E7;
        *len.entry(w.id as u64).or_default() += v
            .windows(2)
            .map(|q| {
                roadcore::dist_m(q[0][0] as f64 * e7, q[0][1] as f64 * e7, q[1][0] as f64 * e7, q[1][1] as f64 * e7)
            })
            .sum::<f64>();
    }
    let served: std::collections::HashSet<u64> =
        freq.chunks_exact(8).map(|c| u32::from_le_bytes(c[..4].try_into().unwrap()) as u64).collect();
    let rels = pipeline::timetables::read_rels(std::io::BufReader::new(std::fs::File::open(&rels_opl)?))?;
    std::fs::remove_file(&rels_opl).ok();
    let lines = pipeline::timetables::rail_lines(&rels, &len, &served);
    let mut kept = pipeline::timetables::pair_keys(&pairs);
    kept.extend(pipeline::timetables::pair_keys(&counted));
    let missed = pipeline::timetables::miss_keys(&std::fs::read(dir.join("rail-miss.bin"))?);
    let routes: serde_json::Value = serde_json::from_slice(&std::fs::read(&raw_routes)?)?;
    let rel_ids = rels.iter().map(|r| r.id).collect();
    let way_ids = len.keys().copied().collect();
    let unmatched = pipeline::timetables::rail_unmatched(&routes, &kept, &missed, &rel_ids, &way_ids);
    eprintln!(
        "rail: {} lines in the coverage, {} timetable routes match nothing on the map",
        lines.len(),
        unmatched.len()
    );
    drop(p);
    let p = phase("uploaded", Class::NasWrite);
    out.put_bytes(rail::RAILFREQ, "bin", &freq)?;
    out.put_bytes(
        pipeline::timetables::RAIL_LINES,
        "json",
        &serde_json::to_vec(&serde_json::json!({"lines": lines, "unmatched": unmatched}))?,
    )?;
    drop(p);
    out.save()?;
    drop(ways);
    std::fs::remove_dir_all(&dir).ok();
    for f in inputs.into_iter().chain([poly, scratch.join("feeds.json")]) {
        std::fs::remove_file(f).ok();
    }
    eprintln!("rail: {} rail ways with trains a day ({:.0?})", freq.len() / 8, t0.elapsed());
    Ok(())
}

/// The labels by importance, worldwide (dem/labels.py on the pass's labels set), split into the
/// labels layer's packs.
fn labels_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass")?,
    };
    let set = out
        .get(&pipeline::osmpass::set_name(&date, "labels"))
        .context("the pass has no labels set (passes before 2026-10-02)")?
        .to_string();
    let work = scratch.join("labels");
    std::fs::create_dir_all(&work)?;
    let local = work.join("labels-set.osm.pbf");
    let parts = Parts(&["Copying the labels set", "Ranking the labels (labels.py)", "Cutting them into packs"]);
    parts.start(0);
    let p = phase("the labels set copied here", Class::NasRead);
    p.count(store::sys::copy_data(out.path(&set), &local).with_context(|| format!("copy {set}"))?, 1);
    drop(p);
    parts.start(1);
    let tiles = work.join("labels.tiles");
    let dem = PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into()));
    let p = phase("labels.py", Class::Mixed);
    let mut c = store::spawn::command("uv");
    c.current_dir(&dem)
        .args(["run", "python", "labels.py", "--src"])
        .arg(&local)
        .arg("--out")
        .arg(&tiles)
        .arg("--work")
        .arg(&work);
    pipeline::timings::child(&mut c);
    let st = store::spawn::status(&mut c).context("run labels.py")?;
    anyhow::ensure!(st.success(), "labels.py failed: {st}");
    drop(p);
    parts.start(2);
    let p = phase("cut into packs and uploaded", Class::NasWrite);
    let arc = roadcore::archive::Archive::open(&tiles)?;
    let lo = layers::split_archive(out, &arc, "labels", "mvt", true, 14)?;
    drop(p);
    eprintln!("labels: root {:?}, {} lo, {} hi packs", lo.root.is_some(), lo.lo.len(), lo.hi.len());
    // Packs of an earlier labels layer that this one doesn't have go from the manifest.
    let keep: std::collections::BTreeSet<String> =
        lo.root.iter().chain(lo.lo.values()).chain(lo.hi.values()).cloned().collect();
    let gone: Vec<String> = out
        .manifest
        .keys()
        .filter(|k| k.starts_with("layers/labels/") && !keep.contains(*k) && out.may_remove(k))
        .cloned()
        .collect();
    for k in gone {
        out.remove(&k);
    }
    out.save()?;
    std::fs::remove_dir_all(&work).ok();
    Ok(())
}

/// A step's work folder, removed when the step ends, however it ends: what's left in it is worth
/// nothing to a later run, and it can be large.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new(p: PathBuf) -> Result<WorkDir> {
        std::fs::create_dir_all(&p)?;
        Ok(WorkDir(p))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// The lakes filled from Canada's own data (pipeline::lakefill::run::all): the pass's basemap
/// surveyed for the NTS sheets whose small lakes OSM lacks, those filled from the declared lake-fill
/// source's zips (`sources/fetched/<id>/`, read in place) band by band in this Mac's scratch, the
/// fill put in the build as `work/lakefill/<date>` (one file), the other passes' removed. `--pass <date>`.
fn lakefill_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let t = std::time::Instant::now();
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass")?,
    };
    let basemap = out.path(out.get(&format!("layers/basemap/world-{date}")).context("the pass has no basemap")?);
    let outlines = out.path(out.get(&format!("sources/osm/{date}/outlines")).context("the pass's outlines")?);
    let decl = pipeline::inputs::sources::read(out.root(), &out.manifest)?;
    let decl = decl.role(pipeline::inputs::sources::LAKE_FILL)?;
    let source = out.root().join(pipeline::lakefill::run::source_dir(decl));
    let work = WorkDir::new(scratch.join("lakefill"))?;
    let mut names = vec!["Finding the map sheets OpenStreetMap's small lakes are missing from".to_string()];
    names.extend(
        pipeline::lakefill::run::BANDS.iter().map(|(w, e)| format!("Filling the sheets from {}° to {}° W", -w, -e)),
    );
    names.push("Putting the bands together".into());
    names.push("Uploading".into());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let p = phase("the lakes filled from CanVec's zips", Class::NasRead);
    let f = pipeline::lakefill::run::all(
        &basemap,
        &outlines,
        &source,
        decl,
        &work.0,
        &|k| pipeline::agent::jobs::part(k, &names),
        &|d, n| pipeline::agent::jobs::report(d, n, "z5 tiles surveyed"),
    )?;
    drop(p);
    pipeline::agent::jobs::part(names.len() - 1, &names);
    let p = phase("uploaded", Class::NasWrite);
    let name = out.put_file_with(&pipeline::lakefill::logical(&date), "sect", &f, &|d, n| {
        pipeline::agent::jobs::report(d, n, "bytes uploaded")
    })?;
    drop(p);
    let older: Vec<String> = out
        .manifest
        .keys()
        .filter(|l| l.starts_with(pipeline::lakefill::PREFIX) && **l != pipeline::lakefill::logical(&date))
        .cloned()
        .collect();
    for l in older {
        out.remove(&l);
    }
    out.save()?;
    eprintln!("lakefill: {name} in {:.0} s", t.elapsed().as_secs_f64());
    Ok(())
}

/// The water layer (pipeline::water): each pixel's share of sea and of inland water, z0–9, drawn
/// from the pass's basemap's z14 water, as the water layer's packs. `--basemap f`: a local archive
/// instead of the manifest's; `--only x/y,…`: those z5 tiles alone; `--archive f`: the tiles into
/// that local archive, nothing written to the NAS (a regional run, measured); `--lakefill f`: the
/// lakes filled from Canada's own data (pipeline::lakefill) from that file instead of the
/// manifest's (`work/lakefill/<date>`, drawn when the manifest names one).
fn water_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::water as wt;
    let t = std::time::Instant::now();
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass")?,
    };
    let basemap = match opt(args, "--basemap") {
        Some(f) => PathBuf::from(f),
        None => out.path(out.get(&format!("layers/basemap/world-{date}")).context("the pass has no basemap")?),
    };
    let only: Option<Vec<(u32, u32)>> = opt(args, "--only").map(|o| {
        o.split(',')
            .filter_map(|t| {
                let (x, y) = t.split_once('/')?;
                Some((x.parse().ok()?, y.parse().ok()?))
            })
            .collect()
    });
    // (A part of the world isn't a layer: its packs would stand in for the whole one's, the rest of
    // the world and z0–4 gone.)
    anyhow::ensure!(
        only.is_none() || opt(args, "--archive").is_some(),
        "water: --only makes part of the layer: give --archive too (nothing written to the NAS)"
    );
    let parts = Parts(&["Reading the basemap's z14 directory", "Drawing the water", "Cutting it into packs"]);
    parts.start(0);
    let p = phase("the basemap's z14 directory read", Class::NasRead);
    let pm = store::pmtiles::PmTiles::open(Box::new(
        store::range::PlainFile::open(&basemap).with_context(|| format!("open {}", basemap.display()))?,
    ))?;
    let z14 = wt::Z14::read(&pm)?;
    eprintln!("water: the z14 directory read in {:.0} s", t.elapsed().as_secs_f64());
    drop(p);
    parts.start(1);
    // (Its z14 tiles read from the basemap on the NAS as it draws, in parallel: one phase.)
    let p = phase("the water drawn from the basemap's z14 tiles", Class::NasRead);
    let fill = match opt(args, "--lakefill") {
        Some(f) => Some(pipeline::lakefill::Fill::open(Path::new(&f), &f)?),
        None => match out.get(&pipeline::lakefill::logical(&date)) {
            Some(c) => Some(pipeline::lakefill::Fill::open(&out.path(c), c)?),
            None => None,
        },
    };
    let (tiles, made) = wt::build(&pm, &z14, fill.as_ref().map(|f| f as &dyn wt::Extra), only.as_deref(), &|d, n| {
        pipeline::agent::jobs::report(d, n, "z5 tiles drawn")
    })?;
    drop(z14);
    drop(p);
    eprintln!("water: {} in {:.0} s", serde_json::to_string(&made)?, t.elapsed().as_secs_f64());
    // (The archive in a folder of its own, removed however the step ends.)
    let work = WorkDir::new(scratch.join("water"))?;
    let local = match opt(args, "--archive") {
        Some(f) => PathBuf::from(f),
        None => work.0.join("water.tiles"),
    };
    let p = phase("the tiles' archive written", Class::Disk);
    let mut w = roadcore::archive::ArchiveWriter::create(&local, "{}")?;
    for (z, x, y, png) in &tiles {
        w.add(*z, *x, *y, png, png.len())?;
    }
    w.finish()?;
    drop(tiles);
    drop(p);
    if opt(args, "--archive").is_some() {
        return Ok(());
    }
    parts.start(2);
    let p = phase("cut into packs and uploaded", Class::NasWrite);
    let arc = roadcore::archive::Archive::open(&local)?;
    let lo = layers::split_archive(out, &arc, wt::LAYER, "water-png", false, wt::STORED_MAXZ)?;
    drop(p);
    eprintln!("water: root {:?}, {} lo, {} hi packs", lo.root.is_some(), lo.lo.len(), lo.hi.len());
    // Packs of an earlier water layer that this one doesn't have go from the manifest, and the small
    // islands and lakes' layer it replaces (`layers/smallwater/`, the dots of before).
    let keep: BTreeSet<String> = lo.root.iter().chain(lo.lo.values()).chain(lo.hi.values()).cloned().collect();
    let prefix = format!("layers/{}/", wt::LAYER);
    let gone: Vec<String> = out
        .manifest
        .keys()
        .filter(|k| {
            ((k.starts_with(&prefix) && !keep.contains(*k)) || k.starts_with("layers/smallwater/")) && out.may_remove(k)
        })
        .cloned()
        .collect();
    for k in gone {
        out.remove(&k);
    }
    out.save()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// A pack job opens its tiles' owners and no other unit (`owners_of`, as their keys read
    /// them): from the mirror where it has them (held, nothing copied), else copied into the cache
    /// from the NAS; another unit's file is neither copied nor opened.
    #[test]
    fn a_pack_job_opens_only_its_tiles_owners() {
        use pipeline::legacy::Unit;
        let d = tempfile::tempdir().unwrap();
        let (root, mirror, cache) = (d.path().join("nas"), d.path().join("mirror"), d.path().join("cache"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let units: Vec<Unit> =
            ["6-32-21", "6-33-21", "6-40-30", "6-31-22"].iter().map(|s| Unit::parse(s).unwrap()).collect();
        let mut m = std::collections::BTreeMap::new();
        let mut names = std::collections::BTreeMap::new();
        for (i, u) in units.iter().enumerate() {
            let mut tb = pipeline::hipack::tile_bounds(u.z, u.x, u.y);
            // (6-40-30's pack built by other reaches: its ways go west into 6/32/21, far past where
            // these put them.)
            if u.dash() == "6-40-30" {
                let t = pipeline::hipack::tile_bounds(6, 32, 21);
                (tb[0], tb[3]) = (t[0] + 1, t[3]);
            }
            for (logical, sect, rec) in [
                (format!("base/{}", u.dash()), "ways", std::mem::size_of::<roadcore::WayRec>()),
                (format!("global/roads/{}", u.dash()), "roads", std::mem::size_of::<roadcore::packs::RoadRec>()),
            ] {
                let tmp = d.path().join("tmp.sect");
                let mut w = store::sect::SectWriter::create(&tmp, serde_json::json!({"unit": u.slash(), "extent": tb}))
                    .unwrap();
                w.add(sect, &vec![0u8; rec]).unwrap();
                w.finish().unwrap();
                let name = store::naming::content_name(&logical, &store::naming::hash16_file(&tmp).unwrap(), "sect");
                // (The first unit's files in the mirror, the rest only on the NAS.)
                let dirs: &[&std::path::Path] = if i == 0 { &[&root, &mirror] } else { &[&root] };
                for dir in dirs {
                    std::fs::create_dir_all(dir.join(&name).parent().unwrap()).unwrap();
                    std::fs::copy(&tmp, dir.join(&name)).unwrap();
                }
                m.insert(logical.clone(), name.clone());
                names.insert(name, *u);
            }
        }
        std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        let out = pipeline::out::Out::unbound(&root, &d.path().join("scratch")).unwrap();
        let tile = [(6u8, 32u32, 21u32)];
        // (No reaches here: each unit's ways taken to lie within its tile + 20 km.)
        let mut reach = pipeline::reach::Reaches::default();
        let owners = pipeline::agent::build::owners_of(Some(&reach), &m, &tile);
        assert_eq!(
            owners.iter().map(|u| u.dash()).collect::<Vec<_>>(),
            ["6-31-22", "6-32-21", "6-33-21"],
            "its neighbours' ways reach into it, a far unit's don't (its extent not kept yet)"
        );
        let packs = super::open_units(&out, &cache, Some(&mirror), Some(&owners)).unwrap();
        let opened: std::collections::BTreeSet<Unit> = packs.iter().map(|bp| bp.unit).collect();
        assert_eq!(opened, owners);
        // Copied into the cache: the owners' files the mirror doesn't have, nothing else.
        let copied: Vec<&String> = names.keys().filter(|n| cache.join(n).exists()).collect();
        let want: Vec<&String> =
            names.iter().filter(|(_, u)| owners.contains(u) && u.dash() != "6-32-21").map(|(n, _)| n).collect();
        assert_eq!(copied, want);
        // The mirror's files it opened are held: the mirror's delete leaves them.
        let held = mirror.join(&m["base/6-32-21"]);
        assert_eq!(store::cachefile::try_remove(&held), store::cachefile::Removed::InUse);
        // The packs' extents (crate::extents), read by a job from their headers when the lead
        // hasn't kept them, in memory only; once the lead has, read from its file: the far unit
        // whose pack spills into the tile is caught, and is an owner.
        let (e, read) = pipeline::extents::fill(&root, &m, Default::default()).unwrap();
        assert_eq!(read, units.len());
        assert!(!root.join(pipeline::extents::FILE).exists(), "a job writes nothing");
        assert_eq!(pipeline::extents::keep(&root, &m).unwrap(), units.len());
        assert_eq!(pipeline::extents::load(&root), e);
        assert_eq!(
            pipeline::extents::fill(&root, &m, pipeline::extents::load(&root)).unwrap(),
            (e.clone(), 0),
            "kept: no header read again"
        );
        reach.extents = e;
        assert_eq!(super::outside_their_ways(&m, &reach).iter().map(|u| u.dash()).collect::<Vec<_>>(), ["6-40-30"]);
        let owners = pipeline::agent::build::owners_of(Some(&reach), &m, &tile);
        assert_eq!(owners.iter().map(|u| u.dash()).collect::<Vec<_>>(), ["6-31-22", "6-32-21", "6-33-21", "6-40-30"]);
        assert_eq!(super::open_units(&out, &cache, Some(&mirror), Some(&owners)).unwrap().len(), 4);
        // A job whose manifest lists fewer packs: its own view drops one, the lead's file keeps every
        // entry; the lead's own manifest listing fewer drops it from the file.
        let mut fewer = m.clone();
        fewer.remove("base/6-33-21");
        assert_eq!(pipeline::extents::fill(&root, &fewer, pipeline::extents::load(&root)).unwrap().0.len(), 3);
        assert_eq!(pipeline::extents::load(&root).len(), 4);
        pipeline::extents::keep(&root, &fewer).unwrap();
        assert_eq!(pipeline::extents::load(&root).len(), 3);
        // No tiles named: every unit.
        assert_eq!(super::open_units(&out, &cache, Some(&mirror), None).unwrap().len(), units.len());
        for name in names.keys() {
            store::cachefile::release(&mirror.join(name));
            store::cachefile::release(&cache.join(name));
        }
    }

    /// A target's done line carries its inputs' measures (task #235): its tile's area, the store's
    /// reads and what the step noted since its start, the threads, and as Cost reads them back.
    #[test]
    fn done_line_carries_sizes() {
        let t = super::cost_start("terrain", "6/33/22");
        store::range::count_read(4096);
        pipeline::costin::set("ways", 12);
        let l = super::done_line("terrain", "6/33/22", t);
        assert_eq!(l["unit"], "terrain 6/33/22");
        let i = &l["sizes"];
        assert_eq!(i["km2"], pipeline::costin::tile_km2(6, 33, 22));
        assert!(i["bytes"].as_u64().unwrap() >= 4096);
        assert_eq!(i["ways"], 12);
        assert!(i["threads"].as_u64().unwrap() >= 1);
        assert!(i["tiffs"].is_u64());
        let c: pipeline::coord::Cost = serde_json::from_value(l).unwrap();
        assert_eq!(c.sizes["ways"], 12);
        // (The next target starts again.)
        let t = super::cost_start("pois", "8/1/2");
        assert!(super::done_line("pois", "8/1/2", t)["sizes"].get("ways").is_none());
    }

    /// The tests' catalogs' moment: fixed, so two runs make the same catalog whenever they run.
    const NOW: u64 = 1_790_000_000;

    /// The catalog names the pass's lake fill in `global` and lists it among its files (so the
    /// server draws it and the mirror copies it); a fill of an older pass's basemap neither.
    #[test]
    fn the_catalog_names_and_lists_the_passs_lake_fill() {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "layers/basemap/world-2026-09-28".to_string(),
            "layers/basemap/world-2026-09-28.0123456789abcdef.pmtiles".to_string(),
        );
        m.insert(
            "sources/osm/2026-09-28/outlines".to_string(),
            "sources/osm/2026-09-28/outlines.0123456789abcdef.sect".to_string(),
        );
        m.insert("work/lakefill/2026-09-28".to_string(), "work/lakefill/2026-09-28.0123456789abcdef.sect".to_string());
        m.insert("work/lakefill/2026-03-01".to_string(), "work/lakefill/2026-03-01.0123456789abcdef.sect".to_string());
        let g = super::worldwide_globals(&m);
        assert_eq!(g.get("lakefill").map(String::as_str), Some("work/lakefill/2026-09-28"));
        assert_eq!(g.get("outlines").map(String::as_str), Some("sources/osm/2026-09-28/outlines"));
        let files: Vec<&String> = m.keys().filter(|l| super::served_file(l, &[], &g)).collect();
        assert!(files.iter().any(|l| *l == "work/lakefill/2026-09-28"));
        assert!(!files.iter().any(|l| *l == "work/lakefill/2026-03-01"));
        // A newer pass without its fill yet: none named.
        m.insert(
            "layers/basemap/world-2027-03-01".to_string(),
            "layers/basemap/world-2027-03-01.0123456789abcdef.pmtiles".to_string(),
        );
        assert_eq!(super::worldwide_globals(&m).get("lakefill"), None);
    }

    /// A pack or lo job's base packs another such job uses (it holds them: store::cachefile) aren't
    /// pruned from under it: once, prune_cache deleted what the other read next.
    #[test]
    fn a_base_pack_another_job_uses_is_never_pruned() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("base");
        let (used, idle) =
            (cache.join("base/6-1-1.0000000000000001.base"), cache.join("base/6-1-2.0000000000000002.base"));
        for p in [&used, &idle] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"pack").unwrap();
        }
        // The other job's hold (another open file: as another process's, to flock).
        store::cachefile::hold_existing(&used).unwrap();
        super::prune_cache(&cache, &std::collections::BTreeSet::new()).unwrap();
        assert!(used.exists(), "pruned from under the job using it");
        assert!(!idle.exists());
        store::cachefile::release(&used);
        super::prune_cache(&cache, &std::collections::BTreeSet::new()).unwrap();
        assert!(!used.exists());
    }
    /// A unit's files kept where this Mac's map tiles read them (`open_units`: the cache by content
    /// name), only on a Mac that has that cache.
    #[test]
    fn a_units_files_are_kept_for_the_packs_where_they_are_made() {
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("local.sect");
        std::fs::write(&local, b"a base pack").unwrap();
        let name = store::naming::content_name("base/6-1-2", &store::naming::hash16_file(&local).unwrap(), "sect");
        // No packs' cache here (a Mac that doesn't make map tiles): nothing.
        let none = d.path().join("none");
        super::keep_for_packs(&none, "base/6-1-2", &local);
        assert!(!none.exists());
        let cache = d.path().join("base");
        std::fs::create_dir_all(&cache).unwrap();
        super::keep_for_packs(&cache, "base/6-1-2", &local);
        assert_eq!(std::fs::read(cache.join(&name)).unwrap(), b"a base pack");
        // Kept again (a unit made again the same): as it was.
        super::keep_for_packs(&cache, "base/6-1-2", &local);
        assert_eq!(std::fs::read(cache.join(&name)).unwrap(), b"a base pack");
        // Not held: the packs' prune takes it once the manifest no longer names it.
        super::prune_cache(&cache, &std::collections::BTreeSet::new()).unwrap();
        assert!(!cache.join(&name).exists());
    }

    /// Delta catalogs (`--delta`): written as what changed over the last, read back whole as the
    /// catalog written whole would be (its time and app aside), with the sizes of the files the last
    /// one lists taken from it (one gone from the NAS since isn't looked at; a whole one's check
    /// would stop on it).
    #[test]
    fn a_delta_catalog_is_the_whole_one_and_takes_the_last_ones_sizes() {
        let d = tempfile::tempdir().unwrap();
        let roots = [d.path().join("delta"), d.path().join("whole")];
        let put = |root: &std::path::Path, logical: &str, bytes: &[u8]| -> String {
            let name =
                store::naming::content_name(logical, &format!("{:016x}", bytes.len() * 7 + logical.len()), "pack");
            std::fs::create_dir_all(root.join(&name).parent().unwrap()).unwrap();
            std::fs::write(root.join(&name), bytes).unwrap();
            let p = root.join("state/build/manifest.json");
            let mut m: std::collections::BTreeMap<String, String> = pipeline::out::read_record(&p).unwrap();
            m.insert(logical.into(), name.clone());
            std::fs::write(&p, serde_json::to_vec(&m).unwrap()).unwrap();
            name
        };
        let run = |root: &std::path::Path, way: super::CatalogWay| {
            let mut out = pipeline::out::Out::unbound(root, &root.join("scratch")).unwrap();
            super::catalog(&mut out, false, None, way, NOW).unwrap();
        };
        let delta = super::CatalogWay { delta: true, whole: false };
        let mut gone = String::new();
        for root in &roots {
            std::fs::create_dir_all(root.join("state/build")).unwrap();
            std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
            // (The regions' accepted version, none in it, as the readers read it.)
            let mut m = std::collections::BTreeMap::new();
            pipeline::inputs::regions::accept_fixture(root, root, &[], &mut m);
            std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
            put(root, "layers/roads/hi/6-1-2", b"one");
            gone = put(root, "layers/roads/lo/3-0-0", b"lo pack");
            // (Enough files that a change is small beside the whole.)
            for i in 0..40 {
                put(root, &format!("layers/roads/hi/6-2-{i}"), format!("tile {i}").as_bytes());
            }
            run(root, super::CatalogWay::default());
        }
        // A tile drawn again and one added; the lo pack gone from the NAS (its size from the last).
        for root in &roots {
            put(root, "layers/roads/hi/6-1-2", b"one again");
            put(root, "layers/roads/hi/6-1-3", b"three");
        }
        std::fs::remove_file(roots[0].join(&gone)).unwrap();
        run(&roots[0], delta);
        run(&roots[1], super::CatalogWay::default());
        assert!(roots[0].join("catalog/2.delta.json.zst").exists() && !roots[0].join("catalog/2.json.zst").exists());
        let strip = |mut c: store::catalog::Catalog| {
            c.created.clear();
            c
        };
        let (a, b) = (
            store::catalog::latest(&roots[0].join("catalog")).unwrap().unwrap(),
            store::catalog::latest(&roots[1].join("catalog")).unwrap().unwrap(),
        );
        assert_eq!(a.n, 2);
        assert_eq!(strip(a), strip(b));
        // Asked for whole: whole, whatever.
        put(&roots[0], "layers/roads/hi/6-1-4", b"four");
        std::fs::write(roots[0].join(&gone), b"lo pack").unwrap();
        run(&roots[0], super::CatalogWay { delta: true, whole: true });
        assert!(roots[0].join("catalog/3.json.zst").exists());
        // A whole one checks every file: one the last catalog lists, gone from the NAS, stops it.
        std::fs::remove_file(roots[0].join(&gone)).unwrap();
        put(&roots[0], "layers/roads/hi/6-1-5", b"five");
        for way in [super::CatalogWay { delta: true, whole: true }, super::CatalogWay::default()] {
            let mut out = pipeline::out::Out::unbound(&roots[0], &roots[0].join("scratch")).unwrap();
            let e = super::catalog(&mut out, false, None, way, NOW).unwrap_err();
            assert!(format!("{e:#}").contains("missing on the NAS"), "{e:#}");
        }
        // So much new that the delta is as big as the whole one: a delta still (the whole one comes
        // in idle time), the files whose sizes came from the last not looked at.
        for i in 0..3000 {
            put(&roots[0], &format!("layers/roads/hi/6-9-{i}"), format!("new {i}").as_bytes());
        }
        run(&roots[0], delta);
        assert!(roots[0].join("catalog/4.delta.json.zst").exists());
        assert!(store::catalog::whole_due(&roots[0].join("catalog")).unwrap());
    }

    /// A delta catalog's sizes are the records' (from the files' writers, Out) and the last
    /// catalog's: it looks at the NAS only for a file written before sizes were recorded, once,
    /// and lists the same sizes as a whole catalog of the same manifest, which looks at every file.
    #[test]
    fn a_deltas_sizes_are_the_records_and_equal_a_wholes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
        let mut m = std::collections::BTreeMap::new();
        pipeline::inputs::regions::accept_fixture(root, root, &[], &mut m);
        std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        let write = |files: &[(String, Vec<u8>)]| {
            let mut out = pipeline::out::Out::unbound(root, &root.join("scratch")).unwrap();
            for (l, b) in files {
                out.put_bytes(l, "pack", b).unwrap();
            }
            out.save().unwrap();
        };
        let run = |way| {
            let mut out = pipeline::out::Out::unbound(root, &root.join("scratch")).unwrap();
            super::catalog(&mut out, false, None, way, NOW).unwrap()
        };
        let delta = super::CatalogWay { delta: true, whole: false };
        write(&(0..40).map(|i| (format!("layers/roads/hi/6-2-{i}"), vec![b'x'; 10 + i])).collect::<Vec<_>>());
        let first = run(super::CatalogWay::default());
        assert_eq!((first.delta, first.stated), (false, 40));
        // A file from before the records kept sizes (named in the manifest by hand, none recorded).
        let old = store::naming::content_name("layers/roads/hi/6-1-1", "00000000000000aa", "pack");
        std::fs::write(root.join(&old), b"written long ago").unwrap();
        let p = root.join("state/build/manifest.json");
        let mut m: std::collections::BTreeMap<String, String> = pipeline::out::read_record(&p).unwrap();
        m.insert("layers/roads/hi/6-1-1".into(), old.clone());
        std::fs::write(&p, serde_json::to_vec(&m).unwrap()).unwrap();
        write(&[
            ("layers/roads/hi/6-1-2".into(), b"new".to_vec()),
            ("layers/roads/hi/6-2-3".into(), b"redrawn".to_vec()),
        ]);
        // The old file alone looked at; the next delta looks at none, even with every file gone.
        let second = run(delta);
        assert_eq!((second.delta, second.stated, second.files), (true, 1, 42));
        write(&[("layers/roads/hi/6-1-3".into(), b"three".to_vec())]);
        let names: Vec<String> = pipeline::out::read_record::<std::collections::BTreeMap<String, String>>(&p)
            .unwrap()
            .into_values()
            .filter(|c| c.starts_with("layers/"))
            .collect();
        let kept: Vec<(String, Vec<u8>)> =
            names.iter().map(|c| (c.clone(), std::fs::read(root.join(c)).unwrap())).collect();
        for (c, _) in &kept {
            std::fs::remove_file(root.join(c)).unwrap();
        }
        let third = run(delta);
        assert_eq!((third.delta, third.stated, third.files), (true, 0, 43));
        let as_delta = store::catalog::latest(&root.join("catalog")).unwrap().unwrap();
        // A whole one of the same manifest: every file looked at, the same sizes.
        for (c, b) in &kept {
            std::fs::write(root.join(c), b).unwrap();
        }
        let whole = run(super::CatalogWay { delta: true, whole: true });
        assert_eq!((whole.delta, whole.stated), (false, 43));
        let as_whole = store::catalog::latest(&root.join("catalog")).unwrap().unwrap();
        assert_eq!(as_delta.files, as_whole.files);
        // A recorded size the NAS disagrees with stops a whole one.
        std::fs::write(root.join(&kept[0].0), b"another size altogether").unwrap();
        let mut out = pipeline::out::Out::unbound(root, &root.join("scratch")).unwrap();
        let e = super::catalog(&mut out, false, None, super::CatalogWay::default(), NOW).unwrap_err();
        assert!(format!("{e:#}").contains("as recorded"), "{e:#}");
    }

    /// What a catalog says of itself (pipeline::pubtimes::CatalogStats): whole, then a delta that
    /// stat'ed only the new files, its phases taken once written (the write's phase among them).
    #[test]
    fn a_catalog_says_what_it_did_once_written() {
        pipeline::timings::start("catalog");
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
        let mut m = std::collections::BTreeMap::new();
        pipeline::inputs::regions::accept_fixture(root, root, &[], &mut m);
        let mut put = |logical: &str, bytes: &[u8]| {
            let name =
                store::naming::content_name(logical, &format!("{:016x}", bytes.len() * 7 + logical.len()), "pack");
            std::fs::create_dir_all(root.join(&name).parent().unwrap()).unwrap();
            std::fs::write(root.join(&name), bytes).unwrap();
            m.insert(logical.to_string(), name);
            std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        };
        for i in 0..40 {
            put(&format!("layers/roads/hi/6-2-{i}"), format!("tile {i}").as_bytes());
        }
        let run = |way| {
            let mut out = pipeline::out::Out::unbound(root, &root.join("scratch")).unwrap();
            super::catalog(&mut out, false, None, way, NOW).unwrap()
        };
        let first = run(super::CatalogWay::default());
        assert_eq!((first.n, first.delta), (1, false));
        assert_eq!(first.stated, first.files);
        put("layers/roads/hi/6-1-3", b"three");
        let second = run(super::CatalogWay { delta: true, whole: false });
        assert_eq!((second.n, second.delta, second.stated, second.files), (2, true, 1, first.files + 1));
        let written = second.phases.iter().find(|(p, _)| p == "the catalog written").expect("the write's phase");
        assert!(written.1 > 0.0, "{:?}", second.phases);
        let line = second.line();
        assert_eq!(pipeline::pubtimes::CatalogStats::from_log(&line), Some(second));
    }

    #[test]
    fn catalog_times_both_ways() {
        for t in ["2026-10-05T03:55:11Z", "2024-02-29T23:59:59Z", "1970-01-01T00:00:00Z", "2000-03-01T12:00:00Z"] {
            assert_eq!(super::utc(super::epoch_of(t).unwrap()), t);
        }
        assert_eq!(super::epoch_of("1970-01-02T00:00:01Z"), Some(86401));
        assert_eq!(super::epoch_of("2026-10-05 03:55:11"), None);
    }

    #[test]
    fn a_boxs_canopy_files() {
        // Unit 6/20/22 grown by 30 km (New Brunswick): one 10° square, its three files.
        let b = pipeline::stage::tile_box_grown(6, 20, 22, pipeline::stage::MARGIN_KM);
        assert_eq!(
            super::canopy_files(b),
            ["median", "p95", "cover5m"].map(|s| format!("meta_chm_lat=50.0_lon=-70.0_{s}.tif"))
        );
        // A box across 50° N and 0° E: four squares.
        let names = super::canopy_files([-0.5, 49.5, 0.5, 50.5]);
        assert_eq!(names.len(), 12);
        for (top, left) in [(50, -10), (60, -10), (50, 0), (60, 0)] {
            assert!(names.contains(&format!("meta_chm_lat={top}.0_lon={left}.0_p95.tif")), "{top} {left}: {names:?}");
        }
    }

    /// The road → units index from the units' road ids (a unit built before them: its road
    /// values), reading only the lists changed since the index before: the same index as one made
    /// from every list.
    #[test]
    fn the_road_index_reads_only_the_lists_changed() {
        use pipeline::legacy::{RoadRec, Unit};
        let rec = |road: u64| RoadRec { road, len: 1.0, offset: 0.0, dir: 0, _pad: [0; 7] };
        let (a, b, c) = (Unit::parse("6/1/1").unwrap(), Unit::parse("6/1/2").unwrap(), Unit::parse("6/1/3").unwrap());
        // (a built before the road ids: its road values alone; b and c with theirs.)
        let fill = |out: &mut pipeline::out::Out, c_roads: &[RoadRec]| {
            super::put_roads(out, a, &[rec(5), rec(1), rec(5)], None).unwrap();
            for (u, rs) in [(b, &[rec(1), rec(2)][..]), (c, c_roads)] {
                super::put_roads(out, u, rs, None).unwrap();
                super::put_road_ids(out, u, rs).unwrap();
            }
        };
        let pairs = |out: &pipeline::out::Out| -> Vec<u64> {
            let f = store::range::PlainFile::open(&out.path(out.get("global/roadunits").unwrap())).unwrap();
            store::sect::SectReader::open(f).unwrap().read_pod("pairs").unwrap()
        };
        let flat = |v: &[(u64, u64)]| v.iter().flat_map(|&(r, u)| [r, u]).collect::<Vec<_>>();
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mut out = pipeline::out::Out::unbound(root, &root.join("scratch")).unwrap();
        fill(&mut out, &[rec(3)]);
        super::roadunits(&mut out).unwrap();
        let (ka, kb, kc) = (a.key(), b.key(), c.key());
        let mut want = vec![(1, ka), (1, kb), (2, kb), (3, kc), (5, ka)];
        want.sort_unstable();
        assert_eq!(pairs(&out), flat(&want));
        // c's list changed, a's and b's files gone from the NAS: only c's is read.
        for l in ["global/roads/6-1-1", "global/roadids/6-1-2"] {
            std::fs::remove_file(out.path(out.get(l).unwrap())).unwrap();
        }
        super::put_road_ids(&mut out, c, &[rec(4)]).unwrap();
        super::roadunits(&mut out).unwrap();
        let mut want: Vec<(u64, u64)> = want.into_iter().map(|p| if p.1 == kc { (4, kc) } else { p }).collect();
        want.sort_unstable();
        assert_eq!(pairs(&out), flat(&want));
        // Made afresh from every list: the same file.
        let fresh = tempfile::tempdir().unwrap();
        let mut o2 = pipeline::out::Out::unbound(fresh.path(), &fresh.path().join("scratch")).unwrap();
        fill(&mut o2, &[rec(4)]);
        super::roadunits(&mut o2).unwrap();
        assert_eq!(o2.get("global/roadunits"), out.get("global/roadunits"));
    }
}
