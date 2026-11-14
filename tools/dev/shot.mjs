#!/usr/bin/env node
// A proof screenshot at full resolution, saved to a file (#1301): headless Chrome (chrome.mjs:
// its own profile, the GPU, the debugging pipe) opens the page, waits for it to be idle, does
// what it's told (keys, clicks; idle again after each) and saves Page.captureScreenshot's PNG,
// the composited page (WebGL canvas and DOM together) at the size × scale asked.
//
//   node tools/dev/shot.mjs <url> <out.png> [--size 1440x900] [--scale 2] [--hide-ui]
//        [--press KEYS] [--click X,Y] [--scheme dark|light] [--quiet MS] [--timeout S]
//
//   --hide-ui     presses I (the map's HUD off) before the capture
//   --press KEYS  keys pressed in turn, comma-separated: a character (b, +) or a key name
//                 (Escape, ArrowUp, Enter)
//   --click X,Y   a left click at CSS pixels X,Y (repeatable; done after the keys)
//   --quiet MS    idle means MS ms (default 1000) with no animation frame requested and no request
//                 in flight, fonts loaded, and no task under way where the page says (the map's
//                 <html data-busy>, its status line's tasks: tiles, labels, searches)
//   --timeout S   the most each wait may take (default 60 s); past it: exit 2, saying what kept
//                 the page busy (frames requested a second, requests in flight)
//
// Prints one line: the file, its pixel size, the times (idle, capture, all). Exit 1 on an error,
// 2 on a timeout, 0 with the file written and its size checked.

import { writeFileSync } from "node:fs";
import { launch } from "./chrome.mjs";

const argv = process.argv.slice(2);
const pos = [], opt = { size: "1440x900", scale: "2", quiet: "1000", timeout: "60", press: "", click: [] };
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === "--hide-ui") opt.hideUi = true;
  else if (a === "--click") opt.click.push(argv[++i]);
  else if (a.startsWith("--") && a.slice(2) in opt) opt[a.slice(2)] = argv[++i];
  else if (a === "--scheme") opt.scheme = argv[++i];
  else if (a.startsWith("--")) usage(`unknown option ${a}`);
  else pos.push(a);
}
if (pos.length !== 2) usage("a URL and an output file are needed");
const [url, out] = pos;
const [W, H] = opt.size.split("x").map(Number), SCALE = Number(opt.scale), QUIET = Number(opt.quiet), TIMEOUT = Number(opt.timeout) * 1000;
if (!(W > 0 && H > 0 && SCALE > 0 && QUIET >= 0 && TIMEOUT > 0)) usage("bad --size, --scale, --quiet or --timeout");
const clicks = opt.click.map((c) => c.split(",").map(Number));
if (clicks.some((c) => c.length !== 2 || !c.every(Number.isFinite))) usage("--click takes X,Y");
const keys = [...(opt.hideUi ? ["i"] : []), ...opt.press.split(",").filter(Boolean)];

function usage(why) {
  console.error(`shot: ${why}\nusage: node tools/dev/shot.mjs <url> <out.png> [--size WxH] [--scale N] [--hide-ui] [--press KEYS] [--click X,Y] [--scheme dark|light] [--quiet MS] [--timeout S]`);
  process.exit(1);
}

// Counts the animation frames the page asks for (before its own scripts run).
const COUNT_FRAMES = `(() => { const raf = window.requestAnimationFrame.bind(window); let n = 0;
  window.requestAnimationFrame = (cb) => { n++; return raf(cb); }; Object.defineProperty(window, "__shotFrames", { value: () => n }); })();`;

