//! Multi-phase runs: several sessions recorded as one document.
//!
//! A [`BenchSession`](crate::session::BenchSession) measures one thing and returns one
//! [`BenchReport`]. A realistic load test is usually several — a warm-up and a steady
//! state, a ramp in steps, or a sequence of scenario stages that hand state to each
//! other. The individual reports say nothing about the run they belong to: which phase
//! they were, what order they ran in, or what the run as a whole was configured for.
//!
//! [`RunReport`] is that missing document. It is a **recorder, not a driver**: phases are
//! pushed into it as they finish, so a run whose phases overlap — a churn measurement
//! taken inside a background load's window, say — is recorded exactly as it happened.
//! Nothing here starts a session or decides what runs next.
//!
//! # Example
//!
//! ```ignore
//! #[derive(serde::Serialize)]
//! struct Fleet { agents: usize }
//!
//! let mut run = RunReport::new(Fleet { agents: 1_000 });
//!
//! let report = BenchSession::new(registration).opts(opts.clone()).run().await?;
//! run.record(PhaseReport::builder("registration", &report).build());
//!
//! let report = BenchSession::new(steady).opts(opts).run().await?;
//! run.record(PhaseReport::builder("steady", &report).note("under a 1k fleet").build());
//! ```

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use crate::histogram::LatencyStats;
use crate::report::BenchReport;
use crate::schedule::Pacing;
use crate::status::{Status, StatusKind};

/// How an iteration [`Status`] is named in [`PhaseReport::outcomes`].
///
/// The default is [`Status`]'s `Display`, which yields strings like `Success(0)`.
/// Consumers that map their protocol's codes onto [`Status`] usually want their own
/// names back.
pub type StatusLabel = fn(&Status) -> String;

fn display_status(status: &Status) -> String {
    status.to_string()
}

/// One phase of a run: a session's report, labelled and placed in time.
///
/// Build it with [`PhaseReport::builder`].
#[derive(Debug, Clone, Serialize)]
pub struct PhaseReport {
    /// What this phase was, and the name it is tracked under.
    pub label: String,

    /// When the measured work began.
    pub started_at: DateTime<Utc>,

    /// How long the phase ran, excluding setup and warm-up.
    pub duration_secs: f64,

    /// Workers the session ran with.
    pub concurrency: u32,

    /// Iterations the schedule called for. Zero outside the open load model.
    pub offered: u64,

    /// Iterations that finished, whether they succeeded or failed.
    pub completed: u64,

    /// Iterations that finished successfully.
    pub ok: u64,

    /// Iterations the schedule called for that no worker was free to take.
    pub dropped: u64,

    /// The rate the phase was configured for, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_per_second: Option<u32>,

    /// What set the pace, and so whether this phase's throughput is a result.
    pub pacing: Pacing,

    /// Latency of the iterations that returned one, in nanoseconds.
    pub latency: LatencyStats,

    /// Every outcome the phase saw, by name: statuses and errors in one taxonomy.
    ///
    /// Statuses are named by the [`StatusLabel`] the builder was given; errors keep the
    /// strings they were reported with. A name reached both ways is counted once.
    pub outcomes: BTreeMap<String, u64>,

    /// Completions per second, as `(second_since_start, count)`.
    ///
    /// Empty unless an [`observer::Throughput`](crate::observer::Throughput) was attached
    /// to the session and its buckets handed to the builder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub throughput_per_sec: Vec<(u64, u64)>,

    /// Anything about this phase worth carrying into the write-up.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl PhaseReport {
    /// Start building a phase entry from a finished session's report.
    pub fn builder<'a>(label: impl Into<String>, report: &'a BenchReport) -> PhaseBuilder<'a> {
        PhaseBuilder {
            label: label.into(),
            report,
            started_at: None,
            status_label: display_status,
            offered: None,
            throughput_per_sec: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Iterations that finished per second over the phase.
    ///
    /// Read it against [`pacing`](Self::pacing): under
    /// [`Schedule`](Pacing::Schedule) or [`Harness`](Pacing::Harness) this is a property
    /// of the driver, not of the target.
    pub fn throughput(&self) -> f64 {
        if self.duration_secs <= 0.0 {
            return 0.0;
        }
        self.ok as f64 / self.duration_secs
    }
}

/// Builder for a [`PhaseReport`]. See [`PhaseReport::builder`].
#[derive(Debug, Clone)]
pub struct PhaseBuilder<'a> {
    label: String,
    report: &'a BenchReport,
    started_at: Option<DateTime<Utc>>,
    status_label: StatusLabel,
    offered: Option<u64>,
    throughput_per_sec: Vec<(u64, u64)>,
    notes: Vec<String>,
}

