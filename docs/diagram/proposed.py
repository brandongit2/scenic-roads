"""The proposed pipeline (docs/plan.md v5): OpenStreetMap comes from one worldwide download, cut by area; a region
is only an outline of what to build. Every step runs per area (a z6 tile) or once, worldwide."""
from diag import COLS, Diagram

GAP, PAD = 12, 10


def build(check=False):
    d = Diagram('p', check)

    # ---- 0. OpenStreetMap: the planet, cut by area; your regions are outlines -----------------
    y0 = 60
    cy = y0 + 24
    s_osm = d.src('osm', cy, 'OSM planet', ['the whole world; the NAS', 'fetches it twice a year,', 'resuming if interrupted'],
                  kept=['sources/osm/<date>/'])
    n_pass = d.card('osm', 'd1', cy, 'OSM pass', 'osmium', ['filtered, then cut by area', '(z6 tiles, 10 km buffer);', 'sets kept whole, worldwide'],
                    [(['pieces/<z6>.osm.pbf'], 'PBF · all land ≈ 45 GB'), (['sets/'], 'rail, ferries, areas, places')], scope='global')
    s_reg = d.src('osm', s_osm.b + 8, 'Your regions', ['outlines only: Geofabrik', 'units of any size; their', 'union is the coverage'],
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
    n_base = d.card('base', 'd1', cy, 'basemap', 'Java', ['Planetiler, the whole world,', 'jar and extras pinned'],
                    [(['layers/basemap/'], 'packs · MVT z0–14')], scope='global')
    n_lab = d.card('base', 'd1', n_base.b + 22, 'labels', 'Python', ['ranked worldwide: places and', 'every area’s candidates'],
                   [(['layers/labels/'], 'packs · MVT z0–12')], scope='global')
    s_pl = d.src('osm', cy, 'OSM pass', ['the basemap’s part: water,', 'borders, parks, places'], minh=n_lab.b - cy)
    p_base = d.pill('base', n_base.y + 30, ['/tiles/base'], note=['the pack for the tile;', 'English attached'])
    b_base = d.layer('base', 0, 'Basemap', ['water · borders · parks · rivers'], cy=p_base.my)
    b_coast = d.layer('base', b_base.b + 8, 'Coastal shading', ['distance to shore, from the', 'basemap’s water, in a worker'], computed=True)
    b_lab = d.layer('base', max(n_lab.y + 6, b_coast.b + 8), 'Place labels', ['placed by importance'])
    p_lab = d.pill('base', b_lab.my, ['/tiles/labels'], note='English attached')
    d.arrow('osm', (s_pl.r, n_base.y + 18), (n_base.l, n_base.y + 18))
    d.arrow('osm', (s_pl.r, n_lab.y + 18), (n_lab.l, n_lab.y + 18))
    d.arrow('base', (n_base.r, p_base.my), (p_base.l, p_base.my))
    d.arrow('base', (n_lab.r, p_lab.my), (p_lab.l, p_lab.my))
    d.arrow('base', (p_base.r, b_base.my), (b_base.l, b_base.my))
    d.arrow('base', (p_base.r, p_base.my + 8), (b_coast.l - 12, p_base.my + 8), (b_coast.l - 12, b_coast.my), (b_coast.l, b_coast.my))
    d.to_layer('base', p_lab, b_lab)
    y1 = max(n_lab.b, b_lab.b, s_pl.b) + PAD
    d.lane('MAP CONTEXT', y0, y1)

    # ---- 2. names in English: inventories out, your translations straight to the servers --------
    y0 = y1 + GAP
    cy = y0 + 24
    s_osmn = d.src('osm', cy, 'OSM pieces · sets', ['names, with OSM’s own English', 'and Japanese readings'])
    n_inv = d.card('base', 'd1', cy, 'names', 'Rust · Py', ['every area’s inventory; per', 'reading area, the own-English', 'table and what lacks English'],
                   [(['global/names/'], 'tables per reading area'), (['translations/todo/'], 'a list per reading area')], scope='global')
    y2 = max(s_osmn.b, n_inv.b) + 32
    s_tr = d.src('base', y2, 'Translations', ['your finished files, dropped', 'into the NAS folder'], kept=['translations/<area>/'])
    p_en = d.pill('base', s_tr.my, ['English attached', 'to all it serves'], note=['tiles, landmarks, details …'])
    b_en = d.layer('base', 0, 'English everywhere', ['labels, basemap names, roads,', 'rail lines, stops, popups'], cy=p_en.my)
    d.arrow('osm', (s_osmn.r, n_inv.y + 18), (n_inv.l, n_inv.y + 18))
    xe = p_en.l - 16
    d.arrow('base', (n_inv.r, n_inv.y + 46), (xe, n_inv.y + 46), (xe, p_en.y + 7), (p_en.l, p_en.y + 7),
            label='own-English tables', at=(n_inv.r + 10, n_inv.y + 41))
    yt = p_en.b - 8
    d.arrow('base', (s_tr.r, yt), (p_en.l, yt),
            label='read by both Macs’ servers within a minute: no rebuild, M4 awake or not', at=(n_inv.l + 30, yt - 5))
    yl = (n_inv.b + s_tr.t) / 2   # the translators' loop, outside the pipeline
    d.arrow('base', (n_inv.l + 30, n_inv.b), (n_inv.l + 30, yl), (s_tr.l + 120, yl), (s_tr.l + 120, s_tr.t), dashed=True,
            label='translators', at=(n_inv.l + 38, yl + 4))
    d.to_layer('base', p_en, b_en)
    y1 = max(n_inv.b, s_tr.b, b_en.b) + PAD
    d.lane('NAMES IN ENGLISH', y0, y1)

    # ---- 3. places & heritage ------------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_reg2 = d.src('place', cy, 'Official registers (~25)', ['UNESCO · Parks Canada · NRHP', 'Mérimée · NHLE · 文化財 …'], kept=['sources/registers/'])
    s_osm2 = d.src('osm', s_reg2.b + 8, 'OSM pieces · sets', ['POIs · designated areas'])
    s_wd = d.src('place', s_osm2.b + 8, 'Wikidata · Wikipedia', ['facts; pageviews, one table', 'per season, all languages'])
    n_her = d.card('place', 'd1', cy, 'heritage · details', 'Python',
                   ['registers apply by location;', 'Wikidata facts; peaks’', 'prominence (Rust)'],
                   [(['base/<z6>'], 'in each area’s base pack')], minh=s_wd.b - cy, scope='area')
    n_lay = d.card('place', 'd2', cy, 'landmarks · stops', 'Rust', ['tiles per area; fame and', 'each kind’s top, worldwide'],
                   [(['layers/landmarks/'], 'packs · counts in lo packs')], scope='area')
    n_shr = d.card('place', 'd2', n_lay.b + 22, 'overlays', 'Python', ['each area assembled once,', 'simplified, clipped per pack'],
                   [(['layers/overlays/'], 'packs · MVT')], scope='global')
    p_lay = d.pill('place', n_lay.y + 30, ['/api/landmarks/…', '/api/detail/<osm id>'], note='the pack for the area')
    b_lm = d.layer('place', 0, 'Landmarks', ['dots on the GPU; names tiled', 'in a worker; popups'], cy=p_lay.my)
    p_ov = d.pill('place', n_shr.y + 30, ['/tiles/overlays'])
    b_ov = d.layer('place', 0, 'Area overlays', ['heritage areas, Indigenous', 'lands, World Heritage outlines'], cy=p_ov.my)
    for s in (s_reg2, s_osm2, s_wd):
        d.arrow('osm' if s is s_osm2 else 'place', (s.r, s.my), (n_her.l, s.my))
    d.arrow('place', (n_her.r, n_lay.y + 18), (n_lay.l, n_lay.y + 18))
    d.arrow('place', (n_her.r, n_shr.y + 18), (n_shr.l, n_shr.y + 18))
    d.arrow('place', (n_lay.r, p_lay.my), (p_lay.l, p_lay.my))
    d.arrow('place', (n_shr.r, p_ov.my), (p_ov.l, p_ov.my))
    d.to_layer('place', p_lay, b_lm)
    d.to_layer('place', p_ov, b_ov)
    y1 = max(n_her.b, s_wd.b, b_ov.b, n_shr.b) + PAD
    d.lane('PLACES & HERITAGE', y0, y1)

    # ---- 4. terrain: one global pyramid -------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_aws = d.src('terr', cy, 'AWS Terrain Tiles', ['Terrarium PNG · ~27 m at z12', 'kept raw, never edited'],
                  kept=['sources/aws/ (raw packs)'])
    n_terr = d.card('terr', 'd1', cy, 'terrain', 'Rust', ['z0–8 world; z9–12 in the', 'coverage + 20 km (z11, z10', 'nearer the poles); repaired'],
                    [(['layers/terrain/'], 'packs · Terrarium PNG')], scope='global')
    n_slope = d.card('terr', 'd2', cy + 34, 'slope', 'Rust', ['Horn at z12, kept to z11;', 'each pixel 4 quarter means'],
                     [(['layers/slope/'], 'packs · PNG')], scope='global')
    p_t = d.pill('terr', cy + 12, ['/tiles/terrain'], note='the pack for the tile')
    b_terr = d.layer('terr', 0, '3D terrain · hill-shading', ['elevation tint'], cy=p_t.my)
    b_cont = d.layer('terr', b_terr.b + 8, 'Contour lines', ['traced from terrain tiles'], computed=True)
    b_slope = d.layer('terr', b_cont.b + 8, 'Slope tint', ['colour per quarter, averaged'])
    p_s = d.pill('terr', b_slope.my, ['/tiles/slope'], note='z12 made when asked')
    d.arrow('terr', (s_aws.r, n_terr.y + 18), (n_terr.l, n_terr.y + 18))
    d.arrow('terr', (n_terr.r, n_slope.y + 18), (n_slope.l, n_slope.y + 18))
    d.arrow('terr', (n_terr.r, p_t.my), (p_t.l, p_t.my))
    d.arrow('terr', (n_slope.r, p_s.my), (p_s.l, p_s.my))
    d.to_layer('terr', p_t, b_terr)
    d.arrow('terr', (p_t.r, p_t.my + 8), (b_cont.l - 12, p_t.my + 8), (b_cont.l - 12, b_cont.my), (b_cont.l, b_cont.my))
    d.to_layer('terr', p_s, b_slope)
    y1 = max(n_terr.b, n_slope.b, b_slope.b, s_aws.b) + PAD
    d.lane('TERRAIN', y0, y1)
    l4_bottom = y1

    # ---- 5. roads (per road point, per area) ---------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_ext = d.card('net', 'd1', cy, 'extract', 'Rust', ['the ways each tile owns,', 'inside the coverage'],
                   [(['ways · verts'], 'in base/<z6>.pack'), (['junction pairings'], None)], scope='area')
    n_samp = d.card('terr', 'd2', cy, 'sample.py · tile elev', 'Py · Rust', ['DEMs chosen by location,', 'then clean-up and grade'],
                    [(['elev.f32 · src.u8', 'final.u16 · grade.u8'], 'in the base pack')], kept='its last run is its cache', scope='area')
    n_scen = d.card('scen', 'd3', cy, 'scenic', 'Rust', ['samples every 100 m: horizons', 'to 300 m past trees & buildings,', 'views to 15 km, designations'],
                    [(['scenic.u8'], '13 per road point'), (['samples.* · near.i8', 'vterrain.i16'], 'in the base pack')],
                    kept='its last run is its cache', scope='area')
    n_tile = d.card('net', 'd4', cy, 'roads & rails', 'Rust', ['per z6 pack: base data within', '100 km, worldwide road', 'values; climbs, tiles'],
                    [(['layers/roads/ · rails/'], 'packs · RT z6–14'), (['query parts'], 'by offset along each road')], scope='area')
    s_osm4 = d.src('osm', cy, 'OSM pieces', ['roads · rail lines'])
    s_dem = d.src('terr', s_osm4.b + 8, 'Road DEMs', ['HRDEM · 3DEP · MRDEM (N. Am.)', 'GSI (Japan) · FABDEM 30 m', 'read by range, road blocks only'],
                  minh=n_ext.b + 22 - (s_osm4.b + 8))
    p_road = d.pill('net', n_tile.y + 30, ['/tiles/roads · rails', '/api/road · profile'], note=['the pack for the tile;', 'APIs by OSM id'])
    b_road = d.layer('net', 0, 'Roads & rail lines', ['WebGL, coloured per point:', 'elevation, grade, scenic score'], cy=p_road.my)
    y_sc = max(n_tile.b + 16, b_road.b + 8 + 32.5)
    p_scen = d.pill('scen', y_sc, ['/api/drives · rides', '/api/viewshed'], note=['zoomed out: 1 km', 'summaries'])
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
    l5_bottom = y1

    # ---- 6. land cover, trees & buildings: global layers -----------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_wc = d.src('land', cy, 'ESA WorldCover', ['10 m land cover, 2021'])
    n_lc = d.card('land', 'd1', cy, 'grids', 'Python', ['land cover, canopy, cover at', 'z11, coverage + 20 km'],
                  [(['layers/grids/'], 'packs · z11 rasters')], scope='global')
    y2 = max(s_wc.b, n_lc.b) + 22
    s_bld = d.src('bldg', y2, 'Overture buildings', ['footprints; heights known', 'for 12–58 %'], kept=['sources/overture/'])
    s_off = d.src('bldg', s_bld.b + 8, 'Official 3D buildings', ['PLATEAU in Japan (250+ cities)', 'first; others where needed'],
                  kept=['inputs/manual/ · sources/'])
    n_bld = d.card('bldg', 'd1', y2, 'buildings · later', 'Rust', ['height: official, else tagged,', 'else floors × 3 m, else', 'typical for its kind'],
                   [(['layers/buildings/'], 'packs · MVT z13–14'), (['buildings.f32'], 'boxes & heights, for scenic')],
                   scope='global', minh=s_off.b - y2)
    p_bld = d.pill('bldg', n_bld.y + 64, ['/tiles/buildings'], note='z13–14, overzoomed')
    b_bld = d.layer('bldg', 0, '3D buildings', ['extruded on the terrain', '(MapLibre), zoom 14 and in'], cy=p_bld.my)
    d.arrow('bldg', (s_bld.r, n_bld.y + 18), (n_bld.l, n_bld.y + 18))
    d.arrow('bldg', (s_off.r, s_off.my), (n_bld.l, s_off.my))
    d.arrow('bldg', (n_bld.r, p_bld.my), (p_bld.l, p_bld.my))
    d.to_layer('bldg', p_bld, b_bld)
    s_can = d.src('land', max(s_off.b, n_bld.b) + 12, 'Canopy height · leaf type', ['Meta & WRI (1.2 m imagery)', 'Copernicus HRL · NALCMS'],
                  kept=['sources/canopy/ (10°)'])
    n_trees = d.card('land', 'd1', s_can.y + 30, 'trees', 'Python', ['cover · height · leaf type,', 'coverage + 20 km'],
                     [(['layers/trees/'], 'packs · Terrarium WebP')], scope='global')
    p_trees = d.pill('land', n_trees.my, ['/tiles/trees/{var}'], note='the pack for the tile')
    b_trees = d.layer('land', 0, 'Tree cover · height · leaf', ['colour-relief on the GPU'], cy=p_trees.my)
    d.arrow('land', (s_wc.r, n_lc.y + 18), (n_lc.l, n_lc.y + 18))
    d.arrow('land', (s_can.r, n_trees.y + 18), (n_trees.l, n_trees.y + 18))
    d.arrow('land', (n_trees.r, p_trees.my), (p_trees.l, p_trees.my))
    d.to_layer('land', p_trees, b_trees)
    y1 = max(n_trees.b, s_can.b, b_trees.b) + PAD
    d.lane('LAND COVER, TREES & BUILDINGS', y0, y1)

    # ---- 7. worldwide network steps: whole roads, rail & ferry service -------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_bd = d.src('net', cy, 'Every area’s base data', ['junction pairings and', 'way lengths (~30 B a way)'])
    n_whole = d.card('net', 'd1', cy, 'whole roads', 'Rust', ['pairings joined worldwide', '(union-find): each way’s', 'road, length and offset'],
                     [(['global/roads/'], 'sliced per tile')], scope='global')
    d.arrow('net', (s_bd.r, n_whole.y + 18), (n_whole.l, n_whole.y + 18))
    d.arrow('net', (n_whole.r, n_whole.y + 18), (n_whole.r + 34, n_whole.y + 18),
            label='read by “roads & rails” above: lengths for the length filter, offsets for drives', at=(n_whole.r + 42, n_whole.y + 22))
    y2 = max(s_bd.b, n_whole.b) + 22
    s_tt = d.src('net', y2, 'Timetables', ['GTFS via Mobility Database,', 'operators’ own timetables'], kept=['sources/gtfs/ · inputs/'])
    s_osm6 = d.src('osm', s_tt.b + 8, 'OSM sets', ['the world’s tracks, stations,', 'routes and ferries'])
    n_tr = d.card('net', 'd1', y2, 'train & ferry service', 'Rust · Py', ['trains a day on the world’s', 'track graph; ferries; stations'],
                  [(['global/rail/'], 'trains a day, sliced per tile'), (['ferries · stations'], 'small worldwide layers')],
                  minh=s_osm6.b - y2, scope='global')
    p_tr = d.pill('net', n_tr.y + 40, ['/api/railfreq', '/api/ferries', '/api/stations'])
    b_tr = d.layer('net', 0, 'Ferries · rail stops', ['rail lines by trains a day,', 'sailings a day per route'], cy=p_tr.my)
    d.arrow('net', (s_tt.r, s_tt.y + 24), (n_tr.l, s_tt.y + 24))
    d.arrow('osm', (s_osm6.r, s_osm6.my), (n_tr.l, s_osm6.my))
    d.arrow('net', (n_tr.r, p_tr.my), (p_tr.l, p_tr.my))
    d.to_layer('net', p_tr, b_tr)
    y1 = max(n_tr.b, s_osm6.b, b_tr.b) + PAD
    d.lane('WHOLE ROADS, RAIL & FERRY SERVICE', y0, y1)
    h = y1 + 8

    # ---- connectors between rows (clear of the tabs, which sit top right) ---------------------
    xc = n_terr.l + 40   # terrain → peaks
    d.arrow('terr', (xc, n_terr.t), (xc, n_her.b), label='terrain (peaks)', at=(xc + 7, (n_her.b + n_terr.t) / 2 + 4), cross=True)
    yb, xb = l4_bottom + GAP / 2, n_scen.l + 40   # terrain → scenic (drape heights, the z11 analysis)
    d.arrow('terr', (n_terr.l + 160, n_terr.b), (n_terr.l + 160, yb), (xb, yb), (xb, n_scen.t), label='terrain', at=(n_slope.l + 8, yb - 5), cross=True)
    xd = n_scen.l + 90   # designated areas (the overlays' assembled areas) → scenic flags
    d.arrow('place', (n_shr.r, n_shr.b - 12), (xd, n_shr.b - 12), (xd, n_scen.t), label='designated areas', at=(n_shr.r + 8, n_shr.b - 17), cross=True)
    for k, bx, by, lab, dx in [('land', n_lc.r, n_lc.y + 34, 'grids', 40), ('bldg', n_bld.r, n_bld.y + 22, 'buildings · heights', 84),
                               ('land', s_can.r, s_can.y + 11, 'canopy', 128)]:
        x = n_scen.l + dx
        d.arrow(k, (bx, by), (x, by), (x, n_scen.b), label=lab, at=(COLS['d2'][0] + 8, by - 5), cross=True)

    heads = [(COLS['src'][0], 'SOURCES', 'dashed: kept on the NAS'),
             (COLS['d1'][0], 'BUILD STEPS → FILES THEY WRITE', 'on the NAS; run by the build Mac, per area (z6 tile) or worldwide'),
             (COLS['srv'][0], 'SERVER (RUST)', 'Mac’s copy, else the NAS'), (COLS['brw'][0], 'MAP (BROWSER)', None)]
    aria = ('Proposed data pipeline. OpenStreetMap comes from one worldwide download, twice a year, cut into pieces per z6 tile '
            'plus worldwide sets of rail, ferries, designated areas and places. Your regions are only outlines: their union says '
            'which tiles and features get built. Every step runs per area or once worldwide: terrain, slope, trees, grids, the '
            'basemap, overlays and labels worldwide; each tile’s roads, elevations, scenic values and landmarks per area, reading '
            'its neighbours within 100 km; whole roads, rail and ferry service worldwide, sliced per tile. So region borders and tile '
            'edges don’t show. The servers read your translation files directly and attach English to everything they serve. '
            'The server reads the Mac’s copy of a file if it has it, else the NAS’s.')
    return d.svg(h, aria, heads)
