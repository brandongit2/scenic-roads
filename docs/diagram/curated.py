"""The curated inputs: what a person or an AI agent made by hand (research, curation, agents' writing, hand downloads,
hand-run steps, constants tuned by hand), not a deterministic job. For each: what it feeds, who or what makes it, where
it lives, where its method is written down, and what making it again would take today. The owner's starting point for
standardising them (9 Oct 2026). Kept to what's true on main; plan.md §6 "Hand-made inputs" says how each is made."""
from html import escape as E

import diag

# (reproducibility, its words or None for REPRO's, class, name, feeds, made by, lives in, written down, to make it again)
GROUPS = [
    ('Written by Claude agents', [
        ('part', None, 'base', 'Place-name translations (today’s)',
         'English everywhere: labels, basemap names, roads, rail, popups (the servers, names::table)',
         'the earlier translation work (its own repository): word rules, romanisation, Claude agents (lines tagged agent:sonnet); converted once by tools/names/convert-tables.py',
         'NAS translations/0-converted/ (2.78 M lines); the area tables it came from in translations/<area>/',
         'plan.md §7 (the conversion, its log); the rules and the agents’ brief only in the other repository',
         'that repository’s rules and brief, or translating again from the to-do lists'),
        ('doc', None, 'base', 'New translations',
         'as above, within a minute or two of a drop',
         'Claude agents on request (Haiku, per plan.md §7), from the names-todo job’s lists; a pilot, then checks',
         'NAS translations/ (any folder but todo/)',
         'tools/names/translations-todo.md (the brief, as translations/todo/README.md) and check.py; not the model, batch sizes or the pilot',
         'agents on translations/todo/*.jsonl with the brief, then check.py'),
        ('part', None, 'base', 'Descriptions written so far',
         'popups of landmarks, heritage sites and areas',
         'Sonnet agents in four rounds (a pilot, fixes, a second voice, researched), on targets chosen by fame',
         'NAS descriptions/heritage/',
         'each line credits its sources (title, URL); the briefs (WRITERS, FIXERS, RESEARCH) in inputs/descriptions/briefs-2026-09/ (plan.md §6); not the targets’ rule or the checks',
         'the new brief on the to-do list; the same set again: the old briefs, and the extracts in the registers snapshot’s desc/'),
        ('doc', None, 'base', 'New descriptions',
         'as above',
         'Claude agents on request (Sonnet, per plan.md §7), from descriptions/todo/',
         'NAS descriptions/',
         'tools/names/descriptions-todo.md (the brief: 55 words, sources credited); the model only in plan.md',
         'agents on descriptions/todo/*.jsonl with the brief'),
        ('part', None, 'net', 'Ferry timetables looked up',
         'the ferries job: sailings a day where no feed has them',
         'Claude research agents, with a written prompt (the model not recorded)',
         'NAS inputs/ferries/freq/timetables-*.json',
         'plan.md §6; each entry’s page and the month it was checked; the prompt, batches and not-found lists in inputs/ferries/research/; not the model; entries keyed by OSM ids that may drift',
         'agents with that prompt over the lines with no feed'),
        ('doc', None, 'net', 'Ferry operators’ GTFS, counted',
         'the ferries job: sailings a day (40 operators)',
         'a feed list an agent picked (2026-09-28), counted by dem/gtfs.py run by hand',
         'NAS inputs/ferries/freq/gtfs-*.json',
         'plan.md §6 (the command); the verified feed list, how it was found and the zips counted, in inputs/ferries/; each entry’s URL and date window',
         'gtfs.py by hand, as written (no job runs it)'),
        ('part', None, 'net', 'The MTR’s lines',
         'trains a day in Hong Kong (rail, from sources/rail/mtr-pairs)',
         'Claude research on mtr.com.hk; dem/mtrpairs.py and scenic-build put, by hand',
         'NAS sources/rail/mtr, mtr-pairs',
         'plan.md §6 (the commands); each line’s source and month; no research brief',
         'the research again, then mtrpairs.py (it needs a Hong Kong stations extract) and put'),
    ]),
    ('Chosen, researched or downloaded by hand', [
        ('doc', None, 'osm', 'Region recipes',
         'the coverage: what every step builds',
         'you, in the Regions panel (or scenic add): a list of OSM boundaries each',
         'NAS inputs/regions/ (88; backed up daily)',
         'plan.md §5',
         'nothing: they are the definition'),
        ('part', None, 'place', 'Heritage registers snapshot',
         'heritage sites and chain: heritage landmarks, the area overlays, the areas’ flags',
         'hand downloads (Canada’s federal list, Quebec’s, Ontario’s, Nova Scotia’s, New Brunswick’s registers …), dem/federal.py, crhp.py and heritage.py’s fetches; imported by registers-import',
         'NAS sources/registers/legacy (274 MB), legacy-seeds (8 MB: park facts, pageview months, English names)',
         'plan.md §6 (the folder, federal.py, crhp.py, registers-import); which register per jurisdiction and their licences: README’s table; no download URL or date per source; the seeds’ makers gone',
         'the downloads again, then federal.py, crhp.py and registers-import as written; the seeds can’t be made again'),
        ('part', None, 'base', 'Planetiler’s pins',
         'the basemap, and from it the water, the terrain’s water and labels',
         'downloaded by hand: the jar (0.10.2), Natural Earth, the water polygons, lake centrelines',
         'NAS sources/basemap/',
         'plan.md §6 (the pins; a new jar waits for the shoreline check); no URL or date for the data',
         'the same versions fetched again, or new ones checked by tools/coastcheck'),
        ('part', None, 'terr', 'Which DEM where',
         'roads’ elevations (elev): HRDEM, 3DEP, MRDEM in North America, GSI in Japan, FABDEM elsewhere',
         'chosen by hand (pipeline::rules); HRDEM’s tile lists committed once',
         'rules.rs; dem/hrdem_2m_tiles.txt, hrdem_tile_index.geojson',
         'README’s table; the tile lists’ origin not recorded. Taiwan’s MOI DTM was never put in inputs/moi-dtm/ (403): FABDEM serves Taiwan',
         'the order is in code; the tile lists from NRCan again'),
        ('doc', None, 'base', 'Languages by territory',
         'the spoken job: which languages a name is looked up in, and which to-do list it goes on',
         'tools/names/cldr-languages.py from CLDR 47; refinements by hand (spoken::REFINED)',
         'crates/names/src/territory-languages.tsv, spoken.rs',
         'plan.md §7 (not CLDR’s download URL)',
         'the script on CLDR 47’s territoryInfo.json'),
        ('doc', None, 'net', 'Rail feeds’ hand lists',
         'rail-feeds: national operators the catalogue lacks, feeds filed under another country, replacements, rail::OWN_CODE',
         'chosen by hand, in code',
         'dem/railfeeds.py, pipeline::rail',
         'plan.md §6 Rail service, with the URLs in the code',
         'nothing: in the code'),
        ('doc', None, 'net', 'The rail feeds’ catalogue',
         'rail-feeds: which feeds there are (1,545 checked)',
         'the Mobility Database’s list, downloaded once and put on the NAS by hand',
         'NAS sources/rail/',
         'plan.md §6 (the put); fetching it again every ~6 months is planned',
         'the catalogue downloaded again'),
        ('part', None, 'net', 'Keys',
         'rail-feeds (Singapore’s LTA); ODPT and TDX planned',
         'you: accounts made by hand',
         'NAS inputs/keys.env',
         'which keys: plan.md §6; not how each was got',
         'the accounts again'),
        ('part', None, 'mix', 'The cut-over’s sign-off',
         'whether a held catalog went out',
         'a comparison judged by hand (its tool since deleted)',
         'NAS inputs/hold-catalog.compare-2026-10-04.md, …released-2026-10-04',
         'plan.md §8 (compared by hand); the criteria not written',
         'only for a future held catalog'),
    ]),
    ('Tuned by hand, in the code', [
        ('part', None, 'bldg', 'Storey heights and fills',
         '3D buildings’ heights where none is measured',
         'dem/bldmeasure.py run once (B0, 6 Oct) on the coverage; its fits copied into the code by hand',
         'pipeline::bld fill.rs (the tables); fit.json not kept',
         'docs/buildings3d.md §2.3',
         'bldmeasure.py again, then the tables by hand'),
        ('doc', None, 'terr', 'The terrain repair’s thresholds',
         'terrain and the worldwide z8',
         'tuned by hand on named places, checked by terrain --scan over the coverage',
         'roadcore grid.rs',
         'README “Terrain repair”, plan.md §6; the scans’ outputs not kept',
         'nothing: in the code'),
        ('doc', None, 'mix', 'Lists and weights in the code',
         'OSM filters and sets, road and rail selection, landmark kinds, tiers and fame, ferry groups, the to-do lists’ rules',
         'chosen by hand',
         'osmpass.rs, extract.rs, marks.rs, heritagetiers.py, interest.py, labels.py, ferries.py, namestodo.rs …',
         'README (road and rail selection, ferries), phase5.md, plan.md §7',
         'nothing: in the code'),
        ('none', None, 'scen', 'Scenic and ride presets',
         'the map’s weights for drives and rides, its colour ramps',
         'set by hand; ramps.json by dem/ramps.py run by hand',
         'web/src/data/weight-presets.json, rail-presets.json, ramps.json',
         'no rationale anywhere',
         'nothing to remake, but no reason to keep or change them is written'),
    ]),
    ('Frozen, or not used on main', [
        ('gone', None, 'mix', 'The old build’s converted files',
         'nothing live (catalog 17 serves the jobs’ own)',
         'the old pipeline (deleted)',
         'NAS global/legacy/ (38 files)',
         'being deleted with the old pipeline',
         'nothing: going'),
        ('gone', None, 'base', 'Old Haiku translations',
         'nothing',
         'the old dem/names.py translators',
         'NAS inputs/names/tr-2026-09/ (443 files, their brief)',
         'their brief beside them',
         'nothing: orphaned'),
        ('none', 'downloaded, no code on main', 'terr', 'Depths and Canada’s lakes',
         'nothing yet: the lab’s depth layers; Canada’s missing small lakes (#124)',
         'downloaded by hand: NCEI’s CRM volumes and Great Lakes, GEBCO 2026, CanVec’s hydro by province (robots.txt)',
         'NAS sources/ncei-crm/, ncei-greatlakes/, gebco/, canvec/50k/',
         'in the lab’s and #124’s notes, not on main',
         'the same downloads by hand'),
    ]),
]

