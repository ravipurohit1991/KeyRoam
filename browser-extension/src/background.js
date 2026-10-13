// KeyRoam background script (service worker in Chromium, event page in Firefox).
import { api } from './lib/browser.js';
import { Background, HOST_NAME } from './lib/background-core.js';
import { NativeClient } from './lib/native.js';

const client = new NativeClient(() => api.runtime.connectNative(HOST_NAME), {
  lastError: () => api.runtime.lastError?.message,
});
const background = new Background({ api, client });

api.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  background.onMessage(msg, sender).then(sendResponse, (e) =>
    sendResponse({ error: { code: 'failed', message: String(e?.message ?? e) } }),
  );
  return true;
});

api.tabs.onUpdated.addListener((tabId, changeInfo) => {
  background.onTabUpdated(tabId, changeInfo);
});

api.tabs.onRemoved.addListener((tabId) => {
  background.onTabRemoved(tabId);
});

api.commands.onCommand.addListener((command) => {
  if (command === 'fill-login') background.fillShortcut();
});

// Pending save prompts (which hold a password until answered) live in
// memory-only session storage; keep them away from content scripts.
api.storage.session.setAccessLevel?.({ accessLevel: 'TRUSTED_CONTEXTS' });

// Reachable from the service-worker console and the end-to-end test.
globalThis.keyroamBackground = background;
