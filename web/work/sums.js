// The build page's sums, apart from the page so Node tests them (tools/check/worksums.test.mjs).

// A stage's or step's line finished: nothing left (build::Line, build::Item).
export const finished = (x) => (x.left != null ? x.left === 0 : x.total != null && x.done >= x.total && x.total > 0);

// The schedule's lanes worth a row: those with a bar in the window [t0, t1] (a run ending at or
// after t0 and starting by t1), in the order given.
export const busyLanes = (names, lanes, t0, t1) => names.filter((nm) => (lanes[nm] || []).some((l) => l.until >= t0 && l.from <= t1));

// While this Mac hasn't read the build's state (a 503 from /work/swarm: its page view reads only
// while asked lately, crates/pipeline/src/agent/pageview.rs; a 503 from /work/history alone waits
// for the next regular poll): asked again every READ_RETRY_MS while the page is seen, with a quiet
// line, not an error, until READ_WAIT_MS after the first 503 in a row; then an error, asked again
// only with the regular poll. `since` is when the 503s began (ms),
// `answered` whether the build was shown before. Returns { state, error, retryMs } (retryMs null:
// no quick retry).
export const READ_RETRY_MS = 2000;
export const READ_WAIT_MS = 60000;
export function notRead(answered, since, now) {
  if (now - since >= READ_WAIT_MS) {
    return { state: null, error: `this Mac hasn't read the build's state in ${Math.round((now - since) / 1000)} s`, retryMs: null };
  }
  const state = answered ? "reading the build's state again… (what's shown may be stale)" : "reading the build's state…";
  return { state, error: null, retryMs: READ_RETRY_MS };
}

// ---- The Jobs tab's schedule --------------------------------------------------------------------
// A slot's number from its worker's name (crate::agent::is_slot_of): 1 for the bare host's, N for
// "<host> #N" (N ≥ 2); null for none of `host`'s. A job in slot index k is on lane slotName(host, k).
export function slotNo(name, host) {
  if (name === host) return 1;
  const rest = name.startsWith(`${host} #`) ? name.slice(host.length + 2) : "";
  return /^\d+$/.test(rest) && +rest >= 2 ? +rest : null;
}
export const isSlotOf = (name, host) => slotNo(name, host) != null;
export const slotName = (host, k) => (k ? `${host} #${k + 1}` : host);

