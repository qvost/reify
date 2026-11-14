# Verification record

All six workspace members and the final requested CLI dependencies compile.
The performance audit adds nine regression tests; no tests were disabled or
made to accept a production failure.

## Completed checks

- `cargo test --workspace --offline --no-run` compiles every workspace test.
- `cargo clippy --workspace --all-targets --offline -- -D warnings` passes.
- `cargo fmt --all --check` passes.
- The workspace run excluding only the six TCP fixtures below passes **77 tests**:
  bridge 6, CLI 7, core 21, probe 13, server 17, synth 13.
- A real process-level CLI failure fixture confirms browser children and their
  temporary profiles are reaped before binary exit.
- The release-built Axum fixture checks successful routing, complete response
  bodies, and equivalent tools. Its measurements and scope are in
  [PERFORMANCE.md](PERFORMANCE.md).

## Environment restriction

`cargo test --workspace` does **not** pass in this sandbox. The OS rejects TCP
listener binding with `EPERM` / `Operation not permitted`. The same failures
were reproduced before this audit. The workspace run stops at the probe's two
socket fixtures; a separate `cargo test -p reify-server` confirms its four TCP
fixtures fail at the same mock-listener bind, before executing tested behavior:

- `loopback_http_discovers_websocket`
- `socket_correlates_out_of_order_replies_and_reports_disconnect`
- `test_tool_execution_proxy`
- `test_drift_detection_and_self_healing`
- `schema_404_rebuilds_without_retrying_the_upstream_mutation`
- `invalid_arguments_and_missing_mount_never_touch_upstream`

Those tests remain enabled. A complete test pass, live Chromium capture, real
HTTP connection reuse, and production latency remain unverified here.

The explicit reduced run used for this environment is:

```sh
cargo test --workspace --offline -- \
  --skip loopback_http_discovers_websocket \
  --skip socket_correlates_out_of_order_replies_and_reports_disconnect \
  --skip test_tool_execution_proxy \
  --skip test_drift_detection_and_self_healing \
  --skip schema_404_rebuilds_without_retrying_the_upstream_mutation \
  --skip invalid_arguments_and_missing_mount_never_touch_upstream
```

## Re-run with TCP access

```sh
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

A Chromium installation and network access are additionally needed for live-site
smoke tests. No universal cold compilation or network latency guarantee is made.
