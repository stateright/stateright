use parking_lot::{Condvar, Mutex};
use std::{collections::VecDeque, sync::Arc};

pub struct JobMarket<Job> {
    has_new_job: Arc<Condvar>,
    market: Arc<Mutex<JobMarketInner<Job>>>,
}

impl<Job> Clone for JobMarket<Job> {
    fn clone(&self) -> Self {
        Self {
            has_new_job: Arc::clone(&self.has_new_job),
            market: Arc::clone(&self.market),
        }
    }
}

impl<Job> Drop for JobMarket<Job> {
    fn drop(&mut self) {
        let mut market = self.market.lock();
        market.open = false;
        market.open_count = market.open_count.saturating_sub(1);
        self.has_new_job.notify_all();
    }
}

struct JobMarketInner<Job> {
    open: bool,
    /// Number of markets working on jobs.
    open_count: usize,
    /// Jobs available.
    jobs: Vec<VecDeque<Job>>,
}

impl<Job> JobMarket<Job> {
    pub fn new(thread_count: usize) -> Self {
        Self {
            has_new_job: Arc::new(Condvar::new()),
            market: Arc::new(Mutex::new(JobMarketInner {
                open: true,
                open_count: thread_count,
                jobs: Vec::new(),
            })),
        }
    }

    pub fn pop(&mut self) -> VecDeque<Job> {
        let mut market = self.market.lock();
        if !market.open {
            return VecDeque::new();
        }
        loop {
            if let Some(job) = market.jobs.pop() {
                log::trace!("Got jobs. Working.");
                return job;
            } else {
                // Otherwise more work may become available.
                market.open_count = market.open_count.saturating_sub(1);
                if market.open_count == 0 {
                    // we are the last running thread, notify all others and return so we can
                    // shutdown properly
                    log::trace!("No jobs. Last running thread.");
                    self.has_new_job.notify_all();
                    market.open = false;
                    return VecDeque::new();
                }
                log::trace!("No jobs. Awaiting. running={}", market.open_count);
                self.has_new_job.wait(&mut market);
                market.open_count += 1;
            }
        }
    }

    pub fn push(&mut self, jobs: VecDeque<Job>) {
        let mut market = self.market.lock();
        if !market.open {
            return;
        }
        market.jobs.push(jobs);
        log::trace!("Pushing jobs. running={}", market.open_count);
        self.has_new_job.notify_one();
    }

    pub fn split_and_push(&mut self, jobs: &mut VecDeque<Job>) {
        let mut market = self.market.lock();
        if !market.open {
            // remove any jobs to be done
            jobs.clear();
            return;
        }
        let pieces = 1 + std::cmp::min(market.open_count, jobs.len());
        let size = jobs.len() / pieces;
        log::trace!(
            "Sharing work. pieces={} size={} running={}",
            pieces,
            size,
            market.open_count
        );
        for _ in 1..pieces {
            market.jobs.push(jobs.split_off(jobs.len() - size));
            self.has_new_job.notify_one();
        }
    }

    pub fn is_closed(&self) -> bool {
        let market = self.market.lock();
        market.jobs.is_empty() && market.open_count == 0
    }
}