impl PhaseBuilder<'_> {
    /// When the measured work began.
    ///
    /// Defaults to now minus the session's elapsed time, which is right for a phase
    /// recorded as soon as it finishes. Set it when the real start was recorded.
    pub fn started_at(mut self, started_at: DateTime<Utc>) -> Self {
        self.started_at = Some(started_at);
        self
    }

    /// How to name an iteration [`Status`] in the outcome taxonomy.
    pub fn status_label(mut self, status_label: StatusLabel) -> Self {
        self.status_label = status_label;
        self
    }

    /// Attach a per-second series, normally from
    /// [`observer::Throughput::buckets`](crate::observer::Throughput::buckets).
    pub fn throughput(mut self, buckets: Vec<(u64, u64)>) -> Self {
        self.throughput_per_sec = buckets;
        self
    }

    /// Declare what the phase was asked to do, when nothing offered it.
    ///
    /// In the open load model the schedule is what asks, and
    /// [`offered`](PhaseReport::offered) comes from its counter. A closed-loop phase has
    /// no schedule, but it usually still has a target the caller knows — a fleet to
    /// enrol, a slice to reconnect — and the gap between that and
    /// [`completed`](PhaseReport::completed) is the thing worth reading. Declare it here
    /// and the two models report the same shape.
    pub fn offered(mut self, offered: u64) -> Self {
        self.offered = Some(offered);
        self
    }

    /// Add a note. Call it more than once for more than one.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Add several notes at once.
    pub fn notes<I>(mut self, notes: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.notes.extend(notes.into_iter().map(Into::into));
        self
    }

    /// Reduce the session's report into a phase entry.
    pub fn build(self) -> PhaseReport {
        let report = self.report;
        let errors: u64 = report.error_dist.values().sum();
        let ok: u64 = report
            .stats
            .by_status
            .iter()
            .filter(|(status, _)| status.kind() == StatusKind::Success)
            .map(|(_, counter)| counter.iters)
            .sum();

        let mut outcomes: BTreeMap<String, u64> = BTreeMap::new();
        for (status, count) in &report.status_dist {
            *outcomes.entry((self.status_label)(status)).or_default() += count;
        }
        for (error, count) in &report.error_dist {
            *outcomes.entry(error.clone()).or_default() += count;
        }

        let started_at = self.started_at.unwrap_or_else(|| {
            let elapsed = TimeDelta::from_std(report.elapsed).unwrap_or(TimeDelta::zero());
            Utc::now() - elapsed
        });

        PhaseReport {
            label: self.label,
            started_at,
            duration_secs: report.elapsed.as_secs_f64(),
            concurrency: report.concurrency,
            offered: self.offered.unwrap_or(report.offered),
            completed: report.stats.overall.iters + errors,
            ok,
            dropped: report.dropped,
            rate_per_second: report.rate_per_second,
            pacing: report.pacing,
            latency: LatencyStats::from(&report.hist),
            outcomes,
            throughput_per_sec: self.throughput_per_sec,
            notes: self.notes,
        }
    }
}

/// Everything one invocation measured, in the order it measured it.
///
/// `M` carries whatever the run as a whole was configured for — the fleet size, the
/// target, the scenario — and is serialised alongside the phases. It defaults to `()`
/// for a run with nothing to say about itself.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport<M = ()> {
    /// When the run began.
    pub started_at: DateTime<Utc>,

    /// What the run as a whole was configured for.
    pub meta: M,

    /// The phases, in the order they were recorded.
    pub phases: Vec<PhaseReport>,
}

impl<M> RunReport<M> {
    /// Start a run now.
    pub fn new(meta: M) -> Self {
        Self { started_at: Utc::now(), meta, phases: Vec::new() }
    }

    /// Start a run that began at a known time.
    pub fn started_at(started_at: DateTime<Utc>, meta: M) -> Self {
        Self { started_at, meta, phases: Vec::new() }
    }

    /// Record a finished phase.
    pub fn record(&mut self, phase: PhaseReport) {
        self.phases.push(phase);
    }

