use super::{FactError, NumericValue, WordElement, WordObservation};
use crate::domain::{EvmValue, facts::Symbol};
use alloy_primitives::U256;
use serde::Serialize;
use std::{cell::Cell, collections::BTreeSet, fmt, rc::Rc};

/// A range with a genuine extra predicate deliberately omitted from exports.
#[derive(Debug, Serialize)]
struct ExcludingWord {
    lower: U256,
    upper: U256,
    excluded: BTreeSet<U256>,
    #[serde(skip)]
    payload: Vec<u8>,
    #[serde(skip)]
    copies: Rc<Cell<usize>>,
    #[serde(skip)]
    imports: Rc<Cell<usize>>,
    #[serde(skip)]
    refusal: Option<FactError>,
}
impl Clone for ExcludingWord {
    fn clone(&self) -> Self {
        self.copies.set(self.copies.get() + 1);
        Self {
            lower: self.lower,
            upper: self.upper,
            excluded: self.excluded.clone(),
            payload: self.payload.clone(),
            copies: Rc::clone(&self.copies),
            imports: Rc::clone(&self.imports),
            refusal: self.refusal.clone(),
        }
    }
}
impl PartialEq for ExcludingWord {
    fn eq(&self, other: &Self) -> bool {
        (self.lower, self.upper, &self.excluded) == (other.lower, other.upper, &other.excluded)
    }
}
impl Eq for ExcludingWord {}
impl fmt::Display for ExcludingWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}..={} excluding {:?}",
            self.lower, self.upper, self.excluded
        )
    }
}
impl WordElement for ExcludingWord {
    fn top() -> Self {
        Self {
            lower: U256::ZERO,
            upper: U256::MAX,
            excluded: BTreeSet::new(),
            payload: Vec::new(),
            copies: Rc::new(Cell::new(0)),
            imports: Rc::new(Cell::new(0)),
            refusal: None,
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            lower: value,
            upper: value,
            ..Self::top()
        }
    }
    fn from_observation(observation: &WordObservation) -> Self {
        let (lower, upper) = observation.interval().unsigned_bounds();
        Self {
            lower,
            upper,
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        WordObservation::unsigned_range(self.lower, self.upper).unwrap()
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        self.imports.set(self.imports.get() + 1);
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let (lower, upper) = observation.interval().unsigned_bounds();
        let lower = self.lower.max(lower);
        let upper = self.upper.min(upper);
        if lower > upper {
            return Err(FactError::Contradiction {
                subject: Symbol::THIS,
            });
        }
        Ok(Self {
            lower,
            upper,
            ..self.clone()
        })
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && !self.excluded.contains(&word)
    }
    fn singleton(&self) -> Option<U256> {
        (self.lower == self.upper && self.contains(self.lower)).then_some(self.lower)
    }
    fn is_top(&self) -> bool {
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_empty()
    }
    fn work_size(&self) -> usize {
        16usize
            .saturating_add(self.excluded.len())
            .saturating_add(self.payload.len())
    }
}
fn narrowed() -> ExcludingWord {
    ExcludingWord {
        lower: U256::from(9),
        upper: U256::from(255),
        excluded: BTreeSet::from([U256::from(42)]),
        ..ExcludingWord::top()
    }
}

#[test]
fn custom_default_views_share_stable_top_values_without_copying_the_carrier() {
    let first = narrowed();
    let second = ExcludingWord::constant(U256::from(9));
    let expected = NumericValue::top();
    for value in [&first, &second] {
        assert_eq!(value.finite_constants(), expected.finite_constants());
        assert_eq!(value.known_bits(), expected.known_bits());
        assert_eq!(value.interval(), expected.interval());
        assert_eq!(value.congruence(), expected.congruence());
        assert!(std::ptr::eq(
            value.finite_constants(),
            first.finite_constants()
        ));
        assert!(std::ptr::eq(value.known_bits(), first.known_bits()));
        assert!(std::ptr::eq(value.interval(), first.interval()));
        assert!(std::ptr::eq(value.congruence(), first.congruence()));
        assert_eq!((value.copies.get(), value.imports.get()), (0, 0));
    }
    // Conservative default views do not replace the custom element's knowledge.
    assert!(!first.contains(U256::from(42)));
    assert_eq!(second.singleton(), Some(U256::from(9)));
}

#[test]
fn retained_binding_import_keeps_path_bounds_and_unexported_predicates() {
    let observation = WordObservation::unsigned_range(U256::ZERO, U256::from(255)).unwrap();
    let value = narrowed();
    assert_ne!(value.observation(), observation);
    // Rebuilding from the old observation demonstrably loses both guarantees.
    let lossy = ExcludingWord::from_observation(&observation);
    assert!(lossy.contains(U256::from(3)) && lossy.contains(U256::from(42)));
    let materialized = value.import_binding(&observation);
    assert_eq!(materialized, value);
    assert!(!materialized.contains(U256::from(3)));
    assert!(!materialized.contains(U256::from(42)));
    assert!(materialized.contains(U256::from(9)));
    let tighter = WordObservation::unsigned_range(U256::from(10), U256::from(200)).unwrap();
    let replayed = materialized
        .import_binding(&tighter)
        .import_binding(&observation);
    assert_eq!(
        (replayed.lower, replayed.upper),
        (U256::from(10), U256::from(200))
    );
    assert!(!replayed.contains(U256::from(42)));
}

#[test]
fn optional_binding_refusal_retains_the_existing_element_without_bottom() {
    for refusal in [
        FactError::Capacity {
            max_atoms: 1,
            required: 2,
        },
        FactError::UnsupportedOperation(0xfe),
        FactError::Contradiction {
            subject: Symbol::THIS,
        },
    ] {
        let value = ExcludingWord {
            refusal: Some(refusal),
            ..narrowed()
        };
        let result = value.import_binding(&WordObservation::constant(U256::from(42)));
        assert_eq!(result, value);
        assert!(!result.contains(U256::from(42)));
        assert!(result.contains(U256::from(9)));
    }
}

#[test]
fn binding_quote_does_not_clone_or_run_custom_constraint_work() {
    let value = ExcludingWord {
        payload: vec![7; 4096],
        ..narrowed()
    };
    let observation = WordObservation::unsigned_range(U256::ZERO, U256::from(255)).unwrap();
    let quote = value.import_binding_work(&observation);
    assert!(quote >= value.clone_work());
    assert!(quote >= value.payload.len());
    assert_eq!((value.copies.get(), value.imports.get()), (0, 0));
    let result = value.import_binding(&observation);
    assert_eq!(result, value);
    assert_eq!((value.copies.get(), value.imports.get()), (1, 1));
}

#[test]
fn default_binding_import_restores_raw_authority_without_changing_constrain() {
    let observation = WordObservation::unsigned_range(U256::ZERO, U256::from(255)).unwrap();
    let value = NumericValue::unknown_byte()
        .constrain(&WordObservation::unsigned_range(U256::from(9), U256::from(255)).unwrap())
        .unwrap();
    assert!(!value.contains(U256::from(3)));
    let materialized = value.import_binding(&observation);
    assert_eq!(materialized, NumericValue::from_observation(&observation));
    assert!(materialized.contains(U256::from(3)));
    assert!(materialized.constants().is_none());
    assert_eq!(
        materialized.known_bits(),
        &crate::domain::known_bits::KnownBits::top()
    );
    assert_eq!(value.import_binding_work(&observation), 0);
    assert!(
        !value
            .constrain(&observation)
            .unwrap()
            .contains(U256::from(3))
    );
}

#[test]
fn custom_native_serialization_keeps_exact_exclusions_without_cloning() {
    let value = EvmValue::from_numeric(narrowed());
    let json = serde_json::to_value(&value).unwrap();
    assert_eq!(
        json["numeric"],
        serde_json::json!({
            "lower": "0x9", "upper": "0xff", "excluded": ["0x2a"]
        })
    );
    assert_eq!(
        json["numeric"],
        serde_json::to_value(value.numeric()).unwrap()
    );
    assert_eq!(value.numeric().copies.get(), 0);
    assert!(value.numeric().observation().contains(U256::from(42)));
    assert!(!value.numeric().contains(U256::from(42)));
    let top = serde_json::to_value(EvmValue::<ExcludingWord>::top()).unwrap();
    assert_eq!(
        top["numeric"],
        serde_json::json!({
            "lower": "0x0", "upper": format!("0x{:x}", U256::MAX), "excluded": []
        })
    );
}

#[test]
fn product_native_serialization_keeps_actual_components_for_top_and_constants() {
    let top = serde_json::to_value(EvmValue::<NumericValue>::top()).unwrap();
    assert!(top.is_object());
    assert_eq!(
        top["numeric"],
        serde_json::to_value(NumericValue::top()).unwrap()
    );
    assert_eq!(top["numeric"], serde_json::json!({}));
    let value = EvmValue::<NumericValue>::constant(U256::from(42));
    let json = serde_json::to_value(&value).unwrap();
    assert!(json.is_object());
    assert_eq!(
        json["numeric"],
        serde_json::json!({
                "constants": ["0x2a"],
                "known_bits": value.known_bits(),
                "interval": value.interval(),
                "congruence": value.congruence(),
                "nonzero": true,
        })
    );
    assert_eq!(
        json["numeric"],
        serde_json::to_value(value.numeric()).unwrap()
    );
    assert_eq!(
        json["provenance"],
        serde_json::to_value(value.provenance()).unwrap()
    );
    assert_eq!(json["numeric"]["constants"], serde_json::json!(["0x2a"]));
}
