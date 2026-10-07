//! Bitwuzla primitives through the session-owned safe native adapter.

use crate::{
    Bool, Bv, Error, Outcome, Provider, Unknown,
    backend::{self, Backend, Status},
    term::{Node, Op, Sort},
};
use bitwuzla_native::{Kind, Solver, Term};

struct Bitwuzla(Solver);
fn error(error: bitwuzla_native::Error) -> Error {
    Error {
        provider: Provider::Bitwuzla,
        message: error.to_string(),
    }
}
impl Backend for Bitwuzla {
    type Term = Term;
    fn make(&mut self, node: &Node, args: &[Term]) -> Result<Term, Error> {
        let kind = match &node.op {
            Op::Bool(value) => return self.0.bool(*value).map_err(error),
            Op::Literal(hex) => {
                let Sort::Bv(width) = node.sort else {
                    unreachable!("typed bit-vector literal");
                };
                return self.0.bv(width, hex).map_err(error);
            }
            Op::Variable(name) => {
                let width = match node.sort {
                    Sort::Bool => None,
                    Sort::Bv(width) => Some(width),
                };
                return self.0.variable(width, name).map_err(error);
            }
            Op::ZeroExtend(extra) => {
                return self
                    .0
                    .apply(Kind::ZeroExtend, args, &[*extra])
                    .map_err(error);
            }
            Op::Extract(high, low) => {
                return self
                    .0
                    .apply(Kind::Extract, args, &[*high, *low])
                    .map_err(error);
            }
            Op::Eq => Kind::Eq,
            Op::Not => Kind::Not,
            Op::And => Kind::And,
            Op::Or => Kind::Or,
            Op::Ite => Kind::Ite,
            Op::Add => Kind::Add,
            Op::Sub => Kind::Sub,
            Op::Mul => Kind::Mul,
            Op::Udiv => Kind::Udiv,
            Op::Sdiv => Kind::Sdiv,
            Op::Urem => Kind::Urem,
            Op::Srem => Kind::Srem,
            Op::Ult => Kind::Ult,
            Op::Ule => Kind::Ule,
            Op::Ugt => Kind::Ugt,
            Op::Uge => Kind::Uge,
            Op::Slt => Kind::Slt,
            Op::Sle => Kind::Sle,
            Op::Sgt => Kind::Sgt,
            Op::Sge => Kind::Sge,
            Op::BvAnd => Kind::BvAnd,
            Op::BvOr => Kind::BvOr,
            Op::BvXor => Kind::BvXor,
            Op::BvNot => Kind::BvNot,
            Op::Shl => Kind::Shl,
            Op::Lshr => Kind::Lshr,
            Op::Ashr => Kind::Ashr,
        };
        self.0.apply(kind, args, &[]).map_err(error)
    }
    fn assert(&mut self, term: &Term) -> Result<(), Error> {
        self.0.assert(term).map_err(error)
    }
    fn check(&mut self) -> Result<Status, Error> {
        Ok(match self.0.check().map_err(error)? {
            bitwuzla_native::Status::Sat => Status::Sat,
            bitwuzla_native::Status::Unsat => Status::Unsat,
            bitwuzla_native::Status::Unknown {
                exhausted: true, ..
            } => Status::Unknown(Unknown::ResourceLimit),
            bitwuzla_native::Status::Unknown { polls, .. } => Status::Unknown(Unknown::Solver(
                format!("Bitwuzla returned unknown after {polls} termination checks"),
            )),
        })
    }
    fn model(&mut self, term: &Term) -> Result<String, Error> {
        self.0.model(term).map_err(error)
    }
}

pub(crate) fn solve(assertions: &[Bool], value: Option<&Bv>, rlimit: u32) -> Outcome {
    match Solver::new(rlimit) {
        Ok(solver) => backend::execute(assertions, value, Bitwuzla(solver)),
        Err(reason) => Outcome::Error(error(reason)),
    }
}
