//! Two benchmark phases in one process, with state minted by the first phase and reused by
//! the second.
//!
//! This is the shape a fleet driver needs: phase one enrols workers and keeps whatever
//! identity the server hands back, phase two drives traffic with those identities. It works
//! because [`BenchSession`] returns its report in-process instead of printing and exiting,
//! and because the identities live outside the session — a session tears every worker down
//! when it ends.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rlt::session::BenchSession;
use rlt::{BenchOpts, BenchReport, BenchResult, BenchSuite, IterInfo, IterReport, Result, Status};

const WORKERS: u32 = 8;

/// What enrolment hands back for a worker: the credential the second phase sends with.
#[derive(Debug, Clone)]
struct Identity {
    worker_id: u32,
    credential: String,
}

/// Phase one: each worker enrols once and keeps its own credential.
#[derive(Clone)]
struct Enrol {
    minted: Arc<Mutex<Vec<Identity>>>,
}

impl BenchSuite for Enrol {
    type WorkerState = Identity;

    async fn setup(&mut self, worker_id: u32) -> BenchResult<Identity> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let identity = Identity { worker_id, credential: format!("cred-{worker_id:04}") };
        self.minted.lock().expect("minted lock").push(identity.clone());
        Ok(identity)
    }

    async fn bench(&mut self, state: &mut Identity, info: &IterInfo) -> BenchResult<IterReport> {
        assert_eq!(state.worker_id, info.worker_id, "worker state must not migrate");
        let t = Instant::now();
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(IterReport {
            duration: t.elapsed(),
            status: Status::success(0),
            bytes: state.credential.len() as u64,
            items: 1,
        })
    }
}

/// Phase two: the credentials already exist, so a worker only picks the one it owns.
#[derive(Clone)]
struct Traffic {
    identities: Arc<Vec<Identity>>,
}

impl BenchSuite for Traffic {
    type WorkerState = Identity;

    async fn setup(&mut self, worker_id: u32) -> BenchResult<Identity> {
        self.identities
            .get(worker_id as usize)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no identity for worker {worker_id}"))
    }

    async fn bench(&mut self, state: &mut Identity, _: &IterInfo) -> BenchResult<IterReport> {
        let t = Instant::now();
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(IterReport {
            duration: t.elapsed(),
            status: Status::success(0),
            bytes: state.credential.len() as u64,
            items: 1,
        })
    }
}

fn opts() -> BenchOpts {
    BenchOpts::builder()
        .concurrency(WORKERS)
        .duration(Duration::from_secs(1))
        .build()
        .expect("bench opts")
}

fn summarize(phase: &str, report: &BenchReport) {
    println!(
        "{phase}: iters={} elapsed={:?} success_ratio={:.2}",
        report.stats.overall.iters,
        report.elapsed,
        report.success_ratio()
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let minted = Arc::new(Mutex::new(Vec::new()));
    let enrol = BenchSession::new(Enrol { minted: Arc::clone(&minted) }).opts(opts());
    let report = enrol.run().await?;
    summarize("enrol", &report);

    let identities = Arc::new(minted.lock().expect("minted lock").clone());
    assert_eq!(identities.len(), WORKERS as usize, "one enrolment per worker");
    let mut ids: Vec<u32> = identities.iter().map(|i| i.worker_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), WORKERS as usize, "worker ids must be distinct");

    let send = BenchSession::new(Traffic { identities }).opts(opts());
    let report = send.run().await?;
    summarize("send", &report);

    println!("both phases ran in-process with credentials carried between them");
    Ok(())
}
