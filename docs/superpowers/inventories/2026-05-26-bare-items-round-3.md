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
- **skip-needs-fixture**: item requires non-trivial test fixtures (real sandbox setup, env var injection, multi-thread coordination) that exceed reasonable doctest scope; deferred to future round.
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

## tau-plugin-sdk

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | configure.rs:31 | `pub enum ConfigError` | done | Fence at line 24 (round 2 or prior); shows `ConfigError::MissingField` + `format!` assertion. |
| 2 | configure.rs:99 | `pub trait Configure` | done | Fence at line 76 (round 2 or prior); shows full `impl Configure for MyPlugin` with `from_config`. |
| 3 | error.rs:19 | `pub enum SdkError` | done | Fence at line 12; shows `SdkError::HandshakeMissing` + `format!` assertion. |
| 4 | handshake.rs:29 | `pub struct PluginMeta` | include | Constructor with 5 params + `#[non_exhaustive]`; §3.1 constructor rule. New fence calls `PluginMeta::new` with all 5 args and asserts `plugin_name`, `port`, `methods.len()`. |
| 5 | handshake.rs:45 | `PluginMeta::new(...)` (impl method) | done | Covered by `PluginMeta` struct-level fence (row 4 above) which calls `::new`. |
| 6 | handshake.rs:77 | `pub async fn drive_handshake` | include | 2+ generic params (`R: AsyncRead + Unpin`, `W: AsyncWrite + Unpin`) + returns `Result`; §3.1 generic-bounds + error-path rule. `no_run` fence (blocks on stdin in real use). Asserts `request.port` after a hypothetical successful handshake. |
| 7 | lib.rs:21 | `pub mod configure` | skip-trivial | Module declaration; no doctest surface. |
| 8 | lib.rs:22 | `pub mod error` | skip-trivial | Module declaration; no doctest surface. |
| 9 | lib.rs:23 | `pub mod handshake` | skip-trivial | Module declaration; no doctest surface. |
| 10 | lib.rs:24 | `pub mod runners` | skip-trivial | Module declaration; no doctest surface. |
| 11 | lib.rs:25 | `pub mod streaming` | skip-trivial | Module declaration; no doctest surface. |
| 12 | lib.rs:26 | `pub mod tracing_layer` | skip-trivial | Module declaration; no doctest surface. |
| 13 | runners/llm_backend.rs:46 | `pub async fn run_llm_backend` | include | Entry-point runner; §3.1 free-function with generic `P: LlmBackend + Send + Sync + 'static`. `no_run` fence (blocks on stdin). Shows typical `#[tokio::main]` entry-point pattern with hidden `MyPlugin` LlmBackend impl. |
| 14 | runners/llm_backend.rs:73 | `pub async fn run_llm_backend_with_io` | include | Same family; 3 generic params + explicit reader/writer; §3.1 generic-bounds. `no_run` fence with full hidden LlmBackend impl and FramedReader/FramedWriter construction. |
| 15 | runners/llm_backend.rs:153 | `pub async fn run_llm_backend_with_config` | done | Fence at line 122 (`no_run`); shows `run_llm_backend_with_config::<MyPlugin>` in `main`. |
| 16 | runners/llm_backend.rs:179 | `pub async fn run_llm_backend_with_config_with_io` | include | `_with_io` variant of above; 3 generics + Configure bound; no prior fence. `no_run` with hidden Configure + LlmBackend impl, shows turbofish `::< _, _, MyPlugin>`. |
| 17 | runners/storage.rs:34 | `pub async fn run_storage` | include | Entry-point runner for Storage port; §3.1 free-function. `no_run` fence with hidden Storage impl showing all 4 async methods. |
| 18 | runners/storage.rs:60 | `pub async fn run_storage_with_io` | include | `_with_io` variant; 3 generic params. `no_run` fence with full hidden Storage impl + FramedReader/FramedWriter. |
| 19 | runners/storage.rs:112 | `pub async fn run_storage_with_config` | include | Configure variant; no prior fence. `no_run` with hidden Configure + Storage impl. |
| 20 | runners/storage.rs:137 | `pub async fn run_storage_with_config_with_io` | include | `_with_io` + Configure variant; no prior fence. `no_run` with full hidden impls + turbofish. |
| 21 | runners/tool.rs:53 | `pub async fn run_tool` | include | Entry-point runner for Tool port; §3.1 free-function. `no_run` fence with hidden Tool impl (all 5 methods + `type Session = ()`). |
| 22 | runners/tool.rs:76 | `pub async fn run_tool_with_io` | include | `_with_io` variant; 3 generic params. `no_run` fence with full hidden Tool impl + FramedReader/FramedWriter. |
| 23 | runners/tool.rs:158 | `pub async fn run_tool_with_config` | done | Fence at line 125 (`no_run`); shows `run_tool_with_config::<MyTool>` in `main`. |
| 24 | runners/tool.rs:179 | `pub async fn run_tool_with_config_with_io` | include | `_with_io` + Configure variant; no prior fence. `no_run` with full hidden impls + turbofish. |
| 25 | streaming.rs:42 | `pub async fn stream_completion` | include | Takes `CompletionStream` (generic stream); §3.1 free-function + non-trivial params. `no_run` fence showing `Box::pin(stream::iter(...))` with `CompletionChunk::Text { delta }` + `Finish` then `stream_completion` call. |
| 26 | tracing_layer.rs:17 | `pub fn install()` | include | Idempotent install with no return value; §3.1 free-function. Executable fence (calls `install()` twice, asserts `true` to verify no panic). |


