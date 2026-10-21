//! Shared work accounting before domain, byte-range and join operations.

use super::EvmExecution;
use super::{
    address,
    candidates::{self, Candidates},
    query,
};
use crate::domain::{EvmNumericAlgebra, WordElement};

use crate::{
    analysis::{ExecutionConfig, FrontierReason},
    bytecode::Program,
    domain::{AlgebraRef, EvmValue, SingletonProof},
    resource::WorkBudget,
    world::{Code, EvmStore},
};
use revm_bytecode::opcode;

#[cfg(test)]
mod cost_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod uniform_tests;

/// Literal construction for quotation is a separate paid phase. A certified
/// fresh-word fee needs no temporary operands; other policies price real indices.
pub(super) fn quotation_work<D: EvmNumericAlgebra>(
    result: &EvmExecution<D::Element>,
    op: u8,
    args: &[EvmValue<D::Element>],
    domain: AlgebraRef<'_, D>,
) -> usize {
    match op {
        opcode::MSTORE
            if result
                .payload
                .active()
                .memory
                .uniform_word_write_work_with(&args[0], &args[1], domain)
                .is_some() =>
        {
            // The injected algebra owns this cached-only complete quote,
            // including all literal construction in the certified writer.
            0
        }
        opcode::MSTORE | opcode::MSTORE8
            if domain.memory_write_passes(
                &args[0],
                &args[1],
                if op == opcode::MSTORE { 32 } else { 1 },
            ) == 0 =>
        {
            0
        }
        opcode::MSTORE if domain.byte_quotes_uniform() => {
            domain.literal_quote_work(crate::U256::ZERO)
        }
        opcode::MSTORE => (0..32).fold(0usize, |work, index| {
            work.saturating_add(domain.literal_quote_work(crate::U256::from(index)))
        }),
        opcode::MSTORE8 => domain.literal_quote_work(crate::U256::from(255)),
        _ => 0,
    }
}

/// A fee and the word discoveries used to compute it. Dispatch reuses the
/// target cover; logical range admission reuses the unchanged size proof.
pub(super) struct OperationQuote<'a> {
    pub(super) work: usize,
    pub(super) targets: Option<Candidates<'a>>,
    pub(super) size: SingletonProof,
}

#[expect(
    clippy::too_many_arguments,
    reason = "same explicit metered operation inputs as the transfer dispatcher"
)]
/// Reserve discovery and temporary literal construction before the real quote.
/// Refusal retains prior payments and publishes no executable effects.
pub(super) fn operation_work_budgeted<'a, D: EvmNumericAlgebra>(
    result: &EvmExecution<D::Element>,
    op: u8,
    args: &'a [EvmValue<D::Element>],
    program: &Program,
    domain: AlgebraRef<'_, D>,
    config: &ExecutionConfig,
    entry: &crate::world::EvmEntry<D::Element>,
    budget: &mut WorkBudget,
) -> Result<OperationQuote<'a>, FrontierReason> {
    query::checkpoint(budget)?;
    let prepare = quotation_work(result, op, args, domain);
    query::checkpoint(budget)?;
    if prepare == usize::MAX || !budget.charge(prepare) {
        return Err(FrontierReason::Work);
    }
    query::checkpoint(budget)?;
    let (extent, size) = if op == opcode::KECCAK256 {
        let (values, proof) = candidates::prepare(&args[1], budget)?;
        (maximum(values.as_ref()), proof)
    } else {
        (0, SingletonProof::NotQueried)
    };
    let targets = if matches!(
        op,
        opcode::EXTCODECOPY | opcode::EXTCODEHASH | opcode::EXTCODESIZE | opcode::BALANCE
    ) {
        if candidates::is_self(&args[0], entry) {
            Some(Candidates::single(crate::U256::from_be_slice(
                entry.address.as_slice(),
            )))
        } else {
            candidates::prepare(&args[0], budget)?.0
        }
    } else {
        None
    };
    let work = operation_work_prepared(
        result,
        op,
        args,
        program,
        domain,
        config,
        extent,
        targets.as_ref(),
        entry,
    );
    query::checkpoint(budget)?;
    if work == usize::MAX {
        Err(FrontierReason::Work)
    } else {
        Ok(OperationQuote {
            work,
            targets,
            size,
        })
    }
}

