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
