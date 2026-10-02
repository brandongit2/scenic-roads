"""The pipeline as the app builds it today (published as version 2; kept as the 'Today' view)."""
from diag import COLS, Diagram

GAP, PAD = 12, 10


def build(check=False):
    d = Diagram('c', check)

    # ---- 1. map context ------------------------------------------------------------------------
    y0 = 52
    cy = y0 + 24
    n_base = d.card('base', 'd1', cy, 'Planetiler', 'Java', ['water · borders · places · parks'],
                    [(['base.pmtiles', 'base-parts/<region>.pmtiles'], 'PMTiles · MVT z0–14 · 9.5 GB')])
    n_lab = d.card('base', 'd1', n_base.b + 14, 'names.py → labels.py', 'Python', ['names ranked by importance'],
                   [(['labels.tiles'], 'archive · MVT z0–12 · 0.2 GB')], kept='data/names · 7.8 GB')
    s_osm1 = d.src('osm', cy, 'OpenStreetMap', ['Geofabrik extracts (Overpass', 'where none) per region'],
                   kept=['data/osm/merged.osm.pbf', '21 GB'], minh=n_lab.b - cy)
    p_base = d.pill('base', n_base.y + 34, ['/tiles/base.pmtiles', '/tiles/base-parts/'])
    b_base = d.layer('base', 0, 'Basemap', ['water · borders · parks · rivers'], cy=p_base.my)
    b_coast = d.layer('base', b_base.b + 8, 'Coastal shading', ['distance to shore, from the', 'basemap’s water, in a worker'], computed=True)
    b_lab = d.layer('base', max(n_lab.y + 6, b_coast.b + 8), 'Place labels', ['placed by importance'])
    p_lab = d.pill('base', b_lab.my, ['/tiles/labels'])
    d.arrow('osm', (s_osm1.r, n_base.y + 18), (n_base.l, n_base.y + 18))
    d.arrow('osm', (s_osm1.r, n_lab.y + 18), (n_lab.l, n_lab.y + 18))
    d.arrow('base', (n_base.r, p_base.my), (p_base.l, p_base.my))
    d.arrow('base', (n_lab.r, p_lab.my), (p_lab.l, p_lab.my))
    d.arrow('base', (p_base.r, b_base.my), (b_base.l, b_base.my))
    d.arrow('base', (p_base.r, p_base.my + 8), (b_coast.l - 12, p_base.my + 8), (b_coast.l - 12, b_coast.my), (b_coast.l, b_coast.my))
    d.to_layer('base', p_lab, b_lab)
    y1 = max(n_lab.b, b_lab.b, s_osm1.b) + PAD
    d.lane('MAP CONTEXT', y0, y1)

    # ---- 2. places & heritage ------------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_reg = d.src('place', cy, 'Official registers (~25)', ['UNESCO · Parks Canada · NRHP', 'Mérimée · NHLE · 文化財 …'], kept=['data/heritage · 4 GB'])
    s_osm2 = d.src('osm', s_reg.b + 8, 'OpenStreetMap', ['POIs · protected areas'])
    s_wd = d.src('place', s_osm2.b + 8, 'Wikidata · Wikipedia', ['facts · pageview dumps', '+ descriptions by Claude'])
    n_her = d.card('place', 'd1', cy, 'heritage · details', 'Python',
                   ['designations & their areas;', 'Wikidata facts, descriptions;', 'peaks’ prominence (Rust)'],
                   [(['heritage.json · peaks.json', 'details-*.jsonl'], 'GeoJSON · JSONL · 0.3 GB'), (['grid.areas.u8'], 'z11 grid · 3.2 GB')],
                   minh=s_wd.b - cy)
    n_lay = d.card('place', 'd2', cy, 'interest · layers.py', 'Python', ['fame from pageviews, rarity', 'nearby; lean layers per kind'],
                   [(['layer-*.json'], 'GeoJSON · best known last')])
    p_lay = d.pill('place', n_lay.y + 32, ['/api/layer/{name}', '/api/detail · park'])
    b_lm = d.layer('place', 0, 'Landmarks', ['dots on the GPU; names tiled', 'in a worker; popups'], cy=p_lay.my)
    for s in (s_reg, s_osm2, s_wd):
        d.arrow('osm' if s is s_osm2 else 'place', (s.r, s.my), (n_her.l, s.my))
    d.arrow('place', (n_her.r, n_lay.y + 18), (n_lay.l, n_lay.y + 18))
    d.arrow('place', (n_lay.r, p_lay.my), (p_lay.l, p_lay.my))
    d.to_layer('place', p_lay, b_lm)
    y1 = max(n_her.b, s_wd.b, b_lm.b) + PAD
    d.lane('PLACES & HERITAGE', y0, y1)

    # ---- 3. terrain ----------------------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_aws = d.src('terr', cy, 'AWS Terrain Tiles', ['Terrarium PNG · ~27 m at z12', 'z0–8: one box round all roads', 'z9–12: on or beside roads'])
    n_terr = d.card('terr', 'd1', cy, 'terrain', 'Rust', ['repairs voids & spikes;', 'z0–8 remade from finer tiles'],
                    [(['terrain.tiles'], 'Terrarium PNG archive · 13 GB'), (['grid.idx · grid.terrain.i16'], 'z11 grid · 6.4 GB')],
                    kept='reuses last terrain.tiles')
    n_slope = d.card('terr', 'd2', cy + 26, 'slope', 'Rust', ['Horn’s method at z12; each', 'pixel keeps 4 quarter means'],
                     [(['slope.tiles'], 'PNG archive · 21 GB')])
    p_t = d.pill('terr', cy + 12, ['/tiles/terrain'])
    b_terr = d.layer('terr', 0, '3D terrain · hill-shading', ['elevation tint'], cy=p_t.my)
    b_cont = d.layer('terr', b_terr.b + 8, 'Contour lines', ['traced from terrain tiles'], computed=True)
    b_slope = d.layer('terr', b_cont.b + 8, 'Slope tint', ['colour per quarter, averaged'])
    p_s = d.pill('terr', b_slope.my, ['/tiles/slope'])
    d.arrow('terr', (s_aws.r, n_terr.y + 18), (n_terr.l, n_terr.y + 18))
    d.arrow('terr', (n_terr.r, n_slope.y + 18), (n_slope.l, n_slope.y + 18))
    d.arrow('terr', (n_terr.r, p_t.my), (p_t.l, p_t.my))
    d.arrow('terr', (n_slope.r, p_s.my), (p_s.l, p_s.my))
    d.to_layer('terr', p_t, b_terr)
    d.arrow('terr', (p_t.r, p_t.my + 6), (b_cont.l - 12, p_t.my + 6), (b_cont.l - 12, b_cont.my), (b_cont.l, b_cont.my))
    d.to_layer('terr', p_s, b_slope)
    y1 = max(n_terr.b, n_slope.b, b_slope.b, s_aws.b) + PAD
    d.lane('TERRAIN', y0, y1)
    l3_bottom = y1

    # ---- 4. roads (per road point) -------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_ext = d.card('net', 'd1', cy, 'extract', 'Rust', ['728 M points, one every 8 m'],
                   [(['ways.bin · verts.bin'], 'flat arrays · 7 GB'), (['pois.json'], None)])
    n_samp = d.card('terr', 'd2', cy, 'sample.py · tile elev', 'Py · Rust', ['DEM height at each point,', 'then clean-up and grade'],
                    [(['elev.f32 · src.u8', 'final.i16 · grade.u8'], 'flat arrays · 5.8 GB')], kept='data/cache/dem-cache 9.5 GB')
    n_scen = d.card('scen', 'd3', cy, 'scenic', 'Rust', ['samples every 100 m: horizons', 'to 300 m, views to 15 km,', 'buildings, designation flags'],
                    [(['scenic.u8'], '13 per road point · 9.5 GB'), (['samples.* · near.i8', 'vterrain.i16'], 'flat arrays · 7.3 GB'),
                     (['grid.{canopy,cover}.u8'], 'z11 grids · 6.4 GB')], kept='data/cache/scenic · 1.1 GB')
    n_tile = d.card('net', 'd4', cy, 'tile', 'Rust', ['simplify · climbs · z4–14'],
                    [(['roads.tiles', 'rails.tiles · climbs.*'], 'archive · RT v6 · 8.6 GB')])
    s_osm4 = d.src('osm', cy, 'OpenStreetMap', ['roads · rail · ferry lines'])
    s_dem = d.src('terr', s_osm4.b + 8, 'Road DEMs', ['HRDEM · 3DEP · MRDEM (N. Am.)', 'GSI (Japan) · FABDEM 30 m', 'read by range, road blocks only'])
    p_road = d.pill('net', n_tile.my, ['/tiles/roads · rails', '/api/road · climbs …'])
    b_road = d.layer('net', 0, 'Roads & rail lines', ['WebGL, coloured per point:', 'elevation, grade, scenic score'], cy=p_road.my)
    y_sc = max(n_tile.b + 16, b_road.b + 8 + 26.5)
    p_scen = d.pill('scen', y_sc, ['/api/drives · rides', '/api/viewshed'])
    b_scen = d.layer('scen', 0, 'Drives · rides · viewshed', ['best stretches in view; what', 'you can see from a point'], cy=p_scen.my)
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

    # ---- 5. land cover & trees -----------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_wc = d.src('land', cy, 'ESA WorldCover', ['10 m land cover, 2021'])
    s_bld = d.src('bldg', s_wc.b + 8, 'Overture buildings', ['footprints, read with DuckDB'], kept=['data/buildings · 4.4 GB'])
    s_can = d.src('land', s_bld.b + 8, 'Canopy height · leaf type', ['Meta & WRI (1.2 m imagery)', 'Copernicus HRL · NALCMS'], kept=['data/cache/chm10 · 43 GB'])
    n_lc = d.card('land', 'd1', cy, 'landcover.py', 'Python', ['WorldCover at each grid cell'], [(['grid.class.u8'], 'z11 grid · 3.2 GB')])
    n_trees = d.card('land', 'd1', s_can.y + 22, 'leaftype.py · trees.py', 'Python', ['cover · height · leaf type'],
                     [(['trees-cover.tiles', 'trees-height · trees-leaf'], 'WebP archives · 8.7 GB')])
    p_trees = d.pill('land', n_trees.my, ['/tiles/trees/{var}'])
    b_trees = d.layer('land', 0, 'Tree cover · height · leaf', ['colour-relief on the GPU'], cy=p_trees.my)
    d.arrow('land', (s_wc.r, n_lc.y + 18), (n_lc.l, n_lc.y + 18))
    d.arrow('land', (s_can.r, n_trees.y + 18), (n_trees.l, n_trees.y + 18))
    d.arrow('land', (n_trees.r, p_trees.my), (p_trees.l, p_trees.my))
    d.to_layer('land', p_trees, b_trees)
    y1 = max(n_trees.b, s_can.b, b_trees.b) + PAD
    d.lane('LAND COVER & TREES', y0, y1)

    # ---- 6. rail & ferry service ---------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_tt = d.src('net', cy, 'Timetables', ['GTFS via Mobility Database,', 'operators’ own timetables'], kept=['data/rail · data/ferries', '3.7 GB'])
    s_osm6 = d.src('osm', s_tt.b + 8, 'OpenStreetMap', ['ferry routes · rail stops'])
    n_tr = d.card('net', 'd1', cy, 'train & ferry service', 'Rust · Py', ['railfreq · gtfs · ferries · stations', 'trains & sailings a day'],
                  [(['rail-freq.bin'], 'trains a day per rail way'), (['ferries.json · stations.json'], 'GeoJSON')], minh=s_osm6.b - cy)
    p_tr = d.pill('net', n_tr.y + 40, ['/api/railfreq', '/api/layer/ferries', '/api/layer/stations'])
    b_tr = d.layer('net', 0, 'Ferries · rail stops', ['rail lines by trains a day,', 'sailings a day per route'], cy=p_tr.my)
    d.arrow('net', (s_tt.r, s_tt.y + 24), (n_tr.l, s_tt.y + 24))
    d.arrow('osm', (s_osm6.r, s_osm6.my), (n_tr.l, s_osm6.my))
    d.arrow('net', (n_tr.r, p_tr.my), (p_tr.l, p_tr.my))
    d.to_layer('net', p_tr, b_tr)
    y1 = max(n_tr.b, s_osm6.b, b_tr.b) + PAD
    d.lane('RAIL & FERRY SERVICE', y0, y1)
    h = y1 + 8

    # ---- connectors between rows ---------------------------------------------------------------
    xa = n_ext.l + 52   # road points → terrain (which tiles; the grid near roads)
    d.arrow('net', (xa, n_ext.t), (xa, n_terr.b), label='road points', at=(xa + 7, (n_terr.b + n_ext.t) / 2 + 4), cross=True)
    xc = n_terr.l + 120   # terrain → peaks
    d.arrow('terr', (xc, n_terr.t), (xc, n_her.b), label='terrain tiles (peaks)', at=(xc + 7, (n_her.b + n_terr.t) / 2 + 4), cross=True)
    yb, xb = l3_bottom + GAP / 2, n_scen.l + 40   # terrain → scenic (drape heights, z11 grid)
    d.arrow('terr', (n_terr.l + 160, n_terr.b), (n_terr.l + 160, yb), (xb, yb), (xb, n_scen.t), label='terrain · z11 grid', at=(n_slope.l + 8, yb - 5), cross=True)
    yd, xd = n_lay.b + 14, n_scen.l + 128   # designated areas → scenic flags
    d.arrow('place', (n_her.r, yd), (xd, yd), (xd, n_scen.t), label='designated areas', at=(n_lay.r + 8, yd - 5), cross=True)
    # land cover, buildings, canopy → scenic (higher rows turn up first: no crossings)
    for k, bx, by, lab, dx in [('land', n_lc.r, n_lc.y + 34, 'land cover', 40), ('bldg', s_bld.r, max(s_bld.y + 14, n_lc.b + 9), 'buildings', 84),
                               ('land', s_can.r, s_can.y + 11, 'canopy', 128)]:
        x = n_scen.l + dx
        d.arrow(k, (bx, by), (x, by), (x, n_scen.b), label=lab, at=(COLS['d2'][0] + 8, by - 5), cross=True)

    heads = [(COLS['src'][0], 'SOURCES', None), (COLS['d1'][0], 'BUILD STEPS → FILES THEY WRITE (data/build)', None),
             (COLS['srv'][0], 'SERVER (RUST)', None), (COLS['brw'][0], 'MAP (BROWSER)', None)]
    aria = ('Data pipeline of the Scenic Roads app today. Six rows: map context, places and heritage, terrain, roads, land cover and trees, '
            'rail and ferry service. Each runs from remote sources through Rust, Python or Java build steps to files in data/build, '
            'then through the Rust server to map layers in the browser. The roads row adds per-point data step by step: OSM extract, '
            'DEM elevations, scenic metrics that also take in terrain, land cover, canopy, buildings and designated areas, then road tiles.')
    return d.svg(h, aria, heads)
