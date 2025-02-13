use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use std::sync::Arc;

use super::Task;
use crate::shaper::Shaper;

/// Shaped per-task worker.
///
/// This worker tries to maintain the required number of requests by polling
/// the [`Shaper`] and executing the given task when required.
#[derive(Debug)]
pub struct ShapedCoroWorker<T> {
    /// Per-task job.
    task: T,
    /// Whether this worker is still active.
    is_running: Arc<AtomicBool>,
    /// The shaper.
    shaper: Shaper,
}

impl<T> ShapedCoroWorker<T> {
    pub fn new(task: T, shaper: Shaper, is_running: Arc<AtomicBool>) -> Self {
        Self { task, shaper, is_running }
    }
}

impl<T> ShapedCoroWorker<T>
where
    T: Task,
{
    pub async fn run(mut self) {
        while self.is_running.load(Ordering::Relaxed) {
            match self.shaper.tick() {
                0 => Self::wait().await,
                mut n => {
                    n = n.min(32);

                    for _ in 0..n {
                        self.task.execute().await;
                    }

                    self.shaper.consume(n);
                }
            }
        }
    }

    #[inline]
    async fn wait() {
        // TODO: park/unpark.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
