//! Source and policy qualification precede publication and URL installation.
use super::{Command, Error, JobOperation, Phase, Task, TransportError};
use evm_abstract_protocol::{
    BlockSelector, DecompileArtifact, DecompileJobReply, DecompileOpenReply, DecompileOpenRequest,
    DecompileOpened, DecompileSettings, DecompileSourceInput, JobState, decompile_sha256,
};

pub(super) struct Expected {
    bytes: Vec<u8>,
    settings: DecompileSettings,
}

impl Task {
    pub fn stale_open_cleanup(
        &mut self,
        generation: u64,
        reply: &Result<DecompileOpenReply, TransportError>,
    ) -> Option<Command> {
        if generation == self.generation {
            return None;
        }
        let requested = self.cancelled_openings.remove(&generation)?;
        let opened = reply.as_ref().ok()?.result.as_ref().ok()?;
        // A retired generation authorizes cleanup of its own source only.
        // Receipt qualification is required even when no artifact is installed.
        validate(&requested, opened).ok()?;
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.id == opened.snapshot.id)
        {
            return None;
        }
        (!super::terminal(&opened.snapshot.state)).then_some(Command::Cancel {
            generation,
            id: opened.snapshot.id,
        })
    }

    pub fn resume_publication(
        &mut self,
        request: &evm_abstract_protocol::DecompileRequest,
        mut snapshot: evm_abstract_protocol::DecompileJobSnapshot,
        publication: evm_abstract_protocol::PublicationVersion,
    ) {
        let settings = DecompileSettings::from_request(request);
        self.begin(Some(&settings));
        self.expected = Some(Expected {
            bytes: runtime(&request.bytecode).expect("accepted source request"),
            settings,
        });
        snapshot.publication = Some(publication);
        self.receive_status(
            self.generation,
            JobOperation::Submit,
            Ok(DecompileJobReply {
                result: Ok(snapshot),
            }),
        );
        self.delivered(publication);
    }
    pub fn open(&mut self, request: DecompileOpenRequest) -> Command {
        self.begin(request.settings.as_ref());
        self.pending_open = Some(request.clone());
        Command::Open {
            generation: self.generation,
            request: Box::new(request),
        }
    }

    pub fn retire(&mut self) {
        self.begin(None);
        self.phase = Phase::Idle;
        self.status_pending = false;
    }

    pub fn invalid_link(&mut self, error: String) {
        self.retire();
        self.phase = Phase::Terminal;
        self.error = Some(Error::Route(error));
    }

    pub fn receive_open(
        &mut self,
        generation: u64,
        reply: Result<DecompileOpenReply, TransportError>,
    ) -> Option<DecompileOpened> {
        if generation != self.generation || self.phase != Phase::Submitting {
            return None;
        }
        let requested = self.pending_open.take()?;
        self.status_pending = false;
        let result = reply
            .map_err(Error::Transport)
            .and_then(|reply| reply.result.map_err(Error::Api))
            .and_then(|opened| validate(&requested, &opened).map(|expected| (opened, expected)));
        let (opened, expected) = match result {
            Ok(value) => value,
            Err(error) => {
                self.phase = Phase::Terminal;
                self.error = Some(error);
                return None;
            }
        };
        self.fork = Some(expected.settings.fork);
        self.execution_domain = expected.settings.config.execution_domain;
        self.expected = Some(expected);
        self.receive_status(
            generation,
            JobOperation::Submit,
            Ok(DecompileJobReply {
                result: Ok(opened.snapshot.clone()),
            }),
        );
        Some(opened)
    }

    pub fn check_opened_artifact(&self, artifact: &DecompileArtifact) -> Result<(), Error> {
        let Some(expected) = &self.expected else {
            return Ok(());
        };
        if artifact.input.runtime_bytecode != expected.bytes {
            return Err(Error::InputMismatch);
        }
        if artifact.config != expected.settings.config {
            return Err(Error::ConfigMismatch);
        }
        Ok(())
    }
}

