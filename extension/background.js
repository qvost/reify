/* Session values travel directly to loopback and are never persisted here. */
const BRIDGE = "http://127.0.0.1:9099";
const PROTOCOL = "2026-07-28";

class BridgeError extends Error {
  constructor(message, status = 0) { super(message); this.status = status; }
}

async function request(path, { token, method = "GET", body } = {}) {
  let response;
  try {
    response = await fetch(`${BRIDGE}${path}`, {
      method,
      headers: {
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...(body ? { "Content-Type": "application/json" } : {}),
      },
      ...(body ? { body: JSON.stringify(body) } : {}),
      credentials: "omit", cache: "no-store", redirect: "error",
      signal: AbortSignal.timeout(3000),
    });
  } catch {
    throw new BridgeError("Cannot reach the local bridge. Run ‘reify bridge’ and try again.");
  }
  if (response.status === 401) throw new BridgeError("Bridge token was rejected. Connect with the token used by ‘reify bridge’.", 401);
  if (!response.ok) throw new BridgeError("The bridge could not complete this request. Check that Reify is up to date.", response.status);
  return response.status === 204 ? null : response.json();
}

async function currentTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  try {
    const url = new URL(tab?.url);
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || tab.incognito) return null;
    return { url: url.href, origin: url.origin, domain: url.host };
  } catch { return null; }
}

async function saveSessions(sessions) {
  const live = sessions.filter(session => {
    try {
      const url = new URL(session.origin);
      return ["http:", "https:"].includes(url.protocol) && url.origin === session.origin &&
        Number.isFinite(Date.parse(session.expires_at)) && Date.parse(session.expires_at) > Date.now();
    } catch { return false; }
  }).map(({ origin, expires_at }) => ({ origin, expires_at }));
  await chrome.storage.local.set({ bridgedOrigins: live });
  return live;
}

async function status() {
  const [tab, saved, credentials] = await Promise.all([
    currentTab(), chrome.storage.local.get("bridgedOrigins"), chrome.storage.session.get("bridgeToken"),
  ]);
  let sessions = await saveSessions(saved.bridgedOrigins || []);
  let online = false;
  let paired = false;
  let message = "";
  try {
    const health = await request("/health");
    online = health.status === "ok" && health.protocol_version === PROTOCOL;
    if (!online) message = "Update Reify to use this extension’s bridge protocol.";
  } catch { /* Offline is a normal popup state. */ }
  if (online && credentials.bridgeToken) {
    try {
      const remote = await request("/sessions", { token: credentials.bridgeToken });
      sessions = await saveSessions(remote.sessions);
      paired = true;
    } catch (error) {
      message = error.message;
      if (error.status === 401) await chrome.storage.session.remove("bridgeToken");
    }
  }
  return { online, paired, tab, sessions, message };
}

function cookieHeader(cookies) {
  const token = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const octets = /^[\x21\x23-\x2B\x2D-\x3A\x3C-\x5B\x5D-\x7E]*$/;
  return [...cookies].sort((a, b) => b.path.length - a.path.length).map(cookie => {
    const value = cookie.value.startsWith('"') && cookie.value.endsWith('"')
      ? cookie.value.slice(1, -1) : cookie.value;
    if (!token.test(cookie.name) || !octets.test(value)) throw new BridgeError("This site uses a cookie format the bridge cannot share.");
    return `${cookie.name}=${cookie.value}`;
  }).join("; ");
}

async function handle(message) {
  if (message.type === "STATUS") return status();
  if (message.type === "PAIR") {
    const token = typeof message.token === "string" ? message.token.trim() : "";
    if (!/^[A-Za-z0-9._~-]{16,}$/.test(token)) throw new BridgeError("Enter the bridge token (at least 16 characters).");
    const remote = await request("/sessions", { token });
    await chrome.storage.session.set({ bridgeToken: token });
    await saveSessions(remote.sessions);
    return status();
  }
  const { bridgeToken: token } = await chrome.storage.session.get("bridgeToken");
  if (!token) throw new BridgeError("Connect your local bridge first.");
  if (message.type === "BRIDGE") {
    const tab = await currentTab();
    if (!tab || tab.origin !== message.origin) throw new BridgeError("The active tab changed. Refresh this popup before sharing.");
    const cookies = (await chrome.cookies.getAll({ url: tab.url }))
      .filter(cookie => cookie.expirationDate === undefined || cookie.expirationDate > Date.now() / 1000);
    if (!cookies.length) throw new BridgeError("This tab has no cookies to share. Sign in to the site first.");
    const ttl = Math.max(1, Math.min(3600, ...cookies.filter(cookie => cookie.expirationDate !== undefined)
      .map(cookie => Math.floor(cookie.expirationDate - Date.now() / 1000))));
    const session = await request("/session", { token, method: "POST", body: {
      origin: tab.origin, headers: { cookie: cookieHeader(cookies), "user-agent": navigator.userAgent }, ttl_seconds: ttl,
    } });
    const { bridgedOrigins = [] } = await chrome.storage.local.get("bridgedOrigins");
    await saveSessions([...bridgedOrigins.filter(item => item.origin !== tab.origin), session]);
    return status();
  }
  if (message.type === "REVOKE") {
    const url = new URL(message.origin);
    if (url.origin !== message.origin || !["http:", "https:"].includes(url.protocol)) throw new BridgeError("Invalid shared origin.");
    await request("/session", { token, method: "DELETE", body: { origin: url.origin } });
    const { bridgedOrigins = [] } = await chrome.storage.local.get("bridgedOrigins");
    await saveSessions(bridgedOrigins.filter(item => item.origin !== url.origin));
    return status();
  }
  throw new BridgeError("Unknown extension request.");
}

chrome.storage.local.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" }).catch(() => {});
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id || sender.tab || sender.url !== chrome.runtime.getURL("popup.html")) return false;
  handle(message).then(data => respond({ ok: true, data }), error => respond({
    ok: false, error: error instanceof BridgeError ? error.message : "Could not share this session. Reopen the popup and try again.",
  }));
  return true;
});
