//! The build steps of the new pipeline (docs/plan.md §6), writing to the NAS project folder.
//!
//! usage: scenic-build <step> --root <nas project folder> --scratch <local dir> [options]
//!
//!   pack [--cache dir] [T …]     pack(T) for z6 tiles T (default: every tile with ways): road and
//!                                rail hi packs (z9–14) and hidata
//!   lo [--cache dir] [Q …]       lo packs (z4–8 road and rail tiles) for z3 tiles Q (default: all)
//!   osm-pass --planet <p> --date <d>  the OSM pass (pieces, sets, basemap, road values); resumable
//!   patch-ferries [--pass d] [--near-coverage]  the pass's pieces given the standalone ferry ways
//!                                its filtered planet lacked (pipeline::osmpass::patch_ferries), all
//!                                or those meeting the regions (10 km round); run by hand
//!   unit [U …] [--pass d] [--layers-root r] [--regions dir] [--dem dir] [--cache-dir dir] [--buildings dir]
//!                                base(U) from the pass's pieces (the unit's programs on a unit folder):
//!                                default every unit whose piece meets the coverage
//!   unit-snap U --out d --cache c [--carry]  unit U's folder built from the records into d, nothing
//!                                written to the NAS, snapshotted around each step's program (for
//!                                tools/check/same.py and tail.mjs)
//!   tail-spec U                  unit U's tail as a task gives it a worker (tools/check/tail.mjs)
//!   roadunits                    the road → units index from every unit's road values
//!   terrain [T …] [--regions dir] [--pass d] [--raw dir]  terrain packs for z6 tiles T near the
//!                                coverage (default: all of them): hi z9–12, then their z3 lo packs,
//!                                from AWS's raw tiles (cached in --raw)
//!   slope [T …] [--regions dir]  slope packs (z3–11) of z6 tiles T from the terrain packs
//!                                (default: every z6 tile near the coverage)
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
//!                                Overture's release, in z8 tiles, onto the NAS with their index
//!   bld-fetch [--pass d] [--dem dir]  the 3D buildings' sources onto the NAS (dem/bldfetch.py):
//!                                the pinned Overture release's files and GHSL's tiles meeting the
//!                                coverage grown by 20 km; what's there already skipped
//!   bldprep <T …> [--dem dir]    the 3D buildings' normalized files of z6 tiles T (pipeline::bld::prep:
//!                                dem/bldprep.py reads the downloaded Overture row groups and the
//!                                GHSL windows under T), as work/bld/6-x-y
//!   bldtiles <T …> [--pass d] [--regions dir]  the 3D buildings' tiles of z6 tiles T
//!                                (pipeline::bld::job): the buildings touching the coverage, their
//!                                heights filled, z12–14, as the hi pack layers/buildings/hi/6-x-y;
//!                                under the agent, some z8 areas offered to workers as tasks
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
//!   ferries --pass d [--dem dir]  the pass's ferries set through ferries.py (with
//!                                inputs/ferries/freq), as the ferries' blocks
//!   rail-feeds [--pass d] [--dem dir]  the rail feeds for the coverage (dem/railfeeds.py), each
//!                                fetched once into the rail sources
//!   rail [--pass d] [--dem dir] [--cache dir]  trains a day on the coverage's rail ways
//!                                (dem/railgtfs.py, then railfreq on the pass's rail set), as
//!                                global/railfreq
//!   put <logical> <ext> <file>   upload a file under a logical name
//!   verify                       check every unverified upload on the NAS (SHA-256 over SSH)
//!   catalog                      publish a catalog of the build manifest
//!   rekey-check [--pass d]       what re-keying the records for the units' new keys
//!                                (pipeline::agent::rekey) would do now: the units re-keyed, and
//!                                those left to build again, each with why; and the times that show
//!                                its one assumption holds. Reads only: writes nothing, keeps no index
//!   p5-check trees [--pass d] [--costs f]  what switching tree cover to pieces and assemblies
//!                                (pipeline::agent::rekey) would do now: each z3 tile re-keyed, or
//!                                made again as pieces and why, the work left and its time by the
//!                                last runs (`f`: the coordinator's costs.json). Reads only
//!
//! `--cache` is where base packs are kept locally (copied from the NAS when missing).