## tau-domain

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | agent.rs:40 | `pub enum AgentStatus` | done | Fence at line 19 (shows `::failed` + `match`). |
| 2 | agent.rs:82 | `AgentStatus::failed(kind, detail)` | done | Fence at line 72 (shows `AgentStatus::failed` + `match` arm). |
| 3 | agent.rs:94 | `pub enum FailureKind` | skip-trivial | Unit-variant enum; no constructors or methods; all usage covered by `AgentStatus::failed` fence. |
| 4 | agent.rs:157 | `pub struct AgentDefinition` | done | Fence at line 135 (shows `::new` with all 4 params + `assert_eq!(def.id.as_str(), …)`). |
| 5 | agent.rs:176 | `AgentDefinition::new(id, display_name, package, llm_backend)` | done | Covered by struct-level fence at line 135. |
| 6 | agent.rs:193 | `AgentDefinition::with_system_prompt(prompt)` | include | Builder method returning `Self`; §3.1 non-trivial conversion. New executable fence: builds `AgentDefinition`, calls `with_system_prompt`, asserts `system_prompt.as_deref()`. |
| 7 | agent.rs:199 | `AgentDefinition::with_config(config)` | include | Builder method returning `Self`; §3.1 non-trivial conversion. New executable fence: builds `AgentDefinition`, calls `with_config` with a `BTreeMap`, asserts `.config.get(…).as_string()`. |
| 8 | error.rs:23 | `pub enum PackageNameError` | done | Fence at line 14 (shows `PackageName::from_str()` → `PackageNameError::Empty`). |
| 9 | error.rs:64 | `pub enum AgentIdError` | done | Fence at line 52 (shows `AgentId::from_str()` → `AgentIdError::Empty`). |
| 10 | error.rs:106 | `pub enum PackageSourceError` | done | Fence at line 93 (shows `PackageSource::from_str()` → `PackageSourceError::Empty`). |
| 11 | error.rs:136 | `pub enum PackageKindError` | include | `#[non_exhaustive]` error enum; §3.1 error path. New executable fence: constructs `PackageKindError::Empty` and asserts `to_string()` contains `"empty"`. |
| 12 | error.rs:149 | `pub enum PackageManifestError` | include | Error enum with associated data variants; §3.1 enum with associated data + error path. New executable fence: constructs `EmptyDescription` and `CapabilityEmptyName { index: 0 }`, asserts `to_string()` content. |
| 13 | error.rs:200 | `pub enum PortKindError` | done | Fence at line 187 (shows `PortKind::from_str(nonsense)` → `PortKindError::Unknown`). |
| 14 | error.rs:223 | `pub enum PluginKindError` | done | Fence at line 209 (shows `PluginKind::from_str(nonsense)` → `PluginKindError::Unknown`). |
| 15 | fixtures.rs:31 | `pub fn any_package_name()` | skip-feature-gated | Behind `test-fixtures` cargo feature; doctest would require `--features test-fixtures`. |
| 16 | fixtures.rs:36 | `pub fn any_agent_id()` | skip-feature-gated | Same as above. |
| 17 | fixtures.rs:41 | `pub fn any_package_source()` | skip-feature-gated | Same as above. |
| 18 | fixtures.rs:46 | `pub fn any_unchecked_manifest()` | skip-feature-gated | Same as above. |
| 19 | fixtures.rs:66 | `pub fn any_package_manifest()` | skip-feature-gated | Same as above. |
| 20 | fixtures.rs:73 | `pub fn any_agent_definition()` | skip-feature-gated | Same as above. |
| 21 | fixtures.rs:86 | `pub fn any_message()` | skip-feature-gated | Same as above. |
| 22 | fixtures.rs:112 | `pub fn cap_fs_read(paths)` | skip-feature-gated | Same as above. |
| 23 | fixtures.rs:120 | `pub fn cap_fs_write(paths, max_bytes)` | skip-feature-gated | Same as above. |
| 24 | fixtures.rs:129 | `pub fn cap_fs_exec(paths)` | skip-feature-gated | Same as above. |
| 25 | fixtures.rs:137 | `pub fn cap_net_http(hosts, methods)` | skip-feature-gated | Same as above. |
| 26 | fixtures.rs:146 | `pub fn cap_process_spawn(commands)` | skip-feature-gated | Same as above. |
| 27 | fixtures.rs:154 | `pub fn cap_agent_spawn(allowed_kinds)` | skip-feature-gated | Same as above. |
| 28 | fixtures.rs:161 | `pub fn cap_custom(name)` | skip-feature-gated | Same as above. |
| 29 | fixtures.rs:169 | `pub fn cap_custom_with_params(name, params)` | skip-feature-gated | Same as above. |
| 30 | id.rs:24 | `pub struct PackageName(String)` | done | Fence at line 16 (shows `PackageName::from_str(fs-tools)` + `as_str()`). |
| 31 | id.rs:28 | `PackageName::MAX_LEN: usize` | skip-trivial | Numeric constant; value documented in prose; no behavior to demonstrate. |
| 32 | id.rs:31 | `PackageName::as_str(&self)` | skip-getter | Trivial view into inner field; covered by struct-level fence. |
| 33 | id.rs:148 | `pub struct AgentId(String)` | done | Fence at line 139 (shows `AgentId::from_str(researcher)` + `as_str()`). |
| 34 | id.rs:151 | `AgentId::MAX_LEN: usize` | skip-trivial | Numeric constant. |
| 35 | id.rs:154 | `AgentId::as_str(&self)` | skip-getter | Trivial view; covered by struct-level fence. |
| 36 | id.rs:249 | `pub struct AgentInstanceId(uuid::Uuid)` | done | Fence at line 241 (shows `::new` + uniqueness assert). |
| 37 | id.rs:252 | `AgentInstanceId::new()` | done | Covered by struct-level fence at line 241. |
| 38 | id.rs:258 | `AgentInstanceId::from_uuid(u)` | include | Constructor taking a `Uuid` value; §3.1 constructor. New executable fence: creates UUID v7, wraps via `from_uuid`, asserts `as_uuid()` round-trip. |
| 39 | id.rs:263 | `AgentInstanceId::as_uuid(&self)` | skip-getter | Trivial accessor; covered by `from_uuid` fence which calls it. |
| 40 | id.rs:301 | `pub struct MessageId(uuid::Uuid)` | done | Fence at line 293 (shows `::new` + round-trip through `to_string().parse()`). |
| 41 | id.rs:304 | `MessageId::new()` | done | Covered by struct-level fence at line 293. |
| 42 | id.rs:310 | `MessageId::from_uuid(u)` | include | Constructor taking a `Uuid` value; §3.1 constructor. New executable fence: creates UUID v7, wraps via `from_uuid`, asserts `as_uuid()` round-trip. |
| 43 | id.rs:315 | `MessageId::as_uuid(&self)` | skip-getter | Trivial accessor; covered by `from_uuid` fence. |
| 44 | lib.rs:8 | `pub mod agent` | skip-trivial | Module declaration; no doctest surface. |
| 45 | lib.rs:9 | `pub mod error` | skip-trivial | Module declaration; no doctest surface. |
| 46 | lib.rs:10 | `pub mod id` | skip-trivial | Module declaration; no doctest surface. |
| 47 | lib.rs:11 | `pub mod message` | skip-trivial | Module declaration; no doctest surface. |
| 48 | lib.rs:12 | `pub mod package` | skip-trivial | Module declaration; no doctest surface. |
| 49 | lib.rs:13 | `pub mod value` | skip-trivial | Module declaration; no doctest surface. |
| 50 | lib.rs:14 | `pub mod version` | skip-trivial | Module declaration; no doctest surface. |
| 51 | lib.rs:17 | `pub mod fixtures` | skip-feature-gated | Gated behind `test-fixtures` feature; module itself has no doctest surface. |
| 52 | message.rs:14 | `pub enum Address` | include | `#[non_exhaustive]` enum with associated-data variant (`Agent(AgentInstanceId)`); §3.1. New executable fence: constructs all 4 variants, asserts `matches!`. |
| 53 | message.rs:31 | `pub enum MessagePayload` | include | `#[non_exhaustive]` enum with multiple associated-data variants; §3.1. New executable fence: constructs `Text`, `ToolCall`, `ToolError` variants and asserts `matches!`. |
| 54 | message.rs:89 | `pub struct Message` | done | Fence at line 74 (shows `Message::new` + `parent_id.is_none()`). |
| 55 | message.rs:131 | `Message::new(sender, recipient, payload)` | done | Fence at line 119 (shows `Message::new` + `matches!` + `parent_id.is_none()`). |
| 56 | package/capability.rs:35 | `pub enum Capability` | done | Fence at line 20 (shows `Capability::Custom` + `required_shape()`). |
| 57 | package/capability.rs:344 | `Capability::required_shape(&self)` | include | Returns non-trivial `CapabilityShape`; §3.1 non-trivial method. New executable fence on `impl Capability` block: constructs `Capability::Custom`, asserts `required_shape()` equals `CapabilityShape::Custom { name }`. |
| 58 | package/capability.rs:89 | `pub enum FsCapability` | done | Fence at line 78 (shows `CapabilityShape::FilesystemRead` assertion). |
| 59 | package/capability.rs:131 | `pub enum NetCapability` | done | Fence at line 120 (shows `CapabilityShape::NetworkHttp` assertion). |
| 60 | package/capability.rs:161 | `pub enum ProcessCapability` | done | Fence at line 150 (shows `CapabilityShape::ProcessExec` assertion). |
| 61 | package/capability.rs:189 | `pub enum AgentCapability` | done | Fence at line 178 (shows `CapabilityShape::AgentSpawn` assertion). |
| 62 | package/capability.rs:223 | `pub enum SkillCapability` | done | Fence at line 212 (shows `CapabilityShape::SkillSpawn` assertion). |
| 63 | package/capability.rs:250 | `pub enum CapabilityShape` | skip-trivial | Enum variants demonstrated by sub-capability fences above (rows 58-62). |
| 64 | package/capability.rs:280 | `pub struct CapabilityShapeSet` | include | Constructor `::new()` + `insert` + `contains` + `is_subset_of` methods with 2+ params; §3.1. New executable fence: builds two sets, asserts `is_subset_of` + `contains` + `len`. |
| 65 | package/capability.rs:303 | `CapabilityShapeSet::new()` | done | Exercised by struct-level fence at line 280 (`new()` called to build both sets). |
| 66 | package/capability.rs:308 | `CapabilityShapeSet::insert(&mut self, shape)` | done | Exercised by struct-level fence at line 280 (`insert` called on both sets). |
| 67 | package/capability.rs:315 | `CapabilityShapeSet::contains(&self, shape)` | done | Exercised by struct-level fence at line 280 (`assert!(adapter.contains(…))`). |
| 68 | package/capability.rs:320 | `CapabilityShapeSet::is_subset_of(&self, other)` | done | Exercised by struct-level fence at line 280 (`assert!(plan.is_subset_of(&adapter))`). |
| 69 | package/capability.rs:325 | `CapabilityShapeSet::iter(&self)` | skip-getter | Trivial iterator accessor; not called in struct-level fence; pattern is straightforward `.iter()` delegation. |
| 70 | package/capability.rs:330 | `CapabilityShapeSet::len(&self)` | done | Exercised by struct-level fence at line 280 (`assert_eq!(adapter.len(), 2)`). |
| 71 | package/capability.rs:335 | `CapabilityShapeSet::is_empty(&self)` | skip-getter | Trivial boolean getter; not called in struct-level fence; no behavior to demonstrate beyond `is_empty() == (len() == 0)`. |
| 72 | package/manifest.rs:34 | `pub struct PackageDep` | done | Fence at line 21 (shows field-type construction + assertions). |
| 73 | package/manifest.rs:60 | `pub struct PackageId` | done | Fence at line 45 (shows `::new` + field assertions). |
| 74 | package/manifest.rs:87 | `PackageId::new(name, version)` | done | Exercised by struct-level fence at line 45 (`PackageId::new(…)` called directly). |
| 75 | package/manifest.rs:110 | `pub enum PackageKind` | done | Fence at line 100 (shows `PackageKind::Custom { kind: tool }` + `matches!`). |
| 76 | package/manifest.rs:171 | `pub mod kinds` | skip-trivial | Module with string constants; individually covered by prose. |
| 77 | package/manifest.rs:203 | `pub struct UncheckedManifest` | done | Fence at line 195 (`no_run`; shows TOML parse + validate pattern). |
| 78 | package/manifest.rs:518 | `UncheckedManifest::validate(self)` | done | Fence at line 508 (`no_run`; `UncheckedManifest` is `#[non_exhaustive]` with no public constructor; shows `unchecked.validate()?` pattern). |
| 79 | package/manifest.rs:262 | `pub struct PackageManifest(UncheckedManifest)` | done | Fence at line 255 (`no_run`; shows `unchecked.validate()?` pattern). |
| 80 | package/manifest.rs:266 | `PackageManifest::name(&self)` | skip-getter | Trivial accessor returning `&PackageName`. |
| 81 | package/manifest.rs:270 | `PackageManifest::version(&self)` | skip-getter | Trivial accessor returning `&Version`. |
| 82 | package/manifest.rs:274 | `PackageManifest::description(&self)` | skip-getter | Trivial accessor returning `&str`. |
| 83 | package/manifest.rs:278 | `PackageManifest::authors(&self)` | skip-getter | Trivial accessor returning `&[String]`. |
| 84 | package/manifest.rs:282 | `PackageManifest::license(&self)` | skip-getter | Trivial accessor returning `Option<&str>`. |
| 85 | package/manifest.rs:286 | `PackageManifest::source(&self)` | skip-getter | Trivial accessor returning `&PackageSource`. |
| 86 | package/manifest.rs:290 | `PackageManifest::kind(&self)` | skip-getter | Trivial accessor returning `&PackageKind`. |
| 87 | package/manifest.rs:294 | `PackageManifest::dependencies(&self)` | skip-getter | Trivial accessor returning `&[PackageDep]`. |
| 88 | package/manifest.rs:298 | `PackageManifest::capabilities(&self)` | skip-getter | Trivial accessor returning `&[Capability]`. |
| 89 | package/manifest.rs:306 | `PackageManifest::plugin(&self)` | skip-getter | Trivial accessor returning `Option<&PluginManifest>`. |
| 90 | package/manifest.rs:311 | `PackageManifest::sandbox(&self)` | skip-getter | Trivial accessor returning `&PluginSandboxRequirements`. |
| 91 | package/manifest.rs:319 | `PackageManifest::skill(&self)` | skip-getter | Trivial accessor returning `Option<&SkillManifest>`. |
| 92 | package/mod.rs:3 | `pub mod capability` | skip-trivial | Module declaration. |
| 93 | package/mod.rs:4 | `pub mod manifest` | skip-trivial | Module declaration. |
| 94 | package/mod.rs:5 | `pub mod plugin` | skip-trivial | Module declaration. |
| 95 | package/mod.rs:6 | `pub mod sandbox` | skip-trivial | Module declaration. |
| 96 | package/mod.rs:7 | `pub mod skill` | skip-trivial | Module declaration. |
| 97 | package/mod.rs:8 | `pub mod skill_format` | skip-trivial | Module declaration. |
| 98 | package/mod.rs:9 | `pub mod source` | skip-trivial | Module declaration. |
| 99 | package/plugin.rs:33 | `pub enum PortKind` | done | Fence at line 21 (shows `from_str("llm_backend")` + `to_string()` round-trip). |
| 100 | package/plugin.rs:105 | `pub enum PluginKind` | done | Fence at line 96 (shows `from_str("rust-cargo")` + `to_string()` round-trip). |
| 101 | package/plugin.rs:169 | `pub struct PluginManifest` | done | Fence at line 153 (shows `::new` with all 3 params + field assertions). |
| 102 | package/plugin.rs:181 | `PluginManifest::new(provides, kind, bin)` | done | Exercised by struct-level fence at line 153 (`PluginManifest::new(…)` called directly). |
| 103 | package/sandbox.rs:34 | `pub struct PluginSandboxRequirements` | done | Fence at line 21 (shows `default()` + assertions on null state). |
| 104 | package/sandbox.rs:57 | `pub enum PluginRequiredTier` | include | Enum with `PartialOrd`/`Ord` impl (non-trivial ordering); §3.1 non-trivial impl. New executable fence: asserts ordering `None < Light < Strict` and `max()` behavior. |
| 105 | package/skill.rs:35 | `pub const SKILL_DIR_VAR: &str` | skip-trivial | String constant (`"${SKILL_DIR}"`); value documented in prose. |
| 106 | package/skill.rs:45 | `pub struct SkillManifest` | include | `#[non_exhaustive]` struct; §3.1 (production type). `no_run` fence — construction requires `serde` + TOML parsing; pattern shows TOML-parse → `u.skill.as_ref()` access. |
| 107 | package/skill.rs:77 | `pub struct SkillFrontmatter` | include | `#[non_exhaustive]` struct; §3.1 (production type). `no_run` fence — instances come from `parse_skill_md` (feature-gated); pattern shows parse → `.frontmatter.name`. |
| 108 | package/skill.rs:95 | `pub struct SkillContent` | include | `#[non_exhaustive]` struct; §3.1 (production type). `no_run` fence — instances come from `parse_skill_md` (feature-gated); pattern shows parse → `.body`. |
| 109 | package/skill.rs:106 | `pub enum SkillContentError` | include | Error enum with multiple variants; §3.1 error path. New executable fence: constructs `MissingFrontmatterOpener` and `MissingName`, asserts `to_string()` content. |
| 110 | package/skill.rs:144 | `pub fn parse_skill_md(input)` | skip-feature-gated | Gated behind `#[cfg(feature = "serde")]`; serde not on by default; comprehensive tests in `#[cfg(all(test, feature = "serde"))]`. |
| 111 | package/skill_format.rs:32 | `pub enum SkillFormat` | include | `#[non_exhaustive]` enum; §3.1. New executable fence: constructs all 3 variants and asserts `eq` / `ne`. |
| 112 | package/skill_format.rs:52 | `pub fn detect_format(dir)` | include | Free function; §3.1 free function. New executable fence: creates two `tempdir()`s (with `tau.toml` / empty), asserts `Tau` and `Invalid`. |
| 113 | package/skill_format.rs:78 | `pub fn synthesize_manifest_from_skill_md(parsed, source)` | skip-feature-gated | Gated behind `#[cfg(feature = "serde")]`; same reasoning as `parse_skill_md`. |
| 114 | package/skill_format.rs:123 | `pub enum SynthesizeError` | include | Error enum; variants constructible externally without any feature gate (only the producer `synthesize_manifest_from_skill_md` is feature-gated). New executable fence: constructs `InvalidName`, asserts `to_string()` contains the name. |
| 115 | package/source.rs:31 | `pub enum PackageSource` | done | Fence at line 19 (shows `from_str("https://…#main")` + `to_string()` round-trip). |
| 116 | package/source.rs:85 | `pub enum GitLocation` | include | `#[non_exhaustive]` enum with `Scp { user, host, path }` associated-data variant; §3.1. New executable fence: constructs both shapes via `PackageSource::from_str`, asserts variant via `matches!`. |
| 117 | value.rs:52 | `pub enum Value` | done | Two fences at lines 23 and 40 (Object + accessor chain; Bytes serde round-trip). |

