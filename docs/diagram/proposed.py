"""The proposed pipeline (docs/plan.md v3): global layers in packs, regions per country from Geofabrik,
shared labels and overlays, English baked in by cheap per-region steps."""
from diag import COLS, Diagram

GAP, PAD = 12, 10


def build(check=False):
    d = Diagram('p', check)

    # ---- 0. OpenStreetMap: one Geofabrik extract per region -----------------------------------
    y0 = 60
    cy = y0 + 24
    s_osm = d.src('osm', cy, 'OpenStreetMap', ['Geofabrik: an extract per', 'country, refreshed every', '~3 months'],
                  kept=['sources/geofabrik/'])
    n_fetch = d.card('osm', 'd1', cy, 'fetch', 'Python', ['the country’s extract, clipped', 'where Geofabrik bundles', 'places (Singapore)'],
                     [(['osm.pbf'], 'PBF · 11 regions ≈ 21 GB')], scope='region')
    d.arrow('osm', (s_osm.r, n_fetch.y + 18), (n_fetch.l, n_fetch.y + 18))
    d.arrow('osm', (n_fetch.r, n_fetch.y + 18), (n_fetch.r + 34, n_fetch.y + 18), label='read by the rows below as “OSM extract”',
            at=(n_fetch.r + 42, n_fetch.y + 22))
    y1 = max(s_osm.b, n_fetch.b) + PAD
    d.lane('OPENSTREETMAP', y0, y1)

    # ---- 1. map context: one worldwide basemap, shared labels ----------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_pl = d.src('osm', cy, 'OSM planet', ['once a year; it only passes', 'through the build Mac'])
    n_base = d.card('base', 'd1', cy, 'basemap', 'osmium · Java', ['planet filtered to water,', 'borders, parks, places;', 'Planetiler, the whole world'],
                    [(['global/basemap/'], 'packs · MVT z0–14')], scope='global')
    n_lab = d.card('base', 'd1', n_base.b + 22, 'labels.py', 'Python', ['every region’s labels, and', 'world places outside them'],
                   [(['shared/labels/'], 'packs · MVT z0–12')], scope='shared')
    p_base = d.pill('base', n_base.y + 30, ['/tiles/base'], note='the pack for the tile')
    b_base = d.layer('base', 0, 'Basemap', ['water · borders · parks · rivers'], cy=p_base.my)
    b_coast = d.layer('base', b_base.b + 8, 'Coastal shading', ['distance to shore, from the', 'basemap’s water, in a worker'], computed=True)
    b_lab = d.layer('base', max(n_lab.y + 6, b_coast.b + 8), 'Place labels', ['placed by importance'])
    p_lab = d.pill('base', b_lab.my, ['/tiles/labels'])
    d.arrow('osm', (s_pl.r, n_base.y + 18), (n_base.l, n_base.y + 18))
    d.arrow('base', (n_base.l + 60, n_base.b), (n_base.l + 60, n_lab.t), label='world places', at=(n_base.l + 67, (n_base.b + n_lab.t) / 2 + 4))
    d.arrow('base', (n_base.r, p_base.my), (p_base.l, p_base.my))
    d.arrow('base', (n_lab.r, p_lab.my), (p_lab.l, p_lab.my))
    d.arrow('base', (p_base.r, b_base.my), (b_base.l, b_base.my))
    d.arrow('base', (p_base.r, p_base.my + 8), (b_coast.l - 12, p_base.my + 8), (b_coast.l - 12, b_coast.my), (b_coast.l, b_coast.my))
    d.to_layer('base', p_lab, b_lab)
    y1 = max(n_lab.b, b_lab.b, s_pl.b) + PAD
    d.lane('MAP CONTEXT', y0, y1)
    l1_bottom = y1

    # ---- 2. names in English: each region's inventory out, translations in, baked by cheap steps
    y0 = y1 + GAP
    cy = y0 + 24
    s_osmn = d.src('osm', cy, 'OSM extract', ['names, with OSM’s own English', 'and Japanese readings'])
    n_inv = d.card('base', 'd1', cy, 'names', 'Python', ['every named thing the map', 'shows; what lacks English'],
                   [(['names.jsonl'], 'inventory'), (['translations/todo/<id>.jsonl'], None)], scope='region')
    y2 = max(s_osmn.b, n_inv.b) + 32
    s_tr = d.src('base', y2, 'Translations', ['your finished files, dropped', 'into the NAS folder'], kept=['translations/<scope>.jsonl'])
    n_tab = d.card('base', 'd1', y2, 'English', 'Python', ['own, else the translation;', 'reruns within minutes of a', 'drop, no region rebuild'],
                   [(['names-en.json'], 'baked into every named file')], scope='region')
    p_en = d.pill('base', n_tab.y + 34, ['/api/road · lines', '/api/stations …'], note='names with English')
    b_en = d.layer('base', 0, 'English in the app’s text', ['roads, rail lines, ferries,', 'stops, landmarks'], cy=p_en.my)
    d.arrow('osm', (s_osmn.r, n_inv.y + 18), (n_inv.l, n_inv.y + 18))
    d.arrow('base', (s_tr.r, n_tab.y + 18), (n_tab.l, n_tab.y + 18))
    yl = (n_inv.b + s_tr.t) / 2   # the translators' loop, outside the pipeline
    d.arrow('base', (n_inv.l + 30, n_inv.b), (n_inv.l + 30, yl), (s_tr.l + 120, yl), (s_tr.l + 120, s_tr.t), dashed=True,
            label='translators', at=(n_inv.l + 38, yl + 4))
    d.arrow('base', (n_tab.r, p_en.my), (p_en.l, p_en.my))
    d.to_layer('base', p_en, b_en)
    y1 = max(n_tab.b, s_tr.b, b_en.b) + PAD
    d.lane('NAMES IN ENGLISH', y0, y1)

    # ---- 3. places & heritage ------------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_reg = d.src('place', cy, 'Official registers (~25)', ['UNESCO · Parks Canada · NRHP', 'Mérimée · NHLE · 文化財 …'], kept=['sources/registers/'])
    s_osm2 = d.src('osm', s_reg.b + 8, 'OSM extract', ['POIs · protected areas'])
    s_wd = d.src('place', s_osm2.b + 8, 'Wikidata · Wikipedia', ['facts, pageviews; written', 'descriptions (drop-ins)'])
    n_her = d.card('place', 'd1', cy, 'heritage · details', 'Python',
                   ['registers apply by location;', 'Wikidata facts, descriptions;', 'peaks’ prominence (Rust)'],
                   [(['heritage.json · peaks.json', 'details-*.jsonl'], 'GeoJSON · JSONL'), (['grid.areas.u8'], 'designated areas')],
                   minh=s_wd.b - cy, scope='region')
    n_lay = d.card('place', 'd2', cy, 'landmarks', 'Python', ['fame from pageviews, rarity', 'nearby; a file per kind'],
                   [(['landmarks/<kind>.json'], 'GeoJSON · English baked in')], scope='region')
    n_shr = d.card('place', 'd3', cy, 'overlays · top', 'Python', ['area outlines tiled; the', 'best known for zoomed out'],
                   [(['shared/overlays/'], 'packs · MVT'), (['shared/top/<kind>.json'], None)], scope='shared')
    p_lay = d.pill('place', n_lay.y + 30, ['/api/landmarks/…', '/api/detail/<gid>'], note='top file below z7')
    b_lm = d.layer('place', 0, 'Landmarks', ['dots on the GPU; names tiled', 'in a worker; popups'], cy=p_lay.my)
    p_ov = d.pill('place', b_lm.b + 30, ['/tiles/overlays'])
    b_ov = d.layer('place', 0, 'Area overlays', ['heritage areas, Indigenous', 'lands, World Heritage outlines'], cy=p_ov.my)
    for s in (s_reg, s_osm2, s_wd):
        d.arrow('osm' if s is s_osm2 else 'place', (s.r, s.my), (n_her.l, s.my))
    d.arrow('place', (n_her.r, n_lay.y + 18), (n_lay.l, n_lay.y + 18))
    d.arrow('place', (n_lay.r, n_shr.y + 18), (n_shr.l, n_shr.y + 18))
    d.arrow('place', (n_shr.r, p_lay.my - 6), (p_lay.l, p_lay.my - 6))
    d.arrow('place', (n_shr.r, n_shr.b - 12), (p_ov.l - 14, n_shr.b - 12), (p_ov.l - 14, p_ov.my), (p_ov.l, p_ov.my))
    d.to_layer('place', p_lay, b_lm)
    d.to_layer('place', p_ov, b_ov)
    y1 = max(n_her.b, s_wd.b, b_ov.b, n_shr.b + 22) + PAD
    d.lane('PLACES & HERITAGE', y0, y1)

    # ---- 4. terrain: one global pyramid -------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_aws = d.src('terr', cy, 'AWS Terrain Tiles', ['Terrarium PNG · ~27 m at z12', 'kept raw, never edited'],
                  kept=['sources/aws/ (raw packs)'])
    n_terr = d.card('terr', 'd1', cy, 'terrain', 'Rust', ['z0–9 world; z10–12 inside', 'outlines (z11, z10 nearer', 'the poles); repaired once'],
                    [(['global/terrain/'], 'packs · Terrarium PNG')], scope='global')
    n_slope = d.card('terr', 'd2', cy + 34, 'slope', 'Rust', ['Horn at the finest level;', 'each pixel 4 quarter means'],
                     [(['global/slope/'], 'packs · PNG')], scope='global')
    p_t = d.pill('terr', cy + 12, ['/tiles/terrain'], note='the pack for the tile')
    b_terr = d.layer('terr', 0, '3D terrain · hill-shading', ['elevation tint'], cy=p_t.my)
    b_cont = d.layer('terr', b_terr.b + 8, 'Contour lines', ['traced from terrain tiles'], computed=True)
    b_slope = d.layer('terr', b_cont.b + 8, 'Slope tint', ['colour per quarter, averaged'])
    p_s = d.pill('terr', b_slope.my, ['/tiles/slope'], note='the pack for the tile')
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

    # ---- 5. roads (per road point) -------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_ext = d.card('net', 'd1', cy, 'extract', 'Rust', ['the whole extract as context;', 'writes the ways it owns'],
                   [(['ways.bin · verts.bin'], 'flat arrays · gids'), (['pois.json'], None)], scope='region')
    n_samp = d.card('terr', 'd2', cy, 'sample.py · tile elev', 'Py · Rust', ['DEMs chosen by location,', 'then clean-up and grade'],
                    [(['elev.f32 · src.u8', 'final.i16 · grade.u8'], 'flat arrays')], kept='its own DEM cache', scope='region')
    n_scen = d.card('scen', 'd3', cy, 'scenic', 'Rust', ['samples every 100 m: horizons', 'to 300 m past trees & buildings,', 'views to 15 km, designations'],
                    [(['scenic.u8'], '13 per road point'), (['samples.* · near.i8', 'vterrain.i16'], 'query & build files')],
                    kept='its own scenic cache', scope='region')
    n_tile = d.card('net', 'd4', cy, 'tile', 'Rust', ['z4–14 in chunks, packed by z6;', 'startup indexes'],
                    [(['roads/ · rails/'], 'RT v7 packs'), (['startup.*'], 'endpoints, drives, rail lines')], scope='region')
    s_osm4 = d.src('osm', cy, 'OSM extract', ['roads · rail · ferry lines'])
    s_dem = d.src('terr', s_osm4.b + 8, 'Road DEMs', ['HRDEM · 3DEP · MRDEM (N. Am.)', 'GSI (Japan) · FABDEM 30 m', 'read by range, road blocks only'])
    p_road = d.pill('net', n_tile.my, ['/tiles/roads · rails', '/api/road · drives …'], note=['every region’s chunks', 'appended; by gid'])
    b_road = d.layer('net', 0, 'Roads & rail lines', ['WebGL, coloured per point:', 'elevation, grade, scenic score'], cy=p_road.my)
    y_sc = max(n_tile.b + 16, b_road.b + 8 + 26.5)
    p_scen = d.pill('scen', y_sc, ['/api/drives · rides', '/api/viewshed'])
    b_scen = d.layer('scen', 0, 'Drives · rides · viewshed', ['joined across borders by', 'shared endpoints'], cy=p_scen.my)
    ys = cy + 18
    d.arrow('osm', (s_osm4.r, ys), (n_ext.l, ys))
    d.arrow('net', (n_ext.r, ys), (n_samp.l, ys))
    d.arrow('terr', (n_samp.r, ys), (n_scen.l, ys))
    d.arrow('scen', (n_scen.r, ys), (n_tile.l, ys))
    y_dem = max(n_ext.b + 10, s_dem.y + 14)
    d.arrow('terr', (s_dem.r, y_dem), (n_samp.l, y_dem))
    d.arrow('net', (n_tile.r, p_road.my), (p_road.l, p_road.my))
    d.arrow('scen', (n_scen.r, y_sc), (p_scen.l, y_sc))
    d.to_layer('net', p_road, b_road)
    d.to_layer('scen', p_scen, b_scen)
    y1 = max(n_scen.b, n_samp.b, s_dem.b, b_scen.b) + PAD
    d.lane('ROADS · PER ROAD POINT', y0, y1)

    # ---- 6. land cover, trees & buildings: global layers -----------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_wc = d.src('land', cy, 'ESA WorldCover', ['10 m land cover, 2021'])
    n_lc = d.card('land', 'd1', cy, 'grids', 'Python', ['land cover, canopy, cover at', 'z11, near every region’s roads'],
                  [(['global/grids/'], 'packs · z11 rasters')], scope='global')
    y2 = max(s_wc.b, n_lc.b) + 22
    s_bld = d.src('bldg', y2, 'Overture buildings', ['footprints; heights known', 'for 12–58 %'], kept=['sources/overture/'])
    s_off = d.src('bldg', s_bld.b + 8, 'Official 3D buildings', ['PLATEAU in Japan (250+ cities)', 'first; others where needed'],
                  kept=['inputs/manual/ · sources/'])
    n_bld = d.card('bldg', 'd1', y2, 'buildings · later', 'Rust', ['height: official, else tagged,', 'else floors × 3 m, else', 'typical for its kind'],
                   [(['global/buildings/'], 'packs · MVT z13–14'), (['buildings.f32'], 'boxes & heights, for scenic')],
                   scope='global', minh=s_off.b - y2)
    p_bld = d.pill('bldg', n_bld.y + 64, ['/tiles/buildings'], note='z13–14, overzoomed')
    b_bld = d.layer('bldg', 0, '3D buildings', ['extruded on the terrain', '(MapLibre), zoom 14 and in'], cy=p_bld.my)
    d.arrow('bldg', (s_bld.r, n_bld.y + 18), (n_bld.l, n_bld.y + 18))
    d.arrow('bldg', (s_off.r, s_off.my), (n_bld.l, s_off.my))
    d.arrow('bldg', (n_bld.r, p_bld.my), (p_bld.l, p_bld.my))
    d.to_layer('bldg', p_bld, b_bld)
    s_can = d.src('land', max(s_off.b, n_bld.b) + 12, 'Canopy height · leaf type', ['Meta & WRI (1.2 m imagery)', 'Copernicus HRL · NALCMS'],
                  kept=['sources/canopy/ (10°)'])
    n_trees = d.card('land', 'd1', s_can.y + 30, 'trees', 'Python', ['cover · height · leaf type', 'inside outlines'],
                     [(['global/trees/'], 'packs · Terrarium WebP')], scope='global')
    p_trees = d.pill('land', n_trees.my, ['/tiles/trees/{var}'], note='the pack for the tile')
    b_trees = d.layer('land', 0, 'Tree cover · height · leaf', ['colour-relief on the GPU'], cy=p_trees.my)
    d.arrow('land', (s_wc.r, n_lc.y + 18), (n_lc.l, n_lc.y + 18))
    d.arrow('land', (s_can.r, n_trees.y + 18), (n_trees.l, n_trees.y + 18))
    d.arrow('land', (n_trees.r, p_trees.my), (p_trees.l, p_trees.my))
    d.to_layer('land', p_trees, b_trees)
    y1 = max(n_trees.b, s_can.b, b_trees.b) + PAD
    d.lane('LAND COVER, TREES & BUILDINGS', y0, y1)

    # ---- 7. rail & ferry service -------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_tt = d.src('net', cy, 'Timetables', ['GTFS via Mobility Database,', 'operators’ own timetables'], kept=['sources/gtfs/ · inputs/'])
    s_osm6 = d.src('osm', s_tt.b + 8, 'OSM extract', ['ferry routes · rail stops'])
    n_tr = d.card('net', 'd1', cy, 'train & ferry service', 'Rust · Py', ['railfreq · gtfs · ferries · stations', 'feeds apply by location'],
                  [(['rail-freq.bin'], 'trains a day per rail way'), (['ferries · stations'], 'per kind, English baked in')],
                  minh=s_osm6.b - cy, scope='region')
    p_tr = d.pill('net', n_tr.y + 40, ['/api/railfreq', '/api/ferries', '/api/stations'])
    b_tr = d.layer('net', 0, 'Ferries · rail stops', ['rail lines by trains a day,', 'sailings a day per route'], cy=p_tr.my)
    d.arrow('net', (s_tt.r, s_tt.y + 24), (n_tr.l, s_tt.y + 24))
    d.arrow('osm', (s_osm6.r, s_osm6.my), (n_tr.l, s_osm6.my))
    d.arrow('net', (n_tr.r, p_tr.my), (p_tr.l, p_tr.my))
    d.to_layer('net', p_tr, b_tr)
    y1 = max(n_tr.b, s_osm6.b, b_tr.b) + PAD
    d.lane('RAIL & FERRY SERVICE', y0, y1)
    h = y1 + 8

    # ---- connectors between rows (clear of the tabs, which sit top right) ---------------------
    xc = n_terr.l + 40   # terrain → peaks
    d.arrow('terr', (xc, n_terr.t), (xc, n_her.b), label='terrain (peaks)', at=(xc + 7, (n_her.b + n_terr.t) / 2 + 4), cross=True)
    yb, xb = l4_bottom + GAP / 2, n_scen.l + 40   # terrain → scenic (drape heights, the z11 analysis)
    d.arrow('terr', (n_terr.l + 160, n_terr.b), (n_terr.l + 160, yb), (xb, yb), (xb, n_scen.t), label='terrain', at=(n_slope.l + 8, yb - 5), cross=True)
    yd, xd = max(n_lay.b, n_shr.b) + 14, n_scen.l + 90   # designated areas → scenic flags (under the overlays card)
    d.arrow('place', (n_her.r, yd), (xd, yd), (xd, n_scen.t), label='designated areas', at=(n_lay.r + 8, yd - 5), cross=True)
    for k, bx, by, lab, dx in [('land', n_lc.r, n_lc.y + 34, 'grids', 40), ('bldg', n_bld.r, n_bld.y + 22, 'buildings · heights', 84),
                               ('land', s_can.r, s_can.y + 11, 'canopy', 128)]:
        x = n_scen.l + dx
        d.arrow(k, (bx, by), (x, by), (x, n_scen.b), label=lab, at=(COLS['d2'][0] + 8, by - 5), cross=True)
    xu, yu = n_tab.r + 15, l1_bottom + GAP / 2   # English → labels (up) and landmarks (down)
    d.arrow('base', (n_tab.r, n_tab.y + 14), (xu, n_tab.y + 14), (xu, yu), (n_lab.r - 22, yu), (n_lab.r - 22, n_lab.b), cross=True,
            label='English', at=(xu + 6, n_inv.y + 30))
    xw = n_lay.l + 34
    d.arrow('base', (n_tab.r, n_tab.b - 14), (xw, n_tab.b - 14), (xw, n_lay.t), cross=True, label='English', at=(xw + 6, n_lay.t - 16))

    heads = [(COLS['src'][0], 'SOURCES', 'dashed: kept on the NAS'),
             (COLS['d1'][0], 'BUILD STEPS → FILES THEY WRITE', 'on the NAS: global/, regions/<id>/ or shared/; run by the build Mac'),
             (COLS['srv'][0], 'SERVER (RUST)', 'Mac’s copy, else the NAS'), (COLS['brw'][0], 'MAP (BROWSER)', None)]
    aria = ('Proposed data pipeline. Each region is a country, fetched as a Geofabrik extract. Terrain, slope, trees, the analysis grids, '
            'the basemap and later 3D buildings are global layers: one worldwide tile pyramid each, stored in packs and grown as regions '
            'need them; the basemap comes from the OSM planet once a year. Roads, rail, places, names and transit are built per region, '
            'computing over the whole extract and writing the features each region owns. Labels, area overlays and zoomed-out landmark '
            'files are shared, built from all regions. English names come from your translation files, baked in by cheap per-region '
            'steps that rerun within minutes of a drop. The server reads the Mac’s copy of a file if it has it, else the NAS’s.')
    return d.svg(h, aria, heads)
