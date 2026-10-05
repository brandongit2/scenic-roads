// The status bar's build items (docs/plan.md §1, §8 Status): the NAS when it can't be reached (map
// data not mirrored on this Mac may be missing), the build Mac's state from its heartbeat ("Build
// Mac · building …", "· paused: on battery", "· idle", "· last seen 3 h ago"), and new map data or
// a new app that wants a reload. A click opens the details: the job and its log, what waits and
// why, the last jobs to finish, the conditions.
import type { Agent, CatalogWatch } from '../catalog';
import { h } from './dom';

/** A heartbeat older than this: the build Mac is asleep, away or off (its agent writes one at least
 * every two minutes). */
const STALE_S = 360;

const now = () => Date.now() / 1000;

/** A span of seconds: "40 s", "12 min", "3 h 5 min", "2 days". */
function span(s: number): string {
  s = Math.max(0, Math.round(s));
  if (s < 60) return `${s} s`;
  const min = Math.round(s / 60);
  if (min < 60) return `${min} min`;
  if (min < 24 * 60) return `${Math.floor(min / 60)} h${min % 60 ? ` ${min % 60} min` : ''}`;
  const d = Math.round(min / (24 * 60));
  return `${d} day${d === 1 ? '' : 's'}`;
}
/** How long ago a time (seconds since the epoch) was, roughly: "40 s ago", "12 min ago", "3 h ago",
 * "2 days ago". */
function ago(t: number): string {
  const s = Math.max(0, now() - t);
  return `${s < 3600 ? span(s) : s < 86400 ? `${Math.floor(s / 3600)} h` : span(s)} ago`;
}
/** A date and time: "2 Oct 19:34" (the year too when it isn't this one's). */
function when(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const year = d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric';
  return d.toLocaleString('en-GB', { day: 'numeric', month: 'short', year, hour: '2-digit', minute: '2-digit' }).replace(',', '');
}

/** A time to come (seconds since the epoch): "16:40" today, "Tue 07:50" within the week, else
 * "8 Oct 07:50". */
function at(t: number): string {
  const d = new Date(t * 1000);
  const hm = d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
  if (d.toDateString() === new Date().toDateString()) return hm;
  if (Math.abs(t - now()) < 6 * 86400) return `${d.toLocaleDateString('en-GB', { weekday: 'short' })} ${hm}`;
  return `${d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })} ${hm}`;
}

/** A job in the bar: "building OpenStreetMap pass", "backing up translations…" (without its
 * parenthesis). */
function doing(what: string): string {
  const w = what.replace(/\s*\([^)]*\)\s*$/, '');
  const first = w.split(' ')[0];
  // A plain word ("Backing", "Removing") lower-cased; a name ("OpenStreetMap") kept.
  const s = /^[A-Z][a-z]+$/.test(first) ? w.charAt(0).toLowerCase() + w.slice(1) : w;
  return /^[a-z]+ing\b/.test(s) ? s : `building ${s}`;
}

/** The build Mac in a few words, and how its dot shows. */
function agentState(a: Agent): { text: string; dot: 'run' | 'paused' | 'idle' | 'away' } {
  if (now() - a.beat > STALE_S) return { text: `last seen ${ago(a.beat)}`, dot: 'away' };
  if (a.pause) return { text: a.job && !a.job.paused ? 'pausing' : 'paused', dot: 'paused' };
  if (a.job?.paused) return { text: `paused: ${a.job.paused.split(':')[0]}`, dot: 'paused' };
  if (a.job) return { text: doing(a.job.what), dot: 'run' };
  if (!a.conditions.nas) return { text: 'can’t reach the NAS', dot: 'paused' };
  return { text: a.waiting.length ? `idle · ${a.waiting.length} waiting` : 'idle', dot: 'idle' };
}

export class BuildStatus {
  private nas = h('button', { class: 'bs-nas', type: 'button' });
  private agent = h('button', { class: 'bs-agent', type: 'button' });
  private reload = h('button', { class: 'bs-reload', type: 'button', onclick: () => location.reload() });
  private pop: HTMLDivElement | null = null;
  private off: (() => void)[] = [];

  constructor(root: HTMLElement, private watch: CatalogWatch) {
    root.classList.add('build');
    root.append(this.reload, this.nas, this.agent);
    for (const b of [this.nas, this.agent]) b.addEventListener('click', () => (this.pop ? this.close() : this.open()));
    this.render();
    watch.on(() => this.render());
  }

