# Our own MapLibre

Scenic Roads draws its map with this copy of MapLibre GL JS (BSD-3-Clause, `LICENSE.txt`), changed
wherever upstream holds the map back. Nothing here goes upstream.

- **Upstream:** github.com/maplibre/maplibre-gl-js, tag `v6.11.2`, without its history, `docs/`,
  `developer-guides/`, changelog, or render/query test cases and assets (125 MB;
  `tools/fetch-upstream-tests.sh` fetches them from the same tag).
- **Changes** are marked `Scenic Roads:` in the code (`git log -- web/maplibre`).

## How the app builds it

The app compiles this source with its own code; there is no separate build or `dist/`.

- `web/vite.config.ts` aliases `maplibre-gl` to `src/index.ts` (the worker is
  `src/source/worker.ts?worker&url`, the stylesheet `built/maplibre-gl.css`), in dev and in
  `tools/app/publish.sh`.
- Shaders are made from the `.glsl` files on import (`build/shaders_plugin.ts`).
- Other generated files (`src/data/array_types.g.ts`, `src/style/**/*_properties.g.ts`,
  `src/util/unicode_properties.g.ts`) are committed: run `npm run codegen` here after changing the
  style spec's properties or the struct arrays. The stylesheet is committed too: `npm run
  build-app-css` after changing `src/css/`.
- Runtime dependencies are the app's (`web/package.json`), pinned at this package's lockfile's
  versions, checked by `vite.config.ts` and resolved from `web/node_modules` only. Publishing needs
  only `npm ci` in `web/`.
- Types: `npm run typecheck` in `web/` writes this package's declarations into `types/` (under
  upstream's looser settings), then checks the app against them.

## Testing it

`npm ci` here first (then `npx puppeteer browsers install chrome` if needed).

- Unit tests: `npx vitest run --config vitest.config.unit.ts <paths>` (all: `npm run test-unit`).
- Render tests: `tools/fetch-upstream-tests.sh`, `npm run build-dev`, then `npx vitest run
  test/integration/render/render.test.ts -t <filter>`. Our cases are in
  `test/integration/render/tests/scenic/` (lifted lines across tile seams, a see-through sea
  surface, the globe's pole, `sea`-anchored labels, the `drape` hook and `sceneDepth`).
- `test/scenic/demo.html?case=custom|lines|water|rasterwater` shows the features on the app's
  terrain.

## What we changed

### Rendering

- **Circles and symbols behind the terrain** (`circle.vertex.glsl`, `symbol_*.vertex.glsl`): their
  visibility depth is computed as the terrain depth texture was drawn, so on the globe points and
  labels aren't all "behind the terrain" in tilted views.
- **The slope tint** (`color_relief.fragment.glsl`): the slope source (base shift 32768.5) holds
  four slopes a pixel; each is coloured and the four averaged in linear light.
- **The globe without float32 noise** (`_projection_globe.vertex.glsl`): positions are the tile's
  flat projection plus an interpolated globe-minus-flat difference, so terrain and buildings don't
  shimmer.
- **Walls on the terrain under each corner** (`fill_extrusion*.vertex.glsl`): a base of 0 is the
  ground under each corner less 2 m.
- **Distance fog, on the globe too** (`sky.ts` `fogRange`, `Map#setFogRange`, `terrain.*.glsl`,
  `draw_terrain.ts`, and the extrusions'): a vertex's fog depth is its distance along the view,
  scaled from 0 at the map's centre to 1 at the horizon (`fogDepth`); fog runs from the range's
  `start` to `end`, squared, to its `opacity`, from `fog-color` to `horizon-color` past
  `horizon-fog-blend`. Any pitch, either projection. `Sky#fogUniform` and `Sky#fogDepth` give custom
  layers the same values.
