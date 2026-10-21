//! Successful opaque outputs owe a separate, single singleton proof.

use super::*;
use crate::domain::WordObservation;
use serde::Serialize;
use std::{fmt, rc::Rc};

use super::super::super::pack as pack_in;

const MEMBERS: usize = 512;
const QUERY_FEE: usize = MEMBERS * 64 + 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interrupt {
    None,
    Quote,
    Query,
}

#[derive(Debug)]
struct Probe {
    interrupt: Interrupt,
    quotes: Cell<usize>,
    queries: Cell<usize>,
    visits: Cell<usize>,
    copies: Cell<usize>,
    observations: Cell<usize>,
}
impl Probe {
    fn new(interrupt: Interrupt) -> Rc<Self> {
        Rc::new(Self {
            interrupt,
            quotes: Cell::new(0),
            queries: Cell::new(0),
            visits: Cell::new(0),
            copies: Cell::new(0),
            observations: Cell::new(0),
        })
    }
}

// The exported Top deliberately forgets both complete membership and parity.
// A successful pack must retain this actual carrier and its owned proof data.
#[derive(Debug, Serialize)]
struct Queried {
    members: Vec<U256>,
    odd: bool,
    #[serde(skip)]
    probe: Option<Rc<Probe>>,
}
impl PartialEq for Queried {
    fn eq(&self, other: &Self) -> bool {
        self.odd == other.odd
            && self.members.iter().copied().collect::<BTreeSet<_>>()
                == other.members.iter().copied().collect::<BTreeSet<_>>()
    }
}
impl Eq for Queried {}
impl Clone for Queried {
    fn clone(&self) -> Self {
        if let Some(probe) = &self.probe {
            probe.copies.set(probe.copies.get() + 1);
        }
        Self {
            members: self.members.clone(),
            odd: self.odd,
            probe: self.probe.as_ref().map(Rc::clone),
        }
    }
}
impl fmt::Display for Queried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} retained members; odd={}",
            self.members.len(),
            self.odd
        )
    }
}
impl WordElement for Queried {
    fn top() -> Self {
        Self {
            members: Vec::new(),
            odd: false,
            probe: None,
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            members: vec![value],
            ..Self::top()
        }
    }
    fn from_observation(observation: &WordObservation) -> Self {
        observation
            .singleton()
            .map_or_else(Self::top, Self::constant)
    }
    fn observation(&self) -> WordObservation {
        if let Some(probe) = &self.probe {
            probe.observations.set(probe.observations.get() + 1);
        }
        WordObservation::top()
    }
    fn singleton(&self) -> Option<U256> {
        if let Some(probe) = &self.probe {
            probe.queries.set(probe.queries.get() + 1);
        }
        let first = *self.members.first()?;
        let mut singleton = true;
        for member in &self.members {
            if let Some(probe) = &self.probe {
                probe.visits.set(probe.visits.get() + 1);
            }
            singleton &= *member == first;
        }
        if self
            .probe
            .as_ref()
            .is_some_and(|p| p.interrupt == Interrupt::Query)
        {
            analysis_control::current_cancellation().cancel();
        }
        (singleton && (!self.odd || first.bit(0))).then_some(first)
    }
    fn singleton_work(&self) -> usize {
        if let Some(probe) = &self.probe {
            probe.quotes.set(probe.quotes.get() + 1);
            if probe.interrupt == Interrupt::Quote {
                analysis_control::current_cancellation().cancel();
            }
        }
        // A cached length funds each four-limb comparison and the final parity
        // check. Quotation neither scans nor exports the retained payload.
        self.members.len().saturating_mul(64).saturating_add(64)
    }
    fn contains(&self, word: U256) -> bool {
        (self.members.is_empty() || self.members.contains(&word)) && (!self.odd || word.bit(0))
    }
    fn is_top(&self) -> bool {
        self.members.is_empty() && !self.odd
    }
    fn work_size(&self) -> usize {
        self.members.len().saturating_mul(32).saturating_add(16)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Exact,
    Odd,
}

#[derive(Debug)]
struct Successful {
    relations: bool,
    shape: Shape,
    probe: Rc<Probe>,
}
impl EvmNumericAlgebra for Successful {
    type Element = Queried;
    fn spec(&self) -> DomainSpec {
        Domain::default().spec().with_relations(RelationLimits {
            enabled: self.relations,
            ..Default::default()
        })
    }
    fn project_numeric(&self, value: &Queried) -> Queried {
        value.clone()
    }
    fn join_numeric(&self, _left: &Queried, _right: &Queried) -> Queried {
        Queried::top()
    }
    fn widen_numeric(&self, _old: &Queried, _next: &Queried) -> Queried {
        Queried::top()
    }
    fn widen_procedure_numeric(&self, _old: &Queried, _next: &Queried) -> Queried {
        Queried::top()
    }
    fn reduce_numeric(&self, value: &Queried) -> NumericReduction<Queried> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, op: u8, args: &[Queried]) -> NumericReduction<Queried> {
        let [left, right] = args else {
            return NumericReduction::stable(Queried::top());
        };
        if left.members.is_empty()
            || right.members.is_empty()
            || !matches!(op, opcode::SHL | opcode::OR)
        {
            return NumericReduction::stable(Queried::top());
        }
        let mut members = BTreeSet::new();
        for left in &left.members {
            for right in &right.members {
                let value = if op == opcode::OR {
                    *left | *right
                } else {
                    usize::try_from(*left)
                        .ok()
                        .filter(|shift| *shift < 256)
                        .map_or(U256::ZERO, |shift| *right << shift)
                };
                members.insert(value);
                if members.len() > MEMBERS {
                    return NumericReduction::stable(Queried::top());
                }
            }
        }
        NumericReduction::stable(Queried {
            odd: self.shape == Shape::Odd && members.iter().all(|word| word.bit(0)),
            members: members.into_iter().collect(),
            probe: None,
        })
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Queried>) -> usize {
        args.fold(1usize, |pairs, value| {
            pairs.saturating_mul(value.members.len().saturating_add(1))
        })
        .saturating_mul(1024)
    }
    fn projection_work(&self, value: &Queried) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &Queried, facts: usize) -> usize {
        value.work_size().saturating_add(facts).saturating_mul(256)
    }
    fn pack_word_numeric(&self, bytes: &[&Queried; 32]) -> Option<Queried> {
        // Only these two fixed tuples opt in: each has the exact balanced fold
        // given below. No default observation import reconstructs the result.
        let first = match self.shape {
            Shape::Exact => {
                assert!(bytes.iter().all(|b| b.members == [U256::from(7)]));
                U256::from_be_bytes([7u8; 32])
            }
            Shape::Odd => {
                assert!(bytes[..31].iter().all(|b| b.members == [U256::ZERO]));
                assert_eq!(bytes[31].members, [U256::ONE, U256::from(3)]);
                assert!(bytes[31].odd);
                U256::ONE
            }
        };
        let mut members = vec![first; MEMBERS];
        if self.shape == Shape::Odd {
            members[MEMBERS - 1] = U256::from(3);
        }
        Some(Queried {
            members,
            odd: self.shape == Shape::Odd,
            probe: Some(Rc::clone(&self.probe)),
        })
    }
}

