//! Fuzz harness for `tau_domain::package::skill::parse_skill_md`.
//!
//! `parse_skill_md` is user-input-facing: anyone authoring a skill
//! writes a SKILL.md by hand, and that input flows through the YAML
//! frontmatter splitter + parser into a `SkillContent`. A panic here
//! crashes whatever tool is loading the skill (CLI, runtime,
//! tau-pkg install path).
//!
//! Triage signals:
//!   - Process abort → libFuzzer reports a crash. Treat as a bug.
//!   - Timeout (default 25s/run) → potential exponential YAML or
//!     frontmatter-split path. Bug.
//!   - Memory blowup (default 2 GiB) → unbounded allocation. Bug.
//!
//! Run locally:
//!     rustup toolchain install nightly
//!     cargo install cargo-fuzz
//!     cd crates/tau-domain/fuzz
//!     cargo +nightly fuzz run parse_skill_md -- -max_total_time=60

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_domain::package::skill::parse_skill_md;

fuzz_target!(|data: &[u8]| {
    // parse_skill_md takes &str, so reject invalid UTF-8 to focus the
    // fuzz signal on the parser logic itself. UTF-8 sanity is handled
    // at I/O boundaries (read_to_string) by callers.
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    let _ = parse_skill_md(s);
});
