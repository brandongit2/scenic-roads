# Consistency pass (task #840)

A read-only pass over the whole repo at `c8db398b` (29 Oct 2026). It covers the Rust crates, the map
app (`web/src`), the build page (`web/work`), the menu bar (`tools/status`), the dev, app and
taskboard tools, `producers/`, `dem/` and `docs/`. No code was changed; this file is the only output.
Each finding gives `file:line` evidence, why it matters, a proposed fix, and whether the fix moves
build keys, stored names or NAS formats (**rebuild** / **migrate** / no).

## 1. Summary

- **About 22 % of today's code is still from the project's first week** (28 Sep – 5 Oct), measured
  by `git blame -w`, skipping the rustfmt commit `e5a08b46`. The share is 72 % in `roadcore`, 45 % in
  `server`, 37 % in `store`, 59 % in `web/src`, and 10–20 % in the scheduler (`agent/`, `coord/`,
  `pool/`). The newer code is consistent with itself: the steps table, the records and the
  measured-not-typed rule. The drift sits at the seams where it meets the first week's code:
  - the server's own readers of store formats;
  - Web-Mercator, tile-key and Earth constants copied in every layer;
  - the one-region assumptions in `bin/extract.rs` and `view.rs`;
  - `roads/layer.ts` doubling as the rail layer;
  - the word `unit`.
- **Five real bugs or near-bugs:**
  - **F01:** the Rides list and the map score rail differently.
  - **F02:** every road in the western hemisphere outside eight north-eastern states, plus PR, French Guiana and Saint-Pierre-et-Miquelon, gets a Canadian shield class.
  - **F06:** `whole::copy` shares one temporary file between threads.
  - **F09:** the build page, the menu bar and the agent disagree on "out of touch": 6 min against 10.
  - **F10:** the menu bar sees only the lead's first two jobs.
- **Several near-bugs come from "one fact, several copies":**
  - the pack zoom split;
  - the checklist counts, done three times;
  - the Swift copies of Rust's wording;
  - about 20 per-step lists kept outside the steps table;
  - trees' drops left out of the "one mechanism for every step's drops" that commit `c55b6539` claims.
