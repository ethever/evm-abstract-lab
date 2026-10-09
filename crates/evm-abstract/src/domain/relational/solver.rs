//! Shared EVM bit-vector encoding over the provider-independent SMT layer.
//! Every check owns a fresh native session; EVM arithmetic semantics live here.

use super::{CheckResult, Constraint, QueryReason, RelationLimits, RelationState, ValueQuery};
use crate::domain::symbolic::{ExprId, ExprKind};
use alloy_primitives::U256;
use embedded_smt::{Bool, Bv as BV, Context, Outcome, Unknown};
use revm_bytecode::opcode;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

struct Encoder<'a> {
    limits: &'a RelationLimits,
    symbols: Context,
    work: usize,
    memo: BTreeMap<ExprId, BV>,
    definitions: Vec<Bool>,
    emitted_definitions: usize,
}
impl<'a> Encoder<'a> {
    fn new(limits: &'a RelationLimits) -> Self {
        Self {
            limits,
            symbols: Context::default(),
            work: 0,
            memo: BTreeMap::new(),
            definitions: Vec::new(),
            emitted_definitions: 0,
        }
    }
    fn charge(&mut self, work: usize) -> Result<(), QueryReason> {
        self.work = self.work.saturating_add(work);
        if self.work > self.limits.max_nodes {
            Err(QueryReason::ExpressionLimit)
        } else {
            Ok(())
        }
    }
    fn bind(&mut self, value: BV) -> BV {
        let variable = self.symbols.fresh_bv("evm_intermediate", 256);
        self.definitions.push(variable.eq(&value));
        variable
    }
    fn emit_definitions(&mut self, assertions: &mut Vec<Bool>) {
        for definition in &self.definitions[self.emitted_definitions..] {
            assertions.push(definition.clone());
        }
        self.emitted_definitions = self.definitions.len();
    }
    fn bind_observed_constants(
        &mut self,
        state: &RelationState,
        assertions: &mut Vec<Bool>,
    ) -> Result<(), QueryReason> {
        for constraint in &state.constraints {
            let Constraint::Equal { left, right } = constraint else {
                continue;
            };
            let (variable, value) = if let Some(value) = right.as_constant() {
                (left, value)
            } else if let Some(value) = left.as_constant() {
                (right, value)
            } else {
                continue;
            };
            if !matches!(variable.kind(), ExprKind::Input(_) | ExprKind::Fresh(_)) {
                continue;
            }
            if let ExprKind::Input(input) = variable.kind()
                && matches!(
                    input.symbol,
                    crate::domain::identity::Symbol::To
                        | crate::domain::identity::Symbol::Caller
                        | crate::domain::identity::Symbol::Origin
                        | crate::domain::identity::Symbol::Coinbase
                )
                && value > U256::MAX >> 96
            {
                assertions.push(Bool::from_bool(false));
            }
            self.charge(1)?;
            let constant = word(value);
            if let Some(previous) = self.memo.get(variable) {
                assertions.push(previous.eq(&constant));
            }
            self.memo.insert(variable.clone(), constant);
        }
        Ok(())
    }
    fn affine_result(&mut self, op: u8, args: &[ExprId]) -> Result<Option<BV>, QueryReason> {
        if !matches!(op, opcode::ADD | opcode::SUB) {
            return Ok(None);
        }
        let variable = if args[0].as_constant().is_some() {
            &args[1]
        } else if args[1].as_constant().is_some() {
            &args[0]
        } else {
            return Ok(None);
        };
        if self.memo.contains_key(variable) {
            return Ok(None);
        }
        let full_word = matches!(variable.kind(), ExprKind::Fresh(_))
            || matches!(variable.kind(),ExprKind::Input(input) if !matches!(input.symbol,crate::domain::identity::Symbol::To | crate::domain::identity::Symbol::Caller | crate::domain::identity::Symbol::Origin | crate::domain::identity::Symbol::Coinbase));
        if !full_word {
            return Ok(None);
        }
        // ADD/SUB by a constant is a bijection on Word256. Bind the guarded
        // result and express its free input by the exact modular inverse.
        self.charge(3)?;
        let result = self.symbols.fresh_bv("evm_affine_result", 256);
        let inverse = if let Some(constant) = args[1].as_constant() {
            if op == opcode::ADD {
                result.bvsub(word(constant))
            } else {
                result.bvadd(word(constant))
            }
        } else {
            let constant = args[0].as_constant().expect("constant operand");
            if op == opcode::ADD {
                result.bvsub(word(constant))
            } else {
                word(constant).bvsub(&result)
            }
        };
        self.memo.insert(variable.clone(), inverse);
        Ok(Some(result))
    }
    fn expression(&mut self, expression: &ExprId) -> Result<BV, QueryReason> {
        if expression.nodes() > self.limits.max_nodes || expression.depth() > self.limits.max_depth
        {
            return Err(QueryReason::ExpressionLimit);
        }
        if let Some(ast) = self.memo.get(expression) {
            return Ok(ast.clone());
        }
        self.charge(1)?;
        let value = match expression.kind() {
            ExprKind::Constant(value) => word(*value),
            ExprKind::Input(input) => match input.symbol {
                crate::domain::identity::Symbol::To
                | crate::domain::identity::Symbol::Caller
                | crate::domain::identity::Symbol::Origin
                | crate::domain::identity::Symbol::Coinbase => {
                    self.symbols.fresh_bv("evm_address", 160).zero_ext(96)
                }
                _ => self.symbols.fresh_bv("evm_input", 256),
            },
            ExprKind::Fresh(_) => self.symbols.fresh_bv("evm_runtime", 256),
            ExprKind::Operation { opcode: op, args } => {
                if let Some(result) = self.affine_result(*op, args)? {
                    result
                } else {
                    let exponent = args.get(1).and_then(ExprId::as_constant);
                    let args = args
                        .iter()
                        .map(|arg| self.expression(arg))
                        .collect::<Result<Vec<_>, _>>()?;
                    self.operation(*op, &args, exponent)?
                }
            }
        };
        self.memo.insert(expression.clone(), value.clone());
        Ok(value)
    }
    fn operation(
        &mut self,
        op: u8,
        args: &[BV],
        exponent: Option<U256>,
    ) -> Result<BV, QueryReason> {
        let a = &args[0];
        let b = args.get(1);
        let zero = word(U256::ZERO);
        let result = match op {
            opcode::ADD => a.bvadd(b.expect("validated arity")),
            opcode::MUL => a.bvmul(b.expect("validated arity")),
            opcode::SUB => a.bvsub(b.expect("validated arity")),
            opcode::DIV | opcode::SDIV | opcode::MOD | opcode::SMOD => {
                let b = b.expect("validated arity");
                let quotient = match op {
                    opcode::DIV => a.bvudiv(b),
                    opcode::SDIV => a.bvsdiv(b),
                    opcode::MOD => a.bvurem(b),
                    _ => a.bvsrem(b),
                };
                b.eq(&zero).ite(&zero, &quotient)
            }
            opcode::ADDMOD | opcode::MULMOD => {
                // Preserve the full mathematical intermediate, unlike 256-bit
                // ADD/MUL. A 512-bit product and 257-bit sum both fit here.
                let modulus = args[2].zero_ext(256);
                let a = a.zero_ext(256);
                let b = b.expect("validated arity").zero_ext(256);
                let wide = if op == opcode::ADDMOD {
                    a.bvadd(&b)
                } else {
                    a.bvmul(&b)
                };
                args[2]
                    .eq(&zero)
                    .ite(&zero, &wide.bvurem(&modulus).extract(255, 0))
            }
            opcode::LT => boolean(a.bvult(b.expect("validated arity"))),
            opcode::GT => boolean(a.bvugt(b.expect("validated arity"))),
            opcode::SLT => boolean(a.bvslt(b.expect("validated arity"))),
            opcode::SGT => boolean(a.bvsgt(b.expect("validated arity"))),
            opcode::EQ => boolean(a.eq(b.expect("validated arity"))),
            opcode::ISZERO => boolean(a.eq(&zero)),
            opcode::AND => a.bvand(b.expect("validated arity")),
            opcode::OR => a.bvor(b.expect("validated arity")),
            opcode::XOR => a.bvxor(b.expect("validated arity")),
            opcode::NOT => a.bvnot(),
            // SMT shifts use unsigned shift amounts of the same width, and
            // define oversized logical/arithmetic shifts exactly as EVM does.
            opcode::SHL => b.expect("validated arity").bvshl(a),
            opcode::SHR => b.expect("validated arity").bvlshr(a),
            opcode::SAR => b.expect("validated arity").bvashr(a),
            opcode::BYTE => {
                let offset = word(U256::from(31)).bvsub(a).bvmul(word(U256::from(8)));
                let byte = b
                    .expect("validated arity")
                    .bvlshr(&offset)
                    .bvand(word(U256::from(255)));
                a.bvuge(word(U256::from(32))).ite(&zero, &byte)
            }
            opcode::SIGNEXTEND => {
                let b = b.expect("validated arity");
                let bit = a.bvmul(word(U256::from(8))).bvadd(word(U256::from(7)));
                let mask = word(U256::from(1))
                    .bvshl(bit.bvadd(word(U256::from(1))))
                    .bvsub(word(U256::from(1)));
                let negative = b
                    .bvlshr(&bit)
                    .bvand(word(U256::from(1)))
                    .eq(word(U256::from(1)));
                let extended = negative.ite(&b.bvor(mask.bvnot()), &b.bvand(&mask));
                a.bvuge(word(U256::from(32))).ite(b, &extended)
            }
            opcode::CLZ => {
                self.charge(256)?;
                let mut count = word(U256::from(256));
                for bit in 0..256 {
                    count = a
                        .extract(bit, bit)
                        .eq(BV::from_u64(1, 1))
                        .ite(&word(U256::from(255 - bit)), &count);
                }
                count
            }
            opcode::EXP => {
                let exponent = exponent.or_else(|| b.and_then(literal));
                if let (Some(base), Some(exponent)) = (literal(a), exponent) {
                    self.charge(256)?;
                    return Ok(word(base.wrapping_pow(exponent)));
                }
                let mut result = word(U256::from(1));
                let mut power = a.clone();
                if let Some(exponent) = exponent {
                    let bits = 256 - exponent.leading_zeros();
                    self.charge(
                        bits.saturating_sub(1)
                            .saturating_add(exponent.count_ones())
                            .saturating_mul(3),
                    )?;
                    let mut initialized = false;
                    for bit in 0..bits {
                        if exponent.bit(bit) {
                            result = if initialized {
                                self.bind(result.bvmul(&power))
                            } else {
                                initialized = true;
                                power.clone()
                            };
                        }
                        if bit + 1 < bits {
                            power = self.bind(power.bvmul(&power));
                        }
                    }
                } else {
                    // Definitions keep every multiplication shallow. A deeply
                    // nested product DAG can make native model evaluation expand
                    // exponentially outside solver fuel; no such term escapes.
                    self.charge(256usize.saturating_mul(9))?;
                    let exponent = b.expect("validated arity");
                    for bit in 0..256 {
                        let selected = exponent.extract(bit, bit).eq(BV::from_u64(1, 1));
                        result = self.bind(selected.ite(&result.bvmul(&power), &result));
                        if bit + 1 < 256 {
                            power = self.bind(power.bvmul(&power));
                        }
                    }
                }
                result
            }
            _ => return Err(QueryReason::Unsupported(op)),
        };
        // Naming compound arithmetic exposes guarded results as independent
        // SSA-like solver variables. In y=x+1, y<10, the solver may eliminate
        // the unconstrained x instead of bit-blasting a full-width adder inside
        // every range/mask guard. Definitions preserve exact Word256 semantics.
        if matches!(
            op,
            opcode::ADD
                | opcode::SUB
                | opcode::MUL
                | opcode::DIV
                | opcode::SDIV
                | opcode::MOD
                | opcode::SMOD
                | opcode::ADDMOD
                | opcode::MULMOD
        ) {
            self.charge(2)?;
            Ok(self.bind(result))
        } else {
            Ok(result)
        }
    }
    fn truth(&mut self, expression: &ExprId) -> Result<Bool, QueryReason> {
        if expression.nodes() > self.limits.max_nodes || expression.depth() > self.limits.max_depth
        {
            return Err(QueryReason::ExpressionLimit);
        }
        if let ExprKind::Operation { opcode: op, args } = expression.kind()
            && matches!(
                *op,
                opcode::EQ | opcode::LT | opcode::GT | opcode::SLT | opcode::SGT | opcode::ISZERO
            )
        {
            self.charge(1)?;
            let left = self.expression(&args[0])?;
            return Ok(match *op {
                opcode::ISZERO => left.eq(word(U256::ZERO)),
                opcode::EQ => left.eq(self.expression(&args[1])?),
                opcode::LT => left.bvult(self.expression(&args[1])?),
                opcode::GT => left.bvugt(self.expression(&args[1])?),
                opcode::SLT => left.bvslt(self.expression(&args[1])?),
                _ => left.bvsgt(self.expression(&args[1])?),
            });
        }
        Ok(self.expression(expression)?.eq(word(U256::ZERO)).not())
    }
    fn constraint(&mut self, constraint: &Constraint) -> Result<Bool, QueryReason> {
        self.charge(1)?;
        Ok(match constraint {
            Constraint::Truth {
                expression,
                nonzero,
            } => {
                let truth = self.truth(expression)?;
                if *nonzero { truth } else { truth.not() }
            }
            Constraint::Range {
                expression,
                lower,
                upper,
            } => {
                let value = self.expression(expression)?;
                if lower == upper {
                    value.eq(word(*lower))
                } else {
                    Bool::and(&[value.bvuge(word(*lower)), value.bvule(word(*upper))])
                }
            }
            Constraint::Bits {
                expression,
                zero,
                one,
            } => {
                let value = self.expression(expression)?;
                Bool::and(&[
                    value.bvand(word(*zero)).eq(word(U256::ZERO)),
                    value.bvand(word(*one)).eq(word(*one)),
                ])
            }
            Constraint::Members { expression, values } => {
                self.charge(values.len())?;
                let value = self.expression(expression)?;
                Bool::or(
                    &values
                        .iter()
                        .map(|candidate| value.eq(word(*candidate)))
                        .collect::<Vec<_>>(),
                )
            }
            Constraint::Equal { left, right } => self.expression(left)?.eq(self.expression(right)?),
        })
    }
}
fn word(value: U256) -> BV {
    BV::from_str(256, &value.to_string()).expect("valid unsigned decimal word")
}
fn literal(value: &BV) -> Option<U256> {
    U256::from_str_radix(value.literal_hex()?, 16).ok()
}
fn boolean(value: Bool) -> BV {
    value.ite(&word(U256::from(1)), &word(U256::ZERO))
}
fn prepared<'a>(
    state: &RelationState,
    extra: Option<Constraint>,
    limits: &'a RelationLimits,
) -> Result<(Vec<Bool>, Encoder<'a>), QueryReason> {
    if !limits.enabled {
        return Err(QueryReason::Disabled);
    }
    if !limits.valid() {
        return Err(QueryReason::Configuration);
    }
    if state.constraints.len() > limits.max_constraints {
        return Err(QueryReason::ConstraintLimit);
    }
    let mut assertions = Vec::new();
    let mut encoder = Encoder::new(limits);
    encoder.bind_observed_constants(state, &mut assertions)?;
    for constraint in &state.constraints {
        assertions.push(encoder.constraint(constraint)?);
    }
    if let Some(extra) = extra {
        assertions.push(encoder.constraint(&extra)?);
    }
    encoder.emit_definitions(&mut assertions);
    Ok((assertions, encoder))
}

