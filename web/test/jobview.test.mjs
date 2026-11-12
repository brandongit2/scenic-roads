// A job's page's sums (web/buildpage/jobview.js) and a run's lanes (runlanes.js), task #1105.
import { test } from "node:test";
import assert from "node:assert/strict";
import { clockText, crossesMidnight, markers, median, outcome, recordFigures, spanLook, stepMedians, timeWords, tplus, linesIn } from "../buildpage/jobview.js";
import { lineage, pack, spanLanes, spanTree } from "../buildpage/runlanes.js";
import { jobHref, pastRuns, packRuns, segHref } from "../buildpage/sums.js";

// (Local times, whatever this Mac's zone: made from their parts.)
const local = (y, mo, d, h, mi, s, ms = 0) => new Date(y, mo - 1, d, h, mi, s, ms).getTime() / 1000;

test("T+ times: minutes and seconds, hours only when needed, milliseconds when asked", () => {
  assert.equal(tplus(301.158, { ms: true }), "T+05:01.158");
  assert.equal(tplus(301.158), "T+05:01");
  assert.equal(tplus(3905), "T+1:05:05");
  assert.equal(tplus(65, { hours: true }), "T+0:01:05");
  assert.equal(tplus(-3), "T−00:03");
  assert.equal(tplus(59.9996, { ms: true }), "T+01:00.000");
});

test("clock times: the day's time, its milliseconds, its date when asked", () => {
  const t = local(2026, 11, 12, 23, 59, 58, 158);
  assert.equal(clockText(t), "23:59:58");
  assert.equal(clockText(t, { ms: true }), "23:59:58.158");
  assert.equal(clockText(t, { date: true }), "Thu 12 Nov 23:59:58");
  assert.ok(crossesMidnight(t, t + 5));
  assert.ok(!crossesMidnight(t - 60, t));
});

test("the axis's ticks: round steps of the job's time, milliseconds zoomed in, a midnight named in clock times", () => {
  const o = local(2026, 11, 12, 10, 0, 0) + 0.25;
  const rel = timeWords("rel", o, o + 600);
  const a = rel.ticks(o, o + 600, 6);
  assert.deepEqual(a.map((x) => x.label), ["T+00:00", "T+02:00", "T+04:00", "T+06:00", "T+08:00", "T+10:00"]);
  const z = rel.ticks(o + 5, o + 5.05, 6);
  assert.equal(z[0].label, "T+00:05.000");
  assert.equal(z[1].label, "T+00:05.010");
  assert.equal(timeWords("rel", o, o + 4000).at(o + 61.5), "T+0:01:01.500");
  // (Clock times: on the local day's round times; a run past midnight names the day there.)
  const n0 = local(2026, 11, 12, 23, 50, 0), n1 = local(2026, 11, 13, 0, 10, 0);
  const clock = timeWords("clock", n0, n1);
  assert.ok(clock.date);
  const ts = clock.ticks(n0, n1, 5);
  assert.deepEqual(ts.map((x) => x.label), ["23:50", "23:55", "Fri 13 Nov", "00:05", "00:10"]);
  assert.deepEqual(ts.map((x) => x.day), [false, false, true, false, false]);
  assert.equal(clock.at(n0 + 1.5), "Thu 12 Nov 23:50:01.500");
});

const rec = (job, wall, cpu, peak, nas = 0) => ({ job, wall_s: wall, cpu_s: cpu, threads: 4, phases: [{ name: "a", wall_s: wall, waits: { nas_s: nas }, peak_mb: peak / 2, sub: [{ name: "b", peak_mb: peak }] }, { name: "bg", background: true, waits: { nas_s: 99 } }] });
test("a record's figures, and its step's medians without it", () => {
  assert.deepEqual(recordFigures(rec("x", 100, 250, 800, 12)), { wall: 100, cpu: 250, cores: 2.5, peak: 800, threads: 4, nas: 12, net: 0 });
  assert.equal(recordFigures(null), null);
  assert.equal(median([3, null, 1, 2]), 2);
  assert.equal(median([4, 1, 2, 3]), 2.5);
  assert.equal(median([]), null);
  const m = stepMedians([rec("me", 1000, 10, 9000), rec("a", 100, 200, 400), rec("b", 300, 300, 600)], "me");
  assert.equal(m.n, 2);
  assert.deepEqual([m.wall, m.cpu, m.peak], [200, 250, 500]);
});

test("how it ended: running, its run's word, else the history's; why from its error, exit and notes", () => {
  assert.equal(outcome({ running: true, spans: [{ id: "job", status: "running" }] }).state, "running");
  const failed = outcome({ running: false, spans: [{ id: "job", status: "fail" }], events: [{ kind: "error", text: "no grid" }, { kind: "exit", ok: false, text: "exit 1" }], history: [{ kind: "end", ok: false, note: "no grid" }] });
  assert.deepEqual([failed.state, failed.why], ["failed", ["no grid", "exit 1"]]);
  const legacy = outcome({ legacy: true, history: [{ kind: "lease" }, { kind: "fail", note: "out of memory" }] });
  assert.deepEqual([legacy.state, legacy.why], ["failed", ["out of memory"]]);
  assert.equal(outcome({ legacy: true, history: [{ kind: "lease" }] }).state, "unknown");
  assert.ok(outcome({ spans: [{ id: "job", status: "ok" }], events: [{ kind: "paused" }] }).why.includes("paused at a safe point"));
});

