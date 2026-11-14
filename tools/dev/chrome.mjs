// Headless Chrome for the dev tools (shot.mjs, touch-test.mjs), spoken to over its debugging pipe
// (--remote-debugging-pipe), not a websocket: Node's built-in WebSocket (undici) asks Chrome for
// compressed messages and drops the connection, code 1006, when one inflates past 4 MiB
// (undici's permessage-deflate kDefaultMaxDecompressedSize), so a 2× screenshot of a busy map
// (a PNG of ~3 MB, ~4.2 MB as base64) never answered (#1301). The pipe has no such limit.
//
// Chrome is our own child with a throwaway profile; close() ends it by its PID (SIGTERM, SIGKILL
// after 5 s) and deletes the profile, and so does this process's exit or a SIGINT or SIGTERM to it
// (then exit 130 or 143). Its GL is
// Chrome's own choice: on a Mac, ANGLE on Metal, the GPU (SwiftShader, --use-angle=swiftshader,
// draws the map at a few frames a second).

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { constants, tmpdir } from "node:os";
import { join } from "node:path";

export const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/** Starts Chrome. `send(method, params, sessionId)` → its result; `on(method, fn, sessionId)`;
 * `page()` → a new tab's session ({send, on, close}); `close()`. A call still waiting when Chrome
 * goes is refused (never left hanging). */
export async function launch({ flags = [] } = {}) {
  const profile = mkdtempSync(join(tmpdir(), "scenic-chrome-"));
  const chrome = spawn(CHROME, ["--headless=new", "--remote-debugging-pipe", `--user-data-dir=${profile}`, "--no-first-run",
    "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars", ...flags, "about:blank"],
  { stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"] });
  const gone = () => { if (chrome.exitCode === null && chrome.signalCode === null && chrome.pid) try { process.kill(chrome.pid, "SIGTERM"); } catch {} rmSync(profile, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 }); };
  process.once("exit", gone);
  let err = "";
  chrome.stderr.on("data", (d) => (err = (err + d).slice(-2000)));
  const wait = new Map(), subs = new Map();
  let id = 0, buf = Buffer.alloc(0), ended = null;
  const end = (why) => { if (ended) return; ended = why; for (const [, [, no]] of wait) no(new Error(why)); wait.clear(); };
  chrome.once("error", (e) => end(`Chrome didn't start: ${e.message}`));
  chrome.once("exit", (c, s) => end(`Chrome exited (${s || c})${err ? `: ${err.trim().split("\n").at(-1)}` : ""}`));
  chrome.stdio[4].on("data", (d) => {
    buf = Buffer.concat([buf, d]);
    for (let i; (i = buf.indexOf(0)) >= 0; buf = buf.subarray(i + 1)) {
      const v = JSON.parse(buf.subarray(0, i).toString());
      if (v.id && wait.has(v.id)) { const [ok, no] = wait.get(v.id); wait.delete(v.id); v.error ? no(new Error(`${v.error.message}${v.error.data ? `: ${v.error.data}` : ""}`)) : ok(v.result); }
      else if (v.method) for (const fn of subs.get(`${v.sessionId || ""} ${v.method}`) || []) fn(v.params);
    }
  });
  chrome.stdio[3].on("error", () => {});
  const send = (method, params = {}, sessionId) => new Promise((ok, no) => {
    if (ended) return no(new Error(ended));
    wait.set(++id, [ok, no]);
    chrome.stdio[3].write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + "\0");
  });
  const on = (method, fn, sessionId) => { const k = `${sessionId || ""} ${method}`; subs.set(k, [...(subs.get(k) || []), fn]); };
  // Once, however often asked (a signal and the caller's own cleanup alike).
  let closing;
  const close = () => (closing ||= (async () => {
    if (chrome.pid && chrome.exitCode === null && chrome.signalCode === null) {
      const exited = new Promise((ok) => chrome.once("exit", ok));
      chrome.kill("SIGTERM");
      const t = setTimeout(() => chrome.kill("SIGKILL"), 5000);
      await exited;
      clearTimeout(t);
    }
    rmSync(profile, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 });
    process.removeListener("exit", gone).removeListener("SIGINT", signal).removeListener("SIGTERM", signal);
  })());
  // (A signal's own exit skips "exit" listeners: Chrome would go with its pipe, the profile stay.)
  const signal = (sig) => close().finally(() => process.exit(128 + constants.signals[sig]));
  process.once("SIGINT", signal).once("SIGTERM", signal);
  await send("Browser.getVersion");
  return {
    pid: chrome.pid,
    send, on,
    async page() {
      const { targetId } = await send("Target.createTarget", { url: "about:blank" });
      const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
      return {
        send: (m, p) => send(m, p, sessionId),
        on: (m, fn) => on(m, fn, sessionId),
        close: () => send("Target.closeTarget", { targetId }).catch(() => {}),
      };
    },
    close,
  };
}
