# Phase 5: landmarks, stations and overlays by view

Design for plan §10 phase 5's landmarks, stations and overlays, and base(U) step 5 ("POIs,
heritage, details, peaks"), from a survey of today's client and legacy pipeline (2026-10-03).
**Version 3**, after the Opus review of 2026-10-03 and its re-check; the findings are marked where
resolved ([C1], [I3], …). The zoomed-out drive and ride summaries, the rest of plan phase 5, are designed on their
own (end of this file). Amends plan §6 (pack(T) no longer writes landmark and station tiles, the lo
packs no longer hold top landmarks or per-cell counts: their own jobs do; the OSM pass gains sets);
recorded in §13.

## Why

Today the client fetches whole worldwide files on first use (`/api/layer/<name>`: stops and sights
per kind, heritage 61 MB, heritage areas, Indigenous lands, special areas, World Heritage outlines,
summits, stations, ferries) and its landmarks worker indexes them to compute the In view
statistics, the dots' layout and MapLibre point tiles (`lmk://`). That can't scale past today's
regions (a worldwide heritage file alone would be gigabytes), and new regions get none of these:
the files are today's, converted.

## What changes for the user

- Today's regions look and answer the same: dots, names, popups, the In view numbers and
  histograms. The golden test holds the In view answer to the legacy worker's exactly.
- Zoomed out (below zoom 6), the specks (dots below the size range) are drawn from counts per cell
  made at build time, at zooms 6 and up from every point as today. The sized dots and names are
  the same.
- One deliberate limit: zoomed out with a locked range (auto off) low enough to size more dots than
  the tiles hold, the 5,000 most prominent of the rest are sized and the others show as specks.
  Today every one is sized (up to ~300 k).

## Ids [C1]

One id per landmark, unique within its group (the point kinds together; each area overlay;
stations), below 2^52, so it's exact as a JS number, a vector-tile feature id and a MapLibre
feature-state id.

- An OSM object the point is: `id × 4 + type` (node 0, way 1, relation 2).
- Otherwise `2^51 + h`, h the first 51 bits of xxh3-64 of a reference:
  - trailheads (hiking-route ends): `trail:<relation id>:<node id>`;
  - register records: `reg:<module>:<record id>` where the module declares a stable record id (a
    field, or URLs that each name one record: a per-module allow-list); else
    `reg:<module>:<name>|<designation>|<lon E7>,<lat E7>`;
  - World Heritage: the site's dot `whc:<site id>` (a single-component site's point too);
    components `whc:<component id>` (`874-012`);
  - today's converted points: the OSM id from details-poi when no other converted point of the
    group has it (114 repeat one today, e.g. n4281698882 twice); else
    `legacy:<layer>|<kind>|<lon E7>,<lat E7>|<name>|<url>`.
- Stations: a merged stop (stations.py merges members by name within 800 m) takes its lowest
  member's `id × 4 + type`.
- A reference occurring more than once gets `#2`, `#3`, … in the byte order of its records'
  canonical JSON. Exact duplicates are kept that way, so today's counts stay.
- Then the ids are sorted and checked. A clash (different references, one id) moves the reference
  later in byte order to `<reference>#h1` (`#h2`, …) until none remain, and the job asserts
  uniqueness.
- Nothing stores ids across catalogs (URL state holds only `s` and `st`; descriptions are keyed by
  QID or OSM id), so a rebuild changing one breaks nothing.

## Points: the 8 kinds

Viewpoint, peak, waterfall, lighthouse, covered_bridge, rest (rest_area and picnic_site),
trailhead, heritage.

### Stored

- **`markdata/6-x-y`** (sectioned file per z6 tile: every point positioned in it, all kinds, sorted
  by kind then id; a catalog map like `base`):
  - `kinds`: per kind, first row and count;
  - `ids`: u64;
  - `pts`, 28 B per point:
    - lon, lat: i32 E7;
    - fa, ia, mz: f32 (ia 20000 when unknown, as the worker reads it; mz NaN when none);
    - rank: u32, the point's place in its kind's order (today: the legacy file's; the `marks` job:
      its sorted output), the tie-break [I3];
    - kz: u8, the lowest zoom whose thinned tile keeps the point (6: none);
    - class: u8 (heritage: level class + 3 × group, as dotData);
    - tier: u8 (heritage);
    - flags: u8 (named; World Heritage component; picnic site);
  - `fvals`: per kind, its range filters' values as f64 columns, NaN unknown: the JSON doubles
    exactly [I3];
  - `props`: the lean properties (today's names, plus `id`), JSON per point, zstd blocks of 256;
  - `info`: popup records, likewise;
  - `summits`: the named peaks with a height, as the summits list has them (lon, lat at 5
    decimals, ele rounded half-even, name), with their place in the worldwide list (height, then
    fame), for the highest named peak in view.
- **`layers/marks-<kind>/{root,lo}`** (packs; root z0–2, lo z3–5 per z3): thinned tiles in the
  marks tile format.
  - Kept at zoom z, per kind and tile: the named points with mz ≤ z − 3; the top 256 by score at
    balances 0, ¼, ½, ¾ and 1; the top 64 by each range filter except years (largest first).
    Score and value ties go by rank everywhere, so each rule is monotone in z and "kept at z" is
    "kz ≤ z". World Heritage components aren't kept (they're drawn close in, from z6 blocks).
  - The rest, as speck cells at zoom z + 10 (1,024² per tile, covering a 2× screen at fractional
    zooms): cell, count; heritage per (cell, tier).
- **`global/marks/summary`**: totals per kind, heritage tier and area overlay, for the Layers
  panel.

### The marks tile format

What the client reads, for thinned tiles, z6 blocks and `extra`. It's not MVT: positions are exact
(E7), values are typed arrays straight from the bytes, and the worker never hands these to MapLibre
[I2, I9].

