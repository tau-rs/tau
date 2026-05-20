//! `tau target list` — enumerate the registry.

use crate::cli::TargetListArgs;
use crate::cmd::target::render;
use crate::output::Output;

/// Run `tau target list`.
pub fn run(args: &TargetListArgs, output: &mut Output) -> anyhow::Result<()> {
    let entries: Box<dyn Iterator<Item = &'static tau_ports::target::TargetTripleEntry>> =
        if args.all {
            Box::new(tau_ports::target::list_all())
        } else {
            Box::new(tau_ports::target::list_available())
        };

    for e in entries {
        if output.is_json() {
            render::render_json_event(e, output)?;
        } else {
            render::render_human_line(e, output)?;
        }
    }
    Ok(())
}
