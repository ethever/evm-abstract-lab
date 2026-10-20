//! A numeric carrier whose parity guarantee is deliberately absent from exports.

use super::{EvmByteArray, conditional};
use crate::{
    Fork,
    domain::{
        Domain, DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, ValueAlgebra,
        WordElement, WordObservation,
        facts::{FactError, Symbol},
        symbolic::ExprId,
    },
    resource::WorkBudget,
    world::{EvmAccount, EvmLog, EvmStore, EvmWorld, LogKey, LogSourceKind},
};
use alloy_primitives::{Address, B256, U256};
use revm_bytecode::opcode;
use serde::Serialize;
use std::{cell::Cell, collections::BTreeMap, fmt};

static BINDING_IMPORTS: crate::test_instrumentation::Counter<usize> =
    crate::test_instrumentation::Counter::new(0);
static SINGLETON_QUERIES: crate::test_instrumentation::Counter<usize> =
    crate::test_instrumentation::Counter::new(0);

mod cancellation;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Word {
    lower: U256,
    upper: U256,
    odd: bool,
}

impl Word {
    fn odd_byte() -> Self {
        Self {
            lower: U256::ZERO,
            upper: U256::from(255),
            odd: true,
        }
    }
    fn bounds(lower: U256, upper: U256) -> Self {
        Self {
            lower,
            upper,
            odd: false,
        }
    }
}

impl fmt::Display for Word {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}..={} odd={}",
            self.lower, self.upper, self.odd
        )
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self::bounds(U256::ZERO, U256::MAX)
    }
    fn constant(value: U256) -> Self {
        Self::bounds(value, value)
    }
    fn from_observation(observation: &WordObservation) -> Self {
        let (lower, upper) = observation.interval().unsigned_bounds();
        observation
            .singleton()
            .map_or_else(|| Self::bounds(lower, upper), Self::constant)
    }
    fn observation(&self) -> WordObservation {
        // Export intentionally forgets parity. Importing this payload cannot
        // restore the actual executable element's additional guarantee.
        WordObservation::unsigned_range(self.lower, self.upper).unwrap()
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        if let Some(word) = observation.singleton() {
            return if self.contains(word) {
                Ok(Self {
                    lower: word,
                    upper: word,
                    odd: self.odd,
                })
            } else {
                Err(FactError::Contradiction {
                    subject: Symbol::THIS,
                })
            };
        }
        let (lower, upper) = observation.interval().unsigned_bounds();
        let lower = self.lower.max(lower);
        let upper = self.upper.min(upper);
        if lower > upper || (self.odd && (lower | U256::ONE) > upper) {
            return Err(FactError::Contradiction {
                subject: Symbol::THIS,
            });
        }
        Ok(Self {
            lower,
            upper,
            odd: self.odd,
        })
    }
    fn import_binding(&self, observation: &WordObservation) -> Self {
        BINDING_IMPORTS.update(|count| *count += 1);
        self.constrain(observation).unwrap_or_else(|_| self.clone())
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && (!self.odd || word.bit(0))
    }
    fn singleton(&self) -> Option<U256> {
        SINGLETON_QUERIES.update(|count| *count += 1);
        (self.lower == self.upper).then_some(self.lower)
    }
    fn is_top(&self) -> bool {
        self.lower == U256::ZERO && self.upper == U256::MAX && !self.odd
    }
    fn work_size(&self) -> usize {
        32
    }
}

