# The pipeline's crates (task #916)

**Status: Planned.** Steps 1 (`crates/pyport`), 2 (`crates/codecs`), 3 (`crates/jobrt`), 4
(`crates/buildstore`), 5a (`crates/landmarks`) and 5b (`crates/mapview`) are built; every later
step below is planned, the next ones in §6.

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
costin and geotiff (outside it) moved out, with fetch. pipeline keeps an `out` of 68 lines (the
`Out` its code names, binding buildstore's to the pool's records and the steps table), which is in
the cycle (→ agent, pool).

Rerun on 5 Nov 2026 on main at 9baf1315: one cycle of 47 modules, ~188k lines (peaks ↔ summits a
small one of their own). Steps 5a and 5b moved only modules outside it (heritage_layers ↔ interest,
a small cycle, went whole into landmarks), so after them it is the same 47 modules.

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
| out → agent::steps | `steps::writes` (each step's write set, checked on every save) | built (step 6a): `Out::open` takes the write set as a function (`out::Writes`); pipeline's `out::open(root, scratch, step)` passes `agent::steps::writes` |
| out → pool::records | out's two wrappers (`records_now`, `brief_now`), and `Out` itself: it opens from `Records::now`'s manifest and pending, and a catalogue's sizes are its (`recorded_sizes`) | built (steps 4, 6a): the wrappers are gone; which file holds the records now (`now`, `newest`, `brief_now`, term 1's fallbacks), the NAS's operations and the current term are buildstore's `records`, generic over what's read: `Out` reads its `Recorded`, the pool its `Records`. `read_manifest` is buildstore's |
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
   the NAS, from osmpass), `records` (the records' read side, step 6a: pool/nas.rs, pool::term's
   reading, the choice of the file read; `Member`). ~13k lines (5 Nov 2026), no pipeline
   dependency. Its `Out` is plain: it reads the records itself, and a step's job is handed its
   write set (`Out::open(…, writes)`), so a crate below pipeline can open one.
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
     namestodo. ~17k lines. Its part outside the cycle is built as **landmarks** (step 5a; the
     crate `names` was taken): wikiask, heritagewd, heritage_layers, heritagedetails, filterprops,
     interest, items, pageviews, labels, areadetails, whsshapes, ~6.5k lines, on pyport, jobrt and
     buildstore (and det, roadcore, store, names). Planned: heritage (→ stage, terrain_pack),
     ferries, gtfs (→ lakefill), railfeeds, ovconv, markconv, marksjob, namestodo, once the
     families below them are out.
   - **bld**: bld (3D buildings). ~9k lines.
7. **coordination**: coord, pool, needs, pubtimes, answers, offload, menu, appupdate (coord ↔ pool
   ↔ agent are one cycle today; they split after the steps are out). ~33k lines.
