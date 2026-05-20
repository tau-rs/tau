# Target triple reference

Tau target triples identify a deployment surface for a tau bundle.
The canonical form is `<platform>-<adapter>-<tier>` (three
hyphen-separated segments) or `passthrough` (single-segment special).

Codified in [ADR-0034](../decisions/0034-target-triple-registry.md).

## Axes

| Axis | Variants |
|---|---|
| Platform | `linux`, `darwin`, `windows`, `any` |
| AdapterFamily | `native`, `container`, `remote`, `wasi`, `passthrough` |
| SandboxTier | `strict`, `light`, `none` |

## v1 triples

### Available (5)

| Triple | Required shapes | Notes |
|---|---|---|
| `linux-native-strict` | `fs.r`, `fs.w`, `exec`, `net.http` | Linux landlock + seccomp + namespaces. Best-effort default for Linux production. |
| `linux-native-light` | `fs.r`, `fs.w`, `exec`, `net.http` | Linux landlock only. No seccomp, no namespaces. Lower overhead for trusted plugins. |
| `linux-container-strict` | `fs.r`, `fs.w`, `exec`, `net.http` | Linux container (engine = `podman` or `docker`, set via `[sandbox.container].engine`). |
| `darwin-native-strict` | `fs.r`, `fs.w`, `exec`, `net.http` | macOS sandbox-exec. |
| `passthrough` | `fs.r`, `fs.w`, `exec`, `net.http`, `agent.spawn` | Explicit no-isolation. Universal opt-out. |

### Reserved (1 individual + 2 namespaces)

| Triple / namespace | Reason |
|---|---|
| `windows-native-strict` | `tau-sandbox-windows` is scaffold-only per ADR-0023; probe returns `Unavailable`. Triple parses + validates; the bundle does not yet run. |
| `linux-remote-*`, `darwin-remote-*`, `any-remote-*` | Remote sandbox adapter family is registered but no concrete provider has shipped. |
| `linux-wasi-*`, `any-wasi-*` | Wasi adapter family has no `RegistryKind` in v1; whole namespace reserved. |

## Inspecting the registry

```bash
tau target list             # Available triples
tau target list --all       # Available + Reserved
tau target show linux-native-strict   # full matrix for one triple
tau target show --json windows-native-strict
```

## Validating a project against a target

```bash
tau check --target linux-native-strict      # all categories, validate against target
tau check sandbox --target passthrough      # one category form
```

Validation rules:

- **Plugin shape ⊆ target shape**: a plugin declaring `agent.spawn`
  validated against `linux-native-strict` is an Error (the target
  doesn't enforce `agent.spawn` at the sandbox layer).
- **Project required_tier ≤ target tier**: a project asking for
  `Strict` validated against a Light target is an Error.
- **Local adapter availability**: if no locally registered adapter
  satisfies the target, a Warning is emitted (you can still validate
  statically; the bundle just won't run *here*).
- **Reserved triple**: validation runs against the documented matrix
  but emits a Warning that no shipping adapter exists.

## Stability

Triples shipped as Available are immutable. New triples are added
via ADR amendment + registry entry. See ADR-0034 §"Stability
discipline".
