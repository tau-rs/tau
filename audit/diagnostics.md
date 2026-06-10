# Diagnostics & observability findings

Focus: logging quality/consistency, error-context propagation, tracing coverage,
swallowed errors, and debuggability of plugin + transport failures.
Severity scale: Critical / High / Medium / Low.

---

## O1. Silent config no-op: `--idle-timeout` produces no log, no behaviour

**Severity:** Medium
**Locations:** `crates/tau-app/src/serve/lifecycle.rs:16-85` (option never read); set at `crates/tau-cli/src/cmd/serve.rs:28`

**Description:** A user passes `--idle-timeout 300`, and nothing happens — and
nothing is logged to say it was ignored (see D1 for the functional side). There is
no startup log line echoing the effective serve configuration
(`max_concurrent`, `idle_timeout`, `shutdown_grace`), so an operator cannot tell
from the logs whether their flags took effect.

**Impact:** Misconfiguration is invisible; debugging "why didn't my server shut
down" requires reading source.

**Recommendation:** Log the resolved `ServeOptions` at startup
(`info!(?opts, "serve config")`) and, until idle-timeout is implemented, warn when
it is set.

## O2. Inconsistent diagnostic channel: `eprintln!` vs `tracing` across the install/build path

**Severity:** Medium
**Locations:**
- `crates/tau-pkg/src/install.rs:628-657` (build progress + cargo stdout/stderr via `eprintln!`/`eprint!`)
- `crates/tau-pkg/src/install.rs:731-735, 745-751` (capability warnings via `eprintln!`)
- vs. structured `tracing` used throughout `tau-runtime-tokio` (e.g. `plugin_host/process.rs:201-279`)

**Description:** The install pipeline writes human messages and full cargo output
straight to stderr with `eprintln!`, while the runtime uses structured `tracing`
with targets and fields. The two cannot be filtered, correlated, or captured
uniformly — `RUST_LOG` controls one and not the other, and there is no run-id
field tying install output to a session.

**Impact:** No unified log stream; install warnings (including the
security-relevant capability warnings) cannot be elevated, suppressed, or shipped
to a structured sink.

**Recommendation:** Route install diagnostics through `tracing` with a consistent
target (`tau_pkg::install`) and fields (package name/version). Keep raw cargo
output streaming, but tag the surrounding lifecycle messages as events.

## O3. Proxy byte-splice errors are fully swallowed

**Severity:** Low
**Locations:**
- `crates/tau-sandbox-proxy/src/lib.rs:198-201` and `:246-249` (`let _ = tokio::try_join!(copy(...), copy(...))`)

**Description:** Both directions of the proxied connection are spliced with
`tokio::io::copy` whose combined result is discarded via `let _ =`. A mid-stream
failure (reset, upstream drop, partial transfer) leaves no trace. The accept loop
and connection setup do log (`warn!`), but the data-path failures — the ones that
matter when a plugin's network call mysteriously truncates — are invisible.

**Impact:** Hard-to-debug truncated/aborted plugin network calls; no signal on
egress failures.

**Recommendation:** Inspect the `try_join!` result and `tracing::debug!`/`warn!`
on error with byte counts and host, at least at debug level.

## O4. Dispatcher backpressure / dropped responses are unobservable

**Severity:** Low
**Locations:**
- `crates/tau-app/src/serve/dispatch.rs:216-253` (`send_ok`/`send_err`/`send_notification` all `let _ = out_tx.send(...).await`)
- `crates/tau-app/src/serve/framing.rs:65` (`stdout.flush().await.ok()`)

**Description:** Every outbound message ignores the channel send result, and the
writer ignores stdout flush errors. If the writer task has died or the client
stopped reading, responses are dropped silently — the server keeps doing work
whose results never reach anyone, with no log. There's no metric/log for "writer
gone" or "flush failed (client closed stdout)."

**Impact:** A wedged or disconnected client looks identical to a healthy one in
the logs; silent data loss.

**Recommendation:** When `out_tx.send` errors, log once (writer gone → begin
shutdown). Don't `.ok()` the flush — on error, propagate or log and tear down.

## O5. Parse errors collapse the request id to `0`, breaking client correlation

**Severity:** Low
**Locations:**
- `crates/tau-app/src/serve/dispatch.rs:46-58` (parse error → `RequestId::Int(0)`)
- `crates/tau-app/src/serve/dispatch.rs:65-78` (invalid-request → also `RequestId::Int(0)`)

**Description:** JSON-RPC parse/invalid-request errors are returned with a
fabricated id `0` (the protocol's `RequestId` enum can't represent JSON `null`).
The code comments acknowledge this. A client that legitimately uses id `0`, or
that pipelines requests, cannot distinguish which message failed. The malformed
line *is* logged (`warn!`), but only server-side.

**Impact:** Client-side error correlation is unreliable for malformed input.

**Recommendation:** Model `RequestId` with a `Null` variant (JSON-RPC allows null
id on these errors) so the response carries the spec-correct id.

## O6. Plugin handshake/spawn failures are typed and logged — a positive

**Severity:** (none — strength to preserve)
**Locations:**
- `crates/tau-runtime-tokio/src/plugin_host/process.rs:201-301` (spawn logs `plugin`, `binary_path`, `pid`; failure path kills the child deterministically)
- `crates/tau-runtime-tokio/src/plugin_host/process.rs:315` (`stderr_loop` re-emits plugin stderr under the plugin name)
- `crates/tau-app/src/serve/error_map.rs` (RuntimeError → structured JSON-RPC error with `kind` + capability/tool detail)

**Description:** The plugin host has good observability: structured spawn logs
with pid, a dedicated stderr re-emit task tagged by plugin name, deterministic
child cleanup on handshake failure, and a thorough `RuntimeError → ErrorObject`
mapping that preserves capability/tool context for clients. This is the model the
rest of the codebase (O2) should follow.

**Recommendation:** Keep; use as the reference pattern for unifying O2's
`eprintln!` paths.
