# Phase 5: landmarks, stations, ferries and overlays by view

The design behind plan §10 phase 5:
- landmarks, stations, ferries and area overlays served by view;
- the landmark build jobs;
- the heritage chain;
- the zoomed-out drive and ride summaries (end of this file).

**Version 5**, 2026-10-03: checked against the code, and the heritage chain switched on. It's built.
It replaces plan §6's first design, in which pack(T) wrote landmark and station tiles and the lo
packs held top landmarks and per-cell counts.

## Why

**Before:** the client fetched whole worldwide files on first use (`/api/layer/<name>`): stops and
sights per kind, heritage (61 MB), heritage areas, Indigenous lands, special areas, World Heritage
outlines, summits, stations and ferries. Its landmarks worker indexed them to compute:
- the In view statistics;
- the dots' layout;
- MapLibre point tiles (`lmk://`).

That couldn't scale past today's regions: a worldwide heritage file alone would be gigabytes.

**Now:** a catalog with the new layers loads them by view. The whole-file path stays only for
catalogs without them.

## What changes for the user

- **Today's regions look and answer the same:** dots, names, popups, the In view numbers and
  histograms. The golden test holds the In view answer to the legacy worker's exactly.
- **Zoomed out (below zoom 6),** the specks (dots below the size range) are drawn from counts per cell
  made at build time.
  - At zoom 6 and up, they come from every point.
  - A tilted view draws its far ground from thinned tiles two zooms coarser, with their cells and no
    extras.
  - The sized dots and names are the same.
- **One deliberate limit:** zoomed out with a locked range (auto off) low enough to size more dots than
  the tiles hold, the 5,000 most prominent of the rest are sized and the others show as specks.
  Before, every one was sized (up to ~300 k).

## Ids

One id per landmark, unique within its group, below 2^52, so it's exact as a JS number, a vector-tile
feature id and a MapLibre feature-state id. A group is the point kinds together, each area overlay,
stations, or ferries.

- **An OSM object the point is:** `id × 4 + type` (node 0, way 1, relation 2).
- **Otherwise** `2^51 + h`, where h is the first 51 bits of the xxh3-64 of a reference:
  - **candidates (the marks job):** their key with the kind (`trail:<relation>:<node>:trailhead`;
    `n123:peak` for an OSM id that is a point twice, a point of interest and a covered bridge);
  - **World Heritage sites' dots:** `whc:<site id>`;
  - **Canada's federal designations:** `reg:dfhd:<DFHD id>`;
  - **every other heritage point,** components included, built from the pass or converted:
    `legacy:heritage|<tier>|<lon E7>,<lat E7>|<name>|<url>`;
  - **converted points of interest:** their OSM id (from details-poi) when no other point of
    the group has it, else `legacy:poi|<kind>|<lon E7>,<lat E7>|<name>`;
  - **areas:** the OSM id when unique, else `legacy:<layer>|<name>|<box centre E7>`;
  - **stations:** a stop merged from several members takes its first member's `id × 4 + type`, nodes
    before ways, each by id; converted stops take `legacy:station|<name>|<lon>,<lat>`;
  - **ferries:** ways `way × 4 + 1`; terminals `legacy:terminal|…`.
- **Repeats:** a reference occurring more than once gets `#2`, `#3`, … in the byte order of its
  records' canonical JSON. Exact duplicates are kept that way, so the counts stay.
- **Clashes:** the ids are then sorted and checked. A clash (different references, one id) moves the
  later reference in byte order to `<reference>#h1` (`#h2`, …) until none remain, and the job asserts
  uniqueness.
- **Nothing stores ids across catalogs:** URL state holds only `s` and `st`, and descriptions are
  keyed by QID or OSM id. So a rebuild changing one breaks nothing.

## Points: the 8 kinds

Viewpoint, peak, waterfall, lighthouse, covered_bridge, rest (rest_area and picnic_site),
trailhead, heritage.

### Stored

**`markdata/6-x-y`**: a sectioned file per z6 tile, holding every point positioned in it, all kinds,
sorted by kind then id (a catalog map like `base`). Sections:
- `kinds`: per kind, first row and count;
- `ids`: u64;
- `pts`, 28 B per point:
  - lon, lat: i32 E7;
  - fa, ia, mz: f32 (ia is 20000 when unknown, as the worker reads it; mz is NaN when none);
  - rank: u32, the point's place in its kind's order. It's the tie-break: converted points take the
    legacy file's order, the `marks` job its sorted output;
  - kz: u8, the lowest zoom whose thinned tile keeps the point (6: none);
  - class: u8 (heritage: level class + 3 × group, as dotData);
  - tier: u8 (heritage);
  - flags: u8 (named; World Heritage component; picnic site);
- `fvals`: per kind, one f64 column per property of its filters (range and flag alike), NaN unknown:
  the JSON doubles exactly;
- `props`: the lean properties, JSON per point, in zstd blocks of 256;
- `info`: popup records, likewise;
- `summits`: the named peaks with a height, as the summits list has them (lon and lat at 5 decimals,
  ele rounded half-even, name), each with its place in the worldwide list (height, then fame), for the
  highest named peak in view.

**`layers/marks-<kind>/{root,lo}`** are packs (root z0–2, lo z3–5 per z3) of thinned tiles, in the
marks tile format.
- **Kept at zoom z,** per kind and tile:
  - the named points with mz ≤ z − 3;
  - the top 256 by score at balances 0, ¼, ½, ¾ and 1;
  - the top 64 by each range filter except years (largest first).
- Score and value ties go by rank everywhere, so each rule is monotone in z and "kept at z" is
  "kz ≤ z".
- World Heritage components aren't kept: they're drawn close in, from z6 blocks.
- **The rest** go as speck cells at zoom z + 10 (1,024² per tile, covering a 2× screen at fractional
  zooms): cell and count, and for heritage, per (cell, tier).

**`global/marks/summary`**: `{fmt, kinds, tiers}`.
- Kinds are counted by their `kind` property (rest areas and picnic sites apart), components left out.
- The area overlays' totals come from `layer-summary`: the overlays job's (`global/heritage/`), else
  today's (`global/legacy/`).

### The marks tile format

What the client reads for thinned tiles, z6 blocks and `extra`. It's not MVT: positions are exact
(E7), values are typed arrays straight from the bytes, and the worker never hands these to MapLibre.

- **Header** (32 bytes): `RDMT`, version, n points, nf filter values, n cells, props byte length, and a
  reserved u64.
