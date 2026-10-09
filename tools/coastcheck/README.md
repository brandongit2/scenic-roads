# The shoreline check

Does the map's water, at any zoom, tilt and terrain, look as the full-detail water would from the
same camera? Each view of a set (`views.json`) is drawn twice in one page of the app and compared
pixel by pixel:

- **The app**, in its eval mode (`?eval`, `web/src/evalmode.ts`): the view as its link gives it (3D
  terrain on or off at the app's exaggeration, globe or flat Web Mercator), everything but land
  and water off (roads, rail, ferries, labels, landmarks, trees, buildings, hill-shading and tint,
  contours, the coastal shading, rivers drawn as lines, the regions panel's layers, the sky), land
  white and water black. The canvas is read back (`preserveDrawingBuffer`): a pixel's water is 1
  less its grey (MapLibre blends the stored values, so half covered is mid-grey).
- **The reference**: the water at full detail, its exact share of each pixel, from
  `coastcheck serve` (`crates/pipeline/src/bin/coastcheck.rs`, `pipeline::watercov`) as 512-px
  raster tiles drawn 64 CSS px each (8 texels a CSS px), in the same page and camera, at 4 × the
  pixel ratio (2 × with 3D terrain, whose draped textures are made as much finer), then
  box-filtered back. So the projection, globe and terrain are MapLibre's own.
- **Settled** before each capture: everything loaded and drawn (MapLibre's `idle`), the camera no
  longer moving on its own (with 3D terrain the app puts its pivot on the ground once the terrain
  is in), and with 3D terrain every draped texture drawn again (one drawn before the eval mode hid
  a layer can otherwise stay). A GL error or lost context in either capture is reported.
  - The full detail: every ring of the basemap's pinned water polygons (the sea, as Planetiler
    reads it) and every water area of the pass's `water` set as osmium assembles it (lakes,
    reservoirs, rivers' areas; tunnels and covered water left out as OpenMapTiles leaves them),
    27.2 million rings, 898 million points for 2026-09-28's planet: a 7.4 GB store
    (`coastcheck build`, 9 min on the build Mac).
  - Exact coverage: each edge adds its area to the pixels it crosses (as font renderers do); a
    pond a hundredth of a pixel counts a hundredth.
  - Checked against MapLibre's own drawing of vectors (`--vector`: the basemap's z13.5–14 water
    drawn at 4 × without its anti-aliasing outlines and box-filtered): the shores agree to
    −0.14…+0.04 px (+0.01 px with 3D terrain), the mean difference 0.0001–0.007 (`--set proof`).

## The metric

Per view, over the pixels drawn in both (the sky and the globe's limb left out):

- **mean |Δ|, max |Δ|** of the water's share;
- **off**: the share of pixels off by more than 0.25 (an edge drawn in place but anti-aliased by
  another filter differs by up to about that: MapLibre's fill outline against exact area);
- **visible**: the share of pixels whose difference, blurred by a Gaussian of σ 1 device px (half a
  CSS px at 2×, finer than the eye resolves on a 2× screen at reading distance), is over 0.1 (25
  grey levels between white and black, the starkest the map can be): what you'd see at 1×;
- **features**: 8-connected regions of visible difference of one sign holding 1 device px² of
  water or more: water **missing** (a lake or shore the app lacks), **extra** (an island the app
  lacks), **misplaced** (both side by side);
- **shift**: the shore's mean displacement, px (the summed difference over the reference's shore
  length: + the app's water reaches further).

"Identical" here: no visible pixels and no features, which a view can't quite reach (the
reference's own box filter against the app's bilinear one leaves |Δ| ≤ 0.25 on edges).

## The screen (`--screen`)

What the owner sees: the water, its coastal shading and the land as drawn, against the same view at
full detail rendered and then shrunk to the screen's pixels in linear light (task #123: zoomed out,
the map is to look exactly like that, small things faint as full detail makes them, never
exaggerated nor dropped).

- **The app**, in eval mode with `&look=bw` (land white, water black, the shading white over it:
  the starkest contrast) and `&look=app` (the app's own colours, the land its background with no
  hill-shading), the shading on at its default settings: the canvas's colours.
- **The reference** (`referenceScreen`): the sea's and the inland water's exact coverage at 8
  texels a CSS px, and the shading measured from the reference's shares at the same density
  (every pixel holding any land a shore, web/src/coastdist.ts; exactly within 256 of its pixels,
  beyond that from 4 × coarser tiles, coarser yet toward the poles: coast.worker.ts), each
  drawn at `--ss` × the pixel ratio. Each sample is coloured as the map colours a pixel (land, sea
  and lake mixed by coverage in linear light, the shading blended over that as MapLibre blends),
  and each device pixel is the mean of its samples in linear light. The same averaged as the
  stored values (`bwS`, `appS`) tells how much of a difference is linear light's.
- **The metric:** each pixel's lightness, L*/100; then as above (mean |Δ|, visible after the σ 1 px
  blur past 0.1 in black and white, past 0.03 in the app's colours, about 3 L* where the whole map
  spans about 25; regions darker or lighter than the reference).
