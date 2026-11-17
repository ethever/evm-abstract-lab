//! Standalone source resolution and content-qualified result restoration.
use super::{DecompileConfig, DecompileJobSnapshot, DecompileRequest};
use sha2::{Digest, Sha256};

record! {
    /// Complete settings of one decompilation, independent of its source spelling.
    pub struct DecompileSettings {
        /// Selected instruction rules.
        pub fork: crate::Fork,
        /// Every recovery, resource and presentation setting.
        pub config: DecompileConfig,
    }
}
impl Default for DecompileSettings {
    fn default() -> Self {
        Self::from_request(&DecompileRequest::default())
    }
}
impl DecompileSettings {
    /// Extract the complete policy without copying runtime text.
    pub fn from_request(request: &DecompileRequest) -> Self {
        Self {
            fork: request.fork,
            config: DecompileConfig {
                execution_domain: request.execution_domain,
                strategy: request.strategy,
                limits: request.limits.clone(),
                readability: request.readability.clone(),
            },
        }
    }
    /// Bind these settings to one normalized runtime input.
    pub fn into_request(self, bytecode: String) -> DecompileRequest {
        DecompileRequest {
            bytecode,
            fork: self.fork,
            execution_domain: self.config.execution_domain,
            strategy: self.config.strategy,
            limits: self.config.limits,
            readability: self.config.readability,
        }
    }
    /// Stable settings key; engine identity is separately checked by the server.
    pub fn sha256(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("fixed decompiler settings serialize");
        decompile_sha256(&encoded)
    }
}

/// SHA-256 of actual runtime bytes, not hexadecimal text or an Ethereum code hash.
pub fn decompile_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut text, "{byte:02x}").expect("String writes are infallible");
    }
    text
}

record! {
    /// A stored runtime and an optional exact settings selection.
    pub struct DecompileHashSource {
        /// Runtime byte SHA-256, as 64 hexadecimal digits.
        pub sha256: String,
        /// Absent selects current defaults; no arbitrary saved variant is chosen.
        pub config_sha256: Option<String> = None,
    }
}
record! {
    /// Address acquisition requests code only, never an execution world.
    pub struct DecompileAddressInput {
        /// Twenty-byte account address.
        pub address: String,
        /// A configured provider; absent selects the first configured provider.
        pub provider_id: Option<String> = None,
        /// Resolve once to an exact canonical block hash.
        pub block: crate::BlockSelector = crate::BlockSelector::Latest,
        /// Optional expected chain ID, in decimal or hexadecimal.
        pub chain_id: Option<String> = None,
    }
}
/// Explicit source of a standalone decompiler page.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecompileSourceInput {
    /// Locally supplied hexadecimal runtime.
    Bytecode(String),
    /// Previously stored content; a hash alone cannot fetch unknown bytes.
    Sha256(DecompileHashSource),
    /// Runtime from a configured RPC at one pinned block.
    Address(DecompileAddressInput),
}
record! {
    /// Resolve a source before creating or restoring a normal task publication.
    pub struct DecompileOpenRequest {
        /// Runtime content or its acquisition locator.
        pub source: DecompileSourceInput,
        /// Explicit overrides, or defaults/saved settings selected by the source.
        pub settings: Option<DecompileSettings> = None,
    }
}
record! {
    /// This particular address acquisition, not ownership of a shared artifact.
    pub struct DecompileAddressSource {
        /// Backend provider used for this acquisition.
        pub provider_id: String,
        /// Normalized account address.
        pub address: String,
        /// Verified chain identifier in decimal.
        pub chain_id: String,
        /// Exact canonical block used by getCode.
        pub block_hash: String,
        /// Pinned block height without browser integer loss.
        pub block_number: crate::DecimalU64,
    }
}
record! {
    /// Canonical shareable content and settings identifiers.
    pub struct DecompileSource {
        /// SHA-256 of parsed runtime bytes.
        pub sha256: String,
        /// SHA-256 of the complete effective settings.
        pub config_sha256: String,
        /// Present only for this address resolution, never inferred from another cache user.
        pub origin: Option<DecompileAddressSource>,
        /// The result was restored without executing recovery again.
        pub cache_hit: bool,
    }
}
record! {
    /// The result body still uses the existing task result/publication endpoint.
    pub struct DecompileOpened {
        /// Exact effective request, including settings restored from a link.
        pub request: DecompileRequest,
        /// Source and settings identities for canonical browser navigation.
        pub source: DecompileSource,
        /// A fresh task identity, even for a cached result.
        pub snapshot: DecompileJobSnapshot,
    }
}
record! {
    /// Typed standalone source-open response.
    pub struct DecompileOpenReply {
        /// Opened task or a precise source/cache/admission error.
        pub result: Result<DecompileOpened, crate::ApiError>,
    }
}
choice! {
    /// Optional cache failures never change the computed artifact's semantics.
    pub enum DecompileCacheWarningKind {
        /// A file could not be persisted.
        Io,
        /// The artifact exceeds the configured cache capacity.
        Capacity,
        /// Content or artifact identity failed cache admission.
        Integrity,
    }
}
record! {
    /// The task result is valid, but its cache publication was refused.
    pub struct DecompileCacheWarning {
        /// Machine-readable reason.
        pub kind: DecompileCacheWarningKind,
        /// Human-readable explanation without endpoint credentials.
        pub message: String,
    }
}