use anyhow::{bail, Context, Result};
use pipeline::basepack::BasePack;
use pipeline::hipack::{self, b, grow, meets, tile_bounds};
use pipeline::layers::{self, LayerOut};
use pipeline::legacy::{self, Legacy, Unit};
use pipeline::out::Out;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SSH: [&str; 8] = ["ssh", "-o", "ControlMaster=auto", "-o", "ControlPath=~/.ssh/cm-%r@%h:%p", "-o", "ControlPersist=12h", "brandontsang@fishandchips.local"];
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
    let step = args.get(1).cloned().unwrap_or_default();
    // tail-spec U: unit U's tail as a task gives it a worker (its steps, where its places lie, the
    // data servers it may read), for tools/check/tail.mjs; nothing read or written.
    if step == "tail-spec" {
        let u = positional(&args).first().and_then(|s| Unit::parse(s)).context("tail-spec U")?;
        let spec = serde_json::json!({ "runs": pipeline::unit::tail(u, true, true), "places": pipeline::offload::places(), "web_hosts": pipeline::coord::WEB_HOSTS });
        println!("{spec}");
        return Ok(());
    }
    let root = PathBuf::from(opt(&args, "--root").context("--root <nas project folder>")?);
    // (Before the scratch folder: it writes nothing.)
    if step == "rekey-check" {
        return rekey_check(&root, &args);
    }
    if step == "p5-check" {
        return p5_check(&root, &args);
    }
    if step == "names-todo" {
        // names-todo [--out <dir>] [--translations <dir>]: the translations' and descriptions' to-do
        // lists from the newest catalog (pipeline::namestodo), into the root's translations/todo/
        // and descriptions/todo/ (or --out's); it writes nothing else.
        let scratch = PathBuf::from(opt(&args, "--scratch").unwrap_or_else(|| "/tmp/scenic-build".into()));
        let out = opt(&args, "--out").map_or_else(|| root.clone(), PathBuf::from);
        let tr = opt(&args, "--translations").map_or_else(|| root.join("translations"), PathBuf::from);
        let t0 = std::time::Instant::now();
        let r = pipeline::namestodo::run(&root, &out, &tr, &scratch, &|what, done, total| pipeline::agent::jobs::report(done, total, what))?;
        eprintln!("names-todo: {} in {:.0} s", serde_json::json!({"langs": r.langs, "read": r.read, "own": r.own, "lined": r.lined, "settlements": r.settlements, "english": r.english, "nowhere": r.nowhere, "landmarks": r.landmarks, "areas": r.areas}), t0.elapsed().as_secs_f64());
        return Ok(());
    }
    let scratch = PathBuf::from(opt(&args, "--scratch").unwrap_or_else(|| "/tmp/scenic-build".into()));
    let mut out = Out::open(&root, &scratch)?;
    let t0 = std::time::Instant::now();
    match step.as_str() {
        "pack" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            let mirror = opt(&args, "--mirror").map(PathBuf::from);
            pack(&mut out, &cache, mirror.as_deref(), &positional(&args))?
        }
        "lo" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            let mirror = opt(&args, "--mirror").map(PathBuf::from);
            lo(&mut out, &cache, mirror.as_deref(), &positional(&args))?
        }
        "osm-pass" => {
            let planet = PathBuf::from(opt(&args, "--planet").context("--planet <path>")?);
            let date = opt(&args, "--date").context("--date YYYY-MM-DD")?;
            let extract = PathBuf::from(opt(&args, "--extract").unwrap_or_else(|| "target/release/extract".into()));
            let planetiler = PathBuf::from(opt(&args, "--planetiler").unwrap_or_else(|| "tools/planetiler.jar".into()));
            pipeline::osmpass::check_tools(&extract, &planetiler)?;
            // Room first: caches the pass makes worthless or can refill (the pack cache).
            for dir in args.windows(2).filter(|w| w[0] == "--clear").map(|w| PathBuf::from(&w[1])) {
                if dir.exists() {
                    eprintln!("osm-pass: clearing {}", dir.display());
                    std::fs::remove_dir_all(&dir)?;
                }
            }
            pipeline::osmpass::run_pass(&mut out, &planet, &date, &scratch, &extract, &planetiler)?
        }
        "patch-ferries" => {
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
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
        // catalog [--held] [--ready <ids>]: held, it goes to catalog-held/, which no server reads (a
        // build to compare before it's switched to: inputs/hold-catalog); `--ready`, the regions it
        // records as built (comma-separated; the agent's plan says which), the others as the last
        // catalog had them (without it: every region as its recipe is now).
        "catalog" => {
            // (`<id>=<outline digest>` each, or a bare id: a region built with the outline it has now.)
            let ready: Option<BTreeMap<String, Option<String>>> = opt(&args, "--ready").map(|v| {
                v.split(',').filter(|s| !s.is_empty()).map(|e| match e.split_once('=') {
                    Some((id, d)) => (id.to_string(), Some(d.to_string())),
                    None => (e.to_string(), None),
                }).collect()
            });
            catalog(&mut out, args.iter().any(|a| a == "--held"), ready.as_ref())?
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
            let dir = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
            if args.iter().any(|a| a == "--order") {
                let mut paths = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut paths)?;
                let have = pipeline::rawpack::packed(&store)?;
                let list = pipeline::rawpack::order(&paths, |a, k| have.get(a).is_some_and(|s| s.contains(&k)));
                eprintln!("raw-pack: {} of {} files already in the archives", paths.lines().count() - list.lines().count(), paths.lines().count());
                print!("{list}");
            } else if args.iter().any(|a| a == "--check") {
                let every = opt(&args, "--every").map(|n| n.parse()).transpose()?.unwrap_or(0);
                let c = pipeline::rawpack::check(&store, every)?;
                eprintln!("raw-pack: {} archives of {} tiles; {} bad; {} tiles matched against the loose ones, {} differ", c.archives, c.tiles, c.bad.len(), c.sampled, c.differ.len());
                for b in c.bad.iter().chain(&c.differ) {
                    eprintln!("  {b}");
                }
                anyhow::ensure!(c.bad.is_empty() && c.differ.is_empty(), "the archives aren't right");
            } else if opt(&args, "--from-tar").as_deref() == Some("-") {
                let expect = opt(&args, "--expect").map(|n| n.parse()).transpose()?;
                let r = pipeline::rawpack::pack_tar(std::io::stdin().lock(), &dir, &store, out.root(), expect)?;
                eprintln!("raw-pack: {} files in the stream, {} tiles, {} new to their archives", r.files, r.tiles, r.added);
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
                let raw_dir = PathBuf::from(opt(&args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
                let raw = raw_tiles(&out, &raw_dir);
                let parts = Parts(&["Fetching and repairing the world's z8 terrain", "Packing the new raw tiles onto the NAS"]);
                parts.start(0);
                let (n, none) = pipeline::terrain_z8::build(&mut out, &raw)?;
                eprintln!("terrain-z8: {n} tiles ({none} of open sea)");
                parts.start(1);
                pack_raw_with(&out, &raw_dir, &|what, done, total| pipeline::agent::jobs::report(done, total, what));
            }
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
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let src = pipeline::markconv::heritage_source(&out, &date);
            stage(0, 1, "making the area overlays' tiles");
            let dots = pipeline::ovconv::marks_dots(&out)?;
            pipeline::ovconv::overlays(&mut out, &src, &dots)?;
            out.save()?;
        }
        "registers-import" => registers_import(&mut out, &args, &scratch)?,
        "slope" => slope_step(&mut out, &args)?,
        "labels" => labels_step(&mut out, &args, &scratch)?,
        "spoken" => {
            // spoken [--pass <date>]: the languages spoken where (names::spoken), from the pass's
            // outlines, as global/spoken, which the servers read.
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let outlines = out.path(out.get(&format!("sources/osm/{date}/outlines")).context("the pass's outlines")?);
            let s = pipeline::outlines::Outlines::open(&outlines)?.spoken()?;
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("spoken.bin");
            std::fs::write(&local, s.to_bytes())?;
            let name = out.put_file("global/spoken", "bin", &local)?;
            std::fs::remove_file(&local).ok();
            out.save()?;
            eprintln!("spoken: {} regions, {name}", s.regions().count());
        }
        "water" => water_step(&mut out, &args, &scratch)?,
        "pass-sets" => {
            // pass-sets [--pass <date>]: the sets the pass lacks in their current filters
            // (osmpass::SETS versions), from its kept filtered planet: copied here first when
            // there's room, since osmium reads it two or three times a set (from the NAS over
            // Wi-Fi that took hours a set; over the LAN, 65 min for the water set).
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let nas = out.path(out.get(&format!("sources/osm/{date}/filtered")).context("the pass's filtered planet")?);
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("filtered.osm.pbf");
            let size = std::fs::metadata(&nas)?.len();
            // (Room for the copy, the sets made from it, and the agent's reserve left free.)
            let room = pipeline::agent::cond::free_bytes(&scratch).unwrap_or(0) + std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0) > size + (20 << 30) + pipeline::agent::room::RESERVE;
            let src = if room && !pipeline::osmpass::missing_sets(&out, &date).is_empty() {
                pipeline::osmpass::copy_resume_with(&nas, &local, &mut |d, t| pipeline::agent::jobs::report(d >> 20, t >> 20, "MB of the filtered planet copied here (then the sets)"))?;
                local.clone()
            } else {
                nas
            };
            let n = pipeline::osmpass::make_missing_sets(&mut out, &date, &src, &scratch)?;
            std::fs::remove_file(&local).ok();
            eprintln!("pass-sets: {n} made");
        }
        "trailends" => {
            // trailends [--pass <date>]: every hiking route's ends, worldwide, from the hikes set.
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let set = out.path(out.get(&pipeline::osmpass::set_name(&date, "hikes")).context("the pass has no hikes set (pass-sets makes it)")?);
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("set-hikes.osm.pbf");
            let parts = Parts(&["Copying the hiking routes", "Finding their ends", "Uploading"]);
            parts.start(0);
            store::sys::copy_data(&set, &local).with_context(|| format!("copy {}", set.display()))?;
            parts.start(1);
            let ends = pipeline::trailends::ends(&local)?;
            parts.start(2);
            let file = scratch.join("trailends.jsonl.zst");
            pipeline::trailends::write(&file, &ends)?;
            out.put_file(&format!("work/trailends/{date}"), "jsonl.zst", &file)?;
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
        "bldtile-task" => bldtile_task_step(&mut out, &args)?,
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
            let ts: Vec<Unit> = positional(&args).iter().map(|t| Unit::parse(t).filter(|u| u.z == 3 || u.z == 6).with_context(|| format!("not a z3 or z6 tile: {t}"))).collect::<Result<_>>()?;
            let same: BTreeSet<String> = opt(&args, "--expect-same").map(|v| v.split(',').filter(|t| !t.is_empty()).map(String::from).collect()).unwrap_or_default();
            let named: BTreeSet<String> = ts.iter().filter(|u| u.z == 6).map(|u| u.slash()).collect();
            anyhow::ensure!(same.is_subset(&named), "--expect-same names z6 tiles not given: {:?}", same.difference(&named).collect::<Vec<_>>());
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
                let t = cost_start();
                let writing = || pipeline::agent::jobs::part(2 * k + 1, &names);
                if u.z == 3 {
                    pipeline::treepacks::build_with(&mut out, &cov, u, &dem, &chm, &scratch, workers, &writing)?;
                } else {
                    pipeline::treepacks::build_piece(&mut out, &cov, u, &dem, &chm, &scratch, workers, same.contains(&u.slash()), &writing)?;
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
            let qs: Vec<Unit> = positional(&args).iter().map(|t| Unit::parse(t).filter(|u| u.z == 3).with_context(|| format!("not a z3 tile: {t}"))).collect::<Result<_>>()?;
            for (k, &q) in qs.iter().enumerate() {
                pipeline::control::safe_point("trees-lo");
                pipeline::agent::jobs::report(k as u64, qs.len() as u64, "areas");
                let t = cost_start();
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
            let t = positional(&args).first().and_then(|t| Unit::parse(t)).filter(|u| u.z == 6).context("treeblock-task <6/x/y> --row y")?;
            let y: u32 = opt(&args, "--row").context("--row y")?.parse()?;
            let shapes = pipeline::trees::mask::Shapes::parse(&serde_json::to_string(&pipeline::treepacks::coverage_json(&cov, t))?)?;
            let row: Vec<(u32, u32)> = pipeline::trees::task::rows_of(&pipeline::trees::blocks_of(&shapes, 6, t.x, t.y)).into_iter().flatten().filter(|b| b.1 == y).collect();
            anyhow::ensure!(!row.is_empty(), "{}: no block of row {y} meets the coverage", t.slash());
            let dir = PathBuf::from(opt(&args, "--out").context("--out dir")?);
            treeblock_task(&out, &cov, &row, &dir)?;
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
            // ferries --pass <date> [--dem dir]: the pass's ferries set through ferries.py, as blocks.
            let date = opt(&args, "--pass").context("--pass <date>")?;
            let dem = PathBuf::from(opt(&args, "--dem").unwrap_or_else(|| "dem".into()));
            stage(0, 1, "finding the ferries (ferries.py) and their blocks");
            let n = pipeline::ovconv::ferries_job(&mut out, &date, &dem)?;
            eprintln!("ferries: {n} blocks");
        }
        "rail-feeds" => rail_feeds_step(&mut out, &args, &scratch)?,
        "rail" => rail_step(&mut out, &args, &scratch)?,
        "terrain-root" => {
            let raw_dir = PathBuf::from(opt(&args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
            let raw = raw_tiles(&out, &raw_dir);
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
            let (Some(l), Some(ext), Some(f)) = (p.first(), p.get(1), p.get(2)) else { bail!("put <logical> <ext> <file>") };
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
fn put_roads(out: &mut Out, u: Unit, recs: &[pipeline::legacy::RoadRec]) -> Result<String> {
    let mut byroad: Vec<[u64; 2]> = recs.iter().enumerate().map(|(i, r)| [r.road, i as u64]).collect();
    byroad.sort_unstable();
    put_sect(out, &format!("global/roads/{}", u.dash()), serde_json::json!({"fmt": 1, "unit": u.slash()}), &[("roads", b(recs)), ("byroad", b(&byroad))])
}

fn put_sect(out: &mut Out, logical: &str, meta: serde_json::Value, sections: &[(&str, &[u8])]) -> Result<String> {
    let local = out.scratch_file(&format!("{logical}.sect"));
    let mut w = store::sect::SectWriter::create(&local, meta)?;
    for (n, bytes) in sections {
        w.add(n, bytes)?;
    }
    w.finish()?;
    out.put_file(logical, "sect", &local)
}

// ---- base packs, locally ------------------------------------------------------------------

/// Every unit's base pack and road values, copied into `cache` when missing, opened.
/// Every unit's base pack and road values, opened from local copies: this Mac's mirror's when it
/// has them (`mirror`, the server's: same content names, so no second copy), else the cache's,
/// copied from the NAS when missing. Cached files no unit uses any more go first, so the cache
/// holds at most one copy of what the mirror lacks.
fn open_units(out: &Out, cache: &Path, mirror: Option<&Path>) -> Result<Vec<BasePack>> {
    std::fs::create_dir_all(cache)?;
    let mut units: Vec<(String, String, String)> = Vec::new();
    for (k, v) in &out.manifest {
        if let Some(u) = k.strip_prefix("base/") {
            let roads = out.get(&format!("global/roads/{u}")).with_context(|| format!("no road values for {u}"))?;
            units.push((u.to_string(), v.clone(), roads.to_string()));
        }
    }
    let needed: BTreeSet<&str> = units.iter().flat_map(|(_, b, r)| [b.as_str(), r.as_str()]).collect();
    prune_cache(cache, &needed)?;
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
            // (The mirror renames a file into place only once it's copied and checked.)
            if let Some(m) = mirror.map(|m| m.join(name)).filter(|m| m.is_file()) {
                local.push(m);
                continue;
            }
            // (Held for the job, copied from the NAS first when it isn't here: store::cachefile.)
            let p = store::cachefile::hold(&cache.join(name), &mut |tmp| {
                store::sys::copy_data(out.path(name), tmp)?;
                if store::naming::hash16_file(tmp).map_err(std::io::Error::other)? != name.rsplit('.').nth(1).unwrap_or("") {
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
/// interrupted copies.
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

fn pack(out: &mut Out, cache: &Path, mirror: Option<&Path>, only: &[String]) -> Result<()> {
    let parts = Parts(&["Getting the areas' base packs here", "Drawing the map tiles"]);
    parts.start(0);
    let packs = open_units(out, cache, mirror)?;
    let refs: Vec<&BasePack> = packs.iter().collect();
    let ts: Vec<Unit> = if only.is_empty() { tiles_touched(&packs, 6)?.into_iter().collect() } else { parse_tiles(only, 6)? };
    eprintln!("pack: {} tiles from {} units", ts.len(), packs.len());
    parts.start(1);
    let t0 = std::time::Instant::now();
    let n = ts.len() as u64;
    // (Within a tile: its ways read, then its tiles drawn, then written.)
    let at = |k: usize, f: f64| pipeline::agent::jobs::report_f(k as f64 + f, n, "map tiles");
    for (k, t) in ts.iter().enumerate() {
        // (A safe point before each tile: with the build pausing, the job ends here.)
        pipeline::control::safe_point("pack");
        at(k, 0.0);
        let tb = tile_bounds(t.z, t.x, t.y);
        let halo_b = grow(tb, 100.0);
        let near: Vec<&BasePack> = refs.iter().copied().filter(|bp| meets(bp.extent, halo_b)).collect();
        let halo = hipack::ways_in(&near, halo_b)?;
        let in_t: Vec<hipack::Staged> = halo.iter().copied().filter(|s| meets(s.bbox, tb)).collect();
        if in_t.is_empty() {
            // No ways here (any more): what an earlier build made for the tile goes.
            drop_entries(out, &[format!("hidata/{}", t.dash()), format!("layers/roads/hi/{}", t.dash()), format!("layers/rails/hi/{}", t.dash())])?;
            pipeline::control::done("pack", &t.slash());
            continue;
        }
        let win = hipack::way_inputs(&near, &in_t)?;
        at(k, 0.25);
        let (roads, rails) = hipack::tiles(&win, 6, t.x, t.y, 9..=14, false);
        at(k, 0.6);
        for (layer, enc) in [("roads", &roads), ("rails", &rails)] {
            let mut it = enc.iter().map(|e| {
                let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                (z, x, y, e.gz.clone(), e.raw_len as u32)
            });
            if layers::write_pack(out, layer, "rt7", true, "hi", (6, t.x, t.y), &mut it)?.is_none() {
                drop_entries(out, &[format!("layers/{layer}/hi/{}", t.dash())])?;
            }
        }
        at(k, 0.75);
        let hd = hipack::hidata(*t, &near, &in_t, &halo)?;
        // The zoomed-out summaries (docs/phase5.md), from the same parts.
        let ls = roadcore::lsum::build(&hd.parts, &hd.psamples, &hd.pch, &hd.here, &hd.railinfo);
        put_sect(
            out,
            &format!("hidata/{}", t.dash()),
            serde_json::json!({"fmt": 1, "tile": t.slash(), "here": hd.here.len(), "parts": hd.parts.len(), "climbs": hd.climbs.len(), "lsum": roadcore::lsum::LSUM_V}),
            &[
                ("here", b(&hd.here)),
                ("ends", b(&hd.ends)),
                ("parts", b(&hd.parts)),
                ("psamples", b(&hd.psamples)),
                ("pch", b(&hd.pch)),
                ("climbs", b(&hd.climbs)),
                ("climbgeom", b(&hd.climbgeom)),
                ("railinfo", b(&hd.railinfo)),
                ("railstr", &hd.railstr),
                ("lparts", b(&ls.lparts)),
                ("lbins", b(&ls.lbins)),
                ("lrparts", b(&ls.lrparts)),
                ("lrbins", b(&ls.lrbins)),
            ],
        )?;
        out.save()?;
        pipeline::control::done("pack", &t.slash());
        eprintln!("pack {} ({}/{}): {} ways, {} road tiles, {} parts, {} climbs ({:.0?})", t.slash(), k + 1, ts.len(), in_t.len(), roads.len(), hd.parts.len(), hd.climbs.len(), t0.elapsed());
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

fn lo(out: &mut Out, cache: &Path, mirror: Option<&Path>, only: &[String]) -> Result<()> {
    let parts = Parts(&["Getting the areas' base packs here", "Drawing the zoomed-out tiles"]);
    parts.start(0);
    let packs = open_units(out, cache, mirror)?;
    let refs: Vec<&BasePack> = packs.iter().collect();
    let qs: Vec<Unit> = if only.is_empty() { tiles_touched(&packs, 3)?.into_iter().collect() } else { parse_tiles(only, 3)? };
    eprintln!("lo: {} tiles from {} units", qs.len(), packs.len());
    parts.start(1);
    let t0 = std::time::Instant::now();
    let n = qs.len() as u64;
    let at = |k: usize, f: f64| pipeline::agent::jobs::report_f(k as f64 + f, n, "zoomed-out tiles");
    for (k, q) in qs.iter().enumerate() {
        pipeline::control::safe_point("lo");
        at(k, 0.0);
        let qb = tile_bounds(q.z, q.x, q.y);
        let near: Vec<&BasePack> = refs.iter().copied().filter(|bp| meets(bp.extent, qb)).collect();
        let staged = hipack::ways_in(&near, qb)?;
        if staged.is_empty() {
            drop_entries(out, &[format!("layers/roads/lo/{}", q.dash()), format!("layers/rails/lo/{}", q.dash())])?;
            pipeline::control::done("lo", &q.slash());
            continue;
        }
        let win = hipack::way_inputs(&near, &staged)?;
        at(k, 0.3);
        let (roads, rails) = hipack::tiles(&win, 3, q.x, q.y, 4..=8, true);
        at(k, 0.8);
        for (layer, enc) in [("roads", &roads), ("rails", &rails)] {
            let mut it = enc.iter().map(|e| {
                let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                (z, x, y, e.gz.clone(), e.raw_len as u32)
            });
            if layers::write_pack(out, layer, "rt7", true, "lo", (3, q.x, q.y), &mut it)?.is_none() {
                drop_entries(out, &[format!("layers/{layer}/lo/{}", q.dash())])?;
            }
        }
        out.save()?;
        pipeline::control::done("lo", &q.slash());
        eprintln!("lo {} ({}/{}): {} ways, {} road tiles ({:.0?})", q.slash(), k + 1, qs.len(), staged.len(), roads.len(), t0.elapsed());
    }
    pipeline::agent::jobs::report(n, n, "zoomed-out tiles");
    Ok(())
}

// ---- catalog --------------------------------------------------------------------------------

/// The map's meta, added up from the units' summaries: each base pack's own, else worked out from
/// its sections once (packs without it) and kept in `state/build/summaries.json` by content
/// name (a content name never changes).
fn units_meta(out: &Out, base: &BTreeMap<String, String>) -> Result<serde_json::Value> {
    use pipeline::summary::Summary;
    let path = out.root().join("state/build/summaries.json");
    let mut known: BTreeMap<String, Summary> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
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
    m["built"] = serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs());
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
        // The 3D buildings: z12–14 hi packs (pipeline::bld).
        "buildings" => (pipeline::bld::MINZOOM, pipeline::bld::MAXZOOM),
        _ => return None,
    })
}

fn catalog(out: &mut Out, held: bool, ready: Option<&BTreeMap<String, Option<String>>>) -> Result<()> {
    let mut layers: BTreeMap<String, LayerOut> = BTreeMap::new();
    let (mut base, mut roads, mut hidata, mut global, mut basemap) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), Vec::new());
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
                    _ => "unknown",
                };
                let zs = match *scope {
                    "root" => (0, 2),
                    "lo" => (3, 8),
                    _ => (9, 14),
                };
                layers.entry(layer.to_string()).or_insert_with(|| LayerOut::new(enc)).add(scope, (u.z, u.x, u.y), logical.clone(), zs);
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
    // The latest pass's outlines, for the Regions panel and the modules.
    // (Exactly sources/osm/<date>/outlines: the pass's outline set, sets/outlines, is its input.)
    if let Some(l) = out.manifest.keys().filter(|k| matches!(k.split('/').collect::<Vec<_>>()[..], ["sources", "osm", _, "outlines"])).max() {
        global.insert("outlines".to_string(), l.clone());
    }
    let meta = units_meta(out, &base)?;
    let units: Vec<String> = base.keys().cloned().collect();
    // The coverage it's built for, drawn from the outlines it lists, and the credits of the sources
    // its data comes from: where the coverage is, and where the units' ways are.
    let dir = out.root().join(if held { "catalog-held" } else { "catalog" });
    let regions = catalog_coverage(out, global.get("outlines").map(String::as_str), ready, &dir)?;
    let credits = pipeline::rules::catalog_credits(&regions, &unit_extents(out, &base));
    eprintln!("catalog: {} regions, {} of {} credits", regions.len(), credits.len(), pipeline::rules::CREDITS.len());
    // Only what the map reads: build sources (the planet's pieces, sets and road values) stay out,
    // or every Mac's mirror would copy them. A file missing on the NAS stops the publish.
    let served = |l: &str| {
        (l.starts_with("layers/") && (!l.starts_with("layers/basemap/") || basemap.iter().any(|b| b == l)))
            || l.starts_with("base/")
            || l.starts_with("hidata/")
            || l.starts_with("markdata/")
            || l.starts_with("ovdata/")
            || l.starts_with("global/")
            || global.values().any(|g| g == l)
    };
    let mut files: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let listed: Vec<(&String, &String)> = out.manifest.iter().filter(|(l, _)| served(l)).collect();
    let (total, step) = (listed.len() as u64, (listed.len() / 100).max(1));
    for (k, (l, n)) in listed.into_iter().enumerate() {
        if k % step == 0 {
            pipeline::agent::jobs::report(k as u64, total, "files checked on the NAS");
        }
        let size = std::fs::metadata(out.path(n)).with_context(|| format!("{l}: {n} is missing on the NAS"))?.len();
        files.insert(l.clone(), serde_json::json!({"file": n, "size": size, "fmt": 1}));
    }
    std::fs::create_dir_all(&dir)?;
    let n = store::catalog::next_n(&dir)?;
    let cat = serde_json::json!({
        "fmt": 1,
        "n": n,
        "created": chrono_now(),
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
        // (`recorded`: this catalog says which regions it's built for, even none of them yet; one
        // made before catalogs did has none, and the server draws the recipes for it.)
        "coverage": {"regions": regions, "recorded": true},
    });
    let catalog: store::catalog::Catalog = serde_json::from_value(cat)?;
    let path = store::catalog::write(&dir, &catalog)?;
    eprintln!("published {}", path.display());
    Ok(())
}

/// The coverage a catalog records: the regions built as their recipes are now (`ready`; without it
/// every recipe), each outline entry simplified for drawing (pipeline::coverage::drawn), `osm:`
/// ones from `outlines` (the catalog's own); and a region not built yet as the last catalog in
/// `catalogs` had it, if it had it (on the map as it was: its old outline, its units not yet rebuilt),
/// in the recipes' order. So the regions are those the catalog's data is built for; the Regions
/// panel shows a recipe it lacks, or has with another outline, as still to come.
/// A read that fails (the NAS) fails the catalog, to be tried again, rather than record a region
/// without its outline or leave a region out.
fn catalog_coverage(out: &Out, outlines: Option<&str>, ready: Option<&BTreeMap<String, Option<String>>>, catalogs: &Path) -> Result<Vec<pipeline::coverage::DrawnRegion>> {
    let dir = out.root().join("inputs/regions");
    std::fs::read_dir(&dir).with_context(|| format!("the regions ({})", dir.display()))?;
    let (recipes, bad) = pipeline::agent::recipes::load(&dir);
    for (f, e) in &bad {
        // (A recipe that reads but doesn't parse is left out; one that doesn't read is the NAS.)
        std::fs::read(dir.join(f)).with_context(|| format!("region {f}"))?;
        eprintln!("catalog: region {f} left out: {e}");
    }
    let outlines = match outlines.and_then(|l| out.get(l)) {
        Some(c) => Some(pipeline::outlines::Outlines::open(&out.path(c)).context("the pass's outlines")?),
        None => None,
    };
    let Some(ready) = ready else { return pipeline::coverage::drawn(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines")) };
    // (Built with the outline its recipe has now: one redrawn since the plan said so isn't.)
    let is_ready = |r: &pipeline::agent::recipes::Recipe| ready.get(&r.id).is_some_and(|d| d.as_ref().is_none_or(|d| *d == pipeline::agent::recipes::outline_digest(&r.outline)));
    let built: Vec<_> = recipes.iter().filter(|r| is_ready(r)).cloned().collect();
    let mut drawn: BTreeMap<String, pipeline::coverage::DrawnRegion> = pipeline::coverage::drawn(&built, outlines.as_ref(), &out.root().join("inputs/outlines"))?.into_iter().map(|d| (d.id.clone(), d)).collect();
    // (The first held catalog keeps the regions as the served one has them.)
    let mut last = store::catalog::latest(catalogs).with_context(|| format!("the last catalog in {}", catalogs.display()))?;
    if last.is_none() && catalogs.ends_with("catalog-held") {
        last = store::catalog::latest(&out.root().join("catalog")).context("the last catalog")?;
    }
    let had: BTreeMap<String, pipeline::coverage::DrawnRegion> = last
        .and_then(|c| c.coverage.get("regions").and_then(|v| serde_json::from_value::<Vec<pipeline::coverage::DrawnRegion>>(v.clone()).ok()))
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
    let known: BTreeMap<String, pipeline::summary::Summary> = std::fs::read(out.root().join("state/build/summaries.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    base.iter()
        .filter_map(|(u, logical)| match out.get(logical).and_then(|c| known.get(c)) {
            // (An empty unit's extent is inverted: nothing of it is anywhere.)
            Some(s) => (s.extent[0] <= s.extent[2]).then_some(s.extent),
            None => Unit::parse(u).map(|u| tile_bounds(u.z, u.x, u.y)),
        })
        .collect()
}

/// A target's start, for `note_cost`: its time, and the job's peak memory started again.
fn cost_start() -> std::time::Instant {
    pipeline::sys::reset_group_peak();
    std::time::Instant::now()
}

/// What a job of `step` took for `target` (since its `cost_start`): the most memory the job's
/// processes held together meanwhile (sampled: a pool's workers summed, an earlier target's peak
/// not counted) and its time, noted for the coordinator (`SCENIC_COSTS`), which gives a helper only
/// work that fits its memory (crate::coord::job_peak).
fn note_cost(step: &str, target: &str, t0: std::time::Instant) {
    let Some(p) = std::env::var_os("SCENIC_COSTS") else { return };
    let line = serde_json::json!({ "unit": pipeline::coord::cost_key(step, target), "peak_mb": pipeline::sys::group_peak() >> 20, "secs": t0.elapsed().as_secs(), "v": pipeline::coord::cost_version(step) });
    let r = std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("{step} {target}: noting what it cost: {e}");
    }
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
    match pipeline::rawpack::pack_local_with(dir, &out.root().join("sources/aws-terrarium"), out.root(), true, progress) {
        Ok(_) => {}
        Err(e) => eprintln!("raw tiles: not packed now ({e:#}); they wait in the cache"),
    }
}

/// AWS's raw terrain tiles: the local cache `dir`, where each is downloaded once, filled from the
/// NAS's archives (`sources/aws-terrarium/packs/`) when it lacks one.
fn raw_tiles(out: &Out, dir: &Path) -> pipeline::terrain_pack::RawTiles {
    pipeline::terrain_pack::RawTiles::with_store(dir, &out.root().join("sources/aws-terrarium"))
}

/// UTC now as RFC 3339 (no chrono dependency).
fn chrono_now() -> String {
    utc(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64)
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
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
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
    let (recipes, bad) = pipeline::agent::recipes::load(&root.join("inputs/regions"));
    anyhow::ensure!(bad.is_empty(), "regions that can't be read now (the agent re-keys nothing meanwhile): {bad:?}");
    let outlines = m.get(&format!("sources/osm/{date}/outlines")).map(|c| pipeline::outlines::Outlines::open(&root.join(c))).transpose()?;
    let cov = pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &root.join("inputs/outlines"))?;
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
    println!("pass {date}, {} regions; the terrain packs' indexes: {n} read in {:.1} s, {} not", recipes.len(), t0.elapsed().as_secs_f64(), tiles.unread().count());
    for (c, why) in tiles.unread() {
        println!("  {c}: {why}");
    }
    let t1 = std::time::Instant::now();
    let old = rekey::v1::unit_keys(&cov, &date, &m, Some(&reach), &digests);
    let t_old = t1.elapsed().as_secs_f64();
    let new: BTreeMap<String, Option<String>> = build::unit_keys(&cov, &date, &m, Some(&reach), &digests, &tiles).into_iter().map(|(u, k)| (u.slash(), k)).collect();
    let t_new = t1.elapsed().as_secs_f64() - t_old;
    let times = rekey::FileTimes::new(root);
    let older = |x: u32, y: u32| times.hi_older(&m, x, y);
    let mut after = keys.clone();
    let r = rekey::rekey(&mut after, &cov, &date, &m, Some(&reach), &digests, &tiles, &times);
    println!("(the old keys took {t_old:.1} s, the new {t_new:.1} s, the re-keying {:.1} s)", t1.elapsed().as_secs_f64() - t_old - t_new);
    // (Again on what it made: nothing to do, as each plan's re-keying finds once it's done.)
    let t2 = std::time::Instant::now();
    let mut twice = after.clone();
    let again = rekey::rekey(&mut twice, &cov, &date, &m, Some(&reach), &digests, &tiles, &times);
    println!("a second pass: {} (in {:.1} s)", if again.changed() || twice != after { "it changed the records again" } else { "nothing to do" }, t2.elapsed().as_secs_f64());
    let rec = |u: Unit| keys.unit.get(&u.slash());
    let new_of = |u: Unit| new.get(&u.slash()).and_then(Option::as_ref);
    let current: Vec<Unit> = old.iter().filter(|(u, k)| rec(*u) == Some(k)).map(|(u, _)| *u).collect();
    let already = old.iter().filter(|(u, _)| rec(*u).is_some() && rec(*u) == new_of(*u)).count();
    let stale = old.iter().filter(|(u, k)| rec(*u).is_some_and(|x| x != k && Some(x) != new_of(*u))).count();
    let never = old.iter().filter(|(u, _)| rec(*u).is_none()).count();
    let built: BTreeSet<String> = old.iter().map(|(u, _)| u.slash()).collect();
    let words = |v: &[String]| if v.is_empty() { String::new() } else { format!(": {}", v.join(", ")) };
    println!("units (worked out in {:.1} s): {} recorded, {} the coverage builds", t1.elapsed().as_secs_f64(), keys.unit.len(), old.len());
    println!("  current under the old keys: {}", current.len());
    println!("    re-keyed: {} ({} of them without outputs{})", r.moved.len(), r.empty.len(), words(&r.empty));
    println!("    left to build again: {}", r.left.len());
    println!("    unknown now (an index unread): {}", r.unknown.len());
    println!("  under the new keys already: {already}");
    println!("  stale under the old keys (built again either way): {stale}");
    println!("  never built: {never}");
    println!("  recorded, but not built by the coverage now (pruned in time): {}", keys.unit.keys().filter(|u| !built.contains(*u)).count());
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
    let hi: Vec<(u32, u32)> = m.keys().filter_map(|l| l.strip_prefix("layers/terrain/hi/")).filter_map(Unit::parse).map(|t| (t.x, t.y)).collect();
    let left: Vec<String> = hi.iter().filter(|t| pieces.contains(*t) && older(t.0, t.1) != Some(false)).map(|t| format!("6/{}/{}{}", t.0, t.1, if older(t.0, t.1).is_none() { " (its files' times unread)" } else { "" })).collect();
    let gone = hi.iter().filter(|t| !pieces.contains(*t)).count();
    println!("terrain hi packs: {}, {} of them stale: {gone} of z6 tiles the coverage left, {} an earlier run left (older than their area's lo pack){}", hi.len(), gone + left.len(), left.len(), words(&left));
    // (The served catalogs and those held for review, as they were made.)
    let mut cats: Vec<(i64, String, store::catalog::Catalog)> = Vec::new();
    for dir in ["catalog", "catalog-held"] {
        for n in store::catalog::list(&root.join(dir))? {
            match store::catalog::read(&root.join(dir).join(store::catalog::file_name(n))) {
                Ok(c) => cats.push((epoch_of(&c.created).unwrap_or(i64::MAX), format!("{dir} {n}"), c)),
                Err(e) => println!("  ({dir}/{n} not read: {e:#})"),
            }
        }
    }
    cats.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let first = store::catalog::list(&root.join("catalog"))?.into_iter().min().map(|n| store::catalog::read(&root.join("catalog").join(store::catalog::file_name(n)))).transpose()?.context("no catalog")?;
    let lo_of = |c: &store::catalog::Catalog, q: &str| -> Option<String> { Some(c.files.get(c.layers.get("terrain")?.lo.get(q)?)?.file.clone()) };
    let mtime = |content: &str| std::fs::metadata(root.join(content)).and_then(|md| md.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
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
        let areas: BTreeSet<String> = read.iter().filter(|t| (4..=8).contains(&t.0)).map(|&(z, x, y, _)| format!("3/{}/{}", x >> (z - 3), y >> (z - 3))).filter(|q| targets.contains(q)).collect();
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
            let each: Vec<String> = areas.iter().map(|q| format!("{q} {}", first_new.get(q).map_or("none".to_string(), |(t, _)| utc(*t)))).collect();
            not.push(format!("  {} ({}), built {}; first of {}", u.slash(), if is_moved { "re-keyed" } else { "left to build" }, at.map_or("?".to_string(), utc), each.join(", ")));
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
/// runs (`--costs`: the coordinator's costs.json; a helper's at the build Mac's pace, twice its
/// speed) and the packs made again (their size now).
fn p5_check(root: &Path, args: &[String]) -> Result<()> {
    use pipeline::agent::{build, rekey, tiles::TerrainTiles};
    anyhow::ensure!(positional(args).first().map(String::as_str) == Some("trees"), "p5-check trees (terrain and slope come later)");
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(root)).context("no complete OSM pass")?;
    let m: BTreeMap<String, String> = pipeline::out::read_record(&root.join("state/build/manifest.json"))?;
    let keys = build::Keys::load_strict(root)?;
    let (recipes, bad) = pipeline::agent::recipes::load(&root.join("inputs/regions"));
    anyhow::ensure!(bad.is_empty(), "regions that can't be read now (the agent re-keys nothing meanwhile): {bad:?}");
    let outlines = m.get(&format!("sources/osm/{date}/outlines")).map(|c| pipeline::outlines::Outlines::open(&root.join(c))).transpose()?;
    let cov = pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &root.join("inputs/outlines"))?;
    let costs: BTreeMap<String, pipeline::coord::Cost> = match opt(args, "--costs") {
        Some(p) => pipeline::out::read_record(Path::new(&p))?,
        None => BTreeMap::new(),
    };
    let me = pipeline::agent::cond::host();
    // (A z3 tile's last run, at the build Mac's pace.)
    let last_run = |q: &str| costs.get(&pipeline::coord::cost_key("trees", q)).map(|c| if c.worker.as_deref().is_none_or(|w| w == me) { c.secs as f64 } else { c.secs as f64 / 2.0 });
    let t0 = std::time::Instant::now();
    let times = rekey::FileTimes::new(root);
    let z3: Vec<(String, String)> = keys.trees.iter().filter(|(t, _)| Unit::parse(t).is_some_and(|u| u.z == 3)).map(|(t, k)| (t.clone(), k.clone())).collect();
    println!("pass {date}, {} regions; tree cover's records: {} z3 tiles' whole runs (the old scheme), {} pieces, {} assemblies", recipes.len(), z3.len(), keys.trees.len() - z3.len(), keys.trees_lo.len());
    // The re-keying, its tree cover rules alone (no reaches: the units' are rekey-check's).
    let mut after = keys.clone();
    let r = rekey::rekey(&mut after, &cov, &date, &m, None, &BTreeMap::new(), &TerrainTiles::new(None), &times);
    let mut twice = after.clone();
    let again = rekey::rekey(&mut twice, &cov, &date, &m, None, &BTreeMap::new(), &TerrainTiles::new(None), &times);
    println!("re-keyed: {} z3 tiles as their pieces and assemblies; dropped: {}; unknown now: {}; a second pass: {} ({:.1} s)", r.trees_moved.len(), r.trees_dropped.len(), r.trees_unknown.len(), if again.changed() || twice != after { "it changed the records again" } else { "nothing to do" }, t0.elapsed().as_secs_f64());
    // Each z3 tile recorded: what it was, whose its packs are, what the switch does.
    let old: BTreeMap<String, String> = rekey::v1::trees_targets(&cov, &m).into_iter().collect();
    let tt = pipeline::treepacks::targets(&cov, &m);
    let mtime = |c: &str| std::fs::metadata(root.join(c)).and_then(|md| md.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
    let size = |c: &str| std::fs::metadata(root.join(c)).map(|md| md.len()).unwrap_or(0);
    let packs_of = |q: Unit| -> Vec<&String> {
        m.iter().filter(|(l, _)| pipeline::treepacks::LAYERS.iter().any(|layer| l.starts_with(&format!("layers/{layer}/"))) && (l.ends_with(&format!("/lo/{}", q.dash())) || l.rsplit('/').next().and_then(Unit::parse).is_some_and(|u| u.z == 6 && (u.x >> 3, u.y >> 3) == (q.x, q.y)))).map(|(_, c)| c).collect()
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
        "its time by the z3 tiles' last runs (at the build Mac's pace): made again {:.1} h{}, the packs uploaded again {:.2} GB; the mids backfilled {:.1} h; the assemblies seconds each",
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
    Ok(b.chunks_exact(32).map(|c| (u64::from_le_bytes(c[..8].try_into().unwrap()), bytemuck::pod_read_unaligned(&c[8..32]))).collect())
}

/// pois <units…> [--pass <date>]: each unit's landmark candidates (pipeline::candidates), from
/// its piece through `extract --candidates` with the pass's hiking-route ends, as `work/pois/<u>`.
fn pois_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass on the NAS (--pass)")?,
    };
    let regions = opt(args, "--regions").map(PathBuf::from).unwrap_or_else(|| out.root().join("inputs/regions"));
    let (recipes, _) = pipeline::agent::recipes::load(&regions);
    anyhow::ensure!(!recipes.is_empty(), "no regions in {}", regions.display());
    let outlines_file = out.get(&format!("sources/osm/{date}/outlines")).map(|n| out.path(n));
    let outlines = outlines_file.as_deref().map(pipeline::outlines::Outlines::open).transpose()?;
    let cov = pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines"))?;
    let trailends = out.get(&format!("work/trailends/{date}")).map(|c| out.path(c)).context("no hiking-route ends for the pass (the trailends step)")?;
    let extract = std::env::current_exe()?.parent().context("bin")?.join("extract");
    std::fs::create_dir_all(scratch)?;
    let units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    for (k, &u) in units.iter().enumerate() {
        pipeline::control::safe_point("pois");
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas");
        let t = cost_start();
        let Some(piece) = out.get(&format!("sources/osm/{date}/pieces/{}", u.dash())).map(|n| out.path(n)) else {
            eprintln!("pois {}: no piece", u.slash());
            continue;
        };
        let local = scratch.join(format!("piece-{}.osm.pbf", u.dash()));
        store::sys::copy_data(&piece, &local).with_context(|| format!("copy {}", piece.display()))?;
        let dir = scratch.join(format!("pois-{}", u.dash()));
        let mut c = std::process::Command::new(&extract);
        c.arg(&dir).arg("8").arg("--candidates").arg("--trailends").arg(&trailends).arg(&local);
        let o = c.output().with_context(|| format!("run {}", extract.display()))?;
        anyhow::ensure!(o.status.success(), "extract --candidates for {}: {}\n{}", u.slash(), o.status, String::from_utf8_lossy(&o.stderr).lines().rev().take(8).collect::<Vec<_>>().join("\n"));
        let cands = pipeline::candidates::from_pois(&std::fs::read(dir.join("pois.json"))?, u, &cov)?;
        let file = scratch.join(format!("pois-{}.jsonl.zst", u.dash()));
        pipeline::candidates::write(&file, &cands)?;
        out.put_file(&format!("work/pois/{}", u.dash()), "jsonl.zst", &file)?;
        out.save()?;
        pipeline::control::done("pois", &u.slash());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(&local).ok();
        std::fs::remove_file(&file).ok();
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
            if n != me && n.len() == me.len() && n.starts_with(pre) && n.ends_with(post) && pipeline::osmpass::is_date(&n[pre.len()..pre.len() + 10]) {
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
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t = std::time::Instant::now();
    let parts = Parts(&["Copying the summits", "Reading them", "Taking their heights from the worldwide z8 terrain", "Uploading"]);
    parts.start(0);
    let set = local_copy(out, &pipeline::osmpass::set_name(&date, "summits"), &cache)?;
    parts.start(1);
    let mut summits = pipeline::summits::read_set(&set)?;
    parts.start(2);
    let z8 = open_z8(out, &cache)?;
    let raised = pipeline::summits::add_z8(&mut summits, &z8)?;
    parts.start(3);
    std::fs::create_dir_all(scratch)?;
    let file = scratch.join("summits.jsonl.zst");
    pipeline::summits::write(&file, &summits)?;
    out.put_file(&format!("work/summits/{date}"), "jsonl.zst", &file)?;
    out.save()?;
    std::fs::remove_file(&file).ok();
    eprintln!("summits: {} ({} with a z8 height) ({:.0?})", summits.len(), raised, t.elapsed());
    Ok(())
}

/// peaks <units…> [--pass <date>] [--raw dir] [--cache dir] [--coarse-threads n]: prominence and
/// isolation of each unit's peak candidates (pipeline::peaks::unit), as `work/peaks/<u>`.
fn peaks_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::peaks::unit;
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let raw_dir = PathBuf::from(opt(args, "--raw").unwrap_or_else(|| cache.join("aws-terrarium").to_string_lossy().into_owned()));
    let raw = raw_tiles(out, &raw_dir);
    let coarse_threads: usize = opt(args, "--coarse-threads").map(|s| s.parse()).transpose()?.unwrap_or(4);
    let summits = pipeline::summits::read(&local_copy(out, &format!("work/summits/{date}"), &cache)?)?;
    let base8 = unit::Z8Base::new(&summits);
    let z8 = open_z8(out, &cache)?;
    std::fs::create_dir_all(scratch)?;
    let units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    // Its parts: the areas' peaks, then the raw tiles AWS gave packed onto the NAS.
    let parts = Parts(&["Measuring the areas' peaks", "Packing the new raw tiles onto the NAS"]);
    parts.start(0);
    for (k, &u) in units.iter().enumerate() {
        // (A safe point before each area: the raw tiles fetched wait in the cache for the next job.)
        pipeline::control::safe_point("peaks");
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas");
        let t = cost_start();
        let pois = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)).with_context(|| format!("no candidates for {} (the pois step)", u.slash()))?;
        let peaks: Vec<unit::UnitPeak> = pipeline::candidates::read(&pois)?
            .into_iter()
            .filter(|c| c.kind == "peak")
            .filter_map(|c| Some(unit::UnitPeak { id: c.osm.clone()?, key: c.key, lon: c.lon, lat: c.lat, ele: c.ele }))
            .collect();
        let want = unit::tiles_wanted(&peaks.iter().map(|p| (p.lon as f64 * 1e-7, p.lat as f64 * 1e-7)).collect::<Vec<_>>(), 29.5);
        let z12 = unit::UnitZ12::load(out, &raw, &want)?;
        let res = unit::run(&peaks, &summits, &base8, &z12, &z8, coarse_threads)?;
        let file = scratch.join(format!("peaks-{}.jsonl.zst", u.dash()));
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
        out.put_file(&format!("work/peaks/{}", u.dash()), "jsonl.zst", &file)?;
        out.save()?;
        pipeline::control::done("peaks", &u.slash());
        std::fs::remove_file(&file).ok();
        eprintln!("peaks {}: {} peaks; z12 tiles {} from the packs, {} from AWS, {} sea ({:.0?})", u.slash(), res.len(), z12.from.0, z12.from.1, z12.from.2, t.elapsed());
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
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let read_json = |p: PathBuf| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)?) };
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
    let facts: HashMap<String, Value> = serde_json::from_value(read_json(facts_file)?)?;
    let views: HashMap<String, f64> = serde_json::from_value(read_json(views_file)?)?;
    let units = pipeline::agent::build::pois_keys(&cov, &date, &out.manifest);
    let parts = Parts(&["Reading every area's candidates and peaks", "Ranking the landmarks", "Writing their tiles"]);
    parts.start(0);
    let mut cands: Vec<(String, pipeline::marksjob::Candidate)> = Vec::new();
    let (mut with_peaks, mut missing_peaks) = (0, 0);
    for (u, _) in &units {
        let Some(pc) = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)) else { continue };
        let mut peaks: HashMap<String, Value> = HashMap::new();
        if let Some(pk) = out.get(&format!("work/peaks/{}", u.dash())).map(|c| out.path(c)) {
            use std::io::BufRead;
            for line in std::io::BufReader::new(zstd::Decoder::new(std::fs::File::open(&pk)?)?).lines() {
                let mut v: Value = serde_json::from_str(&line?)?;
                let key = v.as_object_mut().and_then(|o| o.remove("key")).and_then(|k| k.as_str().map(str::to_string)).context("a peak without its key")?;
                peaks.insert(key, v);
            }
        }
        for c in pipeline::candidates::read(&pc)? {
            let pk = peaks.get(&c.key).cloned();
            if c.kind == "peak" {
                if pk.is_some() { with_peaks += 1 } else { missing_peaks += 1 }
            }
            cands.push((c.key.clone(), pipeline::marksjob::from_unit(&c, pk, &facts)));
        }
    }
    anyhow::ensure!(missing_peaks == 0, "marks: {missing_peaks} peaks have no prominence yet (the peaks step)");
    // One order for every candidate, whichever unit it came from.
    cands.sort_by(|a, b| a.0.cmp(&b.0));
    let cands: Vec<pipeline::marksjob::Candidate> = cands.into_iter().map(|c| c.1).collect();
    eprintln!("marks: {} candidates from {} units ({with_peaks} peaks)", cands.len(), units.len());
    parts.start(1);
    let pts = pipeline::marksjob::poi_points(&cands, &views);
    let summits = pipeline::marksjob::summits_list(&pts);
    let mut all = pts;
    // The pass's heritage (the heritage job's outputs), else the converted build's (plan §10).
    let src = pipeline::markconv::heritage_source(out, &date);
    eprintln!("marks: heritage from {src}");
    all.extend(pipeline::markconv::heritage_marks(out, &src)?);
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
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into()));
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned())).join("items");
    let (mut facts, mut views) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    let is_qid = |q: &str| q.len() > 1 && q.starts_with('Q') && q[1..].bytes().all(|b| b.is_ascii_digit());
    // (items.py marks its three: SCENIC_PARTS names them.)
    const PARTS: &[&str] = &["Gathering the landmark candidates' Wikidata items", "Fetching their facts from Wikidata", "Looking up their Wikipedia articles", "Counting their pageviews in four months of Wikipedia's dumps", "Uploading"];
    let parts = Parts(PARTS);
    parts.start(0);
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
    // The pass's answers: this Mac's and the NAS's made one.
    let kept = pipeline::answers::items(out.root(), &cache, &date);
    let answers = || pipeline::answers::items_files(&cache, &date);
    eprintln!("items: the pass's answers {}", kept.sync(&answers(), &scratch.join("answers"))?.words());
    let dir = scratch.join("items-out");
    let mut c = std::process::Command::new("uv");
    c.current_dir(&dem).env("SCENIC_PARTS", serde_json::to_string(PARTS)?).env("SCENIC_PAGEVIEWS_STORE", out.root().join("sources/pageviews")).args(["run", "python", "items.py", "--qids"]).arg(&qfile).arg("--epoch").arg(&date).arg("--cache").arg(&cache).arg("--out").arg(&dir);
    let st = c.status().context("run items.py")?;
    // What it fetched, on the NAS whether or not it finished (stopped, at the next start).
    match kept.keep(&answers(), &scratch.join("answers")) {
        Ok(true) => eprintln!("items: the pass's answers sent to the NAS"),
        Ok(false) => {}
        Err(e) => eprintln!("items: the pass's answers not sent to the NAS now ({e:#}); the next start sends them"),
    }
    anyhow::ensure!(st.success(), "items.py failed: {st}");
    parts.start(4);
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
/// it (one clip per pass and cover, kept in the cache). The seeds' park facts and pageview months
/// seed the caches (`sources/registers/legacy-seeds`); the pageview months are the items job's
/// cache, the epoch's months; the layers' English names use the seeds' names table.
/// No stops & sights (the marks job's). Its outputs go to `work/heritage/<date>/<file>`. What the
/// chain fetched goes to the NAS as it ends, finished or not, or at the next start when it was
/// stopped (`Epoch`).
fn heritage_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::heritage::{base_logical, cover_tiles, tiles_geojson, COVER_Z};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
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
    stage(0, 5, "reading the registers' snapshot and the heritage sites");
    let epoch = heritage_epoch(out, &date, &cache, scratch)?;
    // As it ends, finished or failed (stopped by a signal, it doesn't run: the next start sends what
    // it fetched): the named places' export gone (made again each run, 3 GB), and what the chain
    // fetched kept on the NAS.
    let _end = OnEnd(|| {
        std::fs::remove_file(epoch.dir.join("osm/named.geojsonseq")).ok();
        epoch.keep(scratch);
    });
    let seeds = registers_extract(out, "sources/registers/legacy-seeds", &cache)?;
    let root = heritage_root(scratch, &dem, &epoch.dir)?;
    let b = root.join("data/build");
    // The heritage-sites job's outputs, as heritage.py left them.
    for stem in ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"] {
        let c = out.get(&base_logical(&date, stem)).with_context(|| format!("no {stem} for the pass of {date} (the heritage-sites step)"))?;
        store::sys::copy_data(out.path(c), b.join(format!("{stem}.json")))?;
    }
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
    let named = osmium_clip(&out.path(out.get(&pipeline::osmpass::set_name(&date, "named")).context("the pass's named set")?), &poly, &scratch.join("named-cover.osm.pbf"))?;
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
    osmium_run(c, "osmium tags-filter (named)")?;
    std::fs::create_dir_all(epoch.dir.join("osm"))?;
    let mut c = pipeline::osmpass::osmium();
    c.arg("export").arg(&named_today).args(["-f", "geojsonseq", "--overwrite", "-o"]).arg(epoch.dir.join("osm/named.geojsonseq"));
    osmium_quiet(c, "osmium export (named)")?;
    std::fs::remove_file(&named).ok();
    std::fs::remove_file(&named_today).ok();
    // The kept filtered planet within the cover, once per pass and cover (whsshapes.py's merged.osm.pbf).
    stage(4, 5, "clipping the pass's filtered planet to the coverage (osmium, once a pass)");
    let merged = merged_over_cover(out, &date, &poly, &cache)?;
    pipeline::sys::symlink(&merged, &root.join("data/osm/merged.osm.pbf"))?;
    // The seeds' park facts, seeding this pass's cache of them.
    let facts = epoch.dir.join("areas-wikidata.json");
    if !facts.exists() {
        store::sys::copy_data(seeds.join("areas/wikidata.json"), &facts)?;
    }
    pipeline::sys::symlink(&facts, &root.join("data/areas/wikidata.json"))?;
    // The pageview months: the items job's cache (the same files), the seeds' months seeding it.
    let pv = cache.join("items");
    std::fs::create_dir_all(pv.join("months"))?;
    for e in std::fs::read_dir(seeds.join("pageviews/months"))?.flatten() {
        // (Made whole under a name of its own, never over one here: store::cachefile.)
        let dest = pv.join("months").join(e.file_name());
        if !dest.exists() {
            store::cachefile::create(&dest, &mut |t| store::sys::copy_data(e.path(), t).map(|_| ()))?;
            store::cachefile::release(&dest);
        }
    }
    pipeline::sys::symlink(&pv, &root.join("data/pageviews"))?;
    // The seeds' names table, for the layers' English names.
    std::fs::create_dir_all(root.join("data/names"))?;
    pipeline::sys::symlink(&seeds.join("names/english.json"), &root.join("data/names/english.json"))?;
    for (k, (part, script, sargs)) in chain.iter().enumerate() {
        if k == 0 || chain[k - 1].0 != *part {
            parts.start(*part);
        }
        let mine: Vec<&str> = chain.iter().filter(|c| c.0 == *part).map(|c| c.1).collect();
        stage(mine.iter().position(|s| s == script).unwrap_or(0) as u64, mine.len() as u64 + (*part == 4) as u64, script);
        heritage_script(&root, &dem, out.root(), script, sargs)?;
    }
    stage(1, 2, "uploading");
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
        if inputs.contains(&name.as_str()) || name.ends_with(".tmp") || name.starts_with('.') || stem.starts_with("layer-pois") || stem == "layer-summits" {
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
        .filter(|l| !l[prefix.len()..].contains('/') && !wrote.contains(l))
        .collect();
    for l in stale {
        out.remove(&l);
    }
    out.save()?;
    eprintln!("heritage: {} files ({:.0?})", wrote.len(), t0.elapsed());
    Ok(())
}

