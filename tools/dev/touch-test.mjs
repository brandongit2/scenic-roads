#!/usr/bin/env node
// The build page at phone size with touch (#614): headless Chrome over CDP as a 390×844 phone
// (DPR 3, mobile, 5 touch points, an iPhone user agent), real touches (Input.dispatchTouchEvent),
// every tab walked: no sideways scroll, no console errors, screenshots; then the touch checks
// (cards pin and unpin, the world maps pan and pinch, the Regions split drags, the schedule's
// dropdown, settings fields focus without a jump). One PASS/FAIL line per check; exit 1 on a FAIL.
//
//   node tools/dev/touch-test.mjs [--out DIR] [--port N]   the Chrome run
//   node tools/dev/touch-test.mjs --serve [--port N]       only the server (for Mobile Safari)
//   --catalogs FILE   /work/catalogs answered from FILE (`scenic catalogs > FILE`: an agent before
//                     #629 hasn't the call), `after` honoured; else forwarded as the other reads
//   --tocome FILE     /work/tocome answered from FILE ({at, macs: {mac: {step: [[tile, s]…]}}}, the
//                     lead's forecast::ToCome: an agent before #622 hasn't the call), `mac` honoured
//   --built FILE      /work/built answered from FILE ({at, steps: {step: [z/x/y…]}}, agent::builtview:
//                     an agent before #674 hasn't the call)
//   --live FILE       /work/live's `net.hosts` given FILE's hosts ({host: {in, out, requests, conns,
//                     open, retries, holds, limit, robots}}) beside the agent's own (an agent before
//                     #635 tells no robots.txt; a paused build moves nothing), the Activity tab's rows
//   --steps FILE      /work/regions with `steps` answered from FILE ({at, regions: {id: {step:
//                     [built, all]}}}, the lead's build::region_steps: an agent before #636 hasn't
//                     them); the Regions tab's stage bars then checked filled by them
//   --member NAME     the Overview map's "Show only for" set to NAME: its tiles to come shown, a
//                     card of one found, screenshots (chrome-member*), then set back
//   --width W         the Chrome run at W px wide (default 390, a phone; 768 or more: a desktop, DPR 2)
//   --scheme S        dark or light (prefers-color-scheme; default the system's)
//   --agent H:P       the agent read (default 127.0.0.1:8090; the lead's to see the NAS tab filled)
//   --only-catalogues only the Overview map's Catalogues tab's checks and screenshots
//
// The page comes from a read-only server on its own port (8097): web/work as /work/, and only the
// reading calls forwarded to this Mac's agent (127.0.0.1:8090); everything else is refused, so the
// page can't lease work, act or set a setting. Its Pause is the test's own (#673): /work/pause is
// answered here, never forwarded, and /work/swarm's `pause` is this server's (none until a press;
// then PAUSE_LAG ms after it, as the build takes seconds to say so), so the pool is never paused. The page opens as `?view` (never helps).
// (Other Macs' /work/settings and /work/live the page asks straight; it only reads them.) A setting
// the page sets (the limits' drags and editor, #612) never leaves it: the page's fetch is wrapped
// before its scripts run, a POST to any /work/settings with a setting in its body recorded in
// `window.__posted` and sent as a read ({}) instead; this server refuses such a body anyway.
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
// (`--agent HOST:PORT`: another Mac's agent read instead, e.g. the lead's: the page is then its own
// Mac's, the NAS tab's answer its own, no cross-origin call the test origin isn't allowed.)
const AGENT = (([host, port]) => ({ host, port: +port }))(arg("--agent", "127.0.0.1:8090").split(":"));
const TABS = ["overview", "jobs", "machines", "regions", "nas", "dropboxes", "activity", "details"];

