# Our own MapLibre

Scenic Roads draws its map with this copy of MapLibre GL JS (BSD-3-Clause, `LICENSE.txt`), changed
wherever upstream holds the map back (task #153). Nothing here goes upstream.

- **Upstream:** github.com/maplibre/maplibre-gl-js, tag `v6.11.2`, its source tree without its
  history. Left out: `docs/`, `developer-guides/`, the changelog, and the render and query tests'
  cases and assets (`test/integration/render/tests/`, `test/integration/assets/`, 125 MB), which
  `tools/fetch-upstream-tests.sh` fetches from the same tag when needed.
- **Changes** are marked `Scenic Roads:` in the code, each in its own commit (`git log -- web/maplibre`).

## How the app builds it

The app compiles this source with its own code: there is no separate build, and no `dist/` is used.

- `web/vite.config.ts` aliases `maplibre-gl` to `src/index.ts` (and `maplibre-gl/…` to this folder:
  the worker is `src/source/worker.ts?worker&url`, the stylesheet `built/maplibre-gl.css`), in the
  dev server and the production build alike, and so in `tools/app/publish.sh`.
- The shaders are made from the `.glsl` files as they're imported (`build/shaders_plugin.ts`, used by
  the app's Vite, this package's Rolldown build and its Vitest configs); upstream wrote them out with
  `npm run generate-shaders`, which went stale when a shader was edited without it.
- The other generated files (`src/data/array_types.g.ts`, `src/style/**/*_properties.g.ts`,
  `src/util/unicode_properties.g.ts`) are committed, since making them needs this package's
  development dependencies. Run `npm run codegen` here after changing the style spec's properties or
  the struct arrays.
- The stylesheet is `built/maplibre-gl.css`, committed (its `svg-load()` needs postcss plugins):
  `npm run build-app-css` after changing `src/css/`.
- Its runtime dependencies are the app's own (`web/package.json`), pinned at the versions this
  package's lockfile names. `vite.config.ts` checks them at every build and resolves them from
  `web/node_modules` only (`dedupe`), never from this folder's own `node_modules` (its tests').
  The publish therefore needs nothing installed here: `npm ci` in `web/` is enough.
- Types: `npm run typecheck` in `web/` writes this package's declarations (`tsconfig.app.json`, into
  `types/`, ignored), checking its source under its own, looser settings (upstream's: not
  `strict`), then checks the app against them (`web/tsconfig.json` maps `maplibre-gl` there).

## Testing it

Its own tests need its development dependencies, from its lockfile: `npm ci` here (npm 11 skips
install scripts it hasn't been told to allow; Puppeteer's browser is then `npx puppeteer browsers
install chrome`, or an existing one in `~/.cache/puppeteer`).

- Unit tests: `npx vitest run --config vitest.config.unit.ts <paths>` (the whole suite:
  `npm run test-unit`).
- Render tests: `tools/fetch-upstream-tests.sh` (upstream's cases and assets, ignored), then
  `npm run build-dev` (the `dist/` the harness loads) and `npx vitest run
  test/integration/render/render.test.ts -t <filter>`. Our own cases live in
  `test/integration/render/tests/scenic/` and are committed (the harness's report templates too,
  and its `scenic-depth-column` custom layer, `render.test.ts`), over the upstream assets'
  `terrain-shading` DEM (the Wetterstein):
  - `line-z-offset/across-seams`: a densely sampled line 250 m above the ground (its source's
    `tolerance` 0) and a level one at a sea-referenced 1,300 m, across a z13 seam over high terrain:
    both whole (no stencil cuts; the build before the fix lost the level line and drew the other
    elsewhere);
  - `fill-z-offset/sea-surface`: a see-through level surface at a sea-referenced 1,500 m, the hills
    through it;
  - `fill-z-offset/globe-pole`: upstream's `fill-planet-pole` near the pole, the water lifted
    (50 km up, clear of the z-fighting below): its polar cap drawn, not clipped away;
  - `symbol-height-anchor/sea`: point labels at 1,300 m, `sea` (times the exaggeration) against
    `absolute`;
  - `custom-layer/drape-and-scene-depth`: the `drape` hook's stripes and a column testing
    `sceneDepth` itself (green in front of the terrain, red behind).

  The covers' culling after the DEM tiles load has no render case: in the harness's views the
  stale volumes didn't show (upstream's build drew the same image), as they need the DEM to arrive
  after the covers with the camera unclamped from the ground, as the app's is. Its unit tests
  (`globe_covering_tiles.test.ts`, failing without the fix) and the Argyll loch view hold it.
