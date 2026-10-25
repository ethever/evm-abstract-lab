use super::{DomainSpec, DomainSpecError};
use crate::domain::relational::RelationLimits;
use std::num::NonZeroUsize;

#[test]
fn owned_history_cost_model_is_version_nine() {
    let one = NonZeroUsize::new(1).unwrap();
    for spec in [DomainSpec::default(), DomainSpec::new(one, one, one)] {
        assert_eq!(spec.cost_version(), 9);
        let serialized = serde_json::to_value(spec).unwrap();
        assert_eq!(serialized["cost_version"], 9);
    }
}

#[test]
fn zero_relation_bounds_are_rejected_even_when_disabled() {
    for (max_constraints, max_nodes, max_depth) in [(0, 1, 1), (1, 0, 1), (1, 1, 0)] {
        for enabled in [true, false] {
            let limits = RelationLimits {
                enabled,
                max_constraints,
                max_nodes,
                max_depth,
            };
            assert_eq!(
                DomainSpec::default().with_relations(limits).validate(),
                Err(DomainSpecError::InvalidRelationLimits),
                "{limits:?}",
            );
        }
    }
}

#[test]
fn default_and_minimal_positive_limits_are_valid() {
    assert_eq!(DomainSpec::default().validate(), Ok(()));
    let one = NonZeroUsize::new(1).unwrap();
    for enabled in [true, false] {
        let spec = DomainSpec::new(one, one, one).with_relations(RelationLimits {
            enabled,
            max_constraints: 1,
            max_nodes: 1,
            max_depth: 1,
        });
        assert_eq!(spec.validate(), Ok(()));
    }
}
