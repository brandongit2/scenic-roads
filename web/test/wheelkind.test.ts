// node web/test/wheelkind.test.ts (tools/dev/check.sh's web part): src/wheelkind.ts's reading of
// a stream of wheel events. Recordings of real devices go in as fixtures: arrays of WheelSample.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { BURST_GAP_MS, WheelClassifier, classifyWheel, type WheelKind, type WheelSample } from '../src/wheelkind.ts';

const ev = (t: number, deltaY: number, o: Partial<WheelSample> = {}): WheelSample => ({ deltaX: 0, deltaY, deltaMode: 0, ctrlKey: false, timeStamp: t, ...o });
const fixtures: [string, WheelSample[], WheelKind][] = [
  ['Chromium mouse notch', [ev(0, 100, { wheelDeltaY: -120 }), ev(60, 100, { wheelDeltaY: -120 })], 'mouse'],
  ['Firefox mouse lines', [ev(0, 3, { deltaMode: 1 }), ev(50, 3, { deltaMode: 1 })], 'mouse'],
  ['Firefox mouse pixels', [ev(0, 4.000244140625), ev(40, 8.00048828125)], 'mouse'],
  ['trackpad swipe, fractional with sideways motion', [ev(0, 1.5, { wheelDeltaY: -4.5 }), ev(16, 12.25, { deltaX: 3, wheelDeltaY: -36.75 }), ev(32, 20.5, { deltaX: 2, wheelDeltaY: -61.5 })], 'trackpad'],
  ['trackpad vertical flick opening small', [ev(0, 4, { wheelDeltaY: -12 }), ev(8, 60, { wheelDeltaY: -180 })], 'trackpad'],
  ['owner mouse notch, then a spin', [ev(0, 13, { wheelDeltaY: -39 }), ev(40, 103, { wheelDeltaY: -309 }), ev(48, 205, { wheelDeltaY: -615 })], 'mouse'],
  ['pinch (ctrl) leaves the burst alone', [ev(0, 13), ev(8, -3.2, { ctrlKey: true })], 'mouse'],
  ['a later burst is judged afresh', [ev(0, 100, { wheelDeltaY: -120 }), ev(1000, 1.5, { deltaX: 1 })], 'trackpad'],
  ['and back', [ev(0, 1.5, { deltaX: 1 }), ev(1000, 3, { deltaMode: 1 })], 'mouse'],
  ['empty', [], 'trackpad'],
  ['Windows notch 100 px', [ev(0, 100), ev(70, 100)], 'mouse'],
  ['Windows notch 120 px, two at once', [ev(0, 240), ev(90, 120)], 'mouse'],
  ['mouse with a small notch (8 px) reads as a trackpad', [ev(0, 8, { wheelDeltaY: -24 }), ev(60, 8, { wheelDeltaY: -24 })], 'trackpad'],
  ['smooth-scrolling mouse opening small', [ev(0, 2, { wheelDeltaY: -6 }), ev(8, 9, { wheelDeltaY: -27 }), ev(16, 30, { wheelDeltaY: -90 })], 'trackpad'],
  ['Magic Mouse, small steps on both axes', [ev(0, 3, { deltaX: 1, wheelDeltaY: -9 }), ev(8, 14, { wheelDeltaY: -42 })], 'trackpad'],
  ['trackpad opening big is taken back at its first small step', [ev(0, 40, { wheelDeltaY: -120 }), ev(8, 30, { wheelDeltaY: -90 }), ev(16, 5, { deltaX: 1, wheelDeltaY: -15 })], 'trackpad'],

];
for (const [name, events, want] of fixtures) assert.equal(classifyWheel(events), want, name);

// The owner's own mouse and trackpad (25 Oct), each gesture cut to its first 40 events:
// [device (m mouse, t trackpad scroll, tp trackpad pinch), ms, deltaX, deltaY, deltaMode, wheelDeltaY, ctrlKey].
// One person's devices: a different mouse (another notch size, smooth scrolling) is not covered.
for (const browser of ['chrome', 'safari']) {
  const rows: [string, number, number, number, number, number, number][] = JSON.parse(readFileSync(new URL(`./wheel-${browser}.json`, import.meta.url), 'utf8'));
  const c = new WheelClassifier();
  let right = 0, n = 0, last = -Infinity;
  const gestures: { want: WheelKind; first: WheelKind; end: WheelKind }[] = [];
  for (const [dev, t, deltaX, deltaY, deltaMode, wheelDeltaY, ctrl] of rows) {
    const opens = t - last > BURST_GAP_MS;
    last = t;
    const kind = c.push({ deltaX, deltaY, deltaMode, wheelDeltaY, ctrlKey: !!ctrl, timeStamp: t }).kind;
    if (dev === 'tp' || ctrl) continue; // a pinch says nothing of the device
    const want: WheelKind = dev === 'm' ? 'mouse' : 'trackpad';
    n++;
    if (kind === want) right++;
    if (opens || !gestures.length) gestures.push({ want, first: kind, end: kind });
    gestures[gestures.length - 1].end = kind;
  }
  assert.equal(right, n, `${browser}: ${n - right} of ${n} events misread`);
  assert.ok(gestures.length > 20 && gestures.every((g) => g.first === g.want && g.end === g.want), `${browser}: a gesture misread`);
}
console.log('wheelkind ok');