- The type check: `npm run typecheck` in `web/`; this package's own `npm run typecheck` also checks
  its tests.

## What we changed, and why

### Carried over from the app's bundle patches

Until task #153 the app patched MapLibre's minified bundle in `vite.config.ts`. Those patches are
now source changes:

- **Circles and symbols behind the terrain** (`circle.vertex.glsl`, `symbol_*.vertex.glsl`): their
  visibility depth is computed as the terrain depth texture was drawn (`projectTileFor3D`). On the
  globe `projectTileWithElevation` returns the clipping-plane depth instead, so in tilted, close
  views every point and label was "behind the terrain".
- **The slope tint's colours** (`color_relief.fragment.glsl`): the slope source (known by its
  encoding's base shift, 32768.5) holds four slopes a pixel; each is coloured and the four averaged
  in linear light.
- **The globe without float32 noise** (`_projection_globe.vertex.glsl`, `interpolateProjectionFor3D`):
  positions are the tile's flat projection plus the globe-minus-flat difference interpolated across
  the z14 cell, so the terrain's surface and the buildings' feet don't shimmer (task #115).
- **Walls on the terrain under each corner** (`fill_extrusion*.vertex.glsl`): a base of 0 is the
  ground under each corner less 2 m, never above the roof (upstream sank every base 10 m below the
  centroid's ground).
- **Fog on the extrusions** (`fill_extrusion.fragment.glsl`, `fill_extrusion_program.ts`,
  `draw_fill_extrusion.ts`): they take the terrain's fog, as the terrain sets it for the same tile.
- **A depth bias on request** (`draw_fill_extrusion.ts`): a fill-extrusion layer whose metadata
  sets `scenic:depth-bias` to [factor, units] (each > 0) is drawn (both its passes) with a polygon
  offset of −factor, −units, reset after. The app's highlights (the heritage tint, the hover) are buildings drawn again a few
  tenths of a metre larger; far off the depth couldn't tell them from the buildings under them and
  the two fought as the map moved. Its unit test: `draw_fill_extrusion.test.ts`.
- **Trilinear mipmaps on request** (`draw_raster.ts`): a raster layer whose metadata has
  `"scenic:mipmap": "linear"` is minified between its two nearest mipmap levels (the app's water).

### Tile covers over 3D terrain

- **Covers worked out again when the terrain's elevations arrive**
  (`globe_covering_tiles_details_provider.ts`, `bounding_volume_cache.ts`, `TileManager.demVersion`,
  `Terrain.demVersion`): the globe keeps each tile's bounding volume for as long as each frame asks
  for it, with the terrain's elevations as they were when it was made. One made before its DEM tile
  loaded lay at sea level, so tilted over steep ground the tiles on the slopes nearest the camera
  were culled and not asked for until the camera moved (lochs missing in the Highlands until a
  no-op `jumpTo`). The cache is now cleared whenever a DEM tile loads, the exaggeration
  changes or the terrain does (`map.setTerrain` makes a new `Terrain` for any source), compared as
  two numbers and a reference for each tile the cover traverses.
- **A tile's zoom by its distance over the terrain** (`covering_tiles.ts`): the zoom function
  measures a tile's distance from the camera as if the tile lay at the center's elevation (sea
  level on the app's globe, where its camera pivots), so with 3D terrain the tiles near the camera
  were up to two zooms too coarse: a Chamonix view that looks like z15 reports zoom 12.8, and its
  buildings' tiles were z12–13 (only towers drew). Where a tile's elevations are known, its zoom
  gains log2 of how much closer the middle of its elevation range is than the function assumed,
  capped at the map's highest zoom (a reparsed vector tile's layout, its overscaled zoom, raised by
  the gain to the source's maxzoom + 1 at most, or to upstream's own layout zoom where that's
  deeper, rounded as the source rounds its zoom, so a camera low over the ground doesn't lay its
  tiles out again at z19); ground below the center's elevation (the flat map pivots on the
  ground) leaves it as it was, never coarser than upstream's. The middle, not the top: the top
  over-fetched where a tile spans a valley and a ridge. Every source's cover, the terrain's own
  render-to-texture tiles (the drapes) included; below, what that costs them.
- **A tile's zoom by its depth, not its distance** (`covering_tiles.ts`, `tileDepthDistance2d`):
  what's drawn of a tile shrinks with its depth along the camera's forward vector, but the zoom
  function took its distance from the camera, so a tile at the screen's sides, farther than one at
  the same depth in the middle, got a coarser zoom: Tokyo at z13.5, 65°, z12–13 at the left and right
  edges against z14 in the middle (the buildings blurrier at the edges, task #102). The function is
  now given, for each tile, the horizontal distance at which a point of the tile's least depth (its
  corners' and its nearest point's) would lie under the line of sight, √(depth² − height²): for the
  tiles on the line of sight what it was, and never more than upstream's. On the globe the same
  from the tile's flat-map corners (the globe's cover measures its distances on the flat map too).
  Tiles ahead of the center on the line of sight are finer too (their depth is less than their
  distance), and a view straight down takes the center's zoom everywhere. The terrain's
  render-to-texture tiles too (below). Tested by
  `covering_tiles_depth.test.ts` (a tilted view's middle row one zoom from edge to edge; equal
  depths given equal distances); seven of upstream's cover lists in `covering_tiles.test.ts`
  changed with it (finer tiles at a pitched view's sides), and twelve render cases draw finer
  tiles at their sides or near the camera (their expected images below).
- **The drapes' tiles by elevation and by depth** (`terrain_tile_manager.ts`,
  `TerrainTileManager.terrainAwareZoom`, true by default; false gives upstream's zoom, a switch for
  comparing): the terrain's render-to-texture tiles, into which every draped layer (roads, water,
  fills, lines) is drawn, take both zooms above, so the drapes near the camera and at the screen's
  sides are as fine as what stands on them. Each tile is a 1024² RGBA8 texture with mipmaps
  (5.3 MB a texture a stack of draped layers) at most. The zoom over the terrain alone lifted the
  tiles near a low camera to the map's highest zoom, far finer than the screen's pixels (the camera
  400 m above the Chamonix valley, the map's zoom 11.6 on the globe's sea-level pivot: 734 tiles,
  585 of them z19, 3.9 GB). Two rules, by the screen's pixels, keep the sharpness and not the waste
  (`TerrainTileManager.footprintSizing`, true by default; false for comparing). A tile's footprint
  is the device pixels its edge would span seen face-on at its nearest point (its square between
  its elevations' minimum and maximum; `tileFootprintPx`): the most its texture could need. The
  terrain's gain stops splitting a tile once its footprint is within its full-size texture, never
  coarser than upstream's zoom (`covering_tiles.ts`, `texelParity`); and each tile's texture is the
  power of two at or above its footprint, from 128² to 1024² (`Tile.rttTargetSize`). A texture
  coarser than its tile now wants, or more than twice as fine, is drawn again as one at another
  zoom is (kept while moving, then a tile a frame); the pool hands out a texture of the size asked
  for first. No tile or byte budget, and no maxzoom. The textures held at rest (build Mac, windowed
  Chrome, 2880 × 1626 device pixels, `tools/bldcheck/drape.mjs --sizing 0|1`; upstream's zoom → the
  zoom over the terrain without the two rules → with them): the low Chamonix camera 25 → 734 → 110
  tiles (133 MB → 3.9 GB → 495 MB; 87 at 1024², 23 at 512²), Chamonix at 65° 17 → 42 → 40 (91 →
  224 → 213 MB), Annecy at 72° 26 → 49 → 47 (139 → 261 → 231 MB), Tokyo at 70° 25 → 31 → 31 (133 →
  165 → 165 MB). At these views almost every tile's footprint is over 512 device pixels, so the
  sizes cut little there; the parity stop is what removes the low camera's z19 tiles. Close-ups
  of the near drapes with and without the rules match. The median GPU time of a moving frame
  for the zoom over the terrain without the two rules (quiet gate, upstream's → this): Chamonix at
  65° 2.96 → 3.94 ms turning, 3.25 → 3.55 gliding; Annecy at 70° 1.84 → 2.28 and 2.16 → 2.29;
  Tokyo at 65° 7.88 → 8.00 and 8.10 → 8.68. With the two rules: not measured yet.
- **Elevation ranges of parts of a DEM tile** (`dem_data.ts`'s grids, `Terrain.getMinMaxElevation`):
  a tile deeper than its DEM tile took the whole DEM tile's range (a z14 tile in the Alps the peak
  5 km away); each DEM tile now keeps the minimum and maximum of a 32 × 32 grid and its coarser
  levels, made with its own min and max in the worker, and a tile's range is read from at most
  5 × 5 cells. Tighter culling volumes too.

### Custom layers: their place in the passes, and the frame's depth

Upstream gave a custom layer `prerender` (offscreen) and `render` (the main pass, at its place in
the layer order, a `"3d"` one depth-tested in hardware) and nothing else: no way onto the 3D terrain
but projecting with elevations itself, the terrain's depth only through internals
(`terrain._fboDepthTexture`), and the frame's depth not at all (the app's #102 field prototype
copied it). Now (`custom_style_layer.ts`, `draw_custom.ts`, `render_to_texture.ts`, `painter.ts`,
`scene_target.ts`):

| Pass | Hook | When, and into what |
|---|---|---|
| offscreen | `prerender(gl, args)` | first, any framebuffer of its own (as upstream) |
| the terrain's | `drape(gl, args)` | 3D terrain on: once per terrain tile drawn, into its texture, in the layer's order among the draped layers (fills, lines, rasters, hillshade…); kept between frames, drawn again when the tiles in view, the zoom or `drapeRevision` change |
| main | `render(gl, args)` | at its place in the order; for a layer with `drape`, right after the terrain carrying its drape (before the next layer) |

The arguments (`CustomRenderMethodInput`) gain:

- `framebuffer`: the one the call draws into (`null`: the canvas), to bind again after binding
  others;
- `depthRangeFor3D`: the depth range the 3D layers draw in;
- `terrainDepth` (`{texture, width, height, revision, unpack}`, null without terrain): the terrain's
  depth texture MapLibre already draws for the symbols (packed RGBA8), read in place, `width` and
  `height` its own size in whole pixels (the drawing buffer's over `devicePixelRatio`), `unpack` a GLSL function
  reading it;
- `sceneDepth` (`{texture, width, height}`), for a layer with `readsSceneDepth: true`: the frame's
  depth so far, a `DEPTH24_STENCIL8` texture (the terrain's mesh, the extrusions, the 3D layers
  before it), with `framebuffer` the frame's colour without the depth attached (sampling a texture
  attached to the framebuffer drawn into is an error in WebGL), so the layer tests depth in its
  shader (`gl_FragCoord.z` against `texelFetch(depth, ivec2(gl_FragCoord.xy), 0).r`).
- `drape`'s also `tileID`, `size` and `getDrapeMatrix(tile)` (a tile's 0..8192 coordinates to clip
  space in the texture; `tile.wrap` its world copy).

