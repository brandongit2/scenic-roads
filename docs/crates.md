# The pipeline's crates (task #916)

**Status: Planned.** Steps 1 (`crates/pyport`), 2 (`crates/codecs`), 3 (`crates/jobrt`), 4
(`crates/buildstore`), 5a (`crates/landmarks`), 5b (`crates/mapview`), 6a to 6d (§6, 6c
`crates/coordwire`, 6d `crates/inputs`) and 6e (`crates/offload`, `crates/terrain`, `crates/trees`,
`crates/roads`, the names family into `crates/landmarks`, `crates/bld`) are built, and 6f1 and
6f2 (§6.3: `build::pass_work`, the frozen-plan check and the `Plan` trait); every later step
below is planned (6f3 and 6f4: §6).

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

Rerun on 6 Nov 2026 for step 6e: on main at 8d97cc8e (after 6d), one cycle of 35 modules, 146.7k
lines; after 6e's terrain, one of 21 modules, 127.9k lines (agent, answers, bld, buildtiles, coord,
heritage, leaftype, needs, offload, out, ovconv, paces, pool, pubtimes, reach, stage, timetables,
treepacks, trees, unit, uses): the terrain family's twelve modules and grant_tests moved out, and
markconv, whose way back into it went through layers, left it. peaks ↔ summits is still a small
cycle of its own.

Rerun on 6 Nov 2026 for 6e's trees, roads and names: on main at dd7246c6, one cycle of 21 modules,
128.3k lines; after them, one of 12 modules, 117.6k lines (agent, answers, bld, buildtiles, coord,
needs, offload, out, paces, pool, pubtimes, uses): the trees family's four, the roads family's eight
(peaks ↔ summits with them, a cycle inside the roads crate now) and the names family's eight moved
out.

Rerun on 6 Nov 2026 for 6e's bld: on main at f286dcc6, one cycle of 12 modules, 117.9k lines; after
it, one of 10 modules, 108.1k lines (agent, answers, coord, needs, offload, out, paces, pool,
pubtimes, uses): bld and buildtiles moved out. Every step family is out of pipeline; what is left
in the cycle is the agent, its plan and its runtime (6f).

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
| dem → terrain_deep, terrain_zoom | the terrain's zoom rule (7 references) | built (step 6d): `PACK_Z`, `CELL_FACTOR`, `DEEPEST`, `zoom_for`, `polar_zoom_for` and `Detail` are dem::zoom's (not codecs': `Detail::of` reads dem's declared datasets); terrain_zoom and terrain_deep re-export them |
| inputs → lakefill, rail | 2 references to lakefill, 4 to rail, from the input checks | built (step 6d): all three in the inputs crate |
| cli → polar | one reference to polar | cli stays in pipeline; what the steps below it run programs by, `program` and `TOOLS`, is jobrt::sys's (step 6d; cli re-exports both). #883's steps left edit only `scenic gc`'s flags (cli/scenic.rs), so the rest of cli and net can move any time |
| layers → ovconv | one item (`MAXZ`, in a test) | built (step 6e): `MAXZ` is codecs::vtgen's; ovconv re-exports it |
| terrain_task, trees, bld, offload → coord | offloading a step's parts to other Macs | built (step 6e): what the steps offering tasks name is the offload crate's (`Offload`, its waits, unit's `Run`; on coordwire's real client, not a trait); coord appears only in tests that start a coordinator, which move to pipeline with their family (terrain_task's: `terrain_tests`; trees::task's: `trees_tests`; bld::task's: `bld_tests`) |

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
   resume constants, `DRAIN_GRACE`; cli's `program` and `TOOLS` (step 6d). ~9.6k lines (1 Nov
   2026), no pipeline dependency. Planned: the rest of cli, and net (#883's steps left edit only
   `scenic gc`'s flags, cli/scenic.rs), and nasread's NAS reading (after #923, which edits
   agent/nasread.rs). Offloading a step's parts is no trait here but the **offload** crate (step
   6e: `Offload` on coordwire's client, its waits, unit's `Run`), below the families that offer
   tasks.
