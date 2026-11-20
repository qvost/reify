# Shaligram Reify — Agent Session Bridge

Share the cookies available to your current tab with Reify on your computer.
The popup uses Shaligram Labs' bone/carbon palette and an original spiral emblem.
No bundler, dependencies, content scripts, or remote assets are required.

## Load unpacked

1. Build Reify: `cargo build --release -p reify-cli`.
2. Open `chrome://extensions` in Chrome 120 or newer and enable **Developer mode**.
3. Choose **Load unpacked** and select this repository's `extension/` directory.
4. Pin **Shaligram Reify** to your toolbar.

## Connect and share

Use one token for the bridge and the MCP gateway. For example, in one terminal:

```sh
export REIFY_BRIDGE_TOKEN="$(openssl rand -hex 24)"
reify bridge
```

In another terminal, set `REIFY_BRIDGE_TOKEN` to the same value, then start the gateway:

```sh
reify install
reify serve
```

The bridge listens on `127.0.0.1:9099`; the MCP gateway listens on
`127.0.0.1:8080/mcp`. Both processes are needed. `reify serve` does not start the
bridge automatically. Custom gateway ports are supported; this companion uses
the bridge's default port 9099.

Open the extension popup, enter your bridge token under **Bridge connection**,
and select **Connect**. The token stays in `chrome.storage.session` until Chrome
exits or the extension reloads. The popup never displays it again.

Sign in to a website, open the popup, check the displayed domain, and click
**Bridge Current Tab**. Cookie extraction happens only on that click. Use the
Reify MCP meta-tool from your agent to compile that website, or run:

```sh
reify probe https://github.com 'search repositories'
```

When `REIFY_BRIDGE_TOKEN` is set, probing reads the current origin's session and
seeds an isolated Chromium profile before navigation. Tool execution reads live
credentials again on every request. Revoke a session in the popup to prevent
subsequent tool calls from reusing its cached cookies. Requests already sent
cannot be canceled by revocation. Start a new compilation after sharing a session
if an earlier public compilation is already cached.

## Session behavior

- Sharing lasts at most one hour, shortened to the earliest cookie expiration.
- **Revoke** deletes the backend session, then its local metadata. Failed or
  offline revocations remain visible so you can retry.
- The bridge and gateway keep session credentials in memory. Restarting the
  bridge clears its shared sessions; the popup reconciles its origin list when
  it reconnects. Cached gateway credentials cannot bypass a live revocation.
- Local extension storage contains only origins and expiration times. Cookies
  are sent directly to loopback, never written to extension storage or logs.
- `reify probe` prints raw captured request/response headers and bodies as its
  diagnostic output. Authenticated captures can contain private site data.
- Backend lookup matches the exact origin, including scheme and port. It does
  not authorize sibling hosts. Isolated Chromium uses ordinary host-scoped
  browser cookies; browser cookie rules do not distinguish ports.
- Extraction includes applicable unpartitioned cookies, including HttpOnly
  cookies. It does not copy localStorage, IndexedDB, partitioned cookies, OAuth
  access tokens, or page-specific CSRF fields. Sites that depend on those may
  need additional authentication support.
- The wire format carries a Cookie header without path metadata. If duplicate
  names exist, forwarding preserves their order; Chromium seeding uses the first
  value for each name at `/`. Incognito and browser-internal tabs are excluded.
- Reify does not recreate browser TLS fingerprints. A site can still reject an
  authenticated HTTP client or require another interactive challenge.

## Loopback API

| Endpoint | Purpose |
| --- | --- |
| `GET /health` | Public health and protocol version; no credentials |
| `POST /session` | Store `{ origin, headers, ttl_seconds }` |
| `GET /sessions` | List live origins and expiration times only |
| `DELETE /session` | Revoke `{ origin }`; safe to repeat |
| `GET /session/auth?origin=…` | Native gateway credential lookup; extension origins are rejected |
| `POST /forward` | Forward an authenticated tool request with matching session headers |

All endpoints except health require `Authorization: Bearer <REIFY_BRIDGE_TOKEN>`.
Normal web page origins are rejected. Extension CORS is limited to health and
session management, and requires a valid Chrome extension origin. Credential
reads and forwarding remain native-client operations.

The example payload is in [tests/fixtures/session.json](tests/fixtures/session.json).
The Rust integration tests consume that same fixture.

## Verify

```sh
node tests/background.test.cjs
node --check background.js
node --check popup.js
cargo test -p reify-bridge --test extension_bridge # from the repository root
```

The Node suite uses mocked Chrome APIs and loopback responses to exercise cookie
sharing, token pairing, TTL, revocation, offline behavior, and message permissions.
The Rust suite exercises real Axum handlers in process without TCP sockets.
Use **Inspect views → service worker** in `chrome://extensions` for Chrome's
extension lifecycle diagnostics. Regenerate the icons with
`python3 icons/generate.py`.
