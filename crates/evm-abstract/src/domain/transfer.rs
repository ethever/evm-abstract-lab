//! 运算关系和可信复制事实通过同一语义语言进入 transfer；
//! 同名来源和相同抽象摘要从不产生 Eq。
use super::{
    Domain, Reduction, Value,
    congruence::Congruence,
    facts::{
        BinaryFact, BinaryPredicate, Fact, FactLattice, OperationRelation, RelationalFact, Symbol,
        Term,
    },
    interval::Interval,
    known_bits::KnownBits,
    provenance::Provenance,
    reduce,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) fn apply(domain: Domain, op: u8, args: &[Value]) -> Reduction {
    let operands = (0..args.len())
        .map(|i| Term::Symbol(Symbol::new(i as u32 + 1)))
        .collect::<Vec<_>>();
    let relation = OperationRelation::new(op, Symbol::THIS, operands)
        .expect("facade validates pure operation and arity");
    let mut facts = FactLattice::new(domain.spec().fact_limit());
    let mut relation_available = facts
        .insert(Fact::Relation(RelationalFact::Operation(relation.clone())))
        .is_ok();
    if args.len() > 1 && args[0].provenance.same_identity(&args[1].provenance) {
        relation_available &= facts
            .insert(Fact::Binary(BinaryFact::new(
                Term::Symbol(Symbol::new(1)),
                BinaryPredicate::Eq,
                Term::Symbol(Symbol::new(2)),
            )))
            .is_ok();
    }
    let provenance = Provenance::transfer(
        &args
            .iter()
            .map(|v| v.provenance.clone())
            .collect::<Vec<_>>(),
    );
    // 只有交换表中的可信 Eq 支持相关值规则，表容量不足则回到非关系 transfer。
    if relation_available && args.len() > 1 && facts.equivalent(Symbol::new(1), Symbol::new(2)) {
        let exact = match relation.opcode() {
            opcode::XOR | opcode::SUB | opcode::LT | opcode::GT | opcode::SLT | opcode::SGT => {
                Some(U256::ZERO)
            }
            opcode::EQ => Some(U256::from(1)),
            _ => None,
        };
        if let Some(exact) = exact {
            let mut value = Value::constant(exact);
            value.provenance = provenance;
            return Reduction::unchanged(value);
        }
    }
    if relation.opcode() == opcode::ISZERO && (!args[0].may_be_zero() || !args[0].may_be_nonzero())
    {
        let mut value = Value::constant(U256::from(u8::from(!args[0].may_be_nonzero())));
        value.provenance = provenance;
        return Reduction::unchanged(value);
    }
    if let Some(exact) = args
        .iter()
        .map(Value::singleton)
        .collect::<Option<Vec<_>>>()
    {
        let mut value = Value::constant(super::evaluate(
            relation.opcode(),
            exact[0],
            exact.get(1).copied().unwrap_or(U256::ZERO),
            exact.get(2).copied().unwrap_or(U256::ZERO),
        ));
        value.provenance = provenance;
        return Reduction::unchanged(value);
    }
    let mut finite_value = domain.finite_apply(relation.opcode(), args);
    if !finite_value.finite.is_top() && args.iter().all(|v| v.constants().is_some()) {
        // 完整枚举已给精确 scalar 集合；重新传播其等价摘要不会增加信息。
        finite_value.provenance = provenance;
        return Reduction::unchanged(finite_value);
    }
    let finite = finite_value.finite;
    let value = Value {
        finite,
        bits: KnownBits::transfer(
            relation.opcode(),
            &args.iter().map(|v| v.bits).collect::<Vec<_>>(),
        ),
        interval: Interval::transfer(
            relation.opcode(),
            &args.iter().map(|v| v.interval).collect::<Vec<_>>(),
        ),
        congruence: Congruence::transfer(
            relation.opcode(),
            &args
                .iter()
                .map(|v| v.congruence.clone())
                .collect::<Vec<_>>(),
        ),
        provenance,
        nonzero: false,
    };
    let mut reduced = reduce::reduce(domain, value);
    if !relation_available {
        reduced.status = super::ReductionStatus::FactLimit;
    }
    reduced
}