- **Columns**, each 8-byte aligned:
  - ids: f64;
  - fvals: f64 × n per field;
  - lon, lat: i32 E7;
  - fa, ia, mz: f32;
  - rank: u32, the draw order's and specks' tie-break;
  - kz, class, tier, flags: u8;
  - cells: code (u32, the cell's Morton code within the tile), count (u32), tier (u8);
  - props offsets: u32 × (n + 1).
- **props:** a lean JSON object per point, back to back, so one point's props parse alone.

The server attaches the display names when serving: `main`/`sub`, and `cmain`/`csub` from a World
Heritage component's `cn`. Served gzip'd.

### Served

- **`GET /api/marks/tile/{kind}/{z}/{x}/{y}`** (z ≤ 5): a thinned tile.
- **`GET /api/marks/block/{kind}/6/{x}/{y}`**: every point of the kind in the z6 tile, from markdata,
  with no cells.
- **`GET /api/marks/specks/{kind}/{z}/{x}/{y}?q=…`** (z ≤ 5): the speck cells of the points that pass
  the kind's filters, unknowns and switched-off tiers, and that the tile doesn't keep. Made from
  markdata over the tile, and cached per tile and query.
- **`POST /api/marks/view`**: today's worker `query`, in Rust, over the `pts` of the z6 tiles meeting
  the outline. It returns the same JSON as the worker's `result`:
  - the 512-bin score histogram and the scores at the given ranks;
  - per kind, its count and best-known named point;
  - the top 60 overall and per kind;
  - the open filters' histograms;
  - the highest named peak, from `summits`, whatever the filters and the Peaks switch (as the
    worker does).

  World Heritage components are skipped, as the worker does.

  It also returns `extra`.
  - **The request** carries `tz` (the zoom of the tiles the client shows; 6 for blocks), the range
    when it's locked, and the ids of the extras the client already holds.
  - **`extra`** is the points sized at this view's range that the tiles at `tz` don't keep (kz > tz),
    less those the client holds:
    - the range is spreadRange of the scores at the ranks, or the locked range;
    - at most 5,000 in all, the most prominent first;
    - each in the tile format's fields, as JSON;
    - usually 0–20 at the default ranks; with ranks of 1,000, 1.5–3 k at first, then only those
      coming into view.
- **`GET /api/marks/count?kind=…&q=…`**: worldwide filtered counts, cached per catalog and query.
- **`GET /api/marks/detail/{kind}/{id}?at=lon,lat`**: the popup record, with descriptions laid over
  (plan §7).
  - It searches markdata's `info` in the z6 tile of `at`, then its eight neighbours, and answers 204
    when none holds the point.
- **The marks version:** requests carry it, a hash of the marks files' content names and the
  translations' version. A mismatch answers 409, and the client asks again after switching.
- While the catalog lists `global/legacy/*`, `/api/layer/*` and `/api/detail/{layer}/{i}` stay.

### Exactness

The golden test (`tools/golden-marks`) compares the server's answers with the legacy worker's code,
run under Node on today's files with the same requests: several places, zooms, balances, ranks,
filters and tier switches. They must be exactly equal:
- **Score:** `(1 − b)·min(1, fa/5) + b·clamp((log10(max(0.05, ia)) + 1.3)/5.6, 0, 1)`, in f64 from
  the f32 fa and ia, as the worker computes it.
  - The server's log10 is `marks::log10_js`: V8's fdlibm log10, with the fused multiply-adds clang
    makes on arm64, so it matches Node bit for bit.
- **Degrees:** from E7 as `e7 as f64 / 1e7`. A division gives back the JSON double for up to 7
  decimals; a multiply by 1e-7 can be off by one ulp.
- **Outline and bounds:** parsed as f64, not truncated; across the antimeridian (west > east), as in
  the worker.
- **Ties:**
  - the best per kind is the highest fa, then the lowest rank;
  - the top lists go by score, then the kind's place in the request, then rank (the worker's
    stable insertion);
  - the scores at ranks come from the scores rounded to f32 (the worker's Float32Array sort);
  - the summit is the first in view in the summits list's own order (height descending as stored,
    then fame ascending).
- **Filters and histograms:** over the f64 values, binned as filterHists.

Done 2026-10-03: equal in 163 views, every block, and 15,170 popups.

## Areas: heritage areas, Indigenous lands, special areas, World Heritage outlines

- **`layers/ov-{heritage-areas,indigenous,special,whs}`** (root, lo; hi z9–12 within the coverage +
  20 km): MVT, extent 4096, layer `a`, simplified per zoom and clipped.
  - Each feature has its id as the MVT feature id, the lean properties, and `own` (heritage areas,
    Indigenous lands and special areas only): the z3 tile whose ovdata holds its details.
  - A World Heritage outline carries its site dot's id and position (`px`, `py`).
- **`ovdata/3-x-y`**: sectioned.
  - It holds area details by id and park records (name, bbox, tags, Wikidata).
  - The owner is the z3 tile of the feature's or park's box centre.
