// Runs a wasm32-wasip1 program under Node's WASI (V8), the host's filesystem preopened at "/", for
// tools/check/same.py. The last line on stderr: "compile <ms> run <ms> exit <code> mem <MB>".
//   node tools/check/wasi-run.mjs <prog.wasm> <env as JSON> <arg>...
import { WASI } from 'node:wasi';
import { readFileSync } from 'node:fs';
const [, , wasmPath, envJson, ...args] = process.argv;
const t0 = performance.now();
const module = await WebAssembly.compile(readFileSync(wasmPath));
const t1 = performance.now();
const wasi = new WASI({ version: 'preview1', args: [wasmPath, ...args], env: JSON.parse(envJson), preopens: { '/': '/' }, returnOnExit: true });
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
const t2 = performance.now();
const code = wasi.start(instance);
const t3 = performance.now();
const mb = instance.exports.memory.buffer.byteLength / 2 ** 20;
process.stderr.write(`\ncompile ${(t1 - t0).toFixed(0)} run ${(t3 - t2).toFixed(0)} exit ${code} mem ${mb.toFixed(0)}\n`);
process.exit(code);
