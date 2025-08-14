use core::{future::Future, num::NonZero};
use std::thread::Builder;

use anyhow::Error;
use tokio::runtime::LocalRuntime;

/// Represents a thread pool for running workers each in a separate thread.
#[derive(Debug)]
pub struct ThreadPool<F> {
    num_threads: NonZero<usize>,
    factory: F,
}

impl<F> ThreadPool<F> {
    pub fn new(num_threads: NonZero<usize>, factory: F) -> Self {
        Self { num_threads, factory }
    }
}

impl<F, U> ThreadPool<F>
where
    F: FnMut(usize) -> U,
    U: FnOnce() -> Result<(), Error> + Send + 'static,
{
    /// Runs this [`ThreadPool`] by spawning threads and waiting for them to
    /// complete.
    pub fn run(mut self) -> Result<(), Error> {
        let num_threads = self.num_threads.get();
        let mut threads = Vec::with_capacity(num_threads);

        let name = "dwd:w".to_string();
        for idx in 0..num_threads {
            let thread = {
                let worker = (self.factory)(idx);

                Builder::new().name(name.clone()).spawn(worker)?
            };

            threads.push(thread);
        }

        for thread in threads {
            thread.join().expect("no self join")?;
        }

        Ok(())
    }
}

/// Per-CPU task set.
#[derive(Debug)]
pub struct LocalTaskPool<F> {
    num_tasks: NonZero<usize>,
    factory: F,
}

impl<F> LocalTaskPool<F> {
    pub fn new(num_tasks: NonZero<usize>, factory: F) -> Self {
        Self { factory, num_tasks }
    }
}

impl<F, T> LocalTaskPool<F>
where
    F: FnMut(usize) -> T,
    T: Future + 'static,
{
    pub fn run(mut self) -> Result<(), Error> {
        let runtime = LocalRuntime::new()?;

        let num_tasks = self.num_tasks.get();
        let mut jobs = Vec::with_capacity(num_tasks);

        for idx in 0..num_tasks {
            let job = runtime.spawn_local((self.factory)(idx));

            jobs.push(job);
        }

        let future = async move {
            for job in jobs {
                job.await.unwrap();
            }
        };

        runtime.block_on(future);

        Ok(())
    }
}