- **Routes:** `GET /api/overlays/detail/{layer}/{id}?own=3/x/y` (`/api/areas/` is the Regions
  panel's); `/api/park` from ovdata when the catalog has it.
- **Client:**
  - vector sources;
  - the area histograms come from `querySourceFeatures`, de-duplicated by feature id;
  - clicks are de-duplicated by id too.

## Stations

- **`layers/stations/{root,lo,hi}`:** MVT, extent 4096, layer `s` (`n, en, g, m, sp, mz`, and the
  feature id).
  - A tile at zoom z holds the stops shown up to z + 1 (mz ≤ z − 2.585).
  - Zoom 12 holds every stop, within the built units' tiles + 20 km. Overzoomed, positions are within
    about 0.75 m at 51° N.
- **Client:** a vector source, with feature-state and `querySourceFeatures` on `sourceLayer: 's'`.

## Ferries

- **`layers/ferries/{root,lo}`:** gzip'd GeoJSON blocks: z0 simplified to 5 km, z3 to 300 m, z6 full.
  - A block holds the ferry ways whose box meets the tile grown by 30 km, the terminals within 30 km
    (from zoom 3), and its ways' lines' records.
- **Client:** ferries.ts merges blocks by id.
- **In-view kilometres and histograms:** each way carries its full length, and its share in view is
  measured on the geometry loaded, so the numbers stay within 0.5 % of the whole files' zoomed out.

## Server, generally

- Tiles with names have one handler, generalised from the labels', with a rule per layer. The marks
  tile format has its own attacher.
- **Caches:**
  - The marks tiles and blocks made with names are cached by bytes (192 MB): a z6 block reaches
    1.6 MB.
  - The named-MVT cache (overlays, stations, labels) holds up to 6,000 tiles and is cleared when
    full.
- markdata and ovdata are typed section views: paged from the NAS, mapped from the mirror.

## Client

| Before | Now |
|---|---|
| a kind's file loaded whole into the worker | per kind, tiles: z6 blocks from zoom 6 (and back below 5.75), thinned tiles at floor(zoom) below (±0.25); a tilted view's far ground from thinned tiles two zooms coarser; held as typed arrays and raw props (decoded for a lmk tile, a popup or a list); the least recently used dropped past 256 MB |
| the worker's `query` | `/api/marks/view`, asked and parsed by the worker; `applyResult` unchanged; `extra` joins its kind's points, de-duplicated by id |
| the summits file | the view's highest named peak |
| `count`, tier counts, `layer-summary` | `/api/marks/count`; totals from `global/marks/summary` |
| a source's dot layout | per kind over the loaded tiles and extras, laid out again (debounced 60 ms) when they change; the order within a z4 chunk is the files' (fame, then rank), so the draw order and the zoom-6-and-up specks are the same |
| — | below zoom 6, speck cells become pseudo-points: fa 0 and ia 0.05 (score 0 at any balance); the cell's count weighs it in visWords; class "the rest" in its tier's group colour (heritage), else 0 |
| filter masks per source | per kind, as before; below zoom 6 a filtered kind's cells come from `/api/marks/specks`, its unfiltered cells hidden until they arrive |
| `lmk://` tiles from the whole index | from the loaded points and extras, the same caps; `TileNames.ids` a Float64Array |
| area overlays as GeoJSON | vector sources |
| `stations.json` | a vector source |
| ferries files | blocks for the view, merged by id |
| details by `i` | marks by `{kind, id, at}`, areas by `{layer, id, own}`; cache keys `mark:` and `area:` (the legacy `layer:i` keys can't collide with them) |

The whole-file path stays for catalogs without the new layers (every kind or none).

## Build

The jobs form a chain without cycles: every input exists before its reader runs. In the agent's order
(plan §8), the landmark chain follows the units and runs beside the roads chain:

| Job | Reads | Writes |
|---|---|---|
| OSM pass sets | the filtered planet | `summits` (`-v2`): natural=peak and volcano nodes and ways, with or without `ele`, worldwide; `hikes`: hiking and foot route relations with their member ways; `named`: today's heritage filter (historic, heritage, museum/attraction/viewpoint, lighthouse, station, church and place of worship, protected area and park, military) plus `ref:whc` and `heritage:operator=whc`; `marks`: the point kinds' tags (read by no job yet) |
| `trailends` (per pass) | the `hikes` set | `work/trailends/<d>`: every simple linear hiking route's two ends worldwide (way ends used once, exactly two), with the route's name and relation id |
| `terrain-z8` (once; network) | AWS's raw z8 tiles (the build Mac's raw-tile cache) | `sources/terrain-z8-v1` (not served): every z8 tile repaired, and `-v1-max`: each tile's maximum after the repair |
| `summits` (per pass) | the `summits` set, `terrain-z8` | `work/summits/<d>`: every summit (OSM id, E7 position, kind, `ele` as the candidates have them, its `z8` overlay value), nodes then ways, each by id |
| `heritage-sites` (before the units) | the registers snapshot, the pass's `areas` set, the coverage | `work/heritage/<d>/base/…`, and per z6 tile the sites' positions and the designated areas' polygons ("Heritage and area flags") |
| `unit`, base(U) | as plan §6, with the heritage slices within U + 30 km | the base pack |
| `pois` (per unit) | U's piece, `work/trailends/<d>`, the coverage near U | `work/pois/<u>`: U's candidates (below) |
| `peaks` (per unit; network) | `work/pois/<u>`'s peaks, `work/summits/<d>`, z12 within 30 km of each peak (the terrain packs, else the same tile from the raw-tile cache, processed alike), `terrain-z8` | `work/peaks/<u>`: prominence and isolation by candidate key |
| `items` (per pass; network) | the QIDs of every current unit's candidates | `sources/items/<d>/{facts,views,meta}` |
| `heritage` (network; off) | the heritage-sites outputs, the pass's `areas` and `named` sets and the filtered planet within the cover, the seeds | `work/heritage/<d>/…`: today's chain's outputs ("Heritage and area flags") |
| `marks` (worldwide) | the current units' `work/pois` and `work/peaks`, the facts and views (else today's, `sources/legacy/m1/`), the heritage points (the pass's when the heritage job's outputs are in the manifest, else today's `global/legacy`) | `marks-*` packs, `markdata`, `global/marks/summary`, `work/marks/heritage-dots` |
| `overlays` (off) | the heritage job's outputs, `work/marks/heritage-dots` | `ov-*` packs, `ovdata`, `global/heritage/{layer-summary,heritage-sources}` |
| `stations` (roads chain) | the `rail` set, the built units' tiles | `stations` packs |
| `ferries` (roads chain) | the `ferries` set (worldwide), `inputs/ferries/freq` | `ferries` packs |

**Keys** are mostly the step version plus the content names of what a job reads:
- heritage-sites, heritage and items include the pass's date, and the coverage enters as a hash of its
  shapes;
- stations, ferries and overlays name the built units (ferries also the timetables' digest);
- `pass-sets` and `terrain-z8` have no keys: they run when their versioned outputs are missing.

**Network jobs** (terrain, terrain-z8, peaks, items, heritage): a tile or answer that can't be
fetched fails the job (retried later), never counts as "none".

**Run by hand:** `registers-import` (the registers snapshot), `convert-legacy-marks` and
`convert-legacy-overlays` (today's points and overlays).

**Determinism across units:**
- **Peaks:**
  - **The inputs are the same everywhere.** A peak's result reads only z12 within 28 km of it and the
    worldwide z8, the same whichever unit computes it and whatever the coverage. z12 is the packs'
    tiles or the same raw tile processed alike; z8 is one artifact.
  - **Neighbouring summits** count at their own heights, worked out the same way wherever they are.
    Ties go by tagged height, distance, then OSM id.
  - **A z12 tile AWS doesn't have** (404: open sea) is sea level in every unit, never an upsampled
    ancestor.
- **Neighbourhood rules** (trailheads within 150 m) run on U plus the piece's buffer, in one order
  (rank, then key), so neighbouring units agree on the points near their border.
  - A kept point belongs to the unit that owns it.
  - Hiking-route ends come from the whole route (`trailends`), not from the piece, where a route
    leaving and coming back would show other ends.
  - At the coverage's edge, a point kept outside it can still drop one inside and take its name: the
    clip comes after the dedup, so every unit agrees.
- **The municipal heritage dedupe (40 m)** runs once, in heritage-sites, inside today's heritage.py,
  over the whole cover.

**Where today's steps went:**

| Today | Job | Language |
|---|---|---|
| extract.rs POIs | `pois` (extract `--candidates`, with the pass's `trailends`) | Rust, today's rules, keeping OSM ids and tags (no 60 m matching) |
| poidetails.py | `pois` (kept tags, `length_m`, `viewpoint`, the re-kinds), `items` (facts), `marks` | Rust; Python for the fetches |
| peaks.rs | `peaks` (`pipeline::peaks`) | Rust |
| heritage.py (with heritage_eu.py) | `heritage-sites`, unchanged; federal.py's and crhp.py's outputs are in the snapshot | Python |
| heritagewd.py, heritagedetails.py, areadetails.py, whsshapes.py (on the filtered planet within the cover), filterprops.py, pageviews.py, interest.py, layers.py | `heritage` | Python, unchanged |
| pageviews.py, for the candidates | `items` (dem/items.py) | Python |
| filterprops.py, interest.py, layers.py, for stops & sights | `marks` | Rust, sorted so it's deterministic, rounding as Python does (half-even on the exact binary value; mz from the unrounded ia), distances across the antimeridian |
| stations.py | `stations` | Rust |
| ferries.py, hand timetables | `ferries` (gtfs.py's results among the timetables) | Python |

### The landmark jobs, step by step

**Why peaks run in their own job, on the worldwide z8:** a peak's result has to depend only on what
lies near it.
- A flood near U's border can't go on over coarse ancestors of U's staged terrain.
- The packs' z8 depends on the coverage (it's made again from z9 where z9 exists).
- A key must name everything read.

- **Candidates, their own job per unit (`pois`)**, so that a change to their rules doesn't make
  every unit again. The unit's own extract keeps today's points, since its view step reads the
  viewpoints for the road flags.
  - **The run:** `pois` runs extract on U's piece with `--candidates` and the pass's `trailends`,
    then writes `work/pois/<u>` (zstd JSON lines sorted by key).
  - **One order:** by (trailhead rank, key, kind) before the 150 m dedup, so the kept point and the
    name it lends don't depend on thread timing.
    - Keys are `n<id>`, `w<id>` or `trail:<relation>:<node>`, plus the kind: one way can be a point
      of interest and a covered bridge.
    - Three runs over Taiwan are byte-identical.
  - **Hiking-route ends:** the pass's `trailends` near the piece's roads (the 300 m road test stays
    per unit).
  - **The clip** comes after the dedup: to the points U owns that are in the coverage (nodes inside
    it; ways with a node inside it).
  - **Kinds** as today's map has them:
    - natural=peak (nodes, and ways' centres);
    - a viewpoint that is also a volcano is re-kinded to a peak, as poidetails.py did. Pure
      volcanoes aren't points: they are often a crater node beside the rim's peaks, and they stay in
      the summits. This applies in the candidates only, not in the unit's points.
  - **Each candidate** carries:
    - `key, kind, lon, lat` (E7 integers);
    - `name, ele, osm, qid` (the `wikidata` tag as tagged);
    - `en`: `name:en`; in Japan `name:ja-Latn` or `name:ja_rm`;
    - kept tags (poidetails' lists; not `image`, which today's details never had and which would move
      fame);
    - what poidetails.py added: `length_m` for covered bridges (its planar formula over the way's own
      nodes, rounded half-even), and `viewpoint: "yes"` for peaks tagged tourism=viewpoint.
- **`trailends`** (per pass): the `hikes` set's routes (route=hiking or foot).
  - It finds each way's end nodes and keeps the ends used once; exactly two make a simple linear
    route.
  - It's worldwide, so every unit sees the same ends of a route that leaves its piece.
- **`terrain-z8`** (once, network): AWS's 65,536 raw z8 tiles, repaired as the packs' tiles are
  (`terrain_pack::process`, no children: the voids filled with 32,767 m and the spike clusters go).
  - It's one content-named pack under `sources/` (a `global/` file would be served and mirrored),
    with each tile's maximum.
  - It's coverage-free, so the isolation searches and the coarse floods give the same answer
    whatever is built.
  - **Why raw z8:** AWS's own z8 comes from a coarser source than today's (made again from z9 near
    roads). Measured on 864 mountainous z8 tiles of today's packs (max ≥ 1,000 m; 51.7 M land
    pixels), today minus raw:
    - per pixel: mean −0.1 m; |diff| median 0.0, p95 6.2 m, p99 34.5 m (max 1,514 m, a repair);
    - each tile's maximum: p5 −15.8, median and p95 0.0, extremes −103 and +200 m (a summit today's
      z9 means kept).
- **`summits`** (per pass): the `summits` set (natural=peak or volcano, nodes and ways), nodes then
  ways, each by id.
  - **A summit has one identity:** the same position and `ele` whether it's read as a candidate or as
    a neighbour. A node's position, or a way's centre as extract makes it (the integer mean of its
    nodes, with the closing node counted twice, truncated); `ele` is parsed and rounded as extract
    does, by shared code.
  - **The z8 overlay:** each summit's z8 pixel is raised to its tagged `ele` when that's plausible
    there, else left alone. Plausible means:
    - at most 8,900 m;
    - at most 1,500 m over the highest z8 pixel within one pixel;
    - at most twice that plus 300 m. Feet tagged as metres overshoot by 2.28 times the height, which a
      fixed margin lets through below ~1,100 m.
- **`peaks`** (per unit, network): `pipeline::peaks`, today's peaks.rs as a library, on U's peak
  candidates. On today's archive and candidates it gives today's peaks.json byte for byte.
  - **z12:** a tile is the terrain pack's when the manifest has it, else AWS's raw tile from the build
    Mac's cache, processed the same way.
    - The tile is read back from the PNG `process` returns (quantised as stored), never from its
      floats.
    - A tile not cached is fetched; a failed fetch fails the job.
    - Packs the terrain job made from raw are the same bytes.
    - Today's packs came from the legacy terrain step, which repaired stored tiles again. On 2,406
      land z12 tiles, 2,390 are byte-identical to the raw tile processed alike. The other 16 differ
      by under half a metre along coasts, or in one to three single pixels by 126–303 m, which the
      peaks' own despike clamps either way. Every tile's maximum is equal.
  - **Summits near a peak:** each summit within 28 km + 2 × (150 m + 2 pixels) gets, all from z12 the
    same way for every unit:
    - its summit pixel: the highest within 150 m;
    - its height: max(ele, DEM) when `ele` is within −30/+200 m of the DEM's, else the DEM's;
    - its claim: when several share one pixel, the highest tagged, then the nearest to it, then the
      lowest OSM id. The others start from their own point.
  - **Fine stage** (z12, a 600k-pixel flood): a flood that would read a pixel more than 28 km from
    the summit stops, and the peak goes to the coarse stage.
    - The isolation search (25 km) counts only pixels within 25 km, and never opens a tile whose
      great-circle lower bound is beyond. So its result depends only on z12 within 25 km, and U + 30
      km of z12 always suffices.
  - **Coarse stage** (z8): the flood (40M pixels) and the isolation search run over `terrain-z8` with
    the z8 overlay.
    - The summits within the fine radius (28.3 km) count at their z12 heights. That includes the
      peak's own, since a tag over 200 m above its DEM would otherwise make its own pixel its higher
      ground.
    - The tag rule applies beyond.
    - Tiles are taken in order of a great-circle lower bound, with x wrapping at the antimeridian.
      A tile is skipped when neither its maximum nor its overlay is higher, and tiles are decoded
      through a bounded cache.
    - Nothing higher within 5,000 km gives a lower bound of the radius searched.
  - **"Sea"** is ≤ 0 m, and `process` clamps every negative value to 0, so polders and depressions
    count as sea until the planned sea mask (plan §6).
  - **Output:** `work/peaks/<u>`, by candidate key: e, p, pl, c, ce, iso, il, hi.
  - **Checked** (`examples/peaks_check`):
    - **The Sierra Nevada's 8,235 peaks** in 6.7 s; 8,225 are identical to today's peaks.json.
      - Of the rest, three pairs swap a near-tied claim (the check's positions are today's floats, not
        OSM's E7), and two coarse cols come out 14 and 22 m higher.
      - Two overlapping halves agree with the whole run on every shared peak.
    - **Famous peaks:** Mont Blanc, Fuji, Ben Nevis, Yushan, Robson and Washington keep today's
      height, prominence (Robson +10 m) and nearest higher pixel.
      - Isolations are a little shorter as great-circle distances: Fuji 2,076.3 km (Wikipedia
        2,077), Ben Nevis 738.6 km (739), Mont Blanc 2,804.7 km.
      - Mont Blanc's flood spends its 40M pixels and stops at the same col as today's (128 m, a lower
        bound in both): 54 s, 3.4 GB.
- **`items`** (Python, network, per pass; dem/items.py) runs after the candidates exist.
  - **QIDs:** those of the current units' candidates. The heritage records' items stay with the
    heritage job.
  - **Facts** as poidetails.py fetched them, for single-QID tags. As before, a multi-QID tag gets no
    facts.
  - **Pageviews** for the first QID: the mean of four months pinned per pass. The months are the last
    November, February, May and August that ended at least 20 days before the pass, so their dumps
    are out.
  - **Refetching:**
    - Everything is fetched again at each pass. Between passes, only QIDs it hasn't seen, so a run
      started by new coverage doesn't move fame elsewhere.
    - Each item's Wikipedia articles are listed again at each pass too, so new articles count. New
      articles between passes are batched, since each run streams the four dumps again (~20 GB).
  - **Caches** are appended 5,000 items at a time, so a run stopped midway keeps what it fetched. A
    pageview month is cached only when curl, bzip2 and grep all finished cleanly.
  - **Output:** `sources/items/<d>/{facts,views,meta}`. meta holds the months, and the first and last
    days anything was fetched (QLever's index is whatever it serves those days).
  - **Requests:** User-Agent "road-elevations/0.1 (personal offline map)" (no contact address:
    identifying details stay out of requests), within the APIs' rate limits.
    - With no contact, Wikimedia may refuse by User-Agent, so a 403 or any failed batch fails the job
      loudly.
- **`marks`** (Rust, worldwide):
  - **Reads** the current units' `work/pois` and `work/peaks` (from the manifest, not every file under
    `work/`), in one order by key.
  - **Builds** `marksjob::Candidate`s (details: kept tags, `osm`, facts as `wd`, `length_m`,
    `viewpoint`), turns them into points with `marksjob::poi_points`, and adds the heritage points.
  - **Writes** with `markconv::write`; its summits list is by −ele, stable over fame order.
  - **Repeats:** an OSM id that repeats gets a reference from its key, as `assign_ids` needs.
  - **Descriptions** aren't read here: they're applied when serving (plan §7).
- **The checks:**
  1. **Port:** `pipeline::peaks` on today's archive and `pois.json`, with today's overlay rule, gives
     today's `peaks.json` byte for byte. Done.
  2. **Determinism:** a unit run twice gives the same bytes, and two neighbouring units agree on every
     point and peak within 10 km of their border. Done for candidates (Taiwan) and peaks (overlapping
     halves).
  3. **One planet:** a region-sized extract and the union of its units' candidates are the same
     (counts, E7 positions, OSM ids, tags). In the cutover's comparison.
  4. **Against today's files:** nodes by exact E7 position, then by details-poi's `osm`; adds, removes
     and moves per kind; trailheads per source. In the cutover's comparison.
  5. **Peaks:** exact where both used the same z12 and finished in the fine stage (those that went
     coarse differ by design, reported apart). Otherwise, of peaks with ≥ 50 m prominence, 95 %
     within |Δe| ≤ 30 m, |Δp| ≤ max(20 m, 10 %), |Δiso| ≤ max(0.5 km, 10 %), and the rest listed.
     Today's lower bounds (`pl`, `il`) are checked as new ≥ old. Done for the Sierra Nevada.
  6. **marks:**
     - exact on today's inputs: done (marks_regression);
     - then new candidates with today's facts and pageviews, then fresh facts: in the cutover's
       comparison. That compares per kind the fa rank correlation, the top 100 per z6 tile (≥ 90 %
       the same), the mz and kz histograms, a dozen In view answers (top 60), and screenshots.

### Heritage and area flags

**Status:** built and run: heritage-sites and the units' flags, the heritage job, the overlays job
and the server's switch to their outputs (checked below).

**The approach.** Today's heritage chain (heritage.py, heritagewd.py, heritagedetails.py,
areadetails.py, whsshapes.py, filterprops.py's heritage part, pageviews.py, interest.py's heritage
part, layers.py) runs unchanged.
- It runs in a stand-in root laid out as the repository: `dem/` holds the app's scripts, and
  `data/heritage/` the registers' snapshot.
- It's split into two jobs: the units need only heritage.py's output, and the rest needs Wikidata and
  the pageview dumps, whose outages mustn't hold up the roads.

- **The registers' snapshot** is one archive in the manifest, `sources/registers/legacy`.
  - It's imported with `scenic-build registers-import`: 9,337 files, 261 MB, since thousands of small
    files copy slowly over SMB.
  - It's the build Mac's `data/heritage` without `osm/`: today's map was built there. (The other
    Mac's copy has an older federal.json, with 1,346 Parks Canada sites against 1,347.)
  - The jobs extract it once per archive and clone it per pass (APFS). So a pass's runs share the
    caches the scripts add, and a new pass or snapshot starts from the snapshot again.
- **`heritage-sites`** runs after slope, before the units.
  - **The cover:** the z12 tiles within 20 km of the coverage (exact, `Coverage::meets_rect`). It
    replaces today's analysis grid (z11) through heritage.py's `--tiles`.
  - **The designated areas:** the pass's `areas` set, clipped to the cover (`osmium extract -s smart`,
    the tiles as rectangles), stands for today's areas.geojsonseq. heritage.py writes the areas'
    polygons with their flag bits instead of rasterising them.
  - **Outputs:** `work/heritage/<d>/base/<file>`, and per z6 tile:
    - the sites' positions: `pos/6-x-y`, E7, sorted;
    - the polygons whose bounding box meets the tile: `areas/6-x-y`, keyed by content.
  - **Key:** the step's version, the pass, its areas set, the snapshot, the coverage. It took 96 s for
    today's regions.
- **The units** (`UNIT_V` 3) read the slices of the z6 tiles within U + 30 km.
  - `heritage.json` feeds the flags step.
  - areaflags.py rasterises the polygons onto the unit's own grid, chosen by bounding box. Mercator is
    monotone per axis, so no polygon touching an edge tile is dropped.
  - Their keys name those slices.
- **Checks:**
  - **Reproduction:** today's heritage.py in the stand-in root, on today's grid and the snapshot,
    gives today's outputs exactly.
    - grid.areas.u8 matches byte for byte.
    - The sites, heritage areas, special areas and Indigenous lands are equal once the properties
      later steps add are set aside.
  - **Per unit:** rasterised per unit, six units' grids equal today's on all ~6,100 tiles they share
    with it, and two neighbouring units agree on all 228 tiles they share.
  - **The new cover against today's grid:** 61 sites are added and 24 dropped, of 224,010.
    - In: roadless places within 20 km (Pimachiowin Aki, Okinoshima, northern Parks Canada sites).
    - Out: places today's grid reached past borders along roads (northern Sardinia from Corsica,
      Korea's Gaya tumuli).
- **The `heritage` job** (in the landmarks chain, after items, before marks) runs the rest of the
  chain on the heritage-sites outputs, in the same stand-in root, over the same cover.
  - **Its OSM inputs** are the pass's areas and named objects within the cover (named with today's
    exact filter; the set also keeps the World Heritage tags). For whsshapes, it uses the filtered
    planet within the cover: one clip per pass and cover, in the cache, which is what today's
    regional extracts were.
  - **Seeds:** today's park facts, pageview months and names table (`sources/registers/legacy-seeds`,
    7 MB).
  - **Pageviews:** the months are the items job's cache, and pageviews.py takes the pass's months
    (`--epoch`, the items job's rule).
  - **English:** the layers' `en` comes from today's names table.
  - **Failures:** heritagewd's short descriptions, and the special areas' and UNESCO sites' Wikidata
    labels, fail the run when a query fails. Every cache is written through a temporary file.
  - **Outputs:** `work/heritage/<d>/<stem>`.
- **The switch.**
  - Marks and the server take the pass's heritage whenever the heritage job's outputs are in the
    manifest: `markconv::heritage_source`, and the server's `global/heritage/…` over today's.
  - **The marks job** then saves the World Heritage dots' ids (`work/marks/heritage-dots`).
  - **The overlays job** (after marks) makes the area overlays and parks from the same outputs, with
    those ids. ovconv kept them in step by replaying today's whole marks assignment.
  - **Until the heritage job's first run,** catalogs keep today's heritage points and area overlays.
- **Checks of the chain:**
  - **Reproduction:** today's whole chain in the stand-in root, on today's inputs and the seeds, made
    17 of today's outputs byte for byte: every details file, the area layers, the World Heritage
    outlines and sites, props-heritage. It needed no query or download, since every answer was
    cached.
  - **Fame:** heritage and layer-heritage differ only in fame.
    - Today's lacks pageviews for 2,280 sites (recent World Heritage inscriptions, Japanese and
      Taiwanese register sites…): its fame was worked out before the build Mac's last pageview run.
    - The run's totals equal that last run's for all 63,618 items.
  - **The switch on those outputs:**
    - the overlays job makes all 309 of today's overlay packs and ovdata byte for byte (with today's
      dots);
    - the heritage points match today's by reference, with 4,885 of 224,095 differing in fame and
      what follows from it (pv, fa, mz, ia).

