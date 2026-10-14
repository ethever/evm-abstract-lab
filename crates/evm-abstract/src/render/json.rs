//! Cancellable native JSON output without a whole-output byte limit.
//! An error preserves the already computed analysis and carries exact usage.
use crate::analysis::control::Control;
use embedded_smt::Cancellation;
use serde::Serialize;
use std::io::{self, Write};
use thiserror::Error;

/// Native result extraction failed without changing execution facts.
#[derive(Debug, Error)]
pub enum JsonError {
    /// Cancellation was observed before another serialization write.
    #[error("native JSON export cancelled after {written} bytes")]
    Cancelled {
        /// Bytes accepted before cancellation.
        written: usize,
    },
    /// A native value rejected serialization within its own typed policy.
    #[error("native JSON serialization failed: {0}")]
    Serialization(serde_json::Error),
    /// Computing the exact initial-state fingerprint failed before rendering.
    #[error("native JSON fingerprint failed: {0}")]
    Fingerprint(crate::world::FingerprintError),
}
impl From<crate::world::FingerprintError> for JsonError {
    fn from(error: crate::world::FingerprintError) -> Self {
        Self::Fingerprint(error)
    }
}
impl From<serde_json::Error> for JsonError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}
struct Writer {
    bytes: Vec<u8>,
    cancellation: Cancellation,
    failure: Option<JsonError>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failure.is_some() {
            return Err(io::ErrorKind::Other.into());
        }
        if self.cancellation.is_cancelled() {
            self.failure = Some(JsonError::Cancelled {
                written: self.bytes.len(),
            });
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode<T: Serialize>(
    value: &T,
    cancellation: Cancellation,
    pretty: bool,
) -> Result<String, JsonError> {
    let mut writer = Writer {
        bytes: Vec::new(),
        cancellation,
        failure: None,
    };
    let result = if pretty {
        serde_json::to_writer_pretty(&mut writer, value)
    } else {
        serde_json::to_writer(&mut writer, value)
    };
    if let Some(failure) = writer.failure {
        return Err(failure);
    }
    if writer.cancellation.is_cancelled() {
        return Err(JsonError::Cancelled {
            written: writer.bytes.len(),
        });
    }
    result?;
    Ok(String::from_utf8(writer.bytes).expect("JSON serializer emits valid UTF-8"))
}
/// Pretty native output, preserving the current thread's cancellation scope.
pub fn pretty<T: Serialize>(value: &T) -> Result<String, JsonError> {
    encode(value, embedded_smt::current_cancellation(), true)
}
/// Compact output, preserving the current thread's cancellation scope.
pub fn compact<T: Serialize>(value: &T) -> Result<String, JsonError> {
    encode(value, embedded_smt::current_cancellation(), false)
}
/// Pretty extraction under an explicit cancellation scope.
pub fn with_control<T: Serialize>(value: &T, control: &Control) -> Result<String, JsonError> {
    control.scope(|| encode(value, control.cancellation().clone(), true))
}

/// Compact extraction under an explicit cancellation scope.
pub fn compact_with_control<T: Serialize>(
    value: &T,
    control: &Control,
) -> Result<String, JsonError> {
    control.scope(|| encode(value, control.cancellation().clone(), false))
}
#[cfg(test)]
mod tests;
