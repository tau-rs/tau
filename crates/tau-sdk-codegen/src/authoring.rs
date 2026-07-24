//! The authoring-surface descriptor: which factory has which fields, and how
//! each field maps into `tau.toml`. This is the composition the (post-lowering)
//! IR schema cannot describe; it mirrors what `tau-ts-extract` recognizes and
//! is pinned by the byte-equal conformance test.

/// The value shape of an authoring field (drives TS/Python type emission and
/// the Python TOML renderer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTy {
    /// A string scalar.
    Str,
    /// A boolean scalar.
    Bool,
    /// The `[models]` alias table: name -> { backend, model }.
    ModelMap,
    /// A list of tool symbolic names.
    ToolList,
}

/// One authoring field on a factory.
#[derive(Debug, Clone, Copy)]
pub struct AuthField {
    /// Field name in the SDK surface (TS/Python).
    pub sdk_name: &'static str,
    /// TOML key it lowers to.
    pub toml_key: &'static str,
    /// Value shape.
    pub ty: FieldTy,
    /// Whether the field is required.
    pub required: bool,
}

/// Where a factory writes in `tau.toml`.
#[derive(Debug, Clone, Copy)]
pub enum TomlTarget {
    /// A single `[name]` table (e.g. `[project]`).
    Table(&'static str),
    /// A map of named subtables `[name.<key>]` (e.g. `[models.<alias>]`,
    /// `[agents.<id>]`). The key is the model alias / agent id.
    KeyedTable(&'static str),
}

/// One authoring factory.
#[derive(Debug, Clone, Copy)]
pub struct Factory {
    /// Factory function name (`agent`, `models`, ...).
    pub name: &'static str,
    /// TOML target.
    pub target: TomlTarget,
    /// Fields, in emission order.
    pub fields: &'static [AuthField],
}

const AGENT_FIELDS: &[AuthField] = &[
    AuthField {
        sdk_name: "display_name",
        toml_key: "display_name",
        ty: FieldTy::Str,
        required: true,
    },
    AuthField {
        sdk_name: "package",
        toml_key: "package",
        ty: FieldTy::Str,
        required: true,
    },
    AuthField {
        sdk_name: "model",
        toml_key: "model",
        ty: FieldTy::Str,
        required: true,
    },
];

const MODELS_FIELDS: &[AuthField] = &[AuthField {
    sdk_name: "models",
    toml_key: "models",
    ty: FieldTy::ModelMap,
    required: true,
}];

/// The authoring surface covered by the first conformance fixture.
pub const SURFACE: &[Factory] = &[
    Factory {
        name: "models",
        target: TomlTarget::KeyedTable("models"),
        fields: MODELS_FIELDS,
    },
    Factory {
        name: "agent",
        target: TomlTarget::KeyedTable("agents"),
        fields: AGENT_FIELDS,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_declares_agent_and_models() {
        let names: Vec<_> = SURFACE.iter().map(|f| f.name).collect();
        assert!(names.contains(&"agent"));
        assert!(names.contains(&"models"));
    }

    #[test]
    fn agent_has_required_display_name() {
        let agent = SURFACE.iter().find(|f| f.name == "agent").unwrap();
        let dn = agent
            .fields
            .iter()
            .find(|f| f.sdk_name == "display_name")
            .unwrap();
        assert!(dn.required);
        assert_eq!(dn.toml_key, "display_name");
    }
}
