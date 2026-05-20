import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(resolve(here, '../../../Cargo.toml'), 'utf8');
const match = cargoToml.match(/^\s*version\s*=\s*"([^"]+)"/m);

if (!match) {
  throw new Error('Could not find version in Cargo.toml');
}

export const version = match[1];
