//! Borrowed routing into one shared run and one generation-owned client store.
use super::{
    ImmutableInputPolicy, Inputs, ProcessMeet, ProcessRefinements, Refinements, StackMeet,
    global::{
        Continuation, Continuations, Edge, EntryPolicy, Generations, ObservationPolicy, Points,
        Procedures, SetUnion, Shared, Subscriptions, Summaries, SummaryClosure, SummaryClosures,
        SummaryPolicy, Topology,
    },
};
use crate::domain::WordElement;
use analysis_kernel::{Ascending, Channel, Descending, Here, Select, There};
use std::{marker::PhantomData, sync::Arc};

/// Explicit route into the once-per-run client provider list.
pub struct SharedRoute<P>(PhantomData<P>);

/// Process guarantees, separate from Native stack observations and client tails.
pub struct ProcessRefinementRoute;

/// Explicit route of the global point-discovery directory.
pub struct PointsRoute;
/// Explicit route of stable global current-generation selectors.
pub struct GenerationRoute;
/// Explicit route of global open CFG predecessor/successor observations.
pub struct TopologyRoute;
/// Explicit route of native global procedure-output observations.
pub struct SummaryRoute;
/// Explicit route of source-guarded procedure completion certificates.
pub struct SummaryClosureRoute;
/// Explicit route of the open procedure-key directory.
pub struct ProcedureRoute;
/// Explicit route of structural origin-to-callee relationships.
pub struct CalleeRoute;
/// Explicit route of relative procedure entry/frame descriptors.
pub struct ProcedureEntryRoute;
/// Explicit route of the open continuation-key directory.
pub struct SubscriptionRoute;
/// Explicit route of native global caller-template dependency observations.
pub struct ContinuationRoute;

/// Statically routed capability view. Builtin channels are shared across every
/// point; only the client tail is generation owned. Borrowing never copies a
/// channel or creates a private provider for an identically named global route.
pub struct Ports<'a, N: WordElement, Tail, Global = analysis_kernel::Nil> {
    pub(crate) shared: &'a mut Shared<N>,
    pub(crate) local: &'a mut Tail,
    pub(crate) global: &'a mut Global,
}
impl<N: WordElement, Tail, Global> Select<Here> for Ports<'_, N, Tail, Global> {
    type Output = Channel<Inputs<N>, ImmutableInputPolicy<N>>;
    fn select(&self) -> &Self::Output {
        &self.shared.inputs
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        &mut self.shared.inputs
    }
}
impl<N: WordElement, Tail, Global> Select<There<Here>> for Ports<'_, N, Tail, Global> {
    type Output = Channel<Refinements, Descending<StackMeet>>;
    fn select(&self) -> &Self::Output {
        &self.shared.refinements
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        &mut self.shared.refinements
    }
}
impl<N: WordElement, Tail: Select<P>, Global, P> Select<There<There<P>>>
    for Ports<'_, N, Tail, Global>
{
    type Output = Tail::Output;
    fn select(&self) -> &Self::Output {
        self.local.select()
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        self.local.select_mut()
    }
}
macro_rules! global_route {
    ($route:ty, $field:ident, $channel:ty) => {
        impl<N: WordElement, Tail, Global> Select<$route> for Ports<'_, N, Tail, Global> {
            type Output = $channel;
            fn select(&self) -> &Self::Output {
                &self.shared.$field
            }
            fn select_mut(&mut self) -> &mut Self::Output {
                &mut self.shared.$field
            }
        }
    };
}
global_route!(ProcessRefinementRoute, process_refinements, Channel<ProcessRefinements, Descending<ProcessMeet>>);
global_route!(PointsRoute, points, Channel<Points, Ascending<SetUnion<super::Point>>>);
global_route!(GenerationRoute, generations, Channel<Generations, EntryPolicy>);
global_route!(TopologyRoute, topology, Channel<Topology, Ascending<SetUnion<Edge>>>);
global_route!(
    SummaryRoute,
    summaries,
    Channel<Summaries<N>, SummaryPolicy<N>>
);
global_route!(
    SummaryClosureRoute,
    closures,
    Channel<SummaryClosures, Ascending<ObservationPolicy<SummaryClosure>>>
);
global_route!(
    ContinuationRoute,
    continuations,
    Channel<Continuations<N>, Ascending<ObservationPolicy<Arc<Continuation<N>>>>>
);

impl<N: WordElement, Tail, Global: Select<P>, P> Select<SharedRoute<P>>
    for Ports<'_, N, Tail, Global>
{
    type Output = Global::Output;
    fn select(&self) -> &Self::Output {
        self.global.select()
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        self.global.select_mut()
    }
}

global_route!(ProcedureRoute, procedures, Channel<Procedures, Ascending<SetUnion<super::ProcedureId>>>);
global_route!(SubscriptionRoute, subscriptions, Channel<Subscriptions, Ascending<SetUnion<super::ContinuationId>>>);

global_route!(CalleeRoute, callees, Channel<super::Callees, Ascending<SetUnion<super::ProcedureId>>>);
global_route!(ProcedureEntryRoute, entries, Channel<super::ProcedureEntries, Ascending<super::navigation::DescriptorPolicy>>);