// The Mac a worker's name is and its slot (1, N for "<host> #N", 2 for an older agent's
// "<host> (second job)"), whatever name the Mac went by: one of `hosts` when the name's host is it,
// or is the start of it up to a "-" (macOS renamed "Mac-mini" to "Mac-mini-de-Brandon" when its
// owner's name was set: the history keeps the old one), else the name's host (a Mac gone).
export function macSlot(name, hosts = []) {
  const m = /^(.*?)(?: #(\d+)| \(second job\))$/.exec(name);
  const base = m ? m[1] : name, slot = m ? (m[2] ? +m[2] : 2) : 1;
  const host = hosts.find((h) => h === base) || hosts.find((h) => h.startsWith(`${base}-`)) || base;
  return { host, slot };
}

// The jobs that ended at or after `from`, as the history has them, as runs {lane, step, from, until,
// n, ok, targets, seq}, by start: a lead's job's end (kind "end": its worker the slot) and a
// worker's leased job's ("done", "fail": its worker the Mac, no slot; put on the first of its Mac's
// lanes free by then). `t` is the end, `secs` how long it ran. Lanes are named by `hosts` (macSlot).
// An event without its time or step is passed over; one given twice (by its seq), once. Each run's
// `after`: the seqs of the ends of the jobs it waited on, as its start (a lead's job: its worker's
// last "start" of its step) or its lease names them (crate::coord::history::Event::after; none
// before task #791).
export function pastRuns(events, from, hosts = []) {
  const seen = new Set(), out = [], leased = [];
  const started = new Map(), leases = new Map();
  for (const e of [...(events || [])].sort((p, q) => p.seq - q.seq)) {
    if (e.kind === "start") started.set(`${e.worker} ${e.step}`, e.after);
    if (e.kind === "lease") leases.set(e.lease, e.after);
    if (!["end", "done", "fail"].includes(e.kind) || !e.step || e.secs == null || !e.worker || e.t < from || seen.has(e.seq)) continue;
    seen.add(e.seq);
    const { host, slot } = macSlot(e.worker, hosts);
    // (A start is its slot's next end's alone: taken, so a later end whose start isn't held names none.)
    const key = e.kind === "end" ? `${e.worker} ${e.step}` : null;
    const after = (key ? started.get(key) : leases.get(e.lease)) || [];
    if (key) started.delete(key);
    const r = { lane: slotName(host, slot - 1), step: e.step, from: e.t - e.secs, until: e.t, n: (e.targets || []).length, ok: e.ok !== false, targets: e.targets || [], seq: e.seq, after };
    (e.kind === "end" ? out : leased).push(r);
    if (e.kind !== "end") r.host = host;
  }
  const free = {};
  for (const r of leased.sort((p, q) => p.from - q.from)) {
    const ends = (free[r.host] ||= []);
    let k = ends.findIndex((u) => u <= r.from);
    if (k < 0) k = ends.length;
    ends[k] = r.until;
    r.lane = slotName(r.host, k);
    delete r.host;
    out.push(r);
  }
  return out.sort((p, q) => p.from - q.from);
}

// A lane's runs packed for a track drawn `gap` seconds a pixel: a run of the same step starting
// within `gap` of the last one's end joins it as one segment (its targets summed, `parts` its runs,
// `ok` while every one was), so a week's hundreds of short jobs aren't a bar each. By start.
export function packRuns(runs, gap) {
  const out = [];
  for (const r of [...runs].sort((p, q) => p.from - q.from)) {
    const prev = out[out.length - 1];
    if (prev && prev.step === r.step && r.from - prev.until <= gap) {
      prev.until = Math.max(prev.until, r.until);
      prev.n += r.n;
      prev.ok = prev.ok && r.ok !== false;
      prev.parts.push(r);
    } else out.push({ ...r, ok: r.ok !== false, parts: [r] });
  }
  return out;
}

// The schedule's lanes grouped by Mac: each host of `hosts` in order with its slots' lanes among
// `names` (by slot number); then each other lane's host (its name less the slot's number: a Mac
// gone), a group each.
export function laneGroups(names, hosts) {
  const left = new Set(names), out = [];
  const take = (host) => {
    const lanes = names.filter((nm) => left.has(nm) && isSlotOf(nm, host)).sort((p, q) => slotNo(p, host) - slotNo(q, host));
    lanes.forEach((nm) => left.delete(nm));
    if (lanes.length) out.push({ host, lanes });
  };
  hosts.forEach(take);
  for (const nm of names) if (left.has(nm)) take(nm.replace(/ #\d+$/, ""));
  return out;
}

// The schedule's view (task #787): its middle as seconds from now (`c`: it moves with now) and its
// width in seconds (`span`), held in the timeline's domain [lo, hi] (unix s: the history's seven
// days back to the forecast's end) and no narrower than SCHED_MIN_SPAN. `viewRange` is the times
// it shows, {t0, t1, span}; `viewZoom` zooms by `k` (over 1: in) about the point at `f` (0–1) of its
// width, that time kept under it; `viewPan` moves it `dt` seconds later. Each answers a view held
// in the domain.
export const SCHED_MIN_SPAN = 600;
export function viewRange(v, now, lo, hi) {
  const span = Math.min(Math.max(SCHED_MIN_SPAN, hi - lo), Math.max(SCHED_MIN_SPAN, v.span));
  const t0 = Math.max(lo, Math.min(hi - span, now + v.c - span / 2));
  return { t0, t1: t0 + span, span };
}
const viewOf = (t0, span, now, lo, hi) => {
  const r = viewRange({ c: t0 + span / 2 - now, span }, now, lo, hi);
  return { c: r.t0 + r.span / 2 - now, span: r.span };
};
export function viewZoom(v, k, f, now, lo, hi) {
  const r = viewRange(v, now, lo, hi), at = r.t0 + f * r.span;
  const span = Math.min(Math.max(SCHED_MIN_SPAN, hi - lo), Math.max(SCHED_MIN_SPAN, r.span / k));
  return viewOf(at - f * span, span, now, lo, hi);
}
export function viewPan(v, dt, now, lo, hi) {
  const r = viewRange(v, now, lo, hi);
  return viewOf(r.t0 + dt, r.span, now, lo, hi);
}

// The forecast's runs numbered as `/work/tocome` numbers them (forecast::Runs): the lanes by name
// in byte order (Rust's BTreeMap of strings: UTF-8 bytes), each lane's runs in order. {first: {lane:
// its first run's number}, total}.
const utf8 = new TextEncoder();
const byBytes = (p, q) => {
  const a = utf8.encode(p), b = utf8.encode(q);
  for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] - b[i];
  return a.length - b.length;
};
export function runNumbers(lanes) {
  const first = {};
  let total = 0;
  for (const nm of Object.keys(lanes || {}).sort(byBytes)) {
    first[nm] = total;
    total += lanes[nm].length;
  }
  return { first, total };
}
// The runs' dependencies (`/work/tocome`'s `deps`, [a, b]: b waits on a) both ways: `on` each
// run's runs it waits on, `then` those waiting on it.
export function runLinks(deps) {
  const on = new Map(), then = new Map();
  for (const [a, b] of deps || []) {
    if (!on.has(b)) on.set(b, []);
    if (!then.has(a)) then.set(a, []);
    on.get(b).push(a);
    then.get(a).push(b);
  }
  return { on, then };
}

// Where to ask `/work/history` from (its `since`) to reach back to time `from`, given the oldest
// event held (`first`, {seq, t}): up to `chunk` events before it; null once it's reached (`first.t`
// at or before `from`, or the first event) or with nothing held.
export function pastFrom(first, from, chunk = 5000) {
  if (!first || first.t <= from || first.seq <= 1) return null;
  return Math.max(0, first.seq - 1 - chunk);
}

// ---- The overview's map -------------------------------------------------------------------------
// A target's tile ("z/x/y", crate::coord's cost key for a tile step; or the map's key for it,
// "<step> z/x/y") as {z, x, y}; null for any other target (a worldwide step's, an input's).
// (The map keys a tile by its step too: two steps' at one spot are two entries, #615.)
export function tileOf(t) {
  const m = /^(?:\S+ )?(\d{1,2})\/(\d+)\/(\d+)$/.exec(String(t));
  if (!m) return null;
  const [z, x, y] = [+m[1], +m[2], +m[3]];
  return x < 2 ** z && y < 2 ** z ? { z, x, y } : null;
}
// A tile's square on the map (Web Mercator, the world 0–1000 each way, y down): [x0, y0, side].
export const tileBox = ({ z, x, y }) => { const s = 1000 / 2 ** z; return [x * s, y * s, s]; };
// The tile areas (#659), by the packs the steps make at each zoom (docs/steps.md, task #639): the
// root (z0–2, one tile: 0/0/0), lo (a z3 tile's), hi (a z6 tile's) and deep (a z10 tile's), each
// [name, zoom]. A tile's area is its zoom's.
export const AREAS = [["root", 0], ["lo", 3], ["hi", 6], ["deep", 10]];
export const areaOf = (t) => { const z = tileOf(t)?.z; return AREAS.find(([, k]) => k === z)?.[0] ?? `z${z}`; };
// A job's targets as the map draws them: a root step's (`<layer>-root`, worldwide: its target its
// own name) is the root tile, 0/0/0; any other's as they are.
export const areaTargets = (step, targets) => (/-root$/.test(step || "") ? ["0/0/0"] : targets || []);
const tileWords = (s) => String(s || "").split(/\s+/).filter((w) => tileOf(w));

// A running job's targets: a lead's or helper's job (`{step, id, started}`) as its "start" event
// names them (its step's, within 10 s of its start: its id names the first alone), a leased one
// (`{lease, what}`) as its "lease" event does; else the tiles its id or words name.
export function targetsOf(run, events) {
  return startOf(run, events)?.targets || tileWords(run.lease != null ? run.what : String(run.id || "").split(" ").slice(1).join(" "));
}
// A running job's "start" or "lease" event (as `targetsOf` finds it), null: none held. A start of
// its step within 10 s of it: its slot's own (`worker`) if one is, else the last.
export function startOf(run, events) {
  const last = (ok) => { for (let i = (events || []).length - 1; i >= 0; i--) if (events[i].targets?.length && ok(events[i])) return events[i]; return null; };
  if (run.lease != null) return last((v) => v.kind === "lease" && v.lease === run.lease);
  const near = (v) => v.kind === "start" && v.step === run.step && Math.abs(v.t - run.started) <= 10;
  return (run.worker != null && last((v) => near(v) && v.worker === run.worker)) || last(near);
}

// Each running job's tiles, by key ("<step> z/x/y"): {step, frac (0–1 of the tile done), current (the
// one under way), run}. From its live data (crate::agent::jobs::Live) where it has some: its
// targets done (taken as the first of its list, its current one aside) whole, the one under way
// at its time so far against the step's time a target (`each_s`, under 95% until it ends), the
// rest none; else from the beat's `frac` of the whole job, spread over its tiles in order; else
// none. A tile two runs hold keeps the further one. `runs`: [{step, targets, live, frac}].
export function runningTiles(runs, eachS, now) {
  const out = new Map();
  for (const r of runs) {
    const tiles = (r.targets || []).filter((t) => tileOf(t));
    if (!tiles.length) continue;
    const l = r.live, fr = {};
    if (l && (l.targets || l.target)) {
      let done = l.targets_done || 0;
      for (const t of tiles) {
        if (t === l.target) {
          const each = eachS?.[r.step];
          fr[t] = each && l.target_since != null ? Math.min(0.95, Math.max(0, (now - l.target_since) / each)) : 0;
        } else fr[t] = done-- > 0 ? 1 : 0;
      }
    } else {
      const k = Math.max(0, Math.min(1, r.frac || 0)) * tiles.length;
      tiles.forEach((t, i) => { fr[t] = Math.max(0, Math.min(1, k - i)); });
    }
    for (const t of tiles) {
      const k = `${r.step} ${t}`, had = out.get(k);
      if (!had || fr[t] > had.frac) out.set(k, { step: r.step, frac: fr[t], current: t === l?.target, run: r });
    }
  }
  return out;
}

// The finished tiles, by key ("<step> z/x/y"), each {t, step, worker} (#674): every tile the build's
// keys record as built (`built`, `/work/built`'s steps: {step: [z/x/y]}, crate::agent::builtview),
// whenever it was, and those jobs finished well at or after `from` (not merged into the keys yet);
// the time and worker of the last of the history's ends held for it (a lead's "end", a worker's
// "done"), at any time; a recorded tile with none held, null for both, and `before`: `heldFrom`,
// the oldest event the page holds (its end, not held, came before it). A recorded tile that made
// nothing to publish (`empty`, `/work/built`'s: {step: [index into its steps list]}, #839: a deep
// tile with no fine data there) has `none`, kept when an end held is taken for it.
export function finishedTiles(ends, from, built = {}, heldFrom = null, empty = {}) {
  const out = new Map();
  for (const [step, tiles] of Object.entries(built || {})) {
    const none = new Set(empty?.[step] || []);
    tiles.forEach((t, i) => { if (tileOf(t)) out.set(`${step} ${t}`, none.has(i) ? { t: null, before: heldFrom, step, worker: null, none: true } : { t: null, before: heldFrom, step, worker: null }); });
  }
  for (const e of ends || []) {
    if ((e.kind !== "end" && e.kind !== "done") || e.ok === false) continue;
    for (const t of areaTargets(e.step, e.targets)) {
      const k = `${e.step} ${t}`, had = out.get(k);
      if (tileOf(t) && (e.t >= from || had) && !(had?.t > e.t)) out.set(k, had?.none ? { t: e.t, step: e.step, worker: e.worker, none: true } : { t: e.t, step: e.step, worker: e.worker });
    }
  }
  return out;
}

// A tile's area in degrees: "45.1°–48.9° N, 5.6°–11.3° E" (south and west negative).
export function tileCovers({ z, x, y }) {
  const n = 2 ** z, lon = (v) => (v / n) * 360 - 180, lat = (v) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * v) / n))) * 180) / Math.PI;
  return `${lat(y + 1).toFixed(1)}°–${lat(y).toFixed(1)}° N, ${lon(x).toFixed(1)}°–${lon(x + 1).toFixed(1)}° E`;
}

