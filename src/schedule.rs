//! Open-loop scheduling: an arrival schedule that does not wait for the workers.
//!
//! In the [closed](LoadModel::Closed) model — rlt's original behaviour — every worker
//! runs one iteration after another, and `--rate` only caps how fast a *free* worker may
//! start the next one. When the target slows down, workers block in `bench`, fewer
//! iterations start, and the benchmark quietly settles at whatever rate the target can
//! serve. The report then shows a healthy-looking rate that is really the target's
//! capacity, and the shortfall against the requested rate is nowhere.
//!
//! In the [open](LoadModel::Open) model the schedule is authoritative. A dispatcher owns
//! the clock and releases work when the schedule says it is due:
//!
//! * the number of iterations due is derived from an **absolute** schedule
//!   (`elapsed × rate`), never incremented per tick, so a late tick cannot accumulate
//!   drift;
//! * when no worker is free the iteration is **dropped and counted**, never awaited.
//!
//! Backpressure therefore surfaces as [`ScheduleCounters::dropped`] instead of a lower
//! rate, which is what makes an overloaded target distinguishable from a slow one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::select;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::clock::Clock;

/// How often the dispatcher wakes to release due work.
///
/// Fine enough to spread a high rate smoothly, coarse enough to stay cheap. The absolute
/// schedule means the interval affects only burstiness, never the total.
const TICK: Duration = Duration::from_millis(10);

/// How iterations are paced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum LoadModel {
    /// Workers run back to back; `rate` caps how fast a free worker may start again.
    ///
    /// The rate the report shows is the rate the target sustained.
    #[default]
    Closed,

    /// The schedule releases work at the requested rate whether or not a worker is free.
    ///
    /// Iterations that find no free worker are counted as dropped. Requires a rate.
    Open,
}

/// Counts of what the open-loop schedule asked for and what it could not place.
///
/// Both are zero in the [closed](LoadModel::Closed) model, where the schedule never asks
/// for more than the workers can take.
#[derive(Debug, Default)]
pub struct ScheduleCounters {
    offered: AtomicU64,
    dropped: AtomicU64,
}

impl ScheduleCounters {
    /// Iterations the schedule called for.
    pub fn offered(&self) -> u64 {
        self.offered.load(Ordering::Relaxed)
    }

    /// Iterations no worker was free to take — the shortfall.
    ///
    /// A non-zero value means the requested rate was not delivered. It says nothing on
    /// its own about *why*: the target may be saturated, or the load generator may be.
    /// Both are worth knowing, and neither is visible if the shortfall is smoothed away
    /// into a lower rate.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Release iterations on an absolute schedule until the run ends.
///
/// Ends when `duration` of clock time has passed, when `iterations` have been offered, or
/// when `cancel` fires — whichever comes first. Dropping the sender is how the workers
/// learn the run is over.
pub(crate) async fn dispatch(
    clock: Clock,
    rate_per_second: f64,
    duration: Option<Duration>,
    iterations: Option<u64>,
    tx: mpsc::Sender<u64>,
    counters: Arc<ScheduleCounters>,
    cancel: CancellationToken,
) {
    if rate_per_second <= 0.0 {
        return;
    }

    let mut ticker = clock.ticker(TICK);
    let mut released: u64 = 0;
    loop {
        select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => (),
        }

        let elapsed = clock.elapsed();
        if duration.is_some_and(|d| elapsed >= d) {
            break;
        }

        // Message k is due at t = k / rate. A tick releases everything that comes due
        // before the next one, so nothing waits a whole tick past its due time and the
        // total does not shrink as the rate rises: ceil((elapsed + TICK) × rate).
        //
        // Deriving that from the clock rather than counting ticks is what keeps a late
        // tick from losing work; the cost is that work may start up to one tick early.
        let mut due = ((elapsed + TICK).as_secs_f64() * rate_per_second).ceil() as u64;
        if let Some(iterations) = iterations {
            due = due.min(iterations);
        }

        while released < due {
            let seq = released;
            released += 1;
            counters.offered.fetch_add(1, Ordering::Relaxed);
            // The whole point: never await a full channel.
            if tx.try_send(seq).is_err() {
                counters.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }

        if iterations.is_some_and(|n| released >= n) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::{ScheduleCounters, dispatch};
    use crate::clock::Clock;

    /// Ten per second for ten seconds is a hundred, whatever the consumer does.
    #[tokio::test(start_paused = true)]
    async fn offers_what_the_schedule_calls_for() {
        let (tx, mut rx) = mpsc::channel(4096);
        let counters = Arc::new(ScheduleCounters::default());
        let clock = Clock::new_paused();
        clock.resume();

        let drain = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(seq) = rx.recv().await {
                seen.push(seq);
            }
            seen
        });

        dispatch(
            clock,
            10.0,
            Some(Duration::from_secs(10)),
            None,
            tx,
            Arc::clone(&counters),
            CancellationToken::new(),
        )
        .await;
        let seen = drain.await.expect("drain task");

        assert_eq!(counters.offered(), 100, "offered must be rate x elapsed");
        assert_eq!(counters.dropped(), 0, "a consumer that keeps up must lose nothing");
        assert_eq!(seen, (0..100).collect::<Vec<_>>(), "sequence must be dense and in order");
    }