/// Runs an osmium command, failing with its name; how far it is (its `--progress`) is said as the
/// stage's under way (pipeline::agent::jobs::within).
fn osmium_run(mut c: std::process::Command, what: &str) -> Result<()> {
    c.arg("--progress");
    let st = pipeline::agent::jobs::run_watched(&mut c, pipeline::agent::jobs::within).with_context(|| format!("run {what}"))?;
    anyhow::ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// Runs an osmium command that's quick after the one before in its stage (an export of what was
/// just clipped), failing with its name: its progress not said (a second bar would set the stage's
/// back).
fn osmium_quiet(mut c: std::process::Command, what: &str) -> Result<()> {
    let st = c.status().with_context(|| format!("run {what}"))?;
    anyhow::ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// `src` within the polygon file (whole relations: smart), into `dest`.
fn osmium_clip(src: &Path, poly: &Path, dest: &Path) -> Result<PathBuf> {
    let mut c = pipeline::osmpass::osmium();
    c.args(["extract", "--strategy", "smart", "--overwrite", "-p"]).arg(poly).arg(src).arg("-o").arg(dest);
    osmium_run(c, &format!("osmium extract ({})", src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))?;
    Ok(dest.to_path_buf())
}

/// The pass's protected areas and Indigenous lands within the cover, as heritage.py's areas.geojsonseq.
fn areas_over_cover(out: &Out, date: &str, poly: &Path, scratch: &Path, dest: &Path) -> Result<()> {
    let set = out.path(out.get(&pipeline::osmpass::set_name(date, "areas")).context("the pass's areas set")?);
    let clip = osmium_clip(&set, poly, &scratch.join("areas-cover.osm.pbf"))?;
    let mut c = pipeline::osmpass::osmium();
    c.args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "-a", "type,id", "--overwrite", "-o"]).arg(dest).arg(&clip);
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

/// registers-import --from <dir> [--name legacy]: a registers' snapshot (a folder laid out as
/// heritage.py reads it, without osm/, which the pass's sets replace: docs/plan.md §6, Hand-made
/// inputs) as one archive in the manifest,
/// `sources/registers/<name>` (tar.zst, files in sorted order): what the heritage jobs extract,
/// once per archive (thousands of small files are slow to copy over SMB one by one).
fn registers_import(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let from = std::fs::canonicalize(opt(args, "--from").context("--from <dir>")?)?;
    let name = opt(args, "--name").unwrap_or_else(|| "legacy".into());
    std::fs::create_dir_all(scratch)?;
    let (list, tar) = (scratch.join("registers.list"), scratch.join("registers.tar.zst"));
    let mut c = std::process::Command::new("sh");
    c.env("PATH", format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default()));
    c.arg("-c")
        .arg(r#"set -e; cd "$1"; find . -type f ! -path './osm/*' ! -name .DS_Store | LC_ALL=C sort > "$2"; tar -cf - -T "$2" | zstd -T0 -10 -q -f -o "$3""#)
        .arg("sh")
        .arg(&from)
        .arg(&list)
        .arg(&tar);
    let st = c.status().context("tar | zstd")?;
    anyhow::ensure!(st.success(), "archiving {} failed: {st}", from.display());
    let files = std::fs::read_to_string(&list)?.lines().count();
    let size = std::fs::metadata(&tar)?.len();
    let got = out.put_file(&format!("sources/registers/{name}"), "tar.zst", &tar)?;
    out.save()?;
    eprintln!("registers-import: {files} files, {} MB → {got}", size >> 20);
    Ok(())
}

/// This pass's working copy of the registers' snapshot (`dir`, a copy of `snap`), with the answers
/// the heritage chain fetched, which the NAS keeps too (`kept`).
struct Epoch {
    dir: PathBuf,
    snap: PathBuf,
    kept: pipeline::answers::Kept,
}

impl Epoch {
    /// What the chain fetched sent to the NAS (as a step ends, finished or not; a step stopped by a
    /// signal doesn't run this, and the next start sends it).
    fn keep(&self, scratch: &Path) {
        match self.kept.keep(&pipeline::answers::heritage_files(&self.dir, &self.snap), &scratch.join("answers")) {
            Ok(true) => eprintln!("heritage: the pass's answers sent to the NAS"),
            Ok(false) => {}
            Err(e) => eprintln!("heritage: the pass's answers not sent to the NAS now ({e:#}); the next start sends them"),
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

/// This pass's working copy of the registers' snapshot, which the heritage scripts add their caches
/// to: the archive extracted once (`cache/registers-<id>`), then cloned per pass
/// (`cache/heritage-<date>-<id>`, APFS clones cost nothing; a copy elsewhere keeps the files'
/// times), so a pass's runs share their caches and a new pass or snapshot starts again from the
/// snapshot. Other passes' and snapshots' copies go. What the chain fetched for the pass is on the
/// NAS too (pipeline::answers: `sources/items/<date>/heritage-<id>.tar.zst`), made one with this
/// Mac's here.
fn heritage_epoch(out: &Out, date: &str, cache: &Path, scratch: &Path) -> Result<Epoch> {
    let snap = registers_extract(out, "sources/registers/legacy", cache)?;
    let id = snap.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("registers-")).context("registers folder")?.to_string();
    let epoch = cache.join(format!("heritage-{date}-{id}"));
    if !epoch.join(".done").exists() {
        std::fs::remove_dir_all(&epoch).ok();
        let st = std::process::Command::new("cp").arg("-c").arg("-R").arg(&snap).arg(&epoch).status()?;
        if !st.success() {
            std::fs::remove_dir_all(&epoch).ok();
            let st = std::process::Command::new("cp").arg("-R").arg("-p").arg(&snap).arg(&epoch).status()?;
            anyhow::ensure!(st.success(), "copying {} failed: {st}", snap.display());
        }
        std::fs::write(epoch.join(".done"), b"")?;
    }
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let other_epoch = n.strip_prefix("heritage-").is_some_and(|r| r.len() > 11 && pipeline::osmpass::is_date(&r[..10])) && e.path() != epoch;
        // (heritage-data: the first heritage job's rsync'd copy; heritage-venv: the scripts' own
        // Python environment, before they ran in the app's.)
        if other_epoch || n == "heritage-data" || n == "heritage-venv" {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    let e = Epoch { kept: pipeline::answers::heritage(out.root(), &epoch, &snap, date, &id), dir: epoch, snap };
    eprintln!("heritage: the pass's answers {}", e.kept.sync(&pipeline::answers::heritage_files(&e.dir, &e.snap), &scratch.join("answers"))?.words());
    Ok(e)
}

/// An archive of `sources/registers/` extracted once into the cache (`registers-<id>`, by its
/// content), the current ones kept: the snapshot and the seeds.
fn registers_extract(out: &Out, logical: &str, cache: &Path) -> Result<PathBuf> {
    let content = out.get(logical).with_context(|| format!("no {logical} in the manifest (scenic-build registers-import)"))?.to_string();
    let id = store::naming::hash16(content.as_bytes())[..12].to_string();
    std::fs::create_dir_all(cache)?;
    let dir = cache.join(format!("registers-{id}"));
    if !dir.join(".done").exists() {
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir)?;
        let mut c = std::process::Command::new("sh");
        c.env("PATH", format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default()));
        c.arg("-c").arg(r#"set -e; zstd -dcq "$1" | tar -xf - -C "$2""#).arg("sh").arg(out.path(&content)).arg(&dir);
        let st = c.status().context("zstd | tar")?;
        anyhow::ensure!(st.success(), "extracting {content} failed: {st}");
        std::fs::write(dir.join(".done"), b"")?;
    }
    // Extracts of archives the manifest no longer names go.
    let current: std::collections::BTreeSet<String> = out
        .manifest
        .range("sources/registers/".to_string()..)
        .take_while(|(l, _)| l.starts_with("sources/registers/"))
        .map(|(_, c)| format!("registers-{}", &store::naming::hash16(c.as_bytes())[..12]))
        .collect();
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("registers-") && !current.contains(&n) {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    Ok(dir)
}

/// A stand-in root laid out as the repository for the heritage scripts: `dem/` the app's
/// scripts and their Python project, `data/heritage` this pass's copy of the registers' snapshot.
fn heritage_root(scratch: &Path, dem: &Path, epoch: &Path) -> Result<PathBuf> {
    let root = scratch.join("heritage-root");
    std::fs::remove_dir_all(&root).ok();
    for d in ["dem", "data/build", "data/areas", "data/osm"] {
        std::fs::create_dir_all(root.join(d))?;
    }
    for e in std::fs::read_dir(dem)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if e.path().is_file() && (n.ends_with(".py") || n == "pyproject.toml" || n == "uv.lock" || n == ".python-version") {
            store::sys::copy_data(e.path(), root.join("dem").join(&n))?;
        }
    }
    pipeline::sys::symlink(&epoch, &root.join("data/heritage"))?;
    Ok(root)
}

/// Runs one of the heritage scripts in a stand-in root, in the Python environment of the app's `dem`
/// (`dem/.venv`, which uv makes from the app's lock file the first time a step runs on a Mac, as
/// for every other Python step; the lock file as is).
fn heritage_script(root: &Path, dem: &Path, nas: &Path, script: &str, args: &[&str]) -> Result<()> {
    let t = std::time::Instant::now();
    let mut c = std::process::Command::new("uv");
    // (The pageview months' indexes the items job keeps, on the NAS: pageviews.py.)
    c.current_dir(root.join("dem")).env("UV_PROJECT_ENVIRONMENT", dem.join(".venv")).env("SCENIC_PAGEVIEWS_STORE", nas.join("sources/pageviews")).args(["run", "--frozen", "python", script]).args(args);
    let st = c.status().with_context(|| format!("run {script}"))?;
    anyhow::ensure!(st.success(), "{script} failed: {st}");
    eprintln!("heritage: {script} done ({:.0?})", t.elapsed());
    Ok(())
}

/// heritage-sites [--pass <date>] [--dem dir] [--cache dir]: the heritage sites and designated
/// areas the units read (pipeline::heritage): heritage.py in a stand-in root, over the
/// tiles within 20 km of the coverage, on the registers' snapshot and the pass's protected areas
/// (its `areas` set within those tiles, as heritage.py's areas.geojsonseq). Its outputs go to
/// `work/heritage/<date>/base/<file>`, and per z6 tile the sites' positions and the area polygons.
/// What heritage.py fetched goes to the NAS as it ends, finished or not, or at the next start when
/// it was stopped (`Epoch`).
fn heritage_sites_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::heritage::{base_logical, cover_tiles, put_slices, slice_areas, slice_sites, tiles_bytes, tiles_geojson, COVER_Z};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(scratch)?;
    let parts = Parts(&["Reading the registers' snapshot", "Clipping the protected areas to the coverage (osmium)", "Locating the registers' sites (heritage.py)", "Slicing them per area", "Uploading"]);
    parts.start(0);
    let epoch = heritage_epoch(out, &date, &cache, scratch)?;
    let _end = OnEnd(|| epoch.keep(scratch));
    let root = heritage_root(scratch, &dem, &epoch.dir)?;
    let b = root.join("data/build");
    // The cover: its tiles for heritage.py, as rectangles for osmium.
    let tiles = cover_tiles(&cov);
    std::fs::write(b.join("cover.idx"), tiles_bytes(&tiles))?;
    let poly = scratch.join("cover.geojson");
    std::fs::write(&poly, serde_json::to_vec(&tiles_geojson(COVER_Z, &tiles))?)?;
    eprintln!("heritage-sites: {} z{COVER_Z} tiles within 20 km of the coverage ({:.0?})", tiles.len(), t0.elapsed());
    // The pass's protected areas and Indigenous lands within them (whole relations: smart).
    parts.start(1);
    areas_over_cover(out, &date, &poly, scratch, &root.join("data/areas/areas.geojsonseq"))?;
    parts.start(2);
    heritage_script(&root, &dem, out.root(), "heritage.py", &["../data/build", "--tiles", "../data/build/cover.idx", "--zoom", &COVER_Z.to_string(), "--date", &date])?;
    parts.start(3);
    // The units' slices, then the whole files.
    let sites = slice_sites(&std::fs::read(b.join("heritage.json"))?)?;
    let areas = slice_areas(&std::fs::read_to_string(b.join("area-shapes.geojsonseq"))?)?;
    let (ns, na) = put_slices(out, &date, &sites, &areas)?;
    parts.start(4);
    for (stem, ext) in [("heritage", "json"), ("heritage-areas", "json"), ("special", "json"), ("indigenous", "json"), ("heritage-sources", "json"), ("area-shapes", "geojsonseq")] {
        out.put_file(&base_logical(&date, stem), ext, &b.join(format!("{stem}.{ext}")))?;
    }
    out.save()?;
    eprintln!("heritage-sites: {} sites' and {} areas' slices ({:.0?})", ns, na, t0.elapsed());
    Ok(())
}

/// The unit step's global-source layers: the manifest's, or a pilot's published catalog.
fn layers_source<'a>(out: &'a Out, pilot: &'a Option<(PathBuf, store::catalog::Catalog)>, blobs: Option<&'a store::blobs::Blobs>) -> pipeline::stage::Source<'a> {
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
    let regions = opt(args, "--regions").map(PathBuf::from).unwrap_or_else(|| out.root().join("inputs/regions"));
    let (recipes, bad) = pipeline::agent::recipes::load(&regions);
    for (f, e) in &bad {
        eprintln!("unit: skipping region {f}: {e}");
    }
    anyhow::ensure!(!recipes.is_empty(), "no regions in {}", regions.display());
    let outlines_file = out.get(&format!("sources/osm/{date}/outlines")).map(|n| out.path(n));
    let outlines = outlines_file.as_deref().map(pipeline::outlines::Outlines::open).transpose()?;
    let cov = pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines"))?;
    Ok((date, cov, recipes.len()))
}

