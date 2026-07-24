# tau authoring SDKs (generated)

These packages are produced by `tau-sdk-codegen`, which loads and validates
the frozen IR JSON schema and pins the SDK surface against it. Do not edit by
hand — run `cargo run -p tau-sdk-codegen --bin gen`.

- `ts/`     — `@tau/sdk`   (`import { agent, models } from "tau"`)
- `python/` — `tau-sdk`    (`from tau_sdk import agent, models`)

Publishing to npm/PyPI is out of scope for EPIC 5.3.
