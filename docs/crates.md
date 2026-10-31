# The pipeline's crates (task #916)

**Status: Planned.** Steps 1 (`crates/pyport`) and 2 (`crates/codecs`) are built; every later step below is planned.

The `pipeline` crate held ~244k lines of Rust in `crates/pipeline/src` (85 % of the repo's Rust)
and 15 binaries. A crate is rustc's unit: an edit anywhere recompiles all of it, little of it
compiles in parallel, and one compile of it peaked at 9.3 GB on the owner's 16 GB M1 (31 Oct
2026). This plan splits it along its seams, one step at a time, each step shipping alone with the
build green and every binary's behaviour unchanged.

## 1. The module graph

`tools/dev/modgraph.py` derives it from the sources: for each top-level module of
`crates/pipeline/src` (a file `x.rs` and/or folder `x/`), the other top-level modules its code
names by `crate::<m>` (also inside `use crate::{…}` groups) or, in a binary, `pipeline::<m>`; then
the cycles (strongly connected components). `--dot` prints Graphviz. Rerun it rather than trusting
numbers copied here.

What it showed on 31 Oct 2026 (main at 3cba2d2a):

- **One cycle of 60 modules, ~203k lines** (of ~244k): agent, coord, pool, needs, inputs, out,
  control, timings, resume_log, coverage, localread, osmpass, dem, fetch, geotiff, every terrain
  and slope module, depth, polar, lakefill, water, trees, treepacks, leaftype, landcover, bld,
  buildtiles, unit, reach, candidates, rail, gtfs, ferries, timetables, ovconv, markconv, layers,
  heritage, heritage_layers, interest, peaks, summits, namestodo, answers, pubtimes, offload,
  handoff, rawpack, pyramids, stage, road_terrain, rules, areafiles, costin, cli.
