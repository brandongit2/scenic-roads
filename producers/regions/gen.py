"""Task #422: region files for the whole list (~/Documents/region-list.txt) in the format of
docs/inputs.md §6.1.3, from #420's land files.

    python3 -I producers/regions/gen.py [--src ~/Documents/regions-new] [--out ~/Documents/regions-new-v2]
        [--eez /Volumes/personal/projects/scenic-roads/sources/fetched/marineregions-eez-v12/eez_v12.geojson]

Reads #420's `regions/` (land outlines), its `built.json` and `regions.json` (the list parsed: names,
groupings, notes), and the EEZ v12 GeoJSON (the zones' names, types and boxes; read only). Writes
`<out>/regions/` (replaced whole), `<out>/gen-notes.json` (where each zone went, what's approximate,
what's added). Every rule of the list it applies is below, by the list's words."""
import argparse, json, os, re, shutil, tomllib, unicodedata

ap = argparse.ArgumentParser()
ap.add_argument('--src', default=os.path.expanduser('~/Documents/regions-new'))
ap.add_argument('--out', default=os.path.expanduser('~/Documents/regions-new-v2'))
ap.add_argument('--eez', default='/Volumes/personal/projects/scenic-roads/sources/fetched/marineregions-eez-v12/eez_v12.geojson')
args = ap.parse_args()
SRC = os.path.join(args.src, 'regions')
OUT = os.path.join(args.out, 'regions')

notes = {'approx': {}, 'added': [], 'removed': [], 'zones': {}, 'unassigned_zones': []}


def approx(id, why):
    notes['approx'].setdefault(id, []).append(why)


def slug(s):
    s = unicodedata.normalize('NFKD', s).encode('ascii', 'ignore').decode().lower()
    return re.sub(r'[^a-z0-9]+', '-', s).strip('-')


# ---- #420's files -------------------------------------------------------------------------------
if os.path.exists(OUT):
    shutil.rmtree(OUT)
shutil.copytree(SRC, OUT)
files = {}
for d, _, fs in os.walk(OUT):
    for f in fs:
        if f.endswith('.toml') and f not in ('pins.toml', '_group.toml'):
            files[f[:-5]] = os.path.relpath(os.path.join(d, f), OUT)
built = json.load(open(os.path.join(args.src, 'built.json')))
by_name = {}
for r in built['rows'] + built['skipped']:
    by_name[(r[0], r[1])] = r[2]


def ID(name, parent=''):
    return by_name[(name, parent)]


def folder_of(parent):
    return '/'.join(slug(p) for p in parent.split('/')) if parent else ''


R = {}
for id, rel in files.items():
    text = open(os.path.join(OUT, rel)).read()
    t = tomllib.loads(text)
    R[id] = {'head': [l for l in text.splitlines() if l.startswith('#')], 'name': t['name'], 'outline': t.get('outline', []),
             'minus': t.get('minus', []), 'intersect': t.get('intersect', []), 'water': [], 'hand_out': None,
             'lines': t.get('lines', {}), 'path': rel}


def new(id, name, parent, head):
    path = (folder_of(parent) + '/' if parent else '') + id + '.toml'
    files[id] = path
    R[id] = {'head': ['# ' + head] if head else [], 'name': name, 'outline': [], 'minus': [], 'intersect': [], 'water': [],
             'hand_out': None, 'lines': {}, 'path': path}


for r in built['skipped']:
    if r[2] not in R:
        new(r[2], r[0], r[1], r[4] or r[3])

# ---- the zones ------------------------------------------------------------------------------------
Z = {}
for ft in json.load(open(args.eez))['features']:
    p = ft['properties']
    xs, ys = [], []
    g = ft['geometry']
    for poly in (g['coordinates'] if g['type'] == 'MultiPolygon' else [g['coordinates']]):
        for ring in poly:
            for x, y in ring:
                xs.append(x)
                ys.append(y)
    Z[p['mrgid']] = {'type': p['pol_type'], 'name': p['geoname'], 'ter': p['territory1'], 'km2': p['area_km2'],
                     'box': [min(xs), min(ys), max(xs), max(ys)]}

