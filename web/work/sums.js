// The build page's sums, apart from the page so Node tests them (tools/check/worksums.test.mjs).

// A stage's or step's line finished: nothing left (build::Line, build::Item).
export const finished = (x) => (x.left != null ? x.left === 0 : x.total != null && x.done >= x.total && x.total > 0);

// The whole build's progress, every stage's: the work done of the whole, in time at the lead's
// pace. Done: each step's targets done (the stages' items) at the forecast's time for a target
// of it (`each_s`, measured or estimated); left: the forecast's work left (its steps' `work_s`,
// what "of work left" says). 100 only when every stage is finished and the forecast has nothing
// left; null without a forecast that weighs (an older lead's: no `each_s`).
// Returns { pct, done_s, left_s } or null.
export function wholeProgress(lines, fc) {
  if (!fc?.each_s) return null;
  let done = 0;
  for (const l of lines || []) {
    const items = l.items?.length ? l.items : (l.steps || []).slice(0, 1).map((step) => ({ step, done: l.done || 0 }));
    for (const it of items) done += (it.done || 0) * (fc.each_s[it.step] || 0);
  }
  const left = (fc.steps || []).reduce((t, s) => t + (s.work_s || 0), 0);
  const all = (lines || []).length > 0 && lines.every(finished) && left === 0;
  if (all) return { pct: 100, done_s: done, left_s: 0 };
  const whole = done + left;
  const pct = whole > 0 ? Math.min(99, Math.floor((done / whole) * 100)) : 0;
  return { pct, done_s: done, left_s: left };
}


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

// The jobs that ended at or after `from`, as the history has them: each job's end (kind "end": its
// worker the slot, `t` its end, `secs` how long it ran) as a run {lane, step, from, until, n, ok,
// what, targets, seq}, by start. An end without its time or step is passed over; an event given
// twice (by its seq), once.
export function pastRuns(events, from) {
  const seen = new Set(), out = [];
  for (const e of events || []) {
    if (e.kind !== "end" || !e.step || e.secs == null || !e.worker || e.t < from || seen.has(e.seq)) continue;
    seen.add(e.seq);
    out.push({ lane: e.worker, step: e.step, from: e.t - e.secs, until: e.t, n: (e.targets || []).length, ok: e.ok !== false, what: e.what || "", targets: e.targets || [], seq: e.seq });
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

// Where to ask `/work/history` from (its `since`) to reach back to time `from`, given the oldest
// event held (`first`, {seq, t}): up to `chunk` events before it; null once it's reached (`first.t`
// at or before `from`, or the first event) or with nothing held.
export function pastFrom(first, from, chunk = 5000) {
  if (!first || first.t <= from || first.seq <= 1) return null;
  return Math.max(0, first.seq - 1 - chunk);
}
