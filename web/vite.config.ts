import { defineConfig, type Plugin } from 'vite';

/**
 * MapLibre fades circles and symbols behind 3D terrain by comparing their depth with its terrain
 * depth texture. On the globe the texture holds perspective depth (projectTileFor3D) while the
 * points' depth comes from projectTileWithElevation, which there returns MapLibre's clipping-plane
 * depth instead, so the comparison is meaningless: in tilted, zoomed-in views every point and
 * label was "behind the terrain" and vanished. Compute the points' visibility depth the way the
 * texture was drawn. (On the flat map the two functions are the same.)
 */
function maplibreTerrainVisibility(): Plugin {
  const fixes: [string, string][] = [
    ['calculate_visibility(projectTileWithElevation(circle_center,ele))', 'calculate_visibility(projectTileFor3D(circle_center,ele))'],
    ['calculate_visibility(projectedPoint)', 'calculate_visibility(projectTileFor3D(translated_a_pos,ele))'],
  ];
  return {
    name: 'maplibre-terrain-visibility',
    enforce: 'pre',
    transform(code, id) {
      if (!/maplibre-gl\/dist\/maplibre-gl(-dev)?\.mjs/.test(id)) return null;
      let out = code;
      for (const [from, to] of fixes) {
        if (!out.includes(from)) this.warn(`maplibre-terrain-visibility: "${from}" not found (MapLibre changed?)`);
        out = out.split(from).join(to);
      }
      return { code: out, map: null };
    },
  };
}

// In development the Rust backend serves data; Vite serves the app with HMR.
const backend = 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [maplibreTerrainVisibility()],
  server: {
    port: 5173,
    proxy: { '/api': backend, '/tiles': backend, '/fonts': backend },
  },
  // MapLibre v6 loads its worker relative to its own module; pre-bundling breaks that.
  optimizeDeps: { exclude: ['maplibre-gl'] },
  worker: { format: 'es' },
  build: { target: 'es2022', sourcemap: true, chunkSizeWarningLimit: 2000 },
});