// The queue (task #685): every tile the lead's last forecast places, by key ("<step> z/x/y"), each
// {step, at (unix s, its first start), mac}: `/work/tocome`'s answer ({at, macs: {mac: {step:
// [[z/x/y, s from at]…]}}}, forecast::ToCome), the earliest where two Macs name one.
export function queuedTiles(a) {
  const out = new Map();
  for (const [mac, steps] of Object.entries(a?.macs || {})) {
    for (const [step, tiles] of Object.entries(steps)) {
      for (const [t, off] of tiles) {
        const k = `${step} ${t}`, at = a.at + off;
        if (tileOf(t) && !(out.get(k)?.at <= at)) out.set(k, { step, at, mac });
      }
    }
  }
  return out;
}

// A map tile's state (#627, #685): "queued" (to build: the forecast's, or a running job's tile
// not begun), "progress" (under way), "completed" (its job's work on it done, or finished, and not
// in a catalogue yet: a finish after the last catalogue's time `pubAt`), "published" (finished
// by `pubAt`: in the app; a recorded tile whose end isn't held, `t` null, finished before the
// oldest event held, `before`: published when that's by `pubAt`, else completed, as it can't be
// told) or "empty" (built, nothing to publish: its job made no file to publish, `none`, #839;
// told before the catalogue's time, as no catalogue will ever name it).
export const TILE_STATES = ["queued", "progress", "completed", "published", "empty"];
// The states the map hides until toggled on in the legend (`f.tiles[state] === true`).
export const HIDDEN_STATES = ["empty"];
export const runningState = (r) => (r.frac >= 1 ? "completed" : r.frac > 0 || r.current ? "progress" : "queued");
export const finishedState = (d, pubAt) => {
  if (d.none) return "empty";
  const by = d.t ?? d.before;
  return pubAt != null && by != null && by <= pubAt ? "published" : "completed";
};

