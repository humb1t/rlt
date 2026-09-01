//! Bencher-format reporter for `benchmark-action/github-action-benchmark`.
//!
//! The action's `cargo` parser reads libtest's benchmark lines:
//!
//! ```text
//! test latency/p99 ... bench:      1234567 ns/iter (+/- 0)
//! ```
//!
//! Emitting them lets a run be tracked over time next to Criterion suites, without a
//! second harness. Everything is reported as nanoseconds per iteration, which is what the
//! format can express: latency statistics directly, and throughput as its reciprocal
//! (lower is better in both, so the action's regression arrow points the right way).
//!
//! # The pacing guard
//!
//! The throughput line is emitted only when [`Pacing::Platform`] says the target set
//! the rate. Under [`Pacing::Schedule`] the rate is an input — it echoes `--rate` back
//! and moves when that flag moves, with no change in the system under test — and under
//! [`Pacing::Harness`] it is the driver's own concurrency bound. Tracking either would
//! fill the history with changes that mean nothing. Latency stays meaningful in every
//! case, and the shortfall is in the JSON report for whoever needs it.

use std::io::Write;

use super::{BenchReporter, ReporterResult};
use crate::baseline::Comparison;
use crate::report::BenchReport;
use crate::schedule::Pacing;

/// A reporter that writes bencher-format lines.
///
/// # Example
///
/// ```ignore
/// use rlt::reporter::{BenchReporter, BencherReporter};
///
/// let reporter = BencherReporter::new(Some("registration".into()));
/// let mut output = Vec::new();
/// reporter.print(&mut output, &report, None)?;
/// ```
#[derive(Debug, Default, Clone)]
pub struct BencherReporter {
    prefix: Option<String>,
}

impl BencherReporter {
    /// Create a reporter, optionally namespacing every metric with `prefix/`.
    ///
    /// A prefix keeps several phases of one run apart in the same history.
    pub fn new(prefix: Option<String>) -> Self {
        Self { prefix }
    }

    fn line(&self, w: &mut dyn Write, name: &str, ns: f64, variance: f64) -> ReporterResult<()> {
        let name = match &self.prefix {
            Some(prefix) => format!("{prefix}/{name}"),
            None => name.to_owned(),
        };
        writeln!(w, "test {name} ... bench: {:.0} ns/iter (+/- {:.0})", ns, variance)?;
        Ok(())
    }
}

impl BenchReporter for BencherReporter {
    fn print(
        &self,
        w: &mut dyn Write,
        report: &BenchReport,
        _comparison: Option<&Comparison>,
    ) -> ReporterResult<()> {
        if report.hist.is_empty() {
            return Ok(());
        }

        let ns = |d: std::time::Duration| d.as_secs_f64() * 1e9;

        self.line(w, "latency/mean", ns(report.hist.mean()), ns(report.hist.stdev()))?;
        self.line(w, "latency/p50", ns(report.hist.median()), 0.0)?;
        self.line(w, "latency/p90", ns(report.hist.value_at_quantile(0.90)), 0.0)?;
        self.line(w, "latency/p99", ns(report.hist.value_at_quantile(0.99)), 0.0)?;
        self.line(w, "latency/max", ns(report.hist.max()), 0.0)?;

        // See "The pacing guard" above: a rate the target did not set is not a result.
        let elapsed = report.elapsed.as_secs_f64();
        let iters = report.stats.overall.iters;
        if report.pacing == Pacing::Platform && iters > 0 && elapsed > 0.0 {
            self.line(w, "throughput", elapsed / iters as f64 * 1e9, 0.0)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::{BenchReporter, BencherReporter};
    use crate::histogram::LatencyHistogram;
    use crate::report::{BenchReport, IterReport};
    use crate::schedule::Pacing;
    use crate::stats::IterStats;
    use crate::status::Status;

    fn report(pacing: Pacing, offered: u64) -> BenchReport {
        let mut hist = LatencyHistogram::new();
        let mut stats = IterStats::new();
        for _ in 0..100 {
            let iter = IterReport {
                duration: Duration::from_millis(10),
                status: Status::success(0),
                bytes: 0,
                items: 1,
            };
            hist.record(iter.duration).unwrap();
            stats.record(&iter);
        }

        BenchReport {
            concurrency: 1,
            hist,
            stats,
            status_dist: HashMap::default(),
            error_dist: HashMap::default(),
            elapsed: Duration::from_secs(1),
            offered,
            dropped: offered / 2,
            pacing,
            rate_per_second: None,
        }
    }

    fn render(pacing: Pacing, offered: u64) -> String {
        let mut out = Vec::new();
        BencherReporter::new(None).print(&mut out, &report(pacing, offered), None).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn platform_paced_runs_report_latency_and_throughput() {
        let out = render(Pacing::Platform, 0);
        assert_eq!(
            out,
            "test latency/mean ... bench: 9998336 ns/iter (+/- 0)\n\
             test latency/p50 ... bench: 10002431 ns/iter (+/- 0)\n\
             test latency/p90 ... bench: 10002431 ns/iter (+/- 0)\n\
             test latency/p99 ... bench: 10002431 ns/iter (+/- 0)\n\
             test latency/max ... bench: 10002431 ns/iter (+/- 0)\n\
             test throughput ... bench: 10000000 ns/iter (+/- 0)\n"
        );
    }

    /// The guard: an open-loop run's rate is set by the flag, so it must not be tracked.
    #[test]
    fn schedule_paced_runs_omit_throughput() {
        let out = render(Pacing::Schedule, 500);
        assert!(out.contains("latency/p99"), "latency is still reported: {out}");
        assert!(!out.contains("throughput"), "scheduled throughput must not be tracked: {out}");
    }

    /// The case the counters cannot reveal: a closed-loop run bounded by the harness
    /// offers nothing and drops nothing, yet its rate is still not the target's.
    #[test]
    fn harness_paced_runs_omit_throughput() {
        let report = report(Pacing::Harness, 0);
        assert_eq!(report.offered, 0, "a closed run has no schedule to offer against");

        let out = render(Pacing::Harness, 0);
        assert!(out.contains("latency/p50"), "latency is still reported: {out}");
        assert!(!out.contains("throughput"), "a concurrency bound is not a platform rate: {out}");
    }

    #[test]
    fn a_prefix_namespaces_every_metric() {
        let mut out = Vec::new();
        BencherReporter::new(Some("registration".into()))
            .print(&mut out, &report(Pacing::Platform, 0), None)
            .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.lines().all(|l| l.starts_with("test registration/")), "{out}");
    }
}
