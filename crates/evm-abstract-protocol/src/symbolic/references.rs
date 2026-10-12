//! Typed dependency validation and bounded direct references.
use super::{
    ByteStateRef, ByteViewRef, Expression, ExpressionRef, PathConstraint, PathRef, SlotStateRef,
    SymbolicGraph, SymbolicReference, SymbolicReferenceFailure, SymbolicSort,
};
impl Expression {
    /// Semantic sort; identity names never change a node's sort.
    pub fn sort(&self) -> SymbolicSort {
        use SymbolicSort as S;
        match self {
            Self::Constant(_)
            | Self::Input(_)
            | Self::Fresh(_)
            | Self::Operation(_)
            | Self::Ite(_)
            | Self::ByteRead(_)
            | Self::WordRead(_)
            | Self::SlotRead(_)
            | Self::Keccak(_) => S::Word,
            Self::BytesZero
            | Self::BytesInput(_)
            | Self::BytesExact(_)
            | Self::WriteByte(_)
            | Self::WriteWord(_)
            | Self::Copy(_)
            | Self::Slice(_)
            | Self::BytesIte(_) => S::Bytes,
            Self::SlotsZero | Self::SlotsInput(_) | Self::Store(_) | Self::SlotsIte(_) => S::Slots,
            Self::View(_) => S::View,
            Self::PathTrue
            | Self::PathFalse
            | Self::PathAtoms(_)
            | Self::PathAnd(_)
            | Self::PathIte(_) => S::Path,
        }
    }
    /// Direct dependencies only. Logical ranges and exact bytes are not expanded.
    pub fn references(&self) -> Vec<SymbolicReference> {
        use SymbolicSort as S;
        let r = |sort, node| SymbolicReference { sort, node };
        let e = |v: ExpressionRef| r(S::Word, v.node);
        let b = |v: ByteStateRef| r(S::Bytes, v.node);
        let s = |v: SlotStateRef| r(S::Slots, v.node);
        let v = |v: ByteViewRef| r(S::View, v.node);
        let p = |v: PathRef| r(S::Path, v.node);
        match self {
            Self::Constant(_)
            | Self::Input(_)
            | Self::Fresh(_)
            | Self::BytesZero
            | Self::BytesExact(_)
            | Self::SlotsZero
            | Self::PathTrue
            | Self::PathFalse => vec![],
            Self::Operation(op) => op.arguments.iter().copied().map(e).collect(),
            Self::Ite(x) => vec![e(x.condition), e(x.then_value), e(x.else_value)],
            Self::ByteRead(x) => vec![b(x.state), e(x.index)],
            Self::WordRead(x) => vec![v(x.view), e(x.offset)],
            Self::SlotRead(x) => vec![s(x.state), e(x.key)],
            Self::Keccak(x) | Self::Slice(x) => vec![v(*x)],
            Self::BytesInput(x) => std::iter::once(e(x.source.token))
                .chain(std::iter::once(e(x.length)))
                .chain(x.bindings.iter().flat_map(|binding| {
                    std::iter::once(e(binding.value)).chain(binding.numeric.expression.map(e))
                }))
                .collect(),
            Self::SlotsInput(x) => std::iter::once(e(x.source.token))
                .chain(x.bindings.iter().flat_map(|binding| {
                    std::iter::once(e(binding.value)).chain(binding.numeric.expression.map(e))
                }))
                .collect(),
            Self::WriteByte(x) | Self::WriteWord(x) => vec![b(x.previous), e(x.offset), e(x.value)],
            Self::Copy(x) => vec![b(x.previous), e(x.destination), v(x.source), e(x.length)],
            Self::BytesIte(x) => vec![e(x.condition), b(x.then_state), b(x.else_state)],
            Self::Store(x) => vec![s(x.previous), e(x.key), e(x.value)],
            Self::SlotsIte(x) => vec![e(x.condition), s(x.then_state), s(x.else_state)],
            Self::View(x) => vec![b(x.state), e(x.offset), e(x.length)],
            Self::PathAnd(paths) => paths.iter().copied().map(p).collect(),
            Self::PathIte(x) => vec![e(x.selector), p(x.then_path), p(x.else_path)],
            Self::PathAtoms(atoms) => atoms
                .iter()
                .flat_map(|atom| match atom {
                    PathConstraint::Truth(x) => vec![e(x.expression)],
                    PathConstraint::Range(x) | PathConstraint::SignedRange(x) => {
                        vec![e(x.expression)]
                    }
                    PathConstraint::Modulo(x) => vec![e(x.expression)],
                    PathConstraint::Bits(x) => vec![e(x.expression)],
                    PathConstraint::Equal(x) => vec![e(x.left), e(x.right)],
                    PathConstraint::Members(x) => vec![e(x.expression)],
                })
                .collect(),
        }
    }
}
impl SymbolicGraph {
    /// Validate all node sorts and dependency order before following references.
    pub fn validate(&self) -> Result<(), SymbolicReferenceFailure> {
        for (owner, node) in self.nodes.iter().enumerate() {
            for reference in node.references() {
                self.reference(Some(owner), reference)?;
            }
        }
        Ok(())
    }
    /// Validate a root or a direct dependency against this directory.
    pub fn reference(
        &self,
        owner: Option<usize>,
        reference: SymbolicReference,
    ) -> Result<(), SymbolicReferenceFailure> {
        let actual = self.nodes.get(reference.node).map(Expression::sort);
        let forward = owner.is_some_and(|owner| reference.node >= owner);
        if actual != Some(reference.sort) || forward {
            return Err(SymbolicReferenceFailure {
                owner,
                node: reference.node,
                expected: reference.sort,
                actual,
                forward,
            });
        }
        Ok(())
    }
}
