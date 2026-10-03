# Phase 5: landmarks, stations and overlays by view

Design for plan §10 phase 5 ("landmarks, stations and overlays by view; zoomed-out queries") and the
missing base(U) step 5 ("POIs, heritage, details, peaks"), from a survey of today's client and
legacy pipeline (2026-10-03). Amends plan §6 (pack(T) no longer writes landmark and station tiles:
their own jobs do) — recorded in §13.

## Why

Today the client fetches whole worldwide files on first use (`/api/layer/<name>`: stops and sights
per kind, heritage 61 MB, heritage areas, Indigenous lands, special areas, World Heritage outlines,
summits, stations, ferries) and its landmarks worker indexes them to compute the In view
statistics, the dots' layout and MapLibre point tiles (`lmk://`). That can't scale past today's
regions (a worldwide heritage file alone would be gigabytes), and new regions get none of these:
the files are today's, converted.

## What changes for the user

Nothing visible on today's regions (same dots, names, popups, In view numbers and histograms), and
new regions get the same layers. Zoomed out, the faint density layer is drawn from per-cell counts
(specks) instead of every point.

## Ids

One 52-bit key per landmark, safe as a JS number, a vector-tile feature id and a feature-state id:
- OSM objects: `id × 4 + type` (node 0, way 1, relation 2);
- register sites: `2^51 + hash(register, reference)` (reference: the register's id or URL, else
  name + designation + position rounded to 10 m);
- World Heritage dots: `2^51 + hash("whc", site id)`;
- synthetic points (trailheads at hiking-route ends, legacy points without an id): `2^50 + hash`.

## Served artifacts

| Artifact | Format | Zooms / scope | Read by |
|---|---|---|---|
| `layers/marks-<kind>/{root,lo}` (viewpoint, peak, waterfall, lighthouse, covered_bridge, rest, trailhead, heritage) | MVT. z0–5: layer `p`, the points that can show (name shown when `mz ≤ z − 3`; the top 256 by score at balances 0, ¼, ½, ¾, 1; the top 64 by each size filter); layer `s`, speck cells (512² per tile) counting the rest (`n`, plus `t` for heritage tiers). z6: every point (WHS components included). Today's property names plus `id`. | root z0–2, lo z3–6 | landmarks worker |
| `markdata/6-x-y` | sectioned: `ids` (u64, sorted), `pts` (48 B: kind, tier, flags, lon, lat, fa, ia, ele, the size-filter values as f32, NaN unknown), `props` (lean properties, zstd), `info` (popup records, zstd blocks) | per z6 | `/api/marks/view`, `/api/marks/count`, `/api/detail/{poi,heritage}/{id}?at=` |
| `global/marks/summary` | JSON: totals per kind, per heritage tier, per area overlay | world | the Layers panel |
| `layers/ov-{heritage-areas,indigenous,special,whs}` | MVT, simplified per zoom and clipped; properties plus `id` and `own` (the z3 tile holding the details); WHS features carry `sid` and the site dot's position | root, lo; hi z9–12 within coverage + 20 km | MapLibre vector sources |
| `ovdata/3-x-y` | sectioned: area details by id, an area column for the counts, park records (name, bbox, tags, Wikidata) | per z3 | `/api/detail/{harea,special,indigenous}/{id}?own=`, `/api/park` |
| `layers/stations/…` | MVT, layer `s`: `n, en, g, m, sp, mz, id`; z0–8 keep stops with `mz ≤ z − 2.58`; z9 complete within coverage | root, lo, hi z9 | MapLibre vector source |
| `layers/ferries/{root,lo}` | gzip'd GeoJSON blocks at z0 (simplified to 5 km), z3 (300 m), z6 (full): the ferry ways touching the block, their terminals, the records of their lines | — | ferries.ts (merged by id) |
| `layers/grid-areas/hi/6-x-y` | as today, made for new coverage too (read by base(U) for road flags) | z11 grids | base(U) |
| `global/whs-sites` | World Heritage sites (one dot per site, its components) | world | `marks` |

Names are attached by the server at serve time, as for labels (`names::mvt`): `n`/`cn` →
`main`/`sub` and `cmain`/`csub`, ETags following the translations.

## Server

