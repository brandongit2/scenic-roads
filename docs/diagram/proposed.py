"""How the map is built (docs/plan.md v7): OpenStreetMap comes from one worldwide download, cut by area; a region is
only an outline of what to build. Every step runs per area (a z6 tile), per z3 pack near the coverage, or once,
worldwide. Dashed cards are not built yet, or built but off."""
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
    s_reg = d.src('osm', s_osm.b + 8, 'Your regions', ['outlines: administrative areas,', 'Geofabrik units, drawn shapes;', 'their union is the coverage'],
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
    d.arrow('osm', (s_pl.r, n_base.y + 18), (n_base.l, n_base.y + 18))
    d.arrow('osm', (s_pl.r, n_lab.y + 18), (n_lab.l, n_lab.y + 18))
    d.arrow('base', (n_base.r, p_base.my), (p_base.l, p_base.my))
    d.arrow('base', (n_lab.r, p_lab.my), (p_lab.l, p_lab.my))
    d.arrow('base', (p_base.r, b_base.my), (b_base.l, b_base.my))
    d.arrow('base', (p_base.r, p_base.my + 8), (b_coast.l - 12, p_base.my + 8), (b_coast.l - 12, b_coast.my), (b_coast.l, b_coast.my))
    d.to_layer('base', p_lab, b_lab)
    y1 = max(n_lab.b, b_lab.b, s_pl.b) + PAD
    d.lane('MAP CONTEXT', y0, y1)

    # ---- 2. names in English: your translations straight to the servers --------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_osmn = d.src('osm', cy, 'Every area’s names', ['with each thing’s own English:', 'OSM’s, Wikidata’s, Wikipedia’s'])
    n_inv = d.card('base', 'd1', cy, 'names to-do · later', 'Rust', ['what lacks English, by', 'language: a name, its kind,', 'where it is, how known'],
                   [(['translations/todo/'], 'a list per language')], scope='global', later=True)
    y2 = max(s_osmn.b, n_inv.b) + 32
    s_tr = d.src('base', y2, 'Translations', ['your finished files, dropped', 'into the NAS folder'], kept=['translations/'])
    p_en = d.pill('base', s_tr.my, ['English attached', 'to all it serves'], note=['a thing’s own English,', 'else a translation'])
    b_en = d.layer('base', 0, 'English everywhere', ['labels, basemap names, roads,', 'rail lines, stops, popups'], cy=p_en.my)
    d.arrow('osm', (s_osmn.r, n_inv.y + 18), (n_inv.l, n_inv.y + 18))
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
    n_hs = d.card('place', 'd1', cy, 'heritage sites', 'Python', ['today’s heritage.py over the', 'coverage + 20 km, sliced', 'per area'],
                  [(['work/heritage/<date>/'], 'positions · areas per z6')], scope='global')
    n_poi = d.card('place', 'd1', n_hs.b + 22, 'candidates · peaks', 'Rust', ['each area’s points; prominence', 'and isolation over z12 and', 'the worldwide z8'],
                   [(['work/pois/ · work/peaks/'], 'per area')], scope='area')
    n_items = d.card('place', 'd2', n_poi.y, 'facts · pageviews', 'Python', ['the candidates’ Wikidata', 'items, once a pass'],
                     [(['sources/items/<date>/'], 'facts · views')], scope='global')
    n_hc = d.card('place', 'd2', cy, 'heritage chain · off', 'Python', ['today’s chain on the pass:', 'details, outlines, fame'],
                  [(['work/heritage/<date>/'], 'today’s, until switched on')], scope='global', later=True)
    n_mk = d.card('place', 'd3', cy, 'landmarks', 'Rust', ['points per kind: fame, rarity,', 'thinned tiles, per-cell counts'],
                  [(['markdata/<z6> · marks-*/'], 'sectioned · RDMT tiles')], scope='global')
    n_ov = d.card('place', 'd3', n_mk.b + 22, 'area overlays · off', 'Rust', ['from the heritage chain, with', 'the landmarks’ site ids'],
                  [(['layers/ov-*/ · ovdata/'], 'packs · MVT; details per z3')], scope='global', later=True)
    p_mk = d.pill('place', n_mk.y + 30, ['/api/marks/…'], note=['tiles by view; In view,', 'details'])
    b_lm = d.layer('place', 0, 'Landmarks', ['dots on the GPU; names tiled', 'in a worker; popups'], cy=p_mk.my)
    p_ov = d.pill('place', n_ov.y + 30, ['/tiles/ov/…'], note='today’s, converted')
    b_ov = d.layer('place', 0, 'Area overlays', ['heritage areas, Indigenous', 'lands, World Heritage outlines'], cy=p_ov.my)
    d.arrow('place', (s_reg2.r, n_hs.y + 18), (n_hs.l, n_hs.y + 18))
    d.arrow('osm', (s_osm2.r, s_osm2.my), (n_poi.l, s_osm2.my))
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
                  kept=['the build Mac’s cache'])
    n_terr = d.card('terr', 'd1', cy, 'terrain', 'Rust', ['z9–12 within 20 km of the', 'coverage (z11, z10 nearer the', 'poles); z3–8 per pack; repaired'],
                    [(['layers/terrain/'], 'packs · Terrarium PNG')], scope='pack')
    n_slope = d.card('terr', 'd2', cy + 34, 'slope', 'Rust', ['Horn at z12, kept to z11;', 'each pixel 4 quarter means'],
                     [(['layers/slope/'], 'packs · PNG')], scope='pack')
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
                   [(['base/<z6>'], 'sectioned · per road point')], scope='area')
    n_samp = d.card('terr', 'd2', cy, 'sample.py · tile elev', 'Py · Rust', ['DEMs chosen by location,', 'then clean-up and grade'],
                    [(['elevations · grade'], 'in the base pack')], kept='today’s DEM cache, as a seed', scope='area')
    n_scen = d.card('scen', 'd3', cy, 'scenic', 'Rust', ['samples every 100 m: horizons', 'to 300 m past trees, views to', '15 km, buildings, designations'],
                    [(['13 channels a point'], 'in the base pack')], scope='area')
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
    n_lc = d.card('land', 'd1', cy, 'grids', 'Python', ['land cover, canopy, cover at', 'z11, made in each area’s job'],
                  [(['layers/grid-*/'], 'packs · z11, not served')], scope='area')
    y2 = max(s_wc.b, n_lc.b) + 22
    s_bld = d.src('bldg', y2, 'Overture buildings', ['footprints; heights known', 'for 12–58 %'], kept=['sources/legacy/ (today’s)'])
    s_off = d.src('bldg', s_bld.b + 8, 'Official 3D buildings', ['PLATEAU in Japan (250+ cities)', 'first; others where needed'])
    n_bld = d.card('bldg', 'd1', y2, 'buildings · later', 'Rust', ['height: official, else tagged,', 'else floors × 3 m, else', 'typical for its kind'],
                   [(['layers/buildings/'], 'packs · MVT z13–14')], scope='global', minh=s_off.b - y2, later=True)
    p_bld = d.pill('bldg', n_bld.y + 52, ['/tiles/buildings'], note='z13–14, overzoomed')
    b_bld = d.layer('bldg', 0, '3D buildings', ['extruded on the terrain', '(MapLibre), zoom 14 and in'], cy=p_bld.my)
    d.arrow('bldg', (s_bld.r, n_bld.y + 18), (n_bld.l, n_bld.y + 18))
    d.arrow('bldg', (s_off.r, s_off.my), (n_bld.l, s_off.my))
    d.arrow('bldg', (n_bld.r, p_bld.my), (p_bld.l, p_bld.my))
    d.to_layer('bldg', p_bld, b_bld)
    s_can = d.src('land', max(s_off.b, n_bld.b) + 12, 'Canopy height · leaf type', ['Meta & WRI (1.2 m imagery)', 'Copernicus HRL · NALCMS'],
                  kept=['the build Mac (10° files)'])
    n_trees = d.card('land', 'd1', s_can.y + 30, 'trees · later for new areas', 'Python', ['cover · height · leaf type;', 'today’s, converted'],
                     [(['layers/trees-*/'], 'packs · Terrarium WebP')], scope='global', later=True)
    p_trees = d.pill('land', n_trees.my, ['/tiles/trees/{var}'], note='the pack for the tile')
    b_trees = d.layer('land', 0, 'Tree cover · height · leaf', ['colour-relief on the GPU'], cy=p_trees.my)
    d.arrow('land', (s_wc.r, n_lc.y + 18), (n_lc.l, n_lc.y + 18))
    d.arrow('land', (s_can.r, n_trees.y + 18), (n_trees.l, n_trees.y + 18))
    d.arrow('land', (n_trees.r, p_trees.my), (p_trees.l, p_trees.my))
    d.to_layer('land', p_trees, b_trees)
    y1 = max(n_trees.b, s_can.b, b_trees.b) + PAD
    d.lane('LAND COVER, TREES & BUILDINGS', y0, y1)

    # ---- 7. rail & ferry service -----------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_tt = d.src('net', cy, 'Timetables', ['GTFS via Mobility Database,', 'operators’ own timetables'], kept=['inputs/ferries/freq/'])
    s_osm6 = d.src('osm', s_tt.b + 8, 'OSM sets', ['the world’s tracks, stations,', 'routes and ferries'])
    n_st = d.card('net', 'd1', cy, 'rail stops · ferries', 'Rust · Py', ['stops near the built areas;', 'ferries worldwide, with', 'their timetables'],
                  [(['layers/stations/ · ferries/'], 'packs · MVT, GeoJSON blocks')], scope='global')
    n_rf = d.card('net', 'd2', cy, 'trains a day · later', 'Rust · Py', ['on the world’s track graph;', 'today’s, converted'],
                  [(['global/railfreq'], 'trains a day per way')], scope='global', later=True)
    p_tr = d.pill('net', n_st.y + 40, ['/tiles/stations', '/tiles/ferries', '/api/railfreq'])
    b_tr = d.layer('net', 0, 'Ferries · rail stops', ['rail lines by trains a day,', 'sailings a day per route'], cy=p_tr.my)
    d.arrow('net', (s_tt.r, s_tt.y + 24), (n_st.l, s_tt.y + 24))
    d.arrow('osm', (s_osm6.r, s_osm6.my), (n_st.l, s_osm6.my))
    d.arrow('net', (n_st.r, n_st.y + 18), (n_rf.l, n_st.y + 18))
    d.arrow('net', (n_rf.r, p_tr.my), (p_tr.l, p_tr.my))
    d.to_layer('net', p_tr, b_tr)
    y1 = max(n_st.b, n_rf.b, s_osm6.b, b_tr.b) + PAD
    d.lane('RAIL & FERRY SERVICE', y0, y1)
    h = y1 + 8

    # ---- connectors between rows (clear of the tabs, which sit top right) ---------------------
    xc = n_terr.l + 40   # z12 and z8 terrain → peaks
    d.arrow('terr', (xc, n_terr.t), (xc, n_poi.b), label='terrain z12 · z8 (peaks)', at=(xc + 7, (n_poi.b + n_terr.t) / 2 + 4), cross=True)
    yb, xb = l4_bottom + GAP / 2, n_scen.l + 40   # terrain → scenic (drape heights, the z11 analysis)
    d.arrow('terr', (n_terr.l + 160, n_terr.b), (n_terr.l + 160, yb), (xb, yb), (xb, n_scen.t), label='terrain', at=(n_terr.l + 168, yb - 5), cross=True)
    xd = n_scen.l + 90   # heritage sites and designated areas → the areas' flags
    yh = y_places + GAP / 2
    d.arrow('place', (n_hs.r, n_hs.b - 12), (n_hs.r + 14, n_hs.b - 12), (n_hs.r + 14, yh), (xd, yh), (xd, n_scen.t),
            label='sites and designated areas', at=(xd + 8, yh + 12), cross=True)
    for k, bx, by, lab, dx in [('land', n_lc.r, n_lc.y + 34, 'grids', 40), ('land', s_can.r, s_can.y + 11, 'canopy', 128)]:
        x = n_scen.l + dx
        d.arrow(k, (bx, by), (x, by), (x, n_scen.b), label=lab, at=(COLS['d2'][0] + 8, by - 5), cross=True)
    yg, xr = s_bld.t - 11, n_scen.l + 84   # today's Overture boxes → roadside buildings
    d.arrow('bldg', (s_bld.l + 150, s_bld.t), (s_bld.l + 150, yg), (xr, yg), (xr, n_scen.b), label='roadside buildings',
            at=(COLS['d2'][0] + 8, yg - 5), cross=True)

    heads = [(COLS['src'][0], 'SOURCES', 'dashed: kept between runs'),
             (COLS['d1'][0], 'BUILD STEPS → FILES THEY WRITE', 'on the NAS; run by the build Mac per area, per z3 pack or worldwide; dashed: later or off'),
             (COLS['srv'][0], 'SERVER (RUST)', 'Mac’s copy, else the NAS'), (COLS['brw'][0], 'MAP (BROWSER)', None)]
    aria = ('How the map is built. OpenStreetMap comes from one worldwide download, twice a year, cut into pieces per z6 tile, '
            'with worldwide sets, outlines and every way’s whole road. Your regions are only outlines: their union says which '
            'tiles and features get built. Terrain and slope are built per z3 pack near the coverage, heritage sites over the '
            'coverage, each area’s roads, elevations and scenic values per area, reading its neighbours within 110 km; the '
            'basemap, labels, landmarks, rail stops and ferries worldwide. Not built yet: the names to-do list, trees for new '
            'areas, trains a day, buildings; built but off: the heritage chain and the area overlays it makes. The servers read '
            'your translation files directly and attach English to everything they serve. The server reads the Mac’s copy of a '
            'file if it has it, else the NAS’s.')
    return d.svg(h, aria, heads)
