# Scenic Roads

An interactive dark-theme map of every car-accessible public road in **Ontario, Québec, New Brunswick, Nova Scotia, Prince Edward Island, Newfoundland and Labrador, New York, Vermont, New Hampshire, Maine, Massachusetts, Connecticut and Rhode Island**. That is 1.06 M km of road.

It is built for finding scenic drives:
- Roads are coloured by elevation, grade, or a family of scenic metrics.
- The metrics come from 15 km viewsheds that account for terrain and **tree canopy heights**.
- The roads are draped on 3D terrain with hill-shading.

Everything is downloaded and processed locally, then rendered as vectors by a custom WebGL2 layer.

```bash
make data   # download + build everything (first run ≈ 1.5 h, refreshes minutes)
make run    # http://localhost:8080
```

## Finding scenic drives

- **3D terrain + hill-shading, on by default** (height ×3, adjustable 1–6×).
  - ⌥ Option + two-finger drag: horizontal rotates, vertical tilts. Right-drag and Ctrl-drag do the same, as do the on-screen buttons. Zoom, rotation and tilt all keep the 3D point under the cursor fixed on screen: the camera dollies toward it or orbits it rigidly.
  - Hill-shading method (combined, standard, Igor, multidirectional, basic), light direction and strength are adjustable.
  - The camera does not rise and sink with the terrain under the view centre. After each move the pivot is put back on the ground at the view centre *without moving the camera*, so the zoom level (tile detail, widths, labels) matches the real distance to the ground. "Camera follows terrain height" restores MapLibre's default.
  - **Globe** (default on). It hands over to flat Web Mercator at zoom 11–12, MapLibre's own default. There the camera is still 30–60 km out and the curvature across the screen is under a pixel. It can't go deeper: MapLibre's globe camera measures its distance from sea level and must stay above the terrain, so with ×3 mountains a close-up globe couldn't get near them. Cursor-anchored zoom, tilt and rotate are exact on both: on the globe, MapLibre moves the camera and then a Newton-step pan pins the terrain point back under the cursor.
  - Optional terrain tint, coloured by **elevation or terrain slope**. Slope is in %, computed with Horn's method at z12. Coarser zooms show the *mean of the slopes* beneath them (the `slope` pipeline step), so colours stay consistent as you zoom out. It uses a slope-class ramp from green to red to purple (100 % = 45°), and gentle slopes fade to fully transparent (adjustable per variable).
  - Tint options:
    - 8 ramps, including one that matches the road palette;
    - range: whole region, fit to the view, match the road colours, or custom;
    - smooth or 25–500 m bands;
    - a lowland ↔ highland emphasis curve;
    - an opacity slider and a live legend.
  - Contour lines computed on the fly, sky and distance fog, and a label-opacity slider.
- **Colour modes** (Colour card), switched on the GPU:

  | Mode | What it shows |
  |---|---|
  | Elevation | Height above sea level. |
  | Grade | Road steepness. |
  | Relief | Local relief (view min→max). |
  | Scenic score | A weighted blend of the factors below, with presets: balanced, big vistas, lakes & coast, mountains, twisty, foliage, quiet backroads, heritage. |
  | Views | Area visible within 15 km; trees and terrain block sight lines. |
  | Water views | Water area visible within 15 km. |
  | Vista distance | Mean farthest visible distance. |
  | Terrain drama | Relief within 3 km. |
  | Ridge ↔ valley | Height above or below surrounding terrain within 1.5 km. |
  | Curviness | Turning in °/km. |
  | Unblocked views | Share of directions not walled in within 300 m. |
  | Roadside trees | p95 tree height within 30 m. |
  | Forest cover | Tree cover nearby. |
  | Open land | Fields and meadows. |
  | Built-up | Built-up land nearby. |

- **No overlap brightening:** each pixel is painted by one road. A stencil pass draws the solid core of the first road at each pixel, with bridges and major roads first, then anti-aliased edges only outside cores. So junctions, joints and dense areas are no brighter than a single road. The "Blend overlapping roads" option restores the additive look.
- **Perspective line widths:** in tilted views, road widths scale with distance, so far roads get thinner like the ground they're on.
- **Threshold highlight also filters interaction:** roads dimmed by "Highlight above/below" can't be hovered or selected.
- **Accurate hover and click at any tilt:** candidates come from the terrain point under the cursor, and the winner is the road nearest on screen, measured between its projected endpoints.
- **Low values fade out:** roads at the low end of the colour scale become transparent (default 70 % over the lower 60 % of the scale; both adjustable, and the legend shows the fade).
- **Auto-fit percentiles** are configurable (default 1st–99th of road length in view).
- **Histogram equalisation** (the "Equalise" pill) spreads colours evenly over the road length in view. Ranges auto-fit to the view or can be locked or dragged, and the threshold highlight works on any metric.
- **Scenic drives** tab: the best 2/5/10/25 km stretch of every road in view, ranked with your weights and scored server-side in about 20 ms. Hover highlights a stretch; click opens its profile.
- **Profile panel:**
  - scenic summary: average and best score, share of the road with open views or water in view, and roadside tree height;
  - an overlay line for any scenic metric;
  - **▶ Drive**, a 3D fly-along of the road.