## Storage

- `markdata/` and `ovdata/` are catalog maps (formats.md), so GC handles them.
- **`work/` holds build intermediates,** none of them in a catalog:
  - `work/pois/<u>`, `work/peaks/<u>`;
  - `work/trailends/<d>`, `work/summits/<d>`;
  - `work/heritage/<d>/…`;
  - `work/marks/heritage-dots`.
- **GC keeps what the build manifest names.** A retired pass's `work/` entries leave the manifest when
  a newer pass completes. A unit's `work/pois` and `work/peaks` stay named after the unit leaves the
  coverage (plan §10).

## Today's regions

1. `scenic-build convert-legacy-marks` wrote markdata and the marks packs from `global/legacy/*`: the
   legacy fa, ia and mz, file order as rank, and ids as above.
2. **Golden:** the view answers equal the legacy worker's exactly, and details match by id.
   Screenshots match except the zoomed-out specks.
3. The client followed; both formats work, every kind or none.
4. Overlays, stations and ferries were converted the same way (`convert-legacy-overlays`,
   `pipeline::ovconv`). The app switches on the catalog's layers (meta `ovTiles`, `stationTiles`,
   `ferryBlocks`).
   - Checked against today's files: the same stops drawn (London 500, Paris 952); ferries' in-view km,
     routes and terminal colours equal; area popups with their details.
