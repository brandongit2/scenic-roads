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
  `src/util/unicode_properties.g.ts`) are committed: run `pnpm run codegen` here after changing the
  style spec's properties or the struct arrays. The stylesheet is committed too: `pnpm run
  build-app-css` after changing `src/css/`.
- Runtime dependencies are the app's (`web/package.json`), pinned at this package's lockfile's
  versions, checked by `vite.config.ts` and resolved from `web/node_modules` only. Publishing needs
  only `pnpm install --frozen-lockfile` in `web/`.
- Types: `pnpm run typecheck` in `web/` writes this package's declarations into `types/` (under
  upstream's looser settings), then checks the app against them.

## Testing it

`pnpm install --frozen-lockfile` here first (then `pnpm exec puppeteer browsers install chrome` if needed). `pnpm-workspace.yaml` here lets pnpm run only canvas's install script (puppeteer's would download Chrome, hence the command above).

- Unit tests: `pnpm exec vitest run --config vitest.config.unit.ts <paths>` (all: `pnpm run test-unit`).
- Render tests: `tools/fetch-upstream-tests.sh`, `pnpm run build-dev`, then `pnpm exec vitest run
  test/integration/render/render.test.ts -t <filter>`. Our cases are in
  `test/integration/render/tests/scenic/` (lifted lines across tile seams, a see-through sea
  surface, the globe's pole, `sea`-anchored labels, the `drape` hook and `sceneDepth`).
- `test/scenic/demo.html?case=custom|lines|water|rasterwater` shows the features on the app's
  terrain.

## The app's reads and writes

Besides the changes below, the app reaches into internals that aren't API (listed in
`web/package.json`'s note too). Among them: `web/src/main.ts`'s terrain rise at start writes
`map.terrain.exaggeration` (the `Terrain`'s field) each frame, as `setTerrain` would build a new
`Terrain`; `map.getTerrain()` keeps the options' value meanwhile, so readers that must follow the
height (`web/src/bldfield.ts`'s `terrainKey`) read `map.terrain.exaggeration`.

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
  as bytes, the same pixels (premultiplied rounding up, as Safari's own upload). They're fetched
  through the image request queue (`ImageRequest.getImage`'s `decode`), so its limits and expiry
  apply. Other images, a worker that fails or takes over 10 s, and other browsers: as before.
  `?pngdec=0`: always as bitmaps.
- **Shaders linked in the background in Safari** (`program.ts` `ready`, `context.ts`
  `asyncPrograms`/`linkComplete`, `painter.ts` `BLOCKING_PROGRAMS`): with
  KHR_parallel_shader_compile, a program's link status, attributes and uniforms are read only once
  the driver says the link is complete; until then its draws are skipped (`pendingPrograms`), the
  map draws another frame, and a draped texture missing a draw is drawn again next frame. A layer
  can so appear a frame or two late the first time its shader is needed. The completion isn't asked
  in the frame a program was made, nor again in a frame after one link was found incomplete (each
  question waits for Safari's GPU process); a link still not complete after 120 frames
  (`LINK_WAIT_FRAMES`) is waited for. A failed link throws at each draw, as upstream. The tiles' clipping masks, the depth pass, the
  hill-shading's prepare pass and the terrain's depth are always waited for. On by default where
  `navigator.vendor` is Apple's; elsewhere (Chrome showed no gain) as upstream. The app's custom
  layers do the same (`web/src/glprog.ts`); `?ashader=0|1` sets it in any browser.

### Tile covers over 3D terrain

- **A source's finest zoom by area** (`scenic:zoom-by-area` on a `raster` or `raster-dem` source,
  `{"zoom": z, "tiles": {"x/y": finest zoom}}`; covering_tiles.ts `ZoomByArea`, `areaMaxZoom`;
  `RasterTileSource.zoomByArea`, `TileManager`'s cover and its memo key, `TerrainTileManager.getSourceTile`):
  in a listed tile of zoom z (z at most the source's maxzoom) the cover goes on to that tile's own
  finest zoom, overzoomed past it; elsewhere it is upstream's, so a source with finer data in some
  places asks for no extra tiles anywhere else, and the finer tiles sit beside the coarser cover
  with no gap (a cover is a partition). The terrain's elevations read the DEM at the same cap. The
  app's terrain, hill-shading and slope go to z16 where the catalog has 2 m terrain
  (`covering_tiles_area.test.ts`; `test/scenic/zoom-by-area.html` shows an edge). Read when the
  source loads, and anew by `RasterTileSource.setZoomByArea` (a new catalog's areas, main.ts).
- **DEM borders filled across zooms** (`tile_manager_raster_dem.ts` `backfillDEM`,
  `DEMData.backfillFrom`): a loaded DEM tile's 2 px border, and that of each renderable tile it
  touches, is filled from every renderable tile touching it, of any zoom, each border pixel from the
  pixel holding its centre, as stored (exact between tiles of one zoom; the slope's and the trees'
  packed pixels kept whole, so not interpolated), coarsest first so the finest data at a place is
  written last. Upstream filled it from its eight same-zoom neighbours only (their `neighboringTiles`
  and `backfillBorder`, gone), so beside a tile of another zoom (an area's edge, or a coarser tile
  standing in) the border clamped: a dark line in the hill-shading along the edge.
- **Hill-shading at the DEM's own pixel size** (`hillshade_prepare.fragment.glsl` `u_dem_zoom`): the
  slopes take the DEM tile's canonical zoom for its pixels' size, the drawn zoom only for the
  exaggeration. Upstream took the drawn zoom for both, so a tile overzoomed n levels shaded 2^n too
  steep, and stepped in tone beside a tile of finer data at the same zoom (an area's edge at z14).
  Past the data's zoom the shading is now that of the true slopes, softer than upstream's.

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
- **New drape textures over a few frames while moving** (`standInFrames`, 2;
  `TerrainTileManager._retired`, `RenderToTexture._pickStandIns`, `u_texture_rect` in
  `terrain.vertex.glsl`): with pacedDrapes, a terrain tile leaving the cover keeps its textures
  (retired) while a tile under it in the new cover has none, and is taken back whole if the cover
  returns to it (marked to be drawn again, paced: a source tile may have loaded under it meanwhile;
  a retired tile keeps its marks). While the camera moves, the new tiles that have such a retired ancestor are drawn
  over at most `standInFrames` + 1 frames, the longest-waiting first and as many a frame as finish
  them in time; meanwhile each draws its own mesh with its part of the ancestor's texture (coarser
  for a frame or two). Tiles without one are drawn at once. At rest none waits and the retired
  textures are released; the texture rect is (0, 0, 1, 1), so a still view is drawn as before. When
  the cover changed level in a pan over the lochs, 4–8 new textures came in one frame.
- **A style change redraws the drapes at the same pace** (`Map#_handleTerrainDataEvent`,
  `TerrainTileManager.dirtyAllRTT`): with pacedDrapes, a `style` data event (any paint, layout or
  filter change) notes every texture to be drawn again instead of dropping them all: at rest all
  are drawn again in that frame, as upstream; while moving `movingRedraws` a frame, the others kept
  meanwhile (a draped layer's new paint reaches the whole view over a few frames). The app eases the
  ferry lines' colour range every 100 ms during gestures, and each change drew every texture again
  in one frame: 19–29 textures of 1024² ~7 times a second in a pan over the lochs.

- **A camera moved a frame at a time counts as moving** (`Map#isMoving`, `MOVING_HOLD_MS`): the
  map is moving for 100 ms after any frame whose view-projection matrix differs from the last
  frame's, not only during an ease or a handler's drag. The app moves the camera with `jumpTo` (its
  trackpad gestures, glides and flights), which upstream never counted as moving, so the painter's
  `moving`, and with it the drapes' pacing above, the RTT pool's reuse, the hillshade's alignment
  and the image queue's throttling, were those of a map at rest in every gesture: each drape whose
  source tiles or zoom changed was drawn again in the frame it changed (20–30 textures of 1024² in
  one frame of a pan over the lochs, 50–100 ms a time) and the drape pool was emptied every frame.
  `idle` comes at least 100 ms after the last camera change (a frame is scheduled for the hold's
  end, only while the hold alone keeps the map from idle; a map still loading waits for its tiles as
  upstream), and so does everything that waits for it: the app's `idle` listeners, bldcmp's and
  evalmode's waits, the benches' settling.
- **Tile covers shared between sources** (`Style#_updateSources`, `TileManager#_coverMemo`): within
  one update, sources asking `coveringTiles` with the same options (tile size, zoom range, rounding,
  overscaling, content elevation, zoom function) share one answer (a copy each). The app's 28 sources
  in use ask 8 distinct covers; the covers were 2–3 ms of each frame of a pan over terrain.
- **Visible coordinates once a frame** (`TileManager#getVisibleCoordinates`, `_visibleMemo`): the
  renderable tiles' ids are sorted once a frame per source (the painter asked three times, the
  drapes and the app's layers again), a copy each time.

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

### Tiles fetched again by part

- **`Map#refreshTilesWhere(sourceId, changed)`** (`TileManager#refreshTilesWhere`): fetches again
  the tiles `changed(z, x, y)` picks, drawn from their old data until the new is in (loading and
  errored tiles too), and drops the picked ones kept out of view. The app's request transform gives
  each tile's URL its pack's version (`web/src/api.ts` packUrl), so a new catalog refreshes only
  the tiles of the packs it changed instead of `setTiles` reloading the whole source.

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
