//! The existing iterative slot reader, with the same fallible native payer.
use super::payment::{ReadPayment, ReadResult};
use super::{
    Context, EvmNumericAlgebra, EvmValue, WordElement, disjoint_paid, numeric_domain,
    scalar_in_paid,
};
use crate::{
    domain::{
        AlgebraRef,
        symbolic::{ExprId, SlotStateId, SlotStateKind},
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;

enum Continuation<'s, 'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> {
    Stored(&'s ExprId),
    Selected(Context<'a, N, D>),
    Else {
        condition: &'s ExprId,
        state: &'s SlotStateId,
        parent: Context<'a, N, D>,
    },
    Join {
        parent: Context<'a, N, D>,
        then_context: Context<'a, N, D>,
    },
}
pub(in crate::world) fn slot_with_payment<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &SlotStateId,
    key: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> ReadResult<EvmValue<N>> {
    slot_paid(root, key, context, domain, &mut ReadPayment::new(budget))
}
pub(in crate::world) fn slot<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &SlotStateId,
    key: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    slot_paid(root, key, context, domain, &mut ReadPayment::unpaid())
        .unwrap_or_else(|_| EvmValue::top())
}
fn slot_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &SlotStateId,
    key: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<EvmValue<N>> {
    let domain = numeric_domain(domain);
    let mut cursor = root;
    let mut context = context.clone();
    let mut pending = Vec::new();
    let mut values = Vec::new();
    'read: loop {
        payer.context(&context)?;
        payer.reserve(16)?;
        let mut out = match cursor.kind() {
            SlotStateKind::Zero => {
                scalar_in_paid(&ExprId::constant(U256::ZERO), &context, domain, payer)?
            }
            SlotStateKind::Input { bindings, .. } => {
                let key = scalar_in_paid(key, &context, domain, payer)?;
                if let Some(keys) = payer.candidates(&key)? {
                    let mut out = None;
                    for key in keys.iter() {
                        payer.reserve(bindings.len().saturating_mul(8).saturating_add(1))?;
                        let value = if let Some(binding) =
                            bindings.iter().find(|binding| binding.index == *key)
                        {
                            let value = scalar_in_paid(&binding.value, &context, domain, payer)?;
                            let retained = context.has_binding_value_paid(&binding.value, payer)?;
                            payer.bind(value, retained, &binding.numeric, &context, domain)?
                        } else {
                            payer.top(domain)?
                        };
                        out = Some(match out {
                            Some(old) => payer.join(&old, &value, &context, domain)?,
                            None => payer.copy(&value)?,
                        });
                    }
                    match out {
                        Some(value) => value,
                        None => payer.top(domain)?,
                    }
                } else {
                    payer.top(domain)?
                }
            }
            SlotStateKind::Store {
                previous,
                key: written,
                value,
            } => {
                payer.reserve(key.comparison_work(written))?;
                if key == written {
                    scalar_in_paid(value, &context, domain, payer)?
                } else {
                    let requested = scalar_in_paid(key, &context, domain, payer)?;
                    let stored = scalar_in_paid(written, &context, domain, payer)?;
                    if let (Some(requested), Some(stored)) =
                        (payer.singleton(&requested)?, payer.singleton(&stored)?)
                    {
                        if requested == stored {
                            scalar_in_paid(value, &context, domain, payer)?
                        } else {
                            cursor = previous;
                            continue 'read;
                        }
                    } else {
                        if !disjoint_paid(&requested, &stored, payer)? {
                            pending.push(Continuation::Stored(value));
                        }
                        cursor = previous;
                        continue 'read;
                    }
                }
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let c = scalar_in_paid(condition, &context, domain, payer)?;
                if !payer.may_zero(&c)? {
                    let branch = context.branch_when_scoped_paid(condition, true, payer)?;
                    pending.push(Continuation::Selected(std::mem::replace(
                        &mut context,
                        branch,
                    )));
                    cursor = then_state;
                } else if !payer.may_nonzero(&c)? {
                    let branch = context.branch_when_scoped_paid(condition, false, payer)?;
                    pending.push(Continuation::Selected(std::mem::replace(
                        &mut context,
                        branch,
                    )));
                    cursor = else_state;
                } else {
                    let branch = context.branch_when_scoped_paid(condition, true, payer)?;
                    pending.push(Continuation::Else {
                        condition,
                        state: else_state,
                        parent: std::mem::replace(&mut context, branch),
                    });
                    cursor = then_state;
                }
                continue 'read;
            }
        };
        while let Some(continuation) = pending.pop() {
            match continuation {
                Continuation::Stored(value) => {
                    let value = scalar_in_paid(value, &context, domain, payer)?;
                    out = payer.join(&out, &value, &context, domain)?;
                }
                Continuation::Selected(parent) => context = parent,
                Continuation::Else {
                    condition,
                    state,
                    parent,
                } => {
                    let branch = parent.branch_when_scoped_paid(condition, false, payer)?;
                    pending.push(Continuation::Join {
                        parent,
                        then_context: std::mem::replace(&mut context, branch),
                    });
                    values.push(out);
                    cursor = state;
                    continue 'read;
                }
                Continuation::Join {
                    parent,
                    then_context,
                } => {
                    let then_value = values.pop().expect("completed then read");
                    out = payer.join(&then_value, &out, &context, domain)?;
                    context = parent;
                    drop(then_context);
                }
            }
        }
        payer.context(&context)?;
        return Ok(out);
    }
}