- **Hover inspector:**
  - score, visible area, water, vista, openness and tree height at the cursor;
  - flags: scenic route, viewpoint nearby, waterfront, park, heritage nearby, covered bridge, UNESCO/dark-sky area, Indigenous land.
- **"What can I see from here?"** (Layers → Tools): click anywhere for a 2,048-ray viewshed (5–25 km, standing, platform or tower height), draped on the terrain, with visible and water area.
- **Layers** (all toggleable, off by default):
  - parks & protected areas;
  - heritage sites by level (World Heritage, national, National Register, provincial/state, municipal);
  - heritage districts;
  - biosphere reserves, geoparks and dark-sky places;
  - Indigenous lands;
  - viewpoints, peaks, waterfalls, lighthouses, covered bridges, rest areas and picnic sites, and trailheads;
  - gold glow on designated scenic byways and routes touristiques.

  Clicking a feature shows its designation, date and authority, with a link to the official record.
- Everything from v1 remains:
  - climbs;
  - stats;
  - surface and class filters;
  - line weight (0.1–1×, default 0.5×);
  - shareable URL (it includes bearing and pitch, and pasting a link applies it);
  - Google Maps link;
  - trackpad pan and pinch;
  - smooth mouse-wheel zoom;
  - progress indicators.

**Settings persist.** Every map setting, plus UI choices such as tabs, drive length, climb sort, viewshed options, profile overlay and collapsed sections, is saved in localStorage and restored when you open the app without a link. A link's URL hash takes precedence.

## How the scenic metrics are computed

1. **Samples.** Roads are sampled every 100 m, which gives 11.2 M samples. Eye height is 1.5 m above the road. Bridges use the deck, and tunnels see nothing.
2. **Near field (0–300 m).** A horizon is computed in 32 directions from the terrain plus the **median canopy height** from Meta/WRI's global 1 m canopy-height map (10° aggregate product). A direction that rises more than 5° counts as blocked, so tree walls and cuts register as enclosure.
3. **Far field (300 m – 15 km).** Rays are cast over a z11 grid (~55 m) of terrain plus canopy, with earth curvature and refraction. Each ray starts at that direction's near-field horizon, so roadside trees block distant views too. The pass accumulates visible area, visible water (ESA WorldCover) and farthest visible distance.
4. **Landscape.** The pass also computes:
   - relief within 3 km;
   - topographic position within 1.5 km;
   - open land within 1 km;
   - built-up land within 500 m;
   - forest cover within 150 m;
   - p95 roadside tree height within 30 m.
5. **Flags.** These add viewpoints within 1 km, heritage sites within 500 m (plus heritage-district polygons), parks, UNESCO and dark-sky areas, Indigenous lands, and waterfront within 100 m. The `scenic flags` step recomputes them in seconds when designations change.
6. **Score.** Each metric is normalised to 0–1 and weighted: score = Σ wᵢcᵢ / Σ max(wᵢ, 0). The browser (per vertex, in the shader) and the server (drives ranking) use the same formula.

## Data

| What | Source | Licence / terms |
|---|---|---|
| Roads, water, boundaries, places, parks, POIs, Indigenous land boundaries | OpenStreetMap (Geofabrik extracts) | © OpenStreetMap contributors, ODbL |
| Road elevation | NRCan **HRDEM** lidar (8 m overview) → USGS **3DEP** 10 m → NRCan **MRDEM** 30 m | OGL–Canada / public domain |
| 3D terrain, hill-shading, contours, analysis grid | Terrain Tiles (Terrarium) on AWS Open Data | Mapzen / various open sources |
| Tree canopy height & cover | Meta & WRI global canopy height (1 m, 10° aggregates: median, p95, cover > 5 m) | CC BY 4.0 |
| Land cover (water, open land, built-up) | ESA WorldCover 2021 10 m | CC BY 4.0 |
| UNESCO World Heritage | whc.unesco.org syndication feed (see below) | © UNESCO/World Heritage Centre, **private non-commercial use only** |
| Canadian federal designations | Parks Canada **Directory of Federal Heritage Designations** (National Historic Sites, heritage lighthouses & railway stations, federal heritage buildings) | OGL–Canada |
| US designations | NPS **National Register of Historic Places** incl. National Historic Landmarks | Public domain |
| Québec | MCC **Répertoire du patrimoine culturel** (classified, declared, national, municipally cited; heritage-site perimeters) | CC BY 4.0 |
| Ontario | **Ontario Heritage Act Register** (Ontario Heritage Trust), heritage conservation districts | Personal non-commercial use only |
| Nova Scotia | Registered Heritage Properties (provincial); Halifax (HRM) municipal heritage properties | NS Open Government Licence; HRM open data |
| New Brunswick, PEI, Newfoundland & Labrador | **Canadian Register of Historic Places** (historicplaces.ca); Moncton open data | Non-commercial reproduction with credit |
| Biosphere reserves, geoparks, dark-sky places | UNESCO MAB & Global Geoparks lists, DarkSky International, RASC dark-sky programme (`data/heritage/special-official.json`) | Facts from the official registries |

