// Builds the extension for each browser family into dist/:
//
//   dist/chromium/  Chrome, Edge, Brave, Vivaldi (service worker background)
//   dist/firefox/   Firefox 115+ (background scripts, gecko add-on ID)
//
// `node build.mjs` needs no dependencies. Load dist/chromium as an unpacked
// extension, or dist/firefox as a temporary add-on.

import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const FIREFOX_ID = 'keyroam@ravipurohit1991.github.io';

export function chromiumManifest(base) {
  return structuredClone(base);
}

export function firefoxManifest(base) {
  const m = structuredClone(base);
  delete m.key;
  delete m.minimum_chrome_version;
  m.background = { scripts: [base.background.service_worker], type: 'module' };
  m.browser_specific_settings = {
    gecko: {
      id: FIREFOX_ID,
      strict_min_version: '115.0',
      data_collection_permissions: { required: ['none'] },
    },
  };
  return m;
}

export function build(outDir = join(here, 'dist')) {
  const src = join(here, 'src');
  const base = JSON.parse(readFileSync(join(src, 'manifest.json'), 'utf8'));
  const targets = { chromium: chromiumManifest(base), firefox: firefoxManifest(base) };
  for (const [name, manifest] of Object.entries(targets)) {
    const dir = join(outDir, name);
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });
    cpSync(src, dir, { recursive: true });
    writeFileSync(join(dir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  }
  return targets;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  build();
  console.log('Built dist/chromium and dist/firefox');
}