// ---- The read-only server ------------------------------------------------------------------------
// A call forwarded only when its body has no other keys than these (an empty body: {}).
const READS = { pause: ["pause"], swarm: [], steps: [], regions: ["steps"], settings: [], history: ["since", "max"], live: ["range", "series", "net", "since"], catalogs: ["after"], tocome: ["mac"], built: [] };
const CATS = arg("--catalogs", null);
const TOCOME = arg("--tocome", null);
const BUILT = arg("--built", null);
const LIVE = arg("--live", null);
const STEPS = arg("--steps", null);
const MEMBER = arg("--member", null);
const WIDTH = +arg("--width", 390);
const PHONE = WIDTH < 768;
const NARROW = WIDTH < 720; // dash.css's phone layout (max-width: 719px): stacked, wrapping
const SCHEME = arg("--scheme", null);
const PAUSE_LAG = +arg("--pause-lag", 3000);
const fake = { pause: null, posts: 0 }; // (the page's Pause as this server keeps it)
const ONLY_CATS = process.argv.includes("--only-catalogues");
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".png": "image/png", ".webmanifest": "application/manifest+json", ".svg": "image/svg+xml" };
function serve() {
  const srv = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const call = url.pathname.match(/^\/work\/([a-z]+)$/)?.[1];
    if (call && call in READS) {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", async () => {
        let v;
        try { v = JSON.parse(body); } catch { v = null; }
        if (req.method !== "POST" || !v || typeof v !== "object" || Array.isArray(v) || Object.keys(v).some((k) => !READS[call].includes(k))) {
          res.writeHead(403, { "Content-Type": "application/json" });
          return res.end(JSON.stringify({ error: "the touch-test server forwards reads only" }));
        }
        if (call === "catalogs" && CATS) {
          const a = JSON.parse(readFileSync(CATS, "utf8"));
          a.catalogs = a.catalogs.filter((c) => v.after == null || c.n > v.after);
          res.writeHead(200, { "Content-Type": "application/json" });
          return res.end(JSON.stringify(a));
        }
        if (call === "built" && BUILT) {
          res.writeHead(200, { "Content-Type": "application/json" });
          return res.end(readFileSync(BUILT));
        }
        if (call === "regions" && v.steps && STEPS) {
          res.writeHead(200, { "Content-Type": "application/json" });
          return res.end(readFileSync(STEPS));
        }
        if (call === "tocome" && TOCOME) {
          // (As forecast::ToCome::answer: the Mac named, or the one its name starts with before a "-";
          // its `at` the agent's forecast's, as the page takes this Mac's answer only for the forecast
          // its status shows (else asks the lead), the offsets kept from FILE's own `at`.)
          const a = JSON.parse(readFileSync(TOCOME, "utf8"));
          const mac = String(v.mac || ""), hit = Object.keys(a.macs).find((k) => k === mac) || Object.keys(a.macs).find((k) => mac.startsWith(`${k}-`));
          const at = await fetch(`http://${AGENT.host}:${AGENT.port}/work/swarm`, { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" }).then((r) => r.json()).then((w) => w.agent?.forecast?.at ?? a.at, () => a.at);
          const steps = hit ? Object.fromEntries(Object.entries(a.macs[hit]).map(([k, ts]) => [k, ts.map(([t, s]) => [t, Math.max(0, s + a.at - at)])])) : {};
          res.writeHead(200, { "Content-Type": "application/json" });
          return res.end(JSON.stringify({ at, mac: hit || mac, steps, macs: Object.keys(a.macs) }));
        }
        if (call === "pause") {
          fake.posts++;
          const p = v.pause && { ...v.pause, at: Math.floor(Date.now() / 1000) };
          setTimeout(() => { fake.pause = p; }, PAUSE_LAG);
          res.writeHead(200, { "Content-Type": "application/json" });
          return res.end("{}");
        }
        const out = JSON.stringify(v);
        if (call === "live" && LIVE && v.net !== false) {
          const a = await fetch(`http://${AGENT.host}:${AGENT.port}/work/live`, { method: "POST", headers: { "Content-Type": "application/json" }, body: out }).then((r) => r.json(), () => ({}));
          a.net = { events: [], ...a.net, hosts: { ...a.net?.hosts, ...JSON.parse(readFileSync(LIVE, "utf8")) } };
          res.writeHead(200, { "Content-Type": "application/json" });
          return res.end(JSON.stringify(a));
        }
        const fw = request({ ...AGENT, method: "POST", path: url.pathname, headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(out) } }, (r) => {
          if (call !== "swarm" || r.statusCode !== 200) { res.writeHead(r.statusCode, r.headers); return r.pipe(res); }
          let b = "";
          r.on("data", (c) => (b += c));
          r.on("end", () => { const o = { ...JSON.parse(b), pause: fake.pause }; res.writeHead(200, { "Content-Type": "application/json" }); res.end(JSON.stringify(o)); });
        });
        fw.on("error", (e) => { console.error(`touch-test server: ${url.pathname}: ${e.message}`); res.writeHead(502); res.end("{}"); });
        fw.end(out);
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/__posts") { res.writeHead(200); return res.end(String(fake.posts)); }
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
  // (A Ctrl-C, a kill or a run past 5 min: Chrome and its profile go too, by its own PID.)
  const stop = () => { chrome.kill("SIGTERM"); setTimeout(() => { rmSync(profile, { recursive: true, force: true }); process.exit(2); }, 1000); };
  process.once("SIGINT", stop).once("SIGTERM", stop);
  setTimeout(() => { console.log("FAIL the run (past 5 min)"); stop(); }, 300000).unref();
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
    await page.send("Emulation.setDeviceMetricsOverride", PHONE ? { width: WIDTH, height: 844, deviceScaleFactor: 3, mobile: true } : { width: WIDTH, height: 900, deviceScaleFactor: 2, mobile: false });
    if (PHONE) {
      await page.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
      await page.send("Emulation.setUserAgentOverride", { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1", platform: "iPhone" });
    }
    if (SCHEME) await page.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: SCHEME }] });
    const js = async (expr) => { const r = await page.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text); return r.result.value; };
    const touch = (type, pts) => page.send("Input.dispatchTouchEvent", { type, touchPoints: pts.map(([x, y], id) => ({ x, y, id })) });
    const tap = async (x, y, hold = 0) => { await touch("touchStart", [[x, y]]); if (hold) await sleep(hold); await touch("touchEnd", []); await sleep(250); };
    const drag = async (x, y, dx, dy) => { await touch("touchStart", [[x, y]]); for (let i = 1; i <= 10; i++) { await touch("touchMove", [[x + (dx * i) / 10, y + (dy * i) / 10]]); await sleep(16); } await touch("touchEnd", []); await sleep(300); };
    const pinch = async (x, y, from, to) => { await touch("touchStart", [[x - from, y], [x + from, y]]); for (let i = 1; i <= 10; i++) { const d = from + ((to - from) * i) / 10; await touch("touchMove", [[x - d, y], [x + d, y]]); await sleep(16); } await touch("touchEnd", []); await sleep(300); };
    // (The whole page at 1 px a CSS px: a phone's 3× of a long tab is slow to make and to read.)
    const shot = async (name) => { const h = await js("document.scrollingElement.scrollHeight"); const { data } = await page.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: true, clip: { x: 0, y: 0, width: WIDTH, height: Math.min(h, 6000), scale: PHONE ? 1 / 3 : 1 / 2 } }); writeFileSync(join(OUT, `${name}.png`), Buffer.from(data, "base64")); };
    const show = async (tab) => { await js(`location.hash = "#${tab}"; window.scrollTo(0, 0)`); await sleep(1200); };
    // (The center of the first visible element matching `sel`, in the viewport, scrolled to.)
    const at = (sel, where = "") => js(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.getClientRects().length && e.closest(".panel:not([hidden])")${where}); if (!el) return null; el.scrollIntoView({ block: "center" }); const r = el.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, r.width, r.height]; })()`);

    await page.send("Page.addScriptToEvaluateOnNewDocument", { source: stub() });
    await page.send("Page.navigate", { url: `http://127.0.0.1:${PORT}/work/?view#overview` });
    for (let i = 0; i < 40 && !(await js(`!!document.querySelector('.panel[data-tab="overview"]')?.children.length`).catch(() => false)); i++) await sleep(250);
    await sleep(1500);

    // The Overview map's Catalogues tab (#629): shown, its catalogues read, the map drawn, the
    // timeline's marks; a mark hovered lights its tiles; the range's inputs swap two times out of
    // order and refuse two spans; back to Build progress, its map there.
    {
      errors.length = 0;
      await show("overview");
      const tabBtn = (t) => js(`(() => { const b = [...document.querySelectorAll(".panel:not([hidden]) .maptabs button")].find((e) => e.textContent === ${JSON.stringify(t)}); if (!b) return false; b.click(); return true; })()`);
      let tabbed = false;
      for (let i = 0; i < 40 && !(tabbed = await tabBtn("Catalogues")); i++) await sleep(250);
      check("overview: the map's tabs shown", tabbed);
      let marks = 0;
      for (let i = 0; i < 120 && !marks; i++) { await sleep(500); marks = await js(`document.querySelectorAll(".panel:not([hidden]) .cat-tl g.mk").length`); }
      check("catalogues: the timeline marks the catalogues in its span", marks > 0, `${marks} marks; ${await js(`document.querySelector(".cat-status")?.textContent`)}`);
      const lbls = await js(`(() => { const r = [...document.querySelectorAll(".panel:not([hidden]) .cat-tl .lbl")].map((e) => { const b = e.getBoundingClientRect(); return [b.left, b.right, e.textContent]; }).sort((p, q) => p[0] - q[0]); const bad = r.filter((v, i) => i && v[0] < r[i - 1][1]).map((v) => v[2]); return [r.length, bad, r.filter((v) => !/^\\d+ · /.test(v[2])).length]; })()`);
      check("catalogues: mark labels name number and time, never overlap", lbls[0] > 0 && !lbls[1].length && !lbls[2], `${lbls[0]} labels; overlapping ${lbls[1].join(", ")}; number alone ${lbls[2]}`);
      const cv = `[...document.querySelectorAll(".panel:not([hidden]) .cat canvas.wmap")][0]`;
      const pix = () => js(`(() => { const c = ${cv}; if (!c || !c.width) return null; const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data; let h = 0; for (let j = 0; j < d.length; j += 4 * 13) h = (h * 31 + d[j] + d[j + 1] * 3 + d[j + 2] * 7) | 0; return h; })()`);
      const p0 = await pix();
      check("catalogues: its map drawn", p0 != null);
      const wide = await js(`[document.scrollingElement.scrollWidth, document.documentElement.clientWidth]`);
      check("catalogues: no horizontal scroll", wide[0] <= wide[1], `${wide[0]} in ${wide[1]}`);
      await js(`document.querySelector(".panel:not([hidden]) .cat")?.scrollIntoView({ block: "start" })`);
      await sleep(300);
      await shot(`chrome-catalogues`);
      // (The mark with the most tiles in the range, hovered by a mouse.)
      const mk = await js(`(() => { const all = [...document.querySelectorAll(".panel:not([hidden]) .cat-tl g.mk")]; const top = (g) => { const q = g.querySelector(".hit").getBoundingClientRect(); return document.elementFromPoint(q.left + q.width / 2, q.top + q.height / 2)?.closest("g.mk") === g; }; const g = all.reverse().find((e) => !e.classList.contains("out") && top(e)) || all[0]; if (!g) return null; g.scrollIntoView({ block: "center" }); const r = g.querySelector(".hit").getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, g.dataset.n]; })()`);
      if (!mk) check("catalogues: a mark hovered lights its tiles", false, "no mark");
      else {
        await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: mk[0], y: mk[1], pointerType: "mouse" });
        await sleep(400);
        const lit = await js(`[document.querySelector(".panel:not([hidden]) .cat-tl g.mk.on")?.dataset.n, !document.querySelector(".cat-tip").hidden, document.querySelector(".cat-tip").textContent]`);
        check("catalogues: a mark hovered lights its tiles", lit[0] === mk[2] && lit[1] && (await pix()) !== p0, `${lit[2]}`);
        // (Its tip under the timeline, off the legend and the timeline's marks above it.)
        const over = await js(`(() => { const t = document.querySelector(".cat-tip").getBoundingClientRect(); const hit = (s) => { const r = document.querySelector(".panel:not([hidden]) " + s).getBoundingClientRect(); return t.left < r.right && r.left < t.right && t.top < r.bottom && r.top < t.bottom; }; return [hit(".cat-legend"), hit(".cat-tl")]; })()`);
        check("catalogues: the hover tip covers neither the legend nor the timeline", !over[0] && !over[1], `legend ${over[0]}, timeline ${over[1]}`);
        // (The page whole, the pointer held on the mark: a screenshot past the viewport would
        // move it.)
        const { data } = await page.send("Page.captureScreenshot", { format: "png" });
        writeFileSync(join(OUT, "chrome-catalogues-hover.png"), Buffer.from(data, "base64"));
        await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 2, y: 2, pointerType: "mouse" });
      }
      // The range's inputs (typed as a user would, committed by Enter).
      const typeIn = async (i, text) => {
        await js(`(() => { const e = document.querySelectorAll(".panel:not([hidden]) input.cat-when")[${i}]; e.focus(); e.select(); })()`);
        await page.send("Input.insertText", { text });
        await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
        await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
        await sleep(300);
        return js(`[...document.querySelectorAll(".panel:not([hidden]) input.cat-when")].map((e) => e.value).concat(document.querySelector(".cat-msg").hidden ? "" : document.querySelector(".cat-msg").textContent)`);
      };
      const saved = await js(`localStorage.getItem("catalogues")`);
      await typeIn(1, "2026-10-20 12:00");
      const sw = await typeIn(0, "2026-10-22 12:00");
      check("catalogues: a left time later than the right swaps them", sw[0] === "2026-10-20 12:00" && sw[1] === "2026-10-22 12:00", sw.join(" | "));
      await typeIn(0, "48 hours");
      const two = await typeIn(1, "7 days");
      check("catalogues: one span at a time, the left's negative", two[0] === "-2 days" && two[1] === "2026-10-22 12:00" && /Only one/.test(two[2]), two.join(" | "));
      await js(`localStorage.${saved == null ? `removeItem("catalogues")` : `setItem("catalogues", ${JSON.stringify(saved)})`}`);
      check("catalogues: no console errors", !errors.length, errors.slice(0, 3).join(" | ").slice(0, 300));
      await tabBtn("Build progress");
      await sleep(500);
      check("overview: back on Build progress, its map shown", await js(`!![...document.querySelectorAll(".panel:not([hidden]) .wmap-box > .wmap-wrap canvas.wmap")].find((c) => c.getClientRects().length && c.width)`));
      await js(`localStorage.removeItem("mapTab")`);
    }
    if (ONLY_CATS) throw Object.assign(new Error("only the Catalogues tab"), { done: true });

    // A pool member chosen (#622): its tiles to come in the legend and on the map, a card of one
    // saying when and where; then all members again.
    if (MEMBER) {
      errors.length = 0;
      await show("overview");
      const pick = (v) => js(`(() => { const e = [...document.querySelectorAll('.panel:not([hidden]) .wmap-filters input[type=radio][value=' + ${JSON.stringify(JSON.stringify(v))} + ']')].find((x) => x.name.startsWith("mf-member")); if (!e) return false; e.click(); return true; })()`);
      check("member: its radio chosen", await pick(MEMBER));
      let lg = null;
      for (let i = 0; i < 40 && !(lg = await js(`[...document.querySelectorAll(".panel:not([hidden]) .wmap-legend .lg")].map((e) => e.textContent).find((t) => / to come on /.test(t)) || null`)); i++) await sleep(250);
      check("member: the legend counts its tiles to come", !!lg && /^[1-9]/.test(lg), lg || "none");
      await shot("chrome-member");
      // (A spot whose stack has a tile to come, tapped (a click on a desktop): its pinned card says
      // when and on which Mac. The spots found as a mouse names their stacks in data-also.)
      // (With --tocome, only spots holding one of FILE's tiles for the member: those its card names.)
      const want = TOCOME ? (() => { const a = JSON.parse(readFileSync(TOCOME, "utf8")), hit = Object.keys(a.macs).find((k) => k === MEMBER || MEMBER.startsWith(`${k}-`)); return hit ? Object.entries(a.macs[hit]).flatMap(([st, ts]) => ts.map(([t]) => `mt ${st} ${t}`)) : []; })() : [];
      const spots = await js(`(() => { const want = ${JSON.stringify(want)}, c = [...document.querySelectorAll("canvas.wmap")].find((e) => e.getClientRects().length); c.scrollIntoView({ block: "center" }); const r = c.getBoundingClientRect(); const out = [], seen = new Set(); for (let i = 1; i < 60 && out.length < 40; i++) for (let j = 1; j < 40 && out.length < 40; j++) { const x = r.left + r.width * i / 60, y = r.top + r.height * j / 40; if (document.elementFromPoint(x, y) !== c) continue; c.dispatchEvent(new PointerEvent("pointermove", { clientX: x, clientY: y, pointerType: "mouse", bubbles: true })); const also = c.dataset.also; if (also && (want.length ? want.some((k) => also.includes(k)) : also.includes("mt ")) && !seen.has(also)) { seen.add(also); out.push([x, y]); } } c.dispatchEvent(new PointerEvent("pointerleave", { pointerType: "mouse" })); return out; })()`);
      let said = null;
      for (const [x, y] of spots) {
        if (PHONE) await tap(x, y);
        else { for (const type of ["mousePressed", "mouseReleased"]) await page.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, pointerType: "mouse" }); await sleep(250); }
        said = await js(`(document.querySelector(".card-tip.pinned")?.textContent || "").match(/to come at ≈ \\d\\d:\\d\\d on \\S+/)?.[0] || null`);
        if (said) break;
        await js(`document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`);
      }
      check("member: a tile to come's card says when and where", !!said, said || `none in ${spots.length} spots`);
      if (said) {
        const { data } = await page.send("Page.captureScreenshot", { format: "png" });
        writeFileSync(join(OUT, "chrome-member-card.png"), Buffer.from(data, "base64"));
        await js(`document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`);
      }
      check("member: no console errors", !errors.length, errors.slice(0, 3).join(" | ").slice(0, 300));
      await pick("");
      await sleep(300);
    }

    // Each tab: no sideways scroll, no console errors, screenshots.
    let machineW = null;
    for (const tab of TABS) {
      errors.length = 0;
      await show(tab);
      const wide = await js(`(() => { const w = document.documentElement.clientWidth; const over = [...document.querySelectorAll(".panel:not([hidden]) *, header *")].filter((e) => { const r = e.getBoundingClientRect(); return r.width && r.right > w + 1 && !e.closest(".twrap, [style*='overflow']"); }).slice(0, 4).map((e) => e.tagName.toLowerCase() + (e.className && typeof e.className === "string" ? "." + e.className.split(" ").join(".") : "") + " →" + Math.round(e.getBoundingClientRect().right)); return [document.scrollingElement.scrollWidth, w, over]; })()`);
      check(`${tab}: no horizontal scroll`, wide[0] <= wide[1], wide[0] > wide[1] ? `page ${wide[0]} px wide in ${wide[1]}; past the edge: ${wide[2].join(", ")}` : "");
      // (No text cut off below 720 px, where the page wraps what won't fit (dash.css, #617); a
      // desktop keeps labels on one line, ellipsis and all, as the owner asked: not checked there.)
      const cut = NARROW ? await js(`(() => [...document.querySelectorAll(".panel:not([hidden]) *, header *")].filter((e) => e.scrollWidth > e.clientWidth + 1 && e.clientWidth && e.textContent.trim() && getComputedStyle(e).overflowX !== "visible" && getComputedStyle(e).overflowX !== "auto" && getComputedStyle(e).overflowX !== "scroll" && ![...e.childNodes].every((n) => n.nodeType === 1 && n.tagName !== "SPAN" && n.tagName !== "B" && n.tagName !== "I" && n.tagName !== "SMALL") && ![...e.childNodes].some((n) => n.nodeType === 1 && n.tagName === "CANVAS") ).slice(0, 6).map((e) => e.tagName.toLowerCase() + "." + String(e.className).split(" ").join(".") + " “" + e.textContent.trim().slice(0, 40) + "” " + e.scrollWidth + ">" + e.clientWidth))()`) : [];
      if (NARROW) check(`${tab}: no text cut off`, !cut.length, cut.join("; "));
      // (Regions: the list's rows have one fixed height, so every row's text must fit it: walked through the whole list.)
      if (tab === "regions") {
        const tall = await js(`(async () => { const R = document.querySelector(".rrow")?.parentElement; if (!R) return []; const L = R.parentElement, out = new Set(); for (let t = 0; t <= R.offsetHeight; t += 400) { L.scrollTop = t; L.dispatchEvent(new Event("scroll")); await new Promise((r) => setTimeout(r, 20)); for (const e of R.children) if (e.scrollHeight > e.clientHeight + 1) out.add(e.querySelector(".nm")?.textContent + " " + e.scrollHeight + ">" + e.clientHeight); } L.scrollTop = 0; return [...out].slice(0, 4); })()`);
        check("regions: every row's text fits its height", !tall.length, tall.join("; "));
        // (With --steps: each stage the lead counts filled by its share built, its numbers on hover;
        // the one under way striped past it, #636.)
        if (STEPS) {
          await js(`new Promise((r) => setTimeout(r, 300))`);
          const f = await js(`(() => { const s = [...document.querySelectorAll(".rrow .sbar > i.fill")]; const p = s.map((e) => parseFloat(e.style.getPropertyValue("--f"))); return [s.length, s.filter((e) => / of .+ built \\(\\d+%\\)/.test(e.title)).length, p.filter((x) => x > 0 && x < 100).length, p.filter((x) => !(x >= 0 && x <= 100)).length, document.querySelectorAll(".rrow .sbar > i.fill.run").length]; })()`);
          check("regions: the stage bars filled by the share built, numbers on hover", f[0] > 0 && f[1] === f[0] && f[2] > 0 && !f[3], `${f[0]} filled, ${f[1]} with numbers, ${f[2]} part-built, ${f[3]} out of range, ${f[4]} under way`);
        }
      }
      // (Jobs: each step's jobs on a row of their own under it, every one a box of its targets, none
      // folded into a "+N".)
      if (tab === "jobs") {
        const j = await js(`(() => { const s = document.querySelector(".panel:not([hidden]) .stages"); if (!s) return null; const rows = [...s.querySelectorAll(":scope > .cells")]; return [rows.length, s.querySelectorAll(".cells > .jb").length, s.querySelectorAll(".cells .more").length, rows.filter((r) => !r.previousElementSibling?.classList.contains("st")).length]; })()`);
        // (A paused build with no lease and no forecast has no job to draw: nothing to check then.)
        const post = (call) => fetch(`http://${AGENT.host}:${AGENT.port}/work/${call}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" }).then((r) => r.json(), () => ({}));
        const idle = j && !j[1] && (await (async () => {
          const sw = await post("swarm");
          const tc = TOCOME ? JSON.parse(readFileSync(TOCOME, "utf8")) : await post("tocome");
          const toCome = Object.values(tc.macs || {}).reduce((n, steps) => n + Object.values(steps).reduce((k, l) => k + l.length, 0), 0);
          return sw.pause && !sw.leases?.length && !toCome;
        })());
        if (idle) console.log("INFO jobs: each step's jobs a row under it: skipped, the build is paused with no job leased and no forecast (nothing to draw)");
        else check("jobs: each step's jobs a row under it, a box a job, none folded", j && j[1] > 0 && !j[2] && !j[3], j ? `${j[0]} rows, ${j[1]} jobs, ${j[2]} folded, ${j[3]} misplaced` : "no stages");
      }
      // (Machines and NAS: every measure a chart over time, the same width (the owner's 04:33 ask,
      // #634): the NAS tab's rows are the Machines tab's, no gauge left. The lead answering is the
      // agent's own business: with none, the rows are hidden and the check says so.)
      const charts = `[...document.querySelectorAll(".panel:not([hidden]) .rate canvas")].filter((c) => c.getClientRects().length).map((c) => Math.round(c.getBoundingClientRect().width))`;
      if (tab === "machines") for (let i = 0; i < 5 && machineW == null; i++) { machineW = (await js(charts))[0] ?? null; if (machineW == null) await sleep(1000); }
      if (tab === "nas") {
        await sleep(2000);
        const n = await js(`[${charts}, document.querySelectorAll(".panel:not([hidden]) .d-nas .rate").length, document.querySelectorAll(".panel:not([hidden]) .d-nas .cbar, .panel:not([hidden]) .d-nas .chips").length, document.querySelector(".panel:not([hidden]) .d-nas .lhead")?.textContent || ""]`);
        check("nas: every measure a chart, no gauges", n[1] === 9 && !n[2], `${n[1]} rows, ${n[2]} gauges; ${n[3].slice(0, 80)}`);
        check("nas: its charts as wide as the Machines tab's (±2 px: the cards' borders)", !n[0].length ? /asking|no live/.test(n[3]) : machineW != null && n[0].every((w) => Math.abs(w - machineW) <= 2), `${n[0].join(", ") || "none shown"} against ${machineW}`);
      }
      // (Activity: the websites' table ends in the mockup's robots.txt column, each row's cell a
      // state: n/a for an API's or data's host, else not checked yet, allowed or disallowed.)
      if (tab === "activity") {
        const r = await js(`(() => { const t = document.querySelector(".panel:not([hidden]) table.xhosts"); if (!t) return null; const rows = [...t.tBodies[0].rows]; return [[...t.tHead.rows[0].cells].at(-1).textContent, rows.length, rows.filter((tr) => !/^(n\\/a: API or data|not checked yet|(allowed|disallowed) \\(checked \\d\\d:\\d\\d\\))$/.test(tr.cells[tr.cells.length - 1].textContent)).length]; })()`);
        check("activity: the websites' last column is robots.txt, each row a state", r && r[0] === "robots.txt" && !r[2], r ? `${r[1]} rows, ${r[2]} without a state` : "no table");
      }
      await shot(`chrome-${tab}`);
      check(`${tab}: no console errors`, !errors.length, errors.slice(0, 3).join(" | ").slice(0, 300));
    }

    // The header's Pause, Freeze and Resume (#673): a press opens a confirmation and asks nothing;
    // Cancel asks nothing either; the confirmed one is pending at once (its words, disabled, the
    // header still saying what the build says), the page asks again each second so the state shows
    // within a few seconds of the lag, not at the next 10 s poll; then Resume the same way. The
    // asks go to this server's own /work/pause (never the pool's).
    {
      errors.length = 0;
      await show("overview");
      const until = async (expr, ms = 15000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { if (await js(expr).catch(() => false)) return Date.now() - t0; await sleep(50); } return -1; };
      const ctl = (text) => js(`(() => { const b = [...document.querySelectorAll(".verdict .ctl button")].find((e) => e.textContent.includes(${JSON.stringify(text)})); if (!b) return null; const r = b.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
      const pop = (text) => `[...document.querySelectorAll(".card-tip.pinned")].some((e) => e.querySelector(".ct")?.textContent.includes(${JSON.stringify(text)}))`;
      const inPop = (text) => js(`(() => { const b = [...document.querySelectorAll(".card-tip.pinned button")].find((e) => e.textContent === ${JSON.stringify(text)}); if (!b) return null; const r = b.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
      const posts = async () => +(await (await fetch(`http://127.0.0.1:${PORT}/__posts`)).text());
      const verdict = () => js(`document.querySelector(".verdict .say")?.textContent || ""`);
      check("pause: the controls are there", (await until(`!!document.querySelector(".verdict .ctl button")`)) >= 0);
      const p0 = await posts();
      await tap(...(await ctl("Pause at a safe point")));
      check("pause: a press opens a confirmation and asks nothing", (await until(pop("Pause at a safe point?"))) >= 0 && (await posts()) === p0 && !/Paused/.test(await verdict()), `${await js(`document.querySelectorAll(".card-tip.pinned").length`)} cards; ${await posts()} vs ${p0} posts; ${await verdict()}; ${errors.join(" / ").slice(0, 300)}`);
      await shot("chrome-pause-confirm");
      await tap(...(await inPop("Cancel")));
      check("pause: Cancel closes it and asks nothing", (await until(`!document.querySelector(".card-tip.pinned")`)) >= 0 && (await posts()) === p0);
      await tap(...(await ctl("Freeze now")));
      check("pause: Freeze asks too", (await until(pop("Freeze every job now?"))) >= 0);
      await tap(...(await inPop("Cancel")));
      await tap(...(await ctl("Pause at a safe point")));
      await until(pop("Pause at a safe point?"));
      const t0 = Date.now();
      await tap(...(await inPop("Pause")));
      const shown = await until(`(() => { const b = document.querySelector(".verdict .ctl button.pending"); return !!b && b.disabled && b.textContent === "Pausing…" && !document.querySelector(".card-tip.pinned") && [...document.querySelectorAll(".verdict .ctl button")].every((x) => x.disabled); })()`, 2000);
      check("pause: the press is pending at once (its words, the controls disabled, the card gone)", shown >= 0 && Date.now() - t0 < 2000 && !/Paused/.test(await verdict()), `${shown} ms`);
      await shot("chrome-pause-pending");
      const settled = await until(`/Paused/.test(document.querySelector(".verdict .say")?.textContent || "") && !document.querySelector(".verdict .ctl button.pending") && [...document.querySelectorAll(".verdict .ctl button")].some((x) => x.textContent === "Resume" && !x.disabled)`, 9000);
      check("pause: the header says Paused within seconds of the build's state, not at the next 10 s poll", settled >= 0, `${Date.now() - t0} ms after the press, lag ${PAUSE_LAG} ms`);
      await tap(...(await ctl("Resume")));
      check("resume: a press opens a confirmation and asks nothing more", (await until(pop("Resume the build?"))) >= 0 && (await posts()) === p0 + 1);
      await shot("chrome-resume-confirm");
      const t1 = Date.now();
      await tap(...(await inPop("Resume")));
      check("resume: pending at once", (await until(`document.querySelector(".verdict .ctl button.pending")?.textContent === "Resuming…"`, 2000)) >= 0 && /Paused/.test(await verdict()));
      const back = await until(`!/Paused/.test(document.querySelector(".verdict .say")?.textContent || "") && [...document.querySelectorAll(".verdict .ctl button")].some((x) => x.textContent === "Pause at a safe point" && !x.disabled)`, 9000);
      check("resume: the header drops Paused within seconds", back >= 0, `${Date.now() - t1} ms after the press`);
      check("pause: no console errors", errors.length === 0, errors.slice(0, 2).join(" | "));
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

    // Task #659: the map's legend a toggle per step, grouped by stage; the tile areas by pack type;
    // no display names (every stage, step and job label shown is the code's name); the Jobs tab's
    // cards as the mockup's.
    {
      errors.length = 0;
      const t = await js(`fetch("/work/steps", { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" }).then((r) => r.json())`);
      const steps = new Set((t.steps || []).map((s) => s.id)), stages = (t.stages || []).map((g) => g.id);
      const tiled = (t.steps || []).filter((s) => !s.of && (["area", "tile", "z10"].includes(s.scope) || /-root$/.test(s.id))).map((s) => s.id);
      await show("overview");
      const lg = await js(`(() => { const box = [...document.querySelectorAll(".wmap-steps")].find((e) => e.getClientRects().length); if (!box) return null; return [...box.querySelectorAll(".lgg")].map((g) => ({ stage: g.dataset.stage, head: g.querySelector(".lg.stage")?.dataset.name, steps: [...g.querySelectorAll(".lg.step")].map((b) => [b.dataset.name, b.textContent]) })); })()`);
      const flat = (lg || []).flatMap((g) => g.steps.map(([id]) => id));
      const order = (lg || []).map((g) => stages.indexOf(g.stage));
      const misgrouped = (lg || []).flatMap((g) => g.steps.filter(([id]) => (t.steps.find((s) => s.id === id)?.stage || "") !== g.stage).map(([id]) => id));
      check("legend: a toggle per step, grouped by stage in the build's order", lg && lg.length > 1 && lg.every((g) => g.head === g.stage && g.steps.length) && tiled.every((id) => flat.includes(id)) && !misgrouped.length && order.every((k, i) => k >= 0 && (!i || k > order[i - 1])),
        lg ? `${lg.length} stages, ${flat.length} steps; missing ${tiled.filter((id) => !flat.includes(id)).join(", ") || "none"}; misgrouped ${misgrouped.join(", ") || "none"}` : "no step legend");
      check("legend: each step counted", (lg || []).every((g) => g.steps.every(([id, text]) => text === `${id} ${text.slice(id.length + 1)}` && /^[\d,]+$/.test(text.slice(id.length + 1)))), (lg || []).flatMap((g) => g.steps.map(([, x]) => x)).slice(0, 4).join(" | "));
      // (A step toggled off: struck, its state kept; on again.)
      const flip = (sel) => js(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.getClientRects().length); if (!b) return null; b.click(); return [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.getClientRects().length)?.getAttribute("aria-pressed"); })()`);
      const s1 = await flip(`.wmap-steps .lg.step[data-name="${flat[0]}"]`), s2 = await flip(`.wmap-steps .lg.step[data-name="${flat[0]}"]`);
      check("legend: a step's item toggles it", s1 === "false" && s2 === "true", `${s1} then ${s2}`);
      const areas = await js(`[...([...document.querySelectorAll(".wmap-areas")].find((e) => e.getClientRects().length)?.querySelectorAll(".lg.area") || [])].map((b) => [b.dataset.name, getComputedStyle(b.querySelector("i")).borderTopWidth])`);
      check("legend: the tile areas by pack type, each its outline", areas.map((a) => a[0]).join() === "root,lo,hi,deep" && new Set(areas.map((a) => a[1])).size >= 2, areas.map((a) => `${a[0]} ${a[1]}`).join(", "));
      const cv = `[...document.querySelectorAll(".panel:not([hidden]) .wmap-box canvas.wmap")].find((c) => c.getClientRects().length)`;
      const pix = () => js(`(() => { const c = ${cv}; const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data; let h = 0; for (let j = 0; j < d.length; j += 4 * 7) h = (h * 31 + d[j] + d[j + 1] * 3 + d[j + 2] * 7) | 0; return h; })()`);
      const big = await js(`[...document.querySelectorAll(".wmap-areas .lg.area")].filter((e) => e.getClientRects().length).map((e) => [e.dataset.name, +e.textContent.replace(/.*· /, "").replace(/,/g, "")]).sort((p, q) => q[1] - p[1])[0]`);
      const p0 = await pix();
      const a1 = await flip(`.wmap-areas .lg.area[data-name="${big[0]}"]`);
      await sleep(300);
      const p1 = await pix();
      const a2 = await flip(`.wmap-areas .lg.area[data-name="${big[0]}"]`);
      check("legend: a tile area's item hides its tiles", a1 === "false" && a2 === "true" && (big[1] === 0 || p1 !== p0), `${big[0]} (${big[1]} tiles): ${a1} then ${a2}, map ${p1 !== p0 ? "changed" : "unchanged"}`);
      check("map: no \"Show tile areas\" radio left", !(await js(`[...document.querySelectorAll(".wmap-filters")].some((e) => /Show tile areas/.test(e.textContent))`)));
      // (Every label of a stage, step or job: the code's name. The legend's, the strip's stage cards,
      // the Jobs tab's stage and step rows, the schedule's legend, each running job on the Machines tab.)
      const names = await js(`({ legend: [...document.querySelectorAll(".wmap-legend .lg[data-name]")].map((b) => b.dataset.name), legendText: [...document.querySelectorAll(".wmap-steps .lg.step")].map((b) => b.textContent.split(" ")[0]), strip: [...document.querySelectorAll(".strip > i[data-card]")].map((e) => e.dataset.card.slice(2)) })`);
      await show("jobs");
      Object.assign(names, await js(`(() => { const txt = (e) => e.textContent.replace(" ⇄", "").trim(); return { stageRows: [...document.querySelectorAll(".stages > .sh .title")].map(txt), stepRows: [...document.querySelectorAll(".stages > .st .title")].map(txt), schedule: [...document.querySelectorAll(".sched .legend > span:not(.keys)")].map(txt).filter((x) => x !== "a region all on the map") }; })()`));
      await show("machines");
      names.jobs = await js(`[...document.querySelectorAll(".mc .job .what")].map((e) => [...e.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join("").trim()).filter((x) => x && !/^(Waiting|Nothing|Paused|This Mac)/.test(x))`);
      const area = new Set(["root", "lo", "hi", "deep"]);
      const bad = [
        ...names.legend.filter((x) => !steps.has(x) && !stages.includes(x) && !area.has(x)), ...names.legendText.filter((x) => !steps.has(x)),
        ...names.strip.filter((x) => !stages.includes(x)), ...names.stageRows.filter((x) => !stages.includes(x)), ...names.stepRows.filter((x) => !steps.has(x)),
        ...names.schedule.filter((x) => !stages.includes(x)), ...names.jobs.filter((x) => !steps.has(x.split(" ")[0])),
      ];
      const shown = Object.values(names).reduce((k, v) => k + v.length, 0);
      check("no display names: every stage, step and job label is the code's name", shown > 0 && !bad.length, `${shown} labels; not a code name: ${bad.slice(0, 5).join(" | ") || "none"}`);
      // The Jobs tab's cards: a step's (hovered by a mouse on a desktop, pinned by a tap on a phone),
      // then a job's: head (swatch, kicker, title), stat tiles, rows, a foot, twice the map cards'
      // width; a world map of its tiles, small until pinned; ≤ 50svh, off the pointer's line (#627).
      await show("jobs");
      const card = async (sel, where = "") => {
        const p = await at(sel, where);
        if (!p) return null;
        await sleep(300);
        if (PHONE) { await tap(p[0], p[1]); await sleep(300); }
        else { await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: p[0], y: p[1] }); await sleep(300); }
        const r = await js(`(() => { const t = [...document.querySelectorAll(".card-tip")].find((e) => e.getClientRects().length && !e.hidden); if (!t) return null; const b = t.getBoundingClientRect(), m = t.querySelector(".tc-map"); return { wide: !!t.querySelector(".tc-head.wide"), parts: [".tc-sw", ".tc-kick", ".tc-title", ".cstats", ".ckv", ".tc-foot"].filter((s) => !t.querySelector(s)), title: t.querySelector(".tc-title")?.textContent, kick: t.querySelector(".tc-kick")?.textContent, w: Math.round(b.width), top: b.top, bottom: b.bottom, h: b.height, max: innerHeight / 2, map: m ? Math.round(m.getBoundingClientRect().width) : 0 }; })()`);
        if (PHONE) await js(`document.querySelector(".card-tip.pinned .x")?.click()`);
        return { ...(r || { none: true }), y: p[1] };
      };
      const wide = Math.min(560, WIDTH - 16);
      const sc = await card(".stages > .st .title");
      check("jobs card: a step's as the mockup's (head, stat tiles, rows, foot; twice as wide)", sc && sc.wide && !sc.parts.length && steps.has(sc.title) && Math.abs(sc.w - wide) <= 2, sc ? `${sc.kick} / ${sc.title}, ${sc.w} px; missing ${sc.parts.join(" ") || "none"}` : "no step row");
      if (sc && !PHONE) check("jobs card: off the pointer's line, ≤ 50svh (#627)", (sc.top >= sc.y || sc.bottom <= sc.y) && sc.h <= sc.max + 1, `${Math.round(sc.top)}–${Math.round(sc.bottom)}, pointer ${Math.round(sc.y)}`);
      // (A job of a step whose targets are tiles: its map drawn. The tab's past jobs come with the
      // history read further back: waited for, up to 20 s. None then: an INFO only when the last
      // 6 h of the history has no such job's end either (the live build's state), else a FAIL.)
      const jobSel = ".cells > .jb.c-run, .cells > .jb.c-done", jobWhere = ` && ${JSON.stringify(tiled)}.includes(e.parentElement.previousElementSibling?.querySelector(".title")?.firstChild?.textContent)`;
      for (let k = 0; k < 40 && !(await at(jobSel, jobWhere)); k++) await sleep(500);
      const jc = await card(jobSel, jobWhere);
      const ended = jc ? 0 : await js(`(async () => { const tail = await fetch("/work/history", { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" }).then((r) => r.json()); const h = await fetch("/work/history", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ since: Math.max(0, (tail.seq || 0) - 5000), max: 5000 }) }).then((r) => r.json()); const from = Date.now() / 1000 - 6 * 3600; return (h.events || []).filter((e) => ["end", "done"].includes(e.kind) && e.ok !== false && e.t >= from && ${JSON.stringify(tiled)}.includes(e.step)).length; })()`);
      if (!jc && !ended) console.log("INFO jobs card: no finished or running job of a tiled step in the last 6 h (the live build's state), its map not checked");
      else if (!jc) check("jobs card: a job's named as the code names it, a world map of its tiles", false, `the history has ${ended} finished jobs of tiled steps in 6 h, none shown in 20 s`);
      else if (jc.none) check("jobs card: a job's named as the code names it, a world map of its tiles", false, "no card shown on a job's box");
      else check("jobs card: a job's named as the code names it, a world map of its tiles", jc.wide && steps.has(String(jc.title).split(/[ :]/)[0]) && jc.map > 0 && (PHONE || jc.map <= 200), `${jc.kick} / ${jc.title}, map ${jc.map} px`);
      if (!PHONE) await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 2, y: 2 });
      check("#659: no console errors", !errors.length, errors.slice(0, 3).join(" | ").slice(0, 300));
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
        // (Seen, not only marked hidden: the map's card is laid out by a rule of its own, which once
        // outranked [hidden] and kept it up, #627.)
        const seen = () => js(`[...document.querySelectorAll(".card-tip:not(.pinned)")].some((t) => t.getClientRects().length)`);
        const mouse = async (x, y) => { await page.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y }); await sleep(250); };
        const empty = await js(`(() => { for (let y = 60; y < innerHeight - 20; y += 10) for (let x = 6; x < innerWidth - 6; x += 10) { const e = document.elementFromPoint(x, y); if (e && !e.closest("[data-card], a, button, input, select, label, canvas, .card-tip")) return [x, y]; } return null; })()`);
        if (empty) await mouse(...empty);
        check("overview: the card goes on leaving the map", empty && !(await seen()) && (await js(`+${c}.dataset.outlined`)) === 0, empty ? "" : "no empty spot found");
        // (Out of the window: a pointerout to nowhere, as the browser sends one.)
        await mouse(spot[0], spot[1]);
        await js(`(() => { const c = ${c}; c.dispatchEvent(new PointerEvent("pointerout", { pointerType: "mouse", bubbles: true, relatedTarget: null })); c.dispatchEvent(new PointerEvent("pointerleave", { pointerType: "mouse" })); })()`);
        check("overview: the card goes on leaving the window", !(await seen()));
        // (A card's element, the stage bar's segment: on it, then off it.)
        const seg = await js(`(() => { const e = [...document.querySelectorAll("[data-card]:not(canvas)")].find((e) => e.getClientRects().length && e.getBoundingClientRect().width > 4); if (!e) return null; e.scrollIntoView({ block: "center" }); const r = e.getBoundingClientRect(), x = r.left + r.width / 2, y = r.top + r.height / 2; return document.elementFromPoint(x, y)?.closest("[data-card]") === e ? [x, y] : null; })()`);
        await sleep(300);
        const off = seg && (await js(`(() => { for (let y = 20; y < innerHeight - 20; y += 10) for (let x = 6; x < innerWidth - 6; x += 10) { const e = document.elementFromPoint(x, y); if (e && !e.closest("[data-card], a, button, input, select, label, canvas, .card-tip")) return [x, y]; } return null; })()`));
        if (off) { await mouse(...seg); const on = await seen(); await mouse(...off); check("overview: a card goes on leaving its element", on && !(await seen()), on ? "" : "no card shown on it"); }
        else check("overview: a card goes on leaving its element", false, "no card element or empty spot found");
        // (A tall card, the pointer near the window's bottom, then near its top: wholly above or
        // below the pointer's line, never across it, and no taller than 50svh.)
        for (const [where, frac] of [["bottom", 0.85], ["top", 0.15]]) {
          const at = await js(`(() => { const c = ${c}; c.scrollIntoView({ block: "center" }); const ax = ${spot[0]}, ay = ${spot[1]} + scrollY; scrollTo(0, ay - innerHeight * ${frac}); const y = ay - scrollY; return y > c.getBoundingClientRect().top && y < c.getBoundingClientRect().bottom ? [ax, y] : null; })()`);
          await sleep(300);
          if (!at) { check(`overview: a card near the ${where} stays off the pointer's line, ≤ 50svh`, false, "the map's spot can't scroll there"); continue; }
          await mouse(at[0] + 1, at[1]);
          const t = await js(`(() => { const t = [...document.querySelectorAll(".card-tip:not(.pinned)")].find((t) => t.getClientRects().length); if (!t) return null; const r = t.getBoundingClientRect(); return { top: r.top, bottom: r.bottom, h: r.height, max: innerHeight / 2 }; })()`);
          check(`overview: a card near the ${where} stays off the pointer's line, ≤ 50svh`, t && (t.top >= at[1] || t.bottom <= at[1]) && t.h <= t.max + 1, t ? `card ${Math.round(t.top)}–${Math.round(t.bottom)}, pointer ${Math.round(at[1])}, max ${Math.round(t.max)}` : "no card shown");
        }
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
    if (NARROW) check("regions: no split bar at phone width (stacked)", !(await at(".rsplit")));
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
    await limitChecks({ js, at, touch, tap, page, check, shot, PHONE });
    if (cors) console.log(`INFO ${cors} failed calls to the other Macs (CORS refused this test origin, or away; not counted)`);
    page.close();
  } catch (e) {
    if (!e.done) check("the run", false, e.message);
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

// The page's settings writes recorded, never sent (see the server's note).
function stub() {
  return `(() => {
  const real = window.fetch.bind(window);
  window.__posted = [];
  window.fetch = (u, o = {}) => {
    // (Anything but a bare read to a /work/settings is recorded and sent as one: a Request object,
    // a body not JSON, a trailing slash; this test's server refuses a write to its own Mac besides.)
    const url = typeof u === "string" || u instanceof URL ? String(u) : u.url;
    if (/\\/work\\/settings\\/?$/.test(new URL(url, location.href).pathname) && (typeof u === "object" && !(u instanceof URL) || (o.body != null && String(o.body).trim() !== "{}"))) {
      let b = o.body;
      try { b = JSON.parse(o.body); } catch {}
      window.__posted.push({ url, body: b });
      return real(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}", signal: o.signal });
    }
    return real(u, o);
  };
})();`;
}

// The limits on the Machines tab's charts (#612): the own Mac's memory line long-pressed and dragged
// sets it (recorded by stub(), not sent), Escape gives a drag up, the ✎'s editor sets and cancels, the
// battery floor's tag says it's the pool's; a mouse drags the line itself (desktop width).
async function limitChecks({ js, at, touch, tap, page, check, shot, PHONE }) {
  const own = `[...document.querySelectorAll('.panel[data-tab="machines"] .live')].find((b) => b.querySelector(".reftag.set"))`;
  const tagSel = (word) => `[...${own}.querySelectorAll(".reftag.set")].find((t) => t.title.includes(${JSON.stringify(word)}))`;
  let ready = false;
  for (let t = 0; t < 40 && !ready; t++) { ready = await js(`!!(${own} && ${tagSel("build memory")})`); if (!ready) await sleep(250); }
  if (!ready) { check("machines: the build memory's limit drags on a long press", false, "no settable memory line on this Mac's charts"); return; }
  const posted = () => js("window.__posted.length");
  const where = (word) => js(`(() => { const t = ${tagSel(word)}; t.scrollIntoView({ block: "center" }); const r = t.firstChild.getBoundingClientRect(); return [r.left + Math.min(12, r.width / 2), r.top + r.height / 2]; })()`);
  const press = async (x, y, dy, escape) => {
    await touch("touchStart", [[x, y]]); await sleep(650);
    for (let i = 1; i <= 8; i++) { await touch("touchMove", [[x, y + (dy * i) / 8]]); await sleep(30); }
    const preview = await js(`${tagSel("build memory")}?.classList.contains("live")`);
    if (escape) await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await touch("touchEnd", []); await sleep(500);
    return preview;
  };
  if (PHONE) {
    let n0 = await posted();
    const [x, y] = await where("build memory");
    const pv = await press(x, y, 25, true);
    check("machines: Escape gives up a limit's drag", pv && (await posted()) === n0, `previewed ${pv}, ${(await posted()) - n0} sent`);
    const scroll0 = await js("scrollY");
    const [x2, y2] = await where("build memory");
    const pv2 = await press(x2, y2, 25, false);
    const last = await js("window.__posted.at(-1)");
    check("machines: the build memory's limit drags on a long press and sets on release", pv2 && (await posted()) === n0 + 1 && last && Object.keys(last.body).join() === "memory" && Number.isInteger(last.body.memory),
      `previewed ${pv2}, sent ${JSON.stringify(last?.body)}`);
    check("machines: a limit's drag doesn't scroll the page", Math.abs((await js("scrollY")) - scroll0) <= 2);
  } else {
    const n0 = await posted();
    // (A mouse on the line itself where the plot shows it left of its tag (past the scale's
    // gutter), else on the tag's words; either at once, no long press.)
    const box = await js(`(() => { const t = ${tagSel("build memory")}; const c = t.parentNode.querySelector("canvas"); c.scrollIntoView({ block: "center" }); const r = c.getBoundingClientRect(), q = t.getBoundingClientRect(), w = t.firstChild.getBoundingClientRect(); return [r.left + 60, q.top, q.left, w.left + 12, w.top + w.height / 2]; })()`);
    let held = 0;
    const mouse = (type, x, y) => { held = type === "mousePressed" ? 1 : type === "mouseReleased" ? 0 : held; return page.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" && !held ? "none" : "left", buttons: held, clickCount: type === "mouseMoved" ? 0 : 1 }); };
    let line = null;
    if (box[2] - box[0] > 6) for (let dy = -6; dy <= 26 && !line; dy++) { await mouse("mouseMoved", box[0], box[1] + dy); if (await js(`${tagSel("build memory")}.parentNode.querySelector("canvas").style.cursor === "ns-resize"`)) line = [box[0], box[1] + dy]; }
    const by = line ? "line" : "tag";
    line ||= [box[3], box[4]];
    await mouse("mouseMoved", line[0], line[1]); await mouse("mousePressed", line[0], line[1]);
    for (let i = 1; i <= 6; i++) { await mouse("mouseMoved", line[0], line[1] - 4 * i); await sleep(30); }
    await mouse("mouseReleased", line[0], line[1] - 24); await sleep(500);
    const last = await js("window.__posted.at(-1)");
    check("machines: a mouse drags the build memory's limit and sets it on release", (await posted()) === n0 + 1 && last && "memory" in last.body, `by its ${by}, sent ${JSON.stringify(last?.body)}`);
  }
  // The ✎: the editor opens on the chart; Cancel closes it, Set sends the number typed.
  const penAt = () => js(`(() => { const t = ${tagSel("disk room")} || ${tagSel("build memory")}; t.scrollIntoView({ block: "center" }); const r = t.querySelector(".pen").getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, t.title]; })()`);
  const pen = await penAt();
  const n1 = await posted();
  await tap(pen[0], pen[1]);
  const open = await js(`!!${own}.querySelector(".cfg input")`);
  await shot("machines-limit-editor");
  const cancel = await js(`(() => { const b = [...${own}.querySelectorAll(".cfg button")].find((x) => x.textContent === "Cancel"); b.scrollIntoView({ block: "center" }); const r = b.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
  await tap(...cancel);
  const still = await js(`!!${own}.querySelector(".cfg")`);
  check("machines: a limit's ✎ opens its editor on the chart; Cancel closes it, nothing sent", open && !still && (await posted()) === n1, `opened ${open}, still open ${still}, ${(await posted()) - n1} sent`);
  const pen2 = await penAt();
  await tap(pen2[0], pen2[1]);
  await js(`(() => { const i = ${own}.querySelector(".cfg input"); i.value = "7"; })()`);
  const setB = await js(`(() => { const b = [...${own}.querySelectorAll(".cfg button")].find((x) => x.textContent === "Set"); b.scrollIntoView({ block: "center" }); const r = b.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
  await tap(...setB); await sleep(500);
  const sent = await js("window.__posted.at(-1)");
  const key = /disk room/.test(pen[2]) ? "room" : "memory";
  check("machines: the editor's Set sends the number typed", (await posted()) === n1 + 1 && sent?.body?.[key] === 7, JSON.stringify(sent?.body));
  check("machines: the battery floor's tag says it's pool-wide", await js(`[...${own}.querySelectorAll(".reftag.set")].some((t) => t.title.includes("battery floor") && /pool-wide/.test(t.textContent))`) || !(await js(`[...${own}.querySelectorAll(".reftag")].some((t) => /battery/.test(t.textContent))`)), "");
  // A limit at the chart's very top (every core) is still drawn, settable, its tag under the line.
  const top = await js(`(async () => {
    const { draw } = await import("/work/chart.js");
    const box = document.createElement("div"); box.id = "toptest"; box.style.cssText = "position:relative;width:420px";
    const c = document.createElement("canvas"); c.style.cssText = "display:block;width:100%;height:62px"; box.append(c);
    ${own}.append(box); box.scrollIntoView({ block: "center" });
    const now = Date.now() / 1000;
    draw(c, { from: now - 600, to: now, step: 5, axis: true, times: true, unit: "", stacked: true, max: 8, series: [], refs: [{ v: 8, label: "build's limit: 8 cores (all)", edit: { key: "in_use", title: "Cores for the build", unit: "cores", min: 1, max: 8, step: 1, label: (v) => "build's limit: " + v, set: async () => true } }] });
    const t = box.querySelector(".reftag.set");
    const r = t && t.getBoundingClientRect(), b = box.getBoundingClientRect();
    return t ? [r.top - b.top, r.bottom - b.top] : null;
  })()`);
  await shot("limit-at-top");
  check("machines: a limit at every core is drawn settable, its tag under the line, inside the chart", !!top && top[0] > 6.5 && top[1] <= 62, JSON.stringify(top));
  await js(`document.getElementById("toptest")?.remove()`);
  await js(`${own}.querySelector(".cfg")?.remove()`);
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
