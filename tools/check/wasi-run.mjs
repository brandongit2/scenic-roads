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
// The programs import their memory (tools/app/wasm.sh): given one here, up to 4 GB.
const imports = wasi.getImportObject();
const wantsMemory = WebAssembly.Module.imports(module).some((i) => i.module === 'env' && i.name === 'memory');
let memory = null, instance = null;
for (let initial = 1024; !instance; initial *= 2) {
  if (wantsMemory) imports.env = { memory: (memory = new WebAssembly.Memory({ initial, maximum: 65536 })) };
  try {
    instance = await WebAssembly.instantiate(module, imports);
  } catch (e) {
    if (!wantsMemory || !(e instanceof WebAssembly.LinkError) || initial >= 65536) throw e;
  }
}
memory = memory || instance.exports.memory;
const t2 = performance.now();
const code = wasi.start(wantsMemory ? { exports: { ...instance.exports, memory } } : instance);
const t3 = performance.now();
const mb = memory.buffer.byteLength / 2 ** 20;
process.stderr.write(`\ncompile ${(t1 - t0).toFixed(0)} run ${(t3 - t2).toFixed(0)} exit ${code} mem ${mb.toFixed(0)}\n`);
process.exit(code);