fn policy(shape: Shape, relations: bool, probe: &Rc<Probe>) -> Paid<Successful> {
    Paid::new(
        Successful {
            relations,
            shape,
            probe: Rc::clone(probe),
        },
        Some(FEE),
        Hook::Delegate,
    )
}
fn bytes(shape: Shape) -> [EvmValue<Queried>; 32] {
    std::array::from_fn(|i| {
        let numeric = if shape == Shape::Odd && i == 31 {
            Queried {
                members: vec![U256::ONE, U256::from(3)],
                odd: true,
                probe: None,
            }
        } else {
            Queried::constant(if shape == Shape::Exact {
                U256::from(7)
            } else {
                U256::ZERO
            })
        };
        let mut byte = EvmValue::from_numeric(numeric);
        byte.set_origin(Origin::Storage);
        byte.with_symbolic_limit()
    })
}

fn balanced(bytes: &[EvmValue<Queried>; 32], policy: &Successful) -> EvmValue<Queried> {
    let domain = ValueAlgebra::new(policy);
    let mut parts: Vec<_> = bytes
        .iter()
        .enumerate()
        .map(|(i, byte)| {
            domain.apply(
                opcode::SHL,
                &[EvmValue::constant(U256::from((31 - i) * 8)), byte.clone()],
            )
        })
        .collect();
    while parts.len() > 1 {
        parts = parts
            .chunks(2)
            .map(|pair| domain.apply(opcode::OR, pair))
            .collect();
    }
    parts.pop().unwrap()
}

