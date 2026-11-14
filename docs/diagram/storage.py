"""Where the data lives: the NAS holds the bulk data, the lead (the Mac mini) keeps the pool's state, the records and
the map on its own disk, each Mac's agent keeps its own work until the lead has it, and devices only read.
Each label's source is cited where it is written; paths under "its own disk" are under the app's folder
(~/Library/Application Support/scenic/)."""
from diag import W, Fig


def build(check=False):
    f = Fig('s', check)
    top = 16

    # The sources declared in the NAS's inputs/sources/*.toml (docs/inputs.md §6.9; each fetched under sources/ or
    # sources/fetched/) and docs/storage.md's tree (aws-terrarium, pyramids: GSI's tiles, fetched/glo30, rail, basemap);
    # docs/macs.md, "Downloads are kept on the NAS, each made once".
    src_t = ('the OSM planet; elevation (AWS’s terrain tiles, GLO-30, GSI’s tiles) and geoid models; depths (GEBCO, '
             'EMODnet, BlueTopo, NONNA, NCEI’s models, lakes’); land cover, canopy heights, leaf type; Overture’s '
             'buildings, GHSL’s heights; Wikidata, Wikipedia, its pageviews; rail feeds; lakes, sea boundaries; '
             'the basemap’s sources')
    # docs/storage.md's tree, folder by folder; state/: also docs/coordination.md §3.2 (the token, the history's
    # archive), crates/agentcore/src/agent/lead.rs (state/build/lead-member.json), crates/agentcore/src/pool/records.rs
    # (state/build/records.json, written whole), crates/agentcore/src/agent/live.rs (ARCHIVE = state/live-archive).
    nas_rows = [
        ('inputs/', 'the inputs folders: regions, languages, translations, descriptions, heritage, timetables, elevation, sources; the keys'),
        ('sources/', 'the OSM planet and its pass; every download, made once; the input check’s checked copies'),
        ('base/ · hidata/', 'per z6 tile: the base packs; query parts, rail lines, zoomed-out summaries'),
        ('markdata/ · ovdata/', 'landmark points per z6 tile, in one bundle; area and park details per z3 tile'),
        ('global/', 'worldwide files: road values, rail frequencies, landmark totals, heritage, the languages spoken'),
        ('layers/', 'what the map draws: root, lo and hi packs, or one bundle a small layer; the basemap'),
        ('work/ · cache/', 'build intermediates, not served; what z6 tiles keep for their later runs'),
        ('state/', 'the lead’s declaration; the records’ snapshot and its brief; the pool’s token; the archives '
                   '(history, forecasts, live measures); backups')]
    # agent/pool/records.json: crates/agentcore/src/agent/pool.rs (RECORDS), docs/pool.md §6.2, docs/storage.md
    # (the build manifest, job keys, unverified uploads, sizes). agent/coord/store/: docs/coordination.md §4.1.
    # agent/maplog/: crates/agentcore/src/agent/publish.rs (MAPLOG), docs/formats.md "The lead's store" (gc.jsonl there).
    # agent/coord/history.jsonl: docs/pool.md §7.5, crates/agentcore/src/coord/history.rs (a week).
    # app/releases/: crates/jobrt/src/appupdate.rs (RELEASES), docs/coordinator.md §10.
    # mirror/ · idx/: docs/macs.md, "Mirror, the Mac mini's" (only what's downloaded; pack indexes cached).
    lead_rows = [
        ('agent/pool/records.json', 'the records, the truth: the build manifest, the job keys, the unverified uploads, the files’ sizes'),
        ('agent/coord/store/', 'the pool’s state, a snapshot and a log: leases, the members’ beats, hand-offs taken, holds, settings'),
        ('agent/maplog/', 'the map (every file the map server reads) and the log of its updates; GC’s queue'),
        ('agent/coord/history.jsonl', 'the week’s history'),
        ('app/releases/', 'the live app release and the one before'),
        ('mirror/ · idx/', 'the map server’s downloads (the World, regions) and its pack indexes')]
    # docs/macs.md: Caches (agent/cache: filled from the NAS), "The offline map, on demand"; docs/coordination.md §5.3
    # (agent/pool/: held leases, hand-offs until a saved records snapshot names them); docs/roles.md §8.1 (scratch).
    mac_t = ('its jobs’ scratch; caches filled from the NAS (agent/cache/); its held leases and its hand-offs, kept '
             'until a records snapshot the lead saved names them (agent/pool/); on a laptop, the offline map on demand: what it downloaded')
    # docs/storage.md, Writers (the owner's inputs folders); docs/inputs.md (the input check).
    hand_t = 'the owner and Claude agents: regions, translations, descriptions, registers, timetables, declarations'
    # docs/macs.md, Devices (the map's address, over the tailnet); docs/coordinator.md §7 (the page at the root).
    dev_t = ('the map, at its address on the tailnet; the build page, at the lead’s coordinator’s root; '
             'a phone, an iPad, the other Macs')

    sx, sw = 16, 180
    nx, nw = 286, 500
    lx, lw = 934, 300
    dx = 1300
    dw = W - 16 - dx
    hh = max(f.measure(nw, rows=nas_rows), f.measure(lw, rows=lead_rows))
    src = f.box(sx, top, sw, 'Online sources', src_t)
    nas = f.box(nx, top, nw, 'NAS · the bulk data', rows=nas_rows, cls='nas', h=hh)
    lead = f.box(lx, top, lw, 'The lead · the Mac mini’s own disk', rows=lead_rows, h=hh)
    dev = f.box(dx, top, dw, 'Devices', dev_t)

    by = top + hh + 58
    mx = nx + 96
    bh = max(f.measure(sw, hand_t), f.measure(nas.r - mx, mac_t))
    hand = f.box(sx, by, sw, 'Made by hand', hand_t, h=bh)
    mac = f.box(mx, by, nas.r - mx, 'Every Mac’s agent · its own disk', mac_t, h=bh)

    def gap_arrow(a, b, y, label, back=False):
        """An arrow across the gap between two boxes of the top row, its label above it."""
        p, q = ((b.x, y), (a.r, y)) if back else ((a.r, y), (b.x, y))
        f.arrow(p, q, label=label, at=((a.r + b.x) / 2, y - 8), anchor='middle')

    # docs/macs.md: each download made once, by a job (docs/steps.md §1: the planet by the lead's planet-fetch).
    gap_arrow(src, nas, top + 60, 'downloads')
    # docs/macs.md, Mirror: the map reads a file from the Mac when it's there, else from the NAS.
    gap_arrow(nas, lead, top + 60, 'the map’s files')
    # crates/agentcore/src/pool/records.rs: written whole to state/build/records.json each save (brief.json beside it).
    gap_arrow(nas, lead, top + 112, 'the records’ snapshot', back=True)
    # docs/storage.md, GC: the lead's gc job removes what its queue says is due.
    gap_arrow(nas, lead, top + 164, 'GC’s removals', back=True)
    gap_arrow(lead, dev, top + 60, 'tiles')
    # docs/storage.md, Writers: every job uploads its files, content-named; docs/coordinator.md §2: jobs read the NAS.
    ym = top + hh + 33
    f.arrow((mx + 110, nas.b), (mx + 110, by), label='inputs', at=(mx + 102, ym), anchor='end')
    f.arrow((mx + 230, by), (mx + 230, nas.b), label='built files, content-named', at=(mx + 238, ym))
    # docs/coordination.md §3.1: POST /pool/handoff, by HTTP to the lead alone.
    hx = lx + lw / 2
    f.arrow((mac.r, by + bh / 2), (hx, by + bh / 2), (hx, lead.b), label='hand-offs, by HTTP', at=(hx + 8, ym))
    # docs/storage.md, Writers: the owner's inputs folders.
    ix = nx + 48
    f.arrow((hand.r, by + bh / 2), (ix, by + bh / 2), (ix, nas.b), label='by hand', at=(ix - 8, ym), anchor='end')

    aria = ('Where the data lives. The NAS holds the bulk data: the inputs folders, the sources (each download made once), '
            'the built files per z6 and z3 tile and worldwide, the layers’ packs, build intermediates, and under state the '
            'lead’s declaration, the records’ snapshot, the pool’s token, archives and backups. The lead, the Mac mini, keeps '
            'on its own disk the records, the pool’s state in one store, the map with the log of its updates and GC’s queue, '
            'the week’s history, the app’s releases and the map server’s downloads. Every Mac’s agent keeps its jobs’ scratch, '
            'caches, held leases and hand-offs on its own disk; its jobs read inputs from the NAS, upload content-named files '
            'there and hand their record changes to the lead by HTTP. Inputs made by hand go into the NAS’s inputs folders. '
            'Devices read the map and the build page from the Mac mini.')
    return f.svg(by + bh + 14, aria)
