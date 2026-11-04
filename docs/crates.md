# The pipeline's crates (task #916)

**Status: Planned.** Steps 1 (`crates/pyport`), 2 (`crates/codecs`), 3 (`crates/jobrt`) and 4
(`crates/buildstore`) are built; every later step below is planned.

The `pipeline` crate held ~244k lines of Rust in `crates/pipeline/src` (85 % of the repo's Rust)
and its binaries (15 then; 2 since #917, `scenic` and `scenic-build`, the other 13 programs
scenic-build's tools). A crate is rustc's unit: an edit anywhere recompiles all of it, little of it
compiles in parallel, and one compile of it peaked at 9.3 GB on the owner's 16 GB M1 (31 Oct
2026). This plan splits it along its seams, one step at a time, each step shipping alone with the
build green and every program's behaviour unchanged.

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

Rerun on 1 Nov 2026: on main at 0d42821d (#879 step 4b in), one cycle of 57 modules, ~203k lines
(and peaks ↔ summits); after step 3, one of 49 modules, ~194k lines: control, timings and
resume_log moved out, and costin, geotiff, water, heritage_layers and interest, which reached the
cycle only through them, left it (heritage_layers ↔ interest and peaks ↔ summits are small cycles
of their own).

Rerun on 4 Nov 2026 on main at 86626025: still one cycle of 49 modules, ~194k lines. After step 4,
one of 43 modules, ~184k lines: out, handoff, rawpack, localread, coverage and outlines (in it),
costin and geotiff (outside it) moved out, with fetch. pipeline keeps an `out` of 61 lines (the
`Out` its code names, binding buildstore's to the pool's records and the steps table), which is in
the cycle (→ agent, pool).

The cycle is held together by a few thin edges from low-level code up to `agent`, `pool`,
`inputs` and `control`, not by the steps' logic. The ones that matter most:

| edge | what it uses | cut by |
|---|---|---|
| control → agent | `agent::DRAIN_GRACE` (one const) | built (step 3): the const is control's; the agent uses it from there |
| control → coord | `coord::asks::{SettingAsk, Settings}` (the asks control queues) | built (step 3): the settings, their names and the ask are jobrt's `settings`; coord::asks re-exports them |
| timings → control | pause and safe-gap bookkeeping (`gaps_begin`, `safe_gaps`, `SAFE_GAP_BOUND_S`) | built (step 3): `timings` and `control` moved down together |
| timings → fetch | `fetched_bytes` (the network bytes a phase counts) | built (step 3): the counter is timings'; fetch adds to it and re-exports `fetched_bytes` |
| resume_log → agent | `agent::resume::{RESUME_ENV, HOLD_S}` | built (step 3): the constants are resume_log's; agent::resume re-exports them, osmpass uses them |
| resume_log → out | `names(out: &Out, …)` (a manifest's names) | built (step 3): `names` takes a `resume_log::Manifest` (one method, `get`), which out::Out implements |
| localread, osmpass → agent::room | `job_target`, `free_past`, `MAX_TARGET` | items moved (step 3): jobrt's `room` (with cond's `free_bytes`); agent::room re-exports them. The edge goes once its users name jobrt's path (they move in steps 4-5) |
| osmpass, layers, … → agent::jobs | progress reporting (`stage`, `report`, `part`, `within`, `run_watched`, `each_line`) | items moved (step 3): jobrt's `jobs` (with the log's reading: `tail`, `said`, `parts`, `error_of`); agent::jobs re-exports them. The edge goes as for room |
| coverage, dem → agent::recipes | the `Recipe` type | built (step 4): agent/recipes.rs is buildstore's `recipes`; agent::recipes re-exports it |
| out → agent::steps | `steps::writes` (each step's write set, checked on every save) | built (step 4): buildstore's `Out<A: Above>` asks `A::writes`; pipeline's `out::Out` is `Out<Pipeline>`, whose `writes` is `agent::steps::writes` |
| out → pool::records | out's two wrappers (`records_now`, `brief_now`), and `Out` itself: it opens from `Records::now`'s manifest and pending, and a catalogue's sizes are its (`recorded_sizes`) | built (step 4): the wrappers are gone (their callers name `pool::records`); `Out` reads the records by `A::records`, `Pipeline`'s from `Records::now`. `read_manifest` stays in pipeline's `out` |
| out → inputs | `in_inputs_folder` | built (step 4): it and `DROP_ROOTS` are buildstore's `inputs_folder`; inputs re-exports both |
| out → pool::term, agent::resume | `term::DIR` (whether the pool has begun), `UNMERGED_ENV` | built (step 4): `out::TERMS` and `out::UNMERGED_ENV`; `pool::term::DIR` is `out::TERMS`, agent::resume re-exports the other |
| fetch → control | `draining_lately` | built (step 3): `control` is jobrt's |
| fetch, out, outlines → osmpass | osmpass's ssh helpers (out: `ssh_opened`; fetch: `proxy_addr`, in a test) and its set names (outlines: a measuring test) | built (step 4): `SSH_START`, `watched`, `ssh_opened`, `ssh_stalled` and `proxy_addr` are buildstore's `nasssh`, osmpass and agent::gc use them from there; the measuring test is osmpass's |
| rawpack → agent, pyramids | `agent::stopping` (the agent asked to stop), `pyramids::DIR` | built (step 4): the stop flag is jobrt's `control::{ask_stop, stopping}` (agent::stopping re-exports it); the folder is `rawpack::PYRAMIDS`, which `pyramids::DIR` names |
| localread → agent::room | `job_target`, `free_past`, `MAX_TARGET` | built (step 4): localread names jobrt's `room` |
| inputs → coverage | `impl Coverage { fn accepted }` in inputs::regions::read, an inherent impl that can't stay once `Coverage` is another crate's | built (step 4): `inputs::regions::read::accepted_coverage`, its callers renamed |
| dem → terrain_deep, terrain_pack | terrain's tile helpers (7 references) | the helpers move down (codecs or buildstore), or dem joins the terrain family |
| inputs → lakefill, rail | 2 references to lakefill, 4 to rail, from the input checks | the named items move down beside inputs, or the checks call them through the agent |
| cli → polar | one reference to polar | it moves down with cli (cli and net stay in pipeline until #883, which edits cli/build.rs, cli/progs.rs and net.rs, lands) |
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
3. **jobrt** (a job's runtime; built, step 3): sys, budget, phase_names, pyenv, control, timings,
   resume_log, `jobs` (the job side of agent::jobs: its progress lines and its log read back),
   `room` (the room target a job is told and the free space past it, from agent::room and
   agent::cond), `settings` (the pool's settings and the ask to set one, from coord::asks), the
   resume constants, `DRAIN_GRACE`. ~9.6k lines (1 Nov 2026), no pipeline dependency. Planned:
   cli and net (after #883), nasread's NAS reading (after #923, which edits agent/nasread.rs), and
   the `Offload` trait with the step families that use it (step 5: terrain_task, trees and offload
   are its only users).
4. **buildstore** (the build's files: the NAS/store layer above `store`; built, step 4): out,
   handoff, rawpack, localread, costin, coverage, outlines, fetch, geotiff, `recipes` (the
   `Recipe` type), `inputs_folder` (the inputs folders' predicate), `nasssh` (a bounded `ssh` to
   the NAS, from osmpass). ~12.4k lines (4 Nov 2026), no pipeline dependency. Its `Out` is generic
   over what it reads from above (`out::Above`: the records and the write sets), so its methods
   compile in pipeline, where `Pipeline` is named. Planned: the records' reading
   (pool::records' `Records::now`, `Brief::now`, pool::term's `current`) down here, before step 5:
   a step family's crate below pipeline can't name `Pipeline`, so its code and tests can't open
   an `Out` until then.
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

| program | lines | uses | its code's crate |
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

Since #917 only `scenic` and `scenic-build` are binaries; the other rows are scenic-build's tools
(`scenic-build tool <name>`, src/bin/tools/, `pipeline::cli::TOOLS`), linked into it once. A tool's
file stays with scenic-build; the last column is the crate the modules it uses go to, which a
step moves only with its whole set. Jobs and tasks run a tool by its name (`pipeline::cli::program`).

## 4. Steps

Each step ships alone: build, clippy and the affected tests green, no behaviour change, the
programs' names unchanged. Files paused branches edit (#879: agent/steps.rs, agent/build.rs;
#856: crates/server; #882 step 4: coord; #914) move only after those land.

1. **pyport** (built): move pyjson, pytext, pynumpy, tpsimplify into `crates/pyport`; pipeline
   re-exports them. No other file changes.
2. **codecs** (built): the acyclic leaves of §2.2 (modgraph.py, rerun 31 Oct 2026: none names a
   module outside the set but tiling the root's `count_bar`), into `crates/codecs`, with the root's
   `morton`, progress bars and `ProgressRead`; pipeline re-exports them. `areaflags::is_clockwise`
   became `pub` (trees::mask uses it); the test helpers pipeline's tests use (whole::testfiles,
   terrain_water::tests) sit behind a `testkit` feature, as store's do. None is a file a paused
   branch changes.
3. **jobrt** (built): control, timings, sys, budget, resume_log, phase_names, pyenv into
   `crates/jobrt`, with the job side of agent/jobs.rs (`jobs`), the room primitives of
   agent/room.rs and agent/cond.rs (`room`), the settings types of coord/asks.rs (`settings`), the
   resume constants and `DRAIN_GRACE`; pipeline re-exports the modules at their old paths, and
   agent::jobs, agent::room, agent::cond, agent::resume and coord::asks the items moved out of
   them. The cuts are §1's table's (control → agent and coord, timings → fetch, resume_log →
   agent and out); the test hooks other modules' tests use (resume_log::STOP_AFTER,
   budget::TEST_RESOLVE, timings::reset) sit behind a `testkit` feature. cli and net are left
   for after #883 (it edits both), nasread's reading for after #923. The step modules still name
   the moved items through the agent's paths (`crate::agent::jobs::report`), so modgraph.py still
   shows their edges to agent; those paths are rewritten as each moves (steps 4-5), several being
   files #883 edits.
4. **buildstore** (built): out, handoff, rawpack, localread, costin, coverage, outlines, fetch,
   geotiff into `crates/buildstore`, with agent/recipes.rs (`recipes`), `inputs_folder` (from
   inputs) and `nasssh` (from osmpass); pipeline re-exports the modules at their old paths. The
   cuts are §1's table's. `out` was planned behind a registry the agent fills for the write sets;
   `Out` also reads the pool's records itself (every open, and `recorded_sizes`), and a registry
   would have to be filled in every process and test that opens one, so `Out` is generic over an
   `Above` (records and write sets) instead, and pipeline's `out::Out` is `Out<Pipeline>`: its 144
   callers unchanged. The test helpers other crates' tests use (`Out::handing_off`,
   coverage's fixtures and `testkit::tree_agrees`, geotiff's `testtiff` and `write_f32`,
   `costin::TEST_LOCK`) sit behind a `testkit` feature. Out's tests run in buildstore with an
   `Above` of their own (today's files, two steps' write sets); pipeline's out keeps one test of
   the binding. One line in a file #929 edits: `pyramids::DIR` names `rawpack::PYRAMIDS`.
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

After step 3, 1 Nov 2026 11:41-11:43 UTC, rustc 1.98.1, `cargo build --workspace --bins` (dev)
from a built target dir (the shared one, tools/dev/bin/cargo), one build at a time; "jobrt edit":
one line appended to jobrt's timings.rs (the "core edit" above, now in jobrt), "agent edit": to
agent/jobs.rs, "codecs edit": to hipack.rs:

| run | after step 3 |
|---|---|
| dev, jobrt edit | 23 s, 2.43 GB (load 15-17): jobrt, pipeline, server recompiled |
| dev, agent edit | 18 s, 2.36 GB (load 14-16): pipeline, server |
| dev, codecs edit | 22 s, 2.36 GB (load 17-21): codecs, pipeline, server |

After step 4, 4 Nov 2026 06:54-06:56 UTC, on the M4 (48 GB, 14 cores; the pool's jobs and other
agents beside), rustc 1.98.1, `cargo build --workspace --bins` (dev). "Clean": a fresh target
folder, third-party crates from sccache, the shared folder's lock held so no other cargo ran beside
it. The edits: one comment line appended to buildstore's geotiff.rs, agent/jobs.rs, jobrt's
timings.rs, codecs' hipack.rs, each built from a built tree (the shared folder):

| run | after step 4 |
|---|---|
| dev, clean | 81 s, 4.33 GB (load 16-32) |
| dev, buildstore edit | 8 s, 2.28 GB (load 20): buildstore, pipeline, server recompiled |
| dev, agent edit | 7 s, 2.28 GB (load 20): pipeline, server |
| dev, jobrt edit | 7 s, 2.28 GB (load 20): jobrt, buildstore, pipeline, server |
| dev, codecs edit | 7 s, 2.28 GB (load 19): codecs, buildstore, pipeline, server |

The machine's load (other agents' builds, 30-125) swamps any difference step 1 could make; these
are a baseline of the method, not evidence either way. What follows from the structure, not the
timings: an edit in a crate recompiles every crate that depends on it, so moving a *leaf* module
down (step 1's pyjson, step 2's hipack) does not spare the pipeline's recompile after an edit to it. The gains come
when the modules edited most (the steps, the agent) sit in crates that few or no others depend on,
and the big middle (inputs, terrain) compiles beside the other families: steps 3-6.
