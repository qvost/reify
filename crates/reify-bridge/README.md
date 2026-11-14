# reify-bridge

A loopback development HTTP proxy using explicitly configured target credentials.
`reify bridge` runs it at `127.0.0.1:9099`; `--port` changes the loopback port.
Embedders can construct `BridgeServer::new()` and retrieve its generated local
bearer token through `token()`, or use `with_transport` with `HttpTransport` and an
explicit token. `store()` exposes the thread-safe `SessionStore` configuration API.
`serve_with_shutdown` and `serve_listener` support graceful caller-driven shutdown.
Non-loopback listeners are rejected, including pre-bound listeners.

## API

GET `/health` returns public status and protocol version, without credentials:

```json
{"status":"ok","protocol_version":"2026-07-28"}
```

POST `/forward` requires `Authorization: Bearer <local bridge token>`. A present
browser Origin is denied on all routes; no permissive CORS layer is installed.
The local bridge authorization header is not forwarded to the upstream target.

```sh
curl http://127.0.0.1:9099/forward \
  -H "Authorization: Bearer $REIFY_BRIDGE_TOKEN" \
  -H 'Content-Type: application/json' \
  --data '{"target_url":"https://api.example.com/items","method":"POST","headers":{"X-Developer-Request":"example"},"body":{"enabled":true}}'
```

The typed `ForwardRequest` includes `target_url: Url`, `method`, a string header
map, and an optional JSON body. Omitted body means no body; explicit `null` sends
JSON null. URL credentials and non-HTTP(S) schemes are rejected. CONNECT/TRACE,
hop-by-hop/Host/length overrides, malformed header names/values, and duplicate
case-insensitive names are rejected before the transport is called. A JSON request
body is serialized by Reqwest; this API is not a raw binary-body tunnel.

Successful dispatch returns HTTP 200 containing a typed `ForwardResponse`, even
when the upstream status is a failure:

```json
{
  "status": 418,
  "headers": {"content-type":["application/json"],"set-cookie":["a=1","b=2"]},
  "body": {"type":"json","value":{"message":"upstream status preserved"}}
}
```

Response body variants are `json`, UTF-8 `text`, or binary `base64`. Repeated
response headers retain all values. Non-ASCII and sentinel-shaped literal header
values use `=?base64?...?=` encoding; decode one layer to recover exact bytes.
Credentials do not appear in `/health`, error messages, or Debug implementations
of the request, server, or session store. Upstream response bodies/headers remain
visible to the authorized developer, including upstream Set-Cookie values.

The production pooled client supports Rustls HTTPS, disables inherited system
proxies and automatic redirects, and has a 5s connect / 20s total timeout. It reads
responses incrementally with a 4 MiB limit. At most 32 forwards execute at once;
overload returns HTTP 503. The handler also enforces a 25s outer transport timeout
and a 1 MiB inbound request limit. Transport failures return HTTP 502 with a
sanitized JSON error. Requests are never automatically retried.

## Session-header storage

`SessionStore::insert(domain, auth).await` validates and installs an `AuthContext`.
Keys are normalized bare hostnames, including IDNA and canonical IP addresses;
trailing DNS dots/case are normalized. Exact host matching deliberately excludes
implicit subdomain or sibling-domain credential sharing. Ports are not part of a
domain key. `get` hides expired entries; `remove` deletes a configuration.

Stored cookies, Authorization, API-key custom headers, and CSRF tokens are encoded
using core's shared `AuthContext::to_headers`. Header values are marked sensitive
on real requests; reserved credential transport/body headers and invalid Cookie
octets are rejected. Explicit headers in a forward request override configured
headers by case-insensitive name. The domain store is independent of the MCP
server's compiled-app cache and contains only developer-supplied state.

This component does not read local browser profiles, extract login sessions, or
promise a WAF bypass. Requests originate through the local network's normal HTTP
client; a target still applies its own authorization and security policies.

## Tests

Five tests exercise health/version reporting, typed forward/header/status/body
preservation, repeated Set-Cookie values, exact-domain store roundtrips and expiry,
origin/auth/header rejection, loopback binding restrictions, and explicit null
body handling. The forwarding fixture dispatches a real in-process Axum request
through an injectable transport so it works without opening a socket. Production
`HttpTransport` uses Reqwest sockets and requires normal network permissions.

```sh
cargo test -p reify-bridge
```
