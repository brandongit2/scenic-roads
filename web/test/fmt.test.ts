// node web/test/fmt.test.ts (tools/dev/check.sh's web part): src/fmt.ts, the app's number formats.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fmt } from '../src/fmt.ts';

test('signed: a true minus, no sign on zero', () => {
  assert.equal(fmt.signed(12), '+12');
  assert.equal(fmt.signed(-40), '−40');
  assert.equal(fmt.signed(0), '0');
  assert.equal(fmt.signed(-0.3), '0');
  assert.equal(fmt.signed(1250), '+1,250');
});

test('area: two decimals below 1, one below 10, then whole', () => {
  assert.equal(fmt.area(0.4219), '0.42');
  assert.equal(fmt.area(3.14), '3.1');
  assert.equal(fmt.area(1250.4), '1,250');
});

test('perDay: a day from one a day, else a week', () => {
  assert.equal(fmt.perDay(24), '24 a day');
  assert.equal(fmt.perDay(2.5, 'trains'), '2.5 trains a day');
  assert.equal(fmt.perDay(2), '2 a day');
  assert.equal(fmt.perDay(1500), '1,500 a day');
  assert.equal(fmt.perDay(0.7, 'trains'), '5 trains a week');
  assert.equal(fmt.perDay(0.15), '1.1 a week');
  assert.equal(fmt.perDay(0.3), '2 a week');
  assert.equal(fmt.perDayShort(24), '24/day');
  assert.equal(fmt.perDayShort(0.3), '2/wk');
  assert.equal(fmt.perDayNum(24), '24');
  assert.equal(fmt.perDayNum(0.3), '2/wk');
});