// Deliberately neither Clone nor Copy: all recursive paths must retain original
// D behind one cheap borrowed facade, including relations-off Reader views.
#[derive(Debug)]
struct Words {
    name: String,
    applies: Cell<usize>,
    joins: Cell<usize>,
}
impl Words {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            applies: Cell::new(0),
            joins: Cell::new(0),
        }
    }
}
impl EvmNumericAlgebra for Words {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        Domain::default().spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        self.joins.set(self.joins.get() + 1);
        Word {
            lower: left.lower.min(right.lower),
            upper: left.upper.max(right.upper),
            odd: left.odd && right.odd,
        }
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, op: u8, args: &[Word]) -> NumericReduction<Word> {
        self.applies.set(self.applies.get() + 1);
        let exact = args.iter().map(Word::singleton).collect::<Option<Vec<_>>>();
        let value = if let Some(words) = exact {
            let word = match (op, words.as_slice()) {
                (opcode::ADD, [a, b]) => a.overflowing_add(*b).0,
                (opcode::SUB, [a, b]) => a.overflowing_sub(*b).0,
                (opcode::MUL, [a, b]) => a.overflowing_mul(*b).0,
                (opcode::AND, [a, b]) => *a & *b,
                (opcode::OR, [a, b]) => *a | *b,
                (opcode::LT, [a, b]) => U256::from(a < b),
                (opcode::GT, [a, b]) => U256::from(a > b),
                (opcode::EQ, [a, b]) => U256::from(a == b),
                (opcode::ISZERO, [a]) => U256::from(*a == U256::ZERO),
                (opcode::SHL, [shift, word]) => usize::try_from(*shift)
                    .ok()
                    .filter(|shift| *shift < 256)
                    .map_or(U256::ZERO, |shift| *word << shift),
                (opcode::BYTE, [index, word]) => usize::try_from(*index)
                    .ok()
                    .filter(|index| *index < 32)
                    .map_or(U256::ZERO, |index| {
                        (*word >> ((31 - index) * 8)) & U256::from(255)
                    }),
                _ => return NumericReduction::stable(Word::top()),
            };
            Word::constant(word)
        } else {
            match (op, args) {
                (opcode::AND, [value, mask])
                    if mask.singleton() == Some(U256::from(255))
                        && value.upper <= U256::from(255) =>
                {
                    value.clone()
                }
                (opcode::BYTE, [index, value]) if value.upper <= U256::from(255) => {
                    match index.singleton() {
                        Some(index) if index == U256::from(31) => value.clone(),
                        Some(_) => Word::constant(U256::ZERO),
                        None => Word::unknown_byte(),
                    }
                }
                (opcode::SHL, [shift, value]) if shift.singleton() == Some(U256::ZERO) => {
                    value.clone()
                }
                (opcode::OR, [zero, value]) if zero.singleton() == Some(U256::ZERO) => {
                    value.clone()
                }
                (opcode::OR, [value, zero]) if zero.singleton() == Some(U256::ZERO) => {
                    value.clone()
                }
                _ => Word::top(),
            }
        };
        NumericReduction::stable(value)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Word>) -> usize {
        args.fold(1024usize, |work, value| {
            work.saturating_add(value.work_size())
        })
    }
    fn projection_work(&self, _value: &Word) -> usize {
        64
    }
    fn refinement_work(&self, _value: &Word, _facts: usize) -> usize {
        1024
    }
}

fn literal(word: u64) -> EvmValue<Word> {
    EvmValue::constant(U256::from(word))
}
fn odd() -> EvmValue<Word> {
    EvmValue::from_numeric(Word::odd_byte())
}
fn assert_odd(value: &EvmValue<Word>) {
    assert!(value.numeric().odd, "custom guarantee lost: {value:?}");
    assert!(!value.contains(U256::from(2)));
    assert!(value.contains(U256::from(3)));
}

#[test]
fn custom_word_survives_memory_history_and_summary_instantiation() {
    let numeric = Words::new("memory");
    let policy = ValueAlgebra::new(numeric);
    let domain = policy.borrowed();
    assert_eq!(literal(0).constants(), None);
    assert_eq!(literal(0).singleton(), Some(U256::ZERO));
    assert_ne!(
        Word::from_observation(&odd().numeric().observation()),
        *odd().numeric()
    );

    let mut memory = EvmByteArray::memory();
    memory
        .write_word_with(&literal(0), &odd(), 4096, domain)
        .unwrap();
    assert_odd(&memory.read_word_with(&literal(0), domain));
    let original = memory.clone();
    memory
        .write_byte_with(&literal(31), &literal(2), 4096, domain)
        .unwrap();
    assert_eq!(
        memory.read_word_with(&literal(0), domain).singleton(),
        Some(U256::from(2))
    );
    assert_odd(&original.read_word_with(&literal(0), domain));
    let summary = original.summary_projection_with(domain).unwrap();
    let first = summary.instantiate_with(domain).unwrap();
    let second = summary.instantiate_with(domain).unwrap();
    assert_odd(&first.read_word_with(&literal(0), domain));
    assert_odd(&second.read_word_with(&literal(0), domain));
    assert_ne!(first.state_root(), second.state_root());
    assert_eq!(summary, first.summary_projection_with(domain).unwrap());
    assert!(policy.numeric_algebra().applies.get() > 0);
}