- **The jump:** for a flat, straight-down view at z.4 and its twin at z.6 (either side of a
  half-zoom tile switch), the z.6 error (app − reference) scaled to the z.4 view's pixels about the
  centre and compared with the z.4 error: how much the screen changes beyond what the zoom
  explains. A steady error (the same at both zooms) is no jump.
- **Images:** `<id>.bw.png` and `<id>.app.png`: the app, the reference and the difference (red where
  the app is darker).

## The views

`views.json`: Maine's coast, the Thousand Islands, the lakes north of Mont-Laurier, Finland's
Saimaa, Labrador's lakes, Argyll's lochs, the Seto Inland Sea, the Azores and Lake of the Woods.
- `all` (127): every place straight down at z4–12, z2–3 on the globe at three; each pitched 45° at
  z6 and 75° at z9 at its own bearing; pitch sweeps (30–80°, the app's maximum) over Finland z7 and
  Maine z9; eight with 3D terrain; four on flat Web Mercator.
- `core` (27): a cross-section for trying designs.
- `high` (10): z13–16, straight down, pitched and with 3D terrain.
- `proof` (6): z13.5–14, for checking the reference against MapLibre's vectors (`--vector`).
- `fade` (70, for `--screen`): places dense with small water (northern Quebec's lakes east of
  task #124's data gap, Hudson Bay's Belcher Islands, Saimaa, Maine's coast, the Stockholm
  archipelago), flat Web Mercator straight down at z4.4/4.6 … 8.4/8.6, and tilted 60° on the globe
  at z5.4, 5.6, 7.4 and 7.6; `fadecore` (10) a cross-section of it; `owner` (5) the owner's own
  comparisons: Hudson Bay at z4, 5 and 6, northern Quebec at z6.4 and 6.6.
- `polar` (26, for `--screen`): toward the poles, where a tile's pixels hold ever fewer metres. The
  whole globe (z1.2, the app's least zoom) at each pole (the app keeps the centre within 65.8° there)
  and at 50° N and S, a pole near the limb; Antarctica at z2, the Ross Sea at z3–4, the Arctic Ocean
  at z2 and the Canadian Arctic Archipelago at z3–4 on the globe; Svalbard, East Greenland
  (Scoresby Sound) and the Antarctic Peninsula at z3–6; four of them flat. `polarcore` (8) a
  cross-section of it.

## Running it

On this Mac (the reference's store is on the build Mac):

```sh
# the app: a server of this checkout on a test root, its web build
target/release/server --root <test root> --home <scratch home> --port 18094 --listen 127.0.0.1 --no-mirror
# the reference: the store and its server on the build Mac, reached through a tunnel
ssh m4 'coastcheck build --set <water set .osm.pbf> --water-polygons <zip> --out geom'   # once a pass
ssh m4 'coastcheck serve --store geom --port 18095' & ssh -N -L 18095:127.0.0.1:18095 m4 &
# a Chrome of its own (no throttling of background tabs; the window sized for 800 × 600 at 2×)
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new \
  --remote-debugging-port=18099 --user-data-dir=<scratch profile> --use-angle=metal --enable-gpu \
  --ignore-gpu-blocklist --disable-background-timer-throttling --disable-renderer-backgrounding \
  --disable-backgrounding-occluded-windows --window-size=800,687 --force-device-scale-factor=2 about:blank
node tools/coastcheck/check.mjs --app http://127.0.0.1:18094 --ref http://127.0.0.1:18095 --set all --out <dir>
```

It writes per view `<id>.json` and `<id>.png` (the app, the reference and their difference: red
where the app lacks water, blue where it has water the reference doesn't), and `summary.json` and
`index.html` over them. References are kept in `--cache` by the exact camera the app settled on: an
unchanged view's is drawn once.

- `--screen`: the screen against full detail shrunk (above); `--extra 'k=v'` adds to the app's query.
- `--shade`: the coastal shading on (white over the black water), the reference's measured by the
  same code from the reference's own shares (`?raw=1`).
- `--raster tileSize,size`: a trial, the water as coverage tiles from the reference's server drawn
  as a raster layer would be (how the water layer's density was chosen).
- `--vector`: the check of the reference above.
