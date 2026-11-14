# reify-probe

```rust,no_run
use reify_core::domain::Target;
use reify_probe::ProbeEngine;
use std::time::Duration;

# async fn example() -> Result<(), reify_core::ReifyError> {
let target = Target::new("https://example.com", None)?;
let output = ProbeEngine::new().probe(&target, Duration::from_secs(20)).await?;
println!("{} exchanges at {}", output.frames.len(), output.final_url);
# Ok(())
# }
```

Each call starts a separate Chromium with an isolated temporary profile. Executable
selection checks `REIFY_CHROMIUM_PATH` first (an invalid override is an error), then
Playwright's cache under HOME, then PATH. `with_chromium` overrides discovery.
Linux Chromium and Playwright headless-shell layouts are supported. Profile
directories use mode 0700 on Unix. This stage navigates only: it does not click UI,
solve authentication flows, or drive tool-specific tasks from the target goal.

## Lifecycle and deadlines

The browser announces a debugging port on stderr. A dedicated thread continuously
drains that pipe, with bounded line buffering. Startup has a ten-second limit.
HTTP `/json/version` discovery disables proxies and locates the browser WebSocket.
The client creates a page, attaches a flattened session, and enables Page, Network,
and Runtime before navigating. See the canonical
[Target](https://chromedevtools.github.io/devtools-protocol/tot/Target/) and
[Network](https://chromedevtools.github.io/devtools-protocol/tot/Network/) protocols.

One socket actor handles command correlation, interleaved events, ping/pong, and
closure. Each command has a five-second deadline, including queue submission.
Canceled command receivers are pruned before accepting further commands. Queues
are bounded (64 submitted commands, 256 awaiting replies, 1024 events); overflowing
the event queue terminates the connection and reports failure rather than silently
losing traffic. Socket messages are limited to 8 MiB. Dropping the client aborts
the actor and releases the socket and pending receivers.

The overall probe timeout covers launch, setup, observation, page inspection, and
cookie extraction. A zero budget starts no browser. Observation completes after
page load and 750ms without retained pending requests or body retrievals. Failed
requests are removed; redirects replace state for the reused request ID, capturing
the final hop. Continuous requests, long polling, and missing load events can
exhaust the overall deadline, which returns an error rather than partial output.

On success, failure, or future cancellation, the browser owner starts a runtime-
independent reaper thread. On Unix it signals the dedicated process group with
SIGTERM, allows 500ms, sends SIGKILL to remaining group members, waits for the child,
and removes the profile. The child is deliberately not reaped before the final
group signal, preventing process ID reuse. Non-Unix platforms use child kill/wait;
Unix process groups provide the descendant cleanup guarantee. Reaping runs in the
background and can finish after `probe` returns. OS termination of the host process
cannot guarantee execution of Rust destructors.

## Capture and credentials

Static assets and the configured tracker substrings are discarded at the event
layer; they are not blocked from downloading, preserving normal page behavior.
Candidate traffic is restricted to the target host and its subdomains, with label
boundaries and HTTP(S) scheme checks. Sibling hosts are excluded: this deliberately
avoids insecure guesses at registrable domains. APIs on unrelated domains will
not be captured. Filtering is heuristic; no universal 99% reduction is claimed.

XHR/Fetch, JSON/GraphQL MIME types, and GraphQL request heuristics retain response
metadata. There are at most 512 admitted requests per probe (including requests
later rejected), 16 concurrent body reads, and 1 MiB per request/response JSON body.
Exceeding body concurrency fails the probe; reaching the admission count ignores
subsequent requests. Large responses and unavailable/evicted bodies retain their
metadata with `response_body: None`; valid JSON null is `Some(Value::Null)`.
Base64 bodies are decoded before parsing. Non-JSON payloads remain absent. Binary
and raw text are deliberately not coerced into JSON. Frames retain request-start
order; timestamps record local receipt time in UTC.

Credentials are extracted only after first-party/noise checks: Authorization,
`x-api-key`, `api-key`, `apikey`, `x-auth-token`, `x-access-token`, and CSRF headers.
`Network.getCookies` queries the target URL plus captured URLs so API paths and
subdomain cookies can be included. Cookies are checked again against the original
target host's domain boundary, accepting its subdomains and applicable ancestor
cookie domains. Expired cookies are skipped;
the earliest finite cookie expiry becomes `expires_at`. Cookie names retain case;
header names are lowercase. `csrftoken`, `xsrf-token`, and `_csrf` cookies are also
CSRF tokens. CSRF response headers are merged separately from outgoing API keys.
The resulting flat maps use last-observed values for repeated names; they cannot
represent cookies of the same name on different paths. No credentials are logged
or included in Debug output. This fresh browser has no pre-existing user login.

## Verification

`cargo test -p reify-probe` covers static/tracker and first-party filtering, API
retention, cookie/auth/CSRF assembly, protocol parsing, real loopback WebSocket
correlation and closure, mocked filesystem discovery, and (Unix) child cleanup
with an executable that ignores SIGTERM. Internal tests cover capture joins,
redirect/failure removal, Base64 JSON, and body limits. These deterministic fixtures
need no browser installation or external HTTP service. A live website smoke test
requires Chromium and network access and is separate from these unit tests.

The CLI shuts down its owning runtime before calling the blocking
`process::wait_for_cleanup` process-exit hook. Managed-child accounting includes
both live children and independent reapers, preventing the binary from exiting
before normal SIGKILL/reap/profile-cleanup work completes. Embedders must drop or
cancel browser owners before calling this hook; it does not cancel live probes.
