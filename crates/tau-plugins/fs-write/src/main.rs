//! `fs-write-plugin` binary. Spawned by tau-runtime::plugin_host as a
//! subprocess; talks MessagePack-RPC over stdio per ADR-0008.
//!
//! Thin shim over [`tau_plugin_sdk::run_tool_with_config`].
//!
//! [`FsWritePlugin`]: fs_write_plugin_lib::plugin::FsWritePlugin

use fs_write_plugin_lib::plugin::FsWritePlugin;
use tau_plugin_sdk::{run_tool_with_config, SdkError};

#[tokio::main]
async fn main() -> Result<(), SdkError> {
    run_tool_with_config::<FsWritePlugin>(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).await
}
