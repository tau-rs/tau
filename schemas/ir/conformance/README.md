# tau IR conformance kit

Validate any tool's generated IR against the published schema in any language:

1. Take the schema: `../tau-ir.v2.5.0.schema.json` (JSON Schema draft 2020-12).
2. Validate your generated `IrModule` JSON with any draft-2020-12 validator
   (Rust `jsonschema`, JS `ajv`, Python `jsonschema`, …).
3. `valid/*.json` are modules that MUST validate; `invalid/*.json` MUST be
   rejected. Run your validator over both sets to prove conformance.

The schema is generated from the `tau-ir` Rust serde types and is byte-stable
per `ir_format` version (see ADR-0056). Pin the version segment in the filename.
