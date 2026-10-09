//! Server-owned RPC endpoints and their credential-free public catalogue.

use evm_abstract_protocol as api;
use std::{collections::BTreeSet, fs, io, path::Path};
use thiserror::Error;

/// Startup configuration failure. Neither display nor debug contains config values.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("could not read RPC configuration ({0:?})")]
    Read(io::ErrorKind),
    /// Invalid JSON shape, including duplicate, missing or unknown fields.
    #[error("invalid RPC configuration JSON at line {line}, column {column}")]
    Json {
        /// One-based JSON line.
        line: usize,
        /// JSON column.
        column: usize,
    },
    /// Invalid provider entry; its index identifies it without disclosing secrets.
    #[error("invalid RPC provider at index {index}: {reason}")]
    Provider {
        /// Zero-based index within the providers array.
        index: usize,
        /// A fixed explanation that never includes supplied values.
        reason: &'static str,
    },
}

// Deliberately omit Debug/Serialize: private endpoints never belong in diagnostics
// or replies. Only `catalogue` projects the public ID and display name.
struct Provider {
    id: String,
    name: String,
    endpoint: String,
}
struct Configuration {
    providers: Vec<Provider>,
}

/// Validated backend configuration shared immutably by HTTP admission and workers.
#[derive(Default)]
pub struct Registry {
    providers: Vec<Provider>,
}

impl Registry {
    /// Read the configuration before starting the HTTP listener or worker pool.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let bytes = fs::read(path).map_err(|error| ConfigError::Read(error.kind()))?;
        Self::from_json(&bytes)
    }

    /// Decode strict JSON and validate all endpoints without making network calls.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ConfigError> {
        let config: Configuration =
            serde_json::from_slice(bytes).map_err(|error| ConfigError::Json {
                line: error.line(),
                column: error.column(),
            })?;
        let mut ids = BTreeSet::new();
        for (index, provider) in config.providers.iter().enumerate() {
            let invalid = |reason| ConfigError::Provider { index, reason };
            if provider.id.is_empty()
                || provider.id.len() > 64
                || !provider
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            {
                return Err(invalid(
                    "id must contain 1–64 ASCII letters, digits, '-' or '_'",
                ));
            }
            if !ids.insert(&provider.id) {
                return Err(invalid("duplicate id"));
            }
            if provider.name.trim().is_empty()
                || provider.name.chars().count() > 128
                || provider.name.chars().any(char::is_control)
            {
                return Err(invalid(
                    "name must contain 1–128 characters without controls",
                ));
            }
            let endpoint = reqwest::Url::parse(&provider.endpoint)
                .map_err(|_| invalid("endpoint must be an absolute HTTP(S) URL"))?;
            if !matches!(endpoint.scheme(), "http" | "https")
                || endpoint.host_str().is_none()
                || endpoint.fragment().is_some()
                || !provider
                    .endpoint
                    .split_once("://")
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(endpoint.scheme()))
                || provider
                    .endpoint
                    .chars()
                    .any(|ch| ch.is_whitespace() || ch.is_control())
            {
                return Err(invalid(
                    "endpoint must be an absolute HTTP(S) URL without whitespace or a fragment",
                ));
            }
        }
        Ok(Self {
            providers: config.providers,
        })
    }

    /// Return only public metadata in configuration order, never endpoints.
    pub fn catalogue(&self) -> Vec<api::RpcProvider> {
        self.providers
            .iter()
            .map(|provider| api::RpcProvider {
                id: provider.id.clone(),
                name: provider.name.clone(),
            })
            .collect()
    }

    /// Reject unknown RPC IDs before scheduling work; bytecode needs no provider.
    pub fn validate_request(&self, request: &api::AnalyzeRequest) -> Result<(), api::ApiError> {
        if let api::AnalysisInput::Rpc(input) = &request.input {
            self.endpoint(&input.provider_id)?;
        }
        Ok(())
    }

    pub(crate) fn endpoint(&self, id: &str) -> Result<&str, api::ApiError> {
        self.providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|provider| provider.endpoint.as_str())
            .ok_or_else(|| api::ApiError {
                code: api::ApiErrorCode::InvalidRequest,
                message: "RPC provider is unavailable; select a provider configured by the backend"
                    .into(),
                details: api::ErrorDetails::Validation(api::ValidationFailure {
                    kind: api::ValidationErrorKind::Field,
                    field: Some("provider_id".into()),
                    value: None,
                }),
            })
    }
}

// Concrete visitors preserve no_dyn and reject ambiguous or misspelled config.
macro_rules! config_record {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Visitor;
                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(stringify!($name))
                    }
                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                        $(let mut $field: Option<$ty> = None;)*
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::custom("duplicate config field"));
                                    }
                                    $field = Some(map.next_value()?);
                                },)*
                                _ => return Err(serde::de::Error::custom("unknown config field")),
                            }
                        }
                        Ok($name { $($field: $field.ok_or_else(|| serde::de::Error::custom("missing config field"))?),* })
                    }
                }
                deserializer.deserialize_map(Visitor)
            }
        }
    };
}
config_record!(Configuration { providers: Vec<Provider> });
config_record!(Provider {
    id: String,
    name: String,
    endpoint: String
});
