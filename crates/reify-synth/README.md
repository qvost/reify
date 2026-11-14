# reify-synth

A synchronous, stateless compiler from captured exchanges to typed `CompiledTool`s.
It makes no HTTP requests, executes no browser operations, and copies no captured
payloads during clustering. The only additional dependency beyond the requested
set is the workspace JSON Schema validator: it checks inferred schemas, validates
format annotations, and validates invocation arguments in the reference renderer.
Remote schema resolution is disabled by its workspace dependency configuration.

```rust
use reify_core::domain::{AuthContext, CapturedFrame, Target};
use reify_synth::SynthEngine;

fn compile(target: &Target, frames: &[CapturedFrame], auth: &AuthContext)
    -> Result<Vec<reify_core::domain::CompiledTool>, reify_core::ReifyError>
{
    Ok(SynthEngine::new().synthesize(target, frames, auth)?.tools)
}
```

## Clustering

The engine restricts captures to the target host and its subdomains using label
boundaries and HTTP(S) URLs. It accepts JSON or GraphQL exchanges. HTTP method
case, scheme, host, nondefault port, static path bytes, and trailing slashes remain
significant. Query strings and fragments are excluded from endpoint identity.

Path segments consisting of signed decimal integers, UUIDs, or hexadecimal hashes
of at least 16 characters become `{resource_id}`. Valid ISO calendar dates become
`{resource_date}`. Resource names derive from the preceding static segment with
simple English singularization: `/api/users/123/profile` becomes
`/api/users/{user_id}/profile`. Repeated names receive `_2`, `_3`, etc. Version
labels like `v1` stay static. Detection deliberately does not guess arbitrary slugs,
decode percent-encoded segments, or infer that short hexadecimal words are IDs.
A static route that looks exactly like an ID/date is indistinguishable from a
parameter using these observations; this is an explicit induction heuristic.

Groups borrow frames and sort observations by URL, request body, status, and
response body. Query keys are decoded into ordered maps; single occurrences stay
strings, repeated occurrences become ordered arrays of strings. `001` is preserved
as text. Required query keys appear in every exchange of the group; missing keys
are optional. An empty value is present, distinct from a missing key.

GraphQL operationName, document, and persisted-query information refine grouping
so distinct recognizable operations sharing `/graphql` do not collapse. Exact
document text participates in identity; whitespace normalization, GraphQL AST
parsing, and batched operation decomposition are not implemented. Batch bodies
still receive structurally valid request schemas as one observed endpoint.

## Schema algebra

The induction result is an empirical structural generalization, not a proof of
what the upstream server accepts. It accepts every observation, with no examples,
defaults, or enums copied from sensitive payloads. Object keys and required arrays
are sorted. Null is a type, distinct from absence. Object fields are required iff
present in every observed object of that structural branch; other properties are
forbidden. Nested objects apply the same rule independently.

Numbers join to `integer` when all observed values are mathematically integral,
otherwise `number`. Arrays join all observed element types. Empty arrays add no
element constraints; exclusively empty arrays use `items: {}`. Different JSON
kinds form deterministic `anyOf` branches. All object observations join together
before alternatives are emitted, so optionality is based on field presence rather
than insertion order. No observations and recursion beyond 64 levels widen to
`{}`. This depth limit preserves acceptance while bounding schema recursion.

String formats (`date`, `date-time`, `uuid`, `uri`, `email`) require unanimous
recognition among all strings in the branch. Proposed annotations are checked
against JSON Schema's asserted-format validators to avoid differences between URL
or date parser permissiveness and format syntax. Mixed formatted/unformatted
strings widen to plain strings. Formats remain annotations for downstream
validators that do not enable format assertion.