5. The new jobs reproduce today's data. On the legacy inputs, `marks` reproduces fa, ia and mz exactly
   (marks_regression). The rest comes with the cutover's comparison.

Steps 1–4 are done, and step 5's legacy-input regression too.

## Sizes (measured from today's files)

- **Points:** 582,665, plus 1,768 World Heritage components, in 251 z6 tiles. The densest tiles hold
  47 k (Spain, peaks) and 43.9 k (London).
- **marks lo packs per z3** hold the thinned z3–5 tiles. The server makes z6 blocks from markdata:
  London's come to 6.4–8.6 MB. The largest, London's heritage block, is 1.2–1.6 MB. Putting `id`
  into the properties would have added ~35 %.
- **markdata per z6:** London about 5 MB (pts 1.2, props 1.3, info 1.8, fvals).
- **Speck cells at z5:** 274 k cells stand for 517 k points not kept (at 512² per tile; more at the
  1,024² chosen), at most 27.9 k per kind and tile.
- **The densest view in the browser** (zoom 5.75–6): 247–313 k points, where the whole files always
  held 584 k.
- **Server In view cost:**

| View | z6 tiles | Points | pts read | Cold from the NAS | Warm or mirrored |
|---|---|---|---|---|---|
| Europe at zoom 3–3.5 | 31–50 | ~410 k | 11 MB | ~0.5–1 s, plus ~0.5–1.5 s for the top items' props | ~10 ms |
| Globe | 251 | 584 k | 16 MB | | |

