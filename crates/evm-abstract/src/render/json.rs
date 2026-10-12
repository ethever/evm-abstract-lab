//! Bounded native JSON output. Limits apply before appending serialized bytes;
//! an error preserves the already computed analysis and carries exact usage.
use crate::analysis::control::Control;
use embedded_smt::Cancellation;
use serde::Serialize;
use std::io::{self, Write};
use thiserror::Error;

/// Native JSON output policy, independent of logical memory and solver limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonLimits {
    /// Maximum serialized UTF-8 bytes, including metadata and repeated snapshots.
    pub max_bytes: usize,
}
impl Default for JsonLimits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
        }
    }
}
/// Native result extraction failed without changing execution facts.
#[derive(Debug, Error)]
pub enum JsonError {
    /// Output would exceed the declared byte policy.
    #[error(
        "native JSON export requires at least {required} bytes, limit {limit} (retained {written})"
    )]
    Bytes {
        /// Required bytes at the failed write.
        required: usize,
        /// Frozen policy.
        limit: usize,
        /// Bytes accepted before failure.
        written: usize,
    },
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
    limits: JsonLimits,
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
        let required = self.bytes.len().saturating_add(bytes.len());
        if required > self.limits.max_bytes {
            self.failure = Some(JsonError::Bytes {
                required,
                limit: self.limits.max_bytes,
                written: self.bytes.len(),
            });
            return Err(io::ErrorKind::Other.into());
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
    limits: JsonLimits,
    cancellation: Cancellation,
    pretty: bool,
) -> Result<String, JsonError> {
    let mut writer = Writer {
        bytes: Vec::new(),
        limits,
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
    encode(
        value,
        JsonLimits::default(),
        embedded_smt::current_cancellation(),
        true,
    )
}
/// Compact bounded output for consumers that need a JSON value after encoding.
pub fn compact<T: Serialize>(value: &T) -> Result<String, JsonError> {
    encode(
        value,
        JsonLimits::default(),
        embedded_smt::current_cancellation(),
        false,
    )
}
/// Explicit bounded/cancellable extraction for application callers.
pub fn with_control<T: Serialize>(
    value: &T,
    limits: JsonLimits,
    control: &Control,
) -> Result<String, JsonError> {
    control.scope(|| encode(value, limits, control.cancellation().clone(), true))
}

/// Compact extraction under an explicit whole-output policy and cancellation.
pub fn compact_with_control<T: Serialize>(
    value: &T,
    limits: JsonLimits,
    control: &Control,
) -> Result<String, JsonError> {
    control.scope(|| encode(value, limits, control.cancellation().clone(), false))
}
#[cfg(test)]
mod tests;
