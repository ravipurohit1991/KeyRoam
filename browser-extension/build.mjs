// Builds the unpacked extension for each browser:
//   dist/chromium/  Chrome, Edge, Brave, Chromium (load unpacked, or zip it)
//   dist/firefox/   Firefox (load as a temporary add-on, or zip it)
// Usage: node build.mjs [chromium|firefox]...

import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { TARGETS, manifestFor } from './manifest.mjs';

const root = dirname(fileURLToPath(import.meta.url));
const { version } = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
const targets = process.argv.slice(2).length ? process.argv.slice(2) : TARGETS;

for (const target of targets) {
  const out = join(root, 'dist', target);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  cpSync(join(root, 'src'), out, { recursive: true });
  writeFileSync(join(out, 'manifest.json'), `${JSON.stringify(manifestFor(target, version), null, 2)}\n`);
  console.log(`built ${target} ${version} -> ${out}`);
}