#[test]
fn successful_custom_pack_pays_one_opaque_query_and_reuses_it_for_metadata() {
    for path in PATHS {
        for (shape, relations) in [
            (Shape::Exact, true),
            (Shape::Exact, false),
            (Shape::Odd, false),
            (Shape::Odd, true),
        ] {
            let probe = Probe::new(Interrupt::None);
            let policy = policy(shape, relations, &probe);
            let expected = balanced(&bytes(shape), &policy.domain);
            let (_, setup, _) = context(path, usize::MAX, &policy, |_| ());
            let (result, used, exhausted) =
                context(path, setup + FEE + QUERY_FEE, &policy, |context| {
                    let before = context.demand_usage();
                    let result = pack_in(&bytes(shape), context.algebra(), context);
                    assert_eq!(context.demand_usage(), before);
                    assert!(!context.stopped());
                    if matches!(path, Path::Prepared) {
                        assert_eq!(numeric_receipts(path, context), [FEE, QUERY_FEE]);
                    }
                    result
                });
            assert_eq!(used, setup + FEE + QUERY_FEE);
            assert!(!exhausted);
            assert_eq!(policy.packs.get(), 1);
            assert_eq!(probe.quotes.get(), 1);
            assert_eq!(probe.queries.get(), 1);
            assert_eq!(probe.visits.get(), MEMBERS);
            assert_eq!(probe.copies.get(), 0, "the wrapper moves the custom result");
            assert_eq!(
                probe.observations.get(),
                0,
                "hidden constraints never round-trip"
            );
            assert!(policy.actions.borrow().is_empty());
            if shape == Shape::Odd && relations {
                assert!(
                    result.is_none(),
                    "relation terms still require the ordinary fold"
                );
            } else {
                let result = result.unwrap();
                assert_eq!(result, expected);
                assert_eq!(result.identity(), expected.identity());
                assert_eq!(result.expression(), expected.expression());
                assert_eq!(result.numeric().members.len(), MEMBERS);
                assert_eq!(result.symbolic_limit_reached(), shape == Shape::Odd);
                assert!(result.expression().is_none());
                assert!(result.identity().input().is_none());
                assert_eq!(
                    result.provenance().origins().sources(),
                    Some(&BTreeSet::from([
                        Origin::Constant,
                        Origin::Arithmetic,
                        Origin::Storage
                    ]))
                );
                if shape == Shape::Odd {
                    assert!(result.contains(U256::ONE) && result.contains(U256::from(3)));
                    assert!(!result.contains(U256::from(2)));
                } else {
                    assert!(result.contains(U256::from_be_bytes([7u8; 32])));
                }
            }
        }
    }
}

#[test]
fn query_receipt_minus_one_stops_direct_and_reader_paths_before_query_or_publication() {
    let state = ByteStateId::exact(Arc::from([7u8; 32]));
    let offset = ExprId::constant(U256::ZERO);
    for path in PATHS {
        for through_reader in [false, true] {
            let read = |context: &Context<'_, Queried, Paid<Successful>>| {
                if through_reader {
                    let mut reader = Reader::with_context(context.clone(), context.algebra());
                    let caches = reader.cache_stats();
                    let result = reader.word(&state, &offset);
                    if context.stopped() {
                        assert_eq!(result, EvmValue::top());
                        assert_eq!(reader.cache_stats(), caches);
                        None
                    } else {
                        Some(result)
                    }
                } else {
                    pack_in(&bytes(Shape::Exact), context.algebra(), context)
                }
            };
            let pilot = Probe::new(Interrupt::None);
            let pilot_policy = policy(Shape::Exact, !through_reader, &pilot);
            let (_, full, exhausted) = context(path, usize::MAX, &pilot_policy, |context| {
                let result = read(context).unwrap();
                assert_eq!(result.numeric().members.len(), MEMBERS);
                assert_eq!(pilot.queries.get(), 1);
                assert!(!context.stopped());
            });
            assert!(!exhausted);
            if !through_reader {
                let (_, setup, _) = context(path, usize::MAX, &pilot_policy, |_| ());
                assert_eq!(full, setup + FEE + QUERY_FEE);
            }
            let probe = Probe::new(Interrupt::None);
            let policy = policy(Shape::Exact, !through_reader, &probe);
            let (_, used, exhausted) = context(path, full - 1, &policy, |context| {
                let before = context.demand_usage();
                assert!(read(context).is_none());
                assert!(context.work_exhausted());
                assert!(!context.cache_refused());
                if !through_reader {
                    assert_eq!(context.demand_usage(), before);
                }
                if matches!(path, Path::Prepared) {
                    assert_eq!(numeric_receipts(path, context), [FEE]);
                }
                let usage = context.demand_usage();
                let clone = context.clone();
                assert!(read(&clone).is_none());
                assert_eq!(
                    word_plan::evaluate(&state, None, &offset, &clone, clone.algebra()),
                    Ok(None)
                );
                assert_eq!(context.demand_usage(), usage);
            });
            assert_eq!(used, full - QUERY_FEE);
            assert!(exhausted);
            assert_eq!(policy.quotes.get(), 1);
            assert_eq!(policy.packs.get(), 1);
            assert_eq!(probe.quotes.get(), 1);
            assert_eq!(probe.queries.get(), 0);
            assert_eq!(probe.visits.get(), 0);
            assert_eq!(probe.copies.get(), 0);
            assert_eq!(probe.observations.get(), 0);
            assert!(policy.actions.borrow().is_empty());
        }
    }
}

