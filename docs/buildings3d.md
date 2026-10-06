# 3D buildings

**Phases B0 and B1 done** (2026-10-06): the sources downloaded and measured (B0); the steps, the
layer and the map built and piloted by hand on six z6 tiles (B1, §5.1), the iPad's measurements to
come (§4.6's checklist). Nothing of B2–B4 built: the agent doesn't run the steps, nothing is
published. The first of plan.md §10's phase 7 features ("3D buildings, then PLATEAU"); plan.md §6
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

**At which zooms.** Tiles at z12, z13 and z14; MapLibre overzooms z14 to the map's 19.5.
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
mode; opacity; height scale (1–3×, or with the terrain's exaggeration); detail (all, or the skyline:
40 m or more). In the link with the other settings (`bd=`); **B** toggles the layer. All three only
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
   area's median, a 2.5 m² footprint 22 m tall in Paris; smaller footprints go on to rules 4–5,
   the bound rule 4 uses for small footprints. B0's hold-out can't see it: tiny structures are
   rarely measured.)
4. **GHSL, in high-rise cores:** the ANBH value of the 3″ cell holding the footprint's centroid, when
   it is 20 m or more; a footprint under 60 m² takes at most 4 m. Sampled on 2026-10-05: Midtown
   Manhattan 31.5 m, Lower Manhattan 35.2, downtown Toronto 42.8; Brooklyn's row houses (18.0), Back
   Bay (17.3), the Plateau in Montréal (12.7) and Montpelier (15.0) fall through to rule 5.