/// The canopy files the canopy step reads for box `b` (w, s, e, n, degrees): each 10° square's
/// median, p95 and cover (scenic-metrics names them by the square's top and left).
fn canopy_files(b: [f64; 4]) -> Vec<String> {
    let (lefts, tops) = ((b[0] / 10.0).floor() as i32..=(b[2] / 10.0).floor() as i32, (b[1] / 10.0).ceil() as i32..=(b[3] / 10.0).ceil() as i32);
    tops.flat_map(|t| lefts.clone().map(move |l| (t * 10, l * 10))).flat_map(pipeline::unit::canopy_square_files).collect()
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
    let reach = pipeline::reach::Reaches::load(out.root(), &out.manifest, &date).map_err(|e| anyhow::anyhow!("the pass's reaches: {e:?}"))?.context("no reaches for the pass")?;
    let index = pipeline::buildtiles::Index::load(out)?.context("the roadside buildings aren't made")?;
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
    let carry = args.iter().any(|a| a == "--carry").then(|| -> Result<pipeline::scache::Carry> {
        let (from, to) = (out.root().join("cache/scenic-units").join(u.dash()), shared.join("scenic-units").join(u.dash()));
        std::fs::remove_dir_all(&to).ok();
        std::fs::create_dir_all(&to)?;
        for e in std::fs::read_dir(&from).with_context(|| format!("no kept results in {}", from.display()))?.flatten() {
            store::sys::copy_data(e.path(), to.join(e.file_name()))?;
        }
        Ok(pipeline::scache::Carry { dir: to })
    }).transpose()?;
    let bdir = dir.join(format!("{}-buildings", u.dash()));
    let n = pipeline::buildtiles::stage(out.root(), &index, u, reach.get(u), &bdir)?;
    eprintln!("unit-snap {}: buildings from {n} tiles", u.slash());
    let tools = Tools {
        bin: std::env::current_exe()?.parent().context("bin")?.to_path_buf(),
        dem: PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into())),
        cache: PathBuf::from(opt(args, "--cache").context("--cache <dir>")?),
        buildings: Some(bdir),
        moi_dtm: Some(out.root().join("inputs/moi-dtm")),
        sources: Some(out.root().join("sources")),
        shared: Some(shared),
        chm: None,
        stores_read_only: false,
        spacing_m: 8,
        snap: Some(dir.join("snap")),
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
    let rep = build_folder(u, &local, &folder, &cov, &pipeline::stage::Source::Manifest(out, Some(&blobs)), &tools, &heritage, carry.as_ref())?;
    eprintln!("unit-snap {}: {} of {} ways kept, {} owned, in {:.1?}; snapshots in {}", u.slash(), rep.kept_ways, rep.piece_ways, rep.owned, t.elapsed(), dir.join("snap").display());
    Ok(())
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
    use pipeline::unit::{prepare_folder, run_tail, Tools};
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
        moi_dtm: Some(out.root().join("inputs/moi-dtm")),
        sources: Some(out.root().join("sources")),
        shared: Some(out.root().join("cache")),
        chm: None,
        stores_read_only: false,
        spacing_m: 8,
        snap: None,
    };
    // The DEM cache's seed, where the units' elevations start from (once per Mac).
    pipeline::unit::dem_seed(out.root(), &tools.cache)?;
    // What units kept in this Mac's own cache: moved to the shared one (both Macs' units read it).
    if let Some(shared) = &tools.shared {
        match pipeline::unit::move_kept_to_shared(&tools.cache, shared) {
            Ok(0) => {}
            Ok(n) => eprintln!("unit: moved {n} kept results to the shared cache ({})", shared.display()),
            Err(e) => eprintln!("unit: moving kept results to the shared cache: {e:#}"),
        }
    }
    // The pass's heritage sites and designated areas (the heritage-sites step), for every unit's
    // flags.
    anyhow::ensure!(
        out.get(&pipeline::heritage::base_logical(&date, "heritage-sources")).is_some(),
        "no heritage sites for the pass of {date} (the heritage-sites step)"
    );
    // The units: as asked, else every unit whose piece meets the coverage.
    let pieces: serde_json::Value = serde_json::from_slice(&std::fs::read(out.path(out.get(&format!("sources/osm/{date}/pieces")).context("the pass's pieces list")?))?)?;
    let mut units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    // The pass's reaches: which units the coverage builds, and the roadside buildings' tiles each
    // reads (unless a folder of them is given), from the release's (the buildings step).
    let buildings = match tools.buildings {
        Some(_) => None,
        None => Some(pipeline::buildtiles::Index::load(out)?.context("the roadside buildings aren't made (the buildings step)")?),
    };
    let reach = if units.is_empty() || tools.buildings.is_none() {
        Some(pipeline::reach::Reaches::load(out.root(), &out.manifest, &date).map_err(|e| anyhow::anyhow!("the pass's reaches: {e:?}"))?.context("no reaches for the pass (the reach step)")?)
    } else {
        None
    };
    if units.is_empty() {
        let reach = reach.as_ref().context("the pass's reaches")?;
        for k in pieces["pieces"].as_object().context("pieces")?.keys() {
            let u = Unit::parse(k).context("unit")?;
            if pipeline::agent::build::builds(&cov, reach, u) {
                units.push(u);
            }
        }
    }
    eprintln!("unit: pass {date}, {regions} region(s), {} unit(s)", units.len());
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
    // Other workers, through the build Mac's coordinator (the agent's jobs there), and the units
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
        let t = std::time::Instant::now();
        if let Some(h) = ahead.take() {
            h.join().ok();
        }
        if let Some(&next) = units.get(k + 1) {
            let o: &Out = out;
            let b = pipeline::stage::tile_box_grown(next.z, next.x, next.y, pipeline::stage::MARGIN_KM);
            let packs = layers_source(o, &pilot, Some(&blobs)).pack_contents(b);
            let piece = o.get(&format!("sources/osm/{date}/pieces/{}", next.dash())).map(|c| (o.path(c), scratch.join(format!("piece-{}.osm.pbf", next.dash()))));
            // Its canopy squares' files, from the NAS's store into the cache the canopy step reads
            // (scenic-metrics: it takes what's there whole, else copies it itself).
            let squares: Vec<(PathBuf, PathBuf)> = match &tools.sources {
                Some(s) => canopy_files(b).into_iter().map(|n| (s.join("canopy").join(&n), tools.cache.join("chm10").join(&n))).collect(),
                None => Vec::new(),
            };
            let (root, blobs) = (o.root().to_path_buf(), blobs.clone());
            ahead = Some(std::thread::spawn(move || {
                // (Under this process's own temporary name, renamed whole: another job on this Mac
                // may be copying the same file.)
                let copy = |src: &Path, dst: &Path| {
                    if dst.exists() || !src.exists() {
                        return;
                    }
                    let tmp = pipeline::whole::tmp_name(dst);
                    let whole = || std::fs::metadata(src).ok().map(|m| m.len()) == std::fs::metadata(&tmp).ok().map(|m| m.len());
                    if dst.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && store::sys::copy_data(src, &tmp).is_ok() && whole() {
                        std::fs::rename(&tmp, dst).ok();
                    } else {
                        std::fs::remove_file(&tmp).ok();
                    }
                };
                if let Some((src, dst)) = &piece {
                    copy(src, dst);
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
        let mut laps = pipeline::unit::Laps::default();
        // (Copied ahead, while the unit before it built, unless it's the job's first.)
        if std::fs::metadata(&local_piece).map(|m| m.len()).ok() != std::fs::metadata(&piece).map(|m| m.len()).ok() {
            store::sys::copy_data(&piece, &local_piece).with_context(|| format!("copy {}", piece.display()))?;
        }
        laps.lap("piece copied from the NAS");
        // Its scenic results from its last run, kept in the shared cache (pipeline::scache::Carry).
        let carry = pipeline::scache::Carry { dir: tools.scenic_kept(u) };
        pipeline::unit::take_peak();
        let (rep, tools) = {
            let o: &Out = out;
            let heritage = |b: [f64; 4], d: &Path| pipeline::heritage::unit_inputs(o, &date, b, d);
            // Its roadside buildings: the folder given by hand, else the release's tiles near its
            // roads.
            let mut tools = tools.clone();
            if let Some(index) = &buildings {
                let n = pipeline::buildtiles::stage(o.root(), index, u, reach.as_ref().and_then(|r| r.get(u)), &bdir)?;
                eprintln!("unit {}: buildings from {n} tiles", u.slash());
                laps.lap("buildings staged");
                tools.buildings = Some(bdir.clone());
            }
            (prepare_folder(u, &local_piece, &dir, &cov, &layers_source(o, &pilot, Some(&blobs)), &tools, &heritage, Some(&carry))?, tools)
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
        laps.skip();
        let b = Built { u, dir: dir.clone(), bdir: bdir.clone(), rep, tools, carry, piece, t, laps, peak: pipeline::unit::take_peak(), anywhere: anywhere.to_vec() };
        let offered = match &offload {
            Some(o) if b.rep.kept_ways > 0 && out_now.len() < o.depth() => match o.offer(u, &b.dir, b.tools.buildings.as_deref(), &b.anywhere) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("unit {}: its last steps not offered ({e:#}); run here", u.slash());
                    None
                }
            },
            _ => None,
        };
        match offered {
            Some(task) => out_now.push_back((b, task)),
            None => {
                let mut b = b;
                if b.rep.kept_ways > 0 {
                    run_tail(&b.anywhere, &b.dir, &b.tools)?;
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
    laps: pipeline::unit::Laps,
    peak: u64,
    /// Its tail's steps any worker may run.
    anywhere: Vec<pipeline::unit::Run>,
}

/// A tail's time on the build Mac until it has timed its stages (seconds).
const TAIL_GUESS_S: f64 = 120.0;

/// Settles the units whose last steps are out with other workers (pipeline::offload), in order,
/// and commits them: with `wait` false only those a worker finished or failed; with it, each after
/// the patience its steps' time here and its worker's pace allow (pipeline::offload::Patience).
fn settle_tails(out: &mut Out, date: &str, offload: Option<&pipeline::offload::Offload>, waiting: &mut std::collections::VecDeque<(Built, pipeline::offload::Offered)>, wait: bool) -> Result<()> {
    let Some(o) = offload else { return Ok(()) };
    let mut i = 0;
    while i < waiting.len() {
        let (b, task) = &mut waiting[i];
        // (Its last steps run here, when they're taken back: its stages.)
        areas(|a| a.on(&b.u.slash()));
        let (dir, tools, anywhere) = (b.dir.clone(), b.tools.clone(), b.anywhere.clone());
        let mut here = || pipeline::unit::run_tail(&anywhere, &dir, &tools);
        // (This Mac's own run of its steps, as its stages have taken here: a worker is waited on
        // only while its pace says it'll be back sooner, and measured against it.)
        let mut here_s = None;
        areas(|a| here_s = a.here_s(&anywhere.iter().map(|r| r.what.as_str()).collect::<Vec<_>>()));
        let patience = pipeline::offload::Patience { here_s: Some(here_s.unwrap_or(TAIL_GUESS_S)) };
        match o.settle(task, &b.dir, wait, patience, &mut here)? {
            None => i += 1,
            Some(how) => {
                let (mut b, _) = waiting.remove(i).unwrap();
                b.peak = b.peak.max(pipeline::unit::take_peak());
                let how = match how {
                    pipeline::offload::Settled::Remote(w) => format!("by {w}"),
                    pipeline::offload::Settled::Here(None) => "here".into(),
                    pipeline::offload::Settled::Here(Some((w, true))) => format!("here, and by {w} the same"),
                    pipeline::offload::Settled::Here(Some((w, false))) => format!("here: {w}'s differed, and it gets no more work"),
                };
                commit_unit(out, date, b, &how)?;
            }
        }
    }
    Ok(())
}

/// A unit's last part, its folder ready: the grids its packs lacked, its scenic results kept for
/// its next run, its base pack, road values and roads' English saved, its folders removed, and what
/// it cost noted. `how`: where its tail's last steps ran. (What it saves is
/// `pipeline::unit::saved_files`, all a helper's hand-off may change: keep the two together.)
fn commit_unit(out: &mut Out, date: &str, b: Built, how: &str) -> Result<()> {
    let Built { u, dir, bdir, rep, tools, carry, piece, t, mut laps, peak, .. } = b;
    use pipeline::unit::owns;
    areas(|a| a.on(&u.slash()));
    laps.skip();
    // The DEM samples its elevations made (its last steps' first, here or by a worker), kept.
    if rep.kept_ways > 0 {
        pipeline::unit::keep_dem_samples(u, &dir, &tools);
        laps.lap("DEM samples kept");
    }
    let clean = || {
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&bdir).ok();
    };
    // Grids its packs lacked (new coverage), made in the folder: the unit's own z6 tile's go up,
    // for later units and packs. (The canopy step made canopy and cover for every tile.)
    for var in ["class", "canopy", "cover"] {
        if rep.staged.missing.get(var).copied().unwrap_or(0) == 0 {
            continue;
        }
        let mut tiles = pipeline::stage::grid_tiles_in(&dir, var, u.x, u.y)?.into_iter();
        layers::write_pack(out, &format!("grid-{var}"), "u8-zstd", false, "hi", (6, u.x, u.y), &mut tiles)?;
    }
    laps.lap("missing grids written");
    // Its scenic results, for its next run. A cache: not keeping them only costs time later.
    if let Err(e) = carry.save(&dir) {
        eprintln!("unit {}: its scenic results not kept: {e:#}", u.slash());
    }
    laps.lap("scenic results kept");
    eprintln!("unit {}: {} of {} ways touch the coverage, {} owned; {} heritage sites, {} area polygons; its last steps run {how}", u.slash(), rep.kept_ways, rep.piece_ways, rep.owned, rep.heritage, rep.areas);
    if rep.kept_ways == 0 || rep.owned == 0 {
        // None of its ways in the coverage (any more): a base pack and road values from an
        // earlier coverage go, so the map and the map tiles stop showing them.
        let gone: Vec<String> = [format!("base/{}", u.dash()), format!("global/roads/{}", u.dash()), format!("global/roaden/{}", u.dash())].into_iter().filter(|l| out.get(l).is_some()).collect();
        if !gone.is_empty() {
            for l in &gone {
                out.remove(l);
            }
            eprintln!("unit {}: removed its earlier {}", u.slash(), gone.join(" and "));
        }
        // (Saved before it's noted done: its grids above too.)
        out.save()?;
        clean();
        pipeline::control::done("unit", &u.slash());
        areas(|a| a.finished(&u.slash()));
        return Ok(());
    }
    // The owned ways, in base-pack order, with the pass's road values.
    let lg = Legacy::open(&dir)?;
    let tb = tile_bounds(u.z, u.x, u.y);
    let idx: Vec<u32> = lg.units().remove(&u).unwrap_or_default().into_iter().filter(|&i| owns(tb, lg.first_vertex(&lg.ways.ways()[i as usize]))).collect();
    let built = format!("pass:{date}");
    let bs = legacy::base_sections(&lg, u, &idx, &built);
    laps.lap("base pack made");
    let secs: Vec<(&str, &[u8])> = bs.sections.iter().map(|(n, v)| (*n, v.as_slice())).collect();
    put_sect(out, &format!("base/{}", u.dash()), bs.meta, &secs)?;
    laps.lap("base pack written to the NAS");
    let vals = pass_roads(out, date, u)?;
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
                    let len: f64 = vs.windows(2).map(|p| roadcore::dist_m(p[0][0] as f64 * 1e-7, p[0][1] as f64 * 1e-7, p[1][0] as f64 * 1e-7, p[1][1] as f64 * 1e-7)).sum();
                    pipeline::legacy::RoadRec { road: w.id as u64, len: len as f32, offset: 0.0, dir: 0, _pad: [0; 7] }
                }
            }
        })
        .collect();
    put_roads(out, u, &recs)?;
    laps.lap("road values made and written");
    // The roads' own English (OSM's name:en where it isn't the name), for the server to show
    // with them.
    let en: BTreeMap<String, String> = std::fs::read(dir.join("name-en.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mine: BTreeMap<String, String> = idx.iter().filter_map(|&i| en.get(&ways[i as usize].id.to_string()).map(|e| (ways[i as usize].id.to_string(), e.clone()))).collect();
    let logical = format!("global/roaden/{}", u.dash());
    if !mine.is_empty() {
        out.put_bytes(&logical, "json", &serde_json::to_vec(&mine)?)?;
    } else if out.get(&logical).is_some() {
        out.remove(&logical);
    }
    out.save()?;
    pipeline::control::done("unit", &u.slash());
    laps.lap("records saved");
    areas(|a| a.finished(&u.slash()));
    drop(lg);
    clean();
    laps.lap("its folders removed");
    // The most memory one of its steps' programs took here, against its piece's size; noted for
    // the coordinator (`SCENIC_COSTS`), which gives a worker only units that fit its memory.
    let piece_mb = std::fs::metadata(&piece).map(|m| m.len() >> 20).unwrap_or(0);
    let peak = peak.max(pipeline::unit::take_peak());
    eprintln!("unit {}: base pack of {} ways in {:.0?}; piece {piece_mb} MB, its steps' programs' peak memory {:.1} GB", u.slash(), idx.len(), t.elapsed(), peak as f64 / 1e9);
    if let Some(p) = std::env::var_os("SCENIC_COSTS") {
        let line = serde_json::json!({ "unit": u.slash(), "peak_mb": peak >> 20, "secs": t.elapsed().as_secs() });
        let r = std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
        if let Err(e) = r {
            eprintln!("unit {}: noting what it cost: {e}", u.slash());
        }
    }
    Ok(())
}

/// Drops the prune targets' entries from the manifest (agent::build's prune works): the next
/// catalog leaves them out, and GC frees their files once no catalog of the last 14 days lists them.
fn prune_step(out: &mut Out, args: &[String]) -> Result<()> {
    let mut gone = 0;
    for t in positional(args) {
        let (kind, at) = t.split_once(' ').with_context(|| format!("{t:?}: a prune target is \"<kind> <z/x/y>\""))?;
        let d = Unit::parse(at).with_context(|| format!("{t:?}: not a tile"))?.dash();
        let logicals = match kind {
            "unit" => vec![format!("base/{d}"), format!("global/roads/{d}"), format!("global/roaden/{d}")],
            "pois" => vec![format!("work/pois/{d}"), format!("work/peaks/{d}")],
            "pack" => vec![format!("hidata/{d}"), format!("layers/roads/hi/{d}"), format!("layers/rails/hi/{d}")],
            "lo" => vec![format!("layers/roads/lo/{d}"), format!("layers/rails/lo/{d}")],
            "bldprep" => vec![format!("work/bld/{d}")],
            "bldtiles" => vec![format!("layers/{}/hi/{d}", pipeline::bld::LAYER)],
            k => bail!("{t:?}: unknown prune target {k:?}"),
        };
        for l in logicals {
            if out.get(&l).is_some() {
                out.remove(&l);
                gone += 1;
            }
        }
    }
    out.save()?;
    eprintln!("prune: {gone} entries dropped from the manifest");
    Ok(())
}

/// Every unit's reach (pipeline::reach) from its piece, as `sources/osm/<date>/reach`: each piece
/// copied here and read twice (its ways, then their nodes).
fn reach_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::reach::{logical, of_piece, Reaches};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let list = out.get(&format!("sources/osm/{date}/pieces")).context("the pass's pieces list")?.to_string();
    let pieces: pipeline::osmpass::Pieces = serde_json::from_slice(&std::fs::read(out.path(&list))?)?;
    std::fs::create_dir_all(scratch)?;
    let local = scratch.join("reach-piece.osm.pbf");
    let mut all = Reaches { fmt: 1, date: date.clone(), ..Default::default() };
    let only: Vec<String> = positional(args).iter().filter_map(|s| Unit::parse(s)).map(|u| u.slash()).collect();
    let todo: Vec<(&String, &String)> = pieces.pieces.iter().filter(|(u, _)| only.is_empty() || only.contains(u)).collect();
    let n = todo.len() as u64;
    let t0 = std::time::Instant::now();
    for (k, (u, l)) in todo.into_iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, n, "areas");
        let unit = Unit::parse(u).with_context(|| format!("unit {u}"))?;
        let src = out.path(out.get(l).with_context(|| format!("{l} isn't in the manifest"))?);
        store::sys::copy_data(&src, &local).with_context(|| format!("copy {}", src.display()))?;
        let t = std::time::Instant::now();
        if let Some(r) = of_piece(&local, unit).with_context(|| format!("piece {u}"))? {
            if !only.is_empty() {
                let (owned, verts) = (r.long.iter().filter(|w| w.owned).count(), r.long.iter().map(|w| w.verts.len()).sum::<usize>());
                eprintln!("reach {u}: owned box {:?}, {} long ways ({owned} owned, {verts} vertices), extent {:?} ({:.1?}, {} MB)", r.owned, r.long.len(), r.extent(unit), t.elapsed(), std::fs::metadata(&local).map(|m| m.len() >> 20).unwrap_or(0));
            }
            all.units.insert(unit.slash(), r);
        }
    }
    std::fs::remove_file(&local).ok();
    if !only.is_empty() {
        return Ok(());
    }
    out.put_bytes(&logical(&date), "json.zst", &all.encode()?)?;
    out.save()?;
    eprintln!("reach: {} units with roads of {} pieces ({:.0?})", all.units.len(), n, t0.elapsed());
    Ok(())
}

