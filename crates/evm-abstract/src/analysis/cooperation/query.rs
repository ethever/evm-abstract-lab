//! Public tracked point routing followed by actual immutable input adoption.
use super::{GenerationRoute, Generations, Input, InputRoute, Inputs, Point};
use crate::domain::WordElement;
use analysis_kernel::{Answer, Discover, GenerationEntry, Read, ReadGeneration, Scope, Scoped};

/// Require another point's actual current native input on the shared routes.
/// Unknown points and stale known producers remain mandatory Pending answers.
/// Retain the Point and resolve it again after a wake; a historical Subject is
/// not a live selector. Value never certifies complete predecessor discovery.
pub fn current_input<N, Cx>(
    cx: &mut Cx,
    universe: Scope,
    point: Point,
) -> Result<Answer<Input<N>>, <Cx as Read<Generations, GenerationRoute>>::Error>
where
    N: WordElement,
    Cx: Read<Generations, GenerationRoute>
        + ReadGeneration<Error = <Cx as Read<Generations, GenerationRoute>>::Error>
        + Read<Inputs<N>, InputRoute, Error = <Cx as Read<Generations, GenerationRoute>>::Error>,
{
    let entry =
        <Cx as Read<Generations, GenerationRoute>>::query(cx, Scoped::new(universe, point))?;
    selected_input(cx, entry)
}

/// Explore an as-yet unknown point without blocking its own CFG discovery.
/// Only the initial directory lookup is optional. Once a producer exists,
/// current-generation validity and its actual Input are mandatory tracked reads;
/// discovery cannot make a stale guarantee available or prove unreachability.
pub fn discover_input<N, Cx>(
    cx: &mut Cx,
    universe: Scope,
    point: Point,
) -> Result<Answer<Input<N>>, <Cx as Discover<Generations, GenerationRoute>>::Error>
where
    N: WordElement,
    Cx: Discover<Generations, GenerationRoute>
        + ReadGeneration<Error = <Cx as Discover<Generations, GenerationRoute>>::Error>
        + Read<Inputs<N>, InputRoute, Error = <Cx as Discover<Generations, GenerationRoute>>::Error>,
{
    let entry =
        <Cx as Discover<Generations, GenerationRoute>>::discover(cx, Scoped::new(universe, point))?;
    selected_input(cx, entry)
}

fn selected_input<N, Cx>(
    cx: &mut Cx,
    entry: Answer<GenerationEntry>,
) -> Result<Answer<Input<N>>, <Cx as ReadGeneration>::Error>
where
    N: WordElement,
    Cx: ReadGeneration + Read<Inputs<N>, InputRoute, Error = <Cx as ReadGeneration>::Error>,
{
    entry.try_then(|generation| {
        cx.current_generation(&generation)?.try_then(|scope| {
            <Cx as Read<Inputs<N>, InputRoute>>::query(cx, Scoped::new(scope, ()))
        })
    })
}

/// Failure of the native snapshot/certificate protocol. A mismatched ready
/// pair is inconsistent, not an unresolved kernel read or an empty bound.
#[derive(Debug, thiserror::Error)]
pub enum SummaryReadError<E> {
    /// An underlying required capability query failed.
    #[error("summary capability query failed: {0}")]
    Capability(E),
    /// The publisher supplied a ready certificate for another native snapshot.
    #[error("summary generation {snapshot} differs from certificate generation {certificate}")]
    GenerationMismatch {
        /// Native MAY snapshot generation.
        snapshot: usize,
        /// Native generation covered by the completion certificate.
        certificate: usize,
    },
}

// Explicit conversion keeps the cause concrete, using Error::source's default.
impl<E> From<E> for SummaryReadError<E> {
    fn from(error: E) -> Self {
        Self::Capability(error)
    }
}

/// A typed complete-bound query, distinct from an incremental MAY snapshot.
pub type ClosedSummaryResult<N, E> =
    Result<Answer<std::sync::Arc<super::Summary<N>>>, SummaryReadError<E>>;

/// Require a complete procedure output bound, including complete-empty. The
/// native snapshot alone is an incremental MAY observation. Both required reads
/// below retain their real kernel dependencies and producer/source validity.
/// A mismatched ready pair is a typed protocol error; it cannot manufacture a
/// Pending answer whose underlying kernel reads are already satisfied.
pub fn closed_summary<N, Cx>(
    cx: &mut Cx,
    universe: Scope,
    procedure: super::ProcedureId,
) -> ClosedSummaryResult<N, <Cx as Read<super::Summaries<N>, super::SummaryRoute>>::Error>
where
    N: WordElement,
    Cx: Read<super::Summaries<N>, super::SummaryRoute>
        + Read<
            super::SummaryClosures,
            super::SummaryClosureRoute,
            Error = <Cx as Read<super::Summaries<N>, super::SummaryRoute>>::Error,
        >,
{
    let snapshot = <Cx as Read<super::Summaries<N>, super::SummaryRoute>>::query(
        cx,
        Scoped::new(universe, procedure),
    )?;
    let proof = <Cx as Read<super::SummaryClosures, super::SummaryClosureRoute>>::query(
        cx,
        Scoped::new(universe, procedure),
    )?;
    proof.try_then(|proof| {
        if let Some(snapshot) = snapshot.value()
            && snapshot.generation != proof.generation
        {
            return Err(SummaryReadError::GenerationMismatch {
                snapshot: snapshot.generation,
                certificate: proof.generation,
            });
        }
        Ok(snapshot)
    })
}

/// Explore completion without blocking its discovery. When a certificate is
/// available, upgrade to required reads of both certificate and native snapshot;
/// Value has exactly the same complete-bound contract as [`closed_summary`]. A
/// Pending answer requires an independently sound fallback for optional feedback.
pub fn discover_closed_summary<N, Cx>(
    cx: &mut Cx,
    universe: Scope,
    procedure: super::ProcedureId,
) -> ClosedSummaryResult<N, <Cx as Read<super::Summaries<N>, super::SummaryRoute>>::Error>
where
    N: WordElement,
    Cx: Read<super::Summaries<N>, super::SummaryRoute>
        + Read<
            super::SummaryClosures,
            super::SummaryClosureRoute,
            Error = <Cx as Read<super::Summaries<N>, super::SummaryRoute>>::Error,
        > + Discover<
            super::SummaryClosures,
            super::SummaryClosureRoute,
            Error = <Cx as Read<super::Summaries<N>, super::SummaryRoute>>::Error,
        >,
{
    let proof = <Cx as Discover<super::SummaryClosures, super::SummaryClosureRoute>>::discover(
        cx,
        Scoped::new(universe, procedure),
    )?;
    proof.try_then(|_| closed_summary(cx, universe, procedure))
}
