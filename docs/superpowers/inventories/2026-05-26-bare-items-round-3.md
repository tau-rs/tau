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

## Status log

- 2026-05-26 — tau-plugin-protocol classifications + 2 includes (PR-A).
- 2026-05-26 — round-3 spec-review fixes: added missing impl-method rows (Frame::decode/encode, FramedReader/Writer methods, all TraceContext/HandshakeRequest/MethodSchema/HandshakeResponse ::new constructors, meta constants, all FakeStdioPeer methods); added skip-feature-gated category; relabeled FakeStdioPeer + methods from skip-trivial to skip-feature-gated; rewrote FramedReader/FramedWriter fences as duplex round-trips with .expect() assertions.
- 2026-05-26 — tau-plugin-sdk classifications + 14 includes (PR-B).
- 2026-05-26 — tau-domain classifications + 17 includes (PR-C).
- 2026-05-26 — round-3 spec-review fixes (PR-C round-3): SynthesizeError reclassified from skip-feature-gated to include (+1 fence); added 23 missing impl-method rows (7 CapabilityShapeSet methods, 1 Capability::required_shape with new fence, 12 PackageManifest skip-getter rows, PackageId::new done, PluginManifest::new done, UncheckedManifest::validate done); tau-domain include count now 18.