8. **pipeline** (the agent, kept under this name so `cargo run -p pipeline --bin …` and the
   server's dependency stay): agent (66k lines; later split by its own seams, docs/agent-parts.md)
   and the binaries. The scenic metrics' map-side parts (scache, view, terr, buildings: the
   scenic-metrics tool's, and unit's scache) are built as **mapview** (step 5b, ~1.3k lines, on
   codecs, det, roadcore, store). crates/server names none of them; it uses pipeline's inputs,
   depth, water, coord and others, so it keeps its pipeline dependency.

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
   cuts are §1's table's. (`Out` was generic over what it read from above until step 6a.) The
   test helpers other crates' tests use (`Out::handing_off`,
   coverage's fixtures and `testkit::tree_agrees`, geotiff's `testtiff` and `write_f32`,
   `costin::TEST_LOCK`) sit behind a `testkit` feature, helpers only: a dev-dependency's features
   reach examples and test-built binaries too, so no behaviour rides on one. What differs in a
   test comes from the caller's own `cfg!(test)`: fetch's offline start and (since 6a)
   `out::testing` (Out's retry waits, verify's deadline and grace), which a crate's unit tests set
   as their process starts (lib.rs `TEST_START`); tests/test_mode.rs checks a build with the
   dev-dependencies has none of it. Out's tests run in buildstore with write sets of their own
   (two steps'); pipeline's out keeps one test of the steps table's. One line in a file #929
   edits: `pyramids::DIR` names `rawpack::PYRAMIDS`.
5. **inputs** and the step families (one step each, bottom up: terrain, trees, roads, names, bld),
   each after rerunning modgraph.py shows it leaves no edge back into what remains above; the
   cross-family edges of §2.6 are cut where cheap so families compile in parallel. Built so far,
   the parts outside the cycle:
   - 5a, **landmarks** (built): wikiask, heritagewd, heritage_layers, heritagedetails,
     filterprops, interest, items, pageviews, labels, areadetails, whsshapes into
     `crates/landmarks`; pipeline re-exports them. Cuts: `osmpass::osmium` is jobrt::sys's
     (osmpass re-exports it), `rail::day_of` jobrt::timings'. Their unit tests start offline as
     pipeline's do (lib.rs `TEST_START`, checked by `test_start`). unicode-normalization went with
     them (pipeline had no other user).
   - 5b, **mapview** (built): scache, view, terr, buildings into `crates/mapview`; pipeline
     re-exports them. Cut: terr's `z9_block` (no caller).
   Next: §6, breaking the 47-module cycle: 6a (built) made `Out` nameable below pipeline; then
   (planned) inputs, the step families, and the agent's plan split from its runtime.
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
| tools/dev/check.sh at cfd2b6ab (07:00 UTC) | fmt 2 s, clippy 27 s, guards 2 s, tests 451 s (run here: the M4 is the test Mac; its affected set is most of pipeline's tests); largest process 5.6 GB |

Before and after steps 5a-5b, 5 Nov 2026 15:50-16:10 UTC, on the M4 (the pool's jobs and a publish's
tests beside), rustc 1.98.1, `cargo build --workspace --bins --timings` (dev), main at 9baf1315
against the branch, each tree in a folder of its own on the M4 with a fresh target folder (third-party
crates from sccache). Memory: this build's rustc processes alone (their arguments name its target
folder), sampled every 0.3 s: the largest one's RSS (always pipeline's) and all of them at once.
The edits: one comment line appended to wikiask.rs (landmarks after), scache.rs (mapview after) and
agent/jobs.rs, each built from the tree the clean build left:

| run | before (9baf1315) | after (5a-5b) |
|---|---|---|
| dev, clean | 77 s, 3.93 GB (load 134) | 93 s, 3.90 GB (load 60) |
| dev, wikiask edit | 6.8 s, 2.13 GB: pipeline, server | 15.2 s, 2.03 GB: landmarks 0.5 s, pipeline 10.4 s, server |
| dev, scache edit | 6.8 s, 2.05 GB: pipeline, server | 10.6 s, 1.95 GB: mapview 0.2 s, pipeline 8.3 s, server |
| dev, agent edit | 6.8 s, 2.04 GB: pipeline, server | 6.5 s, 1.93 GB: pipeline, server |

`--timings` of the clean builds: the workspace's crates take 62 s of compile in either, and run
almost one at a time (0.86 crates at once on average before, 0.69 after, over the span from the
first to the last): pipeline's lib is 38 s before and 36 s after, the binaries and server's 4-8 s
after it, every other workspace crate under 2.1 s. So far the split moves leaves out from under
pipeline, and pipeline's compile is still the build's critical path. The edits' differences above
came from unequal load: the re-measure below didn't find them.

