# Readiness and keys from declared reads

**Steps 1 and 2 built (task #879, §6); steps 3–4 Planned.** This doc says how the build derives, from
one declaration per step, both when a target may run (its readiness) and what it is keyed on (its
key). Readiness is derived now (§3, as built in §6 step 1); the keys are still today's, reproduced
byte for byte under scheme 1, and the scheme-2 derivation (§4) is planned. The sections marked
*today* describe main at 1edf631e, before step 1.

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

The reads also order the work (task #909): readiness holds what's upstream ahead of what reads
it, so no step is ranked by hand. The plan lists a region's ready work with each step ahead of the
steps it reads (`steps::readers_first`; docs/pool.md §7.2), and every ask is granted in plan order, and a step whose targets
another reads several of for one of its own (`steps::read_together`, probed from these same
declarations) is planned in compact Z-order blocks (`steps::block_order`), so its readers become
ready a neighbourhood at a time.

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

1. **Land the derivation behind scheme 1. Done (task #879).** As built:
   - `Step::reads: &[Read]` replaced `reads` and `after`. `Read` is `Targets { step, of }`,
     `Mids { step, of }`, `Whole { step }` and `Input { name }`; `Grain` and an input's identity
     function come with scheme 2, which needs them for the key lines. `Mids` is what terrain-lo
     and trees-lo read of their z6 tiles: the mids, left while a z6 tile is stale or isn't current
     with its mid (made again for it alone, its packs unchanged); what reads the packs (slope, the
     z6 tiles, the summits and peaks) waits only for those stale. `View` is `left`, `mid_left`
     (a `Mids` read), `any_left` (a `Whole` read) and `known` (an `Input`); `steps::Left` is the
     planner's view (`Plan::left`).
   - `steps::readiness` gives `Yes`, `Waits` (with targets) or `Unknown`; `steps::read_targets`
     lists what a target waits for; `steps::wait_note` words the holds ("wait for terrain-hi (12)").
     The planner holds every target through it (terrain, slope, trees, the z6 tiles' wait for
     heritage-sites, reach and buildings, the landmarks, the 3D buildings, depth, polar, the
     summits' hold in the agent); the forecast's items wait on `read_targets` (`waits_of`, a
     `Whole` read one shared set); the page and `scenic status` show the planner's notes.
     `/work/tocome` gives job-to-job dependencies (`jobs`, `deps`, `after` per job) and the build
     page draws single-job arrows.
   - Declared reads that differ from §5's table, because the planner held them so and the keys
     read so: the peaks wait for all terrain-hi and terrain-lo (their key names every lo pack
     through the z8 digest), their own tile's candidates and the hiking-route ends; the items, the
     marks and the overlays also declare what their reads' keys read (trailends, summits,
     heritage-sites, the spacing and registers inputs) so none runs on a read about to change;
     bldprep reads an input (the declared release), not `Whole bld-fetch`: it runs beside the
     fetch, as the planner had it; depth-root reads `Whole depth` as well as `Whole depth-lo`; a
     pole's root reads its own pole's blocks (`Targets`), not every block; pack and the catalogue
     declare nothing yet (their reads are the plan's).
   - Decision 2 (the terrain root a waited read) is not in step 1: the root is made in the
     publishing chain, behind the map tiles and the low zooms' cadence, so slope waiting for it
     would wait for that chain. It lands with scheme 2, the root moved to the terrain stage.
   - `steps::key` (`KEY_SCHEME = 1`) gave each step's key through the `*_key` functions and
     target lists the planner used, not lines assembled from the reads (they carried no
     identities yet); the steps keyed on the whole plan's state (pack, lo, prune, roadunits,
     stations, ferries, the catalogue, lakefill, names-todo) weren't derived.
   - Cut: `build::waits`, `build::wait_note`, the hand-written wait strings, `steps::Reads`,
     `steps::reads_of`, `Step::after`, `TerrainWork::is_left`'s step match, `Plan::terrain_built`,
     `forecast::chain_deps`, `UNITS_NEED`, the `reads_terrain` group, the held-chain guard.
2. **Merge task-832-keys onto it. Done (task #879).** #832's coverage cut (`build::piece_coverage`:
   the z9–12 tiles a terrain z6 tile makes near the coverage, digested) is the `coverage:piece`
   input, declared by terrain-hi, terrain-deep and pyramids-fill, and the elevation covers read
   their outlines (`dem::declared::cover_read`). It changes every terrain-hi, terrain-deep and
   pyramids-fill key, so it ships only with step 3.
3. **Flip to scheme 2 with #862. Built (task #879; not published: its publish starts #862's
   rebuild).** `KEY_SCHEME = 2`, and a `rekey` scheme (agent/rekey.rs, docs/building.md §2): a record current
   under scheme 1 is recorded under its scheme-2 key when its scheme-2 lines name no input the
   scheme-1 key didn't pin. That's every step outside the terrain family. The terrain family's
   records go, and they are #862's planned rebuild. The cascade then follows by content names: a
   unit whose terrain tiles' XXH3 didn't change keeps its key. So the rebuild is terrain and slope,
   plus what genuinely reads changed tiles.

   Built in parts (task #879), the flip last:
   - **3a (built): the derivation beside scheme 1.** `Read::Targets` and `Read::Whole` carry a
     `Grain` (`Outputs`, or `Read(f)`: what the run reads inside them), `Read::Input` an identity
     function (`Ident`, in agent/idents.rs, the runs' own code), and `Step::versions` the named
     code versions. `steps::key_lines` assembles §4's lines from them (an output by
     `steps::output_names`, a whole step's by its write-set, a mid by its `Read::Mids` identity),
     `steps::derived_key` hashes them, `steps::none_key` gives a "none" target's. A step with no
     versions declared isn't derived yet.
     Tests: `a_key_names_exactly_its_reads` and `scheme_2_pins_no_input_scheme_1_didnt` (each manifest
     entry and input changed alone moves a target's scheme-2 key only when it moves its scheme-1
     key: a scheme-2 key may pin less, never more), both over every derived step. It holds for every derived step, the terrain family
     too: scheme 1 as this code makes it already keys on #832's coverage cut (step 2), so no
     step's scheme-2 lines name an input its scheme-1 key doesn't pin, and which records go at
     the switch needs no list: those scheme 1 itself no longer finds current (§6 step 3d).
   - **3b (built): every step keyed from a step's own inputs declared so**: the pass's world steps,
     heritage-sites, the terrain and slope family, tree cover, depth, the polar caps, the roads,
     the landmarks, the 3D buildings, rail, timetables-todo, and from the publishing chain the road
     index, rail stops and ferries (their keys moved out of `publish_chain` into
     `build::roadunits_key` and `transit_key`). Both tests pass over six fixture builds (the
     plans run to the end; with a water source; with a pyramid read tile by tile; with a fine
     dataset past the base's zoom; at a pole; with depth sources), and every derived step is keyed
     in one of them. What it changed:
     - **Readiness passes waits on.** A target waits while a target it reads waits (`held`), and a
       step read whole while anything a target of it reads is left (`whole_left`), so a step
       declares only what its run reads. Step 1's waits standing for a read's own reads went (the
       peaks' for the trailends and the spacing, the items', the marks' and the overlays' for the
       chain's earlier steps, summits' and peaks' for all terrain-hi, depth-root's for depth).
       Only a mid made again for a target whose packs stay passes nothing on (`held` reads
       through a view where `mid_left` is `left`), and a `Grain::Tiles` read (`passes_on`)
       neither: the tiles it names may not change when what it reads is made again (a unit
       doesn't wait for the terrain-hi a terrain-lo assembly it reads tiles of waits for).
     - `Grain::Tiles(f)` beside `Grain::Read(f)`: the terrain tiles a slope tile or a z6 tile's
       roads read by XXH3 (`build::slope_tile_lines`, `build::unit_terrain`). A `Read::Mids` names
       its mids by an `Ident` (those of the z6 tiles near the coverage, as the assembly takes them).
     - Reads whose lines are another read's (`idents::nothing`): the overlays wait for the marks'
       reads without reading what they name (the dots are an input); a z6 tile's roads wait for
       the reaches, which they read through `ways`; a slope tile's terrain-lo and -hi tiles are
       named once, on one read.
     - Still keyed on the whole plan's state, so not derived: pack(T) and lo (their

       owners, by every base pack's reach), prune (no key), the catalogue (what's ready to list),
       lakefill (its source's pin, the plan's) and names-todo (the map's sequence served). The
       pass's own and upkeep's (osm-pass, pass-sets, regions-extract, buildings, inputs, gc, redo,
       backup) are keyed in agent::plan, outside this derivation.
   - **3c (built): decision 2.** terrain-root is the terrain stage's (its row and the checklist's
     terrain line): planned in `terrain_work` once its readiness (it reads every terrain-lo,
     whole) says so, first of the regions-done slope's work; left while stale. slope-lo and
     slope-root read it whole (`Grain::Read`: its content) and wait for it. slope-hi and slope-deep
     read of it only the root tiles (z0–2) their pyramid resolves to (`slope_reads` takes a
     missing tile's nearest ancestor: an edge neighbour's past a z3 tile with no lo pack, beyond the
     coverage), by XXH3 in their slope tiles' lines; that is their `root-tiles` input
     (`idents::root_tiles`), and only the targets reading one (`TerrainTargets::reads_root`) wait,
     through it (`Left::unknown_for`), while the root is stale. Every other slope tile reads
     nothing of the root and doesn't wait for it, so a region's slope still comes with it while
     another region's terrain is left (#909's per-region order). The publishing chain keeps the
     slope's root alone (`root_held`).
   - **A z6 tile's roads keyed on the terrain tiles they read (built).** Scheme 1 named the hi
     and deep packs over a z6 tile's box whole, so a terrain pack made again re-keyed every z6 tile
     reading any of it, however little of it changed. Its roads' heights read, of the terrain, the
     tiles staged over its box to z12 (`unit_terrain_tiles`, already in its key by XXH3) and the
     deep tiles under its ways' points and their neighbours within a pixel
     (crate::road_terrain::deep_tiles); its key now names those deep tiles by XXH3
     (`build::unit_deep_tiles`: the deep packs' tiles over its box that, grown by a pixel, meet
     where its ways lie, `reached_of`) and no pack whole. Narrower than scheme 1, which the rekey
     allows (`scheme_2_pins_no_input_scheme_1_didnt` checks only that direction).
   - **3d (built): the rekey, and what it would do now.** `rekey::scheme_2` records each record
     current under scheme 1 (its scheme-1 key for its target, "none" ones too) under its scheme-2
     key; one scheme 1 doesn't find current is kept as it is, and built again; one whose key can't
     be worked out now, or no target any more, is kept too. It isn't called until the flip (3e).
     `scenic-build rekey-check` says what it would do to the records now, reading only. Test:
     `rekey::tests::scheme_2_keeps_what_reads_nothing_new`.
     Measured on the NAS's records (31 Oct, pass 2026-09-28, read only, 16 min, 3.4 GB on the M1):
     every step outside terrain re-keys what scheme 1 finds current. Built again: terrain-water's one
     run (its water wants name #832's cut), the 20 pyramids-fill z6 tiles (#832), and every
     terrain-hi (2,326) and terrain-deep (10,258) target, whose keys can't be worked out until that
     terrain-water run has digested their water under the new cut (`unknown` there now) and then
     name #832's cut: #862's rebuild. Every other record counted "rebuilt" (peaks 1,306, slope-hi
     1,090, depth 416, trees 193, bldtiles 213, unit 85, the 34 slope-lo, 10 terrain-lo, the roots,
     summits, marks, overlays, …) is stale under scheme 1 as main makes it too: those steps' scheme-1
     code is main's (step 2 changed only the terrain's coverage cut), so the live build has them
     to make either way. What the terrain rebuild then re-keys follows by content names: an
     assembly's mids, a slope tile's terrain tiles and a z6 tile's roads' staged tiles by XXH3,
     those reading the hi and deep packs whole (a z6 tile's roads' heights) by the packs' content.
   - **3e (built): the flip.** `KEY_SCHEME = 2`. Every enumeration of targets (terrain and slope's
     `terrain_slope_targets`, `pyramids_targets`, `trees_targets`, `depth_targets`,
     `polar_targets`, `bld_targets`, `unit_keys`, `pois_keys`, `peaks_keys`, the world steps'
     `*_work` and `*_key`) takes a `build::KeyCtx` (the coverage, the pass, the manifest, the
     planner's inputs, the reaches, the terrain packs) and keys each target through one helper,
     `build::key_now`: a "none" target's `steps::none_key`, any other's `steps::derived_key`
     (`UNKNOWN` while it can't be worked out). They still work out the scheme-1 key first (which
     targets, which are "none", which unknown come with it) under `KeyCtx::scheme` 1, which is
     what the rekey's adapter reads: `rekey::scheme1::keys` (`build::step_keys` under scheme 1)
     and `rekey::scheme1::is_none_key`. `build::is_none_key` knows either scheme's.
     - Every reader of the records re-keys them with the same context (`rekey::as_read`, a
       `KeyCtx`: the plan, the checklist, the regions' counts, `--expect-same` (its step's alone,
       `as_read_of`), `scenic-build rekey-check`); `agent::key_inputs` is the planner's inputs they
       all read (the checklist and the regions' counts now read the feeds and spacing too). The
       pass's worldwide steps (`build::PASS_STEPS`: outlines, regions-land, trailends, reach,
       labels, spoken, water, summits) are planned before there's a coverage, so they're keyed in
       a `build::PassCtx` (no coverage, reaches, terrain packs or inputs: none of their keys reads
       them, `the_pass_steps_read_no_coverage`) and their records re-keyed so
       (`rekey::pass_as_read`), the reach's alone in `current_reach`. timetables-todo reads the
       coverage: it's planned after the plan's re-keyed records.
     - `rekey::scheme_2` keys a step's records under scheme 1 only while one isn't its key now
       (`build::step_keys`, the planner's keys; `Scheme2Step::current` counts those that are). Each
       reader re-keys in memory what isn't saved yet (step 4 saves it).
     - Not derived (`steps::derived`: no versions declared): pack(T) and lo (their owners, by
       every base pack's reach: `pack_lo_targets` over the whole plan; deriving them would take
       the owners as a per-plan memo in the `KeyCtx`), prune (no key), the catalogue (what's ready
       to list), lakefill (its source's pin, the plan's) and names-todo (the map's sequence
       served).
     - Tests: `build::tests::offered_keys_are_derived` (every key the plan offers is
       `steps::key`'s, a "none" target's `steps::none_key`), `the_pass_steps_read_no_coverage`;
       the tests that keyed targets by hand key them by `steps::key`, `none_key` or the
       planner's enumerations.
     - `build::step_keys` leaves out every target whose key can't be worked out now (`UNKNOWN`),
       so no record is re-keyed to it; terrain's and slope's targets are worked out once for
       their six steps per re-keying (`step_keys_kept`), and the 3D buildings' coverage shapes
       are kept by the coverage's version (`build::bld_tiles`): every target's key lines ask.
       Measured on the NAS's state (1 Nov, the M4, read only): a re-keying 30–70 s (13–15 min
       before these two), a checklist 17 s (18 min before).
     - Measured after the flip (`scenic-build rekey-check --unknown n`, 1 Nov, with main's
       planner run on the same records at once): built again, terrain-water 1, pyramids-fill 20,
       terrain-hi 1,789 and terrain-deep ~6,230 (their keys unknown until that terrain-water run,
       then #832's cut: #862's rebuild), 52 unit z6 tiles and 7 terrain-lo assemblies more than
       main (their keys name the elevation datasets meeting them, whose regions' cut #832 names
       by what's in the box, not by the outlines' files); every other record counted "rebuilt" is
       stale under main's planner too, target for target. "Unknown" outside terrain (slope-hi 537,
       trees 457, bldtiles 455, bldprep 332, unit 6, the z3 tiles' 3–6) and terrain-hi 537 and
       terrain-deep ~4,450 of the unknown are records of targets neither scheme (nor main) lists
       now: kept as they are, built by neither.
4. **Save the re-keyed records, then delete the adapter.** In two parts, the second merged only
   after the first has run on the live records.
   - **4a (built): the lead saves them once.** `rekey::scheme_2` lists what it re-keyed
     (`Scheme2Step::moved`: target, scheme-1 key, scheme-2 key) and drops the records of no target
     of the step's now (`build::step_keys_all`, the unknown ones too), whose
     outputs no manifest entry names (`steps::output_named`, the step's declared outputs:
     `Scheme2Step::dropped`; task #920 found such records, deep tiles' "none" drops and tiles left
     by the coverage, kept forever and read as unknown). The plan keeps what its reading did
     (`Planner::rekeyed`), and the lead runs a job per step with any (`rekey_jobs`, "redo rekey
     <step>"): `scenic-build rekey-check --step <step>` lists them, changing no file, and its done
     record (`rekey::record`) is a restore's, "<step> <target> <from>" with the scheme-2 key, or
     with no key for a dropped one. The merge (`build::Keys::record`) records each only while its
     record is still `from` (one built since keeps its own), a world step's in the lo keys; it's
     the one writer, the records' normal save. After it, every reader finds those records current
     and re-keys or drops nothing; scheme 1's keys (the reads' cost) are worked out for a step
     only while one of its records of a target with a key now isn't that key (until its stale
     records are built again). Measured on the live records (1 Nov, `rekey-check --saved`, the
     M1 over SMB): the reading's re-keying 399 s, read again after the save 214 s with nothing
     re-keyed or dropped; what's left is `scheme_2`'s own scheme-2 enumeration (the planner works
     the same keys out again), which goes with 4b, not with the save. Dropped then: terrain-deep
     4,449, terrain-hi 519, slope-hi 519, trees 457, bldtiles 455, terrain-lo, slope-lo and unit
     5-6 each, bldfield-lo and trees-lo 3; every dropped record's declared outputs are absent, and
     those of the step's targets are named (terrain-hi and slope-hi 1,788 of 1,789). Test: `rekey::tests::scheme_2_keeps_what_reads_nothing_new`
     (the save gives the keys the in-memory re-keying does; read again, nothing moves).
     Records of a step that isn't derived (pack, lo) aren't looked at, so none of theirs is dropped.
   - **4b (planned): delete the adapter** once `scenic-build rekey-check` shows nothing re-keyed
     on the live records: `rekey::scheme1`, `KeyCtx::scheme` and `scheme_1`, `rekey::scheme_2` and
     the readers' calls of it, and the scheme-1 key the enumerations work out first (`key_now`'s
     `k1`: which targets are "none" or unknown would come from the enumerations themselves) with
     every `*_key` function only it calls (§8); agent::restore's check ported to scheme-2 keys or
     deleted. A record scheme 1 finds stale or can't key stays stale either way.

## 7. Tests

- `every_wait_is_a_read` (built, in `build::tests`, with the planner's fixtures; every `run_all`
  plan is checked so): for each fixture state, every target the planner leaves
  out of its offers for readiness has `readiness != Yes`, and every target with `Yes` and a stale key
  is offered (the planner holds nothing undeclared). Replaces #867's held-chain guard.
- `forecast::tests::the_forecast_waits_for_what_the_planner_does` (built): for each fixture and on a
  `SCENIC_FORECAST_RECORD` record, each item's deps (expanded through its groups) equal
  `read_targets` filtered to items in the plan, and the forecast places none before what it waits
  on ends. Planned: replaying the forecast's first grants through the planner, each offered at
  the time the forecast places it. Supersedes
  `a_replay_never_places_a_target_the_live_plan_holds_back`.
- `steps::tests::a_key_names_exactly_its_reads`: for each step, changing each declared input (one at
  a time, in a fixture manifest) changes the key; changing any undeclared logical file doesn't.
  Replaces `declared_reads_cover_the_keys` (which checked one direction for six steps).
- `steps::tests::a_run_reads_only_what_it_declares`: per-target steps run on fixtures under the
  store's read log; each logical file read is named by a declared read. (This is the direction
  nothing checks today.)
- `offered_keys_are_derived` (built, in `build::tests`; every `run_all`
  plan and the depth, polar and 3D buildings chains' tests check each offered key against
  `steps::key`, a "none" target's against `steps::none_key`) and
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

## Decisions (PM, 30 Oct night)

1. At the flip to scheme 2, records outside the terrain family are rekeyed; only #862's terrain rebuild runs.
2. The terrain root is a waited read: it is built once all terrain-lo is done, and slope waits for it.
3. docs/steps.md §12 is generated from the declarations.
