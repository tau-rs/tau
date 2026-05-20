//! Canonical TOML serialization for `BundleManifest`.
//!
//! `serde` + `toml`'s default emitter does not guarantee deterministic
//! field ordering across versions; we hand-roll a small emitter that
//! writes fields in a fixed order so the self-hash is reproducible.

use std::fmt::Write;

use crate::bundle::manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundlePackage,
};

/// Emit the canonical TOML serialization of a `BundleManifest`.
///
/// Field order is fixed; arrays of tables (`[[packages]]`, `[[agents]]`)
/// are emitted in the order they appear in the input struct. Empty
/// optional values are omitted per the spec.
pub fn to_canonical_toml(manifest: &BundleManifest) -> String {
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "schema_version = {}", manifest.schema_version);

    // [bundle]
    out.push('\n');
    out.push_str("[bundle]\n");
    write_str_kv(&mut out, "sha256", &manifest.bundle.sha256);
    write_str_kv(&mut out, "created_at", &manifest.bundle.created_at);
    write_str_kv(&mut out, "tau_version", &manifest.bundle.tau_version);
    write_str_kv(&mut out, "target", &manifest.bundle.target.to_string());

    // [project]
    out.push('\n');
    out.push_str("[project]\n");
    write_str_kv(&mut out, "name", &manifest.project.name);
    write_str_kv(&mut out, "version", &manifest.project.version.to_string());
    write_str_kv(
        &mut out,
        "tau_toml_sha256",
        &manifest.project.tau_toml_sha256,
    );

    // [[packages]]
    for pkg in &manifest.packages {
        out.push('\n');
        out.push_str("[[packages]]\n");
        write_package(&mut out, pkg);
    }

    // [[agents]]
    for agent in &manifest.agents {
        out.push('\n');
        out.push_str("[[agents]]\n");
        write_agent(&mut out, agent);
    }

    out
}

fn write_package(out: &mut String, pkg: &BundlePackage) {
    write_str_kv(out, "name", &pkg.name);
    write_str_kv(out, "version", &pkg.version.to_string());
    // PackageSource serializes to a string form via its own serde impl;
    // we use that for consistency.
    let source_str = match toml::Value::try_from(&pkg.source) {
        Ok(toml::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(e) => panic!("PackageSource serialization must succeed for canonical TOML: {e}"),
    };
    write_str_kv(out, "source", &source_str);
    write_str_kv(out, "tree_sha256", &pkg.tree_sha256);
    if let Some(bin) = &pkg.binary_sha256 {
        write_str_kv(out, "binary_sha256", bin);
    }
    if !pkg.required_shapes.is_empty() {
        write_string_array(
            out,
            "required_shapes",
            pkg.required_shapes
                .iter()
                .map(capability_shape_to_str)
                .collect::<Vec<_>>(),
        );
    }
}

fn write_agent(out: &mut String, agent: &BundleAgent) {
    write_str_kv(out, "id", agent.id.as_str());
    write_backend_inline(out, &agent.backend);
    write_str_kv(out, "system_prompt_sha256", &agent.system_prompt_sha256);
    if !agent.required_tools.is_empty() {
        write_string_array(out, "required_tools", agent.required_tools.to_vec());
    }
    if !agent.effective_capabilities.is_empty() {
        out.push_str("\n[agents.effective_capabilities]\n");
        write_effective_capabilities(out, &agent.effective_capabilities);
    }
}

fn write_backend_inline(out: &mut String, backend: &BackendRef) {
    out.push_str("backend = { ");
    write!(out, "kind = {}", toml_string(&backend.kind)).unwrap();
    if let Some(model) = &backend.model {
        write!(out, ", model = {}", toml_string(model)).unwrap();
    }
    for (k, v) in &backend.extra {
        let v_toml = v.to_string();
        write!(out, ", {} = {}", toml_bare_key(k), v_toml).unwrap();
    }
    out.push_str(" }\n");
}

fn write_effective_capabilities(out: &mut String, caps: &BundleEffectiveCapabilities) {
    if !caps.allow_fs_read.is_empty() {
        write_string_array(out, "allow_fs_read", caps.allow_fs_read.clone());
    }
    if !caps.deny_fs_read.is_empty() {
        write_string_array(out, "deny_fs_read", caps.deny_fs_read.clone());
    }
    if !caps.allow_fs_write.is_empty() {
        write_string_array(out, "allow_fs_write", caps.allow_fs_write.clone());
    }
    if !caps.deny_fs_write.is_empty() {
        write_string_array(out, "deny_fs_write", caps.deny_fs_write.clone());
    }
    if !caps.allow_exec.is_empty() {
        write_string_array(out, "allow_exec", caps.allow_exec.clone());
    }
    if !caps.deny_exec.is_empty() {
        write_string_array(out, "deny_exec", caps.deny_exec.clone());
    }
    if !caps.allow_net_http.is_empty() {
        write_string_array(out, "allow_net_http", caps.allow_net_http.clone());
    }
    if !caps.deny_net_http.is_empty() {
        write_string_array(out, "deny_net_http", caps.deny_net_http.clone());
    }
    if !caps.allow_agent_spawn.is_empty() {
        write_string_array(out, "allow_agent_spawn", caps.allow_agent_spawn.clone());
    }
    if !caps.deny_agent_spawn.is_empty() {
        write_string_array(out, "deny_agent_spawn", caps.deny_agent_spawn.clone());
    }
}

fn write_str_kv(out: &mut String, key: &str, value: &str) {
    writeln!(out, "{} = {}", key, toml_string(value)).unwrap();
}

fn write_string_array(out: &mut String, key: &str, items: Vec<String>) {
    out.push_str(key);
    out.push_str(" = [");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&toml_string(s));
    }
    out.push_str("]\n");
}

