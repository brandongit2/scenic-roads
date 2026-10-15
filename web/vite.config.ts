import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { shadersPlugin } from './maplibre/build/shaders_plugin.ts';

/**
 * The map is drawn by our own MapLibre (web/maplibre/, FORK.md): `maplibre-gl` names its source,
 * which Vite compiles with the app (its shaders from their .glsl files, shadersPlugin), in the dev
 * server and the build alike. Its runtime dependencies are the app's own (package.json), resolved
 * from web/node_modules alone (`dedupe`: never a copy under web/maplibre/node_modules, its tests'),
 * at the versions its lockfile names: checked here, so the build fails rather than draw with others.
 */
const fork = fileURLToPath(new URL('./maplibre/', import.meta.url));
const forkDeps = Object.keys(JSON.parse(readFileSync(`${fork}package.json`, 'utf8')).dependencies as Record<string, string>)
  .filter((d) => !d.startsWith('@types/'));
{
  const lock = JSON.parse(readFileSync(`${fork}package-lock.json`, 'utf8')).packages as Record<string, { version: string }>;
  for (const d of forkDeps) {
    const want = lock[`node_modules/${d}`]?.version;
    let have = '(missing)';
    try {
      have = JSON.parse(readFileSync(new URL(`./node_modules/${d}/package.json`, import.meta.url), 'utf8')).version;
    } catch {}
    if (have !== want) throw new Error(`MapLibre's dependency ${d} is ${have} in web/node_modules, its lockfile has ${want}: set package.json to it and npm install`);
  }
}

// In development the Rust backend serves data; Vite serves the app with HMR.
const backend = 'http://127.0.0.1:7311';

export default defineConfig({
  plugins: [shadersPlugin()],
  resolve: {
    alias: [
      { find: /^maplibre-gl$/, replacement: `${fork}src/index.ts` },
      { find: /^maplibre-gl\//, replacement: fork },
    ],
    dedupe: forkDeps,
  },
  server: {
    port: 5173,
    proxy: { '/api': backend, '/tiles': backend, '/fonts': backend },
  },
  // (The dependency scan reads the fork's source too, its shaders' imports included.)
  optimizeDeps: { rolldownOptions: { plugins: [shadersPlugin()] } },
  worker: { format: 'es', plugins: () => [shadersPlugin()] },
  build: { target: 'es2022', sourcemap: true, chunkSizeWarningLimit: 2000 },
});