Re-measured by the review, 5 Nov 2026 16:37-16:51 UTC, on the M4 (load 90-170: the pool's jobs and
other agents' builds), before (3763caa5) and after (29a5adca) each in a clone of its own with its
own target folder, interleaved (before, after; then after, before; …), 3 runs each, the same
command, `cargo build --workspace --bins` (dev) through tools/dev/bin/cargo. "Clean": the
workspace's crates cleaned (`cargo clean -p` each), third-party crates kept built. Peak: the
largest process's (`time -l`, always pipeline's rustc). CPU: user + system of the whole build. The
edits: one comment line appended to wikiask.rs, scache.rs, agent/jobs.rs, each built at once:

| run | before | after |
|---|---|---|
| dev, clean | 50.9, 52.9, 52.0 s (CPU 106-110 s), 4.37 GB | 52.4, 51.4, 48.6 s (CPU 105-111 s), 4.28-4.33 GB |
| pipeline's lib in it (`--timings`) | 38.6, 40.3, 40.1 s | 38.4, 36.0 s |
| dev, wikiask edit | 9.6, 10.8, 10.1 s (CPU 13.8-14.4 s), 2.39 GB | 7.0, 10.6, 9.7 s (CPU 14.0-14.7 s), 2.28 GB |
| dev, scache edit | 7.0, 10.5, 9.5 s (CPU 13.5-14.3 s), 2.33 GB | 7.0, 10.0, 9.3 s (CPU 13.5-14.5 s), 2.22 GB |
| dev, agent edit | 7.0, 10.5, 9.7 s (CPU 13.5-14.5 s), 2.33 GB | 7.0, 10.0, 9.2 s (CPU 13.2-14.1 s), 2.22 GB |
| test build after a wikiask edit: its unit tests | pipeline --lib: 9.5, 7.2, 7.2 s, 3.34 GB | landmarks --lib: 0.9, 0.7, 0.6 s, 0.24 GB |
| … and pipeline's lib tests | 9.5, 7.2, 7.2 s | 7.6, 7.3, 7.3 s |

So steps 5a-5b leave dev builds where they were (clean, edits, within noise; the largest rustc
~2-4 % smaller), cost an edit below pipeline nothing measurable, and make the moved modules' own
test loop ~10 times faster. The gain on builds needs the big middle out of pipeline (§6).

The machine's load (other agents' builds, 30-125) swamps any difference step 1 could make; these
are a baseline of the method, not evidence either way. What follows from the structure, not the
timings: an edit in a crate recompiles every crate that depends on it, so moving a *leaf* module
down (step 1's pyjson, step 2's hipack) does not spare the pipeline's recompile after an edit to it. The gains come
when the modules edited most (the steps, the agent) sit in crates that few or no others depend on,
and the big middle (inputs, terrain) compiles beside the other families: steps 3-6.

## 6. Breaking the big cycle (6a built; the rest planned)

**Status: 6a built, 6b-6f planned** (design of 5 Nov 2026, after steps 5a-5b). Figures are from main at 3763caa5;
rerun `tools/dev/modgraph.py` before each step.

### 6.1 What holds the cycle

