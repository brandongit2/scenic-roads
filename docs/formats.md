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

## Water (`layers/water/{root,lo,hi}`; pipeline::water)

Each pixel's exact share of water, z0–9, made per pass from the basemap's z14 water (docs/plan.md
§6, Water). Packs as the other layers', encoding `water-png`, blobs not gzip'd: each tile a 512 ×
512 8-bit grey-and-alpha PNG, grey the sea's share (OpenMapTiles' class `ocean`) and alpha the
inland water's (the rest of the basemap's `water` layer, tunnels left out), 255 whole. The bytes
keep what a pixel holds however little (`water::bytes`): grey 255
only where there's nothing but sea (anything else, grey at most 254), and grey + alpha 255 or more
only where there's no land (any land, at most 254); a share under half a byte rounds the other
way to keep that. Each coarser pixel holds what any of its four does. A z10 tile is
drawn from its 256 z14 tiles (`pipeline::watercov::Raster`: the area inside each pixel), each
coarser tile's pixel the mean of the four under it. A tile one value throughout isn't stored: its
stored ancestor's pixels over it say which.

Served at `/tiles/water/{z}/{x}/{y}` (z0–18, crates/server/src/water.rs): stored tiles decoded and
encoded again in the form asked for; deeper ones drawn from the z14 tiles under or over them as the
build draws them (or, when the basemap can't be read, the nearest stored zoom's scaled up, not to
be cached). `?c=<sea>,<lake>[,<land>]`
(hex; anything else a 400): an RGBA PNG, the water in those colours (mixed by the two shares;
the sea's where there's none) and alpha their sum (at most whole), or with the land's colour the
alpha whose blend over that land has the lightness of the two mixed as light (`alpha_for`); `?raw=1`: an RGB PNG, red the sea's share, green the inland water's.

## 3D buildings (pipeline::bld; docs/buildings3d.md §3.4)

**Normalized buildings** (`work/bld/6-<x>-<y>.<h>.sect`, RDSECT v1, content-named, in the build
manifest; a work file, not served): every building and building part of the pinned Overture
release whose centroid is in the z6 tile, underground ones left out, made by `bldprep`. Meta:
`{"fmt": 1, "tile": "6/x/y", "release", "buildings", "parts", "srcs", "classes", "subtypes",
"roofs", "ghsl": "R2023A", "read"}`: the counts; the strings the records' codes index (code 0
none, code k the list's k-th; each list sorted: Overture's height source datasets, classes,
subtypes and roof shapes); `read`, what bldprep.py read (`files`: [name, ETag, [row groups]],
`ghsl`: the GHSL tiles, `rows`).

| Section | Record | Notes |
|---|---|---|
| `index` | `(u64 z14 tile key, u64 offset, u32 len, u32 count)` | a block per z14 tile holding a centroid, sorted by key |
| `blocks` | zstd blocks (level 9) | each a z14 tile's records, sorted by Overture id (a UUID as a number), column by column, below |