## tau-runtime

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | builder.rs | `Runtime::builder()` | include | Factory with no args; §3.1 constructor + fluent chain. Fence shows `builder() → with_llm_backend → build`. |
| 2 | builder.rs | `RuntimeBuilder::with_llm_backend` | include | Builder method; §3.1 constructor. Fence shows MockLlmBackend attachment. |
| 3 | builder.rs | `RuntimeBuilder::with_tool` | include | Builder method; §3.1. |
| 4 | builder.rs | `RuntimeBuilder::with_storage` | include | Builder method; §3.1. |
| 5 | builder.rs | `RuntimeBuilder::with_dyn_llm_backend` | include | Builder method (dyn variant); §3.1. |
| 6 | builder.rs | `RuntimeBuilder::with_dyn_tool` | include | Builder method (dyn variant); §3.1. |
| 7 | builder.rs | `RuntimeBuilder::with_dyn_storage` | include | Builder method (dyn variant); §3.1. |
| 8 | builder.rs | `RuntimeBuilder::build` | include | Builder terminal; returns `Result`; §3.1 Result. |
| 9 | builder.rs | `RuntimeBuilder::build_allow_empty` | include | Builder terminal (serve-mode variant); §3.1 Result. |
| 10 | error.rs | `CapabilityDenial` (struct) | include | Production struct; §3.1. Added `::new()` constructor; fence exercises it + `to_string()`. |
| 11 | error.rs | `HandshakeFailureReason` (enum) | include | Enum with associated-data variants; §3.1. Fence constructs `Timeout` + `ProtocolVersionMismatch`. |
| 12 | stream.rs | `RunEvent` (enum) | include | `#[non_exhaustive]` streaming event enum; §3.1. Fence shows pattern-match helper + `TextDelta` + `ToolCallStarted` construction. |
| 13 | orchestration/budget.rs | `BudgetWatchdog` (struct) | include | Stateless watchdog; §3.1. Fence shows default-budget succeeds. |
| 14 | orchestration/budget.rs | `BudgetWatchdog::new` | skip-trivial | Unit-struct constructor; covered by struct-level fence (row 13). |
| 15 | orchestration/budget.rs | `BudgetWatchdog::tick` | include | Method returning `Result`; §3.1. Fence shows within-budget + exceeded paths. |
| 16 | orchestration/error.rs | `OrchestrationError` (enum) | include | Error enum with multiple variants; §3.1 error path. Fence shows `TaskNotFound` + `BudgetExceeded`. |
| 17 | orchestration/persistence.rs | `run_log_path` | include | Free function; §3.1. Fence checks path shape. |
| 18 | orchestration/persistence.rs | `RunLogLine` (enum) | include | Serde-tagged union; §3.1. Fence round-trips `TaskMutation` through JSON. |
| 19 | orchestration/run_state.rs | `RunState` (struct) | include | Core per-run state; §3.1. Fence constructs + asserts `run_id`/`status`. |
| 20 | orchestration/run_state.rs | `RunState::new` | include | Constructor with 4 params; §3.1. Fence checks `root_agent_id` + `plan.is_empty()`. |
| 21 | orchestration/run_state.rs | `RunState::append_plan_note` | include | Method; §3.1. Fence shows 2-note append + `ends_with('\n')`. |
| 22 | orchestration/run_state.rs | `RunState::add_tokens` | include | Method; §3.1. Fence shows cumulative add. |
| 23 | orchestration/run_state.rs | `RunState::record_agent_spawn` | include | Method; §3.1. Fence shows counter increments. |
| 24 | orchestration/run_state.rs | `RunState::snapshot` | include | Method returning snapshot; §3.1 non-trivial return. Fence checks `tokens_used` + `status`. |
| 25 | orchestration/skill_resolve.rs | `SkillSpawnRequest` (struct) | include | Result struct; §3.1. Fence shows field access. |
| 26 | orchestration/skill_resolve.rs | `SkillSpawnArgs` (struct) | include | Input struct with optional fields; §3.1. Fence constructs + asserts fields. |
| 27 | orchestration/skill_resolve.rs | `substitute_skill_dir` | include | Free function; §3.1. Fence shows `${SKILL_DIR}` substitution. |
| 28 | orchestration/skill_resolve.rs | `apply_scope_paths` | include | Free function returning `Result`; §3.1. Fence shows narrowing + error path. |
| 29 | orchestration/task_list.rs | `TaskList` (struct) | include | Core task store; §3.1. Fence constructs + asserts empty. |
| 30 | orchestration/task_list.rs | `TaskList::new` | include | Constructor; §3.1. |
| 31 | orchestration/task_list.rs | `TaskList::create` | include | Method with 5 params; §3.1. Fence shows hierarchical id allocation. |
| 32 | orchestration/task_list.rs | `TaskList::claim` | include | CAS claim returning `Result`; §3.1. Fence shows success + `TaskLocked` failure. |
| 33 | orchestration/task_list.rs | `TaskList::heartbeat` | include | Method returning `Result`; §3.1. Fence shows lease extension. |
| 34 | orchestration/task_list.rs | `TaskList::release` | include | Method returning `Result`; §3.1. Fence shows status → Pending. |
| 35 | orchestration/task_list.rs | `TaskList::update` | include | Method returning `Result`; §3.1. Fence shows InProgress transition. |
| 36 | orchestration/task_list.rs | `TaskList::complete` | include | Method returning `Result`; §3.1. Fence shows Done + result_summary. |
| 37 | orchestration/task_list.rs | `TaskList::fail` | include | Method returning `Result`; §3.1. Fence shows Failed + error field. |
| 38 | orchestration/task_list.rs | `TaskList::discard` | include | Method returning `Result`; §3.1. Fence shows Discarded state. |
| 39 | orchestration/task_list.rs | `TaskList::list` | include | Query method; §3.1 non-trivial params. Fence shows filter by status. |
| 40 | orchestration/task_list.rs | `TaskList::expire_leases` | include | Sweep method; §3.1. Fence uses fixed timestamps to deterministically test expiry. |
| 41 | orchestration/task_list.rs | `TaskList::all_terminal` | include | Predicate; §3.1. Fence shows vacuous true, pending false, done true. |
| 42 | orchestration/trace.rs | `TraceStream` (struct) | include | Fan-out stream; §3.1. Fence shows `new` + `subscribe` + `subscriber_count`. |
| 43 | orchestration/trace.rs | `TraceStream::new` | include | Constructor; §3.1. Fence asserts `subscriber_count() == 0`. |
| 44 | orchestration/trace.rs | `TraceStream::subscribe` | include | Method; §3.1. Fence shows two-subscriber count. |
| 45 | orchestration/trace.rs | `TraceStream::emit` | include | Fan-out method; §3.1. Fence uses `try_recv()` to verify delivery. |
| 46 | orchestration/virtual_tools.rs | `is_virtual` | include | Free function; §3.1. Fence checks `task.*`, `agent.*.spawn`, `skill.*.spawn`, negative cases. |
| 47 | orchestration/virtual_tools.rs | `required_capability` | include | Free function; §3.1. Fence checks `task.list` → read, `task.create` → write, `task.discard` → manage. |
| 48 | orchestration/virtual_tools.rs | `dispatch` | include | Free function returning `Result`; §3.1. Fence shows `task.create` + `run.note` + `run.plan`. |
| 49 | orchestration/virtual_tools.rs | `check_capability_subset` | include | Free function returning `Result`; §3.1. Fence shows subset allowed + extras rejected. |
| 50 | orchestration/virtual_tools.rs | `AgentSpawnRequest` (struct) | include | Result struct; §3.1. Fence constructs via `validate_agent_spawn`. |
| 51 | orchestration/virtual_tools.rs | `validate_agent_spawn` | include | Free function returning `Result`; §3.1. Fence shows authorized success + unauthorized failure. |
| 52 | orchestration/virtual_tools.rs | `validate_skill_spawn` | skip-trivial | Requires real `Scope` object (filesystem-backed); dispatches to `resolve_skill_for_spawn` which is already tested in integration tests. No meaningful stub. |
| 53 | sandbox/passthrough.rs | `PassthroughSandbox` (struct) | include | No-isolation adapter; §3.1. Fence constructs + asserts `name() == "passthrough"`. |
| 54 | sandbox/passthrough.rs | `PassthroughSandbox::new` | include | Constructor; §3.1. Fence calls `validate_plan` with empty plan → Ok. |
| 55 | sandbox/registry.rs | `PlatformSet::includes` | include | Method with `&str` param; §3.1. Fence checks All, LinuxAndDarwin, LinuxOnly. |
| 56 | sandbox/registry.rs | `detect_platform` | include | Free function; §3.1. Fence asserts known values. |
| 57 | sandbox/registry.rs | `RegistryKind::name` | include | Method; §3.1. Fence asserts all 3 names. |
| 58 | sandbox/resolution_error.rs | `ResolutionRejection` (enum) | include | Error enum; §3.1. Fence shows `PlatformMismatch` + `ProbeUnavailable` Display. |
| 59 | sandbox/resolution_error.rs | `ResolutionError` (enum) | include | Error enum with associated data; §3.1. Fence constructs `NoAdapterMatches` + asserts Display content. |
| 60 | sandbox/resolver.rs | `SandboxAdapter::name` | include | Dispatch method; §3.1. Fence wraps `PassthroughSandbox` + asserts name. |
| 61 | sandbox/target_match.rs | `kind_to_family` | include | Free function; §3.1. Fence checks 3 variants. |
| 62 | sandbox/target_match.rs | `adapter_satisfies` | include | Free function returning bool; §3.1. Fence checks passthrough + linux-native-strict. |
| 63 | sandbox/target_match.rs | `registration_for_triple` | include | Free function returning `Option`; §3.1. Fence shows Some + None (Reserved triple). |
| 64 | sandbox/validation.rs | `SandboxValidationError::new` | include | Constructor (`#[non_exhaustive]` struct); §3.1. Fence constructs + asserts `plan_id` + `to_string()`. |
| 65 | sandbox/validation.rs | `validate_plan_against_adapter` | include | Free function returning `Result<(), Vec<_>>`; §3.1. Fence shows fs.read passes, empty plan passes. |
| 66 | plugin_host/mod.rs | `RecordingSink` (enum) | include | Enum with path variant; §3.1. Fence constructs `JsonlFile` + asserts `matches!`. |
| 67 | plugin_host/mod.rs | `PluginHostOptions` (struct) | include | `#[non_exhaustive]`; §3.1. Fence converts `rust,ignore` to executable: `default()` + field mutation + assertions. |
| 68 | plugin_host/recording.rs | `Direction` (enum) | include | Direction enum; §3.1. Fence constructs both variants + asserts `matches!`. |
| 69 | plugin_host/recording.rs | `Recorder::new` | include | Constructor; §3.1. Fence asserts `{:?}` contains plugin name. |
| 70 | options.rs | `TokenUsage` | done | Fence at line 12 (shows `default()` + field assertions). |
| 71 | options.rs | `RunOptions` | done | Fence at line 38 (shows `default()` + `max_turns` + `trace_label` mutation). |
| 72 | outcome.rs | `RunOutcome` | done | Fence at line 24 (shows pattern-match helper with `_ => {}` catch-all). |
| 73 | builder.rs | `BuildError` | done | Fence at line 54 (shows `builder().build().unwrap_err()` → `NoLlmBackend`). |
| 74 | run.rs | `Runtime::run_streaming` | done | Fence at line ~xxx (round-2 fixture; `tokio_test::block_on` + `MockLlmBackend` + stream collect). |
| 75 | run.rs | `Runtime::run_streaming_with_history` | done | Fence at line ~xxx (round-2 fixture; `tokio_test::block_on` + history replay). |
| 76 | plugin_host/mod.rs | `__internals` (mod) | skip-feature-gated | Items require `test-support` feature or are `#[doc(hidden)]`. |
| 77 | plugin_host | `IpcLlmBackend`, `IpcStorage`, `IpcTool`, `PluginProcess`, `DynAsyncWriter` | skip-feature-gated | Behind `test-support` feature. |
| 78 | plugin_host | `drive_handshake` | skip-feature-gated | `__internals` re-export only; no public path. |
| 79 | capability_override | `CapabilityOverride`, `EffectiveCapability`, `OverrideExpandError`, `compute_effective` | skip-reexport | Pure shim re-exporting from `tau_pkg::capability_override`. |
| 80 | lib.rs:19 | `pub mod builder` | skip-trivial | Module declaration; no doctest surface. |
| 81 | lib.rs:21 | `pub mod capability_override` | skip-trivial | Module declaration; no doctest surface. |
| 82 | lib.rs:23 | `pub mod error` | skip-trivial | Module declaration; no doctest surface. |
| 83 | lib.rs:24 | `pub mod options` | skip-trivial | Module declaration; no doctest surface. |
| 84 | lib.rs:25 | `pub mod orchestration` | skip-trivial | Module declaration; no doctest surface. |
| 85 | lib.rs:26 | `pub mod outcome` | skip-trivial | Module declaration; no doctest surface. |
| 86 | lib.rs:27 | `pub mod plugin_host` | skip-trivial | Module declaration; no doctest surface. |
| 87 | lib.rs:29 | `pub mod sandbox` | skip-trivial | Module declaration; no doctest surface. |
| 88 | lib.rs:30 | `pub mod stream` | skip-trivial | Module declaration; no doctest surface. |
| 89 | builder.rs:59 | `pub trait DynLlmBackend` | skip-marker | Internal object-safety wrapper trait; re-implemented via blanket impl; users implement `LlmBackend` from tau-ports, not this trait. |
| 90 | builder.rs:100 | `pub trait DynTool` | skip-marker | Internal object-safety wrapper for `Tool`; same reasoning as `DynLlmBackend`. |
| 91 | builder.rs:163 | `pub trait DynStorage` | skip-marker | Internal object-safety wrapper for `Storage`; same reasoning. |
| 92 | builder.rs:244 | `pub trait DynSandbox` | skip-marker | Internal object-safety wrapper for `Sandbox`; same reasoning. |
| 93 | builder.rs:314 | `pub struct Runtime` | skip-trivial | Struct itself has no public fields; constructed exclusively via `Runtime::builder()`; covered by rows 1–9 fences. |
| 94 | error.rs:32 | `pub enum PluginKind` | skip-display | Simple tag enum; Display impl tested via `HandshakeFailureReason` + `RuntimeError` fences; no standalone behavior to demonstrate beyond `format!("{}", PluginKind::Tool)`. |
| 95 | error.rs:197 | `pub enum RuntimeError` | skip-trivial | Error variants carry `std::io::Error` / `ExitStatus` — not constructible in a doctest without real plugin infrastructure; `CapabilityDenial` (row 10) and `HandshakeFailureReason` (row 11) cover the constructible error-surface. |
| 96 | orchestration/mod.rs:19 | `pub mod budget` | skip-trivial | Module declaration; no doctest surface. |
| 97 | orchestration/mod.rs:20 | `pub mod error` | skip-trivial | Module declaration; no doctest surface. |
| 98 | orchestration/mod.rs:21 | `pub mod persistence` | skip-trivial | Module declaration; no doctest surface. |
| 99 | orchestration/mod.rs:22 | `pub mod run_state` | skip-trivial | Module declaration; no doctest surface. |
| 100 | orchestration/mod.rs:23 | `pub mod skill_resolve` | skip-trivial | Module declaration; no doctest surface. |
| 101 | orchestration/mod.rs:24 | `pub mod task_list` | skip-trivial | Module declaration; no doctest surface. |
| 102 | orchestration/mod.rs:25 | `pub mod trace` | skip-trivial | Module declaration; no doctest surface. |
| 103 | orchestration/mod.rs:26 | `pub mod virtual_tools` | skip-trivial | Module declaration; no doctest surface. |
| 104 | orchestration/task_list.rs:11 | `pub const DEFAULT_LEASE: Duration` | skip-trivial | Duration constant (5 min); value documented in prose; no behavior to demonstrate. |
| 105 | orchestration/trace.rs:17 | `pub type TraceSubscriber` | skip-alias | `pub type X = Y`; alias for `mpsc::UnboundedSender<TraceEvent>`; exercised by `TraceStream::subscribe` fence (row 44). |
| 106 | orchestration/skill_resolve.rs:282 | `pub fn resolve_skill_for_spawn` | skip-needs-fixture | Requires a real filesystem-backed `Scope` (reads installed skill directory); deferred to future round. |
| 107 | sandbox/mod.rs:3 | `pub mod passthrough` | skip-trivial | Module declaration; no doctest surface. |
| 108 | sandbox/mod.rs:5 | `pub mod registry` | skip-trivial | Module declaration; no doctest surface. |
| 109 | sandbox/mod.rs:6 | `pub mod resolution_error` | skip-trivial | Module declaration; no doctest surface. |
| 110 | sandbox/mod.rs:7 | `pub mod resolver` | skip-trivial | Module declaration; no doctest surface. |
| 111 | sandbox/mod.rs:18 | `pub mod target_match` | skip-trivial | Module declaration; no doctest surface. |
| 112 | sandbox/registry.rs:149 | `pub struct AdapterRegistration` | skip-trivial | No public constructor; instances exist only as static registry entries (`REGISTRY`); all fields are readable but the struct is only meaningful in context of the registry; covered by `PlatformSet::includes` (row 55) and `RegistryKind::name` (row 57) fences. |
| 113 | sandbox/plan.rs:25 | `pub fn build_plan(...)` | include | Free function returning `Result`; §3.1. Fence constructs minimal `Capability` slice + empty override, asserts `plan.capabilities.len()`. |
| 114 | sandbox/resolver.rs:284 | `pub fn instantiate_for_probe(kind)` | skip-needs-fixture | Probes the real sandbox adapter stack (feature-dependent: NativeSandbox on Linux, DarwinSandbox on macOS, WindowsSandbox on Windows); result varies by OS and privilege; deferred to future round. |
| 115 | sandbox/resolver.rs:332 | `pub async fn resolve_adapter(...)` | skip-needs-fixture | Reads `TAU_TESTING_ALLOW_MOCK_SANDBOX` env var + invokes the full filter pipeline; requires real platform detection and optional sandbox probes; deferred to future round. |
| 116 | sandbox/resolver.rs:520 | `pub async fn resolve_strict_for_validation()` | skip-needs-fixture | Calls `resolve_adapter` with a strict-tier requirements object; same OS/fixture dependency; deferred. |
| 117 | sandbox/resolver.rs:588 | `pub async fn resolve_adapter_forced(kind)` | skip-needs-fixture | Bypasses the filter pipeline but still instantiates the real adapter; same fixture dependency; deferred. |
| 118 | orchestration/persistence.rs:76 | `pub fn spawn_writer(path, rx)` | skip-needs-fixture | Spawns a tokio task writing to a real filesystem path; requires a running tokio runtime + tempdir; comprehensive `#[tokio::test]` coverage already in `persistence.rs`. |
| 119 | orchestration/persistence.rs:123 | `pub async fn replay(path)` | skip-needs-fixture | Reads from a real filesystem JSONL path; requires a pre-written file and a running tokio runtime; comprehensive `#[tokio::test]` coverage already in `persistence.rs`. |

