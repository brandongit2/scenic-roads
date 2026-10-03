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
| OSM pass, three more sets | the filtered planet (kept on the NAS) | `marks`: the point kinds' objects with tags and positions (ways and areas as points by today's rules); `summits`: natural=peak and volcano nodes with `ele`, worldwide; `named`: today's heritage filter (historic, heritage, museum/attraction/viewpoint, lighthouse, station, church and place of worship, protected area and park, military; Makefile:98–100) plus `ref:whc` and `heritage:operator=whc`, for locating register records |
| `registers` (network; twice a year) | the register modules whose areas meet the coverage (whole jurisdictions); UNESCO's list and the World Heritage items; the special-area lists; the register-id → QID tables (Wikidata, per register property) | `sources/registers/<d>/<module>/…` (raw) |
| `heritage` | registers, `named`, the `outlines` set (provinces), coverage | `work/heritage/<d>/`: located, filtered points with tiers and records, per z6 slice; heritage areas. heritage.py's rules: the municipal dedupe, federal.py's locating, and `covered()`, which becomes "within the coverage + 20 km" (today it tests the analysis grids, which base(U) makes later) |
| `overlays` (geometry) | the `areas` and `named` sets; the kept filtered planet, for World Heritage parts by QID (as whsshapes.py does on merged.osm.pbf); heritage areas, special lists, UNESCO, coverage | `ov-*` packs; `grid-areas` near the coverage; `work/whs-sites` |
| `items` (network; per pass) | the QIDs of the `marks` set within the coverage, the `areas` set, heritage records (`work/heritage`), UNESCO | `sources/items/<d>/facts`, `sources/pageviews/<seasons>/views` |
| `unit`, base(U) | as now, plus the heritage points within U + 500 m (for road flags; today's converted `heritage.json` until `heritage` runs) | the base pack |
| `unit-marks` | U's slice of `marks`, the worldwide `summits` set, U's piece (trailheads: hiking-route ends), coverage, terrain z12 within U + 30 km and z8 worldwide (cached on the build Mac; isolation searches up to 5,000 km) | `work/marks/<u>`: candidates (key, kind, position, name, en, ele, OSM id, QID, kept tags, peak result) |
| `marks` (worldwide) | every `work/marks/*`, `work/heritage`, facts, pageviews, `work/whs-sites` | `marks-*` packs, `markdata`, `global/marks/summary` |
| `ovdata` | the overlays' areas, facts | `ovdata` |
| `stations` | the `rail` set, coverage | `stations` packs |
| `ferries` | the `ferries` set, GTFS and the hand timetables, coverage | `ferries` packs |

Each job's key is its step version plus the content names of what it reads.

**Agent order:**
1. pass; registers whenever stale (network);
2. heritage, then items (network);
3. terrain, slope, overlays;
4. unit, unit-marks;
5. pack, lo, roots;
6. marks, ovdata, stations, ferries;
7. catalog.

**Determinism across units:**
- **Peaks:** prominence and isolation come from the worldwide `summits` set. Every tagged summit
  raises its pixel wherever its unit is, and ties go by id. The result doesn't depend on unit
  borders or the coverage (plan §6).
- **Neighbourhood rules** (trailheads within 150 m): run on U plus the piece's buffer in a global
  order by key. A kept point belongs to the unit that owns it, so neighbouring units agree.
- **The municipal heritage dedupe (40 m):** runs worldwide in `heritage`, ordered by key.

**Where today's steps go:**

| Today | New job | Language |
|---|---|---|
| extract.rs POIs | `marks` set, `unit-marks` | Rust, today's rules, keeping OSM ids and tags (no 60 m matching) |
| poidetails.py | `marks` (kept tags and facts) | Rust |
| peaks.rs | `unit-marks` | Rust |
| heritage.py, heritage_eu.py, federal.py, crhp.py, heritagewd.py's matching | `heritage` | Python on staged inputs, like landcover.py and labels.py. The registers become modules declaring their jurisdictions (today NRHP covers 7 states, hard-coded) |
| heritagewd.py's fetches, pageviews.py | `items` | Python |
| heritagedetails.py, interest.py, layers.py, filterprops.py | `marks` | Rust, sorted so it's deterministic, rounding as Python does (half-even on the exact binary value; mz from the unrounded ia), distances across the antimeridian |
| whsshapes.py | `overlays` | Python, on the `named` set |
| areadetails.py | `ovdata` | Python |
| stations.py | `stations` | Rust |
| ferries.py, gtfs, hand timetables | `ferries` | Python |

## Storage

- `markdata/` and `ovdata/` are catalog maps (formats.md), so GC handles them.
- `work/` holds build intermediates (`work/marks/<u>`, `work/heritage/<d>`, `work/whs-sites`). The
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
4. **The inputs for today's coverage**, compared with today's files:
   - the pass's `marks`, `summits` and `named` sets, from the kept filtered planet;
   - `registers`, `heritage` and `items`.
5. **The new jobs:**
   - `unit-marks` and `marks` in Rust, with the legacy-input regression;
   - `overlays`, `ovdata`, `stations` and `ferries`;
   - a pilot new region.
6. **Agent keys, order and waves.**

## Risks

- **Zoomed out:** specks come from cells, and extras are capped. Check with screenshots at
  zooms 2–5, auto and locked, filtered and not.
- **Continental queries cold from the NAS:** the section caches and the mirror; a per-z3 summary if
  measurements need it.
- **Fame and isolation drift once recomputed:** the legacy-input regression comes first.
- **Peaks read z8 terrain worldwide:** cached on the build Mac.
- **Registers fetched whole:** format changes and id stability (module tests pin record ids).
- **Fetch volume for ~1 M QIDs worldwide:** incremental, and limited to the coverage's candidates.
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
(older packs) are queried exactly.
- **What:** roads at least 2 km long (`LO_MIN_ROAD`, the shortest window answered from summaries:
  no window that long fits on a shorter road), and all rail (rail lines sum every run).
- **Bins:** consecutive samples of one part (one road inside T). A bin closes 500 m (`BIN_M`)
  after its first sample, at a gap over `GAP_M` (300 m), where the filters' attributes change
  (roads: class, unpaved, toll, unnamed), and for rail at every way (one way per rail bin: its
  trains a day and its line are the way's).
- `lparts`: `LPart { u64 road; u32 first (lbins index); u32 count; f32 road_len; u32 pad }` (24 B).
- `lbins`: `LBin` (64 B):
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
- **Size:** ~4–5 M bins for today's coverage, ~300 MB (~8 % of hidata); the densest z3 tile's 64
  hidata hold ~50 MB of summaries against ~0.85 GB of query sections.

### Server

- **The switch is the client's:** it asks with `approx=1` below zoom 5.5 and goes back to exact
  above 6 (the hysteresis keeps a panned or resized view from flipping modes; tile edges, coastline
  and pitch don't move it). The server answers from summaries when every z6 hidata of the view
  plus margin has them and the window is at least `LO_MIN_ROAD` (rail lines: any), else exactly,
  never mixing; every answer says which (`approx`).
- **Reads:** `lparts` and `lbins` whole (and `railinfo`, `railstr` for rail), never `here`,
  `parts`, `psamples` or `pch`.
- **Runs:** bins of one road in offset order across the tiles read; a run splits where a bin's
  `off0` is more than `GAP_M` past the running maximum of the run's last offsets (a road zigzagging
  over a tile edge has bins that overlap).
- **Windows (drives, rides):** each bin stands for `n` samples spread evenly from `off0` to
  `off0 + len`, each with the bin's score (its component means, the same weights, clamped per bin);
  today's best-window scan runs on these (at least the length asked, mean over samples, the middle
  one in view, its position on its bin's chord). So windows start and end inside bins, and means are
  weighted by samples.
- **The answer:** `length_m` from the window's end offsets; `geom` from the window's end positions
  (on their bins' chords) and the bins' first and last samples between; `way`/`at` the first bin's
  (`lonm`, `latm` is on `way`); names from the middle bin's way (as today, top hits only); `parts`
  the component means weighted by samples; rides' trains a day the maximum over the window's
  bins' ways.
- **Rail lines:** a bin is in view when its middle sample is; a line's length is the sum over its
  bins in view of `next.off0 − off0` within a run (a run's last bin: `len`), its score weighted by
  those lengths; `geom` the bins' sample positions, a polyline per stretch in view.
- **Cancelled queries stop:** the request's future dropping (the client aborted a superseded
  query) sets a flag the computation checks between phases and in its loops.

### Client

- `approx=1` by zoom with hysteresis (above); lists from summaries say so ("≈" by the count, and
  lengths rounded to 0.5 km).

### Accuracy and checks

Measured by the review's simulator with this bin model (500 m, windows inside bins), against the
exact answers on 9 views × 3 presets:
- windows of 5 km and longer: the top 20 overlap by 90 % or more in every case, scores within
  1.5 points;
- 2 km: 23 of 27 cases at 90 % (the lowest 80 %), scores within 3.7 points.

The test (`approx` against exact on the same hidata, a dozen continental views, 2/5/10/25 km, three
presets, drives and rides; rail lines' totals and lengths) holds the implementation to those, and
totals within 5 %.
