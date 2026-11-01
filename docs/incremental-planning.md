# Incremental planning

**Planned (task #880); step 0 built, step 1 tried and dropped (§5).** Today every plan round recomputes
`agent::build::plan` (crates/pipeline/src/agent/build.rs) from scratch; this doc says how it would
update from what changed instead, and how each step ships alone.

## 1. Where the time goes today

Measured read-only on the live lead (the Mac mini), from the "the loop's phase … took N s" lines
of `~/Library/Logs/scenic/agent.log` (logged only for phases of 10 s or more), the last 3000 such
lines (1 Nov 2026, roughly two days). The lead ran a build from before #879 step 4b, so it still
logs "the records re-keyed".

| Phase (`nasread::phase` / `Planner::phase`) | Lines | Median s | Max s | Sum s |
|---|---|---|---|---|
| the plan: build::plan | 1163 | 27 | 330 | 42 735 |
| the checklist and counts | 543 | 18 | 383 | 15 764 |
| the plan (outside the sub-phases below) | 209 | 17 | 1012 | 15 577 |
| the plan: its jobs | 561 | 14 | 163 | 9 143 |
| the grants | 73 | 105 | 236 | 7 152 |
| the pool's step | 318 | 14 | 69 | 5 521 |
| the plan: the records re-keyed (gone with #879 step 4b) | 39 | 16 | 229 | 1 280 |

Stall lines ("the plan has run > 300 s, in …") in the same window: 21 in "the checklist and
counts", 6 in "the plan". So:

- `build::plan` itself is the largest single cost (median 27 s, every plan round).
- "the checklist and counts" (`Planner::count`, after the plan) was the one that stalled past 5 min;
  since #924 the checklist is made inside `build::plan` from the plan's own keys (one `KeyMemo` for
  both).
- "its jobs" (turning `Plan` into jobs and waits) is proportional to the plan's size, and shrinks
  only once the plan is handed over as a diff (§5 step 5).

Since step 0, `build::plan`'s sections are the plan thread's phases (`nasread::PLAN`: "build::plan:
the sites, water and pyramids", … the terrain and slope, the tree cover, the z6 tiles' keys, the
regions, the map tiles, the regions' order, the chains and publishing, the checklist), logged like
the rest when one takes 10 s or more; no live figures for them yet. The state it reads: the
manifest (37 078 entries on the live NAS, 1 Nov), the done keys, the coverage and each region's,
the inputs' identities, the reaches and the terrain packs' indexes.

`bench_plan_on_a_real_root` (an ignored test; `SCENIC_BENCH_ROOT`) on a frozen copy of the live
records (the M4, test profile, 1 Nov, before #924): `build::plan` 200-322 s a run, and
`terrain_slope_targets` alone, which `build::plan` calls, 199-294 s: the next to look at. The bench
also writes every work's step, target and key (`SCENIC_BENCH_PLAN_OUT`) and checks each derived key
of the plan's work from one memoized context against a fresh one per target.

## 2. What a plan is a function of

`build::plan(cov, date, m, done, inputs, reach, tiles, Publish)` is pure. Since #879 every derived
key is `hash16(key_lines(step, t, x))`, and `key_lines` reads only what the step's row declares
(`Step::reads`): outputs of named targets in `m`, mids, a step's outputs read whole, and input
identities. So a key's dependencies are known from the declaration, without running it.

What changes, and what it invalidates:

| Change | Arrives as | Invalidates |
|---|---|---|
| A job ends | its records: manifest entries and done keys of one step's targets | that step's `done` entries; by the declared reads, the keys of every target whose `Read::Targets` names one of those targets (its `of` inverted), and of every target of a step reading that step `Read::Whole` |
| An input changes | `inputs` entry `name` gets a new identity | every target of every step with a `Read::Input { name }` |
| Coverage (a region added, removed, edited) | `cov`, `Publish::each` | the target lists of the enumerations (which targets exist, which are "none"), per region; keys only through reads that take `x.cov` |
| Reaches, terrain packs' indexes | `reach`, `tiles` | the reads whose `Grain::Read`/`Tiles` function takes them (declared in the row) |
| Time (catalogue cadence, holds) | `Publish::since_*`, `held_for` | only the publish chain's ordering, never a key: recomputed every round (cheap) |

## 3. What is kept between plans

1. **Key memo**: `(step, target) → (key, read set)`, where the read set is the manifest logical
   names, input names and whole-read steps its `key_lines` consulted. Built by an instrumented
   `key_lines` (the same function, recording what it looked up), so the dependency list is derived,
   never hand-kept.
2. **Reverse index**: logical name → readers, input name → readers, step (whole) → readers. A job's
   end looks up the logical names it changed and drops exactly those memo entries.
3. **Whole-read lines per step**: the `w <step> …` lines computed once per manifest change of that
   step's outputs instead of once per target.
4. **Target lists per enumeration and region**, invalidated by coverage, reach and pack changes.
5. **Stale set per step**: the targets whose memoized key differs from `done`, maintained as keys
   and done records change. The ordering (regions, publish chain, chains) is then recomputed from
   the stale sets each round: it is cheap once keys are not re-derived.

Memory budget: the memo holds one 16-char key plus a small read set per target. It must stay within
a few times the manifest's own in-memory size (the manifest is the largest thing the plan already
holds); step 2 measures it on a recorded live state and logs it with the plan's phase line, so the
figure is measured, not hand-kept.

## 4. Staying equal to a full plan

The incremental result must be the full plan, byte for byte. As the map log did
(docs/maplog.md, `incremental_digest_is_the_whole_digest`):

- **Shadow comparer** on the lead: every Nth round (and on any doubt: a restart, a NAS return) a
  full `build::plan` runs on a thread beside the incremental one on the same inputs; a difference is
  logged with the first differing step and target, the full plan is used, and the memo is rebuilt.
- **Tests on recorded states**: a sequence of real states (copied read-only from the lead: manifest,
  done keys, coverage, inputs) replayed change by change; after each, incremental equals full.
- The incremental path is switched on only after the shadow has run clean on the lead for a stated
  time; the full plan stays as the fallback and the comparer stays on.

## 5. Migration, each step shipping alone

| Step | What | Done when |
|---|---|---|
| 0 (built) | Time `build::plan`'s sections (the ten of §1, `nasread::PLAN`) as the plan thread's phases | the live log names where the 27 s go |
| 1 (tried, dropped) | The lines of a step read whole by its outputs (`steps::key_lines`, a scan of the whole manifest per target) worked out once per plan | measured no gain (the M4, 1 Nov 2026, before #924): `build::plan` 311-322 s before, 200-317 s after on the bench (the M4's load varying more than that), every key the same (186 of the plan's work); dropped, #924's `KeyMemo` already removing repeated key work |
| 2 | Key memo across plans with the derived read sets and reverse index (§3.1-3), invalidated by the manifest/inputs diff between plans; the full plan still runs and is compared (shadow, §4) | shadow clean on the lead; memo size logged |
| 3 | Target lists and stale sets kept (§3.4-5), coverage/reach/pack changes invalidating them | shadow clean; `build::plan` time logged |
| 4 | The incremental plan used, the full plan only in shadow every Nth round | owner-visible phase times drop |
| 5 | The plan handed to "its jobs" as a diff, so it stops rebuilding from the whole plan (the checklist, made inside `build::plan` since #924, follows the plan's keys) | "its jobs" and "build::plan: the checklist" phase times drop |

Each step keeps the full plan as the reference; nothing changes what is built, only how fast the
same plan is found.
