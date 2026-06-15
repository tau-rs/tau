//! Ordered, element-by-element diff of two normalized event streams.
use crate::event::ConformanceEvent;

#[derive(Debug)]
pub struct Divergence {
    pub index: usize,
    pub report: String,
}

/// Compare `expected` vs `actual`. Returns `None` if identical, else the
/// first divergence with a readable windowed report (±2 events).
pub fn diff(expected: &[ConformanceEvent], actual: &[ConformanceEvent]) -> Option<Divergence> {
    let n = expected.len().max(actual.len());
    for i in 0..n {
        let e = expected.get(i);
        let a = actual.get(i);
        if e != a {
            let mut report = format!("event-stream divergence at index {i}\n");
            match (e, a) {
                (Some(e), Some(a)) => {
                    report.push_str(&format!("  expected: {e:?}\n  actual:   {a:?}\n"));
                }
                (Some(e), None) => report.push_str(&format!("  actual stream missing: {e:?}\n")),
                (None, Some(a)) => report.push_str(&format!("  actual stream extra:   {a:?}\n")),
                (None, None) => unreachable!(),
            }
            let lo = i.saturating_sub(2);
            report.push_str("  --- expected window ---\n");
            for (j, ev) in expected.iter().enumerate().skip(lo).take(5) {
                report.push_str(&format!("    [{j}] {ev:?}\n"));
            }
            report.push_str("  --- actual window ---\n");
            for (j, ev) in actual.iter().enumerate().skip(lo).take(5) {
                report.push_str(&format!("    [{j}] {ev:?}\n"));
            }
            return Some(Divergence { index: i, report });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ConformanceEvent;

    fn run_started() -> ConformanceEvent {
        ConformanceEvent::RunStarted
    }
    fn completed(o: &str) -> ConformanceEvent {
        ConformanceEvent::RunCompleted { outcome: o.into() }
    }

    #[test]
    fn equal_streams_have_no_diff() {
        let a = vec![run_started(), completed("Success")];
        assert!(diff(&a, &a).is_none());
    }

    #[test]
    fn first_divergence_reported_with_index() {
        let a = vec![run_started(), completed("Success")];
        let b = vec![run_started(), completed("Failure")];
        let d = diff(&a, &b).expect("streams differ");
        assert_eq!(d.index, 1);
        assert!(d.report.contains("index 1"));
    }

    #[test]
    fn length_mismatch_reported() {
        let a = vec![run_started(), completed("Success")];
        let b = vec![run_started()];
        let d = diff(&a, &b).expect("length differs");
        assert_eq!(d.index, 1);
        assert!(d.report.contains("missing") || d.report.contains("extra"));
    }
}