# The dataset's territory names -> the list's.
ALIAS = {
    'Ascension': 'Ascension Island', 'Pitcairn': 'Pitcairn Islands', 'Islas San Félix and San Ambrosio': 'Chile',
    'Easter Island': 'Chile', 'Trindade': 'Espírito Santo', 'Phoenix Group': 'Phoenix Islands',
    'Amsterdam and Saint Paul Islands': 'Saint-Paul and Amsterdam Islands', 'Cocos Islands': 'Cocos (Keeling) Islands',
    'Comores': 'Comoros', 'Heard and McDonald Islands': 'Heard Island and McDonald Islands', 'Kerguélen': 'Kerguelen Islands',
    'Republic of Mauritius': 'Mauritius', 'Oecusse': 'Timor-Leste', 'Republic of the Congo': 'Congo',
    'Prince Edward Islands': 'Western Cape', 'Europa Island': 'Scattered Islands', 'Democratic Republic of the Congo': 'DR Congo',
    'Macquarie Island': 'Tasmania', 'Juan de Nova Island': 'Scattered Islands', 'Bassas da India': 'Scattered Islands',
    'East Timor': 'Timor-Leste', 'Cape Verde': 'Cabo Verde', 'Faeroe': 'Faroe Islands', 'Saint-Barthélemy': 'Saint Barthélemy',
    'Sint-Eustatius': 'Sint Eustatius', 'Collectivity of Saint Martin': 'Saint Martin', 'Saint-Pierre and Miquelon': 'Saint Pierre and Miquelon',
    'Sint-Maarten': 'Sint Maarten', 'United States Virgin Islands': 'U.S. Virgin Islands', 'Ivory Coast': "Côte d'Ivoire",
    'Serrana Bank': 'Archipelago of San Andrés, Providencia and Santa Catalina',
    'Quitasueño Bank': 'Archipelago of San Andrés, Providencia and Santa Catalina',
    'Serranilla Bank': 'Archipelago of San Andrés, Providencia and Santa Catalina',
    'Bajo Nuevo Bank': 'Archipelago of San Andrés, Providencia and Santa Catalina',
    'Andaman and Nicobar': 'Andaman and Nicobar Islands', 'Turkey': 'Türkiye', 'Line Group': 'Line Islands', 'Galapagos': 'Ecuador',
    'Federal Republic of Somalia': 'Somalia', 'Chagos Archipelago': 'British Indian Ocean Territory',
}
LIST = json.load(open(os.path.join(args.src, 'regions.json')))
leaf_parent = {}
for e in LIST:
    leaf_parent.setdefault(e['name'], []).append('/'.join(e['path']))
groupings = set('/'.join(e['path'][:k + 1]) for e in LIST for k in range(len(e['path'])))
gname = {g.split('/')[-1]: g for g in groupings}
# "In France, Portugal and Spain, the rest is the mainland region's"; a grouping without a waters
# region gives its zone to its region holding the coast.
MAINLAND = {'France': 'metropolitan-france', 'Portugal': 'mainland-portugal', 'Spain': 'mainland-spain', 'Norway': 'mainland-norway',
            'United Kingdom': 'great-britain-and-northern-ireland', 'New Zealand': 'new-zealand-proper', 'Danish Realm': 'denmark',
            'Kingdom of the Netherlands': 'netherlands', 'DR Congo': 'kongo-central', 'Sudan': 'red-sea'}
WATERS = {parent: id for (name, parent), id in by_name.items() if name.endswith(' waters') and parent}


def target(ter):
    n = ALIAS.get(ter, ter)
    if n in gname:
        g = gname[n]
        if g in WATERS:
            return ('group', g)
        if n in MAINLAND:
            return MAINLAND[n]
        return ('group?', g)
    if n in leaf_parent:
        return ID(n, leaf_parent[n][0])
    return None


def W(id, part):
    R[id]['water'].append(part)


group_zones = {}


def zone_to(who, m):
    if isinstance(who, tuple):
        group_zones.setdefault(who[1], []).append(f'eez:{m}')
    else:
        W(who, f'eez:{m}')
    notes['zones'][m] = str(who)


def land(*ids):
    return [f'land:{i}' for i in ids]


def nearest(of, to, than):
    return {'nearest': of, 'to': to, 'than': than}


SPECIAL = set()


def split(m, parts):
    """A zone split by nearest coast: [(region or ('group', path), [its lands], [the others'])]."""
    SPECIAL.add(m)
    for who, mine, others in parts:
        p = nearest(f'eez:{m}', land(*mine), land(*others))
        if isinstance(who, tuple):
            group_zones.setdefault(who[1], []).append(p)
        else:
            W(who, p)
    notes['zones'][m] = 'split: ' + ', '.join(str(p[0]) for p in parts)