test("its markers: the history's lease and the run held again, its retries, safe points and end", () => {
  const m = markers({
    events: [{ kind: "safe", t: 5, t1: 9, n: 4 }, { kind: "retry", t: 3, text: "busy" }, { kind: "end", t: 10, ok: true }, { kind: "nope", t: 1 }],
    history: [{ kind: "lease", t: 0, worker: "m1" }, { kind: "lease", t: 6, worker: "m1" }, { kind: "done", t: 11, ok: true }],
  });
  assert.deepEqual(m.map((x) => [x.lane, x.kind, x.t]), [["leases", "lease", 0], ["retries", "retry", 3], ["safe points", "safe", 5], ["leases", "held again", 6], ["the end", "end", 10], ["the end", "done", 11]]);
  assert.equal(m[2].t1, 9);
  assert.ok(m[1].bad && m[1].text.endsWith("busy"));
});

test("a span's look: what it waited on most, else its CPU", () => {
  assert.equal(spanLook({ attrs: { nas_s: 3, cpu_s: 1 } }), "nas");
  assert.equal(spanLook({ attrs: { cpu_s: 2, disk_s: 1 } }), "cpu");
  assert.equal(spanLook({}), "old");
  assert.deepEqual(linesIn([{ t: 1, text: "a" }, { t: 5, text: "b" }, { t: null, text: "c" }], 0, 2).map((l) => l.text), ["a"]);
});

// A timeline's stand-in: tracks holding what's added.
const fakeTl = () => ({ track: () => ({ segs: [] }) });
test("a run's lanes: the flat spans' tree rebuilt, a group's depths, rows that don't overlap", () => {
  const spans = [
    { id: "job", parent: null, t0: 0, t1: 10 },
    { id: "1", parent: "job", kind: "phase", t0: 0, t1: 4 },
    { id: "2", parent: "job", kind: "phase", t0: 4, t1: 10 },
    { id: "3", parent: "1", kind: "phase", t0: 1, t1: 2 },
    { id: "4", parent: "gone", kind: "phase", t0: 2, t1: 2.001 },
    { id: "5", parent: "job", kind: "beside", t0: 1, t1: 3, attrs: { th: "1" } },
    { id: "6", parent: "job", kind: "beside", t0: 2, t1: 5, attrs: { th: "1" } },
  ];
  const tree = spanTree(spans, "job");
  assert.deepEqual(["1", "3", "4", "5"].map((id) => tree.depth.get(id)), [1, 2, 1, 1]);
  assert.deepEqual([...lineage(tree, "1")].sort(), ["1", "3", "job"]);
  assert.deepEqual([...lineage(tree, "3")].sort(), ["1", "3", "job"]);
  assert.deepEqual([...lineage(tree, "5")].sort(), ["5", "job"]);
  const added = [];
  const rows = spanLanes(fakeTl(), tree, { gap: 0.01, sep: 0, now: 10, outer: "group", order: ["phase"], add: (t, s, short) => added.push([s.id, short]), label: (g, d, n) => `${g} ${d} ${n}` });
  // (Phase 1 and 2 touch: one row at sep 0; the short span its own row first; the threads' two overlap: two rows.)
  assert.deepEqual(rows.map((r) => r.label), ["phase 1 3", "", "phase 2 1", "beside 1 2", ""]);
  assert.ok(added.some(([id, short]) => id === "4" && short));
  assert.deepEqual(pack([{ from: 0, until: 2 }, { from: 2, until: 3 }, { from: 1, until: 4 }], 0).map((r) => r.length), [2, 1]);
  // (A loop of parents is cut, not followed for ever.)
  assert.ok(spanTree([{ id: "a", parent: "b" }, { id: "b", parent: "a" }], "job").depth.get("a") >= 1);
});

test("the schedule's jobs link to their pages: one job's, a running one's; packed ones and those of no id none", () => {
  assert.equal(jobHref("880-17"), "job/880-17");
  assert.equal(jobHref(null), null);
  const runs = pastRuns([{ kind: "done", seq: 1, t: 100, secs: 10, step: "unit", worker: "m1", job: "5-1" }, { kind: "done", seq: 2, t: 102, secs: 1, step: "unit", worker: "m1" }], 0, ["m1"]);
  assert.deepEqual(runs.map((r) => r.job), ["5-1", null]);
  assert.equal(segHref(packRuns(runs.slice(0, 1), 1)[0], "past"), "job/5-1");
  assert.equal(segHref(packRuns(runs, 10)[0], "past"), null);
  assert.equal(segHref({ job: { job: "5-9" } }, "running"), "job/5-9");
  assert.equal(segHref({ job: {} }, "running"), null);
  assert.equal(segHref({}, "next"), null);
});
