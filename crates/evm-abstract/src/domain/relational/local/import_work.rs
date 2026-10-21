//! Reservation for one fresh bounded lattice and one scalar import, without
//! reduction rounds. Exact built-in proofs take priority; other bounded scalar
//! facts use the selected native import hook, with a whole-quote fallback.
//! The caller supplies `EvmValue::from_numeric(known)`; `known` and `learn`
//! keep their independent charges. These are logical source actions, not time.

use super::Fact;
use crate::domain::facts::{BinaryPredicate, Symbol, Term, UnaryPredicate};
use crate::domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement};
use alloy_primitives::U256;

#[cfg(test)]
mod tests;

// Pairs of fact/term tags, subject/constant checks, finite cardinality,
// congruence tags, predicate tags, candidate/binary flags and checked assembly.
const ADMISSION: usize = 2 + 2 + 2 + 2 + 2 + 2 + 2 + 2;
// Two five-field lattice headers plus four empty containers in each snapshot.
const FRESH_LATTICE: usize = 2 * 5 + 2 * 4;
// One map entry, nine ScalarFacts slots, dispatch and at most two slot writes.
const APPLY_UNARY: usize = 1 + 9 + 1 + 2;
// Filter dispatch, bits/congruence/two bounds singleton checks, one nine-slot
// predicate, and its fixed-word mask/bounds/coordinate operations.
const VALIDATE: usize = 1 + 2 + 1 + 2 + 9 + 16;
// Empty edge tests, identity representative, default construction/comparison,
// and retention of the sole scalar. No general equality normalization runs.
const CLEANUP: usize = 2 + 3 + 9 + 9 + 1;
// Scalar and representative maps each have at most one node: allocation,
// entry installation and eventual entry/node disposal.
const MAP_STORAGE: usize = 2 * (1 + 1 + 1);
// Two empty relation traversals, nine atom slots, capacity checks and the five
// lattice fields compared/installed by the atomic insert.
const FINISH_INSERT: usize = 2 + 9 + 2 + 5;
// Five from_numeric fields and five cloned wrapper fields; payload is separate.
const VALUE_WRAPPERS: usize = 2 * 5;
// Scalar lookup, nine scalar components, finite checks and result completion.
const IMPORT_DISPATCH: usize = 1 + 9 + 2 + 1;
// At most two bit imports (IsAddress): two masks, constructor word/field work,
// meet word/field work, output write, plus two mask shifts.
const IMPORT_BITS: usize = 2 * (2 + 2 + 2 + 4 + 2 + 1) + 2;
// segments/normalize word work: emptiness2, sign1, branch2, XOR4,
// clipping6, normalize sign1, extrema8. Also count two extrema initializers,
// four output fields, four vector/iterator headers, at most two segments with
// four endpoint copies/two visits each, and first/last lookup plus pair copies.
// The two temporary vectors each allocate, drop a header and drop <=2 entries.
const NORMALIZE_INTERVAL: usize =
    (2 + 1 + 2 + 4 + 6 + 1 + 8) + 2 + 4 + 4 + 2 * (4 + 1 + 1) + 2 * (1 + 2) + 2 * (1 + 1 + 2);
// Constructor and meet each normalize once; each has four input fields, with
// four additional min/max operations in meet. There are at most two segments.
const IMPORT_INTERVAL: usize = 2 * (4 + NORMALIZE_INTERVAL) + 4;
// At most two fresh one-entry BTree/Arc containers (fact and finite/finite
// intersection): header, node allocation, entry copy, Arc allocation/wrapper,
// entry/node/Arc disposal. Also two meets, singleton intersection iterators and
// comparison, one empty rejection rebuild, and <=6 filter Arc clones/drops.
const SINGLETON_FINITE: usize = 2 * 8 + 2 * 4 + (2 + 2 + 1) + 6 + 6;
// Scalar validation and imported filtering each visit at most one candidate:
// iterator setup/advance4, component slots9 and fixed-word predicate work16.
const SINGLETON_PREDICATES: usize = 2 * (4 + 9 + 16);
// Three singleton summaries and their meets: iterator3 in each; bits exact3
// and meet4; interval coordinate2/fields4/meet4/normalize; congruence exact2
// and Top/Exact meet5. Singleton folds perform no join, gcd or CRT.
const SINGLETON_SUMMARIES: usize = (3 + 3 + 4) + (3 + 2 + 4 + 4 + NORMALIZE_INTERVAL) + (3 + 2 + 5);

