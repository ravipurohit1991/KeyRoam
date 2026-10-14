/*
 * KeyRoam extension: pending "save login?" prompts.
 *
 * After a form submission the page usually navigates away, so the prompt is
 * kept per tab for a short time and shown again on the next page. The
 * password stays in the background script; the page only ever sees the
 * public view (site, username, action).
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.KeyRoamPrompts = api;
})(typeof self !== 'undefined' ? self : globalThis, function () {
  'use strict';

  const DEFAULT_TTL_MS = 2 * 60 * 1000;

  function randomId() {
    const bytes = new Uint8Array(12);
    globalThis.crypto.getRandomValues(bytes);
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  }

  class PromptStore {
    constructor(options) {
      const o = options || {};
      this.ttl = o.ttlMs || DEFAULT_TTL_MS;
      this.now = o.now || (() => Date.now());
      this.byTab = new Map();
    }

    /** Record a prompt for a tab, replacing any earlier one. */
    add(p) {
      const prompt = Object.assign({}, p, { id: randomId(), createdAt: this.now() });
      this.byTab.set(p.tabId, prompt);
      return prompt;
    }

    _live(p) {
      return p && this.now() - p.createdAt <= this.ttl;
    }

    forTab(tabId) {
      const p = this.byTab.get(tabId);
      if (this._live(p)) return p;
      this.byTab.delete(tabId);
      return null;
    }

    /** The prompt with `id`, if it belongs to `tabId` and has not expired. */
    get(tabId, id) {
      const p = this.forTab(tabId);
      return p && p.id === id ? p : null;
    }

    remove(tabId) {
      this.byTab.delete(tabId);
    }

    /** What the page may see: never the password. */
    static view(p) {
      return { id: p.id, action: p.action, site: p.site, username: p.username, title: p.title || null };
    }

    toJSON() {
      return Array.from(this.byTab.values()).filter((p) => this._live(p));
    }

    load(list) {
      this.byTab.clear();
      for (const p of list || []) if (this._live(p)) this.byTab.set(p.tabId, p);
    }
  }

  return { PromptStore, DEFAULT_TTL_MS };
});
