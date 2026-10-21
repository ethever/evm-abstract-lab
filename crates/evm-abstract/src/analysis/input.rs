//! Validated immutable root input, independent of execution and numeric policy.

use super::{
    ConfigError,
    layers::{FactRevision, SnapshotId, StateFacts},
};
use crate::{
    bytecode::Program,
    domain::WordElement,
    world::{Code, EvmAccount, EvmEntry, EvmEnvironmentValues, EvmWorld, SnapshotIdentity},
};

/// An owned root request with a real snapshot and initial-fact revision.
/// Cloning retains the source and snapshot identities; constructing another
/// offline request creates a distinct snapshot even if its label is identical.
#[derive(Clone, Debug)]
pub struct AnalysisInput<N: WordElement> {
    pub(super) world: EvmWorld<N>,
    pub(super) entry: EvmEntry<N>,
    pub(super) facts: StateFacts,
}
impl<N: WordElement> AnalysisInput<N> {
    /// Validate the immutable environment before admitting this root request.
    pub fn new(world: EvmWorld<N>, mut entry: EvmEntry<N>) -> Result<Self, ConfigError> {
        world.apply_snapshot_environment(&mut entry.environment);
        entry.environment.validate_structure()?;
        if let Some(observed) = entry.environment.to.as_concrete()
            && observed != entry.address
        {
            return Err(crate::world::environment::EnvironmentError::Destination {
                expected: entry.address,
                observed,
            }
            .into());
        }
        let snapshot = match world.identity() {
            SnapshotIdentity::Offline { label } => SnapshotId::offline(label),
            SnapshotIdentity::Chain {
                chain_id,
                block_hash,
            } => SnapshotId::chain(*chain_id, *block_hash),
        };
        Ok(Self {
            world,
            entry,
            facts: StateFacts::new(snapshot),
        })
    }
    /// Prepare a single-program request for the same native execution engine.
    pub fn from_program(
        program: Program,
        environment: EvmEnvironmentValues<N>,
    ) -> Result<Self, ConfigError> {
        let address = environment
            .to
            .as_concrete()
            .unwrap_or(crate::Address::repeat_byte(0x11));
        let mut world = EvmWorld::new(program.fork(), "single-bytecode symbolic environment");
        let mut account = EvmAccount::unknown();
        account.code = Code::Runtime(program);
        world
            .insert(address, account)
            .expect("input world and program have the same fork");
        Self::new(
            world,
            EvmEntry {
                address,
                environment,
            },
        )
    }
    /// Original immutable world, including its symbolic source scope.
    pub fn world(&self) -> &EvmWorld<N> {
        &self.world
    }
    /// Validated root environment.
    pub fn entry(&self) -> &EvmEntry<N> {
        &self.entry
    }
    /// Snapshot whose concrete initial states this request denotes.
    pub fn snapshot(&self) -> &SnapshotId {
        self.facts.snapshot()
    }
    /// Initial-fact version consumed by a run.
    pub fn revision(&self) -> FactRevision {
        self.facts.revision()
    }
}
