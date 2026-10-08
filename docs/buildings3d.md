# 3D buildings

**Phases B0 and B1 done** (2026-10-06): the sources downloaded and measured (B0); the steps, the
layer and the map built and piloted by hand on six z6 tiles (B1, §5.1), measured on the iPad
(2026-10-08, §4.6). **B2's code built** (2026-10-08): the agent runs the steps for every tile as
part of the build (§3.2–3.6), the mirror, the iPad's service worker and the credits know the
layer; not yet published, so no agent runs it and no tile beyond B1's pilot is built: the next
app published starts it (§5.1). Nothing of B3–B4 built. The first of plan.md §10's phase 7 features ("3D buildings, then PLATEAU"); plan.md §6
(Global-source layers) points here. Companions: `docs/plan.md` (the pipeline, keys, order),
`docs/formats.md` (files), `docs/workers.md` and `docs/pool.md` (sharing the work). Its sources are
on the NAS (§2.6); `dem/bldmeasure.py` measured them (§2.2–2.5); `dem/bldprep.py` and
`pipeline::bld` build them (§3).

**The idea in one line:** every building in the coverage, extruded to its height on the 3D terrain,
from the same pinned Overture release the roadside buildings read, the missing heights estimated
from floors and the neighbours, then from GHSL's 100 m heights in high-rise cores, then from the
footprint's size and kind; built per z6 tile in a chain of its own beside the regions', shared with
helper Macs and pages, and drawn by MapLibre's fill-extrusion.

## 1. What the map shows

**A building** is a footprint polygon (with its holes) extruded from the ground to its height.
- Overture's building parts (OSM's `building:part`: a church's nave and tower, a skyscraper's
  setbacks) are drawn between their own base and top, and the outline they belong to (`has_parts`)
  isn't extruded (its parts are its shape), as OSM's Simple 3D Buildings has it; an outline none of
  whose parts are in the files is drawn as a building.
- Flat roofs. Roof shapes are known for 0.7 % of the coverage's buildings and roof colours for
  0.1 % (§2.2): neither is drawn in the first phases. Pitched roofs are a later option (§4.7).
- Underground buildings (`is_underground`) are left out.

**Where:** every building that touches the coverage, as a way does (plan.md §5: any vertex inside
it, with its 1 km buffer), whether or not a road is near; not the world. 339 million buildings
and 2.8 million building parts in today's 88 regions (§2.5). Where the coverage ends, buildings end,
as roads do.

**At which zooms.** Tiles at z12, z13 and z14; above z14 the map draws the z14 tiles, whole, up to
its 19.5 (§4.1).
- z14: every building.
- z13: buildings 20 m tall or more, or with a footprint of 2,000 m² or more.
- z12: buildings 40 m tall or more: the skyline.
- So the layer shows the towers from zoom 12, the large buildings from 13 and every building from 14.
  A tilted view takes coarser tiles toward the horizon (MapLibre's cover), so every building near the
  camera, only the tall ones farther out, and none beyond the z12 tiles.
- The tiles' sizes (B0, §2.5), against ~300 KB a tile: the thresholds keep z12 and z13 under it (the
  fullest z12 tile 10,339 buildings, 176 KB at most; z13 17,842, 284 KB at most). z14 holds every
  building, and about 15 of its 2.2 million tiles are over 300 KB, at most 490 KB: old towns drawn on
  Spain's cadastral parcels and three of Tokyo's wards. They stay as they are until the iPad is
  measured (§4.6): simplified to a grid unit, they would lose ~18 %. B1's pilot built them: Barcelona's
  14/8292/6115 is 484 KB (25,483 buildings), the heaviest of Tokyo's 313 KB.

**Beside the terrain.**
- A building stands on the 3D terrain at its centroid's height (MapLibre samples the terrain there),
  its base sunk 10 m so that it doesn't float on a slope. Phase B3 sets each wall's foot on the
  terrain under its own corner, the roof level (§4.3).
- Heights are true, not exaggerated: at the default 3× terrain a house looks low beside the hills,
  as in Google Earth. A setting scales them (1–3×) or makes them follow the terrain's exaggeration.
- The hill-shading, slope tint, tree cover and contours lie on the ground under the buildings.
- Fog: MapLibre fogs the terrain, not extrusions (its fill-extrusion fragment shader is the colour
  alone, checked in 6.11.2); B3 patches it as `vite.config.ts` patches the circle and symbol
  shaders, so the far skyline fades with the ground.

**Beside the roads and rail.** Buildings are drawn after the road and rail layers (§4.2):
- a road behind a building is hidden by it (faint through it with an opacity under 1);
- a road in front of a building stays in front, because a road lies on the terrain, and anything
  behind the terrain's surface at a pixel fails the depth test against the terrain;
- the exceptions are what stands above the terrain: bridges and elevated rail in front of a
  building are painted over by it. B3 draws bridge and elevated pieces again after the buildings.

**Beside the landmarks and labels.** The landmark dots (`dots.ts`) draw without a depth test, after
the buildings, so a heritage site's dot stays visible on its own cathedral. Labels are symbols,
which MapLibre doesn't hide behind extrusions. The selected road, drives and climbs are draped line
layers: a building in front of them hides them, as it should.

