//! Canonical borrowed structure over native immutable handles. The handles,
//! interners and cached metrics retain their original equality/work policies.
//! Structural maps must rebuild through smart constructors before retention.

use super::{ByteStateId, ByteView, ExprId, InputAtom, SlotStateId, SourceId, StateBinding};
use alloy_primitives::U256;
use std::{marker::PhantomData, sync::Arc};
pub use symbolic_structure::{Edge, Mapper, Node, Postorder, Representation, Sort};

// Kind declarations use the same field schema as maps and exports. The
// scalar kind keeps its ordering and private Fresh serialization behavior;
// byte/slot kinds keep full numeric equality and their original hash derives.
macro_rules! native_enum {
    ($(#[$derive:meta])* $native:ident;
        units { $($(#[$um:meta])* $unit:ident => $uflat:ident;)* }
        prefix { $($(#[$pm:meta])* $prefix:ident => $pflat:ident ($($pw:tt)*) {
            $($pf:ident: $pr:ident => $pwf:ident),* $(,)?
        })* }
        leaves { $($(#[$lm:meta])* $leaf:ident => $lflat:ident: $lr:ident [$(#[$la:meta])*];)* }
        records { $($(#[$rm:meta])* $record:ident => $rflat:ident ($($wire:tt)*) {
            $($field:ident: $role:ident => $wf:ident),* $(,)?
        })* }
    ) => {
        #[doc = concat!("Immutable native ", stringify!($native), " structure.")]
        $(#[$derive])*
        pub enum $native {
            $($(#[$um])* $unit,)*
            $($(#[$pm])* $prefix { $(#[doc = stringify!($pf)] $pf: symbolic_structure::$pr<Native>),* },)*
            $($(#[$lm])* $leaf($(#[$la])* symbolic_structure::$lr<Native>),)*
            $($(#[$rm])* $record { $(#[doc = stringify!($field)] $field: symbolic_structure::$role<Native>),* },)*
        }
    };
}
macro_rules! native_kind {
    (View, $native:ident, $($body:tt)*) => {};
    (Word, $native:ident, $($body:tt)*) => {
        native_enum! {
            #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
            $native; $($body)*
        }
    };
    ($sort:ident, $native:ident, $($body:tt)*) => {
        native_enum! {
            #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize)]
            $native; $($body)*
        }
    };
}
macro_rules! native_kinds {
    ($($sort:ident $native:ident { $($body:tt)* })*) => {
        $(native_kind!($sort, $native, $($body)*);)*
    };
}
symbolic_structure::with_nodes!(native_kinds);

/// Owned native operands and payloads, used before smart reconstruction.
#[derive(Clone, Debug)]
pub struct Native;
impl Representation for Native {
    type Word = ExprId;
    type Words = Vec<ExprId>;
    type Bindings = Vec<StateBinding>;
    type Bytes = ByteStateId;
    type Slots = SlotStateId;
    type View = ByteView;
    type Constant = U256;
    type Input = InputAtom;
    type Exact = Arc<[u8]>;
    type Source = SourceId;
    type Binding = StateBinding;
    fn source_words(source: &SourceId, action: &mut impl FnMut(ExprId)) {
        action(source.token().clone());
    }
    fn binding_words(binding: &StateBinding, action: &mut impl FnMut(ExprId)) {
        action(binding.value.clone());
    }
}

/// Native view that borrows payloads and handles, including observed numeric
/// facts. Discovering edges does not clone or re-interpret those facts.
#[derive(Clone, Debug)]
pub struct Borrowed<'a>(PhantomData<&'a ()>);
impl<'a> Representation for Borrowed<'a> {
    type Word = &'a ExprId;
    type Words = std::slice::Iter<'a, ExprId>;
    type Bindings = std::slice::Iter<'a, StateBinding>;
    type Bytes = &'a ByteStateId;
    type Slots = &'a SlotStateId;
    type View = &'a ByteView;
    type Constant = &'a U256;
    type Input = &'a InputAtom;
    type Exact = &'a Arc<[u8]>;
    type Source = &'a SourceId;
    type Binding = &'a StateBinding;
    fn source_words(source: &&'a SourceId, action: &mut impl FnMut(&'a ExprId)) {
        action(source.token());
    }
    fn binding_words(binding: &&'a StateBinding, action: &mut impl FnMut(&'a ExprId)) {
        action(&binding.value);
    }
}

/// Retained native graph key. Words retain structural identity; states retain
/// collision-checked interner identity and views retain all three operands.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Handle {
    /// Word256 expression.
    Word(ExprId),
    /// Immutable byte state.
    Bytes(ByteStateId),
    /// Immutable slot state.
    Slots(SlotStateId),
    /// Bounded byte view.
    View(ByteView),
}
impl Handle {
    /// Cached conservative graph work (views combine three cached operands).
    pub fn nodes(&self) -> usize {
        match self {
            Self::Word(v) => v.nodes(),
            Self::Bytes(v) => v.nodes(),
            Self::Slots(v) => v.nodes(),
            Self::View(v) => v.nodes(),
        }
    }
    /// Cached dependency depth, preserving the view's existing depth policy.
    pub fn depth(&self) -> usize {
        match self {
            Self::Word(v) => v.depth(),
            Self::Bytes(v) => v.depth(),
            Self::Slots(v) => v.depth(),
            Self::View(v) => v.depth(),
        }
    }
    /// Ordered direct operands, without expanding byte payloads/ranges.
    pub fn dependencies(&self) -> Vec<Self> {
        let mut out = Vec::new();
        self.structure().visit_edges(|edge| {
            out.push(match edge {
                Edge::Word(v) => Self::Word(v.clone()),
                Edge::Bytes(v) => Self::Bytes(v.clone()),
                Edge::Slots(v) => Self::Slots(v.clone()),
                Edge::View(v) => Self::View(v.clone()),
            });
        });
        out
    }
}

macro_rules! native_node {
    (View, $native:ident, $value:ident,
        units {} prefix {} leaves {} records {
            $($(#[$meta:meta])* $variant:ident => $flat:ident ($($wire:tt)*) {
                $($field:ident: $role:ident => $wf:ident),*
            })*
        }
    ) => {
        $(Node::$flat { $($field: symbolic_structure::borrow_field!($role, &$value.$field)),* })*
    };
    ($sort:ident, $native:ident, $value:ident,
        units { $($(#[$um:meta])* $unit:ident => $uflat:ident;)* }
        prefix { $($(#[$pm:meta])* $prefix:ident => $pflat:ident ($($pw:tt)*) {
            $($pf:ident: $pr:ident => $pwf:ident),* $(,)?
        })* }
        leaves { $($(#[$lm:meta])* $leaf:ident => $lflat:ident: $lr:ident [$(#[$la:meta])*];)* }
        records { $($(#[$rm:meta])* $record:ident => $rflat:ident ($($wire:tt)*) {
            $($field:ident: $role:ident => $wf:ident),*
        })* }
    ) => {
        match $value.kind() {
            $($native::$unit => Node::$uflat,)*
            $($native::$prefix { $($pf),* } => Node::$pflat { $($pf: symbolic_structure::borrow_field!($pr, $pf)),* },)*
            $($native::$leaf(value) => Node::$lflat(symbolic_structure::borrow_field!($lr, value)),)*
            $($native::$record { $($field),* } => Node::$rflat {
                $($field: symbolic_structure::borrow_field!($role, $field)),*
            },)*
        }
    };
}
macro_rules! native_structure {
    ($($sort:ident $native:ident { $($body:tt)* })*) => {
        impl Handle {
            /// Borrow the canonical node shape without exposing mutable nodes.
            pub fn structure(&self) -> Node<Borrowed<'_>> {
                match self {
                    $(Self::$sort(value) => native_node!($sort, $native, value, $($body)*),)*
                }
            }
        }
    };
}
symbolic_structure::with_nodes!(native_structure);