CSS = """
.cur-list{margin:0 0 18px;padding:12px 14px;border:1px solid var(--rule);border-radius:10px;background:var(--surface)}
.cur-list>h3{font:600 11.5px/1 var(--sans);letter-spacing:.08em;color:var(--faint);text-transform:uppercase;margin:0 0 6px}
.cur-list>p{margin:0 0 10px;color:var(--muted);font-size:14px;max-width:1080px}
.cur-list h4{font:600 13.5px/1.2 var(--sans);color:var(--fg);margin:14px 0 4px}
.cur-row,.cur-head{display:grid;grid-template-columns:minmax(150px,1.05fr) 1.1fr 1.25fr 1fr 1.25fr 1fr;gap:4px 14px;padding:6px 0;border-top:1px solid var(--rule);font-size:13.5px;line-height:1.4;color:var(--muted)}
.cur-head{font:600 11px/1.2 var(--sans);letter-spacing:.06em;text-transform:uppercase;color:var(--faint);border-top:0;padding:2px 0 4px}
.cur-row b{display:flex;gap:7px;align-items:flex-start;font:600 14px/1.3 var(--sans);color:var(--fg)}
.cur-row b svg{flex:none;margin-top:1px}
.cur-row .v{display:block;font:400 12px/1.3 var(--mono);color:var(--muted);margin:2px 0 0 23px}
.cur-row code{font:400 12.5px var(--mono);color:var(--fg)}
.cur-row>span>i{display:none;font-style:normal;color:var(--faint);font:600 10.5px/1 var(--sans);letter-spacing:.06em;text-transform:uppercase;margin-right:6px}
.cur-row svg .rp{fill:var(--surface);stroke:var(--fg);stroke-width:1.1}
.cur-row svg .rp.full,.cur-row svg .rp-half{fill:var(--fg)}
.cur-row svg .rp-x{stroke:var(--fg);stroke-width:1.1;stroke-linecap:round}
.cur-row b .k{width:3px;align-self:stretch;border-radius:2px;background:var(--k);flex:none}
@media (max-width:900px){.cur-head{display:none}.cur-row{grid-template-columns:1fr;gap:3px}.cur-row>span>i{display:inline}}
"""

