//! Public navigation of true entries across callers and site-specific Body keys.
use super::*;
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::{
    Fork,
    analysis::{
        CorrelatedJoin, EvmMachinePayload, FeedbackAcceleration, MachineKey, MachinePartition,
        Partitioned, PartitionedRelative, PolicyContext, PolicySite, ProjectedSummaries,
        RelativeState, SemanticPolicies, StructuralPartition,
        cooperation::{
            CalleeRoute, Callees, ProcedureEntries, ProcedureEntryRoute, ProcedureId,
            ProcedureRoute, Procedures,
        },
    },
    domain::{EvmNumericAlgebra, EvmValue},
    world::EvmAccount,
};
use std::collections::BTreeMap;

struct BodyKeys(bool);
impl<D: EvmNumericAlgebra> MachinePartition<D, Option<bool>> for BodyKeys {
    type Key = (MachineKey, Option<bool>, bool);
    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError> {
        let Partitioned {
            key: (key, class),
            payload,
        } = StructuralPartition.prepare(input, site, class, cx)?;
        Ok(Partitioned {
            key: (key, class, false),
            payload,
        })
    }
    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError> {
        let exact = if self.0 && site == PolicySite::Body {
            let native = input.instantiate(cx)?;
            let counter = native
                .store
                .slots()
                .get(&(native.active().key.address, U256::from(2)));
            if let Some(counter) = counter {
                cx.reserve(counter.singleton_work())?;
                counter.singleton().is_some()
            } else {
                true
            }
        } else {
            false
        };
        let PartitionedRelative {
            key: (key, class),
            payload,
        } = StructuralPartition.prepare_relative(input, site, class, cx)?;
        Ok(PartitionedRelative {
            key: (key, class, exact),
            payload,
        })
    }
}
#[derive(Default)]
struct Navigation {
    feedback: bool,
    callers: BTreeMap<ProcedureId, BTreeSet<Point>>,
    entries: BTreeMap<ProcedureId, BTreeSet<Subject>>,
    counters: BTreeMap<ProcedureId, BTreeSet<(U256, U256)>>,
    self_call_bodies: BTreeMap<(ProcedureId, usize), BTreeSet<Point>>,
    refined: usize,
}