- Header `RDMT`, then version, n points, nf filter values, n cells.
- Columns, each 8-byte aligned:
  - ids: f64;
  - fvals: f64 × n per field;
  - lon, lat: i32 E7;
  - fa, ia, mz: f32;
  - rank: u32 (the draw order's and specks' tie-break, as today's file order);
  - kz, class, tier, flags: u8;
  - cells: code (u32, the cell's Morton code within the tile), count (u32), tier (u8);
  - props offsets: u32 × (n + 1).
- props: a lean JSON object per point, back to back, so one point's props parse alone [I9].

The server attaches the display names (`main`/`sub`; `cmain`/`csub` from a World Heritage
component's `cn`) when serving, with ETags following the translations of the tile's areas, as
`names::mvt` does for labels. Served gzip'd.

### Served

- **`GET /api/marks/tile/{kind}/{z}/{x}/{y}`** (z ≤ 5): a thinned tile.
- **`GET /api/marks/block/{kind}/6/{x}/{y}`**: every point of the kind in the z6 tile, from
  markdata, no cells.
- **`GET /api/marks/specks/{kind}/{z}/{x}/{y}?q=…`** (z ≤ 5; the kind's filters, unknowns and
  switched-off tiers): the speck cells of the points passing them that the tile doesn't keep.
  From markdata over the tile; cached per tile and query [I1].
- **`POST /api/marks/view`**: today's worker `query`, in Rust over the `pts` of the z6 tiles
  meeting the outline. It returns the same JSON as the worker's `result`:
  - the 512-bin score histogram and the scores at the given ranks;
  - per kind, its count and best-known named point;
  - the top 60 overall and per kind;
  - the open filters' histograms;
  - the highest named peak, from `summits`, whatever the filters and the Peaks switch (as the
    worker does).

  World Heritage components are skipped, as the worker does.

  It also returns `extra` [I1]. The request carries `tz`, the zoom of the tiles the client shows
  (6 for blocks), the range when it's locked, and the ids of the extras the client already holds.
  `extra` is then the points sized at this view's range that the tiles at `tz` don't keep
  (kz > tz), less those the client holds:
  - the range is spreadRange of the scores at the ranks, or the locked range;
  - at most 5,000 in all, the most prominent first;
  - each in the tile format's fields, as JSON;
  - usually 0–20 at the default ranks; with ranks of 1,000, 1.5–3 k at first, then only those
    coming into view.
- **`GET /api/marks/count?kind=…&q=…`**: worldwide filtered counts (per catalog and query,
  cached).
- **`GET /api/marks/detail/{kind}/{id}?at=lon,lat`**: the popup record (markdata's `info` in the z6
  tile of `at`), with descriptions laid over as now. A small worldwide index finds points whose
  position follows the coverage (World Heritage dots move with their components here).
- Requests carry the catalog's marks version. A mismatch (a catalog switch mid-request) answers
  409, and the client asks again after switching.
- While the catalog lists `global/legacy/*`, `/api/layer/*` and `/api/detail/{layer}/{i}` stay.

### Exactness [I3]

The golden test compares the server's answer with the legacy worker's code, run under Node on
today's files with the same requests (several places, zooms, balances, ranks, filters and tier
switches). They must be exactly equal:
- **Score:** `(1 − b)·min(1, fa/5) + b·clamp((log10(max(0.05, ia)) + 1.3)/5.6, 0, 1)` in f64 from
  the f32 fa and ia, as the worker computes it. A test compares the server's log10 with V8's on
  every distinct ia of the data; if any differs, the server uses a port of V8's ieee754 log10.
- **Degrees:** from E7 as `e7 as f64 / 1e7`. A division gives back the JSON double for up to 7
  decimals; a multiply by 1e-7 can be off by one ulp.
- **Outline and bounds:** parsed as f64, not truncated; across the antimeridian (west > east) as
  in the worker.
- **Ties:**
  - the best per kind is the highest fa, then the lowest rank;
  - the top lists go by score, then the kind's place in the request, then rank (the worker's
    stable insertion);
  - the scores at ranks come from the scores rounded to f32 (the worker's Float32Array sort);
  - the summit is the first in view in the summits list's own order (height descending as
    stored, then fame ascending).
- **Filters and histograms:** over the f64 values, binned as filterHists.

## Areas: heritage areas, Indigenous lands, special areas, World Heritage outlines

- **`layers/ov-{heritage-areas,indigenous,special,whs}`** (root, lo; hi z9–12 within the coverage
  + 20 km): MVT, simplified per zoom and clipped, the properties plus `id` and `own` (the z3 tile
  holding the details). World Heritage features carry the site dot's id and position.
- **`ovdata/3-x-y`**: sectioned; area details by id (owned by the z3 tile of the area's
  representative point), and park records (name, bbox, tags, Wikidata).
- **Routes:** `GET /api/overlays/detail/{layer}/{id}?own=3/x/y` (`/api/areas/` is the Regions
  panel's); `/api/park` as now.
- **Client:**
  - vector sources;
  - the area histograms from `querySourceFeatures`, de-duplicated by `id` (today `p.i`,
    overlays.ts:480);
  - click dedup by `id`.

## Stations [I2]

- **`layers/stations/{root,lo,hi}`:** MVT, layer `s` (`n, en, g, m, sp, mz, id`).
  - Zooms 0–11 keep the stops with mz ≤ z − 2.58.
  - Zoom 12 is complete within the coverage, so overzoomed positions are within about 1 m
    (MapLibre holds 8,192 units a tile). A z9 tile would leave them about 6 m out at 51° N.
- **Client:** a vector source; feature-state and `querySourceFeatures` with `sourceLayer: 's'`
  (stations.ts:41, 82, 94).

## Ferries

- **`layers/ferries/{root,lo}`:** gzip'd GeoJSON blocks: z0 simplified to 5 km, z3 to 300 m, z6
  full. Each block holds the ferry ways touching it, their terminals and their lines' records.
- **Client:** ferries.ts merges blocks by id.
- **In-view kilometres and histograms:** each way carries its full length, and its share in view is
  measured on the geometry loaded, so the numbers stay within 0.5 % of today's zoomed out.

## Server, generally

- Tiles with names: one handler generalised from the labels', a rule per layer; the marks tile
  format has its own attacher.
- The tile rewrite cache is bounded by bytes, not entries: a z6 block reaches 1.6 MB.
- markdata and ovdata are typed section views, paged from the NAS and mapped from the mirror.

## Client

| Today | After |
|---|---|
| a kind's file loaded whole into the worker | per kind, tiles: z6 blocks from zoom 6 (±0.25 hysteresis), thinned tiles at floor(zoom) below; held as typed arrays and raw props (decoded for a lmk tile, a popup or a list); the least recently used dropped past 256 MB [I9] |
| the worker's `query` | `/api/marks/view`, asked and parsed by the worker; `applyResult` unchanged; `extra` joins its kind's points, de-duplicated by id |
| the summits file | the view's highest named peak |
| `count`, tier counts, `layer-summary` | `/api/marks/count`; totals from `global/marks/summary` |
| a source's dot layout | per kind and z4 chunk over the loaded tiles and extras: a tile or extra arriving lays out only the chunks it touches (one buffer per kind, a range per chunk), and the order within a chunk is today's (fame, then rank), so the draw order and the zoom-6-and-up specks are today's |
| — | below zoom 6, speck cells become pseudo-points: fa 0 and ia 0.05 (score 0 at any balance); the cell's count weighs it in visWords (today `count++`); class "the rest" in its tier's group colour (heritage), else 0 |
| filter masks per source | per kind, as now; below zoom 6 a filtered kind's cells come from `/api/marks/specks`, its unfiltered cells hidden until they arrive [I1] |
| `lmk://` tiles from the whole index | from the loaded points and extras, the same caps; `TileNames.ids` a Float64Array |
| area overlays as GeoJSON | vector sources |
| `stations.json` | a vector source |
| ferries files | blocks for the view, merged by id |
| details by `i` | marks by `{kind, id, at}`, areas by `{layer, id, own}`; cache keys `mark:` and `area:` (the legacy `layer:i` keys can't collide with them) |

Both formats work while today's converted files exist: the client uses the new layers when the
catalog lists them.

## Build [C2, I4, I5, I7]

The jobs form a chain without cycles: every input exists before its reader runs.

| Job | Reads | Writes |
|---|---|---|
| OSM pass, four more sets | the filtered planet (kept on the NAS) | `marks`: the point kinds' objects with tags and positions (ways and areas as points by today's rules); `summits`: natural=peak and volcano nodes and ways, with or without `ele`, worldwide; `hikes`: hiking and foot route relations with their member ways; `named`: today's heritage filter (historic, heritage, museum/attraction/viewpoint, lighthouse, station, church and place of worship, protected area and park, military; Makefile:98–100) plus `ref:whc` and `heritage:operator=whc`, for locating register records |
| `trailends` (per pass) | the `hikes` set | `work/trailends/<d>`: every simple linear hiking route's two ends worldwide (way ends used once, exactly two), with the route's name and relation id |
| `terrain-z8` (once; network) | AWS's raw z8 tiles (the build Mac's raw-tile cache) | `sources/terrain-z8` (not served): every z8 tile repaired (`terrain_pack::process` with no children, so no coverage in it), with each tile's maximum |
| `summits` (per pass) | the `summits` set, `sources/terrain-z8` | `work/summits/<d>`: every summit (OSM id, E7 position, kind, `ele` as the candidates have them), sorted by id, and the z8 overlay (below) |
| `registers` (network; twice a year) | the register modules whose areas meet the coverage (whole jurisdictions); UNESCO's list and the World Heritage items; the special-area lists; the register-id → QID tables (Wikidata, per register property) | `sources/registers/<d>/<module>/…` (raw) |
| `heritage` | registers, `named`, the `outlines` set (provinces), coverage | `work/heritage/<d>/`: located, filtered points with tiers and records, per z6 slice; heritage areas. heritage.py's rules: the municipal dedupe, federal.py's locating, and `covered()`, which becomes "within the coverage + 20 km" (today it tests the analysis grids, which base(U) makes later) |
| `overlays` (geometry) | the `areas` and `named` sets; the kept filtered planet, for World Heritage parts by QID (as whsshapes.py does on merged.osm.pbf); heritage areas, special lists, UNESCO, coverage | `ov-*` packs; `grid-areas` near the coverage; `work/whs-sites` |
| `unit`, base(U) | as now, plus the heritage points within U + 500 m (for road flags; today's converted `heritage.json` until `heritage` runs) | the base pack |
| `pois` (per unit) | U's piece, `work/trailends/<d>`, the coverage near U | `work/pois/<u>`: U's candidates (below) |
| `peaks` (per unit; network) | `work/pois/<u>`'s peaks, `work/summits`, z12 within 30 km of each peak (the terrain packs, else the same tile from the raw-tile cache, processed alike), `sources/terrain-z8` | `work/peaks/<u>`: prominence and isolation by candidate key |
| `items` (network; per pass) | the QIDs of every current unit's `work/pois` (single-QID tags for facts, the first QID for pageviews), the `areas` set, heritage records (`work/heritage`), `work/whs-sites`, UNESCO | `sources/items/<d>/facts`, `sources/pageviews/<months>/views` |
| `marks` (worldwide) | the current units' `work/pois` and `work/peaks`, `work/heritage`, facts, pageviews, `work/whs-sites` | `marks-*` packs, `markdata`, `global/marks/summary` |
| `ovdata` | the overlays' areas, facts | `ovdata` |
| `stations` | the `rail` set, coverage | `stations` packs |
| `ferries` | the `ferries` set, GTFS and the hand timetables, coverage | `ferries` packs |

Each job's key is its step version plus the content names of what it reads.

**Agent order:**
1. pass, then the sets it lacks (`pass-sets`), trailends; registers whenever stale (network);
2. terrain-z8 once, then summits; terrain, slope;
3. heritage, overlays;
4. unit; pois;
5. peaks;
6. items (network: after the candidates and `work/whs-sites` exist);
7. pack, lo, roots;
8. marks, ovdata, stations, ferries;
9. catalog.

Network jobs (terrain, terrain-z8, peaks, items, registers): a tile or answer that can't be
fetched fails the job (retried later), never counts as "none".

**Determinism across units:**
- **Peaks:** a peak's result reads only what lies within 28 km of it at z12 and the worldwide z8,
  both the same whichever unit computes it and whatever the coverage (z12: the packs' tiles or
  the same raw tile processed alike; z8: one artifact). The summits near it count at their own
  heights, worked out the same way wherever they are; ties go by tagged height, distance, then OSM
  id (plan §6). A z12 tile AWS doesn't have (404: open sea) is sea level in every unit, never an
  upsampled ancestor.
- **Neighbourhood rules** (trailheads within 150 m): run on U plus the piece's buffer in one order
  (rank, then key), so neighbouring units agree on the points near their border. A kept point
  belongs to the unit that owns it. Hiking-route ends come from the whole route (`trailends`), not
  from the piece, where a route leaving and coming back would show other ends. (At the coverage's
  edge a point kept outside it can still drop one inside, and take its name: the clip comes
  after the dedup, so that every unit agrees.)
- **The municipal heritage dedupe (40 m):** runs worldwide in `heritage`, ordered by key.

**Where today's steps go:**

| Today | New job | Language |
|---|---|---|
| extract.rs POIs | `pois` (`work/pois/<u>`; extract `--candidates`), with the pass's `trailends` | Rust, today's rules, keeping OSM ids and tags (no 60 m matching) |
| poidetails.py | `pois` (kept tags, `length_m`, `viewpoint`, the re-kinds), `items` (facts), `marks` | Rust; Python for the fetches |
| peaks.rs | `peaks` (`pipeline::peaks`, the binary kept as a thin wrapper for today's builds) | Rust |
| heritage.py, heritage_eu.py, federal.py, crhp.py, heritagewd.py's matching | `heritage` | Python on staged inputs, like landcover.py and labels.py. The registers become modules declaring their jurisdictions (today NRHP covers 7 states, hard-coded) |
| heritagewd.py's fetches, pageviews.py | `items` | Python |
| heritagedetails.py, interest.py, layers.py, filterprops.py | `marks` | Rust, sorted so it's deterministic, rounding as Python does (half-even on the exact binary value; mz from the unrounded ia), distances across the antimeridian |
| whsshapes.py | `overlays` | Python, on the `named` set |
| areadetails.py | `ovdata` | Python |
| stations.py | `stations` | Rust |
| ferries.py, gtfs, hand timetables | `ferries` | Python |

### Steps 4–5: how the landmark jobs run (implementation notes, 2026-10-03, revised after review)

The first notes ran peaks inside the unit job on U's staged terrain. Reviewed (Opus): a flood near
U's border went on over coarse ancestors past the staged box, so results depended on where U
was; the z8 of the packs depends on the coverage (made again from z9 only where z9 exists); the
key couldn't name what was read; and extract's point order changed between runs. Revised:

- **Candidates, their own job per unit (`pois`)**, so that a change to their rules doesn't make
  every unit again (the unit's own extract keeps today's points: its view step reads the
  viewpoints for the road flags). `pois` runs extract on U's piece with `--candidates` and the
  pass's `trailends`, then writes `work/pois/<u>` (zstd JSON lines sorted by key); its key: its
  step version, U's piece, the coverage near U, `trailends`.
  - One order: by (trailhead rank, key, kind) before the 150 m dedup, so the kept point and the
    name it lends don't depend on thread timing; keys `n<id>`, `w<id>`, `trail:<relation>:<node>`,
    plus the kind (one way can be a point of interest and a covered bridge). Done 2026-10-03:
    Taiwan's three runs had differed by 92–123 trailheads; now byte-identical.
  - Hiking-route ends: the pass's `trailends` near the piece's roads (the 300 m road test stays
    per unit). Done 2026-10-03 (extract `--trailends`).
  - Clipped after the dedup to the points U owns that are in the coverage (nodes inside it; ways
    with a node inside it).
  - Kinds as today's map has them: natural=peak (nodes, ways' centres), and a viewpoint that is
    also a volcano re-kinded to a peak, as poidetails.py did (pure volcanoes aren't points: they
    are often a crater node beside the rim's peaks; they stay in the summits). In the candidates
    only, not in the unit's points.
  - Each candidate: `key, kind, lon, lat` (E7 integers), `name, ele, osm, qid` (the `wikidata` tag
    as tagged), `en` (name:en; in Japan name:ja-Latn or name:ja_rm, kept for it: the server shows
    it when there's no translation line), kept tags (poidetails' lists; not `image`, which today's
    details never had and which would move fame), and what poidetails.py added: `length_m` for
    covered bridges (its planar formula over the way's own nodes, rounded half-even),
    `viewpoint: "yes"` for peaks tagged tourism=viewpoint.
- **`trailends`** (per pass): the `hikes` set's routes (route=hiking or foot), each way's end
  nodes, the ends used once; exactly two make a simple linear route. Worldwide, so every unit sees
  the same ends of a route that leaves its piece. Done 2026-10-03, with the pass's sets versioned
  (osmpass::SETS: a changed filter is a new set, `summits-v2`) and `pass-sets`, which makes the
  sets a finished pass lacks from its kept filtered planet.
- **`terrain-z8`** (once, network): AWS's 65,536 raw z8 tiles, repaired as the packs' tiles are
  (`terrain_pack::process`, no children: the voids filled with 32,767 m and the spike clusters
  go), in one content-named pack under `sources/` (a `global/` file would be served and mirrored),
  with each tile's maximum. Coverage-free, so the isolation searches and the coarse floods give
  the same answer whatever is built. AWS's own z8 comes from a coarser source than today's
  (made again from z9 near roads), so before trusting the coarse values: raw z8 against today's z8
  over today's coverage, per pixel and at the cols and nearest higher ground of today's
  coarse-stage peaks. If they drift too far, the coverage-free fallback is z8 as the 2×2 means of
  processed raw z9 everywhere (today's rule, worldwide: 262,144 tiles to fetch). Measured
  2026-10-03 on 864 mountainous z8 tiles of today's packs (max ≥ 1,000 m; 51.7 M land pixels):
  today − raw per pixel, mean −0.1 m, |diff| median 0.0, p95 6.2, p99 34.5 m (max 1,514 m, a
  repair); each tile's maximum, today − raw: p5 −15.8, median and p95 0.0, extremes −103 and
  +200 m (a summit today's z9 means kept). Raw z8 it is.
- **`summits`** (per pass): the `summits` set (natural=peak or volcano, nodes and ways), sorted
  by OSM id. A summit has one identity: the same position and `ele` read as a candidate or as a
  neighbour (a node's position; a way's centre as extract makes it, the integer mean of its
  nodes with the closing node counted twice, truncated; `ele` parsed and rounded as extract does,
  by shared code). The z8 overlay: each summit's z8 pixel raised to its tagged `ele` when that is
  plausible there: at most 8,900 m, at most 1,500 m over the highest z8 pixel within one pixel,
  and at most twice that plus 300 m (feet tagged as metres overshoot by 2.28 times the height,
  which a fixed margin lets through below ~1,100 m); else left alone. Calibrated on today's
  coverage: the tags this accepts that the z12 check (below) rejects, counted.
- **`peaks`** (per unit, its own job, network): `pipeline::peaks`, today's peaks.rs as a library
  (done 2026-10-03: byte-identical to today's binary), on U's peak candidates.
  - z12: a tile is the terrain pack's when the manifest has it, else AWS's raw tile from the
    build Mac's cache processed the same way, read back from the PNG `process` returns (quantised
    as stored), never its floats; a tile not cached is fetched, and a failed fetch fails the job.
    z12 has no children, so the two are the same bytes for packs the terrain job made from raw.
    Today's packs came from the legacy terrain step, which repaired stored tiles again (not
    idempotent). Measured 2026-10-03 on 2,406 land z12 tiles of today's hi packs: 2,390
    byte-identical to the raw tile processed alike; the other 16 differ by under half a metre
    along coasts (bathymetry clamping) or in one to three single pixels by 126–303 m (spikes and
    pits one repair kept), which the peaks' own despike clamps either way; every tile's maximum
    equal. So no terrain rebuild first.
  - Summits near a peak: each summit within 28 km + 2 × (150 m + 2 pixels) gets its summit pixel
    (highest within 150 m), its height (max(ele, DEM) when `ele` is within −30/+200 m of the DEM's,
    else the DEM's) and its claim (several on one pixel: the highest tagged, then the nearest to
    it, then the lowest OSM id; the others start from their own point), all from z12 the same way
    for every unit.
  - Fine stage (z12, today's 600k-pixel flood): a flood that would read a pixel more than 28 km
    from the summit stops, and the peak goes to the coarse stage. The isolation search (25 km)
    counts only pixels within 25 km and never opens a tile whose great-circle lower bound is
    beyond, so its result depends only on z12 within 25 km. So U + 30 km of z12 always suffices.
  - Coarse stage (z8): the flood (40M pixels) and the isolation search over `terrain-z8` with the
    z8 overlay, where the summits within the fine radius (28.3 km) count at their z12 heights (the
    peak's own included: a tag over 200 m above its DEM would otherwise make its own pixel its
    higher ground), the tag rule beyond. Tiles in order of a great-circle lower bound (today's
    bound used the summit's metres per pixel, wrong toward the poles past ~1,000 km), x wrapping
    at the antimeridian, a tile skipped when neither its maximum nor its overlay is higher, tiles
    decoded through a bounded cache; `terrain-z8` and `work/summits` read from local
    content-named copies. Nothing higher within 5,000 km: a lower bound of the radius searched
    (today: 25 km, flagged).
  - "Sea" is ≤ 0 m, and `process` clamps every negative value to 0: polders and depressions count
    as sea (as today), until plan §6's sea-masked terrain.
  - Writes `work/peaks/<u>` (by candidate key: e, p, pl, c, ce, iso, il, hi); its key: the step
    version, U's candidates, `work/summits`, the terrain hi packs within U + 30 km, `terrain-z8`.
  - Done 2026-10-03 (pipeline::peaks::unit, `examples/peaks_check`, every one of today's peaks a
    summit): the Sierra Nevada's 8,235 peaks in 6.7 s, 8,225 identical to today's peaks.json; the
    others three pairs swapping a near-tied claim (the check's positions are today's floats, not
    OSM's E7) and two coarse cols 14 and 22 m higher; two overlapping halves agree with the whole
    run on every shared peak. Around famous peaks (the worldwide z8, the long searches): Mont
    Blanc, Fuji, Ben Nevis, Yushan, Robson and Washington keep today's height, prominence (Robson
    +10 m) and nearest higher pixel; isolations are a little shorter as great-circle distances
    (Fuji 2,081.8 → 2,076.3 km, Wikipedia 2,077; Ben Nevis 739.9 → 738.6, Wikipedia 739; Mont
    Blanc 2,828.0 → 2,804.7). Mont Blanc's flood spends its 40M pixels and stops at the same col
    as today's (128 m, a lower bound in both): 54 s, 3.4 GB.
- **`items`** (Python, network, per pass; dem/items.py): after the candidates exist. The QIDs of
  the current units' candidates (the heritage records' and areas' items stay with the heritage
  job until the registers are built here). Facts as poidetails.py fetches them, for single-QID
  tags (as today: a multi-QID tag gets no facts); pageviews for the first QID, the mean of four
  months pinned per pass: the last November, February, May and August that ended at least 20 days
  before the pass (their dumps are out). Everything is fetched again at each pass; between passes
  only QIDs it hasn't seen (a run started by new coverage doesn't move fame elsewhere). Each
  item's Wikipedia articles are listed again at each pass too (today's wp.jsonl never refreshes,
  so new articles never count); new articles between passes are batched (each run streams the
  four dumps again, ~20 GB). The caches are appended 5,000 items at a time, so a run stopped
  midway keeps what it fetched; a pageview month is cached only when curl, bzip2 and grep all
  finished cleanly. `sources/items/<d>/facts`, `views` and `meta` (the months and the first and
  last days anything was fetched: QLever's index is whatever it serves those days). User-Agent
  "road-elevations/0.1 (personal offline map)" (no contact address: identifying details stay out
  of requests), the APIs' rate limits; with no contact Wikimedia may refuse by User-Agent, so a
  403 or any failed batch fails the job loudly (today's heritagewd.shortdescs skips a failed batch
  silently).
- **`marks`** (Rust, worldwide): the current units' `work/pois` and `work/peaks` (from the
  manifest, not every file under `work/`) in one order by key → `marksjob::Candidate` (details:
  kept tags, `osm`, facts as `wd`, `length_m`, `viewpoint`) → `marksjob::poi_points` → with the
  heritage points → `markconv::write` (its summits list: by −ele, stable over fame order). An OSM
  id that repeats (a way that is a point of interest and a covered bridge) gets a reference from
  its key, as `assign_ids` needs. Written descriptions aren't read here (they are applied when
  serving, plan §7): a Wikipedia summary's source article goes in its description line, as `src`
  already does for researched ones, and the server's credit uses it, so records need no
  `long_src`.
- **Step 4's comparison, one change at a time:**
  1. Port: `pipeline::peaks` on today's archive and `pois.json` with today's overlay rule gives
     today's `peaks.json` byte for byte.
  2. Determinism: a unit run twice gives the same bytes; two neighbouring units agree on every
     point and peak within 10 km of their border.
  3. One planet: a region-sized extract and the union of its units' candidates are the same
     (counts, E7 positions, OSM ids, tags).
  4. Against today's files: nodes by exact E7 position, then by details-poi's `osm`; adds,
     removes and moves per kind, trailheads per source.
  5. Peaks: exact where both used the same z12 and finished in the fine stage (those that went
     coarse differ by design, reported apart); otherwise, of peaks with ≥ 50 m prominence, 95 %
     within |Δe| ≤ 30 m, |Δp| ≤ max(20 m, 10 %), |Δiso| ≤ max(0.5 km, 10 %), the rest listed;
     today's lower bounds (`pl`, `il`) checked as new ≥ old.
  6. marks: exact on today's inputs (marks_regression), then new candidates with today's facts
     and pageviews, then fresh facts: per kind the fa rank correlation, the top 100 per z6 tile
     (≥ 90 % the same), the mz and kz histograms, a dozen In view answers (top 60), screenshots.

### Heritage and area flags (proposed 2026-10-03, after the cutover starts; to be reviewed)

Today's heritage chain (heritage.py, heritagewd.py, heritagedetails.py, areadetails.py,
whsshapes.py, filterprops.py's heritage part, pageviews.py, interest.py's heritage part,
layers.py) runs unchanged as one `heritage` job, in a stand-in root laid out as the repository's
(`dem/` the app's scripts, `data/heritage/` the registers' snapshot, today's being the legacy
caches; `data/areas/areas.geojsonseq` from the pass's `areas` set; `data/heritage/osm/` from its
`named` and `outlines` sets; `data/osm/merged.osm.pbf` its kept filtered planet, which keeps every
`wikidata`-tagged object, for the World Heritage parts; no stops & sights, which are the marks
job's). Its outputs (heritage sites and areas, details, World Heritage outlines and sites, the
overlays' layers, area details) go to `work/heritage/<d>/`; markconv's heritage points and
ovconv's overlays and area details read them there instead of `global/legacy/`.

Two changes to heritage.py: what is "covered" is within the coverage + 20 km (a polygon file the
job writes), not today's analysis grid, which units make later; and it no longer rasterises the
areas onto a grid. The area flags (park, heritage area, special area, Indigenous land) are
rasterised per unit instead, onto the unit's own z11 grid, from the overlay polygons near it (the
`areas` set's parks and Indigenous lands, the heritage job's heritage and special areas), so
there's no worldwide `grid-areas` layer to keep in step: the unit key names those inputs, and the
heritage job runs before the units. Today's regions keep today's heritage and area grids until
then.

## Storage

- `markdata/` and `ovdata/` are catalog maps (formats.md), so GC handles them.
- `work/` holds build intermediates (`work/pois/<u>`, `work/peaks/<u>`, `work/trailends/<d>`,
  `work/summits/<d>`, `work/heritage/<d>`, `work/whs-sites`). The
  agent's GC keeps what the newest job keys name and removes the rest.
- Nothing in `work/` is in a catalog.

## Today's regions

1. `scenic-build convert-legacy-marks` writes markdata and the marks packs from `global/legacy/*`:
   the legacy fa, ia and mz, file order as rank, and ids as above.
2. Golden: the view answers equal the legacy worker's exactly, and details match by id.
   Screenshots match except the zoomed-out specks.
3. The client follows; both formats work until one release after cutover.
4. Overlays, stations and ferries are converted the same way.
5. The new jobs reproduce today's data:
   - first from the legacy inputs: `marks`, given today's candidates and pageview table, must
     reproduce fa, ia and mz exactly;
   - then from new inputs, compared by counts and distributions.

## Sizes (measured from today's files)

- **Points:** 582,665, plus 1,768 World Heritage components, in 251 z6 tiles. The densest tiles
  hold 47 k (Spain, peaks) and 43.9 k (London).
- **marks lo packs per z3:**
  - London: 6.4–8.6 MB of z6 blocks, plus the thinned z3–5 tiles;
  - largest tile: London's heritage block, 1.2–1.6 MB (no duplicated `id` property: it adds ~35 %).
- **markdata per z6:** London about 5 MB (pts 1.2, props 1.3, info 1.8, fvals).
- **Speck cells at z5:** 274 k cells stand for 517 k points not kept (at 512² per tile; more at
  the 1,024² chosen: measured in step 1); at most 27.9 k per kind and tile.
- **The densest view in the browser** (zoom 5.75–6): 247–313 k points. Today it always holds 584 k.
- **Server In view cost:**

| View | z6 tiles | Points | pts read | Cold from the NAS | Warm or mirrored |
|---|---|---|---|---|---|
| Europe at zoom 3–3.5 | 31–50 | ~410 k | 11 MB | ~0.5–1 s, plus ~0.5–1.5 s for the top items' props | ~10 ms |
| Globe | 251 | 584 k | 16 MB | | |

At world scale, a globe query would read 0.2–0.4 GB: a per-z3 summary then.

## Order [I8]

1. **Peaks end to end:**
   - formats;
   - `convert-legacy-marks` for peaks (and summits);
   - the server's tiles, blocks, specks, view query, count and details by id;
   - golden against the legacy worker;
   - the client for peaks, the other kinds staying on the legacy path;
   - screenshots.
2. **The other seven kinds** (heritage: tiers, World Heritage components, classes):
   - the same golden;
   - the client's legacy path goes when the catalog lists the new layers.
3. **Overlays, stations and ferries converted:**
   - the client's vector sources and ferry blocks;
   - counts and summary.
   - Done 2026-10-03 (`convert-legacy-overlays`, pipeline::ovconv): areas, details and parks,
     stations and ferries by view, the app switching on the catalog's layers (meta `ovTiles`,
     `stationTiles`, `ferryBlocks`); checked against today's files: the same stops drawn (London
     500, Paris 952), ferries' in-view km, routes and terminal colours equal, area popups with
     their details. Counts and the areas' summary stay today's (`layer-summary`).
   - Measured 2026-10-03 (bench Chrome, headless, 1512×900 @2, both servers reading the NAS without
     a mirror, every landmark and overlay on; by view = catalog 5, files = catalog 3, same app):

     | | by view | files |
     |---|---|---|
     | heap after load (London z7 / Alps z8 tilted / Europe z4.5) | 74 / 53 / 54 MB | 186 / 232 / 213 MB |
     | cold load: boot / map done / quiet, Europe z4.5 | 4.3–4.7 / 4.3–4.7 / 6.4–6.9 s | 4.9–5.2 / 5.7–5.9 / 7.1–7.4 s |
     | the same, London z7 | 2.7 / 3.3 / 5.3 s | 2.4 / 4.7 / 6.0 s |
     | the same, Alps z8 tilted | 2.9 / 2.9 / 5.1 s | 2.3 / 4.7 / 6.6 s |
     | landmark and overlay data at load | 1.7–9.3 MB | 35.6 MB |
     | pan / pinch / orbit, London z7 (fps, uncapped) | 215 / 177 / 173 | 220 / 176 / 192 |
     | the same, Europe z4.5 | 194 / 84 / 143 | 183 / 88 / 152 |