// The world map's tiles (#615, #627, #685): `running`, `queued` and `done` (runningTiles',
// queuedTiles', finishedTiles') as the map's view keeps them, each given its `state`; a tile in
// more than one is the running one, else the queued one (built before, to be built again: its
// finished tile not drawn, the queued one `built: true`: built before, #833). Kept: the states shown (`f.tiles[state] === false`: hidden), the pool member
// `f.member` (the Mac that ran, runs or will run it: `macOf(worker)` its host), the tile areas not
// toggled off (`f.offAreas`: AREAS' names), and with `solo` (step ids) those steps' alone. The
// published tiles are apart in `published` (drawn only for a solo, #833), whatever `f.tiles` says;
// those built with nothing to publish ("empty") are finished ones, shown only when toggled on
// (HIDDEN_STATES). Returns {running, queued, finished, published}, new Maps.
export function mapTiles(running, queued, done, f, { pubAt = null, macOf = (w) => w, solo = null } = {}) {
  const shown = (st) => (HIDDEN_STATES.includes(st) ? f.tiles?.[st] === true : f.tiles?.[st] !== false);
  const keep = (key, step, who) => (!solo || solo.includes(step)) && !f.offAreas?.includes(areaOf(key)) && (!f.member || (who != null && macOf(who) === f.member));
  const out = { running: new Map(), queued: new Map(), finished: new Map(), published: new Map() };
  for (const [k, r] of running) {
    const state = runningState(r);
    if (shown(state) && keep(k, r.step, r.run?.mac)) out.running.set(k, { ...r, state });
  }
  for (const [k, q] of queued) {
    if (running.has(k) || !shown("queued") || !keep(k, q.step, q.mac)) continue;
    out.queued.set(k, done.has(k) ? { ...q, state: "queued", built: true } : { ...q, state: "queued" });
  }
  for (const [k, t] of done) {
    const state = finishedState(t, pubAt);
    if (running.has(k) || queued.has(k) || !keep(k, t.step, t.worker)) continue;
    if (state === "published") out.published.set(k, { ...t, state });
    else if (shown(state)) out.finished.set(k, { ...t, state });
  }
  return out;
}

