# reify CLI

Build/install the developer binary:

```sh
cargo build --release -p reify-cli
cargo install --path crates/reify-cli
reify --help
```

## Connect an AI agent

```sh
reify install --dry-run
reify install
reify serve --port 8080
```

`install` discovers Cursor, Claude Code, Windsurf, and OpenCode from existing
configuration files, agent directories, or executables on PATH. It configures
detected agents globally by default. Select `--agent cursor|claude|windsurf|opencode`
to create a configuration even when discovery finds no installation.

| Agent | Global configuration | Workspace configuration (`--local`) |
| --- | --- | --- |
| Cursor | `~/.cursor/mcp.json`, or an existing XDG `Cursor` configuration directory | `.cursor/mcp.json` |
| Claude Code | `~/.claude.json`, or existing `~/.claude/config.json` | `.mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | `.codeium/windsurf/mcp_config.json` |
| OpenCode v2 | `$XDG_CONFIG_HOME/opencode/opencode.json` (defaults to `~/.config/opencode/opencode.json`) | `opencode.json` |

```sh
reify install --agent claude --port 8081
reify install --agent cursor --local
reify install --agent opencode --global=false --dry-run
reify install --uninstall
```

The command updates only the `reify` server entry, preserves other servers and
settings, and writes changed JSON through a temporary sibling file followed by
an atomic rename. Existing permissions and config-file symlinks are preserved.
All target configs are parsed before any edit; malformed JSON or an incompatible
server-container type stops the operation. Repeated installs skip unchanged
files. Uninstall preserves empty server maps and creates no missing files.
`--dry-run` prints the planned Reify entry without writing files or directories.
The gateway runs separately with `reify serve`; installation prints its startup
command and endpoint.

## Compile and mount

```sh
reify https://example.com 'search flights'
reify --chromium /path/to/chromium --timeout 30 https://example.com search flights
```

The default mode reserves `127.0.0.1:8080`, resolves the canonical BLAKE3 domain/goal
key through the server's shared `ServerStore`. A cache miss first checks the
embedded profile catalog, then falls back to live capture/synthesis. It stays in
the foreground serving `/mcp` until Ctrl+C or SIGTERM.
It does not fork an unmanaged server process or silently edit client config files.
Chromium is discovered through `REIFY_CHROMIUM_PATH`, Playwright's cache, or PATH.
Put global options before the URL; all following words are the goal, including
words such as `serve` that also name subcommands. `--fresh` (alias `--no-profile`)
can also follow the URL/goal and forces live discovery.

```sh
reify profiles
reify https://news.ycombinator.com 'search stories'
reify --fresh https://news.ycombinator.com 'search stories'
reify serve --no-profile
```

Eight embedded profiles provide 16 typed tools for Hacker News, GitHub, Linear,
DummyJSON, Wikipedia, Open-Meteo, Reddit, and Stripe. Matching goals select the
relevant tools. Profile mounts require no browser or network discovery; private
API calls still require credentials for the actual API origin. `reify probe`
always captures live traffic. Remote `reify call --fresh --gateway …` sends
`fresh: true` to the bootstrap tool, bypassing an existing cached generation.

The profile-match timing is measured separately from one-time process/catalog
initialization and socket binding. The [profile guide](../reify-core/src/profiles/README.md)
documents endpoint sources, matching, credentials, and a reproducible benchmark.

The summary displays measured profile-match or capture/synthesis times, retained API-frame counts,
tool names/descriptions and input types, the endpoint, cache key, and mount token.
No static-noise counts or sub-two-second guarantees are fabricated. Cold capture
latency depends on Chromium startup and the target page. The server's 45-second
whole-mount deadline also bounds the configurable probe budget in mount/call modes.

The printed Claude Code `.mcp.json`, Cursor `.cursor/mcp.json`, and OpenCode v2
`opencode.json` configurations carry `Reify-Mount` in request headers so clients
immediately discover the warmed tools. Every HTTP request remains independently
scoped; no connection session is created. OpenCode's snippet explicitly selects
protocol `2026-07-28`. `reify` remains listed alongside the compiled tools.

Configuration shapes follow the primary [Claude Code](https://code.claude.com/docs/en/mcp),
[Cursor](https://cursor.com/docs/mcp), and [OpenCode](https://opencode.ai/docs/mcp-servers/)
documentation. Clients must support the current stateless MCP revision. Mount
headers are private routing capabilities for one server instance, not a replacement
for host authentication or a persistent credential store.

## Gateway, probe, and call

```sh
reify serve
reify serve --host 127.0.0.1 --port 8081
reify --color never probe https://example.com > capture.json
reify call https://example.com search_flights '{"q":"KTM"}'
reify call https://example.com search_flights '{"q":"KTM"}' --goal 'search flights' \
  --gateway http://127.0.0.1:8080/mcp
