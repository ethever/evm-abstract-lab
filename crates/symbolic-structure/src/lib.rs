//! One typed structural shape for immutable word, byte, slot and view DAGs.
//!
//! Representations choose handles and payloads; mapping never interprets them.
//! In particular numeric binding facts and source identities remain consumer
//! policies. The schema fixes direct edge sorts and deterministic field order.

use std::fmt::Debug;

mod schedule;
mod schema;
pub use schedule::Postorder;

/// Handles and payloads used by one borrowed or owned node representation.
pub trait Representation: Clone + Debug {
    /// Word expression reference.
    type Word: Clone + Debug;
    /// Ordered word operands; borrowed views use a constant-size iterator.
    type Words: Clone + Debug + IntoIterator<Item = Self::Word>;
    /// Ordered observations, retaining their numeric payloads.
    type Bindings: Clone + Debug + IntoIterator<Item = Self::Binding>;
    /// Immutable byte-state reference.
    type Bytes: Clone + Debug;
    /// Immutable slot-state reference.
    type Slots: Clone + Debug;
    /// Bounded byte-view reference.
    type View: Clone + Debug;
    /// Exact Word256 literal.
    type Constant: Clone + Debug;
    /// Scoped immutable input identity.
    type Input: Clone + Debug;
    /// Exact bounded byte payload.
    type Exact: Clone + Debug;
    /// Source identity and namespace.
    type Source: Clone + Debug;
    /// Observed cell, including its numeric guarantees.
    type Binding: Clone + Debug;

    /// Source tokens are word dependencies; namespaces are payload.
    fn source_words(source: &Self::Source, action: &mut impl FnMut(Self::Word));
    /// Native numeric facts contain no edges; wire facts may contain an
    /// additional expression reference and must expose it in stored order.
    fn binding_words(binding: &Self::Binding, action: &mut impl FnMut(Self::Word));
}

