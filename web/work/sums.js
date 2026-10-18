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
