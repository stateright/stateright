use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Write;
use std::time::Duration;

use crate::{DiscoveryClassification, Model, Path};

/// The data sent during a report event.
pub struct ReportData {
    /// The total number of states.
    pub total_states: usize,
    /// The number of unique states found.
    pub unique_states: usize,
    /// Maximum depth explored.
    pub max_depth: usize,
    /// The out degrees of **unique** states in the model.
    pub out_degrees: Vec<usize>,
    /// The in degrees of **unique** states in the model.
    pub in_degrees: Vec<usize>,
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
        M::State: Debug;
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
        let total_out_degree: usize = data.out_degrees.iter().sum();
        let average_out_degree = total_out_degree as f64 / data.out_degrees.len() as f64;
        let total_in_degree: usize = data.in_degrees.iter().sum();
        let average_in_degree = total_in_degree as f64 / data.in_degrees.len() as f64;

        if data.done {
            let _ = writeln!(
                self.writer,
                "Done. states={}, unique={}, depth={}, avg out degree={}, avg in degree={}, sec={}",
                data.total_states,
                data.unique_states,
                data.max_depth,
                average_out_degree,
                average_in_degree,
                data.duration.as_secs(),
            );
        } else {
            let _ = writeln!(
                self.writer,
                "Checking. states={}, unique={}, depth={}, avg out degree={}, avg in degree={}",
                data.total_states,
                data.unique_states,
                data.max_depth,
                average_out_degree,
                average_in_degree
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
