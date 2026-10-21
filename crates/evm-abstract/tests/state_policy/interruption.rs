use super::{
    oracle,
    procedures::recursive_config,
    support::{complete, default_run, entry, input, recursive_counter, run, world},
};
use crate::native;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::domain::ProductAlgebra;
use evm_abstract::{
    analysis::{
        Acceleration, CorrelatedJoin, EnginePolicies, EvmMachinePayload, FeedbackAcceleration,
        FirstResumeStatus, FrontierReason, JoinBoundary, MachineError, MachineKey,
        MachinePartition, Partitioned, PartitionedRelative, PolicyContext, PolicyKey, PolicySite,
        ProcedureOutput, ProcedureSummaryPolicy, ProjectedSummaries, QuotedKey,
        RecentDistinctJumps, RelativeState, SemanticPolicies, Status, StructuralPartition,
        control::{CancelToken, Control},
        progress::Observer,
    },
    domain::NumericValue,
    world::EvmByteArray,
};

struct RefusePartition<'a> {
    site: &'a PolicySite,
}

impl MachinePartition<ProductAlgebra, Option<bool>> for RefusePartition<'_> {
    type Key = (MachineKey, Option<bool>);

    fn prepare(
        &self,
        input: &EvmMachinePayload<NumericValue>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<Partitioned<Self::Key, NumericValue>, MachineError> {
        if site == *self.site {
            return Err(MachineError::Shape);
        }
        StructuralPartition.prepare(input, site, class, cx)
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<NumericValue>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<PartitionedRelative<Self::Key, NumericValue>, MachineError> {
        if site == *self.site {
            return Err(MachineError::Shape);
        }
        StructuralPartition.prepare_relative(input, site, class, cx)
    }
}

#[test]
fn refused_machine_preparation_is_incomplete_in_ordinary_and_recursive_paths() {
    let world = world(&recursive_counter(false));
    let entry = entry(EvmByteArray::<NumericValue>::unknown());
    let baseline = default_run(&world, &entry, recursive_config());
    complete(&baseline);
    oracle::execute(&world, &super::support::entry(input(6))).covers(&baseline);
    for site in [PolicySite::World, PolicySite::Procedure, PolicySite::Body] {
        let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
            SemanticPolicies::new(
                RefusePartition { site: &site },
                CorrelatedJoin,
                FeedbackAcceleration,
                ProjectedSummaries,
            ),
        );
        let graph = run(&world, &entry, recursive_config(), &policies);
        assert_eq!(
            graph.status(),
            Status::Incomplete,
            "refused {site:?} is an obligation, not bottom"
        );
        assert!(
            graph
                .frontiers()
                .iter()
                .any(|f| f.reason == FrontierReason::StateJoin(JoinBoundary::Shape)),
            "missing refusal frontier at {site:?}: {:?}",
            graph.frontiers()
        );
        if site != PolicySite::World {
            assert!(
                graph.procedure_summaries().iter().any(|r| !r.closed),
                "refused recursive preparation cannot publish a closed relation"
            );
        }
    }
}

struct WorkRefusal<'a> {
    site: &'a PolicySite,
    only_growth: bool,
}

impl ProcedureSummaryPolicy<ProductAlgebra> for WorkRefusal<'_> {
    fn admit_relative(
        &self,
        current: &RelativeState<NumericValue>,
        incoming: &RelativeState<NumericValue>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<RelativeState<NumericValue>, MachineError> {
        if site == *self.site && (!self.only_growth || !current.semantic_eq(incoming, cx)?) {
            cx.reserve(usize::MAX)?;
        }
        ProjectedSummaries.admit_relative(current, incoming, site, acceleration, cx)
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<NumericValue>>,
        incoming: &ProcedureOutput<NumericValue>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<ProcedureOutput<NumericValue>, MachineError> {
        if *self.site == PolicySite::Output {
            let refusal = if self.only_growth {
                if let Some(old) = current {
                    cx.reserve(old.work_size().saturating_add(incoming.work_size()))?;
                    old != incoming
                } else {
                    false
                }
            } else {
                true
            };
            if refusal {
                cx.reserve(usize::MAX)?;
            }
        }
        ProjectedSummaries.admit_output(current, incoming, acceleration, cx)
    }
}

#[test]
fn body_subscriber_and_output_refusals_keep_pending_recursive_growth_open() {
    let world = world(&recursive_counter(true));
    let entry = entry(EvmByteArray::<NumericValue>::unknown());
    let baseline = default_run(&world, &entry, recursive_config());
    complete(&baseline);
    oracle::execute(&world, &super::support::entry(input(17))).covers(&baseline);
    // Growth-only cases allow first admission. They fail only when a retained
    // subscriber/output receives semantically different state, so default-only
    // retry or feedback paths cannot accidentally satisfy these tests.
    for (site, only_growth) in [
        (PolicySite::Body, false),
        (PolicySite::Subscriber, false),
        (PolicySite::Output, false),
        (PolicySite::Subscriber, true),
        (PolicySite::Output, true),
    ] {
        let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                FeedbackAcceleration,
                WorkRefusal {
                    site: &site,
                    only_growth,
                },
            ),
        );
        let graph = run(&world, &entry, recursive_config(), &policies);
        assert_eq!(
            graph.status(),
            Status::Incomplete,
            "unpaid {site:?}, growth={only_growth}"
        );
        assert!(
            graph
                .frontiers()
                .iter()
                .any(|f| f.reason == FrontierReason::Work),
            "{:?}",
            graph.frontiers()
        );
        assert!(
            graph.procedure_summaries().iter().any(|r| !r.closed),
            "unpaid body/subscriber/output admission cannot claim closure"
        );
    }
}

