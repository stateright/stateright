use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Write;
use std::time::Duration;
use std::hash::Hash;

use crate::{DiscoveryClassification, Model, Path};

/// The data sent during a report event.
pub struct ReportData {
    /// The total number of states.
    pub total_states: usize,
    /// The number of unique states found.
    pub unique_states: usize,
    /// The current duration checking has been running for.
    pub duration: Duration,
    /// Whether checking is done.
    pub done: bool,
}

/// A discovery found during the checking.
pub struct ReportDiscovery<M>
where
    M: Model,
{
    /// The path that led to the discovery.
    pub path: Path<M::State, M::Action>,
    /// The classification of the path.
    pub classification: DiscoveryClassification,
}

/// A reporter for progress during the model checking.
pub trait Reporter<M: Model> {
    /// Report a progress event.
    fn report_checking(&mut self, data: ReportData);

    /// Report the discoveries at the end of the checking run.
    fn report_discoveries(&mut self, discoveries: HashMap<&'static str, ReportDiscovery<M>>)
    where
        M::Action: Debug,
        M::State: Debug + Hash;
}

pub struct WriteReporter<'a, W> {
    writer: &'a mut W,
}

impl<'a, W> WriteReporter<'a, W> {
    pub fn new(writer: &'a mut W) -> Self {
        Self { writer }
    }
}

impl<'a, M, W> Reporter<M> for WriteReporter<'a, W>
where
    M: Model,
    W: Write,
{
    fn report_checking(&mut self, data: ReportData) {
        if data.done {
            let _ = writeln!(
                self.writer,
                "Done. states={}, unique={}, sec={}",
                data.total_states,
                data.unique_states,
                data.duration.as_secs()
            );
        } else {
            let _ = writeln!(
                self.writer,
                "Checking. states={}, unique={}",
                data.total_states, data.unique_states
            );
        }
    }

    fn report_discoveries(&mut self, discoveries: HashMap<&'static str, ReportDiscovery<M>>)
    where
        M::Action: Debug,
        M::State: Debug,
    {
        for (name, discovery) in discoveries {
            let _ = write!(
                self.writer,
                "Discovered \"{}\" {} {}",
                name, discovery.classification, discovery.path,
            );
        }
    }
}