- **Trilinear mipmaps** (`draw_raster.ts`): a raster source with `"scenic:mipmap": "linear"`
  minifies between mipmap levels (the app's water).

- **Local glyphs drawn in slices** (`glyph_manager.ts`, `GLYPH_SLICE_MS`): glyphs drawn with
  TinySDF (CJK, clusters, `font-faces`) run at most 4 ms of the main thread at a time, the rest in
  later tasks; a city's CJK labels drew 300–600 ms of glyphs in one task. The labels come a frame or
  two later.

- **Raster tiles in Safari as bytes** (`raster_tile_source.ts`, `util/png_rgba.ts`): Safari reads an
  ImageBitmap back on the main thread to upload it (~2.8 ms a 512 px tile on an M1 Pro), so there a
  `raster` source's plain PNGs (8-bit RGB or RGBA, untagged) are decoded on two workers and uploaded
  as bytes, the same pixels; other images, and other browsers, as before. `?pngdec=0`: always as
  bitmaps.

### Tile covers over 3D terrain

- **Covers redone when elevations arrive** (`bounding_volume_cache.ts`, `TileManager.demVersion`,
  `Terrain.demVersion`): the globe's cached bounding volumes are cleared whenever a DEM tile loads
  or the terrain or exaggeration changes, so slopes near the camera aren't culled at sea level.
- **A tile's zoom by its distance over the terrain** (`covering_tiles.ts`): where a tile's
  elevations are known, its zoom gains log2 of how much closer the middle of its elevation range is
  than the centre's elevation assumed, capped at the map's maximum; never coarser than upstream's.
- **A tile's zoom by its depth, not its distance** (`tileDepthDistance2d`): tiles at the screen's
  sides get the zoom of their depth along the view, as in the middle (`covering_tiles_depth.test.ts`).
- **The drapes' tiles by elevation and depth** (`TerrainTileManager.terrainAwareZoom`, default
  true): the terrain's render-to-texture tiles take both zooms above. Two rules keep it from wasting
  memory (`footprintSizing`, default true): a tile stops splitting once its on-screen footprint
  fits its full texture (`texelParity`), and a tile deeper than the view's zoom gets a texture of
  the power of two at or above its footprint, 128² to 1024² (`Tile.rttTargetSize`; one FBO per size,
  `Painter._rttFbos`). A low camera over Chamonix holds ~500 MB of drapes instead of 3.9 GB.
- **Elevations looked up without a cover a point** (`Terrain.elevationForLngLatFn`, `_coverZoom`,
  `coverKey`): where no rendered tile holds a point, upstream's `getElevationForLngLat` worked out
  the camera's whole tile cover for every point; the cover's zoom is now worked out once a batch
  (`elevationForLngLatFn(transform)`, for callers sampling many points) and kept while the camera's
  matrix, the DEM tiles loaded and the exaggeration stay the same, so single lookups share it too.
  The coverage index looks tiles up by number (`samplerByNumber`) rather than by a string a zoom.
  The app's far buildings sample 2,401 points a tile: a wide tilted view (Yosemite, the Alps) took
  minutes of main thread before.
- **Elevation ranges of parts of a DEM tile** (`dem_data.ts`, `Terrain.getMinMaxElevation`): each
  DEM tile keeps min/max on a 32 × 32 grid and coarser levels, so deeper tiles get tight ranges.
- **Draped textures at a steady pace** (`pacedDrapes`, default true; `movingRedraws`, 2): a tile of a
  source with nothing draped no longer drops the terrain's textures; while the camera moves at most
  `movingRedraws` existing textures are redrawn a frame. At rest, as upstream.

- **Feature state redraws only the drapes that read it** (`render_to_texture.ts`): a source's state
  revision is part of a terrain texture's fingerprint only when one of its draped layers reads
  feature state. The ferry terminals' colours (feature state on `ferries`, whose lines are draped
  and don't read it) redrew every terrain texture each time, ~100 ms of GPU a time while orbiting.

### Custom layers: passes and the frame's depth

