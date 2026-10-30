// node web/test/jobrelations.test.mjs (tools/dev/check.sh's web part): the build page's schedule
// ties a hovered job to single jobs, never to its run's whole row (web/work/sums.js jobRelations, #884).
import assert from "node:assert/strict";
import { jobRelations, runLinks } from "../work/sums.js";

const job = (kind, run, r = {}) => ({ kind, run, r });
// Run 0: two jobs to come; run 1: seven jobs to come, waiting on run 0 and on a job that ran
// (seq 5, in their `after`); run 2: three jobs to come, waiting on run 1.
const r0 = [job("next", 0), job("next", 0)];
const r1 = Array.from({ length: 7 }, () => job("next", 1, { after: [5] }));
const r2 = [job("next", 2), job("next", 2), job("next", 2)];
const ran = job("past", null, { seq: 5 });
const runSegs = new Map([[0, r0], [1, r1], [2, r2]]);
const all = [ran, ...r0, ...r1, ...r2];
const links = runLinks([[0, 1], [1, 2]]);

// A middle job of run 1: itself, run 0's last job, run 2's first job, the job that ran: no row.
{
  const s = r1[3];
  const { on, arrows } = jobRelations(s, all, runSegs, links);
  assert.deepEqual(new Set(on), new Set([s, r0[1], r2[0], ran]));
  assert.equal(on.filter((x) => x.run === 1).length, 1);
  assert.deepEqual(arrows, [[r0[1], s], [s, r2[0]], [ran, s]]);
}
// The job that ran: to run 1's first job only, not its seven.
{
  const { on, arrows } = jobRelations(ran, all, runSegs, links);
  assert.deepEqual(on, [ran, r1[0]]);
  assert.deepEqual(arrows, [[ran, r1[0]]]);
}
// A job with nothing tied: itself alone.
{
  const lone = job("next", 9);
  assert.deepEqual(jobRelations(lone, [lone], new Map([[9, [lone]]]), runLinks([])), { on: [lone], arrows: [] });
}
