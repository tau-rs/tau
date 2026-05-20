//! `tau target` subcommand group — inspect the deployment-target registry.

pub mod list;
pub(crate) mod render;
pub mod show;

use crate::cli::TargetSubcommand;
use crate::output::Output;

/// Route `tau target <subcommand>` to its impl module.
pub async fn run(sub: &TargetSubcommand, output: &mut Output) -> anyhow::Result<()> {
    match sub {
        TargetSubcommand::List(args) => list::run(args, output),
        TargetSubcommand::Show(args) => show::run(args, output),
    }
}