// Binary Eq/Ne ordering (comparison/swap), two term classifications, predicate
// dispatch and the two fields of the existing constrain_binary unary adapter.
const CONSTRAIN_BINARY: usize = (1 + 3) + 4 + 1 + 2;
// General normalization adds seven <=1-entry containers beyond the original
// scalar/representative maps: binary, symbols, parents, rebuilt binary,
// numerical, rebuilt scalars, filtered binary. Each pays header, allocation,
// entry copy, entry/node disposal and header disposal.
const BINARY_STORAGE: usize = 7 * 5;
// Scalar-key collection3, binary-term scan5, parent collection3, equality-edge
// scan3, representative lookup/collection7 and identity-star scan2. Eq between
// two symbols is ineligible, so no union or multi-hop parent walk can run.
const BINARY_SYMBOLS: usize = 3 + 5 + 3 + 3 + 7 + 2;
// Clone/remap/order/reinsert one binary15, final filter/clone/collect9, install
// three rebuilt maps/sets3. Their allocations and drops are in BINARY_STORAGE.
const BINARY_REBUILD: usize = 15 + 9 + 3;
// Numerical map: visit/lookup2, clone9, metadata reset2, entry1, default9,
// component meet dispatch9, flags3, origins tag1. Rebuilt scalars: visit1,
// numerical lookup1/clone9, local lookup1/origins1/role1, default9/compare9,
// insert1. Scalar validation is charged separately below.
const BINARY_SCALARS: usize = (2 + 9 + 2 + 1 + 9 + 9 + 3 + 1) + (1 + 1 + 9 + 1 + 1 + 1 + 9 + 9 + 1);
// At most three scalar validations total (including common APPLY_UNARY):
// the extra general validators see only the fact's <=1-element finite side.
const BINARY_VALIDATIONS: usize = 2 * (VALIDATE + 4 + 9 + 16);
// Up to four binary_truth calls: before insertion, pre-normalization relation
// check, normalization filter, final relation check. Each covers term tags/
// equality4, scalar lookup1, singleton dispatch9, contains slots9 and word16,
// then coordinate/predicate2. Lattice congruence is absent for Eq/Ne0.
const BINARY_TRUTH: usize = 4 * (4 + 1 + 9 + 9 + 16 + 2);

const COMMON: usize = ADMISSION
    + FRESH_LATTICE
    + APPLY_UNARY
    + VALIDATE
    + CLEANUP
    + MAP_STORAGE
    + FINISH_INSERT
    + VALUE_WRAPPERS
    + IMPORT_DISPATCH
    + IMPORT_BITS
    + IMPORT_INTERVAL;
const SINGLETON_EXTRA: usize = SINGLETON_FINITE + SINGLETON_PREDICATES + SINGLETON_SUMMARIES;
const BINARY_EXTRA: usize = CONSTRAIN_BINARY
    + BINARY_STORAGE
    + BINARY_SYMBOLS
    + BINARY_REBUILD
    + BINARY_SCALARS
    + BINARY_VALIDATIONS
    + BINARY_TRUTH;

// Ordered binary bounds additionally inspect orientation/strictness, bias one
// word, test an extremum, adjust one strict endpoint and write the two bounds.
const ORDERED_BOUND: usize = 2 + 3 * 4 + 2;

struct Shape {
    singleton: bool,
    binary: bool,
}

pub(super) fn reservation<D: EvmNumericAlgebra>(
    domain: AlgebraRef<'_, D>,
    input: &EvmValue<D::Element>,
    fact: &Fact,
) -> usize {
    if !domain.numeric_algebra().supports_product_scalar_shortcuts() {
        return staged_reservation(domain, input, fact);
    }
    let Some(shape) = eligible(input, fact) else {
        return staged_reservation(domain, input, fact);
    };
    let fixed = COMMON
        + if shape.singleton { SINGLETON_EXTRA } else { 0 }
        + if shape.binary { BINARY_EXTRA } else { 0 };
    fixed.saturating_add(input.work_size())
}

// The selected policy owns native input copying/constraints, including hidden
// carrier state. Only cached finite cardinality and borrowed fact tags/sizes are
// inspected here. Engine::atom/known already pay expression walks and input
// acquisition; learn/meet still pay their independent work after this import.
fn staged_reservation<D: EvmNumericAlgebra>(
    domain: AlgebraRef<'_, D>,
    input: &EvmValue<D::Element>,
    fact: &Fact,
) -> usize {
    let Some((members, structural)) = import_shape(fact) else {
        return domain.refinement_work(input, 1);
    };
    let numeric = input.numeric();
    let finite = numeric
        .finite_constants()
        .cardinality()
        .unwrap_or(0)
        .saturating_add(members);
    domain
        .numeric_algebra()
        .refinement_import_work(numeric, 1, finite, structural)
        .map_or_else(
            || domain.refinement_work(input, 1),
            |work| work.saturating_add(input.work_size().saturating_sub(numeric.work_size())),
        )
}