/// The road → units index from every unit's road values in the manifest.
fn roadunits(out: &mut Out) -> Result<()> {
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    let units: Vec<(Unit, String)> = out.manifest.iter().filter_map(|(k, v)| k.strip_prefix("global/roads/").and_then(Unit::parse).map(|u| (u, v.clone()))).collect();
    for (k, (u, content)) in units.iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas' road values read");
        let r = store::sect::SectReader::open(store::range::PlainFile::open(&out.path(content))?)?;
        let recs: Vec<pipeline::legacy::RoadRec> = r.read_pod("roads")?;
        pairs.extend(recs.iter().map(|x| (x.road, u.key())));
    }
    pairs.sort_unstable();
    pairs.dedup();
    let flat: Vec<u64> = pairs.iter().flat_map(|&(r, u)| [r, u]).collect();
    put_sect(out, "global/roadunits", serde_json::json!({"fmt": 1, "pairs": pairs.len()}), &[("pairs", b(&flat))])?;
    eprintln!("roadunits: {} roads×units over {} units", pairs.len(), units.len());
    Ok(())
}

/// The coverage of the regions (recipes in --regions, else inputs/regions; outlines from --pass,
/// else the latest complete pass).
fn coverage_of(out: &Out, args: &[String]) -> Result<pipeline::coverage::Coverage> {
    let regions = opt(args, "--regions").map(PathBuf::from).unwrap_or_else(|| out.root().join("inputs/regions"));
    let (recipes, bad) = pipeline::agent::recipes::load(&regions);
    for (f, e) in &bad {
        eprintln!("skipping region {f}: {e}");
    }
    anyhow::ensure!(!recipes.is_empty(), "no regions in {}", regions.display());
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root()));
    let outlines = date
        .as_deref()
        .and_then(|d| out.get(&format!("sources/osm/{d}/outlines")).map(|n| out.path(n)))
        .map(|p| pipeline::outlines::Outlines::open(&p))
        .transpose()?;
    pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines"))
}