struct CancelBody<'a> {
    cancellation: &'a CancelToken,
}

impl ProcedureSummaryPolicy<ProductAlgebra> for CancelBody<'_> {
    fn admit_relative(
        &self,
        current: &RelativeState<NumericValue>,
        incoming: &RelativeState<NumericValue>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<RelativeState<NumericValue>, MachineError> {
        if site == PolicySite::Body {
            self.cancellation.cancel();
            cx.reserve(0)?;
        }
        ProjectedSummaries.admit_relative(current, incoming, site, acceleration, cx)
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<NumericValue>>,
        incoming: &ProcedureOutput<NumericValue>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<ProcedureOutput<NumericValue>, MachineError> {
        ProjectedSummaries.admit_output(current, incoming, acceleration, cx)
    }
}

#[test]
fn cancellation_inside_a_borrowed_recursive_policy_returns_no_report_and_scope_recovers() {
    let world = world(&recursive_counter(false));
    let entry = entry(EvmByteArray::<NumericValue>::unknown());
    let control = Control::new(Observer::default());
    let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            CorrelatedJoin,
            FeedbackAcceleration,
            CancelBody {
                cancellation: control.cancellation(),
            },
        ),
    );
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&native::product(), &policies, &mut extension).run(
            AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
            recursive_config(),
            &control,
        )
    };
    assert!(
        control.cancellation().is_cancelled(),
        "the actual relative body must reach the policy"
    );
    assert!(matches!(
        result,
        Err(evm_abstract::analysis::cooperation::CooperationError::Cancelled)
    ));
    let recovered = default_run(&world, &entry, recursive_config());
    complete(&recovered);
    oracle::execute(&world, &super::support::entry(input(6))).covers(&recovered);
}

struct QuotedPartition<'a> {
    refused: &'a PolicySite,
}

impl MachinePartition<ProductAlgebra, Option<bool>> for QuotedPartition<'_> {
    type Key = QuotedKey<(MachineKey, Option<bool>)>;

    fn prepare(
        &self,
        input: &EvmMachinePayload<NumericValue>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<Partitioned<Self::Key, NumericValue>, MachineError> {
        let prepared = StructuralPartition.prepare(input, site, class, cx)?;
        let work = if site == *self.refused {
            usize::MAX
        } else {
            prepared.key.key_work()
        };
        Ok(Partitioned {
            key: QuotedKey::new(prepared.key, work),
            payload: prepared.payload,
        })
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<NumericValue>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<PartitionedRelative<Self::Key, NumericValue>, MachineError> {
        let prepared = StructuralPartition.prepare_relative(input, site, class, cx)?;
        let work = if site == *self.refused {
            usize::MAX
        } else {
            prepared.key.key_work()
        };
        Ok(PartitionedRelative {
            key: QuotedKey::new(prepared.key, work),
            payload: prepared.payload,
        })
    }
}

#[test]
fn unpriced_keys_keep_real_world_procedure_body_and_subscriber_obligations_open() {
    let fixture = recursive_counter(false);
    let world = world(&fixture);
    let entry = entry(EvmByteArray::<NumericValue>::unknown());
    let baseline = default_run(&world, &entry, recursive_config());
    complete(&baseline);
    oracle::execute(&world, &super::support::entry(input(6))).covers(&baseline);
    for site in [
        PolicySite::World,
        PolicySite::Procedure,
        PolicySite::Body,
        PolicySite::Subscriber,
    ] {
        let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
            SemanticPolicies::new(
                QuotedPartition { refused: &site },
                CorrelatedJoin,
                FeedbackAcceleration,
                ProjectedSummaries,
            ),
        );
        let graph = run(&world, &entry, recursive_config(), &policies);
        assert_eq!(
            graph.status(),
            Status::Incomplete,
            "unpriced {site:?} key cannot settle the execution"
        );
        assert!(
            graph
                .frontiers()
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Work),
            "missing {site:?} key obligation: {:?}",
            graph.frontiers()
        );
        assert_eq!(graph.states()[0].program().unwrap().bytes(), fixture.bytes);
        if site == PolicySite::World {
            assert!(graph.states().iter().all(|state| state.exit.is_none()));
            assert!(graph.edges().is_empty());
        } else {
            assert!(
                graph
                    .procedure_summaries()
                    .iter()
                    .any(|record| !record.closed)
            );
        }
    }
}