- **Outside it** (acyclic, no edge back into the cycle): areaflags, budget, chain, extents, marks,
  net, phase_names, pyenv, pyjson, pynumpy, pytext, tpsimplify, scache, shade_codec, stations,
  summary, sys, terr, terrain_codec, terrain_water, trailends, vtgen, watercov, webp, whole, legacy
  (→ summary), view (→ scache), buildings, elev, tiling (→ the crate root's `morton`/bars);
  basepack, basepart, hipack (→ legacy, tiling); and the users of the cycle that nothing in it uses
  back (areadetails, filterprops, heritagedetails, heritagewd, items, labels, marksjob, menu,
  pageviews, railfeeds, whsshapes, wikiask, appupdate…).

The cycle is held together by a few thin edges from low-level code up to `agent`, `pool`,
`inputs` and `control`, not by the steps' logic. The ones that matter most:

| edge | what it uses | cut by |
|---|---|---|
| control → agent | `agent::DRAIN_GRACE` (one const) | the const moves down beside `control` |
| timings → control | pause and safe-gap bookkeeping (`gaps_begin`, `safe_gaps`, `SAFE_GAP_BOUND_S`) | `timings` and `control` move down together |
| resume_log → agent | `agent::resume::{RESUME_ENV, HOLD_S}` | the constants move down (the runtime crate) |
| localread, osmpass → agent::room | `job_target`, `free_past`, `MAX_TARGET` | the disk-room primitives move down; the agent's room policy stays up |
| osmpass, layers, … → agent::jobs | progress reporting (`stage`, `report`, `part`, `within`, `run_watched`, `each_line`) | the job side of `agent::jobs` moves down (runtime crate) |
| coverage, dem → agent::recipes | the `Recipe` type | the type moves down to the store layer |
| out → agent::steps | `steps::writes` (each step's write set, checked on every save) | a registry the agent fills at start (a `fn` the out layer calls), or the write-set table moved down; after #879, which edits `agent/steps.rs` |
| out → pool::records | `Records::now`, `Brief::now` | callers name `pool::records` directly; out's two wrappers go |
| out → inputs | `in_inputs_folder`, the gate | the inputs folder's path predicates move down to the store layer |
| fetch → control | `draining_lately` | `control` sits below `fetch` (runtime crate) |
| fetch, out, outlines → osmpass | osmpass's reading helpers (fetch, out: one reference each; outlines: 2) | the named items move down to buildstore; osmpass (inputs) stays above |
| dem → terrain_deep, terrain_pack | terrain's tile helpers (7 references) | the helpers move down (codecs or buildstore), or dem joins the terrain family |
| inputs → lakefill, rail | 2 references to lakefill, 4 to rail, from the input checks | the named items move down beside inputs, or the checks call them through the agent |
| cli → polar | one reference to polar | it moves down with cli (jobrt) |
| layers → ovconv | one item | layers sits in the terrain family; the item moves down to codecs |
| terrain_task, trees, offload → coord | offloading a step's parts to other Macs | a trait in the runtime crate (`Offload`), implemented by coord; the step names only the trait |

## 2. Proposed crates

Bottom to top; a crate depends only on those above it in this list (and on det, roadcore, store,
names, webpenc as now).

1. **pyport** (built, step 1): pyjson, pytext, pynumpy, tpsimplify. Python's behaviour ported for
   byte-identical outputs. ~1.8k lines, no pipeline dependency.
2. **codecs** (built, step 2; formats with no build state): terrain_codec, shade_codec, webp,
   whole, watercov, terrain_water, vtgen, chain, marks, stations, trailends, extents, areaflags,
   tiling (with the root's `morton` and progress bars), summary, legacy, hipack, basepack,
   basepart. ~8.8k lines (31 Oct 2026), no pipeline dependency.
3. **jobrt** (a job's runtime): sys, budget, net, phase_names, pyenv, control, timings,
   resume_log, cli, the job side of agent::jobs, the room primitives of agent::room, the
   resume constants, nasread's NAS reading, `DRAIN_GRACE`, an `Offload` trait. ~12k lines.
4. **buildstore** (the build's files: the NAS/store layer above `store`): out, handoff, rawpack,
   localread, costin, coverage, outlines, the `Recipe` type, the inputs folder's predicates,
   fetch, geotiff. ~12k lines.
5. **inputs** (the declared inputs): inputs (21.9k lines), dem, rules, osmpass, areafiles. Its
   edges up to agent (23) and pool (7) are cut as in §1 first; ~33k lines.
6. Step families. They are not independent today: modgraph.py shows trees → terrain (treepacks →
   layers), roads → trees and terrain (unit → trees, landcover, stage), names → terrain (heritage →
   stage, terrain_pack; gtfs → lakefill), bld → names and roads (bld → heritage, unit), offload →
   bld, trees, unit. So they stack (terrain, then trees, roads, names, bld) unless those edges are
   cut too (moving the shared items, `layers`, `stage`'s tile helpers, down to codecs/buildstore);
   only the families with no edge between them compile in parallel. An edit in a family still
   rebuilds every family above it and pipeline.
   - **terrain**: terrain_base, terrain_deep, terrain_pack, terrain_resume, terrain_task,
     pyramids, stage, road_terrain, slope_pack, depth, polar, lakefill, water, peaks, summits,
     layers. ~25k lines. (terrain_pack ↔ terrain_deep ↔ stage stay one crate.)
   - **roads**: unit, reach, candidates, buildtiles, elev, rail, timetables. ~5k lines.
   - **trees**: trees, treepacks, leaftype, landcover. ~7k lines.
   - **names** (heritage, places, transit): heritage*, interest, items, labels, wikiask, pageviews,
     areadetails, whsshapes, filterprops, ferries, gtfs, railfeeds, ovconv, markconv, marksjob,
     namestodo. ~17k lines.
   - **bld**: bld (3D buildings). ~9k lines.
7. **coordination**: coord, pool, needs, pubtimes, answers, offload, menu, appupdate (coord ↔ pool
   ↔ agent are one cycle today; they split after the steps are out). ~33k lines.
8. **pipeline** (the agent, kept under this name so `cargo run -p pipeline --bin …` and the
   server's dependency stay): agent (66k lines; later split by its own seams, docs/agent-parts.md)
   and the binaries. The map server's shared parts (scache, view, terr, buildings, used by
   scenic-metrics and crates/server) join **codecs** or a small **mapview** crate.

Each moved module stays reachable at its old path by a `pub use` in `pipeline`'s lib.rs until the
paused branches touching its users have landed; a later step per crate rewrites the paths
(`crate::pyjson` → `pyport::pyjson`) and drops the re-export. The re-exports are therefore
temporary: the list in lib.rs is the record of what still goes through them.

## 3. The binaries

| binary | lines | uses | moves with |
|---|---|---|---|
| scenic | 1.9k | agent, coord, control, pool, budget | pipeline (top) |
| scenic-build | 7.3k | everything | pipeline (top) |
| scenic-metrics | 1.4k | scache, view, terr, whole, trees, unit, control | pipeline (top; uses trees and unit) |
| scenic-polar | 0.1k | polar, dem, resume_log | terrain |
| terrainsub | 0.04k | terrain_task, terrain_pack | terrain |
| coastcheck | 0.5k | lakefill, water, watercov, osmpass | terrain |
| lakefill | 0.4k | lakefill, water, inputs | terrain |
| extract | 1.6k | candidates, elev, trailends, dem | roads |
| elev, tile | 0.06k | elev, road_terrain | roads |
| railfreq | 0.9k | rail | roads |
| trees, landcover | 0.2k | trees, landcover, fetch | trees |
| bldtile | 0.03k | bld, dem | bld |
| areaflags | 0.01k | areaflags, cli | jobrt |

A binary moves only with a step that moves its whole set of modules, and keeps its name (scripts
run binaries by name from the shared target dir). #917 later merges the small tools into
subcommands of `scenic-build`; that changes this table, not the crate plan.

## 4. Steps

Each step ships alone: build, clippy and the affected tests green, no behaviour change, the
binaries' names unchanged. Files paused branches edit (#879: agent/steps.rs, agent/build.rs,
agent/rekey.rs; #856: crates/server; #882 step 4: coord; #914) move only after those land.

1. **pyport** (built): move pyjson, pytext, pynumpy, tpsimplify into `crates/pyport`; pipeline
   re-exports them. No other file changes.
2. **codecs** (built): the acyclic leaves of §2.2 (modgraph.py, rerun 31 Oct 2026: none names a
   module outside the set but tiling the root's `count_bar`), into `crates/codecs`, with the root's
   `morton`, progress bars and `ProgressRead`; pipeline re-exports them. `areaflags::is_clockwise`
   became `pub` (trees::mask uses it). None is a file a paused branch changes.
3. **jobrt**: move the agent's constants and job-side primitives down (agent/jobs.rs,
   agent/room.rs, agent/resume.rs split: the part a step calls goes down, the agent's policy
   stays), then control, timings, sys, budget, resume_log, cli, net. Cuts the control → agent
   edge and most step → agent edges. Needs #879 merged (agent/ edits).
4. **buildstore**: `Recipe` and the inputs folder's predicates down; out's write-set check behind a
   registry the agent fills; out's `pool::records` wrappers removed. Needs #879 (agent/steps.rs).
5. **inputs** and the step families (one step each, bottom up: terrain, trees, roads, names, bld),
   each after rerunning modgraph.py shows it leaves no edge back into what remains above; the
   cross-family edges of §2.6 are cut where cheap so families compile in parallel.
6. **coordination**, after #882 lands.
7. Path rewrites: per crate, `crate::x` → `<crate>::x`, and the re-exports dropped.

## 5. Measurements

Dated measurements, not targets; rerun `tools/dev/modgraph.py` and the commands below for current
figures. Taken on the M1 (MacBook Pro, 16 GB, other agents building beside: the load average is
given), main's Cargo.toml profiles, a fresh target dir, third-party crates from the shared sccache
(so "clean" is the workspace crates' compile plus the cache's hits), `/usr/bin/time -l` (peak RSS:
the largest process, rustc). "Leaf edit": one line appended to pyjson.rs; "core edit": to
timings.rs.

31 Oct 2026, rustc 1.98.1, `cargo build --workspace --bins` (dev) and
`cargo test --workspace --no-run` (test):

| run | before step 1 (21:59 UTC) | after step 1 (22:23 UTC) |
|---|---|---|
| dev, clean | 192 s, 2.15 GB (load 34) | 363 s, 1.94 GB (load 76) |
| dev, leaf edit | 33 s, 1.81 GB | 41 s, 1.36 GB (load 83) |
| dev, core edit | 29 s, 2.05 GB | 43 s, 1.62 GB (load 104) |
| test, clean | 1026 s, 2.94 GB (load 125) | 995 s, 2.42 GB (load 81) |
| test, leaf edit | 41 s, 1.29 GB | 52 s, 1.29 GB |
| test, core edit | 54 s, 1.09 GB | 45 s, 1.36 GB |

After step 2, 31 Oct 2026 23:46 UTC, rustc 1.98.1, `cargo build --workspace --bins` (dev) from a
built target dir; "codecs edit": one line appended to codecs' hipack.rs, "core edit": to timings.rs:

| run | after step 2 |
|---|---|
| dev, codecs edit | 37 s, 1.60 GB (load 29-35) |
| dev, core edit | 33 s, 1.50 GB (load 35-38) |

The machine's load (other agents' builds, 30-125) swamps any difference step 1 could make; these
are a baseline of the method, not evidence either way. What follows from the structure, not the
timings: an edit in a crate recompiles every crate that depends on it, so moving a *leaf* module
down (step 1's pyjson, step 2's hipack) does not spare the pipeline's recompile after an edit to it. The gains come
when the modules edited most (the steps, the agent) sit in crates that few or no others depend on,
and the big middle (inputs, terrain) compiles beside the other families: steps 3-6.
