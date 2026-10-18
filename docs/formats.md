# Formats

The contract between the build steps, the NAS, the server and the browser (design: docs/plan.md).
All integers are little-endian. Coordinates are i32 1e-7 degrees (`E7`) unless stated. A tile key
is `roadcore::archive::tile_key(z, x, y) = z << 58 | x << 29 | y`.

Bump a format's version on any incompatible change; readers accept the current and previous
version (plan §8, Format bumps). Within a container's version 1, additions are told apart by
section names and meta keys, and readers take both forms: base packs' `elevu` (else `elev`), road
values' `byroad`, hidata's `railinfo` and meta `lsum`.

## Names and hashes

- **File hash:** BLAKE3 of the whole file, first 16 hex digits. Content-named file:
  `<logical>.<hash16>.<ext>` (e.g. `layers/roads/hi/6-32-21.0a1b2c3d4e5f6071.pack`). The logical
  name never contains a dot.
- **Blob hash:** XXH3-64 of the stored bytes (the tile as served, still gzip'd), hex in ETags.
- Files are written to `<name>.tmp`, hash-checked, then renamed. A content-named file is never
  rewritten.

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
             (zoom ranges are the catalog's)
…            blobs (identical blobs stored once; entries may share an offset)
index        count × Entry (32 bytes), sorted by key, 8-byte aligned:
             u64 key, u64 offset, u32 len, u32 raw_len (0 if unknown), u64 xxh3
```

- Root pack: z0–2 (root "0/0/0"). Lo pack: z3–8 under one z3 tile. Hi pack: z9–14 under one z6
  tile. A tile lives in exactly one pack.
- Readers fetch header and index once (two range reads) and keep the index locally
  (`idx/<file hash>.idx`), so serving a tile is one range read.

## Sectioned file (`.sect`, RDSECT v1): named arrays

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

A local file is mmapped and its sections cast in place. A NAS file is read through the I/O pool in
256 KB pages, or whole sections for scans.

## Units

Every unit is a z6 tile. A way's owner unit is the z6 tile of its first vertex.

## Base pack (`base/<z>-<x>-<y>.<h>.sect`): what one unit owns

Meta: `{"fmt": 1, "unit": "z/x/y", "ways", "verts", "samples", "extent": [w, s, e, n] (E7, all owned
geometry), "source": "pass:<date>", "scenic": bool, "drape": bool, "summary": {…}}`. `summary` is
what the map's meta adds up: `extent`, `ways`, `vertices`, `elev_min`/`elev_max` (roads, metres),
`hist` (road km by 10 m of elevation, 256 bands), `rail_km` (track km per service group); the
catalog works it out from the sections where missing.

| Section | Record | Notes |
|---|---|---|
| `ways` | `WayRec` (48 B, roadcore) | sorted by (z9 key, Morton) of first vertex; `vstart` local; `name`, `ref_`, `surface`, `route` index `strings` |
| `verts` | `[i32; 2]` | densified geometry |
| `elevu` | `u16` | elevation, decimetres + 5,000 (`final.u16`); older packs: `elev`, `i16` dm clamped at ±3,200 m. Readers take either (`roadcore::elev`) |
| `raw` | `f32` | raw DEM sample, NaN = none |
| `grade` | `u8` | \|grade\|, 0.5 % units |
| `src` | `u8` | DEM source: the dataset's declared `code` (docs/inputs.md §6.8), named by the catalog's `dem_labels` |
| `scenic` | `[u8; 13]` | per-vertex channels (`roadcore::scenic::ch`) |
| `drape` | `i16` | drape height, metres |
| `strings` | UTF-8 lines | line 0 is "" |
| `samples` | `Sample` (24 B) | `way` local; grouped by way, in way order |
| `samplech` | `[u8; 13]` | per sample |
| `sub9` | `(u64 z9 key, u32 first way, u32 way count)` | ways grouped by z9 tile of their first vertex |
| `rail` | `(u32 way, u32 relation lo, u32 relation hi, u32 pad)` | rail ways' primary route relation |

## Road values (`global/roads/<z>-<x>-<y>.<h>.sect`): per owned way of a unit

Section `roads`, one 24-byte record per way of the unit's base pack, in the same order:

```
u64 road id      the lowest OSM way id of the road
f32 road length  metres
f32 offset       metres from the road's start to this way's start, along the road's direction
u8  dir          0: the way runs with the road; 1: against it
[u8; 7] pad
```

Section `byroad`: `(u64 road id, u64 way index)` for every way, sorted (the server sorts `roads`
itself for files without it).

Chaining (plan §6): at each node, way ends of the same kind pair by mutual best continuation.
Levels, best first: same ref (any shared token; two ways whose refs share none never pair), else
same name, else same class when both are unnamed, else (level 0) a way with neither name nor ref
continuing one that has either, same class within 35°. Straightest first within 100° (35° at level
0); a oneway only in its direction; ties to the higher level, then the straightest, then the lower
way id. Rail pairs with rail by line identity (the name before ':', else the first service);
ferries and ways of fewer than 2 vertices never chain. A path walks from its end whose way has the
lower id; a cycle starts at its lowest way id, in that way's direction.

## Hi data (`hidata/<6>-<x>-<y>.<h>.sect`): per z6 pack tile T

| Section | Record | Notes |
|---|---|---|
| `here` | `Here` (40 B) | every way whose box meets T, sorted by `id` |
| `ends` | `(u64 point, u32 here index, u32 pad)` | both end vertices of each `here` way, sorted by point (`(lon as u32) << 32 \| lat as u32`) |
| `parts` | `Part` (32 B) | one road's consecutive samples inside T, ended at gaps over 300 m |
| `psamples` | `PSample` (24 B) | the parts' samples, part by part |
| `pch` | `[u8; 13]` | per part sample |
| `railinfo` | `RailInfo` (32 B) | rail ways' lines, sorted by `here` (without it the server reads base packs) |
| `railstr` | newline-separated | their names and routes (0 is "") |
| `lparts`, `lrparts` | `LPart` (24 B) | zoomed-out summaries (meta `"lsum": 1`; docs/phase5.md): roads of 2 km or more, and rail |
| `lbins`, `lrbins` | `LBin` (64 B) | their bins: up to 500 m of a part with one set of drive filter attributes (rail: one way) |

```
Here    { u64 id; u64 owner (unit tile key); u32 index (in the owner's base pack); u8 class;
          u8 flags (WayRec flags); u8 extra (bit 0 unnamed, bit 1 rail); u8 pad;
          [i32; 4] bbox; }                                                       // 40 bytes
Part    { u64 road; f32 offset (of the first sample along the road); u32 first (psamples index);
          u32 count; f32 road_len; u8 class;
          u8 flags (bit 0 unpaved, bit 1 toll, bit 2 unnamed); [u8; 6] pad }    // 32 bytes
PSample { u32 way (here index); f32 offset (along the road, m); i32 lon; i32 lat; f32 eye;
          u8 flags (sflag); [u8; 3] pad }                                         // 24 bytes
LPart   { u64 road; u32 first (bin index); u32 count; f32 road_len; u32 pad }    // 24 bytes
LBin    { u64 way (road bin: its middle sample's way; rail: its own); f32 off0; f32 len;
          i32 lon0, lat0, lonm, latm, lon1, lat1 (first, middle, last samples); u32 rinfo (rail:
          railinfo row, else u32::MAX); u16 n (samples); u8 class; u8 flags (as Part's; rail 0);
          [u8; 12] comp (component means × 255: drive components, or ride components but trains
          a day); [u8; 4] pad }                                                   // 64 bytes
RailInfo { u32 here; u32 colour; i64 rel (primary route relation, 0 none); u32 name; u32 route
          (railstr indexes); u8 rail (service bits); u8 class; [u8; 6] pad }      // 32 bytes
```

The record types are `roadcore::packs`.

## Landmark points (`markdata/6-<x>-<y>.<h>.sect`): per z6 tile (docs/phase5.md)

Every point of the 8 kinds (`pipeline::marks::KINDS`) positioned in the tile, sorted by kind, then
id. Meta: `{"fmt": 1, "tile": "6/x/y", "block": 256, "fields": {kind: [prop, …]}}` (a reader
checks the filter properties against its own).

| Section | Record | Notes |
|---|---|---|
| `kinds` | `[u32; 2]` × 8 | per kind: first row, count |
| `ids` | `u64` | the point's id (phase5.md "Ids"), sorted within its kind |
| `pts` | `MarkPt` (28 B) | |
| `fvals` | `f64` | per kind, per field (column), its rows' values; NaN none |
| `props`, `props_idx` | zstd blocks; `[u64; 2]` (offset, length) per block | 256 rows a block, an objects block of lean properties |
| `info`, `info_idx` | likewise | popup records (empty: none) |
| `summits` | `SummitRec` (24 B) | named peaks with a height, with their place in the worldwide list |
| `summit_names` | objects block | their names |

```
MarkPt    { i32 lon; i32 lat; f32 fa; f32 ia (20000 unknown); f32 mz (NaN none); u32 rank (the
            kind's order); u8 kz (lowest zoom whose thinned tile keeps it, 6 none); u8 class;
            u8 tier (heritage, TIERS); u8 flags (1 named, 2 World Heritage component,
            4 picnic site) }                                                     // 28 bytes
SummitRec { u32 rank; i32 lon; i32 lat; u32 pad; f64 ele }                        // 24 bytes
objects block: u32 count, u32 × (count + 1) offsets, then the JSON objects back to back
```

## Marks tile (RDMT v1): thinned tiles, z6 blocks, served gzip'd

`layers/marks-<kind>/{root,lo}` packs hold the thinned tiles of zooms 0–5 (encoding `rdmt`); the
server makes z6 blocks from markdata. Each column starts 8-byte aligned.

```
0   "RDMT"; u32 version (1); u32 n (points); u32 nf (fields); u32 nc (speck cells); u32 props bytes;
    u64 reserved
32  f64 ids[n]; f64 fvals[nf][n]; i32 lon[n]; i32 lat[n]; f32 fa[n]; f32 ia[n]; f32 mz[n];
    u32 rank[n]; u8 kz[n]; u8 class[n]; u8 tier[n]; u8 flags[n];
    u32 cell code[nc] (Morton code within the tile at zoom z + 10); u32 cell count[nc];
    u8 cell tier[nc]; u32 props offsets[n + 1]; the props (a JSON object per point)
```

A thinned tile at zoom z holds the points with kz ≤ z, the rest as speck cells. Served props carry
`main`/`sub` (and `cmain`/`csub` from `cn`).

## Overlays, stations and ferries by view (pipeline::ovconv; docs/phase5.md)

Made by the `overlays`, `stations` and `ferries` jobs; `overlays` also writes
`global/heritage/{layer-summary,heritage-sources}`.

- **Areas** `layers/ov-{heritage-areas,indigenous,special,whs}/{root,lo,hi}` (encoding `mvt`,
  z0–12; hi tiles within the coverage + 20 km): extent 4096, layer `a`; the feature id is its id
  (phase5.md "Ids"), with the lean file's properties and (heritage areas, Indigenous lands, special
  areas) `own` ("3/x/y", the ovdata holding its details); a World Heritage outline's id is its site
  dot's, with `px`, `py` its place. Served with `main`/`sub` from `name` (outlines: `n`).
- **`ovdata/3-<x>-<y>.<h>.sect`** per z3 tile (owner: the tile of a feature's box centre): per key
  (`harea`, `indigenous`, `special`, `parks`) `<key>.ids` (u64, sorted; parks: their order),
  `<key>.offs` (u32, n + 1), `<key>.recs` (JSON records end to end); meta `{"fmt": 1, "tile":
  "3/x/y", "records": {key: n}}`.
- **Stations** `layers/stations/{root,lo,hi}` (encoding `mvt`, z0–12): layer `s`, `n, en, g, m, sp,
  mz` and the feature id; a tile at zoom z holds the stops shown up to z + 1 (`mz` ≤ z − 2.585),
  zoom 12 every stop.
- **Ferries** `layers/ferries/{root,lo}` (encoding `geojson-gz`, blocks at zooms 0, 3, 6):
  `{"type": "FeatureCollection", "features": […], "lines": {id: record}}`, gzip'd: the ways whose box
  meets the tile grown by 30 km (id `way × 4 + 1`, whole length `km`; simplified to 5 km at zoom 0,
  300 m at 3), terminals within 30 km (from zoom 3), the records of its ways' lines.

## Water (`layers/water/{root,lo,hi}`; pipeline::water)

Each pixel's exact share of water, z0–9, from the basemap's z14 water (plan §6, Water). Encoding
`water-png`, blobs not gzip'd: 512 × 512 8-bit grey-and-alpha PNG, grey the sea's share
(OpenMapTiles class `ocean`), alpha the inland water's (the rest of `water`, tunnels left out),
255 whole. `water::bytes` keeps what a pixel holds however little: grey 255 only where there's
nothing but sea, grey + alpha ≥ 255 only where there's no land; a share under half a byte rounds
the other way to keep that. A z10 tile is drawn from its 256 z14 tiles (`watercov::Raster`), each
coarser pixel the mean of four. A tile of one value throughout isn't stored: its stored ancestor
says which.

Served at `/tiles/water/{z}/{x}/{y}` (z0–18, crates/server/src/water.rs): stored tiles re-encoded
in the form asked; deeper ones drawn from the z14 tiles as the build draws them (or, when the
basemap can't be read, the nearest stored zoom scaled up, uncached). `?c=<sea>,<lake>[,<land>]`
(hex; else 400): an RGBA PNG in those colours, alpha their sum, or with the land colour the alpha
whose blend has the lightness of the colours mixed as light (`alpha_for`); `?raw=1`: RGB, red the
sea's share, green the inland water's.

## Lakes filled from Canada's own data (`work/lakefill/<date>.<h>.sect`; pipeline::lakefill)

Lakes OSM lacks in Canada's unimported map sheets (plan §6, Water), from CanVec. Meta `{"fmt":
"lakefill-1", "grid_z": 8, "lakes", "rings", "points", "source", "layer", "field", "keep",
"intermittent", "rules": {"tol_m"}, "gap_sheets"}`. Points are Web Mercator world units × 2³² (u32);
outer rings clockwise on the map, holes anticlockwise.

| Section | Record | Notes |
|---|---|---|
| `lakes` | 32 B: `id` (u64), `bbox` [u32; 4] (x0, y0, x1, y1), `first_ring`, `nrings` (u32) | `id`: bit 62 set (never an OSM id), bit 61 intermittent (drawn, not flattened), the rest a hash of its points |
| `rings` | 8 B: first point, count (u32) | a lake's rings in a row |
| `points` | 8 B: [u32; 2] | no closing point |
| `grid` | u32 × (256² + 1) | per z8 cell (row by row) its first entry in `cells`, then the total |
| `cells` | u32 | lake indices, ascending per cell; a lake in every cell its box meets |

The catalog names it in `global` (`"lakefill"`); the server draws its lakes into the water's deeper
zooms as a z14 tile beside the basemap's (layer `water`, class `lake`, extent 4096, clipped to the
tile grown by a sixteenth).

## 3D buildings (pipeline::bld; docs/buildings3d.md §3.4)

**Normalized buildings** (`work/bld/6-<x>-<y>.<h>.sect`, in the manifest, not served): every
building and part of the pinned Overture release whose centroid is in the z6 tile, underground ones
left out (`bldprep`). Meta: `{"fmt": 1, "tile": "6/x/y", "release", "buildings", "parts", "srcs",
"classes", "subtypes", "roofs", "ghsl": "R2023A", "read"}`: the counts; the sorted string lists the
codes index (0 none, k the k-th: height source datasets, classes, subtypes, roof shapes); `read`,
what bldprep.py read (`files`: [name, ETag, [row groups]], `ghsl`, `rows`).

| Section | Record | Notes |
|---|---|---|
| `index` | `(u64 z14 tile key, u64 offset, u32 len, u32 count)` | a block per z14 tile holding a centroid, sorted by key |
| `blocks` | zstd blocks (level 9) | each a z14 tile's records, sorted by Overture id (UUID as a number), column by column |

A block: `u32` records, polygons, rings, vertices; then the columns: centroid (`[i32; 2]`,
area-weighted as GEOS, from the WKB's doubles), footprint area (`f32` m²: degrees² × a degree's
metres² on a 6,371,008.8 m sphere × cos lat), polygons per record (`u32`), rings per polygon
(`u32`, first the exterior), vertices per ring (`u32`), vertices (`[i32; 2]`, each ring's first
absolute, the rest deltas; no closing point), `h` and `m` (`u16` dm: `height`, `min_height`, 0
none), `f` and `mf` (`u8`: `num_floors`, `min_floor`, 0 none, 255 for ≥ 255), class, subtype, roof
shape (`u8` codes), flags (`u8`: 1 a part; 2 a building whose parts are drawn: `has_parts`, with a
non-underground part naming it read in the tile or 0.02° around), the height's dataset (`u8` into
`srcs`: the source whose property is `/properties/height`, else the footprint's), GHSL ANBH at the
centroid (`u16` dm, 0 none), the OSM id where OSM gave the footprint (`u64`: 1 << 62 a way, 2 << 62
a relation, or'ed with the id; 0 none).

**The sources' indexes** (written by dem/bldfetch.py beside the downloads, outside the manifest;
read by `pipeline::bld::sources`): `sources/overture/<release>/buildings.json` (`{"fmt": 1,
"release", "source", "terms", "coverage": {"regions", "margin_km", "at"}, "files": {<name>: {"size",
"etag", "rows", "row_groups", "rows_near", "bbox", "checked"}}}`); `footers.json.gz` (every file of
the release: `{"release/<release>/<name>": {"etag", "size", "rows", "rgs": [[w, s, e, n, rows],
…]}}`); `sources/ghsl/R2023A/index.json` (`{"fmt": 1, "product", "source", "terms", "tiles": {<zip>:
{"size", "bbox", "checked"}}}`). `bld-fetch` passes the coverage as GeoJSON (`--coverage`: a
feature per outline, `buffer_m` its buffer).

**bldprep.py's stream** (stdout to `scenic-build bldprep`): `BLDP1\n`, then frames, each `u8 kind,
u32 header length, header JSON` (with `cols`: [[name, bytes], …]) and the columns' bytes: GHSL
windows (3; all before any row group), each row group's buildings (1) and parts (2), the end (9).
dem/bldprep.py's docstring lists the columns.

**Tiles** `layers/buildings/hi/6-<x>-<y>` (RDPACK, encoding `mvt`, blobs gzip level 6, z14 only):
MVT 2.1, extent 4096, layer `b`, a feature per building or part touching the coverage (centroid or
a vertex in it, with the 1 km buffer), whole in the tile of its centroid (not clipped). A building
or outline (not a part) is also copied whole into each other tile its exteriors reach (`o` 1), from
up to 310 m beyond a z8 area's edge. Quantized to the tile grid; repeated points dropped; rings of
fewer than 3 points or no area dropped (an exterior with its holes); exteriors positive, holes
negative. No edge parallel to an axis beyond the extent: such an edge gets points a unit off it,
outside the ring, at MapLibre's subdivision lines (every 2,048 units) and midway, and a slanted edge
there a point at each such line; inside the ring where outside would cross a ring, else none.
Properties (uint), keys in this order, values in order of first use:
- `h` top (dm); `m` base (dm; left out when 0);
- `s` height source: 0 measured, 1 floors, 2 Microsoft's estimate, 3 neighbours, 4 GHSL, 5 size and
  kind; `f` floors (when `s` is 1);
- `c` kind: 0 unknown, 1 residential, 2 outbuilding, 3 commercial, 4 industrial, 5 religious, 6
  civic, education or medical, 7 agricultural, 8 transportation, 9 other;
- `k`: 1 a part, 2 a building drawn by its parts (flat layer only); left out when 0;
- `o`: 1 a copy; left out otherwise.

No names. Feature id: x << 34 | y << 20 | the index among its own z14 tile's records (a copy has its
building's id). Features sorted by their centroid's Morton code in the tile (12 bits an axis), then
id.

**The field** (`pipeline::bld::field`, docs/buildings3d.md §4.8): every building and part extruded
(not an outline with parts, not a copy) as densities on 512 × 512 texels a tile, levels 4–14 (a
level's tiles are that zoom's), in five classes by top (under 8 m, 8–20, 20–40, 40–100, 100 m and
up): `f` the texel's share under the class's roofs (exact areas, summed, clamped to 1), `w` Σ wall
length × (top − base) ÷ texel area, `v` Σ roof share × top (m), `ox`, `oy` Σ roof share × the offset
from the texel's centre to the building's centroid (area-weighted, in the level's texels); `v`,
`ox`, `oy` scaled with `f`'s clamp. Each level is the mean of the four under it (offsets taken from
the coarser texel's centre).
- **Hi packs** `layers/buildings-field/hi/6-<x>-<y>` (RDPACK, encoding `field5-webp`, not gzip'd),
  levels 9–14, made by `bldtiles`.
- **Lo packs** `layers/buildings-field/lo/3-<x>-<y>`, levels 4–8, made by `bldfield-lo` from the
  mids. (The server takes zooms 3–8 from lo packs.)
- **`field5-webp`**, a tile: a byte of the classes held (bit k class k), then a lossless WebP, 512
  wide and 2 × 512 × (classes held) tall, opaque RGB. First a 512-row block per class held: R
  `round(√f × 255)`, G `round(log2(1 + w) ÷ log2(4097) × 255)` (≤ 255), B the mean top `v ÷ f` as
  `round((H − lo) ÷ (hi − lo) × 255)` clamped, ranges 0–8, 8–20, 20–40, 40–100, 100–1000 m (0 where
  R and G are). Then a block per class held: R `round(ox ÷ f) + 128`, G `round(oy ÷ f) + 128`, B 0
  ((128, 128, 0) where R is 0). A class with R and G 0 everywhere is left out; a tile with none
  isn't stored. A level-8 tile may carry after the WebP its heritage list: u32 n, then n buildings
  or parts holding a heritage site's point (the newest pass's `pos` slice of T less its `parts`
  slice), each u16 top (dm), u16 base (dm), u8 m, m points (u16 x, u16 y in the z8 tile's 65,536 a
  side) of its first polygon's exterior, simplified (Douglas–Peucker, tolerance doubling from half
  a unit) to at most 16.
- **Mids** `work/bldfield-mid/6-<x>-<y>.<h>.zst`: zstd (level 3) of `BFMID03\0`, then each z8 area
  with a building, by x then y: u32 x, u32 y, u32 heritage list bytes and that list, u8 class mask,
  each held class's `f`, `w`, `v`, `ox`, `oy` planes as 512 × 512 f32 (the level-8 values exactly).

**A z8 area's task** (`pipeline::bld::task`, docs/buildings3d.md §3.6; a folder on the job's Mac,
sent to the worker as `u/…`, not in the manifest): `6-<x>-<y>.sect` for each of T's and its 8
neighbours' normalized files with a block the area reads (its own and those within 620 m), each
with the same meta and only those blocks as stored; `coverage.sect` (meta `{"fmt": 1, "shapes":
[{"source", "country", "buffer_m", "rings": [vertex counts]}, …]}`, section `verts` `[i32; 2]`): the
shapes whose buffered box meets the box of every point the area asks about, whole, in the recipes'
order; `heritage.bin`, T's heritage points (`[i32; 2]` pairs, sorted). `bldtile` writes there
`area.tiles` (RDTILES, meta `{"layer": "buildings", "area": "8/x/y", "encoding": "mvt"}`),
`area.field` (RDTILES, `{"layer": "buildings-field", …, "encoding": "field5-webp"}`, levels 9–14),
`area.mid` (a mid of the area alone) and `area.json` (`bld::job::Summary`).

## Other tasks' folders

Each a folder on the job's Mac, its files sent to the worker as `u/…`, not in the manifest.

- **A row of tree cover blocks** (`pipeline::trees::task`, plan §6 Trees): `coverage.json`
  (`{"shapes": [[ring, …], …]}`, rings `[[lon, lat], …]`, only those whose box meets the row's box
  grown by 0.01°). `trees --blocks` writes `8-<x>-<y>/` per block: `trees-cover.tiles`,
  `trees-height.tiles`, `trees-leaf.tiles` (RDTILES, meta `trees::META`: zoom 8–12 tiles) and
  `trees-tops.bin` (`pyramid::Tops::to_bytes`).
- **A terrain piece's subtrees** (`pipeline::terrain_task`, plan §6 Terrain): `in.sect`, meta
  `{"fmt": 1, "step": "terrainsub", "v": TERRAIN_V, "piece": "6/x/y", "subtrees": "8/x/y,…",
  "levels": [[z, [[x, y], …]], …] (z12 → z9), "water", "north", "coarse", "cells": [[lat, lon, "sea"
  | "missing" | "cell", width, first row, rows, first column, columns], …]}` (GLO-30 cells and the
  windows read, two pixels' margin); sections `r-<z>-<x>-<y>` (a raw PNG tile; none: AWS lacks it;
  with "coarse", AWS's z9 tile over each of z10–12), `w-<z>-<x>-<y>` (water polygons: u32 count,
  each u8 kind (1 sea, 2 lake), u64 key, u32 ring count, each ring u32 point count and f64 pairs)
  and `c-<lat>-<lon>` (a cell's window, zstd: f32 heights row by row, then a byte a pixel, filled
  from another DEM or not). `terrainsub` writes `hi.sect` (meta `{"fmt": 1, "step": "terrainsub",
  "piece", "subtrees"}`, a section `t-<z>-<x>-<y>` per tile as the hi pack holds it) and `mid.sect`
  (the group's part of `work/terrain-mid/6-x-y`).

## Names (docs/plan.md §7)

- **Translation lines** (`inputs/translations/**/*.jsonl`, not `todo/` nor `how/`; a drop box of the
  input check, docs/inputs.md §6.3): `{"n", "kind", "langs", "main", "sub", "via"}`: `kind` road,
  settlement or other, or a list; `langs` an ISO 639 language (first subtag counts) or a list;
  `main` required, null or empty for the name itself; `sub` null for none; `via` free text, "todo"
  or "skipped" leaving the line out. Lines without `kind` or `langs` are errors. Converted area
  tables: `inputs/translations/0-converted/<language's English name>.jsonl`, with
  `how/conversion-log.txt`.
- **Descriptions** (`inputs/descriptions/**/*.jsonl`, §6.4): `{"qid": "Q…"}` or `{"id":
  "n…"|"w…"|"r…"}`, `long` (unless `"drop": true`), `src` a string or a list of `{"t", "u"}`;
  `about.toml` beside them (§5.1).
- **Labels** (`layers/labels`, layer `l`): `n` name, `en` its English (`name:en`, else romanised),
  `kana` (when no English), `l` OSM's languages of the name (comma-separated), `o` OSM object
  (`n123`, `w123`, `r123`), `k` kind, `c` class, `mz`, `ms`, `s` (dem/labels.py). Before `LABELS_V`
  2: `n`, `en`, `k`, `c`, `mz`, `ms`, `s`.
- **To translate** (`inputs/translations/todo/<language>.jsonl`, `names-todo`'s, rewritten after
  each catalog): `{"n", "kind", "langs", "things", "example": {"osm" | "qid" | "label", "at": [lon,
  lat]}, "priority"}`, by priority; with `README.md` and `check.py`.
- **Lines with no timetable** (`inputs/timetables/gtfs/todo/lines.jsonl`, `timetables-todo`'s):
  `{"osm", "kind" ("rail" | "ferry"), "name", "ref", "operator", "from", "to", "km", "priority"}`;
  with `README.md` (docs/inputs.md §6.7.3).
- **To describe** (`inputs/descriptions/todo/landmarks.jsonl`, `areas.jsonl`): `{"qid", "id",
  "name", "en", "kind", "designation", "at", "enwiki", "wiki", "register", "source", "fame"}`
  (landmarks; `source` the heritage register's id, docs/inputs.md §6.5) and `{"qid", "id", "name",
  "kind", "bbox", "area_km2", "enwiki", "fame"}` (areas), by fame; with `README.md`.

## Depth (`layers/depth/`, `layers/depth-class/`; pipeline::depth)

Two layers of 256-px PNG tiles in packs like the terrain's: `lo/3-x-y` (z3–8) and `hi/6-x-y` (z9–16,
only where a source is finer than z8). A tile is stored where one of its water pixels has data of
its zoom (every water tile up to z8); a missing one is its nearest stored ancestor scaled up (bilinear
for depth, with that ancestor's neighbours' edges; nearest for classes).

- **`depth`** (encoding `terrarium-png`): metres below the water's surface (the sea's mean level, a
  lake's mean surface), negative; drying heights positive (at most 20 m); +12,000 m where there's
  none (≥ 9,000 is none). Quantised: 0.1 m in the first 100 m, 1 m deeper. The terrain's water
  (`terrain_water`: half a pixel or more of sea or lake) is where it lies, and the land keeps a band
  of 3 pixels of the nearest water's value (≥ 0) so a finer shoreline never meets a gap.
- **`depth-class`** (encoding `class-png`, 8-bit grey): bits 0–2 the class (0 none or land, 1
  measured, 2 interpolated, 3 estimated, 4 filled to the shore), bit 3 dries at low tide, bits 4–7 the
  source's `code` (docs/inputs.md §6.10).
- **The catalog's meta** `depth`: `{"sources": [{"code", "id", "label", "datum", "water", "notice"}]}`,
  the accepted depth declarations (the map's attribution and readout).

## Catalog (`catalog/<n>.json.zst`)

zstd with its content checksum; written as `.tmp`, then renamed. Readers take the highest `<n>`
that decodes and parses, else the next.

```json
{
  "fmt": 1, "n": 7, "created": "2026-10-03T04:05:06Z", "app": "<published app version, or development>",
  "files": {"<logical>": {"file": "<content name>", "size": 123, "fmt": 1}},
  "units": ["6/32/21"],
  "layers": {
    "terrain": {"encoding": "terrarium-png", "minzoom": 0, "maxzoom": 12, "root": "<logical>",
                "lo": {"3/4/2": "<logical>"}, "hi": {"6/32/21": "<logical>"}},
    "roads": {"encoding": "rt7", "minzoom": 4, "maxzoom": 14, "lo": {…}, "hi": {…}}
  },
  "basemap": ["<logical of a .pmtiles>"],
  "base": {"6/32/21": "<logical>"}, "roads": {…}, "hidata": {…}, "markdata": {…},
  "ovdata": {"3/4/2": "<logical>"},
  "global": {"railfreq": "global/railfreq", "roadunits": "global/roadunits", "marks/summary": "…",
             "heritage/<stem>": "…", "outlines": "sources/osm/<date>/outlines"},
  "meta": {"…": "minzoom, maxzoom, bounds, ways, vertices, elev_min, elev_max, elev_hist_10m_km, rail_km, classes, built"},
  "credits": [{"what": "Road elevation, Japan", "source": "…", "terms": "…", "areas": [[122.5, 20.0, 154.0, 46.5]]}],
  "dem_labels": {"5": "GSI lidar (5 m)"},
  "coverage": {"recorded": true, "regions": [{"id": "monaco", "name": "Monaco", "outline": ["geofabrik:europe/monaco"],
               "shapes": {"geofabrik:europe/monaco": [[[[7.4, 43.72], …]]]}},
              {"id": "kanto", "name": "Kantō", "outline": ["osm:…"], "shapes": {…}, "tiles": {"built": 12, "total": 40}}]}
}
```

- `files` holds every file the catalog references.
- `credits`: `pipeline::rules::CREDITS` plus the inputs' accepted descriptions
  (`pipeline::inputs::credits`, docs/inputs.md §4.8; a description replaces the table's entry of its
  `what`), those whose areas meet the coverage + 20 km or a built unit's extent. `areas` are w, s, e,
  n boxes in degrees, left out for credits that hold everywhere. A heritage register's credit has
  `register`, which popups name their source by (docs/inputs.md §6.5).
- `dem_labels`: the elevation datasets' labels by source byte (`src.u8`), from their accepted
  declarations (`label`, else `name`; `pipeline::inputs::credits::dem_labels`, docs/inputs.md
  §6.8): how a road's details name its elevation sources. Left out when none; a catalog without
  it gets "source <n>".
- `coverage`: the regions built for: those done at publish time as their recipes then were
  (`--ready <id>=<outline digest>,…`), and those not done yet as the last catalog had them, in the
  recipes' order. A region still building (the agent's `--building <id>=<built>/<total>,…`: every
  region its plan doesn't record built, with its units built of those it builds, by the records)
  that the last catalog didn't have built is recorded as its recipe is now with `tiles`
  (`coverage::DrawnRegion::tiles`): its units are on the map, and downloadable, as they come. A
  built region has no `tiles`; an app made before them reads such a region as built, and this app
  reads a catalog without them as before. Outlines are GeoJSON MultiPolygon coordinates in degrees to 5 decimals, rings
  closed: `osm:` entries from the pass's simplified outlines, others simplified by size (60 m–1 km);
  an unreadable entry has no shape. A catalog with `"credits": []` and no `recorded`: the server
  gives every credit and builds the coverage from the recipes.
- GC's roots: the newest catalog, every catalog of the last 14 days and the build manifest; an
  unreferenced file goes once also older than 14 days (plan §3). A held catalog is written to
  `catalog-held/` (`inputs/hold-catalog`).

### Delta catalogs (`catalog/<n>.delta.json.zst`)

With `state/pool/delta-catalog` on (plan §8, Publishing's switches), catalog `n` may be written as a
patch on an earlier one (`store::catalog::Delta`), compressed and written as a whole one is; a
number is used once, whole or as a delta:

```json
{"fmt": 1, "n": 8, "of": 7, "of_digest": "<32 hex>", "digest": "<32 hex>", "patch": {"o": {"n": {"=": 8}, "files": {"o": {"layers/roads/hi/6-32-21": {"=": {…}}, "base/6-1-1": null}}, "coverage": {"o": {"regions": {"a": {"ids": ["monaco", "andorra"], "set": {"andorra": {"=": {…}}}}}}}}}}
```

- `digest`: XXH3-128 (hex) of a whole catalog's JSON as written (maps in key order): `of_digest`
  the base, `digest` the result.
- `patch`: `{"=": v}` the value is `v`; `{"o": {key: patch | null}}` an object, each named key
  patched (null: removed), the rest kept; `{"a": {"ids": [...], "set": {id: patch}}}` an array of
  objects with distinct string `id`s, in `ids`' order, those in `set` patched (from nothing, if
  new). The writer takes whichever form is shortest.
- Readers (`store::catalog`: `latest`, `latest_nas_from`, `read_n`) take the highest number that
  reads whole: a delta applied to catalog `of` (itself read the same way), checked against both
  digests; one that doesn't fit is passed over as damaged.
- A delta lists each file's size as the records have it (`state/build/sizes.json`, from its job's
  hand-off) or the last catalog does, looking at the NAS only for a file neither has (one written
  before sizes were recorded: once). A whole catalog looks at every file, and stops on one missing
  or of another size than recorded.
- A whole catalog is written in idle time once the deltas since the last whole one add up to its
  size or their run is half `MAX_CHAIN` (`store::catalog::whole_due`: the agent's `catalog --whole`
  job, nothing else to do), when there's none, when a delta's run nears `MAX_CHAIN`
  (`store::catalog::chain_full`), or on `scenic-build catalog --whole`.
- GC also keeps every catalog a kept delta is read through, back to its whole one. A Mac keeps the
  catalogs it reads whole, in its own `catalog/`.

## On each Mac (`~/Library/Application Support/scenic/`)

```
bin/scenic-launcher     the launcher (never rebuilt)
run/<name>              what the launcher runs, one argument per line: server, agent (written by
                        Scenic.app when it opens)
agent/quit-request.json Scenic's ask to quit (control::Quit: finish or now), until the agent exits
app/<version>/          server, scenic, scenic-build, extract, tile, scenic-metrics, dem/, Scenic.app,
                        web/, fonts/ (app/current → the one in use); dem/.venv made by uv from
                        dem/uv.lock on first use
app/current.json, previous.json   {version, files, sha256}
mirror/<content name>   downloads, named as on the NAS (plan §4, Mirror; .partial/: in progress)
mirror/.basemap/<hash16>/  the basemap's pieces (store::pieces): lo.pmtiles (zooms 0–10),
                        6-<x>-<y>.pmtiles (a z6 tile's 11–14), PMTiles v3 (clustered, gzip'd
                        directories, tiles as stored); sizes.json {piece: bytes}
idx/<hash16>.idx        pack indexes (RDPKIDX1: header, meta, entries, XXH3 trailer)
catalog/<n>.json.zst    the last catalogs read
accepted/<unit>/        accepted inputs copied from the NAS: @index.json ({"version", "index"}) and
                        each file by content name (pipeline::inputs::served)
names/spoken-<content>.bin   languages spoken where, for a catalog without `global/spoken`
                        (names::spoken::Spoken::to_bytes: "SPOKEN01", u32 head length, head (rules'
                        version, then per region its ISO code, a tab, its languages), u32 run
                        count, 23,041 u32 row starts, runs of (u32 first column, u16 region))
regions.json            the last regions read
reserve.json            the mirror's reserve {gb} (crates/server/src/reserve.rs); default 30 GB
downloads.json          {fmt: 2, world: at | null, regions: [{id, name, at}], views: [{id, name,
                        outline: [[lon, lat], …], at}]} (crates/server/src/downloads.rs; `at` unix
                        seconds); one that doesn't read is set aside as downloads.json.bad; an old
                        keep.json is converted once
map-page                the address to open the map on another device (plan §4, Devices; 0600)
pool-<id>.lock          the lock of this Mac's member `<id>` (crate::pool::MemberLock)
```

The agent's folder, `agent/`:

```
timings.jsonl, timings-run.json, timings-run-<k>.json   jobs' timings (below)
status.json             the lead's status; a member not leading writes helper.json
state.json, job.json, agent.lock, logs/
publishing.json         the lead's record of its catalogs {at, low_at, held_at}: when the last
                        went out, the last with the low zooms made, and when a catalog due was
                        first held back since (none: none is) (agent::build::LastCatalog)
clear-request.json      the owner's ask to clear build caches ({by, at}, agent::room::ClearRequest;
                        renamed .taken when taken up)
room-target.json        the owner's free-disk target {bytes, by, at} (agent::room::Target; none: off)
member                  this Mac's pool member id `m-<16 hex>`, then its hardware UUID
pack-idx/<hash16>.idx   indexes of the terrain packs the manifest names (agent::tiles); never
                        trimmed; one unnamed and untouched for a fortnight goes
pool/                   crate::agent::pool: saved.json (crate::pool::driver::Saved: {member, mine:
                        {entries, unwritten}, term, led, made, unfinished?, stood_down?, passing?};
                        saved.earlier.json one set aside), mail.json ({read, sent, n}), members.json,
                        forgotten.json, jobs/<term>-<n>/ (a job under its lease: work.json {step,
                        targets, lease}, its hand-offs, done.txt, costs.jsonl, unmerged.json),
                        lead.json (crate::agent::lead::Kept), notes.jsonl
cache/                  build caches (below)
```

`agent/cache/`: `dem-cache.*` (the seed), `chm10/` (canopy files), `aws-terrarium/` (raw tiles
until packed onto the NAS; `packs/`, archive copies), `blobs/`, `base/`, `sources-*/` and `work-*/`
(local copies of NAS files by logical name), `heritage-merged-<date>-<cover>.osm.pbf`, `items/`
(the items job's answers, `kept-<date>.json`: {archive, files: {name: hash16}}; `months/`: pageview
index copies), `rail/` (stop pairs), `heritage-<date>/` (the heritage chain's answers, with its
`.kept.json`), `unit-stages.json`, and the flocks `answers.gate`, `answers.use`
(crate::answers::Local). Room-making, trimming and clearing are agent::room's (plan §4, §8). Every
file is read and made through store::cachefile (dem/cachefile.py): a shared flock while a job uses
it; made as `<name>.<pid>.<n>.tmp`, locked, and hard-linked to its name only if the name is free;
deleted only under an exclusive flock taken without waiting.

`~/Library/Preferences/nsmb.conf` gets `[FISHANDCHIPS:PERSONAL]` and
`[FISHANDCHIPS.LOCAL:PERSONAL]`, both `soft=yes`.

## Timings (`timings.jsonl`; pipeline::timings, docs/plan.md §8)

A job's run, a JSON object, `pipeline::timings::RunRec`:

```
{v: 1, kind, id, host, start, wall_s, cpu_s, threads, ok, untimed_s, overhead_s, phases: [phase, …]}
phase: {name, class, wall_s, cpu_s, n, bytes, files, overlapped, background, sub: [phase, …]}
```

- `kind`: the job's step (`unit`, `peaks`, `gc`, `backup`, `osm-pass`), or `task <kind>` (`task
  tail`, `task bldtile`, `task treeblock`, `task terrainsub`). `id`: the agent's job id
  (`SCENIC_JOB_ID`, "unit 6/32/24"; a page's task "task <lease>"). `host`: the Mac or page. `start`:
  unix seconds.
- `cpu_s`: user + system, the process's and its finished children's (none on a page). `threads`:
  RAYON_NUM_THREADS. `ok`: ended without error (a pause isn't). `untimed_s`: `wall_s` less the
  top-level non-`background` phases. `overhead_s`: the timing's own cost.
- A phase totals its `n` spans. `class`: `nas-read`, `nas-write`, `disk`, `net`, `compute`, `wait`,
  `mixed`. `overlapped`: another thread's phase ran during it. `background`: off the main thread.
  `sub`: one level of sub-phases (a child program's phases come in here). Empty, zero and false
  fields left out.

Where they're kept:
- A job writes its record once where `SCENIC_TIMINGS` says (`agent/timings-run.json`,
  `timings-run-<k>.json` for slot k's job past the first, or a member's leased task's
  `outbox/<lease>/timings.json`);
  a child program writes its phases where `SCENIC_PHASES_TO` says (removed once read).
- `agent/timings.jsonl`: every job this Mac's agent ran.
- `agent/coord/timings.jsonl` (the lead's coordinator): every worker's, sent with `/work/done`;
  read by `/work/timings {kind?, last?}` (`{runs: [...]}`, each kind's last 20 by default).
- Each log is cut to its newer half past 16 MB.

## Server API

- **Tiles:** `/tiles/{roads,rails,terrain,slope,labels,water,base,buildings,buildings-field,depth,depth-class}/{z}/{x}/{y}`,
  `/tiles/trees/{var}/{z}/{x}/{y}`. Strong `ETag`: the stored blob's hash, plus for named tiles the
  versions of the languages spoken within them and of the spoken-languages raster (plan §7);
  `/tiles/base`'s a hash of the basemap archives' content names, the tile's z/x/y and the names
  version; tiles the server makes (missing terrain and slope, slope z12) carry none. With `?v=` (the
  app's URLs): `public, max-age=31536000, immutable` while that version is current, else
  `no-cache`; the tiles the server makes (terrain and slope from ancestors or neighbours, flat
  tiles, the floor under the water, depth made from a coarser zoom) always `no-cache`, as their sources can lie in other packs. `/api/meta` versions each layer (`buildings.tiles`, `water`: the drawing's, its packs'
  and the basemap's) and, under `packs`, each pack of a tiled layer (`{layer: {root, lo: {"3/x/y"},
  hi: {"6/x/y"}}}`: its content name's hash, 12 hex digits, plus the names version for layers with
  names). The map's tiles carry their pack's token (root z0–2, lo z3–8 by z3 tile, hi z9–14 by z6
  tile: `web/src/api.ts` packVersion; MapLibre's sources through the map's request transform), so a
  catalog re-requests only the tiles of the packs it changed. Every pack token and layer version
  meta gives out is current.
- **Ways:** `/api/way/{id}?at=lon,lat`, `/api/profile/{id}?at=…`, `/api/road/{id}?at=…`: `at` picks
  the hidata of the z6 tile holding it, or a neighbour's whose `here` holds the way.
- `/api/railfreq`: sorted `(u32 way id, f32 trains a day each way)`; negative: a lower bound.
- **Landmarks** (docs/phase5.md): `POST /api/marks/view`; `/api/marks/tile/{kind}/{z}/{x}/{y}` (z ≤
  5) and `/api/marks/block/{kind}/6/{x}/{y}` (RDMT); `/api/marks/specks/{kind}/{z}/{x}/{y}?q=`;
  `/api/marks/count?kind=&q=`; `/api/marks/detail/{kind}/{id}?at=lon,lat`.
- **Overlays:** `/tiles/ov/{heritage-areas,indigenous,special,whs}/{z}/{x}/{y}`,
  `/tiles/stations/{z}/{x}/{y}`, `/tiles/ferries/{z}/{x}/{y}`;
  `/api/overlays/detail/{harea,indigenous,special}/{id}?own=3/x/y`; `/api/park`;
  `/api/layer/summary` (`global/heritage/layer-summary`). Versions `ov-<name>.tiles`,
  `stations.tiles`, `ferries.tiles`.
- Drives, rides and rail lines take `approx=1` (zoomed out): answered from hidata's summaries when
  every hidata of the view plus margin has them (drives and rides: windows of at least 2 km); the
  answer says `approx`.
- `POST /api/catalog/refresh` (this Mac's own requests alone, else 403): follow a newer catalog on
  the NAS now; answers `read` (whether the NAS was read) and `n` (the catalog served after). The
  build agent's, after writing a catalog.
- `/api/catalog`: `n`, `created`, `units` (a count), `layers` (encoding, zooms, version), `coverage`
  (without outlines), `credits`, `online`, `nas`, `held`, `app`, `agent`, `names` (`langs`, `spoken`,
  `warning`, `waiting`), `v`, `marks` (tiles with points, kinds with tiles, totals), `depth` (the
  catalog meta's: its sources, Depth below). The map's meta: `/api/meta`.
- **Depth:** `/tiles/depth/…` and `/tiles/depth-class/…`: the stored tile, else made from its nearest
  stored ancestor (`pipeline::depth::fall`), 404 outside the layer; versions `depth.tiles`,
  `depth-class.tiles`. `/tiles/terrain/{z}/{x}/{y}?floor=1`: the terrain less the depth where the
  class tile says water (the floor under the water, for a 3D floor); the terrain as it is without
  depth in the catalog.
- `/api/names?n=<name>&at=lon,lat` repeated (each optionally with `en=`, `k=road|settlement|other`,
  `l=` before its `at`): `[{main, sub}]`.
- `/api/build`: the agent's status (this Mac's when it leads, else the lead's heartbeat, or this
  member's `helper.json` when that is ten minutes stale), `pool`, `lead_asked`, and `offline:
  {world, areas, bytes, here, nas}` (null without a mirror).
- **Remote access** (plan §4, Devices): no key. A request from anywhere but this Mac, its LAN and
  the tailnet (`tailscale serve` too; never Funnel) is 403 ("not from here"), as is one whose `Host`
  isn't the map's or whose `Origin` is another page's. CORS answers only this Mac's own origins.
- **Downloads** (`downloads.rs`): `GET /api/downloads` → `{mirror, online, slow, rate, free,
  reserve, reserve_min, disk, here, world: {bytes, here, unknown, on, at, state}, wanted: {bytes,
  here, more, unknown}, copying: {what, bytes, have, slow} | null, last: {…} | null, regions: {id:
  {name, bytes, here, unknown, on, state, tiles?}}, views: [{id, name, outline, at, bytes, here, unknown,
  state}]}` (`unknown`: basemap pieces not yet sized; `state`: done, copying, queued, room, away,
  missing; `tiles`: a region still building, as its catalog has it, its files those of the catalog
  served, more with each catalog). `PUT /api/downloads/world {on}` (off refused while a region or view is downloaded);
  `PUT /api/downloads/regions/{id} {on}` (on downloads the World too); `POST
  /api/downloads/views/size {outline}` → `{bytes, here, with_world, need, room, fits}`; `POST
  /api/downloads/views {outline, name?}` → `{id, name}`; `PUT /api/downloads/views/{id} {name}`;
  `DELETE /api/downloads/views/{id}`; `PUT /api/downloads/reserve {gb}` (whole GB, 5 to under the
  disk size) → `{ok, reserve}`. A download that wouldn't fit is refused; refusals are 400 `{error}`.
- **Regions** (read only): `GET /api/regions`, `/api/regions/layer`, `/api/areas/{id}`,
  `/api/coverage` (the catalog's coverage as GeoJSON, one feature per outline entry, with `regions`
  and `catalog`).
- **Names in answers:** MVT tiles and API JSON carry `main` and, when any, `sub`; JSON layer files,
  ferry blocks and marks tiles carry `main` only where it differs from the name; popup records are
  served as stored.

## RT road tiles, version 7

As v6 (`roadcore::tile`), but the way column holds OSM way ids, and lines are sorted by (draw
class, id) within a tile. The client sends the id with the clicked point.

## Other files (listed, not specified)

- **The OSM pass** (`sources/osm/<date>/`): `planet.osm.pbf`; `filtered`; `pieces/<u>`,
  `pieces.json`; `sets/<name>[-v<n>]`; `roads/<u>.bin` (32 B a way: u64 way id, its road values);
  `outlines` (sectioned, meta `fmt` "outlines-1" and the `repaired`, `stale`, `unassembled`
  relations; `recs` (64 B `OutlineRec`), `rings`, `points`, `srings`, `spoints`, `strings`);
  `reach.json.zst` (`{fmt, date, units: {"6/x/y": {owned: [w, s, e, n] | null, long: [{owned,
  ferry, verts}], cells: [[x, y]]}}}`, E7: the box of a unit's ways within its tile + 20 km, every
  way reaching further, whole, and the 1° cells (west, south corners, degrees) its ways other than
  ferries start in; `pipeline::reach`); `pass.json`.
- **Global files:** `global/roads/<u>` (above); `global/roadids/<u>` (`ids`: the unit's road ids,
  sorted u64, each once); `global/roadunits` (`pairs`: sorted u64 road, u64 unit key; `srcs`: JSON
  `[ROADUNITS_V, [[unit key, content name of what it read]]]`); `global/railfreq` (as `/api/railfreq`); `global/marks/summary` (`{fmt, kinds,
  tiers}`); `global/roaden/<u>` (`{OSM way id: English}` where `name:en` differs); `global/spoken`
  (`names::spoken::Spoken::to_bytes`); `global/heritage/*`.
- **Grid layers:** `grid-{class,canopy,cover}` hi packs of z11 tiles, encoding `u8-zstd`, not served.
- **Worldwide z8 terrain:** `sources/terrain-z8-v3` (one RDPACK of every z8 tile, no scope or root)
  and `sources/terrain-z8-v3-max` (each tile's maximum, f32).
- **Work files** (zstd JSON lines unless said): `work/pois/<u>`, `work/peaks/<u>`,
  `work/summits/<date>`, `work/trailends/<date>`; `work/heritage/<date>/{base/<stem>, pos/6-x-y.json,
  parts/6-x-y.json, areas/6-x-y.jsonl, <stem>}` (`parts`: World Heritage components among `pos`;
  an area in each z6 tile its box meets); `work/marks/heritage-dots.json`; `work/rail/used.json`
  (per feed: `id`, `provider`, `url`, `licence`, `fetched`, `status`, and when "ok" `day`, `trips`,
  `duplicates`, `rail_routes`); `work/rail/lines.json` (`{"lines": [{osm, kind, name, ref, operator,
  from, to, km, served_km, passenger}], "unmatched"}`); `work/ferries/lines.json` (`{"lines": [{…,
  passenger, ends}], "with_timetable", "unmatched"}`); `work/timetables/report.json` (`{to_do,
  to_do_km, by_kind, file, unmatched: [{mode, source, static, route, name, agency, osm, why}]}`);
  `work/trees-mid/6-x-y` (sectioned, meta `{fmt: 1, step: "trees", tile, v: TREES_V}`; per block,
  by column then row, `<layer>-8-<x>-<y>` for cover, height and leaf, then `tops-8-<x>-<y>`:
  "TREETOP1", u32 x, u32 y, zstd of cover and height (f32 × 65,536 each) and five leaf-type counts
  (u16 × 65,536 each)); `work/terrain-mid/6-x-y` (meta `{fmt: 1, step: "terrain", tile, v:
  TERRAIN_V}`; per z9 tile `quad-9-<x>-<y>`, its 2×2 means, f32 × 128 × 128; then `lake-ids` u64 and
  `lake-levels` f32); `work/slope-mid/6-x-y` (meta `{fmt: 1, step: "slope", tile, v: SLOPE_V}`;
  `<z>-<x>-<y>`, each z6–8 tile's PNG); `work/water-idx/<hash16 of the pin>` (JSON `{fmt: 1, step:
  "terrain-water", pin, digests: {target: [from, digest]}}`: per terrain target, what decides its
  tiles and a hash16 over its water tiles' polygons; `pipeline::terrain_water::WaterIdx`).
- **Other sources:**
  - `sources/items/<date>/{facts,views,meta}.json`; `answers.tar.zst` (pipeline::answers: the
    items job's caches `facts-<date>.jsonl`, `wp-<date>.jsonl`, `fetched-<date>.json`; tar + zstd,
    rewritten whole as a step starts and ends; one that doesn't read moved aside as
    `<name>.bad-<unix seconds>`) and `heritage.tar.zst` (the heritage chain's working copy, but what
    it makes again each run).
  - `sources/buildings/<release>/8/<x>-<y>.f32` (release with a dash: `2026-09-23-1`; raw f32
    `[xmin, ymin, xmax, ymax]` in degrees per Overture building whose box centre is in the z8 tile,
    sorted, duplicates once) and `index` (`{fmt, release, zoom, tiles: {"8/x/y": count}}`, written
    last).
  - `sources/trees/leaf/lat<top>_lon<left>.tif` (a 10° square's dominant leaf type at 0.0005°: u8,
    0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data; `dem/leaftype.py`; tag `complete=1`
    when whole; chunks kept in `parts/lat<top>_lon<left>/<row>-<col>.npy` or `.none` while made);
    `sources/trees/nalcms-2020.tif` (size and CRC-32 checked).
  - Downloads kept so each is made once (temporary names end `.<host>.<pid>.tmp`):
    `sources/canopy/meta_chm_lat=<top>.0_lon=<left>.0_{median,p95,cover5m}.tif` (empty: Meta lacks
    it); `sources/aws-terrarium/packs/<area>.<hash16>.tiles` (AWS's raw tiles in RDTILES1, areas
    `6-<x>-<y>` for zooms 9–12, `3-<x>-<y>` for 3–8, `root` for 0–2; an empty entry where AWS lacks
    the tile; meta `{"kind":"aws-terrarium raw tiles"}`) with `packs/index.json` (`{areas: {area:
    [{name, bytes}, …]}, gone: {name: unix seconds}}`, archives oldest first, read newest first;
    `gone` deleted a day after), and loose `sources/aws-terrarium/<z>/<x>/<y>.png` (`.none`);
    `sources/pageviews/<YYYY-MM>.tsv.zst` (a `#langs\t<lang>,…` line, then
    `<lang>|<Title>\t<views>` lines; dem/pageviews.py); `sources/fetched/<dataset>/<file>` (a
    declared elevation dataset's `store` files, deflate GeoTIFF, `.none` where missing;
    docs/inputs.md §6.8).
  - `sources/dem-cache/dem-cache.{keys.u64,elev.f32,src.u8}`: the DEM cache's seed (sorted keys
    `(lon + 2³¹) << 32 | (lat + 2³¹)` E7, elevations, sources).
- **Rail sources** (`sources/rail/`, content-named, in the manifest, never swept; `pipeline::rail`,
  plan §6 Rail service):
  - `catalogue.csv`: the Mobility Database catalogue as downloaded (docs/inputs.md §6.7.1);
  - `checked.json`: every catalogue feed checked for rail routes, by `id`: `id`, `provider`, `name`,
    `country`, `subdivision`, `url`, `licence`, `size_mb`, `rail_routes`, `examples`, `status` ("ok",
    a definite failure such as "http <code>" or "no routes.txt", or "no answer (…)", which is
    checked again, as is an "http" 429 or 5xx);
  - `gtfs/<feed id>.zip`: each feed as fetched;
  - `fetched.json`: `{zip content name: "YYYY-MM-DD"}`, the day its timetable counts from;
  - `feeds.json`: `{fmt: 1, feeds: [{id, provider, name?, country, url, licence, replaces?,
    rail_routes?, zip, fetched} or {…, status}]}`, the coverage's feeds in railgtfs.py's order;
  - `ferry-feeds.json`: likewise, `{id, name, url, source, ferry_routes, zip, fetched}` or `status`.
- **Stop pairs** (railgtfs.py's, docs/inputs.md §6.7.2): 21-byte records, f32 lon_a, lat_a, lon_b,
  lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular; +0x80 a lower bound; +0x20 / +0x40 stop A /
  B beyond the coverage), f32 trains A → B on a typical weekday.
- **Kept for units' later runs** (`cache/`, not swept): `cache/dem-units/<u>.<box>.dem` (`<box>` 32
  hex digits: "RDDEM002", u64 count, the points' box (4 × i32), what it was sampled under (4 × u32:
  `u32::MAX`, 0, the declared elevation datasets' digest in two halves), then sorted keys,
  elevations, sources) and `cache/scenic-units/<u>/` (canopy.keys, canopy.tiles, view.keys,
  view.tiles, near.i8, roadside.u8, samples.metrics.u8, grid.canopy.u8.zst, grid.cover.u8.zst,
  basis.json `{v: SCENIC_V, basis: [[[x, y], hash16], …]}`).
- **Workers** (docs/workers.md):
  - `state/coordinator.json`: `{urls: [Tailscale's, then the LAN name's], token, page}`, while the
    lead's agent runs (`page`: the build page over HTTPS when `tailscale serve` proxies it).
  - On the lead, `agent/coord/`: `workers-token` (32 hex digits, mode 600); `page`; `leases.json`
    (`{next, leases: [{id, worker, work, progress}]}`); `costs.json` (`{<cost key>: {peak_mb,
    secs, worker, v, sizes}}`, `sizes` as in `costs.jsonl`, absent before task #235; keys: a unit, `"<step> <target>"`, or `"<kind> <unit>"` for tasks: `tail 6/x/y`,
    `bldtile 8/x/y`, `treeblock 8/x/y`, `terrainsub 8/x/y`); `floors.json` (`{<cost key>: {mb, alone,
    v}}`, crate::agent::memguard::Floor; `scenic pool floors` lists and clears them);
    `disks.json` (`{<cost key>: MB}`, rising only, docs/pool.md §7.7); `job-disks-v3.json`
    (`{<step>: MB}`; an older `job-disks-v2.json` or `job-disks.json` read once without its halo steps' stale peaks); `journal/<worker>/` (hand-offs sent rather than journaled, drained into the
    pool's journal); `tasks/<id>/`; `pause.json` (`{pause: {mode: "drain" | "freeze", by, at} | null,
    at}`, `pipeline::control::Pause`); `costs.jsonl` (`SCENIC_COSTS`: `{unit, started}` as a target
    begins, `{unit, peak_mb, secs, v, disk_mb, sizes}` as it ends; `sizes`, `pipeline::costin`, a
    map of the target's input measures, absent from lines before it: `bytes` and `threads` (the threads it was given) always, `cpu_ms` (its process's CPU time over the target) where it can be read
    (bytes: what the store's readers read for the target, plus a step's own inputs: a unit's piece,
    pack's and lo's base packs, pois' piece); `km2` (the tile's area) and `tiffs` (GeoTIFFs opened)
    for every tile target; a unit's `ways` and `vertices` kept, `piece_ways`, `dem_tiles`. A unit's
    bytes and tiffs include its programs' own (each run with `SCENIC_READS` naming a file it
    appends `<bytes> <tiffs>` to as it exits, `costin::report_on_exit`), its tail's steps run here
    after its other measures were taken among them; a tail a worker ran (settled remotely) adds
    nothing, nor do reads outside the store's readers, remote reads, or Python steps; pack's and
    lo's `units` and `ways`; peaks' `peaks`, `z12_tiles`; bldprep's `rows`, `buildings`, `parts`;
    bldtiles' `buildings`, `parts`; `wrote` and `creates`, the bytes and the count of the files
    the target's job made on the NAS through its `Out` (`put_file`, `store_bytes`; a file already
    there, reused, not counted), absent when it made none); `written`, `written-2`
    (`SCENIC_WRITTEN`, bytes added to caches, every 64 MB); `timings.jsonl`.
  - On a member, `agent/outbox/<lease>/`: a leased task's files, `costs.jsonl`, `timings.json`,
    `spec.json`, `task.json` (`scenic run-task`'s result), `result.json` (`{ok, done: [step,
    [[target, key], …]] | null, failed, interrupted, task, error}`), until the coordinator has them.
  - **Pausing** (`pipeline::control`), in each agent's folder: `pause-request.json` (`{pause, at}`,
    from the menu or `scenic pause`), `pause.json`, `control` (`run` or `drain`,
    `SCENIC_CONTROL`), `done.txt` (`<step> <target>` lines, `SCENIC_DONE`). A job stopped at a safe
    point exits 75. Coordinator answers carry the pause (409 `{error, pause}`, beats `{ok, pause}`);
    `POST /work/pause {pause, at}`; `/work/fail {…, interrupted}` returns a lease unheld.
    `state/build/pause.json` mirrors the lead's.
  - **Input check asks** (`pipeline::inputs::Ask`, docs/inputs.md §4.5): `inputs-asks/<at, 20
    digits>-<pid or "menu">-….json`, `{unit, accept, all, unaccept, check, full, forget, by, at}`,
    from the menu and `scenic inputs check`; taken up at the agent's next
    loop. A member's agent tells the lead's coordinator (`POST /work/inputs`); the build page posts
    there directly (never `forget`).
  - **Lead asks** (`pipeline::control::LeadRequest`, docs/pool.md §6.3): `lead-request.json`, `{ask:
    {kind: "give", to} | {kind: "take", force, downgrade}, by, at}`, from the menu or `scenic lead`; the build page's via `POST /work/lead` (never forced).
  - A hand-off (`pipeline::handoff`): `{changes: {logical: content name | null}, pending: {content
    name: SHA-256}, checked: [content name], done: [step, [[target, key], …]] | null, raw: [[area,
    {name, bytes}], …]}`, named `<ns>-<pid>.json` in write order; `<folder>.merged` holds the last
    merged; `.bad` one set aside unparsed.
  - `state/helpers/<host>.json`: a member's status while not leading.
- **The pool** (docs/pool.md):
  - `state/build/terms/<E>.json`: term E, create-new, `{term, member, host, app, since, how, from,
    seq}`; `state/build/lead.json` a copy of the newest (a hint).
  - `state/build/term/1/first.json` and `state/build/term/<E>/records.json` (`{term, seq, manifest,
    keys, pending, sizes, raw, reflected, rejected, last: {step: {target: lease}}, horizon,
    handed}`; `sizes`: content name → bytes of the files the manifest names, as their jobs wrote
    them); the lead also writes `manifest.json`, `jobs.json`, `pending.json` and `sizes.json` after
    its saves.
  - `state/journal/<day>/<term>-<n>.json`: a job's entry `{member, lease, step, handoff, at}`
    (`handoff.sizes`: the bytes of each file it wrote, by content name; none in older entries);
    `state/journal/rejected/<day>/<term>-<n>.why`.
  - `state/pool/members/<id>.json`: a heartbeat `{member, host, app, beat, leads, handing_to: {to,
    term, offer, since, stage}, ready_for, stood_down, addresses, members, conds: {home, ac, battery,
    able}}` (with brokers on, `broker: {urls, https, waiting: {<kind>: {n, mb}}, out}`).
  - Switches (present or `on` in it: on; `off` or missing: off, unless said): `auto-handover`
    (`scenic lead auto on|off`), `slots` (§7.6), `memory-guard` (missing: on; §7.2), `resume`
    (§7.2, §7.4), `brokers` (§8; every coordinator answers `POST /work/where`), `placement` (§7.2:
    jobs get `SCENIC_OFFER_AHEAD=1`, task offers carry `ahead`), `delta-catalog` and `near-slope`
    (plan §8, Publishing's switches), all under `state/pool/`.
  - A member's ask carries `disk_mb` (free disk less the agent's reserve,
    `pipeline::agent::room::RESERVE`, and the room target).
  - `state/pool/progress/<member>.json`: `{member, worker, jobs: {"osm-pass <date>": {at, since,
    done, scratch}}}` (a resumable job's progress there, offered to it alone for 24 h);
    `state/pool/progress/released/<member>/<job>-<since>` (`scenic pool release`, empty).
  - `state/pool/wiki/<class>/<window>-<slot>`: a lease of the Wikimedia budget (`wiki` or `query`),
    create-new, the leaser's id in it; removed 30 windows on.
  - `state/pool/answers.lease`: `{by, token, until}` (crate::answers::Lease), create-new; a lapsed
    one moved aside as `answers.lease.lapsed-<token>`, then removed.
  - `state/pool/mail/<to>/<from>.json`: the last 64 messages, `{msgs: [[n, msg], …]}`, `msg` one of
    `{"Tell": [keys]}`, `{"Ack": {term, keys, horizon}}`, `{"Passed": term}`, `{"Leads": E}`,
    `{"HandTo": member}`.
  - `state/coord/term/<E>/state.json`: `{leases, costs, floors, disks, job_disks, failed: [[worker,
    cost key, unix seconds, times]], pause, pause_at}`; `state/coord/token`;
    `state/coord/history/<day>/<member>.jsonl` (history events, terms' as `kind: term`).
  - `state/publish/<host>.jsonl` (pipeline::pubtimes): each Mac's map server's lines, appended,
    `{t, host, kind, n, secs?}`, `kind` one of `adopt` (catalog n served), `mirror-start` and
    `mirror-end` (its mirror's sync of catalog n begun, and through everything wanted, `secs` from
    the start: each once per server process, so a restarted server says them again). Past 512 KB a
    file is written again with its newer half.
  - Leases carry `term` and `granted_at`; `/work/done {…, journaled: true}`; `/work/done` and
    `/work/fail` carry `floors`, `disks` (`[[cost key, MB], …]`) and `job_disk_mb`.
- **State:** `state/status.json` (the agent's heartbeat, at least every two minutes): conditions,
  the job, `parts`/`part`, progress, `paused`/`pausing`, what waits, `pause`, `publishing`
  (agent::build::PublishLine: `why`, `tiles`, `units`, `regions`, `low`, `wait_s`), `published`
  (agent::Published: `last` `{n, at}`, `low`, `drawing` by Mac, `adopted` by Mac `{n, at}`), the
  forecast (`regions` with `map_at`; `publishes`: `{what: low | marks | overlays, at}`), the checklist (a line per stage: `id`, `what`, `steps`, `done`/`total`/`left`,
  `unit`, `items` (per step: `step`, `title`, `noun`, `done`, `total`, `left`), `next`, `note`,
  `shared`: docs/pipeline.md §4), `caches`
  (agent::room::Caches: `clearable`, `each`, `why_not`, `trimmed`, `cleared`, `declined`, `room`),
  `pool`, `lead` (crate::agent::lead::View), `inputs`
  (pipeline::inputs::view::InputView: per unit `version`, `state` ok | checking | held, `checked`,
  `held`, `findings`, `accepted`, `stale`). `state/build/{manifest,jobs,pending,summaries,pause}.json`
  (`jobs.json`: job keys by step, target → key: `terrain`, `slope`, `terrain_lo`, `slope_lo`, `unit`,
  `pois`, `peaks`, `pack`, `lo` (z3 tiles, worldwide steps by name, input check jobs as
  `inputs/<unit>`), `trees`, `trees_lo`, the 3D buildings', `catalog`, `catalog_held`).
- **The input check** (docs/inputs.md §3, §4; a unit's name is `/`-separated components of
  `[a-z0-9_-]`; its records start with `@`): drop boxes under `inputs/` (docs/inputs.md §3: `regions`,
  `languages`, `translations`, `descriptions`, `heritage`, `heritage-matches`, `timetables/feeds`,
  `timetables/gtfs`, `elevation`, `sources`; and the test unit's `inputs/_gate-test/`, `{"k", "v"}`
  lines, checked while `state/inputs/gate-test` exists), each box's top-level `todo/` (the app's)
  and `how/` (the owner's notes on how it's made, Markdown and briefs) never listed, and
  `inputs/README.md` describing them (none read by the app); checked
  copies, content-named under `sources/inputs/<unit>/`; the accepted index
  `sources/inputs/<unit>/@index.<hash16>.json` (manifest name `…/@index`: `{fmt: 1, unit, checks,
  files: {<path>: {file, size, keyed, facts}}, accepted: {<warning id>: [files]}}`); the last
  check's listing `@listed` (`{fmt: 1, unit, listed: {<path>: [size, mtime]}, replaced, full_at}`);
  while a change is held, `@held` (`{fmt: 1, unit, checks, held, findings: [{id, level, files,
  message, at, lines}], together, raised}`); the agent's listing for a check
  (`agent/inputs-listing/<unit with / as +>.<key>.json`, `inputs::Planned`); the owner's acceptances
  `state/inputs/accepted/<unit>/<finding id>.json` (create-new, `{id, member, host, at, by,
  message}`).