| Pass | Hook | When, and into what |
|---|---|---|
| offscreen | `prerender(gl, args)` | first, any framebuffer of its own |
| the terrain's | `drape(gl, args)` | with 3D terrain: per terrain tile drawn, into its texture, in layer order; redrawn when the tiles, zoom or `drapeRevision` change (set it, then `map.triggerRepaint()`) |
| main | `render(gl, args)` | at its place in the order; for a layer with `drape`, right after the terrain |

`CustomRenderMethodInput` gains `framebuffer` (to rebind), `depthRangeFor3D`, `terrainDepth`
(`{texture, width, height, revision, unpack}`, the terrain's packed depth), `sceneDepth` (for a
layer with `readsSceneDepth: true`: the frame's depth so far as a `DEPTH24_STENCIL8` texture, with
`framebuffer` lacking a depth attachment, so the layer depth-tests in its shader), and for `drape`
`tileID`, `size` and `getDrapeMatrix(tile)`. While any visible layer reads scene depth, the frame
draws into a `SceneTarget` blitted to the canvas at the end; otherwise straight to the canvas. Not
available with a multisampled canvas (a warning says so). `web/src/bldfield.ts` uses these.

### Lines, fills, rasters and labels lifted off the terrain

Paint properties (`style_spec_extensions.ts`, after Mapbox GL v3's API):

| Property | Values |
|---|---|
| `line-z-offset`, `fill-z-offset` | metres, data-driven, default 0 |
| `line-elevation-reference`, `fill-elevation-reference` | `ground` (default), `sea` |
| `raster-z-offset`, `raster-elevation-reference` | as above, one height per layer |
| `symbol-height-anchor` | upstream's `ground`, `absolute`, plus `sea` |

- `ground`: metres above the terrain under each vertex, not exaggerated; offset 0 drapes as
  upstream. `sea`: above sea level on the terrain's (exaggerated) scale.
- Otherwise the layer isn't draped but drawn in 3D in the main pass, depth-tested against the
  terrain (`render/lifted.ts`, the `LIFTED` program variant; non-lifted layers run upstream's code).
  Opaque lifted fills write depth; see-through ones don't.
- Each vertex takes its own point's elevation, so a `ground` line follows terrain only as finely as
  its vertices (give GeoJSON sources `tolerance: 0`); `sea` surfaces are exact.
- Lifted layers clip to their tile in the fragment shader instead of stencil masks, and take the
  parent/child overlap stencil. Known gap: a parent's features the child lacks still show until the
  parent is released.
- On the globe without terrain, a lifted surface near sea level z-fights with the sphere's depth
  pre-pass at low zooms. Not fixed.
- Sources are culled with room for their lifted layers' heights.
- The new properties aren't in the TypeScript types: cast the layer.
- Line-placed labels aren't lifted (planned: lift the label line's vertices in
  `symbol/projection.ts`).

The app uses these for the water (the raster `water` layer and fills at `sea` 0) and ferry routes
on a 3D sea floor, with `symbol-height-anchor: sea` for their labels.

### Known render-test differences

`test/integration/render/expected/` holds this fork's images where it draws differently on purpose
(`tools/fetch-upstream-tests.sh` puts them in place): the extrusion walls' feet
(`terrain/fill-extrusion*`), finer drapes near the camera (`terrain/default`,
`terrain/fill-extrusion-multipolygon`, `symbol-height-anchor/sea`), and the tile zoom by depth
(`high-pitch/terrain-pitch95`, `field-of-view-number`, `fog-sky-blend-globe`, `heatmap`,
`image-projective`, `occlusion-debug`, `rtt-mipmap-minification/pitch-70`,
`symbol-height-anchor-*`, `symbol-height-offset-building`, `tile-stiching`). Every other upstream
case passes, but under heavy load a few terrain cases flake on timing, and on Apple silicon four
`text-local-*` cases fail on system fonts, as upstream's own build does.