HEAD = ['Input', 'Feeds', 'Made by', 'Lives in', 'Written down', 'To make it again']


def html():
    rows = []
    for title, items in GROUPS:
        rows.append(f'<h4>{E(title)}</h4><div class="cur-head" aria-hidden="true">' + ''.join(f'<span>{E(h)}</span>' for h in HEAD) + '</div>')
        for lvl, words, k, name, feeds, by, where, doc, remake in items:
            mark = f'<svg width="13" height="13" aria-hidden="true">{diag.repro_mark(6.5, 6.5, lvl, r=5)}</svg>'
            cells = [feeds, by, where, doc, remake]
            rows.append(f'<div class="cur-row c-{k}"><span><b><span class="k"></span>{mark}{E(name)}</b>'
                        f'<span class="v">{E(words or diag.REPRO[lvl])}</span></span>'
                        + ''.join(f'<span><i>{E(h)}</i>{E(c)}</span>' for h, c in zip(HEAD[1:], cells)) + '</div>')
    n = sum(len(i) for _, i in GROUPS)
    counts = {lvl: sum(1 for _, i in GROUPS for r in i if r[0] == lvl) for lvl in diag.REPRO}
    summary = (f'{n} kinds: {counts["doc"]} with their method written down, {counts["part"]} partly, {counts["none"]} undocumented, '
               f'{counts["gone"]} frozen with their maker gone.')
    return ('<section class="cur-list" aria-label="Curated inputs"><h3>Curated inputs · made by hand</h3>'
            '<p>Everything a person or an AI agent made by hand rather than a job: what it feeds, who makes it, where it lives, '
            'where its method is written down and what making it again would take today. ' + E(summary) +
            ' “Being documented”: the makers kept from the old pipeline are being made runnable by hand and written up in '
            'plan.md’s hand-made inputs section.</p>' + ''.join(rows) + '</section>')
