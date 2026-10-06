"""How the map is built (docs/plan.md v7): OpenStreetMap comes from one worldwide download, cut by area; a region is
only an outline of what to build. Every step runs per area (a z6 tile), per z3 pack near the coverage, or once,
worldwide; ⇄ marks the steps whose jobs the M1's helper may take, TASK an area's last steps, which any worker may
run (docs/workers.md). Rounds publish what's built as regions are done. Dashed: planned (docs/buildings3d.md for the
3D buildings)."""
from diag import COLS, Diagram

GAP, PAD = 12, 10


def build(check=False):
    d = Diagram('p', check)

    # ---- 0. OpenStreetMap: the planet, cut by area; your regions are outlines -----------------
    y0 = 60
    cy = y0 + 24
    s_osm = d.src('osm', cy, 'OSM planet', ['the whole world; the NAS', 'fetches it twice a year,', 'resuming if interrupted'],
                  kept=['sources/osm/<date>/'])
    n_pass = d.card('osm', 'd1', cy, 'OSM pass', 'osmium', ['filtered, then cut by area', '(z6 tiles, 10 km buffer);', 'sets, outlines and every', 'way’s whole road'],
                    [(['pieces/<z6>.osm.pbf'], 'PBF · all land ≈ 58 GB'), (['sets/ · outlines'], 'nine sets: rail, ferries …'),
                     (['roads/<z6>'], 'each way’s road and offset')], scope='global')
    s_reg = d.src('osm', s_osm.b + 8, 'Your regions', ['88 today, as OSM draws them:', 'provinces, states, countries;', 'their union is the coverage'],
                  kept=['inputs/regions/'])
    d.arrow('osm', (s_osm.r, n_pass.y + 18), (n_pass.l, n_pass.y + 18))
    d.arrow('osm', (n_pass.r, n_pass.y + 18), (n_pass.r + 34, n_pass.y + 18), label='read by the rows below as “OSM pieces” and “OSM sets”',
            at=(n_pass.r + 42, n_pass.y + 22))
    yr = max(s_reg.my, n_pass.b + 16)
    d.arrow('osm', (s_reg.r, yr), (n_pass.r + 34, yr), label='which tiles and features every step below builds; no step knows region borders',
            at=(n_pass.r + 42, yr + 4))
    y1 = max(s_reg.b, n_pass.b) + PAD
    d.lane('OPENSTREETMAP · YOUR REGIONS', y0, y1)

    # ---- 1. map context: one worldwide basemap, worldwide labels ------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_base = d.card('base', 'd1', cy, 'basemap', 'Java', ['Planetiler, the whole world,', 'jar and data pinned'],
                    [(['layers/basemap/world-<date>'], 'PMTiles · MVT z0–14')], scope='global')
    n_lab = d.card('base', 'd1', n_base.b + 22, 'labels', 'Python', ['ranked worldwide: places,', 'water and parks'],
                   [(['layers/labels/'], 'packs · MVT z0–12')], scope='global')
    s_pl = d.src('osm', cy, 'OSM pass', ['the basemap’s part: water,', 'borders, parks, places'], minh=n_lab.b - cy)
    p_base = d.pill('base', n_base.y + 30, ['/tiles/base'], note=['tile by tile from the', 'archive; names attached'])
    b_base = d.layer('base', 0, 'Basemap', ['water · borders · parks · rivers'], cy=p_base.my)
    b_coast = d.layer('base', b_base.b + 8, 'Coastal shading', ['distance to shore, from the', 'basemap’s water, in a worker'], computed=True)
    b_lab = d.layer('base', max(n_lab.y + 6, b_coast.b + 8), 'Place labels', ['placed by importance'])
    p_lab = d.pill('base', b_lab.my, ['/tiles/labels'], note='names attached')
    b_find = d.layer('base', b_lab.b + 8, 'Place search', ['the box: any word of a name,', 'as the map shows it'])
    p_find = d.pill('base', b_find.my, ['/api/places'], note='indexed on this Mac')
    d.arrow('osm', (s_pl.r, n_base.y + 18), (n_base.l, n_base.y + 18))
    d.arrow('osm', (s_pl.r, n_lab.y + 18), (n_lab.l, n_lab.y + 18))
    d.arrow('base', (n_base.r, p_base.my), (p_base.l, p_base.my))
    d.arrow('base', (n_lab.r, p_lab.my), (p_lab.l, p_lab.my))
    d.arrow('base', (n_lab.r, p_find.my), (p_find.l, p_find.my))
    d.arrow('base', (p_base.r, b_base.my), (b_base.l, b_base.my))
    d.arrow('base', (p_base.r, p_base.my + 8), (b_coast.l - 12, p_base.my + 8), (b_coast.l - 12, b_coast.my), (b_coast.l, b_coast.my))
    d.to_layer('base', p_lab, b_lab)
    d.to_layer('base', p_find, b_find)
    y1 = max(n_lab.b, b_find.b, p_find.b, s_pl.b) + PAD
    d.lane('MAP CONTEXT', y0, y1)

    # ---- 2. names in English: your translations straight to the servers --------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_osmn = d.src('osm', cy, 'Every area’s names', ['with each thing’s own English:', 'OSM’s, Wikidata’s, Wikipedia’s'])
    n_inv = d.card('base', 'd1', cy, 'names to-do · planned', 'Rust', ['what lacks English, by', 'language: a name, its kind,', 'where it is, how known'],
                   [(['translations/todo/'], 'a list per language')], scope='global', later=True)
    y2 = max(s_osmn.b, n_inv.b) + 32
    s_tr = d.src('base', y2, 'Translations', ['your finished files, dropped', 'into the NAS folder'], kept=['translations/'])
    p_en = d.pill('base', s_tr.my, ['English attached', 'to all it serves'], note=['a thing’s own English,', 'else a translation'])
    b_en = d.layer('base', 0, 'English everywhere', ['labels, basemap names, roads,', 'rail lines, stops, popups'], cy=p_en.my)
    d.arrow('osm', (s_osmn.r, n_inv.y + 18), (n_inv.l, n_inv.y + 18), dashed=True)
    yt = p_en.b - 8
    d.arrow('base', (s_tr.r, yt), (p_en.l, yt),
            label='read by both Macs’ servers within a minute or two: no rebuild, M4 awake or not', at=(n_inv.l + 30, yt - 5))
    yl = (n_inv.b + s_tr.t) / 2   # the translators' loop, outside the pipeline
    d.arrow('base', (n_inv.l + 30, n_inv.b), (n_inv.l + 30, yl), (s_tr.l + 120, yl), (s_tr.l + 120, s_tr.t), dashed=True,
            label='translators', at=(n_inv.l + 38, yl + 4))
    d.to_layer('base', p_en, b_en)
    y1 = max(n_inv.b, s_tr.b, b_en.b) + PAD
    d.lane('NAMES IN ENGLISH', y0, y1)

    # ---- 3. places & heritage --------------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_reg2 = d.src('place', cy, 'Registers (a snapshot)', ['UNESCO · Parks Canada · NRHP', 'Mérimée · NHLE · 文化財 …'], kept=['sources/registers/'])
    s_osm2 = d.src('osm', s_reg2.b + 8, 'OSM pieces · sets', ['POIs · designated areas', 'summits · hiking routes'])
    s_wd = d.src('place', s_osm2.b + 8, 'Wikidata · Wikipedia', ['facts; pageviews, four', 'months a pass, all languages'])
    n_hs = d.card('place', 'd1', cy, 'heritage sites', 'Python', ['heritage.py on the registers', 'over the coverage + 20 km,', 'sliced per area'],
                  [(['work/heritage/<date>/'], 'positions · areas per z6')], scope='global')
    n_poi = d.card('place', 'd1', n_hs.b + 22, 'candidates · peaks', 'Rust', ['each area’s points; prominence', 'and isolation over z12 and', 'the worldwide z8'],
                   [(['work/pois/ · work/peaks/'], 'per area')], scope='area', shared=True)
    n_items = d.card('place', 'd2', n_poi.y, 'facts · pageviews', 'Python', ['the candidates’ Wikidata', 'items, once a pass'],
                     [(['sources/items/<date>/'], 'facts · views'), (['sources/pageviews/'], 'each month streamed once')], scope='global')
    n_hc = d.card('place', 'd2', cy, 'heritage chain', 'Python', ['on the pass: each site’s', 'details, outlines and fame'],
                  [(['work/heritage/<date>/'], 'details · outlines · fame')], scope='global')
    n_mk = d.card('place', 'd3', cy, 'landmarks', 'Rust', ['points per kind: fame, rarity,', 'thinned tiles, per-cell counts'],
                  [(['markdata/<z6> · marks-*/'], 'sectioned · RDMT tiles')], scope='global')
    n_ov = d.card('place', 'd3', n_mk.b + 22, 'area overlays', 'Rust', ['from the heritage chain, with', 'the landmarks’ site ids'],
                  [(['layers/ov-*/ · ovdata/'], 'packs · MVT; details per z3')], scope='global')
    p_mk = d.pill('place', n_mk.y + 30, ['/api/marks/…'], note=['tiles by view; In view,', 'details'])
    b_lm = d.layer('place', 0, 'Landmarks', ['dots on the GPU; names tiled', 'in a worker; popups'], cy=p_mk.my)
    p_ov = d.pill('place', n_ov.y + 30, ['/tiles/ov/…'], note='the pack for the tile')
    b_ov = d.layer('place', 0, 'Area overlays', ['heritage areas, Indigenous', 'lands, World Heritage outlines'], cy=p_ov.my)
    d.arrow('place', (s_reg2.r, n_hs.y + 18), (n_hs.l, n_hs.y + 18))
    d.arrow('osm', (s_osm2.r, s_osm2.my), (s_osm2.r + 14, s_osm2.my), (s_osm2.r + 14, n_poi.y + 18), (n_poi.l, n_poi.y + 18))
    yw = max(n_poi.b, n_items.b) + 6
    d.arrow('place', (s_wd.l + 90, s_wd.b), (s_wd.l + 90, yw), (n_items.l + 60, yw), (n_items.l + 60, n_items.b))
    d.arrow('place', (n_poi.r, n_items.y + 18), (n_items.l, n_items.y + 18))
    d.arrow('place', (n_hs.r, n_hc.y + 18), (n_hc.l, n_hc.y + 18))
    d.arrow('place', (n_hc.r, n_mk.y + 18), (n_mk.l, n_mk.y + 18))
    xi = n_items.r + 14
    d.arrow('place', (n_items.r, n_items.y + 18), (xi, n_items.y + 18), (xi, n_mk.y + 46), (n_mk.l, n_mk.y + 46))
    d.arrow('place', (n_mk.l + 30, n_mk.b), (n_mk.l + 30, n_ov.t))
    d.arrow('place', (n_mk.r, p_mk.my), (p_mk.l, p_mk.my))
    d.arrow('place', (n_ov.r, p_ov.my), (p_ov.l, p_ov.my))
    d.to_layer('place', p_mk, b_lm)
    d.to_layer('place', p_ov, b_ov)
    y1 = max(n_poi.b, n_items.b, s_wd.b, b_ov.b, n_ov.b, yw) + PAD
    d.lane('PLACES & HERITAGE', y0, y1)
    y_places = y1

    # ---- 4. terrain: per z3 pack near the coverage -----------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_aws = d.src('terr', cy, 'AWS Terrain Tiles', ['Terrarium PNG · ~27 m at z12', 'kept raw, never edited'],
                  kept=['sources/aws-terrarium/', 'an area’s tiles packed'])
    n_terr = d.card('terr', 'd1', cy, 'terrain', 'Rust', ['z9–12 within 20 km of the', 'coverage (z11, z10 nearer the', 'poles); z3–8 per pack; repaired'],
                    [(['layers/terrain/'], 'packs · Terrarium PNG')], scope='pack', shared=True)
    n_slope = d.card('terr', 'd2', cy + 34, 'slope', 'Rust', ['Horn at z12, kept to z11;', 'each pixel 4 quarter means'],
                     [(['layers/slope/'], 'packs · PNG')], scope='pack', shared=True)
    n_z8 = d.card('terr', 'd2', n_slope.b + 22, 'z8 terrain', 'Rust', ['every z8 tile, repaired,', 'for peaks (not served)'],
                  [(['sources/terrain-z8-v1'], 'one pack')], scope='global')
    p_t = d.pill('terr', cy + 12, ['/tiles/terrain'], note='the pack for the tile')
    b_terr = d.layer('terr', 0, '3D terrain · hill-shading', ['elevation tint'], cy=p_t.my)
    b_cont = d.layer('terr', b_terr.b + 8, 'Contour lines', ['traced from terrain tiles'], computed=True)
    b_slope = d.layer('terr', b_cont.b + 8, 'Slope tint', ['colour per quarter, averaged'])
    p_s = d.pill('terr', b_slope.my, ['/tiles/slope'], note='z12 made when asked')
    d.arrow('terr', (s_aws.r, n_terr.y + 18), (n_terr.l, n_terr.y + 18))
    d.arrow('terr', (n_terr.r, n_slope.y + 18), (n_slope.l, n_slope.y + 18))
    d.arrow('terr', (s_aws.l + 90, s_aws.b), (s_aws.l + 90, n_z8.y + 18), (n_z8.l, n_z8.y + 18))
    d.arrow('terr', (n_terr.r, p_t.my), (p_t.l, p_t.my))
    d.arrow('terr', (n_slope.r, p_s.my), (p_s.l, p_s.my))
    d.to_layer('terr', p_t, b_terr)
    d.arrow('terr', (p_t.r, p_t.my + 8), (b_cont.l - 12, p_t.my + 8), (b_cont.l - 12, b_cont.my), (b_cont.l, b_cont.my))
    d.to_layer('terr', p_s, b_slope)
    y1 = max(n_terr.b, n_z8.b, b_slope.b, s_aws.b) + PAD
    d.lane('TERRAIN', y0, y1)
    l4_bottom = y1

    # ---- 5. roads (per road point, per area) ---------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_ext = d.card('net', 'd1', cy, 'extract', 'Rust', ['the ways each area owns,', 'inside the coverage'],
                   [(['base/<z6>'], 'sectioned · per road point')], scope='area', shared=True)
    n_samp = d.card('terr', 'd2', cy, 'elev · tile elev', 'Rust', ['DEMs chosen by location,', 'then clean-up and grade'],
                    [(['elevations · grade'], 'in the base pack')], kept='cache/dem-units/', scope='area', shared=True, task=True)
    n_scen = d.card('scen', 'd3', cy, 'scenic', 'Rust', ['samples every 100 m: horizons', 'to 300 m past trees, views to', '15 km, buildings, designations'],
                    [(['13 channels a point'], 'in the base pack')], kept='cache/scenic-units/', scope='area', shared=True, task=True)
    n_tile = d.card('net', 'd4', cy, 'roads & rails', 'Rust', ['per z6 tile: the areas within', '110 km and their road values;', 'climbs, tiles, query parts'],
                    [(['layers/roads/ · rails/'], 'packs · RT v7 z4–14'), (['hidata/<z6>'], 'query parts · summaries')], scope='area')
    s_osm4 = d.src('osm', cy, 'OSM pieces', ['roads · rail lines'])
    s_dem = d.src('terr', s_osm4.b + 8, 'Road DEMs', ['HRDEM · 3DEP · MRDEM (N. Am.)', 'GSI (Japan) · FABDEM 30 m', 'read by range, road blocks only'],
                  minh=n_ext.b + 22 - (s_osm4.b + 8))
    p_road = d.pill('net', n_tile.y + 30, ['/tiles/roads · rails', '/api/road · profile'], note=['the pack for the tile;', 'APIs by OSM id'])
    b_road = d.layer('net', 0, 'Roads & rail lines', ['WebGL, coloured per point:', 'elevation, grade, scenic score'], cy=p_road.my)
    y_sc = max(n_tile.b + 16, b_road.b + 8 + 32.5)
    p_scen = d.pill('scen', y_sc, ['/api/drives · rides', '/api/viewshed'], note=['zoomed out: 500 m', 'summaries'])
    b_scen = d.layer('scen', 0, 'Drives · rides · viewshed', ['road parts joined by their', 'offset along the road'], cy=p_scen.my)
    ys = cy + 18
    d.arrow('osm', (s_osm4.r, ys), (n_ext.l, ys))
    d.arrow('net', (n_ext.r, ys), (n_samp.l, ys))
    d.arrow('terr', (n_samp.r, ys), (n_scen.l, ys))
    d.arrow('scen', (n_scen.r, ys), (n_tile.l, ys))
    y_dem = max(n_ext.b + 10, s_dem.y + 14)
    d.arrow('terr', (s_dem.r, y_dem), (n_samp.l, y_dem))
    d.arrow('net', (n_tile.r, p_road.my), (p_road.l, p_road.my))
    xq = n_tile.l + 150
    d.arrow('scen', (xq, n_tile.b), (xq, y_sc), (p_scen.l, y_sc))
    d.to_layer('net', p_road, b_road)
    d.to_layer('scen', p_scen, b_scen)
    y1 = max(n_scen.b, n_samp.b, s_dem.b, b_scen.b) + PAD
    d.lane('ROADS · PER ROAD POINT', y0, y1)

    # ---- 6. land cover, trees & buildings ------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_wc = d.src('land', cy, 'ESA WorldCover', ['10 m land cover, 2021'])
    n_lc = d.card('land', 'd1', cy, 'grids', 'Rust', ['land cover, canopy, cover at', 'z11, made in each area’s job'],
                  [(['layers/grid-*/'], 'packs · z11, not served')], scope='area', shared=True)
    y2 = max(s_wc.b, n_lc.b) + 22
    n_rb = d.card('bldg', 'd1', y2, 'roadside buildings', 'Py · Rust', ['every building’s box, from the', 'release’s bbox columns, once', 'per release'],
                  [(['sources/buildings/<rel>/'], 'z8 tiles of boxes · index')], scope='global')
    n_bp = d.card('bldg', 'd1', n_rb.b + 22, 'bldprep · planned', 'Py · Rust', ['an area’s buildings and parts,', 'decoded; GHSL at each'],
                  [(['work/bld/<z6>'], 'sectioned · per z14 block')], scope='area', later=True)
    s_bld = d.src('bldg', y2, 'Overture buildings', ['release 2026-09-23.1, on S3:', 'the world’s footprints; in', 'the coverage, heights or', 'floors for ~46 %'],
                  kept=['sources/overture/<rel>/', 'the coverage’s, for 3D'], minh=n_bp.y + 34 - y2)
    s_gh = d.src('bldg', s_bld.b + 8, 'GHSL building heights', ['EC JRC: the mean height in', 'each ~90 m cell, 2018'], kept=['sources/ghsl/R2023A/'])
    n_b3 = d.card('bldg', 'd2', n_bp.y, '3D buildings · planned', 'Rust', ['heights: measured, else floors,', 'neighbours, GHSL, size, kind;', 'later PLATEAU’s, BD TOPO’s'],
                  [(['layers/buildings/'], 'packs · MVT z12–14')], scope='area', later=True)
    p_bld = d.pill('bldg', n_b3.y + 40, ['/tiles/buildings'], note='z12–14, overzoomed', later=True)
    b_bld = d.layer('bldg', 0, '3D buildings', ['extruded on the terrain:', 'towers from z12, all from z14'], cy=p_bld.my, later=True)
    d.arrow('land', (s_wc.r, n_lc.y + 18), (n_lc.l, n_lc.y + 18))
    d.arrow('bldg', (s_bld.r, n_rb.y + 18), (n_rb.l, n_rb.y + 18))
    d.arrow('bldg', (s_bld.r, n_bp.y + 18), (n_bp.l, n_bp.y + 18), dashed=True)
    yg = max(s_gh.y + 18, n_bp.y + 46)
    d.arrow('bldg', (s_gh.r, yg), (n_bp.l, yg), dashed=True)
    d.arrow('bldg', (n_bp.r, n_b3.y + 18), (n_b3.l, n_b3.y + 18), dashed=True)
    d.arrow('bldg', (n_b3.r, p_bld.my), (p_bld.l, p_bld.my), dashed=True)
    d.to_layer('bldg', p_bld, b_bld, dashed=True)
    s_can = d.src('land', max(s_gh.b, n_bp.b, n_b3.b) + 12, 'Canopy height · leaf type', ['Meta & WRI (1.2 m imagery)', 'Copernicus HRL · NALCMS'],
                  kept=['sources/canopy/ (10°)', 'sources/trees/leaf/'])
    n_trees = d.card('land', 'd1', s_can.y + 30, 'tree cover', 'Rust', ['cover · height · leaf type,', 'z4–12 clipped to the coverage;', 'a region’s before it goes out'],
                     [(['layers/trees-*/'], 'packs · Terrarium WebP')], scope='pack', shared=True)
    p_trees = d.pill('land', n_trees.my, ['/tiles/trees/{var}'], note='the pack for the tile')
    b_trees = d.layer('land', 0, 'Tree cover · height · leaf', ['colour-relief on the GPU'], cy=p_trees.my)
    d.arrow('land', (s_can.r, n_trees.y + 18), (n_trees.l, n_trees.y + 18))
    d.arrow('land', (n_trees.r, p_trees.my), (p_trees.l, p_trees.my))
    d.to_layer('land', p_trees, b_trees)
    y1 = max(n_trees.b, s_can.b, b_trees.b) + PAD
    d.lane('LAND COVER, TREES & BUILDINGS', y0, y1)

    # ---- 7. rail & ferry service -----------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_rf = d.card('net', 'd1', cy, 'trains a day', 'Py · Rust', ['the coverage’s rail feeds, each', 'fetched once; their trains', 'matched onto the pass’s tracks'],
                  [(['global/railfreq'], 'trains a day per way')], scope='global')
    n_st = d.card('net', 'd1', n_rf.b + 22, 'rail stops · ferries', 'Rust · Py', ['stops near the built areas;', 'ferries worldwide, with', 'their timetables'],
                  [(['layers/stations/ · ferries/'], 'packs · MVT, GeoJSON blocks')], scope='global')
    s_tt = d.src('net', cy, 'Timetables', ['rail: GTFS feeds (Mobility', 'Database, operators’ own);', 'ferries: GTFS and published'],
                 kept=['sources/rail/', 'inputs/ferries/freq/'], minh=n_st.y + 30 - cy)
    s_osm6 = d.src('osm', s_tt.b + 8, 'OSM sets', ['the world’s tracks, stations,', 'routes and ferries'])
    p_rf = d.pill('net', n_rf.y + 30, ['/api/railfreq'], note='trains a day, per way')
    b_rf = d.layer('net', 0, 'Trains a day', ['rail lines coloured and', 'filtered by it'], cy=p_rf.my)
    p_st = d.pill('net', max(n_st.y + 34, b_rf.b + 8 + 22), ['/tiles/stations', '/tiles/ferries'])
    b_st = d.layer('net', 0, 'Ferries · rail stops', ['stops in their line’s colour;', 'sailings a day per route'], cy=p_st.my)
    d.arrow('net', (s_tt.r, n_rf.y + 18), (n_rf.l, n_rf.y + 18))
    d.arrow('net', (s_tt.r, n_st.y + 18), (n_st.l, n_st.y + 18))
    d.arrow('osm', (s_osm6.r, s_osm6.my), (n_st.l, s_osm6.my))
    d.arrow('net', (n_rf.r, p_rf.my), (p_rf.l, p_rf.my))
    d.arrow('net', (n_st.r, p_st.my), (p_st.l, p_st.my))
    d.to_layer('net', p_rf, b_rf)
    d.to_layer('net', p_st, b_st)
    y1 = max(n_st.b, s_osm6.b, b_st.b) + PAD
    d.lane('RAIL & FERRY SERVICE', y0, y1)

    # ---- 8. publishing: rounds, as the regions are done ---------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_rd = d.card('mix', 'd1', cy, 'a round', 'Rust', ['when a region is done, an hour', 'after the last round began, and', 'after the last region: its slope', 'and tree cover, the map tiles', 'near what changed, road index', 'and rail stops, then:'],
                  [(['catalog/<n>.json.zst'], 'every file the map reads')], scope='global')
    s_rd = d.src('mix', cy, 'Regions as they’re done', ['a region at a time: those', 'the map lacks first, then', 'the fewest areas left'], minh=n_rd.h)
    p_cat = d.pill('mix', n_rd.y + 46, ['/api/catalog · meta'], note=['the newest that decodes;', 'every 30 s while in use'])
    b_cat = d.layer('mix', 0, 'New data, in place', ['the page checks each minute;', 'URLs change where data did'], cy=p_cat.my)
    d.arrow('mix', (s_rd.r, n_rd.y + 18), (n_rd.l, n_rd.y + 18))
    d.arrow('mix', (n_rd.r, p_cat.my), (p_cat.l, p_cat.my),
            label='with what the trains’ and the landmarks’ chains made by then', at=(n_rd.r + 30, p_cat.my - 5))
    d.to_layer('mix', p_cat, b_cat)
    y1 = max(n_rd.b, s_rd.b, b_cat.b) + PAD
    d.lane('PUBLISHING · IN ROUNDS', y0, y1)
    h = y1 + 8

    # ---- connectors between rows (clear of the tabs, which sit top right) ---------------------
    xc = n_terr.l + 40   # z12 and z8 terrain → peaks
    d.arrow('terr', (xc, n_terr.t), (xc, n_poi.b), label='terrain z12 · z8 (peaks)', at=(xc + 7, (n_poi.b + n_terr.t) / 2 + 4), cross=True)
    yb, xb = l4_bottom + GAP / 2, n_scen.l + 40   # terrain → scenic (drape heights, the z11 analysis)
    d.arrow('terr', (n_terr.l + 160, n_terr.b), (n_terr.l + 160, yb), (xb, yb), (xb, n_scen.t), label='terrain', at=(n_terr.l + 168, yb - 5), cross=True)
    xd = n_scen.l + 72   # heritage sites and designated areas → the areas' flags (left of the TASK tab)
    yh = y_places + GAP / 2
    d.arrow('place', (n_hs.r, n_hs.b - 12), (n_hs.r + 14, n_hs.b - 12), (n_hs.r + 14, yh), (xd, yh), (xd, n_scen.t),
            label='sites and designated areas', at=(xd + 8, yh + 12), cross=True)
    for k, bx, by, lab, dx in [('land', n_lc.r, n_lc.y + 34, 'grids', 40), ('bldg', n_rb.r, n_rb.y + 34, 'roadside buildings', 84),
                               ('land', s_can.r, s_can.y + 11, 'canopy', 128)]:
        x = n_scen.l + dx
        d.arrow(k, (bx, by), (x, by), (x, n_scen.b), label=lab, at=(COLS['d2'][0] + 8, by - 5), cross=True)

    heads = [(COLS['src'][0], 'SOURCES', 'dashed: kept between runs'),
             (COLS['d1'][0], 'BUILD STEPS → FILES THEY WRITE', 'on the NAS; run by the build Mac (⇄: the M1 too) per area, per z3 pack or worldwide; TASK: any worker; dashed: planned'),
             (COLS['srv'][0], 'SERVER (RUST)', 'Mac’s copy, else the NAS'), (COLS['brw'][0], 'MAP (BROWSER)', None)]
    aria = ('How the map is built. OpenStreetMap comes from one worldwide download, twice a year, cut into pieces per z6 tile, '
            'with worldwide sets, outlines and every way’s whole road. Your 88 regions are only outlines: their union says which '
            'tiles and features get built. Terrain, slope and tree cover are built per z3 pack near the coverage, heritage sites over '
            'the coverage, each area’s roads, elevations and scenic values per area, reading its neighbours within 110 km; the '
            'basemap, labels, landmarks, the heritage chain, area overlays, rail stops, ferries and trains a day worldwide; the '
            'roadside buildings worldwide, once per Overture release. The M1’s helper may take the jobs of terrain, slope, tree '
            'cover, the areas and the landmark candidates and peaks; an area’s last steps, from its elevations on, are tasks any '
            'worker may run, a device’s page too. Rounds publish a catalog as regions are done, about hourly, and after the last. '
            'Planned: the names to-do lists and 3D buildings (bldprep, then 3D tiles from Overture and GHSL heights). The servers '
            'read your translation files directly and attach English to everything they serve, and index the place names for the '
            'search box. The server reads the Mac’s copy of a file if it has it, else the NAS’s.')
    return d.svg(h, aria, heads)
