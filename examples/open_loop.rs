//! The same saturated target under both load models.
//!
//! The target here has a hard ceiling: eight concurrent slots, 100ms of work each, so
//! 80 iterations per second and no more. Ask for 200/s and the two models answer
//! differently:
//!
//! * closed — workers block on the target, ~80/s is reported, and the 120/s that never
//!   happened is nowhere in the output;
//! * open — the schedule still asks for 200/s, and the iterations no worker was free to
//!   take are reported as dropped.
//!
//! Run it with `cargo run --release --example open_loop`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rlt::session::BenchSession;
use rlt::{
    BenchOpts, BenchResult, IterInfo, IterReport, LoadModel, Result, StatelessBenchSuite, Status,
};
use tokio::sync::Semaphore;

/// Eight slots, 100ms each: a target that cannot go faster than 80/s.
#[derive(Clone)]
struct Saturating {
    capacity: Arc<Semaphore>,
}

impl StatelessBenchSuite for Saturating {
    async fn bench(&mut self, _: &IterInfo) -> BenchResult<IterReport> {
        let t = Instant::now();
        let _permit = self.capacity.acquire().await.expect("semaphore");
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(IterReport { duration: t.elapsed(), status: Status::success(0), bytes: 0, items: 1 })
    }
}

async fn run(load_model: LoadModel) -> Result<()> {
    let opts = BenchOpts::builder()
        .concurrency(64)
        .duration(Duration::from_secs(10))
        .rate(200)
        .load_model(load_model)
        .build()?;

    let suite = Saturating { capacity: Arc::new(Semaphore::new(8)) };
    let report = BenchSession::new(suite).opts(opts).run().await?;

    let elapsed = report.elapsed.as_secs_f64();
    println!(
        "{load_model:?}: served {} ({:.0}/s), offered {}, dropped {}",
        report.stats.overall.iters,
        report.stats.overall.iters as f64 / elapsed,
        report.offered,
        report.dropped,
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("target ceiling: 8 slots x 100ms = 80/s; asking for 200/s\n");
    run(LoadModel::Closed).await?;
    run(LoadModel::Open).await?;
    Ok(())
}
