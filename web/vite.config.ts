import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The SPA is built to web/dist and served by `uguisu serve --web` (ADR 0031).
// In development, API calls are proxied to a locally running `uguisu serve`.
export default defineConfig(({ mode }) => ({
  plugins: [svelte()],
  // Vitest would otherwise resolve Svelte's server build and refuse to mount
  // a component.
  resolve: mode === 'test' ? { conditions: ['browser'] } : undefined,
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8484',
        changeOrigin: false,
      },
    },
  },
  test: {
    // Components are tested against a DOM, so one environment covers both
    // them and the pure modules.
    environment: 'jsdom',
    setupFiles: ['./src/tests/setup.ts'],
    include: ['src/**/*.test.ts'],
  },
}));