#[test]
fn full_hull_custom_word_keeps_identity_without_complete_standard_views() {
    let algebra = ValueAlgebra::new(Words::new("whole word"));
    let domain = algebra.borrowed();
    let original = EvmValue::from_numeric(Word {
        lower: U256::ZERO,
        upper: U256::MAX,
        odd: true,
    });
    assert!(original.numeric().observation().is_top());
    assert!(original.interval().is_top());
    assert_eq!(original.constants(), None);
    assert!(!original.numeric().is_top());
    let mut memory = EvmByteArray::memory();
    memory
        .write_word_with(&literal(0), &original, 4096, domain)
        .unwrap();
    assert_odd(&memory.read_word_with(&literal(0), domain));
    let summary = memory.summary_projection_with(domain).unwrap();
    let first = summary.instantiate_with(domain).unwrap();
    let second = summary.instantiate_with(domain).unwrap();
    assert_odd(&first.read_word_with(&literal(0), domain));
    assert_odd(&second.read_word_with(&literal(0), domain));
    assert_ne!(first.state_root(), second.state_root());
    assert_eq!(summary, first.summary_projection_with(domain).unwrap());
    assert!(summary.work_size() >= first.work_size());
    let combined = first
        .join_guarded_with(
            &second,
            domain,
            ExprId::fresh().unwrap(),
            super::limits(domain),
        )
        .unwrap();
    assert_odd(&combined.read_word_with(&literal(0), domain));
    assert_odd(
        &combined
            .summary_projection_with(domain)
            .unwrap()
            .instantiate_with(domain)
            .unwrap()
            .read_word_with(&literal(0), domain),
    );
    let joined = summary.join_with(&summary, domain, false);
    assert_eq!(joined, summary);
    assert_odd(
        &joined
            .instantiate_with(domain)
            .unwrap()
            .read_word_with(&literal(0), domain),
    );
    let mut unaligned = EvmByteArray::memory();
    unaligned
        .write_word_with(&literal(7), &original, 4096, domain)
        .unwrap();
    assert_odd(
        &unaligned
            .summary_projection_with(domain)
            .unwrap()
            .instantiate_with(domain)
            .unwrap()
            .read_word_with(&literal(7), domain),
    );
    let complete = unaligned
        .slice_with(&literal(7), &literal(32), 4096, domain)
        .unwrap();
    assert_odd(&complete.read_word_with(&literal(0), domain));
    assert_odd(
        &complete
            .summary_projection_with(domain)
            .unwrap()
            .instantiate_with(domain)
            .unwrap()
            .read_word_with(&literal(0), domain),
    );

    // A whole-word predicate cannot survive a real partial overwrite: word 1
    // is allowed before the write, and word 2 is allowed afterwards.
    let mut changed = first.clone();
    changed
        .write_byte_with(&literal(31), &literal(2), 4096, domain)
        .unwrap();
    assert!(
        changed
            .read_word_with(&literal(0), domain)
            .contains(U256::from(2))
    );
    let mut aliased = first.clone();
    let at = ExprId::fresh().unwrap();
    aliased.root = aliased
        .root
        .write_byte(
            at.clone(),
            ExprId::constant(U256::from(2)),
            super::limits(domain).into(),
        )
        .unwrap();
    aliased.operands.insert(
        at,
        EvmValue::from_numeric(Word::bounds(U256::ZERO, U256::from(31))),
    );
    assert!(
        aliased
            .read_word_with(&literal(0), domain)
            .contains(U256::from(2))
    );
    assert!(
        aliased
            .summary_projection_with(domain)
            .unwrap()
            .instantiate_with(domain)
            .unwrap()
            .read_word_with(&literal(0), domain)
            .contains(U256::from(2))
    );
    let changed = changed
        .summary_projection_with(domain)
        .unwrap()
        .instantiate_with(domain)
        .unwrap();
    assert!(
        changed
            .read_word_with(&literal(0), domain)
            .contains(U256::from(2))
    );
    // Truncating the low byte of allowed word 1, then padding, can yield zero.
    let padded = first
        .slice_with(&literal(0), &literal(31), 4096, domain)
        .unwrap();
    assert!(
        padded
            .read_word_with(&literal(0), domain)
            .contains(U256::ZERO)
    );
    assert!(
        padded
            .summary_projection_with(domain)
            .unwrap()
            .instantiate_with(domain)
            .unwrap()
            .read_word_with(&literal(0), domain)
            .contains(U256::ZERO)
    );
}

