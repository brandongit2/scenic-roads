// Two-finger touch gestures over 3D terrain, measured (task #116): where the terrain point between
// the fingers goes, and how far a pinch zooms. Driven through the Chrome DevTools Protocol.
//
// Start a Chrome with remote debugging and a scratch profile, and the app (a Vite dev server, or
// the map server) with ?checks:
//   "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --remote-debugging-port=18229 \
//     --user-data-dir=<scratch>/chrome --use-angle=metal --enable-gpu --ignore-gpu-blocklist about:blank
//   node tools/check/touch-gestures.mjs --port 18229 --url 'http://127.0.0.1:18221/?checks' \
//     [--places mb,fuji,rockies,sea] [--zooms 12,15,17,18] [--pitches 0,45,70] [--gestures pinch,rotate,tilt,pan] [--out f.json]
//
// For each case it frames the place at the zoom (from the ground: cam3d.frame), hides the panels
// (I), picks the terrain point T under the fingers' midpoint M (cam3d.anchorAt), and runs the
// gesture with Input.dispatchTouchEvent (real touches: the app's own handling runs), the fingers
// held still before they lift. Then:
//   · T's distance from where it should be on screen (px): M; after a pan, M moved with the fingers;
//   · the ground now under that point, in metres from T;
//   · the zoom done (log2 of the camera→T distance's ratio; a pinch doubling the fingers' spread
//     should be 1) and the zoom number's change, the bearing and pitch changes;
//   · the pivot actually used: the fixed point of the camera's rotation (a turn: a vertical axis,
//     its distance from T; a tilt: a horizontal axis, how far ahead of T and how high).
// The gestures: pinch (spread 120 → 240 px), rotate (turned 45°, 180 px apart), tilt (both
// fingers 50 px down from 45° and up from 0°, side by side), pan (180 px apart, moved 120, 60 px).
const args = Object.fromEntries(process.argv.slice(2).reduce((acc, a, i, all) => {
  if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
  return acc;
}, []));
const PORT = Number(args.port ?? 18229);
const URL = String(args.url ?? 'http://127.0.0.1:18221/?checks');
const W = 1180, H = 820;
const PLACES = {
  mb: { name: 'Mont Blanc', lng: 6.8652, lat: 45.8326 },
  fuji: { name: 'Mt Fuji', lng: 138.7274, lat: 35.3606 },
  rockies: { name: 'Rockies (Longs Peak)', lng: -105.6156, lat: 40.2549 },
  sea: { name: 'Sea level (Amsterdam)', lng: 4.8897, lat: 52.3731 },
};
const placeIds = String(args.places ?? 'mb,fuji,rockies,sea').split(',');
const zooms = String(args.zooms ?? '12,15,17,18').split(',').map(Number);
const pitches = String(args.pitches ?? '0,45,70').split(',').map(Number);
const gestures = String(args.gestures ?? 'pinch,rotate,tilt,pan').split(',');
// The fingers' midpoint, in the map's own coordinates (its container sits right of the panel):
// off the view centre (down and right), as a hand usually is. Set once the page is loaded.
let MX = 0, MY = 0, OX = 0, OY = 0;

