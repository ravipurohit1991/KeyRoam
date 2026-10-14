// Manifest V3 for Chromium-based browsers and Firefox, generated from one
// definition. `node build.mjs` writes them into dist/<target>/manifest.json.

// Public key that pins the Chromium extension id to
// ghgndmpoakmlmcagombpmikjdalnmiec (see crates/keyroam-autofill/src/install.rs).
export const CHROMIUM_KEY =
  'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAvcCYICRFQveU8gqU1WUqqbV2+QWhGhN1sxzlYPC8RceSJMcc++wGWuW2MC3gFe0u7VkRP8fk0vA69zIPIleTWjIK5Tfspn6xNxQLERaY0HO6mirIZqrNJcyDKKpWJykFFUXxwv5F2UOnr7o1G1EzARFmlCgMgnU+qZHpkSGgLC6yzeSf+5H8Bn8cMMT5h1EM22m+Z5vuI9JJSORVf6iPsTOmfhRA/kzlZDOeH/TOskaE1g/DVaIpVMdSoXpx4V5080a+NroQZ/iDljDRMMnO4d2aPldnCmss/K++TAxaT8aGRzKAe/koT6EmJGd1SXM3t0uLl1GwR7F+zRTfrm4F/QIDAQAB';

export const FIREFOX_ID = 'keyroam@ravipurohit1991.github.io';

export const TARGETS = ['chromium', 'firefox'];

const BACKGROUND_LIBS = ['lib/protocol.js', 'lib/frames.js', 'lib/prompts.js'];

export function manifestFor(target, version) {
  if (!TARGETS.includes(target)) throw new Error(`unknown target ${target}`);
  const icons = { 16: 'icons/icon-16.png', 32: 'icons/icon-32.png', 48: 'icons/icon-48.png', 128: 'icons/icon-128.png' };
  const manifest = {
    manifest_version: 3,
    name: 'KeyRoam',
    version,
    description: 'Fill and save logins with the KeyRoam desktop app. Your vault stays encrypted in the app.',
    icons,
    action: { default_title: 'KeyRoam', default_popup: 'popup.html', default_icon: icons },
    permissions: ['nativeMessaging', 'activeTab', 'webNavigation', 'storage'],
    host_permissions: ['http://*/*', 'https://*/*'],
    content_scripts: [
      {
        matches: ['http://*/*', 'https://*/*'],
        js: ['lib/forms.js', 'content.js'],
        all_frames: true,
        run_at: 'document_idle',
      },
    ],
  };
  if (target === 'chromium') {
    manifest.key = CHROMIUM_KEY;
    manifest.minimum_chrome_version = '116';
    manifest.background = { service_worker: 'background.js' };
  } else {
    manifest.background = { scripts: [...BACKGROUND_LIBS, 'background.js'] };
    manifest.browser_specific_settings = {
      gecko: {
        id: FIREFOX_ID,
        strict_min_version: '128.0',
        data_collection_permissions: { required: ['none'] },
      },
    };
  }
  return manifest;
}