```

`serve` binds the requested hostname/IP and port, prints the actual endpoint
(including ephemeral port 0), and exposes the root meta-tool. `probe` runs the
real headless capture and writes one pretty JSON document containing frames,
final URL, title, and duration. The standalone `AuthContext` is omitted; raw frame
headers and bodies remain visible for diagnostics and may contain credentials.
No server starts for `probe`.

`call` validates a JSON **object** at the command boundary, resolves the target's
compiled tools, invokes the exact named plan, and prints its result as JSON. Tool
errors are still printed as structured JSON and return exit code 1. HTTP mutation
calls are never retried automatically. Missing tool names report the available
names instead of guessing an endpoint.

Caches live in a running process and are not serialized to disk with credentials.
An independent `call` without `--gateway` has a fresh local cache; it may need a
browser capture. Supply `--gateway` (or `REIFY_GATEWAY`) to reuse an existing
server's cache. Supply the same `--goal` as a prior task-scoped compilation to
resolve that cache key. Remote calls use proper MCP routing/version metadata,
bootstrap a mount through `reify`, then carry its opaque token on invocation.

## Development bridge

```sh
export REIFY_BRIDGE_TOKEN='a-long-random-local-development-token'
reify bridge --port 9099 --session-file ./sessions.json
```

The bridge binds IPv4 loopback and forwards through the local Reqwest client.
The session file is optional, limited to 1 MiB, and maps exact domain names to
explicit `AuthContext` bundles. For example:

```json
{
  "api.example.com": {
    "cookies": {},
    "authorization_header": "Bearer developer-configured-token",
    "csrf_tokens": {},
    "custom_headers": {},
    "expires_at": null
  }
}
```

The token can also be supplied with `--token`; environment configuration avoids
shell-history exposure. Neither startup nor `/health` prints the token or session
credentials. The bridge binary never reads Chromium profiles, keychains, or
browser state; the optional Chrome companion shares cookies only after a click.
See the [bridge guide](../reify-bridge/README.md)
for the typed `/forward` API, header precedence, and response envelope.

For authenticated browser tabs, load the [Chrome companion](../../extension/README.md),
connect it with the bridge token, and click **Bridge Current Tab**. Start
`reify serve` in another terminal with the same `REIFY_BRIDGE_TOKEN`; the gateway
then uses live shared credentials for probing and tool calls. Both processes are
needed. The companion guide covers expiry and revocation.

## Terminal and lifecycle behavior

`--color auto|always|never` honors TTY detection and `NO_COLOR` in auto mode.
The `colored` crate handles styles. Progress appears on **stderr**, only on a real
terminal and outside CI; it is cleared on success, failure, and cancellation.
`probe` and `call` stdout contain only JSON, suitable for piping. Broken stdout
pipes exit successfully. Untrusted tool text has terminal control characters and
bidirectional overrides removed before human-facing rendering.

`tracing-subscriber` sends logs to stderr with `RUST_LOG` filtering (default warn).
Argument errors use Clap's usage/error rendering and exit 2; runtime/tool errors
exit 1; interrupted short operations exit 130. Server shutdown handles Ctrl+C and
Unix SIGTERM, starts graceful draining, and caps drain time at five seconds. The
binary then shuts down remaining runtime tasks and waits up to two seconds for
independent Chromium reapers, so cancellation cannot simply abandon their
SIGTERM/SIGKILL/profile-cleanup work at process exit.

## Verification

Tests cover subcommands/defaults/options, URL and JSON-object validation, goal words
that match command names, clean subprocess errors, configuration/header output,
terminal sanitization, cancellation signaling, and browser profile cleanup before
binary exit. The bridge has separate in-process HTTP tests requiring no TCP socket.

```sh
cargo test -p reify-cli -p reify-bridge
cargo test --workspace
```

The complete suite also requires permission to bind loopback sockets and fetch
new Cargo dependencies. See [workspace verification notes](../../VALIDATION.md)
for the current environment's results and limitations.