const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: 'PUT' })).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let seq = 0;
const pending = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
const send = (method, params = {}) => new Promise((res, rej) => {
  const id = ++seq;
  pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
  ws.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expr, timeout = 120) => {
  const r = await send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true, timeout: timeout * 1000 });
  if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const HELPERS = `
  const map = window.__app.map, cam3d = window.__app.cam3d, tr = map._camera.transform;
  const R = 6371008.8, D = Math.PI / 180;
  const ecef = (lng, lat, alt) => { const la = lat * D, lo = lng * D, r = R + alt; return [r * Math.cos(la) * Math.sin(lo), r * Math.sin(la), r * Math.cos(la) * Math.cos(lo)]; };
  const enuAt = (lng, lat) => { const la = lat * D, lo = lng * D; const sa = Math.sin(la), ca = Math.cos(la), so = Math.sin(lo), co = Math.cos(lo);
    return { E: [co, 0, -so], N: [-sa * so, ca, -sa * co], U: [ca * so, sa, ca * co] }; };
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  // Local east/north/up metres of a point relative to the reference (lng, lat, alt).
  const local = (ref, lng, lat, alt) => { const O = ecef(ref.lng, ref.lat, ref.elev), P = ecef(lng, lat, alt), f = enuAt(ref.lng, ref.lat);
    const v = [P[0] - O[0], P[1] - O[1], P[2] - O[2]]; return [dot(v, f.E), dot(v, f.N), dot(v, f.U)]; };
  const camera = () => { const ll = tr.getCameraLngLat(); return { lng: ll.lng, lat: ll.lat, alt: tr.getCameraAltitude(), bearing: map.getBearing(), pitch: map.getPitch(), zoom: map.getZoom() }; };
  const idle = () => new Promise((r) => { map.once('idle', r); map.triggerRepaint(); setTimeout(r, 15000); });
`;