// When a queued tile was published before (#833): the last catalogue kept that published its
// output, `at(tile)` (catalogues.js publishedAt: {t, n, first}), or null (never, as far as the
// catalogues kept tell). A deep tile (z10) by its own packs' catalogue: a catalogue names a new
// file of them only once its job wrote it, so that's its output published, record or not. A
// shallower one's catalogue is its z3 or z6 tile's (any step's files there): counted only when the
// keys record it built (`q.built`), never for a tile not built yet.
export function publishedBefore(key, q, at) {
  const tile = tileOf(key);
  if (!tile || (tile.z < 10 && !q.built)) return null;
  return at(tile) || null;
}

// Whether point (x, y) is inside rings (flat [x0, y0, x1, y1, …] each, closed), nonzero, as the
// map fills them.
export function inRings(rings, x, y) {
  let wind = 0;
  for (const r of rings || []) {
    for (let i = 0; i + 3 < r.length; i += 2) {
      const [ax, ay, bx, by] = [r[i], r[i + 1], r[i + 2], r[i + 3]];
      const side = (bx - ax) * (y - ay) - (x - ax) * (by - ay);
      if (ay <= y) { if (by > y && side > 0) wind++; } else if (by <= y && side < 0) wind--;
    }
  }
  return wind !== 0;
}

