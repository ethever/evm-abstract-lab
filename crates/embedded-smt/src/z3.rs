//! Z3 adapter. The private context prevents caller timeout/cancel state leaking in.

use crate::{
    Bool, Bv, Error, Outcome, Provider, Unknown,
    backend::{self, Backend, Failure, Status},
    term::{Node, Op, Sort},
};
use ::z3::{
    Config, Params, SatResult, Solver, Sort as NativeSort, StatisticsValue,
    ast::{Array as NativeArray, BV, Bool as NativeBool},
};
use num_bigint::BigUint;

#[derive(Clone)]
enum Native {
    Bool(NativeBool),
    Bv(BV),
    Array(NativeArray),
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
    fn array(&self) -> &NativeArray {
        match self {
            Self::Array(value) => value,
            _ => unreachable!("typed array"),
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
impl Z3 {
    fn make_primitive(&mut self, node: &Node, args: &[Native]) -> Result<Native, Error> {
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
                    Sort::Array {
                        index_width,
                        element_width,
                    } => Native::Array(NativeArray::new_const(
                        name.as_str(),
                        &NativeSort::bitvector(index_width),
                        &NativeSort::bitvector(element_width),
                    )),
                }
            }
            Op::ConstantArray => {
                let Sort::Array { index_width, .. } = node.sort else {
                    return Err(error("constant array has a non-array sort"));
                };
                Native::Array(NativeArray::const_array(
                    &NativeSort::bitvector(index_width),
                    args[0].bv(),
                ))
            }
            Op::Select => Native::Bv(
                args[0]
                    .array()
                    .select(args[1].bv())
                    .as_bv()
                    .ok_or_else(|| error("array selection is not a bit-vector"))?,
            ),
            Op::Store => Native::Array(args[0].array().store(args[1].bv(), args[2].bv())),
            Op::Apply(_) => return Err(error("function application bypassed congruence lowering")),
            Op::Eq => Native::Bool(match &args[0] {
                Native::Bool(value) => value.eq(args[1].bool()),
                Native::Bv(value) => value.eq(args[1].bv()),
                Native::Array(value) => value.eq(args[1].array()),
            }),
            Op::Not => Native::Bool(args[0].bool().not()),
            Op::And => Native::Bool(NativeBool::and(
                &args.iter().map(Native::bool).collect::<Vec<_>>(),
            )),
            Op::Or => Native::Bool(NativeBool::or(
                &args.iter().map(Native::bool).collect::<Vec<_>>(),
            )),
            Op::Ite => match node.sort {
                Sort::Bool => Native::Bool(args[0].bool().ite(args[1].bool(), args[2].bool())),
                Sort::Bv(_) => Native::Bv(args[0].bool().ite(args[1].bv(), args[2].bv())),
                Sort::Array { .. } => {
                    Native::Array(args[0].bool().ite(args[1].array(), args[2].array()))
                }
            },
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
}
impl Backend for Z3 {
    type Term = Native;
    fn make(&mut self, node: &Node, args: &[Native]) -> Result<Native, Failure> {
        self.make_primitive(node, args).map_err(Failure::from)
    }
    fn assert(&mut self, term: &Native) -> Result<(), Error> {
        self.solver.assert(term.bool());
        Ok(())
    }
    fn check(&mut self) -> Result<Status, Error> {
        let before = resources(&self.solver);
        #[cfg(test)]
        crate::query_pair::observe_native();
        let result = self.solver.check();
        #[cfg(test)]
        crate::query_pair::observe_resources(self.rlimit, before, resources(&self.solver));
        Ok(match result {
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

fn solve_current(assertions: &[Bool], value: Option<&Bv>, rlimit: u32) -> Outcome {
    let solver = Solver::new_for_logic("QF_ABV").expect("Z3 supports QF_ABV");
    #[cfg(test)]
    crate::query_pair::observe_preparation();
    let mut params = Params::new();
    params.set_u32("rlimit", rlimit);
    solver.set_params(&params);
    let context = ::z3::Context::thread_local();
    let handle = context.handle();
    let cancellation = crate::current_cancellation();
    crate::cancellation::interruptible(
        &cancellation,
        move || handle.interrupt(),
        || backend::execute(assertions, value, Z3 { solver, rlimit }, rlimit),
    )
}

/// Native state belongs to one <=32 literal chunk. Common lowering's memo and
/// congruence table are gone before this object can push a candidate scope.
pub(crate) struct LiteralFamily {
    backend: Z3,
    target: BV,
}
impl LiteralFamily {
    fn prepare(base: &[Bool], value: &Bv, after: &[Bool], rlimit: u32) -> Result<Self, Failure> {
        backend::checkpoint()?;
        let solver = Solver::new_for_logic("QF_ABV").expect("Z3 supports QF_ABV");
        #[cfg(test)]
        crate::query_pair::observe_preparation();
        let mut params = Params::new();
        params.set_u32("rlimit", rlimit);
        solver.set_params(&params);
        let mut backend = Z3 { solver, rlimit };
        // One original congruence allowance, never allowance * family size.
        let target = backend::prepare_literal_family(base, value, after, &mut backend, rlimit)?;
        Ok(Self {
            backend,
            target: target.bv().clone(),
        })
    }

    fn check(&mut self, candidate: &Bv, rlimit: u32) -> Result<Outcome, Failure> {
        backend::checkpoint()?;
        if candidate.width() != self.target.get_size()
            || !matches!(candidate.0.0.op, Op::Literal(_))
        {
            return Err(error("literal family requires a same-width syntactic literal").into());
        }
        // No DAG lowering or memo survives pop: this primitive cannot introduce
        // UF applications or common definitions. Native ASTs own their nodes.
        let literal = self.backend.make_primitive(&candidate.0.0, &[])?;
        backend::checkpoint()?;
        let equality = self.target.eq(literal.bv());
        backend::checkpoint()?;
        self.backend.solver.push();
        let result = (|| {
            backend::checkpoint()?;
            self.backend.solver.assert(&equality);
            let mut params = Params::new();
            params.set_u32("rlimit", rlimit);
            self.backend.solver.set_params(&params);
            self.backend.rlimit = rlimit;
            backend::checkpoint()?;
            let status = self.backend.check()?;
            backend::checkpoint()?;
            Ok(match status {
                Status::Sat => Outcome::Sat(None),
                Status::Unsat => Outcome::Unsat,
                Status::Unknown(reason) => Outcome::Unknown(reason),
            })
        })();
        self.backend.solver.pop(1);
        result
    }
}

pub(crate) fn solve_literal_candidate(
    family: &mut Option<LiteralFamily>,
    base: &[Bool],
    value: &Bv,
    after: &[Bool],
    candidate: &Bv,
    rlimit: u32,
) -> Outcome {
    let context = ::z3::Context::thread_local();
    let handle = context.handle();
    let cancellation = crate::current_cancellation();
    crate::cancellation::interruptible(
        &cancellation,
        move || handle.interrupt(),
        || {
            let result = (|| {
                backend::checkpoint()?;
                if family.is_none() {
                    *family = Some(LiteralFamily::prepare(base, value, after, rlimit)?);
                }
                family
                    .as_mut()
                    .expect("prepared literal family")
                    .check(candidate, rlimit)
            })();
            result.unwrap_or_else(Failure::outcome)
        },
    )
}

pub(crate) fn solve_in_context(
    context: &::z3::Context,
    assertions: &[Bool],
    value: Option<&Bv>,
    rlimit: u32,
) -> Outcome {
    ::z3::with_z3_context(context, || solve_current(assertions, value, rlimit))
}

pub(crate) fn solve(assertions: &[Bool], value: Option<&Bv>, rlimit: u32) -> Outcome {
    ::z3::with_z3_config(&Config::new(), || solve_current(assertions, value, rlimit))
}
