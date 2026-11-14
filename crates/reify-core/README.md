# reify-core

The library is independent of Tokio and has no network or filesystem side effects.
All public types and operations have Rustdoc. Extra dependencies beyond the
requested shared set provide UUID v4 identifiers, MCP Base64 encoding, and offline
JSON Schema meta-validation. The schema validator disables remote resolution.

## Ownership and validation

`McpHeaders<'a>` borrows ASCII slices from `http::HeaderMap`; parsing copies only
small enum/numeric fields. Missing optional values stay absent. Duplicate recognized
headers are rejected, including duplicates with identical values. Decimal TTLs
accept zero and leading zeros, reject signs, fractions, and overflow. Header names
are case-insensitive and values case-sensitive. `decoded_name()` returns a borrowed
`Cow` for plain names and owns decoded UTF-8 for sentinel-encoded names. The encoding
function escapes controls, Unicode, surrounding whitespace, and sentinel-shaped
literal strings. Parsed views cannot outlive their backing header map.

Requests, responses, captured JSON, and execution plans own their strings and JSON
values so they can move between independent tasks. They make no claims of zero-copy
JSON deserialization. Serialization uses borrowed field references and writes to
any Serde serializer without first creating an intermediate JSON value.
`extract_tool_call()` returns owned call parameters. Its arguments must be an
object; omission defaults to an empty object. Arbitrary method metadata remains
available on the original request. IDs preserve serde_json's integer/float number
representation and accept strings, rejecting null and booleans.

Envelope decoding and encoding enforce version `2.0`, structured request parameters,
and exactly one response outcome. `Some(Value::Null)` is a real successful result,
not an omitted result. Malformed envelopes, batches, and notifications do not decode
as requests. The request type intentionally requires an ID; a future notification
adapter should use a separate type. Unknown JSON fields are ignored for forward
compatibility; duplicate recognized fields fail decoding.

Tool schemas must be JSON objects and pass their draft's JSON Schema meta-schema.
Supported dialect identifiers are listed in `validate_schema`; omitted `$schema`
uses the validator's Draft 2020-12 default. Unsupported/custom dialects produce a
validation error instead of invoking infallible draft detection. Validation does
not fetch references or validate a particular tool-call instance. Public mutable
schema fields are revalidated during serialization.

## Protocol boundary

References: [2026-07-28 Streamable HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)
and [tool messages](https://modelcontextprotocol.io/specification/2026-07-28/server/tools).

`validate_request` checks protocol-version metadata and standard method/name/URI
mirrors. Its header mismatches belong to HTTP 400 / JSON-RPC `-32020` in an HTTP
adapter. Base64 names are decoded before comparison. Modern requests carry the
protocol revision in `params._meta["io.modelcontextprotocol/protocolVersion"]`.
The `is_initialize` helper only identifies legacy traffic; it creates no handshake
or session state.

Reify's `Mcp-Stream`, `Mcp-Cache-Scope`, `Mcp-Ttl-Ms`, and `Mcp-Session-Bridge` are
application extensions. Bridge IDs are companion routing hints, not protocol
sessions. `task`, `domain`, and `global` are Reify cache scopes, distinct from
standard MCP list-result scopes. Internal JSON content and provenance similarly
belong to Reify's storage representation. Use `ToolDefinition::to_mcp_value()` and
`ToolCallResult::to_mcp_value()` to publish portable MCP values: provenance and
cache hints move into namespaced `_meta`; JSON content becomes a text block; the
completed call result includes `resultType: "complete"`.

This crate supplies data structures, not an HTTP server. The server crate must
implement POST/Accept/Content-Type/Origin handling, authorization, per-response
server metadata, schema-specific mirrored parameter validation, RPC dispatch,
HTTP status mapping, SSE framing, and cancellation. Schema annotations such as
`x-mcp-header` require transport-specific checks beyond JSON Schema meta-validation.

## Canonicalization and cache identity

`Target::new` uses `url::Url` to canonicalize host case, IDNA, and default ports.
It accepts HTTP(S), removes fragments, empty queries, and trailing path slashes.
The root still serializes as `/`, as required by the URL representation. Nonempty
queries, percent-encoded path separators, and nondefault ports remain intact.
Goals are lowercased with Unicode whitespace trimmed and collapsed; blank goals
become absent. Deserializing a target repeats the same validation.

`CacheKey` hashes domain plus normalized goal, following the cache algorithm in
the task. Therefore `/api` and `/api/` share a key, as do other paths/queries on the
same domain. This deliberately resolves the task's conflicting request for distinct
trailing-slash keys in favor of its explicit normalization and domain-key rules.
Scheme and credentials are excluded; nondefault ports are retained. IPv6 uses the
URL library's bracketed host representation. This is a compilation artifact key;
private execution-result caching must additionally partition by authentication
and authorization context.

The stable v1 preimage is the literal bytes `reify:target:v1\0`, followed by the
UTF-8 domain byte length as little-endian u64, the domain bytes, the normalized
goal byte length as little-endian u64, and the goal bytes. Length prefixes prevent
tuple-boundary collisions. No goal and an empty normalized goal are equivalent.
The digest is exactly 32 inline bytes, with a 64-character lowercase hex display.

## Capture helpers and credentials

JSON detection recognizes case-insensitive Content-Type values, parameters, and
structured `+json` suffixes; a parsed response body is also evidence of JSON.
GraphQL detection is a heuristic: a `graphql` path segment, a string `query` field,
a persisted-query extension, a nonempty batch of such bodies, or GET query
parameters. It does not validate GraphQL syntax, and generic APIs with a `query`
field can be false positives.

`AuthContext` owns credentials for trusted bridge transport and intentionally has
no Debug implementation. Callers remain responsible for storage and log redaction.
`ExecutionSpec` is declarative: request interpolation and credential resolution
belong to the executor, with no simulated execution in the core library.
