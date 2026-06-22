# tau IR JSON Schema — conformance kit

`tau-ir.schema.json` is the published JSON Schema for tau's authoring/IR
contract (one of tau's two public contracts; see ADR-0055/0056). It is
**generated** from the `tau-ir` Rust types — never hand-edited. The
`ir_format` version is embedded in the schema's `title`/`$id`.

## Validate your generated IR

Frontend / SDK authors that emit tau IR should validate output against this
schema. The schema declares its JSON Schema draft in its `$schema` field —
use a validator for that draft. Example (Rust, `jsonschema` crate):

    let schema = serde_json::from_slice(include_bytes!("tau-ir.schema.json"))?;
    let v = jsonschema::options().with_draft(jsonschema::Draft::Draft7).build(&schema)?;
    assert!(v.is_valid(&your_ir_json));

Any validator matching the schema's declared draft works (ajv, jsonschema-py, etc.).

## Samples

`samples/*.json` are valid IR modules covering an agent + native tool, an
agent + MCP tool, a deterministic step, and a subflow. They are regenerated
by `cargo xtask gen-ir-schema` and are validated against the schema in CI.

## Regenerating (maintainers)

    cargo xtask gen-ir-schema   # rewrites the schema + samples from tau-ir
