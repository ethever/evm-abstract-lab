//! Equal intervals do not establish content equality or iteration invariance.
use super::*;
use crate::{Ir, U256};

fn source() -> Source<'static, impl FnMut(usize) -> bool> {
    static IR: std::sync::OnceLock<Ir> = std::sync::OnceLock::new();
    Source::new(IR.get_or_init(Ir::default), |_| true).unwrap()
}

fn state(memory: u64, value: u64) -> State {
    State {
        memory: Some(EntityId(memory)),
        cells: BTreeMap::from([(
            Expr::constant(U256::from(64)),
            Expr::constant(U256::from(value)),
        )]),
        ..State::default()
    }
}

#[test]
fn fixed_cell_join_retains_a_common_interval_with_its_own_content_identity() {
    let source = source();
    let joined = source.merge(state(10, 128), &state(10, 192), None);
    let address = Expr::constant(U256::from(64));
    let content = joined.cells.get(&address).unwrap();
    assert_eq!(
        content,
        &Expr::atom(Atom::Content(EntityId(10), U256::from(64)))
    );
    assert_eq!(
        joined.range(content),
        Range {
            low: U256::from(128),
            high: U256::from(192)
        }
    );
    assert!(content.exact().is_none());

    let widened = source.merge(joined, &state(10, 256), None);
    let content = widened.cells.get(&address).unwrap();
    assert_eq!(
        widened.range(content),
        Range {
            low: U256::from(128),
            high: U256::from(256)
        }
    );
}

#[test]
fn memory_versions_and_addresses_never_share_identity_from_equal_ranges() {
    let source = source();
    let first = source.merge(state(10, 128), &state(10, 192), None);
    let next = source.merge(state(11, 128), &state(11, 192), None);
    let address = Expr::constant(U256::from(64));
    assert_ne!(first.cells[&address], next.cells[&address]);
    assert!(
        first.cells[&address]
            .sub(&next.cells[&address])
            .exact()
            .is_none()
    );

    let mut a = state(10, 128);
    let mut b = state(10, 192);
    let other = Expr::constant(U256::from(96));
    a.cells
        .insert(other.clone(), Expr::constant(U256::from(128)));
    b.cells
        .insert(other.clone(), Expr::constant(U256::from(192)));
    let joined = source.merge(a, &b, None);
    assert_ne!(joined.cells[&address], joined.cells[&other]);
    assert_eq!(
        joined.range(&joined.cells[&address]),
        joined.range(&joined.cells[&other])
    );
}

#[test]
fn missing_cells_full_ranges_and_mismatched_versions_remain_unknown() {
    let source = source();
    let address = Expr::constant(U256::from(64));
    let mut missing = state(10, 192);
    missing.cells.clear();
    assert!(
        source
            .merge(state(10, 128), &missing, None)
            .cells
            .is_empty()
    );
    assert!(
        source
            .merge(state(10, 128), &state(11, 192), None)
            .cells
            .is_empty()
    );
    let mut unconstrained = state(10, 192);
    unconstrained
        .cells
        .insert(address, Expr::atom(Atom::Source(EntityId(50))));
    assert!(
        source
            .merge(state(10, 128), &unconstrained, None)
            .cells
            .is_empty()
    );
}

#[test]
fn iteration_local_joins_cannot_make_changing_contents_loop_invariant() {
    let source = source();
    let address = Expr::constant(U256::from(64));
    let changed = source.merge(state(10, 128), &state(10, 192), Some(EntityId(20)));
    assert!(!changed.cells.contains_key(&address));
    assert!(!changed.atoms.keys().any(|a| matches!(a, Atom::Content(..))));
    let unchanged = source.merge(state(10, 128), &state(10, 128), Some(EntityId(20)));
    assert_eq!(unchanged.cells[&address].exact(), Some(U256::from(128)));
}