// Aliases keep schema-generated field types visible to derive macros without
// introducing wrapper values or weakening the four distinct reference sorts.
macro_rules! field_aliases {
    ($($role:ident),*) => { $(#[doc(hidden)] pub type $role<R> = <R as Representation>::$role;)* };
}
field_aliases!(
    Word, Words, Bytes, Slots, View, Constant, Input, Exact, Source, Binding, Bindings
);
#[doc(hidden)]
pub trait ScalarFields {
    /// Fixed opcode payload, shared by every representation.
    type Opcode;
    /// Fixed runtime identity payload before consumer numbering.
    type Fresh;
}
impl<R> ScalarFields for R {
    type Opcode = u8;
    type Fresh = u64;
}
#[doc(hidden)]
pub type Opcode<R> = <R as ScalarFields>::Opcode;
#[doc(hidden)]
pub type Fresh<R> = <R as ScalarFields>::Fresh;

/// Semantic sort of a mixed symbolic node or edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// Word256 expression.
    Word,
    /// Immutable byte state.
    Bytes,
    /// Immutable slot state.
    Slots,
    /// Bounded byte view.
    View,
}

/// A typed direct dependency, preserving the representation's handle type.
#[derive(Clone, Debug)]
pub enum Edge<R: Representation> {
    /// Word operand.
    Word(R::Word),
    /// Byte-state operand.
    Bytes(R::Bytes),
    /// Slot-state operand.
    Slots(R::Slots),
    /// View operand.
    View(R::View),
}

/// Explicit payload/handle policies for a structural map. Smart constructors
/// run after mapping; contextual reads must not use this as a value cache.
pub trait Mapper<R: Representation> {
    /// Destination representation.
    type Output: Representation;
    /// Consumer-owned error and cancellation boundary.
    type Error;
    /// Map a word operand.
    fn word(&mut self, value: &R::Word) -> Result<Word<Self::Output>, Self::Error>;
    /// Map a byte-state operand.
    fn bytes(&mut self, value: &R::Bytes) -> Result<Bytes<Self::Output>, Self::Error>;
    /// Map a slot-state operand.
    fn slots(&mut self, value: &R::Slots) -> Result<Slots<Self::Output>, Self::Error>;
    /// Map a bounded-view operand.
    fn view(&mut self, value: &R::View) -> Result<View<Self::Output>, Self::Error>;
    /// Project an exact scalar payload.
    fn constant(&mut self, value: &R::Constant) -> Result<Constant<Self::Output>, Self::Error>;
    /// Project an input while preserving its scope.
    fn input(&mut self, value: &R::Input) -> Result<Input<Self::Output>, Self::Error>;
    /// Preserve native runtime tokens or assign report-local fresh numbers.
    fn fresh(&mut self, value: u64) -> Result<u64, Self::Error>;
    /// Project/charge an exact byte payload.
    fn exact(&mut self, value: &R::Exact) -> Result<Exact<Self::Output>, Self::Error>;
    /// Map a source token while retaining its namespace.
    fn source(&mut self, value: &R::Source) -> Result<Source<Self::Output>, Self::Error>;
    /// Map an observed symbolic cell and preserve/charge its numeric facts.
    fn binding(&mut self, value: &R::Binding) -> Result<Binding<Self::Output>, Self::Error>;
}

macro_rules! map_field {
    (Word, $m:ident, $v:ident) => {
        $m.word($v)?
    };
    (Words, $m:ident, $v:ident) => {
        $v.clone()
            .into_iter()
            .map(|v| $m.word(&v))
            .collect::<Result<_, _>>()?
    };
    (Bytes, $m:ident, $v:ident) => {
        $m.bytes($v)?
    };
    (Slots, $m:ident, $v:ident) => {
        $m.slots($v)?
    };
    (View, $m:ident, $v:ident) => {
        $m.view($v)?
    };
    (Constant, $m:ident, $v:ident) => {
        $m.constant($v)?
    };
    (Input, $m:ident, $v:ident) => {
        $m.input($v)?
    };
    (Fresh, $m:ident, $v:ident) => {
        $m.fresh(*$v)?
    };
    (Opcode, $m:ident, $v:ident) => {
        *$v
    };
    (Exact, $m:ident, $v:ident) => {
        $m.exact($v)?
    };
    (Source, $m:ident, $v:ident) => {
        $m.source($v)?
    };
    (Bindings, $m:ident, $v:ident) => {
        $v.clone()
            .into_iter()
            .map(|v| $m.binding(&v))
            .collect::<Result<_, _>>()?
    };
}
macro_rules! visit_field {
    (Word, $r:ident, $v:ident, $a:ident) => {
        $a(Edge::Word($v.clone()));
    };
    (Words, $r:ident, $v:ident, $a:ident) => {
        for v in $v.clone() {
            $a(Edge::Word(v));
        }
    };
    (Bytes, $r:ident, $v:ident, $a:ident) => {
        $a(Edge::Bytes($v.clone()));
    };
    (Slots, $r:ident, $v:ident, $a:ident) => {
        $a(Edge::Slots($v.clone()));
    };
    (View, $r:ident, $v:ident, $a:ident) => {
        $a(Edge::View($v.clone()));
    };
    (Source, $r:ident, $v:ident, $a:ident) => {
        $r::source_words($v, &mut |v| $a(Edge::Word(v)));
    };
    (Bindings, $r:ident, $v:ident, $a:ident) => {
        for b in $v.clone() {
            $r::binding_words(&b, &mut |v| $a(Edge::Word(v)));
        }
    };
    ($payload:ident, $r:ident, $v:ident, $a:ident) => {
        let _ = $v;
    };
}
macro_rules! define_nodes {
    ($($sort:ident $native:ident {
        units { $($(#[$um:meta])* $unit:ident => $uflat:ident;)* }
        prefix { $($(#[$pm:meta])* $prefix:ident => $pflat:ident ($($pw:tt)*) {
            $($pf:ident: $pr:ident => $pwf:ident),* $(,)?
        })* }
        leaves { $($(#[$lm:meta])* $leaf:ident => $lflat:ident: $lr:ident [$(#[$la:meta])*];)* }
        records { $($(#[$rm:meta])* $record:ident => $rflat:ident ($($wire:tt)*) {
            $($field:ident: $role:ident => $wire_field:ident),* $(,)?
        })* }
    })*) => {
        /// Canonical flat symbolic node, parameterized by typed handles and
        /// payloads. Its serialized native form retains the original variants.
        #[derive(Clone, Debug)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize))]
        #[cfg_attr(feature = "serde", serde(rename = "SymbolicGraphNode"))]
        #[cfg_attr(feature = "serde", serde(bound(serialize = "R::Word: serde::Serialize, R::Bytes: serde::Serialize, R::Slots: serde::Serialize, R::View: serde::Serialize, R::Constant: serde::Serialize, R::Input: serde::Serialize, R::Exact: serde::Serialize, R::Source: serde::Serialize, R::Bindings: serde::Serialize, R::Words: serde::Serialize")))]
        pub enum Node<R: Representation> {
            $($( $(#[$um])* $uflat, )*
              $( $(#[$pm])* $pflat { $( #[doc = stringify!($pf)] $pf: $crate::$pr<R> ),* }, )*
              $( $(#[$lm])* $lflat($crate::$lr<R>), )*
              $( $(#[$rm])* $rflat { $( #[doc = stringify!($field)] $field: $crate::$role<R> ),* }, )*
            )*
        }
        impl<R: Representation> Node<R> {
            /// Semantic result sort, independent of payload and identity.
            pub fn sort(&self) -> Sort {
                match self {
                    $($( Self::$uflat => Sort::$sort, )*
                      $( Self::$pflat { .. } => Sort::$sort, )*
                      $( Self::$lflat(_) => Sort::$sort, )*
                      $( Self::$rflat { .. } => Sort::$sort, )*
                    )*
                }
            }
            /// Visit direct typed edges in schema field order. No logical byte
            /// ranges expand and no payload/numeric facts are copied.
            pub fn visit_edges(&self, mut action: impl FnMut(Edge<R>)) {
                match self {
                    $($( Self::$uflat => {}, )*
                      $( Self::$pflat { $($pf),* } => { $( visit_field!($pr, R, $pf, action); )* }, )*
                      $( Self::$lflat(value) => { visit_field!($lr, R, value, action); }, )*
                      $( Self::$rflat { $($field),* } => {
                          $( visit_field!($role, R, $field, action); )*
                      }, )*
                    )*
                }
            }
            /// Map every typed operand and payload once while retaining shape.
            pub fn try_map<M: Mapper<R>>(&self, mapper: &mut M) -> Result<Node<M::Output>, M::Error>
            where
                Words<M::Output>: FromIterator<Word<M::Output>>,
                Bindings<M::Output>: FromIterator<Binding<M::Output>>,
            {
                Ok(match self {
                    $($( Self::$uflat => Node::$uflat, )*
                      $( Self::$pflat { $($pf),* } => Node::$pflat { $( $pf: map_field!($pr, mapper, $pf) ),* }, )*
                      $( Self::$lflat(value) => Node::$lflat(map_field!($lr, mapper, value)), )*
                      $( Self::$rflat { $($field),* } => Node::$rflat {
                          $( $field: map_field!($role, mapper, $field) ),*
                      }, )*
                    )*
                })
            }
        }
    };
}
with_nodes!(define_nodes);