#[test]
fn additional_nonwrapping_singleton_proof_is_paid_before_the_custom_query() {
    let _queries = SINGLETON_QUERIES.scope();
    let algebra = ValueAlgebra::new(Words::new("word bound payment"));
    let domain = algebra.borrowed();
    let at = ExprId::fresh().unwrap();
    let value = literal(0);
    let fee = value.singleton_work();
    let facts = BTreeMap::from([(at.clone(), value)]);
    let conditional = conditional::ConditionalFacts::default();
    for (funded, allowance) in [(false, fee - 1), (true, fee)] {
        SINGLETON_QUERIES.replace(0);
        let mut budget = WorkBudget::new(allowance);
        let payer = conditional::BindingBudget::new(&mut budget);
        let context = conditional::Context::prepaid(&facts, Some(&conditional), domain, &payer);
        assert_eq!(
            super::reads::nonwrapping_word(&at, &context, domain),
            funded
        );
        assert_eq!(context.work_exhausted(), !funded);
        assert_eq!(SINGLETON_QUERIES.replace(0), usize::from(funded));
    }
}

#[test]
fn tightened_custom_source_keeps_extra_guarantees_beyond_its_binding_observation() {
    use crate::domain::symbolic::{ByteStateId, ExprLimits, SourceId, StateBinding};
    let policy = ValueAlgebra::new(Words::new("tightened source"));
    let domain = policy.borrowed();
    let expression = ExprId::fresh().unwrap();
    let baseline = odd().numeric().observation();
    let narrowed = odd()
        .numeric()
        .constrain(&WordObservation::unsigned_range(U256::from(9), U256::from(255)).unwrap())
        .unwrap();
    assert_ne!(baseline, narrowed.observation());
    let mut input = EvmByteArray::unknown();
    input.length = literal(1);
    input.root = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        ExprId::constant(U256::ONE),
        vec![StateBinding {
            index: U256::ZERO,
            value: expression.clone(),
            numeric: baseline,
        }],
        ExprLimits::default().into(),
    )
    .unwrap();
    input.operands.insert(
        expression.clone(),
        EvmValue::from_numeric(narrowed).with_expression(expression),
    );
    let read = input.byte_at_with(0, domain);
    assert!(
        read.numeric().odd,
        "binding import erased the custom guarantee"
    );
    assert!(!read.contains(U256::from(3)));
    assert!(read.contains(U256::from(9)));
    let summary = input.summary_projection_with(domain).unwrap();
    let replay = summary.instantiate_with(domain).unwrap();
    let read = replay.byte_at_with(0, domain);
    assert!(read.numeric().odd);
    assert!(!read.contains(U256::from(3)));
    assert!(read.contains(U256::from(9)));
}

