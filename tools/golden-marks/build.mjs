// Bundles today's landmarks worker for Node, for the golden test (run.mjs imports ./worker.mjs).
//   node tools/golden-marks/build.mjs
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const here = path.dirname(fileURLToPath(import.meta.url));
const web = path.join(here, '../../web');
const { rolldown } = await import(path.join(web, 'node_modules/rolldown/dist/index.mjs'));
const b = await rolldown({ input: path.join(web, 'src/landmarks.worker.ts'), platform: 'node', external: [/^maplibre-gl/] });
await b.write({ file: path.join(here, 'worker.mjs'), format: 'esm' });
console.log('bundled', path.join(here, 'worker.mjs'));
