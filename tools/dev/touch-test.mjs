#!/usr/bin/env node
// The build page at phone size with touch (#614): headless Chrome over CDP as a 390×844 phone
// (DPR 3, mobile, 5 touch points, an iPhone user agent), real touches (Input.dispatchTouchEvent),
// every tab walked: no sideways scroll, no console errors, screenshots; then the touch checks
// (cards pin and unpin, the world maps pan and pinch, the Regions split drags, the schedule's
// dropdown, settings fields focus without a jump). One PASS/FAIL line per check; exit 1 on a FAIL.
//
//   node tools/dev/touch-test.mjs [--out DIR] [--port N]   the Chrome run
//   node tools/dev/touch-test.mjs --serve [--port N]       only the server (for Mobile Safari)
//
// The page comes from a read-only server on its own port (8097): web/work as /work/, and only the
// reading calls forwarded to this Mac's agent (127.0.0.1:8090); everything else is refused, so the
// page can't lease work, pause, act or set a setting. The page opens as `?view` (never helps).
// (Other Macs' /work/settings and /work/live the page asks straight; it only reads them.)
import { spawn } from "node:child_process";
import { createServer, request } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, join, normalize, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const PORT = +arg("--port", 8097);
const OUT = arg("--out", join(tmpdir(), "scenic-touch"));
const WEB = join(dirname(fileURLToPath(import.meta.url)), "../../web/work");
const AGENT = { host: "127.0.0.1", port: 8090 };
const TABS = ["overview", "jobs", "machines", "regions", "nas", "dropboxes", "activity", "details"];

// ---- The read-only server ------------------------------------------------------------------------
// A call forwarded only when its body has no other keys than these (an empty body: {}).
const READS = { swarm: [], steps: [], regions: [], settings: [], history: ["since", "max"], live: ["range", "series", "net", "since"] };
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".png": "image/png", ".webmanifest": "application/manifest+json", ".svg": "image/svg+xml" };
function serve() {
  const srv = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const call = url.pathname.match(/^\/work\/([a-z]+)$/)?.[1];
    if (call && call in READS) {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        let v;
        try { v = JSON.parse(body); } catch { v = null; }
        if (req.method !== "POST" || !v || typeof v !== "object" || Array.isArray(v) || Object.keys(v).some((k) => !READS[call].includes(k))) {
          res.writeHead(403, { "Content-Type": "application/json" });
          return res.end(JSON.stringify({ error: "the touch-test server forwards reads only" }));
        }
        const out = JSON.stringify(v);
        const fw = request({ ...AGENT, method: "POST", path: url.pathname, headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(out) } }, (r) => { res.writeHead(r.statusCode, r.headers); r.pipe(res); });
        fw.on("error", (e) => { console.error(`touch-test server: ${url.pathname}: ${e.message}`); res.writeHead(502); res.end("{}"); });
        fw.end(out);
      });
      return;
    }
    if (req.method !== "GET" || !url.pathname.startsWith("/work/")) { res.writeHead(403); return res.end(); }
    // (No service worker: it would cache the page across runs.)
    if (url.pathname === "/work/sw.js") { res.writeHead(404); return res.end(); }
    const rel = normalize(url.pathname.slice(6) || "index.html").replace(/^(\.\.[/\\])+/, "");
    let p = join(WEB, rel);
    if (existsSync(p) && statSync(p).isDirectory()) p = join(p, "index.html");
    if (!existsSync(p)) { res.writeHead(404); return res.end(); }
    res.writeHead(200, { "Content-Type": TYPES[extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(readFileSync(p));
  });
  return new Promise((ok) => srv.listen(PORT, "0.0.0.0", () => ok(srv)));
}

if (process.argv.includes("--serve")) {
  await serve();
  console.log(`serving http://localhost:${PORT}/work/?view (reads forwarded to ${AGENT.host}:${AGENT.port}); Ctrl-C stops`);
} else await run();