**Official sources first.** Wikidata is used only to supply coordinates for federal designations, because the Parks Canada directory has none. `dem/federal.py` cross-checks every federal record against Wikidata by name within its province, then falls back to named OSM features. It locates 672 of 705 National Historic Sites and 1,346 of 1,815 federal designations. Records that are unlocated or only loosely matched are listed in `data/heritage/federal-report.csv`. 218 Wikidata items that claim a federal designation missing from the official directory are excluded.

**UNESCO terms.** The World Heritage List data is used privately and non-commercially, with the notice "Copyright © 1992 - 2026 UNESCO/World Heritage Centre. All rights reserved." shown on each site. Descriptions are not reproduced; each site links to its official page. The Ontario Heritage Trust register carries similar personal-use terms. **Do not publish or redistribute the built data or a hosted copy of this map.**

**Road selection.**
- Included:
  - motorway through residential, plus links and living streets;
  - public service roads (not driveways, parking aisles or drive-throughs);
  - car ferries.
- Excluded:
  - private, no-access, customers, delivery, agricultural, forestry, permit, military and similar access, where the most specific tag wins;
  - roads behind private or locked gates;
  - tracks, winter roads and 4wd-only roads.

## Pipeline

```
extract ─ sample.py ─ terrain ─ landcover.py ─ scenic prep ─ scenic canopy ─ scenic view ─┐
                                    federal.py · crhp.py · heritage.py ─ scenic flags ────┴─ tile ─ server ─ web
Planetiler (water, boundaries, places, parks) ──────────────────────────────────────────────────┘
```

| Step | Time | What it does |
|---|---|---|
| `extract` | 20 s | Roads, access rules, 8 m densification, POIs and scenic routes. |
| `sample.py` | ~20 min first run, seconds afterwards | DEM sampling with a vertex cache. |
| `terrain` | minutes | Terrarium tiles z0–12 near roads (2.7 GB) and the z11 analysis grid. |
| `slope` | ~4 min | Slope pyramid (1.7 GB): z12 slopes, coarser levels = mean of the slopes beneath. |
| `landcover.py` | minutes | WorldCover classes on the grid. |
| `scenic prep` | seconds | 100 m samples and drape heights. |
| `scenic canopy` | ~28 min | Streams 12 Meta 10° canopy files (11 GB, cached) and computes near-field horizons. |
| `scenic view` | ~40 s | 11.2 M 32-ray far-field viewsheds. |
| `federal.py`, `crhp.py`, `heritage.py` | minutes | Official registers, areas rasterised to the grid. `crhp.py` politely fetches ~3,400 register pages once and caches them. |
| `scenic flags` | 6 s | Designation flags and per-vertex channels. |
| `tile` | ~70 s | Clean-up, climbs, z4–14 tiles with drape heights and 12 scenic channels. |

- `make heritage` reruns just the designation steps and tiles.
- Every stage writes `*.tmp` files and renames them into place; restart the server to serve new data.
- Binaries are order-only prerequisites: code edits don't force data rebuilds, so delete an output to redo its step.

**Disk:**
- ~16 GB build:
  - 2.7 GB terrain;
  - 2.5 GB basemap;
  - 1.7 GB per-vertex scenic channels;
  - 0.95 GB road tiles;
- 11 GB canopy cache;
- 5.6 GB OSM extracts;
- 1.4 GB elevation cache.

**Requirements:** Rust, Node, [uv](https://docs.astral.sh/uv/), Java 21+, `osmium-tool`.

## Rendering notes

- Each road segment is one instanced quad expanded in screen space. The fragment shader handles caps and joins, antialiasing, dashes and the palette lookup. The metric, including the weighted score from 12 per-vertex channels, is computed in the vertex shader, so mode, weights, equalisation (a CDF lookup texture), threshold and filters are all uniform changes.
- In 3D, roads are lifted by drape heights × exaggeration and depth-tested against MapLibre's terrain. A pitched view uses a quadtree tile cover: fine tiles near the camera, coarse tiles toward the horizon, and nothing more than 6 zooms coarser than the view (the fogged horizon).
- The tile cover projects screen samples analytically onto a plane. `map.unproject` ray-marches the 3D terrain on the CPU; using it cost ~140 ms per frame in tilted views, and the analytic projection takes ~2 ms.
- Per-tile matrices are composed in float64, and tiles decode in a worker pool that also builds the elevation and grade sketches.

## Known limitations

- The analysis grid is ~55 m, so narrow gorges and summit domes are smoothed. A viewshed from a rounded summit at 1.7 m eye height is pessimistic; try the platform or tower height.
- Canopy heights are 2019–2020 medians over ~28 m cells: recent clear-cuts and new growth aren't reflected, and a road through forest reads as enclosed even if its right-of-way is wide.
- Federal designations located "by name" or via OSM are flagged as approximate in their popup.
- Bridge decks are interpolated between abutments, and grade reflects DEM and OSM accuracy.
- Browsers pause WebGL in hidden tabs.