/// Emit a TOML basic-string literal (escapes per the TOML spec).
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                write!(&mut out, "\\u{:04X}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Emit a TOML bare key if `k` is ASCII alphanumeric/dash/underscore;
/// otherwise quote it.
fn toml_bare_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        k.to_string()
    } else {
        toml_string(k)
    }
}

fn capability_shape_to_str(s: &tau_domain::CapabilityShape) -> String {
    match s {
        tau_domain::CapabilityShape::FilesystemRead => "FilesystemRead".into(),
        tau_domain::CapabilityShape::FilesystemWrite => "FilesystemWrite".into(),
        tau_domain::CapabilityShape::ProcessExec => "ProcessExec".into(),
        tau_domain::CapabilityShape::NetworkHttp => "NetworkHttp".into(),
        tau_domain::CapabilityShape::AgentSpawn => "AgentSpawn".into(),
        tau_domain::CapabilityShape::Custom { name } => format!("Custom({name})"),
        // Forward-compat: new variants added in future tau-domain versions
        // are serialized as their debug representation.
        unknown => format!("{unknown:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::manifest::tests_helpers::sample_manifest;
    use crate::bundle::BundleManifest;

    #[test]
    fn canonical_serialization_is_byte_identical_on_repeat() {
        let m = sample_manifest();
        let a = to_canonical_toml(&m);
        let b = to_canonical_toml(&m);
        assert_eq!(a, b);
    }

    #[test]
    fn canonical_round_trip_parses_back_to_equal_manifest() {
        let m = sample_manifest();
        let toml_str = to_canonical_toml(&m);
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed, m);
    }

    #[test]
    fn omits_empty_effective_capabilities() {
        let mut m = sample_manifest();
        m.agents[0].effective_capabilities = BundleEffectiveCapabilities::default();
        let toml_str = to_canonical_toml(&m);
        assert!(
            !toml_str.contains("effective_capabilities"),
            "empty table should be omitted: {toml_str}"
        );
    }

    #[test]
    fn omits_missing_binary_sha256() {
        let mut m = sample_manifest();
        m.packages[0].binary_sha256 = None;
        let toml_str = to_canonical_toml(&m);
        assert!(
            !toml_str.contains("binary_sha256"),
            "None binary_sha256 should be omitted: {toml_str}"
        );
    }

    #[test]
    fn fixed_field_order_in_bundle_table() {
        let m = sample_manifest();
        let toml_str = to_canonical_toml(&m);
        let pos_sha = toml_str.find("sha256 =").expect("sha256 present");
        let pos_created = toml_str.find("created_at =").expect("created_at present");
        let pos_version = toml_str.find("tau_version =").expect("tau_version present");
        let pos_target = toml_str.find("target =").expect("target present");
        assert!(
            pos_sha < pos_created && pos_created < pos_version && pos_version < pos_target,
            "fields out of order: {toml_str}"
        );
    }
}
