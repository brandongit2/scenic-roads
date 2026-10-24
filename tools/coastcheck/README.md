# The shoreline check

Does the map's water, at any zoom, tilt and terrain, look as the full-detail water would from the
same camera? Each view in `views.json` is drawn twice in one page of the app and compared pixel by
pixel.

- **The app**, in eval mode (`?eval`, `web/src/evalmode.ts`): the view as its link gives it, with
  everything but land and water off, land white and water black. A pixel's water is 1 less its grey.
- **The reference**: the water's exact share of each pixel, from `coastcheck serve`
  (`crates/pipeline/src/bin/coastcheck.rs`, `pipeline::watercov`), as raster tiles drawn in the same
  page and camera at 4 × the pixel ratio (2 × with 3D terrain) and box-filtered back, so the
  projection, globe and terrain are MapLibre's own. The full detail is the basemap's water polygons
  plus the water areas of the pass's filtered planet (`coastcheck build`, once a pass); each edge adds its exact area to the
  pixels it crosses. `--vector` checks it against MapLibre's own vector drawing.
- Each capture waits until everything is loaded, the camera has settled and draped textures are
  redrawn; GL errors are reported.

## The metric

Per view, over pixels drawn in both:

- **mean / max |Δ|** of the water share; **off**: share of pixels off by more than 0.25;
- **visible**: share of pixels whose difference, blurred by σ 1 device px, exceeds 0.1;
- **features**: connected regions of visible difference of 1 device px² or more: water
  **missing**, **extra** (an island lost) or **misplaced**;
- **shift**: the shore's mean displacement in px (+ means the app's water reaches further).

## The screen (`--screen`)

What the user sees: water, coastal shading and land in black and white (`&look=bw`) and in the
app's colours (`&look=app`), against the full-detail view rendered and shrunk to screen pixels in
linear light (`referenceScreen`). The metric is lightness (L*/100), visible past 0.1 in black and
white and 0.03 in colour. **The jump** compares a view at z.4 with its twin at z.6, either side of a
half-zoom tile switch. Images: `<id>.bw.png`, `<id>.app.png` (red where the app is darker).

## The views

Sets in `views.json` (pick with `--set`, or `--only id,id`): `all` (every place at z2–12, pitched,
with 3D terrain, flat Mercator), `core` (a cross-section), `high` (z13–16), `proof` (for
`--vector`), `fade` / `fadecore` / `owner` (places dense with small water, for `--screen`), `polar`
/ `polarcore` (toward the poles, for `--screen`).

## Running it

The reference store lives on the lead (the M4).

```sh
# the app: a server of this checkout on a test root
target/release/server --root <test root> --home <scratch home> --port 18094 --listen 127.0.0.1 --no-mirror
# the reference, on the lead, through a tunnel
ssh m4 'coastcheck build --osm <the pass's filtered .osm.pbf> --water-polygons <zip> --out geom'   # once a pass
ssh m4 'coastcheck serve --store geom --port 18095' & ssh -N -L 18095:127.0.0.1:18095 m4 &
# a Chrome of its own, 800 × 600 at 2×
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new \
  --remote-debugging-port=18099 --user-data-dir=<scratch profile> --use-angle=metal --enable-gpu \
  --ignore-gpu-blocklist --disable-background-timer-throttling --disable-renderer-backgrounding \
  --disable-backgrounding-occluded-windows --window-size=800,687 --force-device-scale-factor=2 about:blank
node tools/coastcheck/check.mjs --app http://127.0.0.1:18094 --ref http://127.0.0.1:18095 --set all --out <dir>
```

Output: per view `<id>.json` and `<id>.png` (app, reference, difference: red where the app lacks
water, blue where it has extra), plus `summary.json` and `index.html`. References are cached in
`--cache` by camera.

Other flags: `--extra 'k=v'` (added to the app's query), `--shade` (coastal shading on),
`--raster tileSize,size` (water as a raster layer, a trial), `--vector`, `--verbose`.
