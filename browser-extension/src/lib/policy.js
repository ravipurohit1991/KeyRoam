// Pure helpers shared by the background script and its tests. The KeyRoam
// app enforces every rule again; these exist to ask the user before a request
// would be refused and to explain why.

export function parseUrl(url) {
  try {
    return new URL(url);
  } catch {
    return null;
  }
}

/** True for http and https pages, the only ones KeyRoam fills. */
export function isWebUrl(url) {
  const u = parseUrl(url);
  return u !== null && (u.protocol === 'https:' || u.protocol === 'http:');
}

export function isSecureUrl(url) {
  return parseUrl(url)?.protocol === 'https:';
}

export function originOf(url) {
  return parseUrl(url)?.origin ?? null;
}

/** The `page` object of the KeyRoam protocol. */
export function pageInfo(frameUrl, topUrl, consent = {}) {
  const page = { url: frameUrl, top_url: topUrl };
  if (consent.allowHttp) page.allow_http = true;
  if (consent.allowCrossSite) page.allow_cross_site = true;
  return page;
}

/**
 * Pick the frame to fill: the top frame if it has a login form, otherwise the
 * first frame that reported one, otherwise the top frame.
 * @param {Map<number, {url: string, kind: string}> | undefined} frames
 */
export function chooseFrame(frames) {
  if (frames) {
    const top = frames.get(0);
    if (top && top.kind !== 'none') return { frameId: 0, ...top };
    for (const [frameId, f] of [...frames].sort((a, b) => a[0] - b[0])) {
      if (f.kind !== 'none') return { frameId, ...f };
    }
    if (top) return { frameId: 0, ...top };
  }
  return { frameId: 0, url: null, kind: 'none' };
}

/** Host shown to the user, without "www.". */
export function displayHost(url) {
  const host = parseUrl(url)?.hostname ?? '';
  return host.startsWith('www.') ? host.slice(4) : host;
}

const MESSAGES = {
  host_not_found:
    'The KeyRoam desktop app is not registered with this browser. In KeyRoam, open Browser integration and click “Register with browsers”, then restart the browser.',
  host_exited: 'The KeyRoam helper stopped unexpectedly. Try again.',
  app_not_running: 'Open the KeyRoam app and turn on Browser integration.',
  locked: 'Unlock KeyRoam to fill passwords.',
  timeout: 'KeyRoam did not answer. Is it busy syncing? Try again.',
  insecure_page: 'This page is not secure (http). Anyone on the network could read a password filled here.',
  cross_site_frame: 'The login form is inside an embedded frame from another website.',
  unsupported_page: 'KeyRoam fills logins on websites only.',
  not_found: 'That login is not saved for this website.',
  no_form: 'KeyRoam could not find a login form on this page.',
  no_site_access: 'KeyRoam needs access to websites to find their login forms. It reads forms only; logins are filled only when you choose one.',
  reload_page: 'Access allowed. Reload this page, then open KeyRoam again.',
};

export function describeError(error) {
  if (!error) return '';
  return MESSAGES[error.code] ?? error.message ?? 'Something went wrong.';
}