def give(m, who):
    SPECIAL.add(m)
    zone_to(who, m)


def own(m, id, name, head):
    SPECIAL.add(m)
    if id not in R:
        new(id, name, '', head)
        notes['added'].append({'id': id, 'name': name, 'zone': m, 'why': head})
    W(id, f'eez:{m}')
    notes['zones'][m] = id


# ---- the land the splits read must not overlap (#420's outlines took in the other's land) ------------
for id, ms in {'cyprus': ['region:northern-cyprus'], 'somalia': ['region:somaliland'],
               'georgia-country': ['region:abkhazia', 'region:south-ossetia'], 'morocco': ['region:western-sahara'],
               'mainland-finland': ['region:aland']}.items():
    R[id]['minus'] += [x for x in ms if x not in R[id]['minus']]

# ---- "Where the land splits one zone's coast between regions of different places" -------------------
cy, ncy, akd = ID('Cyprus'), ID('Northern Cyprus'), ID('Akrotiri and Dhekelia', 'United Kingdom')
SPECIAL.add(8376)
W(akd, {'intersect': [{'buffer': f'land:{akd}', 'by': '3 nmi'}, 'eez:8376']})
W(cy, {'minus': [nearest('eez:8376', land(cy, akd), land(ncy)), f'region:{akd}']})
W(ncy, {'minus': [nearest('eez:8376', land(ncy), land(cy, akd)), f'region:{akd}']})
notes['zones'][8376] = 'split: cyprus, northern-cyprus; akrotiri-and-dhekelia 3 nmi'
ge, abk = ID('Georgia'), ID('Abkhazia')
split(5678, [(ge, [ge], [abk]), (abk, [abk], [ge])])
so, sol = ID('Somalia'), ID('Somaliland')
split(8350, [(so, [so], [sol]), (sol, [sol], [so])])
fi, ax = ID('Mainland Finland', 'Finland'), ID('Åland', 'Finland')
split(5676, [(fi, [fi], [ax]), (ax, [ax], [fi])])
MOI = 'United States/U.S. Minor Outlying Islands'
hi, mi = ID('Hawaii', 'United States'), ID('Midway Atoll', MOI)
split(8453, [(hi, [hi], [mi]), (mi, [mi], [hi])])
ba, ho = ID('Baker Island', MOI), ID('Howland Island', MOI)
split(8451, [(ba, [ba], [ho]), (ho, [ho], [ba])])
pa, ki = ID('Palmyra Atoll', MOI), ID('Kingman Reef', MOI)
split(8443, [(pa, [pa], [ki]), (ki, [ki], [pa])])
ma, ws, dn = ID('Morocco'), ID('Western Sahara'), ID('Dakhlet Nouadhibou', 'Mauritania')
split(8368, [(ma, [ma], [ws, dn]), (ws, [ws], [ma, dn]), (('group', 'Mauritania'), [dn], [ma, ws])])
my, sg = ID('Malaysia'), ID('Singapore')
split(64440, [(my, [my], [sg]), (sg, [sg], [my])])
cr, ec = ID('Costa Rica'), ID('Ecuador')
split(48968, [(cr, [cr], [ec]), (ec, [ec], [cr])])
for id in (cr, ec):
    approx(id, 'the Costa Rica–Ecuador joint zone split by nearest coast (Cocos Island against the Galápagos), standing in for the 2014 treaty line')
# "the Sea of Azov and the Kerch Strait are Russia's"
ua, crm, sev = ID('Ukraine'), ID('Crimea', 'Russia'), ID('Sevastopol', 'Russia')
AZOV = {'intersect': [{'union': ['box:34.8,45.25,39.5,47.4', 'box:36.3,44.95,36.8,45.25']}, {'union': ['eez:5695', 'eez:5690']}]}
SPECIAL.add(5695)
W(ua, {'minus': [nearest('eez:5695', land(ua), land(crm, sev)), AZOV]})
group_zones.setdefault('Russia', []).extend([{'minus': [nearest('eez:5695', land(crm, sev), land(ua)), AZOV]}, AZOV])
approx(ua, 'the Sea of Azov and the Kerch Strait drawn as two boxes (34.8–39.5°E 45.25–47.4°N; 36.3–36.8°E 44.95–45.25°N) within the zones')
notes['zones'][5695] = 'split: ukraine; russian-waters (Crimea, Sevastopol, Azov)'
# "the Yellow Sea line is the Northern Limit Line ..., then due west along 38°03'N"
nk, sk = ID('North Korea'), ID('South Korea')
NLL = [[127.2, 37.72], [126.1, 37.72], [125.85, 37.75], [125.6, 37.68], [125.3, 37.68], [125.1, 37.78], [124.85, 37.87],
       [124.6, 37.95], [124.6, 38.05], [121.0, 38.05]]
