import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';
import sitemap from '@astrojs/sitemap';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(resolve(here, '../Cargo.toml'), 'utf8');
const versionMatch = cargoToml.match(/^\s*version\s*=\s*"([^"]+)"/m);
if (!versionMatch) {
  throw new Error('Could not find version in Cargo.toml');
}
const REKODY_VERSION = versionMatch[1];

export default defineConfig({
  site: 'https://rekody.com',
  output: 'static',
  integrations: [sitemap()],
  vite: {
    plugins: [tailwindcss()],
    define: {
      __REKODY_VERSION__: JSON.stringify(REKODY_VERSION),
    },
  },
});
