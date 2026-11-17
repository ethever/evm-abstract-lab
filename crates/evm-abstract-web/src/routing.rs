//! Browser locations describe independent pages and reproducible source inputs.
//! Parsing is pure so a malformed link cannot trigger the analyzer's startup.
use std::collections::BTreeMap;

use evm_abstract_protocol::{
    BlockSelector, DecompileAddressInput, DecompileHashSource, DecompileOpenRequest,
    DecompileSource, DecompileSourceInput,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Analysis,
    Decompiler,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Hash {
        sha256: String,
        config: Option<String>,
    },
    Address(DecompileAddressInput),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Route {
    pub page: Page,
    pub source: Option<Source>,
    pub error: Option<Error>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Error {
    #[error("Unknown workspace path")]
    Path,
    #[error("The link contains invalid URL encoding")]
    Encoding,
    #[error("The link repeats parameter {0}")]
    Duplicate(String),
    #[error("Unknown decompiler parameter: {0}")]
    Parameter(String),
    #[error("Choose one source selector: address or sha256")]
    Source,
    #[error("The address must contain exactly 20 hexadecimal bytes")]
    Address,
    #[error("The {0} must contain exactly 32 hexadecimal bytes")]
    Hash(&'static str),
    #[error("The block must be latest, a block number, or a 32-byte block hash")]
    Block,
    #[error("The chain ID must be an unsigned decimal number")]
    Chain,
    #[error("The parameter {0} requires an address source")]
    AddressParameter(&'static str),
    #[error("A config parameter requires a sha256 source")]
    Config,
    #[error("The provider ID must not be empty")]
    Provider,
}

impl Default for Route {
    fn default() -> Self {
        Self::page(Page::Analysis)
    }
}

impl Route {
    pub fn page(page: Page) -> Self {
        Self {
            page,
            source: None,
            error: None,
        }
    }

    pub fn parse(path: &str, search: &str) -> Self {
        if path == "/" {
            return Self::default();
        }
        let mut route = Self::page(Page::Decompiler);
        if path != "/decompiler" && path != "/decompiler/" {
            route.error = Some(Error::Path);
            return route;
        }
        match parameters(search).and_then(source) {
            Ok(source) => route.source = source,
            Err(error) => route.error = Some(error),
        }
        route
    }

    pub fn canonical(source: &DecompileSource) -> Self {
        Self {
            page: Page::Decompiler,
            source: Some(Source::Hash {
                sha256: source.sha256.clone(),
                config: Some(source.config_sha256.clone()),
            }),
            error: None,
        }
    }

    pub fn url(&self) -> Option<String> {
        if self.error.is_some() {
            return None;
        }
        if self.page == Page::Analysis {
            return Some("/".into());
        }
        let mut parameters = Vec::<(&str, String)>::new();
        match &self.source {
            None => {}
            Some(Source::Hash { sha256, config }) => {
                parameters.push(("sha256", sha256.clone()));
                if let Some(config) = config {
                    parameters.push(("config", config.clone()));
                }
            }
            Some(Source::Address(input)) => {
                parameters.push(("address", input.address.clone()));
                if let Some(provider) = &input.provider_id {
                    parameters.push(("provider", provider.clone()));
                }
                match &input.block {
                    BlockSelector::Latest => {}
                    BlockSelector::Number(number) => parameters.push(("block", number.to_string())),
                    BlockSelector::Hash(hash) => parameters.push(("block", hash.clone())),
                }
                if let Some(chain) = &input.chain_id {
                    parameters.push(("chain", chain.clone()));
                }
            }
        }
        let query = parameters
            .into_iter()
            .map(|(key, value)| format!("{key}={}", encode(&value)))
            .collect::<Vec<_>>()
            .join("&");
        Some(if query.is_empty() {
            "/decompiler".into()
        } else {
            format!("/decompiler?{query}")
        })
    }
}

impl Source {
    pub fn request(&self) -> DecompileOpenRequest {
        DecompileOpenRequest {
            source: match self {
                Self::Hash { sha256, config } => {
                    DecompileSourceInput::Sha256(DecompileHashSource {
                        sha256: sha256.clone(),
                        config_sha256: config.clone(),
                    })
                }
                Self::Address(input) => DecompileSourceInput::Address(input.clone()),
            },
            settings: None,
        }
    }
}

fn parameters(search: &str) -> Result<BTreeMap<String, String>, Error> {
    let mut parsed = BTreeMap::new();
    for entry in search
        .strip_prefix('?')
        .unwrap_or(search)
        .split('&')
        .filter(|entry| !entry.is_empty())
    {
        let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        if !["address", "sha256", "config", "provider", "block", "chain"].contains(&key.as_str()) {
            return Err(Error::Parameter(key));
        }
        if parsed.insert(key.clone(), value).is_some() {
            return Err(Error::Duplicate(key));
        }
    }
    Ok(parsed)
}

fn source(mut parameters: BTreeMap<String, String>) -> Result<Option<Source>, Error> {
    let address = parameters.remove("address");
    let hash = parameters.remove("sha256");
    let config = parameters.remove("config");
    if address.is_some() && hash.is_some() {
        return Err(Error::Source);
    }
    if let Some(hash) = hash {
        for key in ["provider", "block", "chain"] {
            if parameters.contains_key(key) {
                return Err(Error::AddressParameter(key));
            }
        }
        return Ok(Some(Source::Hash {
            sha256: fixed_hex(&hash, 32).ok_or(Error::Hash("SHA-256"))?,
            config: config
                .map(|config| fixed_hex(&config, 32).ok_or(Error::Hash("configuration hash")))
                .transpose()?,
        }));
    }
    if config.is_some() {
        return Err(Error::Config);
    }
    let Some(address) = address else {
        if !parameters.is_empty() {
            return Err(Error::Source);
        }
        return Ok(None);
    };
    let address = format!("0x{}", fixed_hex(&address, 20).ok_or(Error::Address)?);
    let provider_id = parameters.remove("provider");
    if provider_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(Error::Provider);
    }
    let block = parse_block(parameters.remove("block").as_deref().unwrap_or("latest"))?;
    let chain_id = parameters
        .remove("chain")
        .map(|value| parse_chain(&value))
        .transpose()?;
    Ok(Some(Source::Address(DecompileAddressInput {
        address,
        provider_id,
        block,
        chain_id,
    })))
}

pub(crate) fn fixed_hex(value: &str, bytes: usize) -> Option<String> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    (value.len() == bytes * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

pub(crate) fn parse_block(value: &str) -> Result<BlockSelector, Error> {
    if value == "latest" || value.is_empty() {
        return Ok(BlockSelector::Latest);
    }
    if let Some(hash) = fixed_hex(value, 32) {
        return Ok(BlockSelector::Hash(format!("0x{hash}")));
    }
    let number = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    number.map(BlockSelector::Number).map_err(|_| Error::Block)
}

pub(crate) fn parse_chain(value: &str) -> Result<String, Error> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Chain);
    }
    evm_abstract_protocol::decompile_model::U256::from_str_radix(value, 10)
        .map(|value| value.to_string())
        .map_err(|_| Error::Chain)
}

fn decode(value: &str) -> Result<String, Error> {
    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        out.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or(Error::Encoding)?;
                let low = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or(Error::Encoding)?;
                (high * 16 + low) as u8
            }
            byte => byte,
        });
    }
    String::from_utf8(out).map_err(|_| Error::Encoding)
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            write!(out, "%{byte:02X}").expect("writing into a string");
        }
    }
    out
}

#[cfg(test)]
mod tests;
