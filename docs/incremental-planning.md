# Incremental planning

**Status: planned (task #880).** Built: step 0 (the plan's sections timed as the plan thread's
phases, `nasread::PLAN`), step 1 (the regions' counts the plan's, its keys handed on in its round)
and step 2's leases (a lease change filters the last plan's work again). Everything else here is
planned; until a step lands, the code is what is true. Today every plan computes `agent::build::plan` (crates/pipeline/src/agent/build.rs) from
scratch: every target's key, every target list, every region's work, from the whole records. This
doc measures where that time goes, how much of it is recomputed though nothing it reads changed,
and how the plan part (docs/agent-parts.md §4.4, #871) would keep its answers between plans and
update only what each change touches, each step shipping alone.

## 1. Where a plan's time goes (6 Nov 2026)

### 1.1 Live, on the lead

The Mac mini's `~/Library/Logs/scenic/agent.log`, read only, from the first plan after #965's
publish (5 Nov 21:35Z) to 6 Nov 12:26Z: 14.9 h, apps 20261105-2124-d1403c86 to
20261106-1214-b4237b4b. `agent: plan <n> (<why>) took N s` is logged for plans of 10 s or more;
a phase line only for a phase of 10 s or more (`nasread::SLOW_PHASE`).

| Plans (≥ 10 s) | n | median s | p90 s | max s | sum s |
|---|---|---|---|---|---|
| all | 218 | 111 | 181 | 332 | 25 711 |
| "a job ended" | 151 | 123 | 170 | 332 | |
| "what it depends on changed" (`plan_seen`) | 45 | 80 | 126 | 290 | |
| "no plan yet" (the agent started: an app update or a restart) | 14 | 214 | 252 | 260 | |
| "the fallback" (5 min) | 6 | 78 | 150 | 150 | |
| "a timed wait ended" | 2 | 59 | 64 | 64 | |

The plan thread was planning 48 % of the window (25 711 s of 53 460 s).

| Phase (`nasread::PLAN`), where ≥ 10 s | plans | median s | p90 s | sum s | share of plan time |
|---|---|---|---|---|---|
| build::plan: the terrain and slope | 146 | 52 | 69 | 7 543 | 29 % |
| its jobs (`Planner::region_jobs` after `build::plan`) | 217 | 20 | 28 | 4 505 | 18 % |
| build::plan: the z6 tiles' keys | 192 | 16 | 23 | 3 195 | 12 % |
| build::plan: the sites, water and pyramids | 107 | 24 | 35 | 2 773 | 11 % |
| build::plan: the chains and publishing | 136 | 13 | 18 | 1 905 | 7 % |
| build::plan: the regions | 24 | 10 | 11 | 248 | 1 % |
| build::plan: the checklist, the chains | 14 | 14 | 28 | 232 | 1 % |
| phases under 10 s, unlogged (the records' read, the gate, the rest) | | | | ~5 300 | ~21 % |

Beside the plan, on its own thread (`Planner::count`, `build::region_states`, until step 1): the regions' states,
counted every 5 min and again after every job's end, 440 times in the window, median 29 s, 12 341 s
in all. It works out every z6 tile's key again (`unit_keys` with a fresh `KeyCtx`), keys the plan
had just worked out.

The terrain phase is 52 s in two plans of three and under 10 s in the rest: its target lists and
keys are kept between plans (`TerrainTiles::memo("terrain-targets")`, since #965) under one digest
of every terrain, slope and mid entry of the manifest, the coverage and the elevation sources.
Mid-rebuild a terrain or slope job ends every few minutes, and any one of them changes that digest,
so every terrain and slope target is worked out again; and nothing is kept while a slope key can't
be told (`terrain_slope_targets_kept`).

The lead's agent: a footprint of 3.2 GB at 12:44Z (`footprint`); its plan-stall samples of 5-6 Nov
say 2.1-3.4 GB, peaks 2.3-3.8 GB (the loop-stall samples of 30 Oct, before #871's parts and #965,
peaked at 7.6-11.3 GB). The mini has 32 GB.

### 1.2 A frozen copy of the lead's state, on the M4

`bench_plan_on_a_real_root` on private copies of the lead's records (term 752's brief and snapshot,
the term files; everything else symlinked read-only to the NAS share; state A at 12:37Z, seq 18;
state B at 12:42Z, seq 29), the lead's `pack-idx/` copied, on the M4 under the pool's load
(load 55-145), test profile (opt-level 1: slower than the lead's release build). Thread CPU
seconds, each section timed by the plan's own phases:

| `build::plan` section, CPU s | A (first) | A again | B after A | B again |
|---|---|---|---|---|
| the sites, water and pyramids | 1.9 | 3.0 | 1.9 | 3.0 |
| the terrain and slope (its targets kept, see below) | 6.9 | 7.1 | 7.3 | 7.3 |
| the tree cover | 0.1 | 4.1 | 0.1 | 4.1 |
| the z6 tiles' keys | 40.7 | 46.8 | 41.0 | 48.1 |
| the regions | 15.5 | 15.8 | 16.4 | 16.7 |
| the map tiles | 0.0 | 0.0 | 0.0 | 0.0 |
| the regions' order | 3.7 | 3.6 | 3.6 | 3.6 |
| the chains and publishing | 27.8 | 28.1 | 28.8 | 28.5 |
| the checklist | 1.9 | 1.9 | 1.8 | 1.9 |
| **build::plan** | **98.5** | **110.4** | **101.1** | **113.3** |

The same plan each time on a state (A: 7 147 work targets; B: 7 138). The bench works out some
parts alone before each first plan of a state, which is why the terrain section above is 7 s:
those calls left its targets kept. Alone, on the same state:

| Alone, CPU s | A (deep answers cold) | B (deep answers kept from A) |
|---|---|---|
| `terrain_slope_targets_made` (what a plan does when the terrain digest changed) | 229.6 | 80.6 |
| `terrain_slope_targets` right after it (the deep answers now kept; worked out again, then kept for the plan) | 79.0 | 80.8 |
| `unit_keys` (every z6 tile's key, no memo) | 77.2 | 47.0 |
| `region_states` (the counts thread's) | 53.0 | 56.6 |
| `checklist` without the plan's memo | 53.7 | 51.0 |
| the coverage made (live: kept by version) | 85.6 | 145.2 |

So on the lead (release, a lighter load) a plan after a terrain or slope job is the ~50 s of
terrain targets plus the ~60 s of the rest; after any other job, the rest. The test process peaked
at 4.2 GB resident (`time -l`).

The committed bench plans one root, twice; the runs of this section (two states in order, the
memo passed in, the keys and work compared between states in §1.3, the key lines measured in §7)
used a local variant of it, not in the repo: to repeat them, the same changes are made again.

A second run (13:45-13:52Z, the M4 lighter: wall ≈ CPU) split the plan's own work from its keys'
(a local copy of `build::plan` taking its `KeyMemo` from the caller): the plan of B after A's,
then again with a fresh memo (the terrain targets now kept), then again with that memo filled
(every derived key and the terrain targets known: what is left is the plan's own assembly). The
plan is the same each time.

| `build::plan` section, CPU s | B after A | fresh memo | memo filled |
|---|---|---|---|
| the sites, water and pyramids | 2.9 | 1.4 | 0.9 |
| the terrain and slope | 80.9 | 5.5 | 4.3 |
| the tree cover | 2.9 | 0.1 | 0.0 |
| the z6 tiles' keys | 32.4 | 30.7 | 0.3 |
| the regions | 12.1 | 12.3 | 9.2 |
| the regions' order | 2.9 | 3.0 | 2.1 |
| the chains and publishing | 21.3 | 20.9 | 17.4 |
| the checklist | 1.3 | 1.1 | 1.1 |
| **build::plan** | **156.7** | **75.0** | **35.3** |

(The same on A, first: 205.9 s, 121.1 of them the terrain with the deep answers cold; fresh memo
59.8 s; memo filled 35.9 s.) The 35 s left with every key known is not key derivation: the map
tiles' work and owners and the 3D buildings', depths' and polar caps' chains (`publish_chain`,
`bld_work_in`, `depth_work_in`, `polar_work_in`: 17 s), each region's targets (`builds` per region
and z6 tile: 9 s), and `terrain_work` over the kept terrain targets (4 s).

### 1.3 What changes between plans

Between A and B (5 minutes, about two plans' worth on the lead): 38 of 52 284 manifest entries
(17 added, 21 changed, none removed: road and rail map tiles, z6 tiles' data, a terrain z6 tile,
pois) and 39 of 33 279 done keys (29 of them map tiles). Of the plan's 11 211 derived keys, 20
came out different (16 lake-depth, 2 bldfield-lo, 2 unit: 0.18 %); of its work, 61 of ~7 140
target lines (most of them the jobs that ended, gone, and 17 new map tiles). One terrain z6 tile's
pack changed, so every terrain and slope target was worked out again (81 s).

So a plan following a job's end reads a state 99.9 % as the last plan saw it and recomputes it all.
Of the 157 s of B after A, ~122 s (78 %) work out keys and target lists that nearly all come out as
they were (the terrain targets' 81 s, the keys' ~40 s: 99.8 % of the derived keys unchanged); the other 35 s assemble the plan from them,
over inputs equally unchanged. Beside the plan, the regions' counts (§1.1) derive the z6 tiles' keys
again after every job's end. Recomputed for nothing, then: about three quarters of the
plan's time, and all of the counts'.

## 2. What a plan is a function of

`build::plan(cov, date, m, done, inputs, reach, tiles, Publish)` is a function of its arguments,
`tiles` (`TerrainTiles`) included as loaded: which packs' indexes and declared sources it holds and
which it couldn't read (`unread`, "can't be read now"), state the manifest doesn't show. It writes
only caches whose answers are the same either way (`KeyMemo`, `TerrainTiles::memo`, the deep
answers). Every derived key is
`hash16(key_lines(step, t, x))` (crates/pipeline/src/agent/steps.rs), and `key_lines` reads only
what the step's row declares (`Step::reads`): outputs of named targets (`t <step> <target>
<logical>=<content>`, or `-` for a target with none), mids, a step's outputs read whole (`w <step>
…`), the lines of a `Grain::Read`/`Grain::Tiles` function, and input identities (`in <name> …`,
`Read::Input`'s `Ident` functions, crate::agent::idents). `a_key_names_exactly_its_reads` checks,
on fixtures, that changing a manifest entry changes a target's key just when its key lines name
that entry (or the tiles read inside it). It changes entries in place only: an entry added or
removed (a target's first build, a drop) is not checked, and the `Ident` and `Grain` functions read
the manifest, the coverage, the reaches and the terrain indexes however they like (ranges over a
prefix, `contains_key`), so their reads are not declared at the grain of entries.

Beside the keys, the plan works out target lists (which z6 tiles the coverage builds, the terrain
and slope targets near it, the deep levels, the "none" targets the coverage left: from the coverage,
the reaches, the terrain indexes and prefix ranges of the manifest), each region's targets, the
staleness of each target (its key against `done`), and then the order (regions, chains,
publishing), which reads all of these and `Publish`'s times.

After `build::plan`, "its jobs" turns the plan into job specs and offers, filtered by the memory
guard's holds, the leases other workers hold (`held_by_others`, `held`) and the cold starts under
way, and (each `forecast::EVERY`, 60 s) starts the forecast, whose `chains_left` works out the
chains' keys with the plan's `KeyMemo` (step 1).

## 3. Fitting #871: the plan part keeps its state

docs/agent-parts.md §4.4 makes the plan a part of its own: on the lead only (#881, #882), on its own
thread, triggered by changes (a job's end, the records' generation, an edit hold's end, a timed wait,
a 5-min fallback), owning its state `&mut`, publishing `PlanView`, sharing no `Mutex`
(`no_shared_mutex`). Incremental planning stays inside that part:

- **The state lives in the `Planner`**, beside `tiles` and the coverage it already keeps between
  plans: no new thread, no shared state, nothing on the NAS.
- **Events trigger, the diff decides.** The triggers stay `why_plan`'s. What a plan recomputes is
  decided by diffing what it reads against what the last plan read (the manifest and done keys of
  the records' brief, the inputs' digests, the coverage's version, the pass and its reaches, the
  app), never by trusting an event to say what changed: a missed or doubled event costs a plan,
  never a wrong one. Diffing 52 k entries is milliseconds.
- **Granting stays apart.** A lease change, a guard hold or a cold start changes what is offered,
  not what is stale: it re-runs the filtering of "its jobs" over the last `build::plan` result, not
  `build::plan`.

## 4. What the plan keeps between plans

1. **The key memo**, across plans: `(step, target) → (key, reads)`. Today's `KeyMemo` holds keys
   for one plan (#924); this one outlives it. `reads` is what deriving the key looked at: manifest
   entries by name, manifest prefix ranges, input names, whether it read the coverage or the
   reaches, and what of `TerrainTiles`' load state it met (below). It is recorded by construction: `KeyCtx`'s manifest becomes a view
   (`Reads`) whose `get`, `contains_key` and `range` record into the key being derived (into every
   key on the derivation stack: a key derived inside another's adds its reads to its reader's), with
   no `Deref` to the map, so code that derives a key can't read the manifest past it; the compiler
   finds every function that took `x.m` as a map. `TerrainTiles`' readers (`hash`, `resolve`,
   `elevation`, `zoom_sets`, `keyed`, …) take the manifest too, so they take the view and record the
   pack and index entries they look up, not a flag "read a terrain index" (which every z6 tile's key
   sets: any pack's change would derive them all again, today's all-or-nothing). What construction
   alone misses, and the design closes explicitly:
   - **A cache hit inside a derivation.** A key read from `KeyMemo`, a `TerrainTiles::memo` answer
     (a z6 tile's terrain digest, the terrain targets), a deep answer or `KeyMemo`'s terrain-targets
     cell runs no code, so records nothing. A hit adds the cached value's read set (or an edge to
     it, invalidated transitively) to every key on the stack; each such cache keeps the read set of
     what it holds.
   - **A miss.** `get` of an absent entry and an empty `range` are reads: an entry added later
     must find the key.
   - **State the manifest doesn't show.** A key that came out unknown (`None`) or read an index
     `TerrainTiles` couldn't read depends on the load, not the entries: such keys are never kept,
     and a change of `unread` or of what `TerrainTiles` holds invalidates the keys that read it.
   - **Reads beside the arguments.** `steps.rs`, `idents.rs` and `TerrainTiles`' readers read no
     environment, clock or NAS folder today (no `env::var`, `now_s`, `read_dir`; the modules they
     call, such as terrain_pack's `water_source_pin` or heritage's `latest_date`, unchecked); the NAS
     is read for keys by `TerrainTiles::load` and `key_inputs`, whose results are diffed (the inputs)
     or tracked above. The app's version is the process's: the state starts empty after an update.
     The manifest is a `BTreeMap`, so neither the brief's order nor a NAS listing's reaches a key
     through it. A guard test keeping it so (no `env::var`, clock or `nasfs` call
     reachable from `key_lines` but through `KeyCtx`) belongs with step 3.
2. **The reverse index**: entry name → keys, prefix → keys, input name → keys, and the keys that
   read the coverage, the reaches or `TerrainTiles`' load state. A changed name finds its keys by exact
   lookup and by each of its own prefixes (a name is ~60 bytes: ~60 lookups).
3. **Target lists, as memo entries of their own** (`enum <list>`, read the same way): which z6
   tiles the coverage builds, the terrain and slope targets with their "none" ones, the deep
   targets, `coverage_tiles`, each region's targets. They replace the terrain-targets digest
   (`TerrainTiles::memo("terrain-targets")`, all or nothing) with entries each invalidated by what
   it read. #965's `TerrainTiles::deep` answers stay as they are (kept by coverage version and
   declared sets).
4. **The stale sets**, per step: the targets whose key differs from `done`, updated for the keys
   that changed and the done records that changed.
5. **The last `build::plan` result** (`Plan`), which "its jobs" filters; and the order's inputs.

Until step 6b (§8) the rest of the plan (each region's targets, the map tiles' and chains' work,
the order, the checklist) is assembled again each plan from 1-4: 35 s on the bench with every key
known (§1.2). Step 6b keeps those parts as memo entries too, read and invalidated the same way, so
what is assembled each plan is the order alone (2-3 s on the bench).

## 5. Each change and what it invalidates

| Change | Seen as | Invalidates |
|---|---|---|
| A job ended (its hand-off merged into the records) | the brief's manifest entries added, changed, removed; its step's done keys | the keys whose reads name a changed entry or a prefix of it (a first build adds names under its target's prefixes, which the readers' `range` reads recorded); the target lists that read them likewise; the stale state of the targets whose done key changed (no key derived again) |
| An input accepted (the gate) | its accepted version's entries in the manifest; `inputs`' digests (`key_inputs`) | the same, by entry; the keys whose reads name the input |
| The coverage changed (a region added, removed, edited) | the regions' accepted version (`coverage`, already compared each plan) | everything that read the coverage, which is nearly everything: a full plan, kept as the new state. Rare, and held by the edit hold (`EDIT_HOLD`) already |
| A new pass, new reaches | the pass date, `current_reach` | everything that read the reaches: a full plan |
| A terrain index or declared source read that wasn't, or the reverse | `TerrainTiles`' held and `unread` sets (`load`) | the keys that came out unknown or read it (§4.1) |
| An app update | a new process: the state starts empty | a full plan (the "no plan yet" plans, 14 in §1.1's window, 166-260 s); not persisted: a key's versions are the app's |
| A lease granted, lapsed or given back | the coordinator's `lease_gen` | not `build::plan`: "its jobs" filters the last `Plan` again and offers it. `lease_gen` leaves `plan_seen`'s digest for a trigger of its own |
| The pause, the placement switches, the pool's gates | `plan_seen` | "its jobs" again, likewise |
| Time (catalogue cadence, holds, `Publish`) | `Publish`, `held_for` | the publishing chain's ordering only: recomputed every plan |

## 6. Equal to a full plan

The incremental plan must be the full plan, byte for byte (its `Plan`, keys included), as the map
log's `incremental_digest_is_the_whole_digest` holds for its digest (docs/maplog.md):

- **A shadow full plan on the lead**: after a restart, after the NAS's return, and every Nth plan
  (N set on the build page, shown with the plan's phase times), a full `build::plan` on the same
  inputs, on a worker of the plan part (its own thread and memo, none shared), compared with the
  incremental one. A difference is logged with the first differing step and target, its recorded
  reads and the changes since the last plan; the full plan is used, the state rebuilt from it, and
  the status says so (a flag the page shows, not a log line alone). Until step 5 every plan runs
  the shadow. After it, N counts time as well as plans (a shadow at least every 30 min whatever
  the plans' rate), and:
  - **A catalogue is planned from a confirmed plan.** A catalogue (at most hourly) is the plan's one
    output others see; between shadows a wrong key can only delay or repeat a job, which the next
    shadow corrects (a job records the plan's key, which the full plan then finds stale), but a
    region published as ready would stay on the map. The plan that puts a catalogue in the work
    runs its shadow then.
  - **A mismatch shadows every plan** until ten in a row are clean, and saves what reproduces it
    (the brief's `seq` and the inputs' digests of both plans) for a recorded-state test.
- **Tests on fixtures** (every run): for each fixture of `a_key_names_exactly_its_reads`, each
  manifest entry changed, added and removed alone, and each input digest changed: the incremental
  plan after the change equals a full plan of the changed state, key by key and `Plan` whole.
  This covers what that test doesn't (additions and removals, prefix reads, target lists).
- **Tests on recorded states** (ignored, like `bench_plan_on_a_real_root`, run by each step's
  review on frozen copies of the lead's state as in §1.2): a series of the lead's states (A, B, …
  minutes apart), planned incrementally in order and fully at each; equal at every one.
- **Read sets by construction** (§4.1), so a forgotten manifest read is a compile error, not a
  stale key; the cases construction can't see (cache hits, misses, `TerrainTiles`' load state,
  reads beside `KeyCtx`) are handled as §4.1 says and each has a fixture test; the shadow catches
  the rest.

## 7. Memory on the lead

Measured on state A (§1.2): one plan's `KeyMemo` holds 11 211 keys (trees and bldtiles 2 364
each, lake-depth 2 431, bldprep 1 752, unit 1 238, depth 690, …), ~1.2 MB of strings. Their key
lines are 14.5 M lines, 703 MB (1 295 a key on average, one key 1.5 M: a step reading another
whole), naming 13 590 distinct manifest entries. So:

- **Read sets are kept as interned ids, never as lines**: an entry's name once (13.6 k names, a
  few MB with the manifest the plan holds anyway), a key's reads as `u32` ids and prefix ids; a
  read of a step whole (`Read::Whole`) and a `Grain::Tiles` read as one prefix each, not one id per
  entry or tile. The ids are bounded by the lines: at most 14.5 M × 4 B = 58 MB, the reverse index
  as much again; the whole reads, which make most of the lines, collapse to a prefix each, so the
  real figure is far below. Step 3 measures it on the lead and logs it with the plan's phase line.
- **The target lists** (§4.3) are what each plan already holds while it runs (7 147 work
  targets, 33 279 done keys, the terrain and slope targets), now kept between plans instead of
  dropped.
- **No second copy of the records**: the last brief's manifest and keys are kept for the diff
  (52 k entries, 33 k keys: the brief is 5.3 MB as JSON) in place of the one each plan reads now.
- **The shadow full plan** (§6) runs with its own memo, as a plan does today: no more than today's
  peak while it runs.

What a plan asks of the allocator (task #1251, 13 Nov; `plan::tests::frozen_plans_memory` on a
copy of the lead's state of that day, everything built, the M4, test profile; blocks counted by
the test's allocator, a block grown counted again whole): a plan after the first asks for
8.0 GB in 75 M blocks and takes 19 s, where before #1251 it asked for 37.5 GB in 311 M blocks
and took 40-45 s; the first plan after a start (the terrain's targets made, the deep answers
cold) 137 GB in 930 M blocks and 170-199 s, where 166 GB, 1 166 M and 191-235 s (two runs
each, the M4's load apart). Before, by the function
allocating (sampled stacks, GB a plan): `build::depth_water_reads` 13.0 (every depth target's
water tiles, listed to ask for their names), `steps::pack_of` 9.7 (a `Vec` of a name's parts per
manifest entry, for every key reading a step whole), `depth::grids::estimates_key` 4.0 (the
lake-depth packs' digest, made for every key reading the depth's sources),
`dem::spacing::Fine::of` 2.2 (made for every z6 tile's key), `steps::key_lines` 1.9. The first
four are gone. What is left is the key lines themselves, made as text for every key every plan
(`key_lines` 1.9 GB, `build::unit_deep_tiles` 1.0, `roads::stage::Area::tiles` 0.7, a reach as
JSON 0.4): steps 4-5 end that for the keys nothing changed under. Why the bytes matter beyond
the time: macOS's malloc keeps each block size's freed pages resident (docs/pool.md §7.1,
*What leaves them*).

The lead's agent is at 3.2 GB (footprint, 12:44Z) on a 32 GB mini; the bench's test process,
holding the coverage, the terrain indexes, two states and four plans, peaked at 4.2 GB resident.
The planned budget for the kept state is 0.5 GB (1.5 % of the mini), measured and logged on the
lead from step 3 on.

## 8. Steps, each shipping alone

Expected figures are from §1: the lead's live medians, and the bench's CPU where the log doesn't
split. The lead's plans run at ~0.85 of the bench's: in §1.1's window, the plans with a terrain
phase logged (132, "no plan yet" aside) took a median 136 s (the terrain 50 s of it) against 157 s
for B after A; those without (72) 66 s against 75 s for B with a fresh memo. Each step measures
its own before and after, on the lead and on frozen states (§1.2's way).

| Step | What | Expected after it | Done when |
|---|---|---|---|
| 0 (built) | `build::plan`'s sections timed as the plan thread's phases | — | the live log names where the time goes (§1.1) |
| 1 (built) | **The plan's keys shared with their readers**, independent of the rest and small (build.rs and plan.rs, ~+40/−80 lines). The regions' counts are the plan's own: `plan_in` already works out every region's z6 tiles and those stale (`Region`'s `all` and `stale`, by the same `builds` filter and the same comparison with `done.unit` as `region_states`), so `Plan` carries each region's `RegionState` (every region at 0 of 0 on the early return before the reaches) and the Planner publishes it with the plan; the region-states thread, `region_states`, `built_making`, `built_counted` and `COUNT_EVERY` go. A job's end already triggers a plan, so the counts are as fresh, but for the plans an edit hold or the gate delays. `build::plan` takes its `KeyMemo` from the Planner, which hands the same memo, on the same thread, to `heritage_awaits_acceptance`, `timetables_todo_work` and the forecast's `chains_left` in that round, and keeps it with the plan's result for step 2 until the next plan. A memo serves only the context it was first given with (`KeyCtx::stamp`: the coverage's version, the pass, the manifest, the inputs, the reaches, `TerrainTiles::state`); a reader whose context reads otherwise (an index read that couldn't be before, a pass newly on the NAS) works its keys out afresh. `region_steps` (the build page's, while a page asks) is unchanged | the counts thread's 29 s per job's end gone (12 062-12 341 s of CPU in §1.1's 15 h; its brief read over SMB and `key_inputs` each time too); plan latency unchanged but for "its jobs"' key work (median 20 s, not split) | `a_unit_whose_terrain_cant_be_read_now_waits`'s count read from `Plan`; the page's counts the same as `region_states`' on frozen states; phase times logged |
| | *Measured.* Before, on the lead (5 Nov 21:35Z-6 Nov 14:11Z, 16.7 h): the counts thread 498 runs, median 27.8 s, 13 666 s (23 % of a core); plans ≥ 10 s 15.0 an hour, the plan thread busy 48 %. On a frozen state of the lead (term 755, seq 13, 6 Nov 14:29Z; the M4, test profile, loaded): `seam::Planned::built` equal to the counts worked out alone for all 999 regions (192 of 4 298 z6 tiles built); the counts' own work, now none, 61-63 s a run; `chains_left` with the plan's memo 1.9 s against 71.6 s with a fresh `KeyCtx`. The lead's after: once published | | |
| 2 (leases built) | **Granting's inputs out of the plan's trigger**: the leases' generation, the pause, the placement switches and the pool's gates leave `plan_seen`; their change filters the last `Plan` again ("its jobs") and offers it | fewer plans: some of the 45 "what it depends on changed" (the step logs which input changed, to count them) | the same offers as a full plan on a recorded lease change |
| | *Built:* the leases' generation alone: `plan::refilter_due` runs "its jobs" over the kept `build::plan` result and its keys (`seam::PlanHeld`; `build::plan` made again when the keys' context no longer reads the same), under a plan number of its own (a job's grant by its id must find the spec of the offers it was granted from, task #923), logged as "plan N: the last plan's work filtered again (the leases changed)"; `a_lease_change_filters_the_last_plans_work_again`, `a_lease_change_offers_the_last_plans_work_under_a_new_number`. *Planned:* the pause, the placement switches and the pool's gates, and a recorded-state test of the offers | | |
| 3 | **Read sets, recorded (shadow)**: `KeyCtx`'s manifest becomes the recording view (§4.1), the Planner keeps the last brief and diffs the next; each plan builds the read sets and reverse index and logs their size and the keys the diff would have kept; the plan itself unchanged | latency as now (the recording's cost measured); memory measured (§7) | the logged "would keep" equals, on recorded states, the keys that came out unchanged |
| 4 | **Keys kept across plans (shadow)**: the memo outlives the plan, invalidated by the diff; the full plan still runs and is used, and every plan compares the two (§6) | latency as now: the full plan used, the incremental one on a worker of the plan part beside it (its CPU on top: the plan thread is busy 48 % of the time already) | the shadow clean on the lead for a week; the tests of §6 |
| 5 | **The kept keys used**: the plan derives only invalidated keys; the full plan only in shadow every Nth plan | a plan after a job that touches no terrain: ~75 → ~35 s bench, ~66 → ~30 s live; after a terrain or slope job (two plans of three) the terrain's ~50 s live stays on top until 6a | `build::plan`'s phases on the lead; shadow clean |
| 6a | **The terrain and slope targets kept per target** (§4.3) instead of under one digest, with read sets: the largest phase live (29 % of plan time, 50 s in two plans of three), first of step 6 | a plan after a terrain or slope job as after any other (~30 s live) | the same; shadow clean |
| 6b | **The rest of the assembly kept** (§4.3-4.5): each region's targets, the map tiles' work and owners, the chains' work, `terrain_work`, as memo entries with read sets; the order assembled from them | `build::plan` after any job: the order and the changed entries only, a few seconds (the order itself is 2-3 s on the bench) | the same; shadow clean |
| 7 | **The records from the lead's memory** (with #878 step 8b): the brief from the pool driver after its merge is saved, not the NAS; the merge's saved generation the trigger | no records read over SMB per plan (5 MB) | a plan with the NAS hung still plans from the lead's records |
| 8 | **"its jobs" from the plan's changes**: offers and job specs updated by the targets whose staleness changed, not rebuilt from the whole plan | "its jobs" (median 20 s) to the changes' size | offers equal to a full rebuild's on recorded states |

A plan after the agent starts (an app update, 14 in §1.1's window) stays a full plan at every step:
166-260 s today, ~200 s with the deep answers cold on the bench; the steps shorten only the plans
after it.

Nothing changes what is built: each step finds the same plan faster, and the full plan stays as the
reference.

## 9. Interactions

- **#916 (the crate split).** 6d (built) moved inputs, dem, osmpass and the other input modules
  into crates/inputs; 6e (the terrain family, then trees, roads, names, bld, offload) is under way;
  6f moves agentcore and `Plan` (build.rs, steps.rs, plan.rs) after #882's later steps. Step 3 here
  (the recording view) changes the signature of every function reachable from `key_lines` that
  takes the manifest: in idents.rs, build.rs and tiles.rs, and those they call in other modules,
  among them terrain_pack's `water_source_pin`, terrain_deep's `mids_under` and slope_pack's
  `deep_mids_under` (6e's family), treepacks' `mid_logical` and heritage's `latest_date` (the
  families after it), and `inputs::logical` and osmpass (crates/inputs). The view's type belongs in
  a crate under all of them (buildstore, beside `read_manifest`), and step 3 lands after 6e's moves
  (it then edits the new crates), not while one is in flight. Steps 1-2 touch agent/plan.rs and
  agent/build.rs only: 6e moves none of the agent, so they can land now (at most an import line
  in build.rs to merge), and collide with 6f alone. Steps 4-6 should land before 6f, or 6f moves
  the incremental state with `Plan`.
- **#882 (Mac roles).** Planning is the lead's alone (#881, roles.md steps 1-2: no region states,
  checklist or `TerrainTiles` on a member), so the incremental state is held on the mini only and
  costs the laptops nothing. The forecast and the build page's data stay readers of `PlanView`.
- **#878 (coordination off the NAS, landed).** The records are at `state/build/records.json` and
  `brief.json` (the lead's own copy on its disk first), numbered by the store's epoch, and the
  lead's coordinator holds the brief of each save whole (`GET /pool/brief`, docs/coordination.md
  §5.1). The plan reads that one in process (`Planner::brief_now`; the NAS's only before the first
  save), so it reads no records over SMB, and its digest takes the brief's (epoch, seq) beside the
  snapshot's stat: step 7's read is built that way; its trigger by the merge's saved generation
  alone is not. The diff of §3 is the same whichever way the brief comes.
