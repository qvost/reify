const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ROOT = path.join(__dirname, "..");
const TOKEN = "development-fixture-token-123456";

function harness() {
  const state = {
    online: true, local: {}, session: {}, remote: new Map(), requests: [], cookieReads: 0,
    tab: { url: "https://github.com/private/repos?view=all", incognito: false },
    cookies: [
      { name: "user_session", value: "fixture-session", path: "/" },
      { name: "__Host-user", value: "fixture-user", path: "/" },
    ],
  };
  function storage(name) {
    return {
      async get(key) { return { [key]: structuredClone(state[name][key]) }; },
      async set(values) { Object.assign(state[name], structuredClone(values)); },
      async remove(key) { delete state[name][key]; },
      async setAccessLevel() {},
    };
  }
  const chrome = {
    tabs: { async query() { return [state.tab]; } },
    cookies: { async getAll(filter) { state.cookieReads++; state.cookieFilter = filter; return state.cookies; } },
    storage: { local: storage("local"), session: storage("session") },
    runtime: { id: "abcdefghijklmnopabcdefghijklmnop", getURL(file) { return `chrome-extension://${this.id}/${file}`; },
      onMessage: { addListener(listener) { state.listener = listener; } } },
  };
  const context = vm.createContext({ chrome, URL, AbortSignal, navigator: { userAgent: "Mozilla/5.0 ReifyExtensionFixture" },
    fetch: async (url, options) => {
      state.requests.push({ url, ...options });
      assert.ok(url.startsWith("http://127.0.0.1:9099/"));
      assert.equal(options.credentials, "omit");
      assert.equal(options.redirect, "error");
      if (!state.online) throw new Error("offline");
      const route = new URL(url).pathname;
      if (route === "/health") return Response.json({ status: "ok", protocol_version: "2026-07-28" });
      if (options.headers.Authorization !== `Bearer ${TOKEN}`) return Response.json({}, { status: 401 });
      if (route === "/sessions") return Response.json({ sessions: [...state.remote.values()] });
      const body = JSON.parse(options.body);
      if (options.method === "DELETE") { state.remote.delete(body.origin); return new Response(null, { status: 204 }); }
      state.payload = body;
      const shared = { origin: body.origin, expires_at: new Date(Date.now() + body.ttl_seconds * 1000).toISOString() };
      state.remote.set(body.origin, shared);
      return Response.json(shared);
    },
  });
  vm.runInContext(fs.readFileSync(path.join(ROOT, "background.js"), "utf8"), context);
  state.send = (message) => vm.runInContext("handle", context)(message);
  state.formatCookies = (cookies) => vm.runInContext("cookieHeader", context)(cookies);
  return state;
}

test("health checks never read cookies and offline is a normal state", async () => {
  const h = harness(); h.online = false;
  const state = await h.send({ type: "STATUS" });
  assert.equal(state.online, false);
  assert.equal(h.cookieReads, 0);
  assert.equal(h.requests.length, 1);
});

test("pairing keeps the token in session storage and sends the specified payload only on a click", async () => {
  const h = harness();
  await h.send({ type: "PAIR", token: TOKEN });
  assert.equal(h.cookieReads, 0);
  assert.equal(h.session.bridgeToken, TOKEN);
  assert.ok(!JSON.stringify(h.local).includes(TOKEN));
  const status = await h.send({ type: "BRIDGE", origin: "https://github.com" });
  assert.equal(status.sessions.length, 1);
  assert.equal(h.cookieReads, 1);
  assert.equal(h.cookieFilter.url, h.tab.url);
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, "fixtures/session.json"), "utf8"));
  assert.deepEqual(h.payload, fixture);
  const local = JSON.stringify(h.local);
  assert.ok(!local.includes("fixture-session"));
  assert.ok(!local.includes("fixture-user"));
  assert.deepEqual(Object.keys(h.session), ["bridgeToken"]);
});

test("cookie order retains duplicate names and rejects header injection", () => {
  const h = harness();
  assert.equal(h.formatCookies([
    { name: "sid", value: "root", path: "/" }, { name: "sid", value: "scoped", path: "/private" },
  ]), "sid=scoped; sid=root");
  assert.equal(h.formatCookies([{ name: "quoted", value: '"value"', path: "/" }]), 'quoted="value"');
  assert.throws(() => h.formatCookies([{ name: "sid", value: "bad\r\nInjected: true", path: "/" }]));
  assert.throws(() => h.formatCookies([{ name: "bad;name", value: "x", path: "/" }]));
});

