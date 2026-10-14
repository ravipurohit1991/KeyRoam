/*
 * KeyRoam extension: choosing the frame to fill.
 *
 * Frames come from webNavigation.getAllFrames (URLs reported by the browser,
 * not by the page), each annotated with `hasLogin` from the content script.
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.KeyRoamFrames = api;
})(typeof self !== 'undefined' ? self : globalThis, function () {
  'use strict';

  function isWeb(url) {
    return /^https?:\/\//i.test(String(url || ''));
  }

  /**
   * The frame holding the login form: the top frame when it has one,
   * otherwise the first embedded frame that has one, otherwise the top frame.
   * Returns `{ frameId, url, hasLogin }`.
   */
  function chooseTarget(frames, topUrl) {
    const web = (frames || []).filter((f) => isWeb(f.url));
    const top = web.find((f) => f.frameId === 0);
    if (top && top.hasLogin) return { frameId: 0, url: top.url, hasLogin: true };
    const sub = web.filter((f) => f.frameId !== 0 && f.hasLogin).sort((a, b) => a.frameId - b.frameId)[0];
    if (sub) return { frameId: sub.frameId, url: sub.url, hasLogin: true };
    return { frameId: 0, url: top ? top.url : topUrl, hasLogin: false };
  }

  return { chooseTarget };
});