const results = [];
try {
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await send('Emulation.setFocusEmulationEnabled', { enabled: true });
  await send('Page.navigate', { url: URL });
  for (let i = 0; i < 240; i++) {
    if (await evaluate('return !!(window.__app && window.__app.map && window.__app.map.loaded())').catch(() => false)) break;
    await sleep(250);
  }
  const setup = await evaluate(`${HELPERS} const s = window.__app.store.s; return { terrain: s.terrain.on, exag: s.terrain.exaggeration, globe: s.globe, version: window.maplibregl?.version };`);
  console.error('setup', JSON.stringify(setup));
  // The map alone ("I": the panels and bars hidden), so nothing covers the fingers.
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'i', code: 'KeyI', text: 'i', windowsVirtualKeyCode: 73 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
  await sleep(800);
  const box = await evaluate(`const m = window.__app.map, r = m.getCanvasContainer().getBoundingClientRect(), c = m.getCanvas(); return { x: r.left, y: r.top, w: c.clientWidth, h: c.clientHeight };`);
  OX = box.x; OY = box.y;
  MX = Number(args.mx ?? box.w / 2 + 140); MY = Number(args.my ?? box.h / 2 + 90);
  console.error('map box', JSON.stringify(box), 'M', MX, MY);
  const hit = await evaluate(`const c = window.__app.map.getCanvas(); const e = document.elementFromPoint(${MX + OX}, ${MY + OY}); return e === c || c.parentElement.contains(e) ? 'canvas' : e?.tagName + '.' + e?.className;`);
  if (hit !== 'canvas') throw new Error(`the fingers would land on ${hit}, not the map`);

  const touch = (type, pts) => send('Input.dispatchTouchEvent', { type, touchPoints: pts.map(([x, y], i) => ({ x: x + OX, y: y + OY, id: i + 1, radiusX: 4, radiusY: 4, force: 1 })) });
  // A two-finger gesture: finger positions as a function of s in [0, 1]; first finger down, then the second.
  const gesture = async (pos, steps = 24) => {
    const p0 = pos(0);
    await touch('touchStart', [p0[0]]);
    await sleep(30);
    await touch('touchStart', p0);
    await sleep(40);
    for (let i = 1; i <= steps; i++) {
      await touch('touchMove', pos(i / steps));
      await sleep(16);
    }
    // Held still before lifting (no fling).
    for (let i = 0; i < 12; i++) { await touch('touchMove', pos(1)); await sleep(20); }
    const held = await evaluate(`${HELPERS} return camera();`);
    await touch('touchEnd', [pos(1)[0]]);
    await sleep(20);
    await touch('touchEnd', []);
    return held;
  };

  for (const pid of placeIds) {
    const P = PLACES[pid];
    for (const z of zooms) {
      for (const pitch of pitches) {
        for (const g of gestures) {
                    // Frame the place: its ground at the view centre, from the distance the zoom gives over flat ground.
          const pre = await evaluate(`${HELPERS}
            const ll = new (map.getCenter().constructor)(${P.lng}, ${P.lat});
            map.jumpTo({ center: ll, zoom: Math.min(${z}, 14), pitch: 0, bearing: 0 });
            await idle();
            const e0 = map.queryTerrainElevation(ll) ?? 0;
            map.jumpTo(cam3d.frame(map, ll, e0, ${z}, ${pitch}, 20));
            await idle(); await idle();
            cam3d.relevel(map);
            await idle();
            const a = cam3d.anchorAt(map, ${MX}, ${MY});
            if (!a || !a.ground) return { error: 'no ground under the fingers', a };
            return { T: { lng: a.ll.lng, lat: a.ll.lat, elev: a.elev }, cam: camera(), centreElev: map.getCenterElevation(), groundAtCentre: map.queryTerrainElevation(map.getCenter()), globe: !!tr.isGlobeRendering };
          `);
          if (pre.error) { console.error(pid, z, pitch, g, pre.error); results.push({ place: pid, zoom: z, pitch, gesture: g, error: pre.error }); continue; }
          let pos, expect = {};
          if (g === 'pinch') {
            const d0 = 120, d1 = 240; // spread ×2: one level in
            pos = (s) => { const d = d0 + (d1 - d0) * s; return [[MX - d / 2, MY], [MX + d / 2, MY]]; };
            expect = { dz: Math.log2(d1 / d0) };
          } else if (g === 'rotate') {
            const r = 90, th = 45 * Math.PI / 180;
            pos = (s) => { const a = th * s; return [[MX - r * Math.cos(a), MY - r * Math.sin(a)], [MX + r * Math.cos(a), MY + r * Math.sin(a)]]; };
            expect = { dBearing: -45 };
          } else if (g === 'pan') {
            pos = (s) => [[MX - 90 + 120 * s, MY + 60 * s], [MX + 90 + 120 * s, MY + 60 * s]];
            expect = { at: [MX + 120, MY + 60] };
          } else {
            const dy = pitch >= 45 ? 50 : -50; // down: flatter; up: steeper
            pos = (s) => [[MX - 90, MY + dy * s], [MX + 90, MY + dy * s]];
            expect = { dPitch: -dy * 0.5 };
          }
          const held = await gesture(pos);
          const post = await evaluate(`${HELPERS}
            const EX = ${expect.at ? expect.at[0] : MX}, EY = ${expect.at ? expect.at[1] : MY};
            const T = ${JSON.stringify(pre.T)}, c0 = ${JSON.stringify(pre.cam)}, c1 = ${JSON.stringify(held)};
            const LL = map.getCenter().constructor;
            // Where T is on screen now; what ground is under the old midpoint now.
            const q = map.project(new LL(T.lng, T.lat));
            // (project() puts the point at its terrain height, which is T's.)
            const a = cam3d.anchorAt(map, EX, EY);
            const gNow = a ? local(T, a.ll.lng, a.ll.lat, a.elev) : null;
            const C0 = local(T, c0.lng, c0.lat, c0.alt), C1 = local(T, c1.lng, c1.lat, c1.alt);
            const dist0 = Math.hypot(...C0), dist1 = Math.hypot(...C1);
            // Pivot of the bearing change: fixed point of the horizontal rotation taking C0 to C1.
            let bearingPivot = null;
            const db = ((c1.bearing - c0.bearing + 540) % 360) - 180;
            if (Math.abs(db) > 1) {
              // Bearing +db turns the camera clockwise seen from above: positions rotate by -db (east→north is counter-clockwise).
              const phi = -db * D;
              // Complex: b = P + e^{i phi}(a - P) → P = (b - e^{i phi} a) / (1 - e^{i phi}).
              const ca = Math.cos(phi), sa = Math.sin(phi);
              const ax = C0[0], ay = C0[1], bx = C1[0], by = C1[1];
              const nx = bx - (ca * ax - sa * ay), ny = by - (sa * ax + ca * ay);
              const dx = 1 - ca, dy = -sa, den = dx * dx + dy * dy;
              const px = (nx * dx + ny * dy) / den, py = (ny * dx - nx * dy) / den;
              bearingPivot = { e: px, n: py, mFromT: Math.hypot(px, py) };
            }
            // Pivot of the pitch change: in the vertical plane along the view (forward h, up u).
            let pitchPivot = null;
            const dp = c1.pitch - c0.pitch;
            if (Math.abs(dp) > 1 && Math.abs(db) < 1) {
              const b = c0.bearing * D, fwd = [Math.sin(b), Math.cos(b)];
              const h0 = C0[0] * fwd[0] + C0[1] * fwd[1], h1 = C1[0] * fwd[0] + C1[1] * fwd[1];
              // Camera behind the pivot: tilting up (pitch +) swings it down and back toward the horizon. Solve with the measured angle's sign by trying both.
              let best = null;
              for (const sgn of [1, -1]) {
                const phi = sgn * dp * D, ca = Math.cos(phi), sa = Math.sin(phi);
                const nx = h1 - (ca * h0 - sa * C0[2]), ny = C1[2] - (sa * h0 + ca * C0[2]);
                const dx = 1 - ca, dy = -sa, den = dx * dx + dy * dy;
                const ph = (nx * dx + ny * dy) / den, pu = (ny * dx - nx * dy) / den;
                // Check: distance from the pivot preserved.
                const err = Math.abs(Math.hypot(h0 - ph, C0[2] - pu) - Math.hypot(h1 - ph, C1[2] - pu));
                if (!best || err < best.err) best = { ahead: ph, up: pu, err };
              }
              // Across the view (the axis direction) the pivot is undetermined; report how far along the view and the height.
              pitchPivot = { aheadOfT: best.ahead, heightRelT: best.up, heightAbs: T.elev + best.up, fitErr: best.err };
            }
            return {
              tPx: Math.hypot(q.x - EX, q.y - EY), tAt: [Math.round(q.x), Math.round(q.y)],
              groundUnderM_mFromT: gNow ? Math.hypot(...gNow) : null,
              dzTrue: Math.log2(dist0 / dist1), dzNumber: c1.zoom - c0.zoom, dBearing: db, dPitch: dp,
              bearingPivot, pitchPivot, camAlt0: c0.alt, camAlt1: c1.alt, dist0, dist1,
            };
          `);
          const row = { place: pid, zoom: z, pitch, gesture: g, globe: pre.globe, T: pre.T, camAlt: pre.cam.alt, zoomNumber: pre.cam.zoom, centreElev: pre.centreElev, expect, ...post };
          results.push(row);
          console.error(`${pid} z${z} p${pitch} ${g}: T ${post.tPx.toFixed(1)} px off, ground there ${post.groundUnderM_mFromT?.toFixed(0)} m from T; dz ${post.dzTrue.toFixed(3)} (number ${post.dzNumber.toFixed(3)}), dB ${post.dBearing.toFixed(1)}, dP ${post.dPitch.toFixed(1)}` +
            (post.bearingPivot ? `; bearing pivot ${post.bearingPivot.mFromT.toFixed(0)} m from T` : '') +
            (post.pitchPivot ? `; pitch pivot ${post.pitchPivot.aheadOfT.toFixed(0)} m ahead, ${post.pitchPivot.heightRelT.toFixed(0)} m above T (abs ${post.pitchPivot.heightAbs.toFixed(0)})` : ''));
        }
      }
    }
  }
} finally {
  if (args.out) (await import('node:fs')).writeFileSync(String(args.out), JSON.stringify(results, null, 1));
  ws.close();
  await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`).catch(() => {});
}
