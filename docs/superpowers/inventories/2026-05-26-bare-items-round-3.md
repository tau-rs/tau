# Bare-item coverage inventory — round 3

**Source:** post-round-2 bare-item audit across the 5 tier-1 crates on 2026-05-26.
**Spec:** `docs/superpowers/specs/2026-05-26-doctests-round-3-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-26-doctests-round-3.md`.

## Categories

- **include**: classification per spec §3.1 — adds a `///` doctest fence in this PR.
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
| 3 | error.rs:102 | `impl RpcErrorEnvelope { fn new(...) }` | done | Covered by the struct-level fence at line 83 which calls `::new`. |
| 4 | error.rs:114 | `pub const PARSE_ERROR: i32` | skip-trivial | Numeric constant; value documented in prose; no behavior to demonstrate. |
| 5 | error.rs:116 | `pub const INVALID_REQUEST: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 6 | error.rs:118 | `pub const METHOD_NOT_FOUND: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 7 | error.rs:120 | `pub const INVALID_PARAMS: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 8 | error.rs:122 | `pub const INTERNAL_ERROR: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 9 | error.rs:124 | `pub const PLUGIN_CONTRACT_VIOLATION: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 10 | error.rs:126 | `pub const CAPABILITY_DENIED: i32` | skip-trivial | Numeric constant; value documented in prose. |
| 11 | error.rs:131 | `pub const PORT_SPECIFIC_ERROR_BASE: i32` | skip-trivial | Numeric constant; range documented in prose. |
| 12 | frame.rs:39 | `pub enum Frame` | done | Fence at line 27 (Notification round-trip). |
| 13 | framer.rs:30 | `pub struct FramerOptions` | done | Fence at line 23 (shows `default()` value). |
| 14 | framer.rs:44 | `pub struct FramedReader<R>` | include | Constructor with 2 params + generic bound `R: AsyncRead + Unpin`; §3.1 constructor + generic. Added fence at line 47. |
| 15 | framer.rs:94 | `pub struct FramedWriter<W>` | include | Constructor with generic bound `W: AsyncWrite + Unpin`; §3.1 constructor + generic. Added fence at line 105. |
| 16 | handshake.rs:39 | `pub struct TraceContext` | done | Fence at line 30 (shows `::new`). |
| 17 | handshake.rs:79 | `pub struct HandshakeRequest` | done | Fence at line 65 (shows `::new` with all 4 params). |
| 18 | handshake.rs:123 | `pub struct MethodSchema` | done | Fence at line 115 (shows `::new` with JSON schema values). |
| 19 | handshake.rs:158 | `pub struct HandshakeResponse` | done | Fence at line 142 (shows `::new` with all 6 params). |
| 20 | handshake.rs:208 | `pub const PROTOCOL_VERSION: &str` | skip-trivial | String constant (`"1"`); value documented in prose; no behavior to demonstrate. |
| 21 | test_support.rs:20 | `pub struct FakeStdioPeer` | skip-trivial | Behind `test-support` cargo feature; test-only helper with no default-feature path; doctests would require `--features test-support` changing the task-1 invocation; comprehensive tests already live in `test_support.rs #[cfg(test)]`. |

## Status log

- 2026-05-26 — tau-plugin-protocol classifications + 2 includes (PR-A).
