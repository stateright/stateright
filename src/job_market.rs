use parking_lot::{Condvar, Mutex};
use std::{collections::VecDeque, sync::Arc, thread::sleep, time::SystemTime};

/// A market for synchronising the sharing of jobs.
///
/// Maintains synchronisation for multiple threads, including shutdown behaviour once one finishes
/// or panics.
pub(crate) struct JobBroker<Job> {
    /// Get notified when there is a new job to handle.
    has_new_jobs: Arc<Condvar>,
    /// The market that we share.
    market: Arc<Mutex<JobMarket<Job>>>,
}

impl<Job> Clone for JobBroker<Job> {
    fn clone(&self) -> Self {
        Self {
            has_new_jobs: Arc::clone(&self.has_new_jobs),
            market: Arc::clone(&self.market),
        }
    }
}

impl<Job> Drop for JobBroker<Job> {
    fn drop(&mut self) {
        let mut market = self.market.lock();
        log::trace!(
            "{}: Dropped, closing the market.",
            std::thread::current().name().unwrap_or_default()
        );
        market.open = false;
        market.job_batches.clear();
        market.open_count = market.open_count.saturating_sub(1);
        self.has_new_jobs.notify_all();
    }
}

struct JobMarket<Job> {
    /// Whether this market is still open.
    open: bool,
    /// Number of workers.
    thread_count: usize,
    /// Number of markets working on jobs.
    open_count: usize,
    /// Jobs available.
    job_batches: Vec<VecDeque<Job>>,
}

impl<Job> JobBroker<Job>
where
    Job: Send + 'static,
{
    /// Create a new market for a group of threads.
    pub fn new(thread_count: usize, close_at: Option<SystemTime>) -> Self {
        let s = Self {
            has_new_jobs: Arc::new(Condvar::new()),
            market: Arc::new(Mutex::new(JobMarket {
                open: true,
                thread_count,
                open_count: thread_count,
                job_batches: Vec::new(),
            })),
        };
        if let Some(closing_time) = close_at {
            let s1 = s.clone();
            std::thread::Builder::new()
                .name("timeout".to_owned())
                .spawn(move || {
                    if let Ok(time_to_sleep) = closing_time.duration_since(SystemTime::now()) {
                        sleep(time_to_sleep);
                    }
                    let mut market = s1.market.lock();
                    log::debug!("Reached timeout, triggering shutdown");
                    market.open = false;
                })
                .unwrap();
        }
        s
    }
}

impl<Job> JobBroker<Job> {
    /// Pop a group of jobs from the market.
    ///
    /// Returns an empty result if there are no more jobs coming.
    pub fn pop(&mut self) -> VecDeque<Job> {
        let mut market = self.market.lock();
        if !market.open {
            return VecDeque::new();
        }
        loop {
            if let Some(jobs) = market.job_batches.pop() {
                log::trace!(
                    "{}: Got jobs. Working.",
                    std::thread::current().name().unwrap_or_default()
                );
                return jobs;
            } else {
                // Otherwise more work may become available.
                market.open_count = market.open_count.saturating_sub(1);
                if market.open_count == 0 {
                    // we are the last running thread, notify all others and return so we can
                    // shut down properly
                    log::trace!(
                        "{}: No jobs. Last running thread.",
                        std::thread::current().name().unwrap_or_default()
                    );
                    self.has_new_jobs.notify_all();
                    market.open = false;
                    return VecDeque::new();
                }
                log::trace!(
                    "{}: No jobs. Awaiting. running={}",
                    std::thread::current().name().unwrap_or_default(),
                    market.open_count
                );
                self.has_new_jobs.wait(&mut market);
                market.open_count += 1;
            }
        }
    }

    /// Push a new set of job batches into the market.
    pub fn push(&mut self, jobs: VecDeque<Job>) {
        let mut market = self.market.lock();
        if !market.open {
            return;
        }
        market.job_batches.push(jobs);
        log::trace!(
            "{}: Pushing jobs. running={}",
            std::thread::current().name().unwrap_or_default(),
            market.open_count
        );
        self.has_new_jobs.notify_one();
    }

    /// Split the jobs to be done into groups, one for each currently waiting thread and send them
    /// on.
    pub fn split_and_push(&mut self, jobs: &mut VecDeque<Job>) {
        let mut market = self.market.lock();
        if !market.open {
            // remove any jobs to be done
            jobs.clear();
            return;
        }
        let pieces = 1 + std::cmp::min(
            market.thread_count.saturating_sub(market.open_count),
            jobs.len(),
        );
        let size = jobs.len() / pieces;
        log::trace!(
            "{}: Sharing work. pieces={} size={} running={}",
            std::thread::current().name().unwrap_or_default(),
            pieces,
            size,
            market.open_count
        );
        for _ in 1..pieces {
            let to_share = jobs.split_off(jobs.len() - size);
            if to_share.is_empty() {
                continue;
            }
            market.job_batches.push(to_share);
            self.has_new_jobs.notify_one();
        }
    }

    /// See whether the market is closed.
    pub fn is_closed(&self) -> bool {
        let market = self.market.lock();
        !market.open && market.job_batches.is_empty() && market.open_count == 0
    }
}
