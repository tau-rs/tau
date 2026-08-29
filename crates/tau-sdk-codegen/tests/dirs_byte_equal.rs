mod common;

/// Spec invariant: a dir-authored project lowers to byte-identical IR vs its
/// inline-authored twin (`agents/review/strict.md` ≡ [agents."review/strict"]
/// with prompt.system = body). `packages = ["mock-llm"]` is required on both
/// twins identically so `[models].default`'s backend resolves against a
/// declared package (ADR-0057 stage-1 model validation); the directory-
/// scanned agent's own `package = "p@^1"` only declares `p`. See
/// `crates/tau-pkg/tests/dirs_project.rs` for the same ruling.
#[test]
fn dir_authored_equals_inline_authored() {
    let t = tempfile::TempDir::new().unwrap();
    let root = "\
packages = [\"mock-llm\"]\n\
[project]\nname = \"p\"\n\n[dirs]\nagents = \"agents\"\ntools = \"tools\"\n\n\
[models]\ndefault = { backend = \"mock-llm\", model = \"m\" }\n";
    std::fs::write(t.path().join("tau.toml"), root).unwrap();
    std::fs::create_dir_all(t.path().join("agents/review")).unwrap();
    std::fs::create_dir_all(t.path().join("tools")).unwrap();
    std::fs::write(
        t.path().join("agents/review/strict.md"),
        "---\ndisplay_name: A\npackage: p@^1\nmodel: default\n---\nYou review.\n",
    )
    .unwrap();

    let dir_cfg = tau_pkg::project::ProjectConfig::from_path(t.path().join("tau.toml")).unwrap();
    let dir_bytes = common::lower_config_bytes(&dir_cfg);

    let inline = "\
packages = [\"mock-llm\"]\n\
[project]\nname = \"p\"\n\n\
[models]\ndefault = { backend = \"mock-llm\", model = \"m\" }\n\n\
[agents.\"review/strict\"]\ndisplay_name = \"A\"\npackage = \"p@^1\"\nmodel = \"default\"\n\
[agents.\"review/strict\".prompt]\nsystem = \"You review.\\n\"\n";
    let inline_bytes = common::lower_toml_bytes(inline);

    assert_eq!(
        dir_bytes, inline_bytes,
        "dir-authored and inline-authored IR must be byte-identical"
    );
}

/// CRLF-checkout simulation: the same md with `\r\n` endings lowers
/// identically to the `\n` original. Guards `split_frontmatter`'s
/// `normalize_crlf` pass (`crates/tau-pkg/src/project/dirs/file.rs`) at the
/// full from-path-to-IR granularity, not just the unit level.
#[test]
fn dir_authored_crlf_invariant() {
    fn build(t: &std::path::Path, md: &str) -> Vec<u8> {
        let root = "\
packages = [\"mock-llm\"]\n\
[project]\nname = \"p\"\n\n[dirs]\nagents = \"agents\"\ntools = \"tools\"\n\n\
[models]\ndefault = { backend = \"mock-llm\", model = \"m\" }\n";
        std::fs::write(t.join("tau.toml"), root).unwrap();
        std::fs::create_dir_all(t.join("agents/review")).unwrap();
        std::fs::create_dir_all(t.join("tools")).unwrap();
        std::fs::write(t.join("agents/review/strict.md"), md).unwrap();

        let cfg = tau_pkg::project::ProjectConfig::from_path(t.join("tau.toml")).unwrap();
        common::lower_config_bytes(&cfg)
    }

    let lf = "---\ndisplay_name: A\npackage: p@^1\nmodel: default\n---\nYou review.\n";
    let crlf = lf.replace('\n', "\r\n");

    let t_lf = tempfile::TempDir::new().unwrap();
    let t_crlf = tempfile::TempDir::new().unwrap();

    let lf_bytes = build(t_lf.path(), lf);
    let crlf_bytes = build(t_crlf.path(), &crlf);

    assert_eq!(
        lf_bytes, crlf_bytes,
        "CRLF-checkout md must lower to identical IR as the LF original"
    );
}
