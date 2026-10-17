"""The curated inputs: what a person or an AI agent made by hand (research, curation, agents' writing, hand downloads,
hand-run steps), not a deterministic job or configuration in the code. For each: what it feeds, who or what makes it, where
it lives, where its method is written down, and what making it again would take today. The owner's starting point for
standardising them (9 Oct 2026). Kept to what's true on main; each drop box's how/ (docs/inputs.md §8) and plan.md §6
"Hand-made inputs" say how each is made."""
from html import escape as E

import diag

# (reproducibility, its words or None for REPRO's, class, name, feeds, made by, lives in, written down, to make it again)
GROUPS = [
    ('Written by Claude agents', [
        ('part', None, 'base', 'Place-name translations (today’s)',
         'English everywhere: labels, basemap names, roads, rail, popups (the servers, names::table)',
         'the earlier translation work (its own repository): word rules, romanisation, Claude agents (lines tagged agent:sonnet); converted once (log in translations/how/conversion-log.txt)',
         'NAS translations/0-converted/ (2.78 M lines); the area tables it came from in translations/<area>/',
         'its how/: the conversion’s log, the first Haiku brief and launch prompt; the later session’s rules and brief only in the other repository',
         'that repository’s rules and brief, or translating again from the to-do lists'),
        ('doc', None, 'base', 'New translations',
         'as above, within a minute or two of a drop',
         'Claude agents on request (Haiku, per plan.md §7), from the names-todo job’s lists; a pilot, then checks',
         'NAS translations/ (any folder but todo/)',
         'tools/names/translations-todo.md (the brief, as translations/todo/README.md) and check.py; the model, batch sizes and pilot in its how/',
         'agents on translations/todo/*.jsonl with the brief, then check.py'),
        ('part', None, 'base', 'Descriptions written so far',
         'popups of landmarks, heritage sites and areas',
         'Sonnet agents in four rounds (a pilot, fixes, a second voice, researched), on targets chosen by fame',
         'NAS descriptions/heritage/',
         'each line credits its sources (title, URL); the briefs (WRITERS, FIXERS, RESEARCH) in its how/briefs-2026-09/; not the targets’ rule or the checks',
         'the new brief on the to-do list; the same set again: the old briefs, and the Wikipedia extracts in the heritage chain’s answers (desc/)'),
        ('doc', None, 'base', 'New descriptions',
         'as above',
         'Claude agents on request (Sonnet, per plan.md §7), from descriptions/todo/',
         'NAS descriptions/',
         'tools/names/descriptions-todo.md (the brief: 55 words, sources credited); the model in its how/',
         'agents on descriptions/todo/*.jsonl with the brief'),
        ('part', None, 'net', 'Ferry timetables looked up',
         'the ferries job: sailings a day where no feed has them',
         'Claude research agents, with a written prompt (the model not recorded); static GTFS since #141; the input check',
         'NAS inputs/timetables/gtfs/ferries-batch-*.zip',
         'docs/inputs.md §6.7.2; each route’s page and the month it was checked; the prompt, batches and not-found lists in its how/; not the model; routes pinned to OSM ids that may drift',
         'agents with that prompt over the lines with no feed'),
        ('doc', None, 'net', 'Ferry operators’ GTFS feeds',
         'the ferries job: sailings a day (40 operators), counted by dem/ferrygtfs.py',
         'a feed list an agent picked (2026-09-28), now declarations; the input check',
         'NAS inputs/timetables/feeds/*.toml (modes = ["ferry"])',
         'docs/inputs.md §6.7.1; how they were found in its how/; each declaration’s URL, licence and credit',
         'rail-feeds fetches them; the ferries job counts them'),
        ('part', None, 'net', 'The MTR’s lines',
         'trains a day in Hong Kong (the rail job counts its count rows)',
         'Claude research on mtr.com.hk; producers/timetables/mtr.py, by hand; the input check',
         'NAS inputs/timetables/gtfs/mtr.zip (the research in its how/mtr.json)',
         'its how/ and plan.md §6 (the commands); each line’s source and month; no research brief',
         'the research again, then mtr.py (it needs a Hong Kong stations extract) and drop the zip'),
    ]),
    ('Chosen, researched or downloaded by hand', [
        ('doc', None, 'osm', 'Region recipes',
         'the coverage: what every step builds',
         'you, as files in the drop box: OSM boundaries and shapes each',
         'NAS inputs/regions/ (88; backed up daily)',
         'plan.md §5; its how/',
         'nothing: they are the definition'),
        ('part', None, 'place', 'Heritage registers',
         'heritage sites and chain: heritage landmarks, the area overlays, the areas’ flags',
         'producers/heritage/registers.py from the registers’ downloads (Canada’s federal list located by federal.py, the CRHP read by crhp.py, each country’s and province’s open data), a GeoJSON file and a description per register',
         'NAS inputs/heritage/ (40 registers, through the input check), the owner’s matches in inputs/heritage-matches/',
         'inputs.md §6.5: each register’s credit, licence, source, download date and territory in its description; its how/ (the commands, the downloads’ cache); no download URL per file of a register',
         'the producers on the downloads (their data folder), dropped into the drop box'),
        ('part', None, 'base', 'The basemap’s pinned versions',
         'the basemap, and from it the water, the terrain’s water and labels',
         'downloaded by hand: the jar (0.10.2), Natural Earth, the water polygons, lake centrelines',
         'NAS sources/basemap/',
         'plan.md §6 (the pins; a new jar waits for the shoreline check); no URL or date for the data',
         'the same versions fetched again, or new ones checked by tools/coastcheck'),
        ('part', None, 'terr', 'Which DEM where',
         'roads’ elevations (elev): HRDEM, 3DEP, MRDEM in North America, GSI in Japan, FABDEM elsewhere',
         'declared in inputs/elevation/ (one .toml per dataset; the order by rank and resolution read from the data), checked by the input check',
         'the declarations; indexes by producers/elevation/indexes.py from the sources’ listings',
         'docs/inputs.md §6.8; its how/ (the indexes’ commands)',
         'the indexes made again from the listings'),
        ('doc', None, 'base', 'Languages by territory',
         'the spoken job: which languages a name is looked up in, and which to-do list it goes on',
         'producers/languages/cldr-languages.py from CLDR 47, refinements folded in; the input check',
         'inputs/languages/ (territories.tsv, about.toml)',
         'its how/ (the commands; CLDR 47’s package, not its download URL)',
         'the script on CLDR 47’s territoryInfo.json'),
        ('doc', None, 'net', 'Rail feeds’ declarations',
         'rail-feeds: national operators the catalogue lacks, feeds behind a key, the catalogue’s corrections and own codes',
         'chosen by hand, as declarations; the input check',
         'NAS inputs/timetables/feeds/*.toml',
         'docs/inputs.md §6.7.1, each declaration with its URL, licence and credit; its how/',
         'rail-feeds reads the accepted version'),
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
    ]),
    ('Being deleted', [
        ('gone', None, 'mix', 'The old build’s converted files',
         'nothing live (catalog 17 serves the jobs’ own)',
         'the old pipeline (deleted)',
         'NAS global/legacy/ (38 files)',
         'being deleted with the old pipeline',
         'nothing: going'),
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
            ' Each drop box’s how/ on the NAS says how its input is made, pointing to its producers in the repository '
            '(producers/).</p>' + ''.join(rows) + '</section>')
