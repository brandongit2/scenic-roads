# Landmarks, stations, ferries and overlays by view

What the client loads for the view, and what the server answers: landmarks, stations, ferries and
area overlays; the landmark build jobs; the heritage chain; the zoomed-out drive and ride summaries
(end of this file). Worldwide files loaded whole couldn't scale, so the client loads what the view
needs and the server answers the In view statistics.

**What the user sees:** below zoom 6 the specks (dots below the size range) come from per-cell
counts made at build time; from zoom 6, from every point. A tilted view draws its far ground from
thinned tiles two zooms coarser. With a locked range low enough to size more dots than the tiles
hold, the 5,000 most prominent of the rest are sized and the others show as specks.

## Ids

One id per landmark, unique within its group (the point kinds together, each area overlay,
stations, ferries), below 2^52 so it's exact as a JS number, an MVT feature id and a MapLibre
feature-state id.
- **An OSM object the point is:** `id × 4 + type` (node 0, way 1, relation 2).
- **Otherwise** `2^51 + h`, h the first 51 bits of the xxh3-64 of a reference: candidates' key with
  the kind (`trail:<relation>:<node>:trailhead`, `n123:peak`); World Heritage dots `whc:<site id>`;
  Canada's federal designations `reg:dfhd:<id>`; other heritage points
  `legacy:heritage|<tier>|<lon E7>,<lat E7>|<name>|<url>`; areas the OSM id when unique, else
  `legacy:<layer>|<name>|<box centre E7>`; stations merged from several members their first
  member's `id × 4 + type`; ferry terminals `legacy:terminal|…`. (`legacy:` is only a prefix:
  changing it would change the ids.)
- **Repeats** get `#2`, `#3`, … in the byte order of their records' canonical JSON. **Clashes** move
  the later reference to `<reference>#h1` (`#h2`, …), and the job asserts uniqueness.
- Nothing stores ids across catalogs (URL state holds only `s` and `st`; descriptions are keyed by
  QID or OSM id), so a rebuild changing one breaks nothing.

## Points: the 8 kinds

Viewpoint, peak, waterfall, lighthouse, covered_bridge, rest (rest_area and picnic_site),
trailhead, heritage.

### Stored

**`markdata/all`** (docs/formats.md, "Bundles"): one bundle of a sectioned file per z6 tile, every
point in it, sorted by
kind then id: `kinds`, `ids`, `pts` (28 B: E7 position; fa, ia, mz as f32; rank, the point's place in
its kind's order and the tie-break; kz, the lowest zoom whose thinned tile keeps it; class, tier,
flags), `fvals` (per kind, an f64 column per filter property), `props` and `info` (JSON in zstd blocks
of 256), and `summits` (the named peaks with a height, with their place in the worldwide list).

**`layers/marks-<kind>/all`**: thinned tiles (z0–5) in the marks tile format, one bundle per kind
that the catalog names as the kind's root and lo packs.
At zoom z a tile keeps the named points with mz ≤ z − 3, the top 256 by score at balances 0, ¼, ½, ¾
and 1, and the top 64 by each range filter but years. Ties go by rank, so "kept at z" is "kz ≤ z".
World Heritage components are drawn from z6 blocks only. The rest are speck cells at zoom z + 10
(cell and count; for heritage per tier).

**`global/marks/summary`**: `{fmt, kinds, tiers}`; the area overlays' totals come from the overlays
job's `layer-summary`.

### The marks tile format

For thinned tiles, z6 blocks and `extra`; not MVT, and never handed to MapLibre. A 32-byte header
(`RDMT`, version, counts, props length), then 8-byte-aligned columns (ids f64; fvals; lon, lat i32
E7; fa, ia, mz f32; rank u32; kz, class, tier, flags u8; cells: Morton code, count, tier; props
offsets), then each point's lean JSON props back to back. The server attaches display names
(`main`/`sub`, `cmain`/`csub`) and serves it gzip'd.

### Served

