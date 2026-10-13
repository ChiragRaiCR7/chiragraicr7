// @ts-check
import { defineConfig } from 'astro/config';

// BASE_PATH lets the same build run at the domain root (Rust server) or under
// /chiragraicr7/ (GitHub Pages project site).
export default defineConfig({
  site: 'https://chiragraicr7.github.io',
  base: process.env.BASE_PATH ?? '/',
  trailingSlash: 'ignore',
  build: {
    // External, hashed CSS/JS only: the server's CSP allows no inline code.
    inlineStylesheets: 'never',
  },
  // Astro inlines scripts below this size; 0 keeps them as external files.
  vite: { build: { assetsInlineLimit: 0 } },
});
