//! Smoke test: each combination of Format + Writer at least *parses*
//! and returns a guard. Cannot assert on the registry state directly
//! (tracing-subscriber doesn't expose it), so this is a minimum-bar
//! check that the global init path doesn't panic.

use tau_observe::filter::env_or_directive;
use tau_observe::install::{install, Format, InstallOptions, Writer};

#[test]
fn each_format_writer_combination_installs_without_panic() {
    let combos = [
        (Format::Human, Writer::Stderr),
        (Format::Human, Writer::Stdout),
        (Format::Json, Writer::Stderr),
        (Format::Json, Writer::Stdout),
    ];
    for (format, writer) in combos {
        let opts = InstallOptions {
            filter: env_or_directive("tau=info"),
            format,
            writer,
            #[cfg(feature = "otlp")]
            otlp: None,
        };
        let _g = install(opts).expect("install returned err");
    }
}