fn unknown(reason: Unknown, provider: super::SmtProvider) -> QueryReason {
    match reason {
        Unknown::Cancelled => QueryReason::Cancelled,
        Unknown::ResourceLimit => QueryReason::ResourceLimit,
        Unknown::Solver(reason) => QueryReason::SolverUnknown { provider, reason },
    }
}

fn checked(assertions: &[Bool], limits: &RelationLimits) -> CheckResult {
    match embedded_smt::check(assertions, None, limits.provider, limits.rlimit) {
        Outcome::Sat(_) => CheckResult::Sat,
        Outcome::Unsat => CheckResult::Unsat,
        Outcome::Unknown(reason) => CheckResult::Unknown(unknown(reason, limits.provider)),
        Outcome::Error(error) => CheckResult::Unknown(QueryReason::SolverError(error)),
    }
}

pub(super) fn check(
    state: &RelationState,
    extra: Option<Constraint>,
    limits: &RelationLimits,
) -> CheckResult {
    if state.bottom {
        return CheckResult::Unsat;
    }
    let (assertions, _) = match prepared(state, extra, limits) {
        Ok(query) => query,
        Err(reason) => return CheckResult::Unknown(reason),
    };
    checked(&assertions, limits)
}

pub(super) fn unique(
    state: &RelationState,
    expression: &ExprId,
    limits: &RelationLimits,
) -> ValueQuery {
    if state.bottom {
        return ValueQuery::Infeasible;
    }
    if matches!(expression.kind(), ExprKind::Input(_) | ExprKind::Fresh(_))
        && !state.relevant(expression)
    {
        return match check(state, None, limits) {
            CheckResult::Sat => ValueQuery::Multiple,
            CheckResult::Unsat => ValueQuery::Infeasible,
            CheckResult::Unknown(reason) => ValueQuery::Unknown(reason),
        };
    }
    let (mut assertions, mut encoder) = match prepared(state, None, limits) {
        Ok(query) => query,
        Err(reason) => return ValueQuery::Unknown(reason),
    };
    let value = match encoder.expression(expression) {
        Ok(value) => value,
        Err(reason) => return ValueQuery::Unknown(reason),
    };
    encoder.emit_definitions(&mut assertions);
    let printed =
        match embedded_smt::check(&assertions, Some(&value), limits.provider, limits.rlimit) {
            Outcome::Sat(Some(value)) => value,
            Outcome::Sat(None) => return ValueQuery::Unknown(QueryReason::ModelUnavailable),
            Outcome::Unsat => return ValueQuery::Infeasible,
            Outcome::Unknown(reason) => {
                return ValueQuery::Unknown(unknown(reason, limits.provider));
            }
            Outcome::Error(error) => {
                return ValueQuery::Unknown(QueryReason::SolverError(error));
            }
        };
    let constant = if let Some(hex) = printed.strip_prefix("#x") {
        U256::from_str_radix(hex, 16).ok()
    } else if let Some(binary) = printed.strip_prefix("#b") {
        U256::from_str_radix(binary, 2).ok()
    } else {
        None
    };
    let Some(constant) = constant else {
        return ValueQuery::Unknown(QueryReason::ModelUnavailable);
    };
    assertions.push(value.eq(word(constant)).not());
    // A second independent request keeps the same neutral terms and allowance.
    // It cannot accidentally switch a reused Z3 solver into another strategy.
    match checked(&assertions, limits) {
        CheckResult::Unsat => ValueQuery::Unique(constant),
        CheckResult::Sat => ValueQuery::Multiple,
        CheckResult::Unknown(reason) => ValueQuery::Unknown(reason),
    }
}
