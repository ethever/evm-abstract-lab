//! Z3 adapter. The private context prevents caller timeout/cancel state leaking in.

use crate::{
    Bool, Bv, Error, Outcome, Provider, Unknown,
    backend::{self, Backend, Status},
    term::{Node, Op, Sort},
};
use ::z3::{
    Config, Params, SatResult, Solver, StatisticsValue,
    ast::{BV, Bool as NativeBool},
};
use num_bigint::BigUint;

#[derive(Clone)]
enum Native {
    Bool(NativeBool),
    Bv(BV),
}
impl Native {
    fn bool(&self) -> &NativeBool {
        match self {
            Self::Bool(value) => value,
            _ => unreachable!("typed Boolean"),
        }
    }
    fn bv(&self) -> &BV {
        match self {
            Self::Bv(value) => value,
            _ => unreachable!("typed bit-vector"),
        }
    }
}
struct Z3 {
    solver: Solver,
    rlimit: u32,
}
fn error(message: impl Into<String>) -> Error {
    Error {
        provider: Provider::Z3,
        message: message.into(),
    }
}
fn resources(solver: &Solver) -> Option<u32> {
    match solver.get_statistics().value("rlimit count") {
        Some(StatisticsValue::UInt(value)) => Some(value),
        _ => None,
    }
}
impl Backend for Z3 {
    type Term = Native;
    fn make(&mut self, node: &Node, args: &[Native]) -> Result<Native, Error> {
        Ok(match &node.op {
            Op::Bool(value) => Native::Bool(NativeBool::from_bool(*value)),
            Op::Literal(hex) => {
                let Sort::Bv(width) = node.sort else {
                    return Err(error("literal is not a bit-vector"));
                };
                let decimal = BigUint::parse_bytes(hex.as_bytes(), 16)
                    .ok_or_else(|| error("invalid hexadecimal literal"))?
                    .to_str_radix(10);
                Native::Bv(
                    BV::from_str(width, &decimal)
                        .ok_or_else(|| error("invalid bit-vector literal"))?,
                )
            }
            Op::Variable(name) => {
                if name.contains('\0') {
                    return Err(error("symbol contains NUL"));
                }
                match node.sort {
                    Sort::Bool => Native::Bool(NativeBool::new_const(name.as_str())),
                    Sort::Bv(width) => Native::Bv(BV::new_const(name.as_str(), width)),
                }
            }
            Op::Eq => Native::Bool(args[0].bv().eq(args[1].bv())),
            Op::Not => Native::Bool(args[0].bool().not()),
            Op::And => Native::Bool(NativeBool::and(
                &args.iter().map(Native::bool).collect::<Vec<_>>(),
            )),
            Op::Or => Native::Bool(NativeBool::or(
                &args.iter().map(Native::bool).collect::<Vec<_>>(),
            )),
            Op::Ite => Native::Bv(args[0].bool().ite(args[1].bv(), args[2].bv())),
            Op::BvNot => Native::Bv(args[0].bv().bvnot()),
            Op::ZeroExtend(extra) => Native::Bv(args[0].bv().zero_ext(*extra)),
            Op::Extract(high, low) => Native::Bv(args[0].bv().extract(*high, *low)),
            Op::Add => Native::Bv(args[0].bv().bvadd(args[1].bv())),
            Op::Sub => Native::Bv(args[0].bv().bvsub(args[1].bv())),
            Op::Mul => Native::Bv(args[0].bv().bvmul(args[1].bv())),
            Op::Udiv => Native::Bv(args[0].bv().bvudiv(args[1].bv())),
            Op::Sdiv => Native::Bv(args[0].bv().bvsdiv(args[1].bv())),
            Op::Urem => Native::Bv(args[0].bv().bvurem(args[1].bv())),
            Op::Srem => Native::Bv(args[0].bv().bvsrem(args[1].bv())),
            Op::BvAnd => Native::Bv(args[0].bv().bvand(args[1].bv())),
            Op::BvOr => Native::Bv(args[0].bv().bvor(args[1].bv())),
            Op::BvXor => Native::Bv(args[0].bv().bvxor(args[1].bv())),
            Op::Shl => Native::Bv(args[0].bv().bvshl(args[1].bv())),
            Op::Lshr => Native::Bv(args[0].bv().bvlshr(args[1].bv())),
            Op::Ashr => Native::Bv(args[0].bv().bvashr(args[1].bv())),
            Op::Ult => Native::Bool(args[0].bv().bvult(args[1].bv())),
            Op::Ule => Native::Bool(args[0].bv().bvule(args[1].bv())),
            Op::Ugt => Native::Bool(args[0].bv().bvugt(args[1].bv())),
            Op::Uge => Native::Bool(args[0].bv().bvuge(args[1].bv())),
            Op::Slt => Native::Bool(args[0].bv().bvslt(args[1].bv())),
            Op::Sle => Native::Bool(args[0].bv().bvsle(args[1].bv())),
            Op::Sgt => Native::Bool(args[0].bv().bvsgt(args[1].bv())),
            Op::Sge => Native::Bool(args[0].bv().bvsge(args[1].bv())),
        })
    }
    fn assert(&mut self, term: &Native) -> Result<(), Error> {
        self.solver.assert(term.bool());
        Ok(())
    }
    fn check(&mut self) -> Result<Status, Error> {
        let before = resources(&self.solver);
        Ok(match self.solver.check() {
            SatResult::Sat => Status::Sat,
            SatResult::Unsat => Status::Unsat,
            SatResult::Unknown => {
                let reason = self
                    .solver
                    .get_reason_unknown()
                    .unwrap_or_else(|| "unknown".into());
                let exhausted = before
                    .zip(resources(&self.solver))
                    .and_then(|(before, after)| after.checked_sub(before))
                    .is_some_and(|used| used >= self.rlimit);
                Status::Unknown(
                    if exhausted || reason.contains("resource") || reason == "canceled" {
                        Unknown::ResourceLimit
                    } else {
                        Unknown::Solver(reason)
                    },
                )
            }
        })
    }
    fn model(&mut self, term: &Native) -> Result<String, Error> {
        self.solver
            .get_model()
            .and_then(|model| model.eval(term.bv(), true))
            .map(|value| value.to_string())
            .ok_or_else(|| error("model value unavailable"))
    }
}

pub(crate) fn solve(assertions: &[Bool], value: Option<&Bv>, rlimit: u32) -> Outcome {
    ::z3::with_z3_config(&Config::new(), || {
        let solver = Solver::new_for_logic("QF_BV").expect("Z3 supports QF_BV");
        let mut params = Params::new();
        params.set_u32("rlimit", rlimit);
        solver.set_params(&params);
        backend::execute(assertions, value, Z3 { solver, rlimit })
    })
}