  private render() {
    const w = this.watch, c = w.status;
    this.reload.hidden = !w.reload;
    this.reload.textContent = `${w.reload} · Reload`;
    this.reload.title = `${w.reload}: part of it shows only after a reload`;
    // The server itself, else the NAS behind it.
    const down = w.unreachable || (c !== null && !c.online);
    this.nas.hidden = !down;
    this.nas.replaceChildren(h('i', { class: 'dot' }), h('span', {}, w.unreachable ? 'Map server not answering' : 'NAS offline'));
    this.nas.title = w.unreachable ? 'The map server isn’t answering (restarting?)' : 'The NAS can’t be reached: map data not mirrored on this Mac may be missing';
    const a = c?.agent ?? null;
    this.agent.hidden = !a;
    if (a) {
      const st = agentState(a);
      this.agent.replaceChildren(h('i', { class: `dot ${st.dot}` }), h('span', {}, `Build Mac · ${st.text}`));
      this.agent.title = a.job ? `${a.job.what}${a.job.paused ? ` (paused: ${a.job.paused})` : ''}` : 'The build Mac: what it does, what waits and why (click)';
    }
    if (this.pop) this.fill(this.pop);
  }

  private open() {
    const pop = h('div', { class: 'bs-pop', role: 'dialog' });
    this.fill(pop);
    document.body.append(pop);
    const anchor = (this.agent.hidden ? this.nas : this.agent).getBoundingClientRect();
    pop.style.right = `${Math.max(8, window.innerWidth - anchor.right)}px`;
    pop.style.bottom = `${window.innerHeight - anchor.top + 8}px`;
    this.pop = pop;
    const outside = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!pop.contains(t) && !this.nas.contains(t) && !this.agent.contains(t)) this.close();
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      this.close();
    };
    document.addEventListener('pointerdown', outside, true);
    document.addEventListener('keydown', esc, true);
    window.addEventListener('resize', this.close);
    this.off = [
      () => document.removeEventListener('pointerdown', outside, true),
      () => document.removeEventListener('keydown', esc, true),
      () => window.removeEventListener('resize', this.close),
    ];
    // Fresh details.
    void this.watch.poll();
  }

  private close = () => {
    this.pop?.remove();
    this.pop = null;
    this.off.forEach((f) => f());
    this.off = [];
  };

  /** The whole build paused (every Mac's job at its next safe point, or `now`: frozen at once) or
   * going on: an ask to this Mac's agent, through the map's server, which passes it on. */
  private async ask(mode: 'drain' | 'freeze' | null) {
    this.asking = mode === null ? 'resume' : 'pause';
    if (this.pop) this.fill(this.pop);
    try {
      const r = await fetch('/api/build/pause', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ mode }) });
      if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
    } catch (e) {
      this.asking = null;
      alert(`The build couldn't be asked to ${mode === null ? 'go on' : 'pause'}: ${e instanceof Error ? e.message : String(e)}`);
    }
    void this.watch.poll();
  }
  /** An ask sent, until the heartbeat shows it taken up. */
  private asking: 'pause' | 'resume' | null = null;

  /** The details: the build Mac (job, waiting, recent, conditions), then the map data and NAS. */
  private fill(pop: HTMLElement) {
    const c = this.watch.status, a = c?.agent ?? null;
    const hd = (t: string, right: Node | string = '') => h('div', { class: 'bs-hd' }, h('span', {}, t), typeof right === 'string' ? h('span', { class: 'faint' }, right) : right);
    const out: Node[] = [];
    if (a) {
      const stale = now() - a.beat > STALE_S;
      out.push(
        hd('Build Mac', h('span', { class: stale ? 'warn' : 'faint' }, stale ? `last seen ${ago(a.beat)}` : `seen ${ago(a.beat)}`)),
        ...(stale ? [h('div', { class: 'bs-row warn' }, 'Asleep, away or off. As it was then:')] : []),
        h('div', { class: 'bs-row faint' }, [a.host, a.app, ...(stale ? [] : [`running ${span(now() - a.started)}`])].join(' · ')),
        h('div', { class: 'bs-row' }, [a.conditions.ac ? 'On mains power' : 'On battery', !a.conditions.nas ? 'NAS not reachable' : a.conditions.home === false ? 'NAS through Tailscale (away from home)' : 'NAS reachable', a.conditions.idle_s >= 120 ? `idle ${span(a.conditions.idle_s)}` : 'in use'].join(' · ')),
      );
      // When it'll be done and the map next gets new data (the forecast, as the worker page and the
      // menu bar say it).
      const f = a.forecast;
      if (f && !stale) {
        const names = Object.fromEntries(a.regions.map((r) => [r.id, r.name]));
        const next = f.rounds.find((r) => r.regions.length);
        const which = next ? next.regions.slice(0, 3).map((id) => names[id] ?? id).join(', ') + (next.regions.length > 3 ? ` and ${next.regions.length - 3} more` : '') : '';
        out.push(h('div', { class: 'bs-row' }, [
          f.done_at ? `Done ≈ ${at(f.done_at)}${f.range ? ` (${at(f.range[0])}–${at(f.range[1])})` : ''}` : `No finish to forecast: ${f.why ?? 'unknown'}`,
          ...(next ? [`next map update ≈ ${at(next.at)}: ${which}`] : []),
        ].join(' · ')));
      }
      // Pausing the whole build, or letting it go on (pipeline::control).
      if (this.asking && (this.asking === 'pause') === !!a.pause) this.asking = null;
      const btn = (label: string, title: string, mode: 'drain' | 'freeze' | null) => h('button', { class: 'bs-btn', type: 'button', title, onclick: () => void this.ask(mode) }, label);
      // (The opposite of what was asked stays there: an ask is never stuck.)
      if (this.asking) {
        out.push(
          h('div', { class: 'bs-row faint' }, this.asking === 'pause' ? 'Pausing… (the build Mac takes it up within seconds)' : 'Resuming…'),
          h('div', { class: 'bs-row' }, this.asking === 'pause' ? btn('Resume building', 'The build picks up where it stopped', null) : btn('Pause building', 'Every Mac’s running job stops at its next safe point, keeping what it did', 'drain')),
        );
      } else if (a.pause) {
        const how = a.pause.mode === 'freeze' ? 'every Mac’s job frozen where it was' : 'every Mac’s job stops at its next safe point';
        out.push(
          h('div', { class: 'bs-row warn' }, `Paused from ${a.pause.by}, ${ago(a.pause.at)}: ${how}; nothing new starts`),
          h('div', { class: 'bs-row' }, btn('Resume building', 'The build picks up where it stopped', null)),
        );
      } else {
        out.push(h('div', { class: 'bs-row' },
          btn('Pause building', 'Every Mac’s running job stops at its next safe point (an area, a map tile), keeping what it did; nothing new starts until you resume', 'drain'), ' ',
          btn('Pause now', 'Every Mac’s running job frozen where it is at once; it goes on from there when you resume', 'freeze')));
      }
      if (a.job) {
        out.push(
          hd('Now', `for ${span(now() - a.job.started)}`),
          h('div', { class: 'bs-row' }, a.job.what),
          ...(a.job.paused ? [h('div', { class: 'bs-row warn' }, `Paused: ${a.job.paused}`)] : a.job.pausing ? [h('div', { class: 'bs-row warn' }, 'Stopping at its next safe point (what it’s on is kept)')] : []),
          ...(a.job.tail.trim() ? [h('pre', { class: 'bs-log' }, a.job.tail.trimEnd())] : []),
        );
      }
      if (a.waiting.length) {
        out.push(hd('Waiting', String(a.waiting.length)), ...a.waiting.map((x) => h('div', { class: 'bs-item' }, h('div', {}, x.what), h('div', { class: 'faint' }, x.why))));
      }
      if (a.recent.length) {
        out.push(hd('Finished'), ...a.recent.slice(0, 6).map((d) =>
          h('div', { class: 'bs-item' },
            h('div', { class: 'bs-done' }, h('span', { class: d.ok ? 'ok' : 'fail' }, d.ok ? '✓' : '✗'), h('span', { class: 'what' }, d.what), h('span', { class: 'faint' }, `${ago(d.ended)} · ${span(d.secs)}`)),
            ...(!d.ok && d.note.trim() ? [h('pre', { class: 'bs-log' }, d.note.trim().split('\n').slice(-6).join('\n'))] : []),
          )));
      }
      if (a.bad_recipes.length) {
        out.push(hd('Recipes that don’t parse'), ...a.bad_recipes.map(([f, e]) => h('div', { class: 'bs-item' }, h('div', {}, f), h('div', { class: 'warn' }, e))));
      }
    }
    if (c) {
      out.push(
        hd('Map data', `catalog ${c.n}`),
        h('div', { class: 'bs-row' }, `Published ${when(c.created)} · ${c.units} unit${c.units === 1 ? '' : 's'}`),
        h('div', { class: `bs-row${c.online ? ' faint' : ' warn'}` }, c.online ? `NAS reachable${c.nas ? ` (${c.nas})` : ''}` : 'The NAS can’t be reached: map data not mirrored on this Mac may be missing. The server mounts it again once it’s back.'),
      );
    }
    if (this.watch.unreachable) out.push(h('div', { class: 'bs-row warn' }, 'The map server isn’t answering.'));
    if (this.watch.reload) out.push(h('div', { class: 'bs-row' }, `${this.watch.reload}: reload the page for all of it.`));
    pop.replaceChildren(...out);
  }
}
