# Scenic Roads

An interactive dark-theme map of every car-accessible public road, and every passenger rail line, in:
- **Canada:** every province and territory;
- **Saint-Pierre-et-Miquelon;**
- **the northeastern US:** New York, Vermont, New Hampshire, Maine, Massachusetts, Connecticut, Rhode Island;
- **Western Europe:** France, Monaco, Andorra, Spain, Portugal, Gibraltar, Great Britain, Ireland, the Isle of Man and the Channel Islands;
- **Hong Kong.**

It is built for finding scenic drives and rides:
- Roads are coloured by elevation, grade, or a family of scenic metrics.
- The metrics come from 15 km viewsheds that account for terrain and **tree canopy heights**.
- The roads are draped on 3D terrain with hill-shading.
- Passenger rail (trams, metros, commuter, intercity, heritage and mountain railways) is scored for the view from the carriage window.

Everything is downloaded and processed locally, then rendered as vectors by a custom WebGL2 layer.

```bash
make data   # download + build everything (first run ≈ 5 h, refreshes minutes)
make run    # http://localhost:8080
```

## Finding scenic drives

- **3D terrain + hill-shading, on by default** (height ×3, adjustable 1–6×).
  - ⌥ Option + two-finger drag: horizontal rotates, vertical tilts. Right-drag and Ctrl-drag do the same. Zoom, rotation and tilt all keep the 3D point under the cursor fixed on screen: the camera dollies toward it or orbits it rigidly, and stops 30 m short of the ground.
  - Panning moves the ground with your fingers at any height: a two-finger pan carries the ground at the view centre, and a click-drag keeps the grabbed point under the pointer, with the camera keeping its height (rising over ground in the way). The on-screen tilt, rotate, compass and zoom buttons, and double-click (Shift: out), turn about or move toward the ground at the view centre or the click.
  - Hill-shading method (combined, standard, Igor, multidirectional, basic), light direction and strength are adjustable.
  - The camera does not rise and sink with the terrain under the view centre. When zoomed in (past zoom 17 with the globe on, 12.5 on the flat map), the pivot is put back on the ground at the view centre after each move *without moving the camera*, so the zoom level (tile detail, widths, labels) matches the real distance to the ground.
  - **Globe** (default on), true to the Earth's curvature for all but street level. Tilted views show the real horizon distance (≈ 3.6 km × √eye height in m) and its dip, and far ranges sink behind the curve (≈ 800 m at 100 km). It hands over to flat Web Mercator at zoom 15.5–16.5, where the horizon is 30–80 km out in the fog (globe rendering, in float32, would wobble closer in).
    - MapLibre's globe camera orbits a sea-level centre and measures zoom from sea level. Near high ground the zoom number therefore stays lower than the real distance suggests: close to Mt Washington's summit you are at about zoom 13.5, and over high terrain you reach the ground before the flat hand-off. Detail follows the zoom number, and roads and labels are drawn for it.
    - Camera moves don't use MapLibre's zoom, pan and rotate, which work about that sea-level point. They are computed in earth-centred 3D and converted exactly into MapLibre's centre, zoom, pitch and bearing, then trimmed by a fraction of a pixel. This keeps the cursor point to within about 0.25 px from the ground out to the whole planet, including through the hand-off.
    - Flying to a result (a climb, a drive, the highest or lowest point) frames it from the ground, not from sea level. So does ▶ Drive, which stays zoom 14.6 back from the road.
  - Optional terrain tint, coloured by **elevation or terrain slope**. Slope is in %, computed with Horn's method at z12. Each coarser pixel takes the slope of one z12 pixel beneath it, picked pseudo-randomly (the `slope` pipeline step), so every zoom has the same mix of gentle and steep ground, and the colours stay consistent as you zoom out. Averaging would keep the mean but wash out the steep ground: over the White Mountains, ground ≥ 45 % fell from 7 % of the area at z12 to 0 % at z4; now it stays 6–7 %. From afar, the fine mix reads as the blend of colours, like a halftone. Below zoom 11 each tile pixel is drawn with its own value (`resampling: nearest`); MapLibre's default bilinear blending would turn a steep/gentle mix into uniform middling slopes. It uses a slope-class ramp from green to red to purple (100 % = 45°), and gentle slopes fade to fully transparent (adjustable per variable).
  - Tint options:
    - terrain ramps (atlas, slope classes, earth, glacier, greyscale, same as the roads) plus every shared colour map;
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
  | Scenic score | A weighted blend of 12 factors: views, water views, vista distance, mountains (terrain relief), ridge roads, twistiness, unblocked views, forest cover, built-up land, roadside buildings, and two yes/no factors, designated scenic route and viewpoint nearby. Waterfront, parks, heritage, UNESCO/dark-sky and farmland are shown but not scored. Presets: balanced, big vistas, lakes & coast, mountains, twisty, foliage, quiet backroads. The built-in presets are data (`web/src/data/weight-presets.json`, weights keyed by factor). In the app you can save a preset, save as a new one, rename and delete; edits are kept in the browser and "Built-ins" restores the file's list. |
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
  | Roadside buildings | Share of the road's frontage lined with buildings (both sides, ±50 m): full within 30 m of the centreline, fading out at 80 m, so joined village houses count far more than a farm set back from the road. Heights are not counted. |
  | Map | Roads as on a street map, with no metric: fixed colours by road size (OSM Carto, Google-ish, Michelin, Ordnance Survey; Neon, Amber night, Blueprint, Monochrome made for the dark basemap), by **signed route network** (each country's sign colours: Interstates and autoroutes blue, UK primary routes green, routes nationales red, départementales yellow…, from the route number and location), or by an attribute (speed limit, lanes, surface, one-way & toll). Every road is cased when zoomed in. |

- **No overlap brightening:** each pixel is painted by one road. A stencil pass draws the solid core of the first road at each pixel, with bridges and major roads first, then anti-aliased edges only outside cores. So junctions, joints and dense areas are no brighter than a single road.
- **Perspective line widths** (always on): in tilted views, each road segment is drawn as it would be at the zoom its own distance corresponds to, so far roads get thinner and fade like the ground they're on. Tilting about a point changes MapLibre's zoom number (it's measured to the view centre) but not that point's distance, so roads there keep their size.
- **Roads behind 3D terrain** are drawn faint (about a quarter strength, greyed), as if seen through the hill; "Hide roads behind terrain" hides them instead. Each road point is tested against the terrain as if moved toward the camera along its own line of sight by 1.5 % of its distance, at least 75 m × exaggeration. That absorbs where the terrain mesh (a vertex every two DEM pixels) and the roads' own heights disagree on steep slopes, without letting roads show through a ridge that is clearly in front.
- **Whole roads, not segments:** hovering highlights the whole road under the cursor (every OSM way of the same road, as a click selects), fetched once per road from `/api/road/{way}`.
- **Toll roads** (Layers → Roads): Toll-free and Toll roads toggles (OSM `toll=yes`, carried per line in the tiles), with km in view; they filter the map, hover, the km counts and scenic drives (not the elevation and grade distributions).
- **Road length filter** (Layers → Roads): min and max length in km of the *whole* road (every way with the same name or route number, joined end to end, as hover highlights). The `tile` step partitions the network into whole roads (`pipeline::roads`) and stores each line's road length in the tiles and per way (`roadlen.f32`). The map hides lines through a per-tile line mask, so drawing, hovering, the km counts in Layers and scenic drives all agree. The elevation and grade distributions in the stats don't follow it.
- **Threshold highlight:** above, below, or *above low end*, which follows the colour scale's left handle (auto-fitted or dragged). Dimmed roads can't be hovered or selected.
- **Accurate hover and click at any tilt:** candidates come from the terrain point under the cursor, and the winner is the road nearest on screen, measured between its projected endpoints.
- **Low values fade out:** roads at the low end of the colour scale become transparent (default 70 % over the lower 60 % of the scale; both adjustable, and the legend shows the fade).
- **Auto-fit percentiles** are configurable (default 1st–99th of road length in view).
- **Colour maps:** every colour-map picker (roads, rail, ferries, tree cover, terrain tint) is the same dropdown: gradients under section headings, live preview on the map while hovering an option (Escape or leaving the list reverts), and a ⇄ toggle that reverses the ramp (saved in the URL as `<name>_r`). One shared list of 35 ramps: perceptual (viridis, magma, plasma, inferno, cividis, mako, rocket, cubehelix), Crameri's scientific (batlow, hawaii, La Jolla, Oslo, Bamako, Tokyo), rainbow (turbo, Spectral, red–yellow–green), multi-hue (hypsometric and ColorBrewer's YlOrRd, YlGnBu, OrRd, PuBuGn), single hue (blues, greens, oranges, purples, ice, fire) and diverging (blue–grey–orange, red–blue, brown–teal, pink–green, cool–warm, Vik, and Berlin with a dark middle). `dem/ramps.py` samples them from their reference packages into `web/src/data/ramps.json`, oriented dark → light and lifted at the dark end (L* ≥ 25) for the dark map; diverging ramps keep both ends at L* ≥ 30. Layers with their own ramps (forest greens, terrain) list them first.
- **Settings per display type:** Elevation, Grade, Relief and Scenic each keep their own palette, low-end fade, auto-fit percentiles, equalisation and highlight (the scenic metrics share one set); ranges and threshold values, which are in each metric's units, are kept per metric.
- **Histogram equalisation** (the "Equalise" pill) spreads colours evenly over the road length in view. Ranges auto-fit to the view or can be locked or dragged, and the threshold highlight works on any metric.
- **Scenic drives** tab: the best 2/5/10/25 km stretch of every road in view, ranked with your weights and scored server-side in about 20 ms. Only roads with a continuous stretch of the chosen length are listed. Hover highlights a stretch; click opens its profile.
- **"In view" for the lists** (scenic drives and rides, rail lines, sights, the In view summary) is the ground on screen, not its bounding box. The client sends the screen outline, unprojected, and cuts it off where the ground gets too foreshortened to see (a pixel covering more than 4× the ground it covers at the centre). The server tests each stretch's midpoint against that outline (`Region`). A tilted view is a trapezoid whose bounding box took in Portugal and the Azores from Québec, and near a tilted globe's horizon a sliver of Ireland can show.
- **Drive and climb entries are links:** Cmd-click (or middle-click) opens one in a new tab, framed from the ground, with its road's profile open and the stretch highlighted. The URL (and Copy link) carries the stretch too.
- **Profile panel:**
  - scenic summary: average and best score, share of the road with open views or water in view, and roadside tree height;
  - an overlay line for any scenic metric;
  - **▶ Drive**, a 3D fly-along of the road.
- **Layout:** a sidebar on the right holds Layers (top half) and In view / Drives / Rides / Rail lines / Sights (bottom half), a fixed split with each half scrolling on its own; the info bar runs along the bottom up to the sidebar. Both are flush with the window edges and the map fills only the rest, so nothing renders underneath them.
- **Hover inspector** (bottom bar), laid out in fixed slots so nothing shifts as values change. Markers (stops & sights, heritage sites) win over roads, roads and rail lines over areas; a marker or highlighted area (park, heritage district, biosphere/geopark/dark-sky place, Indigenous land) with nothing else under the cursor gets its name, kind and facts in row 1 and a description in row 2 (see **Details** below). Areas under a hovered road appear as "in …" chips.
  - row 1: the road, elevation and grade at the cursor, then the scenic score and each measured score factor as a bar (full = the factor at its cap: 15 km vista, 600 m relief, +60 m ridge, 400 °/km, and so on), with the value at the tip of the fill (inside it when it fits, else just past it; units in the labels); bars for factors your weights penalise are red; then roadside tree height;
  - row 2: chips for scenic route, viewpoint nearby, waterfront, park, heritage nearby, covered bridge, UNESCO/dark-sky area and Indigenous land, then the road's tags and elevation sources; on the right, loading status, cursor position, zoom, scale, Copy link, and **© Credits** (data sources and licences in a dialog).
  - a rail line shows its services and line colour, and the ride factors in the same slots.
- **"What can I see from here?"** (Layers → Tools): click anywhere for a 2,048-ray viewshed (5–25 km, standing, platform or tower height), draped on the terrain, with visible and water area.
- **Layers panel**, in sections: Map (globe, line weight for roads, rail and ferries, water, boundaries by level — countries, provinces & states, counties & regions — place labels), Roads, Passenger rail lines, Ferries, Trees, Terrain, Stops & sights (parks, designations and points of interest), Tools.
- **In view** summary, each part while its layer is on: roads (length, km by class, scenic score and vista distance, share steeper than 10 %, highest and lowest road); rail and ferries (rail km by service group, busiest line in trains a day, highest point on rail; ferry routes, seasonal ones, busiest crossing); landmarks (count per kind and the best-known of each); terrain (highest named summit, relief, share of road above 1,000 m). Named places fly there on click; landmarks open their popup.
- **Drives** list: the best 2/5/10/25 km stretch of each road in view by the scenic score with your weights. "On map" highlights every listed stretch on the map; off, only the stretch under the cursor in the list is drawn.
- **Rides** list: the same for passenger lines, by the ride score with your ride-factor weights (`crates/server/src/rides.rs`: rail ways chained into lines by name, directions merged, "Highland Sleeper: London Euston => Fort William" and its return being one "Highland Sleeper"; the best stretch of each line).
- **Rail lines** list: the lines in view with their length in view, mean ride score and trains a day on the busiest part, sortable; hover highlights a line, click fits it.
- **Sights** list: the most prominent landmarks in view (the score that sizes their dots), with a chip per kind.
- **Stops & sights** (all toggleable, off by default; an Opacity slider scales their dots, areas and labels, labels also following Label opacity). Every landmark dot shows at every zoom, sized and faded by **prominence**: a score (0–100) mixing fame (Wikipedia pageviews, `dem/pageviews.py`: four months, one per season, streamed from Wikimedia's monthly dumps and summed over the place's articles in the map's languages) and rarity (interest isolation, `dem/interest.py`: the distance to a better-known place of its kind; articleless places ordered by name, OSM detail and size), with the Fame ↔ rarity balance. A scale like the roads' (histogram of the landmarks in view, auto-fitted percentiles, Lock / Full / Equalise, Fade low end, Highlight, Size contrast) turns the score into dot size and opacity; names fade with their dots and appear once a place's isolation spans ~90 px, the best-known winning collisions:
  - parks & protected areas;
  - heritage sites by group (World Heritage, national, provincial/state, municipal), each with toggles for its kinds of designation, which cut across jurisdictions (`dem/heritagetiers.py`): World Heritage cultural, natural & mixed; national top grade (Grade I/A, Category A, classés, BIC, monumentos nacionais), second grade (II*, B, inscrits, interesse público, the US National Register), lower grades, ancient & scheduled monuments, parks/gardens/battlefields, historic sites & landmarks, federal heritage buildings; provincial designated, registered, sites/districts/parks; municipal designated, on a local register, conservation areas & districts, agreements & covenants;
  - heritage districts;
  - biosphere reserves, geoparks and dark-sky places;
  - Indigenous lands;
  - viewpoints, peaks, waterfalls, lighthouses, covered bridges, rest areas and picnic sites, and trailheads, shown at every zoom (small dots zoomed out; names from zoom 8–9, highest peaks first; "Place labels" hides them with the place names). OpenStreetMap trailheads: mapped ones (`highway=trailhead`, common in the US), car parks named for a trail ("… Trail Parking", "Stationnement du sentier …") or tagged for hiking, and the two ends of linear hiking routes within 300 m of a drivable road (how Canadian trail starts are usually mapped), merged within 150 m;
  - gold glow on designated scenic byways and routes touristiques.

  Clicking a feature shows its designation, date and authority, with a link to the official record, and below them everything in its details.
- **Details** of stops & sights, heritage sites and areas, looked up from the server as you hover (`/api/detail/{layer}/{i}`, `/api/park`) so the map layers stay light; row 1 of the bottom bar gets the main facts, row 2 the description (truncated), and click popups show all of it with links (Wikipedia, Wikidata, website, OSM):
  - **peaks**: prominence and isolation (OSM- or Wikidata-tagged where given, else computed from the terrain, see below), the key col, hill lists (Munro, Corbett, Graham, Marilyn …), mountain range, SOTA reference, summit cross;
  - **waterfalls**: height, mean flow (Wikidata discharge), the river, seasonal flow;
  - **lighthouses**: light characteristic in chart notation (e.g. "Fl(2) W 10s"), range in nautical miles, focal height, tower height and type, year first lit, operator, light-list number;
  - **viewpoints** (panoramic or which way they look, observation towers), **covered bridges** (length, year built, truss type), **rest areas, picnic sites and trailheads** (toilets, water, shelter, tables, barbecue, fee, hours);
  - **heritage sites**: a short classification (English Wikipedia's short description, else Wikidata's), type, architectural style, architect, year built, the Wikipedia article; for the ~2,000 best-known sites and sights (by Wikipedia pageviews) a description of up to 55 words (written by Claude from the article, or where the article was about something else, researched from the site's register entry and other sources, which are credited);
  - **areas**: land area (every park, heritage district, biosphere/geopark/dark-sky place and Indigenous land), and for parks and protected areas the protection title and class, operator, owner, year established, visitors a year and description.
- Everything from v1 remains:
  - stats;
  - surface and class filters, with each road type's **unnamed** roads (neither a name nor a route number) toggled separately; hidden roads also leave the statistics and drives;
  - line weight (Map section; 0.1–1×, default 0.5×; also scales rail and ferries, relative to their own weights);
  - shareable URL (it includes bearing and pitch, and pasting a link applies it);
  - **G** opens Google Street View, **M** Google Maps and **O** OpenStreetMap with a marker at the cursor (on the hovered road if any), also right after using a slider or dropdown (only a text field keeps the key);
  - trackpad pan and pinch, anchored to the ground;
  - smooth mouse-wheel zoom;
  - progress indicators.

- **Passenger rail** (Layers → Passenger rail lines; styling in the top-left panel under the colour settings):
  - five groups, toggled separately, with km in view: trams; metro & rapid transit; commuter & regional; intercity (sleepers included); heritage & mountain railways (tourist lines, rack railways, funiculars). A track counts for every group whose services use it.
  - colour by the **official line colour** (OSM `colour`; the group's colour where a line has none), by service type, by a metric, or one colour; railway symbol (thin line with cross-ties) or solid; casing; line weight.
  - the metric colouring has the same controls as the roads': a histogram of the rail in view with draggable range handles, auto-fit percentiles (default 2nd–98th), Auto / Lock / Full / Equalise, the colour map (with reverse), low-end fade and fade span, and the highlight (above, below, above low end; dimmed lines can't be hovered). Each metric keeps its own settings.
  - ride-factor **presets** like the road score's (Balanced, Mountain lines, Coastal & water, Viaducts & engineering, Frequent service; `web/src/data/rail-presets.json`), with Save, Save as, Rename and Delete.
  - **ride score**, weights adjustable: views, water in view, long vistas and mountains, measured with the same viewsheds as roads from a carriage window (2.8 m); ledges & gorges (track high on a slope or deep below its surroundings); altitude; **viaduct height** above the ground beneath; tunnels (negative by default: you see nothing); gradient; curves.
  - hover shows the services on a track and its trains a day; click opens the line's profile (elevation, grade and the scenic channels along it), like a road.
  - **service frequency**: trains a day each way on a typical weekday, per stretch of track with every service using it added up. A colouring (log scale; grey where no timetable was found), a ride factor (+0.3 by default, full at 100 trains a day; tracks without a timetable leave it out of their score rather than counting as none), and a filter in Layers (min–max a day, keep or hide tracks without a timetable). Ferries have the same filter on sailings a day.
  - frequencies come from published timetables only (see Data): `dem/railfeeds.py` finds every GTFS feed in the Mobility Database catalogue that runs rail in our regions (reading only each feed's `routes.txt`, by HTTP range requests); `dem/railgtfs.py` counts, for each feed's median-busy Tuesday–Thursday in the next three months, the trains between consecutive stops (a train published in two feeds is counted once, keyed by its end stops and times); `railfreq` matches each stop pair onto the OSM tracks (each stop snapped to its nearest few tracks within 300 m, right kind of track first, so a stop between the two tracks of a double-track line can use either; then a shortest path along the track graph from any of A's to any of B's, other networks' tracks costing 4×, dangling track ends bridged to other tracks within 100 m since crossovers and bits of station throats aren't in the network) and adds its trains to every way on the path. A path takes one track of a multi-track line, so counts are then summed across each corridor (every parallel track within 40 m with the same kind of service) and every track in it gets the total; that gave 43,500 tracks with no trains of their own their corridor's count. About 91 % of stop pairs match; most of the rest are outside our regions (national feeds such as Amtrak's) or have no coordinates. Great Britain's national timetable comes from Catenary Transit's daily GTFS build of the Rail Delivery Group data (the official download needs a Rail Data Marketplace account); Hong Kong's MTR publishes no GTFS, so its lines were researched on mtr.com.hk (`data/rail/mtr.json`, turned into stop pairs by `dem/mtrpairs.py`): the Airport Express and High Speed Rail are exact counts of their published timetables; the other lines and Light Rail routes only have average headways per named period (without the periods' clock times), so they get a lower bound, the service hours at the slowest published weekday off-peak headway, shown as "at least N trains a day".

- **Ferries** (Layers → Ferries; styling in the top-left panel under Passenger rail): every public ferry route in OSM (1,474 lines on 1,857 ways), car ferries included (they stay in the road layer too).
  - four groups with km in view: urban & commuter (city water buses and commuter boats); short crossings; long-distance & overnight (2 h 30 or more); cable & chain ferries.
  - colour by service group, a **metric**, season (year-round daily, year-round some days, seasonal), operator (official line colours where tagged, else one colour per operator), or one colour. Line weight, opacity, dashed or solid.
  - metrics: **sailings a day** each way (log scale; grey where no timetable has been found) and **season length** (months a year: 12 for year-round lines, else parsed from the published season, e.g. "mid-Jun – 15 Sep" = 3; grey where unknown), with the same controls as roads and rail (histogram of the ferry lines in view with draggable handles, auto-fit percentiles, Auto / Lock / Full / Equalise, colour map, fades, highlight), each metric keeping its own.
  - ferry names along the lines and named terminals, following Place labels.
  - hover shows the line, its group, crossing time, sailings, season and operator, with the route and the timetable source in the second row; click lists every line on the way with links to the operator, the timetable source and OSM.
  - frequencies are never estimated: each comes from the operator's GTFS timetable, a published timetable looked up by hand (with its page), or the line's OSM `interval` and `opening_hours` tags when both are given. See Data.

- **Tree cover** (Layers → Trees): tree cover (share of the ground under trees over 5 m), canopy height (95th percentile, where cover is at least 5 %) or forest leaf type (broadleaf, conifer, mixed), draped on the terrain under the hill-shading, about 25 m per pixel over all our regions (roadless interiors included, neighbours outside the region outlines left out).
  - cover and height: a shaded ramp (palettes, low-end cutoff) or a flat forest mask above a threshold; opacity.
  - leaf type: Copernicus HRL Dominant Leaf Type 2018 in Europe (broadleaf / coniferous; some Mediterranean broadleaf is evergreen), NALCMS 2020 in North America (needleleaf, broadleaf deciduous, mixed); none in Hong Kong.
  - tiles (`dem/trees.py`): zoom 4–12, Terrarium-encoded lossless WebP (cover in 2 % steps, height in 2 m steps), coloured on the GPU with MapLibre color-relief like the slope tint; coarser zooms average (leaf type: each class's share is averaged and a pixel shows the commonest leaf type where forest is at least half of it; a cascaded majority vote made farmland with scattered woods look forested when zoomed out).

**Settings persist.** Every map setting, plus UI choices such as tabs, drive length, climb sort, viewshed options, profile overlay and collapsed sections, is saved in localStorage and restored when you open the app without a link. A link's URL hash takes precedence.

## How the scenic metrics are computed

1. **Samples.** Roads and rail lines are sampled every 100 m. Eye height is 1.5 m above the road and 2.8 m above the rail. Bridges use the deck, and tunnels see nothing.
2. **Near field (0–300 m).** A horizon is computed in 32 directions from the terrain plus the **median canopy height** from Meta/WRI's global 1 m canopy-height map (10° aggregate product). A direction that rises more than 5° counts as blocked, so tree walls and cuts register as enclosure.
3. **Far field (300 m – 15 km).** Rays are cast over a z11 grid (~55 m) of terrain plus canopy, with earth curvature and refraction. Each ray starts at that direction's near-field horizon, so roadside trees block distant views too. The pass accumulates visible area, visible water (ESA WorldCover) and farthest visible distance.
4. **Landscape.** The pass also computes:
   - relief within 3 km;
   - topographic position within 1.5 km;
   - open land within 1 km;
   - built-up land within 500 m;
   - **roadside buildings** (`scenic buildings`, `pipeline::buildings`): Overture building footprints' bounding boxes (OSM, Microsoft and Google footprints merged), near the road network. At points every 5 m within 50 m either side of each 100 m sample, each side counts the nearest building whose extent along the road covers the point: fully if its near edge is within 30 m of the centreline, fading to nothing at 80 m. The sample's value is the mean over points and sides. Sheds under 15 m² are ignored; rail and ferries get none. A negative weight in every built-in preset (−0.6 to −1.5); presets and links saved before the factor existed get −1;
   - forest cover within 150 m;
   - p95 roadside tree height within 30 m.
5. **Flags.** These add viewpoints within 1 km, heritage sites within 500 m (plus heritage-district polygons), parks, UNESCO and dark-sky areas, Indigenous lands, and waterfront within 100 m. The `scenic flags` step recomputes them in seconds when designations change.
6. **Score.** Each scored factor is normalised to 0–1 and weighted: score = Σ wᵢcᵢ / Σ max(wᵢ, 0). The browser (per vertex, in the shader) and the server (drives ranking) use the same formula. Waterfront, park, heritage and UNESCO/dark-sky flags and open land are computed and shown but not part of the score.
7. **Fresh data after a rebuild.** Tiles and layers are cached by the browser for a day, so the app adds each data file's build time to their URLs (from `/api/meta`): a rebuilt file is fetched anew.

## Data

| What | Source | Licence / terms |
|---|---|---|
| Roads, water, boundaries, places, parks, POIs, Indigenous land boundaries | OpenStreetMap (Geofabrik extracts) | © OpenStreetMap contributors, ODbL |
| Road & rail elevation, North America | NRCan **HRDEM** lidar (8 m overview) → USGS **3DEP** 10 m → NRCan **MRDEM** 30 m | OGL–Canada / public domain |
| Road & rail elevation, Europe & Hong Kong | **FABDEM** v1-2 30 m (University of Bristol / Fathom; Hawker et al. 2022), Copernicus DEM with forests and buildings removed; read tile by tile from Bristol's zips (`/vsizip//vsicurl/`) | CC BY-NC-SA 4.0 (non-commercial; attribution text in © Credits) |
| Gibraltar | OpenStreetMap via the Overpass API (no Geofabrik extract) | © OpenStreetMap contributors, ODbL |
| 3D terrain, hill-shading, contours, analysis grid | Terrain Tiles (Terrarium) on AWS Open Data | Mapzen / various open sources |
| Tree canopy height & cover | Meta & WRI global canopy height (1 m, 10° aggregates: median, p95, cover > 5 m) | CC BY 4.0 |
| Forest leaf type, Europe | Copernicus HRL Dominant Leaf Type 2018, 10 m (EEA image service; no Azores or Madeira data) | Copernicus free and open data policy |
| Forest leaf type, North America | NALCMS 2020 Land Cover of North America, 30 m (CEC; NRCan/CCRS, USGS, INEGI, CONAFOR) | CEC terms of use (attribution) |
| Land cover (water, open land, built-up) | ESA WorldCover 2021 10 m | CC BY 4.0 |
| Building footprints (roadside buildings) | Overture Maps buildings, release 2026-09-23.1 (OSM, Microsoft, Google) | ODbL · CDLA Permissive 2.0 |
| UNESCO World Heritage | UNESCO open data, dataset `whc001` (data.unesco.org), one point per component | © UNESCO World Heritage Centre, CC BY-SA 4.0 |
| Canadian federal designations | Parks Canada **Directory of Federal Heritage Designations** (National Historic Sites, heritage lighthouses & railway stations, federal heritage buildings) | OGL–Canada |
| US designations | NPS **National Register of Historic Places** incl. National Historic Landmarks | Public domain |
| Québec | MCC **Répertoire du patrimoine culturel** (classified, declared, national, municipally cited; heritage-site perimeters) | CC BY 4.0 |
| Ontario | **Ontario Heritage Act Register** (Ontario Heritage Trust), heritage conservation districts | Personal non-commercial use only |
| Nova Scotia | Registered Heritage Properties (provincial); Halifax (HRM) municipal heritage properties | NS Open Government Licence; HRM open data |
| New Brunswick, PEI, Newfoundland & Labrador, the western provinces, the territories | **Canadian Register of Historic Places** (historicplaces.ca); Moncton open data | Non-commercial reproduction with credit |
| Biosphere reserves, geoparks, dark-sky places | UNESCO MAB & Global Geoparks lists, DarkSky International, RASC dark-sky programme (`data/heritage/special-official.json`) | Facts from the official registries |
| France | Ministère de la Culture, base **Mérimée** (monuments historiques classés and inscrits); sites patrimoniaux remarquables from the Géoportail de l'Urbanisme | Licence Ouverte 2.0 |
| Andorra | Govern d'Andorra, Inventari general del patrimoni cultural (IDE Andorra) | Private, personal use only |
| England · Scotland · Wales · Northern Ireland | Historic England NHLE; Historic Environment Scotland; Cadw via DataMapWales; DfC Historic Environment Division: listed buildings (top two grades), scheduled monuments, registered parks & gardens, battlefields | OGL v3 (attribution in © Credits) |
| Guernsey | States of Guernsey protected buildings and monuments | gov.gg terms (research and private use) |
| Ireland | **NIAH** buildings rated National or International; Sites and Monuments Record entries in State care or under a preservation order | CC BY 4.0 |
| Spain | Regional registers of **bienes de interés cultural** with locations: Catalonia (BCIN), Castilla y León, Aragón, Comunitat Valenciana, Galicia, Navarra, Extremadura; Andalucía's protected heritage (IAPH) | Per region: CC BY / CC BY-SA / free use with credit |
| Portugal | Património Cultural, I.P., **Atlas do Património Classificado** (monumentos nacionais, interesse público, interesse municipal; mainland) | CC BY-NC 4.0 |
| Hong Kong | Antiquities and Monuments Office via the CSDI Portal: declared monuments, graded historic buildings | DATA.GOV.HK terms (credit the CSDI Portal) |
| Details: facts about POIs, heritage sites and parks | Wikidata (items matched by OSM `wikidata` tags and by heritage register IDs), queried on QLever's Wikidata endpoint (University of Freiburg) | CC0 |
| Details: heritage descriptions | English Wikipedia short descriptions; descriptions written by Claude from Wikipedia articles, or researched from register entries and other pages for sites whose article didn't fit | CC BY-SA 4.0 (source article or sources credited) |
| Peak prominence & isolation | Computed from the terrain tiles (`peaks` step); OSM and Wikidata values preferred where tagged | Derived |

**Official sources first.** Wikidata is used only to supply coordinates for federal designations, because the Parks Canada directory has none. `dem/federal.py` cross-checks every federal record against Wikidata by name within its province, then falls back to named OSM features. It locates 672 of 705 National Historic Sites and 1,346 of 1,815 federal designations. Records that are unlocated or only loosely matched are listed in `data/heritage/federal-report.csv`. 218 Wikidata items that claim a federal designation missing from the official directory are excluded.

**Terms.** The UNESCO data is CC BY-SA 4.0 (the notice is shown on each site); descriptions are not reproduced, and each site links to its official page. FABDEM and Portugal's atlas are non-commercial (CC BY-NC); the Ontario Heritage Trust register, Andorra's inventory and Guernsey's data are for personal or private use. **Do not publish or redistribute the built data or a hosted copy of this map.**

**Lowest grades left out.** England and Wales Grade II, Scotland category C, Northern Ireland B1/B2, NIAH "Regional" and Catalonia's local-interest (BCIL) listings, ~450,000 buildings in all, would drown the map in points. The national registers of Spain (no coordinates), Monaco, the Isle of Man (permission required), Jersey (token-protected) and Gibraltar (statutory text only) aren't usable, so those rely on UNESCO and the regional registers.

**Names in the region's language.** Roads, places, parks, peaks and other OSM features use OSM's default `name`, which is the local name, kept bilingual where it is signed that way (Hong Kong, the Basque Country, parts of Wales and New Brunswick). Register names are the registers' own. UNESCO sites use UNESCO's French or Spanish name where those are the local language, and Wikidata's Portuguese, Catalan or Galician label where UNESCO has none. Parks Canada sites in Québec take the French name of their Wikidata match (or the OSM name they were located by). Biosphere reserves, geoparks and dark-sky places, listed in English by the registries, use Wikidata's local label when it names the same kind of place. The English name is kept in the data as `name_en`.

**Passenger rail selection.** Tracks (`railway=rail, light_rail, subway, tram, narrow_gauge, funicular, monorail, preserved`; not yards, sidings, crossovers, or freight, industrial or military usage) used by a passenger route relation (`route=train, tram, subway, light_rail, monorail, funicular`). A train route's group comes from its `service` tag (long-distance, high-speed and night → intercity; tourism or heritage → heritage & mountain; else commuter & regional) or, untagged, from its name, brand and operator (TGV, Intercités, AVE, Alvia, Alfa Pendular, Amtrak, VIA Rail, LNER, sleepers…). Tracks with no route relation are kept when their type says what they are: tram, subway and light rail, funiculars and rack railways, preserved and tourist lines. Each track records every group using it, its services' refs and names, and the most important service's colour.

**Ferries.** `osmium tags-filter` pulls `route=ferry` ways and relations and `amenity=ferry_terminal` out of the merged extract (`data/ferries/`). `dem/ferries.py` builds *lines*: one service in both directions, from a route master's relations, the direction variants of the same ref or the same pair of ports, or a named ferry way (or run of them) that no route relation uses. Private ferries are left out. Groups: cable & chain by `ferry=cable/chain`, `ferry:cable=yes` or the name; long-distance by `duration` of 2 h 30 or more (or 80 km without a duration); urban & commuter by a list of city transit networks and operators; the rest are short crossings. Sailings, in order of preference:
- **GTFS** (`dem/gtfs.py`, feeds in `data/ferries/gtfs-feeds.json`): ferry trips (route types 4, 1000–1099, 1200–1299) that call within 400 m (at most 30 % of the crossing) of both ends of a line, counted per day of service and averaged over both directions. The line gets the median over the days it runs, the quietest month's median (year-long feeds), the midday headway on a typical day, and the weekdays and months with service.
- **Published timetables** looked up by hand for lines without a feed (`data/ferries/freq/timetables-*.json`, each with the page and the date it was checked).
- **OSM tags:** `interval` with a single daily span in `opening_hours` (sailings = span ÷ interval); an interval alone gives the headway only.

On a stretch of sea used by several lines, lines to different ports add up (routes sharing a trunk), while lines between the same two ports (within 3 km: duplicate OSM relations, or one operator's line beside an all-operators one) count once, at the highest figure. A line's ports are its two farthest-apart way ends.

**Details** (`make details`):
- `dem/poidetails.py`: each POI is matched to its OSM feature (an `osmium` export of the POI tags, `data/poi/`: nearest of the same kind within 60 m, same name preferred) and keeps the tags worth showing per kind; 262,238 of 293,825 POIs match; the 26,457 with a `wikidata` tag get heights, elevation, prominence, isolation, discharge, inception, river, range and description from Wikidata. Summits tagged as viewpoints too (Mont Blanc) become peaks. `pois.json` is rewritten with each POI's index (`i`), which the details are keyed by.
- `peaks` (Rust): for every peak, the summit is the highest DEM pixel within 150 m of the OSM point (a pixel several peaks share goes to the one tagged highest), at the tagged height where it is plausible (sharp summits come out 20–50 m low in the DEM), and every summit pixel is raised to its peak's height so neighbours count at their real heights. **Prominence** is a priority flood from the summit (always expanding the highest unvisited pixel) until it reaches higher ground: the lowest point on that path is the key col. It runs at z12 for up to 600,000 pixels, then over the whole region at z8 for the big peaks; reaching sea level ends it (prominence = height). **Isolation** is the distance to the nearest higher pixel, searching tiles nearest first. Single-pixel spikes and pits in the terrain tiles (a 2,781 m pixel over the Laurentides) are clamped first. 154,078 peaks take about 3 minutes; the few whose flood would cover a continent (Mont Blanc) stop at 40 million pixels and give a lower bound. Checks: Mulhacén 3,286 m (published 3,285; its col the Seuil de Naurouze at 193 m, which OSM tags as its key col; isolation 528 km to the Atlas), Aneto 2,811 m (2,812), Mont Ventoux 1,150 (1,148), Ben Nevis 1,345 (1,345; isolation 740 km to Norway, published 739), Snowdon 1,026 (1,039), Mount Washington 1,848 with its col in the Champlain–Hudson lowland and isolation 1,326 km to the Black Mountains.
- `dem/heritagewd.py`: heritage sites matched to Wikidata by register ID, else through the OSM feature at the site (tagged with a Wikidata item and carrying the same register ID or clearly the same name; the only link for registers with no Wikidata property); Wikipedia articles counted in every language (159,066 of 195,661; NHLE, Mérimée, HES, NRHP, Cadw, DGPC, CRHP, RPCQ, IPAC, UNESCO, Irish SMR; the Parks Canada directory already links items): descriptions, Wikipedia articles, sitelinks, inception, type, style and architect; then English Wikipedia's short descriptions. `dem/desctargets.py targets N` picks the N best-known heritage sites and sights with an article (by pageviews), saves their articles' lead sections and splits them into batches for the description writers (subagents following `data/heritage/desc/WRITERS.md`, from the extract only, checked by `dem/desccheck.py`); `desctargets.py skipped` gathers the lines they skipped (the article was about something else) for research from other sources (`RESEARCH.md`; the researched descriptions carry their sources); `dem/heritagedetails.py` assembles `details-heritage.jsonl`.
- `dem/layers.py`: the overlays as the map loads them (`layer-*.json`: lean properties, points in draw order by fame, heritage districts simplified to ~1 m; the heritage sites' other properties in `props-heritage.jsonl`, served with their details).
- `dem/areadetails.py`: spherical area of every area polygon; the 34,050 named protected areas' OSM tags and Wikidata facts (5,525 items), for the lookup by name and position.

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
| `extract` | ~4 min | Roads and passenger rail, access rules, densification (8 m in North America, 15 m elsewhere), POIs, scenic routes, route-network codes and line colours. |
| `sample.py` | ~2 h first run (FABDEM streams 314 one-degree tiles), seconds afterwards | DEM sampling with a vertex cache (412 M vertices). |
| `tile … elev` | ~1 min | Elevation clean-up only (`final.i16`), which the scenic samples need before the tiles exist. |
| `terrain` | ~10 min | Terrarium tiles z0–12 near roads (8.4 GB) and the z11 analysis grid. |
| `slope` | ~4 min; seconds when little changed | Slope pyramid (7.7 GB), only tiles whose terrain is new recomputed: z12 slopes in 1/16 % steps (whole percents left lens-shaped blotches around the tint's thresholds when interpolated); each coarser pixel is one pseudo-randomly picked pixel beneath it, keeping the slope distribution. |
| `landcover.py` | minutes; seconds when little changed | WorldCover classes on the grid (new grid tiles only). |
| `scenic prep` | seconds | 100 m samples and drape heights. |
| `scenic canopy` | ~20 min; ~30 s when little changed | Streams 27 Meta 10° canopy files (22 GB, cached) and computes near-field horizons, for new samples and grid tiles only. |
| `scenic view` | ~1 min | 44 M 32-ray far-field viewsheds (roads and rail), new samples (or those near new terrain) only. |
| `buildings.py` | ~2.5 h once | Streams Overture's building bounding boxes for the regions from its public S3 release with DuckDB (~170 M buildings, ~2.7 GB). |
| `scenic buildings` | minutes | Roadside buildings per sample. |
| `leaftype.py`, `trees.py` | ~20 min, ~35 min; only new squares / blocks later | Leaf-type squares (EEA image service requests; the NALCMS GeoTIFF streamed out of its 3.9 GB zip by byte range), then the tree cover tiles from the cached canopy files (~5 GB). |
| `federal.py`, `crhp.py`, `heritage.py` | minutes | Official registers, areas rasterised to the grid. `crhp.py` politely fetches ~3,400 register pages once and caches them. |
| `scenic flags` | 6 s | Designation flags and per-vertex channels. |
| `tile` | ~7 min | Clean-up, climbs, whole-road lengths, z4–14 road tiles (`roads.tiles`) and rail tiles (`rails.tiles`) with drape heights, 13 scenic channels (tile format v6) and per-line attributes (route network, speed limit, lanes, surface, line colour). |

- `make heritage` reruns just the designation steps and tiles.

### Adding a region

Add it to `regions.json` (a Geofabrik extract path, or an Overpass area query and bbox where Geofabrik has none; and the Overture building file covering it, adding a bbox to `buildings` if none does), then `make data`. Nothing already built is redone:
- **OSM** (`dem/osmupdate.py`): only the new region is downloaded and merged into `merged.osm.pbf`; extracts are deleted once merged and in the basemap (`make osm-refresh` updates every region from Geofabrik).
- **Elevations**: sampled per vertex with a cache, so only new roads are sampled.
- **Terrain tiles**: tiles already downloaded are reused. **Slope**: only tiles whose terrain is new are recomputed (`data/cache/steps/slope.keys`). **Land cover**: new grid tiles only.
- **Canopy and viewsheds** (`pipeline::scache`, `data/cache/scenic`): the previous outputs stay in the build until each step finishes, and the cache records what each of their rows was (every road sample's key: position, eye height, flags; the grid tiles). A sample found keeps its results unless new terrain lies within reach of it (300 m for the near field, ~2 grid tiles for views), so only the new region's roads, and roads right next to it, are computed. Only the 10° canopy files holding new work are read. `scenic <build> seed-cache` records a build made before the cache.
- **Tree cover** (`data/cache/trees`): each zoom-8 block's inputs are fingerprinted; unchanged blocks are copied from the previous archives, and zoom 7–4 are rebuilt from their kept zoom-8 values.
- **Basemap**: base.pmtiles covers the regions in `base.regions`; each region added later gets its own part (`base-parts/<id>.pmtiles`, a Planetiler run over just that region), served alongside and drawn with clones of the same layers. `make basemap-full` rebuilds one basemap for everything.
- **Heritage Wikidata** is cached per register ID, POI and park Wikidata per item, rail timetables per feed.
- Rebuilt in full (a few minutes each): extract, the tile pyramid, the heritage rasters, peaks, the details.

Adding Saint-Pierre-et-Miquelon this way takes minutes of new work plus the full-rebuild steps; the rest of Canada mostly the time to download and compute its own data.
- Every stage writes `*.tmp` files and renames them into place; restart the server to serve new data.
- Binaries are order-only prerequisites: code edits don't force data rebuilds, so delete an output to redo its step.

**Disk:**
- ~50 GB build:
  - 8.4 GB terrain;
  - 7.7 GB slope;
  - 5 GB per-vertex scenic channels;
  - 4.9 GB road tiles, 0.09 GB rail tiles;
  - 6.5 GB analysis grids;
  - the basemap;
- 22 GB canopy cache;
- 26 GB OSM extracts (the regions plus their merge);
- 5 GB elevation cache.

**Memory:** 16 GB is enough but tight: the elevation clean-up and the tile step swap for a while on the 412 M-vertex network.

**Requirements:** Rust, Node, [uv](https://docs.astral.sh/uv/), Java 21+, `osmium-tool`.

## Rendering notes

- Each road segment is one instanced quad expanded in screen space. The fragment shader handles caps and joins, antialiasing, dashes and the palette lookup. The metric, including the weighted score from 12 per-vertex channels, is computed in the vertex shader, so mode, weights, equalisation (a CDF lookup texture), threshold and filters are all uniform changes.
- In 3D, roads are lifted by drape heights × exaggeration and depth-tested against MapLibre's terrain. A pitched view uses a quadtree tile cover: fine tiles near the camera, coarse tiles toward the horizon, and nothing more than 6 zooms coarser than the view (the fogged horizon).
- The tile cover projects screen samples analytically. `map.unproject` ray-marches the 3D terrain on the CPU; using it cost ~140 ms per frame in tilted views. With 3D terrain, each sample's view ray is intersected with levels spanning the terrain heights in view (probed on a 5 × 3 grid), not with the camera pivot's level: on the globe the pivot is at sea level, and over high, exaggerated terrain that level lands far beyond the real ground. About 14 ms per tilted view.
- **Depth precision on the globe.** MapLibre's globe view uses a 0.5 px near plane (its 2D layers compute their own depth), but the terrain mesh and 3D custom layers use the perspective depth. With the near plane that close, float32 left depth good to only ~20 % of the distance: roads and terrain z-fought (roads hatched with the terrain's triangles, flickering as you moved) and the terrain fought itself. While the globe renders, the near plane is the flat map's (viewport height ÷ 50), at most half the camera's height above the ground (`camera3d.tuneDepth`), 28× the precision.
- **Points and labels behind terrain on the globe.** MapLibre fades circles and symbols behind 3D terrain by comparing their depth with its terrain depth texture, but on the globe it computed their depth with its clipping-plane scheme while the texture holds perspective depth, so in tilted views from about zoom 11 every point and label counted as hidden. A small Vite plugin (`vite.config.ts`) patches MapLibre's circle and symbol shaders to compute that depth the way the texture was drawn (identical on the flat map). In a tilted view of Mount Royal, frame-to-frame flicker on a 0.002-zoom nudge fell from 406 k to 110 k pixels (the rest is the image moving), and on the bare terrain from 275 k to 35 k.
- Per-tile matrices are composed in float64, and tiles decode in a worker pool that also builds the elevation and grade sketches.

### Performance

- **Projection pass.** Each frame the camera moves, a transform-feedback pass projects every road and rail segment once (its ends on screen, their depths with the terrain tolerance, its perspective scale) and flags segments wholly in front of or behind the terrain, from MapLibre's terrain depth texture. The draw passes read that instead of each re-projecting every quad corner, and skip flagged segments. The pass is skipped while the camera is still (hover, colour-range easing, restyling).
- **Fewer road passes.** Minor classes (service to unclassified) narrower than a CSS pixel draw in one pass instead of core and fringe, and not at all in the faint behind-the-terrain pass. That pass is skipped entirely when the view is tilted less than 10°.
- **Overlays off the main thread.** The map's overlay sources get their layer files by URL, so MapLibre's worker fetches and tiles them. The files are `layer-*.json` from `dem/layers.py`: lean properties, points sorted by fame so the best known draw on top without a per-dot sort key, and heritage districts simplified to ~1 m. A second worker (`landmarks.worker.ts`) indexes the stops & sights and heritage sites for everything "in view": prominence histogram, counts, Sights list, summit. The page never parses or holds the features. Heritage popups get their record (dates, authority, links, source) from `/api/detail`, merged from `props-heritage.jsonl`.
- **View-dependent work waits for the view to settle.** The trackpad camera moves by `jumpTo`, so MapLibre fires `moveend` after every wheel event. In-view summaries, lists, the link and re-levelling run 160 ms after the last camera change instead. While moving, only the road statistics update, at most every 400 ms. The "in view" ground outline is computed once per camera on the terrain-free globe. The stats loop sleeps when there is nothing to do.
- **Benchmark:** `tools/bench/bench.mjs` drives a headless Chrome with the real GPU through the DevTools protocol (usage at the top of the file). It sends real trackpad pans, pinches, ⌥-orbits and hovers at 120 Hz and reports frame intervals, long tasks, MapLibre's render CPU, GPU time per frame (timer queries), time to settle and page exceptions. `--profile` adds a CPU profile (self and inclusive time), `--profile-load` profiles loading, `--trace` gives busy time per thread, and `--variants` measures the static render with parts of the state switched off one at a time.
- **Measured** (M1 Pro, 1512 × 900 at 2×, 3D terrain, hillshade, tint, trees, rail, ferries, heritage sites and districts, indigenous lands), trackpad pan:

  | View | Before | After |
  |---|---|---|
  | Canada, zoom 4 | — | 59 fps |
  | Northeastern North America, zoom 5 | — | 56 fps |
  | England, zoom 6.5, tilted 30° | 4 fps | 35 fps |
  | Alps, zoom 9, tilted 55° | — | 60 fps |
  | Toronto zoom 10, Paris zoom 12 | — | 60 fps |

  Loading England went from 9.7 s to about 1 s of main-thread blocking, and the page's JavaScript heap from 745 MB to 215 MB. Dense road networks at low zoom are still limited by the roads layer's per-segment attribute fetches, several million segments per pass.

## Known limitations

- The analysis grid is ~55 m, so narrow gorges and summit domes are smoothed. A viewshed from a rounded summit at 1.7 m eye height is pessimistic; try the platform or tower height.
- Canopy heights are 2019–2020 medians over ~28 m cells: recent clear-cuts and new growth aren't reflected, and a road through forest reads as enclosed even if its right-of-way is wide.
- Federal designations located "by name" or via OSM are flagged as approximate in their popup.
- Bridge decks are interpolated between abutments, and grade reflects DEM and OSM accuracy. Drape heights under bridges are the ground beneath; the renderer lifts decks to their own elevation, and the difference is a viaduct's height.
- Rail services depend on OSM route relations: a passenger line without one (and not a tram, metro, funicular or heritage line by its track tags) is missing, and a train route with neither a `service` tag nor a recognisable name counts as commuter & regional.
- Browsers pause WebGL in hidden tabs.
