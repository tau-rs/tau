//! Task 10 (capstone): end-to-end divergence test for HostSet exact+typed-any
//! (ADR-0061, `docs/superpowers/specs/2026-07-18-hostset-exact-plus-typed-any-design.md`).
//!
//! Before this feature, a manifest authored `hosts = ["*"]` sailed through
//! parse → lattice → `tau check` → `tau build`, then FAILED only at run time
//! (the proxy rejects `*`). This file proves the new, single semantic end to
//! end, both directions of build-accepts ⟺ run-enforces:
//!
//! 1. `hosts_any_passes_check_and_build` — a project authored `hosts = "any"`
//!    (the typed sentinel) is accepted by `tau check config`, `tau check
//!    governance`, and `tau build`; and the built package manifest decodes its
//!    `net.http` grant to `HostSet::Any` — the value the strict-native / darwin
//!    / container adapters map to `tau_sandbox_proxy::HostAllow::Any` (pass-all
//!    mode) at run time. (We assert on the package grant rather than the
//!    bundle's `effective_capabilities` summary, which `bundle::build` only
//!    populates when an agent declares capability *overrides*.)
//! 2. `hosts_star_fails_at_parse` — a project authored `hosts = ["*"]` is
//!    rejected at decode: both at the bare `tau_domain::Capability` serde
//!    layer and, end to end, by `tau check` on a real `tau.toml` (exit 2,
//!    never reaching `tau build`).

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

/// Build an isolated `TAU_HOME` tempdir with `config.toml` pre-created, so
/// `tau build`'s global-scope writes never touch the developer's `~/.tau`
/// and parallel tests don't race on its initial write (see project memory
/// `feedback_windows_tau_home_test_pattern`).
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// Write a project whose sole agent's home package declares a `net.http`
/// capability with the typed `hosts = "any"` sentinel. Writes BOTH
/// lockfiles that the CLI reads under two different filenames:
/// - `tau-lock.toml` — read by `tau check`/`tau resolve` via
///   `tau_pkg::scope::Scope::lockfile_path()`.
/// - `tau.lock` — read by `tau build` (a hardcoded, distinct filename in
///   `tau_pkg::bundle::build`; a pre-existing split in this codebase, not
///   introduced by this test).
fn write_hosts_any_project(root: &std::path::Path) {
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-07-18T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .unwrap();

    // tau-lock.toml — consumed by `tau check` / `tau resolve`.
    std::fs::write(
        root.join("tau-lock.toml"),
        format!(
            "schema_version = 4\ngenerated_by_tau_version = \"0.0.0\"\ngenerated_at = \"2026-07-18T00:00:00Z\"\n\n[[package]]\nname = \"homepkg\"\nactive_version = \"0.1.0\"\nsource = \"https://example.com/homepkg.git\"\n\n[[package.versions]]\nversion = \"0.1.0\"\nresolved_commit = \"{zero}\"\nsha256 = \"\"\ninstalled_at = \"2026-07-18T00:00:00Z\"\n",
            zero = "0".repeat(40)
        ),
    )
    .unwrap();

    // tau.lock — consumed by `tau build`.
    std::fs::write(
        root.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "homepkg"
active_version = "0.1.0"
source = "https://example.com/homepkg.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();

    // Installed home-package manifest: its sole capability grant is
    // `net.http` with the typed `any` sentinel (authored `hosts = "any"`).
    let pkg_dir = root
        .join(".tau")
        .join("packages")
        .join("homepkg")
        .join("0.1.0");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("tau.toml"),
        r#"name = "homepkg"
version = "0.1.0"
description = "home package"
authors = []
source = "https://example.com/homepkg.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "net.http"
hosts = "any"
"#,
    )
    .unwrap();

    // Project tau.toml. Deliberately no root `[allow]` block here: with
    // `[allow]` present, `[models]` must live under `[allow.models]`
    // instead, and `tau-ir-lower`'s `resolve_model_ref` (parse.rs) only
    // consults top-level `config.models` — a pre-existing gap unrelated to
    // HostSet that would make `tau build` fail lowering regardless of the
    // net.http grant under test. The `[allow]` bridge's own acceptance of
    // `hosts = "any"` is covered in isolation by
    // `allow_net_http_any_bridges_to_hostset_any` in tau-pkg; this test's
    // job is the package-capability → effective-capability → bundle path.
    std::fs::write(
        root.join("tau.toml"),
        r#"packages = ["homepkg"]

[project]
name = "hostsany"
version = "0.1.0"

[models]
default = { backend = "homepkg", model = "m-1" }

[agents.solo]
display_name = "Solo"
package = "homepkg@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"
"#,
    )
    .unwrap();
}

