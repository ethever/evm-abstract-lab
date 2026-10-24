//! Scalar continuations share the Reader's payer and immutable branch scopes.

use super::payment::{ReadPayment, ReadResult};
use super::{Context, EvmNumericAlgebra, EvmValue, ExprId, ExprKind, WordElement};
use crate::domain::AlgebraRef;

// Guard normalization can put the entire selected snapshot history into one
// scalar ITE. Keep its continuations on the heap just like Reader's content
// walk. Native values move through a separate stack; scheduling never clones
// them or pre-evaluates an unselected branch.
enum Task<'s> {
    Evaluate(&'s ExprId),
    Apply {
        opcode: u8,
        arguments: usize,
    },
    Select {
        condition: &'s ExprId,
        then_value: &'s ExprId,
        else_value: &'s ExprId,
        retained: bool,
    },
    Branch {
        condition: &'s ExprId,
        nonzero: bool,
        value: &'s ExprId,
    },
    EndBranch,
    Refine,
    Join,
}

pub(in crate::world) fn scalar_in_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<EvmValue<N>> {
    scalar_in_paid_with_memo(expression, context, domain, payer, None)
}

pub(super) fn scalar_in_paid_with_memo<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
    memo: Option<&std::cell::RefCell<super::OperationMemo<N>>>,
) -> ReadResult<EvmValue<N>> {
    // The first Evaluate must pass its existing context/reservation before
    // any continuation stack grows. Keep it inline and charge it only once.
    let mut first = Some(Task::Evaluate(expression));
    let mut tasks = Vec::new();
    let mut values = Vec::new();
    let mut branches = Vec::new();
    while let Some(task) = first.take().or_else(|| tasks.pop()) {
        let context = branches.last().unwrap_or(context);
        let result = match task {
            Task::Evaluate(expression) => {
                payer.context(context)?;
                payer.reserve(expression.work_size().saturating_add(1))?;
                let retained = context.value_paid(expression, domain, payer)?;
                if let Some(word) = expression.as_constant() {
                    Some(if let Some(value) = retained {
                        let literal = payer.literal(word, domain)?;
                        let joined = payer.join(&value, &literal, context, domain)?;
                        joined.with_numeric(payer.literal_numeric(word, domain)?)
                    } else {
                        payer.literal(word, domain)?
                    })
                } else if let ExprKind::Ite {
                    condition,
                    then_value,
                    else_value,
                } = expression.kind()
                {
                    let has_retained = retained.is_some();
                    if let Some(value) = retained {
                        values.push(value);
                    }
                    tasks.push(Task::Select {
                        condition,
                        then_value,
                        else_value,
                        retained: has_retained,
                    });
                    tasks.push(Task::Evaluate(condition));
                    None
                } else if let Some(value) = retained {
                    Some(payer.copy(&value)?)
                } else if let ExprKind::Operation { opcode, args } = expression.kind() {
                    tasks.push(Task::Apply {
                        opcode: *opcode,
                        arguments: args.len(),
                    });
                    tasks.extend(args.iter().rev().map(Task::Evaluate));
                    None
                } else {
                    Some(payer.top(domain)?)
                }
            }
            Task::Apply { opcode, arguments } => {
                let args = values.split_off(values.len() - arguments);
                Some(match memo {
                    Some(memo) => memo
                        .borrow_mut()
                        .apply(opcode, args, context, domain, payer)?,
                    None => payer.apply(opcode, &args, context, domain, false)?,
                })
            }
            Task::Select {
                condition,
                then_value,
                else_value,
                retained,
            } => {
                let condition_value = values.pop().expect("scalar condition");
                let selected = if !payer.may_nonzero(&condition_value)? {
                    Some((else_value, false))
                } else if !payer.may_zero(&condition_value)? {
                    Some((then_value, true))
                } else {
                    None
                };
                if let Some((value, nonzero)) = selected {
                    if retained {
                        tasks.push(Task::Refine);
                    }
                    tasks.push(Task::Branch {
                        condition,
                        nonzero,
                        value,
                    });
                    None
                } else if retained {
                    let value = values.pop().expect("retained scalar");
                    Some(payer.copy(&value)?)
                } else {
                    // Construct each branch only when it is entered, after
                    // the earlier branch's native actions and payments.
                    tasks.push(Task::Join);
                    tasks.push(Task::Branch {
                        condition,
                        nonzero: false,
                        value: else_value,
                    });
                    tasks.push(Task::Branch {
                        condition,
                        nonzero: true,
                        value: then_value,
                    });
                    None
                }
            }
            Task::Branch {
                condition,
                nonzero,
                value,
            } => {
                branches.push(context.branch_paid(condition, nonzero, payer)?);
                tasks.push(Task::EndBranch);
                tasks.push(Task::Evaluate(value));
                None
            }
            Task::EndBranch => {
                branches.pop().expect("active scalar branch");
                None
            }
            Task::Refine => {
                let selected = values.pop().expect("selected scalar");
                let retained = values.pop().expect("retained scalar");
                Some(
                    if let Some(exact) = payer.singleton(&selected)?
                        && payer.contains(&retained, exact)?
                    {
                        let out = payer.copy(&retained)?;
                        out.with_numeric(payer.literal_numeric(exact, domain)?)
                    } else {
                        payer.copy(&retained)?
                    },
                )
            }
            Task::Join => {
                let right = values.pop().expect("right scalar");
                let left = values.pop().expect("left scalar");
                Some(payer.join(&left, &right, context, domain)?)
            }
        };
        if let Some(value) = result {
            values.push(value);
        }
    }
    Ok(values.pop().expect("completed scalar"))
}
