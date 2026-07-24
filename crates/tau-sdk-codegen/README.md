# tau-sdk-codegen

Generates tau's typed authoring SDKs from the frozen IR JSON schema:

- `sdk/ts/`     — npm `@tau/sdk` (TypeScript)
- `sdk/python/` — PyPI `tau-sdk` (import `tau_sdk`)

The SDKs are *authoring front-ends*, not IR emitters: they produce the same
`ProjectConfig` the TOML surface parses to, so TOML / TS / Python all lower to
byte-identical canonical IR via the single Rust lowering pass (`tau-ir-lower`).

## Regenerate

    cargo run -p tau-sdk-codegen --bin gen

Then commit `sdk/`. The `drift` test fails if the committed packages diverge
from the emitters.

## Acceptance

`tests/byte_equal.rs` lowers one agent authored three ways and asserts
byte-equal canonical IR (Python via live `python3`; skipped when absent).