// The area a region's fill rings cover (flat, as inRings): each ring's own, summed. A region's
// fill holds its shapes' outer rings only (agent::regionsview outline), wound as each source
// wound them, so each ring counts whole whatever its winding (a signed sum let shapes wound
// opposite ways cancel out).
export function ringsArea(rings) {
  let sum = 0;
  for (const r of rings || []) {
    let a = 0;
    for (let i = 0; i + 3 < r.length; i += 2) a += r[i] * r[i + 3] - r[i + 2] * r[i + 1];
    sum += Math.abs(a / 2);
  }
  return sum;
}

// The one region a point picks among those whose rings hold it (#831): the innermost, the
// smallest, an island over the waters drawn round it, an enclave over the country round it.
// [{id, area}] (area: ringsArea, kept per region) → its id, or null.
export function topRegion(hits) {
  let best = null, bestA = Infinity;
  for (const h of hits) if (h.area < bestA) { best = h.id; bestA = h.area; }
  return best;
}

// ---- The Regions tab ------------------------------------------------------------------------------
// A region's work left, from the forecast's `left` (its targets left by step: agent::build::RegionLeft
// as the lead counts its forecast input): a step at a time in `order` (the steps table's; others
// after, by name), none for a step with nothing left, each with its time at the step's measured
// seconds a target (`eachS`, 0 where unmeasured): [{step, k, s}].
export function regionWork(left, eachS, order = []) {
  const at = (s) => { const i = order.indexOf(s); return i < 0 ? order.length : i; };
  return Object.entries(left || {}).filter(([, k]) => k > 0)
    .sort(([p], [q]) => at(p) - at(q) || p.localeCompare(q))
    .map(([step, k]) => ({ step, k, s: k * (eachS?.[step] || 0) }));
}

// A region's work by stage (`stageOf(step)`), in `stages`' order (others after): [{stage, k, s}],
// its stage bar's segments.
export function stageSegments(work, stageOf, stages = []) {
  const by = new Map();
  for (const w of work) {
    const g = stageOf(w.step) || "";
    const had = by.get(g) || { stage: g, k: 0, s: 0 };
    had.k += w.k;
    had.s += w.s;
    by.set(g, had);
  }
  const at = (g) => { const i = stages.indexOf(g); return i < 0 ? stages.length : i; };
  return [...by.values()].sort((a, b) => at(a.stage) - at(b.stage));
}

// A region's targets built of all by stage (`stageOf(step)`), from its steps' counts as the lead
// gives them (`/work/regions` with `steps`: {step: [built, all]}): {stage: [built, all]}.
export function stageFill(steps, stageOf) {
  const out = {};
  for (const [s, [b, a]] of Object.entries(steps || {})) {
    const g = stageOf(s) || "";
    const had = out[g] || [0, 0];
    out[g] = [had[0] + b, had[1] + a];
  }
  return out;
}

// Every region's targets left, summed by step: {step: k}.
export function stepTotals(regions) {
  const out = {};
  for (const r of regions || []) for (const [s, k] of Object.entries(r.left || {})) if (k > 0) out[s] = (out[s] || 0) + k;
  return out;
}

