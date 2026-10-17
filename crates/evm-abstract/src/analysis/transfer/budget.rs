//! Shared work accounting before domain, byte-range and join operations.
use super::Execution;
use super::address;
use crate::{
    analysis::ExecutionConfig,
    bytecode::Program,
    domain::{AbstractValue, Domain},
    world::{Code, Store},
};
use revm_bytecode::opcode;

pub(super) fn arithmetic_work(args: &[AbstractValue]) -> usize {
    args.iter()
        .try_fold(1usize, |size, value| {
            value.constants().map(|set| size.saturating_mul(set.len()))
        })
        .unwrap_or(1)
}

pub(super) fn maximum(value: &AbstractValue) -> usize {
    value
        .constants()
        .and_then(|values| values.iter().max())
        .and_then(|v| usize::try_from(*v).ok())
        .unwrap_or(0)
}

// A conservative precharge includes finite domain combinations, byte visits,
// and copying/joining sparse arrays. It happens before an allocating operation.
pub(super) fn byte_work(
    args: &[&AbstractValue],
    bytes: usize,
    existing: usize,
    domain: Domain,
) -> usize {
    let branches = args.iter().fold(1usize, |cost, value| {
        cost.saturating_mul(value.constants().map_or(1, |set| set.len()))
    });
    branches.saturating_mul(
        bytes
            .saturating_mul(
                domain
                    .capacity()
                    .saturating_mul(domain.capacity())
                    .saturating_add(1),
            )
            .saturating_add(existing.saturating_mul(2))
            .saturating_add(1024),
    )
}

pub(super) fn operation_work(
    result: &Execution,
    op: u8,
    args: &[AbstractValue],
    program: &Program,
    domain: Domain,
    config: &ExecutionConfig,
    entry: &crate::world::Entry,
) -> usize {
    let frame = result.payload.active();
    let environment = &entry.environment;
    if matches!(
        op,
        opcode::RETURN | opcode::REVERT | opcode::LOG0..=opcode::LOG4
    ) {
        // Slice retains the graph and translates only certified presentation
        // coordinates. Its cheap precharge does not census unrelated arrays.
        return frame.memory.slice_work(&args[0], &args[1], domain);
    }
    if matches!(
        op,
        opcode::CALLDATACOPY | opcode::CODECOPY | opcode::RETURNDATACOPY | opcode::MCOPY
    ) {
        // Quote from cached counts before any whole-array/root census. COPY
        // retains its complete graph but no longer refreshes every coordinate.
        return if op == opcode::CODECOPY {
            frame
                .memory
                .copy_code_work(program.byte_len(), 1, &args[0], &args[1], &args[2], domain)
        } else {
            let source = match op {
                opcode::CALLDATACOPY => &frame.calldata,
                opcode::RETURNDATACOPY => &frame.returndata,
                _ => &frame.memory,
            };
            frame
                .memory
                .copy_work(source, &args[0], &args[1], &args[2], domain)
        };
    }
    if op == opcode::EXTCODECOPY {
        let (bytes, accounts) = external_code_shape(&result.payload.store, &args[0], entry);
        return frame
            .memory
            .copy_code_work(bytes, accounts, &args[1], &args[2], &args[3], domain)
            .saturating_add(external_code_fee(bytes, accounts, domain, true));
    }
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
        opcode::BLOCKHASH | opcode::BLOBHASH => environment
            .work_size()
            .saturating_add(args[0].work_size())
            .saturating_add(
                domain.operation_work(&[]).saturating_mul(
                    args[0]
                        .constants()
                        .map_or(1, std::collections::BTreeSet::len),
                ),
            ),
        opcode::ADDRESS
        | opcode::CALLER
        | opcode::ORIGIN
        | opcode::CALLVALUE
        | opcode::GASPRICE
        | opcode::COINBASE
        | opcode::TIMESTAMP
        | opcode::NUMBER
        | opcode::DIFFICULTY
        | opcode::GASLIMIT
        | opcode::CHAINID
        | opcode::BASEFEE
        | opcode::BLOBBASEFEE
        | opcode::GAS => environment
            .work_size()
            .saturating_add(environment.projection_work(domain)),
        opcode::MLOAD => byte_work(&[&args[0]], 32, array_size, domain),
        opcode::MSTORE => byte_work(&args.iter().collect::<Vec<_>>(), 32, array_size, domain)
            .saturating_add(
                32usize.saturating_mul(domain.operation_work(&[
                    AbstractValue::constant(crate::U256::ZERO),
                    args[1].clone(),
                ])),
            ),
        opcode::MSTORE8 => byte_work(&args.iter().collect::<Vec<_>>(), 1, array_size, domain)
            .saturating_add(domain.operation_work(&[
                args[1].clone(),
                AbstractValue::constant(crate::U256::from(255)),
            ])),
        opcode::EXTCODEHASH => {
            external_code_work(&result.payload.store, &args[0], domain, false, entry)
        }
        opcode::EXTCODESIZE | opcode::BALANCE => args[0]
            .constants()
            .map_or(1, |values| values.len())
            .saturating_mul(domain.capacity())
            .saturating_add(1),
        opcode::CALLDATALOAD => byte_work(&[&args[0]], 32, array_size, domain),
        opcode::KECCAK256 => byte_work(&[&args[0], &args[1]], range_size(1), array_size, domain),
        opcode::SLOAD | opcode::SSTORE | opcode::TLOAD | opcode::TSTORE => result
            .payload
            .store
            .work_size()
            .saturating_mul(domain.capacity())
            .saturating_add(arithmetic_work(args)),
        opcode::SELFDESTRUCT => result
            .payload
            .store
            .work_size()
            .saturating_add(domain.operation_work(&[]).saturating_mul(2)),
        0x01..=0x0b | 0x10..=0x1e => domain.operation_work(args),
        _ => 1,
    }
}

fn external_code_work(
    store: &Store,
    targets: &AbstractValue,
    domain: Domain,
    copied: bool,
    entry: &crate::world::Entry,
) -> usize {
    let (bytes, accounts) = external_code_shape(store, targets, entry);
    external_code_fee(bytes, accounts, domain, copied)
}

fn external_code_shape(
    store: &Store,
    targets: &AbstractValue,
    entry: &crate::world::Entry,
) -> (usize, usize) {
    let actual;
    let targets = if entry.environment.to.as_concrete().is_none()
        && targets.identity().same_identity(
            entry
                .environment
                .address_value(entry.environment.to)
                .identity(),
        ) {
        actual = AbstractValue::constant(crate::U256::from_be_slice(entry.address.as_slice()));
        &actual
    } else {
        targets
    };
    let Some(targets) = targets.constants() else {
        return (0, 0);
    };
    let source_bytes = targets.iter().fold(0usize, |cost, target| {
        let size = match store.code(address(*target)) {
            Some(Code::Runtime(program)) => program.byte_len(),
            Some(Code::Delegation(_)) => 23,
            _ => 0,
        };
        cost.saturating_add(size)
    });
    (source_bytes, targets.len())
}

fn external_code_fee(source_bytes: usize, accounts: usize, domain: Domain, copied: bool) -> usize {
    // Copy constructs source byte maps and joins possible accounts; hash visits
    // every source byte even if no memory output is requested.
    let factor = if copied {
        accounts
            .saturating_mul(domain.capacity())
            .saturating_mul(domain.capacity())
            .saturating_add(2)
    } else {
        2
    };
    source_bytes
        .saturating_mul(factor)
        .saturating_add(accounts.saturating_mul(domain.capacity()).saturating_add(1))
}