A new `drapeRevision` is read at the next frame, which only comes when something asks for one: set
it, then call `map.triggerRepaint()`.

No copies of the frame are made for a layer. While a visible layer has `readsSceneDepth`, the main
pass draws into a colour renderbuffer and a depth-stencil texture (`SceneTarget`) and the colour is
blitted to the canvas once at the end of the frame; while none has, the frame goes straight to the
canvas as upstream's, and the only cost is a check of the layers each frame. Not with a
multisampled canvas (`canvasContextAttributes.antialias: true`, read once from the context's
attributes): WebGL refuses a blit into it, so no target is made, `sceneDepth` is null and a warning
says why (MapLibre's default canvas isn't multisampled, nor is the app's).
`test/scenic/demo.html?case=custom` shows each hook.

**The #102 field (`web/src/bldfield.ts`) on these hooks.** It draws its boxes far to near into
MapLibre's frame in `render`, hardware depth-tested, and wants the frame's depth so far (`u_scene`),
which it copies. Moved over: `readsSceneDepth: true`; `u_scene` becomes `args.sceneDepth.texture`
(the same window depth: drawn in `args.depthRangeFor3D`), with no copy; its hardware depth test
(`gl.enable(gl.DEPTH_TEST)`, `LEQUAL`, polygon offset −2/−8) becomes a test in the fragment
shader, `gl_FragCoord.z - bias > scene` discarding (`args.framebuffer` has no depth attached);
anything it binds of its own is followed by binding `args.framebuffer` again. The terrain under its
tiles can come from `args.terrainDepth` where it now reads `terrain._fboDepthTexture`. Its copy of
the frame's colour (`bg`, for the land colour under the buildings) has no counterpart: sampling the
frame's colour while drawing into it is the same feedback loop, so that needs its own target or a
copy still.

### Lines, fills, rasters and labels lifted off the terrain

Upstream drapes every line and fill on the 3D terrain (drawn into the terrain's textures), so a
ferry route or a bridge follows the ground under it, and a lake's surface is wherever the DEM puts
it. Paint properties change that (`style_spec_extensions.ts`, added to the style spec's own
object, so validation and expressions know them; the API after Mapbox GL v3's `line-z-offset` and
`line-elevation-reference`):

| Property | Values | |
|---|---|---|
| `line-z-offset`, `fill-z-offset` | number, metres, data-driven (`zoom`, `feature`, `feature-state`), default 0 | the height |
| `line-elevation-reference`, `fill-elevation-reference` | `ground` (default), `sea` | what it's measured from |
| `raster-z-offset`, `raster-elevation-reference` | as above, the height one per layer | rasters, below |
| `symbol-height-anchor` | upstream's `ground`, `absolute`, and `sea` | point labels, below |

- `ground`: above the terrain under each vertex, in metres, not exaggerated (as an extrusion's
  height). With an offset of 0 the layer is draped exactly as upstream draws it.
- `sea`: above sea level, on the terrain's scale (times its exaggeration, as the terrain's own
  heights are): 0 is the sea's surface, a lake's level its surface. A ferry route or the basemap's
  water at the water's surface over a 3D sea floor is `sea` with the surface's elevation.
- With either other than the default, the layer isn't draped (`render_to_texture.ts`'s `drapes`)
  but drawn in the main pass in 3D (`is3D()`), at its elevation, with the terrain's perspective
  depth, depth-tested against the terrain and the 3D layers (`draw_line.ts`, `draw_fill.ts`, the
  `line*` and `fill*` vertex shaders' `elevationOf` in `_prelude.vertex.glsl`). An opaque elevated
  fill writes depth (it hides what lies below it); a see-through one doesn't (the terrain below
  shows through).