// The regions the list shows: those whose id or any of their folders' names (`crumbs`, else
// `name`) has `q` (any case), ordered by `order`: "map" (as they reach the map: its time, then
// those with nothing left on the map, each by rank), "name" (A–Z) or "left" (most time left
// first: its `work`'s seconds, then its targets). Each: {id, name, crumbs?, rank?, map_at?, on_map?, work?}.
export function regionsShown(regs, order, q) {
  const f = String(q || "").trim().toLowerCase();
  const list = regs.filter((r) => !f || r.id.toLowerCase().includes(f) || (r.crumbs?.length ? r.crumbs : [r.name]).some((c) => String(c).toLowerCase().includes(f)));
  const secs = (r) => (r.work || []).reduce((t, w) => t + w.s, 0);
  const count = (r) => (r.work || []).reduce((t, w) => t + w.k, 0);
  const toMap = (r) => (count(r) || r.on_map !== true ? r.map_at || 1e12 : 2e12) + (r.rank ?? 1e6);
  const byName = (x, y) => x.name.localeCompare(y.name);
  if (order === "name") return list.sort(byName);
  if (order === "left") return list.sort((x, y) => secs(y) - secs(x) || count(y) - count(x) || byName(x, y));
  return list.sort((x, y) => toMap(x) - toMap(y) || byName(x, y));
}

// A virtual list's rows to draw: those in the box scrolled to `top` (`height` tall, rows `rowH`
// each, `n` of them), `over` more each side: [from, to).
export function rowsWindow(top, height, rowH, n, over = 6) {
  const from = Math.max(0, Math.floor(top / rowH) - over);
  return [Math.min(n, from), Math.min(n, Math.ceil((top + height) / rowH) + over)];
}

// ---- A step's time by phase over the charts' range (task #844) ------------------------------------
// The lead's `phase_times` (crate::timings::PhaseTimes: each step's runs summed by the first of
// `edges` their end is within) over the footer's range `range` (seconds): the runs that ended within
// it (the sums of the edges up to it), each phase's wall and CPU seconds and NAS and internet MB
// summed, and their time in no phase (`untimed`). Null when the step ran none in the range.
export function phaseBreakdown(pt, step, range) {
  const st = pt?.steps?.[step];
  if (!st) return null;
  const upto = (pt.edges || []).reduce((k, e, i) => (e <= range ? i : k), -1);
  const out = { runs: 0, failed: 0, wall: 0, cpu: 0, parts: st.phases.map(([name, cls]) => ({ name, cls, wall: 0, cpu: 0, nas: 0, net: 0 })) };
  for (const [e, runs, failed, wall, cpu, ps] of st.b) {
    if (e > upto) continue;
    out.runs += runs; out.failed += failed; out.wall += wall; out.cpu += cpu;
    for (const [i, w, c, nas, net] of ps) { const p = out.parts[i]; if (!p) continue; p.wall += w; p.cpu += c; p.nas += nas; p.net += net; }
  }
  if (!out.runs) return null;
  out.parts = out.parts.filter((p) => p.wall > 0).sort((a, b) => b.wall - a.wall);
  out.untimed = Math.max(0, out.wall - out.parts.reduce((t, p) => t + p.wall, 0));
  return out;
}

// What a phase waits on, by its class (crate::timings::Class), for the time breakdown's tooltips.
export const PHASE_CLASS = {
  "nas-read": "reading the NAS", "nas-write": "writing to the NAS", disk: "the Mac's own disk", net: "the internet (or another Mac)",
  compute: "the CPU", wait: "a lock, a slot or another worker", mixed: "several things (its sub-phases)",
};