#[test]
fn binding_import_is_prepaid_on_prepared_and_unprepared_reader_paths() {
    use crate::domain::symbolic::{ByteStateId, ExprLimits, SourceId, StateBinding};
    let _imports = BINDING_IMPORTS.scope();
    let algebra = ValueAlgebra::new(Words::new("binding payment"));
    let domain = algebra.borrowed();
    let expression = ExprId::fresh().unwrap();
    let observation = odd().numeric().observation();
    let fee = odd().numeric().import_binding_work(&observation);
    assert!(fee > 0);
    let facts = BTreeMap::from([(expression.clone(), odd())]);
    let conditional = conditional::ConditionalFacts::default();
    let root = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        ExprId::constant(U256::ONE),
        vec![StateBinding {
            index: U256::ZERO,
            value: expression,
            numeric: observation,
        }],
        ExprLimits::default().into(),
    )
    .unwrap();
    let preparation = conditional::preparation(
        root.work_size(),
        &literal(0),
        facts
            .iter()
            .map(|(key, value)| key.work_size() + value.work_size())
            .sum(),
        facts.len(),
        &conditional,
        domain,
    );
    let read = |prepared: bool, budget: &mut WorkBudget| {
        if prepared {
            let context =
                conditional::Context::budgeted(&facts, &conditional, domain, &preparation, budget)
                    .unwrap()
                    .unwrap();
            let value = super::numeric::Reader::with_context(context.clone(), domain)
                .read(&root, &ExprId::constant(U256::ZERO));
            (value, context.work_exhausted(), context.demand_payments())
        } else {
            let payer = conditional::BindingBudget::new(budget);
            let context = conditional::Context::prepaid(&facts, Some(&conditional), domain, &payer);
            let value = super::numeric::Reader::with_context(context.clone(), domain)
                .read(&root, &ExprId::constant(U256::ZERO));
            (value, context.work_exhausted(), Vec::new())
        }
    };
    for prepared in [false, true] {
        BINDING_IMPORTS.replace(0);
        let mut pilot = WorkBudget::new(usize::MAX);
        let (value, exhausted, payments) = read(prepared, &mut pilot);
        assert_odd(&value);
        assert!(!exhausted);
        assert_eq!(BINDING_IMPORTS.replace(0), 1);
        let prefix = if prepared {
            let index = payments
                .iter()
                .position(|&(numeric, work)| numeric && work == fee)
                .unwrap();
            payments[..index].iter().map(|&(_, work)| work).sum()
        } else {
            0
        };
        let mut refused = WorkBudget::new(prefix + fee - 1);
        let (value, exhausted, _) = read(prepared, &mut refused);
        assert!(exhausted && refused.exhausted());
        assert!(value.numeric().is_top());
        assert_eq!(BINDING_IMPORTS.replace(0), 0, "unpaid binding hook ran");

        let mut funded = WorkBudget::new(pilot.used());
        let (value, exhausted, _) = read(prepared, &mut funded);
        assert_odd(&value);
        assert!(!exhausted);
        assert_eq!(funded.used(), pilot.used());
        assert_eq!(BINDING_IMPORTS.replace(0), 1);
    }

    // Exercise the public deferred reader as well: with no conditional scopes
    // it uses the prepaid fallback Context, still on the original ledger.
    let mut input = EvmByteArray::unknown();
    input.length = literal(1);
    input.root = root;
    input.operands = facts;
    let frozen = input.clone();
    let mut pilot = WorkBudget::new(usize::MAX);
    let plan = input
        .prepare_word_read_with(&literal(0), domain, &mut pilot)
        .unwrap()
        .unwrap();
    let prefix = pilot.used();
    BINDING_IMPORTS.replace(0);
    assert!(plan.read_word_budgeted(&mut pilot).unwrap().is_some());
    assert_eq!(pilot.used() - prefix, fee);
    assert_eq!(BINDING_IMPORTS.replace(0), 1);
    let mut refused = WorkBudget::new(prefix + fee - 1);
    let plan = input
        .prepare_word_read_with(&literal(0), domain, &mut refused)
        .unwrap()
        .unwrap();
    assert!(plan.read_word_budgeted(&mut refused).unwrap().is_none());
    assert!(refused.exhausted());
    assert_eq!(BINDING_IMPORTS.replace(0), 0);
    assert_eq!(input, frozen);
}

#[test]
fn native_world_byte_and_store_exports_keep_the_actual_custom_element() {
    fn contains_odd(json: &serde_json::Value) -> bool {
        match json {
            serde_json::Value::Object(fields) => {
                fields.get("odd").and_then(serde_json::Value::as_bool) == Some(true)
                    || fields.values().any(contains_odd)
            }
            serde_json::Value::Array(values) => values.iter().any(contains_odd),
            _ => false,
        }
    }

    let algebra = ValueAlgebra::new(Words::new("native carrier"));
    let domain = algebra.borrowed();
    let owner = Address::repeat_byte(0x33);
    let mut world = EvmWorld::new(Fork::Osaka, "native carrier");
    let mut account = EvmAccount::empty();
    account.balance = odd();
    account.storage.insert(U256::from(9), odd());
    world.insert(owner, account).unwrap();
    let store = EvmStore::new_checked_with(&world, domain).unwrap();
    let mut memory = EvmByteArray::memory();
    memory
        .write_byte_with(&literal(0), &odd(), 4096, domain)
        .unwrap();

    // The interner's WordObservation intentionally has no odd field. These
    // exports must also serialize their real retained executable N.
    for json in [
        serde_json::to_value(&world).unwrap(),
        serde_json::to_value(&store).unwrap(),
        serde_json::to_value(&memory).unwrap(),
    ] {
        assert!(contains_odd(&json), "native export erased custom N: {json}");
    }
}

