//! Pure preopen-selection logic for the guest fs effects (#604 hardening).
//!
//! Compiled on every target (not just wasm32) so the table-driven tests run
//! natively; the wasm dispatcher's `resolve_preopen` feeds it the guest paths
//! from `wasi:filesystem/preopens.get-directories`.
//!
//! Selection is LONGEST-prefix, segment-aware, and purely lexical:
//! - With overlapping grants (`/data` read-only + `/data/logs` read-write) a
//!   path under both binds the most specific preopen, so a write to
//!   `/data/logs/x` reaches the RW descriptor instead of being host-denied
//!   through the RO one.
//! - A `"/"` (root) preopen serves every absolute path.
//! - `..` is NOT rejected or normalized here — the relative remainder goes to
//!   the host `open-at`, which rejects escapes at the descriptor boundary
//!   (ADR-0066 D3). Longest-prefix composes with that guard: a traversal like
//!   `/data/../etc` still binds `/data` and hands the host `../etc`, which
//!   fails closed even when a broader preopen (e.g. `/`) also matched.

/// The `open-at`-relative remainder of `path` under the preopen `guest_path`,
/// or `None` if `path` is not inside that preopen.
///
/// Segment-aware: `/data` covers `/data` (rel `""`, the preopen dir itself)
/// and `/data/x` (rel `"x"`), but NOT `/dataX`. A root preopen (`"/"`) covers
/// every absolute path. Only absolute paths resolve; a trailing `/` on either
/// side is tolerated. Repeated separators collapse out of the remainder's
/// leading edge only — interior normalization stays the host's job.
fn match_rel<'p>(path: &'p str, guest_path: &str) -> Option<&'p str> {
    // Tolerate a trailing separator on the preopen path (`resolve_wasi_config`
    // never emits one, but this resolver must not silently miss if a future
    // producer does). `"/"` is the root preopen, not a trailing slash.
    let guest_path = if guest_path == "/" {
        guest_path
    } else {
        guest_path.trim_end_matches('/')
    };
    if guest_path == "/" {
        // Root preopen: every absolute path is inside it; `/` itself opens
        // the preopen dir (rel "").
        return path.starts_with('/').then(|| path.trim_start_matches('/'));
    }
    if path == guest_path || path.strip_prefix(guest_path) == Some("/") {
        return Some(""); // the preopen dir itself (with or without trailing /)
    }
    let stripped = path.strip_prefix(guest_path)?;
    stripped
        .starts_with('/')
        .then(|| stripped.trim_start_matches('/'))
}

/// Pick the preopen whose guest path is the longest matching prefix of
/// `path`; returns `(index, open-at-relative remainder)`. `None` when no
/// preopen contains `path` (absence of capability — the caller surfaces
/// `FsAccessDenied`). On equal-length duplicates the first wins (the host
/// dedups preopens by directory, so this is unreachable in practice).
pub(crate) fn select_preopen<'p, 'g>(
    path: &'p str,
    guest_paths: impl Iterator<Item = &'g str>,
) -> Option<(usize, &'p str)> {
    let mut best: Option<(usize, &'p str, usize)> = None;
    for (idx, guest_path) in guest_paths.enumerate() {
        if let Some(rel) = match_rel(path, guest_path) {
            let specificity = guest_path.trim_end_matches('/').len().max(1);
            if best.is_none_or(|(_, _, s)| specificity > s) {
                best = Some((idx, rel, specificity));
            }
        }
    }
    best.map(|(idx, rel, _)| (idx, rel))
}

#[cfg(test)]
mod tests {
    use super::select_preopen;

    /// One row: requested path, preopen guest-paths (in `get-directories`
    /// order), expected `(preopen index, open-at rel)`.
    type Case = (
        &'static str,
        &'static [&'static str],
        Option<(usize, &'static str)>,
    );

    #[test]
    fn selection_table() {
        #[rustfmt::skip]
        let cases: &[Case] = &[
            // Exact match opens the preopen dir itself.
            ("/data",          &["/data"],                Some((0, ""))),
            ("/data/",         &["/data"],                Some((0, ""))),
            // Plain nesting.
            ("/data/x",        &["/data"],                Some((0, "x"))),
            ("/data/a/b.txt",  &["/data"],                Some((0, "a/b.txt"))),
            // Longest prefix wins over first match, in either listing order.
            ("/data/logs/x",   &["/data", "/data/logs"],  Some((1, "x"))),
            ("/data/logs/x",   &["/data/logs", "/data"],  Some((0, "x"))),
            ("/data/logs",     &["/data", "/data/logs"],  Some((1, ""))),
            // Root preopen serves subpaths and the root itself.
            ("/",              &["/"],                    Some((0, ""))),
            ("/data/x",        &["/"],                    Some((0, "data/x"))),
            // A more specific preopen still beats the root.
            ("/data/x",        &["/", "/data"],           Some((1, "x"))),
            ("/other/y",       &["/", "/data"],           Some((0, "other/y"))),
            // Segment-aware: `/dataX` is not under `/data`.
            ("/dataX/x",       &["/data"],                None),
            ("/dataX/x",       &["/", "/data"],           Some((0, "dataX/x"))),
            // Trailing slash on the preopen path is tolerated.
            ("/data/x",        &["/data/"],               Some((0, "x"))),
            ("/data",          &["/data/"],               Some((0, ""))),
            // Leading double separator in the remainder collapses.
            ("/data//x",       &["/data"],                Some((0, "x"))),
            // Relative paths never resolve, even against a root preopen.
            ("data/x",         &["/", "/data"],           None),
            ("",               &["/"],                    None),
            // Traversal is NOT resolved lexically: the remainder keeps `..`
            // and binds the longest literal prefix; the host `open-at`
            // rejects the escape (fail-closed even though `/` also matches).
            ("/data/../etc",   &["/data"],                Some((0, "../etc"))),
            ("/data/../etc",   &["/", "/data"],           Some((1, "../etc"))),
            // No preopens → no capability.
            ("/data",          &[],                       None),
        ];
        for (path, preopens, expected) in cases {
            let got = select_preopen(path, preopens.iter().copied());
            assert_eq!(
                got, *expected,
                "path {path:?} against preopens {preopens:?}"
            );
        }
    }
}