Lines are without `#[cfg(test)]` code (what a dev build compiles; pipeline's lib: ~142k of them,
compiled in 36-40 s on the M4, the build's critical path).

| part | lines | its edges up (non-test) |
|---|---|---|
| inputs family: inputs, dem, rules, osmpass, areafiles, rail, lakefill, water | 26.9k | `out::Out` and `read_record`/`read_manifest` (~70), `pool::{term, records::Brief, nas::ReadOnly, member_id}` (8), agent items already moved down but named by the agent's path (`jobs::*`, `recipes::Recipe`, `room::*`: ~30), and a few real ones (`build::{h, PassCtx, spoken_work, outlines_work}`, `steps::row`, `app_version`, `nasread::timed_out`, `regionsview`), `cli::program` |
| terrain: terrain_*, pyramids, stage, road_terrain, slope_pack, depth, polar, layers, peaks, summits | 14.5k | `Out` (~30), `build::{TERRAIN_V, SLOPE_V, SLOPE_LO_V, coverage_tiles, grown_e7}`, `placement::ahead_on`, `offload`, coord only in tests |
| trees, roads (unit, reach, candidates, buildtiles, elev, timetables), names (heritage, ferries, gtfs, railfeeds, ovconv, markconv, marksjob, namestodo), bld, offload | 24.3k | the same kinds; offload's own code names `coord::client::Client` and `coord::task::{Offer, Output, clone, safe, beats, MARGIN}`; the families name only `offload::{Offload, Offered, Patience, Settled, places}`, coord only in tests (`start_for_test`, `Ask`, `Granted`, `Done`) |
| the agent's plan: agent/{build, steps, idents, plan, tiles}.rs | 19.5k | names every family (~1,000 references: targets, keys, reads, write sets) |
| the agent's runtime (the rest of agent/), coord, pool, needs, paces, uses, pubtimes, answers, menu, appupdate, cli, net | 56.8k | names inputs (~50), a few family constants (`*::task::KIND`, `terrain_pack::piece_peak_mb`, `trees::squares_of`), and the plan through ~49 items (106 references: the steps table's queries, `build::{Keys, Work, is_none_key, …}`) |

So the cycle is two loops: every step module up to `out`/`pool`/`agent` (thin, mostly items that
already live below), and the agent's plan ↔ its runtime (thin one way, 106 references).

### 6.2 What moves where

Final layering, bottom to top (each line depends only on lines above it):

