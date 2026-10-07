//! cvc5 primitives through its official safe Rust bindings.
//!
//! Each request owns a fresh term manager and solver. The native per-query
//! resource limit bounds solving; native handles never leave `solve`.

use crate::{
    Bool, Bv, Error, Outcome, Provider, Unknown,
    backend::{self, Backend, Status},
    term::{Node, Op, Sort},
};
use ::cvc5::{Kind, Solver, Term, TermManager, UnknownExplanation};

struct Cvc5 {
    terms: TermManager,
    solver: Solver,
}

fn native_error(error: ::cvc5::Error) -> Error {
    Error {
        provider: Provider::Cvc5,
        message: error.to_string(),
    }
}

fn adapter_error(message: &str) -> Error {
    Error {
        provider: Provider::Cvc5,
        message: message.into(),
    }
}

impl Cvc5 {
    fn new(rlimit: u32) -> Result<Self, Error> {
        let terms = TermManager::new();
        let mut solver = Solver::new(&terms);
        solver.set_logic("QF_BV").map_err(native_error)?;
        solver
            .set_option("produce-models", "true")
            .map_err(native_error)?;
        // A session performs exactly one check, including uniqueness exclusions.
        solver
            .set_option("incremental", "false")
            .map_err(native_error)?;
        // Keep shallow auxiliary equations as a DAG. NonClausalSimp otherwise
        // substitutes repeated-square definitions into each other, and the BV
        // multiplication rewriter expands exponentially before a resource
        // checkpoint. This option disables that pass, not ordinary rewriting
        // or bit-blasting; all original assertions remain in the query.
        solver
            .set_option("simplification", "none")
            .map_err(native_error)?;
        // The default BV backend builds a complete batch of multiplier CNFs
        // inside one theory callback before noticing a pending interruption.
        // Native arithmetic CEGAR keeps wide operators at word level first;
        // its exact refinement remains governed by the same resource manager.
        solver
            .set_option("bv-abstraction", "true")
            .map_err(native_error)?;
        solver
            .set_option("rlimit-per", &rlimit.to_string())
            .map_err(native_error)?;
        Ok(Self { terms, solver })
    }

    fn indexed(&mut self, kind: Kind, indices: &[u32], args: &[Term]) -> Result<Term, Error> {
        let op = self.terms.mk_op(kind, indices).map_err(native_error)?;
        self.terms.mk_term_from_op(op, args).map_err(native_error)
    }
}

impl Backend for Cvc5 {
    type Term = Term;

    fn make(&mut self, node: &Node, args: &[Term]) -> Result<Term, Error> {
        let kind = match &node.op {
            Op::Bool(value) => return Ok(self.terms.mk_boolean(*value)),
            Op::Literal(hex) => {
                let Sort::Bv(width) = node.sort else {
                    return Err(adapter_error("bit-vector literal has a Boolean sort"));
                };
                return self
                    .terms
                    .mk_bv_from_str(width, hex, 16)
                    .map_err(native_error);
            }
            Op::Variable(name) => {
                if name.contains('\0') {
                    return Err(adapter_error("symbol name contains a NUL byte"));
                }
                let sort = match node.sort {
                    Sort::Bool => self.terms.boolean_sort(),
                    Sort::Bv(width) => self.terms.mk_bv_sort(width).map_err(native_error)?,
                };
                return self.terms.mk_const(sort, name).map_err(native_error);
            }
            Op::Eq => Kind::Equal,
            Op::Not => Kind::Not,
            Op::And => Kind::And,
            Op::Or => Kind::Or,
            Op::Ite => Kind::Ite,
            Op::Add => Kind::BitvectorAdd,
            Op::Sub => Kind::BitvectorSub,
            Op::Mul => Kind::BitvectorMult,
            Op::Udiv => Kind::BitvectorUdiv,
            Op::Sdiv => Kind::BitvectorSdiv,
            Op::Urem => Kind::BitvectorUrem,
            Op::Srem => Kind::BitvectorSrem,
            Op::Ult => Kind::BitvectorUlt,
            Op::Ule => Kind::BitvectorUle,
            Op::Ugt => Kind::BitvectorUgt,
            Op::Uge => Kind::BitvectorUge,
            Op::Slt => Kind::BitvectorSlt,
            Op::Sle => Kind::BitvectorSle,
            Op::Sgt => Kind::BitvectorSgt,
            Op::Sge => Kind::BitvectorSge,
            Op::BvAnd => Kind::BitvectorAnd,
            Op::BvOr => Kind::BitvectorOr,
            Op::BvXor => Kind::BitvectorXor,
            Op::BvNot => Kind::BitvectorNot,
            Op::Shl => Kind::BitvectorShl,
            Op::Lshr => Kind::BitvectorLshr,
            Op::Ashr => Kind::BitvectorAshr,
            Op::ZeroExtend(extra) => {
                return self.indexed(Kind::BitvectorZeroExtend, &[*extra], args);
            }
            Op::Extract(high, low) => {
                return self.indexed(Kind::BitvectorExtract, &[*high, *low], args);
            }
        };
        self.terms.mk_term(kind, args).map_err(native_error)
    }

    fn assert(&mut self, term: &Term) -> Result<(), Error> {
        self.solver
            .assert_formula(term.clone())
            .map_err(native_error)
    }

    fn check(&mut self) -> Result<Status, Error> {
        let result = self.solver.check_sat().map_err(native_error)?;
        Ok(if result.is_sat() {
            Status::Sat
        } else if result.is_unsat() {
            Status::Unsat
        } else {
            let reason = match result.unknown_explanation() {
                UnknownExplanation::Resourceout => Unknown::ResourceLimit,
                explanation => Unknown::Solver(format!("{explanation:?}")),
            };
            Status::Unknown(reason)
        })
    }

    fn model(&mut self, term: &Term) -> Result<String, Error> {
        let value = self.solver.get_value(term.clone()).map_err(native_error)?;
        if !value.is_bv_value() {
            return Err(adapter_error("model projection is not a bit-vector value"));
        }
        // Binary preserves the declared width even for non-multiples of four.
        let bits = value.bv_value(2).map_err(native_error)?;
        Ok(format!("#b{bits}"))
    }
}

pub(crate) fn solve(assertions: &[Bool], value: Option<&Bv>, rlimit: u32) -> Outcome {
    match Cvc5::new(rlimit) {
        Ok(backend) => backend::execute(assertions, value, backend),
        Err(error) => Outcome::Error(error),
    }
}

#[cfg(test)]
mod tests;
