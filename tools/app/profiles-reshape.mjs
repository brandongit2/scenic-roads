// A map server's settings profiles rewritten in the web app's current full shape: every setting the
// app has, explicit, as the app read it. The app reads a profile only in the shape it writes, filling
// a setting the profile lacks from web/default-profile.json (README, Using it: Settings); a profile
// saved before a setting was renamed or reshaped would then follow the seed for it. Run at the
// rollout of such a change, with `--app` a checkout of the app before it, this keeps what each
// profile showed; run with this checkout's app, it writes what the seed fills today into each profile.
//
//   node --experimental-transform-types --no-warnings tools/app/profiles-reshape.mjs <command>
//     [--app <checkout>] <in.json> <out.json>   the list rewritten into <out.json>, a new file (a try,
//                                               on a copy or a GET /api/profiles answer)
//     [--app <checkout>] --apply <profiles.json>  the server's file rewritten in place, its backup
//                                               kept beside it as <profiles.json>.before-reshape
//     --restore <profiles.json>                 the backup put back (and so gone)
//     --drop-backup <profiles.json>             the backup deleted, once each profile is confirmed
//
// It prints what changed in each profile. It refuses (exit 1, nothing written) unless, for every
// profile, this checkout's app and `--app`'s read the result as `--app`'s app read the input, and
// this checkout's writes it back unchanged and reads it the same over another base (nothing in it
// comes from the seed). `--apply` refuses while a backup is there (one conversion at a time) and
// when the file changed while it ran (a profile saved meanwhile). Rollout: docs/storage.md,
// "Settings profiles".
import { registerHooks } from 'node:module';
import { copyFileSync, existsSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

// The app's modules import without extensions and import JSON as Vite does: resolved here.
registerHooks({
  resolve(spec, ctx, next) {
    if ((spec.startsWith('./') || spec.startsWith('../')) && !/\.[a-z]+$/.test(spec)) {
      for (const ext of ['.ts', '/index.ts']) {
        try {
          return next(spec + ext, ctx);
        } catch {
          /* the next */
        }
      }
    }
    const r = next(spec, ctx);
    return spec.endsWith('.json') ? { ...r, importAttributes: { type: 'json' } } : r;
  },
});

const die = (m) => {
  console.error(`profiles-reshape: ${m}`);
  process.exit(1);
};
const args = process.argv.slice(2);
const here = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const backupOf = (f) => `${f}.before-reshape`;
const usage = 'usage: profiles-reshape.mjs [--app <checkout>] <in.json> <out.json> | [--app <checkout>] --apply <profiles.json> | --restore <profiles.json> | --drop-backup <profiles.json>';
if (args[0] === '--restore' || args[0] === '--drop-backup') {
  if (args.length !== 2) die(usage);
  const file = resolve(args[1]), backup = backupOf(file);
  if (!existsSync(backup)) die(`no backup ${backup}`);
  if (args[0] === '--restore') {
    renameSync(backup, file);
    console.log(`restored: ${file} is the backup again (${backup} gone)`);
  } else {
    rmSync(backup);
    console.log(`deleted: ${backup}`);
  }
  process.exit(0);
}
let app = here;
if (args[0] === '--app') {
  app = resolve(args[1] ?? die('--app needs a checkout'));
  args.splice(0, 2);
}
const apply = args[0] === '--apply';
if (apply) args.shift();
if (args.length !== (apply ? 1 : 2)) die(usage);
const inPath = resolve(args[0]);
const outPath = apply ? inPath : resolve(args[1]);
if (apply && existsSync(backupOf(inPath))) die(`${backupOf(inPath)} exists: a conversion is waiting to be confirmed (--drop-backup) or undone (--restore)`);
if (!apply && existsSync(outPath)) die(`${outPath} exists: give a new file`);
if (!existsSync(`${app}/web/src/state.ts`)) die(`${app} is not a checkout of the app (no web/src/state.ts)`);

const before = await import(`${realpathSync(app)}/web/src/state.ts`);
const now = await import(`${here}/web/src/state.ts`);
const inBytes = readFileSync(inPath);
const list = JSON.parse(inBytes.toString('utf8'));
if (!list || !Array.isArray(list.profiles) || typeof list.default !== 'string') die(`${inPath} is not a profiles list`);

const diff = (a, b, p = '') => {
  if (isDeepStrictEqual(a, b)) return [];
  if (a && b && typeof a === 'object' && typeof b === 'object' && !Array.isArray(a) && !Array.isArray(b))
    return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap((k) => diff(a[k], b[k], `${p}.${k}`));
  const s = (v) => (v === undefined ? '(none)' : JSON.stringify(v).slice(0, 70));
  return [`${p}: ${s(a)} -> ${s(b)}`];
};
// A base unlike the seed in every setting it can be (each on/off turned over, each number moved),
// for the check that nothing of a profile comes from its base.
const flip = (v) => (typeof v === 'boolean' ? !v : typeof v === 'number' ? v * 0.5 + 0.25 : Array.isArray(v) ? v.map(flip) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, flip(x)])) : v);
const other = now.fromSaved(flip(now.profileSettings(now.seed)), now.seed);

let failed = false;
const profiles = list.profiles.map((p) => {
  const was = before.fromProfile(p.settings);
  const settings = before.profileSettings(was);
  const read = now.fromProfile(settings);
  const problems = [
    ...diff(was, read).map((d) => `read differently ${d}`),
    // (So the list can be replaced while `--app`'s app still serves the map.)
    ...diff(was, before.fromProfile(settings)).map((d) => `read differently by --app's app ${d}`),
    ...diff(settings, now.profileSettings(read)).map((d) => `not written back as is ${d}`),
    ...diff(read, { ...now.fromSaved(settings, other), view: null, selected: null, stretch: null }).map((d) => `taken from the base ${d}`),
  ];
  const changes = diff(p.settings, settings);
  console.log(`${p.id} "${p.name}": ${changes.length ? `${changes.length} changed` : 'already in the current shape'}`);
  for (const c of changes) console.log(`  ${c}`);
  if (problems.length) {
    failed = true;
    for (const x of problems.slice(0, 20)) console.log(`  PROBLEM ${x}`);
  }
  return { id: p.id, name: p.name, settings };
});
if (failed) die('nothing written');
const text = JSON.stringify({ v: 1, default: list.default, profiles }, null, 2);
if (!apply) {
  writeFileSync(outPath, text);
  console.log(`written: ${outPath} (${profiles.length} profiles, default ${list.default})`);
} else {
  // The new list beside the file, then renamed over it (the server reads the file at every request,
  // whole); the backup first, the original's bytes.
  const tmp = `${inPath}.reshape-new`;
  writeFileSync(tmp, text);
  copyFileSync(inPath, backupOf(inPath));
  if (!readFileSync(inPath).equals(inBytes)) {
    rmSync(tmp);
    rmSync(backupOf(inPath));
    die(`${inPath} changed while this ran (a profile saved?): nothing changed, run it again`);
  }
  renameSync(tmp, inPath);
  console.log(`converted: ${inPath} (${profiles.length} profiles, default ${list.default}); backup: ${backupOf(inPath)}`);
  console.log(`once each profile is confirmed: profiles-reshape.mjs --drop-backup ${inPath} (or --restore to undo)`);
}