A block, little-endian: `u32` records, polygons, rings and vertices; then the columns: centroid
(`[i32; 2]` E7, area-weighted as GEOS has it, from the WKB's doubles), footprint area (`f32` m²:
degrees² × a degree's metres² (6,371,008.8 m sphere) × cos(lat)), polygons per record (`u32`),
rings per polygon (`u32`, the first the exterior), vertices per ring (`u32`), the vertices
(`[i32; 2]` E7, each ring's first absolute, the rest as deltas; a ring's closing point left
out), `h` and `m` (`u16` dm: Overture's `height` and `min_height` as given, 0 none), `f` and `mf`
(`u8`: `num_floors` and `min_floor`, 0 none, 255 for 255 or more), class, subtype and roof shape
(`u8` codes), flags (`u8`: 1 a part, 2 a building whose parts are drawn: `has_parts`, and a part
naming it read that isn't underground and whose geometry reads, in the tile or 0.02° around it),
the height's dataset (`u8` into `srcs`:
the source whose property is `/properties/height`, else the footprint's), GHSL's ANBH at the
centroid (`u16` dm, 0 none) and the OSM id where OSM gave the footprint (`u64`: 1 << 62 a way,
2 << 62 a relation, or'ed with the id; 0 none).

**The sources' indexes** (dem/bldfetch.py writes them beside the downloads, not through the
manifest; `bld-fetch` runs it, the agent reads them to key `bldprep`: `pipeline::bld::sources`):
`sources/overture/<release>/buildings.json` (`{"fmt": 1, "release", "source", "terms", "coverage":
{"regions", "margin_km", "at"}, "files": {<name under the release's folder>: {"size", "etag",
"rows", "row_groups", "rows_near", "bbox", "checked"}}}`: the files here, each checked);
`footers.json.gz` (gzip'd JSON, every file of the release: `{"release/<release>/<name>": {"etag",
"size", "rows", "rgs": [[w, s, e, n, rows], …]}}`, each row group's box and rows from its footer's
statistics); `sources/ghsl/R2023A/index.json` (`{"fmt": 1, "product", "source", "terms", "tiles":
{<zip>: {"size", "bbox", "checked"}}}`). `bld-fetch` hands bldfetch.py the coverage as GeoJSON
(`--coverage`: a feature per outline, its rings as polygons in degrees, `buffer_m` its buffer, as
the rail feeds' have it), in the job's scratch folder.

**bldprep.py's stream** (stdout to `scenic-build bldprep`, not a file): `BLDP1\n`, then frames, each
`u8 kind, u32 header length, header JSON` (with `cols`: [[name, bytes], …]) and the columns' bytes
in that order: GHSL windows (3; all before any row group's, which sample them as they come), each
row group's buildings (1) and parts (2), the end (9). dem/bldprep.py's docstring has the columns;
it checks their names and types against what it reads.

**Tiles** `layers/buildings/hi/6-<x>-<y>` (RDPACK v1, encoding `mvt`, blobs gzip'd at level 6,
z12–14; no lo or root packs), made by `bldtiles` from the tile's and its 8 neighbours' normalized
files and the coverage: MVT 2.1, extent 4096, one layer `b`, a feature per building or part that
touches the coverage (its centroid or a vertex in it, with the 1 km buffer), whole in the tile of
its centroid (not clipped); a building or outline (not a part: parts aren't drawn flat) also
copied whole into each other tile at that zoom its exteriors reach (`o` 1: for the flat
footprints, which a fill cuts at its tile's edge; from up to 310 m beyond a z8 area's edge);
quantized to the tile's grid, at z12–13 simplified to one unit first; repeated points dropped,
rings of fewer than 3 points or no area dropped (an exterior with its holes); exteriors positive,
holes negative; no edge parallel to an axis beyond the extent (MapLibre takes one for a clip line
and draws no wall on it: such an edge gets points between its ends a unit off it, outside the
ring, at MapLibre's subdivision lines every 2,048 units and midway, and a slanted edge out there a
point at each such line; inside the ring where outside would cross the ring or its polygon's
others, else none). z14 every building and part, z13 those 20 m or more or
of 2,000 m² or more, z12 those 40 m or more. Properties (uint): `h` the top (dm), `m` the base (dm;
parts; left out when 0), `s` where the height comes from (0 measured, 1 floors, 2 Microsoft's
estimate, 3 neighbours, 4 GHSL, 5 size and kind), `f` floors (when `s` is 1), `c` the kind (0
unknown, 1 residential, 2 outbuilding, 3 commercial, 4 industrial, 5 religious, 6 civic, education
or medical, 7 agricultural, 8 transportation, 9 other), `k` (1 a part, 2 a building drawn by its
parts: the flat layer's only; left out when 0), `o` (1 a copy; left out otherwise); keys in that
order in every tile, values in order of first use. No feature ids, no names. Features sorted by their centroid's Morton code in the
tile (12 bits an axis), then id.

**A z8 area's task** (`pipeline::bld::task`, docs/buildings3d.md §3.6; a folder on the build Mac,
its files sent to the worker as `u/…`, not in the manifest): `6-<x>-<y>.sect`, of T's and its 8
neighbours' normalized files those with a block the area reads (its own and those within 620 m
around it), each a normalized file as above with the same meta and only those blocks, their bytes
as stored; `coverage.sect` (RDSECT v1), meta `{"fmt": 1, "shapes": [{"source", "country",
"buffer_m", "rings": [vertices a ring, …]}, …]}` and section `verts` (`[i32; 2]` E7 a vertex, the
rings' in order): the coverage's shapes whose buffered box meets the box of every point the area
asks about (its blocks' boxes, their records' centroids and vertices), whole, in the recipes'
order. The program `bldtile` writes there `area.tiles` (RDTILES, meta `{"layer": "buildings",
"area": "8/x/y", "encoding": "mvt"}`: the area's z12–14 tiles as the pack holds them, gzip'd) and
`area.json` (what it made: `bld::job::Summary`, as `bldtiles` logs a tile's).

**A row of tree cover blocks' task** (`pipeline::trees::task`, docs/plan.md §6 Trees; a folder on
the build Mac, its file sent to the worker as `u/…`, not in the manifest): `coverage.json`, the
piece's coverage as the trees program reads one (`{"shapes": [[ring, …], …]}`, a ring
`[[lon, lat], …]` in degrees) with only the rings whose box meets the row's box, a hundredth of a
degree around it (a shape left with none goes). The program `trees --blocks` writes there
`8-<x>-<y>/` for each of the row's blocks: `trees-cover.tiles`, `trees-height.tiles`,
`trees-leaf.tiles` (RDTILES, meta trees.py's, `trees::META`: the block's zoom 8–12 tiles, in the
order it made them) and `trees-tops.bin` (its zoom-8 values, `pyramid::Tops::to_bytes`), as `trees
--block` writes one block's.

## Names (docs/plan.md §7)

- **Translation lines** (`translations/**/*.jsonl`, not `todo/`): `{"n", "kind", "langs", "main",
  "sub", "via"}`: `kind` road, settlement or other, or a list; `langs` a language (ISO 639: its first
  subtag counts) or a list; `main` null or empty for the name; `sub` null for none (an `en` stands
  for a missing `sub`); `via` free text,
  "todo" or "skipped" leaving the line out. Lines without `kind` or `langs` (the area tables'
  `{"n", "main", "sub"}` and `{"n", "en"}`) are left out. The converted area tables are
  `translations/0-converted/<language's English name>.jsonl`, with `conversion-log.txt` (JSON lines:
  each disagreement settled).
- **Labels** (`layers/labels`, layer `l`): `n` name, `en` its own English (`name:en`, else its
  romanised name), `kana` its kana reading (when it has no English), `l` the languages OSM gives its
  name (comma-separated), `o` its OSM object (`n123`, `w123`, `r123`), `k` kind, `c` class, `mz`,
  `ms`, `s` (dem/labels.py). Labels made before `LABELS_V` 2 have `n`, `en`, `k`, `c`, `mz`, `ms`,
  `s`.
- **To translate** (`translations/todo/<language>.jsonl`, the `names-todo` job's, rewritten whole
  after each catalog): `{"n", "kind", "langs", "things", "example": {"osm" (n123, w123, r123) | "qid" | "label"
  (its row, labels before `LABELS_V` 2), "at": [lon, lat]}, "priority"}`, by priority; with `README.md` and `check.py`.
- **To describe** (`descriptions/todo/landmarks.jsonl`, `areas.jsonl`, likewise): `{"qid", "id",
  "name", "en", "kind", "designation", "at", "enwiki", "wiki", "register", "source", "fame"}`
  (landmarks) and `{"qid", "id", "name", "kind", "bbox", "area_km2", "enwiki", "fame"}` (areas), by
  fame; with `README.md`.

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
                        web/, fonts/   (app/current → the one in use); dem/.venv: the Python steps'
                        environment, which uv makes from dem/uv.lock the first time a step runs here
mirror/<content name>   what's downloaded (docs/plan.md §4, Mirror), by the same names as on the NAS
                        (.partial/: in progress)
mirror/.basemap/<hash16>/  the basemap's pieces (store::pieces), of the archive with that content
                        hash: lo.pmtiles (zooms 0–10), 6-<x>-<y>.pmtiles (a z6 tile's zooms 11–14),
                        each a PMTiles v3 archive (clustered, gzipped directories, the source's
                        metadata, its tiles as stored); <piece>.pmtiles.part while one is made;
                        sizes.json: {piece: bytes}, each piece's size, worked out from the archive's
                        directory
idx/<hash16>.idx        pack indexes (RDPKIDX1: header, meta, entries, XXH3 trailer), as they're read
catalog/<n>.json.zst    the last catalogs read
translations/  descriptions/   local copies of the NAS folders, compiled by the server
names/spoken-<content>.bin   the languages spoken where, made from the catalog's outlines of that
                        content name (its slashes as underscores), for a catalog without
                        `global/spoken` (same format: names::spoken::Spoken::to_bytes: "SPOKEN01",
                        u32 head length, head (the rules' version, then a line per region: its ISO
                        code, a tab, its languages comma-separated; region 0 none), u32 run count,
                        23,041 u32 row starts, runs of (u32 first column, u16 region))
regions.json            the last regions read; regions-queue/: region edits waiting for the NAS
downloads.json          what this Mac has downloaded (crates/server/src/downloads.rs; the Regions
                        panel): {fmt: 2, world: at or null, regions: [{id, name, at}], views: [{id,
                        name, outline: [[lon, lat], …] (the ground that was in view), at}]}, `at` in
                        seconds since 1970; written through downloads.json.tmp; one that doesn't
                        read is set aside as downloads.json.bad. A keep.json (fmt 1: {regions,
                        views}, what a Mac kept before downloads) is read once, made downloads.json
                        (with the World when it kept anything), and removed
map-page                the address to open the map on another device (docs/plan.md §4, Devices;
                        0600, rewritten when it changes: HTTPS where tailscale serve proxies the
                        server)
agent/                  status.json (the build Mac's; a helper writes helper.json, which that Mac's
                        server shows), state.json, round.json (the last round of publishing:
                        {began, regions, last, units: {logical: content name}, over},
                        agent::build::Round; the jobs of the one under way read its units,
                        SCENIC_UNITS_AS_OF=<path>#<began>), job.json, agent.lock, logs/, cache/,
                        pack-idx/; clear-request.json (the owner's ask to clear this Mac's build
                        caches, from its menu or `scenic clean`: {by, at}, agent::room::ClearRequest;
                        renamed clear-request.json.taken as the agent takes it up, removed once it's
                        answered in the agent's status); room-target.json (the owner's disk room
                        target on this Mac, from `scenic room` or its menu: {bytes, by, at},
                        agent::room::Target; the agent reads it each loop, and keeps that much free;
                        no file: off)
agent/cache/            dem-cache.* (the seed), chm10/ (canopy 10° files) and aws-terrarium/
                        (copies of the NAS's sources/canopy/; the raw tiles as fetched, until packed
                        onto the NAS, and aws-terrarium/packs/: copies of its archives, a job's own
                        new ones among them, each marked used when a job opens it), blobs/ (copies
                        of the records' files staging reads), base/ (the pack cache), sources-*/
                        and work-*/ (scenic-build's local copies of NAS files by their logical
                        names: the z8 terrain, the summits), heritage-merged-<date>-<cover>.osm.pbf
                        (the pass's filtered planet clipped to the cover); items/ (the items job's
                        answers for the pass, facts-, wp- and fetched-<date>, as the NAS keeps them
                        too, with kept-<date>.json: {archive, files: {name: hash16}}, the NAS's
                        archive they last matched and their hashes then; months/: copies of the
                        NAS's pageview indexes, marked used when read), rail/ (the trains' stop
                        pairs), registers-<id>/ (the registers' archives, extracted),
                        heritage-<date>-<id>/ (the pass's copy of the snapshot, which the heritage
                        scripts add their answers to; its .kept.json as items/'s), heritage-venv/
                        (the heritage scripts' old Python environment: their next run removes it)
                        and unit-stages.json (the unit stages' times here).
                        When a job starts with too little free, raw tiles waiting are packed onto
                        the NAS (not kept here), then chm10/, aws-terrarium/, blobs/ and
                        items/months/ lose files: chm10/'s, the archives', blobs/' and the months'
                        idle an hour first, then the rest least recently used first (loose raw
                        tiles a folder at a time; empty markers kept; a canopy file the NAS lacks
                        copied there first, or kept; a month the NAS lacks kept; what the queued
                        jobs read last, room::Hints), until the Mac has a sixth more
                        free than the job needs (the OSM pass: what it needs). Once the build is
                        done, the agent trims the same four by the same rules (the build Mac keeps
                        chm10/); on the owner's ask, it clears them, base/, the seed (while the NAS
                        has it whole), sources-*/, work-*/ and heritage-merged-* (docs/plan.md §4
                        and §8, agent::room). Never through a link, nor in the NAS's folder.
                        Each file is read and made through store::cachefile (dem/cachefile.py):
                        a job holds a shared flock on each file it uses, to its end; a file is
                        made as <name>.<pid>.<n>.tmp beside its name, locked from its making, and
                        given the name by a hard link only if the name is free (never renamed
                        over one); room-making deletes a file only under an exclusive flock taken
                        without waiting, jobs running or not.
agent/member            this Mac's member id in the pool (docs/pool.md §5): `m-<16 hex>`, then the
                        Mac's hardware UUID, a line each (crate::pool::member_id; made once)
agent/pool/             the pool's part of the agent (crate::agent::pool; only with the pool on):
                        saved.json (the driver's state, crate::pool::driver::Saved: {member, mine:
                        {entries: {key: term acknowledged or null}, unwritten: {key: entry}}, term,
                        led, made, unfinished?, stood_down?, passing?}, written whole after every
                        step that changed it; `saved.earlier.json`: one set aside, of an earlier time
                        the pool was on, the NAS having no terms), mail.json ({read: {member: n}, the last message taken
                        from each; sent: {member: [[n, msg], …]}, the last sent each; n}),
                        members.json (the members it knows, by id), jobs/<term>-<n>/ (a job's
                        folder under its lease: work.json {step, targets: [[target, key], …],
                        lease: "<term>-<n>"}, its saves as hand-offs, done.txt, costs.jsonl; removed
                        once a saved state holds its entry), lead.json (the owner's lead asks, crate::
                        agent::lead::Kept: {asked: {ask, by, at, since, state: refused | passed |
                        going | done | failed, said, to}, change: {at, said}, offered: [member,
                        since], auto_at}), notes.jsonl (the terms' history events a member kept for
                        its next process's coordinator, removed once noted there)
agent/shadow/           a shadow run beside the agent (`state/pool/shadow`; crate::agent::shadow):
                        member, and pool-shadow/ with saved.json, mail.json, members.json as pool/'s,
                        watch.json ({seq: the agent's history read up to, jobs: {id: the jobs under
                        way as seen}, started}) and shadow.jsonl (its log: a JSON line each, {t,
                        kind: start, observed, handed, event, sent, list, gate, compare, restarted,
                        …}); `scenic pool-shadow --home <dir>` keeps the same in <dir>/pool-shadow/
pool-<id>.lock          (in the app's folder) the lock of this Mac's member `<id>`: one process runs
                        it (crate::pool::MemberLock)
agent/pack-idx/         <hash16>.idx: the indexes of the terrain packs the build manifest names
                        (RDPKIDX1, as idx/), by each pack's content name, which the units' keys read
                        (agent::tiles): each read once from its pack on the NAS, and never stale.
                        Outside cache/, so no room-making, trim or clear empties it; one whose pack
                        the manifest no longer names goes once untouched for a fortnight.
```

`~/Library/Preferences/nsmb.conf` gets `[FISHANDCHIPS:PERSONAL]` and
`[FISHANDCHIPS.LOCAL:PERSONAL]`, both `soft=yes`.

## Server API (changes)

- Tiles: `/tiles/{roads,rails,terrain,slope,labels,water,base,buildings}/{z}/{x}/{y}`, `/tiles/trees/{var}/{z}/{x}/{y}`
  (`buildings`: the 3D buildings' MVT as stored, versioned `buildings.tiles` in `/api/meta`).
  `/api/meta` says `water` when the catalog has the water layer (`versions.water`: the drawing's version,
  its packs' and the basemap's, as deeper tiles are drawn from the basemap).
  Strong `ETag`: the stored blob's hash, plus for named tiles the versions of the languages spoken
  within them and of the spoken-languages raster (docs/plan.md §7);
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
  catalog that has none), `online`, `nas`, `held`, `app`, `agent`, `names` (`langs`: each language's version; `spoken`:
  the raster's source and version, or null; `warning` when only the area tables' lines are
  there; `waiting`, what names wait for before they're loaded),
  `v`, `marks`. The map's meta is `/api/meta`.
- `/api/names?n=<name>&at=lon,lat` repeated (each optionally with `en=<own English>`,
  `k=road|settlement|other`, `l=<OSM's languages, comma-separated>` before its `at`): `[{main,
  sub}]`; `/api/build` (the agent's status: this Mac's when it runs here, else the NAS's copy).
- Other devices (docs/plan.md §4, Devices): no key. A request from anywhere but this Mac, its LAN
  and the tailnet (through `tailscale serve` too, from those alone: never Tailscale Funnel's) is 403
  ("not from here"), as is one whose `Host` isn't the map's (a public name: "not this map's address")
  or whose `Origin` is another page's ("not from the map's page"). CORS answers only this Mac's own
  origins (localhost, `*.localhost`, a loopback address).
- Downloads (the Regions panel's Downloads on this Mac, `downloads.rs`): `GET /api/downloads`:
  `{mirror, online, slow (the build is running: copies keep to rate), rate (bytes a second),
  free, reserve, here (bytes downloaded), world: {bytes, here, unknown, on, at, state}, wanted:
  {bytes, here, more (the room they lack above the reserve), unknown}, copying: {what, bytes,
  have, slow} or null, last: {at, copied, copied_bytes, removed, removed_bytes, waiting,
  waiting_bytes, failed, pending, end} or null, regions: {id: {name, bytes, here, unknown, on,
  state}}, views: [{id, name, outline, at, bytes, here, unknown, state}]}`; `unknown` counts the
  basemap pieces not sized yet (their bytes not counted); a download's `state` one of done,
  copying, queued, room, away, missing (a region the catalog doesn't have); `PUT
  /api/downloads/world` `{on}` (off refused while a region or view is downloaded); `PUT
  /api/downloads/regions/{id}` `{on}` (on downloads the World too); `POST
  /api/downloads/views/size` `{outline}` → `{bytes, here (of them), with_world (the World's bytes
  still to copy when it isn't downloaded), need (all that would still be copied), room (the free
  space above the reserve), fits}`; `POST /api/downloads/views` `{outline, name?}` (named after the
  place search's most important place in it unless named) → `{id, name}`; `PUT
  /api/downloads/views/{id}` `{name}`; `DELETE /api/downloads/views/{id}`. A download that wouldn't
  fit is refused. Refusals are 400 `{error}`, the reason in words.
- The menu bar (`/api/build`): beside the build's status, `offline: {world, areas, bytes, here,
  nas}` (what's downloaded; null without a mirror).
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
  `name:en` isn't their name; the server's roads' own English); `global/spoken` (the `spoken` job's: the languages spoken where, as
  `names::spoken::Spoken::to_bytes`, the format of a Mac's `names/spoken-*.bin`); `global/heritage/*`;
  `global/legacy/*`
  (today's converted files; `road-en` is no longer read).
- **Grid layers:** `grid-{class,canopy,cover}` hi packs of z11 tiles, encoding `u8-zstd`, not served.
- **Worldwide z8 terrain:** `sources/terrain-z8-v3` (one RDPACK of every z8 tile, meta without scope
  or root) and `sources/terrain-z8-v3-max` (each tile's maximum, f32).
- **Work files** (zstd JSON lines unless said): `work/pois/<u>`, `work/peaks/<u>`,
  `work/summits/<date>`, `work/trailends/<date>`; `work/heritage/<date>/{base/<stem>,
  pos/6-x-y.json, areas/6-x-y.jsonl, <stem>}` (an area in each z6 tile its box meets, whole; one
  across the antimeridian, in those its parts' boxes meet); `work/marks/heritage-dots.json`;
  `work/rail/used.json` (railgtfs.py's report: per feed, `id`, `provider`, `url`, `licence`,
  `fetched`, and `status` with, when "ok", `day`, `trips`, `duplicates`, `rail_routes`);
  `work/trees-mid/6-x-y` (sectioned: a tree cover piece's mid, what its z3 tile's assembly reads;
  meta `{fmt: 1, step: "trees", tile: "6/x/y", v}`, `v` the layers' version, `TREES_V`; per block
  of the tile, by column then row, `<layer>-8-<x>-<y>` for each of cover, height and leaf whose
  zoom-8 WebP tile it made, then `tops-8-<x>-<y>`, its zoom-8 values: "TREETOP1", u32 x, u32 y, then
  zstd of cover and height, f32 × 65,536 each, and the five leaf-type counts, u16 × 65,536 each,
  little-endian).
- **Other sources:** `sources/items/<date>/{facts,views,meta}.json`; the answers Wikidata and
  Wikipedia gave for the pass (pipeline::answers; not content-named, rewritten whole as a step
  starts and ends, tar then zstd with its checksum; one that doesn't read whole moved aside as
  `<name>.bad-<unix seconds>`): `sources/items/<date>/answers.tar.zst`, the items job's cache
  files as dem/items.py keeps them (`facts-<date>.jsonl`, a JSON line per item asked:
  `{qid, …poidetails.py's record}`, or `{qid, sl: 0, missing: true}` for one QLever doesn't know;
  `wp-<date>.jsonl`, `{qid, n (its Wikipedia articles), arts: ["<lang>|<title>", …]}`;
  `fetched-<date>.json`, `{first, last}`: the days anything was fetched), and
  `sources/items/<date>/heritage-<id>.tar.zst`, the heritage chain's: the files of the pass's copy
  of the registers' snapshot `registers-<id>` (the first 12 hex digits of the hash16 of the
  snapshot's content name) that the snapshot lacks or has at another size or time, by their paths
  in it (heritagewd.py's `wd/ids.jsonl`, `wd/wp.jsonl`, `wd/enwiki-shortdesc.json`;
  areadetails.py's `areas-wikidata.json`; heritage.py's `special-wd-labels.json`; a register a
  script downloaded), but what the chain makes again each run (`osm/`, whsshapes.py's OSM extracts
  in `whs/`, `wd/items.jsonl`); `sources/registers/<name>.tar.zst`;
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
    the LAN name's, "http://…:8090"], token, page}`; there while its agent runs. `page`, when
    `tailscale serve` proxies the coordinator: the build page over HTTPS (`https://<its tailnet
    name>/work/`), which the menu bar's web view can load (it loads no plain HTTP but the LAN's).
  - On the build Mac, in the agent's folder, `coord/`: `workers-token` (the build's own key, 32 hex
    digits, mode 600: the Macs' agents use it; `token`, the one pages carried long ago, and
    `devices.json`, the devices pages once had to be accepted as, are removed as the agent starts),
    `page` (the build page's address: the status bar's "Copy the Build Page's Address"),
    `leases.json` (`{next, leases: [{id, worker, work: {Job: {step, targets: [[target, key], …]}},
    progress}]}`: the jobs' leases), `costs.json` (`{unit: {peak_mb, secs}}`, `"<step> <target>"` for
    another shared step's job, and `"<kind> <unit>"` for a task: `"tail 6/x/y"` a unit's last
    steps, `"bldtile 8/x/y"` a 3D buildings' z8 area, `"treeblock 8/x/y"` a row of tree cover blocks,
    by its first),
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
  - **The pool's lead asks** (`pipeline::control::LeadRequest`, docs/pool.md §6.3), in each Mac's
    agent's folder: `lead-request.json`, `{ask: {kind: "give", to: member id or host name} or
    {kind: "take", force, downgrade}, by, at}`, from its menu, `scenic lead` or the map's `POST
    /api/build/lead {to} | {take: true}` (never forced); taken up and removed by its agent at its
    next loop. The build page's go to its coordinator, `POST /work/lead {to} | {take: true}` (a
    page's, with no key; never forced), which its agent takes up the same way. `/api/build` says
    this Mac's `pool` (its agent's `PoolView`, fresh) and `lead_asked` (its ask not yet taken up).
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
- **The pool** (docs/pool.md; `state/pool/enabled`, an empty file, switches it on; `state/pool/shadow`
  runs it in the shadow, below). Its files are made only while it's on:
  - `state/build/terms/<E>.json`: term E, made once with create-new, `{term, member, host, app,
    since (unix seconds), how, from, seq (a handover's: the snapshot of term from to take up)}`;
    `state/build/lead.json`, a copy of the newest a lead took up (a hint, never the truth).
  - `state/build/term/1/first.json` (term 1's first snapshot, made from today's three files) and
    `state/build/term/<E>/records.json` (term E's lead's, written whole after each merge): `{term,
    seq, manifest, keys (as jobs.json), pending, raw: [[area, {name, bytes}], …], reflected: [entry
    keys], rejected: {key: why}, last: {step: {target: lease}}, horizon, handed (the coordinator's
    state, a handover's)}`. Term 1's saves also write today's `manifest.json`, `jobs.json` and
    `pending.json`; a later term's lead writes them after each of its saves.
  - `state/journal/<day>/<term>-<n>.json`: a job's entry, written whole by its member, `{member,
    lease: "<term>-<n>", step, handoff (a hand-off, as below), at}`, `<day>` its UTC day by its
    member's clock; term 0, from before the pool: a helper's outbox under its old lease, the rest
    numbered from 2⁶² up. `state/journal/rejected/<day>/<term>-<n>.why`: a refusal's why.
  - `state/pool/members/<id>.json`: a member's heartbeat, `{member, host, app, beat, leads,
    handing_to: {to, term, offer, since, stage: offered | settling | passed}, ready_for: {term,
    offer}, stood_down, addresses, members: [ids it knows], shadow, conds: {home, ac, battery,
    able}}`, what isn't so left out (`conds`: apps from phase 3's controls on; older ones ignore it).
  - `state/pool/auto-handover`: the owner's switch for the proactive offer to be taken by itself
    (`scenic lead auto on|off`; a line saying who turned it on), off while missing.
  - `state/pool/mail/<to>/<from>.json`: the messages `from` sent `to`, its last 64, `{msgs: [[n,
    msg], …]}`, `n` rising (the sender's clock in ms, and on), `msg` one of `{"Tell": [keys]}`,
    `{"Ack": {term, keys, horizon}}`, `{"Passed": term}`, `{"Leads": E}`, `{"HandTo": member}`.
  - `state/coord/term/<E>/state.json`: term E's coordinator, `{leases: {next, leases: [{id, term,
    granted_at, worker, work, progress}]}, costs, failed: [[worker, cost key, unix seconds, times]],
    pause, pause_at}`; `state/coord/token`: the workers' token, the pool's (copied to each lead's
    `coord/`); `state/coord/history/<day>/
    <member>.jsonl`: a member's history events (as `history.jsonl`'s), its own file: a lead's
    coordinator's, and the terms' events (`kind: term`, `note` in words), every member's as they come.
  - The coordinator's leases (`leases.json`) say their `term` and `granted_at` (unix seconds); a
    grant says its `term`; `/work/done {…, journaled: true}`: the hand-off is in the journal already.
  - `state/pool-off/<day>-<unix seconds>/`: the pool's files above, moved aside by `scenic pool off`
    once its agents left it, by their paths.
  - `state/pool-shadow/`: a shadow run's files, as the pool's above under it (its terms, records,
    journal, heartbeats, mail); it reads today's three files and `state/build/writer` from the real
    folder and writes nothing else.
- **State:** `state/status.json` (the agent's heartbeat, written on a change and at least every two
  minutes: conditions, the job, its parts (`parts`, `part`: the one it's on) and its progress, why
  it's frozen (`paused`) or stopping at its next safe point (`pausing`), what waits (one about a
  job: its `step`), the build's `pause` while it's paused, the checklist to the end: each step's
  `done`/`total` or `left`, its jobs left by name (`next`) and why it waits (`note`), and the Mac's
  build caches (`caches`, agent::room::Caches: `clearable`, the bytes a clear would free, and
  `each`, cache by cache, `[{cache, what, bytes, back}]`, how each comes back with about how long;
  `why_not`, why they can't be cleared now; `trimmed`, `cleared` and `declined`, the last trim
  after the build, the last clear done and the last ask declined, each `{at, asked (a clear's
  ask's at), by, freed: {cache: bytes}, left, why_not}`, the caches named canopy, terrain, blobs,
  months (the pageview months' indexes), base, dem, copies and heritage; `room`, the owner's disk
  room target: `{target: {bytes, by, at}, free (bytes), toward (the last freeing toward it, a
  Freed with its `target` and, when it freed toward a held job's room past it, that room as
  `goal`), short (why the disk is short of it and stays so)}`); with the pool on, `pool`:
  `{member, role: lead | member, gates: {term, leads, duties, settle, caught_up, fresh, listed_at},
  members, unacked, restart, lead}`, a helper's status too; `lead`, the pool as the controls show
  it, crate::agent::lead::View: `{at, term, lead: {term, member, host, app, since, how}, leading,
  members: [{member, host, app, beat, me, leads, state, out_of_touch, away, can_lead, why_not,
  conds}], takeover: {refused, force, downgrade}, no_lead, handing: {to, host, term, stage, since},
  offer: {to, host, why}, auto, asked, change}`);
  `state/build/{manifest,jobs,pending,summaries,pause}.json` (`jobs.json`: the job keys, by step,
  target → key: `terrain`, `slope` (z3 tiles), `unit`, `pois`, `peaks`, `pack` (z6 tiles), `lo`
  (z3 tiles, and the worldwide steps' under their names), `trees` (tree cover's pieces, z6 tiles),
  `trees_lo` (their assemblies, z3 tiles), `catalog`, `catalog_held`).
- **The app:** `app/current.json` and `previous.json`: `{version, files, sha256}`.