#[test]
fn custom_store_checkpoints_and_summaries_keep_values_effects_and_owners() {
    let policy = ValueAlgebra::new(Words::new("store"));
    let domain = policy.borrowed();
    let address = Address::repeat_byte(0x11);
    let other = Address::repeat_byte(0x22);
    let mut world = EvmWorld::new(Fork::Osaka, "custom carrier");
    let mut account = EvmAccount::empty();
    account.balance = odd();
    account.storage.insert(U256::from(7), odd());
    world.insert(address, account).unwrap();
    world.insert(other, EvmAccount::empty()).unwrap();
    let mut store = EvmStore::new_checked_with(&world, domain).unwrap();
    assert_odd(&store.read_with(address, &literal(7), domain));
    store.write_with(address, &literal(9), &odd(), domain);
    store.write_transient_with(address, &literal(10), &odd(), domain);
    store.write_nonce(address, odd());
    let log = LogKey {
        address,
        code_address: other,
        code_hash: B256::ZERO,
        source_kind: LogSourceKind::Runtime,
        pc: 1,
    };
    let mut data = EvmByteArray::memory();
    data.write_byte_with(&literal(0), &odd(), 4096, domain)
        .unwrap();
    store
        .emit_log_with(
            log.clone(),
            EvmLog {
                topics: vec![odd()],
                data,
            },
            domain,
        )
        .unwrap();
    let checkpoint = store.snapshot();
    store.havoc_all();
    store.restore(checkpoint);
    assert_odd(&store.read_with(address, &literal(9), domain));
    assert_odd(&store.read_transient_with(address, &literal(10), domain));
    assert_eq!(
        store.read_with(other, &literal(9), domain).singleton(),
        Some(U256::ZERO)
    );
    let summary = store.summary_projection_with(domain).unwrap();
    let fresh = summary.instantiate_with(domain).unwrap();
    assert_odd(&fresh.read_with(address, &literal(7), domain));
    assert_odd(&fresh.read_with(address, &literal(9), domain));
    assert_odd(&fresh.read_transient_with(address, &literal(10), domain));
    assert_odd(&fresh.read_balance(address));
    assert_odd(&fresh.nonce(address));
    let event = fresh.possible_logs().get(&log).unwrap();
    assert_odd(&event.topics[0]);
    assert_odd(&event.data.byte_at_with(0, domain));
    assert_eq!(summary, fresh.summary_projection_with(domain).unwrap());
}

#[test]
fn independently_instantiated_stores_keep_native_facts_under_their_shared_guard() {
    let algebra = ValueAlgebra::new(Words::new("scoped stores"));
    let domain = algebra.borrowed();
    let owner = Address::repeat_byte(0x51);
    let other = Address::repeat_byte(0x52);
    let original = EvmValue::from_numeric(Word {
        lower: U256::ZERO,
        upper: U256::MAX,
        odd: true,
    });
    let mut world = EvmWorld::new(Fork::Osaka, "scoped stores");
    let mut account = EvmAccount::empty();
    account.storage.insert(U256::ZERO, original.clone());
    world.insert(owner, account).unwrap();
    world.insert(other, EvmAccount::empty()).unwrap();
    let mut store = EvmStore::new_checked_with(&world, domain).unwrap();
    store.write_transient_with(owner, &literal(7), &original, domain);
    let summary = store.summary_projection_with(domain).unwrap();
    let left = summary.instantiate_with(domain).unwrap();
    let right = summary.instantiate_with(domain).unwrap();
    let selector = ExprId::fresh().unwrap();
    let joined = left
        .join_guarded_with(&right, domain, selector.clone(), super::limits(domain))
        .unwrap();
    for transient in [false, true] {
        for (key, _) in left.symbolic_facts(transient) {
            if !right
                .symbolic_facts(transient)
                .any(|(other, _)| other == key)
            {
                assert!(
                    !joined
                        .symbolic_facts(transient)
                        .any(|(other, _)| other == key),
                    "one-sided native facts became unconditional"
                );
            }
        }
    }
    assert_odd(&joined.read_with(owner, &literal(0), domain));
    assert_odd(&joined.read_transient_with(owner, &literal(7), domain));
    assert_eq!(
        joined.read_with(other, &literal(0), domain).singleton(),
        Some(U256::ZERO)
    );
    let replay = joined
        .summary_projection_with(domain)
        .unwrap()
        .instantiate_with(domain)
        .unwrap();
    assert_odd(&replay.read_with(owner, &literal(0), domain));
    assert_odd(&replay.read_transient_with(owner, &literal(7), domain));

    let mut renamed = joined.clone();
    let names = renamed
        .fresh_leaves()
        .into_iter()
        .map(|old| (old, ExprId::fresh().unwrap()))
        .collect();
    renamed
        .rename_fresh_with(&names, super::limits(domain), domain)
        .unwrap();
    assert_odd(&renamed.read_with(owner, &literal(0), domain));
    assert_odd(&renamed.read_transient_with(owner, &literal(7), domain));
    let saved = renamed.snapshot();
    renamed.write_with(owner, &literal(0), &literal(2), domain);
    renamed.restore(saved);
    assert_odd(&renamed.read_with(owner, &literal(0), domain));

    let mut broad = right;
    broad.write_with(owner, &literal(0), &EvmValue::top(), domain);
    broad.write_transient_with(owner, &literal(7), &literal(2), domain);
    let selector = ExprId::fresh().unwrap();
    let mixed = left
        .join_guarded_with(&broad, domain, selector.clone(), super::limits(domain))
        .unwrap();
    assert!(
        mixed
            .read_with(owner, &literal(0), domain)
            .contains(U256::from(2))
    );
    assert!(
        mixed
            .read_transient_with(owner, &literal(7), domain)
            .contains(U256::from(2))
    );
    for nonzero in [true, false] {
        let mut selected = mixed.clone();
        assert!(
            selected
                .refine_guards_with(domain, super::limits(domain), &mut |condition| {
                    Ok::<_, ()>((condition == &selector).then_some(nonzero))
                })
                .unwrap()
        );
        if nonzero {
            assert_odd(&selected.read_with(owner, &literal(0), domain));
            assert_odd(&selected.read_transient_with(owner, &literal(7), domain));
        } else {
            assert!(
                selected
                    .read_with(owner, &literal(0), domain)
                    .contains(U256::from(2))
            );
            assert_eq!(
                selected
                    .read_transient_with(owner, &literal(7), domain)
                    .singleton(),
                Some(U256::from(2))
            );
        }
    }
}