for id in (nk, sk):
    R[id]['lines']['nll'] = {'coords': NLL}
    approx(id, "the Northern Limit Line drawn by hand from its published turning points, not OSM's ways")
SPECIAL.add(8328)
W(nk, {'cut': 'eez:8328', 'by': 'line:nll', 'keep': 'right'})
W(sk, {'cut': 'eez:8328', 'by': 'line:nll', 'keep': 'left'})
notes['zones'][8328] = 'cut at the NLL: north-korea, south-korea'
# "Gaza's waters are Israel's"
give(48953, ID('Israel'))
# Claimed and joint zones, as each region's note says.
GIVE = {8389: ID('Falkland Islands', 'United Kingdom'), 48944: ID('Mayotte', 'France'),
        48946: ID('Scattered Islands', 'France/French Southern Lands'), 48945: ID('Scattered Islands', 'France/French Southern Lands'),
        48948: ID('New Caledonia', 'France'), 8365: ID('Gibraltar', 'United Kingdom'), 48951: ID('Navassa Island', MOI),
        26582: ID('Egypt'), 49000: ID('Mainland Spain', 'Spain'), 49002: ID('Mainland Spain', 'Spain'), 49001: ID('Mainland Spain', 'Spain'),
        48999: ID('Mainland Spain', 'Spain'), 48997: ID('Mainland Spain', 'Spain'), 48998: ID('Perejil Island', 'Spain'),
        48950: ('group', 'Russia'), 48947: ('group', 'Iran'), 48956: ID('Djibouti'), 48954: ID('Japan'), 8319: ID('Wake Island', MOI),
        8321: ID('Taiwan'), 48977: ID('Mainland Norway', 'Norway'), 49003: ('group', 'China'), 48955: ID('South Korea'),
        48978: ID('Alaska', 'United States'), 8383: ID('South Georgia and the South Sandwich Islands', 'United Kingdom'),
        64431: ID('Belize'), 64459: ('group', 'Russia')}
for m, who in GIVE.items():
    give(m, who)
for m, name in {48961: 'Argentina–Uruguay Common Fishing Zone', 21798: 'Torres Strait Treaty joint area',
                21797: 'Nigeria–São Tomé and Príncipe Joint Development Zone', 48969: 'Barbados–Guyana Co-operation Zone',
                48971: 'Colombia–Dominican Republic–Venezuela overlap', 48965: 'Venezuela–Aruba–Dominican Republic overlap',
                48967: 'UK–Faroe Islands special area', 48943: 'Beaufort Sea wedge', 33185: 'Guyana–Trinidad and Tobago–Venezuela overlap',
                21792: 'Jamaica–Colombia Joint Regime Area', 48972: 'Honduras–Cayman Islands joint area', 48975: 'Iceland–Jan Mayen joint area',
                48982: 'Puerto Rico–Dominican Republic overlap', 21796: 'Japan–South Korea Joint Development Zone',
                50167: 'Croatia–Slovenia joint area', 50170: 'Qatar–Saudi Arabia–UAE overlap', 64460: 'Iraq–Iran overlap',
                64430: 'Egypt–Libya overlap'}.items():
    own(m, ID(name), name, '')
# "any other joint zone is its own region" (the list has no line for these).
for m, (id, name) in {48962: ('peru-ecuador-joint-regime-area', 'Peru–Ecuador joint regime area'),
                      48970: ('dominican-republic-colombia-joint-regime-area', 'Dominican Republic–Colombia joint regime area'),
                      48964: ('senegal-guinea-bissau-joint-regime-area', 'Senegal–Guinea-Bissau joint regime area'),
                      48966: ('spain-france-joint-regime-area', 'Spain–France joint regime area'),
                      48976: ('france-italy-joint-regime-area', 'France–Italy joint regime area'),
                      48973: ('iceland-faroe-islands-joint-regime-area', 'Iceland–Faroe Islands joint regime area'),
                      48974: ('ecuador-colombia-joint-regime-area', 'Ecuador–Colombia joint regime area')}.items():
    own(m, id, name, f'the zone "{Z[m]["name"]}", about {Z[m]["km2"]:,.0f} km²: a joint zone the list has no line for, its own region by the list\'s rule')