At world scale, a globe query would read 0.2–0.4 GB: a per-z3 summary then.

**Measured 2026-10-03:**
- **Setup:** bench Chrome, headless, 1512×900 @2, every landmark and overlay on, the same app.
- **The two catalogs:** by view (catalog 5) and whole files (catalog 3), with both servers reading the
  NAS without a mirror.

| | by view | files |
|---|---|---|
| heap after load (London z7 / Alps z8 tilted / Europe z4.5) | 74 / 53 / 54 MB | 186 / 232 / 213 MB |
| cold load: boot / map done / quiet, Europe z4.5 | 4.3–4.7 / 4.3–4.7 / 6.4–6.9 s | 4.9–5.2 / 5.7–5.9 / 7.1–7.4 s |
| the same, London z7 | 2.7 / 3.3 / 5.3 s | 2.4 / 4.7 / 6.0 s |
| the same, Alps z8 tilted | 2.9 / 2.9 / 5.1 s | 2.3 / 4.7 / 6.6 s |
| landmark and overlay data at load | 1.7–9.3 MB | 35.6 MB |
| pan / pinch / orbit, London z7 (fps, uncapped) | 215 / 177 / 173 | 220 / 176 / 192 |
| the same, Europe z4.5 | 194 / 84 / 143 | 183 / 88 / 152 |

