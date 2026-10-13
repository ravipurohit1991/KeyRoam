// The WebExtension API object: `browser` in Firefox, `chrome` in Chromium.
// Both return promises from their MV3 APIs.
export const api = globalThis.browser ?? globalThis.chrome;