1. pyport, codecs, jobrt, buildstore (with the records' read side), landmarks' and mapview's
   leaves, and **coordwire** (coord's client side);
2. **inputs**;
3. the families **terrain**, then **trees**, **roads**, **names** (landmarks grows into it),
   **bld**, **offload**; and, beside them, **agentcore** (the agent's runtime, coord, pool and
   the rest of that row);
4. **pipeline**: the agent's plan, the binaries, the re-exports. It names everything; nothing
   names it.

The choices, against the alternatives:

- **`Out` stops being generic** (built, 6a). Neither a registry nor `Above` across crates:
  `Out<Pipeline>` couldn't be named below pipeline, and making every family generic over
  `A: Above` would compile all of them in pipeline. What `Above` gave comes instead from: the
  records, read by buildstore itself (below); the write set, a plain `out::Writes`
  (`fn(&str, &str, bool) -> Option<bool>`) handed to `Out::open(root, scratch, step, writes)`,
  which only scenic-build (`run`) and agent/gc call outside tests, both through
  `pipeline::out::open(root, scratch, step)` (it passes `agent::steps::writes`); `Out::unbound`
  (the steps' own code) needs neither; and `TESTING`, a process flag `buildstore::out::testing()`,
  `cfg!(test)` in buildstore's own tests and set otherwise only by the `#[cfg(test)]` start-up
  constructor each crate has for `fetch::go_offline` (pipeline's and landmarks' `TEST_START`,
  their test_start.rs probes checking it; every new crate gets one); tests/test_mode.rs checks
  it's off in a build with the dev-dependencies. The family tests that check their step's writes
  against the real table (rail's, timetables', ovconv's, inputs' tests) call
  `crate::out::open`, pipeline's; they move to pipeline's tests/write_sets.rs when their family
  leaves (6d, 6e), not before: moving them now would only collide with the work landing in those
  files.
- **The records' read side goes to buildstore** (built, 6a: `buildstore::records`): pool/nas.rs
  (the `Nas` trait, its `NasRoot` impl, `ReadOnly`; `short` and the tests' `Mem` stay in pool),
  term.rs's read half (`records::term`: `current`, `read`, `next`, `path`, `Term`, `Current`,
  `DIR`, `HINT`, `app_at_least`, `is_version`; `Term::after` and `Term::lead` with it, so
  `Member` and `is_member_id` moved too, which inputs names) and records.rs's choice of file
  (`load`, `newest`, `now`, `brief_now`, `term_one`, `no_pool`, `FIRST`, `TODAY`, the paths,
  `read_or_default`, `FirstUnreadable`), written once, generic over the snapshot read
  (`Snapshot`: `pool::records::Records`, which carries the agent's `Keys`, and buildstore's
  `Recorded`, which reads only the term, manifest, pending and sizes) and the brief
  (`BriefOf`: pool's `Brief`, and `read_manifest`'s manifest alone). Pool re-exports all of it at
  its old paths; `Records::{load, newest, now}` and `Brief::now` are one-line calls of these. The
  writing (save, merge, take_up, making terms) stays in pool. No second copy of the choice.
- **Items the steps name from the agent move to their owners**: the version constants
  (`TERRAIN_V`, `SLOPE_V`, `SLOPE_LO_V`, …) to their family (agent::build re-exports them),
  `grown_e7` to roadcore::geo, `coverage_tiles` to buildstore::coverage, `h` to jobrt,
  `app_version`, `ahead_on` and `room::{disk_free, nas_room, make_room}` to jobrt,
  `nasread::timed_out` and `publish::map_now` to buildstore, `input_digests` to inputs,
  `bldtiles_peak` to bld, `cli::program` to jobrt (with cli, after #883). The plan functions
  a step calls (`PassCtx`, `spoken_work`, `outlines_work`, `names_todo_work`, `trees_targets`)
  go to the family whose work they describe; the plan calls them there.
- **coordwire**: coord/client.rs, coord/task.rs's client half (`Offer`, `Output`, `clone`,
  `safe`, `beats`, `MARGIN`) and the wire types (`Ask`, `Granted`, `Done`, `Handed`,
  held's `Beat`/`Answer`); coord re-exports them. Not a trait object: offload needs the real
  client, and the client is small. Tests that start a coordinator (`coord::start_for_test` in
  terrain_task, trees::task, bld and offload) move to a pipeline test module, since a family
  can't dev-depend on pipeline (its types would be two copies).
- **The agent's plan above its runtime, through a trait.** agentcore can't name the plan (the
  plan names every family and agentcore). The types the runtime reads (`Step`, `Kind`, `Read`,
  `Place`, `Keys`, `Work`) move into agentcore; the table and its functions stay in pipeline,
  which implements `agentcore::Plan` (the ~49 items as methods: `row`, `shared`, `outside`,
  `saves`, `records_only`, `output_names`, `mem_mb`, `read_targets`, `renamed`, `made`, …) and
  passes `&PLAN` to the agent's entry points (`agent::run`, the forecast, the simulator, scenic
  menu). Passed, not installed: no link-time constructor (a static in an rlib may be dropped by
  the linker) and no registry to fill in every test. The family constants agentcore names
  (`*::task::KIND`, `piece_peak_mb`, `squares_of`, `DEEP_MIDS_MB`) go through `Plan` too.

### 6.3 Order

Each ships alone, measured as §5 does (interleaved, CPU time and peak memory too); one move per
commit; re-exports at the old paths as before.

1. **6a, `Out` and the records' read side** (built, 5 Nov 2026): as above. Everything the step
   modules name of `out` and `pool` now lives in buildstore (`Out`, `read_manifest`,
   `records::nas::ReadOnly`, `records::term::{app_at_least, is_version}`, `is_member_id`);
   inputs/served.rs's `Brief::now(…).manifest` became `read_manifest`. They still name most of it
   by pipeline's re-exporting paths (`crate::out::Out`, `crate::pool::term::…`), so modgraph.py
   still draws those edges: 6b rewrites the paths. No module left pipeline. Keys, outputs and the
   records' formats are unchanged; an `Out` now parses only the snapshot's term, manifest,
   pending and sizes (the agent's keys were parsed and dropped), and before the pool reads
   today's manifest, pending and sizes, not jobs.json, and `read_manifest` today's manifest alone.
   Its measurements as §5 takes them (the M4, interleaved, 3 runs each) and a key check on a
   frozen plan (`bench_plan_on_a_real_root`, after against before) are pending: the M4 went
   offline before they ran.
2. **6b, the paths**: the step modules name jobrt's and buildstore's items directly
   (`crate::agent::jobs::report` → `jobrt::jobs::report`, `agent::recipes::Recipe`,
   `agent::room::{free_past, job_target}`), and the item moves of §6.2. Mechanical; modgraph.py
   then shows the step modules' remaining edges up, which should be none but §6.2's.
3. **6c, coordwire**.
4. **6d, inputs** (inputs, dem, rules, osmpass, areafiles, rail, lakefill, water; dem's 7
   references to terrain's tile helpers move those helpers to codecs first). Needs cli out of
   pipeline, so after #883, or `cli::program` alone moved first. First, as the first step module
   leaves: tests/grant_rules.rs scans only crates/pipeline and crates/jobrt; widen it to every
   crate's src (planned for 6a, where no module left, so nothing needed it yet).
5. **6e, terrain**, after #965 lands (it edits terrain_deep). Then trees, roads, names (landmarks
   takes heritage, ferries, gtfs, railfeeds, ovconv, markconv, marksjob, namestodo), bld,
   offload. Cut the cross-family edges where one item is all (`layers → ovconv`, treepacks →
   layers' names to codecs) so trees, roads and names compile side by side.
6. **6f, agentcore and `Plan`**: the last and largest step, after #882's later steps (coord).
   Before it, pipeline still holds ~96k lines (plan 19.5k + runtime 56.8k + bins 12.9k + glue).

### 6.4 Expected gain

Estimated from the 5 Nov `--timings` (pipeline's lib: 36-40 s for ~142k non-test lines, ~0.27 s
per thousand; edits measured in §5) and the line counts above; to be checked against each step's
measurements.

- **Clean dev build** (the workspace's crates; M4): today one serial chain, ~5 s of crates below, pipeline's
  lib 36-40 s, then the binaries and server ~7 s (~50 s in all). After 6a-6e: inputs ~7 s, terrain ~4 s, the families ~2-4 s,
  then pipeline ~20 s (76k lines): about the same sum on one chain, so ~5-10 % off the clean
  build. After 6f: inputs ~7 s, then agentcore ~15 s beside the families, then pipeline's plan
  ~5 s: the chain from ~50 s to ~38 s, **~25 % off**.
- **Memory**: the largest rustc is pipeline's (4.3-4.4 GB on the M4 in dev). After 6a-6e ~2.5 GB,
  after 6f ~2 GB (agentcore, 57k lines), **about half**; the sum at once (agentcore beside a
  family) ~3 GB. The 9.3 GB test-profile peak on the M1 should fall in proportion (~4.5 GB).
- **An edit's rebuild (dev)**: today ~7-10 s on the loaded M4 (§5's re-measure), whatever the
  module, as pipeline recompiles (~5-8 s) then the binaries and server (~2 s, a floor). An edit in
  a crate below rebuilds every crate above it; §5's re-measure found pipeline's rebuild after a
  dependency changed costs about what an edit inside it does (CPU 14.0-14.7 s against 13.8-14.4 s).
  So the edit's cost follows the size of what sits above it: after 6f, an edit in a family
  rebuilds it, the families above it and pipeline (~25k lines plus the binaries): ~3-4 s against
  ~7-10 s; in agentcore, agentcore and pipeline (~76k lines): a little under today; in inputs or
  below, everything: as today. Until 6f, a module moved below pipeline gains nothing here.
- **An edit's tests** gain most: a family's unit tests build only that crate's test binary,
  not pipeline's whole library at the test profile: after a wikiask edit, landmarks' unit tests
  build in 0.6-0.9 s, pipeline's lib tests in 7-9.5 s (§5). A family's crate brings that to its
  steps' tests (terrain's, inputs' …), which today each rebuild pipeline's whole test library.
