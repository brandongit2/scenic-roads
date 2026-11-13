# One engine: MapLibre subsumed into the web app, then the native map app

**Status: planned (tasks #1196, #1246; board folder "Native map app port").** Nothing here is
built. The map is the web app (`web/src`) drawn by our MapLibre (`web/maplibre`, `FORK.md`); until
a step of §8 lands, that code is what is true. Sections 2 and 6 count the code as it is;
everything else is a plan. The owner decided the order of work and most of the open questions on
12 Nov (§7.1, §9.1); what is still open is in §9.2. Line counts are of source files without tests
(§2.1 says how to count them again); every "after" figure is an estimate made by going down the
file lists, good to about a quarter either way.

## 1. Summary

- **What is planned, in two parts.** First the web app is reshaped until MapLibre is no longer a
  thing in it: one engine, in one module tree, written and tested as one program (§7, steps S0 to
  S13). Only when that is finished and checked off (§7.9) does the second part start: a native
  iOS, iPadOS and macOS app that is a translation of that finished web app (steps N0 to N10). The
  native port never has to deal with MapLibre: by then there is none.
- **What "subsumed" means.** Today the map is two programs: MapLibre (79,700 lines, plus the
  17,700-line style-spec interpreter it runs) that owns the camera, the tiles, the terrain, the
  labels and the frame, and the app (39,300 lines) that describes its base layers to MapLibre as
  style JSON and draws its own layers (roads, the far buildings, dots, contours, the polar caps)
  as "custom layers" that reach into MapLibre's internals for the camera's matrices, the tile
  covers, the terrain's textures and the glyphs. Subsumed, there is one engine with one of each
  thing: one camera, one tile pyramid and byte budget, one terrain service, one frame, one label
  engine, one pick, and a set of layers that are all the same kind of object. The base layers are
  defined in code like the roads; nothing is interpreted from a style document; no layer is
  "custom" (§3, §5).
- **It is structural too.** `web/maplibre` goes. The code that came from MapLibre lives in the
  same directories as ours (§5.1), under one naming and code style (file names, identifiers,
  comments, one formatter that `check.sh` enforces), one `package.json` and lockfile, one
  `tsconfig.json`, one build with no alias, one test suite, one set of types, one module for each
  job (no MapLibre helper beside an app helper doing the same). No `FORK.md`, no `Scenic Roads:`
  markers, no comment or name that says "MapLibre", "upstream" or "the fork": in the end nothing
  at the level of the code says which lines came from where. The one thing kept is what
  MapLibre's BSD-3-Clause licence requires, its notice, in one named file and in the app's credits
  (§7.6).
- **The modules that exist only because of the wall go with it** (§2.3): the invisible circle
  layers under the dots, `namefade.ts`, the vector-tile encoders that feed `addProtocol`, the
  polar labels' 2D canvas, `covers.ts`, `placement.ts`, `maptasks.ts`, and the casts
  into internals.
- **How each step is verified (§7.4).** A reference set recorded first (S0): views as images and
  as numbers, each with the tile and API responses it read, so a view can be drawn again from the
  same bytes. MapLibre's unit tests move with the code into the app's one suite, run by
  `check.sh`. MapLibre's own render suite is not used: its cases are style documents, and the
  style document is one of the things deleted.
- **How much code.** 118,900 lines today (136,700 with the style-spec package); about 64,000 when
  the subsuming is done; about 50,000 native (§6).
- **The web app is not frozen meanwhile.** It keeps being developed, so the steps are ordinary
  board tasks that land on main one at a time beside feature work and the decided removals (§7.3).
- **What this order costs:** the hard half is written twice, once reshaped in TypeScript and once
  translated to native (§7.2 states it with the figures).
- **Before any of it:** task #1245 makes labels in Devanagari and the other Indic scripts, Khmer
  and Myanmar shape correctly. The plan assumes a correct text path going in.
- **Native work runs on the M1 (§8.2).** Xcode cannot be installed on the M4; the M1 and the mini
  have it. The native part of `check.sh`, agents' native builds and simulator runs go to the M1;
  the engine's macOS cargo tests run on the M4 with the other Rust tests.
- **The renderer (§5.9), the owner's preference:** the engine and the renderer in Rust, calling
  Apple's GPU API, Metal, directly through Rust bindings (`objc2-metal`), with shaders in the
  Metal Shading Language and no wgpu; Swift only as a thin shell for the window, the SwiftUI
  panels and gestures. The fallback is a Rust core on wgpu. The case for the preference is owning
  the GPU layer and never waiting on wgpu, not that wgpu cannot reach HDR (§5.9). The trial N0
  (proposed, §8.3) builds everything above the GPU once and the GPU layer twice, once on Metal
  directly and once on wgpu, behind one interface, and chooses direct Metal unless a stated rule
  says otherwise; each variant proves HDR output on macOS and iOS and an offscreen-rendered test
  an agent runs with cargo.
- **Native text:** our own shaping and glyph path, translated, for the scripts written a
  character at a time. For CJK and the scripts that need shaping, the web app as #1245 leaves it
  has no path of its own to translate (the browser's text engine draws them from the device's
  fonts); natively the owner chose Core Text, the system's text engine, for those scripts if
  calling it from Rust is at least fairly ergonomic, otherwise a shaper of our own with fonts we
  ship (§9.1); a small probe in N0 finds out (§8.3).

### 1.1 Apple terms used here

- **Xcode:** Apple's IDE and toolchain; `xcodebuild` is its command line. The **Command Line
  Tools** are a smaller install (compilers, the macOS SDK) without the iOS SDKs, the simulators
  or app packaging.
- **Simulator:** an iPhone or iPad running as a Mac process, driven by `xcrun simctl` (install,
  launch, screenshot). It runs the app's Metal code on the Mac's GPU.
- **SwiftUI:** the declarative UI framework (the native counterpart of a component framework);
  **UIKit** (iPhone, iPad) and **AppKit** (Mac) are the older frameworks under it, where the
  **gesture recognizers** (pinch, pan, rotate) and the Mac's trackpad events live.
- **Metal:** the GPU API (the counterpart of WebGL/WebGPU); shaders are written in the **Metal
  Shading Language** (MSL), a C++-based language that the system compiles from source at run
  time, or Xcode's Metal toolchain ahead of time into a library file (`.metallib`). Metal's **API
  validation** (turned on by the `MTL_DEBUG_LAYER` setting) checks each call in debug runs; a
  misuse it does not catch is undefined behaviour. **Shared storage:** on Apple Silicon the CPU
  and GPU see the same memory, so a buffer can be written by the CPU and drawn without a copy. A **memoryless**
  attachment is a render target that lives only in the GPU's tile memory during a pass (no RAM).
  **ASTC** is a compressed texture format the GPU samples directly. A **compute pass** runs
  shader code that is not drawing (the replacement for WebGL's transform feedback). **Tile
  shaders** run compute work inside a render pass, on that tile memory. **MetalFX** is Apple's
  upscaler: the frame is drawn at fewer pixels and shown sharp at full size. A **no-copy
  buffer** is a Metal buffer made over memory our code already allocated, so the GPU draws the
  very bytes the CPU wrote. **Metal 4** is the newest generation of the API (its own command
  queue, allocators and barriers); the classic API (`MTLCommandQueue`) stays beside it. A **GPU
  frame capture** records every Metal call and resource of one frame into a file that Xcode's
  Metal debugger replays.
- **objc2:** a family of Rust crates that expose Apple's frameworks to Rust as bindings generated
  from Apple's headers: `objc2-metal` for Metal, `objc2-quartz-core` for the layer a frame is
  shown in (`CAMetalLayer`), `objc2-app-kit` and `objc2-ui-kit` for the Mac's and the iPhone's
  windows and screens. A call through them is a Rust call into the framework; many are `unsafe`
  (Rust's marker for code whose rules the compiler cannot check: the framework's rules are the
  caller's to keep). In debug builds objc2 also checks at run time that each method it calls
  exists with the declared types.
- **C ABI, cbindgen:** the plain C calling convention two languages can share; `cbindgen`
  generates a C header from Rust's `extern "C"` functions and structs, which Swift imports.
- **Core Text:** the system's text engine: it shapes text in every script (joins, ligatures,
  reordering, bidirectional runs) and draws glyphs. **ImageIO** decodes images; **URLSession** is
  the HTTP client with its own cache.
- **Instruments:** the profiler (memory, GPU, time). **TestFlight:** Apple's channel for
  installing test builds on one's own devices; it and installs lasting more than 7 days need a
  paid Apple Developer Program membership.
- **wgpu:** a Rust implementation of the WebGPU API that runs on Metal on Apple platforms, and on
  Vulkan, DirectX and in browsers elsewhere; it checks every call and tracks what each GPU
  resource is used by, so a misuse is an error rather than undefined behaviour. Its shaders are
  **WGSL**, translated to each platform's shading language. **wgpu-hal** is its lower layer,
  through which the Metal objects under a wgpu device, texture or surface can be reached for what
  wgpu itself does not offer.
- **HDR (EDR) output:** Apple's extended dynamic range. A Metal layer (`CAMetalLayer`) flagged
  for extended range, with a 16-bit float pixel format and an extended-range colour space, shows
  values above 1.0 brighter than white, up to the display's current *headroom*.

## 2. What the app uses of MapLibre today

### 2.1 How the lines are counted

`web/maplibre/src`: every `.ts` and `.glsl` file that is not a `*.test.ts`, each file in exactly
one row of the table below by its path (the "Files" column; a file matching several rows goes to
the first that lists it). 79,688 lines in 414 files (`wc -l` gives 79,663: 25 files end without a
newline, and their last line is counted here). The tests are 62,538 lines in 227 files, plus the
render suite (`test/integration/render`: our 5 cases in the repo, upstream's fetched by
`tools/fetch-upstream-tests.sh`, `FORK.md`). The app: `web/src/**/*.ts` without tests, 39,255
lines. Dependencies are counted from their `src/` in `web/node_modules`.

### 2.2 By subsystem

"Uses" is what the app's code and style actually exercise; "reaches in" is where the app reads or
writes what is not API (`web/package.json`'s note lists the main ones; 146 lines of `web/src`
hold an `as unknown as` or `as any` cast, most of them into MapLibre's internals).

| Subsystem | Lines | Files | The app uses | The app does not use | Reaches in |
|---|---:|---|---|---|---|
| **Gestures** | 4,043 | `ui/handler/`, `ui/handler_manager.ts`, `ui/handler_inertia.ts` | The pointer events behind `map.on('click' …)`. | Every gesture: `trackpad.ts` disables scroll zoom, double-click zoom, drag pan, drag rotate, touch zoom, rotate and pitch and does them itself, anchored on the terrain under the cursor (`camera3d.ts`). Box zoom, keyboard, cooperative gestures, inertia. | — |
| **Camera and transforms** | 7,921 | `ui/camera.ts`, `geo/transform_*`, `geo/projection/*_transform.ts`, `*_camera_helper.ts`, `geo/lng_lat*`, `geo/bounds.ts`, `geo/edge_insets.ts`, `geo/mercator_coordinate.ts`, `util/primitives/` | `jumpTo` (25 calls: every gesture, glide and flight is the app's own, set a frame at a time), `flyTo` (4), `fitBounds`, `cameraForBounds`, `project`/`unproject`, the two transforms and their blend (globe zoomed out, Mercator zoomed in), frustum and horizon. | World copies (`renderWorldCopies: false`), roll, most of `easeTo`'s options, the pluggable transform interface (there are two transforms and always will be). | `camera3d.ts`, `covers.ts`, `terrain.ts` read `transform._mercatorTransform`, `_verticalPerspectiveTransform`, `_cameraPosition`, `_pixelMatrixInverse`, `_globeViewProjMatrixF`; `roads/layer.ts`, `dots.ts`, `contours.ts` reach the transform through `map._camera`; `main.ts` calls `_elevateCameraIfInsideTerrain`, `_updateElevation`. |
| **Map shell and utilities** | 8,620 | `ui/map.ts` (4,841), `ui/events.ts`, `ui/hash.ts`, `index.ts`, `util/util.ts`, `util/evented.ts`, `util/dom.ts`, `util/browser.ts`, small `util/` | The canvas, resize, the frame loop, `idle`, `isMoving`, about 60 of `Map`'s methods (most often `getLayer`, `setPaintProperty`, `getZoom`, `triggerRepaint`, `on`). | The location hash, locales, most of the event types, the long tail of `Map`'s API. | `map._loaded`, `map.style`, `map.terrain`. |
| **Controls, markers, popups** | 3,924 | `ui/control/`, `ui/marker.ts`, `ui/popup.ts`, `util/geolocation_support.ts` | `Popup`, once (`popup.ts`). | Every control (`attributionControl: false`, no `addControl`), `Marker` (the app's markers are a GeoJSON source), geolocation. | — |
| **Tile covers** | 1,116 | `geo/projection/*covering_tiles*`, `util/primitives/bounding_volume_cache.ts` | All of it, with the fork's zoom by depth, elevation, latitude and area (`FORK.md`, "Tile covers over 3D terrain"). | — | `covers.ts` replaces the tile-zoom function and caches frustum tests from outside. |
| **Projection, globe, terrain, drapes** | 5,082 | `geo/projection/*projection*.ts`, `globe_utils.ts`, `pole_caps.ts`, `render/terrain.ts`, `render/subdivision*.ts`, `render/fill_large_mesh_arrays.ts`, `tile/terrain_tile_manager.ts`, `data/dem_data.ts`, `util/create_tile_mesh.ts`, `webgl/render_to_texture.ts`, `rtt_fingerprint.ts`, the terrain's and projection's uniform buffers | Nearly all: the globe, the 3D terrain with its elevation queries, the drapes with the fork's sizing and pacing, the pole caps and pole ground. | The projection factory (the map is always "globe"). | `main.ts` writes `map.terrain.exaggeration` each frame of the opening rise; `bldfield.ts`, `polarglobe.ts`, `dots.ts`, `contours.ts` read the terrain's tile manager, DEM tiles and mesh cache. |
| **Tile sources, loading, caches, feature queries** | 7,091 | `tile/` (but the terrain's), `source/source*.ts`, `raster_*`, `vector_tile_*`, `query_features.ts`, `load_tilejson.ts`, `protocol_crud.ts`, `util/ajax.ts`, `util/image*.ts`, `util/png_rgba*.ts`, `data/feature_index.ts` and its helpers | Vector (9 sources), raster (water), raster-dem (terrain, tint, slope, depth, trees), `addProtocol` (4 uses: tiles made by the app's own workers handed to MapLibre as if fetched), `queryRenderedFeatures`, `querySourceFeatures`, feature state, `refreshTilesWhere`. | TileJSON, MLT tiles, overscaled DEM tiles. | `maptasks.ts`, `terrainstats.ts`, `namefade.ts`, `covers.ts`, `contours.ts`, `buildings.ts` (`keepWhole`: `_zoomLevelsToOverscale`, `_inViewTiles`), `bldfield.ts` read `style.tileManagers`. |
| **GeoJSON, image, video, canvas sources** | 2,543 | `source/geojson_*`, `image_source.ts`, `video_source.ts`, `canvas_source.ts`, `util/geojson_bounds.ts` | GeoJSON (4 sources: ferries, the markers, selections and highlights, the regions' outlines), one image (the viewshed). | Video, canvas, GeoJSON clustering and diffs. | — |
| **Style, properties, atlases** | 9,199 | `style/`, `data/program_configuration.ts`, `render/image_*.ts`, `pattern_atlas.ts`, `line_atlas.ts`, `util/color_ramp.ts` | A style of about 60 layers built in `basemap.ts` (background, fill, line, symbol, circle, fill-extrusion, raster, hillshade, color-relief) and the app's own six kinds of custom layer; `setPaintProperty` (73 call sites), `setLayoutProperty`, `setFilter`; about 40 expression operators; dashes. | Style loading, validation, diffing and migration (the style starts empty and is built by calls), sprites and images (`poiicons.ts` rasterises its own atlas for its own layer), patterns, heatmap, the sky (#1209), light beyond the extrusions' default, transitions. | `polarglobe.ts` reads the hillshade's and the tints' evaluated paint (`paint.get`, `_createColorRamp`); `polarpoints.ts` the label layers' evaluated paint and layout; `details.ts` `style._layers`. |
| **The style-spec package** | 17,724 | `@maplibre/maplibre-gl-style-spec` (a dependency) | The expression parser, type checker and evaluator, filters, colour and interpolation. | Validation, diff, migrate, the reference document, old-style functions. | — |
| **Buckets and tessellation** | 4,864 | `data/bucket*`, `data/segment.ts`, `data/array_types.g.ts`, `util/struct_array.ts`, small helpers | Line, fill (earcut), fill-extrusion, circle and symbol buckets. | Heatmap, patterns, most of the 1,181 generated struct-array types. | `buildings.ts` (`_vectorTileFeature`). |
| **Symbols, glyphs, placement** | 9,257 | `symbol/`, `render/glyph_*.ts`, `font_face_manager.ts`, `source/rtl_text_plugin_*`, `util/script_detection.ts`, `verticalize_punctuation.ts`, `graphemes.ts` | Text labels on points and along lines, wrapping, halos, sort keys, collision and fades, the cross-tile index, server glyph ranges for the scripts written a codepoint at a time; the built-in Arabic shaping (joining forms, lam-alef) and bidirectional reordering (`arabic_shaping.ts`, `bidi.ts` over bidi-js); CJK, and the scripts that need a text engine (the Indic scripts, Thai, Lao, Tibetan, Khmer, Myanmar, Mongolian), cut into units of writing (`graphemes.ts`, `canCombineGraphemes`) and each unit drawn by the browser's text engine, from the device's fonts, into the glyph atlas (`glyph_manager.ts`, TinySDF; web/maplibre/FORK.md, Labels in scripts that need shaping). | Icons (no `icon-image` anywhere), variable anchors, vertical text, the RTL plugin loader (never called: the built-in code replaces it). | `maptasks.ts` (`glyphManager.waiting`), `placement.ts` (`style._updatePlacement`), `polarpoints.ts` (the glyph manager's SDF glyphs). |
| **Render passes, programs, shaders** | 10,378 | `render/painter.ts`, `webgl/draw/`, `webgl/program/`, `shaders/` (4,248 lines of GLSL), `webgl/scene_target.ts` | The offscreen, drape, opaque and translucent passes; the terrain's depth and coordinate targets; the fork's scene depth, lifted layers and water mask; the shaders of the nine layer types in use. | Heatmap, sky and atmosphere, every pattern shader, line gradients, icon and text-and-icon symbols, the debug and collision-debug draws. | `roads/layer.ts` reads `painter._tileClippingMaskIDs`; the custom layers set raw GL state beside MapLibre's cache (`glprog.ts`). |
| **WebGL wrapper** | 2,262 | the rest of `webgl/` | All of it, by MapLibre's own layers; the app's layers call WebGL directly. | — | — |
| **Workers and transfer** | 2,021 | `source/worker*.ts`, `vector_tile_worker_source.ts`, `util/actor*.ts`, `dispatcher.ts`, `web_worker*.ts`, `*worker_pool.ts` | Up to six tile workers (parse, tessellate, shape), the class registry that serialises buckets across threads. The app runs seven worker kinds of its own beside them (road and rail tiles ×12, depth ×2, coast ×2, landmarks, far field, contours, contour DEM), each with its own messages and caches. | Custom worker sources. | — |
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
  layer's paint cannot follow the dots' easing; the landmarks worker encodes its points and the
  depth worker its depth contours as Mapbox vector tiles (`mvt.ts`'s `encodePoints` and
  `encodeLines`), and the contour DEM worker its contour lines (`contour/vtpbf.ts`), so that
  MapLibre will accept them through `addProtocol` (4 uses: `overlays.ts`, `depth.ts`, `coast.ts`,
  `contour/dem.ts`). `mvt.ts`'s readers (`readPolygons`, `readLines`, which the coast, depth and
  contours workers use to decode the server's tiles) are not a workaround: they become the tiles'
  vector-tile reader.
- `polarpoints.ts` and `polarlabels.ts` (589 lines) draw the caps' labels on a 2D canvas over the
  map with glyphs borrowed from the glyph manager, because MapLibre's symbols live in Web Mercator
  tile coordinates.
- `covers.ts`, `placement.ts`, `maptasks.ts` (392 lines) patch or read MapLibre from
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
| Symbols, glyphs, placement | **Keep the core, reshape the edges → `labels`** | Placement, collision, fades, the cross-tile index and line labels stay. The engine takes label candidates in world coordinates from any layer (so the caps' labels and the landmark names are ordinary labels with a per-label opacity) rather than only from symbol buckets of Mercator tiles. Icons, variable anchors, vertical text and the RTL plugin loader go. Natively, our own shaping and glyph path translated (§9.1). |
| Render passes, programs, shaders | **Reshape → `frame`** | A fixed list of passes (§5.2) that every layer takes part in through the same interface; each layer owns its shaders. The painter's per-type dispatch, the custom-layer bridge, and the unused programs go. |
| WebGL wrapper | **Keep on the web, slimmer; not ported** | One small GPU layer used by every layer (today the app's layers bypass it). Natively it is a small layer of our own over Metal, called from Rust (or over wgpu, the fallback, §5.9), with no state cache. |
| Workers and transfer | **Reshape → one job pool** | One pool, one job shape (fetch, decode, write into the final buffer), used by every layer. The actor protocol and class registry go. Natively: threads sharing memory. |

### 3.1 The layers as peers

Every layer implements the same small interface (§5.1): which tiles it wants, how a tile's bytes
become what it draws, what it paints into the drapes, what it draws in 3D, which labels it offers,
what it answers to a pick, how many bytes it holds. None is registered as "custom".

| Layer | Today | As a peer |
|---|---|---|
| **Land: background, elevation tint, hill-shading, trees** | Style layers (`background`, `color-relief`, `hillshade`) over raster-dem sources, with the fork's stored-shading shader; the trees layer is a `color-relief` over its own tiles. | `layers/land`: paints the drapes from the tint, shading and trees tiles. Its parameters (ramp, light, strength) are a struct; the polar cap reads the same struct rather than MapLibre's evaluated paint. |
| **Water, whitewater, coast shading** | A raster layer with `scenic:mipmap`, a color-relief with `scenic:mask`, fills at `sea` level, two coast workers. | `layers/water`: paints the drapes from coverage tiles (two 8-bit shares coloured in the shader, flat tiles as one shared texel: the audit's a9 and a11), the mask a plain second texture of the same layer. |
| **Depth (colour, shading, floor, contours)** | A color-relief and a hillshade over tiles a worker composes and hands in through `addProtocol`; the floor through `setTerrainSurface`. | `layers/depth`: a computed tile source of the common pyramid; the floor is a second surface of the terrain service. |
| **Roads and rail** | A custom layer with its own tile loading, workers, budget, pick grid and a screen-size target each. | `layers/lines`: the same renderer (sprites and quads), its tiles in the common pyramid and budget, its vertices held once (shared with the pick), scenic channels a separate stream bound only for scenic colours (b10), one accumulation target shared. |
| **Basemap lines** (boundaries, waterways, ferries, outlines, highlights) | MapLibre line layers draped into the terrain's textures. | Stay draped at first, tessellated by `layers/vector`. Drawing them with the roads' renderer (sharp at any tilt, one line code path) is a look change left to the owner (§9.2). |
| **Contours** | A custom layer drawing what a MapLibre vector source loads, from tiles two workers make. | `layers/contours`: a computed tile source; drawn by the line renderer; its labels offered to the label engine. |
| **Areas** (parks, heritage, Indigenous lands, special areas) | Fill and line layers. | `layers/vector` fills painted into the drapes. |
| **Buildings, near** | A `fill-extrusion` layer plus `keepWhole` reaching into the tile manager. | `layers/buildings`: extrusion meshes from the z14 tiles (whole-building tiles are its own rule, not a patch). |
| **Buildings, far field** | A custom layer with its own tiles, worker and 1 GB cache target, reading scene depth through a fork hook. | The same module's second representation: the hand-over between meshes and field is decided in one place; tiles in the common budget, never finer than the screen samples (a2). |
| **Dots** (stops, sights, heritage, stations, terminals) | A custom layer for the look plus invisible circle layers for hit-testing and label anchoring. | `layers/dots`: one point renderer for every dot and circle, with its own pick; its names go to the label engine with the dots' eased opacity. |
| **Place and water names** | Symbol layers over the server's label tiles. | `layers/names`: decodes label tiles into label candidates. |
| **Polar caps** | A custom layer for the cap's terrain; a 2D canvas for its labels, dots and roads. | `layers/polar`: the cap's terrain mesh; its names, dots and roads are ordinary candidates and geometry of the label engine, the dots and the line renderer, which all work in world coordinates. |
| **Overlays** (markers, selection, hover, the selected road's profile cursor, the viewshed) | GeoJSON and image sources with style layers. | `layers/overlay`: shapes tessellated directly; the viewshed's image (from `GET /api/viewshed`) painted into the drapes like any ground layer. |

## 4. What stays true of the product

Everything in `.claude/notes/NATIVE-PORT-FEATURES.md` stays except what is already decided gone
(link and saved-state migrations #1203; the debug and benchmark tools #1204; sky and distance fog
#1209; road slope statistics, "Road above 1,000 m" and Drive #1210; live data updates #1212). Three
carried features that are off in every one of the owner's profiles and rated large to carry by
the inventory are kept, by the owner's decision (§9.1): contour lines (E10), the polar caps (E13)
and the 3D sea floor with depth contours (E12).

The subsuming changes two things a user of the web app could notice:

- The caps' labels, dots and roads collide and fade like every other label (S10, S11).
- Memory is bounded by one budget, so a long session holds what the view needs, not what was seen
  (S9).

Text does not change in the subsuming: labels are drawn as today, by the text path as #1245
leaves it (web/maplibre/FORK.md, Labels in scripts that need shaping). The scripts written a
character at a time (Latin, Greek, Cyrillic, Hebrew, Arabic through its presentation forms and
others) come from the server's glyph ranges; CJK and the scripts that need shaping (the Indic
scripts, Thai, Lao, Tibetan, Khmer, Myanmar, Mongolian) are cut into units of writing by our code
and each unit is drawn by the browser's text engine from the device's fonts. The native app
translates the first half and the cutting under either candidate of §5.9; the drawing of those
units needs a text engine natively too: Core Text if calling it from Rust is at least fairly
ergonomic, otherwise a shaper of our own with fonts we ship (the owner, §9.1; N0's probe, §8.3).

Decided for the native app (§9.1): it supports only the latest major OS release on iPhone, iPad
and Mac; region downloads happen on the device; settings profiles are shared with the web app.

## 5. Target architecture

This is the design both parts end in: the web app when S13 is done, then the native app as its
translation. Where a browser cannot do what a paragraph describes (threads sharing memory, the
system's memory signal, some GPU features), the paragraph says what the web app does instead.

### 5.1 Modules, and the layout of `web/src` when the subsuming is done

One tree, no engine/app wall below the user interface. Each directory holds code we wrote and code
that came from MapLibre side by side, in files named and written the same way. Tests sit beside
what they test (`cover.ts`, `cover.test.ts`). The native app has the same modules (§8.2).

```
web/src/
  main.ts     the entry: builds the engine, the layers and the UI
  geo/        coordinates: lng/lat, Mercator, earth-centred, tile ids, bounds; the globe's and
              the flat map's projection maths and the blend between them
  camera/     Camera (a pose), View (matrices, frustum, horizon, project and unproject), the
              ground-anchored moves (today's camera3d.ts), the animator (flights, glides), limits
  input/      wheel, trackpad and touch gestures to camera moves (today's trackpad.ts,
              twofinger.ts, wheelkind.ts); pointer to pick
  tiles/      cover; Pyramid<T>; Store (the one byte budget); the fetch queue; the job pool
              (workers); readers of the shared wire formats (protobuf, vector tiles)
  terrain/    DEM tiles and their min/max pyramids, heights, surfaces (ground, sea floor, pole),
              the mesh and its subdivision, the drapes with their sizing and pacing
  gpu/        the one WebGL layer every layer uses: context, buffers, textures, programs linked
              in the background, state, render targets
  frame/      the canvas and its resize, the frame loop, "moving" and "settled", the fixed list
              of passes, the scene's depth, the Layer interface and the list of layers
  labels/     text (shaping, bidirectional runs, glyph ranges, the glyph atlas), candidates,
              placement, collision, fades, the cross-tile index, line labels
  pick/       one query across the layers; the vector tiles' feature index
  layers/     one directory a layer, each with its decoder, its worker entry, its shaders, its
              typed look and its draw: land/, water/, depth/, lines/ (roads, rail, basemap lines
              if §9.2 says so), vector/ (fills and draped lines), contours/, buildings/ (near
              meshes and the far field), dots/, names/, polar/, overlay/
  app/        state, links, settings profiles, config, the server's API, the catalogue, regions
              and downloads
  ui/         panels, their controllers (today's ctl/), lists, search, the popup, formatting,
              the stylesheet
  util/       small helpers used across the tree: maths, colour parsing and interpolation,
              typed-array helpers, DOM helpers
  data/       presets and ramps (JSON), the countries' flags (SVG; today's flags/)
```

Beside it in `web/`: one `package.json` and `pnpm-lock.yaml`, one `tsconfig.json` for the source
(`tsconfig.node.json` stays for the config files), `vite.config.ts` with no alias and no shader
plugin, one `vitest.config.ts`, `index.html`, `public/` (with the licence notices, §7.6),
`default-profile.json`, and `buildpage/` (the build page, a separate app this plan does not
otherwise touch; its seven tests, in `web/test` today, move to `buildpage/` beside its modules and
run in the same suite). No `web/maplibre`, no `web/test` (fixtures move beside their tests), no
`index.ts` re-exporting a public API: modules import each other directly.

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
   cached tile is never stale. In the native app a downloaded region's tiles are read from the
   device instead (§9.1).
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
  locks on the draw path. (In Rust, the compiler enforces this: what crosses threads must be
  `Send`, and what threads share must be `Sync`.)
- The GPU runs up to two or three frames behind the CPU (frames in flight). A buffer or texture
  the store frees is released only when the last frame that used it has completed, and a decode
  never writes into a buffer a frame in flight still reads.
- **In the web app** the decode pool is Web Workers: a job's buffers are transferred to the main
  thread (handed over, not copied) and uploaded there, and label placement stays on the main
  thread in slices, as today. Shared memory, background placement and frames in flight are the
  native app's.

### 5.4 Memory: who owns it, and the budget

The audit's findings are the rules:

- **One owner counts everything.** Every tile payload (CPU and GPU bytes) is in the `Store`.
  Three classes: in view (kept, but see the next rule), standing in, off screen (least recently
  wanted freed first). One budget for the process, set from what the system says the app may use
  (`os_proc_available_memory` on iPhone and iPad, the physical memory on a Mac), cut on the
  system's memory warning; the budget and what each layer holds are on screen in the status
  readout, not hidden. No cache is sized by a count of tiles (a4, a5, a7, b4).
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
- **In the web app** the budget is a visible setting (a browser reports neither the memory
  available nor pressure) and there are no memoryless attachments; the other rules hold.

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
| Text | Our own shaping and glyph path, translated (§9.1): the glyph-range fetches, the PBF parser, TinySDF, bidirectional text, the Arabic shaping and #1245's cutting of text into units of writing carry over. Drawing those units for CJK and the scripts that need shaping is the browser's text engine's work in the web app, with no code of ours to translate: natively Core Text if it is fairly ergonomic from Rust, otherwise a shaper and fonts of our own (§9.1, 10; N0's probe). |
| Label placement | Collision, cross-tile continuity, line labels on a curved, tilted ground. |
| Tessellation | Lines with joins and caps, polygons, extrusions. |
| The roads' renderer, the far field's march, the dots | The app's own, about 11,000 lines; shaders are rewritten from GLSL in the Metal Shading Language (in WGSL under the fallback, §5.9), and the two transform-feedback passes (the roads' projection, the dots' heights) are redesigned as compute or vertex work. |
| The UI | About as much code as today, in SwiftUI. |

### 5.9 The renderer: Rust calling Metal directly, and the fallback

The owner's preference (12 Nov, §9.1), "shape 3": the engine **and** the renderer in Rust,
calling Metal directly through the objc2 bindings (`objc2-metal`, the layer through
`objc2-quartz-core`), with shaders in the Metal Shading Language and no wgpu. Swift is only a
thin shell: the window or screen, the SwiftUI panels and the gestures. The research note on what
this entails is `.claude/notes/NATIVE-RUST-METAL.md` (12 Nov); its experiment drew offscreen from
Rust through `objc2-metal` on both Macs and in the iOS 27.0 simulator, and reached each Metal
feature named here. The reasons for the preference:

- wgpu's main value is portability across Metal, Vulkan, DirectX and the browser. An Apple-only
  app whose web version goes away (N10) does not need it.
- **Owning the GPU layer, never waiting on wgpu.** wgpu reaches most of what the app wants on
  Metal: wgpu 30 sets a surface's colour space, Metal's extended-range spaces included, and the
  layer's extended-range flag itself (HDR output); wgpu 28 has memoryless ("transient") textures;
  it accepts shaders written in MSL or precompiled to a `.metallib` ("passthrough"); and it can
  import a Metal buffer made outside it. Still reachable only through raw Metal: tile shaders,
  MetalFX, the Metal 4 API, and creating no-copy buffers over our own memory. What shape 3 buys
  is no dependency between the engine and the GPU that ships breaking releases often (seven,
  wgpu 24 to 30, from January 2025 to July 2026), each Metal feature as soon as the bindings
  have it, those four without an escape hatch, and one shading language that Apple's debugger
  and profiler see directly.
- It keeps the agents' Rust workflow: cargo builds, and offscreen rendering in the tests.
- One language for the whole engine, renderer included: no wall inside it.

Its costs, which he accepts: no wgpu validation and safety (a misuse of Metal is caught only by
Metal's API validation in debug runs, or not at all); calls that are more verbose and partly
`unsafe`; bindings fewer projects use directly than wgpu (Zed's editor draws through them, and
wgpu's own Metal backend is built on them); and a GPU layer of our own (buffers, textures,
pipelines, passes, the frame and its drawable, and the frames-in-flight rule of §5.3, kept in one
place), about 2,000 to 3,500 lines by the note's estimate, plus every shader in MSL.

What shape 3 binds the project to (the note, §0, §1, §4):

- **objc2, essentially one maintainer** (95 of the last 100 commits). Its crates are generated
  from Apple's SDK headers, so they cover the whole Objective-C API, but releases lag the SDK:
  the latest release (0.3.2, October 2025) carries Xcode 26.0.1's headers, while Xcode 27.1's
  are on its main branch, unreleased. Using a new OS's Metal API in its first months means
  pinning a git commit of objc2 (whether cargo fetches its generated crates cleanly that way is
  for N0 to check), or declaring the missing method by hand. Planned: our GPU layer is the only
  module that imports `objc2-metal`, so a breaking release changes one module.
- **`unsafe` at the boundary.** About a fifth of `objc2-metal`'s methods are `unsafe` (574 of
  about 2,900 in 0.3.2, counted in the note), every buffer bind and draw among them, and all of
  MetalFX. Nothing but Apple's validation layers, opt-in by environment variable and asserting
  rather than returning errors, checks GPU-side mistakes (an out-of-bounds read, a buffer
  rewritten while a frame in flight reads it). Planned: the GPU layer's typed wrappers contain
  the `unsafe` calls, and every test runs with validation on.
- **The classic command queue for the main path** (planned): Xcode's GPU profiling and shader
  debugging do not yet work on Metal 4's queue, and the simulator lacks it; Metal 4 is used only
  where a feature needs it, behind a check of the device's support.
- **Apple platforms only, and Xcode at the edges**: no part of the renderer runs elsewhere
  (§9.1, 4 makes that no loss), and the iOS builds, offline shader libraries, the GPU debugger
  and Instruments need Xcode, so the M1 (§8.2).

The fallback is a Rust core on wgpu, with raw Metal through `wgpu-hal` where wgpu lacks
something. Swift and Metal throughout is not a candidate. N0 builds both behind one interface as
§8.3 proposes and applies its rule.

What both candidates share:

- §5.1's modules unchanged; the module boundaries that touch the platform (text, surface, input,
  fetch and image decode) are narrow.
- One language boundary, between the Swift shell and the Rust engine. It is not the MapLibre wall
  (both sides are ours and the boundary is drawn by us), but state, picks, labels' text and
  callbacks cross it, through generated bindings (UniFFI or cbindgen).
- Tile formats: the server's own crates decode (`codecs`, `roadcore`, the shading and field
  codecs): one definition of each format, encoder and decoder tested together.
- Text: our own shaping and glyph path, translated (§9.1), and for the scripts the browser
  draws, Core Text or a shaper of our own as N0's probe finds (§9.1, 10).
- The simulator's Metal differs from a device's (§8.2): features reached through raw Metal under
  either candidate are missing there.
- The agents' loop (§8.2): `cargo test` with offscreen rendering for the engine, on the M4 with
  the other Rust tests; `check.sh`, the shared target folder and the guards work as they are, and
  agents here write Rust daily. The app still needs `xcodebuild` (on the M1) and the Swift shell,
  plus the Rust built for three targets (Mac, iPhone, the simulator) and packed as an
  XCFramework (a bundle of a library for several platforms): two build systems in every app
  build.

| | Rust calling Metal directly (preferred) | A Rust core on wgpu (fallback) |
|---|---|---|
| **GPU features** | Direct: no-copy buffers over our memory read in place (b1), memoryless attachments, ASTC, tile shaders, MetalFX, Metal 4, HDR output; Instruments and the Metal debugger see our own calls. | HDR output (wgpu 30 sets the layer's flag and colour space), memoryless (transient) textures and ASTC through wgpu itself; "one copy of geometry" by creating the no-copy buffer through `objc2-metal` and importing it into wgpu, or a kept CPU copy for picks; tile shaders, MetalFX and Metal 4 only through `wgpu-hal`'s Metal objects. |
| **Safety and validation** | Partly `unsafe` calls; a misuse (a buffer released while a frame in flight reads it, a wrong binding index) is undefined behaviour unless Metal's API validation catches it. Our GPU layer holds the rules; the tests run with validation on. | wgpu checks every call and tracks resource use: a misuse is an error. Code through the hal is as unchecked as direct Metal. |
| **Code** | More verbose calls through bindings generated from Apple's headers; a GPU layer of our own to write and keep correct (estimate above). | wgpu's API, widely used; a dependency between the engine and the GPU that changes its API often. |
| **Shaders** | Metal Shading Language, compiled by the system at run time or ahead of time by Xcode's Metal toolchain. | WGSL, translated to the Metal Shading Language by wgpu; MSL by passthrough where needed. |
| **HDR output** (wanted later, §9.1) | A `CAMetalLayer` with `wantsExtendedDynamicRangeContent`, a 16-bit float pixel format and an extended-range colour space, set by our code through `objc2-quartz-core`; headroom from the screen (`NSScreen`, `UIScreen`). | An `Rgba16Float` surface with an extended-range colour space; wgpu 30 sets the layer's flag; headroom read from the screen as under direct Metal. |
| **Risk** | Correctness of our GPU layer rests on our tests and Metal's validation; objc2's release lag and breaking releases are ours to work round. | Metal-only features through escape hatches; the dependency's API changes. |

N0 proves HDR output on macOS and iOS in each candidate (§8.3).

## 6. Sizing

Every column but "Today" is an estimate, made by going down the file lists, good to about a
quarter either way; the rows are written to the nearest hundred only so that they add up. "After
S2" is the web app once the unused parts are deleted; "Subsumed" is the web app when S13 is done
(§8.1 gives the same sums step by step); "Native" is the port's size, under either candidate of
§5.9. Native line counts say little about effort: the hard part of each module (§5.8) is the same
size in any language.

### 6.1 What came from MapLibre

| Subsystem | Today | After S2 | Subsumed | Native |
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

| Group (`web/src`) | Today | After decided deletions | Subsumed | Native |
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

| | Today | After S2 and the decided deletions | Subsumed | Native (either) |
|---|---:|---:|---:|---:|
| MapLibre's source + the app | 118,900 | about 100,000 | about 64,000 | about 50,000 |
| with the style-spec package | 136,700 | about 118,000 | about 64,000 | about 50,000 |

Tests are apart: 62,500 lines of MapLibre unit tests today, most of them of machinery that is
deleted or reshaped in §8.1's steps; the native tests are translated from what remains (§8.2).

## 7. The order of work

### 7.1 The decision

The owner's, 12 Nov: **subsume fully in the web app first, then port.** The web app is reshaped
until it is §5 and the checklist of §7.9 holds; the native app is then a translation of it. The
subsuming is not a requirement of the port: when the port starts it is behind us. It is done in
the code's structure as well as in its design (§7.6), and it starts after #1245 has made the text
path shape every script the map shows.

### 7.2 What this order buys, and what it costs

- **Buys.** The design first exists where everyone already works: the owner opens it, uses it
  daily and judges it; agents have a fast, proven loop (Chrome and Safari, `check.sh`); design
  mistakes (a pyramid that does not suit the far field, a label engine that flickers) surface
  without being tangled with platform mistakes (a new language, Metal, Xcode). Differences from
  the reference set mean bugs, not platform differences, because they are compared in the same
  browser with the same fonts. The web app gets one memory budget in the months before native
  lands (today it holds 12 GB after a tour). If the native app stalls, the owner still has the
  better web app.
- **Costs.** The hard half is written twice. About 64,000 lines of MapLibre (what S2 leaves) are
  reshaped into about 30,000 lines of TypeScript, with about 21,000 lines newly written or
  reworked on the way (§8.1); then those and the app's 33,000 are translated to about 50,000
  native lines. Agents do the typing both times; what doubles is review, verification and the
  owner's attention on the hardest code. The second pass is cheaper than the first: it translates
  a debugged design with a running reference.
- **What the web app cannot show.** Threads sharing memory, the system's memory signal and Metal's
  shared and memoryless buffers do not exist in a browser, so §5.3's and §5.4's native-only parts
  are first proven in N2, not here.

### 7.3 Beside ongoing work

The web app is not frozen: features, fixes and the decided removals (#1203, #1204, #1209, #1210,
#1212) and the memory audit's cuts keep landing. The steps are built to coexist with that:

- **Every step lands on main alone.** There is no long-lived subsuming branch. Main is always the
  published app, working. A step is an ordinary board task (several for the largest steps, §8.1).
- **The mechanical steps are scripts, not diffs.** S3c, S3d and S4 move, reformat and rename
  across tens of thousands of lines. Each is a script (a rename map of files and identifiers,
  import rewrites, the formatter) committed with the task and run again on that day's main just
  before the merge, so it never has to be rebased by hand. Reformatting (4 spaces to 2, quotes)
  changes nearly every line, which defeats git's rename detection, so a branch open across such a
  step does not rebase onto it: it runs the same script on its own tip first, then rebases, and
  only its real changes conflict. Each script therefore takes any tree, before or after, and is
  idempotent. The PM lands a mechanical step when few web branches are open.
- **Reviewing a mechanical step** is reviewing its script and the script's own check, not the
  diff: the reviewer runs the script on the base commit and gets the branch's tree exactly; a
  token comparison of every file before and after (a TypeScript parser's tokens, whitespace and
  comments aside) differs only in import paths (S3c, S4) or only in the identifiers the rename
  map names (S3d); the built bundle's size is within a few per cent; the suite's test count and
  the oracle are unchanged. A hand edit the script cannot make is a separate commit in the same
  task, reviewed as a diff.
- **A decided removal goes first where it shrinks a step.** #1209 (sky and fog, which reach into
  `web/maplibre`) and #1204 (the debug tools, `?eval`, `?checks`) before S2; #1210 and #1212
  (which touch the roads' statistics, `catalogue.ts`, `retry.ts`, `tasks.ts`) before S9; #1203
  (`state.ts`) before S13. If one is still open when its step starts, the step leaves that code as
  it is and the removal lands afterwards, in the new place.
- **New work uses the newest form on main.** After S3c, new code imports what it needs with its
  type, never through a cast. After S8, a new layer is a peer (§5.1), not a custom layer. After a
  layer type has left the style document (S12), nothing new is added to the style for it.
- **The memory cuts** filed from the audit (#1217 roads, #1218 the far field, #1220 MapLibre's
  caches by bytes, #1223 the small cuts, #1224 the flat tiles and water shares) land as their own
  tasks whenever they are ready: they pay months before the steps that would make them
  structural, and each is local to the code it cuts. The step that later reshapes that code
  carries the cut over (S7 the DEM copies, S8 the shared targets, S9 the caches and "never finer
  than the screen", S12b the flat tiles and water shares) and does not do it again. One overlaps a
  step outright: #1220 is what S9a's `Store` does to every cache; if S9a is near when #1220 would
  start, #1220 is closed into S9a.
- **Feature tasks that change what a settings profile stores** (such as #1247, each metric's own
  scale settings) land as features. The steps never change the stored form (§7.7), so the two do
  not meet except in the files they both edit.
- **The reference set moves only on purpose.** The images are of the map's canvas alone (no
  panels), so work on the user interface does not move them. A feature task that changes how a
  reference view looks records that view again in the same task and says so. A subsuming step
  never records again to make itself pass, except for the look changes its row in §8.1 names.

### 7.4 How a step is verified

MapLibre's own render suite does not serve: it is not part of `check.sh`, upstream's cases are
not in the repo (125 MB, fetched from GitHub), ours are five, and every case is a style document,
which S12 and S13 delete. Retiring it (S3a) loses no check the app has today: upstream's cases
test style features in general, those the map uses are seen in the oracle's views, and ours are
recast as views in S0. Three things replace it.

**1. The oracle (S0), recorded before anything changes.**

- *Views.* About 20 places (a city, mountains, the globe, a coast, the south pole, labels in
  several scripts including the ones #1245 fixes, and a place in the app for each of the five
  cases of our render suite: lifted lines across a tile seam, a see-through sea surface, the
  globe's pole, labels anchored at sea level, a drape and the scene's depth), each a link (camera
  and settings) in four layer sets (land and water; plus roads; plus labels and dots; everything)
  and two looks (the Default profile and a scenic-colour profile): about 160 captures.
- *Images.* Chrome on the test Mac (the M4), drawing on its GPU (S0 checks the WebGL renderer
  string: never a software renderer), captures each view's canvas at 2× once the map is settled:
  tiles loaded, fades finished, placement final. The same Mac always, because the GPU is part of
  the picture. The app is captured as `vite build` makes it, the bundle that is published, so a
  difference between development and the build (§8.4's class fields) shows.
- *Responses.* Beside each image, every response the view read (tiles and API answers, by URL)
  and each tile's ancestors, kept as one archive a view. A small replay server answers from the
  archive, so a view is drawn from the same bytes after the map data has been rebuilt. A request
  the archive lacks fails the comparison and is named: it is a bug, or a stated change in what a
  step loads, in which case that view is recorded again on main first and the step is compared
  with the new recording. A settled view reads a few hundred tiles: estimated tens of MB an
  archive, 1 to 2 GB for the set with the images; one file a view, since the NAS takes small files
  slowly.
- *Numbers.* Per view, as JSON: the camera's matrices, the tile cover of each scheme, a grid of
  terrain heights, the placed labels with their boxes. They are stated in the map's terms (a
  matrix, a list of tile ids, a height at a longitude and latitude, a text and a rectangle), so
  they stay comparable while the code that produces them is reshaped. The app gets a small capture
  hook of its own to report them (#1204 deletes `?eval` and `?checks`); each step keeps the hook
  reading the right place.
- *The tour.* The memory audit's tour as a camera path: frames at fixed times, frame times, and
  memory at each stop. Frame times are taken in a windowed Chrome at the display's rate with no
  other tabs, in a quiet period on the M4, as the median of three runs; "no worse" means within
  the spread S0 measures between runs.
- *Tolerance.* Two runs of the same build define it (S0 measures and records it): for images, the
  share of pixels differing by more than a small step per channel; numbers equal to float
  precision; labels equal as sets. A step passes when it is within it.
- *Time.* Estimated 10 to 30 s a capture, so the full set takes most of an hour and runs in the
  background in parts (no foreground command over 10 minutes); the quick set is chosen to take
  under 2 minutes. S0 measures both.
- *Where it lives.* On the NAS beside the inputs, with a copy on the test Mac's disk (§9.2).
- *What it does not cover.* Feel and motion (gesture response, pacing while moving, loading
  order): the owner uses the published app every day, and each step that touches them says so in
  its report. Views nobody recorded: a bug the set missed adds a view. Safari: each step's agent
  opens the views its row names in Safari and saves full-size screenshots (driven by
  `safaridriver` if §9.2's question 6 allows it).

**2. One unit-test suite.** S1 makes vitest the web app's one runner and adds it to `check.sh`'s
`web` part and to `publish.sh`'s other tests (`tools/app/other-tests.zsh`): MapLibre's 62,500
lines of unit tests (227 files, vitest already, with its helpers in `test/unit/lib`), and,
converted from `node --test`, the 18 files of `web/test` (11 of the map's modules, 7 of the build
page's) and the five `tools/check` tests of `web/` modules. The small side is converted to the
large side's runner rather than the reverse. Nothing runs MapLibre's unit tests today, so S1
first runs them as they stand: each failure is fixed (most will be a test that predates a change
of the fork, changed to the fork's behaviour) or the test deleted with its reason in the commit;
S1 lands with every test passing. What they need comes into `web/package.json`'s development
dependencies (vitest at the version that matches the app's Vite, jsdom,
`vitest-webgl-canvas-mock`, `nise`); the two tests that need the native `canvas` package are
rewritten without it or deleted, so the install stays a plain offline one (`web-deps.sh`; one
online install fills the store). From S3c and S4 the tests sit beside the code they test and
move, change and are deleted with it: a step that deletes a subsystem deletes its tests, a step that reshapes one carries over the
cases that still describe the map (the transforms', the covers', placement's, the buckets'
tessellation) and rewrites them against the new module.

**3. A `check.sh` part.** `web` runs the suite, the type check, the formatter's check (from S3c)
and the build whenever `web/` changes, as today plus those. A new `oracle` part, added by S0, draws a quick subset of the views (S0 picks
it and measures its time) from their archives and compares images and numbers; it runs when
`web/src` changes, on the test Mac, and says `NOT RUN` and fails if the archives or a GPU are not
there, as the remote tests do. Each step's agent runs the full set and the tour and reports the
result; the reviewer runs it again.

### 7.5 The order in which subsystems are folded in

Two orders suggest themselves, and neither works alone.

- **The most wall-workarounds first.** These pay most: each deletes a module, a class of bugs and
  often memory. But the large ones cannot come first. The polar labels' 2D canvas goes only when
  the label engine takes world-coordinate candidates; `namefade.ts` and the invisible circles go
  only when it takes a per-label opacity and the dots have their own pick; the three tile caches
  become one only with the common pyramid. Each needs the service it works round to be reshaped
  first.
- **The leaves first.** What nothing else depends on (gestures, controls, markers, unused
  sources and layer types) is cheap and safe to delete, and everything after it is smaller: less
  to move, restyle and type-check. But past the deletions the leaves are the layers, and those
  can only be rewritten once the services under them have their final shape, or they are
  rewritten twice.

**The order chosen** takes from both:

1. *Delete the unused leaves* (S2), checked by the suite and the oracle already in place (S0, S1).
2. *Merge the structure early* (S3a to S3d, S4), while it is mechanical: one package, strict
   types, the move, one naming. This is what turns the
   rest into ordinary refactoring: from here the app imports the engine's parts with their types,
   one type check and one suite cover both sides of every change, and a workaround can be removed
   by changing the code it patched.
3. *Take the cheap workarounds at once* (S5): the modules that patch or read MapLibre from
   outside (`covers.ts`, `placement.ts`, `maptasks.ts`, the casts) become part of
   the code they patched. Small, and the first visible return.
4. *Then the services, bottom up* (S6 to S11): camera, terrain, frame, tiles, labels, pick. Each
   is reshaped under the layers that use it, and each step ends by deleting the workarounds that
   service caused; that deletion is the step's test of being finished.
5. *The style document last* (S12, S13). All of the roughly 60 base layers hang from it; it goes
   a layer type at a time once the frame, the tiles and the labels can carry a layer defined in
   code, and its machinery (the interpreter, the generated property and array code, the `Map`
   shell) is deleted when the last layer has left.

### 7.6 The structural merge: what disappears, and what the licence keeps

| Today | When done | Step |
|---|---|---|
| `web/maplibre/package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `tsconfig.dist.json`, `postcss.config.js`, four vitest configs, the eslint and rolldown configs, `build/` (docs, banner, eslint rules) | `web/package.json` and its lockfile alone; one `vitest.config.ts`; the code generators run from `web/` until S12 and S13 delete them with what they generate | S1 (test tools), S3a |
| `test/integration/render` (the render suite), `test/scenic` demos, `tools/fetch-upstream-tests.sh` | The oracle; the five cases recast as views in S0 | S3a |
| `built/maplibre-gl.css` | The few rules still needed (the canvas and its container) in the app's stylesheet | S3a |
| `tsconfig.app.json` (`strict: false`, `useDefineForClassFields: false`) writing declarations the app is checked against (`paths`) | One `tsconfig.json`, strict, over source and tests; no declarations written | S3b |
| `web/maplibre/src` (414 files), a tree of its own | Its files in `web/src`'s directories (§5.1) beside ours | S3c |
| The alias `maplibre-gl` in `vite.config.ts`, its lockfile check and `dedupe`; `index.ts`, the public entry | Relative imports of the modules used; no entry re-exporting an API | S3c |
| File names `tile_manager.ts`; 4-space indent, `import {a} from './x.ts'`; no formatter on either side | The app's: short lower-case names in a directory, `import { a } from './x'`; one formatter (configured to change the app's own files least), run over all of `web/src` and checked by `check.sh` from then on | S3c, S4 |
| 315 `Scenic Roads:` markers in 107 files | The comments stay where they explain the code; the prefix goes | S3c |
| `FORK.md`, MapLibre's `README.md`, `ARCHITECTURE.md` | `docs/map-engine.md`: how the map's engine works and why (the measured reasons behind the tile covers, the drapes, lifted lines, the scene's depth), with no "what we changed" | S3c (moved), S3d (rewritten) |
| MapLibre's identifiers: 4,778 uses of `this._name` members (the app has one), snake_case vocabulary in names; the app's names after MapLibre's (`MLMap`, `MlMap`, 180 uses) | The app's conventions: no underscore prefix (a clash with a getter of the same name is resolved by the rename map), the type's own name | S3d |
| MapLibre's comments: library API documentation (970 `@param`, 254 `@example`), 72 mentions of upstream; the app's 414 mentions of "MapLibre" in 68 files | Comments in the app's manner, saying what the code does and why; none says where it came from | S3d |
| Duplicate helpers, one from each side: Mercator and lng/lat (`merc.ts`, `geo/mercator_coordinate.ts`), DOM helpers (`ui/dom.ts`, `util/dom.ts`), worker pools (`workerpool.ts`, `util/worker_pool.ts`), colour parsing (the app's, the style spec's `Color`) | One module each | S5 (helpers), S9b (workers), S13 (colour) |
| `web/package.json`'s note listing the reaches; 146 lines with casts into internals | Typed imports; no note | S5 |
| Generated files: `array_types.g.ts`, `*_properties.g.ts` (13), `unicode_properties.g.ts` | Hand-declared vertex layouts in each layer; typed look structs; the Unicode table an ordinary data file of `labels/` | S12, S13 |
| `.glsl` files turned into modules by `shaders_plugin.ts` | Each layer's shaders as strings in its own module, as the app's layers have them | S12, S13 |

**Types.** Under the app's compiler settings, MapLibre's source as it is has 2,168 errors in 184
of its 414 files (measured 12 Nov, TypeScript 7: 690 are fields without an initialiser, 107
fields declared again in a subclass, most of the rest null checks and implicit `any`). Its tests
add about 2,400 more in 230 files, part of them only because vitest is not installed in `web/`
today; S1 measures them with it in place. S3b fixes what S2 leaves, in place in `web/maplibre`
and in parts by directory: a strict fix also compiles under the loose settings, so each part
lands alone, checked by the strict compiler's errors filtered to its directories; the last part
switches to the one strict `tsconfig.json`. One kind needs care: the two configurations compile
class fields differently (`useDefineForClassFields`: false in MapLibre's, true by default in the
app's), and under the app's a field redeclared in a subclass would overwrite what the base
class's constructor set. The last part sets one value for the whole tree and fixes each field it
changes (a `declare` on a redeclared field); the suite and the oracle on the built bundle are the
check.

**The licence.** MapLibre GL JS is BSD-3-Clause; it asks that source redistributions keep its
copyright notice, conditions and disclaimer, and that binary ones reproduce them in the
documentation or other materials. Kept, and where (S3a):

- `web/public/third-party-licenses.txt`: MapLibre GL JS's `LICENSE.txt` whole (it carries the
  notices of Mapbox GL JS and the other code it contains), maplibre-contour's (today
  `web/src/contour/LICENSE`), and the licences of the packages the build bundles (earcut, pbf and
  the others of §7.9's list, MIT or ISC, which ask the same of a binary). It is in the repo and,
  being under `public/`, in every build.
- The app's credits link to it; `README.md` names it.
- The source files that carry a notice of their own today keep it: `transferable_grid_index.ts`
  (ISC, Mapbox), `tile_id.ts`'s inlined function (ISC), `contour/isolines.ts` (ISC, from
  d3-contour); `contour/vtpbf.ts`'s (MIT) goes with the file in S12e.
- The native app shows the same text in its credits, since its code is translated from these.

Nothing else marks provenance: no headers, no folder, no naming.

### 7.7 Rolling a step back

- A step is one merge on main, published by `publish.sh`. Rolling it back is reverting that merge
  and publishing again.
- Nothing a step does outlives a revert. No step changes the link format or what a settings
  profile stores (the layers' typed looks are filled from the same settings). The one step with
  server changes (S12b's tile formats) serves the old and new forms together; removing the old
  form is its own later task.
- A step that others were built on cannot be reverted alone. So a step that depends on another
  starts from main only after the earlier one has been in the published app for a time the PM
  judges enough (the owner uses it daily); after that, bugs are fixed forward.
- There are no runtime switches between an old and a new path: two paths would be twice the
  surface and neither fully tested. The largest steps are instead cut into parts that each land
  alone (S2, S9, S12).

### 7.8 Checkpoints

- **After S4:** the structure merged; the PM checks that branches still land without trouble
  before the services are reshaped.
- **After S9:** the tiles are the map's feel; the owner uses the published app with S9b longest
  before S10 builds on it (§8.4).
- **After S13:** §7.9's checklist, run and reported. Native work starts only when it holds (N0
  perhaps earlier, §9.2).
- **After N0:** the renderer (§5.9): Rust calling Metal directly, or the wgpu fallback, by
  §8.3's rule.
- **After N2:** the native engine shows land and water on terrain, on a Mac and in the iPad
  simulator, inside its byte budget, matching the oracle. If the comparisons are not converging,
  or the loop is too slow for agents, the choices are the other candidate of §5.9 or a pause;
  what is at stake is N0 to N2, and the web app is complete either way.

### 7.9 Done: the checklist

The subsuming is done when all of this holds on main. Each line can be checked by a command or by
looking; S13's report shows each.

- **No MapLibre side.** `web/maplibre` does not exist. `find web -name package.json -not -path
  '*/node_modules/*'` finds `web/package.json` only; one `pnpm-lock.yaml`; one `tsconfig.json`
  for the source, `strict`, with no `paths`; one `vitest.config.ts`; `vite.config.ts` has no
  `alias` and no plugin. `git grep -i maplibre -- web` finds only the licence file and the
  credits.
- **No markers.** `git grep 'Scenic Roads:' -- web` finds nothing; no `FORK.md`; `README.md` and
  `docs/` name neither. `git grep -iwE 'upstream|fork'` and `git grep -i mapbox` over `web/src`
  find nothing but `@mapbox/` package names in imports.
- **One naming and style.** The formatter's check passes over `web/src` (in `check.sh`); no file
  name has an underscore; `git grep -E 'this\._[a-z]' -- web/src` finds nothing; no `@example`
  and no `index.ts`. A reader sampling any directory cannot sort its files into two groups by
  their names, layout, identifiers or comments: the reviewer of S13 tries.
- **No style document.** No `addLayer`, `addSource`, `setPaintProperty`, `setLayoutProperty`,
  `setFilter` or `getLayer` in `web/src`; no `style/` directory; no `*.g.ts`; no code
  generators; `@maplibre/maplibre-gl-style-spec` is not a dependency.
- **No custom-layer API.** No `CustomLayerInterface` and no `type: 'custom'`; every layer in
  `layers/` implements the one interface and is in the frame's one list.
- **No boundary workarounds.** These do not exist: `namefade.ts`, the vector-tile encoders
  (`mvt.ts`'s `encodePoints` and `encodeLines`, `contour/vtpbf.ts`), `polarlabels.ts`,
  `polarpoints.ts` and the 2D canvas over the map, `covers.ts`, `placement.ts`,
  `maptasks.ts`, `terrainstats.ts`'s reads of the tile managers, the invisible circle layers,
  `addProtocol`, `keepWhole`'s reach into the tile manager, `web/package.json`'s note. No cast
  (`as any`, `as unknown as`) reads another module's private state; the casts left are reported
  with a reason each (146 lines today).
- **One of each.** One `Pyramid` and one `Store` hold every layer's tiles (no other cache has a
  budget); one job pool (no other `new Worker`); one pick (no `queryRenderedFeatures`); one label
  engine (no label drawn by anything else); one camera (no conversion to and from a second
  representation); one module for each helper job (one Mercator and tile-id code, one colour
  type, one set of DOM helpers).
- **The layout** is §5.1's: those directories and no others under `web/src`.
- **Dependencies.** Gone: `@maplibre/maplibre-gl-style-spec`, `@maplibre/mlt`,
  `@maplibre/geojson-vt`, `@maplibre/vt-pbf`, `quickselect`. Expected to stay, each only if a
  source file still imports it: `pbf`, `@mapbox/vector-tile`, `@mapbox/point-geometry`, `earcut`,
  `gl-matrix`, `bidi-js`, `@mapbox/tiny-sdf`, `potpack`, `kdbush`, `murmurhash-js`, `tinyqueue`,
  `@mapbox/unitbezier` (#1245 added none: the browser shapes). Development: `typescript`, `vite`,
  `vitest` and what its tests need (`jsdom`), `@types/*`.
- **One suite.** `check.sh`'s `web` part and `publish.sh` run every test of `web/` (the map's
  and the build page's) with one command; no `node --test` of web modules; no render suite; no
  `web/test`.
- **The line counts.** Counted as in §2.1 over `web/src`: about 64,000 lines of source (§6.3),
  the actual figures written into §6 in place of the estimates.
- **The oracle.** Every reference view within tolerance but for the named look changes (the
  caps' labels); the numbers equal; the tour inside the byte budget; the views checked in Safari.
- **The licence notice** is at `web/public/third-party-licenses.txt` and reachable from the
  credits.
- **The docs.** `docs/map-engine.md` describes the engine as it is. This document loses §2, §3,
  §7 and §8.1 and becomes the plan of the native port alone.

## 8. Steps

Each lands alone, with the app working and its checks passing.

### 8.1 The subsuming, in the web app

"Verified by" names what is particular to the step; every step also passes `check.sh` (the suite,
the type check, the build, the oracle's quick set), and its agent runs the oracle's full set and
the tour, and checks the views it changed in Safari.

| Step | What | Depends on | Verified by |
|---|---|---|---|
| **S0** | **The oracle** (§7.4): the views, their images, response archives and numbers; the tour; the capture hook in the app; the replay server and the compare tool under `tools/check/`; the tolerance, the quick set and their times measured; `check.sh`'s `oracle` part. | #1245 (the script views); #1204 (it deletes the old hooks) | Two runs of one build agree; a view drawn from its archive matches the live capture; the part's `NOT RUN` when the archives are missing. |
| **S1** | **One test runner, in `check.sh`.** vitest and what MapLibre's unit tests need become development dependencies of `web/package.json`; one `vitest.config.ts` in `web/` runs them where they are, the app's and the build page's tests converted from `node --test`, and `tools/check`'s tests of `web/` modules; MapLibre's tests made to pass as they stand (§7.4); `check.sh`'s `web` part and `tools/app/other-tests.zsh` run the suite. | — (beside S0) | Every test passes; each deleted test named with its reason; the converted tests' count before and after; `check.sh`'s own tests (`tools/dev/test`). |
| **S2** | **Delete what the map does not use**, in three parts. *(a)* MapLibre's gestures, controls, markers, location hash and locales; the popup becomes the app's own element; pointer events stay. *(b)* Unused sources and formats (video, canvas, MLT, TileJSON), world copies, the RTL plugin loader, the debug draws. *(c)* Unused layer types and shaders (heatmap, sky and atmosphere), patterns, sprites and the image manager, line gradients, icon and text-and-icon symbols, variable anchors, vertical text. Each with its tests. | S0, S1; #1209, #1204 | The oracle unchanged; (a) `tools/check/touch-gestures.mjs` and gestures by hand in Chrome and Safari. |
| **S3a** | **One package.** MapLibre's `package.json`, lockfile, workspace file, configs and `build/` tooling go (§7.6); the code generators run from `web/`; the render suite, demos and `fetch-upstream-tests.sh` deleted; its stylesheet's needed rules in the app's; the licence file and credits. Files stay in `web/maplibre`. | S2 | `pnpm install --frozen-lockfile` from a clean store; the oracle unchanged; the bundle's size. |
| **S3b** | **Strict types**, in parts by directory (about four), fixes valid under both settings; the last part makes one strict `tsconfig.json` over source and tests (its `paths` now naming the source, not declarations), settles `useDefineForClassFields`, and stops writing declarations. No behaviour changes. | S3a | Each part: the strict compiler's errors in its directories at zero; the suite; the oracle on the built bundle. |
| **S3c** | **The move** (§7.3, §7.6), by a script: MapLibre's files into `web/src`'s directories, renamed; where a subsystem is to be dissolved its files wait in a directory named for what they are (`style/`, `buckets/`, `sources/`); relative imports replace the alias and `index.ts`; the formatter added and run over all of `web/src` in its own commit, its check in `check.sh`; the markers' prefix removed; `FORK.md` moved to `docs/map-engine.md` with its paths rewritten; `web/maplibre` gone. | S3b | §7.3's review of a mechanical step: the script re-run, the token comparison, the suite's count, the oracle unchanged, the bundle's size. |
| **S3d** | **One naming**, by a script with a committed rename map: identifiers to the app's conventions (underscore members, `MLMap`); then by hand, in parts by directory: comments to the app's manner (library API docs, mentions of MapLibre, upstream and the fork); `docs/map-engine.md` rewritten as a description of the engine. The comment parts may run beside S5 onwards, each on directories no open step is editing. | S4 | The rename script's token comparison; the comments parts reviewed as diffs; §7.9's naming greps return nothing outside the directories still to do. |
| **S4** | **The app's files move into the layout:** `web/src`'s 85 flat files and `roads/`, `ctl/`, `contour/`, `flags/` into §5.1's directories, `web/test` beside the code and the build page's tests into `buildpage/`, by a script. No behaviour changes. | S3c | As S3c. |
| **S5** | **The patches from outside become the code**, in two parts. *(a)* `covers.ts`'s saved work is the cover function's own; `placement.ts` is the placement's own scheduling; `maptasks.ts` and `terrainstats.ts` read a typed "what is loading" from the tile code; `keepWhole` is the buildings source's rule; the terrain's rise sets a real setter. *(b)* Every cast into internals becomes a typed import or a small method; `web/package.json`'s note goes; the duplicate helpers of §7.6 (Mercator and lng/lat, DOM) become one module each. | S4 | The oracle unchanged; (a) the tour's frame times no worse (`covers.ts` and `placement.ts` exist for speed); (b) the casts left, listed with a reason each. |
| **S6** | **The camera.** One `Camera` (a pose) and one `View`; the anchored moves of `camera3d.ts` applied directly; one animator for flights and glides; limits in one place. The transform interface, the per-projection helper classes and the conversion back to centre, zoom and pivot go. | S5 | The oracle's matrices equal; the transforms' unit tests carried over; anchored moves keep the ground point under the cursor (unit tests); gestures by hand in both browsers. |
| **S7** | **The terrain as a service.** One projection (no factory); heights, a tile's DEM texture, the depth target and the surfaces (ground, sea floor, pole) offered through one typed interface that `layers/` use instead of reading fields; the DEM copies and formats of §5.4. | S6 (both reshape the projection's code) | The oracle's height grids equal; the terrain and pole views; memory at the tour's stops. |
| **S8** | **The frame, the GPU layer, the layer interface.** A fixed list of passes; one `gpu/` used by every layer (the app's `glprog.ts`, `glstate.ts` and MapLibre's wrapper merged; no raw GL state set beside a cache); the Layer interface of §5.1. The app's six custom layers become peers in the frame's list and the custom-layer bridge is deleted; the base layer types, still described by the style, draw through the same interface. Shared and unread targets (§5.4). | S6, S7 | The oracle unchanged in every layer set; frame times in the tour; a lost and restored WebGL context. |
| **S9** | **The tiles**, in two parts. *(a)* `Pyramid`, `Store` with the one byte budget, one fetch queue with priorities and cancellation, for the base layers' tiles; a source is a function from a tile id to bytes. *(b)* The roads, the far field and the app's seven worker kinds join the pyramid, the budget and one job pool; `addProtocol`, the actor protocol and the class registry go; the budget and each layer's bytes shown in the status readout. | S8; #1210, #1212 | The oracle unchanged once settled; the tour: memory at every stop inside the budget (12 GB held today), frame times and time-to-settled no worse; loading by hand on a slow connection. |
| **S10** | **The labels.** The engine takes candidates in world coordinates with an optional animated opacity; `layers/names` decodes the label tiles (the symbol layers leave the style); the caps' labels become ordinary candidates and leave the 2D canvas (`polarlabels.ts` goes). **Look change:** the caps' labels collide and fade like the rest (and are drawn as every label is, which settles #1181's softer caps' names in Mobile Safari). | S8, S9 | The oracle's placed labels equal as sets outside the caps; the script views; the polar views recorded again and shown to the owner; labels while moving, by hand. |
| **S11** | **Pick and the dots.** One `pick` across layers (`queryRenderedFeatures` goes); `layers/dots` is one point renderer with its own pick and offers its names to the labels with the dots' eased opacity; the invisible circle layers, `namefade.ts`, the landmarks' vector-tile encoding (`mvt.ts`'s `encodePoints`) and its `addProtocol` in `overlays.ts` go. The caps' dots and roads are drawn by the dots' and the lines' renderers, and the 2D canvas over the map goes (`polarpoints.ts`). | S10 | Pick unit tests on recorded tiles; the dots views; hover and click on every kind of thing, by hand. |
| **S12** | **The layers in code**, a part per group, each leaving the style document: *(a)* land (background, tint, hill-shading, trees); *(b)* water and depth (computed tiles; the depth contours no longer encoded as vector tiles, `mvt.ts`'s `encodeLines` and the `addProtocol` of `depth.ts` and `coast.ts` go, its readers move to `tiles/`; the one-value flag and the water shares, each a server format served in both forms, unless #1224 has landed them); *(c)* vector fills, draped lines and `layers/overlay` (the GeoJSON and image sources, `geojson-vt` and `vt-pbf` go); *(d)* near buildings; *(e)* contours (`contour/vtpbf.ts` and the last `addProtocol` go). Each gets a typed look, a decoder writing a hand-declared vertex layout, its shaders as strings; its `setPaintProperty` call sites (73 in all) become assignments to the look. | S9, S11; (b) the server's tests | Each part: its layer set of the oracle unchanged; every control of that layer's panel tried by hand. |
| **S13** | **The rest of the machinery, and the count.** With no layer left in it: the style, the expression interpreter (the style-spec dependency; colour parsing and interpolation kept as functions), the generated array and property code and their generators, the `Map` shell and its event bus of string names (a small engine object with typed methods remains), the shader plugin, unused dependencies. §7.9's checklist run; §6 recounted; this document cut down to the native plan. | S12; #1203 | §7.9, line by line. |

Estimated lines of source per step (not tests). "Moved" is renamed without change; "deleted" is
net; "written" is new or reworked lines in what remains. All estimates but S3c's and S4's "moved",
good to about a quarter.

| Step | Moved | Deleted | Written | From MapLibre, after | The app's own, after |
|---|---:|---:|---:|---:|---:|
| Today | | | | 79,700 | 39,300 |
| The decided removals (#1203 to #1212; not steps of this plan) | | 3,400 | | 79,700 | 35,900 |
| S0 | | | 1,500 (tools, outside `web/src`) | 79,700 | 35,900 |
| S1 | | | 100, and 1,900 lines of tests converted | 79,700 | 35,900 |
| S2 | | 15,700 | 200 | 64,000 | 35,900 |
| S3a | | (tooling and the render suite, outside `src`) | 100 | 64,000 | 35,900 |
| S3b | | | about 2,000 lines touched by type fixes, as many in tests | 64,000 | 35,900 |
| S3c | 64,000 (and the app's files reformatted) | | | 64,000 | 35,900 |
| S3d | | | identifiers across both; comments in about 300 files, the API docs mostly deleted | 64,000 | 35,900 |
| S4 | 35,900 | | | 64,000 | 35,900 |
| S5 | | 700 | 400 | 63,900 | 35,300 |
| S6 | | 3,800 | 2,500 | 60,100 | 35,300 |
| S7 | | 800 | 600 | 59,300 | 35,300 |
| S8 | | 2,200 | 2,500 | 57,100 | 35,300 |
| S9 | | 4,700 | 4,000 | 52,500 | 35,200 |
| S10 | | 3,100 | 2,500 | 49,900 | 34,700 |
| S11 | | 600 | 1,000 | 49,900 | 34,100 |
| S12 | | 13,400 | 6,000 | 37,100 | 33,500 |
| S13 | | 7,000, and the 17,700-line style-spec dependency | 1,500 | 30,400 | 33,200 |

After S3d "from MapLibre" is a figure of this table only: nothing in the tree says which lines
those are. The unit tests (62,500 lines) shrink with the code: S2 deletes roughly a quarter with
the features they test, and each later step deletes or rewrites its subsystem's; what the suite
holds at the end is counted then.

### 8.2 How an agent builds, runs and checks native work

Native development happens on the M1: Xcode cannot be installed on the M4, and the M1 has it (so
does the mini, which also runs the build pool and `publish.sh`). This is the loop every N step
uses; N0 measures it. It is the same under both candidates of §5.9: the engine is Rust either
way, and only what is inside its GPU layer differs.

- **Layout.** `native/` holds Rust crates for §5.1's modules (the engine and the app's logic), a
  command-line renderer `scenic-render`, and the tests. A thin Xcode project beside it
  (`native/App`) holds only what must be an app, in Swift: the entry point, the window or screen,
  the SwiftUI panels and their host, the gestures, icons and settings files. It links the engine
  as an XCFramework through generated bindings (§5.9).
- **Where it runs.** The engine's macOS build and its cargo tests, offscreen rendering included,
  run on the M4 with the other Rust tests: they need only the Command Line Tools. Measured in the
  research note's experiment (`.claude/notes/NATIVE-RUST-METAL.md`, 12 Nov), on the M4 with only
  those, over SSH: Rust drew offscreen through `objc2-metal` into a 16-bit float target and wrote
  a PNG, compiled MSL from source at run time (29 to 42 ms for a small file), and Metal's API
  validation named the shader and the binding of a missing buffer. What the M4 cannot do: compile
  shaders ahead of time (no `metal` compiler, an Xcode component), record Instruments traces (no
  `xctrace`), build for iOS (no iOS SDK), or open a GPU frame capture (no Xcode). So the engine's
  tests compile every shader from source at run time, which also checks that each compiles.
  Everything that needs Xcode runs on the M1, in a copy of the agent's worktree: the engine's
  iOS builds and its XCFramework, `.metallib` builds, `xcodebuild` for the app, the simulators,
  Instruments and the Metal debugger. An agent working on the M4 sends its tree there as the
  Rust tests are sent to the test Mac today.
- **The simulator is not a device for Metal.** Apple's guidance is not to design a rendering
  engine against the simulator, whose Metal is a translation onto the Mac's GPU with fewer
  capabilities. On the iOS 27.0 simulator the note's experiment found no Metal 4 queue, no tile
  pipelines, MetalFX absent from the simulator SDK altogether, and no-copy buffers accepted only
  over memory from `mmap` (one VM region; a heap block trapped); memoryless depth, 16-bit float
  targets, compute and the extended-range layer worked. Planned: the engine has a simulator
  profile (no tile pass, no MetalFX, the classic queue, Rust's `cfg(target_env = "sim")` for what
  must not even link), and the simulator checks the shell and the profile's views; tile passes,
  MetalFX, real headroom, memory limits and frame times are checked on the Macs and on the
  owner's devices. No-copy buffers are always made over `mmap`ed memory, on every platform.
- **GPU frame captures need a release build.** Under capture Metal hands out a proxy device, and
  objc2's debug-build method check panicked on it in the note's experiment; a capture run is a
  release build with debug symbols (`MTL_CAPTURE_ENABLED=1`), opened in Xcode on the M1.
- **Seeing a change without a window:** `cargo run -p scenic-render -- '<link>' --out view.png`
  draws any view, from the live map server or from an oracle archive (`--recorded`), and with
  `--compare` writes the difference image and the oracle's numbers side by side. The agent reads
  the full-size PNG, as with the web app's screenshots. The web app is a second reference: any
  view, not only the recorded ones, can be opened in both.
- **Tests are offline:** they draw from the oracle's archives, never the live server, so a data
  build cannot fail them. The unit tests are translated with their modules from the web app's
  suite.
- **The app:** `xcodebuild` for the Mac (launched, then `screencapture -l <window>` for a picture)
  and for the iPhone and iPad simulators (`xcrun simctl install`, `launch`, `io booted
  screenshot`). Interface tests (XCUITest) can drive taps, pinches and rotations in the simulator.
- **What this means for `check.sh` and the remote test runner.** Today `check.sh` runs where the
  agent works and sends its `tests` part to the test Mac, the M4, through
  `tools/app/remote-tests.sh` (a key, a tree, a command); its other parts run where it runs,
  which the standing rules make the M4. Planned, as a task of its own before N0:
  - a `native` part of `check.sh`, run when `native/` changes: the engine's iOS build, its
    XCFramework and bindings, and the app's build for the Mac and the simulator (the engine's
    cargo tests are in the `tests` part, on the M4);
  - it is sent to a second role, the *native Mac* (the M1), by the same runner made to take the
    role; it runs in place when `check.sh` is itself on the M1. The Rust tests, the engine's
    among them, and the web tests stay on the M4;
  - when the M1 cannot be reached the part prints `NOT RUN` and `check.sh` fails, as the remote
    tests do. It never runs on another Mac silently; the mini can be made the native Mac by the
    same visible setting;
  - native builds take turns on the M1 (one at a time, as the shared build folder's lock orders
    cargo): it is the smaller Mac and a burst of compilers has restarted it before;
  - `affected-tests.sh`, the test guard, the report guard and `tools/dev/agent-rules.md`'s "where
    things run" learn `native/` and the role; the runner drops a worktree's copy on the M1 when
    the worktree goes;
  - every build folder of a copy (Xcode's DerivedData set to a folder inside the copy with
    `-derivedDataPath`, cargo's target for the Apple targets) lives inside it and
    goes with it, so nothing accumulates on the M1;
  - the M1 and the mini keep Xcode at the latest release with the latest OS's simulator runtimes
    (§9.1: latest OS only), and the part's first line prints the Xcode and runtime versions it
    used.
- **What stays with the owner:** feel on real devices (N3, N5, N9), installed by Xcode over a
  cable or by TestFlight (§9.2). He never needs to open Xcode himself. SwiftUI's previews (Xcode's
  live canvas) are not part of the loop: an agent cannot use them.

### 8.3 Native: translating the finished web app

The source is `web/src` as §7.9 leaves it: each native module is the translation of the directory
of the same name, with its tests, minus what the platform replaces (§5.8). N steps follow N0 in
order, except N8, which can run beside N2 to N7 against the server's API. Every step is built and
checked on the M1 (§8.2).

| Step | What | Checked by |
|---|---|---|
| **N0** | The trial that settles §5.9, as proposed by the research note (`.claude/notes/NATIVE-RUST-METAL.md`, §8.2); the definition, the thresholds and the time box are proposals for the owner (§9.2). Everything above the GPU is built once, so the comparison isolates the GPU layer and the shading language. **Built once, shared** (Rust crates, tested on the M4): the slice's `geo`, `camera` (the view from a link, the globe-to-flat blend), `tiles/cover`, `terrain` (the mesh from recorded DEM tiles), the oracle reader and the image comparison, and a thin Swift shell (a Mac window with trackpad pan and zoom, pinch on the iPad) over a C ABI generated with cbindgen that hands Rust a `CAMetalLayer`. **Built twice**, behind one small engine-side interface `Gpu` (buffers, textures, pipelines, a frame with the fixed passes): **(M)** on `objc2-metal`, shaders in MSL; **(W)** on wgpu, shaders in WGSL, raw Metal through `wgpu-hal` only where needed. **Each draws** the terrain mesh, one drape with the elevation tint and hill-shading, and the roads' sprite draw with its compute pass in place of transform feedback, for three reference views (the Alps pitched, a city, the globe): offscreen under cargo's test runner, in the Mac window, in the iPad simulator. **Feature probes in each**, a small test each, pass or fail with notes: (1) HDR: a 16-bit float layer with the extended-range flag and an extended linear colour space, a test frame with values up to 4.0, the headroom read from `NSScreen` or `UIScreen`, seen brighter than white on a Mac's XDR display and on the owner's iPhone; (2) memoryless depth in the scene pass; (3) one copy of geometry (b1): a decode writes vertices into `mmap`ed memory wrapped as a no-copy buffer, and the pick reads the same bytes (W: made with `objc2-metal`, imported into wgpu); (4) a tile-shader pass and MetalFX upscaling of the scene (W: through the hal); (5) three frames in flight, a buffer retired while in use, checked by validation; (6) native text, built once beside the shared part (§9.1, 10): a few clusters of Devanagari (`क्षि`, `र्दि`), Thai (`น้ำ`, `ที่`) and CJK (`東京`) shaped and rasterised through `objc2-core-text` (Core Text's glyphs and positions for a run, each glyph drawn into a bitmap) into the slice's glyph atlas as SDF glyphs, compared by eye with the browser's drawing, and a report of how calling Core Text from Rust went (lines, `unsafe` blocks, Core Foundation ownership, what was awkward). **Time box:** two weeks for the shared part, one week per GPU layer, each layer built by a different agent from the same interface. | Measured and written up for the owner, per variant: (a) correctness against the oracle: matrices and covers to float precision, heights against its grids, images within the tolerance; the two variants should agree, and a difference is a bug in one GPU layer; (b) the loop: the clean build, the build and test after a one-line Rust change and after a one-line shader change, `cargo test` with offscreen images on the M4, the app build on the M1; (c) size: lines of the GPU layer, of shaders, of `unsafe` blocks, and of glue to raw Metal (W's hal code counts against W); (d) safety: three seeded bugs (a wrong binding index, a buffer too short for a draw, a buffer reused while in flight): which variant reports each, how (an error, a validation assertion, nothing), and how long the agent took to find it; (e) frame time (the median of a windowed run over the three views) and memory (the slice's bytes at the globe, the resident footprint) on the M1, the M4 and one of the owner's devices, with an Instruments Metal System Trace of each; (f) tools: a GPU frame capture of each opened in Xcode, and a shader debugged on one draw; (g) the agents' log of every cycle lost to tooling. **The rule** (proposed, honouring the owner's preference): choose (M) unless a feature probe fails under (M) and works under (W); or (M) misses a seeded bug that (W) catches and the validation-on runs miss too; or (M)'s loop or its lost cycles are clearly worse, more than about 1.5 times (W)'s on the same tasks; or (M)'s GPU layer for the slice passes about 5,000 lines. (W) wins a tie only on safety. |
| **N1** | `geo`, `camera`, `tiles/cover`: the maths, with the limits and anchored moves. | The oracle's numbers (matrices, project and unproject, covers); the web suite's tests of these modules, translated. |
| **N2** | `tiles` (pyramid, store, fetch, jobs: threads sharing memory in place of workers), `terrain`, `gpu` and `frame`; `layers/land` and `layers/water`. The budget set from the system and cut on its warning. | Heights against the oracle's grids; `scenic-render --compare` on the land-and-water views; the tour replayed within the byte budget; the same views in the Mac app and the iPad simulator. **The checkpoint (§7.8).** |
| **N3** | `input` on the Mac and touch (system gesture recognizers in place of the web's classifiers); the app shell: the server's catalogue, the shared settings profiles read. | Anchored moves keep the ground point under the cursor (unit tests). Simulator: interface tests of pinch, rotate and pan. The owner: gestures on a Mac, an iPad and an iPhone. |
| **N4** | `layers/lines` (roads, rail) with `pick`; then `layers/vector` and `layers/overlay`. | The roads views compared; pick tests on recorded tiles; frame times at the globe and in a city from the tour. |
| **N5** | `labels` and `layers/names`; `layers/dots` with landmark names. The text path: our own shaping and glyphs translated (§9.1), and for CJK and the scripts that need shaping Core Text or a shaper of our own, as N0's probe settles (§9.1, 10). | The oracle's placed labels compared as sets (the same labels shown, boxes within a tolerance; exactly, if the server's glyphs are kept); the views in several scripts. The owner: labels while moving. |
| **N6** | `layers/buildings`: near meshes, then the far field. | The city views at several zooms and tilts; the hand-over zoom; memory at the globe in the tour. |
| **N7** | `layers/depth`, ferries, the viewshed; `layers/contours`, `layers/polar` and the 3D floor (kept, §9.1). | Their views compared; the polar tests' numbers. |
| **N8** | The app: panels, colour scales, lists, search, the selected road's elevation profile, hover readouts, popups, credits (with the licence text), settings profiles shared with the web app (one schema on the server; pick, save, default), and region downloads kept on the device (its store and how the server hands packs to a device need a design of their own first, §9.2). Can run beside N2 to N7. | Unit tests of the state and controllers; a profile saved in one app read by the other. Simulator and Mac app: screenshots of each panel; interface tests of the main flows. The owner: by hand. |
| **N9** | Memory under pressure, performance, the three platforms' fit and finish, on the latest major OS release only. | The tour within the budget on each device class (the simulator for the budget's logic, the owner's devices for the real limits); frame-time targets the owner sets. The owner: on his devices. |
| **N10** | The switch: the web map app and its tools deleted; the server's web-only routes with them. | The owner's decision. |

### 8.4 Risks

- **The tiles step (S9).** Loading order, stand-ins and pacing are the map's feel, and no picture
  of a settled view tests them. S9 is cut in two, compares time-to-settled and frame times on the
  tour, and is the step the owner should use longest before S10 builds on it.
- **Label parity (S10, N5).** Placement is the least testable part and the most noticed. Both
  compare sets, not pixels, and keep the placement algorithm's structure.
- **The style document's long tail (S12).** About 60 layers, 73 `setPaintProperty` call sites and
  about 40 expression operators carry many small rules (a filter, a zoom stop). Each part of S12
  lists the rules of its layers from `basemap.ts` before rewriting them, and the oracle's layer
  sets are the check that none was dropped.
- **The moves and open branches (S3c, S3d, S4).** A move landing under an open feature branch
  costs that branch a re-run of the script on its tip and a rebase. The scripts, the rename map
  and the PM's timing keep it small (§7.3).
- **Class-field semantics (S3b).** §7.6; a wrong fix shows only at run time, so the oracle and the
  suite run on the built bundle as well as in development.
- **Knowledge in the fork.** Each change in `FORK.md` records a measured problem. S3c and S3d move those
  reasons to `docs/map-engine.md` and to the code's comments before the file goes; a behaviour
  dropped in a later step is a decision, written in that step's report.
- **Written twice.** §7.2's cost. It is bounded by keeping §5's module boundaries the same on
  both sides, so the second pass is module-for-module.
- **Shader ports (N4, N6).** The far field's march and the roads' sprite coverage are long shaders
  tuned by measurement; transform feedback has no direct equivalent. N0 includes the roads' sprite
  draw for this reason.
- **Our own GPU layer (N0, N2).** Calling Metal directly, nothing checks our calls as wgpu would:
  a buffer released while a frame in flight reads it, or a wrong binding, is undefined behaviour
  that may show only on some devices. The rules live in the one GPU layer (§5.3's frames in
  flight), the tests run with Metal's API validation on, and N0's seeded bugs measure what that
  catches; its rule falls back to wgpu if direct Metal misses what wgpu catches.
- **objc2 (N0 onwards).** One maintainer carries the bindings, and releases lag Apple's SDK
  (§5.9). If they stalled for good, the generated crates would be vendored, or the renderer moved
  to the fallback; the GPU layer being objc2-metal's only importer bounds either.
- **The simulator (N0, N2 onwards).** Its Metal lacks features the engine uses (§8.2), so it
  cannot check the tile passes or MetalFX, and it draws a profile devices do not run. The checks
  on the Macs and the owner's devices cover the rest, at the cost of a second profile to keep
  working.
- **Not yet checked** (the research note, `.claude/notes/NATIVE-RUST-METAL.md`, §8.1; N0 checks
  each): HDR seen on a real screen (the experiment's layer had no window); a Rust-owned
  `CAMetalLayer` inside a SwiftUI view through resizing, scale and display changes; the launch
  time of compiling the full shader set from source against a precompiled `.metallib`; Rust's
  debug symbols collected into a device build's dSYM (the debug-symbol bundle crash reports are
  read with); and debugging the Rust side from Xcode.
- **Phone memory (N2).** The web app holds 1.6 to 12 GB today; an iPhone allows a small fraction.
  S9's budget makes the web app bounded; N2 proves the budget at a phone's size.
- **Two apps at once.** From N2 to N10 a server or format change must suit both, and the web app
  keeps changing: a feature added to the web app after S13 is added to the native plan's list.
- **The M1 as the only native Mac.** Every native check waits on one machine that is also a Mac
  someone works on. The mini is the fallback, by a setting (§8.2).
- **Supervision.** The owner does not read Swift. What he sees of each native step is the
  comparison images, the numbers and the app on his devices; a step's report always includes them.

## 9. Decisions

### 9.1 Decided (the owner, 12 Nov)

1. **The order:** subsume in the web app first, fully and structurally, then port (§7).
2. **Shaping first:** #1245 lands before the subsuming starts.
3. **The web app is not frozen** until the switch; it keeps being developed (§7.3).
4. **OS support:** only the latest major release on iPhone, iPad and Mac (whichever that is when
   a build is made). The engine may assume that release's Metal features, and the memory budget
   is set for the devices that run it.
5. **Region downloads** happen on the device.
6. **Settings profiles** are shared between the web app and the native one: one schema.
7. **Native development runs on the M1** (§8.2): everything that needs Xcode. The engine's
   macOS cargo tests run on the M4 with the other Rust tests.
8. **Contour lines, the polar caps and the 3D sea floor are kept**, though off in every profile
   today: S10, S11 and S12e reshape them and N7 translates them.
9. **The renderer: Rust calling Metal directly ("shape 3") is preferred.** The engine and the
   renderer in Rust, calling Metal through the objc2 bindings, shaders in the Metal Shading
   Language, no wgpu; Swift only as a thin shell for the window, the SwiftUI panels and gestures.
   §5.9 records his reasons (wgpu's portability is not needed by an Apple-only app; owning the
   GPU layer and never waiting on wgpu for a Metal feature; the agents' Rust workflow kept; one
   language for the engine) and the costs he accepts. The fallback is Rust + wgpu, with raw
   Metal through `wgpu-hal` where needed. Swift and Metal throughout is not a candidate. N0
   settles it by §8.3's rule; each variant it builds must prove HDR output on macOS and iOS and
   an offscreen-rendered test an agent runs with cargo.
10. **Native text:** for the scripts written a character at a time, our own shaping and glyph
    path, the one #1245 leaves in the web app, translated. For CJK and the scripts that need
    shaping, which the web app has the browser draw (no path of ours to translate): Core Text
    for those scripts, but only if calling it from Rust (`objc2-core-text`) is at least fairly
    ergonomic; otherwise a shaper of our own (HarfRust) with fonts we ship for each script and
    glyphs rasterised by glyph id. N0's probe (6) finds out (§8.3); the figures for the second
    way are in §9.2, 7.

### 9.2 Open

Each can be answered in a sentence.

1. **Basemap lines:** stay draped (as now), or drawn by the roads' renderer, sharp at any tilt? A
   look change; it can be tried in S12c without committing.
2. **Where the oracle lives:** on the NAS, as an inputs-like folder with its own check and a copy
   on the test Mac (proposed; the archives and images are too large for the repo)?
3. **N0 as proposed (§8.3):** accept its definition (the shared part built once, both GPU
   layers built for the same slice, each by a different agent), its time box (two weeks for the
   shared part, one week per GPU layer) and its rule's thresholds (direct Metal kept unless its
   loop or lost cycles exceed about 1.5 times wgpu's, or its GPU layer passes about 5,000 lines
   for the slice), or change any of them?
4. **N0 beside the last S steps:** may it start once S9 has landed (it needs the oracle and the
   finished camera, terrain and tiles), or only after §7.9 as planned?
5. **Apple Developer Program** (paid, yearly), needed for TestFlight and for installs on the
   owner's iPhone and iPad that last more than 7 days: join, or the Mac app, the simulators and
   week-long installs from Xcode only?
6. **Safari on the test Mac:** may Safari's remote automation (Develop menu, "Allow Remote
   Automation") be turned on on the M4, so agents capture the oracle's views in Safari with
   `safaridriver`? Otherwise each step's Safari check is the agent's screenshots taken by hand.
7. **Native text after N0's probe:** the owner's rule (§9.1, 10) chooses Core Text for the scripts
   the browser draws (CJK, the Indic scripts, Thai, Lao, Tibetan, Khmer, Myanmar, Mongolian) if
   calling it from Rust is at least fairly ergonomic; is the probe's report (§8.3, N0's (6))
   ergonomic enough? If not, the other way is a shaper of our own: HarfRust (the maintained Rust
   port of HarfBuzz; rustybuzz is archived) natively, with fonts we ship for each script and
   glyphs rasterised by glyph id, and the web app's text path rewritten to the same shaper first
   (HarfBuzz as wasm) so the two apps agree. Measured for that, 12 Nov (#1245): HarfBuzz's wasm
   444 KB (179 KB gzipped) and 117 KB of JavaScript, 25 ms to start, 0.1 ms a label, about 25 MB
   of memory in Node; Noto fonts for sixteen scripts 664 KB as WOFF2 at one weight (the map uses
   three), CJK not counted. The Rust options are listed in the research note
   (`.claude/notes/NATIVE-RUST-METAL.md`, §7).

Region downloads on the device need a design (what is stored, how the server hands a region's
packs to a device, how they are kept current): a task of its own, planned before N8, not a
question for the owner.