4. **buildstore** (the build's files: the NAS/store layer above `store`; built, step 4): out,
   handoff, rawpack, localread, costin, coverage, outlines, fetch, geotiff, `recipes` (the
   `Recipe` type), `inputs_folder` (the inputs folders' predicate), `nasssh` (a bounded `ssh` to
   the NAS, from osmpass), `records` (the records' read side, step 6a: pool/nas.rs, pool::term's
   reading, the choice of the file read; `Member`). ~13k lines (5 Nov 2026), no pipeline
   dependency. Its `Out` is plain: it reads the records itself, and a step's job is handed its
   write set (`Out::open(…, writes)`), so a crate below pipeline can open one.
5. **inputs** (the declared inputs and the steps reading them first; built, step 6d): inputs,
   dem (with the terrain's zoom rule, `dem::zoom`), rules, osmpass, areafiles, rail, lakefill,
   water. ~37k lines with their tests (6 Nov 2026), on det, roadcore, store, names, codecs, jobrt
   and buildstore; no pipeline dependency.
6. Step families. They are not independent: modgraph.py showed trees → terrain (treepacks packs
   its layers by terrain::layers), roads → trees and terrain (unit → trees, landcover, road_terrain;
   stage → the terrain's packs), names → roads and terrain (heritage → stage's `tiles_in`, marksjob
   → candidates' `Cand`, ovconv → timetables' `FERRY_LINES` and layers; gtfs → lakefill, rail), bld
   → names and roads (bld → heritage; buildtiles → reach, stage), and pipeline's offload → bld,
   trees, unit (only `run_task` and `task_kind`: the offload crate the families name is below
   them all). So the crates stack: inputs, then terrain, trees, roads, landmarks (its step 5a part
   now with the names family, so above roads too) and bld, one after another, none
   beside another. Treepacks' use of layers is eight functions (the packs' names, writing and
   dropping them), not one item to move down. An edit in a family still rebuilds every family
   above it and pipeline.
   - **terrain** (built, step 6e): terrain_base, terrain_deep, terrain_pack, terrain_resume,
     terrain_task, terrain_zoom, pyramids, road_terrain, slope_pack, depth, polar, layers. ~17.6k
     lines with their tests (6 Nov 2026), on det, roadcore, store, codecs, jobrt, buildstore,
     inputs and offload. Not stage, peaks and summits: nothing in the family names them; they went
     up with roads.
   - **trees** (built, step 6e): trees, treepacks, leaftype, landcover. ~6.6k lines with their
     tests (6 Nov 2026), on det, roadcore, store, pyport, codecs, jobrt, buildstore, inputs,
     offload and terrain.
   - **roads** (built, step 6e): unit, reach, candidates, elev, stage, peaks, summits, timetables.
     ~5.8k lines with their tests (6 Nov 2026), on det, roadcore, store, codecs, jobrt, buildstore,
     mapview, inputs, offload, terrain and trees. Not buildtiles, as planned first: it reads
     Overture's files through bld's fetch and overture (8 references), so it goes with bld.
   - **names** (heritage, places, transit; built as **landmarks**, the crate `names` being taken):
     its part outside the cycle in step 5a (wikiask, heritagewd, heritage_layers, heritagedetails,
     filterprops, interest, items, pageviews, labels, areadetails, whsshapes), the rest in 6e
     (heritage, ferries, gtfs, railfeeds, ovconv, markconv, marksjob, namestodo). ~15.9k lines
     with their tests (6 Nov 2026), on det, roadcore, store, names, pyport, codecs, jobrt,
     buildstore, coordwire, inputs, terrain and roads.
   - **bld** (built, step 6e): bld (the 3D buildings) and buildtiles (the roadside buildings).
     ~9.6k lines with their tests (6 Nov 2026), on det, roadcore, store, names, pyport, codecs,
     jobrt, buildstore, offload, landmarks, inputs, terrain and roads. The free space its steps
     check before writing (`disk_free`, the NAS reserve's `nas_room`) went first, from agent::room
     to buildstore::room (§6.3).
7. **coordination**: coord, pool, needs, pubtimes, answers, offload, menu, appupdate (coord ↔ pool
   ↔ agent are one cycle today; they split after the steps are out). ~33k lines.
8. **pipeline** (the agent, kept under this name so `cargo run -p pipeline --bin …` and the
   server's dependency stay): agent (66k lines; later split by its own seams, docs/agent-parts.md)
   and the binaries. The scenic metrics' map-side parts (scache, view, terr, buildings: the
   scenic-metrics tool's, and unit's scache) are built as **mapview** (step 5b, ~1.3k lines, on
   codecs, det, roadcore, store). crates/server names none of them; it uses pipeline's inputs,
   depth, water, coord and others, so it keeps its pipeline dependency.

Each moved module stays reachable at its old path by a `pub use` in `pipeline`'s lib.rs while
pipeline's modules above the steps or its binaries name it there. The step modules name the lower
crates directly since step 6b (`crate::pyjson` → `pyport::pyjson`), and the re-exports they alone
used went with it; the rest go as their users move. The inputs crate (6d) they still name by
pipeline's paths (`crate::dem`, ~210 mentions in 23 modules, the depth's among them): each family's
are rewritten as it moves (6e: the terrain, trees, roads, names and bld families'), not in the files others are editing now. The list in lib.rs is the
record of what still goes through them.

## 3. The binaries

| program | lines | uses | its code's crate |
|---|---|---|---|
| scenic | 1.9k | agent, coord, control, pool, budget | pipeline (top) |
| scenic-build | 7.3k | everything | pipeline (top) |
| scenic-metrics | 1.4k | scache, view, terr, whole, trees, unit, control | pipeline (top; uses trees and unit) |
| scenic-polar | 0.1k | polar, dem, resume_log | terrain |
| terrainsub | 0.04k | terrain_task, terrain_pack | terrain |
| coastcheck | 0.5k | lakefill, water, watercov, osmpass | inputs |
| lakefill | 0.4k | lakefill, water, inputs | inputs |
| extract | 1.6k | candidates, elev, trailends, dem | roads |
| elev, tile | 0.06k | elev, road_terrain | roads |
| railfreq | 0.9k | rail | inputs |
| trees, landcover | 0.2k | trees, landcover, fetch | trees |
| bldtile | 0.03k | bld, dem | bld |
| areaflags | 0.01k | areaflags, cli | jobrt |

Since #917 only `scenic` and `scenic-build` are binaries; the other rows are scenic-build's tools
(`scenic-build tool <name>`, src/bin/tools/, `pipeline::cli::TOOLS`), linked into it once. A tool's
file stays with scenic-build; the last column is the crate the modules it uses go to, which a
step moves only with its whole set. Jobs and tasks run a tool by its name (`jobrt::sys::program`,
which pipeline::cli re-exports).

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
   budget::TEST_RESOLVE, timings::reset) sit behind a `testkit` feature. cli and net were left
   for #883 (cli's `program` and `TOOLS` moved in 6d; #883 edits only `scenic gc`'s flags),
   nasread's reading for after #923. The step modules named the
   moved items through the agent's paths (`crate::agent::jobs::report`) until step 6b.
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
   Next: §6, breaking the 47-module cycle: 6a (built) made `Out` nameable below pipeline, 6b
   (built) has the step modules name the lower crates directly, 6c (built) moved the coordinator's
   client side down (coordwire), 6d (built) the inputs family (`crates/inputs`), 6e (built:
   `crates/offload`, `crates/terrain`, `crates/trees`, `crates/roads`, landmarks' names family,
   `crates/bld`) the step families; then (planned) the agent's plan split from its runtime.
6. **coordination**, after #882 lands.
7. Path rewrites: the step modules' were step 6b's; the rest (the agent's, coord's, the
   binaries') per crate as it moves, `crate::x` → `<crate>::x`, and the re-exports dropped.

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

Before step 6a and after step 6b, 6 Nov 2026 07:03-07:13 UTC, on the M4 (load 139-221: the pool's
jobs and other agents' builds), rustc 1.98.1, through tools/dev/bin/cargo. Before: main at bdb8bea1
with 6a's two code commits (7caeb353, de76c1ba) reverted; after: this branch (6a and 6b), each tree a
folder of its own with its own target folder, built once first. Interleaved (before, after; after,
before; before, after), 3 runs each. Peak: the largest process (`time -l`); CPU: user + system of the
whole command. The edits: one comment line appended to marksjob.rs (a leaf step module), to the
terms' reading (pool/term.rs before, buildstore's records/term.rs after: 6a's move) and to the tars
(agent/histarchive.rs before, codecs' tar.rs after: 6b's), each built at once:

| run | before | after |
|---|---|---|
| dev (`build --workspace --bins`), marksjob edit | 10.6, 9.8, 9.1 s (CPU 13.0-13.8 s), 2.22-2.28 GB | 9.7, 10.0, 10.3 s (CPU 13.5-15.7 s), 2.22-2.28 GB |
| unit-test build after the terms' edit | pipeline --lib: 9.5, 8.8, 8.9 s (CPU 9.1-9.2 s), 3.24-3.26 GB | buildstore --lib: 1.2, 1.7, 1.9 s (CPU 1.5-2.2 s), 0.34-0.35 GB |
| unit-test build after the tars' edit | pipeline --lib: 6.9, 6.8, 6.9 s (CPU 8.5-8.6 s), 3.25-3.26 GB | codecs --lib: 0.8, 0.8, 0.8 s (CPU 1.2 s), 0.25 GB |
| first builds, one each (not interleaved) | dev 82.8 s, 3.71 GB; pipeline's lib tests 145.5 s, 4.88 GB | dev 82.4 s, 3.72 GB; pipeline's lib tests 142.6 s, 5.61 GB |

So 6a-6b leave a dev build after a step module's edit where it was (pipeline recompiles either way:
the step modules are still pipeline's), and the moved code's own test loop is 5-9 times faster, its
largest rustc a tenth of pipeline's. The first test builds' peaks (4.88 against 5.61 GB, one run each)
are noise, not 6a-6b: re-measured on the M4 (6 Nov, 08:37-09:07 UTC, load 45-190, its swap all but
full), `cargo test -p pipeline --lib --no-run` from an empty target folder (third-party crates from
sccache), interleaved, the largest rustc pipeline's lib tests each time: before 6a (the "before"
tree above) 5.53, 5.42, 5.90 GB; after 6a (main at 11213648) 5.54, 5.70, 5.24, 5.65, 5.29 GB;
after 6b 4.74, 4.82, 5.63 GB; CPU 745-760 s user in every run. Every crate compiled once a run,
each lower crate's rustc within 0.05 GB of its before (codecs, with tar, 0.52 against 0.50 GB).
tools/dev/check.sh on this branch (6 Nov, 07:41-07:59 UTC, the M4 at load 80-180): fmt 3 s, clippy
38 s, guards 3 s, web 11 s, tests 986 s (1,858 tests of nearly every crate: the branch touches ~160
files, so its affected set is most of the suite; nextest's run itself 263 s, the rest building).
No before figure: a check's time follows its branch's affected set, not 6a-6b.

Before and after step 6c, 6 Nov 2026 11:18-11:28 UTC, on the M4 (load 54-140: the pool's jobs and
other agents' builds), rustc 1.98.1, through tools/dev/bin/cargo. Before: main at 6a706e07; after:
this branch (d4df468f, before its rebase), each tree a folder of its own with its own target folder,
built once first. Interleaved (before, after; after, before; before, after), 3 runs each. Peak: the
largest process (`time -l`); CPU: user + system of the whole command. The edits: one comment line
appended to marksjob.rs (a leaf step module), and to the client (coord/client.rs before,
coordwire's client.rs after), each built at once:

| run | before | after |
|---|---|---|
| dev (`build --workspace --bins`), marksjob edit | 11.5, 10.5, 7.0 s (CPU 13.2-14.6 s), 2.19-2.26 GB | 10.5, 7.0, 10.9 s (CPU 13.5-14.0 s), 1.94-2.27 GB |
| unit-test build after the client's edit | pipeline --lib: 9.9, 7.5, 7.5 s (CPU 9.1-9.7 s), 3.25 GB | coordwire --lib: 0.6, 0.5, 0.6 s (CPU 0.7 s), 0.15 GB |
| first builds, one each (not interleaved) | dev 119 s, 4.20 GB; pipeline's lib tests 172 s, 5.88 GB | dev 83 s, 4.27 GB; coordwire's lib tests 74 s, 0.72 GB |

So 6c leaves a dev build after a step module's edit where it was (pipeline and server recompile
either way), and the client's own test loop is 13-16 times faster, its largest rustc a twentieth of
pipeline's. The first builds' walls are third-party crates from sccache under the load, not 6c.

Before and after step 6d, 6 Nov 2026 07:42-08:50 UTC, on the M4 (load 100-215: the pool's jobs,
other agents' builds and this branch's own check), rustc 1.98.1, cargo through sccache at utility
priority (`taskpolicy -c utility`, as the pool's jobs: so the walls are longer than 6c's). Before:
main at 0875f150 (after 6c); after: this branch, each tree a folder of its own with its own target
folder, built once first. Interleaved (before, after; after, before; before, after), 3 runs each.
Peak: the largest process (`time -l`); CPU: user + system of the whole command. The edits: one
comment line appended to marksjob.rs (a leaf step module) and to rules.rs (a leaf of the family
moved: crates/pipeline/src before, crates/inputs/src after), each built at once:

| run | before | after |
|---|---|---|
| dev (`build --workspace --bins`), marksjob edit | 20.6, 25.2, 20.9 s (CPU 23.4-24.6 s), 2.05-2.11 GB | 18.1, 16.4, 17.4 s (CPU 20.9-21.3 s), 1.60-1.67 GB |
| dev, rules edit | 19.9, 25.2, 19.8 s (CPU 22.6-24.4 s), 1.87-2.05 GB | 15.4, 17.3, 19.3 s (CPU 22.5-23.4 s), 1.60-1.62 GB: inputs, pipeline, server |
| unit-test build after the rules edit | pipeline --lib: 20.4, 23.0, 20.3 s (CPU 17.3-18.7 s), 2.74-3.02 GB | inputs --lib: 4.7, 4.1, 5.9 s (CPU 4.3-4.7 s), 0.76-0.78 GB |
| first builds, one each (not interleaved) | dev 235 s, 3.90 GB; pipeline's lib tests 662 s, 4.48 GB | dev 316 s, 3.08 GB; inputs' lib tests 198 s, 1.86 GB |

So 6d is the first step to shrink pipeline's own compile: a step module's edit rebuilds a smaller
pipeline (CPU ~12 % less, its rustc 0.4 GB smaller, the dev build's largest process 2.1 → 1.6 GB);
an edit in the family costs a dev build what it did (inputs, then pipeline and server, recompile),
and its unit tests build 4-5 times faster, their rustc a quarter of pipeline's. The first builds'
walls follow the load (215 at the end), not 6d; their peaks (3.90 → 3.08 GB dev, pipeline's lib
the largest either way) are 6d's.

Before and after step 6e's terrain, 6 Nov 2026 09:28-09:55 (local) on the M4 (load 70-145: the pool's
jobs and three agents' checks), rustc 1.98.1, through tools/app/remote-tests.sh (nice 20, utility
priority, sccache). Before: main at 8d97cc8e (after 6d); after: this branch (57f051c0), each tree a
folder of its own on the M4 with its own target folder, built once first. Interleaved (before,
after; after, before; before, after), 3 runs each. Peak: the largest process (`time -l`); CPU: user
+ system of the whole command. The edits: one comment line appended to marksjob.rs (a leaf step
module) and to road_terrain.rs (a leaf of the family: crates/pipeline/src before, crates/terrain/src
after), each built at once:

| run | before | after |
|---|---|---|
| dev (`build --workspace --bins`), marksjob edit | 18.6, 16.2, 14.7 s (CPU 19.9-21.3 s), 1.67-1.74 GB | 15.2, 15.2, 26.5 s (CPU 19.8-22.1 s), 1.59-1.65 GB |
| dev, road_terrain edit | 16.3, 13.8, 13.1 s (CPU 20.0-20.5 s), 1.74 GB: pipeline, server | 14.7, 16.7, 18.0 s (CPU 20.7-22.0 s), 1.59-1.60 GB: terrain, pipeline, server |
| unit-test build after the road_terrain edit | pipeline --lib: 15.8, 13.6, 19.7 s (CPU 13.9-15.2 s), 2.56-2.57 GB | terrain --lib: 3.0, 2.8, 3.2 s (CPU 2.8-3.0 s), 0.43 GB |

So 6e's terrain leaves a dev build after an edit where it was, within the load's noise (pipeline and
server recompile either way; an edit in the family adds terrain's own ~1 s), with pipeline's rustc
~0.1 GB smaller; the family's unit tests build 5 times faster, their rustc a sixth of pipeline's.

Before and after 6e's trees, roads and names, 6 Nov 2026 11:36-11:42 (local) on the M4 (load 14-40),
rustc 1.98.1, through tools/app/remote-tests.sh (nice 20, utility priority, sccache). Before: main at
dd7246c6 (after 6e's terrain); after: this branch, each tree a folder of its own on the M4 with its
own target folder, built once first. Interleaved (before, after; after, before; before, after), 3
runs each. Peak: the largest process (`time -l`); CPU: user + system of the whole command. The
edits: one comment line appended to a leaf of each family (landcover.rs, elev.rs, markconv.rs:
crates/pipeline/src before; crates/trees, crates/roads, crates/landmarks after), each built at once:

| run | before | after |
|---|---|---|
| dev (`build --workspace --bins`), landcover edit | 9.7, 7.5, 4.5 s (CPU 11.0-14.1 s), 1.59-1.65 GB: pipeline, server | 10.2, 4.8, 4.9 s (CPU 12.3-17.0 s), 1.36-1.39 GB: trees, roads, landmarks, pipeline, server |
| dev, elev edit | 4.5, 4.7, 4.7 s (CPU 11.0-11.2 s), 1.59 GB | 4.8, 4.6, 5.5 s (CPU 12.1-13.1 s), 1.36 GB: roads, landmarks, pipeline, server |
| dev, markconv edit | 4.6, 4.8, 5.6 s (CPU 11.0-12.6 s), 1.59-1.60 GB | 5.1, 5.5, 5.4 s (CPU 12.6-13.2 s), 1.36 GB: landmarks, pipeline, server |
| unit-test build after the landcover edit | pipeline --lib: 7.9, 5.8, 4.6 s (CPU 6.0-7.7 s), 2.32-2.35 GB | trees --lib: 1.1, 0.6, 0.6 s (CPU 1.1-1.3 s), 0.27 GB |
| … after the elev edit | pipeline --lib: 4.6, 4.6, 4.9 s (CPU 6.0-6.2 s), 2.33 GB | roads --lib: 1.1, 0.6, 0.6 s (CPU 1.1-1.6 s), 0.27 GB |
| … after the markconv edit | pipeline --lib: 4.7, 4.9, 5.3 s (CPU 6.1-6.8 s), 2.32 GB | landmarks --lib: 1.8, 0.8, 0.7 s (CPU 1.3-2.5 s), 0.35 GB |

So the three families' moves leave a dev build after an edit in them where it was in wall time
(the families stack, so an edit recompiles the crates above it, ~1-2 s of CPU more), with
pipeline's rustc, the build's largest, 1.59 → 1.36 GB; their unit tests build 5-8 times faster,
their rustc a seventh of pipeline's.

Before and after 6e's bld, 6 Nov 2026 13:05-13:14 (local) on the M4 (load 50-105: the pool's jobs, a
publish's tests and other agents' checks), rustc 1.98.1, through tools/app/remote-tests.sh (nice 20,
utility priority, sccache). Before: main at f286dcc6; after: this branch (a26815a6), each tree a
folder of its own on the M4 with its own target folder, built once first, and each run's two
builds made fresh before its edits. Interleaved (after, before; before, after; after, before), 3
runs each. Peak: the largest process (`time -l`); CPU: user + system of the whole command. The
edits: one comment line appended to pubtimes.rs (a pipeline module, in both) and to bld/work.rs (a
leaf of the family: crates/pipeline/src before, crates/bld/src after), each built at once:

| run | before | after |
|---|---|---|
| dev (`build --workspace --bins`), pubtimes edit | 30.2, 14.2, 7.7 s (CPU 15.5-21.8 s), 1.36-1.37 GB: pipeline, server | 13.1, 10.8, 9.0 s (CPU 16.7-18.3 s), 1.31 GB: pipeline, server |
| dev, work edit | 26.4, 13.1, 7.3 s (CPU 15.7-20.3 s), 1.36-1.37 GB: pipeline, server | 14.3, 10.8, 10.6 s (CPU 18.3-19.4 s), 1.31-1.32 GB: bld, pipeline, server |
| unit-test build after the work edit | pipeline --lib: 23.6, 12.8, 6.8 s (CPU 8.5-12.3 s), 2.03-2.04 GB | bld --lib: 2.5, 1.9, 1.8 s (CPU 2.3-2.4 s), 0.34-0.35 GB |
| first builds, one each (not interleaved) | dev 266 s, 2.27 GB; pipeline's lib tests 647 s, 3.90 GB | dev 260 s, 2.12 GB; bld's lib tests 436 s, 1.89 GB |

So bld's move leaves a dev build after an edit where it was within the load's noise (pipeline and
server recompile either way; an edit in the family adds bld's own ~1-2 s of CPU), with pipeline's
rustc 1.36 → 1.31 GB; the family's unit tests build 4-7 times faster on ~a fifth of the CPU, their
rustc a sixth of pipeline's. The first builds' walls are third-party crates from sccache under the
load, not 6e.

The machine's load (other agents' builds, 30-125) swamps any difference step 1 could make; these
are a baseline of the method, not evidence either way. What follows from the structure, not the
timings: an edit in a crate recompiles every crate that depends on it, so moving a *leaf* module
down (step 1's pyjson, step 2's hipack) does not spare the pipeline's recompile after an edit to it. The gains come
when the modules edited most (the steps, the agent) sit in crates that few or no others depend on,
and the big middle (inputs, terrain) compiles beside the other families: steps 3-6.

## 6. Breaking the big cycle (6a-6e built; 6f in parts: 6f1 and 6f2 built, 6f3 and 6f4 planned)

**Status: 6a-6e, 6f1 and 6f2 built; 6f3 and 6f4 planned** (design of 5 Nov 2026, after steps 5a-5b). Figures are from main at 3763caa5;
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
   leaves, **coordwire** (coord's client side), and **offload** (above coordwire: what the
   families offering tasks name, built in 6e);
2. **inputs**;
3. the families **terrain**, then **trees**, **roads**, **names** (landmarks grows into it),
   **bld**; and, beside them, **agentcore** (the agent's runtime, coord, pool and the rest of
   that row). Not pipeline's offload, which names the families: a task's kind (`task_kind`, the
   agent's only use) reads the families' `KIND`s through `Plan` as the other family constants
   do (below), and running a task on a worker (`run_task`, over unit's steps; only the `scenic`
   binary calls it) stays pipeline's;
4. **pipeline**: the agent's plan, the binaries, the re-exports. It names everything; nothing
   names it.

The choices, against the alternatives:

- **`Out` stops being generic** (built, 6a). Neither a registry nor `Above` across crates:
  `Out<Pipeline>` couldn't be named below pipeline, and making every family generic over
  `A: Above` would compile all of them in pipeline. What `Above` gave comes instead from: the
  records, read by buildstore itself (below); the write set, a plain `out::Writes`
  (`fn(&str, &str, bool) -> Option<bool>`) handed to `Out::open(root, scratch, step, writes)`,
  which only scenic-build (`step_main`) and agent/gc call outside tests, both through
  `pipeline::out::open(root, scratch, step)` (it passes `agent::steps::writes`); `Out::unbound`
  (the steps' own code) needs neither; and `TESTING`, a process flag `buildstore::out::testing()`,
  `cfg!(test)` in buildstore's own tests and set otherwise only by the `#[cfg(test)]` start-up
  constructor each crate has for `fetch::go_offline` (pipeline's and landmarks' `TEST_START`,
  their test_start.rs probes checking it; every new crate gets one); tests/test_mode.rs checks
  it's off in a build with the dev-dependencies. The family tests that check their step's writes
  against the real table call `crate::out::open`, pipeline's; they move to a pipeline test module
  when their family leaves, not before: inputs' and rail's went to `inputs_tests` with 6d (a unit
  test module, not tests/write_sets.rs: they call the gate's and rail-feeds' public items, but
  write_sets.rs is a scan); timetables' to `roads_tests` and ovconv's to `names_tests` (6e).
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
- **Items the steps name from the agent move to their owners** (built, 6b): the version
  constants to their step (`TERRAIN_V` terrain_pack's, `SLOPE_V` and `SLOPE_LO_V` slope_pack's;
  the plan and the steps table import them), `grown_e7` to roadcore::geo, `coverage_tiles` (with
  `near_coverage`, from terrain_pack) to buildstore::coverage, `h`, `ahead_on` (with
  `AHEAD_ENV`) to jobrt::jobs, `app_version` to jobrt::sys, `nasread::timed_out` to
  buildstore::records::nas, the depth's read boxes (`depth_hi_box_read`,
  `depth_lo_box`) to depth::build, and the in-memory tars (`histarchive::{build, list_tar}`, which
  depth::lakes writes its grids with) to codecs::tar. `publish::map_now` (namestodo; it asks the
  lead's coordinator) went with the client to coordwire in 6c (`coordwire::map_now`; not to
  buildstore, as planned first: the client is all it needs beyond store, and buildstore needs no
  client). `cli::program` (osmpass; with `TOOLS`) went to jobrt::sys in 6d, and
  `room::{disk_free, nas_room}` (buildtiles, bld::fetch) to buildstore::room in 6e, `disk_free`
  answering a test's own disk (`TEST_FREE`) by the process flag `out::testing` rather than
  pipeline's `cfg(test)`. No step's code calls
  the plan's functions (`PassCtx`, `spoken_work`, `outlines_work`, `names_todo_work`,
  `trees_targets`) or `input_digests`: they stay the plan's. Tests in step modules that test the
  plan or the agent (lakefill's of `build::water_work` and `LAKEFILL_HOLDS`, inputs' of
  `steps::row`, trees' of `room::make_room`) move to pipeline's tests with their family, as the
  write-set tests do: lakefill's and inputs' to `inputs_tests` (6d), trees' to `trees_tests` (6e).
- **coordwire** (built, 6c): the client's transport (coord/client.rs's `Client`: the contact
  read from the NAS, the rounds, the kept connections, the files' transfers; `Answered`,
  `Reply`, `Files`), coord/task.rs's client half (`Offer`, `Output`, `clone`, `safe`, `beats`,
  `MARGIN`), the contact (`Contact`, `contact_path`, `POOL_TERM`, `POOL_MEMBER`) and `map_now`;
  coord re-exports what it and the agent name. Not a trait object: offload needs the real client,
  and the client is small. Not the wire types (`Ask`, `Grant`, `Granted`, `Done`, `Fail`, the
  work beat, held's `Beat`/`Answer`), as planned first: they reach the agent's runtime (`Ask`'s
  `Capacity` names `forecast::MacRoom`, `Done` and `Fail` `memguard::Floor`, the beat
  `jobs::Live`, whose methods are in two agent modules, held's `agent::pool::Heartbeat`), and
  below pipeline only the families' coordinator tests name them. So the lease calls that carry
  them (`ask`, `beat`, `beat_paused`, `beat_told`, `pool_beat`, `cancel`, `set_pause`, `done`,
  `fail`, `fail_with`, `give_back`, `give_back_with`, `end_with`) stay in coord, bodies
  unchanged, as the trait `coord::client::LeaseCalls` on coordwire's `Client` (with `Refused`,
  `Handed` and `probe`); they go with coord into agentcore (6f). The test coordinators
  (`answering`, `mute`, `dead`, `serve`) are `coordwire::testkit` (its `testkit` feature).
  Tests that start a coordinator (`coord::start_for_test` in terrain_task, trees::task, bld and
  offload) move to a pipeline test module when their family leaves (6e), as the write-set
  tests do, since a family can't dev-depend on pipeline (its types would be two copies):
  terrain_task's are pipeline's `terrain_tests`, trees::task's `trees_tests` and bld::task's
  `bld_tests` (6e); offload's stay in pipeline's offload,
  building their `Offload` by the offload crate's testkit (`Offload::for_test`).
- **The agent's plan above its runtime, through a trait.** agentcore can't name the plan (the
  plan names every family and agentcore). The types the runtime reads (`Step`, `Kind`, `Read`,
  `Place`, `Keys`, `Work`) move into agentcore; the table and its functions stay in pipeline,
  which implements `agentcore::Plan` (the ~49 items as methods: `row`, `shared`, `outside`,
  `saves`, `records_only`, `output_names`, `read_targets`, `renamed`, `made`, …) and
  passes `&PLAN` to the agent's entry points (`agent::run`, the forecast, the simulator, scenic
  menu). Passed, not installed: no link-time constructor (a static in an rlib may be dropped by
  the linker) and no registry to fill in every test. The family constants agentcore names
  (`*::task::KIND`, `piece_peak_mb`, `squares_of`, `DEEP_MIDS_MB`) go through `Plan` too. The
  planner (plan.rs) is the runtime's, not the plan's (§6.3, 6f).

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
   Key check (`bench_plan_on_a_real_root` on a frozen copy of the lead's term 737, seq 65, the M1,
   5 Nov): before (main 7f1dc02c) and after, the same plan and keys, its 3,110 work targets' lines
   byte-identical; an `Out`'s and `read_manifest`'s records the pool's (`Records::now`,
   `Brief::now`) on that state. Measured with 6b (§5).
2. **6b, the paths** (built, 6 Nov 2026): the step modules name the lower crates' items
   directly: the agent's, out's and the pool's re-exporting paths (`crate::agent::jobs::report` →
   `jobrt::jobs::report`, `agent::recipes` → `buildstore::recipes`, `agent::room::{free_past,
   job_target}` → `jobrt::room`, `out::Out` → `buildstore::out::Out`, `out::read_manifest` and
   `pool::{term, is_member_id}` → `buildstore::records`) and pipeline's root re-exports
   (`crate::timings` → `jobrt::timings`, `crate::fetch` → `buildstore::fetch`, `crate::hipack` →
   `codecs::hipack`, …); and the item moves of §6.2. Cut: the re-exports only the step modules
   used (the root's of pyport's four modules, of codecs' chain, stations, vtgen and webp, of
   landmarks' wikiask; agent::jobs' each_line and fraction_of; inputs' in_inputs_folder and
   DROP_ROOTS). modgraph.py's edges from the step modules up (to agent, out, pool, coord, cli and
   the root; tests included): 1,210 mentions before (main at bdb8bea1, after 6a), 96 after, all
   §6.2's: coord 64 (offload's client, terrain_task's, bld's and trees' coordinator tests: 6c),
   out 15 (the family tests' `out::open`, the write-set tests), agent 11 (map_now, disk_free,
   nas_room, and the plan's and agent's tests above), cli 6 (`cli::program`). None to the root
   or the pool. The cycle is still 46 modules: those edges hold it. Pure paths and moves: over the
   branch's Rust diff, with paths reduced to their last name and whitespace dropped, the tokens
   removed and added differ only by imports (`use` lines of the moved items), `pub(crate)` made
   `pub` (h, the depth boxes), the cut re-exports' names, and codecs::tar's `mod` lines; no
   literal, expression or format string changed, so keys, outputs and on-disk formats are as
   before (no frozen-plan run, as 6a's: the diff leaves it nothing to find).
3. **6c, coordwire** (built, 6 Nov 2026): as §6.2 says. Offload names the client and the
   tasks' shared side by coordwire; namestodo and the binaries name `coordwire::map_now`. Cut:
   `agent::publish::map_now`, `coord::client::Files`, coord::task's re-exports of `clone`, `safe`
   and `MARGIN`, `coord::client::tests` (its helpers coordwire's testkit). modgraph.py's edges
   from the step modules up (as 6b counts them, tests included): 96 before (main at 6a706e07), 90
   after: coord 59 (64; all coordinator tests now: offload 24, terrain_task 13, bld 11, trees 11,
   each with the `LeaseCalls` import its test module gained; offload's code's 8 gone), agent 10
   (map_now gone), out 15, cli 6. No step module's code names coord. The cycle is still 46 modules
   (189.1k lines; 190.1k before): the tests' and out's, agent's and cli's edges hold it. Pure
   moves and paths: over the branch's Rust diff, its lines with whitespace dropped, those removed
   and added differ only by paths (`crate::coord::…`, `crate::agent::jobs::now_s` →
   `jobrt::jobs::now_s`, `agent::publish::map_now` → `coordwire::map_now`), imports, `pub`
   (`Client::request`, which `done` calls; the test helpers), the trait's frame (its
   declarations, `impl LeaseCalls for Client`, the lease calls' `pub` dropped) and docs; no body,
   literal or format string changed, so requests, keys and on-disk formats are as before.
   Measured in §5.
4. **6d, inputs** (built, 6 Nov 2026): inputs, dem, rules, osmpass, areafiles, rail, lakefill
   and water into `crates/inputs`; pipeline re-exports the eight (`pub use ::inputs::{…}`, the
   crate's path: it holds a module of its own name). First, one move a commit, the family's edges
   up: `cli::program` and `TOOLS` to jobrt::sys (#883 edits only `scenic gc`'s flags, so this
   needn't wait for it); the terrain's zoom rule to dem::zoom (`PACK_Z` from terrain_deep;
   `CELL_FACTOR`, `DEEPEST`, `zoom_for`, `polar_zoom_for`, `Detail` from terrain_zoom), not to
   codecs as planned: `Detail::of` reads dem::declared's datasets and `zoom_for` dem::generic's
   tile resolution; a polar block's zoom and a pole's blocks (`BLOCK_Z`, `blocks`) to
   roadcore::polar beside `cap_tiles`; a lake-estimate source's format and method (`FORMAT`,
   `Method`, `GLOBATHY_Z`, `SMALL_Z`) from depth::lakes to inputs::estimate; `water_pin` from
   terrain_pack to lakefill; each old home re-exports what others still name there. The tests
   needing pipeline (the gate's three job tests and rail-feeds' three, through `crate::out::open`;
   lakefill's two of the plan) moved verbatim to pipeline's `inputs_tests`. The crate's unit tests
   start offline (lib.rs `TEST_START`, test_start.rs); the fixtures pipeline's tests use
   (inputs::elevation's, inputs::heritage's and inputs::sources' `accept_fixture`, sources'
   `STAGED`, `accept_files`, `staged`) sit behind its `testkit` feature. The scan tests
   (tests/common's `test_lines`) take an item under exactly `#[cfg(any(test, feature =
   "testkit"))]` as test code, as a `#[cfg(test)]` one (any other `any(test, …)` is scanned), and
   tests/test_mode.rs checks that only dev-dependencies turn a `testkit` on, so no release build
   has one. i_overlay went with the regions. tests/grant_rules.rs, and guards.sh's list of the
   Python steps, now read every crate's src. Cut: terrain_pack's `water_pin` path
   (no other user); nothing else was dead: every re-export left has users above. modgraph.py
   (main at 0875f150, after 6c): the family's mentions of the rest of pipeline, tests included,
   39 before (inputs → out 8, depth 4, agent 1, terrain_deep 1; lakefill → agent 6, terrain_pack
   4; areafiles → polar 4, terrain_deep 1; dem → terrain_zoom 4, terrain_deep 1; rail → out 4;
   osmpass → cli 1), none after (another crate: the compiler holds it); the cycle 46 modules
   (189.0k lines) before, 35 (146.7k) after: the family's seven in it, and ferries, gtfs,
   landcover and namestodo, whose way back into it went through the family. Pure moves and paths:
   the eight modules' files are renames, byte-identical but for rustfmt's lines in three files
   the earlier commits touched and the six fixtures' `cfg` lines; over the branch's Rust diff,
   with paths reduced to their last name and comments and whitespace dropped, the tokens removed
   and added differ only by imports and re-exports (`use`, `pub use`), `mod` lines, the
   fixtures' `cfg(any(test, feature = "testkit"))`, the new lib.rs and test_start.rs (pipeline's
   and landmarks' pattern), grant_rules' loop over the crates and the scan tests' paths; no
   literal, expression or format string of non-test code changed, so keys, outputs and on-disk
   formats are as before (no frozen-plan run, as 6b's: the diff leaves it nothing to find).
   Measured in §5.
5. **6e, the step families.** Terrain first (built, 6 Nov 2026), one move a commit:
   - `tile_of` (a point's tile at a zoom) from bld to roadcore::geo, which terrain_zoom's test
     helper `tile_at` named; `MAXZ` from ovconv to codecs::vtgen (layers' real-data test; ovconv
     re-exports it): §1's `layers → ovconv`.
   - **offload** (`crates/offload`): what the families offering tasks name, which terrain_pack
     and terrain_task did (`Offload`, `Offered`, `Patience`, `Settled`, `Waiting`, `WaitNote`,
     `owner`, `places`, `LEASE_WAIT`, `WAITING_ENV`, the files a task reads) and a task's step
     (unit's `Run`). Not the family it was planned as, above the others: they all name it, so it
     goes below them. Running a task on a worker (`run_task`, over unit's steps) and a task's kind
     (`task_kind`, which names the families' `KIND`s) stay pipeline's offload, which re-exports
     the rest; unit re-exports `Run`. The tests needing no coordinator went with the code; those
     starting one stay, their `Offload` built by the crate's testkit (`for_test`, `client`).
   - terrain_task's tests that start a coordinator, with their helpers, to pipeline's
     `terrain_tests`; the helpers other modules' tests name out of the test modules into a
     `testkit` module each (terrain_task's `north_piece`, terrain_pack's `synthetic_raw` and
     `near_without_hi`).
   - the family's paths: `crate::dem`, `crate::inputs`, `crate::lakefill` → the inputs crate's,
     `crate::offload` and `crate::unit::Run` → the offload crate's, and grant_tests' re-exported
     paths → jobrt's and buildstore's.
   - the move: terrain_base, terrain_deep, terrain_pack, terrain_resume, terrain_task,
     terrain_zoom, slope_pack, layers, pyramids, depth, polar and road_terrain, with grant_tests,
     into `crates/terrain`; pipeline re-exports the twelve (`pub use terrain::{…}`). Their files
     are renames, byte-identical but for the helpers' `cfg` lines (`TEST_BASE`, the testkit
     modules, terrain_zoom's `test_world`, `test_world_ref`, `tile_at`, behind the crate's
     `testkit`) and terrain_resume's `inputs` and `inputs_read` made `pub` (agent::build names
     them). Unit tests start offline (lib.rs `TEST_START`, test_start.rs). GEOS went with
     depth::lakes, webpenc with depth::fall.
   Cut: pipeline's webpenc dependency (no other user); bld's `tile_of` (no re-export: its two
   other callers name roadcore's). Nothing else was dead: every re-export left has users above.
   modgraph.py (main at 8d97cc8e, after 6d): the family's mentions of the rest of pipeline, tests
   included, 104 before (80 of the root's re-exports of the inputs crate and the lower ones;
   terrain_task → coord 13, offload 5, unit 1; terrain_pack → offload 1; terrain_zoom → bld 1;
   layers → ovconv 1; grant_tests → agent 1, out 1), none after (another crate: the compiler
   holds it); the cycle 35 modules (146.7k lines) before, 21 (127.9k) after (§1). Pure moves and
   paths: over the branch's Rust outside tests (`#[cfg(test)]` and testkit items, test files),
   with comments, `use`/`mod`/attribute lines and visibility dropped and every path reduced to
   its last name, the tokens removed: none; added: pipeline's `pub use terrain::{…}` alone. So
   no literal, expression or format string changed, and keys (`TERRAIN_V`, `SLOPE_V`,
   `SLOPE_LO_V` and the rest as they were), outputs and on-disk formats are as before (no
   frozen-plan run, as 6b's and 6d's: the diff leaves it nothing to find). Measured in §5.
   Then trees, roads and names (built, 6 Nov 2026), each as terrain went: its tests needing
   pipeline to a pipeline test module, its paths, then the move.
   - **trees** (`crates/trees`): trees::task's tests that start a coordinator and trees' test of
     room-making mid-run (`agent::room::make_room`) to pipeline's `trees_tests`, the helpers they
     and treepacks' tests name out of the test modules into a `testkit` module each (trees'
     staged sources and squares with the TIFF writers they use, trees::task's coverage,
     and accessors of trees::task's `block_files` and trees' `ROWS_DIR`, which stay private as
     before); the paths
     (`crate::inputs`, `dem`, `osmpass` → the inputs crate's, `layers` → terrain's, `offload` and
     `unit::Run` → the offload crate's, `cli::program` → jobrt::sys's); the move of trees,
     treepacks, leaftype and landcover. Not the cut planned (treepacks → layers' names to codecs):
     treepacks names eight of layers' functions, not one item, so trees sits on terrain.
   - **roads** (`crates/roads`): timetables' job test (its pieces open their `Out` by
     `crate::out::open`) to pipeline's `roads_tests`; the paths (to the inputs, terrain and trees
     crates' and jobrt::sys's); the move of unit, reach, candidates, elev, stage, peaks, summits
     and timetables. Not buildtiles (it goes with bld, §2).
   - **names** (into `crates/landmarks`): ovconv's stale-sweep test (`crate::out::open`) to
     pipeline's `names_tests`, ovconv's `drop_stale` (private, as before) reached through
     landmarks' testkit (`ovconv::testkit`, behind its new `testkit` feature); the paths (to the
     inputs, terrain and roads crates'); the move of heritage, ferries, gtfs, railfeeds, ovconv, markconv,
     marksjob and namestodo, their mentions of landmarks' own modules (`landmarks::interest`)
     made `crate::`. Not the cuts that would have names compile beside roads: heritage names
     stage's `tiles_in`, marksjob candidates' `Cand`, ovconv timetables' `FERRY_LINES`, three
     items of three modules; landmarks sits on roads.
   Pipeline re-exports eighteen of the twenty modules at their old paths (`pub use ::trees::{…}`,
   `roads::{…}`, `landmarks::{…}`); each new crate's unit tests start offline (lib.rs `TEST_START`,
   test_start.rs), the trees and landmarks crates' testkit modules behind their `testkit`
   features. Cut: pipeline's blake3 dependency (reach was its last user), and the re-exports of
   leaftype and ferries (nothing names them by pipeline's paths); every re-export left has users
   above, and libz-sys and image-webp kept users in pipeline (bld::fetch's gzip, the fields'
   tests; both went with bld, below). modgraph.py (main at dd7246c6): the families' mentions of the rest of pipeline, tests
   included, 58 for trees (39 of the root's re-exports of the lower crates; coord 11, offload 5,
   cli, agent and unit 1 each), 40 for roads (root 31, cli 4, out 2, trees and landcover 3), 41
   for names (root 34, stage 4, candidates, timetables and out 1 each); none after (other crates:
   the compiler holds it); the cycle 21 modules (128.3k lines) before, 12 (117.6k) after (§1).
   Pure moves and paths: each moved module compared with main's token by token, comments dropped
   and every path reduced to its last name: the same tokens but for `use` order (reach, stage,
   ferries, gtfs::ferrycount, marksjob, ovconv), and the testkit modules and the moved tests
   (trees, trees::task, trees::tests, timetables, ovconv). So no literal, expression or format string changed, and keys, versions,
   outputs and on-disk formats are as before. Measured in §5.
   - **bld** (built, 6 Nov 2026; `crates/bld`, on landmarks and roads): it reached back into
     pipeline beyond its tests: buildtiles called `agent::room::disk_free` and bld::fetch
     `agent::room::nas_room` (the NAS reserve: `nas_reserve`, `nas_free`, the `Target` file's
     parsing), and `disk_free` answered a test's own disk only under pipeline's `cfg(test)`
     (`TEST_FREE`, 1 TB unless a test sets it). Untangled first, one commit each:
     - `disk_free`, `TEST_FREE` and the NAS reserve (`NAS_RESERVE`, `set_nas_reserve`,
       `nas_reserve`, `nas_room`, `nas_free`, `Target`, `parse_target`) to buildstore::room, where
       the process flag is: `disk_free` answers `TEST_FREE` when the process is a crate's unit tests
       (`out::testing`, set by every crate's `TEST_START` and nothing else), the same processes
       `cfg(test)` chose; `TEST_FREE` in a private module, offered to pipeline's tests by
       `buildstore::room::testkit`. agent::room re-exports what pipeline names. `set_nas_reserve`
       went too (not planned): the reserve's file is written and read in one place.
     - bld::task's test that starts a coordinator to pipeline's `bld_tests`, the helpers it and the
       family's other tests name into testkit modules (bld::prep's buildings and frames,
       bld::task's work files and coverage), and what it reads of an `Offers` (its `out` and
       `records`, `here_s`) through task::testkit's accessors, so they stay private.
     - the paths (`crate::heritage` → landmarks', `reach` and `stage` → roads', `unit::Run` and
       `offload` → the offload crate's, `inputs` → the inputs crate's, `layers` → terrain's,
       `agent::room` → buildstore::room's), then the move of bld and buildtiles into `crates/bld`.
     Pipeline re-exports the two (`pub use ::bld::{bld, buildtiles}`); the crate's unit tests start
     offline (lib.rs `TEST_START`, test_start.rs); its testkit modules and buildtiles'
     `TEST_RELEASE` (the agent's tests name it) behind its `testkit` feature. GEOS (with the
     geos-sys and geos-src pins), libz-sys, parquet, arrow-array, md-5, bytes and crc32fast went
     with it, image-webp to its dev-dependencies. Cut: those dependencies of pipeline (no other
     user), pyport's (no user once bld left) and toml as a normal dependency (only
     tests/source_names.rs reads TOML: a dev-dependency now); agent::room's `walk_for_test` (no
     caller) and `make_room_with` outside tests (only tests call it). Every re-export left has
     users above. modgraph.py (main at f286dcc6): the family's mentions of the rest of pipeline,
     tests included, 58 (bld 48: the root's re-exports 27, coord 11, offload 9, agent 1;
     buildtiles 10: root 9, agent 1), none after (another crate: the compiler holds it); the cycle
     12 modules (117.9k lines) before, 10 (108.1k) after (§1). Pure moves and paths: each moved
     module compared with main's token by token, comments dropped and every path reduced to its
     last name: the same tokens in eleven of the fifteen files; job by a trailing comma (rustfmt's,
     a signature now on one line), buildtiles by `use` order and `TEST_RELEASE`'s cfg, prep and
     task by the testkit modules (their cfg, the helpers' `pub`, task's three accessors) and
     task's coordinator test, now pipeline's. buildstore::room's bodies are agent::room's but for
     `disk_free`'s test branch (`cfg(test)` → `testing()`) and the paths of `now_s`, `free_bytes`
     and `MAX_TARGET` (jobrt's, which agent's re-exported). So no literal, expression or format
     string changed, and keys (`BLDPREP_V`, `BUILDINGS_V`), outputs, on-disk and wire formats are
     as before. Measured in §5.
6. **6f, agentcore and `Plan`** (in parts; its order decided 6 Nov 2026 on main at 34a47749): the
   last and largest step. It needn't wait for #882 (backgrounded): nothing left in it edits what
   #882's later steps would. modgraph.py then: one cycle of 10 modules, 108.0k lines (agent 70.2k,
   coord 21.0k, pool 9.6k, needs, answers, pubtimes, uses, paces, offload, out). What the runtime
   names of the plan and the families, counted then by hand (`build::`, `steps::`, `tiles::` paths
   and their imports; `crate::<family module>`; non-test code unless said):
   - **plan.rs is the planner, not the plan**: the **plan** part's thread (`Planner`, `Handle`,
     its messages `Msg`, `Inputs`, `PlanView`, `Out`). It names coord (19 mentions), pool, needs,
     pubtimes and control, and the runtime's items in agent/mod.rs (`Holds`, `Coverages`,
     `GuardHolds`, `edit_held`, `write_published`, `load_regions`,
     `PASS_SPACE`, `PROXY_REPLAN`, `Forecasted`), forecast's lanes and redo's `done_of`, so it goes
     down with the runtime, its calls of the plan through `Plan`. The plan that stays in pipeline
     is build.rs, steps.rs, idents.rs and tiles.rs (17.3k lines with their tests), not §6.1's
     19.5k.
   - The runtime names the plan's items: in plan.rs ~60 distinct ones (the `*_work` functions,
     `plan`, `plan_with`, `KeyCtx`, `KeyMemo`, `PassCtx`, `Checklist`, `Publishing`, the table's
     readiness …); elsewhere ~100 mentions in 74 functions (agent/mod.rs 48, then forecast, pool,
     builtview, redo, flags, every, coord/mod.rs and sched.rs, needs, jobs, pool/sim.rs and
     records.rs, room, publish, fccheck, pubtimes, out, nasread, asks): the types it holds (`Work`,
     51 mentions with tests; `Keys`, 30; `Line`, `Item`, `PublishLine`, `RegionState`,
     `LastCatalogue`, `ReadTarget`, `Read`, `Along`) and the table's queries (`row`, `task`,
     `current`, `SHARED`, `TASKS`, `records_only`, `saves`, `outside`, `read_targets`,
     `output_names` …).
   - The families, ~15 items: the task kinds (agent's `task_what` and helpers' need, offload's
     `task_kind`), terrain_pack's `piece_peak_mb` and `DEEP_MIDS_MB` (`terrain_peak`, which only
     the planner calls), `timetables::View` (a field of `Status` and `PlanView`), heritage's
     `registers_now` and bld's `sources` (`input_digests`), trees' `squares_of` and `chm_name` and
     stage's `MARGIN_KM` (room's hints), namestodo's `INPUTS` (publish's watches),
     `reach::Reaches` (pubtimes, the planner), buildtiles (the planner), depth's layer names (gc),
     and terrain_deep's `PACK_Z` (dem::zoom's already). More in tests (unit's `tail`, the tasks'
     `spec`s, reach, `buildtiles::TEST_RELEASE`).
   - Test time inside non-test code: `cfg(test)` constants, fields and branches (`JOIN_WAIT`,
     `JOIN_TRY`, `Options::member`, …: ~30 in agent/mod.rs, 19 in coord/mod.rs, more in tiles,
     nasread, jobs, room, gc, pool), which pipeline's tests stop seeing once the code is another
     crate's, as 6e found for `disk_free`.
   So 6f goes in parts, each shipping alone:
   1. **6f1, the types the runtime holds go to their owners.** Built (6 Nov 2026), one move a
      commit: `Work` to agent::jobs (a `JobSpec`'s `record`); `Keys` (with its loading, saving and
      recording) and the step renames (`RENAMED`, `current`, which `Keys::per_target` reads) to
      pool::records (the records carry them: `Records::keys`), steps re-exporting `current` for
      the plan and scenic; the runtime names them there, as do pipeline's inputs_tests and the
      checklist example; and `terrain_deep::PACK_Z` by its home's path (`dem::zoom`). Cut: build's
      imports of `TERRAIN_V`, `SLOPE_V` and `SLOPE_LO_V`, unused since 6b. The review cut
      `Keys::record`'s list of 25 steps and the roots whose keys go with the lo keys: its last
      branch does the same for every step without a key per target. Then publishing's types to
      agent::publish, beside its switches (`Publishing`, `LastCatalogue` with `Ran` and `GATED`,
      the steps its record covers, `PublishLine`, `PublishRegion`, and the cadence constants both
      sides read: `CATALOGUE_EVERY_S`, `CATALOGUE_CAP_S`, `LOW_EVERY_S`, `MARKS_EVERY_S`,
      `OVERLAYS_EVERY_S`, `NAMES_TODO_EVERY_S`); the checklist's lines (`Item`, `Line`, the
      planner's `Checklist` pair) and the regions' counts (`RegionState`) to a runtime module of
      their own, agent::progress (their sayings still read the table's nouns, `steps::noun`: a
      query for `Plan`); `Place` (with `place`, which reads `every::offered`) to agent::every and
      `ReadTarget` to agent::forecast; and `timetables::View` (with `Unmatched`, `VIEW_MAX` and
      the report's name, `REPORT`) to inputs::gtfs, the timetables input it answers
      (docs/inputs.md §6.7.3), a crate below both the roads and the runtime, roads::timetables
      re-exporting `REPORT`. Decided so rather than carrying the field as the plan made it (a JSON
      value the status would parse again): the type is the status's, and the planner's read of
      the report then names no family. The runtime's lines naming the plan (`build::`, `steps::`,
      `idents::`, `tiles::`; comments aside, tests and plan.rs included) 288 → 184 with the first
      part, → 145 after; the pool's edge to agent::build gone. Pure moves: over each commit's Rust
      diff, comments and whitespace dropped and every path reduced to its last name, the tokens
      removed and added differ only by imports, the new module's line, `pub(super)` on the two
      `Item` constructors the plan calls, and inputs_accessor's exemption, whose file is now
      records.rs; so jobs.json, the records, the keys, a job's record, publishing.json and the
      status are as before. Not measured: nothing left pipeline, so no crate's compile changed
      (§5's figures come with 6f4). Not moved, as they are the plan's: `Step` and `Read` (a row's
      reads and idents are functions over the plan's `KeyCtx`), `Along` (its data, the forecast's
      input, went to the seam with 6f2; its filling reads the table and `KeyCtx`), `Kind`
      and `Scope`. The second review made progress's `grouped` private: nothing else calls it.
      What the runtime asks of them (a step's kind a fetch: needs' `Io`, sched's
      downloads; the first read naming a target: needs' `source_secs`; a row's `cost_v`; a
      target's reads along its ways) are `Plan`'s questions (6f2).
   2. **6f2, `Plan`** (built, 6-7 Nov 2026, in parts: the merge, the frozen-plan check, the trait):
      the trait, in the runtime, implemented by the plan; `&'static dyn
      Plan` held by the agent, the planner and coord's scheduler, passed to the forecast and by
      their entry points. Cut at the planner's questions, not at the plan's items: of
      plan.rs's ~60, many were `Planner::region_jobs` (~900 lines) and `checklist` composing the
      worldwide steps one by one (`outlines_work` … `names_todo_work`, their holds and waits), and
      `checklist` asked the same `*_work` calls again for its lines. That composition is the
      plan's, so `Plan` carries it, `plan_with`, the regions' steps and the table's queries rather
      than ~60 items.
      - **The merge** (built): `build::pass_work`, one function answering the pass's worldwide
        jobs and the checklist's lines (`PassWork::left`, over `build::WORLDWIDE_STEPS`, moved
        from agent/mod.rs) from one place. It takes what the planner reads from the root
        (`PassReads`: the 3D buildings' release, lakefill's planned source, why the made reaches
        don't load, asked only when the reach job has no work, and the failure counts) and
        names-to-do's gate (`NamesTodo`: its inputs' key, `since`, `idle`, asked only once the
        outlines hold nothing, as the planner did), and answers each step's job (`PassJob`: id,
        step, the arguments after `--root` and `--scratch`, work) and apart its holds and waits
        (`lakefill_held`, `outlines_hold`, `waits`), the summits' work (placed after the regions'
        plan, which says whether their terrain is built) and whether the buildings' release is
        made (the planner then removes the scan's parts from its scratch). The jobs leave a held
        work out; the checklist still counts it left, with its own checks of an output missing
        from the manifest (the pass's sets, outlines, hikes, summits, the tiles' index), as before.
        The planner turns each `PassJob` into its `JobSpec` (its program, root and scratch);
        `timetables_todo_work` stays where the planner calls it, after the regions' plan (it
        reads the plan's key memo). Cut: `outlines_hold`, `OUTLINES_HOLDS`, `trailends_work`,
        `regions_land_work`, `labels_work`, `spoken_work`, `names_todo_work` and `summits_work`
        made private (only `pass_work` and build's tests call them); plan.rs's `PassCtx` (the
        checklist's and the jobs' each). steps' `every_step_the_agent_runs_has_a_row` also reads
        build.rs's job names (`job("…`, `id: format!("…`).
      - **The frozen-plan check** (built): `plan::tests::frozen_plan_on_a_private_root`, ignored,
        runs a planner's `region_jobs` and `checklist` on a private root (`SCENIC_FROZEN_ROOT`:
        the lead's `state/` copied, every other entry of the NAS's project folder a symlink;
        `SCENIC_FROZEN_HOME` keeps the coverage's cache and the terrain indexes between runs;
        `SCENIC_FROZEN_APP` the lead's app), with no coordinator, gate watch or record of past
        catalogues, the forecast's thread parked, and writes the jobs (commands, targets, keys)
        before and after the holds, the waits, the holds, the offers and the checklist's lines to
        `SCENIC_FROZEN_OUT`, a line each. The symlinks are writable by themselves, so it plans in a
        child of itself under `sandbox-exec`, denying every write under /Volumes and the app's home,
        and refuses a root leading to a live path or a `state/` that is a link. Knobs for states the
        records alone can't make: `SCENIC_FROZEN_PASS`, `SCENIC_FROZEN_FAILURES`,
        `SCENIC_FROZEN_NAMES_SEQ`, `SCENIC_FROZEN_NAMES_AGO`. The state is copied by tar to the M4
        (its `state/` without coord, backups, journal, live-archive, pool-shadow, taskboard and the
        status's temporary files, and only the current term's records: 63 MB). The gate's own jobs
        (`gate_work`, not changed by the merge) stay out of it: no gate watch.
      - **Check of the merge** (7 Nov, the lead's term 771 seq 67 on the M4, test profile; the check's
        commit with these knobs against the merge, each pair side by side, a run's own
        folder named alike before comparing): the same bytes on all nine states, 1.7 KB to 1.0 MB,
        each edited from that copy's brief and records. As it stands (51 jobs, 11 waits, 10 offers);
        the outlines with work, held; the outlines unmade, 5 failures, held; every worldwide step
        unmade, a set, the buildings' index and the heritage input gone, the outlines failed 3 times,
        names-to-do due (19 jobs, 18 after the lake fill's hold); the lake fill failed 3 times,
        the languages and translations gone, the reaches unreadable; the sources gone (the lake fill
        unreadable, held), the reaches damaged, names-to-do's gate shut; no pass, names-to-do due and
        shut; a pass with no outputs yet. `pass_work_holds_stay_apart_from_its_works` keeps these
        branches on every run. One difference no plan shows: `PassReads`'s release and lake-fill
        source are read before the outlines' hold, where the planner read neither under it (the
        checklist needs them either way): a file and a folder's listing more per plan, while
        the outlines have work. A full plan's run took 4.5 to 22 min.
      - **The trait** (built, 7 Nov 2026): `agent/seam.rs` is the runtime's side (it goes down with
        agentcore), `agent/theplan.rs` the plan's: its one implementation, `the_plan()`, which the
        binaries hand in as `Options::the_plan`. `KeyCtx` can't be the runtime's: it borrows
        `TerrainTiles` (which names `terrain_water` and `depth::grids` types) and `Reaches`
        (crates/roads), both above agentcore, and `KeyMemo` holds `TerrainTargets` built from them; a
        `&'static dyn Plan` carries no associated types either. So what the planner carries for the
        plan is opaque, and what it reads, or hands in, is the runtime's data.
        - **`seam::Plan`, the questions:** of the steps table (`step_names`, `is_step`, `shared`,
          `answered`, `alone`, `records_only`, `fetches`, `cost_version`, `sources_naming` (needs'
          `source_secs`), `noun`, `said`, `task_kinds`, `task_disk`, `is_none_key`, `output_named`,
          `output_names`, `saves`, `outside`, `outside_by`, `read_steps`, `read_targets_passing`,
          `input_readers`, `summits_waits`, `table_json`); of build.rs (`reads_sources`,
          `reads_lakefill`, `catalogue_waits_for`, `job_of`, `worldwide_steps`, `fold`,
          `checklist_to_come`); of the families (`terrain_peak`, `task_what`, `tree_task_kind`,
          `task_kind`, `names_todo_inputs`, `names_todo_key`, `canopy_files`, `unit_margin_km`,
          `key_inputs`); and `held`. `Step`, `Read`, `Kind` and `Scope` stay the plan's.
        - **`seam::PlanHeld`, one opaque object:** `Plan::held(index_dir) -> Box<dyn PlanHeld>`
          (`Send`), which the planner keeps for its life in place of its `tiles`, `reach` and
          `kept` fields. It owns those caches and the memo. `PlanHeld::plan(round, publish,
          refilter)` answers a plan, reusing the kept one under `refilter` while `KeyMemo::serves`
          holds; the questions that share its keys go to the same object (`heritage_waits`,
          `timetables_todo`, `chains_left`, `unknown_why`, `restaled_by`, `pass_work`), each by a
          `seam::Round` of plain data, so `region_jobs` makes no `KeyCtx`. `begin_round` reads the
          reaches once a round. The off-loop counts ask `region_counts` for a `Send` closure.
        - **Data (`agent/seam.rs`):** `Planned` (`build::Plan` as it was: `Work`, strings,
          `RegionLeft`, `PublishLine`, `RegionState`, `Checklist`, `Left`: no family type among them;
          renamed, the trait having the name), so the planner goes on reading and editing its
          fields, and `Publish` as the arguments (`Coverage`, maps, the runtime's `Publishing`);
          `PassWork`, `PassJob`, `NamesTodo`. `Along`'s data is here too, not with the plan as 6f1
          left it: `Left` and the forecast's `Input`, whose bytes on disk are as they were, hold it.
          Its filling reads the table and `KeyCtx`, so it stays the plan's (`steps::along_add`,
          `along_add_built`).
        - **`pass_work`:** the plan reads the release (bld's) and the reaches (`LoadError` is roads')
          itself, `build::PassReads` staying its own; the planner gives `seam::PassAsk` (the lake
          fill's source, the failure counts) and `NamesTodo`. The checklist, asking no gate, asks
          nothing of the reaches, as before.
        - **Held and passed:** `Options::the_plan` (the agent, the planner), `Sched::the_plan`
          (`Coordinator::start`, `start_on`, `viewer`, `Broker::start`), `needs::Book`,
          `history::History`, `pool::Run`, `BuiltView`, `PageView` and the map log's `Writer`; passed to
          the forecast (`forecast`, `forecast_by`, `Record::forecast`, `Noted::note`, `speeds`,
          `step_sums`), fccheck's `replay` and `backtest`, `flags::of`, progress's sayings,
          `room::Hints`, `redo::said`, `every`'s functions, `uses::Run`, `Paces::with_starts`,
          `memguard::Watch::floors`. `jobs::note_cost` takes the step's way as a number (`gc::set_way`).
          For the runtime's tests until 6f3, `seam::for_tests()` and coord's `cfg(test)`
          `cost_version` and `saves`.
        - **Cut:** the planner's `tiles`, `reach`, `kept`, `terrain_tiles`, `current_reach`,
          `as_built_parts`; `agent::terrain_peak`, `task_what`, `key_inputs`, `input_digests` and
          `key_names` (moved to `agent/theplan.rs`); `pubtimes::restaled_by` (to build.rs); the
          `Default` of `Sched` and `PageView`, no plan being a default.
        - **What's left of the runtime's names:** the runtime's lines naming the plan's modules
          (`build::`, `steps::`, `idents::`, `tiles::`; comments aside, tests and plan.rs included)
          RUNTIME_COUNT. A family's name in non-test runtime code: gc's depth layers
          (`DepthKeep::orphan`), left for 6f4, as #883 was changing gc.rs.
        - **Check of the trait** (FROZEN_RESULT).
        Moving the planner up with the plan instead was
      weighed and is the larger cut: its pass names ~150 of the runtime's items (forecast, coord, pool, agent/mod.rs), all of which would become agentcore's public
      API. `timetables::View` went
      below both (6f1). `Keys` (6f1) names every per-target step, as the records' format
      `jobs.json` does; `per_target` and `recorded` are that format's table, so they stay with it.
   3. **6f3, test time** (planned): the runtime's `cfg(test)` behaviours (the waits `JOIN_WAIT`
      and `JOIN_TRY`, coord's `LISTEN` and loopback waits, the panics where a test wants one;
      first the guards keeping a test off the real NAS and the pool's port, `find_root` with its
      `TEST_ROOT` and `coord_port` with `test_port`, #906) read the process flag
      (`buildstore::out::testing`), as `disk_free` does since 6e: what they read is then built
      always. 6f3 lands before 6f4: a guard left on `cfg(test)` in agentcore is off in pipeline's
      tests. Safe by the rule 6e's review set: `testing()` is true exactly in a crate's unit tests'
      process (`TEST_START`), and the runtime's code runs under `cfg(test)` today only in
      pipeline's, which keeps its `TEST_START`; agentcore gets its own for its own tests, and no
      crate with a `TEST_START` below agentcore calls it. The rest are not behaviours and don't
      switch: test-only items (`Options::member`, `test_agent`, `planner`, coord's
      `start_for_test`, the `/work/test-block` route, `pool_state`) stay `cfg(test)`
      where only agentcore's own tests use them, and go to pipeline's test side (accessors, as 6e's
      `task::testkit`) where the tests needing the plan use them; a test route never ships behind
      the flag. Those tests (the real table, the families) go to a pipeline test module, as the
      families' went.
   4. **6f4, the move** (planned): the runtime (agent but build, steps, idents and tiles; coord, pool,
      needs, paces, uses, pubtimes, answers, menu, appupdate, cli, net) into `crates/agentcore`,
      on inputs and the crates below it, beside the families; pipeline keeps the plan, offload's
      `run_task` and `task_kind` (through `Plan`), `out::open`, the binaries and the re-exports.
      Measured as §5 does.
   Before it, pipeline holds ~120k lines (the cycle's 108.0k, the binaries' 14.2k).

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
