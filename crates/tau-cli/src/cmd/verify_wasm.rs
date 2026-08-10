//! Pure WIT-world reproducibility comparator for `tau verify --wasm`
//! (EPIC 3.5). Byte-compares a shipped `.wit` sidecar against the world
//! re-derived from a project's declared capabilities. No I/O lives here —
//! the caller reads the file and re-derives the world.

use crate::cmd::build::hex_lower;

/// The first line at which two WIT worlds diverge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitLineDiff {
    /// 1-indexed line number of the first divergence.
    pub line: usize,
    /// The shipped line at `line`, or `None` if shipped has fewer lines.
    pub shipped: Option<String>,
    /// The re-derived line at `line`, or `None` if re-derived has fewer lines.
    pub rederived: Option<String>,
}

/// Outcome of comparing a shipped WIT world against a re-derived one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitReproReport {
    /// True when the shipped and re-derived worlds are byte-identical.
    pub reproducible: bool,
    /// Lowercase-hex sha256 of the shipped world.
    pub shipped_sha256: String,
    /// Lowercase-hex sha256 of the re-derived world.
    pub rederived_sha256: String,
    /// First differing line. `None` when `reproducible`.
    pub first_diff: Option<WitLineDiff>,
}

/// Lowercase-hex sha256 of a byte slice (display-only; the verdict is exact
/// string equality, not the hash).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex_lower(&h.finalize())
}

/// Compare a shipped WIT world against a re-derived one. The verdict is exact
/// byte equality; on mismatch, `first_diff` names the first line that differs
/// (walking both sides in lockstep — either may run out of lines first).
pub fn compare_wit(shipped: &str, rederived: &str) -> WitReproReport {
    let reproducible = shipped == rederived;
    let first_diff = if reproducible {
        None
    } else {
        let mut s = shipped.lines();
        let mut r = rederived.lines();
        let mut line = 0usize;
        loop {
            line += 1;
            let (sl, rl) = (s.next(), r.next());
            match (sl, rl) {
                (Some(a), Some(b)) if a == b => continue,
                (None, None) => break None, // trailing-newline-only difference
                (a, b) => {
                    break Some(WitLineDiff {
                        line,
                        shipped: a.map(str::to_string),
                        rederived: b.map(str::to_string),
                    })
                }
            }
        }
    };
    WitReproReport {
        reproducible,
        shipped_sha256: sha256_hex(shipped.as_bytes()),
        rederived_sha256: sha256_hex(rederived.as_bytes()),
        first_diff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_worlds_are_reproducible() {
        let w = "package tau:generated@0.1.0;\nworld runner {}\n";
        let r = compare_wit(w, w);
        assert!(r.reproducible);
        assert_eq!(r.first_diff, None);
        assert_eq!(r.shipped_sha256, r.rederived_sha256);
    }

    #[test]
    fn one_changed_line_reports_first_diff() {
        let shipped = "line-a\nimport wasi:sockets/x@0.2.3;\nline-c\n";
        let rederived = "line-a\nimport wasi:http/types@0.2.3;\nline-c\n";
        let r = compare_wit(shipped, rederived);
        assert!(!r.reproducible);
        let d = r.first_diff.expect("diff present");
        assert_eq!(d.line, 2);
        assert_eq!(d.shipped.as_deref(), Some("import wasi:sockets/x@0.2.3;"));
        assert_eq!(
            d.rederived.as_deref(),
            Some("import wasi:http/types@0.2.3;")
        );
    }

    #[test]
    fn shipped_has_extra_trailing_line() {
        let shipped = "line-a\nline-b\nextra\n";
        let rederived = "line-a\nline-b\n";
        let r = compare_wit(shipped, rederived);
        assert!(!r.reproducible);
        let d = r.first_diff.expect("diff present");
        assert_eq!(d.line, 3);
        assert_eq!(d.shipped.as_deref(), Some("extra"));
        assert_eq!(d.rederived, None);
    }
}
