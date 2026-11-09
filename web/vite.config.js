// SPDX-License-Identifier: AGPL-3.0-only
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Build output is embedded into the Rust binary (crates/app/src/web/assets.rs).
// No CDN, no inline scripts: the CSP only allows script-src 'self'.
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsInlineLimit: 0, // never inline assets as data: URLs
    modulePreload: { polyfill: false }, // the polyfill is an inline script
    target: 'es2022',
    sourcemap: false,
  },
  server: {
    // `npm run dev` proxies the API to a locally running instance.
    proxy: { '/api': 'http://127.0.0.1:8484', '/healthz': 'http://127.0.0.1:8484' },
  },
});