## tau-pkg

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | bundle/error.rs:5 | `pub enum BundleParseError` | include | Error enum with constructible variant `UnsupportedSchemaVersion { found }`; fence shows `format!("{err}")` contains "unsupported". |
| 2 | bundle/error.rs:19 | `pub enum BundleIoError` | skip-needs-fixture | `Read` variant requires a real `std::io::Error` from a real filesystem; `Parse` variant wraps `BundleParseError` (covered by row 1). |
| 3 | bundle/error.rs:35 | `pub enum BundleIntegrityError` | include | Both variants constructible without fixtures; fence shows `HashFieldEmpty` + `HashMismatch { claimed, computed }` + `format!` assertions. |
| 4 | bundle/canonical.rs:18 | `pub fn to_canonical_toml(manifest)` | skip-trivial | Accessed via `BundleManifest::to_canonical_toml` (row 12 fence). |
| 5 | bundle/hash.rs:11 | `pub fn compute_self_hash(manifest)` | skip-trivial | Accessed via `BundleManifest::compute_self_hash` (row 13 fence). |
| 6 | bundle/hash.rs:22 | `pub fn verify_self_hash(manifest)` | skip-trivial | Accessed via `BundleManifest::verify_self_hash` (row 14 fence). |
| 7 | bundle/manifest.rs:14 | `pub struct BundleManifest` | skip-trivial | Not `#[non_exhaustive]`; struct-literal construction works; but construction requires all sub-structs — covered by `parse_str` (row 11) and the canonical TOML fences below. |
| 8 | bundle/manifest.rs:32 | `pub struct BundleMeta` | skip-trivial | Plain struct; no constructor; all fields covered by round-trip tests. |
| 9 | bundle/manifest.rs:47 | `pub struct ProjectInfo` | skip-trivial | Plain struct; no constructor. |
| 10 | bundle/manifest.rs:58 | `pub struct BundlePackage` | skip-trivial | Plain struct; no constructor. |
| 11 | bundle/manifest.rs:77 | `pub struct BundleAgent` | skip-trivial | Plain struct; no constructor. |
| 12 | bundle/manifest.rs:95 | `pub struct BackendRef` | skip-trivial | Plain struct; no constructor; exercised by `parse_str` fence. |
| 13 | bundle/manifest.rs:110 | `pub struct BundleEffectiveCapabilities` | skip-trivial | `Default`; `is_empty` fence covers it (row 17). |
| 14 | bundle/manifest.rs:161 | `BundleManifest::parse_str(s)` | include | Pure TOML parse; fence shows minimal TOML → asserts `project.name` + `schema_version`. |
| 15 | bundle/manifest.rs:172 | `BundleManifest::from_path(p)` | skip-needs-fixture | Reads a real file; deferred. |
| 16 | bundle/manifest.rs:185 | `BundleManifest::to_canonical_toml(&self)` | include | Pure serialization; fence asserts `canonical` contains `schema_version = 1`, `[bundle]`, `[project]`. |
| 17 | bundle/manifest.rs:191 | `BundleManifest::compute_self_hash(&self)` | include | Pure, no FS; fence asserts 64 hex chars. |
| 18 | bundle/manifest.rs:198 | `BundleManifest::verify_self_hash(&self)` | include | Pure, no FS; fence sets correct hash then tampers it; asserts `Ok`/`Err`. |
| 19 | bundle/manifest.rs:145 | `BundleEffectiveCapabilities::is_empty(&self)` | include | Pure predicate; fence shows `Default::default()` → `is_empty()` true, push path → false. |
| 20 | capability_override/mod.rs:22 | `pub struct CapabilityOverride` | include | `#[non_exhaustive]`; fence uses `::new()` constructor; asserts `kind`, `allow`, `deny` fields. |
| 21 | capability_override/mod.rs:57 | `pub struct EffectiveCapability` | skip-trivial | `#[non_exhaustive]`; no constructor; result type of `compute_effective` only; covered by `compute_effective` fence (row 23). |
| 22 | capability_override/mod.rs:73 | `pub struct OverrideExpandError` | include | Has public fields + `Display`; fence constructs directly and asserts display contains "expands package grant". |
| 23 | capability_override/mod.rs:96 | `pub fn compute_effective(package_caps, project_override)` | include | Pure; fence calls with `fs.read` Capability from serde_json, empty override, asserts `len == 1`. |
| 24 | error.rs:27 | `pub enum ScopeError` | include | Fence constructs `HomeNotFound` + asserts display contains "HOME". |
| 25 | error.rs:75 | `pub enum GitError` | include | Fence constructs `GitMissing` + asserts display contains "git". |
| 26 | error.rs:106 | `pub enum ManifestReadError` | include | Fence constructs `NotFound { path }` + asserts display. |
| 27 | error.rs:133 | `pub enum RegistryError` | include | Fence constructs `Parse { reason }` + asserts display. |
| 28 | error.rs:178 | `pub enum InstallError` | include | Fence shows `GitError → InstallError` via `From`; asserts `matches!` + display. |
| 29 | error.rs:306 | `pub enum UninstallError` | include | Fence constructs `NotInstalled { name }` + asserts display. |
| 30 | install.rs:69 | `pub struct BuildOptions` | done | Fence at line 57 (round-3 prior art); shows `Default` + field mutation. |
| 31 | install.rs:88 | `BuildOptions::new()` | include | Fence asserts `!skip_build`, `cargo_path.is_none()`, `extra_args.is_empty()`. |
| 32 | install.rs:96 | `pub struct InstallOptions` | skip-trivial | `Default`; `block_on_lock`, `force`, `build`, `skip_cross_check` — covered by prose and `install_with_options` doctest. |
| 33 | install.rs:134 | `pub struct InstalledPackage` | skip-trivial | No constructor; only produced by `install`; covered by `install` fence (no_run). |
| 34 | install.rs:164 | `pub fn install(source, scope)` | done | Fence at line 152 (`no_run`; shells out to `git clone`). |
| 35 | install.rs:170 | `pub fn install_with_options(source, scope, options)` | skip-needs-fixture | Same as `install`; shells out to `git clone`. |
| 36 | install.rs:779 | `pub fn uninstall(name, version, scope)` | skip-needs-fixture | Mutates real filesystem; shells out to `git clone` + lockfile update; deferred. |
| 37 | lockfile.rs:63 | `pub const MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION: u32` | skip-trivial | Numeric constant; value documented in inline comment; no behavior to demonstrate. |
| 38 | lockfile.rs:82 | `pub struct LockFile` | done | Fence at line 72 (round-3 prior art); shows `Default` + `schema_version == 6`. |
| 39 | lockfile.rs:120 | `pub enum SynthesizedSource` | include | Fence constructs `Anthropic` + asserts `matches!`. |
| 40 | lockfile.rs:157 | `pub struct LockedPackage` | done | Fence at line 144 (round-3 prior art); shows TOML round-trip + `find` result. |
| 41 | lockfile.rs:232 | `pub struct LockedPlugin` | done | Fence at line 216 (round-3 prior art); shows `LockedPlugin::new` + field assertions. |
| 42 | lockfile.rs:292 | `pub struct SkillFrontmatterSnapshot` | include | `#[non_exhaustive]`; added `::new()` constructor; fence uses it, asserts `name`, `description`. |
| 43 | lockfile.rs:315 | `pub struct LockedSkill` | skip-trivial | `#[non_exhaustive]`; no standalone use; covered by `LockedSkill::new` (row 44). |
| 44 | lockfile.rs (impl) | `LockedSkill::new(sha256, frontmatter)` | include | Fence uses `SkillFrontmatterSnapshot::new` then `LockedSkill::new`; asserts `content_sha256` + `frontmatter.name`. |
| 45 | lockfile.rs:376 | `pub struct LockedVersion` | done | Fence at line 387 (round-3 prior art); shows TOML round-trip obtain + field assertion. |
| 46 | lockfile.rs (impl) | `LockFile::load(path)` | done | Fence at line 502; shows non-existent path → empty default. |
| 47 | lockfile.rs (impl) | `LockFile::from_toml_str(text)` | include | Fence parses inline TOML string with `[[package]]`; asserts `packages.len() == 1`. |
| 48 | lockfile.rs (impl) | `LockFile::save(path)` | done | Fence at line 649. |
| 49 | lockfile.rs (impl) | `LockFile::find(name)` | done | Fence at line 700. |
| 50 | lockfile.rs (impl) | `LockFile::upsert(pkg)` | done | Fence at line 721. |
| 51 | lockfile.rs (impl) | `LockFile::remove(name, version)` | done | Fence at line 759. |
| 52 | manifest.rs:59 | `pub fn read_manifest(path)` | done | Fence at line 41 (round-3 prior art); shows `read_manifest` on a tempdir-written file. |
| 53 | project/agent.rs:27 | `pub enum AgentResolutionError` | skip-needs-fixture | Requires real scope + lockfile + manifest; only returned by `build_agent_definition`. |
| 54 | project/agent.rs:149 | `pub fn build_agent_definition(...)` | skip-needs-fixture | Reads real lockfile + manifest from scope; deferred. |
| 55 | project/project.rs:13 | `pub struct UncheckedProjectConfig` | skip-trivial | Raw serde shape; no semantic behavior; covered by `validate` fence (row 60). |
| 56 | project/project.rs:23 | `pub struct UncheckedProject` | skip-trivial | Plain serde struct. |
| 57 | project/project.rs:33 | `pub struct UncheckedAgent` | skip-trivial | Plain serde struct. |
| 58 | project/project.rs:58 | `pub struct UncheckedRequires` | skip-trivial | Plain serde struct. |
| 59 | project/project.rs:78 | `pub struct UncheckedRequiredTool` | skip-trivial | `#[non_exhaustive]` serde struct; no standalone use beyond `validate`. |
| 60 | project/project.rs:91 | `pub struct UncheckedPrompt` | skip-trivial | Plain serde struct. |
| 61 | project/project.rs:113 | `pub struct UncheckedCapabilityOverride` | skip-trivial | `#[serde(deny_unknown_fields)]` serde struct; covered by `validate` tests. |
| 62 | project/project.rs:147 | `pub struct ProjectConfig` | skip-trivial | `#[non_exhaustive]`; no constructor; produced by `validate`; covered by `validate` fence (row 65). |
| 63 | project/project.rs:159 | `pub struct AgentEntry` | include | `#[non_exhaustive]`; `::new()` constructor; fence calls it with all 8 args; asserts `id`, `display_name`. |
| 64 | project/project.rs:213 | `pub struct RequiresEntry` | skip-trivial | `#[non_exhaustive]`; `Default` produces `tools = []`; no standalone behavior. |
| 65 | project/project.rs:224 | `pub enum PromptEntry` | include | Fence shows all 3 variants (`None`, `Inline`, `File`) with `matches!` assertions. |
| 66 | project/project.rs:239 | `pub enum ProjectConfigError` | include | Fence constructs `EmptyProjectName` + `AgentValidation { id, message }`; asserts display strings. |
| 67 | project/project.rs (impl) | `UncheckedProjectConfig::validate(self)` | include | Fence parses minimal TOML, calls `validate()`, asserts `project_name`. |
| 68 | project/project.rs (impl) | `ProjectConfig::from_path(path)` | skip-needs-fixture | Reads a real file; deferred. |
| 69 | registry.rs:33 | `pub fn list(scope)` | done | Fence at line 25 (round-3 prior art). |
| 70 | registry.rs:54 | `pub fn get(scope, name)` | done | Fence at line 45 (round-3 prior art). |
| 71 | resolve.rs:33 | `pub struct RequiredTool` | include | `#[non_exhaustive]`; fence calls `::new(name, source, version_req)`; asserts `name.as_str()`. |
| 72 | resolve.rs:57 | `pub struct ResolutionPlan` | include | `Default`; fence asserts `installs.is_empty()`, `reuses.is_empty()`. |
| 73 | resolve.rs:67 | `pub struct PlannedInstall` | skip-trivial | `#[non_exhaustive]`; no constructor; only produced by `resolve_requires_tools`. |
| 74 | resolve.rs:81 | `pub struct ReusedInstall` | skip-trivial | `#[non_exhaustive]`; no constructor; only produced by `resolve_requires_tools`. |
| 75 | resolve.rs:91 | `pub enum ResolveError` | include | Fence constructs `Registry(RegistryError::Parse { … })`; asserts display. |
| 76 | resolve.rs:136 | `pub fn resolve_requires_tools(requires, scope)` | skip-needs-fixture | Shells out to `git ls-remote`; deferred. |
| 77 | sandbox_check.rs:39 | `pub enum CrossCheckError` | include | Fence constructs `SpawnFailed` + `HandshakeFailed`; asserts display. |
| 78 | sandbox_check.rs:89 | `pub async fn cross_check_plugin_capabilities(binary, manifest)` | skip-needs-fixture | Spawns a real plugin binary; deferred. |
| 79 | scope.rs:28 | `pub const MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION: u32` | skip-trivial | Numeric constant. |
| 80 | scope.rs:37 | `pub enum ScopeKind` | skip-trivial | Simple tag enum; covered by `ScopeConfig::read_from_str` fence. |
| 81 | scope.rs:53 | `pub struct SandboxRequirements` | done | Fence added in round-3 (prior session); shows `with_tier` + field assertions. |
| 82 | scope.rs:85 | `pub enum SandboxRequiredTier` | done | Fence added in round-3 (prior session); shows ordering + `max()`. |
| 83 | scope.rs:195 | `pub struct ScopeConfig` | done | Fence at line 207 (round-3 prior art). |
| 84 | scope.rs (impl) | `ScopeConfig::read_from_str(s)` | done | Fence added in round-3 (prior session); parses TOML string. |
| 85 | scope.rs:272 | `pub struct Scope` | done | Fence at line 302 (round-3 prior art). |
| 86 | scope.rs (impl) | `Scope::resolve(dir)` | done | Fence at line 336. |
| 87 | scope.rs (impl) | `Scope::global()` | done | Fence at line 370. |
| 88 | scope.rs (impl) | `Scope::new_project(dir)` | done | Fence at line 448. |
| 89 | scope.rs:78 | `SandboxRequirements::with_tier(required_tier)` | done | Covered by `SandboxRequirements` struct fence (row 81); shows `with_tier` + field assertions. |
| 90 | scope.rs:242 | `ScopeConfig::new(kind)` | include | Fence added in PR-E spec-review fix; asserts `kind`, `schema_version == 3`, `defaults.is_empty()`. |
| 91 | scope.rs:290 | `ScopeConfig::to_toml_string(&self)` | include | Fence added in PR-E spec-review fix; asserts TOML contains `schema_version` + `kind = "global"`. |
| 92 | scope.rs:497 | `Scope::path(&self)` | skip-getter | Trivial accessor returning `&Path`; no behavior to demonstrate beyond what `new_project` / `global` fences already show. |
| 93 | scope.rs:502 | `Scope::state_path(&self)` | skip-getter | Trivial accessor returning `&Path`. |
| 94 | scope.rs:507 | `Scope::kind(&self)` | skip-getter | Trivial accessor returning `ScopeKind`; already asserted in `Scope::resolve` + `new_project` fences. |
| 95 | scope.rs:512 | `Scope::lockfile_path(&self)` | skip-getter | Path derivation (`<path>/tau-lock.toml`); trivial accessor. |
| 96 | scope.rs:517 | `Scope::config_path(&self)` | skip-getter | Path derivation (`<state_path>/config.toml`); trivial accessor. |
| 97 | scope.rs:522 | `Scope::packages_dir(&self)` | skip-getter | Path derivation (`<state_path>/packages`); trivial accessor. |
| 98 | scope.rs:527 | `Scope::install_lock_path(&self)` | skip-getter | Path derivation (`<state_path>/locks/install.lock`); trivial accessor. |
| 99 | scope.rs:533 | `Scope::package_dir(&self, name, version)` | include | Fence added in PR-E spec-review fix; 2-param method; asserts path starts with tmp root and contains name + version segments. |
| 100 | skill_check.rs:41 | `pub fn cross_check_skill_package(...)` | skip-needs-fixture | Reads real SKILL.md from installed directory; deferred. |
| 101 | skill_resolve.rs:18 | `pub struct InstalledSkill` | skip-trivial | No constructor; only produced by `find_installed_skill`. |
| 102 | skill_resolve.rs:39 | `pub enum FindSkillError` | include | Fence constructs `InstallPathMissing { name, path }`; asserts display. |
| 103 | skill_resolve.rs:91 | `pub fn find_installed_skill(scope, name)` | skip-needs-fixture | Reads real lockfile + manifest from scope; deferred. |
| 104 | source_list.rs:23 | `pub fn list_versions_at_source(source)` | skip-needs-fixture | Shells out to `git ls-remote`; deferred. |
| 105 | source_list.rs:122 | `pub enum SourceListError` | include | Fence constructs `GitInvoke { message }` + `Unsupported`; asserts display. |
| 106 | synthesize.rs:23 | `pub fn synthesize_anthropic_skill(workspace, source)` | skip-needs-fixture | Reads real SKILL.md from cloned workspace; deferred. |
| 107 | synthesize.rs:51 | `pub enum SynthesizeError` | include | Fence constructs `ReadSkillMd { path, detail }`; asserts display. |
| 108 | tree_hash.rs:35 | `pub enum TreeHashError` | include | Fence constructs `Io { path, message }`; asserts display contains "io error at" + path. |
| 109 | tree_hash.rs:50 | `pub fn sha256_of_file(path)` | include | Fence writes tempfile, calls `sha256_of_file`, asserts 64-char hex. |
| 110 | tree_hash.rs:63 | `pub struct FileHash` | include | `#[non_exhaustive]`; no public constructor; fence illustrates round-trip concept via `sha256_of_file`. |
| 111 | tree_hash.rs:94 | `pub fn tree_hash(root)` | done | Fence at line 127 (round-3 prior art). |
| 112 | update.rs:41 | `pub enum UpdateError` | done | Fence at line 28 (round-3 prior art). |
| 113 | update.rs:113 | `pub struct UpdateResult` | done | Fence at line 101 (round-3 prior art). |
| 114 | update.rs:145 | `pub fn update_package(name, version, scope)` | skip-needs-fixture | Shells out to `git clone`; deferred. |
| 115 | verify.rs:18 | `pub enum VerifyStatus` | include | Fence constructs `Ok`, `Unverified`, `TreeDrift`; asserts `is_drift()`. |
| 116 | verify.rs:74 | `pub enum AnthropicConformanceIssue` | include | Fence constructs `MissingDescription` + `MalformedFrontmatter { detail }`; asserts `matches!`. |
| 117 | verify.rs (impl) | `VerifyStatus::is_drift(&self)` | include | Fence shows `Ok`/`Unverified` → `false`, `Missing` → `true`. |
| 118 | verify.rs:106 | `pub struct VerifyReport` | skip-trivial | `#[non_exhaustive]`; no constructor; only produced by `verify`; covered by `VerifyStatus` + `VerifyError` fences. |
| 119 | verify.rs:118 | `pub enum VerifyError` | include | Fence constructs `PackageNotInstalled { name }`; asserts display. |
| 120 | verify.rs:163 | `pub fn verify_skill_content(install_dir, name, locked)` | include | Fence writes tempfile `SKILL.md`, uses `LockedSkill::new` with wrong sha; asserts `SkillContentDrift`. |
| 121 | verify.rs:199 | `pub fn verify(scope, name, version)` | skip-needs-fixture | Reads real lockfile + install dir; deferred. |
| 122 | verify.rs:288 | `pub fn verify_all(scope)` | skip-needs-fixture | Reads real lockfile; deferred. |
| 123 | verify.rs:305 | `pub fn verify_all_with_options(scope, anthropic_strict)` | skip-needs-fixture | Same as `verify_all`; deferred. |

