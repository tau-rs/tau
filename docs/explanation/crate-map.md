# Crate map

A one-line "what does this crate do" reference for every workspace
crate, plus the dependency rules that keep the hexagonal architecture
honest.

For the *flow* through these crates, read [Architecture
overview](architecture-overview.md). For the package model the crates
serve, read [Packages](packages.md).

## The crates, by layer

### Kernel — stable public surfaces (G6)

| Crate | Purpose |
|---|---|
| `tau-domain` | Canonical data types: `PackageManifest`, `Capability`, `Message`, `Address`, `AgentDefinition`, `Value`. Pure types — no I/O, no runtime. Every other crate depends on this. |
| `tau-ports` | Trait definitions for plugin-replaceable boundaries: `LlmBackend`, `Tool`, `Storage`, `Sandbox`, `SessionContext`. Hex-arch port layer. |
| `tau-runtime` | The kernel. Capability check, plugin host, sandbox resolver, message routing, REPL persistence, multi-agent orchestration (ADR-0024), `Runtime::run_with_history`, `Runtime::invoke_tool`. |
| `tau-pkg` | Package manager. Manifest validation, scope resolution (global / project), lockfile, install / verify / update / uninstall, skill-check pipeline, transitive dependency resolution. |

### Plugin protocol & SDK

| Crate | Purpose |
|---|---|
| `tau-plugin-protocol` | The wire format: handshake messages, `tool.call` / `llm.complete` request/response shapes. Serialized as MessagePack over stdio (ADR-0008). |
| `tau-plugin-base` | Library plugins link against. Drives the handshake + message loop. A plugin author writes a `Tool` / `LlmBackend` impl; `tau-plugin-base` does the rest. |
| `tau-plugin-sdk` | Ergonomic authoring surface on top of `tau-plugin-base`. Adds `run_tool_with_config`, `Configure` trait, `SdkError`. The recommended entry point. |
| `tau-plugin-test-support` | Test fixtures for plugin authors: in-process spawn helpers, capability-grant builders, mock `SessionContext`. |
| `tau-plugin-compat` | The Layer 3 / Layer 4 plugin compatibility harness (ADR-0016 + ADR-0017). Drives the cross-check at install + the e2e tests at CI. |
| `tau-plugin-conformance` | Cross-platform conformance tests: a shipped plugin runs the same way on Linux / macOS / Windows (ADR-0016). |

### Sandbox adapters — additive per platform (ADR-0014)

| Crate | Provides |
|---|---|
| `tau-sandbox-native` | Linux: landlock V1 + seccomp BPF + user/network namespaces + per-command exec gating. |
| `tau-sandbox-darwin` | macOS: `sandbox-exec` profile generation; strict tier only (ADR-0022). |
| `tau-sandbox-windows` | Windows: AppContainer scaffold. Phase 1 probes `Unavailable`; Phase 2 deferred (ADR-0023). |
| `tau-sandbox-container` | Cross-platform fallback: per-plugin Docker / Podman images on top of `tau-plugin-base` (ADR-0021). |
| `tau-sandbox-proxy` | Userspace HTTPS-CONNECT proxy with SNI validation (ADR-0020). Shared by `tau-sandbox-native` and `tau-sandbox-darwin`. |

### Surface — what users interact with

| Crate | Purpose |
|---|---|
| `tau-cli` | The `tau` binary. Argument parsing (clap), command dispatch, project config loading, REPL (rustyline), human / JSON output rendering. |
| `tau-app` | Programmatic-embedding entry point. A parent app (web server, CI job, desktop app) imports `tau-app` and drives the runtime without going through the CLI. |
| `tau-workflow` | Linear pipeline runner (ADR-0022). `tau workflow {run, list, log, resume}` lives here. JSONL persistence + resume. |
| `tau-observe` | Tracing / observability hooks. Emits structured logs for the kernel's RunEvent stream + agent transitions. |

### Plugins (shipped reference implementations)

