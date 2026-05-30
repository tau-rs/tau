//! SBPL profile generation from a [`CapabilityPlan`].
//!
//! Pure functions — no I/O, no spawning. Tested on any platform.

use tau_domain::{Capability, FsCapability, NetCapability};
use tau_ports::CapabilityPlan;

use crate::baseline::SBPL_BASELINE;

/// TCP port the host-side `tau-sandbox-proxy` task listens on. Plugins
/// reach it via `HTTPS_PROXY=http://127.0.0.1:8443`. Must match the
/// `--listen` arg the bridge expects on Linux for cross-platform parity
/// (see `tau-sandbox-native::strict::apply_strict`).
pub(crate) const PROXY_PORT: u16 = 8443;

/// Build a complete SBPL profile string from `plan`. Returns text that can
/// be written to a `.sb` file and passed to `sandbox-exec -f`.
///
/// Profile shape:
/// 1. `(version 1)` + `(deny default)` header
/// 2. The baseline allowlist (libc / dyld bootstrap, /tmp)
/// 3. Per-capability rules (file-read*, file-write*, process-exec)
/// 4. Network: outbound only to `localhost:PROXY_PORT` if any
///    `Network(Http)` capability is present
pub fn build_sbpl_profile(plan: &CapabilityPlan) -> String {
    let mut sbpl = String::new();
    sbpl.push_str("(version 1)\n");
    sbpl.push_str("(deny default)\n");
    sbpl.push_str(SBPL_BASELINE);
    sbpl.push_str("\n;; ---- plan-derived rules ----\n");

    let has_http = plan
        .capabilities
        .iter()
        .any(|c| matches!(c, Capability::Network(NetCapability::Http { .. })));

    // Always allow tmp writes — plans typically need scratch space and
    // `/tmp` is well-isolated by macOS already.
    sbpl.push_str("(allow file-write*\n  (subpath \"/tmp\")\n  (subpath \"/private/tmp\")\n  (subpath \"/private/var/folders\"))\n");

    for cap in &plan.capabilities {
        match cap {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                sbpl.push_str("(allow file-read*\n");
                for path in paths {
                    let cleaned = clean_glob_suffix(path);
                    sbpl.push_str(&format!("  (subpath {})\n", quote_sbpl(&cleaned)));
                }
                sbpl.push_str(")\n");
            }
            Capability::Filesystem(FsCapability::Write { paths, .. }) => {
                sbpl.push_str("(allow file-read* file-write*\n");
                for path in paths {
                    let cleaned = clean_glob_suffix(path);
                    sbpl.push_str(&format!("  (subpath {})\n", quote_sbpl(&cleaned)));
                }
                sbpl.push_str(")\n");
            }
            Capability::Process(_) => {
                // The baseline already grants `(allow process*)` which
                // covers process-exec and process-fork. Plan-level Process
                // capabilities don't add SBPL rules here; the plan's
                // command/binary allowlist is enforced by tau's runtime
                // before the spawn (see tau-runtime/src/plugin_host).
                // sandbox-exec's children inherit the same SBPL profile,
                // so they cannot escape the FS / net rules.
            }
            Capability::Network(NetCapability::Http { .. }) => {
                // Network rules emitted once below — see has_http.
            }
            _ => {
                // Other capability shapes (Storage, Custom) not yet mapped
                // to SBPL. validate_plan refuses these via supported_shapes.
            }
        }
    }

    if has_http {
        sbpl.push_str(&format!(
            "\n;; outbound network restricted to the host-side tau-sandbox-proxy task\n\
             (allow network-outbound (remote tcp \"localhost:{port}\"))\n\
             (allow network-outbound (remote tcp \"127.0.0.1:{port}\"))\n\
             (allow network*-out (remote ip \"localhost:{port}\"))\n",
            port = PROXY_PORT,
        ));
    }

    sbpl
}

/// Strip trailing glob suffixes from a path to produce an SBPL-friendly
/// `subpath` source.
fn clean_glob_suffix(p: &str) -> String {
    p.trim_end_matches("/**")
        .trim_end_matches("/*")
        .trim_end_matches('/')
        .to_string()
}

/// SBPL string literals are double-quoted; macOS's parser does NOT honor
/// `\\` or `\"` escapes — instead it terminates at the first `"`. Reject
/// paths containing a `"` (no real plan path will contain one) so we don't
/// silently truncate.
fn quote_sbpl(s: &str) -> String {
    debug_assert!(!s.contains('"'), "SBPL path contains literal quote: {s:?}");
    format!("\"{s}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan_from(capabilities: serde_json::Value) -> CapabilityPlan {
        let plan_json = json!({
            "capabilities": capabilities,
            "context": null,
            "limits": null,
        });
        serde_json::from_value(plan_json).expect("decode plan")
    }

    #[test]
    fn empty_plan_emits_baseline_and_header() {
        let plan = plan_from(json!([]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.starts_with("(version 1)\n(deny default)\n"));
        assert!(sbpl.contains("mach-lookup"));
    }

    #[test]
    fn fs_read_paths_emit_subpath_rules() {
        let plan = plan_from(json!([
            { "kind": "fs.read", "paths": ["/etc/foo", "/data/cache"] }
        ]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.contains("(subpath \"/etc/foo\")"));
        assert!(sbpl.contains("(subpath \"/data/cache\")"));
        assert!(sbpl.contains("(allow file-read*"));
    }

    #[test]
    fn fs_write_emits_read_and_write_rules() {
        let plan = plan_from(json!([
            { "kind": "fs.write", "paths": ["/data/scratch"] }
        ]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.contains("(allow file-read* file-write*"));
        assert!(sbpl.contains("(subpath \"/data/scratch\")"));
    }

    #[test]
    fn glob_suffix_stripped() {
        let plan = plan_from(json!([
            { "kind": "fs.read", "paths": ["/srv/data/**", "/etc/*", "/tmp/"] }
        ]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.contains("(subpath \"/srv/data\")"));
        assert!(sbpl.contains("(subpath \"/etc\")"));
        // "/tmp/" stripped to "/tmp"
        assert!(sbpl.contains("(subpath \"/tmp\")"));
    }

    #[test]
    fn http_plan_emits_loopback_only_outbound() {
        let plan = plan_from(json!([
            { "kind": "net.http", "hosts": ["api.example.com"], "methods": ["GET"] }
        ]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.contains("network-outbound"));
        assert!(sbpl.contains("127.0.0.1:8443"));
        assert!(sbpl.contains("localhost:8443"));
        // Must NOT allow general network-outbound
        assert!(!sbpl.contains("(allow network-outbound)\n"));
    }

    #[test]
    fn no_http_plan_omits_network_rule() {
        let plan = plan_from(json!([
            { "kind": "fs.read", "paths": ["/etc"] }
        ]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(!sbpl.contains("network-outbound"));
    }

    #[test]
    fn always_allows_tmp_write() {
        let plan = plan_from(json!([]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.contains("(subpath \"/tmp\")"));
        assert!(sbpl.contains("(subpath \"/private/tmp\")"));
        assert!(sbpl.contains("(subpath \"/private/var/folders\")"));
    }

    #[test]
    fn process_exec_capability_handled_by_baseline() {
        // Process capabilities are covered by the baseline's `(allow
        // process*)` — no per-plan SBPL rules emitted. This test asserts
        // the profile compiles cleanly with a Process capability and
        // that the baseline grants process-exec.
        let plan = plan_from(json!([
            { "kind": "process.spawn", "commands": ["/bin/echo"] }
        ]));
        let sbpl = build_sbpl_profile(&plan);
        assert!(sbpl.contains("(allow process*)"));
    }
}