pub(super) fn arithmetic_work<N: WordElement>(args: &[EvmValue<N>]) -> usize {
    args.iter().fold(1usize, |size, value| {
        size.saturating_mul(candidates::count(value))
    })
}

/// Words are already discovered or borrowed from a complete finite cover.
/// An unrepresentable known extent refuses; an unknown cover selects no scan.
pub(super) fn maximum(values: Option<&Candidates<'_>>) -> usize {
    values
        .and_then(|values| values.iter().max().copied())
        .map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX))
}

// A conservative precharge includes finite domain combinations, byte visits,
// and copying/joining sparse arrays. It happens before an allocating operation.
pub(super) fn byte_work<D: EvmNumericAlgebra>(
    args: &[&EvmValue<D::Element>],
    bytes: usize,
    existing: usize,
    domain: AlgebraRef<'_, D>,
) -> usize {
    let branches = args.iter().fold(1usize, |cost, value| {
        cost.saturating_mul(candidates::count(value))
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

// Pure/caller-funded test adapter. Production uses the prepared boundary above.
#[cfg(test)]
pub(super) fn operation_work<D: EvmNumericAlgebra>(
    result: &EvmExecution<D::Element>,
    op: u8,
    args: &[EvmValue<D::Element>],
    program: &Program,
    domain: AlgebraRef<'_, D>,
    config: &ExecutionConfig,
    entry: &crate::world::EvmEntry<D::Element>,
) -> usize {
    let extent = if op == opcode::KECCAK256 {
        maximum(candidates::complete(&args[1]).as_ref())
    } else {
        0
    };
    let targets = if matches!(
        op,
        opcode::EXTCODECOPY | opcode::EXTCODEHASH | opcode::EXTCODESIZE | opcode::BALANCE
    ) {
        if candidates::is_self(&args[0], entry) {
            Some(Candidates::single(crate::U256::from_be_slice(
                entry.address.as_slice(),
            )))
        } else {
            candidates::complete(&args[0])
        }
    } else {
        None
    };
    operation_work_prepared(
        result,
        op,
        args,
        program,
        domain,
        config,
        extent,
        targets.as_ref(),
        entry,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "operation inputs plus already-paid word-dependent quotation data"
)]
fn operation_work_prepared<D: EvmNumericAlgebra>(
    result: &EvmExecution<D::Element>,
    op: u8,
    args: &[EvmValue<D::Element>],
    program: &Program,
    domain: AlgebraRef<'_, D>,
    config: &ExecutionConfig,
    extent: usize,
    targets: Option<&Candidates<'_>>,
    entry: &crate::world::EvmEntry<D::Element>,
) -> usize {
    let frame = result.payload.active();
    let environment = &entry.environment;
    if matches!(
        op,
        opcode::RETURN | opcode::REVERT | opcode::LOG0..=opcode::LOG4
    ) {
        // Slice retains the graph and translates only certified presentation
        // coordinates. Its cheap precharge does not census unrelated arrays.
        return frame.memory.slice_work_with(&args[0], &args[1], domain);
    }
    if matches!(
        op,
        opcode::CALLDATACOPY | opcode::CODECOPY | opcode::RETURNDATACOPY | opcode::MCOPY
    ) {
        // Quote from cached counts before any whole-array/root census. COPY
        // retains its complete graph but no longer refreshes every coordinate.
        return if op == opcode::CODECOPY {
            frame.memory.copy_code_work_with(
                program.byte_len(),
                1,
                &args[0],
                &args[1],
                &args[2],
                domain,
            )
        } else {
            let source = match op {
                opcode::CALLDATACOPY => &frame.calldata,
                opcode::RETURNDATACOPY => &frame.returndata,
                _ => &frame.memory,
            };
            frame
                .memory
                .copy_work_with(source, &args[0], &args[1], &args[2], domain)
        };
    }
    if op == opcode::EXTCODECOPY {
        let (bytes, accounts) = external_code_shape(&result.payload.store, targets);
        return frame
            .memory
            .copy_code_work_with(bytes, accounts, &args[1], &args[2], &args[3], domain)
            .saturating_add(external_code_fee(bytes, accounts, domain, true));
    }
    if op == opcode::MSTORE
        && let Some(work) = frame
            .memory
            .uniform_word_write_work_with(&args[0], &args[1], domain)
    {
        return work;
    }
    let array_size = frame
        .memory
        .work_size()
        .saturating_add(frame.calldata.work_size())
        .saturating_add(frame.returndata.work_size());
    let range_size = || {
        if extent == usize::MAX {
            usize::MAX
        } else if extent <= config.max_memory_bytes {
            extent
        } else {
            0
        }
    };
    match op {
        opcode::BLOCKHASH | opcode::BLOBHASH => environment
            .work_size()
            .saturating_add(args[0].work_size())
            .saturating_add(
                domain
                    .operation_work(&[])
                    .saturating_mul(candidates::count(&args[0])),
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
            .saturating_add(environment.projection_work_with(domain)),
        opcode::MLOAD => byte_work(&[&args[0]], 32, array_size, domain),
        opcode::MSTORE => {
            let passes = domain.memory_write_passes(&args[0], &args[1], 32);
            if passes == 0 {
                return byte_work(&[&args[0], &args[1]], 32, array_size, domain);
            }
            let literals = (0..32).fold(0usize, |work, index| {
                work.saturating_add(domain.literal_quote_work(crate::U256::from(index)))
            });
            if literals == usize::MAX {
                return usize::MAX;
            }
            let operations = if domain.byte_quotes_uniform() {
                let index = EvmValue::<D::Element>::constant(crate::U256::ZERO);
                32usize.saturating_mul(
                    domain.operation_work_for_refs(opcode::BYTE, &[&index, &args[1]]),
                )
            } else {
                (0..32).fold(0usize, |work, index| {
                    let index = EvmValue::<D::Element>::constant(crate::U256::from(index));
                    work.saturating_add(
                        domain.operation_work_for_refs(opcode::BYTE, &[&index, &args[1]]),
                    )
                })
            };
            byte_work(&[&args[0], &args[1]], 32, array_size, domain).saturating_add(
                passes
                    .saturating_mul(literals.saturating_add(operations))
                    // The writer builds one owned value argument per BYTE,
                    // before the numeric facade's separately quoted slice copy.
                    .saturating_add(
                        passes
                            .saturating_mul(32)
                            .saturating_mul(args[1].clone_work()),
                    ),
            )
        }
        opcode::MSTORE8 => {
            let passes = domain.memory_write_passes(&args[0], &args[1], 1);
            if passes == 0 {
                return byte_work(&[&args[0], &args[1]], 1, array_size, domain);
            }
            let literal = domain.literal_quote_work(crate::U256::from(255));
            if literal == usize::MAX {
                return usize::MAX;
            }
            let mask = EvmValue::<D::Element>::constant(crate::U256::from(255));
            byte_work(&[&args[0], &args[1]], 1, array_size, domain).saturating_add(
                passes.saturating_mul(literal.saturating_add(args[1].clone_work()).saturating_add(
                    domain.operation_work_for_refs(opcode::AND, &[&args[1], &mask]),
                )),
            )
        }
        opcode::EXTCODEHASH => {
            let (bytes, accounts) = external_code_shape(&result.payload.store, targets);
            external_code_fee(bytes, accounts, domain, false)
        }
        opcode::EXTCODESIZE | opcode::BALANCE => candidates::count(&args[0])
            .saturating_mul(domain.capacity())
            .saturating_add(1),
        opcode::CALLDATALOAD => byte_work(&[&args[0]], 32, array_size, domain),
        opcode::KECCAK256 => byte_work(&[&args[0], &args[1]], range_size(), array_size, domain),
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
        0x01..=0x0b | 0x10..=0x1e => domain.operation_work_for(op, args),
        _ => 1,
    }
}

fn external_code_shape<N: WordElement>(
    store: &EvmStore<N>,
    targets: Option<&Candidates<'_>>,
) -> (usize, usize) {
    let Some(targets) = targets else {
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

fn external_code_fee<D: EvmNumericAlgebra>(
    source_bytes: usize,
    accounts: usize,
    domain: AlgebraRef<'_, D>,
    copied: bool,
) -> usize {
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
