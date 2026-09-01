//! The benchmark report module.
use std::collections::HashMap;

use tokio::time::Duration;

use crate::BenchOpts;
use crate::histogram::LatencyHistogram;
use crate::schedule::Pacing;
use crate::stats::IterStats;
use crate::status::{Status, StatusKind};

/// The iteration report.
#[derive(Debug, Clone)]
pub struct IterReport {
    /// The reported duration of the iteration.
    pub duration: Duration,
    /// The reported status of the iteration.
    pub status: Status,
    /// The reported processed bytes of the iteration.
    pub bytes: u64,
    /// The reported processed items of the iteration. Useful when testing services with batch support.
    pub items: u64,
}

/// The final benchmark report.
#[derive(Debug, Clone)]
pub struct BenchReport {
    /// Number of workers to run concurrently
    pub concurrency: u32,
    /// Iteration latency histogram.
    pub hist: LatencyHistogram,
    /// Iteration statistics.
    pub stats: IterStats,
    /// Status distribution.
    pub status_dist: HashMap<Status, u64>,
    /// Error distribution.
    pub error_dist: HashMap<String, u64>,
    /// The total elapsed time of the benchmark.
    pub elapsed: Duration,

    /// Iterations the schedule called for, in the [open](crate::LoadModel::Open) model.
    ///
    /// Zero in the closed model, where the workers set the pace and nothing can be
    /// asked for that is not also started.
    pub offered: u64,

    /// Iterations the schedule called for that no worker was free to take.
    ///
    /// The shortfall against the requested rate. Zero in the closed model.
    pub dropped: u64,

    /// What set the pace, and therefore whether throughput is a result worth tracking.
    ///
    /// Carried through from [`BenchOpts::pacing`](crate::BenchOpts::pacing) so a
    /// reporter never has to guess it from the counters. See [`Pacing`].
    pub pacing: Pacing,

    /// The rate the run was configured for, in iterations per second.
    ///
    /// `None` when no rate was set. Reported so a paced run's numbers can be read
    /// against what was asked of it, without the caller having to keep the options
    /// alongside the report.
    pub rate_per_second: Option<u32>,
}

impl From<BenchOpts> for BenchReport {
    fn from(opts: BenchOpts) -> Self {
        #[cfg(feature = "rate_limit")]
        let rate_per_second = opts.rate.map(|rate| rate.get());
        #[cfg(not(feature = "rate_limit"))]
        let rate_per_second = None;

        Self {
            concurrency: opts.concurrency,
            hist: LatencyHistogram::default(),
            stats: IterStats::default(),
            status_dist: HashMap::default(),
            error_dist: HashMap::default(),
            elapsed: Duration::ZERO,
            offered: 0,
            dropped: 0,
            pacing: opts.pacing,
            rate_per_second,
        }
    }
}

impl BenchReport {
    /// Returns the success ratio of the benchmark.
    pub fn success_ratio(&self) -> f64 {
        if self.stats.overall.iters == 0 {
            return 0.0;
        }
        self.stats
            .by_status
            .iter()
            .filter(|(k, _)| k.kind() == StatusKind::Success)
            .map(|(_, v)| v.iters as f64)
            .sum::<f64>()
            / self.stats.overall.iters as f64
    }
}
