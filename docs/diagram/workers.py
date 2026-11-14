"""Who builds the map: the Mac mini, the declared lead, plans, grants every job and alone merges the members' hand-offs
into the records and updates the map; every other Mac's agent takes jobs from its coordinator, by HTTP; the NAS holds
the bulk data and nothing of coordination. Each label's source is cited where it is written."""
from diag import W, Fig


def build(check=False):
    f = Fig('w', check)
    top = 16

    # docs/coordination.md §2 (nothing in coordination goes through the NAS; bulk data stays on it), §3.1 (the
    # declaration, state/build/lead-member.json; the records written whole each save: state/build/records.json).
    nx, nw = 16, 220
    nas_t = ('the bulk data: the inputs, the sources and every built file, by content name; the lead’s declaration; '
             'the records’ snapshot, written whole at each save. Nothing of coordination goes through it')

    # The agent. docs/pool.md §5 (the lead plans, merges the hand-offs, updates the map, runs GC; its slots are
    # ordinary member slots), §7.4 (planning reads only the records); crates/agentcore/src/agent/every.rs (LEAD_ONLY:
    # the OSM pass, the fetches, GC, the backup, the input checks); docs/building.md §2, 5 (a map update at most one
    # a minute: publish::CATALOGUE_EVERY_S); docs/formats.md, The map and its log (a batch appended to its log).
    agent_t = ('plans every step from the records; runs the steps kept to the lead (the OSM pass, the fetches, GC, the '
               'backup, the input checks) and, in its own slots, jobs as any member; alone merges the hand-offs into the '
               'records; updates the map, a batch appended to its log, at most one a minute')
    # The coordinator. crates/coordwire/src/lead.rs (PORT = 8090); docs/coordinator.md §9 (this Mac, its LAN and the
    # tailnet), §5 (leases: crates/agentcore/src/coord/mod.rs TTL = 600 s, renewed by a beat each minute; a member's
    # held until its word); docs/coordination.md §2 (one store on the lead's disk, durable before the answer), §5.1
    # (/map/*, the releases); docs/coordinator.md §7, §10 (the build page at its root; the app's releases).
    coord_t = ('port 8090, for this Mac, its LAN and the tailnet; grants every job, the lead’s own too, on leases of ten '
               'minutes renewed each minute, a member’s held by its beats’ word; keeps the pool’s state in one store on '
               'the lead’s disk, durable before each answer; serves the map’s log, the app’s releases and the build page')
    bx, bw = 352, 724
    ix, iw, gap = bx + 16, 300, 92
    cx = ix + iw + gap
    iy = top + 42
    ih = max(f.measure(iw, agent_t), f.measure(iw, coord_t))
    f.box(ix, iy, iw, 'Its agent', agent_t, h=ih)
    f.box(cx, iy, iw, 'Its coordinator', coord_t, h=ih)
    bh = ih + 56
    # docs/macs.md (the Mac mini: 32 GB, always on, the pool's lead, fixed); docs/pool.md §6.1 (the declaration).
    f.box(bx, top, bw, 'The lead · the Mac mini (32 GB), always on · declared, never elected', cls='group', h=bh, frame=True)
    nas = f.box(nx, top, nw, 'NAS', nas_t, cls='nas', h=bh)

    # docs/coordinator.md §7, §9 (anyone the Mac answers sees the build, with no key, and may pause it; a page
    # carries no token and never works).
    wx = 1190
    ww = W - 16 - wx
    page = f.box(wx, top, ww, 'Any device · the build page',
                 'at the coordinator’s root: anyone on the lead’s LAN or the tailnet sees the build and may pause it, with no key; a page runs no work')
    # docs/macs.md (the M4, 48 GB; the M1, 16 GB: a member takes the jobs that fit its memory and disk); crates/
    # agentcore/src/agent/beat.rs (EVERY = 10 s); docs/coordination.md §3.1, §5.3 (hand-offs posted to the lead, kept
    # until merged); docs/pool.md §8 (each member brokers its jobs' tasks; any member's idle slot takes one), §6.3.
    mem = f.box(wx, page.b + 22, ww, 'The members · the M4 and the M1',
                'the M4 (48 GB) and the M1 (16 GB): each takes the jobs that fit its memory and its disk; beats to the lead every 10 s; hands each job’s record '
                'changes to the lead as one hand-off, kept on its own disk until merged; brokers its own jobs’ tasks, which '
                'any member’s idle slot takes; never leads')

    # docs/pool.md §6.1 (the declaration read every step by the lead); crates/agentcore/src/pool/records.rs.
    ay = iy + 34
    m = (nas.r + ix) / 2
    f.arrow((nas.r, ay), (ix, ay), label='the declaration', at=(m, ay - 7), anchor='middle')
    f.arrow((ix, ay + 44), (nas.r, ay + 44), label='records’ snapshot', at=(m, ay + 37), anchor='middle')
    # docs/coordinator.md §1 preamble (the agent plans; the coordinator grants); docs/coordination.md §3.1 (hand-offs
    # arrive at the coordinator, POST /pool/handoff, and stay in its store until a saved snapshot names them).
    m = ix + iw + gap / 2
    f.arrow((ix + iw, ay), (cx, ay), label='the plan', at=(m, ay - 7), anchor='middle')
    f.arrow((cx, ay + 44), (ix + iw, ay + 44), label='hand-offs', at=(m, ay + 37), anchor='middle')
    m = (cx + iw + wx) / 2
    py = top + 56
    f.arrow((cx + iw, py), (wx, py), label='the page', at=(m, py - 7), anchor='middle')
    f.arrow((wx, py + 28), (cx + iw, py + 28), label='asks', at=(m, py + 21), anchor='middle')
    # docs/coordination.md §5.1: /work/ask's grants; POST /pool/beat, POST /pool/handoff.
    my = mem.y + 24
    f.arrow((cx + iw, my), (wx, my), label='jobs', at=(m, my - 7), anchor='middle')
    f.arrow((wx, my + 28), (cx + iw, my + 28), label='beats · hand-offs', at=(m, my + 21), anchor='middle')
    # docs/storage.md, Writers (every job uploads its files, content-named); docs/coordinator.md §2 (jobs go to
    # workers that mount the NAS).
    ub = max(top + bh, mem.b) + 26
    ux = wx + ww / 2
    f.arrow((ux, mem.b), (ux, ub), (nx + nw / 2, ub), (nx + nw / 2, nas.b),
            label='every job, the lead’s too, reads its inputs on the NAS and uploads its files there, content-named',
            at=(bx + 14, ub - 7))

    # Three notes under the figure, each a third of its width.
    y0 = ub + 26
    g = 16
    tw = (W - 32 - 2 * g) / 3
    notes = [
        # crates/server/src/maplog.rs (the lead's GET /map/changes, long-polled: store::map::feed::WAIT_S = 30 s, answered
        # as a batch comes; the Mac mini's server, and a laptop's on demand); docs/macs.md: Server (resident on the Mac
        # mini alone), Mirror (a file from the Mac when it's there, else the NAS), Devices.
        ('The map server · resident on the Mac mini',
         'follows the map’s log by HTTP (a 30 s long poll, answered as a batch comes), so it has each map update within a '
         'second; reads a file from its downloads when it’s there, else from the NAS; serves the devices at the map’s '
         'address on the tailnet. A laptop runs one of its own on demand, for the offline map'),
        # docs/pool.md §6.3, §6.1; docs/formats.md, The map and its log (with the lead down the map isn't updated).
        ('With the lead gone',
         'No Mac takes its place. A member finishes the jobs it holds, keeps their hand-offs on its own disk and starts '
         'none; the map isn’t updated, and each client keeps the map it holds. Back, the lead merges what its members '
         'post it and grants again. The owner moves the lead only by editing the declaration.'),
        # docs/storage.md, GC; crates/agentcore/src/agent/gc.rs (CHUNK_NAMES = 200, ORPHAN_AGE_S = 14 days),
        # crates/store/src/map/gc.rs (GRACE_S = an hour); agentcore::agent::gc's module doc (run as the NAS budget leaves room).
        ('GC · a job of the lead’s own',
         'never looks for garbage: the lead queues each file as the map and the records stop naming it, and the job '
         'removes what’s due, 200 names at a time, when the NAS budget leaves it room. A name is due once every client’s cursor is past '
         'the update that freed it and an hour has gone by. A sweep every 14 days finds what was never queued.')]
    th = max(f.measure(tw, t) for _, t in notes)
    for i, (title, t) in enumerate(notes):
        f.box(nx + i * (tw + g), y0, tw, title, t, h=th)

    aria = ('Who builds the map. The lead is the Mac mini, declared and never elected. Its agent plans every step from the '
            'records, runs the steps kept to the lead and jobs in its own slots, alone merges the hand-offs into the records '
            'and updates the map, at most once a minute. Its coordinator, on port 8090, grants every job on ten-minute leases, '
            'keeps the pool’s state in one store on the lead’s disk and serves the map’s log, the app’s releases and the build '
            'page. The M4 and the M1 are members: each takes the jobs that fit it, beats to the lead every ten seconds and '
            'hands its jobs’ record changes to it; every job reads its inputs on the NAS and uploads its files there. Any '
            'device’s build page shows the build and may pause it, and runs no work. The NAS holds the bulk data, the '
            'declaration and the records’ snapshot, and nothing of coordination. The map server, resident on the Mac mini, '
            'follows the map’s log; a laptop runs its own on demand. With the lead gone no Mac takes its place. GC is a job of the lead’s own, removing what '
            'its queue says is due.')
    return f.svg(y0 + th + 14, aria)
