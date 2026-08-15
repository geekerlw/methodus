//! Learning-queue driver: claim due jobs, run them, retry with backoff, recover after crash.

use std::path::Path;
use std::sync::Arc;

use chrono::{Duration, Utc};
use methodus_domain::JobStatus;
use methodus_store::Store;

use crate::error::CoreError;
use crate::learning::{self, max_attempts};

const MAX_JOBS_PER_TICK: usize = 16;

/// Drain up to `MAX_JOBS_PER_TICK` due jobs. Returns how many finished (done or failed).
pub fn tick(store: &Arc<Store>, home: &Path) -> Result<usize, CoreError> {
    let _ = learning::unsnooze_due(store)?;
    let mut finished = 0;
    for _ in 0..MAX_JOBS_PER_TICK {
        let Some(job) = store.claim_next_job(Utc::now())? else {
            break;
        };
        match learning::run_job(store, home, &job) {
            Ok(()) => {
                store.update_job_status(&job.id, JobStatus::Done, None)?;
                finished += 1;
            }
            Err(e) => {
                if job.attempts >= max_attempts() {
                    store.update_job_status(&job.id, JobStatus::Failed, None)?;
                    finished += 1;
                } else {
                    let backoff = Duration::seconds(30 * job.attempts.max(1));
                    store.update_job_status(
                        &job.id,
                        JobStatus::Queued,
                        Some(Utc::now() + backoff),
                    )?;
                }
                tracing::warn!("learning job {} failed: {e}", job.id);
            }
        }
    }
    Ok(finished)
}

pub fn recover_jobs(store: &Store) -> Result<usize, CoreError> {
    Ok(store.requeue_running_jobs()?)
}