4. **The inputs for today's coverage**, compared with today's files:
   - the pass's `marks`, `summits` and `named` sets, from the kept filtered planet;
   - `registers`, `heritage` and `items`.
5. **The new jobs:**
   - the candidates in the unit job, `trailends`, `terrain-z8`, `summits`, `peaks` and `marks` in
     Rust, with the legacy-input regression (Step 4's comparison, above);
   - `overlays`, `ovdata`, `stations` and `ferries`;
   - a pilot new region.
6. **Agent keys, order and waves.**

## Risks

- **Zoomed out:** specks come from cells, and extras are capped. Check with screenshots at
  zooms 2–5, auto and locked, filtered and not.
- **Continental queries cold from the NAS:** the section caches and the mirror; zoomed out, the
  summaries in hidata (above).
- **Fame and isolation drift once recomputed:** the legacy-input regression comes first.
- **Peaks read z8 terrain worldwide:** cached on the build Mac.
- **Registers fetched whole:** format changes and id stability (module tests pin record ids).
- **Fetch volume for ~1 M QIDs worldwide:** limited to the coverage's candidates; everything again
  at each pass (twice a year), only new QIDs in between.
- **`global/railfreq`** is another whole-world file the client loads: per-z6 blocks before going
  worldwide.

## Zoomed-out queries [I6]

Plan §6 "Served": drives, rides and rail lines over many hi packs use summaries. Today's queries
read every z6 hidata within the view plus half the window length (query sections: `here`, `parts`,
`psamples`, `pch`, 37 B per sample), so a continental view reads gigabytes cold. The road-length
prefilter (2026-10-03: roads shorter than the window are skipped, identical answers) and rail
lines' identity in hidata (`railinfo`) took the warm CPU and the cold base-pack reads out; what's
left is the bytes read.

Reviewed (2026-10-03) against a simulator on the 164 mirrored hidata tiles: the first design (1 km
bins of every road in a per-z3 `lodata`) came out 4× its size estimate, missed its accuracy target
at the default 5 km (windows cut on bin edges), and its bin record couldn't support the joins. This
is the revision.

### Summaries: sections of hidata

pack(T) writes them into T's hidata, from the same parts it writes `psamples` from, so they're never
older than the samples (no separate step, catalog field or staleness rule); hidata without them
are queried exactly. Their builder is one pure function (parts, samples, channels, `here`,
`railinfo` in; the sections out), shared by pack(T) and the tests.
- **What:** roads at least 2 km long (`LO_MIN_ROAD`, the shortest window answered from summaries:
  no window that long fits on a shorter road), and all rail (rail lines sum every run).
- **Bins:** consecutive samples of one part (one road inside T; parts already end at gaps over
  `GAP_M`). A bin closes 500 m (`BIN_M`) after its first sample, where the filters' attributes
  change (roads: class, unpaved, toll, unnamed), and for rail at every way (one way per rail bin:
  its trains a day and its line are the way's).
- Roads in `lparts`/`lbins`, rail in `lrparts`/`lrbins` (rides and rail lines read only rail's ~8 %);
  hidata's meta says `"lsum": 1`, the format of these sections (a later change can't be misread).
- `LPart { u64 road; u32 first (bins index); u32 count; f32 road_len; u32 pad }` (24 B).
- `LBin` (64 B):
  ```
  LBin { u64 way (OSM id: roads the middle sample's way, rail the bin's way);
         f32 off0 (first sample's offset along the road); f32 len (last − first);
         i32 lon0, lat0 (first sample); i32 lonm, latm (middle sample: on `way`);
         i32 lon1, lat1 (last sample); u32 rinfo (rail: its `railinfo` row, else u32::MAX);
         u16 n (samples); u8 class; u8 flags (unpaved, toll, unnamed);
         [u8; 12] comp (each component's mean over the samples × 255); [u8; 4] pad }
  ```
  Roads: the 12 drive components. Rail: the 11 ride components but trains a day (from `railfreq` at
  query time, by `way`), the grade term from each sample's neighbours within the part.
- **Size** (measured on the 164 mirrored hidata): 4.9 M road bins, 0.42 M rail bins, 0.43 M parts,
  349 MB (10 % of the query sections, 7.5 % of hidata); ~400 MB for all of today's 198 tiles; the
  densest z3 tile's 64 hidata hold ~74 MB against ~0.55–0.9 GB of query sections.
- **Rollout:** `PACK_V` goes up with this change, so every pack(T) runs again (and every lo job,
  whose keys include the packs'), which the cutover's re-pack does anyway (railinfo came without a
  bump); the mirror copies the new hidata (~5.5 GB today).

### Server

- **The switch is the client's:** it asks with `approx=1` when the view outline's bounding box is
  wider than 1,200 km and goes back to exact below 900 km (stable under panning, tile edges,
  coastline and pitch, and it follows how much the exact path would read: a pitched or large window
  at zoom 6 reads 1–2 GB of dense tiles). The server answers from summaries when every z6 hidata of
  the view plus margin has them and the window is at least `LO_MIN_ROAD` (rail lines: any), else
  exactly, never mixing; every answer says which (`approx`).
- **Reads:** the summary sections and (rail) `railinfo`, `railstr`, whole and through the budgeted
  section cache (`Sect::all`); never `parts`, `psamples` or `pch`. Drive names still come from the
  top hits' ways (the ways-here index and base packs, paged), as today.
- **Runs:** bins of one road in offset order across the tiles read; a run splits where a bin's
  `off0` is more than `GAP_M` past the running maximum of the run's last offsets (a road zigzagging
  over a tile edge has bins that overlap).
- **Windows (drives, rides):** each bin stands for `n` samples spread evenly from `off0` to
  `off0 + len`, each with the bin's score (its component means, the same weights, clamped per bin);
  a run's pseudo-samples are merged in offset order (overlapping bins interleave), and today's
  best-window scan runs on them (at least the length asked, mean over samples, the middle one in
  view, its position on its bin's first → middle → last polyline). So windows start and end inside
  bins, and means are weighted by samples.
- **The answer:** `length_m` from the window's end offsets; `geom` real sample positions only (the
  bins' first, middle and last samples, the window's ends at the nearest of them: a chord can leave
  a hairpin road); `way`/`at` the first bin's (`lonm`, `latm` is on `way`); names from the middle
  bin's way (as today, top hits only); `parts` the component means weighted by samples. Rides:
  `name` the line identity of the run's first bin, `rel`, `services` and `colour` the middle bin's
  `railinfo` row, trains a day the maximum over the window's bins' ways.
- **Rail lines:** a bin is in view when its middle sample is; a line's length is the sum over its
  bins in view of `next.off0 − off0` within a run (a run's last bin: `len`), its score weighted by
  those lengths; `at` the first bin's first sample; `geom` the bins' sample positions, a polyline per
  stretch in view.
- **Cancelled queries stop:** when the client aborts, hyper drops the handler's future; a guard in
  it sets a flag the computation checks before each tile read, in its parallel loops and before
  each name lookup (`spawn_blocking` work isn't stopped by the drop itself).

### Client

- One mode for the three panes (drives, rides, rail lines), from the outline's size with the
  hysteresis above, starting by the band's middle (1,050 km); it's part of each pane's request key.
- Lists from summaries say so ("≈" by the count). A pick from one doesn't pin its geometry: the
  stretch is cut from the road's profile, as a link's is.

### Accuracy and checks

Measured with this model (bins per z6 part, 500 m, rail per way, u8 components, pseudo-samples in
offset order) against the exact answers, 9 views × 3 presets (27 cases a length), on the mirrored
hidata:

| | 2 km | 5 km | 10 km | 25 km |
|---|---|---|---|---|
| drives: top-20 overlap, lowest (cases ≥ 90 %) | 0.80 (23) | 0.90 (27) | 0.90 (27) | 0.95 (27) |
| drives: largest score error, top 20 / top 30 | 2.5 / 2.5 | 1.5 / 1.5 | 0.8 / 0.8 | 0.4 / 0.4 |
| rides: top-20 overlap, lowest (cases ≥ 90 %) | 0.85 (26) | 0.85 (26) | 0.95 (27) | 0.95 (27) |
| rides: largest score error, top 20 | 1.2 | 0.6 | 0.3 | 0.1 |

(The implementation's test, `cargo test --release -p server approx_vs_exact -- --ignored`;
rides with each run its own line, as the mirrored hidata have no `railinfo`.)

- Drives filtered (tertiary to trunk, no toll, no unnamed): every case at 90 % or more.
- Totals within 0.06 %; rides' trains a day the same for 2,125 of 2,136 matched hits (a window
  ending inside a busier way's bin takes its trains: at most 2.4×).
- Rail lines: km in view within 0.02 %; a long line's length within 0.2 %, a short one's (2–6 km,
  at the view's edge) within 8 %; scores within 1.2 points; the top 40 overlapping 98–100 %.
- Warm, Western Europe: drives at 5 km 372 → 80 ms; bytes read 1,988 → 166 MB (rail 20 MB).

The test (the summaries built from today's hidata by the shared builder, `approx` against exact, a
dozen continental views, 2/5/10/25 km, three presets, drives, rides and rail lines) holds the
implementation to these, and totals within 1 %.
