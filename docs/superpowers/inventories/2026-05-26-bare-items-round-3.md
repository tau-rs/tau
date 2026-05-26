# Bare-item coverage inventory — round 3

**Source:** post-round-2 bare-item audit across the 5 tier-1 crates on 2026-05-26.
**Spec:** `docs/superpowers/specs/2026-05-26-doctests-round-3-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-26-doctests-round-3.md`.

## Categories

- **include**: classification per spec §3.1 — adds a `///` doctest fence in this PR.
- **skip-feature-gated**: item lives behind a cargo feature flag (e.g., `test-support`); a doctest would need `--features <flag>` and is out of scope for this round.
- **skip-trivial**: trivial item not requiring an example (covered by `///` prose alone).
- **skip-getter / skip-setter**: trivial accessor / mutator.
- **skip-derived**: derived trait impl.
- **skip-alias**: `pub type X = Y`.
- **skip-display / skip-debug**: `Display` / `Debug` impl.
- **skip-marker**: marker trait or unit-struct sentinel.
- **skip-reexport**: `pub use`.
- **done**: already had a fence before round 3 began.

## tau-plugin-protocol

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | error.rs:20 | `pub enum ProtocolError` | done | Fence at line 13 (on enum doc) + variant fence at EmptyFrameSlot line 60. |
| 2 | error.rs:90 | `pub struct RpcErrorEnvelope` | done | Fence at line 83, shows `RpcErrorEnvelope::new`. |
| 3 | error.rs:102 | `RpcErrorEnvelope::new(code, message, data)` | done | Covered by the struct-level fence at line 83 which calls `::new`. |
| 4 | error.rs:114 | `pub const PARSE_ERROR: i32` | skip-trivial | Numeric constant; value documented in prose; no behavior to demonstrate. |
| 5 | error.rs:116 | `pub const INVALID_REQUEST: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 6 | error.rs:118 | `pub const METHOD_NOT_FOUND: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 7 | error.rs:120 | `pub const INVALID_PARAMS: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 8 | error.rs:122 | `pub const INTERNAL_ERROR: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 9 | error.rs:124 | `pub const PLUGIN_CONTRACT_VIOLATION: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 10 | error.rs:126 | `pub const CAPABILITY_DENIED: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 11 | error.rs:131 | `pub const PORT_SPECIFIC_ERROR_BASE: i32` | skip-trivial | Numeric constant; range documented in prose. |
| 12 | frame.rs:39 | `pub enum Frame` | done | Fence at line 27 (Notification round-trip). |
| 13 | frame.rs:81 | `Frame::decode(body)` | done | Exercised by enum-level fence (round-trip pattern at line 27 calls `encode()` then `decode()`). |
| 14 | frame.rs:113 | `Frame::encode(self)` | done | Exercised by enum-level fence (round-trip pattern at line 27 calls `encode()` then `decode()`). |
| 15 | framer.rs:30 | `pub struct FramerOptions` | done | Fence at line 23 (shows `default()` value). |
| 16 | framer.rs:44 | `pub struct FramedReader<R>` | include | Constructor with 2 params + generic bound `R: AsyncRead + Unpin`; §3.1 constructor + generic. Fence rewritten as duplex round-trip with assertions. |
| 17 | framer.rs:63 | `FramedReader::new(inner, options)` | done | Exercised by struct-level fence which calls `FramedReader::new(rx, FramerOptions::default())`. |
| 18 | framer.rs:74 | `FramedReader::next_frame(&mut self)` | done | Exercised by struct-level round-trip fence; `next_frame().await.expect(...)` appears in the struct-level example. |
| 19 | framer.rs:94 | `pub struct FramedWriter<W>` | include | Constructor with generic bound `W: AsyncWrite + Unpin`; §3.1 constructor + generic. Fence rewritten as duplex write+verify with assertions. |
| 20 | framer.rs:119 | `FramedWriter::new(inner)` | done | Exercised by struct-level fence which calls `FramedWriter::new(tx)`. |
| 21 | framer.rs:124 | `FramedWriter::write_frame(&mut self, body)` | done | Exercised by struct-level round-trip fence; `write_frame(&bytes).await.expect(...)` appears in the struct-level example. |
| 22 | handshake.rs:39 | `pub struct TraceContext` | done | Fence at line 30 (shows `::new`). |
| 23 | handshake.rs:51 | `TraceContext::new(run_id, agent_id, root_span_id)` | done | Covered by the struct-level fence at line 30 which calls `TraceContext::new(...)` directly. |
| 24 | handshake.rs:79 | `pub struct HandshakeRequest` | done | Fence at line 65 (shows `::new` with all 4 params). |
| 25 | handshake.rs:95 | `HandshakeRequest::new(protocol_version, port, trace_context, config)` | done | Covered by the struct-level fence at line 65 which calls `HandshakeRequest::new(...)` with all 4 params. |
| 26 | handshake.rs:123 | `pub struct MethodSchema` | done | Fence at line 115 (shows `::new` with JSON schema values). |
| 27 | handshake.rs:132 | `MethodSchema::new(params, result)` | done | Covered by the struct-level fence at line 115 which calls `MethodSchema::new(...)`. |
| 28 | handshake.rs:158 | `pub struct HandshakeResponse` | done | Fence at line 142 (shows `::new` with all 6 params). |
| 29 | handshake.rs:176 | `HandshakeResponse::new(protocol_version, provides, plugin_name, plugin_version, methods, schemas)` | done | Covered by the struct-level fence at line 142 which calls `HandshakeResponse::new(...)` with all 6 params. |
| 30 | handshake.rs:196 | `pub mod meta` | skip-trivial | Module; the `pub const` entries it exports are individually listed below; the module itself has no doctest surface. |
| 31 | handshake.rs:198 | `meta::HANDSHAKE_METHOD: &str` | skip-trivial | String constant (`"meta.handshake"`); value documented in prose; same reasoning as JSON-RPC error code constants. |
| 32 | handshake.rs:200 | `meta::SHUTDOWN_METHOD: &str` | skip-trivial | String constant (`"meta.shutdown"`); value documented in prose. |
| 33 | handshake.rs:203 | `meta::DESCRIBE_METHOD: &str` | skip-trivial | String constant (`"meta.describe"`); value documented in prose. |
| 34 | handshake.rs:208 | `pub const PROTOCOL_VERSION: &str` | skip-trivial | String constant (`"1"`); value documented in prose; no behavior to demonstrate. |
| 35 | test_support.rs:20 | `pub struct FakeStdioPeer` | skip-feature-gated | Behind `test-support` cargo feature; doctests would require `--features test-support`, changing the default `cargo test --doc` invocation; comprehensive unit tests already live in `test_support.rs #[cfg(test)]`. |
| 36 | test_support.rs:36 | `FakeStdioPeer::new()` | skip-feature-gated | Same as above; feature-gated. |
| 37 | test_support.rs:54 | `FakeStdioPeer::expect_handshake(&mut self)` | skip-feature-gated | Same as above; feature-gated. |
| 38 | test_support.rs:84 | `FakeStdioPeer::send_handshake_response(&mut self, ...)` | skip-feature-gated | Same as above; feature-gated. |
| 39 | test_support.rs:101 | `FakeStdioPeer::expect_request(&mut self, expected_method)` | skip-feature-gated | Same as above; feature-gated. |
| 40 | test_support.rs:121 | `FakeStdioPeer::send_response<T>(&mut self, ...)` | skip-feature-gated | Same as above; feature-gated. |
| 41 | test_support.rs:137 | `FakeStdioPeer::send_response_error(&mut self, ...)` | skip-feature-gated | Same as above; feature-gated. |
| 42 | test_support.rs:159 | `FakeStdioPeer::send_stream_chunk<T>(&mut self, ...)` | skip-feature-gated | Same as above; feature-gated. |
| 43 | test_support.rs:178 | `FakeStdioPeer::send_crash(self)` | skip-feature-gated | Same as above; feature-gated. |

## Status log

- 2026-05-26 — tau-plugin-protocol classifications + 2 includes (PR-A).
- 2026-05-26 — round-3 spec-review fixes: added missing impl-method rows (Frame::decode/encode, FramedReader/Writer methods, all TraceContext/HandshakeRequest/MethodSchema/HandshakeResponse ::new constructors, meta constants, all FakeStdioPeer methods); added skip-feature-gated category; relabeled FakeStdioPeer + methods from skip-trivial to skip-feature-gated; rewrote FramedReader/FramedWriter fences as duplex round-trips with .expect() assertions.
