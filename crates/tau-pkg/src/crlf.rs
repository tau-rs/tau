//! Build-time CRLF normalization — the single source of truth.
//!
//! Prompt bytes are hashed twice on the way into a bundle: once as the
//! bundle's `system_prompt_sha256` (`bundle::build::resolve_agent_prompt_bytes`)
//! and once as the IR's content-addressed asset hash (`tau_ir_lower`'s
//! `prompt_file` closure, which *is* `bundle::read_prompt_file`). Directory
//! definitions add a third normalization point: `project::dirs::file` folds
//! CRLF before splitting frontmatter from the markdown body.
//!
//! Because those layers stack, normalization has to be **idempotent** —
//! normalizing already-normalized bytes must be a no-op. A naive
//! `\r\n` → `\n` rewrite is not: `\r\r\n` folds to `\r\n` on the first pass
//! and to `\n` on the second, so a single-normalized hash and a
//! double-normalized hash disagree over the same file. The rule here is
//! therefore "collapse a *run* of `\r` immediately preceding a `\n`", which
//! reaches its fixed point in one pass.
//!
//! A `\r` that is not followed by `\n` (classic Mac OS line ending, or a
//! deliberate control character inside a prompt) is preserved verbatim.

/// Collapse every run of `\r` that immediately precedes a `\n` into just
/// that `\n`. Idempotent.
///
/// # Example
///
/// ```
/// use tau_pkg::crlf::normalize_crlf_bytes;
///
/// let once = normalize_crlf_bytes(b"a\r\r\nb".to_vec());
/// assert_eq!(once, b"a\nb");
/// // Idempotent: a second pass changes nothing.
/// assert_eq!(normalize_crlf_bytes(once.clone()), once);
/// // A lone `\r` is preserved.
/// assert_eq!(normalize_crlf_bytes(b"a\rb".to_vec()), b"a\rb");
/// ```
#[must_use]
pub fn normalize_crlf_bytes(input: Vec<u8>) -> Vec<u8> {
    // Fast path: nothing to fold. Avoids a copy for the overwhelmingly
    // common LF-only file.
    if !input.contains(&b'\r') {
        return input;
    }
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\r' {
            // Span the whole run of `\r`.
            let mut j = i;
            while j < input.len() && input[j] == b'\r' {
                j += 1;
            }
            if input.get(j) == Some(&b'\n') {
                // `\r{1,}\n` → `\n`: drop the run, let the `\n` be emitted
                // by the next iteration.
                i = j;
                continue;
            }
            // A run of `\r` not terminated by `\n` is content, not a line
            // ending. Keep it byte-for-byte.
            out.extend_from_slice(&input[i..j]);
            i = j;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// `&str` flavor of [`normalize_crlf_bytes`], with identical semantics.
///
/// Used by the `[dirs]` definition-file parsers, which work on text
/// (frontmatter + markdown body) rather than raw bytes. Normalizing is
/// UTF-8-preserving: `\r` and `\n` are ASCII and can never appear inside a
/// multi-byte code point.
///
/// # Example
///
/// ```
/// use tau_pkg::crlf::normalize_crlf_str;
///
/// assert_eq!(normalize_crlf_str("a\r\r\nb"), "a\nb");
/// assert_eq!(normalize_crlf_str(&normalize_crlf_str("a\r\r\nb")), "a\nb");
/// ```
#[must_use]
pub fn normalize_crlf_str(s: &str) -> String {
    let bytes = normalize_crlf_bytes(s.as_bytes().to_vec());
    // `normalize_crlf_bytes` only ever deletes ASCII `\r` bytes, which can
    // never be part of a multi-byte UTF-8 sequence, so the result is still
    // valid UTF-8.
    String::from_utf8(bytes).expect("removing ASCII \\r cannot break UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_crlf_folds() {
        assert_eq!(normalize_crlf_bytes(b"a\r\nb\r\n".to_vec()), b"a\nb\n");
    }

    /// The regression this module exists for: `read_prompt_file` normalizes
    /// once and `tau_ir_lower`'s `prompt_file` path normalizes the result
    /// again. With a non-idempotent rewrite, `\r\r\n` produced two different
    /// hashes for the same file — the bundle's `system_prompt_sha256` (one
    /// pass) and the IR asset hash (two passes).
    #[test]
    fn double_cr_before_lf_is_idempotent() {
        let once = normalize_crlf_bytes(b"a\r\r\nb".to_vec());
        assert_eq!(once, b"a\nb");
        let twice = normalize_crlf_bytes(once.clone());
        assert_eq!(
            twice, once,
            "normalization must reach a fixed point in one pass"
        );
    }

    #[test]
    fn long_cr_run_before_lf_collapses_fully() {
        assert_eq!(normalize_crlf_bytes(b"a\r\r\r\r\nb".to_vec()), b"a\nb");
    }

    #[test]
    fn lone_cr_is_preserved_and_stable() {
        let once = normalize_crlf_bytes(b"a\rb\r".to_vec());
        assert_eq!(once, b"a\rb\r");
        assert_eq!(normalize_crlf_bytes(once.clone()), once);
    }

    #[test]
    fn mixed_run_keeps_leading_cr_content_before_a_lone_cr() {
        // `\r\r` then `x` (no `\n`): kept. Then `\r\n`: folded.
        assert_eq!(normalize_crlf_bytes(b"\r\rx\r\n".to_vec()), b"\r\rx\n");
    }

    #[test]
    fn lf_only_input_is_returned_unchanged() {
        assert_eq!(normalize_crlf_bytes(b"a\nb\n".to_vec()), b"a\nb\n");
    }

    #[test]
    fn str_flavor_matches_byte_flavor() {
        for s in ["a\r\nb", "a\r\r\nb", "a\rb", "plain", "\r\n\r\n", "é\r\r\n"] {
            assert_eq!(
                normalize_crlf_str(s).into_bytes(),
                normalize_crlf_bytes(s.as_bytes().to_vec()),
                "divergence on {s:?}",
            );
        }
    }
}