    /// The phase with the given label, if it was recorded.
    pub fn phase(&self, label: &str) -> Option<&PhaseReport> {
        self.phases.iter().find(|phase| phase.label == label)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::{PhaseReport, RunReport};
    use crate::histogram::LatencyHistogram;
    use crate::report::{BenchReport, IterReport};
    use crate::schedule::Pacing;
    use crate::stats::IterStats;
    use crate::status::{Status, StatusKind};

    /// A finished session with `iters` successful 10ms iterations.
    pub(crate) fn bench(iters: u64, offered: u64, elapsed_secs: u64) -> BenchReport {
        let mut hist = LatencyHistogram::new();
        let mut stats = IterStats::new();
        let mut status_dist = HashMap::default();
        for _ in 0..iters {
            let iter = IterReport {
                duration: Duration::from_millis(10),
                status: Status::success(0),
                bytes: 0,
                items: 1,
            };
            hist.record(iter.duration).unwrap();
            stats.record(&iter);
            *status_dist.entry(iter.status).or_default() += 1;
        }
        BenchReport {
            concurrency: 4,
            hist,
            stats,
            status_dist,
            error_dist: HashMap::default(),
            elapsed: Duration::from_secs(elapsed_secs),
            offered,
            dropped: offered.saturating_sub(iters),
            pacing: Pacing::Platform,
            rate_per_second: None,
        }
    }

    #[test]
    fn a_session_report_becomes_a_phase_entry() {
        let phase = PhaseReport::builder("registration", &bench(20, 0, 2)).build();

        assert_eq!(phase.label, "registration");
        assert_eq!(phase.concurrency, 4);
        assert_eq!(phase.completed, 20);
        assert_eq!(phase.ok, 20);
        assert_eq!(phase.dropped, 0);
        assert_eq!(phase.latency.count, 20);
        assert!(phase.latency.p99 > 0);
        assert_eq!(phase.throughput(), 10.0);
    }

    /// Iterations that failed finished, but they produced no latency.
    #[test]
    fn errors_are_completed_but_neither_ok_nor_latency() {
        let mut bench = bench(3, 0, 1);
        bench.error_dist.insert("client/connect".to_owned(), 2);

        let phase = PhaseReport::builder("registration", &bench).build();

        assert_eq!(phase.completed, 5);
        assert_eq!(phase.ok, 3);
        assert_eq!(phase.latency.count, 3);
        assert_eq!(phase.outcomes.get("client/connect"), Some(&2));
        assert_eq!(phase.outcomes.get("Success(0)"), Some(&3));
    }

    /// Statuses and errors share one taxonomy, under the consumer's own names.
    #[test]
    fn a_status_label_renames_the_outcomes() {
        fn label(status: &Status) -> String {
            match status.kind() {
                StatusKind::Success => "ok".to_owned(),
                _ => format!("code/{}", status.code()),
            }
        }

        let mut bench = bench(3, 0, 1);
        *bench.status_dist.entry(Status::server_error(13)).or_default() += 1;
        bench.error_dist.insert("client/connect".to_owned(), 2);

        let phase = PhaseReport::builder("spam", &bench).status_label(label).build();

        assert_eq!(phase.outcomes.get("ok"), Some(&3));
        assert_eq!(phase.outcomes.get("code/13"), Some(&1));
        assert_eq!(phase.outcomes.get("client/connect"), Some(&2));
    }

    #[test]
    fn the_shortfall_survives_into_the_phase() {
        let mut bench = bench(30, 40, 29);
        bench.pacing = Pacing::Schedule;
        bench.rate_per_second = Some(1);

        let phase = PhaseReport::builder("steady", &bench).build();

        assert_eq!(phase.offered, 40);
        assert_eq!(phase.dropped, 10);
        assert_eq!(phase.pacing, Pacing::Schedule);
        assert_eq!(phase.rate_per_second, Some(1));
    }

    /// Phases are recorded as they finish, so an overlapping run stays truthful.
    #[test]
    fn a_run_keeps_the_order_it_was_given() {
        #[derive(serde::Serialize)]
        struct Meta {
            agents: usize,
        }

        let mut run = RunReport::new(Meta { agents: 20 });
        run.record(PhaseReport::builder("background", &bench(5, 0, 1)).build());
        run.record(PhaseReport::builder("handshake", &bench(5, 0, 1)).note("under load").build());

        let labels: Vec<_> = run.phases.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["background", "handshake"]);
        assert_eq!(
            run.phase("handshake").map(|p| p.notes.as_slice()),
            Some(&["under load".to_owned()][..])
        );
        assert!(run.phase("missing").is_none());
    }

    /// A closed-loop phase has no schedule, but it still has a target.
    #[test]
    fn a_declared_target_fills_in_for_a_schedule() {
        let bench = bench(17, 0, 2);
        assert_eq!(bench.offered, 0, "a closed run offers nothing");

        let phase = PhaseReport::builder("registration", &bench).offered(20).build();

        assert_eq!(phase.offered, 20);
        assert_eq!(phase.completed, 17);
    }

    #[test]
    fn notes_accumulate_however_they_are_added() {
        let phase = PhaseReport::builder("steady", &bench(1, 0, 1))
            .note("first")
            .notes(["second", "third"])
            .build();

        assert_eq!(phase.notes, ["first", "second", "third"]);
    }

    /// The default start is the phase's own, not the moment it was recorded.
    #[test]
    fn a_phase_is_dated_from_when_it_started() {
        let before = chrono::Utc::now();
        let phase = PhaseReport::builder("steady", &bench(1, 0, 30)).build();

        assert!(phase.started_at < before, "a 30s phase cannot have started after it ended");
    }
}