- `GET /api/marks/tile/{kind}/{z}/{x}/{y}` (z ≤ 5): a thinned tile.
- `GET /api/marks/block/{kind}/6/{x}/{y}`: every point of the kind in the z6 tile.
- `GET /api/marks/specks/{kind}/{z}/{x}/{y}?q=…` (z ≤ 5): speck cells of the points passing the
  filters that the tile doesn't keep, cached per tile and query.
- `POST /api/marks/view`: the In view query over the `pts` of the z6 tiles meeting the outline,
  returning the worker's `result` JSON (the score histogram and scores at ranks; per kind its count
  and best-known named point; the top 60 overall and per kind; the filters' histograms; the highest
  named peak from `summits`), plus **`extra`**: the points sized at this view's range that the tiles
  at the request's `tz` don't keep, less those the client holds, at most 5,000.
- `GET /api/marks/count?kind=…&q=…`: worldwide filtered counts.
- `GET /api/marks/detail/{kind}/{id}?at=lon,lat`: the popup record with descriptions laid over
  (docs/names.md), from the z6 tile of `at` or its neighbours (204 if none).
- Requests carry the marks version (the marks files' content names and the translations' version);
  a mismatch is 409, and the client asks again after switching.
- `GET /api/layer/summary`: the area overlays' counts.

### Exactness

The server computes as the app does, exactly: the score `(1 − b)·min(1, fa/5) +
b·clamp((log10(max(0.05, ia)) + 1.3)/5.6, 0, 1)` in f64 (`marks::log10_js` matches V8's log10 bit for
bit); degrees as `e7 as f64 / 1e7`; outlines across the antimeridian; ties by fa then rank (best per
kind), by score, the kind's place in the request, then rank (top lists); the summit the first in view
in the summits list's order.

## Areas: heritage areas, Indigenous lands, special areas, World Heritage outlines

- **`layers/ov-{heritage-areas,indigenous,special,whs}/all`** (z0–8; z9–12 within the coverage +
  20 km): one bundle per layer, which the catalog names as each root, lo and hi pack it holds; MVT, layer `a`, each feature with its id, lean properties and `own` (the z3 tile whose
  ovdata holds its details). A World Heritage outline carries its site dot's id and position.
- **`ovdata/3-x-y`**: area details by id and park records, owned by the z3 tile of the box centre.
- **Routes:** `GET /api/overlays/detail/{layer}/{id}?own=3/x/y`; `/api/park`.
- **Client:** vector sources; histograms from `querySourceFeatures`, de-duplicated by id.

## Stations

`layers/stations/{root,lo,hi}`: MVT, layer `s` (`n, en, g, m, sp, mz`). A tile at zoom z holds the
stops shown up to z + 1; zoom 12 holds every stop within the built z6 tiles' tiles + 20 km.

## Ferries

`layers/ferries/{root,lo}`: gzip'd GeoJSON blocks (z0 simplified to 5 km, z3 to 300 m, z6 full),
each with the ferry ways meeting the tile grown by 30 km, terminals within 30 km, and the lines'
records. `ferries.ts` merges blocks by id; in-view kilometres are measured on the loaded geometry.

## Server and client

- Named tiles have one handler with a rule per layer; the marks format has its own attacher. Marks
  tiles and blocks are cached by bytes (192 MB); the named-MVT cache holds up to 6,000 tiles.
- markdata and ovdata are read as typed section views: paged from the NAS, mapped from the mirror.
  A tile's markdata is its member of the bundle, found by the bundle's pack index and read in place;
  the views' caches and the blocks' ETags name it by the bundle and the tile. So a change to any one tile
  gives the bundle a new name, which invalidates every tile's cache entries and ETags.
- The client loads z6 blocks from zoom 6 and thinned tiles below, held as typed arrays (least
  recently used dropped past 256 MB); `/api/marks/view` is asked by the worker and `extra` joins
  its kind's points; specks below zoom 6 are pseudo-points weighted by count; filtered kinds' cells
  come from `/api/marks/specks`; `lmk://` tiles are made from the loaded points. A catalog without
  the points' tiles shows none.

