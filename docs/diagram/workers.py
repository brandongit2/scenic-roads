"""Who builds the map (docs/plan.md §8, Two Macs; docs/workers.md): the build Mac's agent plans and alone writes the
build's records; its coordinator lends work out on leases, jobs to the M1's helper, an area's last steps as tasks to
any worker, a device's page too, and takes the results back. The pool, in which any Mac can lead (docs/pool.md), is
planned."""
from html import escape as E

from diag import W, Diagram, rpath

LINE = 16.5


def build(check=False):
    d = Diagram('w', check)
    tx = d.tx
    # Drawn in this order: the build Mac's frame, arrows, boxes, labels.
    fr, el, ar, lb = [], [], [], []

    def box(x, y, w, title, lines=(), cls='', h=None):
        """A machine or part of one: its title, then its lines; h: at least this tall."""
        hh = max(h or 0, 45 + LINE * (len(lines) - 1) + 14 if lines else 36)
        el.append(f'<rect class="st-box{" " + cls if cls else ""}" x="{x}" y="{y}" width="{w}" height="{hh:.1f}" rx="9"/>')
        el.append(tx('st-t', x + 14, y + 23, title, w - 28))
        for i, s in enumerate(lines):
            el.append(tx('st-l', x + 14, y + 45 + LINE * i, s, w - 28))
        return (x, y, w, hh)

    def arrow(*pts, label=None, at=None, anchor='start', dashed=False):
        ar.append(f'<path class="st-a{" dashed" if dashed else ""}" d="{rpath(pts)}" marker-end="url(#wm-a)"/>')
        if label:
            lb.append(tx('st-lbl', at[0], at[1], label, anchor=anchor))

    top = 16
    # The NAS: the records the build Mac alone writes, and the files every worker's jobs put there.
    nx, nw = 16, 236
    nas = box(nx, top, nw, 'NAS', ['the build’s records', '(state/build/: the manifest,', 'the job keys), which only the',
                                   'build Mac writes; every built', 'file, by content name; a', 'catalog a round; cache/: what',
                                   'areas keep for their next run'], cls='nas')

    # The build Mac: its agent and, inside it, the coordinator.
    bx, bw = 320, 760
    ix, iw, gap = bx + 16, 330, 68
    cx = ix + iw + gap
    agent_lines = ['plans every step from the job keys, a region at a', 'time: those the map lacks first, the fewest areas',
                   'left first; two jobs at once: the plan’s first, and', 'beside it the trains’ or the landmarks’ steps, else',
                   'areas or slope; alone writes the records, merging', 'what workers hand back; publishes a round as',
                   'regions are done, about hourly']
    coord_lines = ['port 8090, for this Mac, its LAN and the tailnet;', 'lends work out on leases of ten minutes,',
                   'renewed by a beat each minute: a lapsed', 'lease’s work is offered again; journals what',
                   'comes back for the agent to merge; checks a', 'worker’s first three tasks against the build',
                   'Mac’s own run, then one in eight']
    iy = top + 40
    agent = box(ix, iy, iw, 'Its agent', agent_lines)
    coord = box(cx, iy, iw, 'Its coordinator', coord_lines)
    bh = agent[3] + 56
    fr.append(f'<rect class="st-box group" x="{bx}" y="{top}" width="{bw}" height="{bh:.1f}" rx="9"/>')
    fr.append(tx('st-t', bx + 14, top + 23, 'Build Mac (M4, 48 GB) · leads the build', bw - 28))

    # The workers: a device's page (tasks), the M1's helper (jobs, and tasks when no job fits it).
    wx, ww = 1148, W - 16 - 1148
    page = box(wx, top, ww, 'Any device · the build page', ['/work/ on the build Mac: anyone on its LAN or', 'the tailnet sees the build there; a tab helps',
                                                            'once you accept it on the build Mac: an area’s', 'last steps, run as WebAssembly'])
    m1 = box(wx, page[1] + page[3] + 22, ww, 'The M1 (16 GB) · a helper', ['its agent (install.sh --helper) takes the jobs',
                                                                        'that fit the 6 GB it spares (10 for a short one',
                                                                        'while you’re away): terrain, slope, tree cover,',
                                                                        'areas, landmark candidates and peaks; when',
                                                                        'none fits, an area’s last steps'])

    # Arrows: the NAS and the agent; the agent and its coordinator; the coordinator and the workers.
    ay = iy + 34
    arrow((nx + nw, ay), (ix, ay), label='reads', at=((nx + nw + bx) / 2, ay - 7), anchor='middle')
    arrow((ix, ay + 44), (nx + nw, ay + 44), label='records', at=((nx + nw + bx) / 2, ay + 37), anchor='middle')
    arrow((ix + iw, ay), (cx, ay), label='offers', at=(ix + iw + gap / 2, ay - 7), anchor='middle')
    arrow((cx, ay + 44), (ix + iw, ay + 44), label='hand-offs', at=(ix + iw + gap / 2, ay + 37), anchor='middle')
    mid = (bx + bw + wx) / 2
    py = page[1] + 56
    arrow((cx + iw, py), (wx, py), label='tasks', at=(mid, py - 7), anchor='middle')
    arrow((wx, py + 28), (cx + iw, py + 28), label='their files', at=(mid, py + 21), anchor='middle')
    my = m1[1] + 24
    arrow((cx + iw, my), (wx, my), label='jobs', at=(mid, my - 7), anchor='middle')
    arrow((wx, my + 28), (cx + iw, my + 28), label='hand-offs', at=(mid, my + 21), anchor='middle')
    # The M1's jobs put their files on the NAS themselves (content-named); only their record changes go back.
    ub = max(top + bh, m1[1] + m1[3]) + 24
    ux = wx + ww / 2
    arrow((ux, m1[1] + m1[3]), (ux, ub), (nx + nw / 2, ub), (nx + nw / 2, nas[1] + nas[3]),
          label='the M1’s jobs upload their files to the NAS themselves (content-named); their record changes go back as hand-offs',
          at=(bx + 14, ub - 7))

    # Planned: the pool (docs/pool.md).
    py0 = ub + 26
    box(nx, py0, W - 32, 'Planned · the pool: any Mac can lead (docs/pool.md)',
        ['Every Mac runs the same agent. The lead is the Mac a term file on the NAS names (made once: two Macs can’t both make it); it plans, merges and publishes, '
         'and every job, its own too, hands its results to a journal on the NAS.',
         'The lead is handed over from any Mac’s menu, the build page, the map or scenic lead, and taken over when it’s gone. Built: its core and a simulator, '
         'not yet wired into the agent.'], cls='later')
    h = py0 + 45 + LINE + 14 + 14

    aria = ('Who builds the map. The NAS holds the build’s records, which only the build Mac writes, every built file by content '
            'name, a catalog per round, and what areas keep for their next run. The build Mac’s agent plans every step, a region '
            'at a time, runs two jobs at once, merges what workers hand back and publishes a round about hourly. Its coordinator, '
            'on port 8090, lends work out on ten-minute leases renewed each minute: jobs to the M1’s helper, which uploads their '
            'files to the NAS itself and hands their record changes back, and an area’s last steps as tasks to any worker, a '
            'device’s build page too once you accept it; a worker’s first three tasks, then one in eight, are checked against the '
            'build Mac’s own run. Planned: the pool, in which any Mac can lead.')
    marker = ('<marker id="wm-a" viewBox="0 0 10 10" refX="9.5" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
              '<path class="st-mk" d="M0,0.8 L10,5 L0,9.2 z"/></marker>')
    return (f'<svg viewBox="0 0 {W} {h:.0f}" role="img" aria-label="{E(aria)}" xmlns="http://www.w3.org/2000/svg">'
            f'<defs>{marker}</defs>' + ''.join(fr + ar + el + lb) + '</svg>')
