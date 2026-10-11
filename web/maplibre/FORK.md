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
  `test/integration/render/tests/scenic/` and are committed.
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
- **Trilinear mipmaps on request** (`draw_raster.ts`): a raster layer whose metadata has
  `"scenic:mipmap": "linear"` is minified between its two nearest mipmap levels (the app's water).

### Tile covers over 3D terrain

- **Covers worked out again when the terrain's elevations arrive**
  (`globe_covering_tiles_details_provider.ts`, `bounding_volume_cache.ts`, `TileManager.demVersion`,
  `Terrain.demVersion`): the globe keeps each tile's bounding volume for as long as each frame asks
  for it, with the terrain's elevations as they were when it was made. One made before its DEM tile
  loaded lay at sea level, so tilted over steep ground the tiles on the slopes nearest the camera
  were culled and not asked for until the camera moved (lochs missing in the Highlands until a
  no-op `jumpTo`). The cache is now cleared whenever a DEM tile loads.
- **A tile's zoom by its distance over the terrain** (`covering_tiles.ts`): the zoom function
  measures a tile's distance from the camera as if the tile lay at the center's elevation (sea
  level on the app's globe, where its camera pivots), so with 3D terrain the tiles near the camera
  were up to two zooms too coarse: a Chamonix view that looks like z15 reports zoom 12.8, and its
  buildings' tiles were z12–13 (only towers drew). Where a tile's elevations are known, its zoom
  gains log2 of how much closer the middle of its elevation range is than the function assumed,
  capped at the map's highest zoom; ground below the center's elevation (the flat map pivots on the
  ground) leaves it as it was, never coarser than upstream's. The middle, not the top: the top
  over-fetched where a tile spans a valley and a ridge. Every source's cover, the terrain's own
  render-to-texture tiles included.
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
  depth texture MapLibre already draws for the symbols (packed RGBA8, CSS pixels), read in place,
  `unpack` a GLSL function reading it;
- `sceneDepth` (`{texture, width, height}`), for a layer with `readsSceneDepth: true`: the frame's
  depth so far, a `DEPTH24_STENCIL8` texture (the terrain's mesh, the extrusions, the 3D layers
  before it), with `framebuffer` the frame's colour without the depth attached (sampling a texture
  attached to the framebuffer drawn into is an error in WebGL), so the layer tests depth in its
  shader (`gl_FragCoord.z` against `texelFetch(depth, ivec2(gl_FragCoord.xy), 0).r`).
- `drape`'s also `tileID`, `size` and `getDrapeMatrix(tile)` (a tile's 0..8192 coordinates to clip
  space in the texture).

No copies of the frame are made for a layer. While a visible layer has `readsSceneDepth`, the main
pass draws into a colour renderbuffer and a depth-stencil texture (`SceneTarget`) and the colour is
blitted to the canvas once at the end of the frame; while none has, the frame goes straight to the
canvas as upstream's, and the only cost is a check of the layers each frame.
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

### Lines and fills lifted off the terrain

Upstream drapes every line and fill on the 3D terrain (drawn into the terrain's textures), so a
ferry route or a bridge follows the ground under it, and a lake's surface is wherever the DEM puts
it. Four paint properties change that (`style_spec_extensions.ts`, added to the style spec's own
object, so validation and expressions know them; the API after Mapbox GL v3's `line-z-offset` and
`line-elevation-reference`):

| Property | Values | |
|---|---|---|
| `line-z-offset`, `fill-z-offset` | number, metres, data-driven (`zoom`, `feature`, `feature-state`), default 0 | the height |
| `line-elevation-reference`, `fill-elevation-reference` | `ground` (default), `sea` | what it's measured from |

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
  subdivides fills; lines are as their data is). A `sea` surface is level, so that's exact for it.
- Without 3D terrain the ground is at 0: both are a height above the map.
- Not in the TypeScript types: the style objects' types are the style-spec package's, so a layer
  using them is cast.
- Labels: point labels already rise off the terrain upstream (`symbol-height-offset`, and
  `symbol-height-anchor` `ground` or `absolute`; `absolute` isn't exaggerated, so a label at the
  water's surface over exaggerated terrain takes the surface's elevation times the exaggeration).
  Line-placed labels (a ferry's name along its route) don't: planned, as a `symbol-height-offset`
  for line placement with an anchor `sea` on the same terms as above (the placement's collision
  boxes projected with the lifted elevation, `symbol/projection.ts`, as the point labels' are).
- `test/scenic/demo.html?case=lines` and `?case=water` show them; the unit tests are in
  `style_spec_extensions.test.ts`.

**The 3D sea floor (#99) on them.** With a terrain whose DEM goes below 0 under the sea (the sea
floor), the basemap's water fill gets `fill-elevation-reference: sea` and `fill-z-offset` 0 (a
lake's surface: its level, a feature property, as `['get', …]`) and a `fill-opacity` below 1, so
the surface lies level at the sea's elevation with the floor showing through; the ferry routes get
`line-elevation-reference: sea` and `line-z-offset` 0 to lie on that surface. Nothing else is
needed of this fork; the floor itself (the DEM below 0) is #99's. One gap: where the catalog has
the water's own tiles the app draws the water as a raster (basemap.ts's `water` layer), and rasters
have no elevation reference yet (the same for `raster` layers, drawn as a level quad per tile at the
elevation, isn't built); the basemap's water polygons, the fill it falls back to, take it as is.

### Known render-test differences

`test/integration/render/expected/` holds this fork's expected image where it draws differently on
purpose (`tools/fetch-upstream-tests.sh` puts them in place):

- `terrain/fill-extrusion`, `-vertical-gradient`, `-vertical-gradient-pattern`: the walls' feet on
  the terrain under each corner (the last also the finer DEM tiles near the camera, below).
- `terrain/fill-extrusion-multipolygon`: a few ridge pixels, the terrain's render-to-texture tiles
  finer near the camera (the tile zoom over the terrain).

Run on this Mac on 2026-10-11 (the last full run under other agents' load, the failures it showed
run again quiet): every other upstream case passes. Under heavy load a handful of terrain cases
(`terrain/symbol*`, `skirts-*`, `tile-transparency-overlap-*`, `line-opacity/terrain`) fail with
differences of 0 pixels at threshold 0.1, or a blank frame, and pass when run again: timing, not
drawing.

On this Mac (Apple silicon, SwiftShader in headless Chrome) upstream's own build fails four render
tests, `text-local-glyphs/{cjk,missing,no-glyphs}` and `text-local-ideographs/cjk-symbols` (the
system's fonts); so does this fork.
