//! Observe the results of each iteration
//!
//! This modules defines a trait that can be implemented on any type which intends to observe the
//! results of each iteration of benchmark. The observer can do whatever it wishes with the
//! results.
//!
//! # Overview
//! [`Observer`] receives a reference to [`IterReport`] via [`BenchResult`]. It can handle the
//! results as it sees fit. [`Observer`]s can also be chained by calling [`ObserverExt::with`]
//! on any observer. This will call the newer observer first and then the original observer.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc;
use futures::future::OptionFuture;

use crate::clock::Clock;
use crate::{BenchError, IterReport};

/// This defines a type that is interested in results of each iteration of a bench. It will get a
/// notification of those results
pub trait Observer {
    /// This method will be called when one iteration of a bench is complete with the reference
    /// of iteration results
    fn notify(&self, result: Result<&IterReport, &BenchError>) -> impl Future<Output = ()> + Send;
}

/// An extension trait for [`Observer`] so that it can add more layers to the observer as needed
pub trait ObserverExt: Sized {
    /// Add another layer of [`Observer`] to observe the results of a [`crate::BenchSuite`]
    /// iteration and hence process [`IterReport`]
    ///
    /// # Example
    /// ```
    /// # use rlt::observer::ObserverExt as _;
    ///
    /// let empty = Some(());
    /// let chain = empty.with(());
    /// ```
    fn with<L: Observer>(self, layer: L) -> Layered<L, Self> {
        Layered { current: layer, inner: self }
    }
}

impl<T: Observer> ObserverExt for T {}

/// An layer in observation stack. It holds the current [`Observer`] and the lower stack. So will
/// pass the result to this layer and then the lower stack
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Layered<L, I> {
    current: L,
    inner: I,
}

impl<L, I> Observer for Layered<L, I>
where
    L: Observer,
    I: Observer,
{
    fn notify(&self, result: Result<&IterReport, &BenchError>) -> impl Future<Output = ()> + Send {
        let current = self.current.notify(result);
        let inner = self.inner.notify(result);
        async move {
            current.await;
            inner.await
        }
    }
}

impl Observer for () {
    async fn notify(&self, _: Result<&IterReport, &BenchError>) {}
}

impl<T> Observer for Option<T>
where
    T: Observer,
{
    fn notify(&self, result: Result<&IterReport, &BenchError>) -> impl Future<Output = ()> + Send {
        let fut: OptionFuture<_> = self.as_ref().map(|v| v.notify(result)).into();
        async move {
            fut.await;
        }
    }
}

/// Records when each completed iteration finished, so a run keeps a time series.
///
/// [`BenchReport`](crate::BenchReport) carries totals and a latency histogram but no
/// series: the rolling windows in [`stats`](crate::stats) exist to drive the TUI and are
/// compiled out with it. This rebuilds a per-second series from the outside — one
/// timestamp per completed iteration, bucketed by whole second.
///
/// Offsets are taken from the session's [`Clock`], which the runner leaves paused until
/// the benchmark proper begins, so bucket 0 is the first second of measured work rather
/// than the first second of setup, and the series lines up with
/// [`BenchReport::elapsed`](crate::BenchReport::elapsed).
///
/// Clone it into the session and keep a handle; every clone appends to the same log.
///
/// # Example
///
/// ```ignore
/// let opts = BenchOpts::builder().concurrency(8).build()?;
/// let throughput = Throughput::new(opts.clock.clone());
/// let report = BenchSession::new(suite).observer(throughput.clone()).opts(opts).run().await?;
/// let series = throughput.buckets();
/// ```
#[derive(Debug, Clone)]
pub struct Throughput {
    clock: Clock,
    completions: Arc<Mutex<Vec<Duration>>>,
}

impl Throughput {
    /// Observe against the clock the session will run on.
    pub fn new(clock: Clock) -> Self {
        Self { clock, completions: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Completions collapsed into `(second_since_start, count)` pairs.
    ///
    /// Seconds in which nothing completed are absent rather than zero, so the series
    /// stays short when a run is sparse.
    pub fn buckets(&self) -> Vec<(u64, u64)> {
        let completions = self.completions.lock().unwrap_or_else(|poisoned| {
            // A panicking worker must not cost the whole run its series.
            poisoned.into_inner()
        });
        let mut buckets: BTreeMap<u64, u64> = BTreeMap::new();
        for offset in completions.iter() {
            *buckets.entry(offset.as_secs()).or_default() += 1;
        }
        buckets.into_iter().collect()
    }
}

impl Observer for Throughput {
    async fn notify(&self, result: Result<&IterReport, &BenchError>) {
        if result.is_err() {
            // An iteration that never completed has no place on a throughput curve; the
            // failure is already in the report's error distribution.
            return;
        }
        let elapsed = self.clock.elapsed();
        match self.completions.lock() {
            Ok(mut completions) => completions.push(elapsed),
            Err(poisoned) => poisoned.into_inner().push(elapsed),
        }
    }
}

#[derive(Debug, derive_more::From, Clone)]
pub(crate) struct MpscObserver(mpsc::UnboundedSender<Result<IterReport, String>>);

impl Observer for MpscObserver {
    async fn notify(&self, result: Result<&IterReport, &BenchError>) {
        let result = result.cloned().map_err(ToString::to_string);
        if let Err(_error) = self.0.unbounded_send(result) {
            #[cfg(feature = "tracing")]
            log::warn!("Failed to send IterReport; error={_error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Observer, Throughput};
    use crate::clock::Clock;
    use crate::report::IterReport;
    use crate::status::Status;

    fn iteration() -> IterReport {
        IterReport {
            duration: Duration::from_millis(1),
            status: Status::success(0),
            bytes: 0,
            items: 1,
        }
    }

    fn running() -> Throughput {
        let clock = Clock::new_paused();
        clock.resume();
        Throughput::new(clock)
    }

    #[tokio::test]
    async fn completions_land_in_the_first_bucket() {
        let throughput = running();
        let iteration = iteration();
        for _ in 0..3 {
            throughput.notify(Ok(&iteration)).await;
        }
        assert_eq!(throughput.buckets(), vec![(0, 3)]);
    }

    /// Clones share the log, which is what lets the session hold one.
    #[tokio::test]
    async fn a_clone_appends_to_the_same_series() {
        let throughput = running();
        let iteration = iteration();
        throughput.clone().notify(Ok(&iteration)).await;
        throughput.notify(Ok(&iteration)).await;
        assert_eq!(throughput.buckets(), vec![(0, 2)]);
    }

    #[tokio::test]
    async fn failures_are_not_throughput() {
        let throughput = running();
        let error = anyhow::anyhow!("connect refused");
        throughput.notify(Err(&error)).await;
        assert!(throughput.buckets().is_empty());
    }

    /// Offsets come from the clock, not from when the observer was built. This is what
    /// keeps setup out of bucket 0 and the series aligned with the reported elapsed time.
    #[tokio::test]
    async fn offsets_are_measured_against_the_clock() {
        let started = tokio::time::Instant::now() - Duration::from_secs(5);
        let throughput = Throughput::new(Clock::start_at(started));

        throughput.notify(Ok(&iteration())).await;

        assert_eq!(throughput.buckets(), vec![(5, 1)]);
    }

    /// A clock still paused has accumulated nothing, whenever it was created.
    #[tokio::test]
    async fn a_paused_clock_has_not_started() {
        let throughput = Throughput::new(Clock::new_paused());
        throughput.notify(Ok(&iteration())).await;
        assert_eq!(throughput.buckets(), vec![(0, 1)]);
    }
}