Output is Draft 2020-12 JSON Schema. See the primary specification for
[array items](https://json-schema.org/understanding-json-schema/reference/array)
and [union semantics](https://json-schema.org/understanding-json-schema/reference/combining).

## Ranking and naming

Goal tokens split snake_case, camelCase, punctuation, and whitespace. Small,
explicit English rules singularize common plural nouns and equate find/lookup
with search, and booking/reserve with book. Relevance considers path segments,
query keys, JSON field names, and GraphQL identifiers. Comments and string literals
are removed from GraphQL documents before scoring; ordinary scalar values do not
contribute keywords. This is lexical matching, not semantic or multilingual NLP.

Each matching goal token contributes 100 points, each matching topical token 50
more, and any successful object/array JSON response contributes 30. Static
configuration, bootstrap, telemetry, heartbeat, health, analytics, metrics, and
ping path tokens receive a 1000-point penalty and are excluded. Goal relevance
requires a topical match when the goal has topical nouns; action-only goals can
match actions. Empty goals accept all non-noise candidates. More captures do not
inflate a group's score.

Select the five highest-ranked relevant endpoints, or all if fewer exist. Thus
captures with at least three relevant endpoints produce 3–5 tools; sparse captures
produce 1–2, and no relevant captures return a descriptive execution error. No tool
is fabricated to meet a count. Score ties use canonical method/origin/path/operation
identity. The highest-ranked tool supplies `primary_tool_name`.

Names use observed action/resource words, e.g. `search_flights`, and default HTTP
verbs where needed. Named GraphQL operations use their snake_case names. Names
are ASCII, at most 96 characters before deterministic collision suffixes. Global
name collisions among selected endpoints get `_2`, `_3`, etc. Descriptions state
the operation, method/path, and observed response shapes/field names without
copying scalar response values. Provenance includes a credential-free URL template.

## Execution contract

`compiler::compile` emits an object input schema with three argument categories:

- Named path arguments, always required strings (IDs are opaque text).
- Flat query arguments, mapping sanitized tool names to original upstream keys.
  Collisions with path/body names or other query keys get deterministic `query_`
  prefixes and numeric suffixes. Presence statistics determine requiredness.
- `body`, the complete JSON request document, preserving nested objects, arrays,
  booleans, numbers, and null. Its structure joins observed bodies. It is required
  only when a body was captured on every request; otherwise omission sends no body.

A body plan is the whole-value JSON string `"{body}"`. This is an intentional,
implemented interpolation token, not an unfinished code placeholder. Executors
must substitute the **JSON value**, not stringify it or replace substrings inside
it. With no observed bodies, `body_template` is absent. Using a whole-document
argument avoids inventing unsupported optional-property omission rules in core's
`Option<Value>` template representation. Bodies and query values are not copied
into the plan as literals or defaults, including GraphQL query text: the caller
supplies the typed `body` at invocation.

`compiler::render` is the executable reference for this contract. It validates
arguments, rejects missing/unknown inputs and malformed types, encodes each path
argument as a single segment, and rejects empty or dot-segment path values.
Encoded percent signs cannot cause a second decoding or path traversal. Query
pairs are appended in sorted argument-name order with form URL encoding; repeated
values preserve array order. Optional arguments are omitted rather than replaced
with captured defaults. Explicit JSON null remains a real body.

Repeated executors can retain `compiler::PreparedTool` to compile schema validation
and query ordering once. Its `render` method borrows the method/body and allocates
the rendered URL without retaining arguments between calls. Rebuild the prepared
tool when its definition changes. The one-shot `compiler::render` API continues
to return an owned request.

Auth requirements contain sorted, deduplicated key names only: `cookie` when any
cookies exist, `authorization` when provided, and lowercase keys from CSRF/custom
header maps. Credentials never become input defaults or schema examples. These
are bundle requirements based on captured context, not proof that every endpoint
needs every key. The executor must resolve them from the per-call auth bundle and
apply its own credential expiry and authorization policy.

## Determinism and checks

Schemas, ranks, names, descriptions, template content, and mapping *values* do not
depend on frame order or HashMap iteration. Core requires HashMap fields in
ExecutionSpec, so direct JSON serialization of those maps may vary in key order;
serialize via `serde_json::to_value` for canonical sorted JSON objects when hashing
or snapshotting compiled tools. Wall-clock `synthesis_duration_ms` is intentionally
outside the deterministic content contract.

```sh
cargo test -p reify-synth
cargo clippy -p reify-synth --all-targets -- -D warnings
cargo fmt --all --check
```

Tests cover the four requested scenarios plus all JSON-kind pairwise joins with
asserted formats, nested optionality, input-order invariance, repeated query keys,
path/query/body name collisions, GraphQL operation separation/relevance, scoped
captures, sparse goals, method identity, credential-name extraction, and encoded
path/body/query rendering. Inference tests validate every observed sample against
the generated schema rather than only comparing expected schema fragments.
