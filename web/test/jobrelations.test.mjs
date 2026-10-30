// node web/test/jobrelations.test.mjs (tools/dev/check.sh's web part): the build page's schedule
// ties a hovered job to single jobs, by `/work/tocome`'s job-to-job dependencies (from the targets'
// readiness, task #879) and the history's `after` (web/work/sums.js jobRelations, #884).
import assert from "node:assert/strict";
import { jobLinks, jobNumbers, jobRelations } from "../work/sums.js";

const job = (kind, no, r = {}) => ({ kind, job: no, r });
// Jobs 0–1 (a run of two), 2–8 (a run of seven: job 5 waits on job 1, the others on job 0, and on
// a job that ran, seq 5, in their `after`), 9 (waiting on job 5 alone).
assert.deepEqual(jobNumbers([2, 7, 1]), { first: [0, 2, 9], total: 10 });
const jobs = Array.from({ length: 10 }, (_, i) => job("next", i, i >= 2 && i <= 8 ? { after: [5] } : {}));
const ran = job("past", null, { seq: 5 });
const jobSegs = new Map(jobs.map((s) => [s.job, s]));
const all = [ran, ...jobs];
const links = jobLinks([[0, 2], [0, 3], [0, 4], [1, 5], [0, 6], [0, 7], [0, 8], [5, 9]]);

// Job 5: itself, the one job it waits on, the one waiting on it, the job that ran: no row.
{
  const s = jobs[5];
  const { on, arrows } = jobRelations(s, all, jobSegs, links);
  assert.deepEqual(new Set(on), new Set([s, jobs[1], jobs[9], ran]));
  assert.deepEqual(arrows, [[jobs[1], s], [s, jobs[9]], [ran, s]]);
}
// Job 0: to each job reading what it makes, not job 5.
{
  const { on, arrows } = jobRelations(jobs[0], all, jobSegs, links);
  assert.deepEqual(on, [jobs[0], ...[2, 3, 4, 6, 7, 8].map((i) => jobs[i])]);
  assert.equal(arrows.length, 6);
}
// The job that ran: to each job naming it in its `after`.
{
  const { on } = jobRelations(ran, all, jobSegs, links);
  assert.deepEqual(on, [ran, ...jobs.slice(2, 9)]);
}
// A job with nothing tied: itself alone.
{
  const lone = job("next", 42);
  assert.deepEqual(jobRelations(lone, [lone], new Map([[42, lone]]), jobLinks([])), { on: [lone], arrows: [] });
}
// A running job (seq 7) as job 3, which job 4 waits on by the forecast and by `after`: one arrow.
{
  const run = job("running", 3, { seq: 7 });
  const next = job("next", 4, { after: [7] });
  const { on, arrows } = jobRelations(run, [run, next], new Map([[3, run], [4, next]]), jobLinks([[3, 4]]));
  assert.deepEqual(on, [run, next]);
  assert.deepEqual(arrows, [[run, next]]);
}
