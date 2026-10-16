// The quiet-Mac gate for the field's timings (task #102): a GPU timing taken while other work runs
// isn't the renderer's. The M4 shares its GPU's memory with the CPU, and builds heat it; on
// 10 Oct, under other agents' builds (1-minute load 110–150), Manhattan at 70° timed 5.3 ms in one
// session and 8.1–8.3 in another, the same code.
//
// Clear when:
//   - none of cargo, rustc, ld (ld64) or a pipeline test binary (pipeline-<hash>) runs, nor any process
//     whose command line holds `scenic-build` (the real agent's build jobs);
//   - other work (every process but the harness's own Chrome and node, and the window server) takes
//     under OTHER_CPU % of a core in all (BLD_OTHER_CPU, 250: two and a half of the Mac's 14 cores,
//     the desktop apps' and a background process's), and
//     no one process more than BUSY_CPU % (BLD_BUSY_CPU, 150): a build or a test suite takes many
//     cores; a single background process (the installed app's own agent works in bursts at one core)
//     is let through, and recorded;
//   - before a view only (phase 'pre', the harness idle): the 1-minute load average under MAX_LOAD
//     (BLD_MAX_LOAD, 5, the owner's limit): the Mac has 14 cores (10 performance, 4 efficiency); idle
//     with the desktop apps it sits around 2–4, a single rustc adds 1, a cargo build 8 or more. After a measurement the
//     average holds the harness's own load (Chrome drawing at 120 Hz lifts it to ~4.5 for a minute),
//     so it isn't tested then: the processes are;
//   - `pmset -g therm` records no thermal or performance warning (and a CPU speed limit, where it's
//     given, of 100).
// snapshot() says what it found; waitQuiet() polls until clear and gives up after a limit, with the
// reasons.
import { execFileSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';

export const MAX_LOAD = Number(process.env.BLD_MAX_LOAD ?? 5);
export const BUSY_CPU = Number(process.env.BLD_BUSY_CPU ?? 150);
export const OTHER_CPU = Number(process.env.BLD_OTHER_CPU ?? 250);
const BUILD = [/^cargo$/, /^rustc$/, /^ld$/, /^ld64(\.lld)?$/, /^pipeline-[0-9a-f]{8,}$/];
const HARNESS = [/Google Chrome/, /(^|\/)node(\s|$)/, /WindowServer/, /^ps\s/, /^pmset\s/];

/** What's running, the load and the thermal state now, and whether that's quiet. */
export function snapshot(phase = 'pre') {
  const reasons = [];
  // (the command lines: an app's path holds spaces; a build's executable is the first word)
  const ps = execFileSync('ps', ['-Ao', 'pid=,pcpu=,args='], { encoding: 'utf8', maxBuffer: 1 << 26 });
  const builds = [], busy = [], others = [];
  let other = 0;
  for (const line of ps.split('\n')) {
    const m = line.match(/^\s*(\d+)\s+([\d.]+)\s+(.*)$/);
    if (!m) continue;
    const [, pid, pcpu, args] = m;
    if (Number(pid) === process.pid) continue;
    const comm = args;
    const name = path.basename(args.split(' ')[0]);
    if (BUILD.some((r) => r.test(name)) || /scenic-build/.test(args)) builds.push({ pid: Number(pid), name, cpu: Number(pcpu), args: args.slice(0, 160) });
    else if (!HARNESS.some((r) => r.test(comm) || r.test(name))) {
      other += Number(pcpu);
      if (Number(pcpu) > 20) others.push({ pid: Number(pid), name, cpu: Number(pcpu), args: args.slice(0, 120) });
      if (Number(pcpu) > BUSY_CPU) busy.push({ pid: Number(pid), name, cpu: Number(pcpu) });
    }
  }
  if (builds.length) reasons.push(`build jobs running: ${builds.map((b) => `${b.name} (${b.pid}, ${b.cpu}%)`).join(', ')}`);
  if (busy.length) reasons.push(`busy processes: ${busy.map((b) => `${b.name} (${b.pid}, ${b.cpu}%)`).join(', ')}`);
  if (other >= OTHER_CPU) reasons.push(`other work ${other.toFixed(0)} % of a core ≥ ${OTHER_CPU}: ${others.map((b) => `${b.name} (${b.pid}, ${b.cpu}%)`).join(', ')}`);
  const load = os.loadavg();
  if (phase === 'pre' && load[0] >= MAX_LOAD) reasons.push(`1-minute load ${load[0].toFixed(2)} ≥ ${MAX_LOAD}`);
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
  return { ok: reasons.length === 0, phase, at: new Date().toISOString(), load, otherCpu: other, others, builds, busy, therm: thermLines, reasons };
}

/** Waits until the Mac is quiet (polling every pollMs), or throws after maxWaitMs with the reasons. */
export async function waitQuiet({ maxWaitMs = 20 * 60e3, pollMs = 15e3, log = console.log, phase = 'pre' } = {}) {
  const t0 = Date.now();
  let s = snapshot(phase), said = '';
  while (!s.ok) {
    const why = s.reasons.join('; ');
    if (why !== said) {
      log(`waiting for a quiet Mac: ${why}`);
      said = why;
    }
    if (Date.now() - t0 > maxWaitMs) throw new Error(`gave up after ${Math.round((Date.now() - t0) / 60e3)} min waiting for a quiet Mac: ${why}`);
    await new Promise((r) => setTimeout(r, pollMs));
    s = snapshot(phase);
  }
  return s;
}

if (process.argv[1] && import.meta.url.endsWith(path.basename(process.argv[1]))) console.log(JSON.stringify(snapshot(), null, 1));