- A vertex takes the elevation of its own point: a long segment between two vertices is straight
  in 3D, so a `ground` line or fill follows the terrain only as finely as its vertices do (the globe
  subdivides fills; lines are as their data is). Each tile's clip adds vertices at its edges, which
  take the ground there, so a sparse `ground` line can step at tile seams; give it dense vertices,
  and a GeoJSON source `tolerance: 0`, or geojson-vt simplifies away the vertices of a stretch
  that's straight on the map (a line along a parallel keeps only its ends and tile-clip vertices).
  It can step too where neighbouring tiles read the ground from DEM tiles of different zooms (a
  z12 tile's from z11's, a z13's from z12's), as an extrusion's base does upstream.
  A `sea` surface is level, so that's exact for it.
- A lifted layer isn't clipped by the tiles' stencil masks (drawn flat at sea level, they'd cut it
  where it stands above another tile on screen): its fragment shaders discard what lies outside
  its tile (`v_tile_pos`), and its layer reports `isTileClipped()` false. The lifted code is a
  program variant (`painter.useProgram`'s `LIFTED` define, `render/lifted.ts`), so a layer that
  isn't lifted runs upstream's projection, clip and gamma unchanged; only an unused `z_offset`
  binder is added to the line and fill programs. On the globe a fill's polar caps, whose vertices
  lie beyond the tile's y, are kept inside the clip (`v_tile_pos` at the tile's edge).