| Package | Kind | Purpose |
|---|---|---|
| `tau-plugins/anthropic` | `llm-backend` | Anthropic Claude Messages API. |
| `tau-plugins/openai` | `llm-backend` | OpenAI completions API. |
| `tau-plugins/ollama` | `llm-backend` | Local Ollama model. |
| `tau-plugins/fs-read` | `tool` | Read bytes from an allow-listed path. |
| `tau-plugins/shell` | `tool` | Spawn allow-listed commands with timeout + output cap. |
| `tau-plugins/echo-llm` | `llm-backend` | Test fixture: replays canned responses. |
| `tau-plugins/echo-tool` | `tool` | Test fixture: echoes inputs. |

### Test infra (not user-facing)

| Crate | Purpose |
|---|---|
| `landlock-exec-repro` | Diagnostic harness for the landlock `AccessFs::Execute` corner case (ADR-0017). Stays in-tree as a reproducer for future regressions. |

## Dependency rules

The hexagonal architecture forbids reverse edges between layers:

| From layer | May depend on | May NOT depend on |
|---|---|---|
| `tau-domain` | std | anything else in the workspace |
| `tau-ports` | `tau-domain` | runtime, plugin SDK, sandbox adapters |
| `tau-runtime` | `tau-domain`, `tau-ports`, `tau-pkg`, `tau-plugin-protocol`, sandbox adapter trait crates | `tau-cli`, `tau-app`, plugins themselves |
| `tau-pkg` | `tau-domain`, `tau-ports` | runtime, CLI |
| sandbox adapters | `tau-ports`, `tau-plugin-protocol`, `tau-sandbox-proxy` | runtime |
| plugin packages | `tau-plugin-sdk`, `tau-plugin-protocol`, `tau-domain` | runtime, CLI, other sandbox adapters |
| `tau-cli` | runtime, pkg, workflow, observe | plugins directly (always via runtime) |
| `tau-workflow` | runtime, domain | CLI |

If you find yourself wanting to import from a "forbidden" direction,
the answer is almost always to add an abstraction in the appropriate
port crate. ADR-0003 (tau-ports) is the canonical write-up of why.

## Where new code typically lands

| Need | Crate | Notes |
|---|---|---|
| New typed capability variant | `tau-domain::package::capability` | Then thread it through `tau-runtime::kernel::capability_check` + sandbox adapters that can enforce its `CapabilityShape`. |
| New CLI subcommand | `tau-cli::cli` (clap enum) + `tau-cli::cmd::<new>` | The dispatcher in `tau_cli::lib::run_main` is one match-arm wider. |
| New tool plugin | new package under `tau-plugins/<name>/` | Follow `fs-read` as the template (see [Write a tool plugin](../how-to/write-a-tool-plugin.md)). |
| New LLM-backend plugin | new package under `tau-plugins/<name>/` | Same pattern as tool plugin but implements `LlmBackend` trait + the `llm.*` wire calls. |
| New sandbox adapter | new `tau-sandbox-<target>` crate | Implement `tau_ports::Sandbox`. Register in `tau-runtime::sandbox::resolver::ADAPTER_REGISTRY`. |
| New ADR | `docs/decisions/NNNN-<slug>.md` | See [Propose an ADR](../how-to/propose-an-adr.md). |
| New workflow step kind | `tau-workflow::engine` step dispatch | Plus update `workflows/*.toml` schema docs. |

## See also

- [Architecture overview](architecture-overview.md) — the request
  flow that ties these crates together.
- [Write a tool plugin](../how-to/write-a-tool-plugin.md) — the
  hands-on entry point.
- [Testing strategy](testing-strategy.md) — where each crate's
  tests live.
- [ADR-0003](../decisions/0003-tau-ports.md) — original
  hex-arch port-crate motivation.
- [ADR-0006](../decisions/0006-tau-runtime.md) — `tau-runtime`'s
  design contract.
- [`CONTRIBUTING.md`](../../CONTRIBUTING.md) — workspace setup
  and PR mechanics.
