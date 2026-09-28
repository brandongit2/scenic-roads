import { defineConfig } from 'vite';

// In development the Rust backend serves data; Vite serves the app with HMR.
const backend = 'http://127.0.0.1:8080';

export default defineConfig({
  server: {
    port: 5173,
    proxy: { '/api': backend, '/tiles': backend, '/fonts': backend },
  },
  // MapLibre v6 loads its worker relative to its own module; pre-bundling breaks that.
  optimizeDeps: { exclude: ['maplibre-gl'] },
  worker: { format: 'es' },
  build: { target: 'es2022', sourcemap: true, chunkSizeWarningLimit: 2000 },
});