// One fresh scalar slot and, for symbol/literal binaries, one fixed-size edge.
// A MemberOf bound is its actual cached cardinality, never the configured fact
// capacity. Scalar bounds/masks/congruences may synthesize at most one literal;
// no singleton query, fact evaluation, candidate visit or allocation is needed
// to reserve that possible word. Structural fees cover fact-table actions, not
// another expression traversal or numeric closure.
fn import_shape(fact: &Fact) -> Option<(usize, usize)> {
    match fact {
        Fact::Unary(fact) => {
            let members = match &fact.predicate {
                UnaryPredicate::MemberOf(set) => set.values().len(),
                UnaryPredicate::Exact(_)
                | UnaryPredicate::IsZero
                | UnaryPredicate::KnownBits(_)
                | UnaryPredicate::UnsignedBounds(_)
                | UnaryPredicate::SignedBounds(_)
                | UnaryPredicate::Congruent(_) => 1,
                UnaryPredicate::BitSet(_)
                | UnaryPredicate::BitClear(_)
                | UnaryPredicate::NonZero
                | UnaryPredicate::IsAddress
                | UnaryPredicate::IsCodeAddress => 0,
                // Origin-set metadata needs its own payload/copy proof; it is
                // not a numeric finite set. Keep the existing complete quote.
                UnaryPredicate::PossibleOrigins(_) => return None,
            };
            Some((members, COMMON))
        }
        Fact::Binary(fact) => {
            if !matches!(
                (fact.left, fact.right),
                (Term::Symbol(_), Term::Constant(_)) | (Term::Constant(_), Term::Symbol(_))
            ) {
                // Multiple symbols or two constants use general normalization;
                // the one-scalar structural proof below does not cover them.
                return None;
            }
            let (members, bounds) = match fact.predicate {
                BinaryPredicate::Eq => (1, 0),
                BinaryPredicate::Ne => (0, 0),
                BinaryPredicate::Ult
                | BinaryPredicate::Ule
                | BinaryPredicate::Slt
                | BinaryPredicate::Sle => (1, ORDERED_BOUND),
            };
            Some((members, COMMON + BINARY_EXTRA + bounds))
        }
        // Operation relations have a separate normalization/operand payload;
        // they are not produced by local scalar translation. Do not pretend
        // their complete import work is covered by a one-scalar quote.
        Fact::Relation(_) => None,
    }
}

// Only tags and fixed-size fields: never enumerate candidates to obtain a fee.
fn eligible<N: WordElement>(input: &EvmValue<N>, fact: &Fact) -> Option<Shape> {
    let (exact, binary) = match fact {
        Fact::Unary(fact) if fact.subject == Symbol::THIS => {
            let exact = match &fact.predicate {
                UnaryPredicate::KnownBits(_)
                | UnaryPredicate::BitSet(_)
                | UnaryPredicate::BitClear(_)
                | UnaryPredicate::UnsignedBounds(_)
                | UnaryPredicate::SignedBounds(_)
                | UnaryPredicate::NonZero
                | UnaryPredicate::IsAddress => false,
                UnaryPredicate::Exact(_) | UnaryPredicate::IsZero => true,
                _ => return None,
            };
            (exact, false)
        }
        Fact::Binary(fact) => {
            let (symbol, value) = match (fact.left, fact.right) {
                (Term::Symbol(symbol), Term::Constant(value))
                | (Term::Constant(value), Term::Symbol(symbol)) => (symbol, value),
                _ => return None,
            };
            if symbol != Symbol::THIS {
                return None;
            }
            let exact = match fact.predicate {
                BinaryPredicate::Eq => true,
                BinaryPredicate::Ne if value == U256::ZERO => false,
                _ => return None,
            };
            (exact, true)
        }
        _ => return None,
    };
    let singleton_input = match input.finite_constants().cardinality() {
        None => false,
        Some(1) => true,
        Some(_) => return None,
    };
    let singleton = exact || singleton_input;
    // No modulo predicate or CRT may run along a candidate-producing path.
    if singleton && !input.congruence().is_top() && input.congruence().singleton().is_none() {
        return None;
    }
    Some(Shape { singleton, binary })
}
