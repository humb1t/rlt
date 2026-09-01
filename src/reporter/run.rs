//! Reporters for a whole [`RunReport`], as opposed to a single session.
//!
//! The single-session reporters in this module's siblings render one
//! [`BenchReport`](crate::BenchReport). These render the document around them, so a
//! multi-phase run comes out as one artifact instead of a pile of unrelated reports.

use std::io::Write;

use serde::Serialize;

use super::{BencherReporter, ReporterResult};
use crate::run::RunReport;

/// Formats and writes a whole run.
///
/// Generic over the run's metadata so an implementation can require it be serialisable
/// (as [`JsonRunReporter`] does) or ignore it entirely (as [`BencherRunReporter`] does).
pub trait RunReporter<M> {
    /// Format and write the run.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails, or if the run cannot be serialised.
    fn print(&self, w: &mut dyn Write, run: &RunReport<M>) -> ReporterResult<()>;
}

/// Writes a run as one pretty-printed JSON document.
///
/// The phases appear in the order they were recorded, each with its own latency,
/// outcomes and pacing, under whatever metadata the run carries.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonRunReporter;

impl<M: Serialize> RunReporter<M> for JsonRunReporter {
    fn print(&self, w: &mut dyn Write, run: &RunReport<M>) -> ReporterResult<()> {
        serde_json::to_writer_pretty(&mut *w, run)?;
        writeln!(w)?;
        Ok(())
    }
}

/// Writes a run as bencher-format lines, one metric group per phase.
///
/// Each phase's metrics are namespaced by its label, so several phases of one run land
/// in one history without colliding — `<prefix>/<label>/latency/p99`. The pacing guard
/// applies per phase: a phase the target did not pace publishes latency but no
/// throughput, which is what keeps a regression history from moving with a CLI flag.
///
/// A phase that recorded no latency contributes no lines.
#[derive(Debug, Default, Clone)]
pub struct BencherRunReporter {
    prefix: Option<String>,
}

impl BencherRunReporter {
    /// Create a reporter, optionally namespacing every phase with `prefix/`.
    pub fn new(prefix: Option<String>) -> Self {
        Self { prefix }
    }
}

impl<M> RunReporter<M> for BencherRunReporter {
    fn print(&self, w: &mut dyn Write, run: &RunReport<M>) -> ReporterResult<()> {
        for phase in &run.phases {
            let name = match &self.prefix {
                Some(prefix) => format!("{prefix}/{}", phase.label),
                None => phase.label.clone(),
            };
            BencherReporter::new(Some(name)).print_stats(
                w,
                &phase.latency,
                phase.pacing,
                phase.latency.count,
                phase.duration_secs,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BencherRunReporter, JsonRunReporter, RunReporter};
    use crate::run::{PhaseReport, RunReport};
    use crate::schedule::Pacing;

    fn render<M>(reporter: &impl RunReporter<M>, run: &RunReport<M>) -> String {
        let mut out = Vec::new();
        reporter.print(&mut out, run).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn run() -> RunReport<()> {
        let mut run = RunReport::new(());

        let platform = crate::run::tests::bench(20, 0, 2);
        run.record(PhaseReport::builder("registration", &platform).build());

        let mut scheduled = crate::run::tests::bench(30, 40, 29);
        scheduled.pacing = Pacing::Schedule;
        run.record(PhaseReport::builder("steady", &scheduled).build());

        let mut bounded = crate::run::tests::bench(5, 0, 1);
        bounded.pacing = Pacing::Harness;
        run.record(PhaseReport::builder("reconnect", &bounded).build());

        run
    }

    #[test]
    fn every_phase_is_namespaced_by_its_label() {
        let out = render(&BencherRunReporter::new(Some("fleet".into())), &run());

        assert!(out.contains("test fleet/registration/latency/p99 ... bench: "), "{out}");
        assert!(out.contains("test fleet/steady/latency/p50 ... bench: "), "{out}");
        assert!(out.contains("test fleet/reconnect/latency/max ... bench: "), "{out}");
    }

    /// The guard is per phase: only the one the target paced publishes a rate.
    #[test]
    fn only_a_platform_paced_phase_publishes_throughput() {
        let out = render(&BencherRunReporter::new(Some("fleet".into())), &run());

        assert!(out.contains("fleet/registration/throughput"), "{out}");
        assert!(!out.contains("fleet/steady/throughput"), "a scheduled rate is an input: {out}");
        assert!(!out.contains("fleet/reconnect/throughput"), "a bound is not a rate: {out}");
    }

    #[test]
    fn a_phase_with_no_latency_contributes_no_lines() {
        let mut run = RunReport::new(());
        run.record(PhaseReport::builder("failed", &crate::run::tests::bench(0, 0, 0)).build());

        assert!(render(&BencherRunReporter::new(None), &run).is_empty());
    }

    #[test]
    fn json_carries_the_metadata_and_every_phase() {
        #[derive(serde::Serialize)]
        struct Meta {
            agents: usize,
        }

        let mut run = RunReport::new(Meta { agents: 20 });
        run.record(
            PhaseReport::builder("registration", &crate::run::tests::bench(20, 0, 2)).build(),
        );

        let out = render(&JsonRunReporter, &run);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        assert_eq!(parsed["meta"]["agents"], 20);
        assert_eq!(parsed["phases"][0]["label"], "registration");
        assert_eq!(parsed["phases"][0]["pacing"], "platform");
        assert!(parsed["phases"][0]["latency"]["p99"].as_u64().unwrap() > 0);
    }
}
