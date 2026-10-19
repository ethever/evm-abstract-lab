//! Typed dependency validation and bounded direct references.
use super::{
    Expression, ExpressionRef, PathConstraint, PathRef, SymbolicGraph, SymbolicReference,
    SymbolicReferenceFailure, SymbolicSort,
};
impl Expression {
    /// Direct dependencies only. Logical ranges and exact bytes are not expanded.
    pub fn references(&self) -> Vec<SymbolicReference> {
        let mut out = Vec::new();
        self.try_visit_references(|reference| {
            out.push(reference);
            Ok::<_, std::convert::Infallible>(())
        })
        .unwrap_or_else(|never| match never {});
        out
    }
    fn try_visit_references<E>(
        &self,
        mut action: impl FnMut(SymbolicReference) -> Result<(), E>,
    ) -> Result<(), E> {
        use SymbolicSort as S;
        let r = |sort, node| SymbolicReference { sort, node };
        let e = |v: ExpressionRef| r(S::Word, v.node);
        if let Some(node) = self.structure() {
            return node.try_visit_edges(|edge| {
                action(match edge {
                    symbolic_structure::Edge::Word(v) => r(S::Word, v.node),
                    symbolic_structure::Edge::Bytes(v) => r(S::Bytes, v.node),
                    symbolic_structure::Edge::Slots(v) => r(S::Slots, v.node),
                    symbolic_structure::Edge::View(v) => r(S::View, v.node),
                })
            });
        }
        let p = |v: PathRef| r(S::Path, v.node);
        match self {
            Self::PathAnd(paths) => paths.iter().try_for_each(|path| action(p(*path))),
            Self::PathIte(x) => {
                action(e(x.selector))?;
                action(p(x.then_path))?;
                action(p(x.else_path))
            }
            Self::PathAtoms(atoms) => {
                for atom in atoms {
                    let (first, second) = match atom {
                        PathConstraint::Truth(x) => (x.expression, None),
                        PathConstraint::Range(x) | PathConstraint::SignedRange(x) => {
                            (x.expression, None)
                        }
                        PathConstraint::Modulo(x) => (x.expression, None),
                        PathConstraint::Bits(x) => (x.expression, None),
                        PathConstraint::Equal(x) => (x.left, Some(x.right)),
                        PathConstraint::Members(x) => (x.expression, None),
                    };
                    action(e(first))?;
                    if let Some(second) = second {
                        action(e(second))?;
                    }
                }
                Ok(())
            }
            Self::PathTrue | Self::PathFalse => Ok(()),
            _ => unreachable!("core nodes were handled by the shared structure"),
        }
    }
}
impl SymbolicGraph {
    /// Validate all node sorts and dependency order before following references.
    pub fn validate(&self) -> Result<(), SymbolicReferenceFailure> {
        for (owner, node) in self.nodes.iter().enumerate() {
            node.try_visit_references(|reference| self.reference(Some(owner), reference))?;
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