const t0 = performance.now();
const secs = (ms) => `${(ms / 1000).toFixed(1)} s`;
const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));
class Timeout extends Error {}
let chrome;
try {
  chrome = await launch();
  const gl = (await chrome.send("SystemInfo.getInfo")).gpu.auxAttributes?.glRenderer || "?";
  const page = await chrome.page();
  const errors = [];
  page.on("Runtime.exceptionThrown", (p) => errors.push(p.exceptionDetails.exception?.description?.split("\n")[0] || p.exceptionDetails.text));
  // Requests in flight (an EventSource, or one open past 10 s, counts as a stream, not loading).
  const flight = new Map();
  let netAt = performance.now();
  page.on("Network.requestWillBeSent", (p) => { if (p.type !== "EventSource") flight.set(p.requestId, { url: p.request.url, at: performance.now() }); netAt = performance.now(); });
  for (const ev of ["Network.loadingFinished", "Network.loadingFailed"]) page.on(ev, (p) => { if (flight.delete(p.requestId)) netAt = performance.now(); });
  const loading = () => [...flight.values()].filter((r) => performance.now() - r.at < 10000);
  await Promise.all(["Page.enable", "Runtime.enable", "Network.enable"].map((m) => page.send(m)));
  await page.send("Page.addScriptToEvaluateOnNewDocument", { source: COUNT_FRAMES });
  await page.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: SCALE, mobile: false });
  if (opt.scheme) await page.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: opt.scheme }] });
  const js = async (expression) => { const r = await page.send("Runtime.evaluate", { expression, returnByValue: true }); if (r.exceptionDetails) throw new Error(r.exceptionDetails.text); return r.result.value; };

  const loaded = new Promise((ok) => page.on("Page.loadEventFired", ok));
  const nav = await page.send("Page.navigate", { url });
  if (nav.errorText) throw new Error(`${url}: ${nav.errorText}`);
  await Promise.race([loaded, sleep(TIMEOUT).then(() => { throw new Timeout(`no load event in ${secs(TIMEOUT)}`); })]);

  // Idle: QUIET ms with no frame requested and nothing loading, the fonts in.
  const idle = async (what) => {
    const start = performance.now();
    let frames = await js("window.__shotFrames()"), frameAt = performance.now();
    const seen = [];
    for (;;) {
      await sleep(100);
      const [n, fonts, busy] = await js("[window.__shotFrames(), document.fonts.status, document.documentElement.dataset.busy]");
      const now = performance.now();
      seen.push([now, n]);
      if (n !== frames) { frames = n; frameAt = now; }
      if (loading().length) netAt = now;
      if (now - frameAt >= QUIET && now - netAt >= QUIET && fonts === "loaded" && !busy) return now - start;
      if (now - start > TIMEOUT) {
        const last = seen.filter(([t]) => now - t <= 1000), rate = last.length > 1 ? last.at(-1)[1] - last[0][1] : 0;
        const l = loading();
        throw new Timeout(`not idle ${what} after ${secs(TIMEOUT)}: ${rate} frames requested in the last second, ${l.length} requests in flight${l.length ? ` (${l.slice(0, 3).map((r) => r.url.slice(0, 100)).join(", ")})` : ""}, fonts ${fonts}${busy ? `, the page busy with: ${busy}` : ""}`);
      }
    }
  };
  let idleMs = await idle("after loading");
  for (const k of keys) {
    const named = k.length > 1;
    const ev = { key: k, code: named ? k : /^[a-z]$/i.test(k) ? `Key${k.toUpperCase()}` : "", ...(named ? {} : { text: k }), windowsVirtualKeyCode: named ? 0 : k.toUpperCase().charCodeAt(0) };
    await page.send("Input.dispatchKeyEvent", { type: named ? "rawKeyDown" : "keyDown", ...ev });
    await page.send("Input.dispatchKeyEvent", { type: "keyUp", ...ev });
    idleMs += await idle(`after the key ${k}`);
  }
  for (const [x, y] of clicks) {
    await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await page.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
    await page.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
    idleMs += await idle(`after the click at ${x},${y}`);
  }

  const c0 = performance.now();
  const { data } = await Promise.race([page.send("Page.captureScreenshot", { format: "png" }), sleep(TIMEOUT).then(() => { throw new Timeout(`the capture didn't answer in ${secs(TIMEOUT)}`); })]);
  const capMs = performance.now() - c0;
  const png = Buffer.from(data, "base64");
  const [pw, ph] = [png.readUInt32BE(16), png.readUInt32BE(20)], [ew, eh] = [Math.round(W * SCALE), Math.round(H * SCALE)];
  if (pw !== ew || ph !== eh) throw new Error(`the capture is ${pw}×${ph}, not ${ew}×${eh}`);
  writeFileSync(out, png);
  await chrome.close();
  console.log(`shot: ${out} ${pw}×${ph} (${(png.length / 1e6).toFixed(1)} MB); idle in ${secs(idleMs)}, captured in ${secs(capMs)}, ${secs(performance.now() - t0)} in all; GL ${gl}${errors.length ? `; ${errors.length} page errors, the first: ${errors[0]}` : ""}`);
  process.exit(0);
} catch (e) {
  await chrome?.close().catch(() => {});
  console.error(`shot: FAILED: ${e.message}`);
  process.exit(e instanceof Timeout ? 2 : 1);
}