**The look:** a muted blue-grey made for the dark map, lit from the hill-shading's light direction
(Settings → Terrain), the walls darker toward the ground (MapLibre's vertical gradient), so the
coloured roads and the landmarks stay what the eye goes to. Colour modes (§4.4): plain; by height
(B1: a fixed ramp; with the shared colour-map picker and scale in B3); by where the height comes
from.

**The toggle:** Settings → Buildings, a section after Trees with its switch in the header, on by
default. In it: 3D or flat (footprints only, also what a map without 3D terrain shows); colour
mode; opacity; height scale (1–3×, or with the terrain's exaggeration). In the link with the other settings (`bd=`); **B** toggles the layer. All three only
with a catalog that has the layer: until then the section is hidden, B does nothing and links
leave `bd=` out (a catalog that gains the layer while the map is open shows them then).

**Hover:** the bottom bar's row 1 gives the building's height, where it comes from ("measured",
"from 6 floors", "estimated by Microsoft", "estimated from neighbours", "estimated, GHSL",
"estimated from its size"), its base when it has one and its kind; the hovered building is lit
amber. Markers win over buildings; a road or rail line wins unless a building hides it (the lines
are picked within a few pixels of the cursor, hidden or not); the areas a building is in show as
chips beside it (an old town is often a heritage area, which would otherwise hide every building
in it). No popup in the first phases.

**The iPad** (8 GB iPad Pro, Safari): §4.6 has the budget and the fallbacks.

## 2. Data

### 2.1 The sources compared

| Source | Gives | Here | Licence | Use |
|---|---|---|---|---|
| **Overture Maps buildings**, release 2026-09-23.1 | 2.53 billion footprints worldwide (OSM, Microsoft, Esri, Google, IGN España, others, conflated), `height`, `num_floors`, `min_height`, `min_floor`, roof shape and colour where known; 4.49 million building parts | 339 M buildings in the coverage: a height for 43.7 % (Microsoft's estimates for 37.1 %), floors for 6.1 %, either for 48.0 % (§2.2) | ODbL 1.0 for the theme; each row's sources and their licences in its `sources` column | **Yes**: every footprint and attribute |
| OpenStreetMap `height`, `building:levels`, `roof:*` | The tags | Already inside Overture: OSM gave 59 % of the coverage's footprints, and its tags became `height` / `num_floors` (all of Japan's heights) | ODbL | Through Overture. Read directly it would need every building in the pass's filter (~600 M ways, a new set) for a month's freshness and nothing else |
| Microsoft Global ML Building Footprints, with heights | ML footprints and height estimates | Already inside Overture: "Microsoft ML Buildings" gave 33 % of the coverage's footprints and 85 % of its heights (99 % of France's, the UK's, Canada's and Ireland's, 84 % of the US's), estimates that grow ~1.2 m a floor (§2.2) | ODbL | Through Overture |
| USGS 3DEP lidar heights | Measured heights | Already inside Overture ("USGS Lidar": 4 % of the coverage's heights, all in the US) | Public domain | Through Overture |
| Google Open Buildings 2.5D Temporal | Building presence and height rasters, 4 m, 2016–2023 | Africa, South and Southeast Asia, Latin America and the Caribbean: of the coverage, only Singapore, Puerto Rico and French Guiana | CC BY 4.0, via Earth Engine (an account) | No. Overture already merges Google's v3 footprints there |
| **GHSL GHS-BUILT-H R2023A, ANBH** (EC JRC) | The average height of the buildings in each 3″ cell (~90 m), epoch 2018, worldwide | Every built-up cell | CC BY 4.0 (© European Union) | **Yes**: the fill in high-rise cores (§2.3), 1.28 GB |
| France: IGN BD TOPO (bâti) | Measured `HAUTEUR` and floors for every building | France | Licence Ouverte 2.0 | Later (B4): would replace France's estimates |
| Japan: MLIT PLATEAU | LoD1/LoD2 city models of ~250 cities, measured heights | Japan's cities | CC BY 4.0 compatible | Later (B4), as plan.md §10 already plans |
| Great Britain | No open building heights (Ordnance Survey's aren't open; the Environment Agency's lidar would be a project of its own) | — | — | No |

**Decision:** Overture alone for footprints and attributes, the same pinned release as the roadside
buildings (`pipeline::buildtiles::RELEASE`, 2026-09-23.1), so the roadside factor and the drawn
buildings agree; GHSL for the fill in high-rise cores. National measured heights later, country by
country, where they replace estimates.

### 2.2 Overture, measured (2026-10-05; every building in the coverage in B0, 2026-10-06)

- **The release:** 515 files on S3: 512 of buildings (276.9 GB, 2,533,842,612 buildings in 82,688 row
  groups of ~34,000) and 3 of building parts (0.62 GB, 4,486,107 parts). Every row group's footer
  carries its bbox statistics, so the files and row groups meeting an area are known from the footers
  alone (`dem/bldfetch.py` reads them once into `footers.json.gz`).
- **The coverage's share:** 100 building files (59.9 GB) and the 3 parts files hold a row group
  meeting the coverage grown by 20 km; those row groups hold ~357 M buildings (344 M with a 1 km
  margin) and 3.1 M parts.
- **It expires:** S3 says `x-amz-expiration: expiry-date="Wed, 25 Nov 2026"`, `rule-id="release
  data 60 day retention"`. Overture keeps about two months of releases (2026-08-19.0,
  2026-09-23.0 and 2026-09-23.1 listed today). Pinning a release means keeping its files.
- **The coverage's buildings** (B0, `dem/bldmeasure.py`: every building and part whose centroid is in
  the coverage grown by 1 km, out of the 350 M rows of the 16,651 row groups meeting it): 339,188,665
  buildings and 2,840,099 parts (all OSM's; 1.9 M of them Spain's); 1,704 underground buildings left
  out. A `height` counts when it is 2–700 m: 4.5 M are under 2 m, nearly all Microsoft's, and count
  as none.

  | | buildings | `height` | of them Microsoft's | `num_floors` | either | neither |
  |---|---|---|---|---|---|---|
  | US | 156.1 M | 72.2 % | 84 % | 2.0 % | 72.5 % | 27.5 % |
  | France | 54.5 M | 18.5 % | 99 % | 1.1 % | 19.4 % | 80.6 % |
  | Japan | 54.2 M | 7.8 % | — | 5.1 % | 8.9 % | 91.1 % |
  | UK | 25.9 M | 44.8 % | 99 % | 7.0 % | 50.3 % | 49.7 % |
  | Spain | 19.2 M | 9.4 % | 93 % | 60.2 % | 64.7 % | 35.3 % |
  | Canada | 14.3 M | 48.7 % | 99 % | 2.4 % | 49.8 % | 50.2 % |
  | Portugal | 6.3 M | 2.3 % | 89 % | 1.9 % | 4.1 % | 95.9 % |
  | Ireland | 3.8 M | 12.6 % | 99 % | 8.5 % | 20.3 % | 79.7 % |
  | Puerto Rico | 2.2 M | 9.4 % | — | 0.4 % | 9.5 % | 90.5 % |
  | Taiwan | 2.0 M | 0.5 % | — | 3.8 % | 3.9 % | 96.1 % |
  | Singapore | 0.24 M | 1.2 % | — | 13.9 % | 14.4 % | 85.6 % |
  | Hong Kong | 0.21 M | 0.9 % | — | 26.9 % | 27.1 % | 72.9 % |
  | the other 8 | 0.27 M | 1.9 % | | 3.0 % | 4.6 % | 95.4 % |
  | all | 339.2 M | 43.7 % | 85 % | 6.1 % | 48.0 % | 52.0 % |

  Roof shapes 0.7 %, roof colours 0.1 %. By place (a height or floors, of the buildings within 2 km
  of a city's centre or 15 km of a rural point, `PLACES` in the script): Manhattan 98.5 %, Chicago
  79.5 %, Los Angeles 97.7 %, rural Kansas 54.6 %, rural Vermont 84.9 %, Toronto 51.0 %, Montréal
  23.8 %, Vancouver 82.3 %, rural Québec 50.2 %, Paris 66.7 % (floors, from OSM), rural France
  23.5 %, London 49.3 %, rural England 62.6 %, Dublin 32.3 %, Madrid 26.2 %, rural Spain 87.8 %
  (floors, from IGN España), Lisbon 23.0 %, Tokyo 10.8 %, Osaka 70.1 %, rural Japan 0.5 %, Taipei
  31.2 %, Hong Kong 45.2 %, Singapore 49.8 %, San Juan 46.1 %, Honolulu 53.7 %.
- **Where they come from:** footprints from OSM 59 %, Microsoft ML Buildings 33 %, IGN España 4 %,
  Esri Community Maps 3 % (CC BY 4.0 with OSM waivers), `doi:10.5281/zenodo.8174931` (70 % of
  Taiwan's, 43 % of Hong Kong's, 6 % of Japan's), Google Open Buildings (Puerto Rico, Singapore,
  French Guiana), City of Vancouver. Heights from Microsoft ML Buildings 85 %, OSM 9 % (all of
  Japan's, Taiwan's, Hong Kong's, Singapore's and Puerto Rico's), USGS lidar 4 % and Esri 2 % (both
  the US's). Floors from OSM (10.2 M), IGN España (9.8 M, Spain's) and Esri (0.8 M, the US's).
- **Microsoft's heights are estimates, and flat.** Where a building has both, its height grows
  1.2 m a floor when Microsoft's (least absolute deviations: 1.21 m × floors + 2.7 m; 1.5 m a floor
  at 10 floors or more), 3.0 m when OSM's (+ 1.4 m), 2.9 m when USGS lidar's (+ 2.3 m), and 3.2–3.5 m
  a floor above 3 floors when Esri's. They hardly reach 20 m: 0.05 % of Microsoft's heights do, 0.87 %
  of the others (the UK's 99th percentile: 9.3 m from Microsoft, 40 m from OSM). So §2.3 ranks them
  after floors and calls them estimates; "measured" in this document means the others (22.5 M).
- **Height per floor, measured** (medians of height ÷ floors, Microsoft's left out): 1 floor 4.3 m,
  2 floors 3.7, 3–4 floors 3.25, 5–9 floors 3.24, 10 or more 3.33; Japan 4.8, 3.8, 3.23, 3.3, 3.25;
  the US 4.3, 3.35, 3.43, 3.63, 3.62. Spain's OSM heights are exactly 3.0 m a floor (derived from
  `building:levels`). The fits per country are in §2.3.
- **Heights' medians:** measured 5.8 m (p99 19 m): the US 5.5 (17.5), Japan 7.1 (24.1), France 7.0
  (52), the UK 7.6 (40), Taiwan 12 (130), Hong Kong 41.5 (252); Microsoft's 4.3 m (p99 9.7).
- **How this repo reads it now:** `dem/buildings.py --world` streams the release's bbox columns
  (~32 GB of its 277) into z8 tiles of boxes (`sources/buildings/2026-09-23-1/`, 11,834 tiles),
  which the units read for the roadside-buildings factor (`pipeline::buildings`). Heights, footprints
  and parts aren't read. That stays as it is.

### 2.3 Filling the missing heights

Each building's height (its top above the ground) is the first of these that it has, and it
records which (`s`, 0–5), for the hover and the "where the height comes from" colouring. The order,
the fits and the defaults are B0's, from the hold-out below:

0. **Measured:** Overture's `height` from lidar (USGS), OSM's `height`, Esri Community Maps or a city
   (Vancouver), taken when it is 2–700 m. A building part's base: `min_height`.
1. **Floors:** `num_floors` × the country's storey height + a roof allowance, fitted by least
   absolute deviations on the measured buildings that have both (§2.2): the US 3.10 m a floor
   + 0.90 m, Japan 2.82 + 1.87, the UK 3.21 + 1.59, France 2.95 + 1.60, Spain 3.00 + 0, Canada
   3.47 + 1.59, Portugal 3.00 + 1.00, Ireland 2.50 + 2.50, Puerto Rico 2.92 + 0.37, Taiwan
   3.32 − 0.96, Singapore 3.62 + 1.54, Hong Kong 3.32 − 1.32; elsewhere the coverage's 3.03 + 1.27.
   A part's base from `min_floor` likewise.
2. **Microsoft's estimate:** Overture's `height` from Microsoft ML Buildings, 2–700 m. After floors,
   which it puts at 1.2 m each (§2.2); 2.4 M buildings have both.
3. **Neighbours:** the median of the heights (by rules 0–2) of the buildings within 150 m whose
   footprint is between half and twice its own, when there are at least 5; else, for a footprint
   of 60 m² or more, of any footprint within 300 m, when there are at least 8. (B1: the 300 m
   stage takes any footprint's height, so a kiosk, a stair housing or a pole stood as tall as its
   area's median, a 2.5 m² footprint 22 m tall in Paris; smaller footprints go on to rules 4–5.
   The bound scored below.)
4. **GHSL, in high-rise cores:** the ANBH value of the 3″ cell holding the footprint's centroid, when
   it is 20 m or more; a footprint under 60 m² takes at most 4 m. Sampled on 2026-10-05: Midtown
   Manhattan 31.5 m, Lower Manhattan 35.2, downtown Toronto 42.8; Brooklyn's row houses (18.0), Back
   Bay (17.3), the Plateau in Montréal (12.7) and Montpelier (15.0) fall through to rule 5.
5. **Size and kind:** the measured median height of its class, in its country where 200 of the
   class are measured, else in the coverage: churches and cathedrals 8.8 m; sheds, garages, carports
   and huts, or under 30 m²: 3.3 m; houses and residential kinds, or under 250 m²: 6.0 m;
   250–2,000 m²: 6.4 m; larger: 9.1 m (the coverage's; the US's 3.3, 5.5, 6.2 and 8.9 m, Japan's
   3.2, 7.5, 9.0 and 14.8 m, France's 5.0, 7.0, 10.0 and 12.0 m and its churches 13.0 m). Known,
   for later: Hong Kong's own two classes, 38 m to 250 m² and 55 m to 2,000 m², sit between the
   coverage's 3.3 m under 30 m² and 9.1 m over 2,000 m² (fewer than 200 measured there), so a
   footprint across either bound goes from a hut to a tower or back.

**Checked, not guessed (B0):** a tenth of the buildings with a height (by a hash of their id:
14.8 M, 2.25 M of them measured) were held out, filled by each rule as if unmeasured (the
neighbours' rule without the held-out heights) and scored against their measured height: the median
/ 90th percentile of |estimate − measured| in metres, and the share of them a rule answers.

| | held out | 1 floors | 3 neighbours | GHSL, any cell | size, the first defaults | 5 size, fitted | 4–5 | 1–5, the first order | 1–5 |
|---|---|---|---|---|---|---|---|---|---|
| US | 1.76 M | 0.9 / 3.0 (9 %) | 0.6 / 2.7 (99 %) | 2.4 / 5.8 | 2.0 / 4.1 | 1.5 / 4.3 | 1.5 / 4.4 | 0.6 / 2.9 | 0.6 / 2.8 |
| Japan | 421 k | 0.9 / 3.1 (50 %) | 0.9 / 3.5 (100 %) | 2.8 / 9.0 | 1.4 / 3.8 | 1.0 / 4.2 | 1.1 / 4.7 | 0.9 / 3.4 | 0.9 / 3.4 |
| France | 14 k | 1.6 / 4.5 (12 %) | 1.1 / 6.1 (96 %) | 3.1 / 10.5 | 2.0 / 11.0 | 2.0 / 10.0 | 2.0 / 10.0 | 1.5 / 6.0 | 1.5 / 6.0 |
| UK | 8 k | 1.8 / 3.2 (61 %) | 0.1 / 3.5 (97 %) | 3.2 / 6.0 | 1.5 / 4.5 | 1.2 / 4.3 | 1.2 / 4.3 | 1.0 / 3.4 | 1.0 / 3.4 |
| Canada | 9 k | 1.1 / 2.6 (40 %) | 0.6 / 2.9 (99 %) | 2.2 / 6.5 | 2.0 / 3.7 | 1.8 / 3.8 | 1.8 / 3.9 | 0.9 / 2.9 | 0.9 / 2.9 |
| Puerto Rico | 20 k | 0.7 / 3.1 (2 %) | 0.5 / 2.7 (100 %) | 4.2 / 7.4 | 3.1 / 4.5 | 0.7 / 2.9 | 0.7 / 2.9 | 0.5 / 2.7 | 0.5 / 2.7 |
| Taiwan | 1 k | 0.6 / 5.7 (75 %) | 3.0 / 13.7 (96 %) | 5.4 / 23.0 | 5.5 / 31.0 | 5.0 / 26.4 | 6.0 / 23.1 | 0.6 / 8.0 | 0.6 / 8.3 |
| all | 2.25 M | 0.9 / 3.0 (18 %) | 0.6 / 3.0 (99 %) | 2.5 / 6.4 | 1.9 / 4.1 | 1.5 / 4.3 | 1.5 / 4.5 | 0.7 / 3.0 | 0.7 / 2.9 |

The first order (the plan before B0) was measured, floors, neighbours, GHSL in any cell with
buildings, then size and kind at 3, 6.5, 9, 10 and 15 m, Microsoft's estimates counted as measured;
scored here with B0's floors. Spain's measured heights are OSM's floors × 3 m, so its floors fit them
exactly (left out above). Every country and rule, by the true height, and against Microsoft's
estimates too: `dem/bldmeasure.py`'s report phase. What set the rules:
- **Floors and neighbours** are the good rules: 0.6–0.9 m in the median. Floors come first although
  the neighbours' median is lower: they alone keep tall buildings tall (of the held-out buildings
  20 m or more, they put 60 % at 20 m or more; the neighbours 24 %).
- **GHSL is worse than the size rule** nearly everywhere (2.5 against 1.5–1.9 m; Japan 2.8 against
  1.0–1.4), but of the two only GHSL knows tall buildings: of the held-out buildings 20 m or more it
  puts 30 % at 20 m or more, the size rule 1 %. GHSL only in its 20 m cells keeps the 30 % at the
  size rule's accuracy (rules 4–5: 1.5 / 4.5 m, where GHSL in any cell then size was 2.5 / 6.4;
  Japan 1.1 / 4.7 against 2.8 / 9.0). A GHSL factor fitted per country (0.47–1.30) did less (1.8 /
  5.4 m).
- **The size rule's first defaults** were high for the many small buildings (+2.0 m under 6 m) and
  the churches; the fitted ones have no bias (1.5 / 4.3 m against 1.9 / 4.1).
- **The neighbours' rule stays as planned:** its 150 m stage answers 91 % of the hold-out at 0.6 /
  2.7 m, the 300 m fallback 7 % at 1.6 / 5.7 m (GHSL, scaled per country, on the same buildings:
  2.4 m); without the footprints' condition the 150 m stage is 0.9 / 3.5 m.
- **Tall buildings come out low**, whatever the rule: the held-out buildings 20–40 m tall by 12 m in
  the median, those of 40 m or more by 22 m (by 35 m where floors aren't known). The z12–13 skyline
  is the measured and the floors-known towers, and GHSL's high-rise cores (§2.5).
- **The hold-out flatters the neighbours:** a measured building's neighbours are measured more often
  than an unmeasured one's. Rule 3 answers 99 % of the held-out buildings but 52 % of the buildings
  with no height or floors; the size rule fills a quarter of the coverage.
- Scored against Microsoft's estimates as well (14.8 M held out, as the plan first had it), the
  chain is 0.65 / 2.4 m, the first order 0.63 / 2.2: Microsoft's flat estimates agree with flat
  guesses, which is why they aren't the yardstick.

Every building in the coverage, by the rule that gives its height:

| | 0 measured | 1 floors | 2 Microsoft's | 3 neighbours | 4 GHSL | 5 size |
|---|---|---|---|---|---|---|
| US | 11.3 % | 1.0 % | 60.3 % | 19.5 % | 0.0 % | 8.0 % |
| France | 0.3 % | 1.1 % | 18.0 % | 59.8 % | 0.0 % | 20.8 % |
| Japan | 7.8 % | 1.2 % | — | 11.0 % | 1.3 % | 78.8 % |
| UK | 0.3 % | 6.8 % | 43.1 % | 44.0 % | 0.0 % | 5.7 % |
| Spain | 0.7 % | 59.6 % | 4.4 % | 24.2 % | 0.0 % | 11.0 % |
| Canada | 0.6 % | 2.1 % | 47.1 % | 28.0 % | 0.0 % | 22.2 % |
| Portugal | 0.2 % | 1.9 % | 2.0 % | 8.0 % | 0.0 % | 87.8 % |
| Taiwan | 0.5 % | 3.4 % | — | 10.6 % | 3.0 % | 82.5 % |
| all | 6.6 % | 5.0 % | 36.4 % | 26.9 % | 0.2 % | 25.0 % |

Estimated (rules 2–5): 88 % of the coverage's buildings; Japan's 91 %, France's 99 %, the UK's 93 %,
Spain's 40 %. Japan's cities get measured heights from PLATEAU in B4, France's from BD TOPO. The
fill's version is in the buildings step's key (§3.2), so a change rebuilds every tile and nothing
else.

These are B0's counts, before rule 3's 60 m² bound (B1).

**Rule 3's bound, scored (B1),** as B0 scored the rules: on the pilot's five tiles (§5.1; 78 M
buildings), a tenth of the measured heights held out (636 k: a hash of the centroid, hidden from
the neighbours' rule, a held-out building with floors keeping their height) and each estimated as
if unmeasured (`bld::job`'s `holdout`, run by hand). Where stage 1 fails and stage 2 answers, stage
2 is off by 2.2–2.7 m in the median under 30 m² (p90 5.2–6.7 m, 6–12 % by more than two storeys),
rules 4–5 by 0.3–0.5 m (p90 2.1–4.3 m, 4–8 %); from 30 to 100 m², stage 2's median is 0.1–0.3 m
nearer and its tail farther (p90 4.0–7.0 m against 3.7–5.3 m); above, the two are alike. Every
held-out building by its class, the whole chain (median / p90 of the error, the share off by more
than two of its country's storeys):

| Footprint | held out | no bound | 30 m² | 60 m² (kept) |
|---|---|---|---|---|
| under 30 m² | 88.8 k | 0.5 / 3.2 m, 2.56 % | 0.4 / 2.5 m, 2.21 % | 0.4 / 2.5 m, 2.21 % |
| 30–60 m² | 115.9 k | 0.9 / 3.5 m, 2.45 % | 0.9 / 3.5 m, 2.45 % | 0.9 / 3.5 m, 2.35 % |
| 60–150 m² | 303.7 k | 0.8 / 3.0 m, 1.87 % | the same | the same |
| 150 m² and more | 127.8 k | 1.3 / 5.7 m, 9.39 % | the same | the same |

60 m² gives the small footprints' lowest errors and leaves the larger classes as they were; 100 m²
gains nothing more (60–150 m²: 1.86 against 1.87 %). The buildings each bound moves from rule 3 to
rules 4–5, of the five tiles' 78 M: 30 m² 8.4 M (10.7 %), 60 m² 11.6 M (14.8 %), 100 m² 13.5 M
(17.3 %), 98–99 % of them to the size rule. By tile, 60 m² took Paris's z6 tile from 59 % by the
neighbours and 23 % by size to 28 % and 53 %, Barcelona's from 55 % and 20 % to 25 % and 50 %, New
York's from 22 % and 4 % to 12 % and 14 %, Vermont's from 22 % and 3 % to 12 % and 14 %, Kantō's
from 14 % and 73 % to 10 % and 77 %: sheds, garages, annexes and kiosks, which the size rule puts
at its sheds' height under 30 m² (3–5 m) and its houses' above (6–8 m in most countries), where the
300 m median gave them the area's, its towers' in a city.

### 2.4 Heights and the roadside factor

The roadside-buildings factor (scenic metrics) stays on the boxes and ignores heights. Building
heights in the near-field horizons and the viewshed tool are a separate phase-7 item: they would
change every unit's key (every unit rebuilt), so they're decided on their own. The normalized files
(§3.4) are made so that a z11 grid of building heights can be drawn from them then.

### 2.5 Sizes

| | |
|---|---|
| downloads (§2.6) | 61.8 GB: Overture 60.5 GB (103 files), GHSL 1.28 GB (91 tiles) |
| buildings in the coverage | 339 M and 2.8 M parts (B0: centroids in the coverage + 1 km) |
| z6 tiles meeting the coverage | 380; 290 hold a building's centroid, 25 over 5 M: the densest 6/56/25 (Tokyo to Osaka, 30.3 M), 6/32/22 (Paris and central France, 15.3 M), 6/18/24 (New York to Washington, 14.6 M), 6/32/23 (southern France and Catalonia, 12.6 M), 6/32/21 (northern France and England east of Greenwich, 11.3 M), 6/31/21 (southern England and Wales, 10.5 M), 6/55/25 (western Japan, 10.3 M), 6/17/25 (the Carolinas, 10.2 M), … |
| normalized buildings (`work/bld/`) | 38–50 B a building, zstd'd (B1's pilot: Kantō 1.15 GB for 30.3 M, Paris 0.67 GB for 15.3 M): ~14 GB in all |
| tiles, encoded in B0 | 178 tiles encoded as §3.4 has them (gzip level 6), the fullest and heaviest by the estimate and a sample: 17.7 B a building at z14 (the median; 10.4–27.9), 35.7 at z13, 20.0 at z12. The fullest z14 tile holds 26,462 buildings (Kyoto, 14/14369/6488: 278 KB); 15 of the 30 heaviest by the estimate are over 300 KB (the rest of the tiles under ~290 KB), at most 490 KB (Valencia, 14/8174/6234: 24,183 buildings), all in old towns drawn on Spain's cadastral parcels (Barcelona's six, Valencia's, Granada's, Málaga's, Córdoba's, Santander's, the Garraf's) or in Tokyo's wards (three, 303–310 KB). Simplified to a grid unit (0.6 m), as z12–13 are, they lose ~18 % (490 → 400 KB); keeping only `h` and `k`, ~2 %. The fullest z13 tile: 17,842 (Valencia: 279 KB; Barcelona's 284 KB); z12: 10,339 (Manhattan: 148 KB; Singapore's 10,204, 176 KB) |
| tiles, estimated | each building's command bytes (§3.4's quantizing, in the scan), calibrated by the encoded tiles: 5.69 GB at z14 (2.23 M tiles: median 21 buildings, p99 2,436), 0.13 GB at z13 (194 k tiles, 3.5 M buildings), 4 MB at z12 (4,679 tiles, 208 k): 5.8 GB; the largest hi pack 6/56/25 0.47 GB |
| tiles, built (B1's pilot, the packs served) | 6/56/25 (Kantō): 30.3 M buildings and parts, 25,029 tiles, a 0.41 GB pack (13 B a building at z14); 6/32/22 (Paris): 15.3 M, 71,485 tiles, 0.27 GB; 6/18/24 (New York): 14.6 M, 47,658 tiles, 0.27 GB; 6/32/23 (Barcelona): 12.6 M, 50,825 tiles, 0.22 GB; 6/19/23 (Vermont and Boston): 6.1 M, 35,634 tiles, 0.12 GB; 6/3/28 (Oahu): 0.2 M, 410 tiles, 3.7 MB. 13–20 B a building at z14, copies included, as B0 estimated |
| tiles and files, built (B2, the M1, 2026-10-08: the agent's command lines, the whole coverage, a scratch root) | 6/18/23 (upstate New York, Vermont's west): 6.03 M rows read, 5.74 M buildings and 0.29 M parts, `bldprep` 79 s and 1.15 GB, a 274 MB work file, `bldtiles` 13 s and 0.88 GB, a 116 MB pack (57,689 tiles); 6/19/23 (Vermont, New Hampshire, Boston): 6.14 M, 72 s and 1.24 GB, 306 MB, 13 s and 0.94 GB, 124 MB; 6/32/22 (Paris): 15.29 M, 183 s and 2.39 GB, 671 MB (the same bytes as B1's on the build Mac), 65 s and 1.17 GB, 269 MB. 44–50 B a building in the work files, 18–20 B in the packs. This Mac (the M1, loaded 20–40) read ~80 k rows a second where the build Mac read 210–450 k in B1 |
| the coverage's 380 tiles, from these (B2's estimate) | the row groups meeting them hold 489 M rows, of which a tile keeps 82–97 % (the pilot's and B2's tiles): ~440 M buildings and parts in the work files (those beyond the coverage in its tiles too), 17–22 GB at 38–50 B each; the packs ~342 M (the coverage's), 4.6–6.9 GB at 13.5–20 B each (B0's 5.8 GB between) |
| an app Mac's mirror | +4.6–6.9 GB (the packs; the work files aren't served) |

### 2.6 Downloads (done 2026-10-06, for the 88 regions: 194 files, 61.8 GB)

`dem/bldfetch.py` fetches both sources onto the NAS, whole files as the sources have them:
- Overture's files with a row group meeting the coverage grown by 20 km (both types), into
  `sources/overture/2026-09-23-1/theme=buildings/type=<type>/<file>`, with `buildings.json` (each
  file's size, ETag, rows, row groups and box, and the coverage they were chosen for) and
  `footers.json.gz` (every file of the release's row-group boxes);
- GHSL's 10° tiles meeting it, into `sources/ghsl/R2023A/<file>.zip`, with `index.json`.

The coverage comes from the map's server (`/api/regions`, `/api/areas/<id>`), so adding a region and
running it again fetches only the new files, while the release is on S3. Each file is written to a
temporary name, resumed after an interruption (a range request; for S3 only while its ETag is the
same), checked (an Overture file's ETag, the MD5 of its 64 MiB parts; a GHSL zip's CRC-32s) and
renamed into place; a file in place at its listed size is skipped. Two transfers at once, at most
3 MB/s in all: the line gave ~2–4 MB/s from S3 on 2026-10-05, and the build needs some of it. The
log is the NAS's `state/logs/bldfetch.log` (a line a file, a progress line a minute). The first run
fetched all 194 files (61.8 GB) in 6 h 48 min, from 18:54 on 2026-10-05 to 01:42. After anything
stops it (a restart, the NAS away too long), the same command, from the repo, goes on from where it
was:

```
nohup caffeinate -s uv run --project dem python dem/bldfetch.py \
  --root /Volumes/personal/projects/scenic-roads \
  >> /Volumes/personal/projects/scenic-roads/state/logs/bldfetch.log 2>&1 &
```

Why the coverage, not the world: the map draws buildings only in the coverage, and the world is
277 GB (four times as much). A region added after 2026-11-25 whose files aren't here waits for the
next pinned release (§5.2).

## 3. Pipeline

### 3.1 Steps

| Step | Target | Reads | Writes | Runs on |
|---|---|---|---|---|
| `bld-fetch` | the release | S3, JRC, the coverage | `sources/overture/<release>/`, `sources/ghsl/R2023A/` | the build Mac, a network job (its second slot's) |
| `bldprep` | a z6 tile T | Overture's row groups meeting T, the GHSL tiles meeting T | `work/bld/6-x-y` | any Mac with the NAS |
| `bldtiles` | a z6 tile T | `work/bld/` of T and its 8 neighbours, the coverage over T | `layers/buildings/hi/6-x-y` | any Mac; its z8 areas as tasks for pages |

(`bldtiles` is the design's `buildings T`: `buildings` is already the roadside buildings' step,
`pipeline::buildtiles`, in scenic-build and the agent.) Built in B1:
`scenic-build bldprep <6/x/y …> [--dem dir]` and `scenic-build bldtiles <6/x/y …> [--pass d]
[--regions dir]`; in B2, `scenic-build bld-fetch [--pass d] [--dem dir]`, and the agent runs all
three (§3.2–3.3).

- **`bld-fetch`** is `dem/bldfetch.py` (§2.6), run by `scenic-build bld-fetch` with the coverage
  written as GeoJSON (`--coverage`, from the pass's outlines, as the rail feeds have it). The agent
  runs it as a network job (one of the second slot's, with the heritage chain and the rail feeds:
  `agent::LIGHT`) when its key changes (§3.2). It skips what's there, so a run is a listing of S3
  and of JRC's tiles. Once the release has left S3 (§5.2), it goes by the footers read before: what
  the coverage needs and is here passes, a file it needs and lacks fails the job (the tiles it
  would have fed wait for the next pinned release).
- **`bldprep T`**, per z6 tile within 1 km of the coverage (so a tile's neighbours within the
  fill's 620 m are read too) that has a downloaded row group or GHSL tile meeting it (380 tiles
  for the 88 regions, 344 with a row group): `dem/bldprep.py` reads the row
  groups meeting T from the downloaded files (pyarrow, four at a time), their rows whose box meets
  T (parts: T grown by 0.02°, so an outline finds its parts), and the GHSL windows under T
  (rasterio, out of the zips), and writes their columns to stdout, the geometry as Overture's WKB;
  Rust (`scenic-build bldprep`, `pipeline::bld::prep`) reads the stream as it comes, parses the WKB,
  computes each centroid (area-weighted, as GEOS) and area in f64, rounds to E7, keeps the
  buildings and parts whose centroid is in T (underground ones left out), samples GHSL at the
  centroid, sorts by (z14 tile, id) and writes the normalized file (§3.4). Python only decodes;
  every number that ends up in a file is computed in Rust. Reads ~60 GB over all tiles, once per
  release.
- **`bldtiles T`**, per z6 tile meeting the coverage (its buffers included), a z8 area at a time
  (`pipeline::bld::job`):
  the buildings of T that touch the coverage, heights filled (§2.3, `pipeline::bld::fill`; it reads
  the buildings within 620 m beyond the area, T's or its neighbours', by their z14 blocks), the
  z12–14 tiles encoded (§3.4, `pipeline::bld::tiles`), the hi pack written. A building within 310 m
  beyond the area that reaches into its tiles is filled there too, the same as its own area fills
  it (every building within 300 m of it read), for its copies (§3.4).
  Pure: its output is a function of its inputs' bytes. A z6 tile with no building in the coverage
  loses its pack.

Both are new `scenic-build` steps and agent steps; the units, the roads' chain and the landmarks
don't change, and no unit's key reads them.

### 3.2 Keys and versions

In `agent::build` (B2), beside the others (`bld_targets`, `bld_work`):
- `BLDPREP_V = 2`, `BUILDINGS_V = 2` (the fill's rules, fits and defaults, and the tiles, are in
  `BUILDINGS_V`; both 2 since B1's review: an outline whose parts are all underground is no longer
  flagged as having parts; rule 3's bound, the copies, the walls): defined in `pipeline::bld` (B1),
  as `TREES_V` is in `pipeline::treepacks`; `BLD_FETCH_V = 1` in `agent::build`.
- `Keys` has `bldprep` and `bldtiles` (the design's `buildings`: that's the roadside buildings'
  worldwide step, kept with the lo keys), maps by z6 tile as `unit` and `pack` are;
  `Keys::map`, `recorded` and `record` take them, and a prune forgets them ("bldprep 6/x/y",
  "bldtiles 6/x/y").
- **`bldprep T`'s key:** `bldprep {BLDPREP_V}`, the release, and for each downloaded file with a row
  group meeting T (a parts file's: T grown by 0.02°, as bldprep.py reads it) its name, ETag and
  those row groups' indexes, and the GHSL tiles meeting T by name and size: `pipeline::bld::sources`
  reads `buildings.json`, `footers.json.gz` and GHSL's `index.json`, a file listed only when its
  footer has its ETag, and `agent::input_digests` puts each tile's as "bldprep 6/x/y" in the plan's
  `inputs` (worked out again only when an index's size or time changes, a file far from a tile
  passed over by its box first; a listed file whose footer is missing or another object's named
  "stale" in the reads of the tiles its listed box meets, where bldprep.py fails on it, and passed
  over elsewhere, as bldprep.py passes it over; no GHSL index, no GHSL tile; "bld-release" says whether
  they read: the release, "" before any download, "?" when they can't be read now, the chain then
  waiting and the status saying why). A file fetched later (the coverage grew) changes the key of
  the tiles it meets; an unchanged result keeps its content name, so nothing after it reruns.
- **`bldtiles T`'s key:** `bldtiles {BUILDINGS_V}`, the content names of `work/bld/` for T and its
  8 neighbours ("-" for none), and `Coverage::shapes_key` of T's box grown by 1 km (B1): the
  coverage's shapes meeting the box in the recipes' order, each as its fingerprint there with its
  country (`Shape::country`). Not `Coverage::fingerprint`, which sorts and dedups by geometry: a
  building's shape is the first whose outline holds its centroid, else the first whose buffer does
  (else a vertex's, likewise), so the order decides where outlines or buffers overlap, and a
  region renamed (the recipes go in id order) can change a building's country with no geometry
  changed. The fill's fits go by that country, from the pass's outline of an `osm:` region
  (`Outlines::country_code`: its own ISO 3166-1 code, a territory's from its subdivision code where
  ISO 3166-2 lists it by its own (US-PR, CN-HK, FR-GF, FR-PM), else the country it lies in; always
  two letters); the coverage's fits where none is known. B1's probe of the 88 regions' 122 outlines
  gave B0's country for each, and for 44 points at borders and in enclaves and territories
  (Monaco, Gibraltar, Llívia, Windsor, Derby Line, Tui, Ceuta, the Canaries, Hong Kong).
- **`bld-fetch`'s key:** its version, the release and `coverage_all`; kept with the lo keys under
  its own name, as `rail-feeds` is. Not the footers' digest the design had: the job reads and
  writes `footers.json.gz`, a release's files never change (the release names them), and a key on
  what a job writes runs it again for its own sake.
- The pinned release is `buildtiles::RELEASE` for both the roadside and the 3D buildings: a new one
  re-keys every unit (the roadside index) and every buildings tile together, never a mix.

### 3.3 Order, rounds, the chain

Built in B2 (`agent::build::bld_work`, listed by `plan`):
- **A fourth chain**, beside the roads', the trains' and the landmarks': it reads no unit and no
  terrain (the map puts buildings on its terrain), so it runs from the start: `bld-fetch` when its
  key changed; a prune of the normalized files and packs no target has any more (the coverage
  shrank: "bldprep 6/x/y", "bldtiles 6/x/y"; the files only while the sources' indexes read, else
  every tile would seem to read nothing); every stale `bldprep T` beside the fetch (a file fetched
  later changes the keys of the tiles it meets: those run again); `bldtiles T` once T and its 8
  neighbours are prepared as they'll stay (none of them a stale `bldprep` target), and once the
  sources are here. Its work is listed after the landmarks' in `plan` (the build Mac's first job
  takes it when the regions' work is done or waits), each step's tiles in the regions' order (the
  first region in the order they're built whose coverage meets the tile, then `spatial_order`), so
  the buildings of the region being built come first. With no "bld-release" in the plan's inputs
  (a caller that didn't read the sources) or "?" (unreadable now), no buildings work at all.
- **The second job** (`agent::SECOND`) takes `bld-fetch` with the network steps (`agent::LIGHT`:
  while the Mac is in use too), and `bldprep` and `bldtiles` last, after units and slope, as CPU
  work (not while the Mac is in use). Never `bldprep` beside another `bldprep` (`agent::NAS_READS`:
  each reads up to ~3 GB of the NAS's parquet), nor anything beside a job that runs alone (the OSM
  pass, the pass's worldwide jobs, the water, GC: `agent::ALONE`), so never beside the planet's
  reads.
- **Rounds:** buildings don't hold a round, and a region's readiness (`ready`) doesn't wait for
  them: their packs go out with the next round's catalog, as the trains' and landmarks' outputs do.
  After the last unit a catalog follows any chain's change, but while the 3D buildings' chain has
  work left, a round (and its catalog) at most an hour after the last began: the worldwide build's
  ~25 `bldtiles` jobs don't each make a catalog. Never for a region done that the map lacks: its
  round begins at once. Nor for the fetch alone (it changes nothing served, and one failing once
  the release has left S3 would hold every catalog to the hour). A round fixes the buildings'
  packs as it begins, with the units (`out::AS_OF_OUTPUTS`), so a `bldtiles` job ending meanwhile
  doesn't change its catalog; they go out with the next. A region can reach the map before its
  buildings, which follow with a later round.
- **Batches:** a fixed number of z6 tiles a job, as the other steps have (`agent::batch_size`):
  `bldprep` 8, `bldtiles` 16; 2 and 4 while the regions' own work (terrain, slope, tree cover,
  units) is left (`agent::job_size`), so a job of theirs beside it or a helper's ends within
  minutes. The design weighed them by building counts; B1–B2's runs make a dense
  tile's `bldprep` about a minute on the build Mac (68 s for Kantō's 30.3 M rows) and its
  `bldtiles` about 10 s, so a fixed count keeps a job within minutes, as leases and pausing want.
- **Status:** the checklist's line "Raising the 3D buildings" (`build::BUILDINGS`: the tiles'
  normalized files and tiles, done of all, known once the sources read; next, the fetch and each
  step's tiles); `label("bld-fetch")` "Fetching the 3D buildings' sources", `label("bldprep")`
  "Reading the regions' buildings", `label("bldtiles")` "Raising the 3D buildings"; parts and
  progress lines as the other steps' (row groups read; z8 areas done). The forecast takes the chain
  with the others (`chains_left`: bldtiles after bldprep, `forecast::chain_deps`); first guesses
  6 s a `bldprep`, 2 s a `bldtiles` (a tile on average: most are sparse; §5.1's estimate), 5 min a
  fetch until timed. A helper is counted on for both (the forecast's shared steps are
  `claims::SHARED`, no longer a copy of its list). The worker page names the steps.

### 3.4 Formats

**Normalized buildings** (`work/bld/6-<x>-<y>.<h>.sect`, RDSECT v1, content-named, in the
manifest; a work file, not served; docs/formats.md has the bytes). Meta `{"fmt": 1, "tile":
"6/x/y", "release", "buildings", "parts", "srcs", "classes", "subtypes", "roofs", "ghsl": "R2023A",
"read"}`: the strings the codes index (each list sorted), and what bldprep.py read.
- `index`: `(u64 z14 tile key, u64 offset, u32 len, u32 count)` per block, sorted by key.
- `blocks`: a zstd block (level 9) per z14 tile (the tile of the centroid): the tile's records
  sorted by id (Overture's UUID as a number), column by column:
  - centroid (i32 E7 × 2), footprint area (m², f32), polygon, ring and vertex counts (rings per
    polygon: a MultiPolygon's exteriors), vertices (i32 E7, each ring's first absolute, the rest as
    deltas; the closing point left out);
  - height and base (u16 decimetres, 0 none), floors and base floor (u8, 0 none), Overture's class,
    subtype and roof shape as codes (u8 each), flags (u8: a part; a building whose parts were read),
    the height's source dataset (u8, into `srcs`), GHSL's value at the centroid (u16 dm, 0 none),
    and the OSM id where OSM gave the footprint (u64, type in the top bits; 0 none).
- Reading a neighbour's edge is a few blocks; a page's task is a slice of blocks (§3.6).

**Tiles** (MVT 2.1, gzip'd, extent 4096, layer `b`):
- One feature a building or part: its polygon(s), outer rings and holes, **whole, in the tile
  holding its centroid** (not clipped: its coordinates may run past the extent). MapLibre's
  extrusion then has one centroid a building, so no step at a tile edge on a slope, and nothing is
  drawn twice; the map keeps the z14 tiles whole above z14 (§4.1).
- **Copies for the flat footprints:** a building reaching into other tiles of its zoom is copied,
  whole, into each (`o` 1), from up to 310 m beyond a z8 area's edge. A fill is cut at its tile's
  edge, so the flat footprints of a building past its tile were cut on the tile line; the
  extruded layer leaves the copies out, the flat and pick layers take them. B1's pilot: 1.1–1.6 %
  more features, 1.6–2.3 % more bytes.
- **No edge parallel to an axis beyond the extent:** MapLibre's extrusion takes such an edge for a
  clipped tile's cut and draws no wall on it (`isBoundaryEdge`), and in whole buildings they're
  walls: 0.12–1 % of buildings lost one in B1's first packs. Such an edge gets points between its
  ends, every other one a unit off it, outside the ring (for a hole, into the building): at
  MapLibre's subdivision lines (every 2,048 units on the globe, where it cuts edges and rounds the
  cuts) and midway; a unit-long one a point a unit off; a slanted edge out there a point at each
  line, where rounding would have made a piece parallel. Its ends stay. Where the points would
  cross the ring or its polygon's other rings, they go inside the ring instead, else the edge is
  left as it is (its wall not drawn): in every triangle and quadrilateral on small grids past an
  edge (a unit test), 1.6 %, each a unit-long edge in a notch a unit wide. Left in B1's packs: none
  on the flat map, 5–8 features in a sampled million on the globe (a cut's rounding elsewhere); and
  five holes of no area in Vermont's z14, a unit across, made by the first rule (a unit away from
  the tile), which the rule outside the ring can't make (packs not rebuilt since).
- Quantized to the tile's grid (z14: 0.6 m at the equator); repeated points dropped, rings that
  collapse dropped; at z12–13, simplified to one grid unit. Known: from zoom 18 the grid shows, a
  unit a few pixels: curved and slanted walls in small sawtooth facets, and a wall past its tile's
  edge with the points above a unit off its line.
- Properties: `h` the top (dm), `m` the base (dm; parts; left out when 0), `s` the height's source
  (0–5, §2.3), `f` floors (when `s` is 1), `c` the kind (0 unknown, 1 residential, 2 outbuilding,
  3 commercial, 4 industrial, 5 religious, 6 civic, 7 agricultural, 8 transport, 9 other), `k` (1 a
  part, 2 an outline with parts: drawn by the flat layer only), `o` (1 a copy). No feature ids, no
  names (B1).
- In a tile, features sorted by their centroid's Morton code, then id.
- Encoded by `pipeline::bld` over `names::mvt`, not `vtgen`, which clips features at the tile's
  edges and simplifies at 3 units (1.8 m at z14: a house's corners). gzip level 6 (flate2).

**Packs:** `layers/buildings/hi/6-x-y` (RDPACK v1, encoding `mvt`, blobs gzip'd): z12–14 of the z6
tile. No lo or root packs. The catalog lists the layer `buildings`, encoding `mvt`, zooms 12–14
(`layer_zooms` and the encoding match in `scenic-build`'s catalog).

### 3.5 Served

- `/tiles/buildings/{z}/{x}/{y}`: the pack's tile as stored (`tiles::plain`, gzip, ETag the blob's
  hash, `?v=` the layer's version: `buildings.tiles` in `/api/meta`'s versions), 204 where there's
  none. No names attached. Built in B1, with the catalog's layer `buildings` (encoding `mvt`, zooms
  12–14); the app reads the tiles from a host of their own (`buildings.localhost` on this Mac).
- **Mirror** (B2): a copy group of its own after the other hi packs (`store::mirror::groups`,
  group 5), so a Mac's mirror has the roads and terrain first; on a Mac whose budget runs out first
  (the M1's) buildings are left out, and the server reads them from the NAS; when room runs short,
  they're let go first within their class (before the never-used roads and terrain). They're never
  essentials (no root or lo packs); a kept area keeps them as every layer's hi packs (plan.md §4).
- **Devices** (B2): the iPad's service worker keeps versioned tiles it has shown (12,000 files at
  most); the building tiles have a cache of their own (`bld`, the last 2,000 files: a city's z14
  tiles are 50–300 KB, a view's ~20), so a city's buildings don't crowd out its roads and terrain.
- **Credits** (B2, `pipeline::rules::CREDITS`): "3D buildings: Overture Maps Foundation buildings,
  release 2026-09-23.1 (© OpenStreetMap contributors, Microsoft, Esri Community Maps, USGS, IGN
  España, Google and others; each building's sources in the release), ODbL 1.0"; "Building heights
  where none are known: GHSL GHS-BUILT-H R2023A (EC JRC), © European Union, 1995-2026, CC BY 4.0";
  both for anywhere, so in every catalog's © Credits (a test holds the release named there to
  `buildtiles::RELEASE`).
- Nothing is published or redistributed: the tiles stay on the NAS and the owner's Macs and devices
  (plan.md §3, the README's Terms).

### 3.6 Sharing the work

- **Older apps:** an app from before B2 drops the steps' records when it saves `jobs.json` and
  refuses a hand-off of them: both Macs run B2's app before the pool's lead may move (pool.md
  §6.1). From B2 on, `Keys` keeps records it doesn't know (`other`).
- **Helper Macs** (today's M1; any member in `docs/pool.md`): both steps are shared steps (B2,
  `agent::claims::SHARED`, last in its order), offered from the far end as terrain and units are;
  a hand-off may save only its tiles' files (`coord::saves`: `work/bld/6-x-y` for `bldprep`,
  `layers/buildings/hi/6-x-y` for `bldtiles`). `bldprep` needs the
  NAS (it reads up to ~3 GB of row groups a tile) and, in B1's pilot, 5.1 GB of memory at most for
  the densest tile (Kantō; 0.4–2.8 GB for the others); `bldtiles` holds a z8 area at a time (3.3 GB
  at most for Kantō's run, 0.4–1.3 GB for the others). Each target's memory is learned
  (`SCENIC_COSTS`: both steps note their targets' costs, `bldprep 6/x/y` and `bldtiles 6/x/y`);
  first guesses, from B1's six runs: 0.3 GB + 160 B a row read for `bldprep` (each run within
  0.16 GB of it; Kantō's 30.3 M rows 5.08 GB), 0.25 GB + 280 B a building of its largest z8 area
  for `bldtiles` (each within 0.11 GB; Tokyo's area, 10.8 M, 3.3 GB). The agent offers each target
  with them (`agent::bld_peak`), by the rows of the row groups its `bldprep` reads
  (`bld::sources`: 8–20 % over the rows it keeps, since a row group reaches past the tile), its
  largest z8 area taken as two fifths of them (Kantō's was 36 %): so Paris's 6/32/22 is offered at
  2.8 GB and 2.0 GB, and took 2.4 and 1.2 GB on the M1 (2026-10-08); Vermont's two tiles 1.3–1.4
  and 1.0 GB, and took 1.15–1.24 and 0.88–0.94.
- **Pages** (`docs/workers.md` §3, built in B3: `pipeline::bld::task`): under the agent (the
  coordinator's address in its environment, `offload::Offload::from_env`), a `bldtiles` job offers
  some of each tile's z8 areas as tasks of kind `bldtile`, as a unit job offers its tail: while
  workers that take them are around (`/task/workers`), up to one per worker and three at once, from
  the far end of the tile's list (the job works from the near end, so an area out has the longest
  before its turn), topped up before each area it makes itself. A task's files, cut on the build
  Mac (docs/formats.md): of T's and its neighbours' work files only the blocks the area reads (its
  own and those within 620 m around it), their bytes as stored, and the coverage's shapes that can
  answer for its points, whole, in the recipes' order (France's outline: 0.6 MB). The program
  `bldtile` (`/work/prog/bldtile.wasm` in a page, the app's `bldtile` on a helper Mac:
  `scenic run-task`) writes the area's z12–14 tiles (an RDTILES archive) and its summary; the job
  puts them into the pack in the area's turn. Nothing waits on a worker: in its turn an area no one
  took is taken back and made here, one a worker holds is raced here, a worker's result is taken,
  or checked (a worker's first three, then one in eight) against the job's own run byte for byte,
  a difference marking the worker bad. Its memory is first guessed as its files three times over
  and the model above (0.25 GB + 280 B a building of the area): Paris's densest area (8/129/88,
  2.6 M buildings, 119 MB of files) 1.3 GB, which took 0.86 GB natively and 0.79 GB of WebAssembly
  memory; a worker's measure replaces it (`bldtile 8/x/y` in the coordinator's costs). Tokyo's
  area (10.8 M buildings) would be guessed at 4–5 GB, more than a page spares (3 GB on the iPad,
  4 GB at most): a helper Mac with the room takes it, else the build Mac makes it, until z9 or z10
  areas are cut for smaller workers (workers.md's planned cutting to the worker).
- **The pool's steps table** (`docs/pool.md` §7.2, planned): `bldprep` {memory learned, disk 15 GB,
  the NAS, power}, `bldtiles` {memory learned, disk 15 GB, power}; neither needs home (`bldprep`'s
  reads are per tile, not the planet's). The pool's phase 1 (built, switched off) takes them as it
  takes every shared step, their write-sets `coord::saves`'.
- `bldprep` isn't a task: it reads 60 GB of parquet from the NAS, which pages can't reach and the
  coordinator shouldn't relay.

### 3.7 Determinism

Same inputs, same bytes, on any machine and in WebAssembly (plan.md §8, Determinism):
- the parquet decoded by one pinned pyarrow and shapely (the app's `dem/` environment), and every
  derived number computed in Rust from the decoded doubles: E7 by `(v * 1e7).round()`, Mercator by
  `det`, areas and centroids in f64 in a fixed order;
- heights as integer decimetres; a median of an even count takes the lower middle; ties by id;
- records sorted by (z14 tile, id), features by (Morton, id); no hash-map order; gzip by `flate2`
  (zlib-rs) at a fixed level, zstd at a fixed level;
- checked as the others were: a dense tile built natively on 1 and 14 threads and as WebAssembly,
  same bytes; the planned "build twice, compare hashes" covers both steps.
- **Checked in B1:** Paris (6/32/22) built twice on the build Mac, on 12 threads and on one, gave
  the same work file and the same pack byte for byte (content names `6-32-22.f07556cc02cd5298` and
  `6-32-22.4a4fb482a84227db`, the pack as rebuilt after the review: its copies' fills included);
  the unit tests build a tile on one thread and on several and compare. WebAssembly (B3,
  2026-10-08, `tools/check/bldtile-same.mjs`): Paris's densest area (8/129/88, 2,623,485 buildings,
  4,606 tiles) cut as a task and made by `bldtile` natively on one thread and on all, and as
  WebAssembly under Node's WASI, gave the same bytes, and the same tiles as the pack built whole
  (`6-32-22.627da12f3caf7b91`). B2's run on the M1 made Paris's work file again the same bytes
  (`6-32-22.f07556cc02cd5298`), another Mac and another Python process.

## 4. The map

### 4.1 Rendering

**MapLibre's fill-extrusion** (6.11.2), not a layer of our own, in the first phases:
- it handles the globe (its vertex shader projects to the sphere) and the 3D terrain (the centroid's
  elevation, a base of 0 sunk 10 m: `get_elevation(a_centroid)` in its shader, as 6.11.2 has it),
  tiles and their cache, and data-driven paint;
- **the z14 tiles whole above z14** (`keepWhole` in `buildings.ts`, for this source only): MapLibre
  6 slices a vector source's deepest tiles into z15–16 pieces up to the map's maximum zoom less
  `zoomLevelsToOverscale` (4: z15.5 here), each clipped 128 units beyond its edges and standing on
  the terrain at its own centroid: a building across a slice's edge drew as pieces, its roof
  stepped on a slope, its overhang past the z14 tile cut off (~0.1 % of buildings reach more than
  32 z14 units past it), the hover's highlight on one piece. Raising the option map-wide keeps
  them whole too, but every vector source then parses its deepest tiles again for each zoom above
  and caches each copy: zooming from 16 to 19 at Kyoto, the buildings' tiles grew from 84 to
  147 MB and the other sources' from 23 to 77 MB. For the buildings alone, with one z14 tile for
  every zoom above (`reparseOverscaled` off: an extrusion doesn't change with the zoom), it costs
  about what slicing did (§4.6). It reaches into MapLibre's tile manager, so `package.json` pins
  MapLibre at 6.11.2 exactly (as `vite.config.ts`'s shader patches need), and it warns in the
  console when what it hooks is missing, or when a tile deeper than z14 is in view once the map is
  above z15 (sliced again: the hook no longer takes);
- not picking: its `queryRenderedFeatures` projects extrusions from sea level with the flat map's
  matrix, ignoring the terrain, and finds nothing on the globe (§4.5);
- on the GPU, ~30 bytes an extruded vertex in B1's views (12-byte vertices, 4-byte centroids, the
  height and base 8 bytes, the triangles' indices, and the pick layer's footprints): Shinjuku's
  1.43 M vertices took 42 MB (§4.6);
- an opacity under 1 draws twice (depth, then colour), so roads behind show faintly; at 1, once.

**A source and four layers** (`web/src/buildings.ts`): the vector source `bld` (z12–14,
`/tiles/buildings/…?v=`), the layer `buildings` (fill-extrusion: height `h / 10` × the scale, base
`m / 10`, colour by the mode, vertical gradient on; parts and buildings without parts, not the
copies), `buildings-flat` (fill: buildings and outlines, not parts; the copies too, so a footprint
past its tile is drawn whole) for the flat mode, `buildings-pick` (fill at opacity 0, which MapLibre
doesn't draw: the footprints the hover queries, copies too) and `buildings-hover` (a GeoJSON
source's fill-extrusion: the hovered building, 1 m larger and taller, amber).

### 4.2 Where in the style

- After the rail layer, before the first symbol layer: `addBuildings(map, 'water-name-line', …)`
  after `rails` (main.ts), when the catalog has the layer (`/api/meta`'s `layers`), or once a new
  catalog brings it. With the other 3D layers: between the draped layers it would split the
  terrain's drape in two (the contours' comment in main.ts).
- `buildings-flat` and `buildings-pick` among the draped layers, after the water and parks, before
  the boundaries (`boundary-county`).
- Occlusion, as it follows from MapLibre's passes: the extrusions test and write depth (LEQUAL) in the
  translucent pass, against the terrain's depth. The road and rail layers (custom, 3D) test against
  that depth with their tolerance (1.5 % of the distance, at least 75 m × exaggeration), write none,
  and were drawn before: so the buildings paint over what's behind them and, a road being on the
  terrain, never over a road in front (§1). The dots and labels come after, without a depth test
  against extrusions.
- **B3, bridges and elevated rail:** their pieces (the tiles' bridge flag) drawn again in a small
  custom layer after the buildings, with the same occlusion against the terrain, so a viaduct in
  front of a tower stays in front.

### 4.3 On the terrain and the globe

- The 3D terrain is the z12 repaired Terrarium, 20–38 m pixels; MapLibre samples it at the
  centroid. **B3:** a shader patch (`vite.config.ts`) puts each wall's foot on the terrain under its
  own corner (`get_elevation(a_pos)` for the base vertices) and keeps the roof level at the
  centroid's ground plus the height, as a building on a slope is; the 10 m sink becomes 2 m.
- Exaggeration: MapLibre exaggerates the ground, not the height; the scale setting multiplies the
  height (1–3×, or the terrain's exaggeration).
- The globe: extrusions follow it to the hand-over at 15.5–16.5; the depth precision tuning
  (`camera3d.tuneDepth`) covers them.
- The camera stops 30 m short of the ground (camera3d): inside a tall building. B3 keeps it a few
  metres above the highest roof under it, from the loaded tiles.

### 4.4 Styling

- **Plain** (default): one blue-grey (#566173; on screen #31363f–#4f5967 as lit), lighter roofs,
  lit by `map.setLight` from the hill-shading's azimuth, intensity 0.35. (The first, #8d9aad, drew
  the eye from the roads.)
- **By height:** B1 has a fixed ramp (viridis, by the square root of the height, 0–150 m), with its
  legend. B3: the shared colour scale (Auto / Lock / Full, the colour-map picker, the low-end fade)
  over the buildings in view, as the terrain tint's.
- **By where the height comes from:** measured (green), floors (blue), Microsoft's estimate
  (violet), neighbours (amber), GHSL (red-orange), size (grey), so the fill can be judged on the map
  (B1).
- B3: buildings holding a heritage site's point tinted by its tier (the heritage overlay's colours),
  for the landmarks' sake.

### 4.5 Interaction

- Hover (B1), at most once a frame: the building the cursor's view ray meets first. MapLibre's
  query of extrusions ignores the terrain and finds nothing on the globe, so the app picks itself
  (`buildingAt`):
  - where the ray first meets the terrain: stepped down from the camera in 64 heights, then halved
    8 times, and stepped again between the camera and that point while that finds an earlier one
    (a ridge thinner than a step), up to 4 times. Not a point fixed from sea level, as camera3d's
    probe takes it, which lands behind a hill with the terrain 3× tall; not MapLibre's
    unprojection, which puts the ground at the camera when the camera is in a hill;
  - the ground under the ray from a metre past that point (a building met up to 1 m below it
    still counts: the terrain's sampling against the building's) back toward the camera, until
    the ray is above the tallest top around (the loaded tiles' tallest × the scale, plus 20 m ×
    the exaggeration for slopes), then on to the camera in 32 steps, where the ground rises back
    within reach (a ridge under the camera); each step takes the footprints (`buildings-pick`,
    copies included) tall enough to reach the ray over it;
  - those of 30 m or more from an index of the loaded tiles' (remade when the map is idle after
    their tiles or filter changed), by their boxes on the ground; the shorter by MapLibre's query
    in boxes on the screen, at most 24 px wide and 96 px tall (the track runs down the screen,
    toward the camera's nadir), below the screen's edge too. A box's ground comes from its corners
    on the terrain: a lower ray in a screen column meets the ground no farther than a higher one,
    so a box's top and bottom bound the rows between, over hills too, where a wide box's sides
    can meet different hills;
  - each tested against the ray between its roof and its base as MapLibre draws it (on the
    terrain at its polygon's centroid, a base of 0 sunk 10 m), the ray from camera3d (`rayAt`,
    globe or flat), the one met highest winning: the tall ones first, then the boxes from the
    camera's end, none once the ray over a box is below a building already met (a query's own
    cost, its corners found on the terrain, is most of a box's).

  Checked against the same ray test over every footprint of the loaded tiles, met above where the
  ray first meets the terrain (marched in 1,024 steps), at 9 × 9 points a view (1200 × 736, framed
  from the ground, the terrain 3× tall): eleven hillside views of Honolulu at z16.8, 70° (Makiki,
  St. Louis Heights, Pacific Heights, Wilhelmina Rise, Maunalani Heights, Waialae Iki, Aina Haina,
  Alewa Heights, Manoa, Punchbowl, Kamilo Iki), Shinjuku at z16, 75° and Tokyo at z13.9, 70°:
  1,050 of the 1,053 the same, the other 3 a camera inside a hill (below). A pointer move's hover
  (the M1; the bottom bar and the highlight included, roads and rails left out) took 0.5–4.6 ms in
  the median, 0.8–6.8 at p90 (Shinjuku 3.5 and 6.3, Tokyo 4.3 and 5.5), at most 10.5. A road or
  rail line under the cursor gives way when the ray to its point meets a building above it (a
  second call). The hovered building is drawn again in a small GeoJSON layer, 1 m larger and
  taller, amber; a tower's other parts and nearer buildings hide it where they're in front (B3: an
  outline drawn over the buildings). The bottom bar's slots as in §1. Known: while the globe hands
  over to the flat map (zoom 15.5–16.5), MapLibre draws a blend of the two that `rayAt` doesn't
  follow: the hover can be off by up to 6.8 px there (at z16.4). Where the framing leaves the
  camera inside a hill (2 m into Aina Haina's ridge), the rays start underground: MapLibre's
  queries put every point at the camera, and the hover finds nothing near it.
- Click: none in B1–B2. Later: **O** opens the OSM way where OSM gave the footprint (its id from the
  work file), served by `/api/building?at=` if wanted.
- The In view summary may add the tallest building in view (from the loaded tiles): B3, optional.

### 4.6 Levels of detail and the iPad

- Detail comes from the tiles (§1): z12 and z13 tiles hold only the tall and large buildings, and a
  tilted view takes them toward the horizon.
- **The iPad's budget** (8 GB iPad Pro; Safari gives a tab ~4 GB, the map's roads, terrain and
  basemap take a share): buildings ≤ 300 MB in the densest view, ≤ 8 ms of GPU a frame. What counts
  is everything the source holds, for the tiles in view **and in its cache**: its buffers
  (vertices, indices, paint) on the GPU, once (MapLibre 6.11.2 frees the page's copy of each once
  uploaded, but for the extrusions' centroids, a dynamic buffer it keeps: about an eighth of the
  buffers), and its raw tiles and their feature index in the page. B1 first counted the buffers in
  view alone, which the cache then outweighed: MapLibre's own cache (five zooms' worth of tiles in
  view, 60 here) held 471 MB of buffers after panning around Tokyo at z13.9, 70°, against 72 MB in
  view: 678 MB in all, with 73 MB of centroids and 62 MB of tiles and index. With whole z14 tiles
  at every zoom (§4.1) a cache of 8 tiles is enough: after the same panning, 86 MB in view and
  73 MB cached, 200 MB in all. Estimate before B1: Shinjuku at zoom
  16, tilted 60°: up to ~10 z14 tiles of 9,000–20,000 buildings near (Shinjuku's own 9,111; the
  wards west of it ~20,000: §2.5), coarser tiles beyond: ~120 k buildings, ~120 MB on the GPU,
  ~2 M triangles.
- **To measure on the iPad** at Shinjuku (z16, 60°), Manhattan (z15, 70°), Paris (z15), Hong Kong's
  Mid-Levels on its slope, Monaco, a Vermont village and a Japanese mountain town; and Barcelona's
  old town, whose z14 tiles are the heaviest (§2.5). Their z14 tiles, by B0: Shinjuku 9,111
  buildings, 141 KB; Midtown Manhattan 4,435, 101 KB; Paris (Châtelet) 4,752, 132 KB; the
  Mid-Levels 3,208, 72 KB; Monaco 2,170, 44 KB; Woodstock, Vermont 630, 13 KB; Takayama 5,855, 76 KB.
  B1's pilot tiles hold Shinjuku, Manhattan, Paris, Barcelona and Woodstock (§5.1); the others wait
  for B2's build.
- **Measured in B1 on the M1** (a 16-inch MacBook Pro, M1 Pro with a 16-core GPU; Chrome 152's
  engine in the Claude app's browser pane, 1200 × 736 CSS px at 2×; the pilot's packs as served,
  whole z14 tiles, a cache of 8; summed over the tiles in view: the source's buffers (on the GPU),
  the centroids the page keeps, its raw tiles and feature index; the extrusions' draw calls timed
  with `EXT_disjoint_timer_query_webgl2`, frames drawn one after another, opacity 0.85; framed
  from the ground, each view fresh):

  | View | tiles | vertices | buffers | centroids kept | raw tiles and index | GPU time a frame (median / p90) |
  |---|---|---|---|---|---|---|
  | Shinjuku, z16, 60° | 7 | 1.43 M | 42.2 MB | 5.7 MB | 5.3 MB | 3.5 / 4.5 ms |
  | Tokyo, Shinjuku to the horizon, z13.9, 70° | 10 | 2.91 M | 86.2 MB | 11.6 MB | 10.0 MB | 5.4 / 5.9 ms |
  | … after panning around it | 10 + 8 cached | 2.91 M | 86.2 + 73.3 MB | 21.5 MB | 18.7 MB | 5.6 / 6.0 ms |
  | Midtown Manhattan, z15, 70° | 9 | 1.02 M | 30.6 MB | 4.1 MB | 2.7 MB | 3.8 / 4.6 ms |
  | Paris (Châtelet), z15, 60° | 10 + 1 cached | 1.84 M | 55.6 + 7.9 MB | 8.4 MB | 4.3 MB | 2.7 / 4.1 ms |
  | Barcelona's old town, z16, 60° | 6 | 2.44 M | 73.2 MB | 9.8 MB | 6.8 MB | 3.2 / 5.1 ms |
  | Woodstock, Vermont, z14.5, 60° | 17 | 0.08 M | 2.3 MB | 0.3 MB | 0.2 MB | 1.2 / 2.6 ms |

  So the densest view after panning holds 200 MB (160 MB of buffers on the GPU, 22 MB of
  centroids and 19 MB of tiles and index in the page), within the budget on the M1's count; the
  iPad's own numbers decide. Whole z14 tiles against MapLibre's slicing (§4.1) with its cache,
  zooming in from 16 to 19 at 60°: about the same memory, and GPU time within ~0.6 ms either way
  (whole tiles draw buildings beyond the view at z17–19, slicing more tiles):

  | Zooming in, 60° | buffers at z19, sliced (in view + cached) | whole | GPU median z16 / z17 / z19, sliced | whole |
  |---|---|---|---|---|
  | Shinjuku | 5.4 + 29.7 MB | 31.8 + 14.7 MB | 3.3 / 2.9 / 1.4 ms | 2.8 / 3.1 / 1.6 ms |
  | Kyoto (the fullest z14 tile) | 24.5 + 58.5 MB | 34.1 + 38.0 MB | 4.1 / 1.1 / 1.5 ms | 4.0 / 1.5 / 2.0 ms |
  | Paris (Châtelet) | 14.4 + 33.3 MB | 26.8 + 21.3 MB | 2.6 / 1.5 / 1.3 ms | 2.6 / 1.7 / 1.4 ms |
  | Barcelona's old town | 14.1 + 53.8 MB | 30.2 + 43.8 MB | 3.1 / 3.3 / 1.5 ms | 3.3 / 3.9 / 2.1 ms |

  B1's first measurements, before the review: opacity 1 (one pass) saved little (Shinjuku 2.9
  against 3.0 ms); the whole map's frame took 5–6 ms
  of GPU at Shinjuku with or without the buildings within the noise.
- **Measured on the iPad** (the owner, 8 Oct; frames timed by `requestAnimationFrame` over 10 s of
  two-finger orbiting, memory from Safari's Web Inspector, buffers by the console snippet below):

  | View | Buildings | fps | median / p90 / worst frame | page max | buildings' buffers in view + cached |
  |---|---|---|---|---|---|
  | Shinjuku | on | 25 | 37 / 61 / 410 ms | 192 MB | 46.3 + 10.2 MB (6 + 4 tiles) |
  | Shinjuku | off | 24.6 | 41 / 57 / 95 ms | 151 MB | — |
  | Midtown | on | 22.8 | 38 / 76 / 137 ms | 439 MB | 39.8 + 11.1 MB (15 + 5 tiles) |
  | Midtown | off | 13.5 | 78 / 123 / 190 ms | 422 MB | — |
  | Châtelet | on | 17.8 | 51 / 103 / 171 ms | 163 MB | 72.4 + 15.4 MB (10 + 3 tiles) |
  | Châtelet | off | 21.2 | 44 / 77 / 132 ms | 187 MB | — |

  The buildings fit the memory budget by far (at most ~88 MB of buffers against 300), and cost
  little frame time beside the map's own: with them off the map runs at 13–25 fps in these tilted
  city views too (one run each; tiles still loading as the view turns weigh on both). The iPad's
  frame rate is the whole map's, not the buildings'. Safari's JavaScript heap peaked at 760–910 MB
  either way.
- **The iPad checklist** (the owner's):
  1. Serve the pilot: a server from this branch with `--root` a folder laid out like the NAS's
     whose newest catalog has the pilot's `buildings` layer (B1 made one: catalog 14 with the
     pilot's six packs added, the rest of the folder linked to the NAS read only), a scratch
     `--home`, listening on every address (no `--listen`) so the iPad reaches it; the address is in
     `<home>/map-page`.
  2. On the iPad, open that address in Safari; on the Mac, Safari → Develop → the iPad → the
     map's page (Web Inspector).
  3. At each view (the hash after the address): `#map=16/35.69/139.70/20/60` (Shinjuku),
     `#map=15/40.758/-73.985/30/70` (Midtown), `#map=15/48.858/2.347/20/60` (Châtelet),
     `#map=16/41.383/2.1765/30/60` (Barcelona's old town), `#map=15.5/43.624/-72.519/20/60`
     (Woodstock: if the view starts inside a hill, zoom out and back in, the hills being 3× tall).
     Wait for the tiles (the bottom bar's loading line), then:
     - **Timelines** → record about 10 s while orbiting slowly with two fingers → Frames: the frame
       rate and the frame times; the Rendering and JavaScript lanes;
     - **Graphics** (or **Memory**): the page's memory and its canvas share;
     - **Console**: the buildings' buffers in view and in the cache (all of the source's
       layers: the extrusions, the pick layer's footprints: on the GPU), what the page keeps of
       them (the extrusions' centroids: MapLibre frees the rest once uploaded), and its raw tiles;
       resident is the buffers, what the page keeps, and the raw tiles with their index (about
       twice the raw):
       ```
       (() => { const tm = __app.map.style.tileManagers.bld;
         const z = (a) => (a && a.bytesPerElement ? a.length * a.bytesPerElement : 0);
         const kept = (a) => a?.arrayBuffer?.byteLength ?? 0;
         const sum = (ts) => { let b = 0, k = 0, r = 0, v = 0; for (const t of ts) {
           r += t.latestRawTileData?.byteLength ?? 0;
           for (const u of Object.values(t.buckets ?? {})) {
             if (u.centroidVertexArray) v += u.layoutVertexArray.length;
             for (const a of [u.layoutVertexArray, u.centroidVertexArray, u.indexArray, u.indexArray2]) { b += z(a); k += kept(a); }
             for (const c of Object.values(u.programConfigurations?.programConfigurations ?? {}))
               for (const d of Object.values(c.binders)) { b += z(d.paintVertexArray); k += kept(d.paintVertexArray); } } }
           const mb = (n) => +(n / 1e6).toFixed(1);
           return { tiles: ts.length, vertices: v, buffersMB: mb(b), keptMB: mb(k), rawMB: mb(r) }; };
         return { inView: sum(tm._inViewTiles.getAllTiles()),
           cached: sum(Object.values(tm._outOfViewCache.data).flat().map((e) => e.value)) }; })()
       ```
  4. Each view with Buildings on, then off (B on a keyboard, or the switch), then opacity 100 %
     (the iPad's default already: a touch screen's is 100 %); and once after panning
     around Shinjuku for a while (the cache full).
  5. Over budget (300 MB of buildings, 8 ms a frame) anywhere: the fallbacks below, and
     Barcelona's old-town tiles simplified (~18 % off).
- **If over budget**, on touch devices: opacity 1 (one pass, the default there anyway), z14 tiles only
  from zoom 15 (the tall and large only between 13 and 15), a smaller cache still (4 tiles held 41 MB of buffers
  after the Tokyo panning, 8 held 73 MB), and, if it comes to it, the extrusions' centroids freed in the page once
  uploaded, as MapLibre frees the rest (a patch, as `vite.config.ts` patches its shaders; MapLibre
  marks their buffer dynamic, so what updates it to be checked first).

### 4.7 Later: a layer of our own

A custom WebGL2 layer, as the roads, contours and dots have, only if B1–B3's measurements or
artefacts call for it (B4):
- exact occlusion with the roads: the buildings' depth drawn into a texture the road, rail and dot
  shaders read beside the terrain's, so a road behind a building fades as one behind a hill does
  (today hidden or faint by the opacity), and the roads' tolerance stays for the terrain alone;
- pitched roofs: tagged shapes (0.6 %), and gabled roofs inferred for small residential footprints
  (four corners, a ridge along the long side), which villages would show;
- distance-based detail like the roads' (perspective), and the iPad's memory under our control.

## 5. Phases, risks, questions, decisions

### 5.1 Phases

| Phase | What | Effort |
|---|---|---|
| **B0 Data** (done 2026-10-06) | The downloads (§2.6). `dem/bldmeasure.py` over the files: heights, floors and their sources by country (§2.2); the storey heights fitted, and the fill's order, fits and defaults set from a held-out tenth (§2.3); the tiles' counts and sizes at z12–14, the fullest and heaviest encoded (§2.5); this document's numbers updated. On the build Mac: 13 minutes to read 44.6 GB of row groups from the NAS, 4 to fill and count, 5 to encode. | 1 day |
| **B1 Pilot** (done 2026-10-06, but the iPad) | `dem/bldprep.py`, `pipeline::bld` (prep, fill, tiles, job), `scenic-build bldprep` and `bldtiles`, run by hand on the build Mac into a scratch root (the NAS's sources read only) on 6/56/25 (Kantō), 6/32/22 (Paris), 6/18/24 (New York), 6/32/23 (Barcelona), 6/19/23 (Vermont, with Boston) and 6/3/28 (Oahu); formats.md entries; the catalog layer, the server's route; `web/src/buildings.ts` with the settings section, the toggle and hover; checked on this Mac in a test server (§4.6). `bldprep`: 88–153 s a tile on the first run (Kantō: 30.3 M rows read in 130 s; one thread: Paris in 71 s), 29–68 s on the run after the review that made the packs served (Oahu 3 s; 5.1 GB at most, Kantō's); `bldtiles`: 3–10 s a tile in that run (Kantō 10 s, 3.3 GB at most; Oahu 0.5 s), 10–43 s in an earlier run of the same code with the build Mac busier. The reviews' fixes: §2.3 (rule 3's bound), §3.2 (countries, keys), §3.4 (copies, walls, their points outside the ring), §4.1 (whole tiles, MapLibre pinned), §4.5 (the hover, on hills too), §4.6 (the cache, memory counted). The iPad's measurements are the owner's (§4.6's checklist). | 6 days |
| **B2 In the build** (code built 2026-10-08; not yet published, nor run by an agent) | The agent: keys, targets, the chain's order, prunes, status and forecast labels, shared steps, `bld-fetch` as a job (§3.1–3.3, 3.6); the mirror's group, the service worker's budget, credits (§3.5); plan.md (§4, §6, §8, §9, §10), workers.md, pool.md, formats.md and the README updated. Measured on the M1 in a scratch root (§2.5): Vermont's two z6 tiles and Paris's, with the agent's command lines; the agent's coverage asks bldfetch.py for exactly the 103 files on the NAS. Left: every tile built and published, which the next app published starts (§5.2). | 4 days |
| **B3 Sharing and polish** | `bldtile` tasks for pages (WebAssembly, byte-identical); bridges and elevated rail over buildings; walls on the terrain under each corner; fog on the extrusions; the camera's clearance; colour by height, by source, heritage tint. | 5 days |
| **B4 Each on its own measurement** | A custom layer (§4.7); measured heights from BD TOPO (France) and PLATEAU (Japan's cities); building heights in the horizons and the viewshed tool (every unit rebuilt). | 2–3 weeks |

B1–B3: about 15 days of work, the build's own time aside. The build's (B2's estimate, the build
Mac's pace in B1 over the 380 tiles' ~440 M rows): `bldprep` 2.2–4.7 s a million rows (Kantō's
pace to Vermont's) and ~2.5 s a tile to start (its Python), 27–51 min in all; `bldtiles` 0.5–2.9 s
a million buildings (~342 M) and a few seconds a job to read the coverage, 5–18 min: 30–70 min of
one slot, less with the second job and the M1 beside it (the M1 at a quarter of the build Mac's
pace when busy, B2). The first `bld-fetch` fetches nothing (a listing of S3 and of JRC's tiles,
and the coverage's 65 MB of GeoJSON unioned by shapely: 10 min on the busy M1).

### 5.2 Risks

- **The release leaves S3 on 2026-11-25.** The coverage's files are kept on the NAS. A region added
  later whose files aren't here waits for the next pinned release: `bld-fetch` then fails, naming
  the files it couldn't fetch (the release's footers, read before, list them), and is tried again
  with a growing delay; the rest of the chain goes on with the files here. (The status says the
  fetch failed; it doesn't yet say "its buildings wait for the next Overture release".) pinning one (~6-monthly, plan.md §8 Planned) fetches 62 GB again
  (~7 hours on this line), rebuilds every unit (the roadside index) and every buildings tile. The old
  release's files are deleted by hand afterwards (GC never sweeps `sources/`).
- **Estimated heights:** 88 % of the buildings (Microsoft's estimates 36 %; Japan's 91 %, France's
  99 %), and tall unmeasured buildings come out 12–35 m low (§2.3): the skyline is thin where
  heights aren't measured (central Tokyo: 11 % have a height or floors). Mitigated by the measured
  rules first, the hold-out checks, the "where the height comes from" colouring, and national data
  later.
- **Occlusion artefacts** with MapLibre's extrusions: viaducts and elevated rail painted over (B3);
  places where the terrain mesh lies above a road, so a building behind the road shows over it (the
  roads' 75 m × exaggeration tolerance exists for that mesh error).
- **The iPad:** dense views may exceed the budget (§4.6's fallbacks).
- **The NAS:** `bldprep` reads 60 GB; with the units' writes, the NAS is the bottleneck (workers.md
  §1). Paced per tile and kept off the OSM pass.
- **Disk on the build Mac** (17 GB free on 2026-10-05): neither step stages more than a tile's row
  groups or a z8 area locally; the downloads went straight to the NAS.
- **Overture's schema** changes between releases (columns renamed or retyped): `bldprep` checks the
  columns it reads, their names and types (a float read as integers would be truncated), and fails
  naming them.
- **Overlapping footprints** left by conflation z-fight: none showed in B1's views; B1 didn't count
  them (B2's build can). If they show, `bldprep` drops the one from the lower-ranked source (OSM
  first, as Overture ranks them) where two overlap by nine-tenths of the smaller.
- **A MultiPolygon's polygon, or a hole, wholly beyond one edge of its tile** (a building in pieces
  straddling a z14 edge; a courtyard past it) isn't drawn, or the hole has no walls: MapLibre's
  extrusion skips a ring outside its tile, a clipped tile's buffer copy as it assumes. Rare (in the
  reviewer's sample of a tenth of the pilot's tiles, 0–3 polygons and 2–12 holes a pack); B2 could
  put each polygon in the tile of its own centroid.
- **Copies reach 310 m:** a building whose centroid is further than that beyond a z8 area's edge
  isn't copied into the area's tiles, so its flat footprint is cut there (an airport terminal).
- **The antimeridian:** a tile's neighbours aren't wrapped across it, so a building within 620 m
  of it doesn't see the other side's (the Aleutians' few; B1 left it).
- **The pilot's edges:** a tile's neighbours' rule reads its 8 neighbours' files; in the pilot only
  Paris and Barcelona had one (each other), so the other tiles' edges had fewer neighbours than the
  build will give them.
- **Python's part in a deterministic step:** the scan only decodes, Rust computes (§3.7); the
  double-build check covers it.

### 5.3 Questions for the owner

None blocks the work; each has a default below.
- Buildings on by default? (Default: on, after the iPad's measurements.)
- Heights true by default, or scaled with the terrain's 3× exaggeration? (Default: true.)
- Raise the download's cap from 3 MB/s when the line is idle? (Default: no; it ends overnight.)
- Pitched roofs and exact occlusion (B4): worth a custom layer? (Decided after B3.)

### 5.4 Decisions made

1. **Overture alone** for footprints and attributes, the release the roadside buildings use
   (2026-09-23.1), pinned once for both; OSM, Microsoft and USGS heights through it.
2. **GHSL ANBH** (CC BY 4.0, 1.28 GB) for the fill, in its cells of 20 m or more (B0: elsewhere the
   size rule is nearer); Google's 2.5D (an account, little overlap) not used; national data later,
   where it replaces estimates.
3. **The fill's order:** measured, floors, Microsoft's estimates, neighbours, GHSL in high-rise cores,
   size and kind, each building saying which; the fits, defaults and order set by B0's held-out tenth
   (§2.3), not by hand.
4. **The coverage**, as roads: a building touching it is built; not roadside only, not the world.
5. **Downloads:** the coverage's Overture files whole (60.5 GB of 277) rather than row groups (44.8 GB):
   the source's own bytes, checkable against its ETags, resumable, and a later region needs only
   whole files more; GHSL's tiles meeting the coverage. Two transfers, 3 MB/s, so the build keeps its
   share of the line.
6. **Tiles z12–14,** each building whole in the tile of its centroid (one centroid, no seams, no
   building drawn twice: its copies in the tiles it reaches are for the flat footprints, which the
   extrusions leave out), and the z14 tiles whole at every zoom above; z12 and z13 only the tall
   and large.
7. **Two steps per z6 tile:** `bldprep` (impure: the NAS's parquet) and `bldtiles` (pure: tasks for
   pages), so the fill and the tiles can change without reading the parquet again.
8. **A chain of its own** that holds no region and no round; tiles in the regions' order.
9. **MapLibre's fill-extrusion** first, after the road and rail layers; a custom layer only if the
   measurements call for it.
10. **True heights** by default, with a scale setting; buildings on by default.
11. **B2's changes to the plan:** `bld-fetch`'s key leaves the footers out (it writes them; the
    release names its files); `bldprep`'s targets are the tiles within 1 km of the coverage, so the
    fill's neighbours are read at a tile's edge, where the coverage meets a tile but not its
    neighbour; batches are a fixed count of tiles (8 and 16), not weighed by buildings; after the
    last unit, a catalog at most each hour while the chain has work left.
