// The quiet-Mac gate for the field's timings (task #102): a GPU timing taken while other work runs
// isn't the renderer's. The build Mac shares its GPU's memory with the CPU, and builds heat it; on
// 10 Oct, under other agents' builds (1-minute load 110–150), Manhattan at 70° timed 5.3 ms in one
// session and 8.1–8.3 in another, the same code.
//
// Clear when:
//   - none of cargo, rustc, ld (ld64) or a pipeline test binary (pipeline-<hash>) runs, nor any process
//     whose command line holds `scenic-build` (the real agent's build jobs);
//   - no other process takes more than BUSY_CPU % of a core (the harness's own Chrome, node and the
//     window server excepted);
//   - the 1-minute load average is under MAX_LOAD (BLD_MAX_LOAD): the Mac has 14 cores (10
//     performance, 4 efficiency); idle with the desktop apps it sits around 1–2, a single rustc adds 1,
//     a cargo build 8 or more, so 3 tells a quiet desktop from any build while leaving room for the
//     harness itself (Chrome's GPU process and the page, node);
//   - `pmset -g therm` records no thermal or performance warning (and a CPU speed limit, where it's
//     given, of 100).
// snapshot() says what it found; waitQuiet() polls until clear and gives up after a limit, with the
// reasons.
import { execFileSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';

export const MAX_LOAD = Number(process.env.BLD_MAX_LOAD ?? 3);
export const BUSY_CPU = Number(process.env.BLD_BUSY_CPU ?? 80);
const BUILD = [/^cargo$/, /^rustc$/, /^ld$/, /^ld64(\.lld)?$/, /^pipeline-[0-9a-f]{8,}$/];
const HARNESS = [/Google Chrome/, /^node$/, /^WindowServer$/, /^ps$/, /^pmset$/];

/** What's running, the load and the thermal state now, and whether that's quiet. */
export function snapshot() {
  const reasons = [];
  const ps = execFileSync('ps', ['-Ao', 'pid=,pcpu=,comm=,args='], { encoding: 'utf8', maxBuffer: 1 << 26 });
  const builds = [], busy = [];
  for (const line of ps.split('\n')) {
    const m = line.match(/^\s*(\d+)\s+([\d.]+)\s+(\S+)\s*(.*)$/);
    if (!m) continue;
    const [, pid, pcpu, comm, args] = m;
    if (Number(pid) === process.pid) continue;
    const name = path.basename(comm);
    if (BUILD.some((r) => r.test(name)) || /scenic-build/.test(args)) builds.push({ pid: Number(pid), name, cpu: Number(pcpu), args: args.slice(0, 160) });
    else if (Number(pcpu) > BUSY_CPU && !HARNESS.some((r) => r.test(comm) || r.test(name))) busy.push({ pid: Number(pid), name, cpu: Number(pcpu) });
  }
  if (builds.length) reasons.push(`build jobs running: ${builds.map((b) => `${b.name} (${b.pid}, ${b.cpu}%)`).join(', ')}`);
  if (busy.length) reasons.push(`busy processes: ${busy.map((b) => `${b.name} (${b.pid}, ${b.cpu}%)`).join(', ')}`);
  const load = os.loadavg();
  if (load[0] >= MAX_LOAD) reasons.push(`1-minute load ${load[0].toFixed(2)} ≥ ${MAX_LOAD}`);
  let therm = '';
  try { therm = execFileSync('pmset', ['-g', 'therm'], { encoding: 'utf8' }); } catch (e) { therm = `pmset failed: ${e.message}`; }
  const thermLines = therm.split('\n').map((l) => l.trim()).filter(Boolean);
  const thermBad = thermLines.filter((l) => {
    if (/No .* (has been )?recorded/i.test(l)) return false;
    const sl = l.match(/Speed_Limit\s*=\s*(\d+)/i);
    if (sl) return Number(sl[1]) < 100;
    const lv = l.match(/warning level\D*(\d+)/i);
    if (lv) return Number(lv[1]) > 0;
    return /warning|limit|failed/i.test(l);
  });
  if (thermBad.length) reasons.push(`thermal: ${thermBad.join(' | ')}`);
  return { ok: reasons.length === 0, at: new Date().toISOString(), load, builds, busy, therm: thermLines, reasons };
}

/** Waits until the Mac is quiet (polling every pollMs), or throws after maxWaitMs with the reasons. */
export async function waitQuiet({ maxWaitMs = 20 * 60e3, pollMs = 15e3, log = console.log } = {}) {
  const t0 = Date.now();
  let s = snapshot(), said = '';
  while (!s.ok) {
    const why = s.reasons.join('; ');
    if (why !== said) {
      log(`waiting for a quiet Mac: ${why}`);
      said = why;
    }
    if (Date.now() - t0 > maxWaitMs) throw new Error(`gave up after ${Math.round((Date.now() - t0) / 60e3)} min waiting for a quiet Mac: ${why}`);
    await new Promise((r) => setTimeout(r, pollMs));
    s = snapshot();
  }
  return s;
}

if (process.argv[1] && import.meta.url.endsWith(path.basename(process.argv[1]))) console.log(JSON.stringify(snapshot(), null, 1));