fn validate(requested: &DecompileOpenRequest, opened: &DecompileOpened) -> Result<Expected, Error> {
    let bytes = runtime(&opened.request.bytecode)?;
    let settings = DecompileSettings::from_request(&opened.request);
    if decompile_sha256(&bytes) != opened.source.sha256
        || settings.sha256() != opened.source.config_sha256
    {
        return Err(Error::Source(
            "runtime or settings checksum does not match its descriptor".into(),
        ));
    }
    if let Some(requested) = &requested.settings {
        if requested != &settings {
            return Err(Error::ConfigMismatch);
        }
    } else if !matches!(&requested.source, DecompileSourceInput::Sha256(hash) if hash.config_sha256.is_some())
        && settings != DecompileSettings::default()
    {
        return Err(Error::ConfigMismatch);
    }
    if opened.source.cache_hit
        && (opened.snapshot.state != JobState::Completed || opened.snapshot.publication.is_none())
    {
        return Err(Error::Source(
            "cache hit has no completed publication".into(),
        ));
    }
    match &requested.source {
        DecompileSourceInput::Bytecode(code) => {
            if runtime(code)? != bytes {
                return Err(Error::InputMismatch);
            }
            if opened.source.origin.is_some() {
                return Err(Error::Source(
                    "bytecode input acquired an unrelated address origin".into(),
                ));
            }
        }
        DecompileSourceInput::Sha256(hash) => {
            if crate::routing::fixed_hex(&hash.sha256, 32).as_ref() != Some(&opened.source.sha256)
                || hash.config_sha256.as_ref().is_some_and(|config| {
                    crate::routing::fixed_hex(config, 32).as_ref()
                        != Some(&opened.source.config_sha256)
                })
            {
                return Err(Error::Source(
                    "the opened result belongs to a different link".into(),
                ));
            }
            if opened.source.origin.is_some() {
                return Err(Error::Source(
                    "content input acquired an unrelated address origin".into(),
                ));
            }
        }
        DecompileSourceInput::Address(address) => {
            let origin = opened
                .source
                .origin
                .as_ref()
                .ok_or_else(|| Error::Source("address input has no pinned origin".into()))?;
            if crate::routing::fixed_hex(&address.address, 20)
                != crate::routing::fixed_hex(&origin.address, 20)
                || crate::routing::fixed_hex(&origin.address, 20).is_none()
                || origin.provider_id.is_empty()
                || crate::routing::fixed_hex(&origin.block_hash, 32).is_none()
                || crate::routing::parse_chain(&origin.chain_id).is_err()
                || chain(&origin.chain_id).is_none()
                || address
                    .provider_id
                    .as_ref()
                    .is_some_and(|id| id != &origin.provider_id)
                || address
                    .chain_id
                    .as_ref()
                    .is_some_and(|id| chain(id) != chain(&origin.chain_id))
            {
                return Err(Error::Source(
                    "address, provider, chain or block origin differs from the request".into(),
                ));
            }
            let block_matches = match &address.block {
                BlockSelector::Latest => true,
                BlockSelector::Number(number) => *number == origin.block_number.0,
                BlockSelector::Hash(hash) => {
                    crate::routing::fixed_hex(hash, 32)
                        == crate::routing::fixed_hex(&origin.block_hash, 32)
                }
            };
            if !block_matches {
                return Err(Error::Source(
                    "the source came from a different block".into(),
                ));
            }
        }
    }
    Ok(Expected { bytes, settings })
}

fn chain(value: &str) -> Option<evm_abstract_protocol::decompile_model::U256> {
    let (digits, radix) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or((value, 10), |v| (v, 16));
    evm_abstract_protocol::decompile_model::U256::from_str_radix(digits, radix).ok()
}

fn runtime(text: &str) -> Result<Vec<u8>, Error> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(&text);
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Source("invalid hexadecimal runtime".into()));
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                .map_err(|_| Error::Source("invalid hexadecimal runtime".into()))
        })
        .collect()
}

#[cfg(test)]
mod tests;
