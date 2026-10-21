use super::*;
use evm_abstract::analysis::cooperation::{CooperationError, PointKind};

struct Missing;
impl<Cx> Analyzer<Cx> for Missing
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Infrastructure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        if let Some(input) = input.value() {
            let missing = current_input::<ExcludingWord, _>(
                cx,
                input.universe(),
                Point {
                    namespace: 0,
                    kind: PointKind::World,
                    index: usize::MAX,
                },
            )?;
            assert!(matches!(
                missing.state,
                analysis_kernel::Availability::Pending
            ));
        }
        Ok(LocalClosure::Stable)
    }
}
struct Failing;
impl<Cx> Analyzer<Cx> for Failing {
    type Task = Subject;
    type Error = &'static str;
    fn evaluate(&mut self, _: &Subject, _: &mut Cx) -> Result<LocalClosure, Self::Error> {
        Err("independent rule failed")
    }
}
#[test]
fn unknown_global_provider_and_independent_error_never_certify_completion() {
    let calls = Calls::default();
    let algebra = evm_abstract::domain::ValueAlgebra::new(Policy::new(&calls));
    let mut missing = Extension::new(Missing);
    let result = analyze_world_cooperating(
        fixtures::world(&[opcode::STOP]),
        fixtures::entry(),
        fixtures::config(100_000_000),
        &algebra,
        &EnginePolicies::default(),
        &mut missing,
    );
    assert!(matches!(
        result,
        Err(CooperationError::Unfinished(
            analysis_kernel::Closure::Open(_)
        ))
    ));
    let mut failing = Extension::new(Failing);
    let result = analyze_world_cooperating(
        fixtures::world(&[opcode::STOP]),
        fixtures::entry(),
        fixtures::config(100_000_000),
        &algebra,
        &EnginePolicies::default(),
        &mut failing,
    );
    assert!(matches!(
        result,
        Err(CooperationError::Extension("independent rule failed"))
    ));
}

#[derive(Default)]
struct Explore {
    origin: Option<Point>,
    scopes: std::collections::BTreeSet<analysis_kernel::Scope>,
    pending: usize,
    found: usize,
}
impl<Cx> Analyzer<Cx> for Explore
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Discover<Topology, TopologyRoute, Error = Infrastructure>
        + Discover<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Infrastructure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        let answer = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = answer.value() else {
            return Ok(LocalClosure::Stable);
        };
        let origin = *self.origin.get_or_insert(subject.point);
        if subject.point != origin {
            return Ok(LocalClosure::Stable);
        }
        self.scopes.insert(subject.scope);
        // These observations find future work; empty discovery is not a must
        // premise and does not prevent the current native block from running.
        let edges = <Cx as Discover<Topology, TopologyRoute>>::discover(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: origin,
                    direction: Direction::Successors,
                },
            ),
        )?;
        if let Some(edges) = edges.value() {
            for edge in edges {
                let future = evm_abstract::analysis::cooperation::discover_input::<ExcludingWord, _>(
                    cx,
                    input.universe(),
                    edge.to,
                )?;
                match future.state {
                    analysis_kernel::Availability::Pending => self.pending += 1,
                    analysis_kernel::Availability::Absent => {}
                    analysis_kernel::Availability::Value(future) => {
                        cx.charge(future.work_size())?;
                        let value = &future.payload().active().stack[0];
                        cx.charge(value.singleton_work())?;
                        assert_eq!(value.singleton(), Some(U256::ZERO));
                        self.found += 1;
                    }
                }
            }
        }
        Ok(LocalClosure::Stable)
    }
}
#[test]
fn optional_future_discovery_expands_native_cfg_then_wakes_the_same_generation() {
    let calls = Calls::default();
    let algebra = evm_abstract::domain::ValueAlgebra::new(Policy::new(&calls));
    let mut extension = Extension::new(Explore::default());
    let result = analyze_world_cooperating(
        fixtures::world(&[opcode::PUSH0, opcode::JUMPDEST, opcode::STOP]),
        fixtures::entry(),
        fixtures::config(100_000_000),
        &algebra,
        &EnginePolicies::default(),
        &mut extension,
    )
    .unwrap();
    fixtures::complete(&result);
    assert!(
        extension.analyzer.pending > 0,
        "a real discovered target exists before its Input is seeded"
    );
    assert!(
        extension.analyzer.found > 0,
        "later publication must wake the original observer"
    );
    assert_eq!(
        extension.analyzer.scopes.len(),
        1,
        "discovery changes wake without retiring a must universe that did not depend on completeness"
    );
    assert!(
        result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&2))
    );
}
