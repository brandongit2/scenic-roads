# Formats (implementation spec for docs/plan.md v7)

The contract between the build steps, the NAS, the server and the browser. All integers are
little-endian. Coordinates are i32 1e-7 degrees (`E7`) unless stated. A tile key is
`roadcore::archive::tile_key(z, x, y) = z << 58 | x << 29 | y`.

Bump a format's version on any incompatible change; readers accept the current and previous
version (plan §8, Format bumps). Within a container's version 1, additions are told apart by
section names and meta keys, and readers take both forms: base packs' `elevu` (else `elev`), road
values' `byroad`, hidata's `railinfo` and meta `lsum`.

## Names and hashes

- **File hash:** BLAKE3 of the whole file, first 16 hex digits. Content-named file:
  `<logical>.<hash16>.<ext>` (e.g. `layers/roads/hi/6-32-21.0a1b2c3d4e5f6071.pack`). The logical
  name never contains a dot.
- **Blob hash:** XXH3-64 of the stored bytes (the tile as served, still gzip'd), as hex in ETags.
- A file is written to `<name>.tmp`, its hash checked, then renamed. Nothing ever rewrites a
  content-named file.

## Pack (`.pack`, RDPACK v1): tiles of one layer under one root tile

```
0   [u8; 8]  "RDPACK01"
8   u32      format version (1)
12  u32      flags (bit 0: blobs are gzip'd)
16  u64      index offset
24  u64      index entry count
32  u32      meta length n
36  [u8; n]  meta JSON: {"layer", "scope": "root"|"lo"|"hi", "root": "z/x/y",
             "encoding" (e.g. "rt7", "terrarium-png", "slope4-png", "terrarium-webp", "mvt"), …}
             (zoom ranges are the catalog's, per layer: a pack's header is written before its tiles)
…            blobs (identical blobs stored once; several entries may share an offset)
index        count × Entry (32 bytes), sorted by key, 8-byte aligned:
             u64 key, u64 offset, u32 len, u32 raw_len (0 if unknown), u64 xxh3
```

- Root pack: z0–2 (root "0/0/0"). Lo pack: z3–8 under one z3 tile. Hi pack: z9–14 under one z6
  tile. A tile lives in exactly one pack.
- Readers fetch the header and index once (two range reads) and keep the index locally
  (`idx/<file hash>.idx`), so serving a tile is one range read, and a 304 none.

## Sectioned file (`.sect`, RDSECT v1): named arrays

Base packs, hi data, road values and other non-tile data.

```
0   [u8; 8]  "RDSECT01"
8   u32      format version (1)
12  u32      section count
16  u64      table offset
24  u32      meta length n
28  [u8; n]  meta JSON
…            sections, each starting on a 64-byte boundary
table        count × Section (48 bytes): [u8; 24] name (NUL-padded), u64 offset, u64 len, u64 xxh3
```

A local file is mmapped and its sections cast in place (64-byte alignment suits every record
type). A NAS file is read through the I/O pool: in 256 KB pages for the records a request needs, or
whole sections for what a query scans.

## Units

Every unit is a z6 tile (a tile key). A way's owner unit is the z6 tile of its first vertex.

## Base pack (`base/<z>-<x>-<y>.<h>.sect`): what one unit owns

Meta: `{"fmt": 1, "unit": "z/x/y", "ways", "verts", "samples", "extent": [w, s, e, n] E7 of all
owned geometry, "source": "legacy:<build>" | "pass:<date>", "scenic": bool, "drape": bool (whether
those sections are there), "summary": {…}}`. `summary` (packs from 2026-10-03 on; the catalog works
it out from the sections for older ones) is what the map's meta adds up: `extent`, `ways`,
`vertices`, `elev_min`/`elev_max` (roads, not rail, metres), `hist` (road km by 10 m of elevation at
segment midpoints, 256 bands) and `rail_km` (track km per service group).

| Section | Record | Notes |
|---|---|---|
| `ways` | `WayRec` (48 B, roadcore) | sorted by (z9 key of first vertex, Morton of first vertex); `vstart` is local; `name`, `ref_`, `surface`, `route` index `strings` |
| `verts` | `[i32; 2]` | densified geometry |
| `elevu` | `u16` | processed elevation, decimetres + 5,000 (−500 to 6,053.5 m; `final.u16`); packs made before 2026-10-03 have `elev` instead: `i16`, decimetres, clamped at ±3,200 m (`final.i16`). Readers take either (`roadcore::elev`) |
| `raw` | `f32` | raw DEM sample (legacy `elev.f32`), NaN = none |
| `grade` | `u8` | \|grade\|, 0.5 % units |
| `src` | `u8` | DEM source (`DemSource`) |
| `scenic` | `[u8; 13]` | per-vertex channels (`roadcore::scenic::ch`) |
| `drape` | `i16` | drape height, metres (legacy `vterrain.i16`) |
| `strings` | UTF-8 lines | line 0 is the empty string |
| `samples` | `Sample` (24 B) | `way` is the local way index; grouped by way, in way order |
| `samplech` | `[u8; 13]` | per sample |
| `sub9` | `(u64 z9 key, u32 first way, u32 way count)` | ways grouped by the z9 tile of their first vertex |
| `rail` | `(u32 way, u32 relation lo, u32 relation hi, u32 pad)` | rail ways' primary route relation (legacy `rail-rels.bin`) |

## Road values (`global/roads/<z>-<x>-<y>.<h>.sect`): per owned way of a unit

Section `roads`, one 24-byte record per way of the unit's base pack, in the same order:

```
u64 road id      the lowest OSM way id of the road
f32 road length  metres
f32 offset       metres from the road's start to this way's start, along the road's direction
u8  dir          0: the way runs with the road; 1: against it
[u8; 7] pad
```

Section `byroad`: `(u64 road id, u64 way index)` for every way, sorted, so a whole road's ways in
the unit are one binary search away. (Files from before 2026-10-03 lack it; the server sorts
`roads` itself for those.)

One chaining (plan §6): at each node, way ends of the same kind pair by mutual best continuation.
Levels, best first: same ref (any shared token of a multi-ref; two ways whose refs share none never
pair), else same name, else same class when both are unnamed (no name and no ref), else (level 0)
a way with neither continuing one that has a name or a ref, of the same class within 35° (a bridge
or a short link without its own name). Straightest first within 100° (35° at level 0); a oneway
only in its direction of travel; ties to the higher level, then the straightest, then the lower way
id. Rail pairs with rail by line identity (the name before ':', else the first service); ferries
and ways of fewer than 2 vertices never chain. Pairs form paths and cycles; a path walks from its
end whose way has the lower id; a cycle starts at its lowest way id, in that way's direction.

## Hi data (`hidata/<6>-<x>-<y>.<h>.sect`): per z6 pack tile T

| Section | Record | Notes |
|---|---|---|
| `here` | `Here` (40 B) | every way whose box meets T, sorted by `id` |
| `ends` | `(u64 point, u32 here index, u32 pad)` | both end vertices of each `here` way, sorted by point (`(lon as u32) << 32 \| lat as u32`) |
| `parts` | `Part` (32 B) | query parts: one road's consecutive samples inside T, ended at gaps over 300 m |
| `psamples` | `PSample` (24 B) | the parts' samples, part by part |
| `pch` | `[u8; 13]` | per part sample |
| `climbs` | `Climb` (64 B) | climbs starting in T |
| `climbgeom` | `[i32; 2]` | their polylines |
| `railinfo` | `RailInfo` (32 B) | rail ways' lines, sorted by `here` (since 2026-10-03; older hidata have none: the server reads base packs) |
| `railstr` | newline-separated | their names and routes (0 is "") |
| `lparts`, `lrparts` | `LPart` (24 B) | the zoomed-out summaries (meta `"lsum": 1`; docs/phase5.md): roads of 2 km or more, and rail, as parts of bins |
| `lbins`, `lrbins` | `LBin` (64 B) | their bins: consecutive samples of a part, up to 500 m, one set of drive filter attributes (rail: one way) |

```
Here    { u64 id; u64 owner (unit tile key); u32 index (in the owner's base pack); u8 class;
          u8 flags (WayRec flags); u8 extra (bit 0 unnamed, bit 1 rail); u8 pad;
          [i32; 4] bbox; }                                                       // 40 bytes
Part    { u64 road; f32 offset (of the first sample along the road); u32 first (psamples index);
          u32 count; f32 road_len (the whole road's length, m: the length filter); u8 class;
          u8 flags (bit 0 unpaved, bit 1 toll, bit 2 unnamed); [u8; 6] pad }    // 32 bytes
PSample { u32 way (here index); f32 offset (along the road, m); i32 lon; i32 lat; f32 eye;
          u8 flags (sflag); [u8; 3] pad }                                         // 24 bytes
LPart   { u64 road; u32 first (bin index); u32 count; f32 road_len; u32 pad }    // 24 bytes
LBin    { u64 way (OSM id: a road bin's middle sample's, a rail bin's own); f32 off0 (first
          sample's offset); f32 len (last − first); i32 lon0, lat0, lonm, latm, lon1, lat1 (first,
          middle, last samples); u32 rinfo (rail: railinfo row, else u32::MAX); u16 n (samples);
          u8 class (rail: the first sample's way's); u8 flags (bit 0 unpaved, bit 1 toll, bit 2
          unnamed; rail 0); [u8; 12] comp (component means × 255: drive components, or ride
          components but trains a day); [u8; 4] pad }                                  // 64 bytes
Climb   { u64 way (OSM id at the start); u64 label (OSM id at the middle); f32 gain_m;
          f32 length_m; f32 start_elev; f32 top_elev; f32 max_grade; f32 road_len (of the
          middle way's road); [i32; 2] mid; u32 geom_start; u32 geom_count; u8 class;
          u8 unpaved; u8 flags (middle way: bit 0 toll, bit 1 unnamed); [u8; 5] pad } // 64 bytes
RailInfo { u32 here; u32 colour; i64 rel (primary route relation, 0 none); u32 name; u32 route
          (railstr indexes); u8 rail (service bits); u8 class; [u8; 6] pad }      // 32 bytes
```

The record types are `roadcore::packs`.

## Landmark points (`markdata/6-<x>-<y>.<h>.sect`): per z6 tile (docs/phase5.md)

Every point of the 8 kinds (`pipeline::marks::KINDS`) positioned in the tile, sorted by kind, then
id. Meta: `{"fmt": 1, "tile": "6/x/y", "block": 256, "fields": {kind: [prop, …]}}` (a kind's filter
properties; a reader checks them against its own).

| Section | Record | Notes |
|---|---|---|
| `kinds` | `[u32; 2]` × 8 | per kind: first row, count |
| `ids` | `u64` | the point's id (phase5.md "Ids"), sorted within its kind |
| `pts` | `MarkPt` (28 B) | |
| `fvals` | `f64` | per kind, per field (column), its rows' values; NaN: none |
| `props`, `props_idx` | zstd blocks; `[u64; 2]` (offset, length) per block | 256 rows a block, each an objects block of the rows' lean properties |
| `info`, `info_idx` | likewise | popup records (empty: none) |
| `summits` | `SummitRec` (24 B) | the named peaks with a height in the tile, with their place in the worldwide list |
| `summit_names` | objects block | their names |

```
MarkPt    { i32 lon; i32 lat; f32 fa; f32 ia (20000 unknown); f32 mz (NaN none); u32 rank (the
            tie-break: the kind's order); u8 kz (lowest zoom whose thinned tile keeps it, 6 none);
            u8 class; u8 tier (heritage, TIERS); u8 flags (1 named, 2 World Heritage component,
            4 picnic site) }                                                     // 28 bytes
SummitRec { u32 rank; i32 lon; i32 lat; u32 pad; f64 ele }                        // 24 bytes
objects block: u32 count, u32 × (count + 1) offsets, then the JSON objects back to back
```

## Marks tile (RDMT v1): thinned tiles, z6 blocks, served gzip'd

`layers/marks-<kind>/{root,lo}` packs hold the thinned tiles of zooms 0–5 (encoding `rdmt`); the
server makes z6 blocks from markdata. Little-endian; each column starts 8-byte aligned.

```
0   "RDMT"; u32 version (1); u32 n (points); u32 nf (fields); u32 nc (speck cells); u32 props bytes;
    u64 reserved
32  f64 ids[n]; f64 fvals[nf][n]; i32 lon[n]; i32 lat[n]; f32 fa[n]; f32 ia[n]; f32 mz[n];
    u32 rank[n]; u8 kz[n]; u8 class[n]; u8 tier[n]; u8 flags[n];
    u32 cell code[nc] (Morton code within the tile at zoom z + 10); u32 cell count[nc];
    u8 cell tier[nc]; u32 props offsets[n + 1]; the props (a JSON object per point)
```

A thinned tile at zoom z holds the points with kz ≤ z, and the rest as speck cells. Served props
carry `main`/`sub` (and `cmain`/`csub` from `cn`) as the layer files do.

## Overlays, stations and ferries by view (pipeline::ovconv; docs/phase5.md)

Made by `convert-legacy-overlays` from today's files, and by the `overlays`, `stations` and
`ferries` jobs from the pass. The `overlays` job also writes `global/heritage/{layer-summary,
heritage-sources}`.

- **Areas** `layers/ov-{heritage-areas,indigenous,special,whs}/{root,lo,hi}` (encoding `mvt`,
  z0–12; hi tiles within the coverage + 20 km): gzip'd vector tiles, extent 4096, layer `a`; each
  feature with its id (docs/phase5.md "Ids") as the feature id, the lean file's properties, and
  (heritage areas, Indigenous lands and special areas) `own` ("3/x/y", the ovdata holding its
  details); a World Heritage outline's id is its site dot's, with `px`, `py` its place. Served with `main`/`sub` from `name` (outlines: `n`).
- **`ovdata/3-<x>-<y>.<h>.sect`** per z3 tile (owner: the tile of a feature's or park's box
  centre): per key (`harea`, `indigenous`, `special`, `parks`) `<key>.ids` (u64, sorted; parks:
  their order), `<key>.offs` (u32, n + 1) and `<key>.recs` (the records, JSON, end to end); meta
  `{"fmt": 1, "tile": "3/x/y", "records": {key: n}}`.
- **Stations** `layers/stations/{root,lo,hi}` (encoding `mvt`, z0–12): layer `s`, `n, en, g, m,
  sp, mz` and the feature id; a tile at zoom z holds the stops shown at zooms up to z + 1 (`mz` ≤
  z − 2.585), zoom 12 every stop (within the built units' tiles + 20 km).
- **Ferries** `layers/ferries/{root,lo}` (encoding `geojson-gz`, blocks at zooms 0, 3 and 6):
  `{"type": "FeatureCollection", "features": […], "lines": {id: record}}`, gzip'd: the ways whose box
  meets the tile grown by 30 km (each with its id `way × 4 + 1` and whole length `km`; simplified to
  5 km at zoom 0, 300 m at 3), the terminals within 30 km (from zoom 3), the records of its ways'
  lines.

## Catalog (`catalog/<n>.json.zst`)

zstd with its content checksum on; written as `<n>.json.zst.tmp`, then renamed. Readers list
`catalog/`, take the highest `<n>` that decodes and parses, and fall back to the next.

```json
{
  "fmt": 1, "n": 7, "created": "2026-10-03T04:05:06Z", "app": "20261004-0252-2684bda (the published app that made it, or development)",
  "files": {"<logical>": {"file": "<content name>", "size": 123, "fmt": 1}},
  "units": ["6/32/21"],
  "layers": {
    "terrain": {"encoding": "terrarium-png", "minzoom": 0, "maxzoom": 12, "root": "<logical>",
                "lo": {"3/4/2": "<logical>"}, "hi": {"6/32/21": "<logical>"}},
    "roads": {"encoding": "rt7", "minzoom": 4, "maxzoom": 14, "lo": {…}, "hi": {…}}
  },
  "basemap": ["<logical of a .pmtiles>"],
  "base": {"6/32/21": "<logical>"},
  "roads": {"6/32/21": "<logical>"},
  "hidata": {"6/32/21": "<logical>"},
  "markdata": {"6/32/21": "<logical>"},
  "ovdata": {"3/4/2": "<logical>"},
  "global": {"railfreq": "global/railfreq", "roadunits": "global/roadunits", "marks/summary": "…",
             "legacy/<stem>": "…", "heritage/<stem>": "…", "outlines": "sources/osm/<date>/outlines"},
  "meta": {"…": "the map's meta, added up from the units' summaries: minzoom, maxzoom, bounds, ways, vertices, elev_min, elev_max, elev_hist_10m_km, rail_km, classes, built"},
  "credits": [{"what": "Road elevation, Japan", "source": "Created by editing GSI Tiles …",
               "terms": "GSI terms of use (Public Data License 1.0)", "areas": [[122.5, 20.0, 154.0, 46.5]]}],
  "coverage": {"regions": [{"id": "monaco", "name": "Monaco", "outline": ["geofabrik:europe/monaco"],
               "shapes": {"geofabrik:europe/monaco": [[[[7.4, 43.72], [7.44, 43.72], [7.44, 43.76], [7.4, 43.76], [7.4, 43.72]]]]}}]}
}
```

`files` holds every file the catalog references.
- `credits`: the sources its data comes from (`pipeline::rules::CREDITS`), those whose areas meet
  the coverage, 20 km around it (how far heritage sites and terrain reach) or a built unit's ways
  (its extent, so a removed region's data keeps its credit while it's served). `areas` is a list of
  w, s, e, n boxes in degrees, left out for credits that hold everywhere.
- `coverage`: the regions the catalog's data is built for: those done at publish time as their
  recipes were then (`--ready <id>=<outline digest>,…`, the agent's plan: one redrawn since isn't),
  and those not done yet as the last catalog had
  them, if it had them (on the map as they were), in the recipes' order; `recorded: true` (a catalog
  built for no region yet says so: one without it predates recorded coverage). Each outline entry's
  polygons as GeoJSON MultiPolygon coordinates in degrees to 5 decimals, rings closed: `osm:`
  entries from the pass's simplified outlines, the others simplified by size (60 m to 1 km). An
  entry that couldn't be read is recorded with no shape.
- A catalog made before these were recorded has `"credits": []` and `{"regions": []}` without
  `recorded`: the server
  then gives every credit, and builds the coverage from the recipes.

GC's roots are the newest catalog, every catalog of the last 14 days and the build manifest; an
unreferenced file goes once it's also older than 14 days, in the folders catalogs index and retired
passes' sources (plan §3). A held catalog is written to `catalog-held/` instead
(`inputs/hold-catalog`).

## On each Mac (`~/Library/Application Support/scenic/`)

```
bin/scenic-launcher     the launcher (never rebuilt)
run/<name>              what the launcher runs, one argument per line: server, agent (build Mac), status
app/<version>/          server, scenic, scenic-build, extract, tile, scenic-metrics, dem/, Scenic.app,
                        web/, fonts/   (app/current → the one in use)
mirror/<content name>   local copies, by the same names as on the NAS (.partial/: in progress;
                        .uses: when each file was last used)
idx/<hash16>.idx        pack indexes (RDPKIDX1: header, meta, entries, XXH3 trailer)
catalog/<n>.json.zst    the last catalogs read
translations/  descriptions/   local copies of the NAS folders, compiled by the server
regions.json            the last regions read; regions-queue/: region edits waiting for the NAS
remote-key              the map's key for other devices (32 hex digits, made once, 0600: docs/plan.md
                        §4, Devices); map-page: the address to open on one, `<base>/#k=<key>` (0600,
                        rewritten when it changes: HTTPS where tailscale serve proxies the server)
