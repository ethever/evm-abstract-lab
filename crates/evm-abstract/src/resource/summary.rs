//! Keep a shared-ledger refusal separate from a summary's typed source error.

use super::CloneError;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SummaryError<E> {
    Budget(CloneError),
    Source(E),
}

impl<E> From<CloneError> for SummaryError<E> {
    fn from(error: CloneError) -> Self {
        Self::Budget(error)
    }
}

impl<E> SummaryError<E> {
    pub(crate) fn map_source<F>(self, map: impl FnOnce(E) -> F) -> SummaryError<F> {
        match self {
            Self::Budget(error) => SummaryError::Budget(error),
            Self::Source(error) => SummaryError::Source(map(error)),
        }
    }
}

impl<E: fmt::Display> fmt::Display for SummaryError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Budget(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Display + fmt::Debug> std::error::Error for SummaryError<E> {}
