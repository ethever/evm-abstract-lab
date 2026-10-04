//! Shared work accounting before domain, byte-range and join operations.
use super::Execution;
use super::address;
use crate::{
    analysis::ExecutionConfig,
    bytecode::Program,
    domain::{Domain, Value},
    world::{Code, World},
};
use revm_bytecode::opcode;

pub(in crate::analysis) struct WorkBudget {
    maximum: usize,
    consumed: usize,
    exhausted: bool,
}
impl WorkBudget {
    pub(in crate::analysis) fn new(maximum: usize) -> Self {
        Self {
            maximum,
            consumed: 0,
            exhausted: false,
        }
    }
    pub(in crate::analysis) fn charge(&mut self, amount: usize) -> bool {
        if amount > self.maximum.saturating_sub(self.consumed) {
            self.exhausted = true;
            return false;
        }
        self.consumed += amount;
        true
    }
    pub(in crate::analysis) fn exhausted(&self) -> bool {
        self.exhausted
    }
    pub(in crate::analysis) fn used(&self) -> usize {
        self.consumed
    }
}

pub(super) fn arithmetic_work(args: &[Value]) -> usize {
    args.iter()
        .try_fold(1usize, |size, value| {
            value.constants().map(|set| size.saturating_mul(set.len()))
        })
        .unwrap_or(1)
}

pub(super) fn maximum(value: &Value) -> usize {
    value
        .constants()
        .and_then(|values| values.iter().max())
        .and_then(|v| usize::try_from(*v).ok())
        .unwrap_or(0)
}

// A conservative precharge includes finite domain combinations, byte visits,
// and copying/joining sparse arrays. It happens before an allocating operation.
pub(super) fn byte_work(args: &[&Value], bytes: usize, existing: usize, domain: Domain) -> usize {
    let branches = args.iter().fold(1usize, |cost, value| {
        cost.saturating_mul(value.constants().map_or(1, |set| set.len()))
    });
    branches
        .saturating_mul(bytes.saturating_add(existing).saturating_add(1))
        .saturating_mul(
            domain
                .capacity()
                .saturating_mul(domain.capacity())
                .saturating_add(1),
        )
}

pub(super) fn operation_work(
    result: &Execution,
    op: u8,
    args: &[Value],
    program: &Program,
    domain: Domain,
    config: &ExecutionConfig,
    world: &World,
) -> usize {
    let frame = result.payload.active();
    let array_size = frame
        .memory
        .work_size()
        .saturating_add(frame.calldata.work_size())
        .saturating_add(frame.returndata.work_size());
    let range_size = |index: usize| {
        let size = maximum(&args[index]);
        if size <= config.max_memory_bytes {
            size
        } else {
            0
        }
    };
    match op {
        opcode::MLOAD | opcode::MSTORE => {
            byte_work(&args.iter().collect::<Vec<_>>(), 32, array_size, domain)
        }
        opcode::MSTORE8 => byte_work(&args.iter().collect::<Vec<_>>(), 1, array_size, domain),
        opcode::CALLDATACOPY | opcode::CODECOPY | opcode::RETURNDATACOPY | opcode::MCOPY => {
            byte_work(
                &args.iter().collect::<Vec<_>>(),
                range_size(2),
                array_size,
                domain,
            )
            .saturating_add(if op == opcode::CODECOPY {
                program.byte_len()
            } else {
                0
            })
        }
        opcode::EXTCODECOPY => byte_work(
            &args.iter().collect::<Vec<_>>(),
            range_size(3),
            array_size,
            domain,
        )
        .saturating_add(external_code_work(world, &args[0], domain, true)),
        opcode::EXTCODEHASH => external_code_work(world, &args[0], domain, false),
        opcode::EXTCODESIZE | opcode::BALANCE => args[0]
            .constants()
            .map_or(1, |values| values.len())
            .saturating_mul(domain.capacity())
            .saturating_add(1),
        opcode::CALLDATALOAD => byte_work(&[&args[0]], 32, array_size, domain),
        opcode::KECCAK256 | opcode::RETURN | opcode::REVERT | opcode::LOG0..=opcode::LOG4 => {
            byte_work(&[&args[0], &args[1]], range_size(1), array_size, domain)
        }
        opcode::SLOAD | opcode::SSTORE | opcode::TLOAD | opcode::TSTORE => result
            .payload
            .store
            .work_size()
            .saturating_mul(domain.capacity())
            .saturating_add(arithmetic_work(args)),
        0x01..=0x0b | 0x10..=0x1e => arithmetic_work(args),
        _ => 1,
    }
}

fn external_code_work(world: &World, targets: &Value, domain: Domain, copied: bool) -> usize {
    let Some(targets) = targets.constants() else {
        return 1;
    };
    let source_bytes = targets.iter().fold(0usize, |cost, target| {
        let size = match world.account(address(*target)).map(|account| &account.code) {
            Some(Code::Runtime(program)) => program.byte_len(),
            Some(Code::Delegation(_)) => 23,
            _ => 0,
        };
        cost.saturating_add(size)
    });
    // Copy constructs source byte maps and joins possible accounts; hash visits
    // every source byte even if no memory output is requested.
    let factor = if copied {
        targets
            .len()
            .saturating_mul(domain.capacity())
            .saturating_mul(domain.capacity())
            .saturating_add(2)
    } else {
        2
    };
    source_bytes.saturating_mul(factor).saturating_add(
        targets
            .len()
            .saturating_mul(domain.capacity())
            .saturating_add(1),
    )
}