- Where a retained parent tile and a loaded child overlap (while one loads), the parent's part
  under the child would draw again: lifted lines and fills take the overlap stencil upstream's
  rasters use (tiles drawn z-descending, each zoom a higher `GEQUAL` reference that `REPLACE`s), so
  the parent draws nowhere the child drew; a fill's outline shares its fill's references (with
  `fill-layer-opacity` too, inside its scratch pass). The stencil masks by the pixels the child
  drew, not by the child's tile: where the parent has features the child lacks (generalised away
  or dropped at the child's zoom), the parent's still show there until it's let go. A known gap.
  The drape pass and the layer-opacity scratch pass clear their own stencil and start their IDs at
  1, restoring the main framebuffer's counter after, so an overflow there can't bring a later lifted
  layer's references below values an earlier one wrote.
- Without 3D terrain the ground is at 0: both are a height above the map. On the globe without
  terrain the 3D layers are depth-tested against upstream's depth pre-pass of the sphere
  (`_renderTilesDepthBuffer`, for the far side), which is tessellated unlike a fill: a lifted
  surface at or near sea level z-fights with it at low zooms (speckled at zoom −0.5 even at
  1,000 m; clean at 1,000 m from about zoom 3). Not fixed.
- A source's tiles are culled with room for its lifted layers' heights (`TileManager`'s content
  elevation: the offsets, data-driven ones by their tiles' largest, times the exaggeration for
  `sea`), as for upstream's lifted symbols.
