// node --test tools/check/profiles-reshape.test.mjs (check.sh's web part): tools/app/profiles-reshape.mjs
// rewrites a profiles list in the app's full shape, keeping what each profile shows, and refuses
// when the app would read the result otherwise.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const tool = join(repo, 'tools/app/profiles-reshape.mjs');
const run = (...a) => spawnSync(process.execPath, ['--experimental-transform-types', '--no-warnings', tool, ...a], { encoding: 'utf8' });
const seed = JSON.parse(readFileSync(join(repo, 'web/default-profile.json'), 'utf8'));
/** Every leaf's path in `v`, objects' keys (an empty one a leaf itself). */
const paths = (v, p = '') => (v && typeof v === 'object' && !Array.isArray(v) && Object.keys(v).length ? Object.entries(v).flatMap(([k, x]) => paths(x, `${p}.${k}`)) : [p]);

test('a list in the full shape: every setting explicit, what each profile set kept; again: unchanged', (t) => {
  const d = mkdtempSync(join(tmpdir(), 'reshape-'));
  t.after(() => rmSync(d, { recursive: true, force: true }));
  const list = { default: 'abcd', writable: true, profiles: [{ id: 'base', name: 'Default', settings: {} }, { id: 'abcd', name: 'Night', settings: { mode: 'curvy', water: { rivers: false }, unknown: 1 } }] };
  writeFileSync(join(d, 'in.json'), JSON.stringify(list));
  const r = run(join(d, 'in.json'), join(d, 'out.json'));
  assert.equal(r.status, 0, r.stderr + r.stdout);
  const out = JSON.parse(readFileSync(join(d, 'out.json'), 'utf8'));
  assert.deepEqual([out.v, out.default, out.profiles.map((p) => [p.id, p.name])], [1, 'abcd', [['base', 'Default'], ['abcd', 'Night']]]);
  for (const p of out.profiles) for (const k of paths(seed)) assert.ok(paths(p.settings).some((q) => q === k || q.startsWith(`${k}.`)), `${p.id}: ${k}`);
  const night = out.profiles[1].settings;
  assert.deepEqual([night.mode, night.water.rivers, night.water.foam, 'unknown' in night], ['curvy', false, seed.water.foam, false]);
  assert.match(r.stdout, /abcd "Night": \d+ changed/);
  const again = run(join(d, 'out.json'), join(d, 'again.json'));
  assert.equal(again.status, 0, again.stderr);
  assert.deepEqual(JSON.parse(readFileSync(join(d, 'again.json'), 'utf8')), out);
  assert.equal((again.stdout.match(/already in the current shape/g) ?? []).length, 2);
});

test('refused: an existing output, an input that is no list, an app reading the result otherwise', (t) => {
  const d = mkdtempSync(join(tmpdir(), 'reshape-'));
  t.after(() => rmSync(d, { recursive: true, force: true }));
  writeFileSync(join(d, 'in.json'), JSON.stringify({ v: 1, default: 'base', profiles: [{ id: 'base', name: 'Default', settings: {} }] }));
  writeFileSync(join(d, 'taken.json'), 'x');
  assert.equal(run(join(d, 'in.json'), join(d, 'taken.json')).status, 1);
  assert.equal(readFileSync(join(d, 'taken.json'), 'utf8'), 'x');
  writeFileSync(join(d, 'bad.json'), '{"profiles": 3}');
  assert.equal(run(join(d, 'bad.json'), join(d, 'o1.json')).status, 1);
  // An app that read a setting this one doesn't offer (as an older app read an older name).
  mkdirSync(join(d, 'app/web/src'), { recursive: true });
  writeFileSync(join(d, 'app/web/src/state.ts'), `import * as s from ${JSON.stringify(join(repo, 'web/src/state.ts'))};
export const profileSettings = s.profileSettings;
export const fromProfile = (o: unknown) => ({ ...s.fromProfile(o), mode: 'gone' });\n`);
  const r = run('--app', join(d, 'app'), join(d, 'in.json'), join(d, 'o2.json'));
  assert.equal(r.status, 1, r.stdout);
  assert.match(r.stdout, /PROBLEM read differently \.mode: "gone" -> "map"/);
  assert.ok(!existsSync(join(d, 'o2.json')));
});

test('in place: the backup beside the file, put back by --restore, deleted by --drop-backup', (t) => {
  const d = mkdtempSync(join(tmpdir(), 'reshape-'));
  t.after(() => rmSync(d, { recursive: true, force: true }));
  const file = join(d, 'profiles.json'), backup = `${file}.before-reshape`;
  const orig = JSON.stringify({ v: 1, default: 'base', profiles: [{ id: 'base', name: 'Default', settings: { mode: 'curvy' } }] });
  writeFileSync(file, orig);
  const a = run('--apply', file);
  assert.equal(a.status, 0, a.stderr + a.stdout);
  assert.equal(readFileSync(backup, 'utf8'), orig);
  assert.equal(JSON.parse(readFileSync(file, 'utf8')).profiles[0].settings.mode, 'curvy');
  assert.ok(Object.keys(JSON.parse(readFileSync(file, 'utf8')).profiles[0].settings).length > 40);
  assert.ok(!existsSync(`${file}.reshape-new`));
  // One conversion at a time: refused while its backup waits.
  const converted = readFileSync(file, 'utf8');
  assert.equal(run('--apply', file).status, 1);
  assert.equal(readFileSync(file, 'utf8'), converted);
  // Undone: the original back, no backup left.
  assert.equal(run('--restore', file).status, 0);
  assert.equal(readFileSync(file, 'utf8'), orig);
  assert.ok(!existsSync(backup));
  // Again, then confirmed: the backup deleted, the converted list kept.
  assert.equal(run('--apply', file).status, 0);
  assert.equal(run('--drop-backup', file).status, 0);
  assert.ok(!existsSync(backup));
  assert.equal(readFileSync(file, 'utf8'), converted);
  assert.equal(run('--drop-backup', file).status, 1);
});
