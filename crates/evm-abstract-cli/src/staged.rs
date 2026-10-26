//! Bounded JSON Lines delivery; one application ledger funds both publications.
//! A complete line is flushed before Native preparation starts. A refused final
//! export leaves the earlier independent publication usable by stream consumers.

use crate::{error::CliError, hybrid::WorldAnalysis, numeric::NumericModel};
use evm_abstract::{
    analysis::{
        HybridAnalysis, RpcHybridCoverage, Status,
        control::Control,
        publication::{ModelHandle, StagePublisher},
    },
    domain::WordElement,
    resource::WorkBudget,
};
use serde::Serialize;
use std::io::{self, Write};

#[derive(Serialize)]
enum Lifecycle {
    Running,
    Final,
}

#[derive(Serialize)]
struct Publication<'a, N: WordElement> {
    version: u64,
    global_version: u64,
    native_version: Option<u64>,
    lifecycle: Lifecycle,
    model_closed: bool,
    numeric_model: NumericModel,
    snapshot: &'a evm_abstract::analysis::layers::SnapshotId,
    revision: evm_abstract::analysis::layers::FactRevision,
    report: &'a HybridAnalysis<N>,
    /// RPC facts/restarts qualify the original global graph; they never relabel it.
    rpc: Option<&'a RpcHybridCoverage>,
    precision: Option<&'a HybridAnalysis<N>>,
}

pub(crate) struct Stream<'a, W, N: WordElement> {
    writer: &'a mut W,
    budget: &'a mut WorkBudget,
    numeric_model: NumericModel,
    control: &'a Control,
    baseline: Option<ModelHandle<N>>,
    error: Option<CliError>,
}

impl<'a, W: Write, N: WordElement> Stream<'a, W, N> {
    pub(crate) fn new(
        writer: &'a mut W,
        budget: &'a mut WorkBudget,
        numeric_model: NumericModel,
        control: &'a Control,
    ) -> Self {
        Self {
            writer,
            budget,
            numeric_model,
            control,
            baseline: None,
            error: None,
        }
    }

    pub(crate) fn finish(mut self, result: &WorldAnalysis<N>) -> Result<(), CliError> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let (report, rpc, precision) = match result {
            WorldAnalysis::Hybrid(report) => (report.as_ref(), None, None),
            WorldAnalysis::Rpc(report) => (
                report.baseline(),
                Some(report.coverage()),
                report.detail_report(),
            ),
        };
        let version = if self.baseline.is_some() { 2 } else { 1 };
        let global_version = self
            .baseline
            .as_ref()
            .filter(|baseline| baseline.same_model(&report.baseline_handle()))
            .map_or(version, |_| 1);
        let native = precision.unwrap_or(report).detail().is_some();
        self.emit(&Publication {
            version,
            global_version,
            native_version: native.then_some(version),
            lifecycle: Lifecycle::Final,
            model_closed: report.status() == Status::Converged,
            numeric_model: self.numeric_model,
            snapshot: report.baseline().snapshot(),
            revision: report.baseline().revision(),
            report,
            rpc,
            precision,
        })
    }

    fn emit<T: Serialize>(&mut self, publication: &T) -> Result<(), CliError> {
        self.control.checkpoint().map_err(CliError::Cancelled)?;
        // Reserve each encoded byte before allocating it. Serialization writes
        // into one paid line, so refusal never sends a truncated JSON object.
        let mut line = PaidLine {
            bytes: Vec::new(),
            budget: self.budget,
            control: self.control,
        };
        serde_json::to_writer(&mut line, publication)?;
        line.write_all(b"\n")?;
        self.writer.write_all(&line.bytes)?;
        self.writer.flush()?;
        Ok(())
    }
}

impl<W: Write, N: WordElement> StagePublisher<N> for Stream<'_, W, N> {
    fn publish(&mut self, report: &HybridAnalysis<N>) {
        if self.baseline.is_some() || self.error.is_some() {
            return;
        }
        match self.emit(&Publication {
            version: 1,
            global_version: 1,
            native_version: None,
            lifecycle: Lifecycle::Running,
            model_closed: true,
            numeric_model: self.numeric_model,
            snapshot: report.baseline().snapshot(),
            revision: report.baseline().revision(),
            report,
            rpc: None,
            precision: None,
        }) {
            Ok(()) => self.baseline = Some(report.baseline_handle()),
            Err(error) => self.error = Some(error),
        }
    }
}

struct PaidLine<'a> {
    bytes: Vec<u8>,
    budget: &'a mut WorkBudget,
    control: &'a Control,
}

impl Write for PaidLine<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.control.checkpoint().map_err(io::Error::other)?;
        if self.budget.exhausted() || !self.budget.charge(bytes.len().saturating_mul(2)) {
            return Err(io::Error::other(
                "staged JSON postprocessing work exhausted",
            ));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