- One handler for vector tiles with names (generalised from the labels'), a `LayerRule` per layer.
- `markdata` and `ovdata` as typed section views (paged from the NAS, mapped from the mirror).
- `POST /api/marks/view` (outline, bounds, balance, kinds with their filters and switched-off
  tiers, `hists`, top, ranks): today's worker `query`, in Rust over the `pts` of the z6 tiles meeting
  the outline: the 512-bin score histogram, the scores at the given ranks, per kind its count and
  best-known named point, the top 60 overall and per kind, the open filters' histograms, the highest
  named peak. Same JSON as the worker's `result`.
- `GET /api/marks/count`: worldwide filtered counts (per catalog and filter set, cached).
- `GET /api/detail/{layer}/{id}?at=lon,lat` (and `?own=` for areas), descriptions laid over as now.
- `/api/layer/*` and `/api/detail/{layer}/{i}` stay while the catalog lists `global/legacy/*`.

## Client

| Today | After |
|---|---|
| a kind's file loaded whole | per kind a tile set: z6 blocks from zoom 6 (±0.25 hysteresis), thinned tiles at `floor(zoom)` below; least recently used tiles dropped past ~1.5 M points |
| the worker's `query` | `/api/marks/view`, same response; `applyResult` unchanged |
| summits file | the view query's highest named peak |
| `count`, tier counts, `layer-summary` | `/api/marks/count`; totals from `global/marks/summary` |
| dot layout per source | per tile (dots.ts sources keyed by kind and tile); speck cells become pseudo-points (fa 0, their count) on the existing speck path |
| filter masks per source | per tile; zoomed out, a filtered kind's speck cells are hidden |
| `lmk://` tiles from the whole index | from the loaded tiles, same caps |
| area overlays as GeoJSON | vector sources; area histograms from `querySourceFeatures`, de-duplicated by id |
| `stations.json` | vector source; feature state with its source layer |
| ferries files | blocks for the view, merged by id |
| details by `i` | by `{layer, id, at}` (areas: `own`) |

Both formats while today's converted files exist: the client uses the new layers when the catalog
lists them.

## Build

| Job | Reads | Writes | Key |
|---|---|---|---|
| `registers` (network; twice a year) | the modules whose areas meet the coverage (whole jurisdictions); UNESCO list and WHS items; special-area lists | `sources/registers/<d>/…` | modules and their versions, refresh epoch |
| `items` (network; per pass) | QIDs of the pass's landmark candidates, register and UNESCO QIDs | `sources/items/<d>/facts`, `sources/pageviews/<seasons>/views` | QID list, seasons |
| `overlays` (before units) | `areas` set, `whs` set, register areas, special lists, UNESCO, facts, coverage | `ov-*` packs, `ovdata`, `grid-areas` near the coverage, `whs-sites` | their content names, coverage |
| `unit-marks` (per unit) | the piece, coverage near U, register slices, terrain z12 within U + 30 km and z8 within 1,500 km (for peaks) | `work/marks/<u>`: candidates (key, kind, position, name, en, ele, OSM id, QID, kept tags, peak result, heritage record) | those |
| `marks` (worldwide; after units) | every `work/marks/*`, facts, pageviews, `whs-sites` | `marks-*` packs, `markdata`, `global/marks/summary` | all of their names |
| `stations` | the `rail` set, coverage | `stations` packs | set, coverage |
| `ferries` | the `ferries` set, frequency sources, coverage | `ferries` packs | those |

`marks` ports `dem/interest.py` (fame from pageviews, else sitelinks; isolation per kind; `mz`),
`dem/layers.py` (lean properties, merged WHS dots, repeated names held back) and
`dem/filterprops.py`, sorted so it's deterministic. base(U) keeps reading heritage sites for road
flags, from the register slices (today: the converted `heritage.json`).

Agent order: terrain → slope → overlays → unit → unit-marks → pack → lo → roots → marks, stations,
ferries → catalog; the network jobs whenever stale.

## Today's regions

1. `scenic-build convert-legacy-marks` writes every artifact above from `global/legacy/*` (legacy
   fa/ia/mz kept; ids from details-poi `osm`, register URLs or `dfhd_id`, else synthetic).
   Golden: `/api/marks/view` equals the legacy worker's result for the nine golden places at zooms
   3–14; details match by id; screenshots match except the zoomed-out specks.
2. The client follows (both formats until one release after cutover).
3. Today's data also becomes `work/marks/<u>` candidates with the legacy pageview table; `marks`
   must reproduce the legacy fa/ia/mz exactly before new units join.
4. Overlays, stations and ferries from the new jobs once registers and the sets exist; compared by
   counts and distributions.

## Sizes (estimated from today's files)

- marks lo packs per z3 (all kinds): London ≈ 5 MB complete z6 blocks + 1.7 MB thinned z3–5; Paris
  ≈ 3.4 + 1.2; Tokyo ≈ 0.8 + 0.4. Largest tile: London's heritage z6 block, ≈ 1 MB.
- markdata per z6: London ≈ 8 MB, Paris ≈ 4.4, Tokyo ≈ 3.2.
- The densest zoom-6 view holds ~300 k points in the browser (today: always 584 k).

## Order

1. Formats; `convert-legacy-marks` (the 8 point kinds); server tiles, `markdata`, `/api/marks/view`,
   details by id; golden against the legacy worker (no client change yet).
2. Client points through tiles (with the legacy path kept).
3. Overlays, stations and ferries converted; client vector sources and ferry blocks; counts and
   summary.
4. Extract keeps OSM ids and tags; `unit-marks`; legacy candidates; `marks` in Rust with the
   regression test; a pilot new region.
5. `registers` and `items`; `overlays` (with `grid-areas` before units); `stations`, `ferries`.
6. Agent keys, order and waves.

## Risks

- The zoomed-out look (specks are unfiltered; rank settings above 256): screenshots at z2–5.
- Server cost of continental queries cold from the NAS: section caches and the mirror; a per-z3
  summary if measurements need it.
- Fame and isolation drift once recomputed: the legacy-input regression first.
- Peaks per unit read many z8 tiles (Mont Blanc): cached on the build Mac.
- Registers fetched whole: format changes, id stability.
- Fetch volume for ~1 M QIDs worldwide (incremental; limit to the units' candidates if slow).
- `global/railfreq` is also a whole-world file the client loads: per-z6 blocks before worldwide.
