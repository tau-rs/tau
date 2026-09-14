//! The projected namespace: which capabilities a model may call as tools,
//! under what names, with what schemas (ADR-0006 §5).

use boon::{Compiler, SchemaIndex, Schemas};
use serde_json::Value;
use tau_kernel::abi::{Capability, Name};
use tau_kernel::bridge::ToolDef;
use tau_kernel::kernel::KernelError;
use tau_kernel::syscall::Handle;

/// Why a capability could not be projected as a tool.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// The kernel refused to describe the capability: this agent does not
    /// hold it, or it does not name a driver.
    #[error("cannot describe {cap}: {source}")]
    Describe {
        /// The capability asked about.
        cap: Capability,
        /// The kernel's refusal.
        #[source]
        source: KernelError,
    },
    /// The driver behind the capability does not describe itself as a tool.
    ///
    /// A model driver, a clock, and the echo driver all take the default.
    /// A harness that wants to offer such a driver anyway names it with
    /// [`Toolbox::add`].
    #[error("{0} is not a tool: its driver has no description")]
    NotATool(Capability),
    /// Two capabilities project to the same tool name.
    #[error("tool `{0}` is already in the toolbox")]
    Duplicate(Name),
    /// The driver's schema bytes are not JSON, or not a schema that compiles.
    #[error("schema for `{name}` is unusable: {reason}")]
    Schema {
        /// The tool.
        name: Name,
        /// What the parser or compiler said.
        reason: String,
    },
}

/// One tool: the capability it resolves to, its definition as the model
/// sees it, and its compiled schema.
pub(crate) struct Tool {
    pub(crate) cap: Capability,
    pub(crate) def: ToolDef,
    schema: SchemaIndex,
}

/// The tools a model may call, in the order they were added.
///
/// Built from a namespace with [`Toolbox::project`], which asks the kernel to
/// [`describe`](Handle::describe) each capability: the name is the
/// `DriverId` the harness registered the driver under, and the schema is the
/// driver's own. A driver that does not describe itself can still be offered
/// with [`Toolbox::add`], under a name and schema the caller supplies — the
/// echo driver, say.
///
/// Plain memory throughout: safe to borrow across an await.
#[derive(Default)]
pub struct Toolbox {
    tools: Vec<Tool>,
    schemas: Schemas,
}

impl Toolbox {
    /// An empty toolbox.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Projects `caps` through `Handle::describe`, in order.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Describe`] if the kernel refuses a capability;
    /// [`ProjectError::NotATool`] if a driver has no description;
    /// [`ProjectError::Duplicate`] if two capabilities share a name;
    /// [`ProjectError::Schema`] if a driver's schema bytes are unusable.
    pub fn project(handle: &Handle, caps: &[Capability]) -> Result<Self, ProjectError> {
        let mut toolbox = Self::new();
        for &cap in caps {
            let (id, schema) = handle
                .describe(cap)
                .map_err(|source| ProjectError::Describe { cap, source })?
                .ok_or(ProjectError::NotATool(cap))?;
            let name = id.name().clone();
            let input_schema: Value =
                serde_json::from_slice(&schema.input_schema).map_err(|e| ProjectError::Schema {
                    name: name.clone(),
                    reason: e.to_string(),
                })?;
            toolbox.add(
                cap,
                ToolDef {
                    name,
                    description: schema.description,
                    input_schema,
                },
            )?;
        }
        Ok(toolbox)
    }

    /// Adds a tool the caller describes, for a driver that does not.
    ///
    /// Authority is still the kernel's: a `send` to `cap` from an agent that
    /// does not hold it is refused, and the loop feeds that back as
    /// `denied`. This only decides what the model is *told* exists.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Duplicate`] if the name is taken;
    /// [`ProjectError::Schema`] if `def.input_schema` does not compile.
    pub fn add(&mut self, cap: Capability, def: ToolDef) -> Result<(), ProjectError> {
        if self.tools.iter().any(|t| t.def.name == def.name) {
            return Err(ProjectError::Duplicate(def.name));
        }
        let uri = format!("tool:///{}", def.name);
        let mut compiler = Compiler::new();
        let unusable = |reason: String| ProjectError::Schema {
            name: def.name.clone(),
            reason,
        };
        compiler
            .add_resource(&uri, def.input_schema.clone())
            .map_err(|e| unusable(e.to_string()))?;
        let schema = compiler
            .compile(&uri, &mut self.schemas)
            .map_err(|e| unusable(e.to_string()))?;
        self.tools.push(Tool { cap, def, schema });
        Ok(())
    }

    /// The definitions, in order, ready for `ModelRequest::tools`.
    #[must_use]
    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| t.def.clone()).collect()
    }

    /// The tool names, in order.
    pub fn names(&self) -> impl Iterator<Item = &Name> + '_ {
        self.tools.iter().map(|t| &t.def.name)
    }

    /// How many tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The tool a model named, if it exists. Matches the name as written:
    /// `serch` resolves to nothing.
    pub(crate) fn resolve(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.def.name.as_str() == name)
    }

    /// Checks `input` against the tool's schema. The error is boon's
    /// report, one line per failed keyword, for the model to read.
    pub(crate) fn validate(&self, tool: &Tool, input: &Value) -> Result<(), String> {
        self.schemas
            .validate(input, tool.schema)
            .map_err(|e| e.to_string())
    }
}