    /// The regression test for the whole module: a stalled consumer must make the
    /// dispatcher drop and count, never slow the schedule down.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_consumer_causes_drops_not_throttling() {
        // Capacity one and nobody receiving: everything past the first has nowhere to go.
        let (tx, _rx) = mpsc::channel::<u64>(1);
        let counters = Arc::new(ScheduleCounters::default());
        let clock = Clock::new_paused();
        clock.resume();

        dispatch(
            clock,
            10.0,
            Some(Duration::from_secs(10)),
            None,
            tx,
            Arc::clone(&counters),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(counters.offered(), 100, "the schedule must keep pace regardless");
        assert_eq!(counters.dropped(), 99, "the shortfall must be counted, not absorbed");
    }

    /// A late tick must release everything the absolute schedule says is due, so the
    /// total never drifts with the tick interval.
    #[tokio::test(start_paused = true)]
    async fn a_rate_far_above_the_tick_interval_still_adds_up() {
        let (tx, mut rx) = mpsc::channel(65536);
        let counters = Arc::new(ScheduleCounters::default());
        let clock = Clock::new_paused();
        clock.resume();

        let drain = tokio::spawn(async move {
            let mut seen = 0u64;
            while rx.recv().await.is_some() {
                seen += 1;
            }
            seen
        });

        // 5,000/s against a 10ms tick: 50 per tick.
        dispatch(
            clock,
            5000.0,
            Some(Duration::from_secs(2)),
            None,
            tx,
            Arc::clone(&counters),
            CancellationToken::new(),
        )
        .await;
        let seen = drain.await.expect("drain task");

        assert_eq!(counters.offered(), 10_000);
        assert_eq!(seen, 10_000);
    }

    #[tokio::test(start_paused = true)]
    async fn an_iteration_limit_ends_the_schedule_early() {
        let (tx, mut rx) = mpsc::channel(4096);
        let counters = Arc::new(ScheduleCounters::default());
        let clock = Clock::new_paused();
        clock.resume();

        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

        dispatch(
            clock,
            10.0,
            Some(Duration::from_secs(600)),
            Some(25),
            tx,
            Arc::clone(&counters),
            CancellationToken::new(),
        )
        .await;
        drain.await.expect("drain task");

        assert_eq!(counters.offered(), 25, "the iteration limit caps the schedule");
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_stops_the_schedule() {
        let (tx, mut rx) = mpsc::channel(4096);
        let counters = Arc::new(ScheduleCounters::default());
        let clock = Clock::new_paused();
        clock.resume();
        let cancel = CancellationToken::new();

        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let canceller = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                cancel.cancel();
            })
        };

        dispatch(
            clock,
            10.0,
            Some(Duration::from_secs(600)),
            None,
            tx,
            Arc::clone(&counters),
            cancel,
        )
        .await;
        canceller.await.expect("canceller");
        drain.await.expect("drain task");

        assert!(counters.offered() < 20, "cancelled after ~1s, got {}", counters.offered());
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_rate_offers_nothing() {
        let (tx, _rx) = mpsc::channel::<u64>(1);
        let counters = Arc::new(ScheduleCounters::default());
        let clock = Clock::new_paused();
        clock.resume();

        dispatch(
            clock,
            0.0,
            Some(Duration::from_secs(10)),
            None,
            tx,
            Arc::clone(&counters),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(counters.offered(), 0);
    }
}
