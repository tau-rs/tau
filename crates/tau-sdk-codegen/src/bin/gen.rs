//! Regenerate the SDK packages: `cargo run -p tau-sdk-codegen --bin gen`.
use std::path::Path;

fn main() -> anyhow::Result<()> {
    // repo root = two levels up from this crate's manifest dir at dev time;
    // callers may pass an explicit root as argv[1].
    let root = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
        });
    tau_sdk_codegen::generate(&root)?;
    eprintln!("generated SDK under {}/sdk", root.display());
    Ok(())
}