// ---- Chrome ------------------------------------------------------------------------------------------
async function run() {
  mkdirSync(OUT, { recursive: true });
  const srv = await serve();
  const profile = mkdtempSync(join(tmpdir(), "scenic-touch-chrome-"));
  const chrome = spawn("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars", "about:blank"],
    { stdio: ["ignore", "ignore", "pipe"] });
  const results = [];
  // (A Ctrl-C, a kill or a run past 3 min: Chrome and its profile go too, by its own PID.)
  const stop = () => { chrome.kill("SIGTERM"); setTimeout(() => { rmSync(profile, { recursive: true, force: true }); process.exit(2); }, 1000); };
  process.once("SIGINT", stop).once("SIGTERM", stop);
  setTimeout(() => { console.log("FAIL the run (past 3 min)"); stop(); }, 180000).unref();
  const check = (name, ok, note = "") => { results.push({ name, ok }); console.log(`${ok ? "PASS" : "FAIL"} ${name}${note ? ` (${note})` : ""}`); };
  try {
    const wsBrowser = await new Promise((ok, no) => {
      let err = "";
      chrome.stderr.on("data", (d) => { err += d; const m = err.match(/ws:\/\/[^\s]+/); if (m) ok(m[0]); });
      chrome.once("error", no); // (No Chrome there: no exit event follows.)
      setTimeout(() => no(new Error("Chrome didn't start in 15 s")), 15000);
    });
    const list = await (await fetch(`http://${new URL(wsBrowser).host}/json/list`)).json();
    const page = await cdp(list.find((t) => t.type === "page").webSocketDebuggerUrl);
    const errors = [];
    page.on("Runtime.exceptionThrown", (p) => errors.push(p.exceptionDetails.exception?.description || p.exceptionDetails.text));
    // (The page asks the other Macs straight; they don't let this test origin read them, or are
    // away: those failures are the test's setting, not the page's, counted apart.)
    let cors = 0;
    const said = (t) => { const u = t.match(/https?:\/\/[^\s'"]+/)?.[0] || ""; if (u && !u.startsWith(`http://127.0.0.1:${PORT}/`)) cors++; else errors.push(t); };
    page.on("Runtime.consoleAPICalled", (p) => p.type === "error" && said(p.args.map((a) => a.value ?? a.description).join(" ")));
    page.on("Log.entryAdded", (p) => p.entry.level === "error" && said(`${p.entry.text} ${p.entry.url || ""}`));
    await Promise.all(["Runtime.enable", "Log.enable", "Page.enable"].map((m) => page.send(m)));
    await page.send("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
    await page.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
    await page.send("Emulation.setUserAgentOverride", { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1", platform: "iPhone" });
    const js = async (expr) => { const r = await page.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text); return r.result.value; };
    const touch = (type, pts) => page.send("Input.dispatchTouchEvent", { type, touchPoints: pts.map(([x, y], id) => ({ x, y, id })) });
    const tap = async (x, y, hold = 0) => { await touch("touchStart", [[x, y]]); if (hold) await sleep(hold); await touch("touchEnd", []); await sleep(250); };
    const drag = async (x, y, dx, dy) => { await touch("touchStart", [[x, y]]); for (let i = 1; i <= 10; i++) { await touch("touchMove", [[x + (dx * i) / 10, y + (dy * i) / 10]]); await sleep(16); } await touch("touchEnd", []); await sleep(300); };
    const pinch = async (x, y, from, to) => { await touch("touchStart", [[x - from, y], [x + from, y]]); for (let i = 1; i <= 10; i++) { const d = from + ((to - from) * i) / 10; await touch("touchMove", [[x - d, y], [x + d, y]]); await sleep(16); } await touch("touchEnd", []); await sleep(300); };
    // (The whole page at 1 px a CSS px: a phone's 3× of a long tab is slow to make and to read.)
    const shot = async (name) => { const h = await js("document.scrollingElement.scrollHeight"); const { data } = await page.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: true, clip: { x: 0, y: 0, width: 390, height: Math.min(h, 6000), scale: 1 / 3 } }); writeFileSync(join(OUT, `${name}.png`), Buffer.from(data, "base64")); };
    const show = async (tab) => { await js(`location.hash = "#${tab}"; window.scrollTo(0, 0)`); await sleep(1200); };
    // (The center of the first visible element matching `sel`, in the viewport, scrolled to.)
    const at = (sel, where = "") => js(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.getClientRects().length && e.closest(".panel:not([hidden])")${where}); if (!el) return null; el.scrollIntoView({ block: "center" }); const r = el.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, r.width, r.height]; })()`);

    await page.send("Page.navigate", { url: `http://127.0.0.1:${PORT}/work/?view#overview` });
    for (let i = 0; i < 40 && !(await js(`!!document.querySelector('.panel[data-tab="overview"]')?.children.length`).catch(() => false)); i++) await sleep(250);
    await sleep(1500);

    // Each tab: no sideways scroll, no console errors, screenshots.
    for (const tab of TABS) {
      errors.length = 0;
      await show(tab);
      const wide = await js(`(() => { const w = document.documentElement.clientWidth; const over = [...document.querySelectorAll(".panel:not([hidden]) *, header *")].filter((e) => { const r = e.getBoundingClientRect(); return r.width && r.right > w + 1 && !e.closest(".twrap, [style*='overflow']"); }).slice(0, 4).map((e) => e.tagName.toLowerCase() + (e.className && typeof e.className === "string" ? "." + e.className.split(" ").join(".") : "") + " →" + Math.round(e.getBoundingClientRect().right)); return [document.scrollingElement.scrollWidth, w, over]; })()`);
      check(`${tab}: no horizontal scroll`, wide[0] <= wide[1], wide[0] > wide[1] ? `page ${wide[0]} px wide in ${wide[1]}; past the edge: ${wide[2].join(", ")}` : "");
      await shot(`chrome-${tab}`);
      check(`${tab}: no console errors`, !errors.length, errors.slice(0, 3).join(" | ").slice(0, 300));
    }

    // Cards pin on a tap, unpin on a tap elsewhere (the Jobs tab's).
    await show("jobs");
    const c = await at("[data-card]", ` && e.tagName !== "CANVAS"`);
    if (!c) check("jobs: a card pins on a tap", false, "nothing with a card shown");
    else {
      await tap(c[0], c[1]);
      check("jobs: a card pins on a tap", await js(`!!document.querySelector(".card-tip.pinned")`));
      await shot("chrome-card-pinned");
      const empty = await js(`(() => { for (let y = 120; y < innerHeight - 40; y += 20) for (let x = 6; x < 40; x += 8) { const e = document.elementFromPoint(x, y); if (e && !e.closest("[data-card], a, button, input, select, .card-tip, canvas")) return [x, y]; } return null; })()`);
      if (empty) await tap(...empty);
      check("jobs: the card unpins on a tap elsewhere", !(await js(`!!document.querySelector(".card-tip.pinned")`)), empty ? "" : "no empty spot found");
      const before = await js(`!!document.querySelector(".card-tip.pinned")`);
      await tap(c[0], c[1], 700);
      const lp = await js(`!!document.querySelector(".card-tip.pinned")`);
      console.log(`INFO jobs: a long-press on a card ${lp && !before ? "pins it" : "does nothing"}`);
      await js(`document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`);
    }

    // The Overview's stage bar: a tap on a segment pins the stage card.
    await show("overview");
    // (The strip comes with the lead's steps: waited for, up to 10 s.)
    let seg = null;
    for (let t = 0; t < 40 && !seg; t++) { seg = await at(".strip > i[data-card]"); if (!seg) await sleep(250); }
    if (!seg) check("overview: a stage-bar segment pins its card on a tap", false, "no segment with a card");
    else {
      await tap(seg[0], seg[1]);
      check("overview: a stage-bar segment pins its card on a tap", await js(`!!document.querySelector(".card-tip.pinned")`));
      await js(`document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`);
    }

    // The Overview's world map: a tap pins the card of the stack under it at once, the next tap
    // (the card let go between: a pinned card is modal, #627) the next spot's (#615: Mobile Safari
    // pinned nothing, then the tap before's), and a tap off the map pins nothing. (The stacks
    // found as a mouse names them in the canvas's data-also.)
    await show("overview");
    const spots = await js(`(() => { const c = [...document.querySelectorAll("canvas.wmap")].find((e) => e.getClientRects().length); c.scrollIntoView({ block: "center" }); const r = c.getBoundingClientRect(); const out = []; for (let i = 1; i < 20 && out.length < 2; i++) for (let j = 1; j < 12 && out.length < 2; j++) { const x = r.left + r.width * i / 20, y = r.top + r.height * j / 12; if (document.elementFromPoint(x, y) !== c) continue; c.dispatchEvent(new PointerEvent("pointermove", { clientX: x, clientY: y, pointerType: "mouse", bubbles: true })); const also = c.dataset.also; if (also && !out.some((o) => o[2] === also)) out.push([x, y, also]); } c.dispatchEvent(new PointerEvent("pointerleave", { pointerType: "mouse" })); return out; })()`);
    if (spots.length < 2) check("overview: a tap on the map pins its stack's card", false, "fewer than two spots with something under them");
    else {
      const pinned = () => js(`document.querySelector(".card-tip.pinned .tc-sec") ? [...document.querySelectorAll(".card-tip.pinned .tc-sec")].length + " " + document.querySelector(".card-tip.pinned .tc-title")?.textContent : null`);
      const results = [];
      // (Taps apart by more than a double tap's time: two quick ones zoom.)
      for (const [x, y, also] of spots) { await tap(x, y); results.push([await pinned(), also]); await js(`document.querySelector(".card-tip.pinned .x")?.click()`); await sleep(500); }
      const ok = results.every(([p, also]) => p && +p.split(" ")[0] === Math.min(8, also.split("|").length) && (!also.startsWith("mt ") || p.endsWith(also.split("|")[0].split(" ").pop())));
      check("overview: a tap on the map pins its stack's card, each tap its own", ok, results.map(([p, a]) => `${p} for ${a.slice(0, 60)}`).join(" | "));
      await tap(spots[0][0], spots[0][1]);
      await shot("chrome-map-pinned");
      // (Modal: under the pinned card a veil; a swipe on it scrolls nothing behind, a tap on it only
      // lets the card go.)
      const veiled = await js(`(() => { const v = document.querySelector(".card-veil"), c = document.querySelector(".card-tip.pinned"); if (!v || !c) return null; const r = c.getBoundingClientRect(); const x = r.left > 60 ? 20 : innerWidth - 20, y = innerHeight - 60; return [x, y, document.elementFromPoint(x, y) === v]; })()`);
      if (!veiled) check("overview: a pinned card's veil blocks a scroll behind it", false, "no veil under the pinned card");
      else {
        const y0 = await js("scrollY");
        await drag(veiled[0], veiled[1], 0, -200);
        await page.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: veiled[0], y: veiled[1], deltaX: 0, deltaY: 400 });
        await sleep(300);
        const y1 = await js("scrollY"), still = !!(await pinned());
        check("overview: a pinned card's veil blocks a scroll behind it", veiled[2] && y1 === y0 && still, `veil on top ${veiled[2]}, scrolled ${y0} → ${y1}, card ${still ? "kept" : "gone"}`);
        await tap(veiled[0], veiled[1]);
        check("overview: a tap on the veil only lets the card go", !(await pinned()) && !(await js(`!!document.querySelector(".card-veil")`)) && (await js("scrollY")) === y0);
      }
      const off = await js(`(() => { for (let y = 120; y < innerHeight - 40; y += 20) for (let x = 6; x < 40; x += 8) { const e = document.elementFromPoint(x, y); if (e && !e.closest("[data-card], a, button, input, select, label, .card-tip, canvas")) return [x, y]; } return null; })()`);
      if (off) await tap(...off);
      check("overview: a tap off the map pins nothing", off && !(await pinned()), off ? "" : "no empty spot found");
      // (The whole map again, as the pinch below starts from it.)
      await js(`[...document.querySelectorAll(".wmap-ctl")].find((e) => e.getClientRects().length).lastChild.click()`);
    }

    // A mouse over the Overview's map: the following card never catches the pointer, every item
    // in the stack is outlined, and leaving the map (out of the window too) lets the card go (#627).
    {
      const c = `[...document.querySelectorAll("canvas.wmap")].find((e) => e.getClientRects().length)`;
      const spot = await js(`(() => { const c = ${c}; c.scrollIntoView({ block: "center" }); const r = c.getBoundingClientRect(); for (let i = 1; i < 20; i++) for (let j = 1; j < 12; j++) { const x = r.left + r.width * i / 20, y = r.top + r.height * j / 12; if (document.elementFromPoint(x, y) !== c) continue; c.dispatchEvent(new PointerEvent("pointermove", { clientX: x, clientY: y, pointerType: "mouse", bubbles: true })); const also = c.dataset.also || ""; if (also.split("|").length > 1) return [x, y]; } return null; })()`);
      if (!spot) check("overview: a mouse's card and outlines on the map", false, "no spot with two or more under it");
      else {
        // (After the scroll to it has landed: a scroll lets a following card go.)
        await js('document.querySelector(".card-tip.pinned .x")?.click()');
        await sleep(400);
        await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: spot[0] + 1, y: spot[1] });
        await sleep(300);
        const st = await js(`(() => { const c = ${c}, t = document.querySelector(".card-tip:not(.pinned):not([hidden])"); if (!t) return null; const r = t.getBoundingClientRect(); const x = r.left + Math.min(20, r.width / 2), y = r.top + Math.min(20, r.height / 2); return { stack: c.dataset.also.split("|").length, outlined: +c.dataset.outlined, pe: getComputedStyle(t).pointerEvents, through: !t.contains(document.elementFromPoint(x, y)), x, y }; })()`);
        check("overview: a following card never catches the pointer", st && st.pe === "none" && st.through, st ? `pointer-events ${st.pe}, under it ${st.through ? "what's behind" : "the card"}` : "no card shown");
        check("overview: every item in the stack is outlined", st && st.outlined === st.stack, st ? `${st.outlined} outlined of ${st.stack}` : "no card shown");
        // (Out of the window: a pointerout to nowhere, as the browser sends one.)
        await js(`(() => { const c = ${c}; c.dispatchEvent(new PointerEvent("pointerout", { pointerType: "mouse", bubbles: true, relatedTarget: null })); c.dispatchEvent(new PointerEvent("pointerleave", { pointerType: "mouse" })); })()`);
        check("overview: the card goes on leaving the map", !(await js(`!!document.querySelector(".card-tip:not(.pinned):not([hidden])")`)) && (await js(`+${c}.dataset.outlined`)) === 0);
      }
    }

    // The world maps (every tab showing one): a pinch zooms, then a one-finger sideways drag pans
    // (fitted, the map has no room to pan sideways); the canvas's pixels change, the page stays put.
    let maps = 0;
    for (const tab of TABS) {
      await show(tab);
      const m = await at("canvas.wmap");
      if (!m) continue;
      maps++;
      const cv = `[...document.querySelectorAll("canvas.wmap")].find((e) => e.getClientRects().length && e.closest(".panel:not([hidden])"))`;
      const px = () => js(`${cv}.toDataURL()`);
      const y0 = await js("scrollY");
      // (The map's own say: its touch-action turns "none" only once zoomed in past the fit.)
      const wasZoomed = await js(`${cv}.style.touchAction === "none"`);
      await pinch(m[0], m[1], 20, 80);
      const zoomedNow = await js(`${cv}.style.touchAction === "none"`);
      check(`${tab}: the world map zooms with a pinch`, !wasZoomed && zoomedNow && (await js("scrollY")) === y0, `zoomed before ${wasZoomed}, after ${zoomedNow}`);
      // (A redraw of its own, tiles fading in after the zoom or new data, would pass a pixel
      // compare: the map waited on until it holds still for 400 ms, at most 4 s.)
      let b = await px(), still = false;
      for (let t = 0; t < 10 && !still; t++) { await sleep(400); const c = await px(); still = c === b; b = c; }
      await drag(m[0] + m[2] * 0.25, m[1], -m[2] * 0.3, 0);
      check(`${tab}: the world map pans with a drag`, still && b !== (await px()) && (await js("scrollY")) === y0, still ? "" : "the map redrew by itself: the pan can't be told");
      await shot(`chrome-${tab}-map-moved`);
    }
    check("world maps shown", maps > 0, `${maps}`);

    // Every filter and legend toggle under each map toggled, focused as a click leaves it: the map still there,
    // sized, drawn (#627: a focused checkbox kept the old part, the map's element gone with it).
    for (const tab of ["overview", "regions"]) {
      await show(tab);
      const cv = `[...document.querySelectorAll("canvas.wmap")].find((e) => e.getClientRects().length && e.closest(".panel:not([hidden])"))`;
      const inputs = `[...document.querySelectorAll(".panel:not([hidden]) .wmap-filters input, .panel:not([hidden]) .wmap-legend button")]`;
      const count = await js(`${inputs}.length`);
      const bad = [];
      for (let i = 0; i < count; i++) {
        const what = await js(`(() => { const el = ${inputs}[${i}]; if (!el) return null; const t = el.closest("label")?.textContent || el.textContent; el.focus(); el.click(); return t; })()`);
        await sleep(350);
        const state = await js(`(() => { const c = ${cv}; if (!c) return "gone"; const r = c.getBoundingClientRect(); if (r.width < 50 || r.height < 50 || !c.width) return "zero size"; const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data; for (let j = 4; j < d.length; j += 4 * 97) if (d[j] !== d[0] || d[j + 1] !== d[1] || d[j + 2] !== d[2]) return ""; return "blank"; })()`);
        if (state) bad.push(`${what}: ${state}`);
      }
      // (Back as they were: the defaults.)
      await js(`localStorage.clear()`);
      check(`${tab}: the map stays drawn through every filter`, count > 0 && !bad.length, count ? bad.slice(0, 4).join("; ") || `${count} toggled` : "no filters shown");
    }

    // The Regions split: stacked at phone width (no bar); dragged at a touch tablet's (900 px).
    await show("regions");
    check("regions: no split bar at phone width (stacked)", !(await at(".rsplit")));
    await page.send("Emulation.setDeviceMetricsOverride", { width: 900, height: 1180, deviceScaleFactor: 2, mobile: true });
    await sleep(600);
    const s = await at(".rsplit");
    const split = () => js(`document.querySelector(".rpane")?.style.getPropertyValue("--split")`);
    if (!s) check("regions: the split pane drags (900 px)", false, "no split bar shown");
    else {
      const f0 = await split();
      await drag(s[0], s[1], 0, -80);
      const f1 = await split();
      check("regions: the split pane drags (900 px)", f0 !== f1, `${f0 || "unset"} → ${f1 || "unset"}`);
    }
    await page.send("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
    await sleep(400);

    // The Jobs schedule's dropdown: a tap focuses it; a choice re-renders the schedule.
    await show("jobs");
    const sel = await at("select.past");
    if (!sel) check("jobs: the schedule's dropdown", false, "not shown");
    else {
      await tap(sel[0], sel[1]);
      const focused = await js(`document.activeElement?.matches("select.past")`);
      const changed = await js(`(async () => { const el = document.querySelector("select.past"); const was = el.value; const next = [...el.options].find((o) => o.value !== was).value; el.value = next; el.dispatchEvent(new Event("change", { bubbles: true })); await new Promise((r) => setTimeout(r, 600)); const now = document.querySelector("select.past").value; el.ownerDocument.querySelector("select.past").value = was; document.querySelector("select.past").dispatchEvent(new Event("change", { bubbles: true })); return now === next; })()`);
      check("jobs: the schedule's dropdown focuses on a tap and takes a choice", focused && changed, `focused ${focused}, choice kept ${changed}`);
    }

    // Settings fields (Machines): a tap focuses each without the page jumping. Never pressed Set.
    // (The fields come with the Macs' /work/settings answers, asked when the tab shows: waited
    // for, up to 10 s, rather than a fixed pause a slow answer outlasts.)
    await show("machines");
    let n = 0;
    for (let t = 0; t < 40 && !n; t++) { n = await js(`document.querySelectorAll('.panel[data-tab="machines"] input:not([type=checkbox]):not([type=radio])').length`); if (!n) await sleep(250); }
    let bad = [];
    for (let i = 0; i < Math.min(n, 4); i++) {
      const r = await js(`(() => { const el = document.querySelectorAll('.panel[data-tab="machines"] input:not([type=checkbox]):not([type=radio])')[${i}]; el.scrollIntoView({ block: "center" }); const b = el.getBoundingClientRect(); return [b.left + Math.min(20, b.width / 2), b.top + b.height / 2]; })()`);
      const y0 = await js("scrollY");
      await tap(...r);
      const [foc, y1] = await js(`[document.activeElement === document.querySelectorAll('.panel[data-tab="machines"] input:not([type=checkbox]):not([type=radio])')[${i}], scrollY]`);
      if (!foc || Math.abs(y1 - y0) > 2) bad.push(`field ${i + 1}: focused ${foc}, moved ${Math.round(y1 - y0)} px`);
      await js("document.activeElement?.blur()");
    }
    check("machines: settings fields focus on a tap without a jump", n > 0 && !bad.length, n ? bad.join("; ") || `${Math.min(n, 4)} of ${n} tried` : "no fields shown");
    if (cors) console.log(`INFO ${cors} failed calls to the other Macs (CORS refused this test origin, or away; not counted)`);
    page.close();
  } catch (e) {
    check("the run", false, e.message);
  } finally {
    chrome.kill("SIGTERM");
    await new Promise((ok) => (chrome.exitCode !== null || chrome.pid === undefined ? ok() : chrome.once("exit", ok)));
    rmSync(profile, { recursive: true, force: true });
    srv.close();
  }
  const failed = results.filter((r) => !r.ok).length;
  console.log(`${failed ? "FAIL" : "PASS"}: ${results.length - failed} of ${results.length} checks; screenshots in ${OUT}`);
  process.exit(failed ? 1 : 0);
}

function sleep(ms) { return new Promise((ok) => setTimeout(ok, ms)); }
// A CDP connection: send(method, params) → result; on(event, fn).
function cdp(url) {
  const ws = new WebSocket(url), wait = new Map(), subs = new Map();
  let id = 0;
  ws.onmessage = (m) => {
    const v = JSON.parse(m.data);
    if (v.id && wait.has(v.id)) { const [ok, no] = wait.get(v.id); wait.delete(v.id); v.error ? no(new Error(v.error.message)) : ok(v.result); }
    else if (v.method) for (const fn of subs.get(v.method) || []) fn(v.params);
  };
  return new Promise((ok) => (ws.onopen = () => ok({
    send: (method, params = {}) => new Promise((res, no) => { wait.set(++id, [res, no]); ws.send(JSON.stringify({ id, method, params })); }),
    on: (ev, fn) => subs.set(ev, [...(subs.get(ev) || []), fn]),
    close: () => ws.close(),
  })));
}
