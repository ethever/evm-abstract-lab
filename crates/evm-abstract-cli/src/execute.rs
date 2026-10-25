//! Native input assembly and execution. Numeric selection is already complete
//! when these helpers are called; inputs retain the chosen carrier throughout.

pub(crate) mod args;
pub(crate) use args::ExecutionArgs;

use crate::{AnalysisArgs, WorldArgs, error::CliError, hybrid::WorldAnalysis, world};
use evm_abstract::{
    analysis::{
        self, AnalysisEngine, AnalysisInput, Config, EnginePolicies, EvmWorldAnalysis,
        ExecutionConfig,
        control::Control,
        cooperation::NoExtension,
        publication::{NoopPublisher, StagePublisher},
    },
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    world::{
        EvmEntry,
        rpc::{self, AccountRequest, RpcBlock, RpcInput},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

impl ExecutionArgs {
    pub(crate) fn config(&self, use_summaries: bool) -> ExecutionConfig {
        ExecutionConfig {
            analysis: Config {
                context_depth: self.context_depth,
                max_states: self.max_states,
                max_transfers: self.max_transfers,
            },
            max_work: self.max_work,
            max_call_depth: self.max_call_depth,
            max_memory_bytes: self.max_memory_bytes,
            use_summaries,
            root_terminal_coverage: self.coverage.enabled(),
        }
    }
}

impl AnalysisArgs {
    pub(crate) fn analyze<D: EvmNumericAlgebra>(
        self,
        algebra: &ValueAlgebra<D>,
        control: &Control,
    ) -> Result<EvmWorldAnalysis<D::Element>, CliError> {
        let config = self.execution.config(true);
        let input = AnalysisInput::from_program(self.input.load()?, self.evm.environment()?)?;
        let policies: EnginePolicies = EnginePolicies::default();
        let mut extension = NoExtension;
        Ok(AnalysisEngine::new(algebra, &policies, &mut extension).run(input, config, control)?)
    }
}

impl WorldArgs {
    pub(crate) fn analyze_hybrid<D: EvmNumericAlgebra>(
        self,
        algebra: &ValueAlgebra<D>,
        control: &Control,
    ) -> Result<WorldAnalysis<D::Element>, CliError> {
        self.analyze_hybrid_with_publisher(algebra, control, &mut NoopPublisher)
    }

    pub(crate) fn analyze_hybrid_with_publisher<
        D: EvmNumericAlgebra,
        T: StagePublisher<D::Element>,
    >(
        self,
        algebra: &ValueAlgebra<D>,
        control: &Control,
        publisher: &mut T,
    ) -> Result<WorldAnalysis<D::Element>, CliError> {
        let (source, config) = self.prepare()?;
        let policies: EnginePolicies = EnginePolicies::default();
        let mut extension = NoExtension;
        let mut engine = AnalysisEngine::new(algebra, &policies, &mut extension);
        match source {
            WorldSource::Offline(input) => {
                Ok(WorldAnalysis::from(engine.run_hybrid_with_publisher(
                    *input,
                    config,
                    analysis::HybridConfig::default(),
                    control,
                    publisher,
                )?))
            }
            WorldSource::Rpc {
                input,
                entry,
                discovery,
            } => {
                if !discovery {
                    let world = rpc::load_with_control(&input, control)?;
                    Ok(WorldAnalysis::from(engine.run_hybrid_with_publisher(
                        AnalysisInput::new(world, *entry)?,
                        config,
                        analysis::HybridConfig::default(),
                        control,
                        publisher,
                    )?))
                } else {
                    let result = analysis::analyze_rpc_hybrid_with_publisher(
                        engine,
                        &input,
                        *entry,
                        config,
                        analysis::HybridConfig::default(),
                        control,
                        publisher,
                    )?;
                    for failure in result.failures() {
                        eprintln!("RPC discovery: {failure}");
                    }
                    Ok(WorldAnalysis::from(result))
                }
            }
        }
    }

    /// Legacy native controls share admission with the default Hybrid command.
    #[cfg(test)]
    pub(crate) fn analyze<D: EvmNumericAlgebra>(
        self,
        algebra: &ValueAlgebra<D>,
        control: &Control,
    ) -> Result<EvmWorldAnalysis<D::Element>, CliError> {
        let (source, config) = self.prepare()?;
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let mut engine = AnalysisEngine::new(algebra, &policies, &mut extension);
        match source {
            WorldSource::Offline(input) => Ok(engine.run(*input, config, control)?),
            WorldSource::Rpc {
                input,
                entry,
                discovery,
            } => {
                if discovery {
                    Ok(
                        analysis::analyze_rpc(engine, &input, *entry, config, control)?
                            .into_analysis(),
                    )
                } else {
                    let world = rpc::load_with_control(&input, control)?;
                    Ok(engine.run(AnalysisInput::new(world, *entry)?, config, control)?)
                }
            }
        }
    }

    fn prepare<N: WordElement>(self) -> Result<(WorldSource<N>, ExecutionConfig), CliError> {
        let config = self.execution.config(!self.no_summaries);
        let entry_address = self
            .evm
            .to
            .expect("clap requires --evm.to for world/RPC input");
        let entry = EvmEntry {
            address: entry_address,
            environment: self.evm.environment()?,
        };
        match (self.world, self.rpc) {
            (Some(path), None) => {
                let input = AnalysisInput::new(world::load(&path)?, entry)?;
                Ok((WorldSource::Offline(Box::new(input)), config))
            }
            (None, Some(endpoint)) => {
                let mut input = RpcInput::new(endpoint, self.fork.unwrap_or_default());
                input.block = match (self.block_hash, self.block_number) {
                    (Some(hash), None) => RpcBlock::Hash(world::hash(&hash, "block")?),
                    (None, Some(number)) => RpcBlock::Number(number),
                    (None, None) => RpcBlock::Latest,
                    (Some(_), Some(_)) => unreachable!("clap makes block selectors exclusive"),
                };
                let mut accounts = BTreeMap::<_, BTreeSet<_>>::new();
                accounts.insert(entry_address, BTreeSet::new());
                for account in self.account {
                    accounts
                        .entry(world::address(&account, "RPC account")?)
                        .or_default();
                }
                for slot in self.slot {
                    accounts.entry(slot.address).or_default().insert(slot.slot);
                }
                input.accounts = accounts
                    .into_iter()
                    .map(|(address, slots)| AccountRequest { address, slots })
                    .collect();
                input.max_accounts = self.max_rpc_accounts;
                input.max_requests = self.max_rpc_requests;
                input.max_response_bytes = self.max_rpc_response_bytes;
                input.timeout = Duration::from_millis(self.rpc_timeout_ms);
                Ok((
                    WorldSource::Rpc {
                        input: Box::new(input),
                        entry: Box::new(entry),
                        discovery: !self.no_rpc_discovery,
                    },
                    config,
                ))
            }
            _ => unreachable!("clap requires exactly one world input"),
        }
    }
}

enum WorldSource<N: WordElement> {
    Offline(Box<AnalysisInput<N>>),
    Rpc {
        input: Box<RpcInput>,
        entry: Box<EvmEntry<N>>,
        discovery: bool,
    },
}
