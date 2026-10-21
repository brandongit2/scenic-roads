"""How the map is built (docs/steps.md, docs/building.md, docs/pipeline.md): OpenStreetMap comes from one worldwide download, cut
by z6 tile; a region is only an outline of what to build. Every step runs per z6 tile, per z3 tile near the coverage, or once,
worldwide; ⇄ marks the steps whose jobs any member of the pool may take (the rest are the lead's), TASK the steps with parts any
worker may run (docs/workers.md). A card's last line is its time and memory: measured (docs/phase-times.md, the steps table's
notes) or the steps table's first guess. The curated inputs (sheets with a folded corner) are what a person or an agent made by
hand, with how reproducible each is today (curated.py lists them)."""
from diag import COLS, Diagram

GAP, PAD = 12, 10


def build(check=False):
    d = Diagram('p', check)

    # ---- 0. OpenStreetMap: the planet, cut by z6 tile; your regions are outlines -----------------
    y0 = 60
    cy = y0 + 24
    s_osm = d.src('osm', cy, 'OSM planet', ['the whole world; the NAS', 'fetches it twice a year,', 'resuming if interrupted'],
                  kept=['sources/osm/<date>/'])
    n_pass = d.card('osm', 'd1', cy, 'OSM pass', 'osmium', ['filtered, then cut by z6 tile', '(10 km buffer); sets,', 'outlines, reach and every', 'way’s whole road'],
                    [(['pieces/<z6>.osm.pbf'], 'PBF · all land ≈ 58 GB'), (['sets/ · outlines'], 'rail, ferries, places …'),
                     (['roads/<z6> · reach'], 'each way’s road and offset')], scope='global',
                    fig='outlines: 8.3 GB at most, measured')
    c_reg = d.cur('osm', s_osm.b + 10, 'Your regions', ['OSM boundaries, shapes, sea', 'zones; the enabled ones’', 'union is the coverage;', 'disabled: checked, not built'],
                  'you, as files in inputs/regions/', 'doc')
    n_rx = d.card('osm', 'd1', n_pass.b + 22, 'regions’ extract', 'Rust', ['the relations and ways the', 'regions name that the', 'outlines lack; filtered planet,', 'then the full one'],
                  [(['sources/osm/<date>/', 'regions-extract'], 'JSON · grows within a pass')], scope='global')
    d.arrow('osm', (s_osm.r, n_pass.y + 18), (n_pass.l, n_pass.y + 18))
    d.arrow('osm', (n_pass.r, n_pass.y + 18), (n_pass.r + 34, n_pass.y + 18), label='read by the rows below as “OSM extracts” and “OSM sets”',
            at=(n_pass.r + 42, n_pass.y + 22))
    yx = n_rx.y + 18
    d.arrow('osm', (c_reg.r, yx), (n_rx.l, yx)) if c_reg.b > yx + 8 else \
        d.arrow('osm', (c_reg.r, c_reg.my), (c_reg.r + 12, c_reg.my), (c_reg.r + 12, yx), (n_rx.l, yx))
    yr = n_rx.y + 46
    d.arrow('osm', (n_rx.r, yr), (n_rx.r + 34, yr), label='the input check reads it; the coverage, the enabled regions’ union, says which tiles and features every step below builds',
            at=(n_rx.r + 42, yr + 4))
    d.arrow('osm', (n_rx.r, yr + 20), (n_rx.r + 34, yr + 20), label='no step knows region borders; with cover = "earth" the regions must cover the Earth once, disabled ones included',
            at=(n_rx.r + 42, yr + 24))
    y1 = max(c_reg.b, n_rx.b) + PAD
    d.lane('OPENSTREETMAP · YOUR REGIONS', y0, y1)

    # ---- 1. map context: the worldwide basemap, its water at every zoom, labels -------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_base = d.card('base', 'd1', cy, 'basemap', 'Java', ['Planetiler, the whole world,', 'once per pass'],
                    [(['layers/basemap/world-<date>'], 'PMTiles · MVT z0–14')], scope='global')
    p_base = d.pill('base', n_base.y + 30, ['/tiles/base'], note=['tile by tile from the', 'archive; names attached'])
    b_base = d.layer('base', 0, 'Basemap', ['roads · borders · parks · rivers'], cy=p_base.my)
    n_wat = d.card('base', 'd3', n_base.y + 46, 'water', 'Rust', ['each pixel’s exact share of', 'sea and inland water, from', 'the basemap’s z14 water and', 'the lake fill (Canada’s CanVec)'],
                   [(['layers/water/'], 'packs · PNG z0–9')], scope='global', fig='7.7 GB at most, measured; the lake fill 10.4 GB')
    b_wat = d.layer('base', b_base.b + 8, 'Water', ['a raster: its share of each', 'pixel, at every zoom'])
    p_wat = d.pill('base', b_wat.my, ['/tiles/water'], note=['z10 and deeper drawn', 'from the basemap’s z14'])
    b_coast = d.layer('base', b_wat.b + 8, 'Coastal shading', ['distance to shore, from the', 'water’s shares, in a worker'], computed=True)
    n_lab = d.card('base', 'd1', max(n_base.b + 22, n_wat.b + 14), 'labels', 'Python', ['ranked worldwide: places,', 'water and parks; their', 'English, kana, languages'],
                   [(['layers/labels/'], 'packs · MVT z0–12')], scope='global')
    s_pl = d.src('osm', cy, 'OSM pass', ['the basemap’s part: water,', 'borders, parks, places;', 'the labels set'], minh=n_base.h)
    c_pin = d.cur('base', s_pl.b + 10, 'The basemap’s pinned versions', ['its jar, Natural Earth, water', 'polygons, lake centrelines'],
                  'downloaded by hand', 'part')
    b_lab = d.layer('base', max(n_lab.y + 6, b_coast.b + 8), 'Place labels', ['placed by importance'])
    p_lab = d.pill('base', b_lab.my, ['/tiles/labels'], note='names attached')
    b_find = d.layer('base', b_lab.b + 8, 'Place search', ['the box: any word of a name,', 'as the map shows it'])
    p_find = d.pill('base', b_find.my, ['/api/places'], note='indexed on this Mac')
    d.arrow('osm', (s_pl.r, n_base.y + 18), (n_base.l, n_base.y + 18))
    d.arrow('base', (c_pin.r, c_pin.y + 20), (c_pin.r + 12, c_pin.y + 20), (c_pin.r + 12, n_base.b - 14), (n_base.l, n_base.b - 14))
    xs = s_pl.r + 14
    d.arrow('osm', (s_pl.r, s_pl.b - 16), (xs, s_pl.b - 16), (xs, n_lab.y + 18), (n_lab.l, n_lab.y + 18))
    d.arrow('base', (n_base.r, p_base.my), (p_base.l, p_base.my))
    d.arrow('base', (n_base.r, n_wat.y + 18), (n_wat.l, n_wat.y + 18))
    d.arrow('base', (n_wat.r, p_wat.my), (p_wat.l, p_wat.my))
    d.arrow('base', (n_lab.r, p_lab.my), (p_lab.l, p_lab.my))
    d.arrow('base', (n_lab.r, p_find.my), (p_find.l, p_find.my))
    d.arrow('base', (p_base.r, b_base.my), (b_base.l, b_base.my))
    d.to_layer('base', p_wat, b_wat)
    d.arrow('base', (p_wat.r, p_wat.my + 8), (b_coast.l - 12, p_wat.my + 8), (b_coast.l - 12, b_coast.my), (b_coast.l, b_coast.my))
    d.to_layer('base', p_lab, b_lab)
    d.to_layer('base', p_find, b_find)
    y1 = max(n_lab.b, b_find.b, p_find.b, c_pin.b) + PAD
    d.lane('MAP CONTEXT', y0, y1)
    y_ctx = y1

    # ---- 2. names in English, and descriptions: straight to the servers --------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_sp = d.card('base', 'd1', cy, 'spoken', 'Rust', ['the languages spoken where,', 'from the pass’s outlines', 'and CLDR, once a pass'],
                  [(['global/spoken'], 'a raster of 1/128° cells')], scope='global')
    n_todo = d.card('base', 'd2', cy, 'to-do lists', 'Rust', ['after a catalog that changed', 'what they read: names with no', 'English; landmarks with no', 'description'],
                    [(['translations/todo/'], 'a list per language'), (['descriptions/todo/'], 'landmarks · areas')], scope='global',
                    fig='10 min a run, all NAS reads')
    s_osmn = d.src('osm', cy, 'Outlines · every name', ['the pass’s outlines; names', 'in labels, roads, landmarks'])
    c_cldr = d.cur('base', max(s_osmn.b, n_sp.b) + 10, 'Languages by territory', ['CLDR’s table; refinements', '(Quebec, Wales, Catalonia …)'],
                   'a script, and rules in code', 'doc')
    c_tr = d.cur('base', c_cldr.b + 10, 'Translations', ['a line per name, kind and', 'language: rules, agents'], 'Claude agents (Haiku; earlier Sonnet)', 'part')
    c_ds = d.cur('base', c_tr.b + 10, 'Descriptions', ['55 words a landmark, its', 'sources credited'], 'Claude agents (Sonnet)', 'part')
    p_en = d.pill('base', c_tr.my, ['English attached', 'to all it serves'], note=['a thing’s own English,', 'else a translation'])
    b_en = d.layer('base', 0, 'English everywhere', ['labels, basemap names, roads,', 'rail lines, stops, popups'], cy=p_en.my)
    p_ds = d.pill('base', c_ds.my + 6, ['descriptions laid', 'over popups'], note='src credited')
    b_ds = d.layer('base', 0, 'Popups’ descriptions', ['landmarks, areas, details'], cy=p_ds.my)
    d.arrow('osm', (s_osmn.r, n_sp.y + 18), (n_sp.l, n_sp.y + 18))
    d.arrow('base', (c_cldr.r, c_cldr.y + 18), (n_sp.l + 30, c_cldr.y + 18), (n_sp.l + 30, n_sp.b))
    d.arrow('base', (n_sp.r, n_todo.y + 18), (n_todo.l, n_todo.y + 18))
    xt = n_sp.r + 14
    yt = p_en.my
    d.arrow('base', (c_tr.r, yt), (p_en.l, yt), label='read by every Mac’s server within a minute or two: no rebuild', at=(n_todo.l + 8, yt - 5))
    yd = p_ds.my
    d.arrow('base', (c_ds.r, yd), (p_ds.l, yd))
    ylp = c_tr.t - 5   # the translators' and writers' loop, outside the pipeline
    d.arrow('base', (n_todo.l + 90, n_todo.b), (n_todo.l + 90, ylp), (c_tr.l + 150, ylp), (c_tr.l + 150, c_tr.t),
            label='lists → agents, on request', at=(n_todo.l + 98, (n_todo.b + ylp) / 2 + 4))
    d.arrow('base', (n_sp.r - 30, n_sp.b), (n_sp.r - 30, p_en.y - 8), (p_en.l + 30, p_en.y - 8), (p_en.l + 30, p_en.t),
            label='the lookup order', at=(n_todo.r + 12, p_en.y + 5))
    d.to_layer('base', p_en, b_en)
    d.to_layer('base', p_ds, b_ds)
    y1 = max(n_todo.b, c_ds.b, b_ds.b, p_ds.b) + PAD
    d.lane('NAMES IN ENGLISH · DESCRIPTIONS', y0, y1)

    # ---- 3. places & heritage --------------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_hs = d.card('place', 'd1', cy, 'heritage sites', 'Python', ['heritage.py on the registers', 'over the coverage + 20 km,', 'sliced per z6 tile'],
                  [(['work/heritage/<date>/'], 'positions · areas per z6')], scope='global', fig='20 min a run')
    c_regs = d.cur('place', cy, 'Registers (a snapshot)', ['UNESCO · Parks Canada · NRHP ·', 'Mérimée · NHLE · 文化財 …;', 'park facts, English names'],
                   'hand downloads, scripts by hand', 'part')
    s_osm2 = d.src('osm', c_regs.b + 10, 'OSM extracts · sets', ['POIs · designated areas ·', 'summits · hiking routes'])
    s_wd = d.src('place', s_osm2.b + 8, 'Wikidata · Wikipedia', ['facts; pageviews, four', 'months a pass, all languages'])
    n_poi = d.card('place', 'd1', n_hs.b + 22, 'candidates · peaks', 'Rust', ['each z6 tile’s points; prominence', 'and isolation over z12 and', 'the worldwide z8'],
                   [(['work/pois/ · work/peaks/'], 'per z6 tile')], scope='z6', shared=True,
                   fig='peaks: median 5 s a tile, p90 24 s; 1.4 GB p90')
    n_items = d.card('place', 'd2', n_poi.y, 'facts · pageviews', 'Python', ['the candidates’ Wikidata', 'items, once a pass'],
                     [(['sources/items/<date>/'], 'facts · views'), (['sources/pageviews/'], 'each month streamed once')], scope='global')
    n_hc = d.card('place', 'd2', cy, 'heritage chain', 'Python', ['on the pass: each site’s', 'details, outlines and fame'],
                  [(['work/heritage/<date>/'], 'details · outlines · fame')], scope='global', fig='83 min a run, 84 % the planet’s clip')
    n_mk = d.card('place', 'd3', cy, 'landmarks', 'Rust', ['points per kind: fame, rarity,', 'thinned tiles, per-cell counts'],
                  [(['markdata/all · marks-*/all'], 'bundles: sectioned per z6 · RDMT')], scope='global')
    n_ov = d.card('place', 'd3', n_mk.b + 22, 'area overlays', 'Rust', ['from the heritage chain, with', 'the landmarks’ site ids'],
                  [(['layers/ov-*/all · ovdata/'], 'bundles · MVT; details per z3')], scope='global')
    p_mk = d.pill('place', n_mk.y + 30, ['/api/marks/…'], note=['tiles by view; In view,', 'details'])
    b_lm = d.layer('place', 0, 'Landmarks', ['dots on the GPU; names tiled', 'in a worker; popups'], cy=p_mk.my)
    p_ov = d.pill('place', n_ov.y + 30, ['/tiles/ov/…'], note='the pack for the tile')
    b_ov = d.layer('place', 0, 'Area overlays', ['heritage areas, Indigenous', 'lands, World Heritage outlines'], cy=p_ov.my)
    d.arrow('place', (c_regs.r, n_hs.y + 18), (n_hs.l, n_hs.y + 18))
    d.arrow('osm', (s_osm2.r, s_osm2.my), (s_osm2.r + 14, s_osm2.my), (s_osm2.r + 14, n_poi.y + 18), (n_poi.l, n_poi.y + 18))
    yw = max(n_poi.b, n_items.b, s_wd.b) + 8
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

    # ---- 4. terrain and slope: a run per z6 tile, an assembly per z3 tile -----------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_aws = d.src('terr', cy, 'AWS Terrain Tiles', ['Terrarium PNG · ~27 m at z12;', 'declared; kept raw, never edited'],
                  kept=['sources/aws-terrarium/', 'a z3 or z6 tile’s tiles packed'])
    s_glo = d.src('terr', s_aws.b + 8, 'Copernicus GLO-30', ['30 m, north of 59.5° N, where', 'AWS mixes datums; declared'], kept=['sources/fetched/glo30/'])
    s_bw = d.src('base', s_glo.b + 8, 'The basemap’s water', ['lakes to their shore’s level,', 'the sea to 0 m; digests per', 'target (terrain-water, lead)'])
    n_terr = d.card('terr', 'd1', cy, 'terrain', 'Rust', ['z9–12 within 20 km of the', 'coverage; repaired; GLO-30', 'north of 60° N; lakes and', 'sea flattened; z8 subtrees', 'as tasks'],
                    [(['layers/terrain/ hi'], 'packs · Terrarium PNG'), (['work/terrain-mid/'], 'z9 means, lakes’ levels')],
                    scope='z6', shared=True, task=True, fig='median 24 s a tile, p90 91 s; 6.8 GB p90')
    n_tlo = d.card('terr', 'd2', cy + 34, 'terrain, zoomed out', 'Rust', ['z3–8 from its z6 tiles’ mids;', 'the root (z0–2) once none', 'is left'],
                   [(['layers/terrain/ lo · root'], 'packs · Terrarium PNG')], scope='z3', shared=True, fig='first guess 1.2 GB')
    n_slope = d.card('terr', 'd3', cy + 34, 'slope', 'Rust', ['Horn at z12, kept to z11,', 'from the terrain packs and', 'the neighbours’ edges'],
                     [(['layers/slope/ hi'], 'packs · PNG')], scope='z6', shared=True, fig='median 10 s a tile, p90 59 s; 0.9 GB p90')
    n_slo = d.card('terr', 'd3', n_slope.b + 22, 'slope, zoomed out', 'Rust', ['z3–8 from its z6 tiles’ mids;', 'the root once none is left'],
                   [(['layers/slope/ lo · root'], 'packs · PNG')], scope='z3', shared=True)
    n_z8 = d.card('terr', 'd2', n_tlo.b + 22, 'z8 terrain', 'Rust', ['every z8 tile, repaired,', 'with its maximum, once', '(peaks read it; not served)'],
                  [(['sources/terrain-z8-v3'], 'one pack')], scope='global')
    p_t = d.pill('terr', cy + 12, ['/tiles/terrain'], note='the pack for the tile')
    b_terr = d.layer('terr', 0, '3D terrain · hill-shading', ['elevation tint'], cy=p_t.my)
    b_cont = d.layer('terr', b_terr.b + 8, 'Contour lines', ['traced from terrain tiles'], computed=True)
    b_slope = d.layer('terr', max(b_cont.b + 8, n_slope.y), 'Slope tint', ['colour per quarter, averaged'])
    p_s = d.pill('terr', b_slope.my, ['/tiles/slope'], note='z12 made when asked')
    d.arrow('terr', (s_aws.r, n_terr.y + 18), (n_terr.l, n_terr.y + 18))
    xt = n_terr.l - 12   # GLO-30 and the water join the terrain step on one trunk
    for k, sb, yin in [('terr', s_glo, n_terr.y + 46), ('base', s_bw, n_terr.y + 74)]:
        d.arrow(k, (sb.r, sb.y + 18), (xt, sb.y + 18), (xt, yin), (n_terr.l, yin))
    d.arrow('terr', (n_terr.r, n_tlo.y + 18), (n_tlo.l, n_tlo.y + 18))
    d.arrow('terr', (n_tlo.r, n_slope.y + 18), (n_slope.l, n_slope.y + 18))
    d.arrow('terr', (n_slope.l + 40, n_slope.b), (n_slope.l + 40, n_slo.t))
    d.arrow('terr', (s_aws.r, s_aws.y + 40), (n_terr.l - 28, s_aws.y + 40), (n_terr.l - 28, n_z8.y + 18), (n_z8.l, n_z8.y + 18))
    d.arrow('terr', (n_terr.r, p_t.my), (p_t.l, p_t.my))
    d.arrow('terr', (n_slope.r, p_s.my), (p_s.l, p_s.my))
    d.to_layer('terr', p_t, b_terr)
    d.arrow('terr', (p_t.r, p_t.my + 8), (b_cont.l - 12, p_t.my + 8), (b_cont.l - 12, b_cont.my), (b_cont.l, b_cont.my))
    d.to_layer('terr', p_s, b_slope)
    y1 = max(n_terr.b, n_z8.b, n_slo.b, b_slope.b, s_bw.b) + PAD
    d.lane('TERRAIN · SLOPE', y0, y1)
    l4_bottom = y1

    # ---- 4b. water depth: per z3 tile, then per z6 tile where a source is finer ---------------------
    y0 = y1 + GAP
    cy = y0 + 24
    c_dg = d.cur('terr', cy, 'Depth grids', ['15 staged: GEBCO, BlueTopo,', 'NONNA, EMODnet, SHOM, GtTM,', 'surveyed lakes, estimates'],
                 'producers/depth/, run by hand on the M4; declared', 'part')
    s_dw = d.src('base', c_dg.b + 8, 'The basemap’s water', ['z6–14 and its rivers; the', 'terrain’s sea and lakes'])
    n_dlo = d.card('terr', 'd1', cy, 'depth, zoomed out', 'Rust', ['z3 tiles within 200 nmi of', 'the coverage a source meets:', 'z6–8 composed, z3–5 means'],
                   [(['layers/depth/ lo'], 'packs')], scope='z3', shared=True, fig='median 66 s, p90 22 min; 2.1 GB p90')
    n_dep = d.card('terr', 'd2', cy, 'depth', 'Rust', ['z9–16 where a source finer', 'than z8 has grids; reads', 'the lo packs round it'],
                   [(['layers/depth/ hi'], 'packs')], scope='z6', shared=True, fig='median 45 s, p90 15 min; 2.7 GB p90')
    p_d = d.pill('terr', cy + 30, ['/tiles/depth', '/tiles/terrain?floor=1'], note=['the floor: terrain', 'less the depth'])
    b_d = d.layer('terr', 0, 'Depth · contours', ['nautical blues on the floor’s', 'shading; sources named'], cy=p_d.my)
    d.arrow('terr', (c_dg.r, n_dlo.y + 18), (n_dlo.l, n_dlo.y + 18))
    d.arrow('base', (s_dw.r, s_dw.y + 18), (n_dlo.l - 12, s_dw.y + 18), (n_dlo.l - 12, n_dlo.y + 46), (n_dlo.l, n_dlo.y + 46))
    d.arrow('terr', (n_dlo.r, n_dep.y + 18), (n_dep.l, n_dep.y + 18))
    d.arrow('terr', (n_dep.r, p_d.my), (p_d.l, p_d.my))
    d.to_layer('terr', p_d, b_d)
    y1 = max(n_dep.b, n_dlo.b, s_dw.b, b_d.b, p_d.b) + PAD
    d.lane('WATER DEPTH', y0, y1)
    y_depth = y1

    # ---- 5. roads (per road point, per z6 tile) ---------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_ext = d.card('net', 'd1', cy, 'extract', 'Rust', ['the ways each z6 tile owns,', 'inside the coverage; with', 'elev and scenic, one job'],
                   [(['base/<z6>'], 'sectioned · per road point')], scope='z6', shared=True,
                   fig='the job: median 140 s, p90 19 min; 6.3 GB p90, 8.4 GB at most')
    n_samp = d.card('terr', 'd2', cy, 'elev · tile elev', 'Rust', ['DEMs chosen by location,', 'then clean-up and grade'],
                    [(['elevations · grade'], 'in the base pack')], kept='cache/dem-units/', scope='z6', shared=True, task=True)
    n_scen = d.card('scen', 'd3', cy, 'scenic', 'Rust', ['samples every 100 m: horizons', 'to 300 m past trees, views to', '15 km, buildings, designations'],
                    [(['13 channels a point'], 'in the base pack')], kept='cache/scenic-units/', scope='z6', shared=True, task=True)
    n_tile = d.card('net', 'd4', cy, 'map tiles', 'Rust', ['per z6 tile: the tiles whose', 'ways come in, their road values;', 'tiles, query parts'],
                    [(['layers/roads/ · rails/'], 'packs · RT v7 z4–14'), (['hidata/<z6>'], 'query parts · summaries')], scope='z6', shared=True,
                    fig='median 0 s a tile, p90 20 s; 0.4 GB p90')
    s_osm4 = d.src('osm', cy, 'OSM extracts', ['roads · rail lines'])
    s_dem = d.src('terr', s_osm4.b + 8, 'Road DEMs', ['HRDEM · 3DEP · MRDEM (N. Am.) ·', 'GSI (Japan) · FABDEM 30 m;', 'read by range, road blocks only'],
                  kept=['sources/fabdem/', 'sources/dem-cache/'])
    c_dem = d.cur('terr', s_dem.b + 10, 'Which DEM where', ['the sources and their order by', 'area; HRDEM’s tile lists'], 'chosen by hand', 'part')
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
    xcd = n_samp.l + 40
    d.arrow('terr', (c_dem.r, c_dem.y + 18), (xcd, c_dem.y + 18), (xcd, n_samp.b))
    d.arrow('net', (n_tile.r, p_road.my), (p_road.l, p_road.my))
    xq = n_tile.l + 150
    d.arrow('scen', (xq, n_tile.b), (xq, y_sc), (p_scen.l, y_sc))
    d.to_layer('net', p_road, b_road)
    d.to_layer('scen', p_scen, b_scen)
    y1 = max(n_scen.b, n_samp.b, c_dem.b, b_scen.b) + PAD
    d.lane('ROADS · PER ROAD POINT', y0, y1)

    # ---- 6. land cover, trees & buildings ------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    s_wc = d.src('land', cy, 'ESA WorldCover', ['10 m land cover, 2021'])
    n_lc = d.card('land', 'd1', cy, 'grids', 'Rust', ['land cover, canopy, cover at', 'z11, made in each z6 tile’s job'],
                  [(['layers/grid-*/'], 'packs · z11, not served')], scope='z6', shared=True)
    y2 = max(s_wc.b, n_lc.b) + 22
    n_rb = d.card('bldg', 'd1', y2, 'roadside buildings', 'Py · Rust', ['every building’s box, from the', 'release’s bbox columns, once', 'per release'],
                  [(['sources/buildings/<rel>/'], 'z8 tiles of boxes · index')], scope='global')
    n_bf = d.card('bldg', 'd1', n_rb.b + 22, 'their sources', 'Python', ['the release’s files and GHSL’s', 'tiles within 20 km of the', 'coverage, asked by distance'],
                  [(['sources/overture/<rel>/'], 'parquet, with their footers'), (['sources/ghsl/R2023A/'], 'GeoTIFF')], scope='global',
                  fig='2.0 GB, measured: the coverage read')
    n_bp = d.card('bldg', 'd2', n_bf.y, 'bldprep', 'Py · Rust', ['a z6 tile’s buildings and parts,', 'decoded; GHSL at each'],
                  [(['work/bld/<z6>'], 'sectioned · per z14 block')], scope='z6', shared=True,
                  fig='median 3 s a tile, p90 56 s; 1.7 GB p90')
    s_bld = d.src('bldg', y2, 'Overture buildings', ['release 2026-09-23.1, on S3:', 'footprints; heights or floors', 'where known'])
    s_gh = d.src('bldg', s_bld.b + 8, 'GHSL building heights', ['EC JRC: the mean height in', 'each ~90 m cell, 2018'])
    n_b3 = d.card('bldg', 'd4', n_bp.y, '3D buildings', 'Rust', ['heights: measured, else floors,', 'neighbours, GHSL, size, kind;', 'z8 tiles as tasks; the far', 'field assembled per z3 tile'],
                  [(['layers/buildings/ hi'], 'packs · MVT z12–14'), (['layers/buildings-field/'], 'hi per z6 · lo per z3')], scope='z6', shared=True, task=True,
                  fig='median 2 s a tile, p90 207 s; 3.4 GB p90')
    p_bld = d.pill('bldg', n_b3.y + 40, ['/tiles/buildings', '/tiles/buildings-field'], note=['z14 meshes; below', 'it the far field'])
    b_bld = d.layer('bldg', 0, '3D buildings', ['extruded on the terrain;', 'the field below zoom 14'], cy=p_bld.my)
    d.arrow('land', (s_wc.r, n_lc.y + 18), (n_lc.l, n_lc.y + 18))
    d.arrow('bldg', (s_bld.r, n_rb.y + 18), (n_rb.l, n_rb.y + 18))
    d.arrow('bldg', (s_bld.r, n_bf.y + 18), (n_bf.l, n_bf.y + 18)) if s_bld.b > n_bf.y + 22 else \
        d.arrow('bldg', (s_bld.r, s_bld.b - 12), (s_bld.r + 14, s_bld.b - 12), (s_bld.r + 14, n_bf.y + 18), (n_bf.l, n_bf.y + 18))
    yg = max(s_gh.y + 18, n_bf.y + 46)
    if yg > s_gh.y + 18:
        d.arrow('bldg', (s_gh.r, s_gh.y + 18), (n_bf.l - 12, s_gh.y + 18), (n_bf.l - 12, yg), (n_bf.l, yg))
    else:
        d.arrow('bldg', (s_gh.r, yg), (n_bf.l, yg))
    d.arrow('bldg', (n_bf.r, n_bp.y + 18), (n_bp.l, n_bp.y + 18))
    d.arrow('bldg', (n_bp.r, n_b3.y + 18), (n_b3.l, n_b3.y + 18))
    d.arrow('bldg', (n_b3.r, p_bld.my), (p_bld.l, p_bld.my))
    d.to_layer('bldg', p_bld, b_bld)
    s_can = d.src('land', max(s_gh.b, n_bf.b, n_bp.b, n_b3.b) + 12, 'Canopy height · leaf type', ['Meta & WRI (1.2 m imagery) ·', 'Copernicus HRL · NALCMS'],
                  kept=['sources/canopy/ (10°)', 'sources/trees/leaf/'])
    n_trees = d.card('land', 'd1', s_can.y + 30, 'tree cover', 'Rust', ['cover · height · leaf type,', 'z9–12 per z6 tile, clipped', 'to the coverage; its mid'],
                     [(['layers/trees-*/ hi'], 'packs · Terrarium WebP'), (['work/trees-mid/'], 'its z8 tiles and values')], scope='z6', shared=True, task=True,
                     fig='median 7 s a tile, p90 33 s; 2.6 GB p90')
    n_tlo = d.card('land', 'd2', n_trees.y, 'tree cover zoomed out', 'Rust', ['z4–8 assembled per z3', 'tile from its z6 tiles’ mids'],
                   [(['layers/trees-*/ lo'], 'packs · Terrarium WebP')], scope='z3', shared=True)
    p_trees = d.pill('land', n_trees.y + 34, ['/tiles/trees/{var}'], note='the pack for the tile')
    b_trees = d.layer('land', 0, 'Tree cover · height · leaf', ['colour-relief on the GPU'], cy=p_trees.my)
    d.arrow('land', (s_can.r, n_trees.y + 18), (n_trees.l, n_trees.y + 18))
    d.arrow('land', (n_trees.r, n_tlo.y + 18), (n_tlo.l, n_tlo.y + 18))
    d.arrow('land', (n_tlo.r, p_trees.my), (p_trees.l, p_trees.my))
    d.to_layer('land', p_trees, b_trees)
    y1 = max(n_trees.b, s_can.b, b_trees.b) + PAD
    d.lane('LAND COVER, TREES & BUILDINGS', y0, y1)

    # ---- 7. rail & ferry service -----------------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_rf = d.card('net', 'd1', cy, 'trains a day', 'Py · Rust', ['the coverage’s rail feeds, each', 'fetched once; their trains', 'matched onto the pass’s tracks'],
                  [(['global/railfreq'], 'trains a day per way')], scope='global')
    n_st = d.card('net', 'd1', n_rf.b + 22, 'rail stops · ferries', 'Rust · Py', ['stops near the built z6 tiles;', 'ferries worldwide, with', 'their sailings a day'],
                  [(['layers/stations/ · ferries/'], 'packs · MVT, GeoJSON blocks')], scope='global')
    s_tt = d.src('net', cy, 'Rail feeds (GTFS)', ['Mobility Database, operators’', 'own: found and fetched by', 'the job'], kept=['sources/rail/gtfs/'])
    c_mtr = d.cur('net', s_tt.b + 10, 'The MTR’s lines', ['Hong Kong’s trains: stations,', 'headways, from mtr.com.hk'], 'Claude research, by hand', 'part')
    c_fy = d.cur('net', c_mtr.b + 10, 'Ferry timetables', ['sailings a day: operators’', 'GTFS counted, pages read'], 'Claude research; feeds declared, counted by the job', 'part')
    s_osm6 = d.src('osm', c_fy.b + 8, 'OSM sets', ['the world’s tracks, stations,', 'routes and ferries'])
    p_rf = d.pill('net', n_rf.y + 30, ['/api/railfreq'], note='trains a day, per way')
    b_rf = d.layer('net', 0, 'Trains a day', ['rail lines coloured and', 'filtered by it'], cy=p_rf.my)
    p_st = d.pill('net', max(n_st.y + 34, b_rf.b + 8 + 22), ['/tiles/stations', '/tiles/ferries'])
    b_st = d.layer('net', 0, 'Ferries · rail stops', ['stops in their line’s colour;', 'sailings a day per route'], cy=p_st.my)
    d.arrow('net', (s_tt.r, n_rf.y + 18), (n_rf.l, n_rf.y + 18))
    xm = s_tt.r + 14
    d.arrow('net', (c_mtr.r, c_mtr.y + 18), (xm, c_mtr.y + 18), (xm, n_rf.y + 46), (n_rf.l, n_rf.y + 46))
    d.arrow('net', (c_fy.r, c_fy.y + 18), (c_fy.r + 26, c_fy.y + 18), (c_fy.r + 26, n_st.y + 46), (n_st.l, n_st.y + 46))
    d.arrow('osm', (s_osm6.r, s_osm6.my), (n_st.l + 30, s_osm6.my), (n_st.l + 30, n_st.b))
    d.arrow('net', (n_rf.r, p_rf.my), (p_rf.l, p_rf.my))
    d.arrow('net', (n_st.r, p_st.my), (p_st.l, p_st.my))
    d.to_layer('net', p_rf, b_rf)
    d.to_layer('net', p_st, b_st)
    y1 = max(n_st.b, s_osm6.b, b_st.b) + PAD
    d.lane('RAIL & FERRY SERVICE', y0, y1)

    # ---- 8. publishing, as the z6 tiles are built -----------------------------------------------------------
    y0 = y1 + GAP
    cy = y0 + 24
    n_rd = d.card('mix', 'd1', cy, 'publishing', 'Rust', ['as soon as a z6 tile is built:', 'the map tiles whose owners', 'changed, a z6 tile’s together,', 'and the road index; then at', 'most one catalog a minute;', 'the low zooms, landmarks and', 'overlays by their cadence'],
                  [(['catalog/<n>.json.zst'], 'fmt 2: every file the map reads, bundles named per pack')], scope='global')
    s_rd = d.src('mix', cy, 'Regions as they’re done', ['a region at a time: those', 'the map lacks first, then', 'the fewest z6 tiles left'], minh=n_rd.h)
    p_cat = d.pill('mix', n_rd.y + 46, ['/api/catalog · meta'], note=['the newest that decodes;', 'every 30 s while in use'])
    b_cat = d.layer('mix', 0, 'New data, in place', ['the page checks each minute;', 'URLs change where data did'], cy=p_cat.my)
    d.arrow('mix', (s_rd.r, n_rd.y + 18), (n_rd.l, n_rd.y + 18))
    d.arrow('mix', (n_rd.r, p_cat.my), (p_cat.l, p_cat.my),
            label='with what the depth, the trains’, the landmarks’ and the 3D buildings’ steps made by then', at=(n_rd.r + 30, p_cat.my - 5))
    d.to_layer('mix', p_cat, b_cat)
    y1 = max(n_rd.b, s_rd.b, b_cat.b) + PAD
    d.lane('PUBLISHING', y0, y1)
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

    heads = [(COLS['src'][0], 'SOURCES · CURATED', 'dashed: kept; folded: by hand'),
             (COLS['d1'][0], 'BUILD STEPS → FILES THEY WRITE', 'on the NAS; ⇄: any member’s jobs, else the lead’s; TASK: any worker’s tasks'),
             (COLS['srv'][0], 'SERVER (RUST)', 'Mac’s copy, else the NAS'), (COLS['brw'][0], 'MAP (BROWSER)', None)]
    aria = ('How the map is built. OpenStreetMap comes from one worldwide download, twice a year, cut into extracts per z6 tile, '
            'with worldwide sets, outlines, reach and every way’s whole road. Your regions are only outlines: the enabled ones’ union, '
            'the coverage, says which tiles and features get built; disabled ones are checked but never built, so the list can cover '
            'the Earth. The regions’ extract supplies the relations and ways they name that the outlines lack. The basemap is drawn '
            'worldwide once a pass, and from its z14 water and the lake fill the water layer, each pixel’s exact share of water at '
            'every zoom. Terrain and slope are built a run per z6 tile and assembled per z3 tile; the water depth per z3 tile near '
            'the coverage, then per z6 tile where a source is finer. Each z6 tile’s roads, elevations and scenic values are one job, '
            'map tiles, tree cover and 3D buildings per z6 tile, zoomed out per z3 tile. Labels, '
            'the languages spoken where, the to-do lists, landmarks, the heritage chain, area overlays, rail stops, ferries and trains '
            'a day are worldwide, run by the lead. Curated inputs, made by hand, feed them, each with how reproducible it is today. '
            'Any member of the pool may take the jobs marked ⇄, the rest are the lead’s; tasks any worker may run, a device’s page too. '
            'Measured time and memory are from docs/phase-times.md. Publishing puts each z6 tile on the map within minutes, a catalog at most each minute.')
    return d.svg(h, aria, heads)