## Risks

- **Zoomed out:** specks come from cells, and extras are capped. Check with screenshots at
  zooms 2–5, auto and locked, filtered and not.
- **Continental queries cold from the NAS:** the section caches and the mirror; zoomed out, the
  summaries in hidata (below).
- **Fame and isolation drift once recomputed:** the legacy-input regression comes first.
- **Peaks read z8 terrain worldwide:** cached on the build Mac.
- **Registers:** today's snapshot, imported by hand. Planned: fetched per jurisdiction, with stable
  record ids.
- **Fetch volume for ~1 M QIDs worldwide:** limited to the coverage's candidates; everything again at
  each pass (twice a year), only new QIDs in between.
- **`global/railfreq`** is another whole-world file the client loads: per-z6 blocks before going
  worldwide.

## Zoomed-out queries

Drives, rides and rail lines over many hi packs use summaries (plan §6 "Served").
- **Exact queries** read every z6 hidata within the view plus half the window length: the query
  sections `here`, `parts`, `psamples` and `pch`, 37 B per sample. A continental view read gigabytes
  cold.
- **Two changes took the warm CPU and the cold base-pack reads out:** the road-length prefilter
  (roads shorter than the window are skipped, with identical answers), and rail lines' identity in
  hidata (`railinfo`).
- **What summaries take out** is the bytes read.

The design was checked against a simulator on the 164 mirrored hidata tiles. A first design, 1 km
bins of every road in a per-z3 `lodata`, came out 4× its size estimate and missed its accuracy target
at the default 5 km; this one replaced it.

### Summaries: sections of hidata

pack(T) writes them into T's hidata, from the same parts it writes `psamples` from.
- So they're never older than the samples: there's no separate step, catalog field or staleness rule.
- hidata without them are queried exactly.
- Their builder is one pure function (`roadcore::lsum`: parts, samples, channels, `here` and
  `railinfo` in; the sections out), shared by pack(T) and the tests.

- **What:**
  - roads at least 2 km long (`LO_MIN_ROAD`, the shortest window answered from summaries: no window
    that long fits on a shorter road);
  - all rail (rail lines sum every run).
- **Bins:** consecutive samples of one part (one road inside T; parts already end at gaps over
  `GAP_M`, 300 m). A bin closes:
  - 500 m (`BIN_M`) after its first sample;
  - where the filters' attributes change (roads: class, unpaved, toll, unnamed);
  - for rail, at every way: one way per rail bin, so its trains a day and its line are the way's.
- **Sections:** roads in `lparts`/`lbins`, rail in `lrparts`/`lrbins`, since rides and rail lines
  read only rail's ~8 %.
  - hidata's meta says `"lsum": 1`, the format of these sections, so a later change can't be misread.
- **Records:** `LPart { u64 road; u32 first (bins index); u32 count; f32 road_len; u32 pad }` (24 B).
  `LBin` (64 B):
  ```
  LBin { u64 way (OSM id: roads the middle sample's way, rail the bin's way);
         f32 off0 (first sample's offset along the road); f32 len (last − first);
         i32 lon0, lat0 (first sample); i32 lonm, latm (middle sample: on `way`);
         i32 lon1, lat1 (last sample); u32 rinfo (rail: its `railinfo` row, else u32::MAX);
         u16 n (samples); u8 class; u8 flags (unpaved, toll, unnamed; rail 0);
         [u8; 12] comp (each component's mean over the samples × 255); [u8; 4] pad }
  ```
  - Roads: the 12 drive components.
  - Rail: the 11 ride components besides trains a day, which comes from `railfreq` at query time, by
    `way`. The grade term comes from each sample's neighbours within the part.
