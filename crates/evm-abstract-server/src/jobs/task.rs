//! Typed task payloads share scheduling without sharing analysis semantics.

use super::{PublishedDecompileResult, PublishedResult};
use evm_abstract_protocol::{
    AnalysisProgress, AnalysisReport, AnalysisTimings, AnalyzeRequest, DecimalU64,
    DecompileArtifact, DecompileJobSnapshot, DecompileRequest, JobId, JobSnapshot, JobState,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Analysis,
    Decompile,
}

pub(super) enum Request {
    Analysis(Box<AnalyzeRequest>),
    Decompile(Box<DecompileRequest>),
}

impl Request {
    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Analysis(_) => Kind::Analysis,
            Self::Decompile(_) => Kind::Decompile,
        }
    }
}

pub(super) enum Snapshot {
    Analysis(JobSnapshot),
    Decompile(DecompileJobSnapshot),
}

impl Snapshot {
    pub(super) fn new(id: JobId, kind: Kind) -> Self {
        match kind {
            Kind::Analysis => Self::Analysis(JobSnapshot {
                id,
                state: JobState::Queued,
                progress: AnalysisProgress::default(),
                timings: AnalysisTimings::default(),
                publication: None,
            }),
            Kind::Decompile => Self::Decompile(DecompileJobSnapshot {
                id,
                state: JobState::Queued,
                progress: Default::default(),
                queue_ns: DecimalU64(0),
                processing_ns: DecimalU64(0),
                publication: None,
            }),
        }
    }

    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Analysis(_) => Kind::Analysis,
            Self::Decompile(_) => Kind::Decompile,
        }
    }

    pub(super) fn state(&self) -> &JobState {
        match self {
            Self::Analysis(snapshot) => &snapshot.state,
            Self::Decompile(snapshot) => &snapshot.state,
        }
    }

    pub(super) fn state_mut(&mut self) -> &mut JobState {
        match self {
            Self::Analysis(snapshot) => &mut snapshot.state,
            Self::Decompile(snapshot) => &mut snapshot.state,
        }
    }

    pub(super) fn copy(&self) -> Self {
        match self {
            Self::Analysis(snapshot) => Self::Analysis(snapshot.clone()),
            Self::Decompile(snapshot) => Self::Decompile(snapshot.clone()),
        }
    }
}

pub(super) enum Outcome {
    Analysis(Box<AnalysisReport>),
    Decompile(Box<DecompileArtifact>),
}

impl Outcome {
    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Analysis(_) => Kind::Analysis,
            Self::Decompile(_) => Kind::Decompile,
        }
    }
}

pub(super) enum Retained {
    Analysis(Arc<AnalysisReport>),
    Decompile(Arc<DecompileArtifact>),
}

pub(super) enum Published {
    Analysis(PublishedResult),
    Decompile(PublishedDecompileResult),
}
