# Formats (implementation spec for docs/plan.md v6)

The contract between the build steps, the NAS, the server and the browser. All integers are
little-endian. Coordinates are i32 1e-7 degrees (`E7`) unless stated. A tile key is
`roadcore::archive::tile_key(z, x, y) = z << 58 | x << 29 | y`.

Bump a format's version on any incompatible change; readers accept the current and previous
version (plan §8, Format bumps).

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
type); a NAS file is read section by section.

## Units

A unit is a tile key; in phase 1 every unit is a z6 tile. A way's owner unit is the unit containing
its first vertex (deepest unit of the pass's unit set that contains it).

## Base pack (`base/<z>-<x>-<y>.<h>.sect`): what one unit owns

Meta: `{"fmt": 1, "unit": "z/x/y", "ways", "verts", "samples", "extent": [w, s, e, n] E7 of all
owned geometry, "source": "legacy:<build>" | "osm:<date>"}`.

| Section | Record | Notes |
|---|---|---|
| `ways` | `WayRec` (48 B, roadcore) | sorted by (z9 key of first vertex, Morton of first vertex); `vstart` is local; `name`, `ref_`, `surface`, `route` index `strings` |
| `verts` | `[i32; 2]` | densified geometry |
| `elev` | `i16` | processed elevation, decimetres (legacy `final.i16`) |
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

One section `roads`, one 24-byte record per way of the unit's base pack, in the same order:

```
u64 road id      the lowest OSM way id of the road
f32 road length  metres
f32 offset       metres from the road's start to this way's start, along the road's direction
u8  dir          0: the way runs with the road; 1: against it
[u8; 7] pad
```

One chaining (plan §6): at each node, way ends pair by mutual best continuation — same ref (any
shared token of a multi-ref), else same name, else same class when both are unnamed, else (level
0) an unnamed way continuing a named one of the same class within 35° (a bridge or a short link
without its own name); straightest first within 100° (35° at level 0); a oneway only in its
direction of travel. Pairs form paths and cycles; a path
walks from its end whose way has the lower id; a cycle starts at its lowest way id, in that way's
direction.

## Hi data (`hidata/<6>-<x>-<y>.<h>.sect`): per z6 pack tile T

| Section | Record | Notes |
|---|---|---|
| `here` | `Here` (40 B) | every way drawn in T's tiles, sorted by `id` |
| `ends` | `(u64 point, u32 here index, u32 pad)` | both end vertices of each `here` way, sorted by point (`(lon as u32) << 32 \| lat as u32`) |
| `parts` | `Part` (32 B) | query parts: one road's consecutive samples inside T |
| `psamples` | `PSample` (24 B) | the parts' samples, part by part |
| `pch` | `[u8; 13]` | per part sample |
| `climbs` | `Climb` (64 B) | climbs starting in T |
| `climbgeom` | `[i32; 2]` | their polylines |

```
Here    { u64 id; u64 owner (unit tile key); u32 index (in the owner's base pack); u8 class;
          u8 flags (WayRec flags); u8 extra (bit 0 unnamed, bit 1 rail); u8 pad;
          [i32; 4] bbox; }                                                       // 40 bytes
Part    { u64 road; f32 offset (of the first sample along the road); u32 first (psamples index);
          u32 count; f32 road_len (the whole road's length, m: the length filter); u8 class;
          u8 flags (bit 0 unpaved, bit 1 toll, bit 2 unnamed); [u8; 6] pad }    // 32 bytes
PSample { u32 way (here index); f32 offset (along the road, m); i32 lon; i32 lat; f32 eye;
          u8 flags (sflag); [u8; 3] pad }                                         // 24 bytes
Climb   { u64 way (OSM id at the start); u64 label (OSM id at the middle); f32 gain_m;
          f32 length_m; f32 start_elev; f32 top_elev; f32 max_grade; f32 road_len (of the
          middle way's road); [i32; 2] mid; u32 geom_start; u32 geom_count; u8 class;
          u8 unpaved; u8 flags (middle way: bit 0 toll, bit 1 unnamed); [u8; 5] pad } // 64 bytes

The record types are `roadcore::packs`.
```

## Catalog (`catalog/<n>.json.zst`)

zstd with its content checksum on; written as `<n>.json.zst.tmp`, then renamed. Readers list
`catalog/`, take the highest `<n>` that decodes and parses, and fall back to the next.

```json
{
  "fmt": 1, "n": 7, "created": "2026-10-03T04:05:06Z", "app": "<app version that published>",
  "files": {"<logical>": {"file": "<content name>", "size": 123, "fmt": 1}},
  "units": ["6/32/21"],
  "layers": {
    "roads": {"encoding": "rt7", "minzoom": 4, "maxzoom": 14, "root": "<logical>",
              "lo": {"3/4/2": "<logical>"}, "hi": {"6/32/21": "<logical>"}}
  },
  "basemap": ["<logical of a .pmtiles>"],
  "base": {"6/32/21": "<logical>"},
  "roads": {"6/32/21": "<logical>"},
  "hidata": {"6/32/21": "<logical>"},
  "global": {"pois.json": "<logical>"},
  "meta": {"…": "the app's meta (bounds, elevation histogram, dem counts …)"},
  "credits": [],
  "coverage": {"regions": [], "outline": "<logical of coverage GeoJSON>"}
}
```

`files` holds every file the catalog references; GC keeps exactly these (plus 14 days of history).

## On each Mac (`~/Library/Application Support/scenic/`)

```
bin/scenic-launcher     the login item (never rebuilt)
run/<name>              what the launcher runs (one argument per line)
app/<version>/          server, web/, fonts/   (app/current → the one in use)
mirror/<content name>   local copies, by the same names as on the NAS
idx/<hash16>.idx        pack indexes
catalog/<n>.json.zst    the last catalogs read
queue/                  region edits waiting for the NAS
```

`~/Library/Preferences/nsmb.conf` gets `[FISHANDCHIPS:PERSONAL]` with `soft=yes`.

## Server API (changes)

- Tiles: `/tiles/{roads,rails,terrain,slope,labels,base}/{z}/{x}/{y}`, `/tiles/trees/{var}/{z}/{x}/{y}`.
  Strong `ETag` = blob hash (plus translations versions for named tiles); `Cache-Control:
  no-cache` so the browser revalidates.
- Ways: `/api/way/{id}?at=lon,lat`, `/api/profile/{id}?at=lon,lat`, `/api/road/{id}?at=lon,lat`.
  `at` picks the hi pack whose `here` holds the way.
- `/api/railfreq`: sorted `(u32 way id, f32 trains a day)` pairs.
- `/api/catalog`: the catalog's `n`, layers' zoom ranges, meta, credits, coverage, the NAS status,
  and translation versions per area.
- Names: every response carrying a name carries `main` and, when there is one, `sub`.

## RT road tiles, version 7

As v6 (`roadcore::tile`), but the way column holds OSM way ids, and lines are sorted by (draw
class, id) within a tile. The client sends the id with the clicked point.