test("sharing expires no later than the first cookie and excludes expired cookies", async () => {
  const h = harness(); await h.send({ type: "PAIR", token: TOKEN });
  h.cookies[0].expirationDate = Date.now() / 1000 + 600;
  h.cookies.push({ name: "expired", value: "never-share", path: "/", expirationDate: 1 });
  await h.send({ type: "BRIDGE", origin: "https://github.com" });
  assert.ok(h.payload.ttl_seconds <= 600 && h.payload.ttl_seconds >= 590);
  assert.ok(!h.payload.headers.cookie.includes("never-share"));
});

test("revocation reaches the backend before clearing local metadata", async () => {
  const h = harness(); await h.send({ type: "PAIR", token: TOKEN });
  await h.send({ type: "BRIDGE", origin: "https://github.com" });
  h.online = false;
  await assert.rejects(h.send({ type: "REVOKE", origin: "https://github.com" }));
  assert.equal(h.local.bridgedOrigins.length, 1);
  h.online = true;
  const status = await h.send({ type: "REVOKE", origin: "https://github.com" });
  assert.equal(status.sessions.length, 0);
  assert.equal(h.remote.size, 0);
  assert.equal(h.local.bridgedOrigins.length, 0);
});

test("restarting the bridge clears stale origin metadata and rejects an old token", async () => {
  const h = harness(); await h.send({ type: "PAIR", token: TOKEN });
  await h.send({ type: "BRIDGE", origin: "https://github.com" });
  h.remote.clear();
  assert.equal((await h.send({ type: "STATUS" })).sessions.length, 0);
  h.session.bridgeToken = "old-token-no-longer-valid";
  assert.equal((await h.send({ type: "STATUS" })).paired, false);
  assert.equal(h.session.bridgeToken, undefined);
});

test("changed tabs, browser pages, incognito, missing tokens, and empty cookies cannot share", async () => {
  const h = harness();
  await assert.rejects(h.send({ type: "BRIDGE", origin: "https://github.com" }));
  await h.send({ type: "PAIR", token: TOKEN });
  await assert.rejects(h.send({ type: "BRIDGE", origin: "https://other.example" }));
  h.tab = { url: "chrome://extensions" };
  await assert.rejects(h.send({ type: "BRIDGE", origin: "https://github.com" }));
  h.tab = { url: "https://github.com", incognito: true };
  await assert.rejects(h.send({ type: "BRIDGE", origin: "https://github.com" }));
  assert.equal(h.cookieReads, 0);
  h.tab = { url: "https://github.com" }; h.cookies = [];
  await assert.rejects(h.send({ type: "BRIDGE", origin: "https://github.com" }));
  assert.ok(!h.requests.some(request => request.method === "POST"));
});

test("only the extension popup can send session messages", () => {
  const h = harness();
  const callback = () => assert.fail("untrusted sender received a response");
  assert.equal(h.listener({ type: "PAIR", token: TOKEN }, { id: "other", url: "https://evil.test" }, callback), false);
  assert.equal(h.listener({ type: "PAIR", token: TOKEN }, {
    id: "abcdefghijklmnopabcdefghijklmnop", url: "https://evil.test", tab: { id: 1 },
  }, callback), false);
  assert.equal(h.requests.length, 0);
});

test("manifest assets exist and Chrome icons have the declared raster dimensions", () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(ROOT, "manifest.json"), "utf8"));
  assert.equal(manifest.manifest_version, 3);
  assert.deepEqual(manifest.permissions, ["cookies", "activeTab", "storage"]);
  for (const [size, file] of Object.entries(manifest.icons)) {
    const png = fs.readFileSync(path.join(ROOT, file));
    assert.equal(png.readUInt32BE(16), Number(size));
    assert.equal(png.readUInt32BE(20), Number(size));
  }
  for (const file of [manifest.background.service_worker, manifest.action.default_popup, "popup.js", "popup.css", "icons/emblem.svg"]) {
    assert.ok(fs.existsSync(path.join(ROOT, file)));
  }
});
