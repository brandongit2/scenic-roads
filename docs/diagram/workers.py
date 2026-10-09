"""Who builds the map (docs/pool.md, phase 1 and phase 3's controls on since 8 Oct 2026; plan.md §8; docs/workers.md):
the Mac the current term names leads (the M4 today): its agent plans, its coordinator lends work out on leases (jobs to
any member's agent, an area's last steps and other tasks to any worker, a device's page too), and it alone merges the
journal into the term's records. Every job, the lead's too, hands its results to the journal on the NAS. What the
pool leaves for later is planned."""
from html import escape as E

from diag import W, Diagram, rpath, wrap

LINE = 18.5


def build(check=False):
    d = Diagram('w', check)
    tx = d.tx
    # Drawn in this order: the lead's frame, arrows, boxes, labels.
    fr, el, ar, lb = [], [], [], []

    def box(x, y, w, title, lines=(), cls='', h=None):
        """A machine or part of one: its title, then its lines (one paragraph, wrapped to the box); h: at least this tall."""
        lines = wrap('st-l', lines, w - 28) if lines else []
        hh = max(h or 0, 49 + LINE * (len(lines) - 1) + 14 if lines else 40)
        el.append(f'<rect class="st-box{" " + cls if cls else ""}" x="{x}" y="{y}" width="{w}" height="{hh:.1f}" rx="9"/>')
        el.append(tx('st-t', x + 14, y + 25, title, w - 28))
        for i, s in enumerate(lines):
            el.append(tx('st-l', x + 14, y + 49 + LINE * i, s, w - 28))
        return (x, y, w, hh)

    def arrow(*pts, label=None, at=None, anchor='start', dashed=False):
        ar.append(f'<path class="st-a{" dashed" if dashed else ""}" d="{rpath(pts)}" marker-end="url(#wm-a)"/>')
        if label:
            lb.append(tx('st-lbl', at[0], at[1], label, anchor=anchor))

    top = 16
    # The NAS: the terms, the journal every job writes to, the term's records only the lead merges into.
    nx, nw = 16, 236
    nas = box(nx, top, nw, 'NAS', ['the terms: who leads, made', 'once each (state/build/terms/);',
                                   'the journal: every job’s', 'results (state/journal/);', 'the term’s records, which',
                                   'only the lead writes; every', 'built file, by content name;', 'a catalog a round; cache/'],
              cls='nas')

    # The lead: its agent and, inside it, the coordinator.
    bx, bw = 320, 760
    ix, iw, gap = bx + 16, 330, 68
    cx = ix + iw + gap
    agent_lines = ['plans every step from the job keys, a region at a', 'time: those the map lacks first, the fewest areas',
                   'left first; runs two jobs at once, which hand off', 'like anyone’s; merges the journal into the',
                   'term’s records; publishes a round as regions', 'are done, about hourly, once caught up']
    coord_lines = ['port 8090, for this Mac, its LAN and the tailnet;', 'lends work out on leases of ten minutes,',
                   'renewed by a beat each minute: a lapsed', 'lease’s work is offered again; its state is the',
                   'term’s, on the NAS; checks a worker’s first', 'three tasks, then one in eight; waits on a',
                   'worker’s task only if it’s measured faster']
    iy = top + 42
    agent = box(ix, iy, iw, 'Its agent', agent_lines)
    coord = box(cx, iy, iw, 'Its coordinator', coord_lines)
    bh = max(agent[3], coord[3]) + 56
    fr.append(f'<rect class="st-box group" x="{bx}" y="{top}" width="{bw}" height="{bh:.1f}" rx="9"/>')
    fr.append(tx('st-t', bx + 14, top + 25, 'The lead (the M4 today, 48 GB) · any Mac can lead', bw - 28))

    # The workers: a device's page (tasks), the M1 (jobs, and tasks when no job fits it).
    wx, ww = 1148, W - 16 - 1148
    page = box(wx, top, ww, 'Any device · the build page', ['/work/ on the lead: anyone on its LAN or the',
                                                            'tailnet sees the build; a tab helps, no key:',
                                                            'an area’s last steps, tree cover’s rows of',
                                                            'blocks, 3D buildings’ z8 areas (WebAssembly)'])
    m1 = box(wx, page[1] + page[3] + 22, ww, 'The M1 (16 GB) · a member', ['takes the jobs that fit the 6 GB it spares',
                                                                         '(10 for a short one while you’re away):',
                                                                         'terrain, slope, tree cover’s pieces, areas,',
                                                                         'candidates, peaks, 3D buildings’ steps; when',
                                                                         'none fits, tasks; it leads when handed it'])

    # Arrows: the NAS and the agent; the agent and its coordinator; the coordinator and the workers.
    ay = iy + 34
    arrow((nx + nw, ay), (ix, ay), label='the journal', at=((nx + nw + bx) / 2, ay - 7), anchor='middle')
    arrow((ix, ay + 44), (nx + nw, ay + 44), label='records', at=((nx + nw + bx) / 2, ay + 37), anchor='middle')
    arrow((ix + iw, ay), (cx, ay), label='offers', at=(ix + iw + gap / 2, ay - 7), anchor='middle')
    arrow((cx, ay + 44), (ix + iw, ay + 44), label='done', at=(ix + iw + gap / 2, ay + 37), anchor='middle')
    mid = (bx + bw + wx) / 2
    py = page[1] + 56
    arrow((cx + iw, py), (wx, py), label='tasks', at=(mid, py - 7), anchor='middle')
    arrow((wx, py + 28), (cx + iw, py + 28), label='their files', at=(mid, py + 21), anchor='middle')
    my = m1[1] + 24
    arrow((cx + iw, my), (wx, my), label='jobs', at=(mid, my - 7), anchor='middle')
    arrow((wx, my + 28), (cx + iw, my + 28), label='done', at=(mid, my + 21), anchor='middle')
    # Every member's jobs put their files on the NAS and their results in the journal; the lead's do too.
    ub = max(top + bh, m1[1] + m1[3]) + 24
    ux = wx + ww / 2
    arrow((ux, m1[1] + m1[3]), (ux, ub), (nx + nw / 2, ub), (nx + nw / 2, nas[1] + nas[3]),
          label='every job, the lead’s too, uploads its files (content-named) and writes its results to the journal; only the lead merges them',
          at=(bx + 14, ub - 7))

    # The pool's controls, built; what it leaves for later, planned.
    py0 = ub + 26
    ctl = box(nx, py0, (W - 32 - 16) / 2, 'The lead moves: the pool (docs/pool.md), on since 8 October',
              ['Handed over from any Mac’s menu bar (Hand the Build To…), the build page, the map’s build', 'panel or scenic lead; taken over (Take Over the Build…) when the lead is gone; a lead that', 'stood down is taken over by itself after two minutes. One lead a term; nothing is lost.'])
    px = nx + ctl[2] + 16
    box(px, py0, W - 16 - px, 'Planned',
        ['The lead’s own jobs in their own processes (pool phase 2); any member serving the build page', 'and brokering its own jobs’ tasks, the pool’s API for its mailboxes (phase 4); the journal’s GC;', 'every job’s detailed timing log, at equal detail (#130, under way).'], cls='later')
    h = py0 + ctl[3] + 14

    aria = ('Who builds the map. The NAS holds the terms, which say who leads, the journal every job writes its results to, the '
            'term’s records, which only the lead writes, every built file by content name, a catalog per round, and what areas keep '
            'for their next run. The lead, the M4 today, plans every step a region at a time, runs two jobs at once, merges the '
            'journal into the records and publishes a round about hourly once caught up. Its coordinator, on port 8090, lends work '
            'out on ten-minute leases renewed each minute: jobs to the M1, a member that takes terrain, slope, tree cover’s pieces, '
            'areas, candidates, peaks and the 3D buildings’ steps, and tasks (an area’s last steps, tree cover’s rows of blocks, the '
            '3D buildings’ z8 areas) to any worker, a device’s build page too, with no key; a worker’s first three tasks, then one '
            'in eight, are checked. The lead is handed over from any Mac’s menu bar, the build page, the map or scenic lead, and '
            'taken over when it’s gone. Planned: the lead’s own jobs in their own processes, any member serving the build page, '
            'the journal’s GC, and every job’s detailed timing log.')
    marker = ('<marker id="wm-a" viewBox="0 0 10 10" refX="9.5" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
              '<path class="st-mk" d="M0,0.8 L10,5 L0,9.2 z"/></marker>')
    return (f'<svg viewBox="0 0 {W} {h:.0f}" role="img" aria-label="{E(aria)}" xmlns="http://www.w3.org/2000/svg">'
            f'<defs>{marker}</defs>' + ''.join(fr + ar + el + lb) + '</svg>')
