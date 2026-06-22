# IR JSON Schema

tau publishes the authoring contract — the IR — as a JSON Schema generated from
the `tau-ir` serde types (ADR-0056). It is version-pinned by `ir_format`.

- **Schema:** [`schemas/ir/tau-ir.v2.3.0.schema.json`](https://github.com/tau-rs/tau/blob/main/schemas/ir/tau-ir.v2.3.0.schema.json)
- **`$id`:** `https://lebocqtitouan.github.io/tau/schemas/ir/v2.3.0/tau-ir.schema.json`
- **Draft:** JSON Schema 2020-12.

The schema is drift-tested byte-equal to a fresh regeneration, so it is provably
the serde types. Frontend / SDK authors validate generated IR against it; the
[conformance kit](https://github.com/tau-rs/tau/tree/main/schemas/ir/conformance)
ships `valid/` and `invalid/` samples for any-language conformance.