/// The z6 tiles named (`6/x/y`), at least one.
fn z6_tiles(args: &[String], step: &str) -> Result<Vec<Unit>> {
    let ts: Vec<Unit> = positional(args).iter().map(|t| Unit::parse(t).filter(|u| u.z == 6 && u.x < 64 && u.y < 64).with_context(|| format!("not a z6 tile: {t}"))).collect::<Result<_>>()?;
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
    let st = std::process::Command::new("uv")
        .current_dir(&dem)
        .args(["run", "python", "bldfetch.py", "--root"])
        .arg(out.root())
        .args(["--release", pipeline::buildtiles::RELEASE, "--coverage"])
        .arg(&cover)
        .status()
        .context("run bldfetch.py")?;
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
        let c = cost_start();
        let st = pipeline::bld::prep::run(out, t, &dem, pipeline::buildtiles::RELEASE)?;
        eprintln!("bldprep {}: {}", t.slash(), serde_json::to_string(&st)?);
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
    // Other workers, through the build Mac's coordinator (the agent's jobs there): some of a tile's
    // z8 areas offered as tasks while it builds the others.
    let offload = pipeline::offload::Offload::from_env(&out.scratch);
    for (k, &t) in ts.iter().enumerate() {
        pipeline::control::safe_point("bldtiles");
        pipeline::agent::jobs::part(k, &names);
        let c = cost_start();
        let sum = pipeline::bld::job::build(out, &cov, t, offload.as_ref())?;
        eprintln!("bldtiles {}: {}", t.slash(), serde_json::to_string(&sum)?);
        pipeline::control::done("bldtiles", &t.slash());
        note_cost("bldtiles", &t.slash(), c);
    }
    Ok(())
}

