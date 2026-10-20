//! Typed failures preserve interruption, scope and semantic-policy distinctions.

use std::fmt;

/// A process-unique epoch could not be allocated without reusing an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityExhausted;
impl fmt::Display for IdentityExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("analysis identity space exhausted")
    }
}
impl std::error::Error for IdentityExhausted {}

/// Scheduling/publication failed without proving a semantic contradiction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelError<S> {
    /// The shared root meter rejected work or observed cancellation.
    Stopped(S),
    /// A work bound overflowed; saturation is not a trustworthy reservation.
    CostOverflow,
    /// A handle belongs to another kernel epoch.
    WrongEpoch,
    /// A scope has been retired or is not owned by this kernel.
    InactiveScope,
    /// A descending rule attempted to refine another input generation.
    WrongScope,
    /// A checked counter cannot advance without reuse.
    VersionExhausted,
    /// An invalid or retired job handle was used.
    InvalidJob,
    /// An invalid fact or obligation handle was used.
    InvalidHandle,
    /// A job was already being evaluated.
    AlreadyRunning,
    /// Completion was attempted outside a live evaluation.
    NotRunning,
    /// A descending publication did not come from a registered reduction.
    UnregisteredReduction,
    /// Published information cannot be removed by declaring it absent.
    CannotErase,
    /// The live graph changed after an unpublished snapshot was made.
    StaleFork,
    /// A replacement generation has not completed its initial admission.
    UnsealedScope,
}
impl<S: fmt::Display> fmt::Display for KernelError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped(stop) => write!(f, "analysis stopped: {stop}"),
            Self::CostOverflow => f.write_str("analysis work bound overflowed"),
            Self::WrongEpoch => f.write_str("handle belongs to another analysis epoch"),
            Self::InactiveScope => f.write_str("input generation is inactive"),
            Self::WrongScope => f.write_str("descending reduction crossed input generations"),
            Self::VersionExhausted => f.write_str("analysis version space exhausted"),
            Self::InvalidJob => f.write_str("invalid or retired analysis job"),
            Self::InvalidHandle => f.write_str("invalid analysis handle"),
            Self::AlreadyRunning => f.write_str("analysis job is already in flight"),
            Self::NotRunning => f.write_str("analysis job is not in flight"),
            Self::UnregisteredReduction => f.write_str("descending rule is not registered"),
            Self::CannotErase => f.write_str("published information cannot be retracted"),
            Self::StaleFork => f.write_str("live analysis changed after the snapshot"),
            Self::UnsealedScope => f.write_str("replacement generation is not sealed"),
        }
    }
}
impl<S: std::error::Error + 'static> std::error::Error for KernelError<S> {}

/// A semantic policy error is distinct from a scheduler/meter interruption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishError<P, S> {
    /// Infrastructure failure; the publication was not installed.
    Kernel(KernelError<S>),
    /// Typed semantic-policy refusal, including a documented contradiction.
    Policy(P),
}
impl<P: fmt::Display, S: fmt::Display> fmt::Display for PublishError<P, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel(error) => error.fmt(f),
            Self::Policy(error) => error.fmt(f),
        }
    }
}
impl<P: std::error::Error + 'static, S: std::error::Error + 'static> std::error::Error
    for PublishError<P, S>
{
}
impl<P, S> From<KernelError<S>> for PublishError<P, S> {
    fn from(error: KernelError<S>) -> Self {
        Self::Kernel(error)
    }
}

/// A joint run stopped with its current job/obligations still retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolveError<A, S> {
    /// Admission, scope or meter failure.
    Kernel(KernelError<S>),
    /// An independent analyzer reported a typed error.
    Analyzer(A),
}
impl<A: fmt::Display, S: fmt::Display> fmt::Display for SolveError<A, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel(error) => error.fmt(f),
            Self::Analyzer(error) => error.fmt(f),
        }
    }
}
impl<A: std::error::Error + 'static, S: std::error::Error + 'static> std::error::Error
    for SolveError<A, S>
{
}
