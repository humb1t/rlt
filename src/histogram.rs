//! A simple wrapper around [`hdrhistogram::Histogram`] for latency measurements.
use std::time::Duration;

use hdrhistogram::Histogram;

use crate::error::CollectorError;

/// The percentile set the built-in reporters render.
pub const PERCENTAGES: &[f64] = &[10.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0, 99.9, 99.99];

/// A simple wrapper around [`hdrhistogram::Histogram`] for latency measurements.
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    hist: Histogram<u64>,
}

impl LatencyHistogram {
    /// Creates a new latency histogram.
    pub fn new() -> LatencyHistogram {
        Self { hist: Histogram::<u64>::new(3).expect("create histogram") }
    }

    /// Records a latency value.
    pub fn record(&mut self, d: Duration) -> std::result::Result<(), CollectorError> {
        let nanos = u64::try_from(d.as_nanos())
            .map_err(|_| CollectorError::LatencyTooLarge { latency: d })?;
        self.hist.record(nanos).map_err(CollectorError::HistogramRecord)
    }

    /// Returns true if this histogram has no recorded values.
    pub fn is_empty(&self) -> bool {
        self.hist.is_empty()
    }

    /// The number of latencies recorded.
    ///
    /// Only iterations that returned a value are recorded, so this is the sample count
    /// the percentiles were computed from — not the number of iterations attempted.
    pub fn len(&self) -> u64 {
        self.hist.len()
    }

    /// Get the highest recorded latency in the histogram.
    pub fn max(&self) -> Duration {
        Duration::from_nanos(self.hist.max())
    }

    /// Get the lowest recorded latency in the histogram.
    pub fn min(&self) -> Duration {
        Duration::from_nanos(self.hist.min())
    }

    /// Get the computed mean value of all recorded latencies in the histogram.
    pub fn mean(&self) -> Duration {
        Duration::from_nanos(self.hist.mean() as u64)
    }

    /// Get the computed standard deviation of all recorded latencies in the histogram.
    pub fn stdev(&self) -> Duration {
        Duration::from_nanos(self.hist.stdev() as u64)
    }

    /// Get the computed median value of all recorded latencies in the histogram.
    pub fn median(&self) -> Duration {
        self.value_at_quantile(0.5)
    }

    /// Get the latency at a given quantile.
    pub fn value_at_quantile(&self, q: f64) -> Duration {
        Duration::from_nanos(self.hist.value_at_quantile(q))
    }

    /// Iterate through histogram values by quantile levels.
    ///
    /// See [`hdrhistogram::Histogram::iter_quantiles`] for more details.
    pub fn quantiles(&self) -> impl Iterator<Item = (Duration, u64)> + '_ {
        self.hist
            .iter_quantiles(1)
            .map(|t| (Duration::from_nanos(t.value_iterated_to()), t.count_since_last_iteration()))
            .filter(|(_, n)| *n > 0)
    }

    /// Compute each latency value at the given percentages.
    pub fn percentiles<'a>(
        &'a self,
        percentages: &'a [f64],
    ) -> impl Iterator<Item = (f64, Duration)> + 'a {
        percentages.iter().map(|&p| (p, self.value_at_quantile(p / 100.0)))
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// A latency distribution reduced to the figures a report carries, in nanoseconds.
///
/// The reporters in this crate each serialise latency in the shape their own output
/// format committed to — seconds as `f64` for JSON and for saved baselines. This is the
/// shape for library consumers: integer nanoseconds, one struct, computed once.
///
/// Built with [`LatencyStats::from`] over a [`LatencyHistogram`]. An empty histogram
/// yields all zeroes rather than an error, so a phase that recorded nothing still
/// reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct LatencyStats {
    /// Number of latencies the figures below were computed from.
    pub count: u64,
    /// Lowest recorded latency.
    pub min: u64,
    /// Arithmetic mean of the recorded latencies.
    pub mean: u64,
    /// Standard deviation of the recorded latencies.
    pub stdev: u64,
    /// Median.
    pub p50: u64,
    /// 90th percentile.
    pub p90: u64,
    /// 95th percentile.
    pub p95: u64,
    /// 99th percentile.
    pub p99: u64,
    /// Highest recorded latency.
    pub max: u64,
}

impl From<&LatencyHistogram> for LatencyStats {
    fn from(hist: &LatencyHistogram) -> Self {
        if hist.is_empty() {
            return Self::default();
        }
        // Every value in the histogram was recorded from a Duration's nanos as a u64,
        // so nothing here can exceed what a u64 holds.
        let ns = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        Self {
            count: hist.len(),
            min: ns(hist.min()),
            mean: ns(hist.mean()),
            stdev: ns(hist.stdev()),
            p50: ns(hist.median()),
            p90: ns(hist.value_at_quantile(0.90)),
            p95: ns(hist.value_at_quantile(0.95)),
            p99: ns(hist.value_at_quantile(0.99)),
            max: ns(hist.max()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LatencyHistogram, LatencyStats};

    #[test]
    fn stats_come_off_the_histogram() {
        let mut hist = LatencyHistogram::new();
        for ms in 1..=100 {
            hist.record(std::time::Duration::from_millis(ms)).unwrap();
        }

        let stats = LatencyStats::from(&hist);
        assert_eq!(stats.count, 100);
        assert!(stats.min <= stats.p50, "{stats:?}");
        assert!(stats.p50 <= stats.p90, "{stats:?}");
        assert!(stats.p90 <= stats.p99, "{stats:?}");
        assert!(stats.p99 <= stats.max, "{stats:?}");
        // The histogram is accurate to three significant figures.
        assert!(stats.max.abs_diff(100_000_000) < 1_000_000, "{stats:?}");
    }

    /// A phase that recorded nothing still has to report something.
    #[test]
    fn an_empty_histogram_yields_zeroes() {
        assert_eq!(LatencyStats::from(&LatencyHistogram::new()), LatencyStats::default());
    }
}