- **Size** (measured on the 164 mirrored hidata):
  - 4.9 M road bins, 0.42 M rail bins and 0.43 M parts: 349 MB, 10 % of the query sections and 7.5 %
    of hidata;
  - ~400 MB for all of today's 198 tiles;
  - the densest z3 tile's 64 hidata hold ~74 MB, against ~0.55–0.9 GB of query sections.
- **Rollout:** `PACK_V` 2 makes every pack(T) and lo job run again, which the cutover's build does
  anyway.

### Server

- **The switch is the client's.** It asks with `approx=1` when the view outline's bounding box is
  wider than 1,200 km, and goes back to exact below 900 km.
  - That's stable under panning, tile edges, coastline and pitch.
  - It follows how much the exact path would read: a pitched or large window at zoom 6 reads 1–2 GB
    of dense tiles.
- **The server's choice:**
  - It answers from summaries when every z6 hidata of the view plus margin has them (tiles with no
    hidata are skipped) and the window is at least `LO_MIN_ROAD` (rail lines: any).
  - Otherwise it answers exactly, never mixing, and every answer says which (`approx`).
- **Reads:**
  - the summary sections and, for rail, `railinfo` and `railstr`, whole and through the budgeted
    section cache (`Sect::all`), never `parts`, `psamples` or `pch`;
  - drive names still come from the top hits' ways (the ways-here index and base packs, paged).
- **Runs:** bins of one road in offset order across the tiles read. A run splits where a bin's `off0`
  is more than `GAP_M` past the running maximum of the run's last offsets (a road zigzagging over a
  tile edge has bins that overlap).
- **Windows (drives, rides):**
  - Each bin stands for `n` samples spread evenly from `off0` to `off0 + len`, each with the bin's
    score (its component means, the same weights, clamped per bin).
  - A run's pseudo-samples are merged in offset order, so overlapping bins interleave.
  - The best-window scan runs on them: at least the length asked, the mean over samples, the middle
    one in view, its position on its bin's first → middle → last polyline.
  - So windows start and end inside bins, and means are weighted by samples.
- **The answer:**
  - `length_m`: from the window's end offsets;
  - `geom`: real sample positions only (the bins' first, middle and last samples, the window's ends
    at the nearest of them), since a chord can leave a hairpin road;
  - `way`, `at`: the first bin's (`lonm`, `latm` is on `way`);
  - names: from the middle bin's way, for the top hits only;
  - `parts`: the component means weighted by samples.
  - **Rides also give:**
    - `name`: the line identity of the run's first bin;
    - `rel`, `services` and `colour`: the middle bin's `railinfo` row;
    - trains a day: the maximum over the window's bins' ways.
- **Rail lines:**
  - A bin is in view when its middle sample is.
  - A line's length is the sum over its bins in view of `next.off0 − off0` within a run (a run's last
    bin: `len`), and its score is weighted by those lengths.
  - `at` is the first bin's first sample; `geom` the bins' sample positions, a polyline per stretch
    in view.
- **Cancelled queries stop.** When the client aborts, hyper drops the handler's future. A guard in it
  sets a flag the computation checks before each tile read, in its parallel loops and before each
  name lookup (`spawn_blocking` work isn't stopped by the drop itself).

### Client

- **One mode** for the three panes (drives, rides, rail lines), set from the outline's size with the
  hysteresis above, starting by the band's middle (1,050 km). It's part of each pane's request key.
- **Lists from summaries say so** ("≈" by the count).
- **A pick from one doesn't pin its geometry:** the stretch is cut from the road's profile, as a
  link's is.

### Accuracy and checks

Measured with this model against the exact answers on the mirrored hidata: bins per z6 part, 500 m,
rail per way, u8 components, pseudo-samples in offset order; 9 views × 3 presets, so 27 cases per
length.

| | 2 km | 5 km | 10 km | 25 km |
|---|---|---|---|---|
| drives: top-20 overlap, lowest (cases ≥ 90 %) | 0.80 (23) | 0.90 (27) | 0.90 (27) | 0.95 (27) |
| drives: largest score error, top 20 / top 30 | 2.5 / 2.5 | 1.5 / 1.5 | 0.8 / 0.8 | 0.4 / 0.4 |
| rides: top-20 overlap, lowest (cases ≥ 90 %) | 0.85 (26) | 0.85 (26) | 0.95 (27) | 0.95 (27) |
| rides: largest score error, top 20 | 1.2 | 0.6 | 0.3 | 0.1 |

- **Drives filtered** (tertiary to trunk, no toll, no unnamed): every case at 90 % or more.
- **Totals:** within 0.06 %.
- **Rides' trains a day:** the same for 2,125 of 2,136 matched hits. A window ending inside a busier
  way's bin takes its trains: at most 2.4×.
- **Rail lines:**
  - km in view within 0.02 %;
  - a long line's length within 0.2 %, a short one's (2–6 km, at the view's edge) within 8 %;
  - scores within 1.2 points;
  - the top 40 overlapping 98–100 %.
- **Speed** (warm, Western Europe): drives at 5 km went from 372 to 80 ms, and bytes read from 1,988
  to 166 MB (rail 20 MB).

**The test** (`cargo test --release -p server approx_vs_exact -- --ignored`):
- **Inputs:** summaries built from today's hidata by the shared builder; the same 9 views; 2/5/10/25
  km; three presets; drives, rides and rail lines. Rides treat each run as its own line, since the
  mirrored hidata have no `railinfo`.
- **Bounds asserted** (looser than the table, so runs don't flake):

  | | 2 km | 5 km | 10 km | 25 km |
  |---|---|---|---|---|
  | drives: lowest top-20 overlap | ≥ 0.75 | ≥ 0.9 | ≥ 0.9 | ≥ 0.95 |
  | drives: largest score error | ≤ 4.5 | ≤ 2.0 | ≤ 1.0 | ≤ 0.6 |
  | rides: lowest top-20 overlap | ≥ 0.75 | ≥ 0.85 | ≥ 0.9 | ≥ 0.95 |
  | rides: largest score error | ≤ 2.0 | ≤ 1.0 | ≤ 0.5 | ≤ 0.3 |

  Rail lines: km within 0.5 % and a top-40 overlap of at least 0.9; totals within 1 %.
- **Not tested:** filtered drives.