5. **Size and kind:** the measured median height of its class, in its country where 200 of the
   class are measured, else in the coverage: churches and cathedrals 8.8 m; sheds, garages, carports
   and huts, or under 30 m²: 3.3 m; houses and residential kinds, or under 250 m²: 6.0 m;
   250–2,000 m²: 6.4 m; larger: 9.1 m (the coverage's; the US's 3.3, 5.5, 6.2 and 8.9 m, Japan's
   3.2, 7.5, 9.0 and 14.8 m, France's 5.0, 7.0, 10.0 and 12.0 m and its churches 13.0 m).

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

These are B0's counts, before rule 3's 60 m² bound (B1). In the pilot's tiles the bound moved 4–31 %
of the buildings from rule 3 to rule 5: Paris's z6 tile 59 → 28 % by the neighbours and 23 → 53 %
by size, Barcelona's 55 → 25 % and 20 → 50 %, New York's 22 → 12 % and 4 → 14 %, Vermont's 22 →
12 % and 3 → 14 %, Kantō's 14 → 10 % and 73 → 77 %. They are sheds, garages, annexes and kiosks,
which the size rule puts at its sheds' height under 30 m² (3–5 m) and its houses' above (6–8 m in
most countries), where the 300 m median gave them the area's, its towers' in a city. B0's hold-out could score the
two rules on footprints under 60 m² before B2 builds every tile.

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
| tiles, built (B1's pilot) | 6/56/25 (Kantō): 30.3 M buildings and parts, 24,956 tiles, a 0.41 GB pack (13 B a building at z14); 6/32/22 (Paris): 15.3 M, 71,238 tiles, 0.26 GB; 6/18/24 (New York): 14.6 M, 47,503 tiles, 0.27 GB; 6/32/23 (Barcelona): 12.6 M, 50,688 tiles, 0.22 GB; 6/19/23 (Vermont and Boston): 6.1 M, 35,516 tiles, 0.12 GB. 13–19 B a building at z14, as B0 estimated |
| an app Mac's mirror | +6 GB |

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
| `bld-fetch` | the release | S3, JRC, the coverage | `sources/overture/<release>/`, `sources/ghsl/R2023A/` | the network slot; by hand until B2 |
| `bldprep` | a z6 tile T | Overture's row groups meeting T, the GHSL tiles meeting T | `work/bld/6-x-y` | any Mac with the NAS |
| `bldtiles` | a z6 tile T | `work/bld/` of T and its 8 neighbours, the coverage over T | `layers/buildings/hi/6-x-y` | any Mac; its z8 areas as tasks for pages |

(`bldtiles` is the design's `buildings T`: `buildings` is already the roadside buildings' step,
`pipeline::buildtiles`, in scenic-build and the agent.) Built in B1, run by hand:
`scenic-build bldprep <6/x/y …> [--dem dir]` and `scenic-build bldtiles <6/x/y …> [--pass d]
[--regions dir]`; the agent's part is B2's.

- **`bld-fetch`** is `dem/bldfetch.py` (§2.6). From B2 the agent runs it as a network job (the
  second slot's first kind, with the heritage chain and the rail feeds) when its key changes: the
  release, `coverage_all` and the release's footers. It skips what's there, so a run is cheap.
- **`bldprep T`**, per z6 tile meeting the coverage (1 km buffer): `dem/bldprep.py` reads the row
  groups meeting T from the downloaded files (pyarrow, four at a time), their rows whose box meets
  T (parts: T grown by 0.02°, so an outline finds its parts), and the GHSL windows under T
  (rasterio, out of the zips), and writes their columns to stdout, the geometry as Overture's WKB;
  Rust (`scenic-build bldprep`, `pipeline::bld::prep`) reads the stream as it comes, parses the WKB,
  computes each centroid (area-weighted, as GEOS) and area in f64, rounds to E7, keeps the
  buildings and parts whose centroid is in T (underground ones left out), samples GHSL at the
  centroid, sorts by (z14 tile, id) and writes the normalized file (§3.4). Python only decodes;
  every number that ends up in a file is computed in Rust. Reads ~60 GB over all tiles, once per
  release.
- **`bldtiles T`**, per z6 tile meeting the coverage, a z8 area at a time (`pipeline::bld::job`):
  the buildings of T that touch the coverage, heights filled (§2.3, `pipeline::bld::fill`; the
  neighbours' rule reads the buildings within 310 m beyond the area, T's or its neighbours', by
  their z14 blocks), the z12–14 tiles encoded (§3.4, `pipeline::bld::tiles`), the hi pack written.
  Pure: its output is a function of its inputs' bytes. A z6 tile with no building in the coverage
  loses its pack.

Both are new `scenic-build` steps and agent steps; the units, the roads' chain and the landmarks
don't change, and no unit's key reads them.

### 3.2 Keys and versions

In `agent::build`, beside the others (B2):
- `BLDPREP_V = 1`, `BUILDINGS_V = 1` (the fill's rules, fits and defaults, and the tiles, are in
  `BUILDINGS_V`): defined in `pipeline::bld` (B1), as `TREES_V` is in `pipeline::treepacks`.
- `Keys` gains `bldprep` and `buildings`, maps by z6 tile as `unit` and `pack` are; `Keys::map`,
  `recorded` and `record` take them, and a prune forgets them ("bldprep 6/x/y", "buildings 6/x/y").
- **`bldprep T`'s key:** `bldprep {BLDPREP_V}`, the release, and for each downloaded file with a row
  group meeting T its name, ETag and those row groups' indexes (from `footers.json.gz` and
  `buildings.json`, which the agent reads as it reads `inputs/`: a digest in `plan`'s `inputs`),
  and the GHSL tiles meeting T by name and size. A file fetched later (the coverage grew) changes the
  key of the tiles it meets; an unchanged result keeps its content name, so nothing after it reruns.
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
- **`bld-fetch`'s key:** the release, `coverage_all`, the footers' digest; kept with the lo keys under
  its own name, as `rail-feeds` is.
- The pinned release is `buildtiles::RELEASE` for both the roadside and the 3D buildings: a new one
  re-keys every unit (the roadside index) and every buildings tile together, never a mix.

### 3.3 Order, rounds, the chain

- **A fourth chain**, beside the roads', the trains' and the landmarks': it reads no unit and no
  terrain (the map puts buildings on its terrain), so it runs from the start, each step once what it
  reads is built: `bldprep T` once T's files are on the NAS; `bldtiles T` once T and its neighbours
  are prepared. Its work is listed after the landmarks' in `plan` (the build Mac's first job takes it
  when the regions' work is done or waits), its tiles in the regions' order (the region with the
  fewest units left first, then `spatial_order`), so the buildings of the region being built come
  first.
- **The second job** takes it beside the regions' work, as CPU work (not while the Mac is in use);
  never `bldprep` beside the OSM pass or another job that reads the planet through the NAS.
- **Rounds:** buildings don't hold a round, and a region's readiness (`ready`) doesn't wait for
  them: their packs go out with the next round's catalog, as the trains' and landmarks' outputs do;
  after the last unit, a catalog follows any chain's change. A region can reach the map before its
  buildings, which follow with a later round.
- **Batches:** about fifteen minutes of work a job, by the tiles' building counts (the densest tile
  alone).
- **Status:** the checklist gets "Raising the 3D buildings"; `label("bldprep")` "Reading the regions'
  buildings", `label("bldtiles")` "Raising the 3D buildings"; parts and progress lines as the other
  steps' (row groups read; z8 areas done; packs written).

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
  holding its centroid** (not clipped: a building is in one tile, its coordinates may run past the
  extent). MapLibre's extrusion then has one centroid a building, so no step at a tile edge on a
  slope, and nothing is drawn twice.
- Quantized to the tile's grid (z14: 0.6 m at the equator); repeated points dropped, rings that
  collapse dropped; at z12–13, simplified to one grid unit.
- Properties: `h` the top (dm), `m` the base (dm; parts; left out when 0), `s` the height's source
  (0–5, §2.3), `f` floors (when `s` is 1), `c` the kind (0 unknown, 1 residential, 2 outbuilding,
  3 commercial, 4 industrial, 5 religious, 6 civic, 7 agricultural, 8 transport, 9 other), `k` (1 a
  part, 2 an outline with parts: drawn by the flat layer only). No feature ids, no names (B1).
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
- **Mirror:** a copy group of its own after the hi packs (`store::mirror::groups`), so a Mac's
  mirror has the roads and terrain first; the M1's budget may leave buildings out, which the server
  then reads from the NAS.
- **Devices:** the iPad's service worker keeps versioned tiles it has shown (12,000 files at most);
  building tiles would crowd out the rest in a city, so they get a budget of their own (B2).
- **Credits** (`pipeline::rules::CREDITS`): "3D buildings: Overture Maps Foundation
  (OpenStreetMap, Microsoft, Esri Community Maps, USGS, IGN España, Google and others), ODbL" over
  the coverage; "Building heights where none are known: GHSL GHS-BUILT-H R2023A, © European Union,
  CC BY 4.0".
- Nothing is published or redistributed: the tiles stay on the NAS and the owner's Macs and devices
  (plan.md §3, the README's Terms).

### 3.6 Sharing the work

- **Helper Macs** (today's M1; any member in `docs/pool.md`): both steps are shared steps
  (`agent::claims::SHARED`), offered from the far end as terrain and units are. `bldprep` needs the
  NAS (it reads up to ~3 GB of row groups a tile) and, in B1's pilot, 4.6 GB of memory at most for
  the densest tile (Kantō; 1.3–2.9 GB for the others); `bldtiles` holds a z8 area at a time (3.6 GB
  at most for Kantō's run, 1.0–1.8 GB for the others). Each target's memory is learned
  (`SCENIC_COSTS`: both steps note their targets' costs, `bldprep 6/x/y` and `bldtiles 6/x/y`);
  first guesses, which B1's runs bear out: 1 GB + 120 B a building for `bldprep`, 0.5 GB + 150 B a
  building of its largest z8 area for `bldtiles`.
- **Pages** (`docs/workers.md`): a `bldtiles` job offers its z8 areas as tasks, as a unit job offers
  its tail. A task's files: the z8 area's blocks and the blocks within 300 m around it (cut from the
  work files on the Mac that runs the job), and the program `bldtile` (Rust, built for wasm32-wasi
  with the others, `/work/prog/bldtile.wasm`). It writes the area's z12–14 tiles (an RDTILES archive);
  the job assembles the pack. The densest z8 area (Tokyo's) is ~1–1.5 GB in memory, within the
  iPad page's 3 GB; a smaller ceiling gets z9 or z10 areas (workers.md's planned cutting to the
  worker). Nothing waits on a page: an area no page took runs on the job's Mac, one a page holds is
  raced there, results are compared (the ramped verification).
- **The pool's steps table** (`docs/pool.md` §6): `bldprep` {memory learned, disk 15 GB, the NAS,
  power}, `bldtiles` {memory learned, disk 15 GB, power}; neither needs home (`bldprep`'s reads are
  per tile, not the planet's).
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
  `6-32-22.8db81a60e19ca8d9`); the unit tests build a tile on one thread and on several and compare.
  WebAssembly waits for `bldtile` (B3).

## 4. The map

### 4.1 Rendering

**MapLibre's fill-extrusion** (6.11.2), not a layer of our own, in the first phases:
- it handles the globe (its vertex shader projects to the sphere) and the 3D terrain (the centroid's
  elevation, a base of 0 sunk 10 m: `get_elevation(a_centroid)` in its shader, as 6.11.2 has it),
  tiles and their cache (z14 tiles over-zoomed by re-parsing them for each z15–19 tile) and
  data-driven paint;
- not picking: its `queryRenderedFeatures` projects extrusions from sea level with the flat map's
  matrix, ignoring the terrain, and finds nothing on the globe (§4.5);
- on the GPU, ~27 bytes a vertex in B1's views (12-byte vertices, 4-byte centroids, the height and
  base 8 bytes, the triangles' indices): Shinjuku's 0.9 M vertices took 24 MB (§4.6);
- an opacity under 1 draws twice (depth, then colour), so roads behind show faintly; at 1, once.

**A source and four layers** (`web/src/buildings.ts`): the vector source `bld` (z12–14,
`/tiles/buildings/…?v=`), the layer `buildings` (fill-extrusion: height `h / 10` × the scale, base
`m / 10`, colour by the mode, vertical gradient on; parts and buildings without parts),
`buildings-flat` (fill: buildings and outlines, not parts) for the flat mode, `buildings-pick`
(fill at opacity 0, which MapLibre doesn't draw: the footprints the hover queries) and
`buildings-hover` (a GeoJSON source's fill-extrusion: the hovered building, 1 m larger and taller,
amber).

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
  (`buildingAt`): candidates are the footprints (`buildings-pick`) under the ray's ground track, a
  thin box on the screen from the cursor down to where the ray is at the tallest top (700 m × the
  scale, under the camera); each is tested against the ray between its roof and its base as
  MapLibre draws it (on the terrain at its polygon's centroid, a base of 0 sunk 10 m), the ray
  from camera3d (`rayAt`, globe or flat, the same as MapLibre's unprojection at the ground), and
  the one met highest wins. Checked on Shinjuku's towers: the building picked covers the pixel (6
  of 7 test points; the seventh at a footprint's edge). A road or rail line under the cursor gives
  way when the ray to its point meets a building above it. The hovered building is drawn again in a
  small GeoJSON layer, 1 m larger and taller, amber; a tower's other parts and nearer buildings hide
  it where they're in front (B3: an outline drawn over the buildings). The bottom bar's slots as in
  §1.
- Click: none in B1–B2. Later: **O** opens the OSM way where OSM gave the footprint (its id from the
  work file), served by `/api/building?at=` if wanted.
- The In view summary may add the tallest building in view (from the loaded tiles): B3, optional.

### 4.6 Levels of detail and the iPad

- Detail comes from the tiles (§1): z12 and z13 tiles hold only the tall and large buildings, and a
  tilted view takes them toward the horizon. "Skyline only" filters `h` (MapLibre filters before it
  builds the buckets, so filtered buildings cost no GPU memory).
- **The iPad's budget** (8 GB iPad Pro; Safari gives a tab ~4 GB, the map's roads, terrain and
  basemap take a share): buildings ≤ 300 MB in the densest view, ≤ 8 ms of GPU a frame. Estimate:
  Shinjuku at zoom 16, tilted 60°: up to ~10 z14 tiles of 9,000–20,000 buildings near (Shinjuku's
  own 9,111; the wards west of it ~20,000: §2.5), coarser tiles beyond: ~120 k buildings, ~120 MB on
  the GPU, ~2 M triangles.
- **To measure on the iPad** at Shinjuku (z16, 60°), Manhattan (z15, 70°), Paris (z15), Hong Kong's
  Mid-Levels on its slope, Monaco, a Vermont village and a Japanese mountain town; and Barcelona's
  old town, whose z14 tiles are the heaviest (§2.5). Their z14 tiles, by B0: Shinjuku 9,111
  buildings, 141 KB; Midtown Manhattan 4,435, 101 KB; Paris (Châtelet) 4,752, 132 KB; the
  Mid-Levels 3,208, 72 KB; Monaco 2,170, 44 KB; Woodstock, Vermont 630, 13 KB; Takayama 5,855, 76 KB.
  B1's pilot tiles hold Shinjuku, Manhattan, Paris, Barcelona and Woodstock (§5.1); the others wait
  for B2's build.
- **Measured in B1 on the M1** (a 16-inch MacBook Pro, M1 Pro with a 16-core GPU, 120 Hz; Chrome
  152's engine in the Claude app's browser pane,
  1200 × 736 CSS px at 2×; the buildings' GPU buffers summed over the tiles in view, and their draw
  calls timed with `EXT_disjoint_timer_query_webgl2`, opacity 0.85; framed from the ground):

  | View | tiles | vertices | triangles | GPU memory | GPU time a frame (median / p90) |
  |---|---|---|---|---|---|
  | Shinjuku, z16, 60° | 17 | 0.90 M | 0.47 M | 24.4 MB | 3.2 / 3.9 ms |
  | Tokyo, Shinjuku to the horizon, z13.9, 70° | 13 | 2.52 M | 1.34 M | 68.5 MB | 5.2 / 5.6 ms |
  | Midtown Manhattan, z15, 70° | 20 | 0.82 M | 0.46 M | 22.5 MB | 3.7 / 4.3 ms |
  | Paris (Châtelet), z15, 60° | 16 | 1.38 M | 0.80 M | 38.0 MB | 2.4 / 3.0 ms |
  | Barcelona's old town, z16, 60° | 15 | 2.07 M | 1.17 M | 56.7 MB | 3.2 / 4.3 ms |
  | Woodstock, Vermont, z14.5, 60° | 8 | 0.05 M | 0.03 M | 1.3 MB | 1.7 / 2.3 ms |

  Opacity 1 (one pass) saved little here (Shinjuku 2.9 against 3.0 ms); "skyline only" took
  Shinjuku's buffers from 28.5 MB to 1.5 MB. The whole map's frame took 5–6 ms of GPU at Shinjuku
  with or without the buildings within the noise; the frame rate swung between ~50 and ~120 with
  or without them (the pane's pacing, other work on the Mac). Well within the iPad's budget on the
  M1; the iPad's own numbers decide.
- **The iPad checklist** (the owner's):
  1. Serve the pilot: a server from this branch with `--root` a folder laid out like the NAS's
     whose newest catalog has the pilot's `buildings` layer (B1 made one: catalog 14 with the
     pilot's six packs added, the rest of the folder linked to the NAS read only), a scratch
     `--home`, listening on every address (no `--listen`) so the iPad reaches it; the address with
     its key is in `<home>/map-page`.
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
     - **Console**: the buildings' GPU buffers in view, as B1 summed them:
       ```
       (() => { const z = (a) => (a ? a.length * a.bytesPerElement : 0); let v = 0, b = 0;
         for (const t of __app.map.style.tileManagers.bld._inViewTiles.getAllTiles()) {
           const k = t.buckets?.buildings; if (!k) continue; v += k.layoutVertexArray.length;
           b += z(k.layoutVertexArray) + z(k.centroidVertexArray) + z(k.indexArray);
           for (const c of Object.values(k.programConfigurations.programConfigurations))
             for (const d of Object.values(c.binders)) b += z(d.paintVertexArray); }
         return { vertices: v, mb: +(b / 1e6).toFixed(1) }; })()
       ```
  4. Each view with Buildings on, then off (B on a keyboard, or the switch), then opacity 100 %
     (the iPad's default already: a touch screen's is 100 %), then Skyline.
  5. Over budget (300 MB of buildings, 8 ms a frame) anywhere: the fallbacks below, and
     Barcelona's old-town tiles simplified (~18 % off).
- **If over budget**, on touch devices: opacity 1 (one pass, the default there anyway), z14 tiles only
  from zoom 15 ("skyline" between 13 and 15), a smaller tile cache for `bld`.

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
| **B1 Pilot** (done 2026-10-06, but the iPad) | `dem/bldprep.py`, `pipeline::bld` (prep, fill, tiles, job), `scenic-build bldprep` and `bldtiles`, run by hand on the build Mac into a scratch root (the NAS's sources read only) on 6/56/25 (Kantō), 6/32/22 (Paris), 6/18/24 (New York), 6/32/23 (Barcelona), 6/19/23 (Vermont, with Boston) and 6/3/28 (Oahu); formats.md entries; the catalog layer, the server's route; `web/src/buildings.ts` with the settings section, the toggle and hover; checked on this Mac in a test server (§4.6). `bldprep`: 88–153 s a tile (Kantō: 30.3 M rows read in 130 s, 4.6 GB at most; one thread: Paris in 71 s); `bldtiles`: 9–23 s a tile (Kantō 22 s, 3.6 GB at most). The iPad's measurements are the owner's (§4.6's checklist). | 6 days |
| **B2 In the build** | The agent: keys, targets, the chain's order, prunes, status and forecast labels, shared steps, `bld-fetch` as a job; the mirror's group, the service worker's budget, credits; every tile built and published. plan.md (§6, §8, §9, §10), workers.md, formats.md and the README updated. | 4 days |
| **B3 Sharing and polish** | `bldtile` tasks for pages (WebAssembly, byte-identical); bridges and elevated rail over buildings; walls on the terrain under each corner; fog on the extrusions; the camera's clearance; colour by height, by source, heritage tint. | 5 days |
| **B4 Each on its own measurement** | A custom layer (§4.7); measured heights from BD TOPO (France) and PLATEAU (Japan's cities); building heights in the horizons and the viewshed tool (every unit rebuilt). | 2–3 weeks |

B1–B3: about 15 days of work, the build's own time aside: `bldprep` reads 60 GB from the NAS (B1:
about 45 minutes over the 339 M buildings at the pilot's pace), `bldtiles` ~6 minutes over all
tiles natively at the pilot's pace (its own CPU; the coverage read once a run).

### 5.2 Risks

- **The release leaves S3 on 2026-11-25.** The coverage's files are being kept now. A region added
  later whose files aren't here waits for the next pinned release ("its buildings wait for the next
  Overture release", in the status); pinning one (~6-monthly, plan.md §8 Planned) fetches 62 GB again
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
- **A MultiPolygon's polygon wholly beyond one edge of its tile** (a building in pieces straddling a
  z14 edge) isn't drawn: MapLibre's extrusion skips a polygon outside its tile, a clipped tile's
  buffer copy as it assumes. Rare (the pieces of one building); B2 could put each polygon in the
  tile of its own centroid.
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
   duplicates); z12 and z13 only the tall and large.
7. **Two steps per z6 tile:** `bldprep` (impure: the NAS's parquet) and `bldtiles` (pure: tasks for
   pages), so the fill and the tiles can change without reading the parquet again.
8. **A chain of its own** that holds no region and no round; tiles in the regions' order.
9. **MapLibre's fill-extrusion** first, after the road and rail layers; a custom layer only if the
   measurements call for it.
10. **True heights** by default, with a scale setting; buildings on by default.