/// treeblock-task's work: row `row`'s task folder in `dir/task` (cut from its piece's coverage as
/// the piece's run cuts it), `dir/task.json` (its blocks and the canopy squares the NAS's store
/// has whole there), and `dir/packs/8-x-y/` its blocks' tiles from the manifest's hi packs (zoom
/// 9–12) and its piece's mid (zoom 8, the values), written as the trees program writes a block.
fn treeblock_task(out: &Out, cov: &pipeline::coverage::Coverage, row: &[(u32, u32)], dir: &Path) -> Result<()> {
    use pipeline::trees::{self, task};
    anyhow::ensure!(!row.is_empty() && row.iter().all(|b| b.1 == row[0].1 && (b.0 >> 2, b.1 >> 2) == (row[0].0 >> 2, row[0].1 >> 2)), "a row: blocks of one z6 tile and one row");
    let piece = Unit { z: 6, x: row[0].0 >> 2, y: row[0].1 >> 2 };
    std::fs::remove_dir_all(dir).ok();
    let cj = serde_json::to_string(&pipeline::treepacks::coverage_json(cov, piece))?;
    task::cut(&cj, row, &dir.join("task"))?;
    let store = out.root().join("sources/canopy");
    let whole = |top: i32, left: i32| ["cover5m", "p95"].iter().all(|k| std::fs::metadata(store.join(trees::chm_name(top, left, k))).is_ok_and(|m| m.len() > 0));
    let mut squares: Vec<(i32, i32)> = row.iter().flat_map(|&(x, y)| trees::squares_of(trees::tile_bounds(trees::ZBLOCK, x, y))).filter(|&(t, l)| whole(t, l)).collect();
    squares.sort_unstable();
    squares.dedup();
    std::fs::write(dir.join("task.json"), serde_json::to_vec(&serde_json::json!({ "blocks": task::blocks_arg(row), "squares": task::squares_arg(&squares), "piece": piece.slash() }))?)?;
    // The NAS's tiles of each block.
    let mid = out.get(&pipeline::treepacks::mid_logical(piece.x, piece.y)).with_context(|| format!("{} has no mid", piece.slash()))?;
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
    println!("{}", serde_json::json!({ "piece": piece.slash(), "blocks": task::blocks_arg(row), "squares": task::squares_arg(&squares), "coverage_bytes": std::fs::metadata(dir.join("task/u").join(task::COVERAGE))?.len() }));
    Ok(())
}

/// bldtile-task <8/x/y | 6/x/y> --out dir [--pass d] [--regions dir]: a z8 area's task folder, as a
/// `bldtiles` job cuts it for a worker (pipeline::bld::task::cut; for the checks: the `bldtile`
/// program then runs over it). A z6 tile: its area with the most buildings.
fn bldtile_task_step(out: &mut Out, args: &[String]) -> Result<()> {
    let u = positional(args).first().and_then(|s| Unit::parse(s)).filter(|u| u.z == 6 || u.z == 8).context("bldtile-task <8/x/y | 6/x/y> --out dir")?;
    let dir = PathBuf::from(opt(args, "--out").context("--out dir")?);
    let cov = coverage_of(out, args)?;
    let t = pipeline::bld::task::tile_of(Unit { z: 8, x: if u.z == 6 { u.x << 2 } else { u.x }, y: if u.z == 6 { u.y << 2 } else { u.y } });
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
    let c = pipeline::bld::task::cut(&files, &cov, t, a, &dir)?;
    println!("{}", serde_json::json!({ "area": format!("8/{}/{}", a.0, a.1), "bytes": c.bytes, "records": c.records, "mem_mb": c.mem_mb(), "secs": t0.elapsed().as_secs_f64() }));
    Ok(())
}

/// The z6 tiles a terrain or slope run makes, by z3 pack: those near the coverage (as the agent's
/// keys list them, `build::coverage_tiles`) of each z3 pack named (`3/x/y`, as the agent asks), or
/// the z6 tiles named, or with none named every z6 tile near the coverage.
fn terrain_targets(cov: &pipeline::coverage::Coverage, args: &[String]) -> Result<BTreeMap<(u32, u32), Vec<(u32, u32)>>> {
    let near = pipeline::agent::build::coverage_tiles(cov);
    let named: Vec<Unit> = positional(args).iter().map(|s| Unit::parse(s).with_context(|| format!("not a tile: {s}"))).collect::<Result<_>>()?;
    if named.is_empty() {
        return Ok(near);
    }
    let mut by_q: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for t in named {
        match t.z {
            3 => {
                let list = near.get(&(t.x, t.y)).with_context(|| format!("3/{}/{}: no tile of it is near the coverage", t.x, t.y))?;
                by_q.entry((t.x, t.y)).or_default().extend(list);
            }
            6 => by_q.entry((t.x >> 3, t.y >> 3)).or_default().push((t.x, t.y)),
            z => anyhow::bail!("{z}/{}/{}: terrain and slope take z3 packs or z6 tiles", t.x, t.y),
        }
    }
    for list in by_q.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    Ok(by_q)
}