# "Scarborough Shoal's 12 nautical miles are China's"
SCAR = {'buffer': 'box:117.70,15.08,117.85,15.25', 'by': '12 nmi'}
group_zones.setdefault('China', []).append(SCAR)
cw = ID('Chinese waters', 'China')
approx(cw, "Scarborough Shoal's 12 nmi buffered from a box round the reef (no OSM feature named)")
approx(cw, 'the 12 nmi rings round the Spratly features others hold in the South China Sea overlap are not drawn: their OSM features are not looked up')
for c in ('Vietnam', 'Philippines', 'Malaysia', 'Taiwan'):
    approx(ID(c), 'the 12 nmi rings round the Spratly features it holds are not drawn: their OSM features are not looked up')

# Every other 200NM zone to the region holding its coast.
for m, z in sorted(Z.items()):
    if m in SPECIAL:
        continue
    t = target(z['ter']) if z['type'] == '200NM' else None
    if t is None or (isinstance(t, tuple) and t[0] == 'group?'):
        notes['unassigned_zones'].append((m, z['name']))
        continue
    zone_to(t, m)
ph = ID('Philippines')
R[ph]['water'] = [{'minus': [p, SCAR]} for p in R[ph]['water']]


# ---- "In a place split into regions, a region's water is what clearly belongs to it" ----------------
def osm_of(id):
    return [p for p in R[id]['outline'] if isinstance(p, str) and p.startswith('osm:')]


def members(g):
    out = []
    for e in LIST:
        k = (e['name'], '/'.join(e['path']))
        if '/'.join(e['path']).startswith(g) and k in by_name and not e['name'].endswith(' waters'):
            out.append(by_name[k])
    return out


# U.S. states, Australian states and territories, Argentine provinces: their OSM outlines at sea.
for g in ('United States', 'Australia', 'Argentina'):
    zs = [p for p in group_zones.get(g, []) if isinstance(p, str)]
    for id in members(g):
        if '/' in R[id]['path'].replace(folder_of(g) + '/', '', 1):
            continue
        if not R[id]['water'] and osm_of(id) and zs:
            o = osm_of(id)
            W(id, {'intersect': [{'union': o} if len(o) > 1 else o[0], {'union': zs} if len(zs) > 1 else zs[0]]})
# Hong Kong's and Macau's waters: their OSM outlines at sea.
for n in ('Hong Kong', 'Macau'):
    id = ID(n, 'China')
    W(id, {'intersect': [osm_of(id)[0], 'eez:8486']})
# Indonesia's geographical units: up to 12 nmi from their own coasts.
units = members('Indonesia')
for id in units:
    W(id, {'intersect': [nearest({'buffer': f'land:{id}', 'by': '12 nmi'}, land(id), land(*[u for u in units if u != id])), 'eez:8492']})
# "The rest of the place's water is the place's own waters region"; a stretch no other region of
# the place borders is the one region's (hand_out).
for g, wid in WATERS.items():
    zs = group_zones.get(g, [])
    if not zs:
        # (Each of its regions has its own zone: nothing is left.)
        notes['removed'].append({'id': wid, 'why': 'every region of its place has its own zone: it would be empty'})
        del R[wid]
        continue
    W(wid, {'minus': [{'union': zs} if len(zs) > 1 else zs[0], f'group:{folder_of(g)}']})
    R[wid]['hand_out'] = folder_of(g)
for g in group_zones:
    if g not in WATERS:
        notes['unassigned_zones'].append((g, str(group_zones[g])))

# ---- lakes ------------------------------------------------------------------------------------------
# "the main basin of Lake Constance is its own region" (east of the line from the Bodanrück to the
# north shore); the Überlinger See Germany's.
LC = 'lake-constance'
if LC not in R:
    new(LC, 'Lake Constance', '', "the main basin (Obersee), from Bregenz to the Überlinger See's mouth")
