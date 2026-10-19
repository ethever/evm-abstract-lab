//! Retained path formulas use the same report-local word references as states.
use super::Pool;
use super::value;
use evm_abstract::domain::{
    relational::{Constraint, RelationPath, RelationPathKind},
    symbolic::ExprId,
};
use evm_abstract_protocol as api;

pub(super) fn expressions(atoms: &[Constraint]) -> impl Iterator<Item = &ExprId> {
    atoms.iter().flat_map(|atom| {
        let (first, second) = match atom {
            Constraint::Truth { expression, .. }
            | Constraint::Range { expression, .. }
            | Constraint::SignedRange { expression, .. }
            | Constraint::Modulo { expression, .. }
            | Constraint::Bits { expression, .. }
            | Constraint::Members { expression, .. } => (expression, None),
            Constraint::Equal { left, right } => (left, Some(right)),
        };
        std::iter::once(first).chain(second)
    })
}
impl Pool<'_> {
    pub(super) fn constraint(
        &mut self,
        atom: &Constraint,
    ) -> Result<api::PathConstraint, api::ApiError> {
        self.charge(1)?;
        use api::PathConstraint as N;
        Ok(match atom {
            Constraint::Truth {
                expression,
                nonzero,
            } => N::Truth(api::PathTruth {
                expression: self.e(expression),
                nonzero: *nonzero,
            }),
            Constraint::Range {
                expression,
                lower,
                upper,
            } => N::Range(api::PathRange {
                expression: self.e(expression),
                lower: value::word(*lower),
                upper: value::word(*upper),
            }),
            Constraint::SignedRange {
                expression,
                lower,
                upper,
            } => N::SignedRange(api::PathRange {
                expression: self.e(expression),
                lower: value::word(*lower),
                upper: value::word(*upper),
            }),
            Constraint::Modulo {
                expression,
                modulus,
                residue,
            } => N::Modulo(api::PathModulo {
                expression: self.e(expression),
                modulus: value::word(*modulus),
                residue: value::word(*residue),
            }),
            Constraint::Bits {
                expression,
                zero,
                one,
            } => N::Bits(api::PathBits {
                expression: self.e(expression),
                zero: value::word(*zero),
                one: value::word(*one),
            }),
            Constraint::Equal { left, right } => N::Equal(api::PathEquality {
                left: self.e(left),
                right: self.e(right),
            }),
            Constraint::Members { expression, values } => {
                self.charge(values.len())?;
                N::Members(api::PathMembers {
                    expression: self.e(expression),
                    values: values.iter().copied().map(value::word).collect(),
                })
            }
        })
    }
    pub(super) fn path_node(
        &mut self,
        path: &RelationPath,
    ) -> Result<api::Expression, api::ApiError> {
        use api::Expression as N;
        Ok(match path.kind() {
            RelationPathKind::True => N::PathTrue,
            RelationPathKind::False => N::PathFalse,
            RelationPathKind::Atoms(atoms) => N::PathAtoms(
                atoms
                    .iter()
                    .map(|atom| self.constraint(atom))
                    .collect::<Result<_, _>>()?,
            ),
            RelationPathKind::And(paths) => N::PathAnd(paths.iter().map(|v| self.p(v)).collect()),
            RelationPathKind::Ite {
                selector,
                then_path,
                else_path,
            } => N::PathIte(api::PathIte {
                selector: self.e(selector),
                then_path: self.p(then_path),
                else_path: self.p(else_path),
            }),
        })
    }
}
