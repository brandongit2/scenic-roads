# Incremental planning

**Status: planned (task #880).** Built: step 0 (the plan's sections timed as the plan thread's
phases, `nasread::PLAN`), step 1 (the regions' counts the plan's, its keys handed on in its round),
step 2 (a change to what granting reads filters the last plan's work again), step 3 (each key's
read set recorded by construction) and step 4 (the plans made again incrementally beside them, in
shadow, the latest taken and those it had no time for skipped, its keys kept across plans and
compared). Everything else here is planned; until a step
lands, the code is what is true. Today every plan computes `agent::build::plan` (crates/pipeline/src/agent/build.rs) from
scratch: every target's key, every target list, every region's work, from the whole records. This
doc measures where that time goes, how much of it is recomputed though nothing it reads changed,
and how the plan part (docs/agent-parts.md §4.4, #871) would keep its answers between plans and
update only what each change touches, each step shipping alone.

## 1. Where a plan's time goes (6 Nov 2026)

### 1.1 Live, on the lead

The Mac mini's `~/Library/Logs/scenic/agent.log`, read only, from the first plan after #965's
publish (5 Nov 21:35Z) to 6 Nov 12:26Z: 14.9 h, apps 20261105-2124-d1403c86 to
20261106-1214-b4237b4b. `agent: plan <n> (<why>) took N s` was logged then for plans of 10 s or more;
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
   reaches, and what of `TerrainTiles`' load state it met (below). It is recorded by construction
   (built in step 3): `KeyCtx`'s manifest and inputs are views (`buildstore::reads::Reads`) with no
   way to the map but their reads. Code reading the manifest for a key takes a `Manifest` (the map
   itself, as a job or a target list has it, or the view), and the view records each read into the
   key being derived while its memo records (`KeyMemo::after`, the shadow plan's: step 4): an entry's content
   (`get`, an absent one too), its presence (`contains_key`), the entries under a prefix (`under`,
   in place of `range`), a content some entry holds (`holds`), a step's outputs read whole by its
   write set (`matching`), the map whole (`whole`, the one way to the map: what the plan reads
   outside a key, recording nothing there) and state the entries don't show (`met_load`). A
   function that took `x.m` as a map no longer compiles with it. `TerrainTiles`' readers (`hash`,
   `resolve`, `elevation`, `zoom_sets`, `keyed`, …) take a `Manifest` and record the pack and index
   entries they look up, not a flag "read a terrain index" (which every z6 tile's key sets: any
   pack's change would derive them all again, today's all-or-nothing). What construction alone
   misses, and how it is closed:
   - **A cache hit inside a derivation.** A key found in `KeyMemo` while another is derived is a
     kid of it (`Recorder::found`): moving the kid moves its reader. A `TerrainTiles::memo` answer
     (a z6 tile's terrain digest, a target's asked line, the slope's tiles) is kept with what it
     was made from (`from`), which the key works out by reading the manifest itself before asking:
     a hit's reads are those. The deep answers are kept by the coverage and the declared sets the
     key reads likewise. The caches warmed on rayon's threads (`warm_asked`, `warm_lakes`,
     `warm_peaks`) read the map unrecorded (`Reads::unrecorded_map`; the view isn't `Sync`, so the
     compiler refuses one captured there): each key reads again what finds its line (lake-depth's:
     its record and its ring's hi packs, `asked_line`, `lake_ring_geo`; peaks': its record and the
     lo packs' indexes and the summits, `peaks_geo`). The terrain-targets cell
     (`TerrainTiles::memo("terrain-targets")`) is read by no key, and the terrain and slope keys
     it serves aren't derived in a plan that finds it, so they get no read set there: they are
     kept by that cell's digest until step 6a. The shadow plan (step 4) reads the cell where the
     plan left it and never writes it (its targets are made from its own keys, which are what it
     checks); when the plan couldn't keep it (a slope key unknown), the shadow works the targets
     out from its keys, kept or derived, as the plan does. Of the other caches the plan shares
     (`KeyCtx::shadow`): the warm passes' marks ("lake-depth:asked warm", "coverage:asked warm",
     "peaks:asked warm") it reads and never writes, skipping all three passes (each key it derives finds its line as above);
     the 3D buildings' tiles (`bld_tiles`, kept by the coverage's digest) it reads from either slot
     and writes only its own, so the plan's stays the plan's; the per-target `TerrainTiles::memo`
     answers (a function of what they were made from, which the key reads) it reads and writes as
     the plan does.
   - **A miss.** `get` of an absent entry and an empty `under` are reads: an entry added later
     must find the key.
   - **State the manifest doesn't show.** A key that came out unknown (`None`) is never kept. A
     reader of `TerrainTiles` meeting what it can't read now records `met_load`, and such keys move
     when what it can't read changes (`TerrainTiles::load_state`: the packs and declarations
     unread). What it holds is by content name, which the keys read in the manifest.
   - **Reads beside the arguments.** `steps.rs`, `idents.rs` and `TerrainTiles`' readers read no
     environment, clock or NAS folder listing today (no `env::var`, `now_s`, `read_dir`; the modules
     they call, such as terrain_pack's `water_source_pin` or heritage's `latest_date`, unchecked).
     Two `TerrainTiles` readers read NAS files while a key is derived, each by a content name the
     key read in the manifest first: `target_asked` (a target's coverage record, read again once
     its geometry moved) and `outlines` (the outlines a record's question names). Content-named
     files don't change under their name, and one that can't be read leaves the key unknown, never
     kept. The NAS is read for keys otherwise by `TerrainTiles::load` and `key_inputs`, whose
     results are diffed (the inputs) or tracked above. The app's version is the process's: the state starts empty after an update.
     The manifest is a `BTreeMap`, so neither the brief's order nor a NAS listing's reaches a key
     through it. Two guards keep it so (task #1517): `tests/key_reads.rs` refuses an
     environment, clock, local file or NAS read in the code of steps.rs, idents.rs, build.rs and tiles.rs
     outside the plan's loads and the two readers above, each listed with why; and
     `store::keyread` makes a NAS call (any of `nasfs`'s, through `store::waits::nas`) or the
     pool's clock (`jobrt::jobs::now_s`) panic in a debug build (the tests') while a key is
     derived on that thread (`key_lines`, `unknown_reads`, the warm loops' lines), but within those
     two readers' recorded reads: every key the tests derive runs under it. Not covered: a direct
     `env::var` or `SystemTime::now` in another crate's code a key calls, a local file another
     crate reads as a key is derived (store's mirror, a cache file: watched nowhere, at run time
     or in the scan), and work a derivation hands to other threads.
2. **The reverse index**: entry name → keys, prefix → keys, input name → keys, and the keys that
   read `TerrainTiles`' load state. A changed name finds its keys by exact lookup and by each of its
   own prefixes (a name is ~60 bytes: ~60 lookups), and the write sets read whole are asked of it.
   Built in step 3 (`buildstore::reads::Graph`: (read, key) and (kid, reader) pairs, sorted). The
   coverage, the pass and the reaches aren't recorded per key: a change of any of them moves every
   key (`build::context_rest`, of the reaches the plan's keys read: none while the reach job is
   due, the last ones read still cached), a full plan as §5 has it.
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
| A lease granted, lapsed or given back | the coordinator's `lease_gen` (`plan::Granting`) | not `build::plan`: "its jobs" filters the last `Plan` again and offers it. Not in `plan_seen`'s digest: a trigger of its own |
| The pause, the placement switch, the pool's gates | `plan::Granting` | "its jobs" again, likewise. Publishing's switches (`hold-catalog`, `near-slope`) stay in `plan_seen`: `build::plan` reads them (`seam::Publish`) |
| Time (catalogue cadence, holds, `Publish`) | `Publish`, `held_for` | the publishing chain's ordering only: recomputed every plan |

## 6. Equal to a full plan

The incremental plan must be the full plan, byte for byte (its `Plan`, keys included), as the map
log's `incremental_digest_is_the_whole_digest` holds for its digest (docs/formats.md, The map and its log):

- **A shadow full plan on the lead**: after a restart, after the NAS's return, and every Nth plan
  (N set on the build page, shown with the plan's phase times), a full `build::plan` on the same
  inputs, on a worker of the plan part (its own thread and memo, none shared), compared with the
  incremental one. A difference is logged with the first differing step and target, its recorded
  reads and the changes since the last plan; the full plan is used, the state rebuilt from it, and
  the status says so (a flag the page shows, not a log line alone: step 5's, with the incremental
  plan used). Until step 5 the roles are the other way round (step 4, built): the full plan is
  the plan, made and used as before, and the incremental one is made beside it, on the plan
  part's worker (theplan's `Shadowing`: a thread of its own and two rayon threads at the utility
  QoS, its own memo and kept keys, a copy of the plan's round, started once no `build::plan` is
  being made), and compared with it. Not every plan's: the worker takes the latest round waiting
  (a one-slot mailbox, `bus::mailbox`), and a round replaced before it was taken is skipped, its
  count on the next shadow's line. A difference is logged,
  each differing key by step and target with its key and the plan's and, for those kept among the
  first five of the differing keys sorted (`SAID_KEYS`), the lines of what it reads that changed
  since it was kept; the incremental state is then dropped,
  and the next shadow derives every key. After step 5, N counts time as well as plans (a shadow
  at least every 30 min whatever the plans' rate), and:
  - **A map update is planned from a confirmed plan.** A map update is the plan's one
    output others see; between shadows a wrong key can only delay or repeat a job, which the next
    shadow corrects (a job records the plan's key, which the full plan then finds stale), but a
    region published as ready would stay on the map. The plan that puts a map update in the work
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

- **Read sets are kept as hashes, never as lines or names** (step 3, `buildstore::reads`): each
  read a 64-bit id (its kind, a 60-bit hash of the name), kept only as the reverse index's
  (read, key) pairs; a write set read whole as one id. Measured on the lead's state (§8 step 3):
  49 MB after the first plan after a start (2.0 M reads), 17.5 MB after a later one; the recorder's
  nodes and the index beside them while it's built are logged as the index's "at most".
- **The target lists** (§4.3) are what each plan already holds while it runs (7 147 work
  targets, 33 279 done keys, the terrain and slope targets), now kept between plans instead of
  dropped.
- **A second copy of the records' manifest, for now**: the shadow keeps the manifest and inputs
  its last plan read for the diff (theplan's `Context`: 10.9 MB, measured on a frozen copy of the
  lead's state, §8 step 4), and each plan hands it a copy of its round (`Ask`: the manifest, the inputs, the done keys, the plan's result
  and keys), so for a moment two of them; its line logs what it keeps. It's a copy because the
  Planner builds its manifest each plan and hands it on by reference (`seam::Round`); shared
  instead, it would cost nothing.
- **The shadow plan** (§6) runs with its own memo on its own thread, as a plan does, beside the
  plan after it when they come close: at most a plan's own live peak more while they overlap
  (measured in §8 step 4).

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
(`key_lines` 1.9 GB, `build::unit_deep_tiles` 1.0, `roads::staging::Area::tiles` 0.7, a reach as
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
| 2 (built) | **Granting's inputs out of the plan's trigger**: the leases' generation, the pause, the placement switch and the pool's gates (`plan::Granting`) leave `plan_seen`; their change filters the last `Plan` again ("its jobs") and offers it | fewer plans: some of the "what it depends on changed" (each plan's line names the parts of `plan_seen` that changed, `plan::Seen`; each filtering's what granting read changed, to count them) | the same offers as a full plan on a recorded state |
| | *Built:* `plan::refilter_due` runs "its jobs" over the kept `build::plan` result and its keys (`seam::PlanHeld`; `build::plan` made again when the keys' context no longer reads the same) when what granting reads changed alone, under a plan number of its own (a job's grant by its id must find the spec of the offers it was granted from, task #923), logged as "plan N: the last plan's work filtered again (<what> changed) in X ms"; every plan is logged, "plan N (<why>) took X.X s", a change naming its parts ("what it depends on changed: the records"). Publishing's switches stay in `plan_seen`: `build::plan` reads them (`seam::Publish`). Tests: `a_lease_change_filters_the_last_plans_work_again`, `what_granting_reads_offers_the_last_plans_work_under_a_new_number`, `the_plans_digest_follows_what_it_depends_on`; on a recorded state, `frozen_refilter_offers_a_full_plans` (ignored, `SCENIC_FROZEN_ROOT`): 19 Nov, the lead's `state/` copied at 17:01Z, on the M4, its work filtered again after the pause, placement and the gates changed in 1.0 s, its jobs, waits and offers the same 33 lines as a full plan's (19.9 s, the first 509 s cold). *Measured on the lead* (the Mac mini's agent.log, 6-19 Nov, plans of 10 s or more, the only ones logged then): 6 023 plans in 309 h, median 40 s, 30 % of a core; "what it depends on changed" 1 600 (median 22 s, sum 61 774 s), "a job ended" 3 425 (median 72 s); the leases' 204 filterings again, median 2.7 s, p90 15 s. The last 48 h: 1 547 plans, median 32 s, 41 % of a core; 225 "what it depends on changed" (median 32 s), 88 filterings (median 2.4 s). How many of those plans the pause, placement and the gates brought is not known from those logs: the per-part line counts them from this step on | | |
| 3 (built) | **Read sets, recorded (shadow)**: `KeyCtx`'s manifest becomes the recording view (§4.1), the Planner keeps the last brief and diffs the next; each plan builds the read sets and reverse index and logs their size and the keys the diff would have kept; the plan itself unchanged | latency as now (the recording's cost measured); memory measured (§7) | the logged "would keep" equals, on recorded states, the keys that came out unchanged |
| | *Built:* `buildstore::reads` (the view, `Manifest`, `Recorder`, `Graph`, `Diff`; §4.1-4.2): the keys of a recording memo record what each read (since step 4 only the shadow plan's, `KeyMemo::after`: the plan's own memo records nothing, and the plan no longer logs "would keep"); kept as the reverse index (reads as 60-bit hashes of the names, a kind on top), they are moved by the diff of the manifest and inputs the keys read to the next round's (`Graph::moved`). Tests: buildstore's `each_read_is_moved_by_what_it_sees` and its kin; `the_recorded_reads_see_every_change_that_moves_a_key` (every fixture and one holding its declarations and coverage records as a load does, `held_fixture`; each entry changed, removed and added alone, each input changed, the reaches gone with their entry the same; the held build's changes again with the keys recorded on terrain whose memos a plan before filled: 62 968 keys, none changed unseen, 930 moved the same); `the_elevation_datasets_are_read_with_their_geoid_sources`. *Measured* (19 Nov, the lead's `state/` copied at 18:46Z and 19:07Z, the M4, test profile, loaded): 3 entries apart (three bldtiles records put); of the first plan's 30 079 keys the diff would keep 30 076; of the 16 606 derived again all came out unchanged; kept but changed 0, moved but unchanged 0, moved by their own record 3 (those three bldtiles targets). The same state again: all 16 607 kept, all unchanged. Recording asked for 2-52 MB more of a plan's 10.2 GB, its time within the M4's noise; the read sets held 49.0 MB after the first plan after a start (30 079 keys, 2.0 M reads), 17.5 MB after a plan after it (16 607 keys, 0.72 M reads), made in 114 and 19 ms, the diff in 1 ms | | |
| 4 (built) | **Keys kept across plans (shadow)**: the memo outlives the plan, invalidated by the diff; the full plan still runs and is used, and the latest plan is made again beside it, the two compared (§6) | latency as now: the full plan used, the incremental one on a worker of the plan part beside it (its CPU on top: the plan thread is busy 48 % of the time already) | the shadow clean on the lead for a week; the tests of §6 |
| | *Built:* the plan is made and used as before, its memo recording nothing; once made, a copy of its round (the manifest, inputs and done keys, its result and keys; the coverages, reaches and terrain indexes shared) goes to the plan part's worker (theplan's `Shadowing`: a thread of its own and a rayon pool of two threads, all at the utility QoS; a one-slot mailbox, `bus::mailbox`, the plan's round replacing one not yet taken, and a hold the plan keeps while it makes `build::plan`, a panic dropping it too, so the worker takes the latest round once no plan is being made and ends when the plan part does; the rounds it had no time for skipped, counted on its line), which plans again with `KeyMemo::after`: the keys of its own plan before that the diff since doesn't move (`Graph::moved`; none that came out unknown nor reading one; none when the coverage, its countries and sources included, the pass or the reaches changed: `context_rest`, `KeyCtx::stamp` and `bld_tiles` name the coverage by `Coverage::identity`, its version (the rings by their fingerprints, kept a shape) with each shape's source and country, where the first two took its version alone, which leaves the 3D buildings' keys' country out; never by `Coverage::digest`, which reads every ring's point. Measured (20 Nov, the lead's `state/` copied at 03:00Z, the M4, test profile, `frozen_plans_memory`'s line): a call of `digest` 1.2 s (the M4's load 14) and 2.9 s (load 107), a call of `identity` 0.7 and 1.5 ms, its first 0.35 and 0.91 s (the shapes' fingerprints made, once a shape, as `version` makes them); a plan after the first 25.6 and 23.6 s with the digest (load 5), 28.2 and 22.6 s with the identity (load 107 falling to 5), the first 230.9 against 249.6 s: within the M4's noise), each found as derived then, the rest derived and recorded (`Recorder::after`, renumbered as the graph is taken). Of the caches the plan shares it writes only the per-target `TerrainTiles::memo` answers and its own `bld_tiles` slot; the terrain-targets cell and the warm passes' marks it reads, never writing them (§4.1). Its keys and `Planned` are compared with the plan's; its line, "plan shadow N: the same as the plan, key by key and whole | DIFFERS …; of the shadow before's K keys, the diff (E entries, I inputs) kept …, … derived, moved but the same … (by step, the first five named), and by their own coverage record put …; the plan's keys it lacks, the terrain's and slope's aside (served by the cell, not derived) …; its CPU and wall time against the plan's, the rounds skipped; what it keeps, in MB", then a line per differing key (step, target, kept or derived, its key and the plan's; for those kept among the first five of the differing keys sorted, `SAID_KEYS`, the key lines of what it reads now and when kept that differ) and the plan's first field apart (`first_difference`). After a difference it keeps nothing: the next shadow derives every key. Phases on its threads go nowhere: the worker's and its pool's are `nasread::unphased`, so `build::plan`'s sections, entering `nasread::PLAN`'s phases on whatever thread runs them, change neither the plan's phase in progress nor its slow-phase lines; with no pool of its own (rayon's threads are the plan's) no shadow runs. Tests: `an_incremental_plan_is_the_full_plan` and its kin, a test per fixture or two (`incremental_is_full`: each entry changed, removed and added back, each input changed, the reaches gone; each plan on terrain of its own, and in the held build as on the lead too, after the full plan on the same terrain): 52 k keys kept in all, every key and `Planned` the full plan's; `keys_kept_twice_are_the_full_plans` (two changes in a row, each shadow going on from the one before's keys); `a_country_change_keeps_no_key`; agentcore's `a_mailbox_holds_the_latest_alone`; `a_shadow_leaves_the_plans_phase`; buildstore's `a_kept_graph_goes_on_without_what_moved`; on recorded states, `frozen_shadow_plans_agree` (ignored, `SCENIC_FROZEN_ROOT`, `SCENIC_FROZEN_ROOT_B`). *Measured* (20 Nov, the lead's `state/` copied at 00:37Z and 00:47Z, the M4, test profile, loaded): every shadow the same as its plan. The first state: its shadow derived all 16 606 keys in 22.2 s of CPU (the terrain's targets and the deep answers found) against the plan's 195.7 s; the second (47 entries apart): kept 14 543, derived 2 063, 106 of them unit keys moved but the same (the diff's imprecision) and 15 moved by their own coverage record, 11.8 s of CPU against the plan's 39.2 s; the second again: all 16 606 kept, 4.9 s against 18.6 s. What it costs the plan: the same state with no shadow 23.1 s, with it 21.7 s, and planned again beside that plan's shadow 21.3 s (the M4's noise); asked for 28 MB more of 9.4 GB (the round's copy); the most live 219-222 MB either way, the footprint 951 MB against 953-954. The shadow held at most 202-211 MB more while it ran alone (a plan's own live peak, beside the plan's when they overlap) and keeps 24.4 MB (the keys 1.9, their reverse index 11.6, the context for the diff 10.9). The lead's peak (`frozen_plans_memory`, the second state, four plans back to back as the plan thread's look makes them, `SCENIC_FROZEN_NO_SHADOW` against not): the footprint's most 1 292 MB without the shadow, 1 606 MB with one started at once (the first shadow, deriving every key, beside the second plan: +314 MB), 1 414 MB with it started once no `build::plan` is being made, as built (+122 MB, 9 %); the footprint after each plan 675-732 MB against 983-988 (the M4's noise), its reusable pages 1.4-1.6 GB against 0.45 (measured, not explained: the shadow's run overlapping the plan's is the likelier cause); the plans 22-24 s either way, asking for 1.3-1.8 GB more in the window the shadow ran in (its own asks). On the lead: once published | | |
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
  (the recording view, built) changed the signature of every function reachable from `key_lines`
  that takes the manifest to a `Manifest`: in idents.rs, build.rs and tiles.rs, and those they call
  in the crates as they are now: terrain's `water_source_pin`, `mids_under`, `named_z10_under`,
  `named_under`, `deep_mids_under`, `depth::lakes::named`, `depth::surface::reads` and
  `depth::grids::estimates_key`, landmarks' `heritage::latest_date`, inputs' `lakefill::water_pin`
  and `current`, `elevation::switched`, `dem::spacing::key_line` and `osmpass::previous_outlines`.
  The view's type is in buildstore (`reads`), under all of them. Steps 1-2 touch agent/plan.rs and
  agent/build.rs only: 6e moves none of the agent, so they can land now (at most an import line
  in build.rs to merge), and collide with 6f alone. Steps 4-6 should land before 6f, or 6f moves
  the incremental state with `Plan`.
- **#882 (Mac roles).** Planning is the lead's alone (#881, #882; docs/agent-parts.md §9.2: no region states,
  checklist or `TerrainTiles` on a member), so the incremental state is held on the mini only and
  costs the laptops nothing. The forecast and the build page's data stay readers of `PlanView`.
- **#878 (coordination off the NAS, landed).** The records are at `state/build/records.json` and
  `brief.json` (the lead's own copy on its disk first), numbered by the store's epoch, and the
  lead's coordinator holds the brief of each save whole (`GET /pool/brief`, docs/coordination.md
  §5.1). The plan reads that one in process (`Planner::brief_now`; the NAS's only before the first
  save), so it reads no records over SMB, and its digest takes the brief's (epoch, seq) beside the
  snapshot's stat: step 7's read is built that way; its trigger by the merge's saved generation
  alone is not. The diff of §3 is the same whichever way the brief comes.
