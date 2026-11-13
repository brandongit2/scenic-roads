# Formats

The contract between the build steps, the NAS, the server and the browser (design: docs/steps.md, docs/storage.md).
All integers are little-endian. Coordinates are i32 1e-7 degrees (`E7`) unless stated. A tile key
is `roadcore::archive::tile_key(z, x, y) = z << 58 | x << 29 | y`.

Bump a format's version on any incompatible change; readers accept the current and previous
version (docs/building.md §2, Format bumps). Within a container's version 1, additions are told apart by
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
36  [u8; n]  meta JSON: {"layer", "scope": "root"|"lo"|"hi"|"deep"|"polar", "root": "z/x/y",
             "encoding" (e.g. "rt7", "terrarium-webp", "shade-webp", "mvt"), …}
             (zoom ranges are the map's)
…            blobs (identical blobs stored once; entries may share an offset)
index        count × Entry (32 bytes), sorted by key, 8-byte aligned:
             u64 key, u64 offset, u32 len, u32 raw_len (0 if unknown), u64 xxh3
```

- Root pack: z0–2 (root "0/0/0"). Lo pack: z3–8 under one z3 tile. Hi pack: z9–14 under one z6
  tile. **The terrain** (one terrain, z0 to each place's native zoom: steps.md §2) adds deep packs,
  `layers/terrain/deep/10-<x>-<y>` (scope `deep`): z13–16 under one z10 tile, only where a dataset
  reaches past z12 (terrain::terrain_deep; a z6 tile's would be up to 130 GB). Every terrain tile, in
  every scope, is Terrarium RGB as lossless WebP (`RIFF…WEBPVP8L`, the packs' encoding
  `terrarium-webp`: codecs::terrain_codec), served as `image/webp`. **Heights are rounded once,
  as stored:** a job carries its levels' heights unrounded (each coarser level the 2×2 means of the
  finer's before rounding), and each tile is rounded as it's encoded, to 1 cm where it is its
  place's finest zoom (terrain::terrain_zoom) and to 10 cm coarser, ties to even (a 2×2 mean of
  rounded heights lands on a tie about a quarter of the time: rounding ties away from zero would
  lift heights by about a quarter quantum a level), then to Terrarium's 1/256 m. Only what a job
  reads of another's packs (the root of the lo packs' z3) is rounded before its means. The map names the deep packs
  under the layer's `deep`, each one's z10 tile's native zoom under `native_zoom`
  (`terrain_deep::native_zoom`: its places' finest, terrain::terrain_zoom; elsewhere their finest is
  z12 or coarser), and the terrain's
  `maxzoom` is the finest of those. The app's meta passes `native_zoom` on as `terrainAreas`
  (`{"zoom": 10, "tiles": {"x/y": z}}`), and the map takes its terrain and shading tiles to that
  zoom there (our MapLibre's `scenic:zoom-by-area`), z12 elsewhere. The shading goes up to two
  zooms finer than the screen within those zooms, nearest the camera first and within a budget,
  each screen tile's once it is in, while the camera moves too (web/src/basemap.ts `SHADE_DETAIL`,
  our MapLibre's `scenic:detail`). The meta's
  `packs.terrain.deep` gives each deep pack's version by z10 tile (`"10/x/y"`), as `lo` and `hi` do
  theirs, so a tile past z12 there carries its deep pack's `?v=` and a map update asks again only
  for the tiles of the deep packs it changed. **The terrain's shading** (the `slope` layer:
  terrain::slope_pack, steps.md §2) has the same packs, its deep packs `layers/slope/deep/10-<x>-<y>`
  beside the terrain's; **the floor's** (`depth-shade`) the depth's (Depth below). Every shading tile
  is encoding `shade-webp` (codecs::shade_codec), RGB as lossless WebP, served as `image/webp`:
  R = 255 × √(steepness ÷ 400 %) of the mean steepness beneath the pixel, G and B its mean unit
  gradient direction (east, south) as 128 + 120 u in steps of 8 (8–248), (128, 128) where that
  direction's length (the coherence) is under 0.25 or the ground is flat (roadcore::shade).
  **The polar caps** (`polar-terrain-<pole>`, `polar-slope-<pole>`: terrain::polar, docs/polar.md)
  are tiled on their pole's polar stereographic grid, not Web Mercator: their root pack
  (`layers/<layer>/root/0-0-0`) holds z0–3 and a block pack (`layers/<layer>/polar/4-<x>-<y>`, scope
  `polar`, root "4/x/y" of the grid) z4–9 under one z4 tile; the map names the blocks under the
  layer's `polar` ("4/x/y"), and meta's `packs` gives their versions there likewise. The tiles are the
  terrain's and the shading's encodings (the shading's directions on the grid: east = +x, south = −y).
  **Offline:** a region's deep packs, the terrain's and its shading's, are on a Mac only where its
  terrain detail is downloaded (an add-on to the region: crates/server/src/downloads.rs). Elsewhere,
  away from the NAS, a deep tile that can't be read is made from its ancestor in the hi packs (the
  z12 terrain or shading, scaled up), as anywhere past the data; the server never answers 503 for
  one. A tile lives in exactly one pack.
- Readers fetch header and index once (two range reads) and keep the index locally
  (`idx/<file hash>.idx`), so serving a tile is one range read.
- **Layer bundles** (`layers/<layer>/all`; meta `{"layer", "scope": "all", "encoding"}`): every tile
  of a layer one job makes whole in one pack (each landmark kind's thinned tiles, the area
  overlays). The map names it for each pack key its tiles fall in (`root`, `lo`'s z3 keys, `hi`'s
  z6 keys), over any pack of before; the keys come from its index, or from the map before when that
  named the same file.

## Bundles of files (`.pack`, an RDPACK of files)

Files made per tile by one job, in one pack keyed by the tile (`roadcore::archive::tile_key`), each
member a whole file as it would be on its own, not gzip'd, starting on a 64-byte boundary
(`PackWriter::aligned`), so a sectioned file in it is mapped and cast in place as on its own
(`terrain::layers::write_file_bundle`). `markdata/all` (meta `{"bundle": "markdata", "fmt": 1}`):
the landmark points, a member per z6 tile. The map's `markdata` names it for each tile it holds;
a reader finds the member through the pack's index (kept as a pack's is) and reads it in place
(`SectView::open_at`; `codecs::marks::markdata_files`). A map naming a `.sect` per tile (before
task #449) is read as before.

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

One reader, `store::sect::SectReader`, takes the header and table for the build and the map server
alike (versions 1 to `sect::VERSION`; a table out of place is an error). A local file is mmapped
(`store::MmapFile`, which refuses a file on the share) and its sections cast in place, unchecked. A
NAS file is read through the I/O pool (`store::PooledFile`) in 256 KB pages, unchecked, or whole
sections for scans, each checked against its XXH3.

## Roads' z6 tiles

The roads are made and stored per z6 tile. A way's owner is the z6 tile of its first vertex. (Stored
names keep the word `unit` for a z6 tile, from before the grid's terms were fixed: the meta key
`"unit"`, the `unit` step, `global/roadunits`, `cache/scenic-units/`, the
map's `units` list; the OSM pass's `pieces/` folder holds its z6 tile extracts.)

## Base pack (`base/<z>-<x>-<y>.<h>.sect`): what one z6 tile owns

Meta: `{"fmt": 1, "unit": "z/x/y", "ways", "verts", "samples", "extent": [w, s, e, n] (E7, all owned
geometry), "source": "pass:<date>", "scenic": bool, "drape": bool, "summary": {…}}`. `summary` is
what the map's meta adds up: `extent`, `ways`, `vertices`, `elev_min`/`elev_max` (roads, metres),
`hist` (road km by 10 m of elevation, 256 bands), `rail_km` (track km per service group); the
map update works it out from the sections where missing.

| Section | Record | Notes |
|---|---|---|
| `ways` | `WayRec` (48 B, roadcore) | sorted by (z9 key, Morton) of first vertex; `vstart` local; `name`, `ref_`, `surface`, `route` index `strings` |
| `verts` | `[i32; 2]` | densified geometry |
| `elevu` | `u16` | elevation, decimetres + 5,000 (`final.u16`); older packs: `elev`, `i16` dm clamped at ±3,200 m. Readers take either (`roadcore::elev`) |
| `raw` | `f32` | raw DEM sample, NaN = none |
| `grade` | `u8` | \|grade\|, 0.5 % units |
| `src` | `u8` | DEM source: the dataset's declared `code` (docs/inputs.md §6.8), named by the map's `dem_labels` |
| `scenic` | `[u8; 13]` | per-vertex channels (`roadcore::scenic::ch`) |
| `drape` | `i16` | drape height, metres |
| `strings` | UTF-8 lines | line 0 is "" |
| `samples` | `Sample` (24 B) | `way` local; grouped by way, in way order |
| `samplech` | `[u8; 13]` | per sample |
| `sub9` | `(u64 z9 key, u32 first way, u32 way count)` | ways grouped by z9 tile of their first vertex |
| `rail` | `(u32 way, u32 relation lo, u32 relation hi, u32 pad)` | rail ways' primary route relation |

**Read in part** (pack and lo, task #396, `crate::basepart`): sections read whole (`ways`, `verts`,
`samples` as the way index is worked out; `strings`, `rail`, `sub9`) are checked against their
XXH3; ranges of a section can't be (the file checksums sections whole), so a part read isn't
checked. **Way index** (`<cache>/base/<content name>.idx`, a Mac's own): `"WAYIDX02"`, u32 chunk
size (1024 ways), u32 chunk count n, n × `[w, s, e, n]` i32 (each chunk's ways' box, E7), (n + 1)
× u64 (each chunk's first sample; the last the samples' count), then u64 XXH3 of all before it;
one that doesn't check is worked out again.

## Road values (`global/roads/<z>-<x>-<y>.<h>.sect`): per owned way of a z6 tile

Section `roads`, one 24-byte record per way of the z6 tile's base pack, in the same order:

```
u64 road id      the lowest OSM way id of the road
f32 road length  metres
f32 offset       metres from the road's start to this way's start, along the road's direction
u8  dir          0: the way runs with the road; 1: against it
[u8; 7] pad
```

Section `byroad`: `(u64 road id, u64 way index)` for every way, sorted (the server sorts `roads`
itself for files without it).

Chaining (docs/steps.md): at each node, way ends of the same kind pair by mutual best continuation.
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
Here    { u64 id; u64 owner (z6 tile key); u32 index (in the owner's base pack); u8 class;
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

## Landmark points (a member per z6 tile of `markdata/all.<h>.pack`; docs/phase5.md)

Every point of the 8 kinds (`codecs::marks::KINDS`) positioned in the tile, sorted by kind, then
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

`layers/marks-<kind>/all` bundles hold the thinned tiles of zooms 0–5 (encoding `rdmt`); the
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

## The polar caps' points (landmarks::polar; docs/polar.md §10)

Made by the `polar-points` job, a file per pole: `global/polar/<n|s>` (`.json`), a GeoJSON
FeatureCollection of what lies past the Web Mercator tiles' 85.05° on that side. Its features, in a
fixed order (labels by score, landmarks by score, roads by id):

- **labels** (Point; `k` place, state, water or park, `c` its class, as the label tiles' `k` and `c`);
- **landmarks** (Point; `k` poi, `c` the map's kind: peak, viewpoint …; `ele` m where it has one);
- **roads** (LineString; `k` road, `c` its class's name: service … motorway).

A point's properties: `n` (its name; a landmark's may be ""), `o` (its OSM object), `s` (its score:
the most important placed first), `r` (m: it shows once `r` spans its kind's label spacing on
screen, at its place), and as it has them `en`, `kana` and `l` (the label tiles'), `w` (an area's
√area, m: its name waits until that spans 20 px at the default spacing). Served at
`/api/layer/polar-<n|s>` with `main`/`sub` attached; meta's `versions` names it `polar-<n|s>.json`.

## Overlays, stations and ferries by view (landmarks::ovconv; docs/phase5.md)

Made by the `overlays`, `stations` and `ferries` jobs; `overlays` also writes
`global/heritage/{layer-summary,heritage-sources}`.

- **Areas** `layers/ov-{heritage-areas,indigenous,special,whs}/all` (bundles; encoding `mvt`,
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

## Water (`layers/water/{root,lo,hi}`; inputs::water)

Each pixel's exact share of water, z0–9, from the basemap's z14 water (docs/steps.md §4). Encoding
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

**The enclosed seas** (`global/water-enclosed`, written by the water step; `water::Enclosed`): the
sea's pixels at z6 (512 a tile) not joined to the world ocean (docs/steps.md §4), little-endian:
the zoom (u32, 6); the runs' count (u64), then each run as row, first pixel and past the last (three
u32s), by row and first; the bodies' count (u64), then each body's pixels (u64) and one pixel of it
(x, y: u32s), largest first. The server reads it (the map's `global`, `"water-enclosed"`) for
the water's surface, sea level only on the world ocean; the water version (`w=`) names it. The
depth steps read it too: an enclosed sea's depth is below its own surface (Depth, below).

## Lakes filled from Canada's own data (`work/lakefill/<date>.<h>.sect`; inputs::lakefill)

Lakes OSM lacks in Canada's unimported map sheets (docs/steps.md §4), from CanVec. Meta `{"fmt":
"lakefill-1", "grid_z": 8, "lakes", "rings", "points", "source", "layer", "field", "keep",
"intermittent", "rules": {"tol_m"}, "gap_sheets"}`. Points are Web Mercator world units × 2³² (u32);
outer rings clockwise on the map, holes anticlockwise. Every lake lies in `lakefill::REACH` (lon
−141.5 to −52, lat 41.5 to 84; `write` refuses one outside), which readers rely on to leave it unread
elsewhere.

| Section | Record | Notes |
|---|---|---|
| `lakes` | 32 B: `id` (u64), `bbox` [u32; 4] (x0, y0, x1, y1), `first_ring`, `nrings` (u32) | `id`: bit 62 set (never an OSM id), bit 61 intermittent (drawn, left out of the depth's water), the rest a hash of its points |
| `rings` | 8 B: first point, count (u32) | a lake's rings in a row |
| `points` | 8 B: [u32; 2] | no closing point |
| `grid` | u32 × (256² + 1) | per z8 cell (row by row) its first entry in `cells`, then the total |
| `cells` | u32 | lake indices, ascending per cell; a lake in every cell its box meets |

The map names it in `global` (`"lakefill"`); the server draws its lakes into the water's deeper
zooms as a z14 tile beside the basemap's (layer `water`, class `lake`, extent 4096, clipped to the
tile grown by a sixteenth).

## 3D buildings (bld::bld; docs/buildings3d.md §3.4)

**Normalized buildings** (`work/bld/6-<x>-<y>.<h>.sect`, in the manifest, not served): every
building and part of the pinned Overture release whose centroid is in the z6 tile, underground ones
left out (`bldprep`). Meta: `{"fmt": 1, "tile": "6/x/y", "release", "buildings", "parts", "srcs",
"classes", "subtypes", "roofs", "ghsl": "R2023A", "read"}`: the counts; the sorted string lists the
codes index (0 none, k the k-th: height source datasets, classes, subtypes, roof shapes); `read`,
what was read (`files`: [name, ETag, [row groups]], `ghsl`, `rows`).

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

**The sources' indexes** (written by `bld-fetch`, `bld::bld::fetch`, beside the downloads,
outside the manifest, as Python's `json` wrote them;
read by `bld::bld::sources`): `sources/overture/<release>/buildings.json` (`{"fmt": 1,
"release", "source", "terms", "coverage": {"margin_km", "at"}, "files": {<name>: {"size",
"etag", "rows", "row_groups", "rows_near", "bbox", "checked"}}}`); `footers.json.gz` (every file of
the release: `{"release/<release>/<name>": {"etag", "size", "rows", "rgs": [[w, s, e, n, rows],
…]}}`); `sources/ghsl/R2023A/index.json` (`{"fmt": 1, "product", "source", "terms", "tiles": {<zip>:
{"size", "bbox", "checked"}}}`).

**Tiles** `layers/buildings/hi/6-<x>-<y>` (RDPACK, encoding `mvt`, blobs gzip level 6, z14 only):
MVT 2.1, extent 4096, layer `b`, a feature per building or part touching the coverage (centroid or
a vertex in it, with the 1 km buffer), whole in the tile of its centroid (not clipped). A building
or outline (not a part) is also copied whole into each other tile its exteriors reach (`o` 1), from
up to 310 m beyond a z8 tile's edge. Quantized to the tile grid; repeated points dropped; rings of
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

**The field** (`bld::bld::field`, docs/buildings3d.md §4.8): every building and part extruded
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
- **Mids** `work/bldfield-mid/6-<x>-<y>.<h>.zst`: zstd (level 3) of `BFMID03\0`, then each z8 tile
  with a building, by x then y: u32 x, u32 y, u32 heritage list bytes and that list, u8 class mask,
  each held class's `f`, `w`, `v`, `ox`, `oy` planes as 512 × 512 f32 (the level-8 values exactly).

**A z8 tile's task** (`bld::bld::task`, docs/buildings3d.md §3.6; a folder on the job's Mac,
sent to the worker as `u/…`, not in the manifest): `6-<x>-<y>.sect` for each of T's and its 8
neighbours' normalized files with a block the z8 tile reads (its own and those within 620 m), each
with the same meta and only those blocks as stored; `coverage.sect` (meta `{"fmt": 1, "shapes":
[{"source", "country", "buffer_m", "rings": [vertex counts]}, …]}`, section `verts` `[i32; 2]`): the
shapes whose buffered box meets the box of every point the z8 tile asks about, whole, in the recipes'
order; `heritage.bin`, T's heritage points (`[i32; 2]` pairs, sorted). `bldtile` writes there
`area.tiles` (RDTILES, meta `{"layer": "buildings", "area": "8/x/y", "encoding": "mvt"}`; the names `area.*` and the meta's `"area"` are
from when a z8 tile was called an area, and stay),
`area.field` (RDTILES, `{"layer": "buildings-field", …, "encoding": "field5-webp"}`, levels 9–14),
`area.mid` (a mid of the z8 tile alone) and `area.json` (`bld::job::Summary`).

## Other tasks' folders

Each a folder on the job's Mac, its files sent to the worker as `u/…`, not in the manifest.

- **A row of tree cover blocks** (`trees::trees::task`, docs/steps.md §2, Trees): `coverage.json`
  (`{"shapes": [[ring, …], …]}`, rings `[[lon, lat], …]`, only those whose box meets the row's box
  grown by 0.01°). `trees --blocks` writes `8-<x>-<y>/` per block: `trees-cover.tiles`,
  `trees-height.tiles`, `trees-leaf.tiles` (RDTILES, meta `trees::META`: zoom 8–12 tiles) and
  `trees-tops.bin` (`pyramid::Tops::to_bytes`).
- **A terrain z6 tile's subtrees** (`terrain::terrain_task`, docs/steps.md §2, Terrain): `in.sect`, meta
  `{"fmt": 2, "step": "terrainsub", "v": TERRAIN_V, "piece": "6/x/y", "subtrees": "8/x/y,…",
  "levels": [[z, [[x, y], …]], …] (z12 → z9), "water", "native": [[z, x, y], …] (its tiles at their
  place's finest zoom: rounded to 1 cm)}`; sections `r-<z>-<x>-<y>` (a raw PNG tile, its sampled and
  relief quarters as the job read them, its children's left void: crate::terrain_pack::Fills; none:
  every quarter its children's, or no dataset has data there)
  and `w-<z>-<x>-<y>` (water polygons: u32 count, each u8 kind (1 sea, 2 lake), u64 key, u32 ring
  count, each ring u32 point count and f64 pairs). `terrainsub` writes `hi.sect` (meta `{"fmt": 1, "step": "terrainsub",
  "piece", "subtrees"}`, a section `t-<z>-<x>-<y>` per tile as the hi pack holds it) and `mid.sect`
  (the group's part of `work/terrain-mid/6-x-y`).

## Names (docs/names.md)

- **Translation lines** (`inputs/translations/**/*.jsonl`, not `todo/` nor `how/`; an inputs folder of the
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
  (`n123`, `w123`, `r123`), `k` kind, `c` class, `mz`, `ms`, `s` (crates/landmarks/src/labels.rs). Before `LABELS_V`
  2: `n`, `en`, `k`, `c`, `mz`, `ms`, `s`.
- **To translate** (`inputs/translations/todo/<language>.jsonl`, `names-todo`'s, rewritten after
  each map update that changed what it reads): `{"n", "kind", "langs", "things", "example": {"osm" | "qid" | "label", "at": [lon,
  lat]}, "priority"}`, by priority; with `README.md` and `check.py`.
- **Lines with no timetable** (`inputs/timetables/gtfs/todo/lines.jsonl`, `timetables-todo`'s):
  `{"osm", "kind" ("rail" | "ferry"), "name", "ref", "operator", "from", "to", "km", "priority"}`;
  with `README.md` (docs/inputs.md §6.7.3).
- **To describe** (`inputs/descriptions/todo/landmarks.jsonl`, `areas.jsonl`): `{"qid", "id",
  "name", "en", "kind", "designation", "at", "enwiki", "wiki", "register", "source", "fame"}`
  (landmarks; `source` the heritage register's id, docs/inputs.md §6.5) and `{"qid", "id", "name",
  "kind", "bbox", "area_km2", "enwiki", "fame"}` (areas), by fame; with `README.md`.

## Depth (`layers/depth/`, `layers/depth-class/`, `layers/depth-shade/`; terrain::depth)

Three layers of 256-px tiles in packs like the terrain's: `root/0-0-0` (z0–2, the means of the lo
packs' z3), `lo/3-x-y` (z3–8) and `hi/6-x-y` (z9–16, only where a source has grids finer than z8);
the map's zooms 0–16 once the root packs
exist, 3–16 before (the app makes z0–2 from z3, web/src/depthgrid.ts; its map tiles, each made from the
zoom below, stop at z15, and MapLibre scales them up past it). A tile is stored where one of its water pixels is a
source's own data of its zoom, or where a finer stored tile lies under it (each pixel then the mean
of the four beneath it with data); never finer than the sources under it (steps.md §3). A missing
one is its nearest stored ancestor scaled up (bilinear for depth and its shading, with the depth
ancestor's neighbours' edges; nearest for classes).

- **`depth`** (encoding `terrarium-webp`: Terrarium RGB as lossless WebP, as the terrain's, but
  rounded as below, not to the terrain's centimetre): metres below the water's surface (the sea's mean level, a
  lake's mean surface, an enclosed sea's own: the terrain's height there, the Caspian's −28 m), negative; drying heights positive (at most 20 m); +12,000 m where there's
  none (≥ 9,000 is none). Quantised: 0.1 m in the first 100 m, 1 m deeper. The basemap's water
  (`terrain_water`: half a pixel or more of sea or lake) is where it lies, and the land keeps a band
  of 3 pixels of the nearest water's value (class 0), so a finer shoreline scaled up from it never
  meets a gap; the map doesn't draw it (its relief reaches from the water's own data:
  web/src/depthgrid.ts), nor the floor.
- **`depth-class`** (encoding `class-webp`: lossless WebP, the byte in R, G and B alike): bits 0–2 the class (0 none or land, 1
  a source's grid at the tile's zoom or finer, 2 a coarser grid's, scaled up, 3 estimated, 4 filled
  to the shore), bit 3 dries at low tide, bits 4–7 the
  source's `code` (docs/inputs.md §6.10).
- Both are libwebp 1.6.0's `-lossless -z 6 -exact` (terrain::depth::fall), decoded exact by Chrome
  and Safari; readers still take the PNGs of packs from before (`terrarium-png`, `class-png`, grey)
  until the depth is rebuilt.
- **`depth-shade`** (encoding `shade-webp`, as the terrain's shading: Pack above): the floor's
  steepness and mean direction, a tile for each depth tile (docs/steps.md §3), made with the
  depth's packs: the lo packs' z8 from the hi packs' z9 where they cover it, the root's z2 from the
  lo packs' z3.
- **The map's meta** `depth`: `{"sources": [{"code", "id", "label", "datum", "water", "estimated", "notice"}]}`,
  the accepted depth declarations (`grids::Source::catalogue_entry`: the credits' depth line and notices, the footer's
  water info on hover, and the estimated ones named in the Depth section's note).

## The map and its log (`store::map`)

**The map** is the current name → file map of everything served. **Its log** is the batches that
change it, each with a sequence number (`seq`). A **map update** is one batch appended. A **client**
is anything that holds a copy of the map at some `seq`. ("Manifest" is the build's: the records'
logical → content map, docs/storage.md.)

The lead's agent keeps the map and its log on its own disk (`agent/maplog/`, below) and its
coordinator serves them over HTTP, from memory. The NAS holds no copy: the lead alone updates the
map and merges hand-offs into the records, a batch is a few kB while the share is slow at small
files and stale to list, and a Mac away from home reaches the lead over the tailnet when it can't
reach the share. With the lead down the map isn't updated, and each client keeps the map it holds.
What makes an update, and when: docs/building.md §2, 5. How the map server and the mirror follow it:
docs/macs.md. What it means for the files on the NAS: docs/storage.md, GC.

### Entries

The map is a flat set of entries, key → value, at the grain a change touches:

| Key | Value |
| --- | --- |
| `f/<logical>` | a served file: `{file, size, fmt}` (`file` its content name) |
| `l/<layer>` | the layer's head: encoding, zooms, `root` |
| `l/<layer>/<lo\|hi\|deep\|polar\|native_zoom>/<tile>` | a logical name (`native_zoom`: a zoom) |
| `s/<section>/<key>` | a logical name: `base`, `roads`, `hidata`, `markdata`, `ovdata`, `global` |
| `u/<z6 tile>` | `true`: a z6 tile built |
| `r/<region id>` | `{"i": its place in the recipes' order, "region": the region}` |
| `m/<name>` | the value: `meta`, `credits`, `dem_labels`, `basemap`, `app`, `fmt`, the coverage's parts that aren't regions, `extra/<k>` (a field this app doesn't know), `units` (a list not sorted, kept whole) |

A value is kept as its canonical JSON (`store::map::Val`: the bytes its hash is over, parsed only
where it's read), so the lead and a map server hold the map once.

**The digest** is the sum, modulo 2^128, of XXH3-128 over each entry's key, a zero byte and its
canonical JSON (`entry_hash`, `digest_of`; written as 32 hex digits). A batch changes it by
subtracting each replaced entry's hash and adding each new one's, so the writer and every client
keep it in time proportional to the batch, and a client checks it after every batch. A map holds
each key once, so the entries are a set, and a map other than the writer's passes only if the
differing entries sum to the same (2^-128 for hashes that behave as random; nothing here is
adversarial).

### A batch

```json
{"seq": 812, "at": "2026-11-02T04:05:06Z", "app": "<version>", "why": "map 812; the low zooms",
 "set": {"f/layers/roads/hi/6-32-21": {"file": "layers/roads/hi/6-32-21.<hash>.pack", "size": 123, "fmt": 1}},
 "del": ["f/base/6-1-1", "s/base/6-1-1"],
 "freed": ["base/6-1-1.<hash>.pack", "layers/roads/hi/6-32-21.<oldhash>.pack"],
 "digest": "<32 hex>"}
```

- `set` and `del`: the entries set and deleted. A change of nothing makes no batch.
- `freed`: the content names the map named before the batch and doesn't after, worked out by the
  writer from the entries it replaced. GC's queue reads it (docs/storage.md, GC).
- `why`: `map <seq>`, then its parts, each after `; ` (`WHY_SEP`): `the low zooms` when the update
  was made with them (`agent::publish::LOW`).
- `digest`: the map's digest after the batch.
- A batch is whole for its clients: each applies it to a copy, or in place with an undo while no
  reader holds the map, and swaps the result in at once. Files are content-named, so applying a
  batch moves no data: a new name is a file already on the NAS.

**Marks.** The log keeps, for each watch its opener names, the last batch that marked it (`Mark`:
`seq`, `at`), saved with the snapshot and kept up as batches are replayed and appended
(`log::Log::open_watching`). A watch is a key prefix (the last batch setting or deleting a key under
it) or `why:` and a part of a batch's `why`. A prefix newly watched starts at the snapshot's `seq`.
The lead watches `why:the low zooms` (when the low zooms last went out) and the names' to-do lists'
inputs (`landmarks::namestodo::INPUTS`, docs/names.md).

### The lead's store (`agent/maplog/`)

```
snapshot.json.zst       the map whole: {seq, at, digest, entries, cursors, marks}, compact JSON in
                        zstd (level 3, with its content checksum); written when a new log is
                        seeded, then once an hour while updates come (agent::publish's
                        SNAPSHOT_EVERY_S), which also removes each segment all of whose batches it
                        holds
<first seq>.jsonl       a segment: a batch a line, appended and fsync'ed before the batch counts; a
                        new one each 10,000 batches or 64 MB (store::seglog)
posted.json.zst         the map the `catalog` step posts (--post: a catalogue, below, zstd with its
                        checksum; a temporary name renamed into place); the agent's writer renames
                        it posted.taken.json.zst, appends its batch and removes it
held/<seq>.json.zst     the maps held for review while the pool's `hold-catalog` is on (--held:
                        each a snapshot, the newest three kept; `GET /map/held` serves the newest)
gc.jsonl                GC's queue (docs/storage.md, GC)
gc-sweeps.json          GC's sweeps' cursors (docs/storage.md, GC)
```

- **A start:** the log loads its snapshot and replays the segments after it. A last line that
  doesn't parse is cut off (a write cut short was never answered); any other line that doesn't, or
  a batch whose digest doesn't hold, fails the open. A map taken and not appended when the agent
  stopped (`posted.taken.json.zst`), then one posted and not yet taken, are appended as the
  writer's thread starts.
- **The lead moved by hand** (docs/pool.md §6.1) takes `agent/maplog/` with it: its log, its
  snapshot and GC's queue.
- **No log on the lead's disk:** the next map the step posts seeds a new one whole. The map is the
  records', so only what the last map alone kept is lost until it's built again (a region whose
  outline changed since it was built, its tiles not yet remade), and the first map's sizes are
  looked up on the NAS. Its `seq` goes past the newest `seq` any client has asked from since the
  agent started (`Feed::asked_past`). A client asking later from past the lead's head is answered
  a reset and follows the lead's map from then; a map server goes on serving its newer one until
  the lead's passes it (`Data::set_map` never takes an older map). GC's queue starts empty;
  the orphan sweep finds what it held (docs/storage.md, GC).
- **A test root's or a `--root`'s map** is `map/<seq>.json.zst` under that root, the same snapshot
  (`store::map::write_root_map`, `root_map`; the newest three kept), written by the tools that make
  test roots (`lakefill catalog-add`, the tests) and read by a `--root` server or a
  job on that root. Refused on the NAS, whose map is the lead's alone.

### Asking the lead (`agentcore::coord::maproutes`)

On the lead's coordinator (port 8090, with the build's key), answered from the feed its log's writer
keeps (`store::map::feed::Feed`): memory alone, never the NAS or a disk.

- `GET /map/changes?since=<seq>&client=<id>&wait=<s>`: `{"head", "more", "batches"}`, the batches
  after `since`, up to 4 MB of them (`more` past that); or `{"reset": <seq>}` when `since` is before
  the oldest batch the lead still holds (64 MB of them at most, `HELD_BYTES`) or past its head: the
  client then takes the snapshot. With `wait` (30 s at most, `WAIT_S`) it answers once a batch comes
  or the wait is out (it looks each 50 ms). A mirror says too `copying=<seq>` and `synced=<seq>`.
- `GET /map/snapshot`: the whole map at the head, a snapshot as above, its `seq` in `X-Map-Seq`;
  made once per `seq` asked.
- `GET /map/held`: the newest map held for review; 404 while none is.
- 503 while there's no map here (not the lead, or its log not open yet): a client goes on as it
  would with the lead away.

`coordwire::map_now` is how a job or a command reads the map: `GET /map/snapshot` from the lead;
none with no lead answering (`scenic status` then says it doesn't know the map), but on a root that
isn't the NAS, that root's own map.

### Cursors

Every `changes` request with a `client` is that client's cursor (`store::map::Cursor`): the `seq` it
asked from, when it was last heard (`heard`), when it was first heard at that `seq` (`at`: its
adoption of that map) and, for a mirror, the map its round copies and the last it got through at
(`copying`, `synced`, each with when first said). The lead keeps them in memory, saves them with
its snapshot and restores them at a start: a restart can only bring a cursor back older, which
keeps files longer and never frees one early; one first heard since is gone until it asks again.

| Client | Id | Holds files on the NAS? |
| --- | --- | --- |
| a Mac's map server | `server/<host>` | yes: it reads the NAS for what its mirror lacks |
| a Mac's mirror | `mirror/<host>` | yes, while it copies a `seq`'s files |
| the lead's agent (the plan, the status, the build page's views) | none: in process | no: it reads names, not files |

A cursor holds, from GC, every file the maps since its `seq` name. One unheard for 6 hours
(`store::map::gc::CURSOR_TTL_S`) holds nothing: a laptop's map server runs on demand and quits 30
minutes after its last use, its cursors left where they were. A mirror's cursor is also answered a
reset whenever the lead didn't hold it (never heard, unheard that long, or held at one `seq` behind
the head that long), since a file it would copy may be gone (docs/macs.md, Mirror). The browser
pages are not clients: a page holds logical names and versions and asks its Mac's server, which
resolves them in the map it holds.

### The catalogue: the map as its readers take it

Readers don't see entries: `store::map::Map::catalogue` makes a `store::catalogue::Catalogue` of
the map (`n` its `seq`, `created` its batch's time), and `store::map::entries` turns one back into
entries, the two exact inverses. The `catalog` step posts the map it builds in this form. No
catalogue is a file on the NAS.

Spelling: the code calls this form a catalogue (the docs say the map, and a map update for the
`catalog` step's batch), but the stored and wire names say `catalog`: the pool's
setting `hold-catalog`, the steps `catalog` and `catalog-held` (`store::catalogue::NAME`,
`HELD_NAME`), the history event kind `catalog`, JSON keys (`catalog`, `catalog_held`, `catalog_t`,
`catalog_at`, `catalogs`), the routes `/api/catalog` and `/api/catalogs`, the commands `scenic
catalogs` and `lakefill catalog-add`, the log line prefix `catalog-stats:` and the phase names
(scenic-build's "sorting the manifest's files into the catalog's" and "writing the catalog",
namestodo's "reading the catalog, the languages spoken and the translations").

```json
{
  "fmt": 2, "n": 7, "created": "2026-10-03T04:05:06Z", "app": "<published app version, or development>",
  "files": {"<logical>": {"file": "<content name>", "size": 123, "fmt": 1}},
  "units": ["6/32/21"],
  "layers": {
    "terrain": {"encoding": "terrarium-webp", "minzoom": 0, "maxzoom": 16, "root": "<logical>",
                "lo": {"3/4/2": "<logical>"}, "hi": {"6/32/21": "<logical>"},
                "deep": {"10/295/360": "<logical>"}, "native_zoom": {"10/295/360": 16}},
    "polar-terrain-s": {"encoding": "terrarium-webp", "minzoom": 0, "maxzoom": 9, "root": "<logical>",
                        "lo": {}, "hi": {}, "polar": {"4/7/8": "<logical>"}},
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

- `files` holds every file the map names.
- `fmt` 2 (task #449): `markdata` may name the bundle of every tile ("Bundles"). Readers take 1 and 2
  (`store::catalogue::MIN_FMT`, `FMT`).
- `credits`: `inputs::rules::CREDITS` plus the inputs' accepted descriptions
  (`inputs::inputs::credits`, docs/inputs.md §4.8; a description replaces the table's entry of its
  `what`), those whose areas meet the coverage + 20 km or a built z6 tile's extent. `areas` are w, s, e,
  n boxes in degrees, left out for credits that hold everywhere. A heritage register's credit has
  `register`, which popups name their source by (docs/inputs.md §6.5).
- `dem_labels`: the elevation datasets' labels by source byte (`src.u8`), from their accepted
  declarations (`label`, else `name`; `inputs::inputs::credits::dem_labels`, docs/inputs.md
  §6.8): how a road's details name its elevation sources. Left out when none; a map without
  it gets "source <n>".
- `coverage`: the regions built for: those done at the map update as their recipes then were
  (`--ready <id>=<outline digest>,…`), and those not done yet as the map had them, in the
  recipes' order. A region still building (the agent's `--building <id>=<built>/<total>,…`: every
  region its plan doesn't record built, with its z6 tiles built of those it builds, by the records)
  that the map didn't have built is recorded as its recipe is now with `tiles`
  (`coverage::DrawnRegion::tiles`): its z6 tiles are on the map, and downloadable, as they come. A
  built region has no `tiles`. Outlines are GeoJSON MultiPolygon coordinates in degrees to 5 decimals, rings
  closed: `osm:` entries from the pass's simplified outlines, others simplified by size (60 m–1 km);
  a region with water (docs/inputs.md §6.1.3) lists its `water` parts after its outline's, each
  `"water <part>"`, and draws one shape, its land and water together;
  an unreadable entry has no shape. A map with `"credits": []` and no `recorded`: the server
  gives every credit and builds the coverage from the recipes.
- `files`' sizes: the records' (from each file's job's hand-off) or the map's (the same content
  name, the same bytes); the step looks at the NAS only for a file neither has.

## On each Mac (`~/Library/Application Support/scenic/`)

```
bin/scenic-launcher     the launcher (never rebuilt)
run/<name>              what the launcher runs, one argument per line: server, agent (written by
                        Scenic.app when it opens)
agent/quit-request.json Scenic's ask to quit (control::Quit: finish or now), until the agent exits
app/<version>/          server, scenic, scenic-build, dem/, Scenic.app,
                        web/, fonts/ (app/current → the one in use), unpacked from the NAS's
                        app/releases/<version>.tgz (docs/storage.md)
pyenv/<key>/            the Python steps' environment of a lock file (dem/uv.lock + .python-version),
                        shared by the versions with it (jobrt::pyenv); .used: its last use
pyenv/.lock-<key>       flock'd by the one making of that key (kept: removing a held lock breaks it)
mirror/<content name>   downloads, named as on the NAS (docs/macs.md, Mirror; .partial/: in progress)
mirror/.basemap/<hash16>/  the basemap's pieces (store::pieces): lo.pmtiles (zooms 0–10),
                        <z>-<x>-<y>.pmtiles (the one tile, z 0–5), mid-6-<x>-<y>.pmtiles (a z6
                        tile's 6–10), 6-<x>-<y>.pmtiles (a z6 tile's 11–14), PMTiles v3 (clustered,
                        gzip'd directories, tiles as stored); sizes.json {piece: bytes}
mirror/.packs/<hash16>/ the pieces of a root or lo pack (store::pieces::plan_pack): <z>-<x>-<y>.pack,
                        mid-6-<x>-<y>.pack, each an RDPACK of those tiles alone (the pack's flags
                        and meta, each blob once, its index); sizes.json {piece: bytes}
idx/<hash16>.idx        pack indexes (RDPKIDX1: header, meta, entries, XXH3 trailer)
accepted/<unit>/        accepted inputs copied from the NAS: @index.json ({"version", "index"}) and
                        each file by content name (inputs::inputs::served); a sync leaves the
                        other `@` names at its top, the server's own:
accepted/regions/@listed.json   the inputs folder's region files as last listed (crates/server/src/
                        regions.rs, `Listing`): {"files": {"<path>": {len, modified (ns), read:
                        {"Ok": <the panel's region, with its state>} | {"Err": "<problem>"}}}};
                        /api/regions answers from it, the regions thread lists the folder each
                        minute and reads only the files whose size or time changed (every file hourly)
accepted/regions/@outlines/z<z>.jsonl   the outlines at zoom z (0–12): a key line (the format, the
                        accepted version or the map's `seq`, the pass's outlines), then a GeoJSON feature
                        a line; made for every zoom when the key changes
names/spoken-<content>.bin   languages spoken where, for a map without `global/spoken`
                        (names::spoken::Spoken::to_bytes: "SPOKEN01", u32 head length, head (rules'
                        version, then per region its ISO code, a tab, its languages), u32 run
                        count, 23,041 u32 row starts, runs of (u32 first column, u16 region))
region-areas.json       each accepted region's land and water (km²) and country, by its geometry's
                        content name: {"<name>": {land, water, country}} (crates/server/src/regions.rs,
                        `sized`; worked out once per geometry, unused names dropped)
reserve.json            the mirror's reserve {gb} (crates/server/src/reserve.rs); default 30 GB
downloads.json          {fmt: 5, world: {at, cat, to?} | null, regions: [{id, name, at, cat, to?,
                        detail: {cat, to?} | null}], lists: {"<cat>": {world?: [item], regions:
                        {"<id>": {coarse: [item], detail: [item], cover}}, partial?}}}
                        (crates/server/src/downloads.rs; `at` unix seconds; `cat` the map (its `seq`) a
                        download is at, 0 until the mirror's next round pins it to the one served;
                        `to` the map an Update is copying; `detail` its terrain detail, when
                        downloaded; `lists` each download's own list at the map served and at
                        each one a download is at or updating to, an item {file, size} (a content
                        name) or {archive, piece} (a basemap piece: lo, 6-x-y), `cover` the share
                        of the region its terrain detail covers, per cent; `partial` true when made
                        while some of its ways' owners weren't known, made again once they are). A file of fmt 4 keeps
                        its pins, its lists made by the mirror's next rounds at the map served; one of
                        fmt 3 or before (`world` a time, `detail` or `fine` a flag, fmt 2's `views`)
                        is written again as fmt 5, each download pinned to the map served; one
                        that doesn't read is set aside as downloads.json.bad
map-page                the address to open the map on another device (docs/macs.md, Devices; 0600)
pool-<id>.lock          the lock of this Mac's member `<id>` (crate::pool::MemberLock)
```

The agent's folder, `agent/`:

```
timings.jsonl, timings-run.json, timings-run-<k>.json   jobs' timings (below)
spans/<epoch>-<n>.{ndjson,json,sent}   a job run's spans, facts and what the lead has of them, until
                        the lead has all (below, Job spans); the lead's: coord/jobs/
status.json             the lead's status; a member not leading writes helper.json
state.json, job.json, logs/   job.json (job-<k>.json, slot k's past the first): the running job's
                        record (agent::jobs::Record) with its slot (`hand`: {lease, job, …}, `job`
                        its run's id, absent in an older app's: its lease's); a job's log's run
                        begins `=== run of <name> (<unix s>, job <epoch>-<n>) ===` (no job part
                        before job ids, or for a job of no lease)
agent.lock              the agent's lock (one agent a Mac), its text its holder's `<pid> <start µs>`
                        (agent::takeover, docs/surface.md)
publishing.json         the lead's record of its map updates {held_at, ran}: when an update due was
                        first held back since (none: none is), the cadenced steps' runs
                        (agent::publish::LastCatalogue); when the last went out, and the last with
                        the low zooms, are the map log's (its head batch, its mark of them)
maplog/                 the lead's map and its log, the maps held for review and GC's queue
                        (above, The map and its log: The lead's store)
catalogues-view.json.zst  the build page's map updates as followed (agent::cataloguesview: its
                        entries, `seq`, tile keys' and `f/` names' hashes; docs/pool.md §10)
clear-request.json      the owner's ask to clear build caches ({by, at, level: caches | deep}, agent::room::ClearRequest;
                        renamed .taken when taken up)
room-target.json        the owner's free-disk target {bytes, by, at} (buildstore::room::Target; none: off)
sweep-margin.json       the dev sweep's margin past it {bytes, by, at} (agent::settings::SWEEP_MARGIN;
                        none: SWEEP_MARGIN_DEFAULT_GB; read by tools/dev/sweep-targets.sh)
member                  this Mac's pool member id `m-<16 hex>`, then its hardware UUID
pack-idx/<hash16>.idx   indexes of the terrain packs the manifest names (agent::tiles); never
                        trimmed; one unnamed and untouched for a fortnight goes; the lead's
                        alone (removed as a member starts)
pool/                   crate::agent::pool: saved.json (crate::pool::driver::Saved: {member, mine:
                        {entries, unwritten}, records?: [epoch, seq]}; an older app's keys read
                        past; saved.bad one that can't be read, set aside), records.json (the lead's own copy
                        of the records it saves, written before the NAS's: docs/pool.md §6.2),
                        mail.json ({read, sent, n}), members.json,
                        forgotten.json, jobs/<epoch>-<n>/ (a job run, by its job id: work.json {step,
                        targets, lease (its job id)}, its hand-offs, done.txt, costs.jsonl, unmerged.json),
                        lead.json (crate::agent::lead::Kept), notes.jsonl
cache/                  build caches (below)
```

`agent/cache/`: `chm10/` (canopy files), `aws-terrarium/` (raw tiles
until packed onto the NAS; `packs/`, archive copies), `blobs/`, `base/`, `sources-*/` and `work-*/`
(local copies of NAS files by logical name), `heritage-merged-<date>-<set and cover>.osm.pbf` (the `whs` set within the cover), `items/`
(the items job's answers, `kept-<date>.json`: {archive, files: {name: hash16}}; `months/`: pageview
index copies), `rail/` (stop pairs), `heritage-<date>/` (the heritage chain's answers, with its
`.kept.json`), `unit-stages.json`, and the flocks `answers.gate`, `answers.use`
(crate::answers::Local). Room-making, trimming and clearing are agent::room's (docs/macs.md and docs/building.md §2). Every
file is read and made through store::cachefile: a shared flock while a job uses
it; made as `<name>.<pid>.<n>.tmp`, locked, and hard-linked to its name only if the name is free;
deleted only under an exclusive flock taken without waiting.

`~/Library/Preferences/nsmb.conf` gets `[FISHANDCHIPS:PERSONAL]` and
`[FISHANDCHIPS.LOCAL:PERSONAL]`, both `soft=yes`.

## Timings (`timings.jsonl`; jobrt::timings, docs/building.md §2)

A job's run, a JSON object, `jobrt::timings::RunRec`:

```
{v: 2, kind, id, job, host, start, wall_s, cpu_s, untimed_cpu_s, threads, held, ok, untimed_s, overhead_s, phases: [phase, …], gaps: [gap, …], safe_points, safe_gap_s, cost_v}
phase: {name, class (only before task #870), wall_s, waits: {nas_s, net_s, disk_s, other_s, both_s, nas_b, net_b} (task #870: measured, by wall clock; both_s, task #891: 10 seconds at once by store::waits::PAIRS, absent when all 0), cpu_s, uncertain_cpu_s, job_cpu_s, n, bytes, files, overlapped, background, peak_mb, sub: [phase, …]}
```

- `kind`: the job's step (`unit`, `peaks`, `gc`, `backup`, `osm-pass`), or `task <kind>` (`task
  tail`, `task bldtile`, `task treeblock`, `task terrainsub`). `id`: the job's name as its agent
  knows it, not unique (`SCENIC_JOB_NAME`, "unit 6/32/24"; a leased task's "<kind> <unit>", "tail
  6/32/24", task #849). `job`: its run's job id (below, Job ids: `SCENIC_JOB`, else stamped by its
  agent and by the lead as they keep it; absent before task #1103 and for a job of no lease).
  `host`: the Mac. `start`: unix seconds.
- `v`: 2 since task #1116 (a phase's `cpu_s` its own work's); 1 before (a phase's `cpu_s` the
  process's while it was open, so an `overlapped` phase's counts other threads' work too).
- `cpu_s`: user + system, the process's and its finished children's. `untimed_cpu_s` (v 2): of
  it, the CPU in no phase; the top-level phases' `cpu_s` (the `background` ones' too) and it add up
  to `cpu_s`. `threads`:
  RAYON_NUM_THREADS. `ok`: ended without error (a pause isn't). `untimed_s`: `wall_s` less the
  top-level non-`background` phases. `overhead_s`: the timing's own cost.
- `held` (`[{threads, wall_s, cpu_s}]`): what its targets run on their job's grant held, by
  threads (task #948). `safe_points`, `safe_gap_s`: the safe points it reached and the longest it
  went without one. `cost_v`: its step's cost version (`coord::cost_version`), stamped by the lead's
  coordinator as it keeps the record; absent before that (task #929 review), read as 0 by the uses.
- `gaps` (`{after, before, wall_s, n}`): where `untimed_s` lies, the main thread's spans between
  two top-level phases summed by the phases either side ("the start" and "the end" at the run's
  ends), its eight largest, each 1 ms or more; left out when none (a Python child's record has
  none).
- A phase totals its `n` spans. `class`: `nas-read`, `nas-write`, `disk`, `net`, `compute`, `wait`,
  `mixed`. `cpu_s`: its own CPU (v 2; jobrt::timings' doc): each thread's CPU charged to the span
  innermost on it, and the job's on threads in no phase (a rayon pool's workers, child programs)
  to the main thread's innermost span; a phase's includes its sub-phases'. `uncertain_cpu_s`: of
  that, the job's CPU in no phase taken in while another thread's phase was open too, which may
  have been that one's work; absent when none (`timings::uncertain_cpu` reads a v 1 record's
  overlapped phase's whole CPU so). `job_cpu_s`: a main thread's top-level phase's, the job's CPU
  (every thread's) while it was open, which the uses' floor reads. `overlapped`: another thread's
  phase ran during it. `background`: off the main thread.
  `peak_mb`: the most memory the job's process group held while it was open (physical
  footprints summed, sampled every 250 ms, as the memory guard measures a job; the most over its
  spans); a phase running a program is that program's peak. Absent where it can't be read. `sub`: one level of sub-phases (a child program's phases come in here). Empty, zero and false
  fields left out.

Where they're kept:
- A job writes its record once where `SCENIC_TIMINGS` says (`agent/timings-run.json`,
  `timings-run-<k>.json` for slot k's job past the first, or a member's leased task's
  `outbox/<lease>/timings.json`);
  a child program writes its phases where `SCENIC_PHASES_TO` says (removed once read).
- `agent/timings.jsonl`: every job this Mac's agent ran.
- `agent/coord/timings.jsonl` (the lead's coordinator): every worker's, sent with `/work/done`;
  read by `/api/timings {kind?, last?}` (`{runs: [...]}`, each kind's last 20 by default).
- Each log is cut to its newer half past 16 MB.
- Its job's spans (below, Job spans): the record is also kept beside them on the lead, by its job
  id, as the record comes in.

## Job ids (jobrt::jobid, docs/pool.md §7.5)

A job run's id is `<epoch>-<n>`, a string: the lease the lead's coordinator first granted the run
under (`n`, the lease's number) and the epoch of the lead's store it was granted in. Epoch 0 is no
lead's (a coordinator out of the pool, a hand-off drained from one). Where a run is recorded:

- the history (`coord/history.jsonl`, `/api/history`): `job` on a worker's job's `lease`, `done`,
  `fail`, `lapse` and `cancel`, a lead's own job's `start`, `end` and `cancel`, a task's `task`,
  `task-fail` and `task-back`; a `merged` event's `data.done` keys (`<day>/<epoch>-<n>`) end in it;
- the status: the coordinator's leases (`pool_view`'s `leases[].job`, beside the lease's own `id`),
  each Mac's running jobs (`jobs[].job`) and its jobs ended (`state.json`'s `recent[].job`);
- the timings' record (`job`, above), the cost book's entries (`cost/<key>`'s `job`), the job's log
  (its run's first line), its record (`job.json`'s `hand.job`), its hand-off (`lease`) and its
  folder (`pool/jobs/<epoch>-<n>/`).

A record from before job ids (task #1103) names its run by its lease's number alone, a JSON number
under `lease` in a history event: read as `0-<n>` (its epoch unsaid). An app before them reads
no `job`, and passes over a history event's run. A run's later lease, held again for it while it
runs on (its own lapsed, or lost to a lead's restart), carries the run's id (`lease/<id>`'s `job`).

## Job spans (jobrt::spans, agentcore::agent::spans, agentcore::coord::jobspans; task #1104)

A job run's timeline, which its timings' record (above) sums: every phase and sub-phase a span with
its start and end, the waits as intervals, what happened, and its log on the same axis. Only a run
with a job id (its agent holds a lease for it) keeps spans; one from before task #1104, or run by
an app before it, has none (the API's `legacy`).

**The job's file** (`SCENIC_SPANS`, set by its agent: `agent/spans/<epoch>-<n>.ndjson`): a JSON
line each, `jobrt::spans::Line`, written once a second by the timings' memory sampler (one append)
and as the run ends. Times are µs from the job's start (`h`'s `start_us`, unix µs):

```
{k: "h", v: 1, start_us, kind, pid, in?}                       the start (a child program's: in = the span running it)
{k: "o", id, in?, th?, name, t0}                              a span open a second or more (its "s" line replaces it)
{k: "s", id, in?, th?, name, t0, t1, n?, busy?, cpu?, w?, b?, f?, cut?}   a span
{k: "w", on, t0, t1, n, busy}                                 waits on "nas" | "net" | "disk" | "other"
{k: "e", e, t? | at?, t1?, n?, ok?, text?}                    an event
{k: "l", t, at}                                               the job's log was `at` bytes long (the file's) at t
```

- A span: its `id` (a child program's under its prefix, `3.`, `3.2.` for that one's child), its
  parent `in` (none: top-level; a child's top-level spans are in the span that ran it), its lane
  `th` (none: the job's main thread; `1`, `2`: other threads in the order they first opened a span,
  a child's under its prefix; a child's main thread is its parent span's lane), its own CPU `cpu`
  (µs, jobrt::timings' doc), its waits while open `w` (µs: NAS, network, disk, other work, the
  job's), bytes `b` and files `f`; `cut` when the job ended with it open.
- Merged: spans under a threshold (1 ms at first, `MIN_OWN0_NS`) with no sub-spans, of one name
  under one parent on one lane, each starting within the threshold of the one before, are one line:
  from the first's start to the last's end, `n` of them, `busy` their time together. Waits of a kind
  within a gap (2 ms at first, `WAIT_GAP0_NS`) likewise: `n` waits, `busy` µs waited. Each 2000
  lines (`STEP`) doubles both; past 40,000 (`LINES_MAX`) only the main thread's top-level spans and
  the events are written (an event `thinned` says so). Each program counts its own lines: a child
  program's are bounded apart from its job's. Safe points a second apart are one event. A retry
  storm's notes past 200 a second (store::waits' `NOTES_MAX`) are one event saying how many more.
- Events (`e`): `retry` (store::retry's line), `safe` (`n` safe points from `t` to `t1`), `pausing`
  (asked to stop at its next safe point), `paused`, `error` (the job's error), `thinned`, `end`
  (`ok`: the job's own word), `exit` (its agent's, `at` unix ms: `ok`, how it ended, `lost` when its
  agent lost it).
- Beside it its agent keeps `<id>.json` (`agent::spans::Facts`: `meta` (`coord::jobspans::Meta`:
  `{name, step, targets, worker, host, start, log_from, app}`, `log_from` the log's length as this run
  began, `app` the app its agent ran, none from an agent before task #1105), its log's path, `ended`) and `<id>.sent` (`{s, l, meta}`: what the lead has). All three go
  once the lead has all of it; any after 14 days (`agent::spans::KEEP`, said in the agent's log).

**To the lead:** a member's beat (`POST /pool/beat`) carries `spans: [Chunk]`, at most 512 KB a
beat (`agent::spans::BEAT_MAX`), to a lead whose last answer said `spans` (`coord::beats::SPANS`;
an older lead's doesn't, and gets none); the answer's `spans: [Ack]` say what it has. The beat
after one that failed carries none, and each failed beat that carried them quarters the bound (to
16 KB, `BEAT_MIN`; doubled again by each that gets through), so a slow link never holds up the
leases' beat. The lead's own jobs' go to its store every 10 s.

```
Chunk {job, meta?, s_at, s, l_at, l, end?}    the spans file from byte s_at (whole lines), the run's log from byte l_at
Ack   {job, s, l, meta?, done?}                the lead has them to s and l; done: all of it (its agent drops its files)
```

A run's log goes whole up to 1.25 MB; past that its first 1 MB (`LOG_HEAD`) as it runs and its last
256 KB (`LOG_TAIL`) once it ended, the middle left out (the lead keeps a `gap`).

**On the lead** (`agent/coord/jobs/<epoch>/<n / 10000>/<epoch>-<n>.zst`): zstd frames, one a chunk
(level 3), the whole file compressed again as one frame (level 9) once its last chunk is in; each
line one of the job's (as written) or the lead's own (`coord::jobspans::Own`): `{k: "job", …Meta}`,
`{k: "log", at, text}` (the log from byte `at` of the run's), `{k: "gap", of: "log", from, to}` (the
log's middle left out), `{k: "at", s, l}` (after each frame), `{k: "done", at}` (unix ms), `{k: "rec",
rec}` (the timings' record, as `/work/done` or the lead's own job's end brings it). A chunk is taken
from where the lead has the job to: one posted again adds nothing; one from past it (a lead that
took over mid-job) isn't taken, and its agent, told where the lead has it to, sends it again from
there, so the new lead has the whole run. The answer says only what's written: a frame that fails
moves nothing. Nothing removes the files, as nothing removes the history's (docs/pool.md §7.5), but
they stay on the Mac that held the lead: a later lead answers `legacy` for the jobs an earlier one
kept (planned: task #1172).

**Measured** (12 Nov, the M4, real jobs by hand with saves handed off, `coord::jobspans::tests::
sizes_on_the_lead` for the lead's size): terrain-deep 10/914/378 (100 s, 169 s CPU): 1,955 lines,
198 KB written, 37 KB on the lead, the spans' own cost 83 ms (0.05 % of its CPU; its CPU 169.3 s
with spans, 169.2 s without); slope-deep 10/906/404 (72 s, 107 s CPU): 762 lines (727 of them
waits), 9 KB on the lead, 2 ms; unit 6/50/31 (131 s, 83 s CPU, with a child program's spans; it
failed on a grid a by-hand scratch lacks, after most of its steps): 192 lines, 5 KB, 5 ms. About 19
bytes a line on the lead. A month: the pool's last fortnight ran 57,000 jobs and 4,300 job-hours a
month, 12,800 of them terrain-deep (mean 6,500 timings' spans a job: ~55 KB each, 0.7 GB) and 4,400
depth (35,000 a job, most merged: up to the 40,000-line bound, ~0.2-0.8 MB each, 1-3 GB), the rest
~10 KB a job (0.4 GB): **about 2-4 GB a month**, mostly depth's; a figure derived from these runs,
to be checked against `agent/coord/jobs/` once the pool runs with spans.

**`GET /api/job/<epoch>-<n>`** (the lead's coordinator; no key, any page; a member's port sends it to
the lead): 400 for a malformed id, 503 off the lead, else:

```
{job, legacy: false, running, now, start, end, kind, name, step, targets, worker, host, app,
 record: RunRec | null,                 the timings' record (above)
 history: [Event],                      the history's events of the run (lease, start, end, done …)
 spans: [{id, parent, kind, name, t0, t1, status, attrs}],
 waits: [{on, t0, t1, n, busy_s}],
 events: [{kind, t, t1, n, ok, text}],
 log: {lines: [{t, text}], bytes},
 gaps: [{of, from, to}],
 runs: [Run], runs_total,               the history's other runs of its step meeting its targets, the newest 50
 beside: [Run]}                         the history's other runs on its Mac while it ran, the first 200 by start
{job, legacy: true, record, history, now, runs, runs_total, beside}   no spans kept here (from before them, an older app's, or kept by an earlier lead)
Run {job, step, targets, n, worker, from, until, ok, lost}   targets: its first 3 (n all); until null: no end in the history (lost: and no lease holds it now)
```

Every time is unix seconds (ms precision). `spans` is in the dev jobs' run shape (tools/devtools'
`GET /api/jobs/runs/<id>`): the run itself first (`id` "job", `kind` "job", `status` `running`,
`fail` when its agent's `exit` (else its last `end`) says not ok, else `ok`), then by start; `kind`
`phase` (the main thread's lane) or `beside` (another thread's); `parent` "job" for a top-level
span; `status` `ok`, `running` (said open, the job running) or `cut` (open as it ended, or as its
word ended: its `t1` the last time heard); `attrs` `{th, n, busy_s, cpu_s, nas_s, net_s, disk_s,
other_s, bytes, files}` where said. A log line's `t` is where the job's log marks place it (between
the two either side, by its bytes; none without marks). `app` is null from an agent before task #1105.
`runs` and `beside` (`coord::jobspans::relate`, task #1105) come from the history's seven days, read
in place (`History::all`, no copy): a run's `from` is its end's time less its `secs`, else its
`start`'s, else its `lease`'s; its end an `end`, `done`, `fail` or `cancel`, or the next lease of its
step and targets under another id (a restart before job ids leased its work again; `ok` null). A
lead's own job's `start` and `end` from before job ids (task #1103) name no job: each is its Mac's
open run of the same step and targets. A run of no end is running while one of the lead's leases holds
it, else `lost` (left out of `beside`). `runs`: the runs of its
step any of whose targets is one of its own (a step of no targets: every run of it), newest first;
`beside`: those on its Mac (any slot) from its start to its end (or now). A legacy job's step,
targets, Mac and times are its own events'. The job's page reads it (docs/pipeline.md §5, A job's
page), its step's medians from `POST /api/timings` (`{kind, last}`, also a page's to ask).

## The forecasts' archive (`state/coord/forecasts/<day>.jsonl.zst`; agentcore::agent::fcarchive, docs/pool.md §7.9)

A day's file (UTC) is zstd frames appended one after another (read frame by frame, a damaged frame
skipped to the next frame's magic number): each a
batch of lines, a line a forecast, `fcarchive::Entry`:

```
{v: 1, code, app, at, done_at?, range?: [soon, late], why?, partial?, measured,
 lanes: [{name, mac, speed, measured, busy_s, from_s?, running?: [step, n], jobs?: [[step, target, gap, secs], …]}, …],
 macs: {<Mac>: {slots, room?: {mem_mb, cores},
        cond?: {best?: {mem_mb, cores}, limit_mb, build_mb, mem_mb, cores, cores_free, kept_cores, disk_mb, in_use, ac, battery, nas}}},
 each_s: {<step>: s},
 step_pace?: {<step>: x},
 nas?: {bytes_s, creates_s, listed_s, stats_s, deletes_s, downloads, held},
 steps: [<step>, …]}
```

- `v`: the record's format (1). `code`: the forecast's code version (`forecast::VERSION`). `app`:
  the lead's app. `at`: when it was made (unix s); `done_at`, `range`, `why`, `partial`, `measured`
  as the status's forecast says them (docs/building.md §2).
- `lanes`: the run's lanes in its order (the lead's first slot first): `name` (a Mac, `<Mac> #2`
  for its next slots: the history's `worker`), `mac` (its Mac), its speed against the lead's and whether
  measured, `busy_s` (s until its job under way ends), `from_s` (s until its slot opens),
  `running` (that job's step and targets, when no placed target is it). `macs`: by Mac, its
  lanes, its room when it had no ask, and its last ask's capacity's fields (crate::coord::Capacity;
  `best`, code 5 on: its sustained best room, what the run took it to have, docs/pool.md §7.2).
  Lines before code 5 may hold a Mac's `held` (the day before's hold, removed), read past. `each_s`: each
  step's time a target at the lead's pace. `step_pace`: each step's pace against its costs the run
  took (`forecast::step_pace`, docs/pool.md §7.9; code 2 on). `nas`: the NAS budget the scheduler
  held to (crate::budget::nas::Limits). From code 2 a lane's `busy_s` is held to its step's longest
  job of late (`forecast::cap_busy`).
- A lane's `jobs`: every target the expected run placed on it, in time order: its step (an index
  into `steps`), the target, its gap (s: its start less the end of the one before it on the lane,
  `at` for the first; may be negative) and its time (s: its end less its start). Back-to-back work
  makes the gaps mostly 0: a third of the size of unix times once compressed (`Entry::placed`
  reads them back as unix times).
- A forecast the same as the last written (docs/pool.md §7.9) isn't written. Days older than 7 go
  into `state/coord/forecasts-archive/<YYYY-MM>.tar`, a ustar of `<day>.jsonl.zst`, kept for good.

## The forecast inputs' archive (`state/coord/forecast-inputs/<day>.jsonl.zst`; agentcore::agent::fcinputs, docs/pool.md §7.9)

A day's file (UTC) is zstd frames appended one after another (read as the forecasts' archive's
are: a damaged frame skipped to the next frame's magic number), each frame lines, a line an input:

```
{"term": <E>, "at": <unix s>, "record": <forecast::Record>}
```

- `term`: the epoch its lead led in when it made the forecast (in older inputs, a term's
  number); `at`: the forecast's time (the record's `input.now`); `record`:
  the forecast's whole input, as the terms' `state/build/term/<E>/forecast-input.json` hold it
  (below, Forecast input), uncompressed inside the frame.
- A frame is written with long-distance matching over a 64 MB window (`fcarchive::encoder`), so an
  input after the first in its frame is stored mostly as what changed since the ones before it.
  A reader decodes a frame whole to read one of its inputs (`fcinputs::Input::record`). A frame
  tried again after its write passed the NAS's deadline may be there twice; a reader keeps the
  first input at each time.
- An input at most every 15 minutes (`fcinputs::EVERY_S`); days before the backtest's default span
  (7 days, `fcinputs::KEEP_S`) are removed by the `gc` job's daily duties.

## Server API

- **Tiles:** `/tiles/{roads,rails,terrain,slope,labels,water,base,buildings,buildings-field,depth,depth-class,depth-shade}/{z}/{x}/{y}`,
  `/tiles/trees/{var}/{z}/{x}/{y}`. Strong `ETag`: the stored blob's hash, plus for named tiles the
  versions of the languages spoken within them and of the spoken-languages raster (docs/names.md);
  `/tiles/base`'s a hash of the basemap archives' content names, the tile's z/x/y and the names
  version; tiles the server makes (missing terrain and shading tiles, a deep one that can't be read)
  carry none. Terrain, shading and depth tiles (`slope`, `depth-shade`: stored as `shade-webp`, Pack;
  `depth`, `depth-class`: Depth) are `image/webp` (a depth pack from before, `image/png`), stored or made (made ones encoded as the stored are, from the nearest stored
  ancestor, scaled up; a stored shading tile in another encoding, a pack from before it, is read as
  none). With `?v=` (the app's URLs): `public, max-age=31536000, immutable` while that version is
  current, else `no-cache`; the tiles the server makes (terrain and shading from ancestors, flat
  tiles, the floor under the water, depth made from a coarser zoom) always `no-cache`, as their sources can lie in other packs. `/api/meta` versions each layer (`buildings.tiles`, `water`: the drawing's, its packs'
  and the basemap's) and, under `packs`, each pack of a tiled layer (`{layer: {root, lo: {"3/x/y"},
  hi: {"6/x/y"}}}`: its content name's hash, 12 hex digits, plus the names version for layers with
  names). The map's tiles carry their pack's token (root z0–2, lo z3–8 by z3 tile, hi z9–14 by z6
  tile: `web/src/api.ts` packVersion; MapLibre's sources through the map's request transform), so a
  map update re-requests only the tiles of the packs it changed. Every pack token and layer version
  meta gives out is current (the polar caps' blocks' too).
- **Every data endpoint answers alike** (`server/src/cache.rs`): `Content-Encoding: gzip` by the
  pack's own flag (`PackIndex::gzip`) or for what the server gzips; `If-None-Match` read as a list,
  a match a 304 with the ETag and the same Cache-Control; a tile the layer hasn't, at an address
  that can hold one, 204 (no body; depth and polar caps included); a URL naming no tile (an unknown
  kind or pole, outside the grid or the layer's zooms) 404; a read that failed (the NAS away) 503.
  What the server keeps in memory is kept in one kind of cache (`cache::Lru`, least recently used
  out past a byte or entry budget).
- **Ways:** `/api/way/{id}?at=lon,lat`, `/api/profile/{id}?at=…`, `/api/road/{id}?at=…`: `at` picks
  the hidata of the z6 tile holding it, or a neighbour's whose `here` holds the way.
- `/api/railfreq`: sorted `(u32 way id, f32 trains a day each way)`; negative: a lower bound. ETag: its
  content name's hash.
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
- `/api/catalog`: `n`, `created`, `units` (a count), `layers` (encoding, zooms, version), `coverage`
  (without outlines), `credits`, `online`, `nas`, `held`, `app`, `agent`, `names` (`langs`, `spoken`,
  `warning`, `waiting`), `v`, `marks` (tiles with points, kinds with tiles, totals), `depth` (the
  map's meta's: its sources, Depth below). The map's meta: `/api/meta`.
- **Depth:** `/tiles/depth/…` and `/tiles/depth-class/…`: the stored tile, else made from its nearest
  stored ancestor (`terrain::depth::fall`), 204 where it has no ancestor, 404 past its zooms; versions `depth.tiles`,
  `depth-class.tiles`. `/tiles/terrain/{z}/{x}/{y}?floor=1`: the water's surface less the depth where
  the class tile says water (the floor under the water, for a 3D floor); the terrain as it is without
  depth in the map. `?surface=1`: the water's surface (sea level where the water tiles say
  sea and it's the world ocean's, not an enclosed sea's (Water, above); the terrain elsewhere), the map's 3D terrain without the floor; a tile with no pixel at sea
  is the stored terrain tile, with its ETag; one with sea, made from a stored terrain tile, has its
  own (that tile's and the water version's). Under the current water version (`w=`), either is
  cached for good.
- `/api/build`: the agent's status (this Mac's when it leads, else the lead's heartbeat, or this
  member's `helper.json` when that is ten minutes stale), `pool`, and `offline:
  {world, regions, bytes, here, nas}` (null without a mirror).
- **Remote access** (docs/macs.md, Devices): no key. A request from anywhere but this Mac, its LAN and
  the tailnet (through `tailscale serve` or Caddy on this Mac too; never Funnel) is 403 ("not from
  here"), as is one whose `Host` isn't the map's (an address, a local or tailnet name, or one in
  `~/.config/scenic/names`) or whose `Origin` is another page's. CORS answers only this Mac's own origins.
- **Downloads** (`downloads.rs`): `GET /api/downloads` → `{mirror, online, slow, rate, free,
  reserve, reserve_min, disk, here, world: {…dl, at}, wanted: {bytes, here, more, unknown}, copying:
  {what, bytes, have, slow} | null, last: {…} | null, regions: {id: {…dl, name, tiles?, detail?:
  {…dl, cover}}}}`, each `dl` `{bytes, here, unknown, on, state, cat?, behind?, update?}`: its size and
  how much is here at its own map (not downloaded: at the one served); `unknown`: basemap
  pieces not yet sized; `state`: done, copying, queued, room, away, missing, updating; `cat`: the
  map it's at; `behind` (behind the one served): `{cat, files, gone, what, bytes, unknown,
  state?}`, the map an update takes it to, the files new to it and those it lets go, the
  kinds of what changed (a layer's name, basemap, road data, lists…), the bytes still to copy;
  `update` (updating): the tally of those new files.
  `PUT /api/downloads/world {on}` (each on its own: a region doesn't
  bring the World, and the World goes while regions stay); `PUT /api/downloads/regions/{id} {on}`
  (off removes its terrain detail too); `PUT /api/downloads/regions/{id}/detail {on}` (its terrain
  detail, the deep packs near it; on refused unless the region is downloaded); a new download is at
  the map served. `POST /api/downloads/world/update`, `…/regions/{id}/update`,
  `…/regions/{id}/detail/update`: update it to the map served (refused when it isn't
  downloaded or isn't behind). `PUT /api/downloads/reserve {gb}` (whole GB, 5 to under the disk
  size) → `{ok, reserve}`. A download or update that wouldn't fit is refused; refusals are 400
  `{error}`.
- **Regions** (read only): `GET /api/regions`; `/api/regions/outlines?z=Z[&bbox=w,s,e,n][&ids=a,b][&union=1]`,
  every region's outline for drawing at zoom Z (0–12, deeper as 12): a GeoJSON feature per region,
  its every shape in one MultiPolygon simplified to half a pixel there (Douglas–Peucker,
  `buildstore::outlines::simplify`, 0.5 × 40,075 km ÷ (512 × 2^Z) m; rings smaller than that left out
  but its largest polygon), properties `region`, `name`, `disabled`, `bbox`, `label` (a point inside
  its largest polygon for its name) and `area` (its first osm: part's `iso`, `country`, `in`: its
  flag), and `z`; `bbox` keeps the regions whose box meets it, `ids` those named; `union=1` sends
  those named as one feature (a folder's outline, hovered in the panel), `region` their ids: their
  outer rings unioned at Z (z5 at most: deeper zooms draw z5's), a hole of one kept only where no
  other covers it, snapped to half the zoom's tolerance and closed by it (grown, then shrunk: the
  slivers between neighbours simplified apart filled), simplified to it again, a ring cut at the antimeridian put back on it (the map leaves stretches along ±180° out as cuts, as it does the `bbox`'s edges); made with GEOS
  (`regions.rs` `union_of`) and kept in memory for the last 32 asked. Made from the
  accepted copy's drawn files kept on the Mac (else the map's recorded coverage) once per zoom
  and version, by the regions thread for every zoom when the version changes, and kept on disk
  (`accepted/regions/@outlines/`) and in memory for the zooms asked: 0.9 MB for the world at z0, 4.7 MB at z4 (the map asks the view's box from z3), against 142 MB for every full outline.
  `/api/regions` answers from the listing kept on the Mac (`accepted/regions/@listed.json`), so at
  once; it follows the folder within a minute.
- **Names in answers:** MVT tiles and API JSON carry `main` and, when any, `sub`; JSON layer files,
  ferry blocks and marks tiles carry `main` only where it differs from the name; popup records are
  served as stored.

## RT road tiles, version 7

As v6 (`roadcore::tile`), but the way column holds OSM way ids, and lines are sorted by (draw
class, id) within a tile. The client sends the id with the clicked point.

## Other files (listed, not specified)

- **The OSM pass** (`sources/osm/<date>/`): `planet.osm.pbf`; `filtered`; `pieces/<u>` (the z6 tile extracts), `pieces.json`; `sets/<name>[-v<n>]`; `roads/<u>.bin` (32 B a way: u64 way id, its road values);
  `outlines` (sectioned, meta `fmt` "outlines-1" and the `repaired`, `stale`, `unassembled`
  relations; `recs` (64 B `OutlineRec`), `rings`, `points`, `srings`, `spoints`, `strings`);
  `regions-land` (the coastline's land by 2° cell: magic `RLAND001`, u32 head length, a JSON head
  `{fmt, pass, stats, cells: [[col, row, offset, length], …]}`, then each cell's rings as zigzag
  varint deltas of E7 points, docs/inputs.md §6.1.7);
  `reach.json.zst` (`{fmt, date, units: {"6/x/y" (`units`: the stored key): {owned: [w, s, e, n] | null, long: [{owned,
  ferry, verts}], cells: [[x, y]]}}}`, E7: the box of a z6 tile's ways within it + 20 km, every
  way reaching further, whole, and the 1° cells (west, south corners, degrees) its ways other than
  ferries start in; `roads::reach`); `pass.json`.
- **Global files:** `global/roads/<u>` (above); `global/roadids/<u>` (`ids`: the z6 tile's road ids,
  sorted u64, each once); `global/roadunits` (`pairs`: sorted u64 road, u64 z6 tile key; `srcs`: JSON
  `[ROADUNITS_V, [[z6 tile key, content name of what it read]]]`); `global/railfreq` (as `/api/railfreq`); `global/marks/summary` (`{fmt, kinds,
  tiers}`); `global/roaden/<u>` (`{OSM way id: English}` where `name:en` differs); `global/spoken`
  (`names::spoken::Spoken::to_bytes`); `global/heritage/*`.
- **Grid layers:** `grid-{class,canopy,cover}` hi packs of z11 tiles, encoding `u8-zstd`, not served.
- **Work files** (zstd JSON lines unless said): `work/pois/<u>`, `work/peaks/<u>`,
  `work/summits/<date>`, `work/trailends/<date>`; `work/heritage/<date>/{base/<stem>, pos/6-x-y.json,
  parts/6-x-y.json, areas/6-x-y.jsonl, <stem>}` (`parts`: World Heritage components among `pos`;
  `areas`: a heritage or designated area's polygon in each z6 tile its box meets); `work/marks/heritage-dots.json`; `work/asked/<step>.json.zst` (what a run of rail-feeds,
  rail, timetables-todo or bld-fetch asked of the coverage, `buildstore::asked::Record`, zstd'd JSON:
  `{"asked": {"points": [[x, y]…], "land": [[w, s, e, n]…], "all": […]}, "also": [[question,
  answer]…], "answers", "version", "line"}`, E7; docs/steps.md §12); `work/rail/used.json`
  (per feed: `id`, `provider`, `url`, `licence`, `fetched`, `status`, and when "ok" `day`, `trips`,
  `duplicates`, `rail_routes`); `work/rail/lines.json` (`{"lines": [{osm, kind, name, ref, operator,
  from, to, km, served_km, passenger}], "unmatched"}`); `work/ferries/lines.json` (`{"lines": [{…,
  passenger, ends}], "with_timetable", "unmatched"}`); `work/timetables/report.json` (`{to_do,
  to_do_km, by_kind, file, unmatched: [{mode, source, static, route, name, agency, osm, why}]}`);
  `work/trees-mid/6-x-y` (sectioned, meta `{fmt: 1, step: "trees", tile, v: TREES_V}`; per block,
  by column then row, `<layer>-8-<x>-<y>` for cover, height and leaf, then `tops-8-<x>-<y>`:
  "TREETOP1", u32 x, u32 y, zstd of cover and height (f32 × 65,536 each) and five leaf-type counts
  (u16 × 65,536 each)); `work/terrain-mid/6-x-y` and `work/terrain-deep-mid/10-x-y` (meta `{fmt: 3,
  step: "terrain", tile, v: TERRAIN_V}`; per tile three zooms down, `quad-<z+3>-<x>-<y>`: its 2×2
  means of its heights before rounding, f32 × 128 × 128: a z6 tile's for its assembly, a z10
  tile's for its z6 tile's terrain-hi run); `work/slope-mid/6-x-y` (meta `{fmt: 1, step: "slope", tile, v: SLOPE_V}`;
  `<z>-<x>-<y>`, each z6–8 tile as stored, `shade-webp`); `work/slope-deep-mid/10-x-y` (a slope-deep
  z10 tile's part of its z6 tile's pyramid, which that z6 tile's slope-hi run takes in: meta `{fmt:
  2, step: "slope-deep", tile, v: SLOPE_V}`; a section `13-<x>-<y>` for each of its z13 tiles (those
  its terrain deep pack's tiles lie in), in (x, y) order: its quadrant at full precision, 128 × 128
  pixels each its steepness and direction's two parts, f32 little-endian, row by row (196,608
  bytes), empty where it shades nothing); `work/lake-depth/<source id>/6-x-y` (a tar, ustar with fixed headers: its first member
  `index.json`, `[{file, zoom, c0, r0, w, h, at: [offset, length]}]`, each grid's zoom, first
  column and row on that zoom's global pixel grid, size, and its GeoTIFF's place in the tar; then
  the GeoTIFFs, in the depth grids' standard, docs/inputs.md §6.10: EPSG:3857, float32, NaN for
  none, Deflate with the floating-point predictor, overviews by averaging; one a source the
  lake-depth step makes and z6 tile with a lake of it; `terrain::depth::lakes`);
  `work/water-idx/<hash16 of the pin>` (JSON `{fmt: 1, step:
  "terrain-water", pin, digests: {target: [from, digest]}}`: per depth target, what decides its
  tiles and a hash16 over its water tiles' polygons; `codecs::terrain_water::WaterIdx`).
- **Other sources:**
  - `sources/items/<date>/{facts,views,meta}.json`; `answers.tar.zst` (agentcore::answers: the
    items job's caches `facts-<date>.jsonl`, `wp-<date>.jsonl`, `fetched-<date>.json`; tar + zstd,
    rewritten whole as a step starts and ends; one that doesn't read moved aside as
    `<name>.bad-<unix seconds>`) and `heritage.tar.zst` (the heritage chain's working copy, but what
    it makes again each run).
  - `sources/buildings/<release>/8/<x>-<y>.f32` (release with a dash: `2026-09-23-1`; raw f32
    `[xmin, ymin, xmax, ymax]` in degrees per Overture building whose box centre is in the z8 tile,
    sorted, duplicates once) and `index` (`{fmt, release, zoom, tiles: {"8/x/y": count}}`, written
    last).
  - `sources/trees/leaf/lat<top>_lon<left>.tif` (a 10° square's dominant leaf type at 0.0005°: u8,
    0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data; `trees::leaftype`, GDAL's GeoTIFF
    bytes; tag `complete=1` when whole; chunks kept in `parts/lat<top>_lon<left>/<row>-<col>.npy`
    (numpy's format) or `.none` while made; a NALCMS square's warp is GDAL's step for step with
    `det`'s maths, the same bytes on every Mac, and its `source` tag says so
    (`leaftype::NALCMS_SOURCE`): one tagged otherwise is made again);
    `sources/trees/nalcms-2020.tif` (size and CRC-32 checked).
  - Downloads kept so each is made once (temporary names end `.<host>.<pid>.tmp`):
    `sources/canopy/meta_chm_lat=<top>.0_lon=<left>.0_{median,p95,cover5m}.tif` (empty: Meta lacks
    it); `sources/aws-terrarium/packs/<shard>.<hash16>.tiles` (AWS's raw tiles in RDTILES1, shards
    `6-<x>-<y>` for zooms 9–12, `3-<x>-<y>` for 3–8, `root` for 0–2; an empty entry where AWS lacks
    the tile; meta `{"kind":"aws-terrarium raw tiles"}`) with `packs/index.json` (`{areas: {shard:
    [{name, bytes}, …]}, gone: {name: unix seconds}, merged: [name, …]}`, archives oldest first,
    read newest first; `gone` deleted a day after; `merged` those of `gone` merged away, never
    named again from a hand-off), and loose `sources/aws-terrarium/<z>/<x>/<y>.png` (`.none`);
    `sources/pageviews/<YYYY-MM>.tsv.zst` (a `#langs\t<lang>,…` line, then
    `<lang>|<Title>\t<views>` lines; landmarks::pageviews); `sources/fetched/<dataset>/<file>` (a
    declared elevation dataset's `store` files, deflate GeoTIFF, `.none` where missing;
    docs/inputs.md §6.8).
- **Rail sources** (`sources/rail/`, content-named, in the manifest, never swept; `inputs::rail`,
  docs/steps.md §10):
  - `catalogue.csv`: the Mobility Database catalogue as downloaded (docs/inputs.md §6.7.1);
  - `checked.json`: every catalogue feed checked for rail routes, by `id`: `id`, `provider`, `name`,
    `country`, `subdivision`, `url`, `licence`, `size_mb`, `rail_routes`, `examples`, `status` ("ok",
    a definite failure such as "http <code>" or "no routes.txt", or "no answer (…)", which is
    checked again, as is an "http" 429 or 5xx);
  - `gtfs/<feed id>.zip`: each feed as fetched;
  - `fetched.json`: `{zip content name: "YYYY-MM-DD"}`, the day its timetable counts from;
  - `feeds.json`: `{fmt: 1, feeds: [{id, provider, name?, country, url, licence, replaces?,
    rail_routes?, zip, fetched} or {…, status}]}`, the coverage's feeds in the rail counter's order;
  - `ferry-feeds.json`: likewise, `{id, name, url, source, ferry_routes, zip, fetched}` or `status`.
- **Stop pairs** (`landmarks::gtfs::railcount`'s, docs/inputs.md §6.7.2): 21-byte records, f32 lon_a, lat_a, lon_b,
  lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular; +0x80 a lower bound; +0x20 / +0x40 stop A /
  B beyond the coverage), f32 trains A → B on a typical weekday.
- **Kept for z6 tiles' later runs** (`cache/`, not swept): `cache/scenic-units/<u>/` (canopy.keys, canopy.tiles, view.keys,
  view.tiles, near.i8, roadside.u8, samples.metrics.u8, grid.canopy.u8.zst, grid.cover.u8.zst,
  basis.json `{v: SCENIC_V, basis: [[[x, y], hash16], …]}`).
- **Workers** (docs/coordinator.md):
  - Each Mac's agent's `lead.json` (`coordwire::lead::Known`, mode 600): `{member, host, urls:
    [Tailscale's, then the LAN name's], token, page}`, the lead's own coordinator's on the lead,
    a member's from its beats' answers (`reach`: `{urls, page}`; `page`: the build page over HTTPS
    when `tailscale serve` proxies it).
  - On the lead, `agent/coord/`: `workers-token` (32 hex digits, mode 600); `page`; `store/` (the
    pool's store, `agentcore::pool::store`, docs/coordination.md §4: `snapshot.json`, `{epoch, seq,
    docs}`, written whole, and segments `<first seq>.jsonl`, one line per change after it, `{seq,
    epoch?, set: {key: value}, del: [key]}`; its documents, `agentcore::coord::kept`: `leases`
    (`{next, holds, cancelled}`), `lease/<id>` (`{id, term (its epoch), granted_at, worker, work, job?
    (the run it's held again for, below, Job ids), progress, …}`), `cost/<key>` (`{peak_mb, secs, worker,
    job?, v, sizes}`: `job` the run that measured it), `floor/<key>`, `disk/<key>`, `job_disks`, `fills` and `pause`, each as the
    files below held them; `member/<host>`, `{speaks: ["held-leases"], at}`, a member as its
    last beat said, `agentcore::coord::held`; `beat/<member>` (its last heartbeat and when heard,
    `kept::Beat`); `inbox/<member>` (its mail not yet read); `hold/<member>` and `settings` (the
    owner's asks, `agentcore::coord::asks`); `handoff/<key>` (a hand-off taken, until a saved
    snapshot names it); `progress/<member>`, `answers-lease` and `budget-hold`
    (`agentcore::coord::budgets`): docs/coordination.md §3.1). Read once, into a store not made yet (an agent before the store's), and
    no longer written: `leases.json` (`{next, leases: [{id, worker, work, progress}]}`); `costs.json` (`{<cost key>: {peak_mb,
    secs, worker, v, sizes}}`, `sizes` as in `costs.jsonl`, absent before task #235; keys: a z6 tile, `"<step> <target>"`, or `"<kind> <tile>"` for tasks: `tail 6/x/y`,
    `bldtile 8/x/y`, `treeblock 8/x/y`, `terrainsub 8/x/y`); `floors.json` (`{<cost key>: {mb, alone,
    v}}`, crate::agent::memguard::Floor; `scenic pool floors` lists and clears them);
    `disks.json` (`{<cost key>: MB}`, rising only, docs/pool.md §7.7); `job-disks-v6.json`
    (`{<step>: [MB, …]}`, the disk its last 20 jobs took, oldest first; an older `job-disks-v5.json`
    (the same) read once whole, `pack`'s and `lo`'s targets' entries in `disks.json` dropped (task
    #657: those of before task #396, brought back by each take-up of the coordinator's state); else an older `job-disks-v4.json`
    (the same) read once without `pack`'s and `lo`'s runs, which counted their copies of whole base
    packs before task #396, their targets' entries in `disks.json` dropped with them; else an older
    `job-disks-v3.json`, `-v2` or `job-disks.json` (`{<step>: MB}`, the most) read once, each peak
    one run, without `pack`'s and `lo`'s, its halo steps' stale peaks nor those past three times
    their targets' 90th percentile and a GB, a step reading a shared cache's peak less its targets'
    90th percentile its cache's fill instead); `cache-fills.json` (`{<cache>: MB}`, "DEM": what
    filling it on a Mac takes, docs/pool.md §7.1; an entry of a cache that isn't one any more is
    dropped as it's read); the measures
    of z3 `terrain` and `trees` runs, an old step shape, dropped from `costs.json`, `floors.json`
    and `disks.json` as they're read); `journal/<worker>/` (hand-offs sent rather than journaled, drained into the
    member's hand-offs); `tasks/<id>/`; `pause.json` (`{pause: {mode: "drain" | "freeze", by, at} | null,
    at}`, `jobrt::control::Pause`); `costs.jsonl` (`SCENIC_COSTS`: `{unit, started}` (`unit`: the target, stored so) as a target
    begins, `{unit, peak_mb, secs, v, disk_mb, sizes}` as it ends; `sizes`, `buildstore::costin`, a
    map of the target's input measures, absent from lines before it: `bytes` and `threads` (the threads it was given) always, `local` (of `bytes`, those read from this Mac's own disk: a mapped or local file's, its whole local copies', task #551, and the part of a NAS file copied whole that this Mac's page cache held as the copy began, task #939) where any were, `cpu_ms` (its process's CPU time over the target, its programs' as each is waited for; a z6 tile's with its tail's steps run here after its other measures were taken) where it can be read, `paused_ms` (the time its process was stopped over the target (SIGSTOP), its tail's too) where it was, `waited_ms` (the wall time over the target in which its process waited on the NAS, the network, the disk or other work, store::waits by its wall clock, its tail's too) where its run was timed (task #896)
    (bytes: what the store's readers read for the target, plus a step's own inputs: a z6 tile's extract,
    pack's and lo's base packs, pois' extract); `km2` (the tile's size in km²) and `tiffs` (GeoTIFFs opened)
    for every tile target; a pyramids-fill z6 tile's `fetched` (the tiles it asked of GSI: what
    weighs its time, `needs::PER_WORK`); a z6 tile's `ways` and `vertices` kept, `piece_ways`, `dem_tiles`. A z6 tile's
    bytes, local and tiffs include its programs' own (each run with `SCENIC_NAS_COUNTS` naming a
    folder of its own it counts in, store::nasio, added to its runner's as it ends), its tail's steps run here
    after its other measures were taken among them; a tail a worker ran (settled remotely) adds
    nothing, nor do reads outside the store's readers, remote reads, or Python steps; pack's and
    lo's `units` and `ways`; peaks' `peaks`, `z12_tiles`; bldprep's `rows`, `buildings`, `parts`;
    bldtiles' `buildings`, `parts`; `wrote` and `creates`, the bytes and the count of the files
    the target's job made on the NAS through its `Out` (`put_file`, `store_bytes`; a file already
    there, reused, not counted), absent when it made none; `read_back`, the bytes uploads read
    back from the NAS to check them while it ran (store::nasio's `ReadBack`: NAS reads, not in
    `bytes`; an upload put off reads back beside the next target's work), always said, 0 too: a
    line without it is from before task #1142); `written`, `written-2`
    (`SCENIC_WRITTEN`, bytes added to caches, every 64 MB); `timings.jsonl`.
  - On a member, `agent/outbox/<lease>/`: a leased task's files, `costs.jsonl`, `timings.json`,
    `spec.json`, `task.json` (`scenic run-task`'s result), `result.json` (`{ok, done: [step,
    [[target, key], …]] | null, failed, interrupted, task, error}`), until the coordinator has them.
  - **Pausing** (`jobrt::control`), in each agent's folder: `pause-request.json` (`{pause, at}`,
    from the menu or `scenic pause`), `pause.json`, `control` (`run` or `drain`, a
    line, the file replaced whole; one an app before task #957 wrote unended is read the same;
    `SCENIC_CONTROL`, `control::Say`), `threads` (the threads the job is granted now, a number,
    the file replaced whole by `control::grant` at the job's start and each re-grant;
    `SCENIC_THREADS_FILE`, docs/pool.md §7.1, Threads re-granted), `done.txt` (`<step> <target>` lines, `SCENIC_DONE`). A job stopped at a safe
    point exits 75. Coordinator answers carry the pause (409 `{error, pause}`, beats `{ok, pause}`);
    `POST /api/pause {pause, at}`; `/work/fail {…, interrupted}` returns a lease unheld.
  - **Input check asks** (`inputs::inputs::Ask`, docs/inputs.md §4.5): `inputs-asks/<at, 20
    digits>-<pid or "menu">-….json`, `{unit, accept, all, unaccept, check, full, forget, by, at}`,
    from the menu and `scenic inputs check`; taken up at the agent's next
    loop. A member's agent tells the lead's coordinator (`POST /api/inputs`); the build page posts
    there directly (never `forget`).
  - **The lead's declaration** (`buildstore::records::lead::Declared`, re-exported as
    `agentcore::pool::lead`, docs/pool.md §6.1), on the NAS: `state/build/lead-member.json`,
    `{member, host, since}`, made once with create-new by the first Mac in a new pool; moved by
    the owner's edit alone. Its being there is the pool's having begun.
  - A hand-off (`buildstore::handoff`): `{changes: {logical: content name | null}, pending: {content
    name: SHA-256}, checked: [content name], done: [step, [[target, key], …]] | null, raw: [[shard,
    {name, bytes}], …]}`, named `<ns>-<pid>.json` in write order; `<folder>.merged` holds the last
    merged; `.bad` one set aside unparsed.
  - A member's status rides in its beat (docs/coordination.md §3.1); the lead's
    status (its agent's `status.json`) lists the members' as `helpers` and their heartbeats as `beats`.
- **The pool** (docs/pool.md):
  - `state/build/records.json` (`{term, seq, manifest,
    keys, pending, sizes, raw, reflected, rejected, last: {step: {target: lease}}}`; `term` the
    epoch it was saved in, its key kept for older apps; `sizes`: content name → bytes of the files
    the manifest names, as their jobs wrote them): the lead's newest snapshot, a copy of the one on
    its disk (`agent/pool/records.json`, the same bytes); readers read it alone (`Records::now`).
    `state/build/brief.json`: `{term, seq, manifest, keys}` of that snapshot, written after it,
    read by readers of those alone (`Brief::now`). `manifest.json`, `jobs.json`, `pending.json`
    and `sizes.json` are the records before the pool (a new pool's first take-up is made from
    them); no lead writes them. The terms' folders (`state/build/term/<E>/`) are neither written nor read, but their
    `forecast-input.json` (the backtest's; docs/coordination.md §9).
  - A job's hand-off, as a member keeps it (`agent/pool/saved.json`'s `mine.unwritten`, by key
    `<day>/<epoch>-<n>`) and posts it whole (`POST /pool/handoff {entry}`, kept in the lead's store
    as `handoff/<day>/<epoch>-<n>` until merged: docs/coordination.md §5.3): `{member, lease
    (its run's job id), step, handoff, at}` (`handoff.sizes`: the bytes of each file it wrote, by content name; none in
    older ones). Nothing goes to the NAS. The old journal, `state/journal/` (entries
    `<day>/<epoch>-<n>.json`), is written by no app and read only for the keys an older app's
    saved state still lists (`mine.entries`, read back once whole, `Mine::read_back`).
  - A member's heartbeat rides in its beat (`POST /pool/beat`, docs/coordination.md §5.1):
    `{member, host, app, beat, leads, addresses, reach, members, conds: {home, ac,
    battery, held}}` (with brokers on, `broker: {urls, https, pages}`; `mail_http` and `beat_http`
    always true, for a lead on the app before #1036).
  - Switches: none is a file. `placement` (§7.2: jobs get `SCENIC_OFFER_AHEAD=1`, task offers
    carry `ahead`), `nas-budget`, `nas-target`, and publishing's `hold-catalog` and `near-slope`
    (docs/building.md §2, The map updates' switches) are the lead's settings (`/pool/settings`).
  - A member's ask carries `disk_mb` (free disk less the owner's room target and its jobs'
    reservations; no fixed reserve).
  - A member's resume progress: `{member, worker, jobs: {"osm-pass <date>": {at, since, done,
    scratch}}}` (a resumable job's progress there, offered to it alone for 24 h), on the member's
    disk (`progress.json` in its agent's folder) and carried in its beats, the lead keeping
    `progress/<member>` (`{progress, released: [[job, since], …]}`) in its store.
  - The Wikimedia budget and the answers' lease are the lead's (`POST /pool/budget/take`,
    `/pool/lease/answers`; the lease as `answers-lease` in its store), nothing on the NAS.
  - The members' mail to the lead goes by HTTP (`POST /pool/mail {from, to, msgs}`): each
    sender's last 64 messages, `[[n, msg], …]`, `msg` one of `{"Cancel": …}`, `{"Retry": …}`; the
    lead sends none.
  - `state/coord/token`;
    `state/coord/history-archive/<day>.tar` (a ustar of `<day>/<member>.jsonl`: the history's
    events of a finished day, the lead's take-ups and step-downs as `kind: term`, packed from the
    lead's `agent/coord/history.jsonl` by GC; `state/coord/history/<day>/` the days an older app
    wrote, packed as they pass the week);
    `state/coord/forecasts/<day>.jsonl.zst` (the lead's forecasts, The forecasts' archive above;
    days older than 7 in `state/coord/forecasts-archive/<YYYY-MM>.tar`);
    `state/coord/forecast-inputs/<day>.jsonl.zst` (the lead's forecast inputs, The forecast
    inputs' archive above; days before the last 7 removed).
  - Leases carry `term` (the epoch they were granted in; header `x-pool-epoch` on the lead's answers) and `granted_at`; `/work/done {…, journaled: true}`; `/work/done` and
    `/work/fail` carry `floors`, `disks` (`[[cost key, MB], …]`) and `job_disk_mb`.
- **State:** the agent's `status.json` in its folder (its heartbeat, at least every two minutes; the lead's asked of its coordinator, `/pool/view`): conditions,
  the job, `parts`/`part`, progress, `paused`/`pausing`, what waits, `pause`, `publishing`
  (agent::publish::PublishLine: `why`, `tiles`, `units`, `regions`, `low`, `wait_s`), `published`
  (agent::Published: `last` `{n, at}`, `low`, `drawing` by Mac, `adopted` by Mac `{n, at}`), the
  forecast (`regions` with `map_at`; `publishes`: `{what: low | marks | overlays, at}`), the checklist (a line per stage: `id`, `what`, `steps`, `done`/`total`/`left`,
  `unit`, `items` (per step: `step`, `noun`, `done`, `total`, `left`), `next`, `note`,
  `shared`: docs/pipeline.md §4), `caches`
  (agent::room::Caches: `clearable`, `clearable_deep`, `each`, `why_not`, `cleared`, `cleared_deep`, `declined`, `room`),
  `pool`, `lead` (crate::agent::lead::View), `inputs`
  (inputs::inputs::view::InputView: per unit `version`, `state` ok | checking | held, `checked`,
  `held`, `findings`, `accepted`, `stale`). `state/build/{manifest,jobs,pending,summaries,pause}.json`
  (`jobs.json`: job keys by step, target → key: `terrain_hi`, `slope_hi` (read from `terrain` and
  `slope` too, their names before task #639), `terrain_deep`, `slope_deep` (z10 tiles, "10/x/y"),
  `terrain_lo`, `slope_lo`, `unit`,
  `pois`, `peaks`, `pack`, `lo` (z3 tiles, worldwide steps by name, input check jobs as
  `inputs/<unit>`), `trees`, `trees_lo`, the 3D buildings', `catalog`, `catalog_held`).
- **The input check** (docs/inputs.md §3, §4; a unit's name is `/`-separated components of
  `[a-z0-9_-]`; its records start with `@`): inputs folders under `inputs/` (docs/inputs.md §3: `regions`,
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
  (`agent/inputs-listing/<unit with / as +>.<key>.json`, `inputs::Planned`; written once per key, and kept while a job may read it: named by this plan, the
  last one, a job running on the lead or a lease of the step, the rest removed with each plan); the owner's acceptances
  `state/inputs/accepted/<unit>/<finding id>.json` (create-new, `{id, member, host, at, by,
  message}`).