- Not in the TypeScript types: the style objects' types are the style-spec package's, so a layer
  using them is cast.
- Rasters: `raster-z-offset` (metres, one per layer: data-constant) and
  `raster-elevation-reference`, on the same terms (`raster_style_layer.ts`, `raster.vertex.glsl`,
  `draw_raster.ts`): the app's water raster (basemap.ts's `water`) as a level surface. A raster's
  tiles' meshes are coarse (a quad, subdivided on the globe), so `ground` follows the terrain only
  at their vertices; `sea` is exact. Where a parent and a child tile overlap, upstream's overlap
  stencil (which rasters always take) draws only the child.
- Point labels: upstream lifts them (`symbol-height-offset`, `symbol-height-anchor` `ground` or
  `absolute`, where `absolute` isn't exaggerated, so a label at a lake's level sank into exaggerated
  terrain); the anchor `sea` is added, its offset times the terrain's exaggeration, in the shaders
  (`u_height_scale`) and in placement and collision (`symbolHeightScale`).
- Line-placed labels (a ferry's name along its route) aren't lifted: upstream's
  `symbol-height-offset` requires point placement. Planned, not built: lifting the label line's
  vertices (`symbol/projection.ts`, which projects each glyph along the line through
  `getSymbolElevation` already) with the same offset and anchor, and the line labels' collision
  circles with them; until then a ferry's name lies draped on the ground under its route.
- `test/scenic/demo.html?case=lines`, `?case=water` (with the lake's name on its surface) and
  `?case=rasterwater` show them; the unit tests are in `style_spec_extensions.test.ts`.

**The 3D sea floor (#99) on them.** With a terrain whose DEM goes below 0 under the sea (the sea
floor), the basemap's water fill gets `fill-elevation-reference: sea` and `fill-z-offset` 0 (a
lake's surface: its level, a feature property, as `['get', …]`) and a `fill-opacity` below 1, so
the surface lies level at the sea's elevation with the floor showing through; the ferry routes get
`line-elevation-reference: sea` and `line-z-offset` 0 to lie on that surface. Where the catalog
has the water's own tiles the app draws the water as a raster (basemap.ts's `water` layer): it gets
`raster-elevation-reference: sea` and `raster-z-offset` 0 alike. A raster has one height, so lakes
above sea level would come from a second layer (or tiles) of their own, or stay draped on the floor
DEM, which holds their surfaces. Labels on the water: `symbol-height-anchor: sea`. Nothing else is
needed of this fork; the floor itself (the DEM below 0) is #99's.

### Known render-test differences

`test/integration/render/expected/` holds this fork's expected image where it draws differently on
purpose (`tools/fetch-upstream-tests.sh` puts them in place):

- `terrain/fill-extrusion`, `-vertical-gradient`, `-vertical-gradient-pattern`: the walls' feet on
  the terrain under each corner (the last also its wall pattern, the DEM tiles finer near the
  camera with the tile zoom over the terrain).
- The drapes' tiles by elevation and by depth: `terrain/default` and
  `terrain/fill-extrusion-multipolygon` (finer drapes and the terrain's mesh with them near the
  camera: edges move by a pixel), and our `symbol-height-anchor/sea` (a few far slopes).
- The tile zoom by depth (2026-10-14): `high-pitch/terrain-pitch95`, `terrain/fill-extrusion` (its
  nearest row of tiles z14, not z13), `field-of-view-number`, `fog-sky-blend-globe`, `heatmap`,
  `image-projective`, `occlusion-debug`, `rtt-mipmap-minification/pitch-70`,
  `symbol-height-anchor-absolute`, `symbol-height-anchor-ground`, `symbol-height-offset-building`,
  `tile-stiching`; and our `symbol-height-anchor/sea` (its DEM tiles finer at the side). Each run
  without the rule passed on the same build.

Run on this Mac on 2026-10-11 (the last full run under other agents' load, the failures it showed
run again quiet): every other upstream case passes. Under heavy load a handful of terrain cases
(`terrain/symbol*`, `skirts-*`, `tile-transparency-overlap-*`, `line-opacity/terrain`) fail with
differences of 0 pixels at threshold 0.1, or a blank frame, and pass when run again: timing, not
drawing.

On this Mac (Apple silicon, SwiftShader in headless Chrome) upstream's own build fails four render
tests, `text-local-glyphs/{cjk,missing,no-glyphs}` and `text-local-ideographs/cjk-symbols` (the
system's fonts); so does this fork.