impl<Cx> Reduction<Cx> for Navigation
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Callees, CalleeRoute, Error = Infrastructure>
        + Read<Procedures, ProcedureRoute, Error = Infrastructure>
        + Read<ProcedureEntries, ProcedureEntryRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Accepts<Refinements, RefinementRoute, Error = PublishError<FactError, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Failure;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        assert_ne!(
            subject.point.kind,
            evm_abstract::analysis::cooperation::PointKind::ProcedureEntry,
            "readonly entries never receive component tasks"
        );
        let own = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(own) = own.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(own.work_size().saturating_mul(4))?;
        let callees = <Cx as Read<Callees, CalleeRoute>>::query(
            cx,
            Scoped::new(own.universe(), subject.point),
        )?;
        if let Some(callees) = callees.value() {
            for id in callees {
                let descriptor = <Cx as Read<ProcedureEntries, ProcedureEntryRoute>>::query(
                    cx,
                    Scoped::new(own.universe(), *id),
                )?;
                let Some(descriptor) = descriptor.value() else {
                    return Ok(LocalClosure::Stable);
                };
                assert_eq!(
                    descriptor.entry.kind,
                    evm_abstract::analysis::cooperation::PointKind::ProcedureEntry
                );
                let entry =
                    current_input::<ExcludingWord, _>(cx, own.universe(), descriptor.entry)?;
                let Some(entry) = entry.value() else {
                    return Ok(LocalClosure::Stable);
                };
                cx.charge(entry.work_size().saturating_mul(4))?;
                assert_eq!(
                    entry.payload().active().key.code_hash,
                    descriptor.frame.code_hash
                );
                assert_eq!(
                    entry.payload().active().key.basic_block_index,
                    descriptor.frame.basic_block_index,
                    "entry is its retained native source, independent of selected Body keys"
                );
                self.callers.entry(*id).or_default().insert(subject.point);
                self.entries.entry(*id).or_default().insert(entry.subject());
                if let Some(counter) = entry
                    .payload()
                    .store
                    .slots()
                    .get(&(descriptor.frame.address, U256::from(2)))
                {
                    let (lo, hi) = counter.numeric().observation().interval().unsigned_bounds();
                    self.counters.entry(*id).or_default().insert((lo, hi));
                }
                if subject.point.kind == evm_abstract::analysis::cooperation::PointKind::Body
                    && own.payload().active().key.code_hash == descriptor.frame.code_hash
                {
                    self.self_call_bodies
                        .entry((*id, fixtures::input_pc(own).expect("native runtime block")))
                        .or_default()
                        .insert(subject.point);
                }
            }
        }
        // Fixture slot0 is immutable. Reading each actual matching entry gives
        // the complete bound used for this proof; no table index/first Body is
        // guessed and no incomplete may enumeration proves an absent behavior.
        let ids =
            <Cx as Read<Procedures, ProcedureRoute>>::query(cx, Scoped::new(own.universe(), ()))?;
        let mut covered = false;
        if let Some(ids) = ids.value() {
            for id in ids {
                let info = <Cx as Read<ProcedureEntries, ProcedureEntryRoute>>::query(
                    cx,
                    Scoped::new(own.universe(), *id),
                )?;
                let Some(info) = info.value() else {
                    return Ok(LocalClosure::Stable);
                };
                if info.frame.code_hash != own.payload().active().key.code_hash {
                    continue;
                }
                let input = current_input::<ExcludingWord, _>(cx, own.universe(), info.entry)?;
                let Some(input) = input.value() else {
                    return Ok(LocalClosure::Stable);
                };
                cx.charge(input.work_size().saturating_mul(4))?;
                covered |= input
                    .payload()
                    .store
                    .slots()
                    .get(&(info.frame.address, U256::ZERO))
                    .is_some_and(|value| !value.numeric().contains(U256::from(42)));
            }
        }
        let guard = own.payload().active().stack.get(1).and_then(EvmValue::expression).is_some_and(|expression| {
            matches!(expression.kind(), ExprKind::Operation { opcode: op, args } if *op == opcode::EQ && args.iter().any(|arg| matches!(arg.kind(), ExprKind::Constant(value) if *value == U256::from(42))))
        });
        if self.feedback && covered && guard {
            <Cx as Accepts<Refinements, RefinementRoute>>::accept(
                cx,
                Scoped::new(subject.scope, ()),
                StackFacts::at(1, WordObservation::constant(U256::ZERO)),
            )
            .map_err(Failure::Feedback)?;
            self.refined += 1;
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn true_entries_support_multicaller_growth_and_distinct_body_policy_keys() {
    for split in [false, true] {
        let mut code = Vec::new();
        for _ in 0..2 {
            code.extend([
                opcode::PUSH1,
                32,
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH2,
                2,
                1,
                opcode::GAS,
                opcode::CALL,
                opcode::POP,
            ]);
        }
        code.extend([opcode::PUSH1, 32, opcode::PUSH0, opcode::RETURN]);
        let recursive = super::super::bridge::fixture::guarded(true);
        let mut world = fixtures::world(&code);
        let mut callee =
            EvmAccount::from_hex(&alloy_primitives::hex::encode(&recursive.code), Fork::Osaka)
                .unwrap();
        callee.balance = fixtures::word(1_000_000);
        callee.storage_unknown = false;
        callee.storage.insert(
            U256::ZERO,
            EvmValue::from_numeric(ExcludingWord::except(U256::from(42))),
        );
        world
            .insert(fixtures::address(fixtures::CALLEE), callee)
            .unwrap();
        let calls = Calls::default();
        let mut numeric = Policy::new(&calls);
        numeric.exclusion_queries = false;
        numeric.relations.enabled = false;
        let algebra = evm_abstract::domain::ValueAlgebra::new(numeric);
        let state = SemanticPolicies::new(
            BodyKeys(split),
            CorrelatedJoin,
            FeedbackAcceleration,
            ProjectedSummaries,
        );
        let policies = EnginePolicies::default().with_state(state);
        let mut extension = Extension::new(Registered::new(Navigation {
            feedback: true,
            ..Navigation::default()
        }));
        let mut config = fixtures::config(5_000_000_000);
        config.analysis.context_depth = 0;

        let result = AnalysisEngine::new(&algebra, &policies, &mut extension)
            .run(
                AnalysisInput::new(world, fixtures::entry()).unwrap(),
                config,
                &Control::default(),
            )
            .unwrap();
        fixtures::complete(&result);
        let nav = extension.analyzer.rule();
        assert!(
            nav.callers.values().any(|points| points
                .iter()
                .filter(|point| point.kind == evm_abstract::analysis::cooperation::PointKind::World)
                .count()
                > 1),
            "multiple real World origins must discover the same callee entry"
        );
        assert!(
            nav.entries.values().any(|scopes| scopes.len() > 1),
            "native entry growth must replace its readonly generation"
        );
        assert!(
            nav.counters.values().any(|values| values.len() > 1),
            "entry growth must be visible in actual published payloads"
        );
        assert_eq!(
            nav.self_call_bodies.values().any(|points| points.len() > 1),
            split,
            "injected Body keys must remain distinct at the same native block while sharing the true procedure entry"
        );
        assert!(
            nav.refined > 0,
            "entry proof must feed actual executable World/Body points"
        );
        assert!(
            result
                .procedure_summaries()
                .iter()
                .flat_map(|record| &record.procedures)
                .flat_map(|procedure| &procedure.outputs)
                .any(
                    |output| output.kind == evm_abstract::analysis::OutcomeKind::Return
                        && output
                            .data
                            .read_word(&fixtures::word(0), algebra.borrowed())
                            .contains(U256::ONE)
                )
        );
        assert!(
            nav.entries.values().all(|scopes| scopes.len() < 60),
            "unchanged versions must settle"
        );
    }
}
