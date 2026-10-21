//! Opaque metadata cancellation in the real certificate publication path.

use super::*;
use crate::{
    analysis::{Status, analyze_world, machine::fixtures},
    domain::relational::QueryReason,
};
use analysis_control::{Cancellation, with_cancellation};
use std::{cell::Cell, cmp::Ordering, convert::Infallible};

#[derive(Debug)]
pub(super) struct CancelClass<'a> {
    pub id: usize,
    pub clones: &'a Cell<usize>,
    pub cancel: Option<Cancellation>,
}

impl Clone for CancelClass<'_> {
    fn clone(&self) -> Self {
        self.clones.set(self.clones.get() + 1);
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        Self {
            id: self.id,
            clones: self.clones,
            cancel: self.cancel.clone(),
        }
    }
}
impl PartialEq for CancelClass<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for CancelClass<'_> {}
impl PartialOrd for CancelClass<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for CancelClass<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}
impl PolicyKey for CancelClass<'_> {
    fn key_work(&self) -> usize {
        128
    }
}

#[test]
fn actual_capture_retains_typed_current_and_captured_clone_cancellation() {
    for cancel_captured in [false, true] {
        let (world, entry) =
            fixtures::fixture(&[(0x101, "5f5f5f5f5f61020061fffff100"), (0x200, "00")]);
        let mut graph = analyze_world(world, entry, fixtures::config()).unwrap();
        assert_eq!(graph.status(), Status::Converged);
        let child = graph
            .states()
            .iter()
            .find(|state| state.entry.call_stack.active_child().is_some())
            .unwrap()
            .id;
        let edges = graph.edges().iter().cloned().collect();
        let diagnostics = graph.diagnostics().iter().cloned().collect();
        let completed = vec![Vec::new(); graph.states().len()];
        let mut setup = WorkBudget::new(100_000_000);
        let runtime = Runtime::<Infallible>::new(&mut setup).unwrap();
        let mut cache = EvmCache::<NumericValue, CancelClass<'_>>::new();
        let input = cache.input(&graph, child, &mut setup).unwrap().unwrap();
        assert!(cache.begin(child, input, &mut setup));
        let before = graph.summary_stats().published;
        let owner = Cancellation::new();
        let current = Cancellation::new();
        let clones = Cell::new(0);
        let target = if cancel_captured {
            owner.clone()
        } else {
            current.clone()
        };
        let classes = (0..graph.states().len())
            .map(|id| CancelClass {
                id,
                clones: &clones,
                cancel: (id == child).then(|| target.clone()),
            })
            .collect::<Vec<_>>();
        let mut budget = with_cancellation(&owner, || WorkBudget::new(100_000_000));
        let published = with_cancellation(&current, || {
            cache.publish_closed(
                &mut graph,
                &edges,
                &runtime,
                &diagnostics,
                &completed,
                &classes,
                &mut budget,
            )
        });
        assert!(!published);
        assert_eq!(
            cache.stop_reason(),
            Some(FrontierReason::Relations(QueryReason::Cancelled))
        );
        assert_eq!(clones.get(), 1);
        assert!(budget.used() > 0);
        assert_eq!(graph.summary_stats().published, before);
        assert_eq!(
            cache.candidates.len(),
            1,
            "the original entry remains an obligation"
        );
        assert!(cache.certificates.is_empty());
    }
}