fn terrain_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let by_q = terrain_targets(&cov, args)?;
    eprintln!("terrain: {} z6 tiles in {} z3 packs", by_q.values().map(Vec::len).sum::<usize>(), by_q.len());
    // AWS's raw tiles: this Mac's cache, filled from the NAS's store.
    let raw_dir = PathBuf::from(opt(args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
    let raw = raw_tiles(out, &raw_dir);
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
        let t = cost_start();
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
        let t = cost_start();
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

/// rail-feeds [--pass <date>] [--dem dir]: the rail feeds for the coverage (pipeline::rail):
/// dem/railfeeds.py on the rail sources (the catalogue, the feeds checked so far, the NAS's zips)
/// with the countries the coverage is in (from the pass's outlines), each feed fetched once, as
/// `sources/rail/feeds`. What it checked and fetched is kept even when it fails (a feed's server
/// that doesn't answer, an upload that fails), so the next try starts from there.
fn rail_feeds_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::rail;
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let catalogue = out.path(out.get(rail::CATALOGUE).context("no rail catalogue (put by hand: docs/plan.md, Hand-made inputs)")?);
    let parts = Parts(&["Finding the countries the coverage is in", "Finding and fetching the feeds (railfeeds.py)", "Uploading"]);
    parts.start(0);
    let outlines = pipeline::outlines::Outlines::open(&out.path(out.get(&format!("sources/osm/{date}/outlines")).context("the pass's outlines")?))?;
    let countries = rail::countries(&cov, &outlines)?;
    eprintln!("rail-feeds: the coverage is in {}", countries.join(", "));
    std::fs::create_dir_all(scratch)?;
    let (cover, checked, cache) = (scratch.join("coverage.geojson"), scratch.join("checked-in.json"), scratch.join("cache.json"));
    std::fs::write(&cover, serde_json::to_vec(&rail::coverage_geojson(&cov))?)?;
    match out.get(rail::CHECKED) {
        Some(c) => store::sys::copy_data(out.path(c), &checked).map(|_| ())?,
        None => std::fs::write(&checked, b"[]")?,
    }
    std::fs::write(&cache, serde_json::to_vec(&rail::cache_index(out, &rail::read_fetched(out)?))?)?;
    // (Its downloads stay in `found` until they're on the NAS: a run cut short keeps them. So does
    // railfeeds.py's record of the servers that haven't answered, until the job completes.)
    let found = scratch.join("found");
    parts.start(1);
    let st = std::process::Command::new("uv")
        .current_dir(&dem)
        .args(["run", "python", "railfeeds.py", "--catalogue"])
        .arg(&catalogue)
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
        .arg(&found)
        .status()
        .context("run railfeeds.py")?;
    parts.start(2);
    if found.join("checked.json").exists() {
        out.put_file(rail::CHECKED, "json", &found.join("checked.json"))?;
        out.save()?;
    }
    if !st.success() {
        let n = rail::keep_downloads(out, &found.join("gtfs"))?;
        out.save()?;
        bail!("railfeeds.py failed ({st}); the {n} zips it fetched are kept");
    }
    let new = rail::put_feeds(out, &found.join("feeds.json"), &found.join("gtfs"))?;
    out.save()?;
    std::fs::remove_dir_all(&found).ok();
    for f in [cover, checked, cache] {
        std::fs::remove_file(f).ok();
    }
    eprintln!("rail-feeds: {new} zips fetched");
    Ok(())
}

/// rail [--pass <date>] [--dem dir] [--cache dir]: trains a day on the coverage's rail ways
/// (pipeline::rail), as `global/railfreq`. The feeds' stop pairs (dem/railgtfs.py, kept in the
/// cache for their list of feeds, which doesn't depend on the coverage) and the MTR's, their stops
/// beyond the coverage marked, matched by `railfreq` onto the rail ways of the pass's rail set that
/// touch the coverage: the set clipped to the tiles within 20 km of it (as the heritage jobs' cover),
/// then `extract` (8 m, as the units), then the ways touching it. The feeds' days and trips go to
/// `work/rail/used`.
fn rail_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::rail;
    let t0 = std::time::Instant::now();
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned())).join("rail");
    let bin = std::env::current_exe()?.parent().context("bin")?.to_path_buf();
    let feeds = out.get(rail::FEEDS).context("no rail feeds yet (the rail-feeds step)")?.to_string();
    let set = out.path(out.get(&pipeline::osmpass::set_name(&date, "rail")).context("the pass has no rail set")?);
    std::fs::create_dir_all(scratch)?;
    // 1. The feeds' stop pairs, once per list of feeds (and the step's version).
    let parts = Parts(&["Reading the feeds' trains (railgtfs.py)", "Adding the stops beyond the coverage", "Clipping the pass's rail to the coverage (osmium)", "Extracting its rail ways", "Matching the trains onto the tracks (railfreq)"]);
    parts.start(0);
    let raw = cache.join(format!("pairs-{}.bin", store::naming::hash16(format!("{} {feeds}", pipeline::agent::build::RAIL_V).as_bytes())));
    if !raw.exists() {
        let (list, pairs, used) = (scratch.join("feeds.json"), scratch.join("pairs-raw.bin"), scratch.join("used.json"));
        std::fs::write(&list, serde_json::to_vec(&rail::feeds_for_counting(out)?)?)?;
        let st = std::process::Command::new("uv")
            .current_dir(&dem)
            .args(["run", "python", "railgtfs.py", "--feeds"])
            .arg(&list)
            .arg("--out")
            .arg(&pairs)
            .arg("--used")
            .arg(&used)
            .status()
            .context("run railgtfs.py")?;
        anyhow::ensure!(st.success(), "railgtfs.py failed: {st}");
        // Only the current list's pairs are kept.
        std::fs::create_dir_all(&cache)?;
        for e in std::fs::read_dir(&cache)?.flatten() {
            if e.file_name().to_string_lossy().starts_with("pairs-") {
                std::fs::remove_file(e.path()).ok();
            }
        }
        // (Where it can't be moved, copied by a temporary name: a copy cut short is never taken for
        // the pairs.)
        if std::fs::rename(&pairs, &raw).is_err() {
            let tmp = raw.with_extension("bin.tmp");
            store::sys::copy_data(&pairs, &tmp)?;
            std::fs::rename(&tmp, &raw)?;
            std::fs::remove_file(&pairs).ok();
        }
        out.put_file("work/rail/used", "json", &used)?;
        out.save()?;
    }
    // 2. Stops beyond the coverage.
    parts.start(1);
    let (pairs, beyond) = rail::mark_beyond(&std::fs::read(&raw)?, &cov);
    let pairs_file = scratch.join("pairs.bin");
    std::fs::write(&pairs_file, &pairs)?;
    let mut inputs = vec![pairs_file.clone()];
    if let Some(c) = out.get(rail::MTR_PAIRS) {
        let (mtr, _) = rail::mark_beyond(&std::fs::read(out.path(c))?, &cov);
        let f = scratch.join("pairs-mtr.bin");
        std::fs::write(&f, &mtr)?;
        inputs.push(f);
    }
    eprintln!("rail: {} stop pairs from the feeds, {beyond} with a stop beyond the coverage ({:.0?})", pairs.len() / rail::PAIR, t0.elapsed());
    // 3. The rail ways touching the coverage.
    parts.start(2);
    let local = scratch.join("rail-set.osm.pbf");
    store::sys::copy_data(&set, &local).with_context(|| format!("copy {}", set.display()))?;
    let poly = scratch.join("cover.geojson");
    std::fs::write(&poly, serde_json::to_vec(&pipeline::heritage::tiles_geojson(pipeline::heritage::COVER_Z, &pipeline::heritage::cover_tiles(&cov)))?)?;
    let clip = scratch.join("rail-cover.osm.pbf");
    let mut c = pipeline::osmpass::osmium();
    c.args(["extract", "--strategy", "complete_ways", "--overwrite", "-p"]).arg(&poly).arg(&local).arg("-o").arg(&clip);
    osmium_run(c, "osmium extract (the rail set over the coverage)")?;
    std::fs::remove_file(&local).ok();
    parts.start(3);
    let dir = scratch.join("ways");
    std::fs::remove_dir_all(&dir).ok();
    let st = std::process::Command::new(bin.join("extract")).arg(&dir).arg("8").arg(&clip).status().context("run extract")?;
    anyhow::ensure!(st.success(), "extract failed: {st}");
    std::fs::remove_file(&clip).ok();
    let (kept, _) = pipeline::unit::subset(&dir, |_, vs| cov.touches(vs))?;
    eprintln!("rail: {kept} ways touch the coverage ({:.0?})", t0.elapsed());
    // 4. The trains on them.
    parts.start(4);
    let st = std::process::Command::new(bin.join("railfreq")).arg(&dir).args(&inputs).status().context("run railfreq")?;
    anyhow::ensure!(st.success(), "railfreq failed: {st}");
    let ways = roadcore::Ways::open(&dir)?;
    let freq = rail::by_way_id(ways.ways(), &std::fs::read(dir.join("rail-freq.bin"))?);
    out.put_bytes(rail::RAILFREQ, "bin", &freq)?;
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
    let set = out.get(&pipeline::osmpass::set_name(&date, "labels")).context("the pass has no labels set (passes before 2026-10-02)")?.to_string();
    let work = scratch.join("labels");
    std::fs::create_dir_all(&work)?;
    let local = work.join("labels-set.osm.pbf");
    let parts = Parts(&["Copying the labels set", "Ranking the labels (labels.py)", "Cutting them into packs"]);
    parts.start(0);
    store::sys::copy_data(out.path(&set), &local).with_context(|| format!("copy {set}"))?;
    parts.start(1);
    let tiles = work.join("labels.tiles");
    let dem = PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into()));
    let st = std::process::Command::new("uv")
        .current_dir(&dem)
        .args(["run", "python", "labels.py", "--src"])
        .arg(&local)
        .arg("--out")
        .arg(&tiles)
        .arg("--work")
        .arg(&work)
        .status()
        .context("run labels.py")?;
    anyhow::ensure!(st.success(), "labels.py failed: {st}");
    parts.start(2);
    let arc = roadcore::archive::Archive::open(&tiles)?;
    let lo = layers::split_archive(out, &arc, "labels", "mvt", true, 14)?;
    eprintln!("labels: root {:?}, {} lo, {} hi packs", lo.root.is_some(), lo.lo.len(), lo.hi.len());
    // Packs of an earlier labels layer that this one doesn't have go from the manifest.
    let keep: std::collections::BTreeSet<String> = lo.root.iter().chain(lo.lo.values()).chain(lo.hi.values()).cloned().collect();
    let gone: Vec<String> = out.manifest.keys().filter(|k| k.starts_with("layers/labels/") && !keep.contains(*k)).cloned().collect();
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

/// The water layer (pipeline::water): each pixel's share of sea and of inland water, z0–9, drawn
/// from the pass's basemap's z14 water, as the water layer's packs. `--basemap f`: a local archive
/// instead of the manifest's; `--only x/y,…`: those z5 tiles alone; `--archive f`: the tiles into
/// that local archive, nothing written to the NAS (a regional run, measured).
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
        o.split(',').filter_map(|t| {
            let (x, y) = t.split_once('/')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        }).collect()
    });
    // (A part of the world isn't a layer: its packs would stand in for the whole one's, the rest of
    // the world and z0–4 gone.)
    anyhow::ensure!(only.is_none() || opt(args, "--archive").is_some(), "water: --only makes part of the layer: give --archive too (nothing written to the NAS)");
    let parts = Parts(&["Reading the basemap's z14 directory", "Drawing the water", "Cutting it into packs"]);
    parts.start(0);
    let pm = store::pmtiles::PmTiles::open(Box::new(store::range::PlainFile::open(&basemap).with_context(|| format!("open {}", basemap.display()))?))?;
    let z14 = wt::Z14::read(&pm)?;
    eprintln!("water: the z14 directory read in {:.0} s", t.elapsed().as_secs_f64());
    parts.start(1);
    let (tiles, made) = wt::build(&pm, &z14, only.as_deref(), &|d, n| pipeline::agent::jobs::report(d, n, "z5 tiles drawn"))?;
    drop(z14);
    eprintln!("water: {} in {:.0} s", serde_json::to_string(&made)?, t.elapsed().as_secs_f64());
    // (The archive in a folder of its own, removed however the step ends.)
    let work = WorkDir::new(scratch.join("water"))?;
    let local = match opt(args, "--archive") {
        Some(f) => PathBuf::from(f),
        None => work.0.join("water.tiles"),
    };
    let mut w = roadcore::archive::ArchiveWriter::create(&local, "{}")?;
    for (z, x, y, png) in &tiles {
        w.add(*z, *x, *y, png, png.len())?;
    }
    w.finish()?;
    drop(tiles);
    if opt(args, "--archive").is_some() {
        return Ok(());
    }
    parts.start(2);
    let arc = roadcore::archive::Archive::open(&local)?;
    let lo = layers::split_archive(out, &arc, wt::LAYER, "water-png", false, wt::STORED_MAXZ)?;
    eprintln!("water: root {:?}, {} lo, {} hi packs", lo.root.is_some(), lo.lo.len(), lo.hi.len());
    // Packs of an earlier water layer that this one doesn't have go from the manifest, and the small
    // islands and lakes' layer it replaces (`layers/smallwater/`, the dots of before).
    let keep: BTreeSet<String> = lo.root.iter().chain(lo.lo.values()).chain(lo.hi.values()).cloned().collect();
    let prefix = format!("layers/{}/", wt::LAYER);
    let gone: Vec<String> = out.manifest.keys().filter(|k| (k.starts_with(&prefix) && !keep.contains(*k)) || k.starts_with("layers/smallwater/")).cloned().collect();
    for k in gone {
        out.remove(&k);
    }
    out.save()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// A pack or lo job's base packs another such job uses (it holds them: store::cachefile) aren't
    /// pruned from under it: once, prune_cache deleted what the other read next.
    #[test]
    fn a_base_pack_another_job_uses_is_never_pruned() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("base");
        let (used, idle) = (cache.join("base/6-1-1.0000000000000001.base"), cache.join("base/6-1-2.0000000000000002.base"));
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
        assert_eq!(super::canopy_files(b), ["median", "p95", "cover5m"].map(|s| format!("meta_chm_lat=50.0_lon=-70.0_{s}.tif")));
        // A box across 50° N and 0° E: four squares.
        let names = super::canopy_files([-0.5, 49.5, 0.5, 50.5]);
        assert_eq!(names.len(), 12);
        for (top, left) in [(50, -10), (60, -10), (50, 0), (60, 0)] {
            assert!(names.contains(&format!("meta_chm_lat={top}.0_lon={left}.0_p95.tif")), "{top} {left}: {names:?}");
        }
    }
}
