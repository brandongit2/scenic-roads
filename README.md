# Scenic Roads

A personal, offline-capable dark map of every car-accessible public road and every passenger rail line and ferry in its regions, built for finding scenic drives and rides. The regions today: Canada, the United States (with DC and Puerto Rico), Western Europe (France, Spain, Portugal and their islands, the UK, Ireland and the small states and Crown dependencies between them), Saint-Pierre-et-Miquelon, French Guiana, Japan, Taiwan, Hong Kong and Singapore.

- Roads are coloured by elevation, grade, or scenic metrics computed from 15 km viewsheds over terrain and **tree canopy heights**, and draped on 3D terrain with hill-shading, on a globe.
- Passenger rail is scored for the view from the carriage window and coloured by official line colour, service or trains a day; ferries by sailings a day or season.
- Landmarks (peaks, waterfalls, lighthouses, viewpoints, heritage sites, parks…) are sized by prominence, with details from official registers and Wikidata.
- Optional 3D buildings, tree cover, coastal shading and contour lines.

## Using it

The map runs on each Mac at http://localhost:7311, installed by `tools/app/install.sh` and kept running by launchd. The build runs while Scenic (/Applications/Scenic.app, installed by the same script) is open. Its data lives on the NAS; nothing is copied to a Mac unless downloaded there (Settings → Regions → Downloads: the World zoomed out, a region, or the view).

- **Navigation:** trackpad pan and pinch anchored to the ground; ⌥ + two-finger drag (or right-drag) rotates and tilts; double-click zooms. **I** hides the interface, **B** toggles buildings. Over a road, list entry or profile: **G** Street View, **M** Google Maps, **O** OpenStreetMap.
- **Settings panel** (left): a section per layer: Roads, Passenger rail, Ferries, Stops & sights, Terrain, Trees, Buildings, Map, Labels, Tools, Regions. Each colour scale has a histogram of what's in view, auto-fit (by screen widths of road or percentiles), Lock / Full / Equalise, a colour map, a low-end fade and a threshold highlight.
- **Lists** (bottom right): In view, Drives (best 2–25 km stretch of each road in view by your weights), Rides, Rail lines, Sights. Clicking a road or line opens its elevation profile with ▶ Drive, a 3D fly-along.
- **Bottom bar:** hover inspector (score factors as bars, tags, areas), progress, the lead's and NAS's state, and **© Credits**.
- **What can I see from here?** (Tools): a 2,048-ray viewshed from any point.
- Settings persist in the browser; the URL carries them, so a link shares the view. Defaults are `defaults` in `web/src/state.ts`; Reset to defaults is at the foot of the panel.

### Display types

| Mode | Shows |
|---|---|
| Elevation, Grade, Relief | Height, steepness, local relief. |
| Scenic score | A weighted blend of 12 factors (below); presets in `web/src/data/weight-presets.json`, editable in the app. |
| Views, Water views, Vista distance | Area and water area visible within 15 km; mean farthest visible distance. |
| Terrain drama, Ridge ↔ valley | Relief within 3 km; height above or below terrain within 1.5 km. |
| Curviness | Turning in °/km. |
| Unblocked views | Share of directions not walled in within 300 m. |
| Roadside trees, Forest cover, Open land, Built-up | Tree height and cover, fields, built-up land nearby. |
| Roadside buildings | Share of frontage lined with buildings within 30–80 m. |
| Map | Street-map schemes, signed route networks, or an attribute (speed, lanes, surface, toll). |

## How the scenic metrics are computed

1. **Samples** every 100 m along roads (eye 1.5 m) and rail (2.8 m); bridges use the deck, tunnels see nothing.
2. **Near field (0–300 m):** a horizon in 32 directions over terrain plus Meta/WRI's median canopy height; a direction rising more than 5° is blocked.
3. **Far field (300 m – 15 km):** rays over a ~55 m grid of terrain plus canopy, with curvature and refraction, starting at the near-field horizon: visible area, visible water (ESA WorldCover), farthest visible distance.
4. **Landscape:** relief within 3 km, topographic position within 1.5 km, open land within 1 km, built-up land within 500 m, roadside buildings (Overture footprints), forest cover within 150 m, p95 roadside tree height within 30 m.
5. **Flags:** scenic route, viewpoint within 1 km, heritage within 500 m, parks, UNESCO/dark-sky areas, Indigenous lands, waterfront within 100 m.
6. **Score:** each factor normalised to 0–1; score = Σ wᵢcᵢ / Σ max(wᵢ, 0), the same in the shader and the server.

Rail frequencies come only from published timetables (GTFS feeds from the Mobility Database and operators, plus hand-researched files), matched onto OSM tracks (`docs/steps.md`). Ferry sailings come from GTFS, hand-checked timetables, or OSM `interval` + `opening_hours`; never estimated.

## Building and running