#[test]
fn prepared_read_cache_rejects_another_policy_instance_with_the_same_spec() {
    let first = ValueAlgebra::new(Words::new("first"));
    let second = ValueAlgebra::new(Words::new("second"));
    assert_eq!(first.spec(), second.spec());
    assert_ne!(first.numeric_algebra().name, second.numeric_algebra().name);
    let expression = ExprId::fresh().unwrap();
    let facts = BTreeMap::from([(expression.clone(), odd())]);
    let conditional = conditional::ConditionalFacts::default();
    let context =
        conditional::Context::prepared(&facts, &conditional, first.borrowed(), usize::MAX);
    assert_odd(&context.value(&expression, first.borrowed()).unwrap());
    assert_odd(
        &context
            .value(&expression, first.borrowed().without_relations())
            .unwrap(),
    );
    assert!(context.value(&expression, second.borrowed()).is_none());
    assert!(context.cache_refused());
    assert!(!context.work_exhausted());

    let mut memory = EvmByteArray::memory();
    memory
        .write_word_with(&literal(0), &odd(), 4096, first.borrowed())
        .unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    let plan = memory
        .prepare_word_read_with(&literal(0), first.borrowed(), &mut budget)
        .unwrap()
        .unwrap();
    assert_odd(&plan.read_word_budgeted(&mut budget).unwrap().unwrap());
}

pub(super) static HEAVY_CLONES: crate::test_instrumentation::Counter<usize> =
    crate::test_instrumentation::Counter::new(0);
