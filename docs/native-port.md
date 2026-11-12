# The native map app: one engine, no MapLibre side

**Status: planned (task #1196; board folder "Native map app port").** Nothing here is built. The
map is the web app (`web/src`) drawn by our MapLibre (`web/maplibre`, `FORK.md`); until a step of
§8 lands, that code is what is true. Sections 2 and 6 count the code as it is; everything else is
a design for the owner to decide on (§9). Line counts are `wc -l` of source files without tests
(§2.1 says how to count them again); every "after" figure is an estimate made by going down the
file lists, good to about a quarter either way.

## 1. Summary

- **What is planned.** A native iOS, iPadOS and macOS app replaces the map web app. It stays a
  frontend to the map server on the Mac mini and the NAS's data: no offline store beyond the
  region downloads, the task board and the build page stay web apps. Parity is relaxed: esoteric
  features, imperceptible visual differences and implementation details need not carry over.
- **What "subsumed" means.** Today the map is two programs: MapLibre (79,700 lines, plus the
  17,700-line style-spec interpreter it runs) that owns the camera, the tiles, the terrain, the
  labels and the frame, and the app (39,300 lines) that describes its base layers to MapLibre as
  style JSON and draws its own layers (roads, the far buildings, dots, contours, the polar caps)
  as "custom layers" that reach into MapLibre's internals for the camera's matrices, the tile
  covers, the terrain's textures and the glyphs. Subsumed, there is one engine with one of each
  thing: one camera, one tile pyramid and byte budget, one terrain service, one frame, one label
  engine, one pick, and a set of layers that are all the same kind of object. The base layers are
  defined in code like the roads; nothing is interpreted from a style document; no layer is
  "custom" (§4, §5).
- **How much code.** Of MapLibre's 79,700 lines the one map needs about 30,000 as TypeScript,
  about 23,000 native (system text, gestures, networking and image decoding replace the rest). The
  whole native app is estimated at 50,000 lines against 119,000 today (§6).
- **What gets simpler natively, what does not.** Simpler: threads with shared memory (no worker
  copies), Core Text (shaping, bidirectional text, glyphs), system gesture recognizers, Metal
  (no state-caching wrapper, no context loss, shared buffers, memoryless attachments), URLSession
  and ImageIO, explicit memory with the system's pressure signal. Not simpler: the globe's and the
  terrain's maths, tile covers, the drapes and their pacing, label placement and collision,
  tessellation, the far field's march, the roads' renderer (§5.8).
- **Recommendation: (C).** Do in the web app only what is cheap there and makes the native work
  smaller or safer: delete what the map does not use of MapLibre (about 16,000 lines, each
  deletion checked by the existing tests), change the served data formats the native design
  depends on while the web app can show they look the same, and record reference views and
  numbers from the web app as the native engine's oracle. Build the subsumed architecture once,
  natively. Doing the whole subsuming in TypeScript first (A) writes the hard half twice, cannot
  have the properties that motivate it (shared memory, Core Text, Metal), and loses its own
  verification on the way: MapLibre's render tests are style documents, and the first thing the
  subsuming deletes is the code that reads them (§7).
- **The renderer's language is open:** Swift and Metal throughout, or a Rust core on wgpu under a
  thin Swift shell. §5.9 gives the considerations; a trial (§8, N0) decides.

## 2. What the app uses of MapLibre today

### 2.1 How the lines are counted

`web/maplibre/src`: every `.ts` and `.glsl` file that is not a `*.test.ts`, each file in exactly
one row of the table below by its path (the "Files" column; a file matching several rows goes to
the first that lists it). 79,688 lines in 414 files. The tests are 62,538 lines in 227 files, plus
the render suite (`test/integration/render`). The app: `web/src/**/*.ts` without tests, 39,255
lines. Dependencies are counted from their `src/` in `web/node_modules`.

### 2.2 By subsystem

"Uses" is what the app's code and style actually exercise; "reaches in" is where the app reads or
writes what is not API (`web/package.json`'s note lists the main ones; 145 casts in `web/src`).

| Subsystem | Lines | Files | The app uses | The app does not use | Reaches in |
|---|---:|---|---|---|---|
| **Gestures** | 4,043 | `ui/handler/`, `ui/handler_manager.ts`, `ui/handler_inertia.ts` | The pointer events behind `map.on('click' …)`. | Every gesture: `trackpad.ts` disables scroll zoom, double-click zoom, drag pan, drag rotate, touch zoom, rotate and pitch and does them itself, anchored on the terrain under the cursor (`camera3d.ts`). Box zoom, keyboard, cooperative gestures, inertia. | — |
| **Camera and transforms** | 7,921 | `ui/camera.ts`, `geo/transform_*`, `geo/projection/*_transform.ts`, `*_camera_helper.ts`, `geo/lng_lat*`, `geo/bounds.ts`, `geo/edge_insets.ts`, `geo/mercator_coordinate.ts`, `util/primitives/` | `jumpTo` (25 calls: every gesture, glide and flight is the app's own, set a frame at a time), `flyTo` (4), `fitBounds`, `cameraForBounds`, `project`/`unproject`, the two transforms and their blend (globe zoomed out, Mercator zoomed in), frustum and horizon. | World copies (`renderWorldCopies: false`), roll, most of `easeTo`'s options, the pluggable transform interface (there are two transforms and always will be). | `camera3d.ts`, `covers.ts`, `terrain.ts`, `roads/layer.ts`, `dots.ts`, `contours.ts` read `transform._mercatorTransform`, `_verticalPerspectiveTransform`, `_cameraPosition`, `_pixelMatrixInverse`, `_globeViewProjMatrixF`; `main.ts` calls `_elevateCameraIfInsideTerrain`, `_updateElevation`. |
| **Map shell and utilities** | 8,620 | `ui/map.ts` (4,841), `ui/events.ts`, `ui/hash.ts`, `index.ts`, `util/util.ts`, `util/evented.ts`, `util/dom.ts`, `util/browser.ts`, small `util/` | The canvas, resize, the frame loop, `idle`, `isMoving`, about 60 of `Map`'s methods (most often `getLayer`, `setPaintProperty`, `getZoom`, `triggerRepaint`, `on`). | The location hash, locales, most of the event types, the long tail of `Map`'s API. | `map._loaded`, `map.style`, `map.terrain`. |
| **Controls, markers, popups** | 3,924 | `ui/control/`, `ui/marker.ts`, `ui/popup.ts`, `util/geolocation_support.ts` | `Popup`, once (`popup.ts`). | Every control (`attributionControl: false`, no `addControl`), `Marker` (the app's markers are a GeoJSON source), geolocation. | — |
| **Tile covers** | 1,116 | `geo/projection/*covering_tiles*`, `util/primitives/bounding_volume_cache.ts` | All of it, with the fork's zoom by depth, elevation, latitude and area (`FORK.md`, "Tile covers over 3D terrain"). | — | `covers.ts` replaces the tile-zoom function and caches frustum tests from outside. |
| **Projection, globe, terrain, drapes** | 5,082 | `geo/projection/*projection*.ts`, `globe_utils.ts`, `pole_caps.ts`, `render/terrain.ts`, `render/subdivision*.ts`, `render/fill_large_mesh_arrays.ts`, `tile/terrain_tile_manager.ts`, `data/dem_data.ts`, `util/create_tile_mesh.ts`, `webgl/render_to_texture.ts`, `rtt_fingerprint.ts`, the terrain's and projection's uniform buffers | Nearly all: the globe, the 3D terrain with its elevation queries, the drapes with the fork's sizing and pacing, the pole caps and pole ground. | The projection factory (the map is always "globe"). | `main.ts` writes `map.terrain.exaggeration` each frame of the opening rise; `bldfield.ts`, `polarglobe.ts`, `dots.ts`, `contours.ts` read the terrain's tile manager, DEM tiles and mesh cache. |
| **Tile sources, loading, caches, feature queries** | 7,091 | `tile/` (but the terrain's), `source/source*.ts`, `raster_*`, `vector_tile_*`, `query_features.ts`, `load_tilejson.ts`, `protocol_crud.ts`, `util/ajax.ts`, `util/image*.ts`, `util/png_rgba*.ts`, `data/feature_index.ts` and its helpers | Vector (9 sources), raster (water), raster-dem (terrain, tint, slope, depth, trees), `addProtocol` (4 uses: tiles made by the app's own workers handed to MapLibre as if fetched), `queryRenderedFeatures`, `querySourceFeatures`, feature state, `refreshTilesWhere`. | TileJSON, MLT tiles, overscaled DEM tiles. | `maptasks.ts`, `terrainstats.ts`, `namefade.ts`, `covers.ts`, `contours.ts`, `buildings.ts` (`keepWhole`: `_zoomLevelsToOverscale`, `_inViewTiles`), `bldfield.ts` read `style.tileManagers`. |
| **GeoJSON, image, video, canvas sources** | 2,543 | `source/geojson_*`, `image_source.ts`, `video_source.ts`, `canvas_source.ts`, `util/geojson_bounds.ts` | GeoJSON (4 sources: ferries, the markers, selections and highlights, the regions' outlines), one image (the viewshed). | Video, canvas, GeoJSON clustering and diffs. | — |
| **Style, properties, atlases** | 9,199 | `style/`, `data/program_configuration.ts`, `render/image_*.ts`, `pattern_atlas.ts`, `line_atlas.ts`, `util/color_ramp.ts` | A style of about 60 layers built in `basemap.ts` (background, fill, line, symbol, circle, fill-extrusion, raster, hillshade, color-relief) and the app's own six kinds of custom layer; `setPaintProperty` (55 call sites), `setLayoutProperty`, `setFilter`; about 40 expression operators; dashes. | Style loading, validation, diffing and migration (the style starts empty and is built by calls), sprites and images (`poiicons.ts` rasterises its own atlas for its own layer), patterns, heatmap, the sky (#1209), light beyond the extrusions' default, transitions. | `polarglobe.ts` reads the hillshade's and the tints' evaluated paint (`paint.get`, `_createColorRamp`); `polarpoints.ts` the label layers' evaluated paint and layout; `details.ts` `style._layers`. |
| **The style-spec package** | 17,724 | `@maplibre/maplibre-gl-style-spec` (a dependency) | The expression parser, type checker and evaluator, filters, colour and interpolation. | Validation, diff, migrate, the reference document, old-style functions. | — |
| **Buckets and tessellation** | 4,864 | `data/bucket*`, `data/segment.ts`, `data/array_types.g.ts`, `util/struct_array.ts`, small helpers | Line, fill (earcut), fill-extrusion, circle and symbol buckets. | Heatmap, patterns, most of the 1,181 generated struct-array types. | `buildings.ts` (`_vectorTileFeature`). |
| **Symbols, glyphs, placement** | 9,257 | `symbol/`, `render/glyph_*.ts`, `font_face_manager.ts`, `source/rtl_text_plugin_*`, `util/script_detection.ts`, `verticalize_punctuation.ts`, `graphemes.ts` | Text labels on points and along lines, wrapping, halos, sort keys, collision and fades, the cross-tile index, server glyph ranges and locally drawn CJK. | Icons (no `icon-image` anywhere), variable anchors, vertical text, the RTL plugin (never loaded: Arabic and Hebrew names are drawn unshaped today). | `glyphs.ts` (`glyphManager._drawGlyph`), `placement.ts` (`style._updatePlacement`), `polarpoints.ts` (the glyph manager's SDF glyphs). |
| **Render passes, programs, shaders** | 10,378 | `render/painter.ts`, `webgl/draw/`, `webgl/program/`, `shaders/` (4,248 lines of GLSL), `webgl/scene_target.ts` | The offscreen, drape, opaque and translucent passes; the terrain's depth and coordinate targets; the fork's scene depth, lifted layers and water mask; the shaders of the nine layer types in use. | Heatmap, sky and atmosphere, every pattern shader, line gradients, icon and text-and-icon symbols, the debug and collision-debug draws. | `roads/layer.ts` reads `painter._tileClippingMaskIDs`; the custom layers set raw GL state beside MapLibre's cache (`glprog.ts`). |
| **WebGL wrapper** | 2,262 | the rest of `webgl/` | All of it, by MapLibre's own layers; the app's layers call WebGL directly. | — | — |
| **Workers and transfer** | 2,021 | `source/worker*.ts`, `vector_tile_worker_source.ts`, `util/actor*.ts`, `dispatcher.ts`, `web_worker*.ts`, `*worker_pool.ts` | Six tile workers (parse, tessellate, shape), the class registry that serialises buckets across threads. The app runs seven worker kinds of its own beside them (road and rail tiles ×12, depth ×2, coast ×2, landmarks, far field, contours, contour DEM), each with its own messages and caches. | Custom worker sources. | — |
| **Test helpers, benches** | 1,367 | `util/test/`, `*.bench.ts` | — | — | — |
| **Total** | **79,688** | | | | |

Other runtime dependencies the map reaches through MapLibre: geojson-vt (2,690 lines), pbf (1,187),
bidi-js (1,046), earcut (1,016), vector-tile (370), tiny-sdf (163), potpack, kdbush, quickselect,
tinyqueue, murmurhash, gl-matrix, vt-pbf (GeoJSON and overzoomed tiles); MLT is bundled and unused.

### 2.3 What the reaches say

The app already owns the parts of the map that matter most to it, and takes from MapLibre mostly
*services*: where the camera is, which tiles cover the view, what the ground's height is, the
terrain's depth, a place in the frame. Each of its own layers gets those through a hole in the
wall, and several features exist only to work round the wall:

- `dots.ts` keeps invisible circle layers under its own dots so MapLibre can hit-test them;
  `namefade.ts` pushes every landmark name's opacity through feature state because a symbol
  layer's paint cannot follow the dots' easing; the landmarks worker encodes its points as
  Mapbox vector tiles (`mvt.ts`) so MapLibre will accept them through `addProtocol`.
- `polarpoints.ts` and `polarlabels.ts` (589 lines) draw the caps' labels on a 2D canvas over the
  map with glyphs borrowed from the glyph manager, because MapLibre's symbols live in Web Mercator
  tile coordinates.
- `covers.ts`, `placement.ts`, `glyphs.ts`, `maptasks.ts` (452 lines) patch or read MapLibre from
  outside to make it cheaper or to see what it is loading.
- `camera3d.ts` computes every camera move as a rigid motion about a point on the ground, then
  converts the result back into MapLibre's centre, zoom and pivot.
- The roads, the far field and MapLibre each keep their own tile cache with its own budget; the
  memory audit (`.claude/notes/NATIVE-PORT-MEMORY.md`) finds 12 GB held after a tour, none of it
  garbage, because no one owner can see the total.

## 3. Per subsystem: delete, reshape, keep

"Delete" is machinery for maps in general that this one map does not need. "Reshape" names what the
module becomes when it serves only this map. The target module is §5.1's.

| Subsystem | Verdict | What happens |
|---|---|---|
| Gestures | **Delete** | Input is the app's: `trackpad.ts`'s gestures become the engine's input module, calling the camera's anchored moves directly. What stays of MapLibre's is a pointer-to-pick dispatcher. Natively: system gesture recognizers. |
| Camera and transforms | **Reshape → `camera`** | One `Camera` (a rigid pose: position and orientation, with centre, zoom, bearing and pitch derived for settings and readouts) and one `View` (the frame's matrices for the globe and the flat map and their blend, frustum, horizon, project and unproject). `camera3d.ts`'s dolly, orbit and ground-anchored pan are its native operations, not a conversion layer; flights and glides are one small animator. The transform interface, the helper classes per projection, world copies and roll go. The projection maths stays as it is. |
| Map shell and utilities | **Reshape → `engine` (small)** | A view that owns the surface, the frame loop, "moving" and "settled", and the list of layers. No event bus of string names, no general `Map` API: the app calls typed methods on the modules. |
| Controls, markers, popups | **Delete** | The one popup becomes the app's own anchored element (natively a popover). |
| Tile covers | **Keep → `tiles/cover`** | One function from a view and a tile scheme to tiles, with every rule the fork added, computed once per distinct scheme a frame (`covers.ts`'s savings built in rather than patched on). |
| Projection, globe, terrain, drapes | **Keep, reshape the edges → `terrain`** | The globe's maths, subdivision, DEM tiles with their min/max pyramids and cross-zoom borders, elevation queries, pole ground, the drapes with footprint sizing, detail and pacing: all kept. What changes: one projection (no factory), the terrain exposed as a service every layer uses the same way (heights at points, the DEM texture of a tile, the depth target), not as fields reached into. |
| Tile sources, loading, caches, feature queries | **Reshape → `tiles`** | One `Pyramid` generic over what a tile holds (wanted, loading, loaded, standing in, retired), used by every layer including roads and the far field; one `Store` with one byte budget; one fetch queue with priorities and cancellation. A "source" is a function from a tile id to bytes (HTTP, or computed, which replaces `addProtocol`). TileJSON, MLT, source classes per type, the event plumbing go. The feature index stays, smaller, as the vector layers' pick. |
| GeoJSON, image, video, canvas sources | **Delete, with a small replacement** | Video and canvas go. The few in-memory shapes (markers, selection, highlights, region outlines, ferries, the viewshed's image) become an `overlay` layer that tessellates its shapes directly, without cutting them into tiles. |
| Style, properties, atlases | **Delete → layers in code** | No style document, no expression interpreter, no diffing, no string-keyed properties. Each layer is a module with a typed parameter struct the app's state sets; what an expression did per feature is a function in the layer's decoder (properties in, vertex attributes out), what it did per zoom is a function evaluated each frame into a uniform. Sprites, images, patterns go; the dash atlas stays with the line renderer. `basemap.ts`'s 1,200 lines of style become layer definitions of about the same size. |
| The style-spec package | **Delete** | Colour parsing and interpolation (about 300 lines) are kept as plain functions. |
| Buckets and tessellation | **Reshape → inside each layer** | Line, fill, extrusion tessellation and earcut stay, each writing a hand-declared vertex layout straight into the buffer that is drawn. The generated struct arrays and the property binders go. Circles go to the dots' renderer. |
| Symbols, glyphs, placement | **Keep the core, reshape the edges → `labels`** | Placement, collision, fades, the cross-tile index and line labels stay. The engine takes label candidates in world coordinates from any layer (so the caps' labels and the landmark names are ordinary labels with a per-label opacity) rather than only from symbol buckets of Mercator tiles. Icons, variable anchors, vertical text and the RTL plugin go. Natively, shaping and glyphs are Core Text's. |
| Render passes, programs, shaders | **Reshape → `frame`** | A fixed list of passes (§5.2) that every layer takes part in through the same interface; each layer owns its shaders. The painter's per-type dispatch, the custom-layer bridge, and the unused programs go. |
| WebGL wrapper | **Keep on the web, slimmer; not ported** | One small GPU layer used by every layer (today the app's layers bypass it). Natively it is Metal or wgpu, with no state cache. |
| Workers and transfer | **Reshape → one job pool** | One pool, one job shape (fetch, decode, write into the final buffer), used by every layer. The actor protocol and class registry go. Natively: threads sharing memory. |

### 3.1 The layers as peers

Every layer implements the same small interface (§5.1): which tiles it wants, how a tile's bytes
become what it draws, what it paints into the drapes, what it draws in 3D, which labels it offers,
what it answers to a pick, how many bytes it holds. None is registered as "custom".

| Layer | Today | As a peer |
|---|---|---|
| **Land: background, elevation tint, hill-shading** | Style layers (`background`, `color-relief`, `hillshade`) over raster-dem sources, with the fork's stored-shading shader. | `layers/land`: paints the drapes from the tint and shading tiles. Its parameters (ramp, light, strength) are a struct; the polar cap reads the same struct rather than MapLibre's evaluated paint. |
| **Water, whitewater, coast shading** | A raster layer with `scenic:mipmap`, a color-relief with `scenic:mask`, fills at `sea` level, two coast workers. | `layers/water`: paints the drapes from coverage tiles (two 8-bit shares coloured in the shader, flat tiles as one shared texel: the audit's a9 and a11), the mask a plain second texture of the same layer. |
| **Depth (colour, shading, floor, contours)** | A color-relief and a hillshade over tiles a worker composes and hands in through `addProtocol`; the floor through `setTerrainSurface`. | `layers/depth`: a computed tile source of the common pyramid; the floor is a second surface of the terrain service. |
| **Roads and rail** | A custom layer with its own tile loading, workers, budget, pick grid and a screen-size target each. | `layers/lines`: the same renderer (sprites and quads), its tiles in the common pyramid and budget, its vertices held once (shared with the pick), scenic channels a separate stream bound only for scenic colours (b10), one accumulation target shared. |
| **Basemap lines** (boundaries, waterways, ferries, outlines, highlights) | MapLibre line layers draped into the terrain's textures. | Stay draped at first, tessellated by `layers/vector`. Drawing them with the roads' renderer (sharp at any tilt, one line code path) is a look change left to the owner (§9). |
| **Contours** | A custom layer drawing what a MapLibre vector source loads, from tiles two workers make. | `layers/contours`: a computed tile source; drawn by the line renderer; its labels offered to the label engine. |
| **Areas** (parks, heritage, Indigenous lands, special areas) | Fill and line layers. | `layers/vector` fills painted into the drapes. |
| **Buildings, near** | A `fill-extrusion` layer plus `keepWhole` reaching into the tile manager. | `layers/buildings`: extrusion meshes from the z14 tiles (whole-building tiles are its own rule, not a patch). |
| **Buildings, far field** | A custom layer with its own tiles, worker and 1 GB cache target, reading scene depth through a fork hook. | The same module's second representation: the hand-over between meshes and field is decided in one place; tiles in the common budget, never finer than the screen samples (a2). |
| **Dots** (stops, sights, heritage, stations, terminals) | A custom layer for the look plus invisible circle layers for hit-testing and label anchoring. | `layers/dots`: one point renderer for every dot and circle, with its own pick; its names go to the label engine with the dots' eased opacity. |
| **Place and water names** | Symbol layers over the server's label tiles. | `layers/names`: decodes label tiles into label candidates. |
| **Polar caps** | A custom layer for the cap's terrain; a 2D canvas for its labels, dots and roads. | `layers/polar`: the cap's terrain mesh; its names, dots and roads are ordinary candidates and geometry of the label engine, the dots and the line renderer, which all work in world coordinates. |
| **Overlays** (markers, selection, hover, viewshed) | GeoJSON and image sources with style layers. | `layers/overlay`: shapes tessellated directly. |

## 4. What stays true of the product

Everything in `.claude/notes/NATIVE-PORT-FEATURES.md` stays except what is already decided gone
(link and saved-state migrations #1203; the debug and benchmark tools #1204; sky and distance fog
#1209; road slope statistics, "Road above 1,000 m" and Drive #1210; live data updates #1212). This
design argues for no further removals. It does change four things a user could notice, each an
improvement or a judgement for §9:

- Arabic and Hebrew names are shaped correctly natively (Core Text); they are not today.
- Labels are set by the system's text engine from fonts shipped in the app, so glyph shapes and
  spacing differ slightly from the server's glyph ranges.
- The caps' labels, dots and roads collide and fade like every other label.
- Memory is bounded by one budget, so a long session holds what the view needs, not what was seen.

## 5. Target architecture

### 5.1 Modules

One tree, no engine/app wall below the user interface. Names are for the design, not final.

```
geo/        coordinates: lng/lat, Mercator, earth-centred, tile ids, the globe-to-flat blend
camera/     Camera (pose), View (matrices, frustum, horizon), anchored moves, animator, limits
input/      gestures to camera moves; pointer to pick
tiles/      cover; Pyramid<T>; Store (one byte budget); fetch queue; decode jobs
terrain/    DEM tiles, heights, surfaces (ground, floor, pole), mesh, drapes and their pacing
frame/      passes, targets, the GPU layer (buffers, textures, pipelines), the frame loop
labels/     text (shaping, glyph atlas), candidates, placement, collision, fades, line labels
pick/       one query across layers
layers/     land, water, depth, lines, vector, contours, buildings, dots, names, polar, overlay
app/        state and profiles, the server's API, panels, lists, search, popups
```

The layer interface, in words: `wants(view)` (tiles of which scheme, at what priority), `decode(id,
bytes)` (off the main thread, writing final buffers), `prepare(frame)` (offscreen work), `drape(tile,
target)` (for layers that paint the ground), `draw(pass)` (for layers drawn in 3D), `labels(view)`,
`pick(point)`, `bytes()` and `trim(level)`. A layer's look is a typed struct set by `app/state`;
changing a field marks what must be redrawn (a drape, a buffer, nothing). Layers do not know each
other; what they share (the ground's height, the land's parameters for the polar cap) they read
from `terrain` or from the state.

### 5.2 From a tile request to pixels

1. The camera changes. `View` is rebuilt once. `tiles/cover` computes the cover for each distinct
   tile scheme in use (tile size, zoom range, zoom-by-area, detail), each once.
2. Each layer's `Pyramid` compares the cover with what it holds: tiles to request, tiles to keep
   as stand-ins (a loaded ancestor or descendants), tiles to retire.
3. Requests go to one fetch queue, nearest the camera first, those the view has left cancelled.
   The response cache is the platform's (HTTP cache); each URL carries its pack's version, so a
   cached tile is never stale.
4. A decode job runs on a worker thread: parse, tessellate or convert, and write straight into
   buffers obtained from the GPU layer. Nothing is copied to hand it over.
5. At the next frame's start the tile is committed to its pyramid: the `Store` adds its bytes;
   drapes it touches are marked to be drawn again (paced while the camera moves, as today).
6. The frame: **prepare** (layers' offscreen work: the roads' projection pass, the dots' heights),
   **drape** (dirty terrain tiles: land, water, depth, areas, draped lines, in a fixed order, into
   that tile's texture), **depth** (the terrain's depth), **scene** (terrain mesh with its drapes;
   buildings; the far field against the scene's depth; lines; dots), **labels**, **overlay**,
   present.
7. After the frame, off the main thread: label placement for the new view; the store trims to
   its budget.

### 5.3 Threads

- **Main:** input, camera, app state and UI, encoding the frame.
- **Decode pool:** a few threads shared by every layer (today: 6 MapLibre workers and up to 20 of
  the app's, each with a heap and private caches).
- **Label placement:** one background task per settled or slowly changing view, working on a
  snapshot of the candidates and the view; the result is swapped in at a frame boundary (today:
  on the main thread, in slices).
- **Network:** the platform's.
- Shared between threads: immutable tile payloads, and buffers handed over exactly once. No
  locks on the draw path.

### 5.4 Memory: who owns it, and the budget

The audit's findings are the rules:

- **One owner counts everything.** Every tile payload (CPU and GPU bytes) is in the `Store`.
  Three classes: in view (kept, but see the next rule), standing in, off screen (least recently
  wanted freed first). One budget for the process, set from the device's memory, cut on the
  system's memory warning. No cache is sized by a count of tiles (a4, a5, a7, b4).
- **Never finer than the screen samples.** A layer does not hold or upload a level of detail the
  current view cannot show: the far field's fine mips at world zooms (a2), the roads' vertices
  below the level of detail in use (a3). This is a rule of `Pyramid`, not of each layer.
- **One copy of geometry.** A buffer the GPU draws is the buffer the pick and the statistics read
  (b1). Decode writes into it (b3).
- **Formats by content.** Heights as 16-bit textures (b5); a CPU copy only of the DEM that is
  queried (the terrain's), border rows for the others (a8); a tile that is one value is one
  shared texel (a9); water as coverage shares (a11); vertex streams bound by need (b10).
- **Targets shared.** One accumulation target for the layers that use one in turn; depth
  attachments that are never read are memoryless (a12, b9).
- The drapes stay uncompressed and are freed with their tiles.

### 5.5 Camera

The state is a pose. Moves are rigid motions about an anchor on the ground under the cursor or
fingers (dolly, orbit, pan), as `camera3d.ts` computes them today, applied directly. Limits
(pitch, zoom range, clearance over the terrain, the pole clamps) are applied to the pose in one
place. Centre, zoom, bearing and pitch are derived for settings profiles, readouts and the
server's queries, and a pose can be made from them. The globe and the flat map are two `View`
matrices with the blend between them by zoom; layers get both through one projection uniform
block.

### 5.6 Picking

`pick(point)` asks the layers front to back and returns typed hits: a road or rail line (the
roads' grid over the drawn vertices), a dot (the dots' own test, as drawn), a building footprint,
a vector feature (the tile's feature index), a marker. Readouts under the cursor (elevation,
depth, region) are queries on `terrain` and the layers, not picks.

### 5.7 Labels

One engine for every label on the map. Layers offer candidates: an anchor in world coordinates
(or a line), text, a look, a priority, and optionally an opacity the layer animates (the landmark
names). The engine shapes them (cached by text and look), places them in priority order against
one collision grid, fades them in and out, and draws them standing on the terrain or the water's
surface. Because anchors are world coordinates, a label past 85° is like any other.

### 5.8 What Apple's platforms make simpler, and what they do not

| Simpler | Why |
|---|---|
| Threads | Shared memory: decode writes the buffer that is drawn; no transfer protocol, no per-worker caches, no 25 heaps. |
| Text | Core Text shapes every script, with bidirectional text and fallback fonts, and rasterises glyphs; the glyph-range fetches, the PBF parser, TinySDF, the slice scheduler, bidi-js and the Arabic tables are not ported. The atlas is keyed by font and glyph id. |
| Gestures | Pinch, rotate, pan, tap and long-press recognizers on touch; scroll and magnify events with phases on the Mac. The wheel-or-trackpad classifier and the two-finger state machine are not ported. |
| GPU | Metal has no hidden state to cache or restore, no lost context, no link-status polling; shared-storage buffers, memoryless attachments, 16-bit and compressed formats; compute passes in place of transform feedback. |
| Network and images | URLSession's cache, priorities and cancellation; ImageIO decodes PNG and WebP exactly off the main thread. Host sharding, retry scheduling and the Safari PNG path are not ported. |
| Memory | Explicit lifetimes, the available-memory query, the pressure warning. |
| The shell | No service worker, link parsing, CSS, panel resizing code. |

| Not simpler | Why |
|---|---|
| Globe and terrain maths | The projection blend, subdivision near the poles, precision tricks (positions as flat plus a globe difference), elevation queries, the pole ground: the same code in another language. |
| Tile covers | Every rule exists for a reason found by measurement; all carry over. |
| Drapes | Sizing by footprint, detail, pacing while moving, stand-ins, fingerprints. |
| Label placement | Collision, cross-tile continuity, line labels on a curved, tilted ground. Core Text gives glyphs, not placement. |
| Tessellation | Lines with joins and caps, polygons, extrusions. |
| The roads' renderer, the far field's march, the dots | The app's own, about 11,000 lines; shaders are rewritten in the new shading language, and the two transform-feedback passes (the roads' projection, the dots' heights) are redesigned as compute or vertex work. |
| The UI | About as much code as today, in SwiftUI. |

### 5.9 The renderer's language: the choice a trial decides

Both fit §5.1 unchanged; the module boundaries that touch the platform (text, surface, input,
fetch and image decode) are narrow in either.

| | Swift and Metal throughout | A Rust core on wgpu, a thin Swift shell |
|---|---|---|
| **The wall** | One language from gesture to pixel: nothing to marshal, no second boundary to grow. | A language boundary between UI and engine. It is not the MapLibre wall (both sides are ours and the boundary is drawn by us), but state, picks, labels' text and callbacks cross it. |
| **Platform features this design counts on** | Direct: shared-storage buffers read in place (b1), memoryless attachments, ASTC, Core Text, the pressure signal, Instruments and the Metal debugger. | Through wgpu's model: buffers are mapped to be read, not permanently shared, so "one copy of geometry" needs a kept CPU copy for picks or backend-specific code; memoryless attachments and some formats depend on what wgpu exposes. Core Text through bindings. |
| **Tile formats** | Decoders written again in Swift (as the TypeScript ones are today), or the Rust codecs linked as a library. | The server's own crates decode (`codecs`, `roadcore`, the shading and field codecs): one definition of each format, encoder and decoder tested together. |
| **Agents' loop** | `swift build` and `swift test` or `xcodebuild`; offscreen Metal rendering in tests on the Macs. New parts for `check.sh`, `affected-tests.sh` and the test guard; the status app (`tools/status`) is the only Swift in the repo today. | `cargo test` with offscreen wgpu rendering; `check.sh`, the shared target folder and the guards work as they are. |
| **Shaders** | Metal Shading Language. | WGSL (translated to Metal by wgpu). |
| **Risk** | Tooling and test infrastructure are new here; no reuse of the Rust side. | A dependency that sits between the engine and the GPU and changes often; two build systems in one app; features of Metal reached late or not at all. |

## 6. Sizing

Estimates but for the "today" columns. "Web deletions" is the web app after §8's W steps;
"Subsumed in TypeScript" is where option A would end; "Native" is the port's size, in either
language.

### 6.1 What came from MapLibre

| Subsystem | Today | After web deletions | Subsumed in TypeScript | Native |
|---|---:|---:|---:|---:|
| Gestures | 4,043 | 250 | 150 | 0 |
| Camera and transforms | 7,921 | 7,600 | 3,800 | 3,200 |
| Map shell and utilities | 8,620 | 6,900 | 1,200 | 500 |
| Controls, markers, popups | 3,924 | 100 | 100 | 0 |
| Tile covers | 1,116 | 1,116 | 1,000 | 1,000 |
| Projection, globe, terrain, drapes | 5,082 | 5,082 | 4,300 | 4,000 |
| Tile sources, loading, caches, feature queries | 7,091 | 6,900 | 3,500 | 2,600 |
| GeoJSON, image, video, canvas sources | 2,543 | 2,100 | 500 | 400 |
| Style, properties, atlases | 9,199 | 7,700 | 1,200 | 1,000 |
| Buckets and tessellation | 4,864 | 4,750 | 2,000 | 2,000 |
| Symbols, glyphs, placement | 9,257 | 8,600 | 6,000 | 4,200 |
| Render passes, programs, shaders | 10,378 | 7,400 | 4,000 | 3,500 |
| WebGL wrapper (natively: GPU helpers) | 2,262 | 2,262 | 1,500 | 500 |
| Workers and transfer (natively: the job pool) | 2,021 | 1,900 | 700 | 300 |
| Test helpers, benches | 1,367 | 1,367 | 400 | — |
| **MapLibre's source** | **79,688** | **64,000** | **30,400** | **23,200** |
| The style-spec package | 17,724 | 17,724 | 300 | 200 |

### 6.2 The app's own code

"After decided deletions" takes out #1203, #1204, #1209, #1210 and #1212.

| Group (`web/src`) | Today | After decided deletions | Subsumed in TypeScript | Native |
|---|---:|---:|---:|---:|
| Files the decided deletions remove whole | 1,854 | 0 | 0 | 0 |
| Panels, controllers, popups, formatting (`ui/`, `ctl/`, …) | 10,876 | 10,700 | 10,700 | 8,000 |
| State, config, API, `main.ts` | 4,257 | 3,300 | 3,000 | 2,500 |
| Camera moves and gestures | 1,908 | 1,908 | 1,900 | 1,400 |
| Roads and rail | 4,408 | 4,250 | 4,300 | 3,500 |
| Buildings, near and far | 3,401 | 3,300 | 3,200 | 3,000 |
| Contours | 1,642 | 1,642 | 1,400 | 1,100 |
| Polar caps | 1,596 | 1,596 | 1,100 | 1,000 |
| Landmarks, dots, stations | 3,510 | 3,510 | 2,900 | 2,600 |
| Water, depth, coast, trees, the terrain's look | 2,520 | 2,450 | 2,300 | 1,900 |
| Basemap, ferries, regions, vector-tile coding | 2,532 | 2,532 | 2,300 | 1,900 |
| Plumbing over MapLibre | 751 | 700 | 150 | 0 |
| **The app** | **39,255** | **35,900** | **33,250** | **26,900** |

### 6.3 Totals

| | Today | Web after W steps (the pick) | Subsumed in TypeScript (A) | Native |
|---|---:|---:|---:|---:|
| MapLibre's source + the app | 118,900 | 99,900 | 63,650 | 50,100 |
| with the style-spec package | 136,700 | 117,600 | 63,950 | 50,300 |

Tests are apart: 62,500 lines of MapLibre unit tests today, most of them of machinery that is
deleted or reshaped; the native tests are written with each step (§8).

## 7. The order of work

Three ways to get there:

- **(A) Subsume in the web app first, then port.** Reshape the TypeScript until it is §5, verify
  each step in Chrome and Safari, then translate one coherent design.
- **(B) Web only where it lowers risk; build §5 natively.**
- **(C) In between:** (B), plus removing the wall in the web app where that is deletion rather
  than redesign.

What weighs on it:

| | (A) | (B) / (C) |
|---|---|---|
| **Code written and thrown away** | About 45,000 lines of MapLibre reshaped into 30,000 of TypeScript, and the app's layers moved onto them, all discarded when native replaces web. The same design is then written again natively. | (B): little. (C): the deletions are thrown away too, but a deletion costs a fraction of a reshape. |
| **What the web can prove** | Behaviour, cheaply: Chrome and Safari, the harnesses. But the properties §5 is for cannot exist there: workers do not share memory, text is still glyph ranges and TinySDF, the GPU is still WebGL behind a browser's process. The TypeScript end state would be coherent, and still not the native design. | The native engine is proven against reference views and numbers taken from the web app (§8, W4), layer by layer. |
| **The tests' reach** | MapLibre's render suite is written as style documents. The first reshape (layers in code) deletes the code that reads them, so the suite stops applying exactly where the risky work starts; the unit tests of each reshaped module are rewritten with it. What remains is the app's own harnesses and eyes. | The suite and unit tests keep applying to everything the W steps leave, because those only delete whole features. |
| **The live map meanwhile** | Months of deep changes to the map the owner uses daily, for an end state that is retired. | The web app changes little after the W steps and stays a stable reference. |
| **Risk moved earlier** | Design mistakes show up where fixing them is cheap. | Design mistakes show up natively. Mitigated by building in thin vertical slices (§8: a camera, a terrain, one drape, one layer, on screen and in an offscreen test, before breadth). |
| **Agents do the work** | The cost is not typing; it is review, verification and the owner's attention across two full passes over the hardest code. | One full pass. |

**The pick: (C).** In the web app: delete what the map does not use of MapLibre; move the served
formats the native design wants; record the oracle. Then build §5 once, natively. The owner's
aim, that the port translate one coherent design rather than "MapLibre plus our app", is met by
this document and §5 being that design, and by the web deletions making what remains of MapLibre
exactly what the map uses, so that an agent porting a subsystem reads only what must be carried
over. The parts of the subsuming that are redesign (layers in code, one pyramid and budget, one
label engine, the camera as a pose, threads) are done where they can be finished.

The case for (A) is real if the owner expects the web app to live on beside the native one for
long: then the reshaped TypeScript is not thrown away, and (A)'s cost buys a smaller, coherent web
app too. Under "native replaces web" it does not pay.

## 8. Steps for (C)

Each lands alone with the tests passing. "Reference views" are W4's, taken once before W1 and
compared after each W step.

### In the web app

| Step | What | Verified by | Lines |
|---|---|---|---:|
| **W0** | A reference set: about 20 views (city, mountains, globe, coast, a pole, labels in several scripts; each in the Default profile and one scenic-colour profile), captured from the web app by headless Chrome at 2× to files, with a script that takes them again and compares within a tolerance. | Two runs on the same build agree. | + small tool |
| **W1** | Delete MapLibre's gestures, controls, markers, location hash and locale; the popup becomes the app's own element; pointer events stay. | Typecheck, build, the remaining unit tests, reference views; gestures by hand in Chrome and Safari (the app's own were already the only ones in use). | −9,000 |
| **W2** | Delete unused sources and formats (video, canvas, MLT, TileJSON), layer types and their shaders (heatmap; sky and atmosphere with #1209), patterns, sprites and the image manager, line gradients, icon and text-and-icon symbols, variable anchors, vertical text, the RTL plugin, world copies, the debug draws. | As W1, plus MapLibre's render suite for the kept layer types (the deleted types' cases removed). | −6,500 |
| **W3** | Served formats the native design depends on, the web app reading them first: a flag for tiles of one value (a9); the water as coverage shares (a11). Each is a server change with both forms served until the web app has switched. | The server's tests; reference views unchanged; the audit's stops measured again. | server + small |
| **W4** | The oracle's numbers: a script that dumps, for each reference view, the camera's matrices, the tile cover of each scheme, a grid of terrain heights, and the placed labels with their boxes, as JSON beside the images. | Two runs agree. | + small tool |

W1 and W2 are independent of the native work and of each other. The audit's web-only cuts (a1 to
a8, a10, a12) are not steps here: they improve an app that is to be retired; whether any is worth
doing for the months in between is the owner's call (§9).

### Native

| Step | What | Verified by |
|---|---|---|
| **N0** | The trial that settles §5.9: the same slice built both ways (camera and view, the globe-to-flat blend, a terrain mesh from DEM tiles, one drape, the roads' sprite draw for a few tiles), rendered offscreen in a test and on a device. | W4's matrices and covers reproduced; the slice's images against W0's views with the other layers off; build and test times, memory, and the points of §5.9 written up for the owner. |
| **N1** | `geo`, `camera`, `tiles/cover`: the maths, with the limits and anchored moves. | Unit tests from W4's numbers (matrices, project and unproject, covers) and ported from MapLibre's and the fork's transform and cover tests. |
| **N2** | `tiles` (pyramid, store, fetch, jobs), `terrain` (DEM, heights, mesh, drapes with sizing and pacing), `frame`; `layers/land` and `layers/water`. | Heights against W4's grids; reference views with only land and water; the byte budget held in a scripted tour (the audit's tour). |
| **N3** | `input` on macOS and touch; the app shell with the server's catalogue and settings profiles read. | Gestures by hand on a Mac, an iPad and an iPhone; anchored moves keep the ground point under the cursor in tests. |
| **N4** | `layers/lines` (roads, rail) with `pick`; then `layers/vector` (fills, draped lines, areas) and `layers/overlay`. | Reference views; pick tests on fixed tiles; frame times at the globe and in a city. |
| **N5** | `labels` and `layers/names`; `layers/dots` with landmark names. | W4's placed labels compared as sets (same labels shown, boxes within a tolerance; glyph shapes differ by design); the script samples. |
| **N6** | `layers/buildings`: near meshes, then the far field. | Reference views in cities at several zooms and tilts; the hand-over zoom; memory at the globe. |
| **N7** | `layers/depth`, `layers/contours`, `layers/polar`, ferries, the viewshed. | Reference views; the polar tests' numbers. |
| **N8** | The app: panels, colour scales, lists, search, the elevation profile, popups, credits, the regions panel and downloads. Can run beside N2 to N7 against the server's API. | UI tests of the state and controllers; by hand. |
| **N9** | Memory under pressure, performance, the three platforms' fit and finish. | The audit's tour within the budget on each device class; frame-time targets. |
| **N10** | The switch: the web map app, `web/maplibre` and their tools deleted; the server's web-only routes with them. | The owner's decision. |

### Risks

- **Label parity.** Placement is the least testable part and the most noticed. N5 compares sets,
  not pixels, and keeps the placement algorithm's structure.
- **Shader ports.** The far field's march and the roads' sprite coverage are long shaders tuned by
  measurement; transform feedback has no direct equivalent. N0 includes the roads' sprite draw for
  this reason.
- **Phone memory.** The web app holds 1.6 to 12 GB; an iPhone allows a small fraction. §5.4's
  budget is not an optimisation there but the condition for running at all; N2 proves it early.
- **Two apps at once.** From N2 to N10 a server or format change must suit both. Freezing the web
  app's features for that time keeps it cheap (§9).
- **Knowledge in the fork.** Each change in `FORK.md` records a measured problem. The port works
  from that list, not from upstream's behaviour; a change not carried over is a decision, written
  down.
- **Test infrastructure.** Offscreen GPU tests need a Mac with a GPU session; where they run and
  how `check.sh` includes them is part of N0.
- **Licences.** The engine carries MapLibre's maths (BSD-3-Clause) and the contour code
  (maplibre-contour, BSD-3-Clause): their notices stay in the app's credits.

## 9. For the owner to decide

1. **The order:** (A), (B) or (C). This document picks (C).
2. **The web app until the switch:** frozen but for W1 to W4 and fixes, or still developed (then
   which of the audit's web-only memory cuts are worth doing meanwhile).
3. **Basemap lines:** stay draped (as now), or drawn by the roads' renderer, sharp at any tilt.
   A look change; it can be tried natively in N4 without committing.
4. **Text:** fonts shipped in the app and shaped by Core Text (slightly different glyphs, correct
   Arabic and Hebrew), or the server's glyph ranges kept for an identical look.
5. **Devices:** the oldest iPhone, iPad and Mac to support; it sets the memory budget and which
   GPU features the engine may assume.
6. **Region downloads on a phone or tablet:** the native app only drives the Mac's mirror as the
   web panel does, or keeps downloads of its own on the device.
7. **Settings profiles:** shared between the web app and the native one while both exist (one
   schema, kept compatible), or separate.
8. **Where the reference views and numbers live** (they are too large for the repo: the NAS, as an
   inputs-like folder with its own check, is the natural place).