The lead Mac's agent (`scenic agent`, `crates/pipeline/src/agent`) builds the map into the NAS from a worldwide OpenStreetMap download, a region at a time, and publishes a catalog that each Mac's server switches to in place. Other Macs and any browser on the LAN help through its coordinator (`docs/workers.md`); the lead can be handed between Macs (`scenic lead`, `docs/pool.md`).

- `scenic status` shows what the build is doing; `scenic pause` / `scenic resume` pause the whole build; `scenic --help` lists the commands. The menu bar item and the lead's build page (`/work/`) show and control the same.
- **Inputs** are NAS drop boxes, each checked before use (`docs/inputs.md`): regions (`inputs/regions/`, §6.1), translations and descriptions (`inputs/translations/`, `inputs/descriptions/`; what still lacks them is listed in each `todo/`), heritage registers (§6.5), languages (§6.2), timetables (§6.7), elevation (§6.8).
- **Publishing the app** (`tools/app/publish.sh`) needs Rust (per `rust-toolchain.toml`), Node, pnpm and the wasi-sdk (per `mise.toml`, `mise install`) and `pnpm install --frozen-lockfile` in `web/`. The agent also needs uv, `osmium-tool`, Java 21+ and Planetiler's jar on the NAS.
- **Formatting**: run `cargo fmt` before committing (`rustfmt.toml`); `tools/app/guards.sh` fails publishing on `cargo fmt --check`. `.git-blame-ignore-revs` skips the one reformat commit (`git config blame.ignoreRevsFile .git-blame-ignore-revs`).
- Tools: `tools/bench/` (load and frame benchmarks), `tools/coastcheck/` (shoreline accuracy), `tools/taskboard/` (task board).

## Docs

