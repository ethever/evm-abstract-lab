//! Typed dependency validation and bounded direct references.
use super::{
    Expression, ExpressionRef, PathConstraint, PathRef, SymbolicGraph, SymbolicReference,
    SymbolicReferenceFailure, SymbolicSort,
};
impl Expression {
    /// Direct dependencies only. Logical ranges and exact bytes are not expanded.
    pub fn references(&self) -> Vec<SymbolicReference> {
        use SymbolicSort as S;
        let r = |sort, node| SymbolicReference { sort, node };
        let e = |v: ExpressionRef| r(S::Word, v.node);
        if let Some(node) = self.structure() {
            let mut out = Vec::new();
            node.visit_edges(|edge| {
                out.push(match edge {
                    symbolic_structure::Edge::Word(v) => r(S::Word, v.node),
                    symbolic_structure::Edge::Bytes(v) => r(S::Bytes, v.node),
                    symbolic_structure::Edge::Slots(v) => r(S::Slots, v.node),
                    symbolic_structure::Edge::View(v) => r(S::View, v.node),
                })
            });
            return out;
        }
        let p = |v: PathRef| r(S::Path, v.node);
        match self {
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
            Self::PathTrue | Self::PathFalse => Vec::new(),
            _ => unreachable!("core nodes were handled by the shared structure"),
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