MOUTH = {'coords': [[9.15, 47.655], [9.29, 47.773]]}
R[LC]['lines']['mouth'] = MOUTH
R[LC]['water'] = [{'cut': 'ne-lake:1159114259', 'by': 'line:mouth', 'keep': 'right'}]
de = ID('Germany')
R[de]['lines']['mouth'] = MOUTH
W(de, {'cut': 'ne-lake:1159114259', 'by': 'line:mouth', 'keep': 'left'})
approx(LC, "Natural Earth's Bodensee polygon; if it holds the Untersee, that falls to Germany rather than land")
# "Lake Malawi has no Tanzanian part": Malawi's but for Mozambique's part.
W(ID('Malawi'), {'minus': ['ne-lake:1159113163', 'osm:195273']})

# ---- the high seas, Antarctica ---------------------------------------------------------------------
R[ID('High seas')]['water'] = ['rest']
an = ID('Antarctica')
# (All land south of 60°S, by the list's words: the OSM relation stops short of the pole and leaves
# out land the coastline has, which no other region takes.)
R[an].update(outline=['box:-180,-90,180,-60'], minus=[], intersect=[])
W(an, {'minus': [{'buffer': f'land:{an}', 'by': '200 nmi'}] + [f'eez:{m}' for m, z in sorted(Z.items()) if z['box'][1] < -45]})

# ---- land the #420 files couldn't say ------------------------------------------------------------------
ko = ID('Koalou')
R[ko]['outline'] = ['box:0.85,10.95,1.15,11.20']
R[ko]['minus'] = ['osm:192784', 'osm:192783', 'osm:192782']  # Benin, Burkina Faso, Togo
approx(ko, 'the land in a box round the tripoint (0.85–1.15°E, 10.95–11.20°N) that the outlines of Benin, Burkina Faso and Togo leave out')
R[ID('Pheasant Island')]['outline'] = ['osm-way:177945817']
rs = ID('Red Sea', 'Sudan')
R[rs]['outline'] = ['osm:3774672']
R[rs]['head'] = ['# Red Sea State (OSM relation 3774672, SD-RS)']
icef = ID('Southern Patagonian Ice Field')
R[icef].update(outline=[{'intersect': ['osm:286393', 'osm:167454', 'box:-74.5,-49.8,-72.0,-49.2']}], minus=[], intersect=[])
ab = ID('Abyei Area')
R[ab].update(outline=[{'intersect': ['osm:192789', 'osm:1656678', 'box:27.0,8.5,30.0,10.8']}], minus=[], intersect=[])
approx(ab, 'the overlap of the two borders taken within a box round Abyei (27–30°E, 8.5–10.8°N)')


# ---- write ----------------------------------------------------------------------------------------------
def tomlv(v):
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return '[' + ', '.join(tomlv(x) for x in v) + ']'
    if isinstance(v, dict):
        return '{ ' + ', '.join(f'{k} = {tomlv(x)}' for k, x in v.items()) + ' }'
    return json.dumps(v)


for d, _, fs in os.walk(OUT):
    for f in fs:
        if f.endswith('.toml') and f not in ('pins.toml', '_group.toml') and f[:-5] not in R:
            os.remove(os.path.join(d, f))
for id, r in R.items():
    out = list(r['head']) + [f'id = {tomlv(id)}', f'name = {tomlv(r["name"])}']
    for k in ('outline', 'minus', 'intersect'):
        if r[k]:
            out.append(f'{k} = {tomlv(r[k])}')
    if r['water']:
        out += ['water = ['] + [f'  {tomlv(p)},' for p in r['water']] + [']']
    else:
        out.append('water = []')
    if r['hand_out']:
        out.append(f'hand_out = {tomlv(r["hand_out"])}')
    for ln, l in r['lines'].items():
        out.append(f'[lines.{ln}]')
        out += [f'{k} = {tomlv(v)}' for k, v in l.items()]
    p = os.path.join(OUT, r['path'])
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, 'w').write('\n'.join(out) + '\n')
if not os.path.exists(os.path.join(OUT, 'pins.toml')):
    open(os.path.join(OUT, 'pins.toml'), 'w').write('osm = "2026-09-28"\n')
json.dump(notes, open(os.path.join(args.out, 'gen-notes.json'), 'w'), indent=1, ensure_ascii=False, default=str)
print(len(R), 'regions;', len(notes['added']), 'added;', len(notes['removed']), 'removed;', len(notes['unassigned_zones']), 'zones unassigned')
for u in notes['unassigned_zones']:
    print('  unassigned', u)
