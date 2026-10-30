# Readiness and keys from declared reads

**Planned (task #879; nothing here is built yet).** This doc says how the build will derive, from
one declaration per step, both when a target may run (its readiness) and what it is keyed on (its
key). Until the work below lands, docs/pipeline.md §2, docs/steps.md §12 and the code are what is
true. The sections marked *today* describe main at 1edf631e.

## 1. The problem

Three places each encode what a target waits for, and the keys are a fourth:

| Where | What it encodes today | Drift seen |
|---|---|---|
| the planner (`agent::build`) | `build::waits` over `Step::reads` for terrain-lo, slope-*, trees-lo and unit (#864); hand-written orderings in `landmarks_work`, `bld_work`, `depth_work`, `polar_next`, the pass's world steps, the z6 tiles' wait for heritage-sites, reach and buildings | |
| the forecast (`agent::forecast`) | `reads()` over the same declaration for the tiled steps; `chain_deps` (`Step::after` plus its own `pre` list: pois after trailends, peaks after summits, heritage after heritage-sites, and a `reads_terrain` flag that waits on *all* terrain); `UNITS_NEED`; the held-chain guard (#867: a chain job the offers lack waits on its stage's earlier chain jobs) | #863, #867 |
| the page (`pageview`, `/work/tocome`) | the forecast's run, so whatever the forecast believes; waiting notes from `Reads::why` or hard-coded strings (`HERITAGE_WAIT`, …) | |
| the keys (`build::*_key`) | each assembled by hand from versions, content names, digests and fingerprints | #832 (over-broad: coverage rings within 20 km); slope-lo reads the terrain root but nothing waits for it |

`declared_reads_cover_the_keys` (#864) checks that a key's target inputs are declared reads, one
direction only, for the six steps that declare reads. Nothing checks that a key names everything a
run reads, or that the forecast waits for what the planner holds.

## 2. The declaration

`Step::reads` and `Step::after` become one field, `Step::reads: &'static [Read]`, plus
`Step::versions`. Everything a target's run reads is one `Read`:

```rust
pub enum Read {
    /// Some targets of another step, by this target (today's `Reads::of`).
    Targets { step: &'static str, of: fn(&str) -> Vec<String>, grain: Grain },
    /// Every target of another step (today's `after`, `UNITS_NEED`, `chain_deps`'s pre list).
    Whole { step: &'static str, grain: Grain },
    /// An input that is no step's target: the coverage as the run cuts it, a dataset pool's lines,
    /// a pass set, an inputs folder's accepted index, the water digests, a release.
    Input { name: &'static str, of: fn(&str, &Ctx) -> Option<String> },
}

pub enum Grain {
    /// The read target's outputs by content name (`steps::output_names`): the default.
    Outputs,
    /// The tiles it reads inside those outputs, by their XXH3 in the packs' indexes (today's
    /// `slope_reads`, `unit_terrain`): a change elsewhere in the pack doesn't re-key.
    Tiles(fn(&str, &str, &Ctx) -> Result<Vec<String>, Unread>),
}
```

`Step::versions: &'static [(&'static str, fn() -> String)]` names each piece of code whose output the
target's bytes depend on: its own version first, then codecs and borrowed code
(terrain-deep: `terrain-deep`, `terrain-hi`, `terrain-codec`). A version is named, never bare, so a
key's text says which code it pins.

Not a read: job shaping (pyramids-fill one tile per job, `job_of`), packing, places. Those decide how
ready targets are batched, not whether they are ready.

## 3. The single readiness function

```rust
pub enum Ready { Yes, Waits(Vec<(&'static str, String)>), Unknown(Vec<&'static str>) }

/// steps::readiness: whether target `t` of `step` may run in `view`.
pub fn readiness(step: &str, t: &str, view: &dyn View) -> Ready;

pub trait View {
    /// Target `t` of `step` is left (stale or unbuilt, runnable or not).
    fn left(&self, step: &str, t: &str) -> bool;
    /// An input's identity now; None while it can't be worked out (today's `UNKNOWN`).
    fn input(&self, name: &str, t: &str) -> Option<String>;
}
```

A target is `Waits` while any `Targets` or `Whole` read is left, `Unknown` while any `Input` (or a
`Tiles` grain) can't be read, else `Yes`. `Whole { step }` is left while any target of that step
is. Its enumeration, `steps::read_targets(step, t)`, is the one list of what a target waits for.

Callers, each supplying only its `View`:

| Caller | Its `View::left` | Replaces |
|---|---|---|
| the planner (`build::plan`, every `*_work`) | the target's recorded key isn't its key now (`Keys`, `TerrainWork::is_left` generalised to every step) | `build::waits`, the chain functions' hand orderings, `HERITAGE_WAIT` and the other wait strings (the note is `Read`-derived: "wait for {step} {targets}") |
| the forecast (`forecast::items`) | not given its own `View`: it calls `read_targets` and maps each to an item index (a `Whole` read to one shared group per step, as #867's `shared_waits`) | `chain_deps`, `UNITS_NEED`, `reads_terrain`, the held-chain guard |
| the page and `scenic status` | the planner's | `wait_note`'s fallbacks |

The planner and the forecast agree by construction: both enumerate `read_targets`; the forecast's
deps are exactly those reads still in the plan. The #867 guard existed because the planner held
chain jobs for reasons it never declared. Under this design the planner may hold a target only
through `readiness`, and a test enforces that (§7). The guard and its "offers lack it" inference go.

Naming: `store::waits` (#870, measured I/O waits) keeps its name. This design adds no `waits`
symbol: `readiness`, `Ready`, `read_targets`. It doesn't touch `Class` or the measures.

## 4. The key derivation

```rust
/// steps::key: target `t`'s key, or None while `readiness` says Unknown.
pub fn key(step: &str, t: &str, view: &dyn View) -> Option<String>
```

The key is `hash16` of these lines, in this order:

1. `scheme 2` (`KEY_SCHEME`: bumped only with a change to this derivation)
2. `step <name> <target>`
3. one line per version, in declared order: `v <name> <version>`
4. one line per read, sorted, each naming its identity:
   - `t <step> <target> <logical>=<content>` per output of a read target (`Grain::Outputs`); a target
     with no outputs: `t <step> <target> -`
   - `tile <step> <target> <z>/<x>/<y> <xxh3>` per tile read (`Grain::Tiles`)
   - `w <step> <target> <logical>=<content>` for a `Whole` read, every target listed (sorted)
   - `in <name> <identity>` per `Input`

So a key's text is self-describing. A planned command (a `key` subcommand of scenic-build) prints the lines, not just
the hash, and a re-keyed target can be diffed against its last run's lines (kept with the record, as
today's restore keeps water's).

The rule *a target re-keys exactly when something it reads changes* holds because the key's lines
are the run's declared reads, and the run itself is held to them (§7: the run's reads recorded by the
store's read log, #866's `outside` machinery, must be a subset of the declared reads).

`Input` identities are named by what the run cuts, never by a broader proxy. The coverage is
`in coverage:<cut> <digest>`, where `<cut>` is how the step reads it: `piece` (#832's
`piece_coverage`: the z9–12 tiles near the coverage), `inside` (trees: the coverage in the tile),
`ways` (unit: as its ways meet it), `shapes` (bldtiles: `Coverage::shapes_key`), `all`
(`coverage_all`). The same cut is one function shared by key and run, so the two can't differ.

"None" targets (a target the coverage left, kept to drop its packs) declare no reads. Their key is
lines 1–3 and `none`, as `none_*_key` today, from one function.

## 5. Every step's key, today and derived

"Same inputs" means the derived key names what today's names, in the new framing. The record is
re-keyed, not rebuilt (§6). "Changes" means the set of inputs differs.

| Step | Today (docs/steps.md §12) | Derived reads | Change, and why |
|---|---|---|---|
| terrain-water | water pin + each target's reads | `in water-pin`, `in water-wants` | same inputs |
| pyramids-fill | `pyramids::V`, tile, coverage 20 km fingerprint, pyramids' lines, water, deep targets | versions; `in coverage:piece`, `in pyramids`, `in water`, `Targets terrain-deep` (identity: the target list, `in deep-targets`) | changes: #832's `piece_coverage` |
| terrain-hi | `TERRAIN_V`, codec, coverage 20 km rings, datasets, deep key lines, water, "no hi tiles" | versions; `in coverage:piece`, `in datasets:terrain`, `in deep-lines`, `in water`, `in makes-hi` | changes: #832 (coverage cut, datasets' outlines) |
| terrain-deep | versions, tile, fine lines, each base z6 tile's terrain-hi inputs | versions (incl. `terrain-hi`); `in fine-lines`; for each base z6 tile the same `in` lines as its terrain-hi | changes: #832, via its bases. Its bases' *inputs*, not their outputs (no wait), stay: terrain-deep builds its base itself |
| terrain-lo | versions, codec, water, sources, z6 mids | `Targets terrain-hi` (mids), `in water`, `in datasets:terrain` | same inputs. The `sources` line is undocumented today; the doc table gains it |
| slope-hi | `SLOPE_V`, codec, hi pack, deep mids, tiles read | `Targets terrain-hi` and `terrain-lo` `Grain::Tiles`, `Targets slope-deep` (mids), `terrain-root` (decision 2) | changes: the root becomes a declared read (today keyed, not waited) |
| slope-deep | `SLOPE_V`, codec, tiles read | `Targets terrain-deep`/`terrain-hi`/`terrain-lo` `Grain::Tiles`, `terrain-root` (decision 2) | changes: root a declared read |
| slope-lo | versions, mids, terrain lo of it + edges + root | `Targets slope-hi` (mids), `Targets terrain-lo` (+ edges), `terrain-root` (decision 2) | changes: root a declared read; doc's "the z3 tile's lo pack" corrected to edges + root |
| slope-root | lo packs, codec | `Whole slope-lo` | same inputs |
| polar-terrain | `polar::TERRAIN_V`, codec, block, datasets by latitude | versions; `in datasets:polar` | same |
| polar-slope | versions, block, terrain of it and built neighbours | `Targets polar-terrain` (block + neighbours) | changes: today `after: polar-terrain` waits on *every* block; derived waits only on its neighbours |
| polar-root | versions, pole, datasets, blocks, every block pack | `Whole polar-terrain`, `Whole polar-slope`, `in datasets:polar` | same |
| trees | coverage inside the tile, `TREES_V` | `in coverage:inside` | same |
| trees-lo | z6 mids, `TREES_LO_V` | `Targets trees` | same |
| heritage-sites | pass, areas set, registers, matches' version, coverage | `in pass:areas`, `in registers`, `in matches`, `in coverage:all` | same |
| unit | extract, road values, coverage as ways meet it, rules, heritage slices, roadside index, elevation plan, terrain tiles by content | `in extract`, `in road-values`, `in coverage:ways`, `in rules`, `Whole heritage-sites` (its slices), `Whole buildings` (roadside index), `Whole reach`, `in datasets:plan`, `Targets terrain-*` `Grain::Tiles` | same inputs; the waits on heritage-sites, reach, buildings move from `UNITS_NEED` and the planner's code into the row |
| pack(T) / lo | owners' base packs and road values | `Targets unit` | same |
| rail-feeds, rail, water, bld-fetch | as §12 | `Input`s, and rail: `Whole rail-feeds` | same |
| bldprep | `BLDPREP_V`, release, row groups and GHSL tile | `Whole bld-fetch` (one world target), `in release` | same |
| bldtiles | `BUILDINGS_V`, normalized files of tile + 8 neighbours, shapes | `Targets bldprep` (tile + 8), `in coverage:shapes` | changes: today `after: bldprep` waits on every bldprep; derived waits on 9 |
| depth | versions, tile, sources in margin, water | versions; `in depth-sources`, `in water:depth` | same |
| depth-lo | versions, sources, water, z6 hi packs | `Targets depth` (its z6 tiles), `in depth-sources`, `in water:depth` | changes: today `after: depth` waits on every depth tile; derived on its 64 |
| depth-root | versions, every depth lo pack | `Whole depth-lo` | same (`after: depth` was redundant) |
| pois | extract spacing, trailends | `in extract`, `Whole trailends` | same |
| summits | (pass) | `Input`s | same |
| peaks | candidates, summits, z8 digest, hi packs within 30 km | `Targets pois` (its tile), `Whole summits`, `Targets terrain-hi` (30 km box), `Targets terrain-lo` | changes in waiting only: the forecast's `reads_terrain` waited on *all* terrain; derived waits on its box. Key same |
| items | date, current candidates | `Whole pois` | same |
| heritage | heritage-sources, sets, coverage | `Whole heritage-sites`, `in pass:*`, `in coverage:all` | same |
| marks | facts, views, heritage files, every tile's pois and peaks | `Whole items`, `Whole heritage`, `Whole pois`, `Whole peaks` | same |
| overlays | heritage outputs, dots | `Whole heritage`, `Whole marks` (dots) | same |
| catalogue | manifest of what's ready | `Whole` of every pack step | same |

The key changes (new inputs) are all in the terrain family, already in #862's rebuild: terrain-hi,
terrain-deep and pyramids-fill (#832), and slope-hi, slope-deep and slope-lo (the root, which they
already re-key on through the manifest's `layers/terrain/root/0-0-0`). The rest gain only framing
(scheme 2 and the identity lines), and some wait less: polar-slope, bldtiles, depth-lo and peaks.

## 6. Migration and the rebuild (#862)

1. **Land the derivation behind scheme 1.** `steps::key` emits today's exact bytes for every step
   while `KEY_SCHEME == 1`: a per-step `legacy` adapter assembling the old order from the declared
   reads. A test asserts byte-equality with each `*_key` on the fixture coverages. Readiness switches
   at once (no rebuild: it changes no key). The forecast's tests (§7) land here. This is the bulk of
   the code change, safe to merge any time.
2. **Merge task-832-keys onto it.** #832's coverage cut becomes the `coverage:piece` input. It still
   ships only with step 3.
3. **Flip to scheme 2 with #862.** In one commit: `KEY_SCHEME = 2`, the legacy adapters and every
   `*_key` deleted, and a `rekey` scheme (agent/rekey.rs, docs/building.md §2): a record current
   under scheme 1 is recorded under its scheme-2 key when its scheme-2 lines name no input the
   scheme-1 key didn't pin. That's every step outside the terrain family. The terrain family's
   records go, and they are #862's planned rebuild. The cascade then follows by content names: a
   unit whose terrain tiles' XXH3 didn't change keeps its key. So the rebuild is terrain and slope,
   plus what genuinely reads changed tiles.
4. The scheme-1 rekey adapter is deleted once no record is scheme 1 (one catalogue after).

## 7. Tests

- `steps::tests::every_wait_is_a_read`: for each fixture state, every target the planner leaves
  out of its offers for readiness has `readiness != Yes`, and every target with `Yes` and a stale key
  is offered (the planner holds nothing undeclared). Replaces #867's held-chain guard.
- `forecast::tests::the_forecast_waits_for_what_the_planner_does`: for each fixture and on a
  `SCENIC_FORECAST_RECORD` record, each item's deps (expanded through its groups) equal
  `read_targets` filtered to items in the plan; and replaying the forecast's first grants through
  the planner offers each at the time the forecast places it. Supersedes
  `a_replay_never_places_a_target_the_live_plan_holds_back`.
- `steps::tests::a_key_names_exactly_its_reads`: for each step, changing each declared input (one at
  a time, in a fixture manifest) changes the key; changing any undeclared logical file doesn't.
  Replaces `declared_reads_cover_the_keys` (which checked one direction for six steps).
- `steps::tests::a_run_reads_only_what_it_declares`: per-target steps run on fixtures under the
  store's read log; each logical file read is named by a declared read. (This is the direction
  nothing checks today.)
- `steps::tests::scheme_1_bytes` (step 1 only, deleted at step 3) and
  `rekey::tests::scheme_2_keeps_what_reads_nothing_new`.

## 8. What gets deleted

- `steps::Reads` and `Step::after` (merged into `Step::reads: &[Read]`), `build::waits`,
  `build::wait_note`'s fallback, `HERITAGE_WAIT` and the planner's other hand-written wait strings.
- `forecast::chain_deps`, `UNITS_NEED` (both copies), the `reads_terrain` terrain group, the
  held-chain guard and its `offered` inference.
- `TerrainWork::is_left`'s step match (each step's left set is the generic `View::left`).
- Every hand-assembled key: `terrain_piece_key`, `piece_key_made`, `terrain_deep_key`,
  `pyramids_key`, `terrain_lo_key`, `slope_lo_key`, `slope_piece_key`, `slope_deep_key`, the
  `none_*_key` trio (one `none` derivation), `unit_key`, `peaks_keys`' and `pois_keys`' hashing,
  `items_key`, `heritage_key`, `marks_key`, `overlays_key`, `heritage_sites_key`, `outlines_key`,
  `reach_key`, `depth_root_key`, `bld_fetch_key`, `bld_key`, `root_key`, `catalogue_key`. What they
  gather survives only as `Input` functions shared with the runs.
- `slope_lo_terrain`, `slope_piece_from`, `slope_deep_from`, `unit_terrain_from` as separate memo
  keys (the memo is keyed on the read's `Grain::Outputs` lines, derived).
- `UNKNOWN` (`Ready::Unknown`).
- docs/steps.md §12's per-step prose becomes a table generated from the declarations (a planned flag of scenic steps), so it can't
  drift either.

## 9. Work in flight

- #870 (measured waits, `store::waits`, no `Class::Mixed`): no shared symbols. The forecast's costs
  are untouched, and this design changes only item deps. Rebase after it merges.
- #889 (resume logs in per-target steps): touches the steps' run code, not their planning or keys.
  `a_run_reads_only_what_it_declares` runs those steps, so it lands after #889.

## 10. Open decisions for the owner

1. **Rekey or rebuild everything at step 3.** As written, steps outside the terrain family are
   re-keyed (no rebuild) and only #862's terrain rebuild runs. The alternative is to drop every
   record and rebuild the world: simpler (no rekey scheme) but days of unit, depth and buildings work.
2. **The root as a waited read.** slope-* would wait for terrain-root, which is built at the finish
   from every lo pack. Either root moves earlier (built per stage once terrain-lo is done), or slope
   keeps reading it without waiting (an explicit `Read::Input` on its content, documented as the one
   read that isn't waited on). Moving root earlier is the root-cause fix; it delays slope until all
   terrain-lo is done.
3. **Generated steps.md §12.** Replace the hand prose with a table generated from the declarations, or keep the
   prose and test it against the declarations.