#[test]
fn custom_result_query_cancellation_stops_reader_before_metadata_or_fallback() {
    let state = ByteStateId::exact(Arc::from([7u8; 32]));
    let offset = ExprId::constant(U256::ZERO);
    for path in PATHS {
        for interrupt in [Interrupt::Quote, Interrupt::Query] {
            let probe = Probe::new(interrupt);
            let policy = policy(Shape::Exact, false, &probe);
            context(path, usize::MAX, &policy, |context| {
                let token = analysis_control::Cancellation::new();
                let mut reader = Reader::with_context(context.clone(), context.algebra());
                let caches = reader.cache_stats();
                analysis_control::with_cancellation(&token, || {
                    assert_eq!(reader.word(&state, &offset), EvmValue::top());
                });
                assert_eq!(context.check(), Err(ExprError::Cancelled));
                assert_eq!(reader.cache_stats(), caches);
                if matches!(path, Path::Prepared) {
                    assert_eq!(
                        numeric_receipts(path, context),
                        if interrupt == Interrupt::Query {
                            vec![FEE, QUERY_FEE]
                        } else {
                            vec![FEE]
                        }
                    );
                }
                let usage = context.demand_usage();
                assert_eq!(
                    word_plan::evaluate(&state, None, &offset, &context.clone(), context.algebra()),
                    Err(ExprError::Cancelled.into())
                );
                assert_eq!(context.demand_usage(), usage);
            });
            assert_eq!(policy.packs.get(), 1);
            assert_eq!(probe.quotes.get(), 1);
            assert_eq!(
                probe.queries.get(),
                usize::from(interrupt == Interrupt::Query)
            );
            assert_eq!(
                probe.visits.get(),
                if interrupt == Interrupt::Query {
                    MEMBERS
                } else {
                    0
                }
            );
            assert_eq!(probe.copies.get(), 0);
            assert_eq!(probe.observations.get(), 0);
            assert!(policy.actions.borrow().is_empty());
        }
    }
}

#[test]
fn prepared_query_payment_cancellation_retains_both_fees_without_querying() {
    let probe = Probe::new(Interrupt::None);
    let policy = policy(Shape::Exact, false, &probe);
    context(Path::Prepared, usize::MAX, &policy, |context| {
        let token = analysis_control::Cancellation::new();
        context.cancel_after_numeric_payments(2, token.clone());
        analysis_control::with_cancellation(&token, || {
            assert!(pack_in(&bytes(Shape::Exact), context.algebra(), context).is_none());
        });
        assert_eq!(context.check(), Err(ExprError::Cancelled));
        assert_eq!(numeric_receipts(Path::Prepared, context), [FEE, QUERY_FEE]);
    });
    assert_eq!(policy.packs.get(), 1);
    assert_eq!(probe.quotes.get(), 1);
    assert_eq!(probe.queries.get(), 0);
    assert_eq!(probe.copies.get(), 0);
}

#[test]
fn pure_library_custom_success_remains_unmetered_without_a_pack_quote() {
    let probe = Probe::new(Interrupt::None);
    let mut policy = policy(Shape::Exact, true, &probe);
    policy.fee = None;
    let actual = super::super::pack(&bytes(Shape::Exact), ValueAlgebra::new(&policy)).unwrap();
    assert_eq!(actual.numeric().members.len(), MEMBERS);
    assert_eq!(policy.quotes.get(), 0);
    assert_eq!(policy.packs.get(), 1);
    assert_eq!(probe.quotes.get(), 1);
    assert_eq!(probe.queries.get(), 1);
    assert_eq!(probe.copies.get(), 0);
}
