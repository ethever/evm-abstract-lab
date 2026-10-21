//! Native input assembly and execution. Numeric selection is already complete
//! when these helpers are called; inputs retain the chosen carrier throughout.

pub(crate) mod args;
pub(crate) use args::ExecutionArgs;

use crate::{AnalysisArgs, WorldArgs, error::CliError, world};
use evm_abstract::{
    analysis::{
        self, AnalysisEngine, AnalysisInput, Config, EnginePolicies, EvmWorldAnalysis,
        ExecutionConfig, control::Control, cooperation::NoExtension,
    },
    domain::{EvmNumericAlgebra, ValueAlgebra},
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
    pub(crate) fn analyze<D: EvmNumericAlgebra>(
        self,
        algebra: &ValueAlgebra<D>,
        control: &Control,
    ) -> Result<EvmWorldAnalysis<D::Element>, CliError> {
        let config = self.execution.config(!self.no_summaries);
        let entry_address = self
            .evm
            .to
            .expect("clap requires --evm.to for world/RPC input");
        let entry = EvmEntry {
            address: entry_address,
            environment: self.evm.environment()?,
        };
        let policies: EnginePolicies = EnginePolicies::default();
        let mut extension = NoExtension;
        let mut engine = AnalysisEngine::new(algebra, &policies, &mut extension);
        match (self.world, self.rpc) {
            (Some(path), None) => {
                let input = AnalysisInput::new(world::load(&path)?, entry)?;
                Ok(engine.run(input, config, control)?)
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
                if self.no_rpc_discovery {
                    let world = rpc::load_with_control(&input, control)?;
                    Ok(engine.run(AnalysisInput::new(world, entry)?, config, control)?)
                } else {
                    let result = analysis::analyze_rpc(engine, &input, entry, config, control)?;
                    for failure in result.failures() {
                        eprintln!("RPC discovery: {failure}");
                    }
                    Ok(result.into_analysis())
                }
            }
            _ => unreachable!("clap requires exactly one world input"),
        }
    }
}
