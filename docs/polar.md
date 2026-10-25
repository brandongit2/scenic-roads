# The polar caps (planned)

**Status: planned (task #601).** The map has no data past 85.05° N/S, where the Web Mercator
tiles end. This is the plan for a polar layer. Built so far:

- the projection and tiling (`roadcore::polar`);
- a tool, `scenic-polar` (`pipeline::polar`), that makes a cap's terrain and shading tiles from the
  elevation pool into a folder (never the NAS);
- the renderer: a custom layer (`web/src/polar.ts`); in the fork, `Map#setPoleCaps` and the
  globe camera reaching 89.5° where the map draws the pole; and a check page
  (`web/test/polar.html`).

No step, pack, catalog entry or server route exists yet, and the app doesn't draw the layer (§7).

## 1. Projection and tiling

- **Projection:** polar stereographic on WGS 84, variant B. The north uses EPSG:3413 (standard
  parallel 70° N, central meridian 45° W), the south EPSG:3031 (71° S, 0°). There is no false
  origin: the pole is at (0, 0). `roadcore::polar::forward`/`inverse` use Snyder's formulas
  (checked against the EPSG guidance note 7-2 example to the centimetre); `web/src/polar.ts` has
  the same inverse.
- **Tiles:** a quadtree over the square ±2^20 m around each pole (`HALF`). Tiles are 256 px, and
  (z, x, y) counts columns from −x and rows from +y, like the Mercator tiles. z0 is 8,192 m a
  pixel; z9 is 16 m (`pixel_m`).
- **Finest zoom: z9.** That is 16 m on the grid, about 16.5 m on the ground (the scale is 0.97 at
  the poles). The Mercator terrain's own rule gives the same: pixels no finer than half of
  GLO-30's 30 m cell.
- **Cap:** the tiles meeting the disc out to 85.0° (`CAP_LAT`, `cap_tiles`). That is a ring about
  5 km past the seam, for Horn's border. A parent meets the disc whenever a child does, so each
  zoom's tiles are the parents of the next zoom's.

## 2. Data

- **Terrain:** made from the elevation pool, like the Mercator base (`terrain_base`): the same
  datasets in the same order, blended alike. Each pixel is the mean of 2 × 2 samples. GeoTIFFs are
  read at the overview nearest half a pixel. The Terrarium tile pyramid is left out, since a Web
  Mercator pyramid has nothing past 85.05°. Past the caps, GLO-30 is the only dataset: 360 one-degree
  files a row in the south, about 1.2 GB a row, 6 GB for 85–90° S. It has nothing north of 85° N,
  which is all sea. Where nothing has data the height is sea level, and below zero is clamped to 0
  (the Mercator rule). The terrain is stored as Terrarium in lossless WebP (`terrain_codec`),
  rounded to 1 cm at z9 and 10 cm coarser.
- **Coarser levels:** each is the 2 × 2 means of its children. Where a child is missing (past the
  cap), the level is sampled itself.
- **Shading:** one precomputed pyramid, as `slope_pack` makes it. At z9 it uses Horn's method on
  the grid, the pixel's ground size being the grid size ÷ the scale at its latitude. Above z9 each
  pixel is the plain mean of its four children. Stored as `shade-webp`. Its directions are the
  grid's (east = +x, south = −y), and the renderer turns them to the ground's by longitude
  (`polar::east_north`). Nothing is computed from heights at render time.
- **Water:** no water mask past 85°. The Arctic is sea level, drawn in the water colour. No lake or
  ice-shelf flattening; Antarctica's ice shelves keep GLO-30's surface.
- **Not yet planned in detail:**
  - depth (IBCAO and IBCSO for the Arctic floor);
  - roads, labels and landmarks past 85° (the South Pole station, the South Pole Traverse). The OSM
    pass cuts by Mercator z6 tiles, so it drops them. That change is to an existing step (§8,
    decision 3).

## 3. Steps and packs (planned)

These follow the terrain's split (docs/steps.md §2). Each is a new step, so no existing step's
version or output changes.

| Step | Target | Makes |
|---|---|---|
| `polar-terrain` | a z4 polar block meeting the coverage | its subtree, z4–z9: pack `layers/polar-terrain/<pole>/4-x-y` |
| `polar-slope` | the same block, after its terrain | its shading pack, `layers/polar-slope/<pole>/4-x-y` |
| `polar-root` | each pole, once its blocks are done | z0–z3 of both layers, from the blocks' z4 tiles |

- **Blocks:** z4 tiles are 131 km a side. 76 of them make up the south cap (§6).
- **Keys:** a block's key holds the datasets meeting its box (`dem::declared::meeting`, as a z6
  tile's), the codecs' versions and the coverage fingerprint over its box.
- **Memory:** `scenic-polar` already makes the cap block by block. A z9 block holds 1,024 tiles of
  heights (256 MB) and shading (768 MB).
- **Coverage:** the regions are lon/lat outlines, so a block is a target when the coverage meets
  its box's footprint (the annular sector between its nearest and farthest latitudes). The
  Antarctica region (#432) reaches 90° S and would make every south block. The north has no land
  and is built only for depth.
- **Packs and catalog:**
  - packs are RDPACK, scope `polar`, root `z/x/y` with the pole in the meta;
  - the catalog adds a `polar` entry per layer and pole (blocks, versions, max zoom);
  - the server serves `/tiles/polar-terrain/<pole>/{z}/{x}/{y}` and `/tiles/polar-slope/…`
    (`image/webp`, `?v=` as other packs);
  - offline, a region downloads the blocks meeting it, like its z6 packs, with the root packs
    always.

## 4. Renderer

- **Globe only.** MapLibre's globe works in Mercator tile coordinates, and the flat map ends at
  85.05°. The fork has no polar projection to add tiles to, so the caps are a custom layer
  (`polarLayer`, renderingMode `3d`):
  - each tile is a 32 × 32 mesh (plus a 400 m skirt) of unit-sphere points, computed in float64
    on the CPU;
  - the vertex shader lifts the mesh by its terrain texture (Terrarium, bilinear between pixel
    centres), times the terrain's exaggeration;
  - the fragment shader shades from the shading texture with the hillshade layer's `combined`
    method and paint, over the land or water colour.
- **Tile choice:** a quadtree from z0. A tile is split while its pixels look larger than about 1.5
  screen pixels. Tiles past the horizon (the globe's clipping plane) or out of the frustum are
  dropped. While a tile loads, its loaded ancestor is drawn.
- **Fork, built:** `Map#setPoleCaps({north, south})` (`geo/projection/pole_caps.ts`). It turns off
  upstream's pole caps on the globe, which are the terrain mesh's and the rasters' fans drawn at
  the last row's height and colour. The polar layer turns its pole's caps off while it's on the map.
- **Fork, built:** the globe's camera centre may go to ±89.5° at a pole whose cap is off
  (`pole_caps.ts` `clampGlobeLatitude`, used by the vertical-perspective transform's constrain
  and the globe's pan and zoom helpers). Elsewhere it keeps upstream's ±85.05°.
- **Fork, planned:**
  1. The camera's ground height past 85.05°. MapLibre reads the terrain there as 0, so a close
     camera over the 2,800 m plateau is under the ice and draws nothing. The polar layer has to
     give MapLibre its heights.
  2. Keep the globe above 80° at every zoom. The app hands over to flat Mercator at z15.5–16.5,
     where the caps can't be drawn.
  3. Vector fills' pole triangles (fill buckets' subdivision) still close the poles. They need the
     same switch.
- **Float32:** sphere points are good to about 0.4 m, and the globe's matrix adds 1–2 m (FORK.md,
  "globe without float32 noise"). That's fine at 16 m pixels. A z12+ cap would need the fork's
  lattice trick.

## 5. The seam at 85.05°

- The cap's tiles reach 85.0°. Its fragments nearer the equator than 85.051129° (`|y| < sin
  85.05°` on the unit sphere) are drawn a little deeper (`RING_DEPTH` in window depth), so the
  Mercator terrain wins wherever it is. The ring fills the cracks where it isn't, and with
  MapLibre's caps off nothing else draws there. (Discarding the ring instead left a visible crack
  between the two meshes.)
- Both sides' heights come from GLO-30 by the same sampler, so they agree to within their
  pixels' interpolation. The Mercator side is z10–11 there, 13–26 m; the cap is 16 m.
- The cap's tiles meet one another over 400 m skirts.
- The colours match because both use the land colour, the water colour and the same hillshade
  paint.
- **Known differences:**
  - the Mercator side has the basemap's layers (water mask, landcover, labels) and the cap doesn't;
  - the cap's shading is computed on its own grid, so a slope crossing the seam can step by a few
    percent;
  - with 3D terrain off, the cap is drawn flat at sea level, like the rest.

## 6. Sizes and costs

Measured with `scenic-polar` (release) on the M4: 8 threads, reading the NAS during the live
rebuild, 25 Oct 2026.

| Run | Tiles | Terrain | Shading | Time | Peak memory |
|---|---|---|---|---|---|
| South cap, z0–z4 (whole) | 117 | 6.1 MB | 8.2 MB | 17.6 min | — |
| Five z4 blocks, z4–z9 (the pole's four, and 8/11 on the Transantarctic Mountains) | 6,825 | 445 MB | 707 MB | 25.8 min | 3.4 GB |

At z9 a tile averages 71 KB of terrain and 102 KB of shading. The shading is the larger:
GLO-30 is a surface model, and over the ice its speckle fills the direction bits.

**The south cap's tiles** (`cap_tiles`):

| Zoom | z4 | z5 | z6 | z7 | z8 | z9 |
|---|---|---|---|---|---|---|
| Tiles | 76 | 256 | 936 | 3,592 | 14,100 | 55,856 |

Scaled from the sample:
- **To z9:** about 9.7 GB at z9, 2.2 GB at z8 and 0.7 GB above, so about **12.6 GB** for the
  south cap. It takes about 4.7 h of the M4 (55 blocks' worth of z9 at 5.2 min a block) and reads
  about 6 GB of GLO-30.
- **To z8:** about 2.9 GB in about 1.5 h.

**The north cap** has no land: every height is sea level. Its tiles compress to almost nothing,
and it is worth building only once depth is planned.

**Seen in the sample** (§8, decision 4):
- GLO-30 leaves voids near the pole, where its fill mask defers to a next dataset that isn't
  there. They come out at sea level: pits 2,800 m deep, drawn in the water colour.
- Its one-degree files show as faint radial lines and a ring at 89° S in the shading.

## 7. What remains

1. The steps, packs, catalog, server routes and offline downloads (§3).
2. The fork's camera height, projection and fill changes (§4, planned 1–3).
3. The app wiring: add the layer when the catalog lists the polar layers, and follow the
   hillshade paint and layer toggles.
4. Depth for the Arctic floor. Roads, labels and landmarks past 85°.
5. Safari: checked only in headless Chrome so far.

## 8. Decisions

1. **Finest zoom:** z9 (16 m, GLO-30's rule, about 12.6 GB south) or z8 (32 m, about 2.9 GB).
2. **Globe-only caps:** the flat map keeps ending at 85.05°.
3. **Roads, labels and landmarks past 85°:** they need the OSM pass to keep nodes past 85.05°.
   That is an existing step's change, after the rebuild.
4. **A second dataset for Antarctica,** to fill GLO-30's voids: REMA, 32 m mosaic, EPSG:3031
   COGs, ranked after GLO-30 in the pool so the roads read it too.
