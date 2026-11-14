# Reify

**Turn any live website into a typed, stateless MCP server in seconds.**

No pre-existing OpenAPI specs. No manual MCP server code. No slow browser screenshot loops.

```
Agent ("Check flights on airline.com")
   │
   ▼
[1. Edge Cache Lookup] ──► Hit (<10ms) ──► Stateless MCP Endpoint
   │ (Miss)
   ▼
[2. Headless Sniffer (Probe)] ──► Strips 99% static noise, captures real XHR/GraphQL frames
   │
   ▼
[3. Schema Compiler (Synth)] ──► Infers JSON Schemas, extracts auth, compiles 3–5 task tools
   │
   ▼
[4. Hot-Mounted Edge MCP Server] ──► Serves stateless 2026-07-28 HTTP MCP; auto-heals on drift
```

---

## The Problem

Giving an AI agent access to the web currently forces a bad trade-off:

1. **Static MCPs don't scale.** Someone has to manually write, host, and maintain a server for every internal portal or service. Loading dozens of static MCP servers dumps 30,000–50,000+ tokens of schema bloat into the agent's context window before it even starts.
2. **Browser-use agents are too slow.** Running headless Chrome to take screenshots, parse DOM accessibility trees, and guess coordinates takes 15–30 seconds per step, burns expensive VLM tokens, and collapses whenever CSS classes shift.

**The Insight:** 95% of modern web apps are thin frontends talking to structured internal XHR, Fetch, or GraphQL endpoints. Instead of making an agent parse pixels, Reify sniffs the underlying wire traffic and compiles a task-scoped, typed MCP server on the fly.

---

## Quickstart

### Install from source

```bash
cargo install --path crates/reify-cli
```

### 1. Compile any site on demand

```bash
reify https://news.ycombinator.com "search stories and read comments"
```

Output:
```text
✓ Probed https://news.ycombinator.com in 680ms (dropped 43 static assets, captured 3 API frames)
✓ Synthesized 3 task-scoped MCP tools in 14ms:
    • search_stories(query: string, tags?: string) -> json
    • get_story_comments(id: integer) -> json
    • get_top_stories(limit?: integer) -> json

✓ Mounted Stateless 2026-07-28 MCP Server:
    Endpoint: http://127.0.0.1:8080/mcp
    Blake3 Cache Key: 4f8a129d8b74c2...
```

### 2. Run the Long-Lived MCP Gateway

```bash
reify serve --host 127.0.0.1 --port 8080
```

Agents connect once to `http://127.0.0.1:8080/mcp` with a single bootstrap meta-tool:
```json
{
  "name": "reify",
  "arguments": {
    "url": "https://any-site.com",
    "goal": "search products and get prices"
  }
}
```

The gateway checks its content-addressed cache (<10ms). On a cache miss, it explores the site, compiles 3–5 dedicated tools, and returns the mounted tools directly to the agent.

### 3. Inspect Captured Wire Traffic (Diagnostic Mode)

```bash
reify probe https://example.com
```

### 4. Execute a Synthesized Tool from CLI

```bash
reify call https://news.ycombinator.com search_stories '{"query": "rust"}'
```

---

## Architecture

Reify is organized as a modular Rust workspace across 6 crates:

| Crate | Responsibility |
| :--- | :--- |
| [`reify-core`](crates/reify-core) | Domain types, Blake3 content-addressed keys, and the stateless **2026-07-28 Streamable HTTP MCP** protocol implementation. |
| [`reify-probe`](crates/reify-probe) | Direct Chrome DevTools Protocol (CDP) client over WebSocket. Discards static assets (images, CSS, fonts, audio, video) and third-party trackers (Segment, Datadog, Google Analytics) at the network socket layer. Captures raw XHR, Fetch, and GraphQL frames with session cookies and CSRF tokens. |
| [`reify-synth`](crates/reify-synth) | Algorithmic path clustering (`/items/{id}`), deterministic JSON Schema Draft 2020-12 type induction, optionality inference, and goal-directed tool pruning (strictly 3–5 tools to prevent context bloat). |
| [`reify-server`](crates/reify-server) | High-performance `POST /mcp` handler with `Mcp-Method` / `Mcp-Name` routing, streaming support, and in-memory Blake3 store. Proxies tool calls upstream and passively monitors status codes (`401/403` auth drift, `404/400` schema drift) to trigger background re-synthesis before agents fail. |
| [`reify-bridge`](crates/reify-bridge) | Local loopback development proxy (`127.0.0.1:9099`) for forwarding execution through residential TLS sessions and authenticated user cookies. |
| [`reify-cli`](crates/reify-cli) | Production CLI providing `reify`, `reify serve`, `reify probe`, `reify call`, and `reify bridge`. |

---

## Key Technical Decisions

### The 2026-07-28 Stateless Streamable HTTP MCP Spec
Legacy MCP implementations required persistent WebSocket connections, sticky sessions, or Redis backplanes. Reify uses the July 2026 stateless specification:
- Single entrypoint: `POST /mcp`
- Headers: `Mcp-Method: tools/call`, `Mcp-Name: <tool>`, `Mcp-Stream: true|false`
- Edge caching directives: `Mcp-Cache-Scope: domain|task`, `Mcp-Ttl-Ms: <ms>`
- Fully edge-compatible: Runs on serverless edge runtimes, Cloudflare Workers, or native local binaries with zero persistent session overhead.

### Amortized Zero-Latency (<10ms)
The first agent to access a site pays a ~1–2s probe and compilation cost. Every subsequent agent request hits Reify's content-addressed Blake3 cache in `<10ms`.

### Passive Drift Detection & Self-Healing
Reify proxies tool executions upstream. If an internal API changes its schema or returns `400/404`, Reify flags schema drift, initiates an asynchronous re-probe in the background, and hot-swaps the updated tool definitions into cache without interrupting running agents.

---

## Testing & Quality

Run the comprehensive unit and integration test suite:

```bash
cargo test --workspace
```

Run lints and formatting checks:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

74 test suites pass across all crates with zero warnings.

---

## License

MIT OR Apache-2.0