## Build

The jobs form a chain without cycles; the landmark chain runs beside the roads chain (docs/building.md §2):

| Job | Reads | Writes |
|---|---|---|
| OSM pass sets | the filtered planet | `summits` (`-v2`), `hikes`, `named` (the heritage chain's filter plus World Heritage tags), `whs` (pipeline::whsshapes' objects and every multipolygon, with their references), `marks` |
| `trailends` (per pass) | `hikes` | `work/trailends/<d>`: every simple linear hiking route's two ends |
| `terrain-z8` (once; network) | AWS's raw z8 tiles | `sources/terrain-z8-v3` and `-v3-max` |
| `summits` (per pass) | `summits` set, `terrain-z8` | `work/summits/<d>` |
| `heritage-sites` (before the z6 tiles) | the registers meeting the cover, the matches (docs/inputs.md §6.5), the pass's `areas` set, the coverage | `work/heritage/<d>/base/…` and per z6 tile slices |
| `unit` | as docs/steps.md, with the heritage slices within U + 30 km | the base pack |
| `pois` (per z6 tile) | U's extract, `trailends`, the coverage | `work/pois/<u>` |
| `peaks` (per z6 tile; network) | U's peak candidates, `summits`, z12 within 30 km, `terrain-z8` | `work/peaks/<u>` |
| `items` (per pass; network) | the current z6 tiles' candidates' QIDs | `sources/items/<d>/{facts,views,meta}` |
| `heritage` (network) | the heritage-sites outputs, the `areas`, `named` and `whs` sets, the pass's answers | `work/heritage/<d>/…` |
| `marks` (worldwide) | the z6 tiles' `work/pois` and `work/peaks`, the items' facts and views, the heritage job's outputs | a `marks-*` bundle per kind, the `markdata` bundle, `global/marks/summary`, `work/marks/heritage-dots` |
| `overlays` | the heritage job's outputs, `work/marks/heritage-dots` | an `ov-*` bundle per layer, `ovdata`, `global/heritage/…` |
| `stations` (roads chain) | the `rail` set, the z6 tiles within 20 km of a built z6 tile | `stations` packs |
| `ferries` (roads chain) | the `ferries` set, the ferry feeds (crate::gtfs::ferrycount), the static timetables (docs/inputs.md §6.7.2) | `ferries` packs |

