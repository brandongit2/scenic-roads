// node web/test/wheelkind.test.ts (tools/dev/check.sh's web part): src/wheelkind.ts's reading of
// a stream of wheel events. Recordings of real devices go in as fixtures: arrays of WheelSample.
import assert from 'node:assert/strict';
import { classifyWheel, type WheelKind, type WheelSample } from '../src/wheelkind.ts';

const ev = (t: number, deltaY: number, o: Partial<WheelSample> = {}): WheelSample => ({ deltaX: 0, deltaY, deltaMode: 0, ctrlKey: false, timeStamp: t, ...o });
const fixtures: [string, WheelSample[], WheelKind][] = [
  ['Chromium mouse notch', [ev(0, 100, { wheelDeltaY: -120 }), ev(60, 100, { wheelDeltaY: -120 })], 'mouse'],
  ['Firefox mouse lines', [ev(0, 3, { deltaMode: 1 }), ev(50, 3, { deltaMode: 1 })], 'mouse'],
  ['Firefox mouse pixels', [ev(0, 4.000244140625), ev(40, 8.00048828125)], 'mouse'],
  ['trackpad swipe, fractional with sideways motion', [ev(0, 1.5, { wheelDeltaY: -4.5 }), ev(16, 12.25, { deltaX: 3, wheelDeltaY: -36.75 }), ev(32, 20.5, { deltaX: 2, wheelDeltaY: -61.5 })], 'trackpad'],
  ['trackpad vertical flick, wheelDelta is -3 x deltaY', [ev(0, 60, { wheelDeltaY: -180 }), ev(16, 40, { wheelDeltaY: -120 })], 'trackpad'],
  ['mouse burst turns trackpad on sideways steps', [ev(0, 100, { wheelDeltaY: -120 }), ev(16, 2, { deltaX: 1 })], 'trackpad'],
  ['a later burst is judged afresh', [ev(0, 100, { wheelDeltaY: -120 }), ev(1000, 1.5, { deltaX: 1 })], 'trackpad'],
  ['and back', [ev(0, 1.5, { deltaX: 1 }), ev(1000, 3, { deltaMode: 1 })], 'mouse'],
  ['empty', [], 'trackpad'],
];
for (const [name, events, want] of fixtures) assert.equal(classifyWheel(events), want, name);
console.log('wheelkind ok');