static CANCEL_HEAVY_CLONE: crate::test_instrumentation::Counter<bool> =
    crate::test_instrumentation::Counter::new(false);

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Heavy {
    word: Word,
    payload: Vec<u8>,
}
impl Clone for Heavy {
    fn clone(&self) -> Self {
        HEAVY_CLONES.update(|count| *count += 1);
        let copy = Self {
            word: self.word.clone(),
            payload: self.payload.clone(),
        };
        // Only an explicitly armed test can interrupt a real retained payload
        // copy. Keep observed work outside the scoped counter's lock.
        let mut cancel = false;
        if !self.payload.is_empty() {
            CANCEL_HEAVY_CLONE.update(|armed| cancel = std::mem::take(armed));
        }
        if cancel {
            analysis_control::current_cancellation().cancel();
        }
        copy
    }
}
impl fmt::Display for Heavy {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.word, out)
    }
}
impl WordElement for Heavy {
    fn top() -> Self {
        Self {
            word: Word::top(),
            payload: Vec::new(),
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            word: Word::constant(word),
            payload: Vec::new(),
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self {
            word: Word::from_observation(facts),
            payload: Vec::new(),
        }
    }
    fn observation(&self) -> WordObservation {
        self.word.observation()
    }
    fn singleton(&self) -> Option<U256> {
        self.word.singleton()
    }
    fn contains(&self, word: U256) -> bool {
        self.word.contains(word)
    }
    fn is_top(&self) -> bool {
        self.word.is_top()
    }
    fn work_size(&self) -> usize {
        self.payload.len().saturating_add(self.word.work_size())
    }
}
pub(super) fn heavy_value(bytes: usize) -> EvmValue<Heavy> {
    EvmValue::from_numeric(Heavy {
        word: Word::constant(U256::ZERO),
        payload: vec![7; bytes],
    })
}

#[derive(Debug, Default)]
pub(super) struct HeavyWords {
    pub(super) packs: Cell<usize>,
}
impl EvmNumericAlgebra for HeavyWords {
    type Element = Heavy;
    fn spec(&self) -> DomainSpec {
        Domain::default().spec()
    }
    fn project_numeric(&self, value: &Heavy) -> Heavy {
        value.clone()
    }
    fn join_numeric(&self, _left: &Heavy, _right: &Heavy) -> Heavy {
        Heavy::top()
    }
    fn widen_numeric(&self, _old: &Heavy, _next: &Heavy) -> Heavy {
        Heavy::top()
    }
    fn widen_procedure_numeric(&self, _old: &Heavy, _next: &Heavy) -> Heavy {
        Heavy::top()
    }
    fn reduce_numeric(&self, value: &Heavy) -> NumericReduction<Heavy> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, _op: u8, _args: &[Heavy]) -> NumericReduction<Heavy> {
        NumericReduction::stable(Heavy::top())
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Heavy>) -> usize {
        args.fold(1024usize, |work, value| {
            work.saturating_add(value.work_size())
        })
    }
    fn projection_work(&self, value: &Heavy) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &Heavy, _facts: usize) -> usize {
        value.work_size()
    }
    fn pack_word_numeric(&self, _bytes: &[&Heavy; 32]) -> Option<Heavy> {
        self.packs.set(self.packs.get() + 1);
        None
    }
}

#[test]
fn word_plan_reserves_the_heavy_offset_snapshot_before_cloning_it() {
    let _scope = HEAVY_CLONES.scope();
    let algebra = ValueAlgebra::new(HeavyWords::default());
    let domain = algebra.borrowed();
    let memory = EvmByteArray::memory();
    let offset = heavy_value(64 * 1024);
    let work = 4096 * 8 + memory.word_numeric_work_with(&offset, domain);
    let mut budget = WorkBudget::new(work);
    HEAVY_CLONES.replace(0);
    assert!(
        memory
            .prepare_word_read_with(&offset, domain, &mut budget)
            .unwrap()
            .is_none()
    );
    assert!(budget.exhausted());
    assert_eq!(
        HEAVY_CLONES.replace(0),
        0,
        "unpaid snapshot cloned custom N"
    );
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(
        memory
            .prepare_word_read_with(&offset, domain, &mut budget)
            .unwrap()
            .is_some()
    );
    assert!(
        HEAVY_CLONES.replace(0) > 0,
        "funded snapshot did not exercise custom Clone"
    );
}

#[test]
fn retained_word_lookup_pays_before_copying_the_actual_custom_payload() {
    use crate::domain::symbolic::{ByteStateId, ByteView, SourceId};
    let _scope = HEAVY_CLONES.scope();
    let algebra = ValueAlgebra::new(HeavyWords::default());
    let domain = algebra.borrowed();
    let expression = ExprId::word_read(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(32))),
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(32)),
        ),
        ExprId::constant(U256::ZERO),
        super::limits(domain),
    )
    .unwrap();
    let value = EvmValue::from_numeric(Heavy {
        word: Word {
            lower: U256::ZERO,
            upper: U256::MAX,
            odd: true,
        },
        payload: vec![7; 64 * 1024],
    });
    let fee = value.clone_work();
    let facts = BTreeMap::from([(expression.clone(), value)]);
    let conditional = conditional::ConditionalFacts::default();
    for (funded, allowance) in [(false, fee - 1), (true, fee)] {
        let mut budget = WorkBudget::new(allowance);
        let payer = conditional::BindingBudget::new(&mut budget);
        let context = conditional::Context::prepaid(&facts, Some(&conditional), domain, &payer);
        HEAVY_CLONES.replace(0);
        let value = context.retained_word(&expression, domain);
        assert_eq!(value.is_some(), funded);
        assert_eq!(context.work_exhausted(), !funded);
        assert_eq!(HEAVY_CLONES.replace(0), usize::from(funded));
        if let Some(value) = value {
            assert!(value.numeric().word.odd);
        }
    }
}