- **Naming:** `unit` has at least five meanings, `pack` five, `area` three (one against the
  glossary's own rule), and `lane`/`run`/`held`/`helper`/`pool`/`Source`/`stage` two or more each.
  Most renames are code-only. Where a name is stored, a serde alias or a read-old-as-new rule is needed.
- **Docs:** most of the steps table matches the code row for row (`docs/pipeline.md` §3 against
  `steps::TABLE`: 59 rows, no mismatch). The prose around it does not:
  - five docs state current behaviour wrongly (F26);
  - about a dozen cite symbols that no longer exist;
  - pool, pipeline, storage, workers, inputs, nas-access and surface narrate history, against the
    docs-match-code rule;
  - `docs/diagram/` is hand-kept and has drifted (PNG where the code writes WebP).
- **Fix plan (§6):** 19 independent tasks. Three of them move build keys and should share one
  planned rebuild of the units:
  - T2: road shields by country;
  - T3: scenic margin and eye height;
  - T10: one geometry and libm.

## 2. Method

1. **Module map and sizes:** about 350k lines outside the vendored MapLibre and GEOS.
   - The largest files are `agent/mod.rs` (19,240 lines), `coord/mod.rs` (10,736) and
     `agent/build.rs` (9,806).
   - In the web app the largest are `roads/layer.ts` (3,199) and `main.ts` (2,520).
2. **Git age map:**
   - When each file was added: `git log --diff-filter=A`.
   - The per-line age of all 683 source files: blame, skipping the reformat (§5).
3. **Six area passes:**
   - the scheduler (`agent`, `coord`, `pool`, `budget`, the CLI);
   - `store`, `server`, `roadcore`, `names`;
   - the pipeline's data steps and `producers/`;
   - the map app;
   - the build page, the menu bar and the tools;
   - docs against code, with a cross-language naming census (§3.1).
4. **Spot checks:** I re-read the cited lines of F01, F02, F04–F07, F09, F10, F14, F17, F19, the
   oldest-code claims in §5, and the `is_none_key` claim against commit `c55b6539`. The "Checked"
   column in §4 marks them; the rest are as the area passes cited them.

## 3. Categories

| # | Category | Findings |
|---|---|---|
| C1 | One fact or rule copied in several places (constants, thresholds, formats) | F09, F11, F12, F15, F18, F25, F31, F33, F37, F44, F49, F57, F58, F63 |
| C2 | Similar logic implemented several times | F07, F08, F16, F22, F23, F27, F29, F30, F32, F34, F35, F46, F52–F56, F60, F62 |
| C3 | Copies that have already drifted into different behaviour (bugs) | F01, F03, F04, F06, F09, F10, F14, F19, F20, F21 |
| C4 | Layers bypassing their own abstractions | F05, F07, F17, F22, F23, F38, F39, F40, F50 |
| C5 | One name, several meanings; one concept, several names | F41, F42, F43, F45, F64–F70 |
| C6 | The first week's code forcing old invariants outward | F02, F03, F04, F28, F29, F47, F48, §5 |
| C7 | Asymmetry between siblings (a feature for one step, layer, surface or endpoint only) | F14, F36, F51, F59, F61, F71 |
| C8 | Vestigial, legacy or dead code; one-caller abstractions | F47, F72, F73, F74, F75 |
| C9 | Docs contradicting code, dangling references, history narration, duplicated docs | F13, F26, F76–F84 |
| C10 | Mixed conventions (notation, units, errors, shell, CSS) | F28, F44, F85–F90 |
| C11 | Structure (god modules, layering inversions) | F24, F65, F91, F92 |

### 3.1 Naming census

These are whole-word, case-insensitive counts. They include generic English uses, so read them as rough.

| term | Rust | web/src | web/work | Swift | docs |
|---|---|---|---|---|---|
| step / stage / job | 5114 / 383 / 3320 | 238 / 9 / 9 | 347 / 108 / 121 | 23 / 4 / 103 | 2674 / 141 / 734 |
| target / unit / task | 2019 / 1996 / 1556 | 64 / 157 / 34 | 71 / 109 / 102 | 77 / 33 / 11 | 327 / 1018 / 323 |
| lane / run / part / phase | 153 / 3169 / 1122 / 828 | 1 / 158 / 94 / 15 | 24 / 161 / 41 / 18 | 0 / 37 / 5 / 1 | 67 / 440 / 152 / 62 |
| region / area / outline / coverage | 1070 / 1356 / 602 / 1622 | 109 / 217 / 124 / 93 | 48 / 27 / 27 / 0 | 5 / 0 / 0 / 0 | 259 / 141 / 337 / 231 |
| catalogue / catalog / manifest | 1524 / 318 / 966 | 152 / 15 / 0 | 54 / 10 / 5 | 1 / 0 / 0 | 268 / 309 / 85 |
| lead / coordinator / broker / member | 2830 / 867 / 242 / 1955 | 4 / 0 / 0 / 0 | 174 / 32 / 4 / 52 | 79 / 3 / 0 / 15 | 810 / 198 / 19 / 300 |
| worker / host / helper / peer | 1090 / 958 / 302 / 49 | 180 / 3 / 0 / 0 | 88 / 173 / 7 / 0 | 6 / 34 / 25 / 0 | 630 / 171 / 20 / 1 |
| pause / hold / held | 415 / 630 / 1991 | 10 / 61 / 55 | 35 / 8 / 95 | 36 / 15 / 68 | 90 / 83 / 255 |

- **`colour` and `color`:** our own identifiers say `colour` everywhere. The `color` hits are CSS,
  MapLibre or AppKit API names, so this pair needs no fix.
- **`catalog` and `catalogue`:** these follow a documented rule (`docs/formats.md:481-488`): stored
  and wire names say `catalog`, code and prose say `catalogue`. One file breaks the rule (F70).
- **History in Rust comments:** 521 "task #NNN" references (105 distinct), 101 "no longer", 65 "as
  before", 61 dated "NN Oct" and 61 "pool off" or "while it's on". The scheduler's comments also
  cite "pool.md §12 phase 4" 12 times; that section no longer exists.

## 4. Ranked findings

Ranked by impact: bugs and real drift risk first (High), then likely future drift and confusion
(Medium), then cosmetic (Low). Column key:

- **Moves:** what the fix touches.
  - **rebuild:** build keys or pack bytes.
  - **migrate:** a stored or wire name; needs an alias or a read-old-as-new rule.
  - **–:** nothing stored.
- **Task:** the fix task in §6; "done (#N)" once task #N has fixed it.

| ID | Sev | Finding | Main evidence | Moves | Task | Checked |
|---|---|---|---|---|---|---|
| F01 | High | The Rides list (server) and the map (client) score rail differently | `roadcore/src/scenic.rs:120-135` vs `web/src/rail.ts:96-110`, `roads/layer.ts:350-358` | – | T1 | yes |
| F02 | High | Shield classes treat everything west of 40° W as Québec or the north-eastern US | `pipeline/src/bin/extract.rs:617-637`, `rules.rs:29` | rebuild | T2 | yes |
| F03 | High | The staging margin is derived by hand and smaller than the reach of the ways a unit owns | `stage.rs:27-28`, `reach.rs:34`, `buildtiles.rs:140-152`, `unit.rs:606` | rebuild | T3 | partly |
| F04 | High | `view.rs` uses a fixed 1.5 m eye, so rail samples get a ground 1.3 m too high | `view.rs:280,290`, `bin/scenic-metrics.rs:131`, `roadcore/src/scenic.rs:20-22` | rebuild | T3 | yes |
| F05 | High | The server parses RDSECT itself: no checksums, a fixed version | `server/src/views.rs:173-305` vs `store/src/sect.rs:198-240,281,335` | – | T5, done (#850) | yes |
| F06 | High | `whole::copy` shares one temporary file between threads; `write` was fixed for this, `copy` was not | `store/src/whole.rs:159-163` vs `:403-419` | – | T4 | yes |
| F07 | High | NAS writers skip `write_whole_made`: no stale-EEXIST tolerance, a mkdir on every heartbeat | `agent/mod.rs:11594-11600,11405-11414`, `resume.rs:147`, `room.rs:761`, `build.rs:182`; `store/src/nasfs.rs:598,747-759` | – | T4 | yes |
| F08 | High | About 12 whole-file writers with about 10 temporary-name schemes; fixed `.tmp` names collide | `store/src/naming.rs:256,315`, `whole.rs:14`, `cachefile.rs:398`, `roadcore/src/lib.rs:194`, `heritagewd.rs:118`, `stage.rs:364`, `railfeeds.rs:869`, `timings.rs:746`, `bld/fetch.rs:352`, `server/src/regions.rs:338,444`, `names_live.rs:317`, `scache.rs:101,138` | – | T4 | partly |
| F09 | High | "Out of touch" is 10 min in the agent and the CLI, 6 min on the page and in the menu, and a literal 600 in one page filter | `pool/beat.rs:14`, `bin/scenic.rs:749`, `tools/status/main.swift:46`, `web/work/dash.js:37,162,204,844` | – | T6 | yes |
| F10 | High | The menu bar shows the lead's first two jobs only, and one per member | `tools/status/main.swift:131-132,693-705,812,841,877` | – | T6 | yes |
| F11 | High | Checklist counts and "finished" are worked out three times, each in different words | `agent/build.rs:4843,4865`, `main.swift:514,521-564`, `web/work/dash.js:90-117`, `sums.js` | – | T6 | |
| F12 | High | Swift re-implements room.rs, memguard and disk wording (already drifted) and writes five request files in its own unpinned JSON | `main.swift:1488,1513,1541-1565,2203-2394`; `room.rs:302-334,381`, `memguard.rs:63` | – | T6 | |
| F13 | High | `publish.sh` runs three crates' tests, not "everything"; some tests are run by nothing | `tools/app/publish.sh:166-169`, `tools/dev/check.sh:55`, `tools/check/*.test.mjs`, `testing.md:33-35` | – | T7 | yes |
| F14 | High | **Done (#849):** trees' and trees-lo's "none" targets are drops (`treepacks::none_piece_key`, `none_lo_key`, `is_none_key`, `run_drop`, `Plan::drops`). Was: they were not drops; `is_none_key` covered terrain and slope only | `agent/build.rs:1603-1610,2578,2723-2738,5557`, `treepacks.rs:123-125,349-355` | – | T8 | yes |
| F15 | High | **Partly done (#849):** `cost_version` is the row's `cost_v`, `PAGE_TASKS` went with the pages (#876), a task's disk need is its `steps::Task::disk`, `NAS_STEPS` lost `"elev"`; unit's row says 8600 MB (it no longer shows 1500). Open: `LEAD_ONLY`, `names_target`, `NAS_STEPS`, `QUIET_STEPS`, `HALO_STEPS`/`GUESS_FIRST` (needs.rs, #904's), `RESUMABLE`, and unit's row against `unit_peak`'s 3.7 GB floor. Was: about 20 per-step lists kept outside the steps table | `every.rs:30,50-57,69`, `coord/sched.rs:254`, `coord/mod.rs:137-160,166-173,179,3029`, `needs.rs:51,61`, `budget/nas.rs:314` | – | T8 | |
| F16 | High | **Done (#849):** `coord::CostKey` and `agent::jobs::JobId` write and read the two encodings (`cost_key`, `memguard::step_of_key`, `agent::step_of` through them); a bare key is unit's only when it is a z6 tile. Was: parsed by hand in about 14 places; a key with no space read as "unit" | `coord/mod.rs:189`, `memguard.rs:218`, `forecast.rs:3566,3663,3674`, `agent/mod.rs:318,339,620,8266` | – | T8 | |
| F17 | High | **Done (#849):** `Granted::Task` carries `task_kind` (an older lead's grant: `offload::task_kind`, the one reader of a spec); a leased task's job id is "<kind> <unit>" (#905). Was: task kinds guessed from the spec or from display text; anything unrecognised became "tail" (5 GB) | `coord/mod.rs:1062-1066`, `agent/mod.rs:538-544,555-588` | – | T8 | yes |
| F18 | High | The pack zoom split (root 0–2, lo 3–8, hi 9–14) is hard-coded in at least 5 places | `server/src/data.rs:504-511`, `terrain_pack.rs:351-352`, `agent/build.rs:734`, `web/src/packs.ts:27-32` | – | T9 | |
| F19 | High | Roads and stop dots fade toward a fixed `#0b0e13`, not the user's land colour | `web/src/config.ts:169`, `roads/layer.ts:403,411,2196`, `main.ts:1425-1429` | – | T11 | yes |
| F20 | High | Roads and rail call a bounding box "in view"; everything else uses the ground outline | `roads/stats.ts:147-166`, `roads/layer.ts:3055-3149` vs `terrainstats.ts:33-35`, `main.ts:1006` | – | T11 | |
| F21 | High | Two copies of the JSON `call<T>()` helper have drifted: NAS-away and abort handling differ | `web/src/regions.ts:59-72` vs `downloads.ts:84-94` | – | T11 | |
| F22 | High | About seven HTTP retry stacks bypass `fetch::Net`, so terrain fetches fail during an agent restart | `fetch.rs:832-990` vs `terrain_pack.rs:22-30,1099-1132`, `trees/squares.rs:236`, `bin/scenic-metrics.rs:812`, `leaftype/mod.rs:188`, `bld/fetch.rs:119`, `wikiask.rs:38` | – | T17 | |
| F23 | High | The canopy-square download and lock protocol is written twice, over raw `std::fs` | `bin/scenic-metrics.rs:268-418`, `trees/squares.rs:179-325` | – | T17 | |
| F24 | High | The CREDITS entries in `rules.rs` copy texts from the source declarations | `rules.rs:88-130` vs `producers/sources/*.toml` | – | T17 | |
| F25 | High | "Near the coverage, 20 km" is a bare `20.0` in about 15 places, and three box-growers disagree | `terrain_pack.rs:1689,2689,3261`, `terrain_deep.rs:245,1048`, `agent/build.rs:392,1234,…`; `hipack.rs:29` vs `stage.rs:176` vs `areafiles.rs:248` | rebuild (math) | T9/T10 | |
| F26 | High | Five docs state current behaviour wrongly | `regions.md:41-43`, `macs.md:8`, `inputs.md:1338`, `diagram/proposed.py:163-173`, `polar.md:3-7` | – | T16 | |
| F27 | Med | The tile-key decoding is copied inline about 15 times; `tile_zxy` lives in a different crate from `tile_key` | `roadcore/src/archive.rs:21`, `store/src/pack.rs:38`; `server/src/ways.rs:63`, `rawpack.rs:546,872,1284`, … | – | T9 | |
| F28 | Med | Tile notation is formatted and parsed by hand; `TileId` lives in `legacy.rs`, with a lax parser | 69 `format!("6/{}/{}")` against 28 `.slash()`; `legacy.rs:12-43` (34 importing files); `cli/progs.rs:128-137,247` | migrate (buildtiles path) | T9 | yes |
| F29 | Med | Web Mercator is written about 25 times across Rust and TS, with different polar clamps; `roadcore::merc`'s doc is wrong past 85° | `roadcore/src/lib.rs:272-279`, `vtgen.rs:61`, `marks.rs:460`, `watercov.rs:38`, … ; `web/src/marksview.ts:82`, `ferries.ts:113`, `depth.ts:350`, … | rebuild (if values move) | T9/T10/T11 | yes |
| F30 | Med | Earth radius and circumference are spelled 6+ ways; the viewshed copies the build's constants | `roadcore/src/lib.rs:261`, `server/src/viewshed.rs:117-144`, `view.rs:22,84`, `grid.rs:136`, `bld/mod.rs:49`; `web/src/roads/worker.ts:152` vs `dots.ts:189` | rebuild (if values move) | T9 | |
| F31 | Med | Std libm in output paths, against `det`'s one-libm rule | `road_terrain.rs:40`, `areafiles.rs:235`, `lakefill/run.rs:455,484`, `leaftype/warp.rs:85-164`, `inputs/regions/tiles.rs:402`, `terrain_deep.rs:620`; `store/src/pieces.rs:203` | rebuild | T10 | |
| F32 | Med | Point-in-polygon is written about 6 times, with different numerics and fill rules | `outlines.rs:984`, `bld/field.rs:700`, `areafiles.rs:305`, `inputs/regions/tiles.rs:2237`, `lakefill/mod.rs:135` | rebuild (bld-field) | T10 | |
| F33 | Med | The web's copies of roadcore's constants have no cross-check test | `web/src/config.ts:14-54`, `basemap.ts:151` vs `roadcore/src/lib.rs:52-96`, `tile.rs:25-41`, `shade.rs` | – | T9 | |
| F34 | Med | About 10 gzip helpers with different decoders and caps | `server/src/marks.rs:662-677`, `cache.rs:161`, `store/src/pieces.rs:115`, `names/src/mvt.rs:643-656`, `labels.rs:565`, … | rebuild (if levels change) | T5 (server side: done, #850)/T17 | |
| F35 | Med | The 2×2 terrain mean is copied, two copies inside `terrain_pack` alone | `terrain_pack.rs:247-258,2310-2320`, `polar.rs:304-330`, `roadcore/src/shade.rs:56` | – | T17 | |
| F36 | Med | Drop handling and mid readers differ between sibling layers | `terrain_pack.rs:1863-1877`, `slope_pack.rs:383-396,468-500,891`, `terrain_deep.rs:1221`, `treepacks.rs:302-330`, `depth/build.rs:625` | – | T17 | |
| F37 | Med | Pack logical names are formatted by hand in 8 modules; step versions are split between modules and their names collide | `terrain_pack.rs:348`, `slope_pack.rs:32`, `bld/mod.rs:99`, `treepacks.rs:43`; `agent/build.rs:33-55` vs `polar.rs:39-41`, `terrain_deep.rs:79` | – | T17 | |
| F38 | Med | The server bypasses `store::range::MmapFile` (its no-mmap-on-NAS guard) and re-derives the mirror's layout | `server/src/data.rs:324-328,650-669`, `views.rs:49-160` vs `store/src/range.rs:56-87`, `mirror.rs:56-58` | – | T5, done (#850) | |
| F39 | Med | The server reads files the pipeline owns with its own paths and parsers; status.json has two caches | `server/src/regions.rs:412,526,594,620`, `main.rs:191-259`, `data.rs:676-690` | – | T5, done (#850) | |
| F40 | Med | The catalogue writer takes `&Path`, so NAS publishes go uncounted through `unchecked_path` | `store/src/catalogue.rs:263,273,663,668,734`, `bin/scenic-build.rs:2262-2264` | – | T4 | |
| F41 | Med | `unit` means at least five things | step `unit`; inputs gate unit; `Line.unit` (a noun); task spec `unit`; z6 tiles in general (`server/src/ways.rs:55`, `catalogue.rs:58`) | migrate (wire) | T14 | |
| F42 | Med | `area` means a z8 tile in bld, Z3's serde name and the JS pack classes, against the glossary | `docs/pipeline.md:19`; `bld/task.rs:17-42`, `steps.rs:69-74`, `web/work/sums.js:199` | migrate (bld files: keep) | T14 | |
| F43 | Med | `pack` means five things; `cache` means three | `store::pack`, bundles, base `.sect`, `rawpack` (RDTILES), `server/src/cache.rs:35-60` | – | T14 | |
| F44 | Med | Durations, clock times and GB are formatted many ways, with binary and decimal GB mixed | `bin/scenic.rs:94,569,853`, `lead.rs:1101`, `dash.js:42,53,1476`, `pool.js:20`, `main.swift:649-667,1541`, `settings.js:20`, `room.rs:328`, `timings.rs:781` | – | T6 | |
| F45 | Med | Each surface works out the state sentence itself, and alerts match on prose | `main.swift:689-719`, `dash.js:178-285,1707`, `pool.js:29-30,79`, `bin/scenic.rs:726-957` | – | T6 | |
| F46 | Med | The rail weight migration, the look models and the "is scenic" test are each kept two or three ways | `scenic.ts:57,109-118`, `state.ts:264,336-376,792-815,895,1301,1507`, `main.ts:545`, `presets.ts:40` | migrate (URL, localStorage) | T12 | |
| F47 | Med | Rollout compatibility code is still live in three languages | `agent/mod.rs:800-817,1059`, `coord/sched.rs:825-828,1049`, `agent/pool.rs:1690-1709,2234`, `lead.rs:476,641`, `coord/mod.rs:2963-3130`, `main.swift:100-129`, `dash.js:177-181`, `install.sh:10-107`, `service.swift:36-56` | migrate (wire fields) | T15 | |
| F48 | Med | Legacy readers survive in the oldest code | `roadcore/src/elev.rs:1-4,62-75`, `grid.rs:221`, `store/src/mirror.rs:60,294`, `bin/extract.rs:16,861,1080`, `terrain_pack.rs:41-59`, `bin/tile.rs:22` | – | T15 | |
| F49 | Med | The program list is kept in four places, and they disagree | `tools/app/wasm.sh:23`, `publish.sh:247-252`, `install.sh:56` (no bldtile, terrainsub) | – | T13 | |
| F50 | Med | Switch files are read five ways with two defaults; hold-catalog is read raw on the loop thread | `brokers.rs:34`, `placement.rs:47`, `resume.rs:41`, `publish.rs:21`, `memguard.rs:48`, `budget/nas.rs:183`, `journal_gc.rs:107`, `agent/mod.rs:7629,8759` | – | T18 | |
| F51 | Med | The server's tile responders and caches differ per endpoint | `server/src/tiles.rs:23-28,70,84,99-140`, `depth.rs:43,78,96`, `terrain.rs:351`, `cache.rs:189`, `water.rs:366-444`, `pages.rs` | – | T19 | |
| F52 | Med | The web app has four retry/backoff schemes; only some loaders refetch when the NAS returns | `retry.ts:17-40`, `roads/layer.ts:1484`, `bldfield.ts:1284`, `marksview.ts:35,277`, `main.ts:2441` | – | T11 | |
| F53 | Med | Colour-stop interpolation is written 3 times (two give NaN on equal stops), hex parsing about 15 | `palettes.ts:60-70`, `terrain.ts:14-26`, `depthlook.ts:41-73`, … | – | T11 | |
| F54 | Med | Coast and depth duplicate a worker pool; worker error envelopes come in three shapes | `coast.ts:38-50`, `depth.ts:130-140`, `roads/types.ts:88`, `bldfield.ts` | – | T11 | |
| F55 | Med | The build page's modules each carry their own `h`/`el`/`plural`/`pad`/`remote`/token helpers | `dash.js:62,123`, `pool.js:7`, `settings.js:7,33-56`, `catalogues.js:37,198,210`, `live.js:37,101`, `chart.js:21`, `worldmap.js:71` | – | T13 | |
| F56 | Med | Shell lock and PID-identity code written 3 times; `pkill -f` against the exact-PID rule | `tools/app/lib.zsh:58-105`, `remote-tests.sh:55-75,95,246`; `install.sh:80`, `tools/status/sandbox.sh:124` | – | T13 | |
| F57 | Med | Disk and battery thresholds hard-coded on the page and in scripts, against "the owner's margin is the only floor" | `sweep-targets.sh:103-110`, `dash.js:213,216` vs `cond.rs:471` | – | T13 | |
| F58 | Med | Hinnant's civil-date conversion is written about 8 times | `nasread.rs:631`, `backup.rs:263`, `rail.rs:220`, `gtfs/mod.rs:222`, `pageviews.rs:51`, `bin/scenic-build.rs:2560`, `whsshapes.rs` | – | T18 | |
| F59 | Med | Region progress is computed four ways | `agent/build.rs:1146-1244,2611,2825`, `forecast.rs:927`, `agent/mod.rs:4637-4684`, `coord/mod.rs:4271-4290` | migrate (status `built`) | T18 | |
| F60 | Med | `/work/status` and `view_of` shape the same lease and task JSON differently; Debug output is sent as wire data | `coord/mod.rs:3260-3295,4110-4138`; route classes at `:4689-4712,4938-4993` | – | T18 | |
| F61 | Med | Controls exist on some surfaces only | `cli/scenic.rs:93-105`, `main.swift:779-791,2329`, `dash.js:1739`, `settings.js:138-175` | – | T6 | |
| F62 | Med | The Drives list re-implements the list base; its error text is stale | `web/src/ui/drives.ts:9,28-48,92` vs `ui/rides.ts:11,21-45` | – | T11 | |
| F63 | Med | The `pairs.bin` record and the program progress bars are not shared definitions | `rail.rs:59-61`, `gtfs/railcount.rs:585`, `bin/railfreq.rs:130-139`; `lib.rs:117-141` (indicatif) vs `jobs::report` | – | T17 | |
| F64 | Med | `lane`, `run`, `progress` and `heartbeat` each mean several things | `forecast.rs:170,743,800,894`, `fcarchive.rs:59`, `steps.rs:88`; `resume.rs:48`, `jobs.rs:775`; `agent::pool::Heartbeat` vs `coord::Beat` | migrate (`lanes` in archive) | T14 | |
| F65 | Med | Layering inversion: `coord`, `pool` and `needs` depend on `agent` | 142 `crate::agent::` uses in `coord/mod.rs`, 32 in `sched.rs`; `needs.rs:36`; `pool/journal.rs:97`; `pool/driver.rs:175` | – | T18 | |
| F66 | Med | member/helper/worker and pause/hold/held overlap | `Status.helpers` (`agent/mod.rs:869`), `helper.json`, `dash.js:822,846`; `hold-request.json` vs `scenic pause --here`; `held_mb`, `edit_held`, `catalog-held` | migrate (keep paths) | T14 | |
| F67 | Med | Module and type names collide | `agent/brokers.rs` vs `coord/brokers.rs`; `agent/pool.rs` vs `pool/`; 5 `Source` types; `inview::Region` vs `inputs::regions::Region`; `stage.rs` (staging) vs stages | – | T14 | |
| F68 | Med | Web channel keys and labels drift | `scenic.ts:40,81` (`relief` twice), `rail.ts:37-48,78-89`, `ferry.ts:13`, `ui/ferry.ts:12`, `config.ts:17` (`FERRY`) | migrate (presets, URL) | T12 | |
| F69 | Med | The panel's name drifts, and UI text points to sections that don't exist | `index.html:23-26` (#colour), `ui/drives.ts:57`, `ui/stats.ts:165`, `ui/rides.ts:2,96`, `ui/sights.ts:38`; README:12,15 | – | T12 | |
| F70 | Med | The page serves `catalogs.js` as an alias of `catalogues.js` | `coord/mod.rs:4518`, `dash.js:28`, `touch-test.mjs:106`, `formats.md:486` | – | T14 | |
| F71 | Med | The depth layer hand-rolls its scale and its in-view test; it has no ScaleControls | `web/src/depth.ts:345-396`, `ui/depth.ts` | migrate (URL `dp=`) | T11 | |
| F72 | Low | Exports nobody imports (web) | `config.ts:4,52`, `roads/stats.ts:241`, `camera3d.ts:134`, `rail.ts:13`, `ferry.ts:94`, `hosts.ts:11,31`, `labellook.ts:60` | – | T15 | |
| F73 | Low | Server JSON fields the app never reads | `server/src/ways.rs:116-118,185` (`idx` "for the app's older field name"), Meta fields at `api.ts:57-66` | migrate (wire) | T15 | |
| F74 | Low | Vestigial build-page and tool bits | `--st-fine` (`web/work/index.html:31,40`), `tools/taskboard/migrate.mjs`, `docs/mockups/today/`, `health-due.sh` | – | T15 | |
| F75 | Low | Twelve hand-written fake `Nas` impls in tests | `pool/records.rs:871,1037,1200,1261`, `pool/driver.rs:2103,…`, `pool/journal.rs:443` | – | T18 | |
| F76 | Med | Docs and comments narrate history | `pipeline.md:261,303,314`, `pool.md:743,801,1249,1343,1955-1969`, `storage.md:57`, `workers.md:174`, `nas-access.md` §1/§3, `surface.md:66-77` | – | T16 | |
| F77 | Med | Dangling references in docs and comments | `pipeline.md:447` `STAGE_COLOURS`, `inputs.md:1472`, `formats.md:936` `dem_tiles`, 12 "pool.md §12 phase 4" comments, `web/src/scenic.ts:2` `drives.rs`, `store/src/lib.rs:1` | – | T16 | yes (drives.rs) |
| F78 | Med | `pipeline.md` §2 is stale and claims "nothing about a step is kept anywhere else" | `pipeline.md:28,35,38,135,348`; `pool.md:566-575` repeats it | – | T16 | |
| F79 | Med | `docs/diagram/` is entirely hand-kept and has drifted | `diagram/proposed.py:4,163-173`, `diagram/workers.py:66-70` | – | T16 | |
| F80 | Med | `testing.md` contradicts the scripts | `testing.md:117,133,153` vs `touch-test.mjs:70`, `cli/scenic.rs:266`, `health.sh:2-4` | – | T16 | |
| F81 | Med | `phase-times.md` is a dated snapshot that other docs cite as measured | `phase-times.md:1-3`, `sizes.md:21`, `diagram/proposed.py:4` | – | T16 | |
| F82 | Med | Decisions and risks live in docs, though README says they are on the board; the README Data table lacks the depth sources | `README.md:58,68-`, `pool.md` §12 and §14, `polar.md` §9, `buildings3d.md:480-497` | – | T16 | |
| F83 | Low | Docs duplicate each other | `workers.md:183-200` vs `pipeline.md` §5; `pool.md` §7.2 vs `pipeline.md` §2; `steps.rs:1-35` | – | T16 | |
| F84 | Low | Small doc drift | README "1.7 m" vs "1.5 m" (`:37,116`), "both Macs" (`surface.md:4,76`, `storage.md:24`), the iPad budget (`buildings3d.md:335-347`), numbering gaps, "worker page" | – | T16 | |
| F85 | Low | Mixed error conventions | `server/src/profiles.rs:249-257` (`{e:?}` strings), `webpenc` returns `String` errors, 400 `lock().unwrap()` against 69 poison-tolerant locks | – | T18 | |
| F86 | Low | CLI flags are declared several ways | `cli/scenic.rs:82,290,319,346-350,444-448` (FOLDER vs DIR; `--root` repeated) | – | T18 | |
| F87 | Low | Shell conventions are mixed | shebangs (zsh, bash, sh, `env bash`), strict mode in some scripts only, launchd plists made four ways | – | T13 | |
| F88 | Low | CSS is split between the page and its file; machine colours are hex picked by position | `web/work/index.html:21-120`, `dash.js:120,166,845,853,1675,1798`; help style ×4 in `web/src/ui` | – | T13 | |
| F89 | Low | Icons have no generator | `web/work/icons/*.png`, `web/public/icons/*.png`, `tools/taskboard/public/*.png` | – | T13 | |
| F90 | Low | Three hostname functions; pids taken two ways; small duplicate helpers | `store/src/sys.rs` (`hostname`/`host`/`host_name`), `bld/fetch.rs:358`, four `now()` in `server/src`, `whole.rs:28-37` | – | T4 | |
| F91 | Med | God modules | `agent/mod.rs` (19k lines, `impl Agent` spans 1704–11065); `web/src/main.ts:87-2474` is one closure; `RoadLayer` is also the rail layer | – | T18/T11 | |
| F92 | Low | Python leftovers and misplaced folders | `dem/` holds heritage, inputs and timings Python; NAS paths hard-coded in `producers/regions/gen.py:5,16` and `producers/depth/smalllakes-run.sh:25-27`; about 20 "as numpy did" comments; producer UA literals ×8 | – | T17 | |

### 4.1 Why each matters

**High**

- **F01:** a high viaduct ranks like a 2 m culvert in the Rides list, while the map and the hover
  bars rank it near the top. README:42's "the same in the shader and the server" holds for drives only.
- **F02:**
  - California's "CA 1", any route in PR, French Guiana or Saint-Pierre-et-Miquelon, and every state
    route outside `NY VT NH ME MA CT RI PA NJ` get `CA_PROVINCIAL`/`CA_REGIONAL`/`CA_AUTOROUTE`.
  - The shields and the Map presets' network colours are wrong across most of the US.
- **F03:** samples 15–20 km or more past a z6 tile get viewsheds over terrain and canopy that were
  never staged. Buildings already stage from `reach::near_box`; terrain doesn't.
- **F04:**
  - TPI, relief and the eye floor of every rail sample are off by 1.3 m.
  - `scenic-metrics.rs:1121` uses `EYE_M` for the same quantity, so the two programs disagree.
- **F05:**
  - A store version bump or layout change silently breaks the server.
  - Damaged NAS bytes are served unchecked, though `sect.rs`'s header promises every section read
    from the NAS is checked.
- **F06:** a second thread's `File::create` truncates the first one's temporary file after its
  length check, so a short file is renamed into place. Reached through `copy_to_local` and
  `copy_from_local` (`answers.rs:254,285`, `rawpack.rs:567`, `room.rs:1322`).
- **F07:**
  - Heartbeat, hold, progress and key writes can fail on SMB's stale `AlreadyExists`, which
    `make_parent` is there to accept.
  - Every heartbeat pays an extra mkdir round-trip.
  - `write_replace_nas`'s comment ("its folder made once a process") is false.
- **F08:**
  - Fixed names collide: `server/src/regions.rs`'s `json.tmp` is shared by concurrent requests, and
    `scache.rs:101`'s `with_extension("tmp")` makes `canopy.keys` and `view.keys` write
    `canopy.tmp` and `view.tmp`.
  - Some writers fsync and some don't; SMB-busy handling differs per site.
  - One Mac writes temporary files under two host names: `naming` uses gethostname, `whole` uses
    LocalHostName.
- **F09:** a Mac quiet for 7 minutes vanishes from the menu and shows "out of touch" on the page,
  while `scenic status` and the pool's own decisions still count it.
- **F10:** the lead ramps to 6 slots (`bin/scenic.rs:516`). Jobs in slot 3 and up are invisible,
  stages show the wrong under-way mark, and "Paused" can show while slot 3 runs.
- **F11:** `pipeline.md` §2 says all surfaces print `{done} of {total} {noun}s`, and §4 says they say
  it the same way. Today one says "156 of 284 z6 tiles", one adds thousands separators, and one
  says "156 z6 tiles done · 128 left".
- **F12:**
  - A field renamed on the Rust side silently breaks a menu control, because only 2 of the 7
    request files have shared fixtures.
  - The menu also skips the validation the page gets through `/work/settings`.
  - The `by` signature mixes ComputerName and LocalHostName.
- **F13:**
  - `crates/server` (59 tests plus `tests/cli.rs`), `det`, `roadcore`, `webpenc`, `web/test`, the
    taskboard, the Swift self-test and the shell tests never gate a publish.
  - `tools/check/{autofit,persp,twofinger,raildraw}.test.mjs` run nowhere.
  - A new web test file is silently skipped by `check.sh`'s hand list.
- **F14:**
  - Trees' and trees-lo's drops are offered and sized as builds and feed the build model.
  - The forecast doesn't mark them as drops.
  - Their run bypasses `run_drop`, so `needs::DROPS` never learns them.
  - Commit `c55b6539` ("every step's none targets are drops through one mechanism") lists six steps
    and leaves these two out.
- **F15:**
  - A new step can silently miss a list: lead-only, NAS-heavy, the cost version, page tasks,
    re-measured.
  - `NAS_STEPS` already names `"elev"`, which is a program, not a step.
  - The page shows unit's row `mem_mb` (1500) while the coordinator's real first guess is
    `unit_peak` ≥ 3700.
  - The docs promise "nothing about a step is kept anywhere else".
- **F16:** a new step whose cost key has no space is silently read as `unit`, so it gets unit's
  memory floor and model.
- **F17:**
  - `coord::Granted::Task` drops the `kind` that `task::Offer` carries, so the agent guesses it back.
  - A new task kind becomes "tail": a hand-typed 5 GB disk need and the wrong cost key.
- **F18:** change the split in one place and the server looks up the wrong pack: a 204 or a stale tile.
- **F19:** with a custom land colour, faded and dimmed roads, casings and stop dots tint toward the
  wrong background (`bldfield.ts:1820` already uses the real one).
- **F20:** in tilted or globe views the roads and rail histograms, auto-fit, Equalise and the Stats
  card count ground that the Drives list and the terrain tint leave out. `main.ts:1006` itself
  notes the box takes in "Toronto from northern British Columbia".
- **F21:** in `downloads.ts`, a NAS-away 503 shows as a raw server string or "HTTP 503".
- **F22:**
  - `Net` waits for the agent's proxy to come back, honours 429, drains and cancels.
  - The other stacks don't, so an agent restart after a publish fails terrain fetches that `Net`
    would have waited out.
  - Each stack also has its own 404/403 rule.
- **F23:** two lock protocols over one NAS store:
  - stale-lock times of 1800 s and 7200 s;
  - only one handles a lock made by a Mac whose clock runs ahead;
  - both use raw `std::fs`, with no `NasPath` accounting.
- **F24:** an edit to a declaration's credit or licence leaves the map's © dialog showing the old text.
- **F25:**
  - A change to the margin in one place leaves the hi tiles, the deep levels and the plan out of step.
  - The growers use km/111.32 or km/110.574, cos at different latitudes, and clamps of 85.0 or 85.05.
- **F26:**
  - `regions.md`'s densification spacing (8/15 m) is wrong: it is the terrain's pixel, with a 4 m floor.
  - `macs.md`'s `ProcessType Interactive` is wrong: `build-service.plist` sets Standard.
  - `inputs.md`'s rekey symbols `dem_rules_meeting` and `pre_elevation_unit_key` exist nowhere.
  - The diagram draws PNG packs (they are WebP) and GLO-30 "north of 59.5°" (it ranks first worldwide).
  - `polar.md` says nothing polar is registered, but three TABLE rows are live.

**Medium and Low**

- **F27 and F28:** each inline copy of the tile-key decode and of the tile notation is a place a
  layout change can miss. The lax `TileId::parse` hides malformed names.
- **F29 and F30:**
  - The copies disagree at the poles (clamps of ±85, ±85.05, ±85.06 and ±89.2) and in the metres
    per Mercator unit (road lengths about 0.1 % apart in the web app).
  - The viewshed's own bilinear groups its arithmetic differently from `grid::terrain_bilinear`.
- **F31:** `det` exists because a Mac and wasm gave different eye heights in the pilot. Std libm left
  in output paths breaks byte reproducibility, the condition for sharing work with browsers.
- **F32:** points on an edge can fall inside for coverage and outside for outlines or bld-field.
- **F33:** a class or flag added in roadcore leaves the web drawing the wrong colours, with no failing test.
- **F34:** marks' gunzip has no size cap, unlike `names::mvt` and `pmtiles`.
- **F35:** "terrain: plain 2×2 mean" (27 Oct) has no single function. Two copies inside one file can drift.
- **F36:**
  - Only slope's mid reads survive SMB reads stuck in the kernel.
  - Some drops save the manifest and some don't, so a crash between leaves different states.
- **F37:**
  - The stored names `layers/<layer>/{hi,lo,root,deep}/z-x-y` are hand-formatted at about 15 sites.
  - `polar::TERRAIN_V` and `agent::build::TERRAIN_V` are two constants with one name.
  - Data writers import the scheduler to learn their own version.
- **F38:** the server mmaps without the `ensure_local` guard against SIGBUS on NAS files, and keeps
  its own copy of the mirror's folder layout.
- **F39:**
  - The server's literal paths break silently if the pipeline renames `@index.json` (a private
    constant) or the agent log names.
  - Two 30 s caches of one file can disagree.
- **F40:** `docs/nas-access.md` §2 promises every NAS method is counted, but catalogue publishes
  aren't, so the NAS budget undercounts them.
- **F41–F43, F64, F66, F67:**
  - `rg unit` cannot tell the five meanings apart, and `Line.unit` sits beside items of step `unit`.
  - "area" breaks the glossary's own rule (geography only).
  - Readers mistake `agent::brokers` for `coord::brokers` and the elevation "pool" for the Mac pool.
- **F44 and F45:**
  - One job reads "about 185 min left" in the CLI, "about 3 h 5 min left" in the menu and "3.1 h"
    on the page.
  - "N GB free" is binary while region sizes beside it are decimal.
  - Rewording a note silently breaks the alerts that regex on it.
- **F46 and F68:**
  - A saved rail preset without `freq` loads it at 0, while a link or saved state gets 0.3.
  - Two first-fit defaults ([1, 99] and [2, 98]).
  - `relief` is both a mode and a different component.
  - `main.ts:545` treats `map` as scenic and samples a metric that is always 0.
- **F47 and F48:** about 71 "an older app" branches in three languages, each a path no current
  process takes, to read and keep compiling.
- **F49:** `install.sh` doesn't chmod `bldtile` and `terrainsub`.
- **F50:** missing switch files mean off for some switches and on for others. The hold-catalog
  read bypasses the switches' deadline pool and their "keeps its last value" rule.
- **F51:**
  - A missing tile is 204 on some routes and 404 on others.
  - Only some routes parse comma lists in `If-None-Match`.
  - `plain` takes gzip as a route literal rather than the pack's own flag.
  - Four cache policies.
- **F52:** buildings, landmarks, depth and polar tiles wait out their timers after the NAS returns;
  roads and rails refetch at once.
- **F53:** `hexStops` and `ramp` give NaN on two equal stop positions; `depthColour` re-parses its
  stops on every call.
- **F54:** three error envelopes (`{type:'error'}`, `{id, error}`, `{type:'failed', key}`) for one
  idea, and two hand-copied two-worker pools.
- **F55:**
  - Three `ask`/`remote` copies each map timeouts their own way.
  - `chart.js`'s fallback `--bad` is `#e5534b` while `index.html` defines `#e0716a`.
- **F56:** `remote-tests.sh` squeezes `lstart` and `lib.zsh` doesn't, so owner ids differ. `pkill -f`
  breaks the exact-PID rule.
- **F57:** the page warns on battery below 40 % while the per-Mac floor defaults to 30 %. The
  "20 GB" in `sweep-targets.sh` is a hidden fixed reserve.
- **F58:** journal day folders and sample names are stored paths. A copy that differs at a leap
  year moves them.
- **F59:**
  - `RegionLeft` has hand-written per-step fields while `RegionFc` keys by step id.
  - `/work/regions?steps` is the only view not taken from the pageview.
- **F60:** two JSON shapes for one lease (`"paused"`/`for_s` against `"pause"`/`granted`). Debug
  output (`{:?}`) sent as wire data changes with any enum rename.
- **F61:** pausing one Mac and clearing caches can't be done from the page; "Try again" exists only there.
- **F62:** `LENGTHS` is defined twice. "Scenic analysis not available — run the scenic pipeline
  step." is shown for any failure, the NAS away included.
- **F63:** `pairs.bin`'s 21-byte layout is defined only in comments, its bit masks hand-copied by
  writer and reader. The unit step's progress can't come from its programs.
- **F65:** `coord` and `pool` cannot be read or tested without `agent`. The journal's stored day
  names come from `agent::backup`.
- **F69:** the UI tells users to go to "Colour → Scenic → Score" and "Layers → Stops & sights",
  sections that don't exist.
- **F70:** two URLs for one module, and `formats.md` lists the alias as a stored spelling.
- **F71:** depth is the only colour-scaled layer without histogram, Lock, Equalise or threshold, and
  its fit uses the box.
- **F72–F75:** dead weight for readers and the type checker. The twelve fake `Nas` impls each copy
  `Mem` to change one call.
- **F76–F84:**
  - The owner's rule: docs say only what is true now. History and stale claims mislead the next
    agent, who reads docs before code.
  - Duplicated sections (the build page described twice, the step fields listed three times) have
    already drifted apart.
- **F85, F86, F90:**
  - Debug-string errors lose the `IoError` classification that `retry::io_of` looks for.
  - Mixed poison handling means one panicking thread takes down the coordinator through
    `lock().unwrap()`.
- **F87–F89:** cosmetic, but every new script picks a convention at random.
- **F91:**
  - `agent/mod.rs` holds per-step memory models, task-kind guessing, status types and write helpers,
    none of which is the agent loop.
  - `main()` in `web/src/main.ts` is one closure, so nothing in it can be imported or tested.
- **F92:** `dem/` holds no DEM code, and is easily confused with `crates/pipeline/src/dem/`. The
  hard-coded NAS paths break on another mount.

## 5. The oldest code

Per-line age by blame, skipping the reformat commit:

| Area | Lines | Share from 28 Sep – 5 Oct |
|---|---|---|
| crates/roadcore | 2,071 | 72 % |
| crates/pipeline/peaks | 1,185 | 64 % |
| web/src | 37,412 | 59 % |
| crates/names | 4,899 | 52 % |
| crates/server | 16,733 | 45 % |
| crates/pipeline/bin | 14,131 | 41 % |
| crates/store | 14,047 | 37 % |
| crates/pipeline (top-level modules) | 72,683 | 29 % |
| coord / pool / agent | 16,058 / 12,327 / 61,503 | 21 % / 20 % / 10 % |
| inputs, bld, depth, budget, gtfs, lakefill, leaftype | — | 0 % (all newer) |

These files are almost unchanged since the first week (share of old lines, median line date):

- **Rust:**
  - `bin/extract.rs` 93 % (29 Sep)
  - `server/src/query.rs` 98 % (3 Oct)
  - `server/src/marks.rs` 97 %
  - `bin/railfreq.rs` 97 % (29 Sep)
  - `server/src/views.rs` 96 %
  - `pipeline/src/marks.rs` 96 %
  - `bin/scenic-metrics.rs` 91 %
  - `store/src/pack.rs` 95 %, `sect.rs` 99 %
  - `vtgen.rs`, `peaks.rs`, `areaflags.rs` 99 %
  - `view.rs` 93 % (28 Sep)
  - `buildings.rs` 96 %
  - `elev.rs` 84 %
  - roadcore's `scenic.rs`, `tile.rs`, `archive.rs` 96–99 %
- **Web:**
  - `roads/worker.ts`, `roads/stats.ts`, `scenic.ts`, `prefs.ts`, `dotlayout.ts`, `covers.ts`, `ui/stats.ts` 100 % (28–30 Sep)
  - `roads/layer.ts` 91 % (3,199 lines)
  - `main.ts` 75 % (2,520 lines)
  - `basemap.ts`, `overlays.ts`, `ui/layers.ts`, `camera3d.ts` 83–86 %

Where the oldest code forces old invariants or words onto newer code:

1. **One region, north-east America:** `bin/extract.rs:617-637` (28–29 Sep). It assumes any road
   west of 40° W is Canada or the north-eastern US. The regions have since grown to the whole US,
   PR, French Guiana and Saint-Pierre-et-Miquelon. `rules.rs:29`'s `NORTH_AMERICA` box encodes the
   same assumption. (F02)
2. **The road eye height:** `view.rs:280,290` (28 Sep) hard-codes 1.5 m. Rail came a day later with
   a 2.8 m eye (`roadcore/src/scenic.rs:20-22`) and inherits the road's ground in TPI, relief and
   the eye floor. (F04)
3. **Roads first in the renderer:** `roads/layer.ts` (Sep 28) became the rail layer too: rail
   shader modes 30–32, `railWeights`, `lineFreq`. Rail code is written in road names (RoadTile,
   `lineRoadLen`, `viewStatsGen(rails…)`). Its in-view box (F20), its fade colour `BG` from
   `config.ts` (F19) and its Mercator math (F29) predate the shared `viewArea` and the user land
   colour, and newer layers route around them.
4. **`roadcore::merc`'s "[0, 1)":** (28 Sep) clamps sin to ±0.9999, about ±89.2°. Since #668 roads
   run past 85°, and `grid::cell_of` inherits the doc's wrong range. The other copies of the math
   each picked their own clamp. (F29)
5. **`unit` as the word for a z6 tile:** the first-week cache `cache/scenic-units`, the step `unit`
   and `server/src/ways.rs:55-64` `unit_key`/`unit_str`. The word spread into `store::catalogue::units`
   (stored), `Line.unit` (now the noun "z6 tiles"), the task spec's `unit`, and the inputs gate's
   "unit". The glossary (`pipeline.md` §1) says "z6 tile", but every reader still meets "unit". (F41)
6. **`TileId` in `legacy.rs`:** written 2 Oct as the reader of the old z6 folders. It is now the
   main tile type of 34 modules. Its lax parser accepts both `/` and `-`, trailing junk and
   x ≥ 2^z, so newer callers add `.replace('-', "/")` and round-trip checks of their own. (F28)
7. **The server's own format readers:** `server/src/views.rs` (2 Oct, 96 % unchanged) parses RDSECT
   itself, written the same day as `store/src/sect.rs`. It never caught up with the store's version
   range and checksums (F05), nor with `range::MmapFile`'s rule against mmapping NAS files (F38).
8. **Old names kept for the old app:** `server/src/main.rs:778-801` serves view versions under
   "their old names" (`roads.tiles`, `rails.tiles`, …), and the web still reads those
   (`web/src/main.ts:2284`, `stations.ts:53`, `ferries.ts:152`). `ways.rs:116` keeps `idx` "for the
   app's older field name", which nothing reads. (F73)
9. **Signed elevation:** `roadcore/src/elev.rs:62-75` still reads signed `final.i16` from before
   3 Oct, and `bin/tile.rs` still deletes it. `UNIT_V ≥ 2` has made everything `u16` since. (F48)
10. **indicatif progress bars:** `lib.rs:117-141`, used by the first-week programs (`view.rs`,
    `elev.rs`, `buildings.rs`, `tiling.rs`, `extract.rs`, `scenic-metrics.rs`). The agent can't read
    them, so the unit step reports stage-weighted progress only, while newer steps use
    `jobs::report`. (F63)
11. **A stale header:** `web/src/scenic.ts:2` (28 Sep) says the normalisation "must match
    crates/server/src/drives.rs", which doesn't exist. `rail.ts:4` points only at `layer.ts`. With
    no live pointer to `roadcore::scenic`, the rail factors drifted. (F01)

## 6. Fix plan

Each task is independent and reviewable on its own. The order is a suggestion: correctness first,
then the single-source tasks that stop further drift, then naming and docs.

**Planned rebuild:** T2, T3 and T10 move build keys or pack bytes. Do them together in one units
rebuild, after the code-only consolidation (T9) has named the shared constants.

| Task | Scope | Findings | Files (main) | Risk | Moves |
|---|---|---|---|---|---|
| **T1** Rail score parity | Make `roadcore::scenic::ride_components` and the TS/GLSL rail factors agree: viaduct from the deck height above the ground, one grade source, one altitude base. Add a parity fixture read by TS and Rust, as `web/test/inview-fixtures.json` is. Fix the `scenic.ts`/`rail.ts` headers. | F01 | `roadcore/src/scenic.rs`, `web/src/rail.ts`, `roads/layer.ts`, `web/test/` | Low | Server output only; no NAS format, unless the sample struct gains a deck-height field (then it does) |
| **T2** Road shields by country | Replace `network_code`'s `lon < -40` branch with per-country rules: a country lookup from the outlines, US state prefixes generally. Bump the networks version. | F02 | `bin/extract.rs:600-700`, `rules.rs:29` | Med (visible map change) | **Rebuild** (unit keys, Americas) |
| **T3** Scenic margin and eye | Derive `stage::MARGIN_KM` from `reach::LONG_KM` + `scenic::FAR_MAX_M`, and stage around the long ways as buildtiles does. `view.rs` uses `EYE_M`/`RAIL_EYE_M` by a sample flag. Bump `SCENIC_V`. Fix README 1.7 m. | F03, F04 | `stage.rs`, `unit.rs:606`, `view.rs`, `bin/scenic-metrics.rs`, README | Med | **Rebuild** (unit keys) |
| **T4** One write path | Fix `whole::copy`'s temporary name first (a one-line, separate commit). Then: one local `whole::write` and one temporary-name helper on `sys::host()`; NAS writers through `write_whole_made` (delete `write_replace_nas`); the catalogue writer takes `Loc`/`NasPath`; one cached hostname. Delete the other writers. | F06, F07, F08, F40, F90 | `store/src/{whole,naming,cachefile,catalogue,sys}.rs`, `roadcore/src/lib.rs`, `agent/{mod,resume,room,build}.rs`, `heritagewd.rs`, `stage.rs`, `railfeeds.rs`, `timings.rs`, `bld/fetch.rs`, `scache.rs`, `server/src/{regions,names_live,profiles}.rs` | Med (many writers) | No |
| **T5** Server on store's readers (done, #850) | `SectView` over `store::sect::SectReader`, verifying whole-section reads; `MmapFile` with an `Arc` form; `PooledFile` takes `RemoteFile`'s reopen; a mirror piece-path accessor; exported `served::kept`, a status reader and a log path; one cached lead status; `IoPool::new(&NasRoot)`; one capped gunzip. | F05, F34, F38, F39 | `server/src/{views,data,main,regions,marks}.rs`, `store/src/{sect,range,mirror,iopool}.rs`, `inputs/served.rs` | Med | No |
| **T6** One status, three printers | The agent serialises per line and item: `counts`, `finished`, `running`, `out_of_touch` per member, a `verdict {state, said}`, a `kind` on waits and notices, and the room, disk and memory sentences. The build page, the menu bar and `scenic status` print these. Swift loops over all jobs. Request files go through the coordinator's `/work/*`, or get shared fixtures. One words module per language (spans, clocks, GB: binary, labelled GiB or GB consistently). Decide each surface's control set. | F09–F12, F44, F45, F61 | `agent/build.rs`, `agent/mod.rs` (Status), `bin/scenic.rs`, `tools/status/main.swift`, `web/work/{dash,sums,pool,live,settings}.js`, `room.rs`, `memguard.rs`, `coord/mod.rs` | Med (three surfaces) | Wire fields added; old ones kept a release |
| **T7** Tests that run | `publish.sh`: `cargo nextest run --workspace` plus `web/test`, `tools/check/*.test.mjs`, the taskboard tests, the Swift self-test and `tools/app/test/shell.zsh`. `check.sh` globs the test folders rather than listing them. Correct `testing.md`. | F13 | `tools/app/publish.sh`, `tools/dev/check.sh`, `tools/dev/testing.md` | Low (publish takes longer) | No |
| **T8** The steps table as the one source | Move `lead_only`, `nas`, `cost_version`, `quiet`, `resumable` and `gated` into `Step`. Derive `every::offered`, `PAGE_TASKS` and `NAS_STEPS` (drop `"elev"`). Make unit's `mem_mb` the real first guess. Trees and trees-lo get `none_*_key` helpers, `is_none_key` entries and `run_drop`. `Granted::Task` carries `kind`. `CostKey`/`JobId` types with parse and format. | F14–F17 | `agent/{steps,every,build,mod,memguard}.rs`, `coord/{mod,sched,task}.rs`, `needs.rs`, `treepacks.rs`, `bin/scenic-build.rs`, `forecast.rs` | Med | Cost versions kept by value; no rebuild |
| **T9** Shared tile and geometry names (values kept) | `roadcore::archive::{tile_key, tile_zxy, valid_tile, Entry::zxy}`; `TileId` out of `legacy.rs` into `crate::tile`, with a strict parser; `fmt_slash`/`fmt_dash`/`parse`; a pack zoom split constant and `pack_key` in `store::catalogue::Layer`; named Earth constants with today's values per site; a `NEAR_COVERAGE_KM` constant; correct `merc`'s doc and add `merc_clamped` (new callers only); a test checking `web/src/config.ts` and `basemap.ts` against roadcore. CLIs take `z/x/y`. | F18, F25 (names), F27, F28, F29 (doc), F30 (names), F33 | `roadcore/src/{archive,lib,tile}.rs`, `store/src/{pack,catalogue}.rs`, `legacy.rs`, `server/src/{data,ways,downloads,markview}.rs`, `cli/progs.rs`, ~15 inline decoders, `web/src/packs.ts` | Low–Med (wide, mechanical) | No (the buildtiles `8/x-y` path stays) |
| **T10** One geometry, one libm | One box-grower, one Mercator per side, `det` everywhere in output paths (plus a clippy `disallowed-methods` on `f64::sin` & co outside tests), one even-odd point-in-polygon (coverage's gridded test aside), one Earth value. | F25 (math), F29, F30, F31, F32 | `hipack.rs`, `stage.rs`, `areafiles.rs`, `road_terrain.rs`, `lakefill/run.rs`, `leaftype/warp.rs`, `inputs/regions/tiles.rs`, `terrain_deep.rs`, `bld/field.rs`, `outlines.rs`, `clippy.toml` | High (bytes move) | **Rebuild**: terrain/slope/deep coverage keys, leaftype, road heights, bld-field |
| **T11** Web shared modules and in-view | `merc.ts` (clamped by `MERCATOR_LAT`), `tilesIn`; colour `stopsFn` and `hexRgb` in `palettes.ts`, with the depth ramps registered there; a `Backoff` helper and one "NAS is back" hook; `WorkerPool<Req, Res>`; one `call<T>`; `fmt.area`/`signed`/`perDay`; `popup.ts`. Roads and rail histograms use `viewArea`. Roads' `u_bg` and `mixBg` take the land colour. Depth gets ScaleControls. `DrivesPane` extends a `ListPane`. Split per-layer controllers out of `main()`. | F19–F21, F29 (web), F52–F54, F62, F71, F91 (web) | `web/src/*` | Med | URL `dp=` only (read the old value) |
| **T12** Web naming and saved state | One key and one label per channel; ferry `freq` → `metric`, rail `group` → `service`; rail `freq` gets `def: 0.3` and the generic `migrateWeights`; one look model per metric; one `isScenic`; one name for the panel ("Settings"), with the UI paths fixed; road class `FERRY` → `CAR_FERRY`. | F46, F68, F69 | `web/src/{scenic,rail,ferry,state,presets,config}.ts`, `web/src/ui/*`, `index.html`, README | Med | **Migrate**: URL, localStorage and preset keys read old as new |
| **T13** Build page and tools hygiene | `web/work/dom.js` and `words.js`; machine colour tokens by a stable host hash; thresholds read from the status; one program list sourced by `wasm.sh`, `publish.sh` and `install.sh`; one shell lib for locks and owner ids, plus `tools/dev/common.sh`; exact-PID kills; styles out of JS and `index.html` into `dash.css`; an icon script; taskboard stage labels from `schema.mjs`; one shebang and strict-mode rule. | F49, F55–F57, F87–F89 | `web/work/*`, `tools/app/*`, `tools/dev/*`, `tools/status/sandbox.sh`, `tools/taskboard/*` | Low | No |
| **T14** Naming pass (code only; stored names aliased) | `Line.unit` → `noun` (wire, alias), the z6 sense of `unit` → `z6` in code; the JS `AREAS` → `LEVELS`, Scope's serde names `z3`/`z6` (wire); server `Packs` → `Prepared`, `cache.rs` → `served.rs`, `roadcore::packs` → `records`; `forecast::Lane`/`lanes` → `Slot`/`Stretch` (serde alias in the archive); "member" in code and UI (stored `helpers` paths kept); "pause" for the Mac pause (`hold-request.json` kept); module renames (`agent::brokers`, `agent::pool`, `takeover`, `stage.rs` → `staging.rs`, the `Source` types, `inview::Region` → `ViewArea`); serve `catalogues.js` under its own name; Swift `Step` → `StageLine`. | F41–F43, F64, F66, F67, F70 | wide | Med (churn) | **Migrate** for wire fields (`Line.unit`, Scope serde, `lanes`); stored names kept |
| **T15** Vestigial removal | Needs the owner's word that no old app runs, and a NAS check that no `final.i16` remains. Remove: the old wire fields (`job`/`beside`/`more`, `Worker::old`), `Heartbeat::shadow`, the "today" files, the six-generation job-disks ladder, `install.sh --legacy` and `legacyAgent`, roadcore's i16 reader, `tile_with_fallback`, `OLD_USES`, `--rail-rels-only`, `terrain_pack::fetch`, the web's dead exports, unused server fields, `--st-fine`, `migrate.mjs`, `docs/mockups/today/`. | F47, F48, F72–F74 | wide | Low once confirmed | Wire fields removed |
| **T16** Docs truth pass | Fix the five wrong docs (F26) and the dangling references; delete history narration, keeping only the compatibility facts the code acts on; one field list (`pipeline.md` §2, with `records_only`, `z10`, `polar`); `pool.md` §7.2 and `workers.md`'s build-page section become pointers; generate the diagram's step cards from `scenic steps` or cut its figures; drop or redirect `phase-times.md`; decisions and risks to board tasks; README Docs list and Data table (depth); `testing.md`; the 12 "§12 phase 4" and 61 "pool off" code comments. | F26, F76–F84 | `docs/*`, README, `docs/diagram/*`, comments in `agent/`, `coord/`, `pool/`, `budget/` | Low | No |
| **T17** Data-step deduplication (same bytes) | HTTP through `fetch::Net` (or one shared Retry policy); one `canopy::ensure_square` over `NasPath`; CREDITS from the accepted declarations; one `mid::read_whole` with deadline and retry; one `layers::drop`; `layers::{hi,lo,deep}_logical`; each layer's version in its own module (distinct polar names); a `rail::Pair` POD; `scenic-metrics` on `geotiff::Tiff`; progress through `jobs::report`; `quad_mean`; `fetch::USER_AGENT` in the 2 Rust literals; `producers/_net.py`; NAS paths as arguments; move `dem/*.py` out of `dem/`. | F22–F24, F34 (build side), F35–F37, F63, F92 | `terrain_pack.rs`, `slope_pack.rs`, `terrain_deep.rs`, `treepacks.rs`, `depth/build.rs`, `trees/squares.rs`, `bin/scenic-metrics.rs`, `rules.rs`, `rail.rs`, `gtfs/railcount.rs`, `bin/railfreq.rs`, `producers/*` | Med | No (keep each site's gzip level and summing order) |
| **T18** Agent structure | Base modules `crate::steps`, `crate::keys` and `crate::time` (Hinnant once), so `coord`/`pool`/`needs` stop importing `agent`. Move the per-step memory models out of `agent/mod.rs` into `needs`, and the task kinds and status types into their own modules. One `Switch { path, default }` table. One map of per-step region progress. Delete `/work/status` (or make it `view_of`) and declare route classes beside routes. A `Faulty` fake-`Nas` helper. One mutex-poison rule. CLI flag declarations once. | F50, F58–F60, F65, F75, F85, F86, F91 | `agent/mod.rs`, `coord/mod.rs`, `pool/*`, `needs.rs`, `backup.rs`, `nasread.rs`, `cli/scenic.rs` | Med (large moves) | Status `built` shape: migrate; journal day names byte-identical |
| **T19** Server endpoint symmetry | One `tiles::respond`/`etag_match` everywhere; decide 204 or 404 for a missing tile; Content-Encoding from `PackIndex::gzip()`; one `Lru` for every cache; one `has_names(layer)`; key view versions by layer name (with the web). | F51, F73 (versions) | `server/src/{tiles,cache,depth,terrain,water,marks,pages,main}.rs`, `web/src/main.ts`, `stations.ts`, `ferries.ts` | Low–Med | Wire (view-version keys), web in step |

Suggested order:

1. T4's one-line `whole::copy` fix (F06).
2. T6, T1, T7, T8 and T5: correctness and single sources.
3. T9 and T17: names and deduplication, with the same bytes.
4. T2, T3 and T10 together, as one planned units rebuild.
5. T11 and T12, T19, T13, T14, T18.
6. T15 once the owner confirms that no old app runs.
7. T16 alongside whichever task touches each doc.
