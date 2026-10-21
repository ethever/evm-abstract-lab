//! Offline public-API client. Run from the repository root:
//! nix develop --no-update-lock-file --command cargo run --locked -p evm-abstract --example cooperative
//!
//! The producer proves `(x | nonzero_constant) == 0` is false for every U256 x.
//! The consumer knows only the client's ZeroGuards vocabulary. Both use the
//! production EVM scheduler; neither drives an interpreter or a second solve.
#[path = "cooperative/consumer.rs"]
mod consumer;
#[path = "cooperative/contracts.rs"]
mod contracts;
#[path = "cooperative/producer.rs"]
mod producer;

use contracts::Failure;
use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        Config, EnginePolicies, ExecutionConfig, MachineError, OutcomeKind, Status,
        cooperation::{
            CooperationError, Extension, PairedTasks, PerInput, analyze_world_cooperating,
        },
    },
    domain::{AbstractValue, Domain, ValueAlgebra, facts::FactError},
    kernel::{Either, KernelError, Pair, PublishError, Registered},
    world::{Account, ByteArray, Entry, World},
};
use std::{convert::Infallible, num::NonZeroUsize};

type ClientFailure<P> = Failure<KernelError<MachineError>, PublishError<P, MachineError>>;
type Error = CooperationError<Either<ClientFailure<FactError>, ClientFailure<Infallible>>>;

fn main() -> Result<(), Error> {
    // A deliberately coarse numeric policy makes the independent proof useful.
    // Its frozen DomainSpec must agree with ExecutionConfig.analysis.
    let domain = Domain::new(NonZeroUsize::new(8).unwrap());
    let spec = domain.spec();
    let config = ExecutionConfig {
        analysis: Config {
            domain_profile: spec.profile(),
            max_constants: spec.capacity(),
            reduction_rounds: spec.reduction_rounds(),
            max_facts: spec.fact_limit(),
            relations: spec.relations(),
            ..Config::default()
        },
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let algebra = ValueAlgebra::new(domain);
    let owner = Address::from_word(U256::from(0x101).into());

    // g := (CALLDATALOAD(0) | 1) == 0; JUMP to 0x0a carrying g;
    // 0x0a: JUMPDEST; if g JUMP to 0x0f; STOP;
    // 0x0f: JUMPDEST; INVALID. The proof needs no restriction on calldata.
    let code = "5f356001175f14600a565b600f57005bfe";
    for (label, producer_enabled, consumer_enabled) in [
        ("both", true, true),
        ("without producer", false, true),
        ("without consumer", true, false),
    ] {
        let mut world = World::new(Fork::Osaka, "cooperative-example:offline");
        world
            .insert(owner, Account::from_hex(code, Fork::Osaka).unwrap())
            .unwrap();
        let entry = Entry::concrete(
            owner,
            Address::ZERO,
            AbstractValue::constant(U256::ZERO),
            ByteArray::unknown(),
        );
        // Consumer deliberately runs first. Its Pending read must cause an
        // automatic rerun when the independent producer publishes ZeroGuards.
        let mut extension = Extension::with_tasks(
            Pair::new(
                Registered::new(consumer::Consumer::new(consumer_enabled)),
                Registered::new(producer::Producer::new(producer_enabled)),
            ),
            PairedTasks::new(PerInput, PerInput),
        )
        .with_stores(contracts::Stores);
        let analysis = analyze_world_cooperating(
            world,
            entry,
            config.clone(),
            &algebra,
            &EnginePolicies::default(),
            &mut extension,
        )?;
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        let impossible = analysis
            .states()
            .iter()
            .any(|s| s.executed_pcs.contains(&0x0f));
        assert_eq!(impossible, !(producer_enabled && consumer_enabled));
        assert!(
            analysis
                .outcomes()
                .iter()
                .any(|o| o.kind == OutcomeKind::Return)
        );
        let consumer = extension.analyzer.left.rule();
        let producer = extension.analyzer.right.rule();
        assert!(consumer.pending > 0);
        if producer_enabled && consumer_enabled {
            assert!(consumer.refinements > 0);
            assert!(producer.feedback_observed > 0);
        }
        println!(
            "{label}: {:?}, impossible branch reached = {impossible}",
            analysis.status()
        );
    }
    Ok(())
}

#[test]
fn public_example_runs_with_both_removal_controls() {
    main().unwrap();
}