## Status log

- 2026-05-26 — tau-plugin-protocol classifications + 2 includes (PR-A).
- 2026-05-26 — round-3 spec-review fixes: added missing impl-method rows (Frame::decode/encode, FramedReader/Writer methods, all TraceContext/HandshakeRequest/MethodSchema/HandshakeResponse ::new constructors, meta constants, all FakeStdioPeer methods); added skip-feature-gated category; relabeled FakeStdioPeer + methods from skip-trivial to skip-feature-gated; rewrote FramedReader/FramedWriter fences as duplex round-trips with .expect() assertions.
- 2026-05-26 — tau-plugin-sdk classifications + 14 includes (PR-B).
- 2026-05-26 — tau-domain classifications + 17 includes (PR-C).
- 2026-05-26 — PR-E spec-review fix: added 11 missing scope.rs rows + 3 new fences (ScopeConfig::new, ScopeConfig::to_toml_string, Scope::package_dir).
- 2026-05-26 — round-3 spec-review fixes (PR-C round-3): SynthesizeError reclassified from skip-feature-gated to include (+1 fence); added 23 missing impl-method rows (7 CapabilityShapeSet methods, 1 Capability::required_shape with new fence, 12 PackageManifest skip-getter rows, PackageId::new done, PluginManifest::new done, UncheckedManifest::validate done); tau-domain include count now 18.
- 2026-05-26 — tau-runtime classifications + 69 includes (PR-D); 74 doctests passing, 0 failed; added `CapabilityDenial::new()` constructor + re-exported `Direction` from plugin_host; fixed `SandboxValidationError` + `validate_plan_against_adapter` import paths; corrected `TraceEventKind::RunStarted` → `Turn` variant.
- 2026-05-26 — PR-D spec-review fix: (1) added 40 missing inventory rows (rows 80–119): lib.rs/orchestration/sandbox pub mod declarations → skip-trivial; DynLlmBackend/DynTool/DynStorage/DynSandbox → skip-marker; Runtime struct → skip-trivial; PluginKind/RuntimeError → skip-trivial; DEFAULT_LEASE → skip-trivial; TraceSubscriber → skip-alias; resolve_skill_for_spawn → skip-needs-fixture; AdapterRegistration → skip-trivial; build_plan → include (fence added); resolver async fns + persistence spawn_writer/replay → skip-needs-fixture; (2) BudgetWatchdog::new downgraded from include to skip-trivial (tautological assert removed); (3) added skip-needs-fixture category; 73 doctests passing, 0 failed.
- 2026-05-26 — tau-pkg classifications + 43 includes (PR-E); 66 doctests passing, 0 failed; added `SkillFrontmatterSnapshot::new()` constructor to fix `#[non_exhaustive]` struct-literal issue in external-crate doctests. Closes round 3.