**Keys** are the step version plus the content names of what a job reads. heritage-sites, heritage
and items include the pass's date; the coverage enters as a hash of its shapes; stations name
the z6 tiles within 20 km of a built z6 tile (`ovconv::cover_of`: a z6 tile amid built ones changes
nothing); overlays name the built z6 tiles, the heritage dots as the last marks run wrote them
(`work/marks/heritage-dots`) and the pass's heritage job's files; ferries the ferries set, the ferry feeds' list (each zip by
content) and the static timetables' index, all `ovconv::ferries_job` reads. `pass-sets` and
`terrain-z8` run when their versioned outputs are missing. peaks (`agent::build::peaks_keys`) name
the z6 tile's candidates, the summits, `terrain-z8` and its max, the terrain hi packs within U + 30 km,
and `TERRAIN_V` (raw tiles outside the packs are processed by its code), not the z12 tiles read
(task #484).

**Network jobs:** a tile or answer that can't be fetched fails the job (retried), never counts as
"none".

**Determinism across z6 tiles:** a peak's result reads only z12 within 28 km and the worldwide z8, the
same whichever z6 tile computes it (z12 the pack's tile, else AWS's raw tile repaired; a missing z12
tile is sea level). Neighbouring summits count at their own heights; ties go by tagged height,
distance, OSM id. Trailheads within 150 m are de-duplicated on U plus the extract's buffer in one
order, so neighbouring z6 tiles agree; route ends come from the whole route; the coverage clip comes
after the dedup. The municipal heritage dedupe (40 m) runs once, in heritage-sites.

### The landmark jobs, step by step

- **Candidates (`pois`)**, a job per z6 tile so their rules don't rebuild the roads: `extract` on U's extract
  with `--candidates` and the pass's trailends, written as zstd JSON lines sorted by key
  (`n<id>`, `w<id>` or `trail:<relation>:<node>`, plus the kind), in one order (trailhead rank,
  key, kind) before the 150 m dedup. Route ends near the extract's roads (300 m). Clipped to the
  points U owns in the coverage. A viewpoint that is also a volcano becomes a peak; pure volcanoes
  stay summits. Each carries `key, kind, lon, lat` (E7), `name, ele, osm, qid`, `en` (`name:en`; in
  Japan `name:ja-Latn`/`name:ja_rm`), kept tags (`marksjob::detail_keys`), `length_m` for covered
  bridges, `viewpoint: "yes"` for peaks tagged so.
- **`trailends`** (per pass): each `hikes` route's way-end nodes used once; exactly two make a simple
  linear route. Worldwide, so every z6 tile sees the same ends.
- **`terrain-z8`** (once): AWS's 65,536 raw z8 tiles repaired (`terrain_pack::process`), one
  content-named pack under `sources/` with each tile's maximum. Coverage-free (the packs' z8 isn't),
  so floods and isolation searches don't depend on what's built.
- **`summits`** (per pass): the `summits` set, nodes then ways by id. A summit's position and `ele`
  are the same as a candidate's (shared code). Each summit's z8 pixel is raised to its `ele` when
  plausible: at most 8,900 m, at most 1,500 m over the highest z8 pixel within one pixel, and at
  most twice that plus 300 m.
- **`peaks`** (per z6 tile, `pipeline::peaks`):
  - z12 is the pack's tile when the manifest has it, else AWS's raw tile repaired with no sources
    (not GLO-30 nor the water: task #465), read back from the PNG as stored. A z6 tile's pack tiles
    are read in spans (neighbours under 256 KB apart together, up to 32 MB a read, 8 reads at
    once, each under a 2-minute deadline and retried after 2, 5, 15, 30 s when it fails
    transiently), and a job's next z6 tile's are read while this one's peaks are measured
    (`unit::read_ahead`).
  - Summits within 28 km + a margin get a summit pixel (highest within 150 m), a height (max(ele,
    DEM) when `ele` is within −30/+200 m of the DEM, else the DEM's) and a claim on shared pixels.
  - **Fine stage** (z12, a 600k-pixel flood, stopping at 28 km; isolation within 25 km), then the
    **coarse stage** over `terrain-z8` with the overlay (40M pixels; tiles by great-circle lower
    bound; nothing higher within 5,000 km gives a lower bound).
  - "Sea" is ≤ 0 m (negative values are clamped).
  - **Output:** `work/peaks/<u>` by candidate key: e, p, pl, c, ce, iso, il, hi.
- **`items`** (`pipeline::items`, then `pipeline::pageviews`, per pass): Wikidata facts for single-QID
  tags, and pageviews for the first QID: the mean of the last November, February, May and August ended 20 days before the pass.
  Everything is fetched again each pass; between passes only QIDs not seen. Caches are appended
  5,000 items at a time. The answers are kept on the NAS (`sources/items/<d>/answers.tar.zst`:
  docs/macs.md, Downloads). meta holds the months and the fetch days. User-Agent "scenic-roads/0.1
  (personal offline map)"; a 403 or failed batch fails the job.
- **`marks`** (worldwide): reads the current z6 tiles' pois and peaks from the manifest, builds
  `marksjob::Candidate`s, turns them into points (`marksjob::poi_points`), adds the heritage points,
  and writes with `markconv::write` (summits by −ele, stable over fame). Descriptions are applied
  when serving.
- **Checks:** a z6 tile run twice gives the same bytes and neighbouring z6 tiles agree near their border
  (done for candidates and peaks); planned: a region-sized extract against the union of its z6 tiles'
  candidates.

### Heritage and area flags

The heritage chain (heritage.py, pipeline::heritagewd, pipeline::heritagedetails, pipeline::areadetails, pipeline::whsshapes,
pipeline::filterprops, pipeline::pageviews, pipeline::interest) runs in a stand-in root laid out as the
repository (`dem/` the app's scripts, `data/heritage/` the pass's working copy of the answers;
docs/inputs.md §6.5.2), split into two jobs so the z6 tiles never wait on Wikidata.
- **The registers** are an input (docs/inputs.md §6.5): heritage-sites links the accepted registers
  meeting the cover and the owner's matches into its scratch for heritage.py (`--registers`,
  `--matches`).
- **The chain's answers** are kept per pass as the items job's are (`heritage-<d>/` in the cache,
  `sources/items/<d>/heritage.tar.zst` on the NAS); a new pass starts from the last's.
- **`heritage-sites`** (before the z6 tiles): the cover is the z12 tiles within 20 km of the coverage
  (`Coverage::meets_rect`, heritage.py's `--tiles`); the designated areas are the pass's `areas` set
  clipped to it. Outputs: `work/heritage/<d>/base/<file>`, and per z6 tile the sites' positions
  (`pos/6-x-y`), World Heritage components among them (`parts/6-x-y`), and the area polygons meeting
  it with their flag bits (`areas/6-x-y`).
- **The z6 tiles** read the slices within U + 30 km: `heritage.json` feeds the flags step, and
  `areaflags` (pipeline::areaflags) rasterises the polygons onto the z6 tile's grid. Their keys name the
  slices.
- **The `heritage` job** (after items, before marks) runs the rest of the chain over the same cover:
  the pass's areas and named objects within it, and for whsshapes the pass's `whs` set clipped to
  it (cached per set and cover; the job waits for the set, which pass-sets makes for an older
  pass). The set holds every object tagged `wikidata`, `ref:whc` or `heritage:operator=whc` and
  every multipolygon, with what they reference, so the clip (osmium's smart strategy) holds what
  whsshapes reads of the filtered planet's clip (task #496: the same export and relations on a
  sample z6 tile), from about a fifth of the bytes: copied here first when there's room, read
  three times by osmium. pipeline::heritagewd indexes named objects with a Wikidata item on a 0.01°
  grid and asks what the answers lack through crate::wikiask (declared APIs, so no robots.txt is
  read: their usage policy, a User-Agent and a polite rate, kept by the job's proxy, docs/inputs.md
  §6.9); pipeline::areadetails asks Wikidata for parks the answers lack; pipeline::pageviews takes the
  pass's months. The map's layers are written last, in Rust (`pipeline::heritage_layers`: the lean
  layer files, the sites' popup records, the areas simplified by a port of GEOS's topology-preserving
  simplifier, `pipeline::tpsimplify`). The layers' `en` is the things' own; the server adds
  translations. Outputs:
  `work/heritage/<d>/<stem>`.
- **Its readers:** marks (`markconv::heritage_source`, waiting for it), which saves the World Heritage
  dots' ids (`work/marks/heritage-dots`, with the `layer-heritage` whose record indexes they key);
  then the overlays job, whose `global/heritage/…` the server reads. The overlays run on the last
  marks' dots, current or not, but take them only when they are of the heritage there now: after a
  heritage rerun they keep their outputs until the marks run on it (the new dots change their key).

## Storage

`markdata/` and `ovdata/` are catalog maps (the markdata's every tile naming the one bundle), so GC
handles them. `work/` holds build intermediates,
in no catalog: `work/pois/<u>`, `work/peaks/<u>`, `work/trailends/<d>`, `work/summits/<d>`,
`work/heritage/<d>/…`, `work/marks/heritage-dots`. GC keeps what the manifest names; a retired pass's
entries leave it when a newer pass completes, a z6 tile's pois and peaks with the prune (docs/regions.md,
Shrinking).

## Zoomed-out queries

Drives, rides and rail lines over many hi packs use summaries. Exact queries read every z6 hidata
within the view plus half the window (the sections `here`, `parts`, `psamples`, `pch`, 37 B a
sample), which for a continental view read gigabytes cold; summaries cut the bytes read.

### Summaries: sections of hidata

pack(T) writes them into T's hidata from the same parts as `psamples`, so they're never older than
the samples; hidata without them are queried exactly. One pure builder, `roadcore::lsum`, shared by
pack(T) and the tests.
- **What:** roads at least 2 km long (`LO_MIN_ROAD`) and all rail.
- **Bins:** consecutive samples of one part, closing 500 m (`BIN_M`) after the first sample, where
  the filters' attributes change (class, unpaved, toll, unnamed), and for rail at every way.
- **Sections:** roads `lparts`/`lbins`, rail `lrparts`/`lrbins`; hidata's meta says `"lsum": 1`.
- **Records:** `LPart { u64 road; u32 first; u32 count; f32 road_len; u32 pad }` (24 B) and `LBin`
  (64 B):
  ```
  LBin { u64 way; f32 off0; f32 len; i32 lon0, lat0; i32 lonm, latm; i32 lon1, lat1;
         u32 rinfo (rail: its `railinfo` row, else u32::MAX); u16 n; u8 class; u8 flags;
         [u8; 12] comp (each component's mean × 255); [u8; 4] pad }
  ```
  Roads carry the 12 drive components; rail the 11 ride components besides trains a day, which
  comes from `railfreq` by `way` at query time.
- **Size:** about 7.5 % of hidata.

### Server

- **The switch is the client's:** `approx=1` when the view outline's box is wider than 1,200 km,
  back to exact below 900 km.
- The server answers from summaries when every z6 hidata of the view plus margin has them and the
  window is at least `LO_MIN_ROAD` (rail lines: any); otherwise exactly, never mixing, and says
  which (`approx`).
- **Reads** the summary sections (and `railinfo`, `railstr`) whole through the section cache, never
  `parts`, `psamples` or `pch`; drive names come from the top hits' ways.
- **Runs:** one road's bins in offset order, split where `off0` is more than `GAP_M` past the run's
  running maximum.
- **Windows:** each bin stands for `n` samples spread evenly over it with the bin's score; the
  best-window scan runs on these pseudo-samples. The answer's `geom` uses real sample positions
  only; `way`, `at` the first bin's; names the middle bin's way; rides add the line identity,
  `railinfo` and the maximum trains a day over the window's ways.
- **Rail lines:** a bin is in view when its middle sample is; lengths sum `next.off0 − off0` within
  a run.
- **Cancelled queries stop:** a guard in the handler's future sets a flag the computation checks
  before each tile read, in its loops and before each name lookup.

### Client

One mode for the three panes, from the outline's size with the hysteresis above, part of each
request key. Lists from summaries say so ("≈"). A pick from one cuts its stretch from the road's
profile.

### Accuracy and checks

Against the exact answers on 9 views × 3 presets: drives' top-20 overlap is at least 0.80 at 2 km
and 0.90–0.95 from 5 km, their largest score error 2.5 at 2 km down to 0.4 at 25 km; rides similar;
totals within 0.06 %; rail km within 0.02 %. Warm drives at 5 km over Western Europe went from 372
to 80 ms.

**The test** (`cargo test --release -p server approx_vs_exact -- --ignored`) builds summaries from
the hidata with the shared builder and asserts, for 2/5/10/25 km:

| | 2 km | 5 km | 10 km | 25 km |
|---|---|---|---|---|
| drives: lowest top-20 overlap | ≥ 0.75 | ≥ 0.9 | ≥ 0.9 | ≥ 0.95 |
| drives: largest score error | ≤ 4.5 | ≤ 2.0 | ≤ 1.0 | ≤ 0.6 |
| rides: lowest top-20 overlap | ≥ 0.75 | ≥ 0.85 | ≥ 0.9 | ≥ 0.95 |
| rides: largest score error | ≤ 2.0 | ≤ 1.0 | ≤ 0.5 | ≤ 0.3 |

Rail lines: km within 0.5 %, top-40 overlap at least 0.9, totals within 1 %. Filtered drives aren't
tested.