- The design: `docs/surface.md` (what the user sees and does), `docs/building.md` (units, jobs, the scheduler and its order), `docs/storage.md` (the NAS), `docs/macs.md` (each Mac's agent, server and mirror), `docs/regions.md` (coverage and regions), `docs/steps.md` (each step in detail, its keys), `docs/names.md` (names and descriptions), `docs/sizes.md`. Phases, known gaps, risks and decisions are on the task board.
- `docs/diagram/`: the pipeline diagram (`python3 page.py out.html`, then republish out.html to https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg).
- `docs/pipeline.md`: every build step as one row of one table (the concepts, the names, the checklist, the build page).
- `docs/formats.md`: the files.
- `docs/inputs.md`: the input drop boxes and their checks.
- `docs/workers.md`: the coordinator and its workers.
- `docs/pool.md`: any Mac leading the build.
- `docs/buildings3d.md`: 3D buildings.
- `web/maplibre/FORK.md`: our MapLibre changes.

## Data

| What | Source | Licence / terms |
|---|---|---|
| Roads, rail, ferries, water, boundaries, places, parks, POIs, Indigenous land boundaries | OpenStreetMap | © OpenStreetMap contributors, ODbL |
| Lakes missing from OSM in parts of Canada | Natural Resources Canada **CanVec** 1:50,000 Hydro (2018) | Contains information licensed under the Open Government Licence – Canada |
| Road & rail elevation, North America | NRCan **HRDEM**, USGS **3DEP** 10 m, NRCan **MRDEM** 30 m | OGL–Canada / public domain |
| Road & rail elevation, Japan | GSI elevation tiles (地理院タイル 標高タイル) | GSI terms of use (Public Data License 1.0): "Created by editing GSI Tiles (elevation tiles (Fundamental Geospatial Data Digital Elevation Model))" |
| Road & rail elevation elsewhere | **FABDEM** v1-2 30 m (University of Bristol / Fathom; Hawker et al. 2022) | CC BY-NC-SA 4.0 (non-commercial; attribution in © Credits) |
| 3D terrain, hill-shading, contours, analysis grid | Terrain Tiles (Terrarium) on AWS Open Data, repaired; north of 60°N Copernicus DEM GLO-30 | Mapzen / various open sources; produced using Copernicus WorldDEM-30 © DLR e.V. 2010–2014 and © Airbus Defence and Space GmbH 2014–2018 provided under COPERNICUS by the European Union and ESA |
| Tree canopy height & cover | Meta & WRI global canopy height | CC BY 4.0 |
| Forest leaf type, Europe | Copernicus HRL Dominant Leaf Type 2018 | Copernicus free and open data policy |
| Forest leaf type, North America | NALCMS 2020 Land Cover of North America (CEC; NRCan/CCRS, USGS, INEGI, CONAFOR) | CEC terms of use (attribution) |
| Land cover | ESA WorldCover 2021 10 m | CC BY 4.0 |
| Buildings (roadside and 3D) | Overture Maps buildings (© OpenStreetMap contributors, Microsoft, Esri Community Maps, USGS, IGN España, Google and others) | ODbL 1.0 · CDLA Permissive 2.0 |
| 3D building heights in high-rise cores | GHSL GHS-BUILT-H R2023A (EC JRC), © European Union, 1995-2026 | CC BY 4.0 |
| Rail and ferry frequency | Operators' GTFS timetables (Mobility Database catalogue; SNCF, Renfe; Rail Delivery Group via Catenary Transit; Hong Kong Transport Department; Singapore LTA DataMall); the MTR's lines researched on mtr.com.hk | Each operator's open-data terms; Singapore Open Data Licence 1.0 |
| UNESCO World Heritage | UNESCO open data `whc001` | © UNESCO World Heritage Centre, CC BY-SA 4.0 |
| Canadian federal designations | Parks Canada Directory of Federal Heritage Designations | OGL–Canada |
| US designations | NPS National Register of Historic Places | Public domain |
| Québec | MCC Répertoire du patrimoine culturel | CC BY 4.0 |
| Ontario | Ontario Heritage Act Register (Ontario Heritage Trust) | Personal non-commercial use only |
| Nova Scotia | Registered Heritage Properties; Halifax municipal heritage properties | NS Open Government Licence; HRM open data |
| Other Canadian provinces and territories | Canadian Register of Historic Places; Moncton open data | Non-commercial reproduction with credit |
| Biosphere reserves, geoparks, dark-sky places | UNESCO MAB & Global Geoparks, DarkSky International, RASC | Facts from the official registries |
| France | Ministère de la Culture, base Mérimée; Géoportail de l'Urbanisme | Licence Ouverte 2.0 |
| Andorra | Govern d'Andorra, Inventari general del patrimoni cultural | Private, personal use only |
| England · Scotland · Wales · Northern Ireland | Historic England NHLE; Historic Environment Scotland; Cadw via DataMapWales; DfC Historic Environment Division | OGL v3 (attribution in © Credits) |
| Guernsey | States of Guernsey protected buildings and monuments | gov.gg terms (research and private use) |
| Ireland | NIAH; Sites and Monuments Record | CC BY 4.0 |
| Spain | Regional registers of bienes de interés cultural; Andalucía's IAPH | Per region: CC BY / CC BY-SA / free use with credit |
| Portugal | Património Cultural, I.P., Atlas do Património Classificado | CC BY-NC 4.0 |
| Hong Kong | Antiquities and Monuments Office via the CSDI Portal | DATA.GOV.HK terms (credit the CSDI Portal) |
| Japan | Agency for Cultural Affairs, 国指定文化財等データベース; MLIT 国土数値情報 A43 | PDL 1.0 ("edited" credit); A43 CC BY 4.0 |
| Taiwan | Bureau of Cultural Heritage open data | Open Government Data License 1.0 |
| Singapore | data.gov.sg: NHB, URA | Singapore Open Data Licence 1.0 |
| Facts about POIs, heritage sites and parks | Wikidata, via QLever (University of Freiburg) | CC0 |
| Heritage descriptions | English Wikipedia short descriptions; descriptions written by Claude from Wikipedia articles or register entries | CC BY-SA 4.0 (sources credited) |
| Peak prominence & isolation | Computed from the terrain; OSM and Wikidata values preferred | Derived |

**Terrain repair** (`roadcore::grid::repair_terrain`, in the `terrain` job). AWS's Terrain Tiles have voids and spurious towers and pits, mostly over water. One pass repairs only what is broken or undefined: voids (above 8,900 m or below −11,500 m); blobs standing over 100 m out of their ground, steeper than real terrain can be and far rougher than their surroundings; small steep spikes; ringing and walled patches where AWS's sources meet (`seam_spikes`, `walled_patches`). Each is filled with a harmonic surface from the ground around it. Summits, ridges, cliffs, islands, sea stacks, mesas and buttes stay; coarser zooms are rebuilt from the repaired finer tiles. North of 60°N the terrain is GLO-30's; everywhere the sea goes to 0 m and each lake to its shore's level, from the basemap's water (`terrain_north`, `terrain_water`, `docs/steps.md`).

The map's © Credits dialog lists each source as its catalog serves it (`CREDITS` in `crates/pipeline/src/rules.rs`, plus the inputs' own descriptions).

**Terms.** FABDEM and Portugal's atlas are non-commercial (CC BY-NC); the Ontario Heritage Trust register, Andorra's inventory and Guernsey's data are for personal or private use. **Do not publish or redistribute the built data or a hosted copy of this map.**

## Known limitations

- The analysis grid is ~55 m, so narrow gorges and summit domes are smoothed; a viewshed from a rounded summit at 1.7 m is pessimistic.
- Canopy heights are 2019–2020 medians over ~28 m cells.
- Bridge decks are interpolated between abutments; grade reflects DEM and OSM accuracy.
- A passenger line with no OSM route relation (and not a tram, metro, funicular or heritage line by its tags) is missing.
- The lowest heritage grades (England Grade II and the like, ~450,000 buildings) are left out; Spain's national register, Monaco, the Isle of Man, Jersey and Gibraltar have no usable register.