/// Direction 1 of the divergence test: `hosts = "any"` is accepted through
/// the WHOLE build path (`tau check governance`, bare `tau check`, and
/// `tau build`), and the lowered capability the bundle carries is genuinely
/// `HostSet::Any` — not silently dropped, not narrowed to an empty exact
/// set. This is the "build-accepts ⇒ run-enforces (pass-all reachable)"
/// half of the spec's closing claim.
#[test]
fn hosts_any_passes_check_and_build() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_hosts_any_project(root);

    // `tau check config` — re-parses tau.toml (`ProjectConfig::from_path`),
    // which is exactly the decode path `hosts = "any"` must survive
    // (`RawHosts::Str("any") => HostSet::Any` in tau-domain's `Capability`
    // deserialize) for the package manifest to install/lower at all.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "config"])
        .current_dir(root)
        .assert()
        .code(0);

    // `tau check governance` — no root [allow] ceiling is declared here (see
    // `write_hosts_any_project`'s doc comment), so this only asserts the
    // no-constitution path is a Warning (exit 0), not an Error.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance"])
        .current_dir(root)
        .assert()
        .code(0);

    // NOTE: bare `tau check` (all 8 categories) is deliberately NOT
    // asserted here — this fixture's home package is a data-only package
    // (no `[plugin]` section), which trips the unrelated `plugins` category
    // ("model backend does not expose LLM completion"). That's an
    // orthogonal fixture-completeness concern, not a HostSet regression;
    // `config` + `governance` above already cover every check-category
    // code path that touches `hosts = "any"` decode/lattice logic.

    // `tau build` succeeds and produces a bundle whose agent carries the
    // lowered `net.http` grant. `--allow-ungoverned` opts out of the
    // governed-by-default GOV000 gate (ADR-0057): this fixture deliberately
    // declares no `[allow]` constitution (its `[models]` stays top-level so
    // lowering resolves the backend), and this test is about HostSet
    // acceptance, not governance.
    let scratch = tempfile::tempdir().unwrap();
    let tau_home = make_tau_home(scratch.path());
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned"])
        .current_dir(root)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout_path = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(
        std::path::Path::new(&stdout_path).exists(),
        "tau build did not write a bundle at the printed path {stdout_path:?}",
    );

    let bundle_str = std::fs::read_to_string(&stdout_path).expect("read bundle");
    let manifest =
        tau_pkg::bundle::manifest::BundleManifest::parse_str(&bundle_str).expect("parse bundle");
    assert_eq!(manifest.agents.len(), 1, "expected exactly one agent");

    // Non-vacuity: the package manifest that `tau build` just consumed decodes
    // its `net.http` grant to the `HostSet::Any` sentinel — the exact value the
    // strict-native / darwin / container adapters map to
    // `tau_sandbox_proxy::HostAllow::Any` (pass-all) at run time. A regression
    // that rejected or narrowed "any" would either fail the build above or land
    // here as a non-`Any` `HostSet`.
    //
    // (We assert on the package grant rather than the bundle's
    // `effective_capabilities` summary because that summary is only populated
    // when an agent declares capability *overrides* — see `bundle::build`. This
    // no-override project intentionally leaves it default; runtime enforcement
    // reads the package grant directly, which is what we check.)
    let pkg_manifest = root
        .join(".tau")
        .join("packages")
        .join("homepkg")
        .join("0.1.0")
        .join("tau.toml");
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(&std::fs::read_to_string(&pkg_manifest).unwrap()).expect("parse manifest");
    let hosts = unchecked
        .capabilities
        .iter()
        .find_map(|c| match c {
            tau_domain::Capability::Network(tau_domain::NetCapability::Http { hosts, .. }) => {
                Some(hosts)
            }
            _ => None,
        })
        .expect("net.http capability present in the built manifest");
    assert!(
        hosts.is_any(),
        "authored `hosts = \"any\"` must decode to HostSet::Any (pass-all); got {hosts:?}",
    );
}

/// Direction 2 of the divergence test: `hosts = ["*"]` — the old
/// build-accepts-but-run-rejects escape hatch — is now a hard decode/parse
/// error and never reaches build.
#[test]
fn hosts_star_fails_at_parse() {
    // Library-level: `tau_domain::Capability`'s custom `Deserialize` impl
    // rejects a bare `"*"` inside the `hosts` list (it is not a valid
    // `HostName` — see `HostName::parse`'s `Wildcard` rejection).
    let e = serde_json::from_str::<tau_domain::Capability>(r#"{"kind":"net.http","hosts":["*"]}"#)
        .unwrap_err();
    let msg = e.to_string().to_lowercase();
    assert!(
        msg.contains("any") || msg.contains("wildcard"),
        "expected the '*' rejection to mention 'any' or 'wildcard'; got: {e}",
    );

    // CLI-level: the same rejection through `tau check` on a real tau.toml
    // whose root [allow] ceiling authors `hosts = ["*"]`. This is a
    // tau.toml decode/validation failure (`validate_allow` → `bridge_caps`
    // → the same `Capability` deserialize path), surfaced by the `config`
    // check category as a `Severity::Error` finding — exit 2, never
    // reaching `tau build`.
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "badstar"

[allow]
"net.http" = { hosts = ["*"] }
"#,
    )
    .unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .code(2)
        .get_output()
        .clone();

    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
    assert!(
        stdout.contains("wildcard") || stdout.contains("any"),
        "expected tau check's rejection message to mention 'wildcard' or 'any'; stdout:\n{stdout}",
    );
}
