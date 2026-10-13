//! Bitwuzla primitives through the session-owned safe native adapter.

use crate::{
    Bool, Bv, Error, Outcome, Provider, Unknown,
    backend::{self, Backend, Failure, Status},
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
impl Bitwuzla {
    fn make_primitive(&mut self, node: &Node, args: &[Term]) -> Result<Term, Error> {
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
                    Sort::Array {
                        index_width,
                        element_width,
                    } => {
                        return self
                            .0
                            .array_variable(index_width, element_width, name)
                            .map_err(error);
                    }
                };
                return self.0.variable(width, name).map_err(error);
            }
            Op::ConstantArray => {
                let Sort::Array { index_width, .. } = node.sort else {
                    unreachable!("typed constant array")
                };
                return self.0.constant_array(index_width, &args[0]).map_err(error);
            }
            Op::Select => Kind::Select,
            Op::Store => Kind::Store,
            Op::Apply(_) => {
                return Err(Error {
                    provider: Provider::Bitwuzla,
                    message: "function application bypassed congruence lowering".into(),
                });
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
}
impl Backend for Bitwuzla {
    type Term = Term;
    fn make(&mut self, node: &Node, args: &[Term]) -> Result<Term, Failure> {
        self.make_primitive(node, args).map_err(Failure::from)
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
        Ok(solver) => {
            let cancellation = crate::current_cancellation();
            if !cancellation.is_enabled() {
                return backend::execute(assertions, value, Bitwuzla(solver), rlimit);
            }
            let interrupt = match solver.interrupt_handle() {
                Ok(interrupt) => interrupt,
                Err(reason) => return Outcome::Error(error(reason)),
            };
            crate::cancellation::interruptible(
                &cancellation,
                move || interrupt.interrupt(),
                || backend::execute(assertions, value, Bitwuzla(solver), rlimit),
            )
        }
        Err(reason) => Outcome::Error(error(reason)),
    }
}
