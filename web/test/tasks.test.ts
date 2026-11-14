// node web/test/tasks.test.ts (tools/dev/check.sh's web part): src/tasks.ts's <html data-busy>,
// the tasks under way that tools/dev/shot.mjs waits out before a capture.
import assert from 'node:assert/strict';
import { test } from 'node:test';

const dataset: Record<string, string> = {};
Object.assign(globalThis, { window: globalThis, document: { documentElement: { dataset } } });
const { tasks } = await import('../src/tasks.ts');
const settle = () => new Promise((ok) => setTimeout(ok, 250));

test('data-busy: every task under way, by label, at once; deleted when none', async () => {
  tasks.begin('a', 'tiles');
  await settle();
  assert.equal(dataset.busy, 'tiles');
  let roadsIn = false;
  tasks.poll(() => (roadsIn ? [] : [{ label: 'roads' }]));
  await settle();
  assert.equal(dataset.busy, 'roads · tiles');
  tasks.end('a');
  await settle();
  assert.equal(dataset.busy, 'roads');
  roadsIn = true;
  tasks.kick();
  await settle();
  assert.equal('busy' in dataset, false);
});