// A line on what each phase does (the time breakdown's tooltip), the phases that take a step's time;
// another phase: its name and class say it.
export const PHASE_ABOUT = {
  "making the piece's tiles (z12 → z9)": "Samples the terrain pool's datasets for a z9 piece's tiles, levels the lakes, shades and encodes them as WebP.",
  "making its deep tiles (the fine datasets read)": "Samples the fine datasets (HRDEM, 3DEP and others) for the z10 tile's deep levels, blends their edges and brings them to EGM2008.",
  "making z8 tiles' tiles here": "Makes the z8 tiles' slope tiles on this Mac from the terrain it reads.",
  "drawing the coverage and finding the credits": "Draws the catalogue's coverage and finds the credits its tiles need.",
  "composing": "Composes the depth tiles from the sources' windows: fills the gaps, scales stored levels up, feathers the seams.",
  "waiting for the uploads beside to land": "Waits for the uploads sent beside the job (packs, tiles) to land on the NAS.",
  "working out slope tiles (the terrain read as needed)": "Works out the slope tiles from the terrain, read from its packs as needed.",
  "loading z12 terrain tiles (packs, else AWS)": "Loads the z12 terrain tiles from the packs, else from AWS's Terrarium tiles.",
  "uploading its deep pack": "Uploads the z10 tile's deep pack to the NAS.",
  "waiting for the next z6 tile's z12 tiles": "Waits for the next z6 tile's z12 terrain tiles, being copied ahead.",
  "reading the regions and their outlines": "Reads the regions and parses their outlines into the coverage's grid: every step's run pays it.",
  "making the z3 tile's tiles (z8 → z3)": "Makes a z3 tile's coarser levels, z8 down to z3, from the finer.",
  "opening the basemap's water": "Opens the basemap's water (on the NAS) for the tiles' lakes and coasts.",
  "sampling and making the world's z8 tiles": "Samples the world's z8 terrain tiles from the pool and makes them, once a build.",
  "reading and checking the changes": "Reads the inputs' changes and checks them before they're accepted.",
  "reading the fine datasets' declarations": "Reads the fine datasets' declarations from the NAS: what covers the tile.",
  "getting the z6 tiles' base packs here": "Copies the z6 tiles' base packs from the NAS to this Mac.",
  "staging layers from the packs": "Stages a z6 tile's layers from its packs on the NAS for the scenic programs.",
  "shading the floor": "Shades the sea floor's relief for the depth tiles.",
  "blending the edges": "Blends the datasets' heights where they meet.",
  "encoding tiles": "Encodes the tiles (WebP).",
  "keeping scenic results": "Keeps the scenic programs' results on the NAS.",
  "copying its terrain pack here": "Copies the z6 tile's terrain pack from the NAS to this Mac.",
  "reading the basemap's water": "Reads the basemap's water for the tiles.",
  "reading the pass, coverage, pieces and reaches": "Reads the OSM pass, the coverage, the pieces and the reaches from the NAS.",
  "copying pieces from the NAS": "Copies the z6 tile's OSM pieces from the NAS to this Mac.",
  "extracting candidates (extract)": "Extracts the candidate features from the pieces (the extract program).",
  "drawing road and rail tiles": "Draws the road and rail vector tiles.",
  "saving records": "Saves the job's records (its outputs' keys) to the NAS.",
  "reading records": "Reads the build's records from the NAS.",
  "waiting for the copying ahead": "Waits for the copies made ahead (the next tile's inputs).",
  "writing packs": "Writes the packs on this Mac.",
  "making the coarser levels from the finer": "Makes the coarser zooms from the finer ones, a plain mean.",
  "packing new raw tiles onto the NAS": "Packs the raw terrain tiles just fetched onto the NAS's archives.",
  "uploading the candidates": "Uploads the candidates to the NAS.",
  "reading the depth sources' declarations": "Reads the depth sources' declarations from the NAS.",
  "finding the block's sources": "Finds the sources that cover the block.",
  "reading the depth sources' windows": "Reads the depth sources' windows over the tile, from the NAS.",
  "opening the depth source files": "Opens the depth sources' files on the NAS.",
  "making whole local copies": "Copies whole source files from the NAS to this Mac to read them locally.",
  "sampling glo30": "Samples Copernicus GLO-30 heights.",
  "sampling hrdem": "Samples Canada's HRDEM heights.",
  "sampling hrdem-1m": "Samples Canada's HRDEM 1 m heights.",
  "offering z8 tiles to other workers": "Offers the z8 tiles to other workers, and takes back what they made.",
  "offering the z6 tile's subtrees to other workers": "Offers the z6 tile's subtrees to other workers, and takes back what they made.",
  "fetching its files from the coordinator": "Fetches the task's files from the coordinator.",
  "sending back what they wrote": "Sends the task's outputs back to the coordinator.",
  "uploading the lo pack": "Uploads the lo pack to the NAS.",
  "reading the pieces' mids": "Reads the pieces' mids from the NAS.",
  "other phases": "The step's smaller phases, summed.",
};
