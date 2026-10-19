//! Typed adapters between the shared symbolic shape and the existing strict
//! wire records. Paths remain a protocol extension, handled by their consumers.

use super::{
    ByteCopy, ByteRead, ByteStateRef, ByteView, ByteViewRef, ByteWrite, BytesInput, BytesIte,
    Expression, ExpressionIte, ExpressionOperation, ExpressionRef, SlotRead, SlotStateRef,
    SlotWrite, SlotsInput, SlotsIte, SymbolicBinding, SymbolicSort, SymbolicSource, WordRead,
};
use std::marker::PhantomData;
use symbolic_structure::{Node, Representation};

/// Typed report-local references and wire payloads after native projection.
#[derive(Clone, Debug)]
pub struct Wire;
impl Representation for Wire {
    type Word = ExpressionRef;
    type Words = Vec<ExpressionRef>;
    type Bindings = Vec<SymbolicBinding>;
    type Bytes = ByteStateRef;
    type Slots = SlotStateRef;
    type View = ByteViewRef;
    type Constant = String;
    type Input = crate::InputIdentity;
    type Exact = String;
    type Source = SymbolicSource;
    type Binding = SymbolicBinding;
    fn source_words(source: &SymbolicSource, action: &mut impl FnMut(ExpressionRef)) {
        action(source.token);
    }
    fn binding_words(binding: &SymbolicBinding, action: &mut impl FnMut(ExpressionRef)) {
        action(binding.value);
        if let Some(expression) = binding.numeric.expression {
            action(expression);
        }
    }
}

/// Borrowed wire view; validation never clones exact bytes or numeric payloads.
#[derive(Clone, Debug)]
pub struct Borrowed<'a>(PhantomData<&'a ()>);
impl<'a> Representation for Borrowed<'a> {
    type Word = &'a ExpressionRef;
    type Words = std::slice::Iter<'a, ExpressionRef>;
    type Bindings = std::slice::Iter<'a, SymbolicBinding>;
    type Bytes = &'a ByteStateRef;
    type Slots = &'a SlotStateRef;
    type View = &'a ByteViewRef;
    type Constant = &'a String;
    type Input = &'a crate::InputIdentity;
    type Exact = &'a String;
    type Source = &'a SymbolicSource;
    type Binding = &'a SymbolicBinding;
    fn source_words(source: &&'a SymbolicSource, action: &mut impl FnMut(&'a ExpressionRef)) {
        action(&source.token);
    }
    fn binding_words(binding: &&'a SymbolicBinding, action: &mut impl FnMut(&'a ExpressionRef)) {
        action(&binding.value);
        if let Some(expression) = &binding.numeric.expression {
            action(expression);
        }
    }
}

impl From<symbolic_structure::Sort> for SymbolicSort {
    fn from(value: symbolic_structure::Sort) -> Self {
        match value {
            symbolic_structure::Sort::Word => Self::Word,
            symbolic_structure::Sort::Bytes => Self::Bytes,
            symbolic_structure::Sort::Slots => Self::Slots,
            symbolic_structure::Sort::View => Self::View,
        }
    }
}

macro_rules! wire_record {
    (reuse $name:ident, $flat:ident, $fields:tt) => { wire_record!(record $name, $flat, $fields) };
    (record $name:ident, $flat:ident, { $($field:ident => $wire:ident),* }) => {
        Expression::$flat($name { $($wire: $field),* })
    };
    (tuple $name:ident, $flat:ident, { $($field:ident => $wire:ident),* }) => {
        Expression::$flat($name)
    };
}
macro_rules! wire_borrow {
    (reuse $name:ident, $flat:ident, $value:ident, $fields:tt) => { wire_borrow!(record $name, $flat, $value, $fields) };
    (record $name:ident, $flat:ident, $value:ident, { $($field:ident: $role:ident => $wire:ident),* }) => {
        Node::$flat { $($field: symbolic_structure::borrow_field!($role, &$value.$wire)),* }
    };
    (tuple $name:ident, $flat:ident, $value:ident, { $($field:ident: $role:ident => $wire:ident),* }) => {
        Node::$flat { $($field: symbolic_structure::borrow_field!($role, $value)),* }
    };
}
macro_rules! wire_structure {
    ($($sort:ident $native:ident {
        units { $($(#[$um:meta])* $unit:ident => $uflat:ident;)* }
        prefix { $($(#[$pm:meta])* $prefix:ident => $pflat:ident ($($pw:tt)*) {
            $($pf:ident: $pr:ident => $pwf:ident),* $(,)?
        })* }
        leaves { $($(#[$lm:meta])* $leaf:ident => $lflat:ident: $lr:ident [$(#[$la:meta])*];)* }
        records { $($(#[$rm:meta])* $record:ident => $rflat:ident ($($wire:tt)*) {
            $($field:ident: $role:ident => $wf:ident),* $(,)?
        })* }
    })*) => {
        impl From<Node<Wire>> for Expression {
            fn from(node: Node<Wire>) -> Self {
                match node {
                    $($(Node::$uflat => Self::$uflat,)*
                      $(Node::$pflat { $($pf),* } => wire_record!($($pw)*, $pflat, { $($pf => $pwf),* }),)*
                      $(Node::$lflat(value) => Self::$lflat(value),)*
                      $(Node::$rflat { $($field),* } => wire_record!($($wire)*, $rflat, { $($field => $wf),* }),)*
                    )*
                }
            }
        }
        impl Expression {
            /// Borrow a core symbolic shape; relation paths are an explicit
            /// extension whose additional dependencies stay protocol-owned.
            pub fn structure(&self) -> Option<Node<Borrowed<'_>>> {
                Some(match self {
                    $($(Self::$uflat => Node::$uflat,)*
                      $(Self::$pflat(value) => wire_borrow!($($pw)*, $pflat, value, { $($pf: $pr => $pwf),* }),)*
                      $(Self::$lflat(value) => Node::$lflat(symbolic_structure::borrow_field!($lr, value)),)*
                      $(Self::$rflat(value) => wire_borrow!($($wire)*, $rflat, value, { $($field: $role => $wf),* }),)*
                    )*
                    Self::PathTrue | Self::PathFalse | Self::PathAtoms(_)
                    | Self::PathAnd(_) | Self::PathIte(_) => return None,
                })
            }
            /// Semantic result sort, derived from the same schema as its edges.
            pub fn sort(&self) -> SymbolicSort {
                match self {
                    $($(Self::$uflat => SymbolicSort::$sort,)*
                      $(Self::$pflat(_) => SymbolicSort::$sort,)*
                      $(Self::$lflat(_) => SymbolicSort::$sort,)*
                      $(Self::$rflat(_) => SymbolicSort::$sort,)*
                    )*
                    Self::PathTrue | Self::PathFalse | Self::PathAtoms(_)
                    | Self::PathAnd(_) | Self::PathIte(_) => SymbolicSort::Path,
                }
            }
        }
    };
}
symbolic_structure::with_nodes!(wire_structure);