agent/                  status.json (the build Mac's; a helper writes helper.json, which that Mac's
                        server shows), state.json, round.json (the last round of publishing:
                        {began, regions, last, units: {logical: content name}, over},
                        agent::build::Round; the jobs of the one under way read its units,
                        SCENIC_UNITS_AS_OF=<path>#<began>), job.json, agent.lock, logs/, cache/
agent/cache/            dem-cache.* (the seed), chm10/ (canopy 10° files) and aws-terrarium/
                        (copies of the NAS's sources/canopy/; the raw tiles as fetched, until packed
                        onto the NAS, and aws-terrarium/packs/: copies of its archives, a job's own
                        new ones among them, each marked used when a job opens it), base/.
                        When a job starts with too little free, raw tiles waiting are packed onto
                        the NAS (not kept here), then chm10/ and aws-terrarium/ lose files: chm10/'s
                        and the archives' idle an hour first, then the rest least recently used
                        first (loose raw tiles a folder at a time; empty markers kept; a canopy file
                        the NAS lacks copied there first, or kept), until the Mac has a sixth more
                        free than the job needs (the OSM pass: what it needs).
```

`~/Library/Preferences/nsmb.conf` gets `[FISHANDCHIPS:PERSONAL]` and
`[FISHANDCHIPS.LOCAL:PERSONAL]`, both `soft=yes`.

## Server API (changes)

- Tiles: `/tiles/{roads,rails,terrain,slope,labels,base}/{z}/{x}/{y}`, `/tiles/trees/{var}/{z}/{x}/{y}`.
  Strong `ETag`: the stored blob's hash, plus the translations versions for named tiles;
  `/tiles/base`'s is a hash of the catalog's basemap archives' content names and the tile's z/x/y, plus
  the names version (a 304 reads no archive); terrain
  and slope tiles the server makes (missing ones, slope z12) carry none. A request with `?v=` (the
  app's URLs) is `public, max-age=31536000, immutable` while that version is current, else
  `no-cache`.
- Ways: `/api/way/{id}?at=lon,lat`, `/api/profile/{id}?at=lon,lat`, `/api/road/{id}?at=lon,lat`.
  `at` picks the hidata of the z6 tile holding it, or of one of its eight neighbours, whose `here`
  holds the way.
- `/api/railfreq`: sorted `(u32 way id, f32 trains a day each way)` pairs; negative: a lower bound
  ("at least").
- Landmarks (docs/phase5.md): `POST /api/marks/view` (the In view statistics, and `extra`);
  `/api/marks/tile/{kind}/{z}/{x}/{y}` (z ≤ 5) and `/api/marks/block/{kind}/6/{x}/{y}` (RDMT);
  `/api/marks/specks/{kind}/{z}/{x}/{y}?q=` (filtered speck cells); `/api/marks/count?kind=&q=`;
  `/api/marks/detail/{kind}/{id}?at=lon,lat`. `/api/catalog` lists `marks`: the tiles with points,
  the kinds with tiles, and their totals.
- Overlays by view: `/tiles/ov/{heritage-areas,indigenous,special,whs}/{z}/{x}/{y}`,
  `/tiles/stations/{z}/{x}/{y}` (MVT, names attached), `/tiles/ferries/{z}/{x}/{y}` (a block,
  names on ways and lines); `/api/overlays/detail/{harea,indigenous,special}/{id}?own=3/x/y`;
  `/api/park` from ovdata when the catalog has it. `/api/meta` says `ovTiles`, `stationTiles`,
  `ferryBlocks`, and versions the tiles as `ov-<name>.tiles`, `stations.tiles`, `ferries.tiles`.
- Drives, rides and rail lines take `approx=1` (zoomed out): answered from hidata's summaries when
  every hidata of the view plus margin has them, and for drives and rides when the window is at
  least 2 km; the answer says `approx`.
- `/api/catalog`: `n`, `created`, `units` (a count), `layers` (encoding, zoom range, version),
  `coverage` (the regions it was built for, without outlines), `credits` (every credit for a
  catalog that has none), `online`, `nas`, `held`, `app`, `agent`, `names` (translation versions),
  `v`, `marks`. The map's meta is `/api/meta`.
- `/api/names`; `/api/build` (the agent's status: this Mac's when it runs here, else the NAS's copy).
- Other devices (docs/plan.md §4, Devices): every request but this Mac's own needs the map's key, in
  the `scenic_k` cookie or `Authorization: Bearer <key>` (else 401, JSON `{"error"}`), but for the
  app itself: `/`, `/index.html`, `/manifest.webmanifest`, `/sw.js`, `/assets/*`, `/icons/*`,
  `/api/ping`, `/api/auth`. `POST /api/auth` `{"key": "<key>"}`: 204 with `Set-Cookie:
  scenic_k=<key>; Path=/; HttpOnly; SameSite=Strict; Max-Age=315360000` (`; Secure` when
  `X-Forwarded-Proto: https`), else 401. A request from anywhere but this Mac, its LAN and the
  tailnet is 403, as is one whose `Host` isn't the map's (a public name: "not this map's address")
  or whose `Origin` is another page's ("not from the map's page"). CORS answers only this Mac's own
  origins (localhost, `*.localhost`, a loopback address).
- Regions (the panel): `/api/regions` (GET, POST), `/api/regions/{id}` (PUT, DELETE),
  `/api/areas?at=`, `/api/areas/search?q=`, `/api/areas/{id}`, `/api/coverage` (the catalog's
  coverage as GeoJSON, one feature per outline entry, with `regions` and `catalog`; built from the
  recipes, without `regions`, for a catalog that records none).
- Names: MVT tiles and API JSON (ways, drives, `/api/names`) carry `main` and, when there is one,
  `sub`; JSON layer files, ferry blocks and marks tiles carry `main` only where it differs from the
  name; popup records (`/api/detail`, `/api/marks/detail`, `/api/overlays/detail`, `/api/park`) are
  served as stored.

## RT road tiles, version 7

As v6 (`roadcore::tile`), but the way column holds OSM way ids, and lines are sorted by (draw
class, id) within a tile. The client sends the id with the clicked point.

## Other files (listed, not specified)

- **The OSM pass** (`sources/osm/<date>/`): `planet.osm.pbf`; `filtered`; `pieces/<u>` and
  `pieces.json`; `sets/<name>[-v<n>]`; `roads/<u>.bin` (32 B a way: u64 way id and its road values);
  `outlines` (sectioned, meta `fmt` "outlines-1": `recs` (64 B `OutlineRec`), `rings`, `points`,
  `srings`, `spoints`, `strings`); `reach.json.zst` (zstd JSON `{fmt, date, units: {"6/x/y": {owned:
  [w, s, e, n] or null, long: [{owned, ferry, verts: [[lon, lat], …]}]}}}`, E7: the box of the ways a
  unit owns within its tile + 20 km, and every way of its piece reaching further, whole;
  `pipeline::reach`); `pass.json`.
- **Global files:** `global/roads/<u>` (above); `global/roadunits` (sectioned, `pairs`: sorted u64
  road, u64 unit key); `global/railfreq` (as `/api/railfreq`: the `rail` job's, by OSM way id from
  `railfreq`'s per-way-index output; each way once); `global/marks/summary`
  (`{fmt, kinds, tiers}`); `global/roaden/<u>` (JSON `{OSM way id: English}`: the unit's roads whose
  `name:en` isn't their name); `global/heritage/*`; `global/legacy/*` (today's converted files).
- **Grid layers:** `grid-{class,canopy,cover}` hi packs of z11 tiles, encoding `u8-zstd`, not served.
- **Worldwide z8 terrain:** `sources/terrain-z8-v1` (one RDPACK of every z8 tile, meta without scope
  or root) and `sources/terrain-z8-v1-max` (each tile's maximum, f32).
- **Work files** (zstd JSON lines unless said): `work/pois/<u>`, `work/peaks/<u>`,
  `work/summits/<date>`, `work/trailends/<date>`; `work/heritage/<date>/{base/<stem>,
  pos/6-x-y.json, areas/6-x-y.jsonl, <stem>}` (an area in each z6 tile its box meets, whole; one
  across the antimeridian, in those its parts' boxes meet); `work/marks/heritage-dots.json`;
  `work/rail/used.json` (railgtfs.py's report: per feed, `id`, `provider`, `url`, `licence`,
  `fetched`, and `status` with, when "ok", `day`, `trips`, `duplicates`, `rail_routes`).
- **Other sources:** `sources/items/<date>/{facts,views,meta}.json`; `sources/registers/<name>.tar.zst`;
  `sources/buildings/<release>/8/<x>-<y>.f32` (the release's dot a dash, as in `2026-09-23-1`; not
  content-named: raw little-endian f32 `[xmin, ymin, xmax, ymax]` in degrees, per Overture building
  whose box's centre is in the z8 tile, sorted, bit-identical boxes once; no file for a tile
  without any) and
  `sources/buildings/<release>/index` (JSON `{fmt, release, zoom, tiles: {"8/x/y": count}}`, written
  last); `sources/trees/leaf/lat<top>_lon<left>.tif` (a 10° square's dominant leaf type at 0.0005°:
  u8 GeoTIFF, 0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data; `dem/leaftype.py`; tag
  `complete=1` when made whole, as the trees job makes them; one made over some regions only,
  without the tag from the EEA, or not whole, is made again, keeping the chunks of it that were
  fetched whole (no holes where a probe shows the EEA has data); while an EEA square is being made, its
  chunks are kept as they come in `sources/trees/leaf/parts/lat<top>_lon<left>/<row>-<col>.npy`, or
  `.none` where the EEA has no data, until the square is saved) and `sources/trees/nalcms-2020.tif`
  (NALCMS's 30 m GeoTIFF, 3.4 GB, kept once fetched, its size and CRC-32 checked against the zip's);
  the downloads kept so each is made once, as the source has them (temporary names end
  `.<host>.<pid>.tmp`): `sources/canopy/meta_chm_lat=<top>.0_lon=<left>.0_{median,p95,cover5m}.tif` (Meta's canopy
  squares; an empty file for one Meta doesn't have), `sources/aws-terrarium/packs/<area>.<hash16>.tiles`
  (AWS's raw terrain tiles in RDTILES1 archives (roadcore::archive), grouped as the terrain's packs
  are: an area is a z6 tile, `6-<x>-<y>`, for zooms 9–12, a z3 tile, `3-<x>-<y>`, for zooms 3–8,
  and `root` for zooms 0–2; the PNG as AWS sent it, an empty entry for one AWS doesn't have; tiles
  in key order; meta `{"kind":"aws-terrarium raw tiles"}`; named by the BLAKE3 of the file) with
  `packs/index.json` (`{areas: {area: [{name, bytes}, …]}, gone: {name: unix seconds}}`: each
  area's archives, oldest first, read newest first; `gone`, the archives on the NAS it doesn't
  name, deleted a day after the time beside them), and the loose tiles from before,
  `sources/aws-terrarium/<z>/<x>/<y>.png` (`<y>.none` for one AWS doesn't have),
  `sources/pageviews/<YYYY-MM>.tsv.zst` (a month of Wikipedia's pageviews, every article of the
  map's languages: a `#langs\t<lang>,<lang>,…` line saying which (an index without it has those of
  2026-10-05, `LANGS_UNSAID`), then `<lang>|<Title_with_underscores>\t<views>` lines, zstd; a title may have more than
  one line, its views summed; dem/pageviews.py),
  `sources/fabdem/<tile>_FABDEM_V1-2.tif`
  (FABDEM's 1° tiles out of Bristol's zips, deflate GeoTIFF; `<tile>.none` for one a zip doesn't
  have);
  `sources/dem-cache/dem-cache.{keys.u64,elev.f32,src.u8}` (today's per-vertex DEM cache, the seed
  the build Mac copies once: sorted keys `(lon + 2³¹) << 32 | (lat + 2³¹)` (E7), elevations,
  sources).
- **The rail sources** (`sources/rail/`, content-named, in the build manifest; `pipeline::rail`, plan
  §6 Rail service; never swept, so a file one replaces stays):
  - `catalogue.csv`: the Mobility Database catalogue (`feeds_v2.csv`), as downloaded;
  - `checked.json`: every catalogue feed checked for rail routes, sorted by `id`: the catalogue's
    `id`, `provider`, `name`, `country` (as `dem/railfeeds.py` corrects it, for a few),
    `subdivision`, `url`, `licence`, and the check's `size_mb`, `rail_routes`, `examples` and
    `status`: "ok"; a definite answer ("http <code>", "no size given", "no range requests", "not a
    zip", "no routes.txt", "an unknown compression…", "routes.txt unreadable (…)", and
    today's build's "no routes.txt (or no range requests)"); or "no answer (…)", which is checked
    again (as is an older "http <code>" with a 429 or a 5xx). The checks seeded from today's build
    that found no rail routes (or no routes.txt) start as "no answer (today's check, asked again)":
    its check could take an answer cut short for none;
  - `gtfs/<feed id>.zip`: each feed's GTFS, as fetched;
  - `fetched.json`: `{zip's content name: "YYYY-MM-DD"}`, the day its timetable counts from (the day
    it was fetched, or last fetched again unchanged; the zips seeded from today's build, the day
    today's figures were counted);
  - `feeds.json`: `{fmt: 1, feeds: […]}`, the coverage's feeds in the order railgtfs.py reads them,
    each `{id, provider, name?, country, url, licence, replaces?, rail_routes?}` with `zip` (its
    content name) and `fetched`, or without them, a `status` saying why it's left out ("replaced by
    …", a download refused, "no answer since <day> (last tried <day>): …", "its check: no answer
    since <day> (last tried <day>)");
  - `mtr-pairs.bin`: the MTR's lines as stop pairs (below), and `mtr.json`, the research
    `dem/mtrpairs.py` makes them from.
- **Stop pairs** (railgtfs.py's, `mtr-pairs`): 21-byte records, f32 lon_a, lat_a, lon_b, lat_b,
  u8 mode (0 tram, 1 metro, 2 rail, 3 funicular; +0x80 when the trains are a lower bound; +0x20 /
  +0x40 when stop A / B is beyond the coverage, which the `rail` job sets), f32 trains from A to B on
  the typical weekday.
- **Kept for units' later runs, both Macs'** (`cache/`, not swept): `cache/dem-units/<u>.<box>.dem`
  (`<box>`: the points' box as 32 hex digits, w, s, e, n as u32; a unit's DEM samples from its last
  run: "RDDEM002", u64 count, the points' box (4 × i32), the
  versions of the DEM rules dem-north-america, -japan, -taiwan, -fabdem it was sampled under (4 ×
  u32), then the sorted keys, elevations and sources) and `cache/scenic-units/<u>/` (its canopy and
  view results: canopy.keys, canopy.tiles, view.keys, view.tiles, near.i8, roadside.u8,
  samples.metrics.u8, grid.canopy.u8.zst, grid.cover.u8.zst, and basis.json: {v (scache::SCENIC_V),
  basis: [[[x, y], hash16 of the z11 tile's terrain and land cover], …]}).
- **Two Macs and other workers** (`docs/workers.md`):
  - `state/coordinator.json`: how to reach the build Mac's coordinator, `{urls: [Tailscale's, then
    the LAN name's, "http://…:8090"], token}`; there while its agent runs.
  - On the build Mac, in the agent's folder, `coord/`: `workers-token` (the build's own key, 32 hex
    digits, mode 600: the Macs' agents and this Mac's menu bar and `scenic devices` use it; `token`,
    the one pages carried before devices asked, is removed as the agent starts), `page` (the build
    page's address: the status bar's "Copy the Build Page's Address"), `devices.json` (`{list: [{ask,
    id, label, hash, code, from, asked, accepted, declined}]}`, mode 600 from the start: the devices
    that asked to help through the page, `pipeline::coord::devices`; `ask`, the ask's name, 16 hex
    digits made by the coordinator and never again; `code`, the four digits its page shows; each
    secret's SHA-256 only; an accepted one's secret is its key, declined ones kept ten minutes,
    unanswered asks a day; a file that doesn't read is set aside as `devices.json.bad`),
    `leases.json` (`{next, leases: [{id, worker, work: {Job: {step, targets: [[target, key], …]}},
    progress}]}`: the jobs' leases), `costs.json` (`{unit: {peak_mb, secs}}`, `"<step> <target>"` for
    another shared step's job, and `"tail <unit>"` for a unit's last steps as a task),
    `journal/<worker>/` (the hand-offs taken, as below; `journal/raw-tiles/`, raw tiles' archives to
    name on their own), `tasks/<id>/` (a task's uploads), `pause.json` (the build's pause:
    `{pause: {mode: "drain" | "freeze", by, at} or null, at}`, `pipeline::control::Pause`, `at` when
    it last changed);
    `costs.jsonl` (what a shared step's job took, `SCENIC_COSTS`: a JSON line per target, `{unit,
    peak_mb, secs}`, `unit` the target for a unit, else "<step> <target>"; `peak_mb` the most the
    job's processes held together during that target, sampled).
  - On a helper, in the agent's folder, `outbox/<lease>/`: its leased job's saves (as below),
    `costs.jsonl`, `spec.json` (a task's), `task.json` (`scenic run-task`'s result) and `result.json`
    (`{ok, done: [step, [[target, key], …]] or null (some of the lease's targets when it paused at
    a safe point, or failed after them), failed (it failed after those: the rest held against it),
    interrupted (stopped, not failed: given back unheld), task, error}`), until the coordinator has
    them (`/work/done {…, failed}`).
  - **Pausing** (`pipeline::control`), in each Mac's agent's folder: `pause-request.json` (this Mac's
    ask, `{pause: {mode, by, at} or null (going on), at}`, from its menu, `scenic pause` or the map's
    `/api/build/pause`; taken up and removed by its agent once passed on), `pause.json` (the build's
    pause as the agent last knew it), `control` (the running job's channel: `run` or `drain`,
    `SCENIC_CONTROL`) and `done.txt` (the targets it finished, `<step> <target>` a line,
    `SCENIC_DONE`). A job that stopped at a safe point exits 75. The coordinator's answers carry the
    pause: an ask refused (409) `{error, pause}`, a beat `{ok, pause}`; `POST /work/pause {pause, at}`
    passes a Mac's ask on (one older than the last change is passed over); a helper's leased job
    keeps `work.json` (its step and targets) and `done.txt` in its outbox folder; `/work/fail {…, interrupted}` gives a lease back unheld.
    `state/build/pause.json` on the NAS mirrors the build Mac's.
  - A hand-off (`pipeline::handoff`): JSON `{changes: {logical: content name, or null when removed},
    pending: {content name: SHA-256}, checked: [content name], done: [step, [[target, key], …]] or
    null, raw: [[area, {name, bytes}], …] (a helper's raw tiles' archives, on the NAS, for the build
    Mac to name; left out when none)}`, named `<ns>-<pid>.json` in the order written, each after the
    last; `<folder>.merged`
    holds the last merged, by name; a `.bad` file is one set aside unparsed.
  - For a helper on an older app: `state/build/claims/<step> <target>` (the target's `/` as `-`,
    e.g. `unit 6-31-20`; the claiming agent, "<host> <pid>"; fresh while its mtime is within 15
    minutes) and `state/build/handoff/<host>/`.
  - `state/build/writer` (the build Mac's name: the records' one writer); `state/helpers/<host>.json`
    (a helper's status, as the heartbeat's; the heartbeat lists those fresh within ten minutes as
    `helpers`, and the workers the coordinator heard from in two minutes as `workers`).
- **State:** `state/status.json` (the agent's heartbeat, written on a change and at least every two
  minutes: conditions, the job, its parts (`parts`, `part`: the one it's on) and its progress, why
  it's frozen (`paused`) or stopping at its next safe point (`pausing`), what waits (one about a
  job: its `step`), the build's `pause` while it's paused, the checklist to the end: each step's
  `done`/`total` or `left`, its jobs left by name (`next`) and why it waits (`note`));
  `state/build/{manifest,jobs,pending,summaries,pause}.json`.
- **The app:** `app/current.json` and `previous.json`: `{version, files, sha256}`.
