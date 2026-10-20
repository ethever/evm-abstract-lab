//! The final prepaid native copy must pass the ordinary typed cancellation gate.

use super::*;
use crate::domain::symbolic::{ByteStateId, ByteView, ExprError, SourceId};

#[test]
fn last_retained_payload_clone_cannot_complete_a_cancelled_prepaid_word_read() {
    let _clones = HEAVY_CLONES.scope();
    let _cancel = CANCEL_HEAVY_CLONE.scope();
    let algebra = ValueAlgebra::new(HeavyWords::default());
    let domain = algebra.borrowed();
    let root = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(32)));
    let expression = ExprId::word_read(
        ByteView::new(
            root.clone(),
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(32)),
        ),
        ExprId::constant(U256::ZERO),
        super::super::limits(domain),
    )
    .unwrap();
    let value = EvmValue::from_numeric(Heavy {
        word: Word {
            lower: U256::ZERO,
            upper: U256::MAX,
            odd: true,
        },
        payload: vec![7; 64 * 1024],
    });
    let copy_work = value.clone_work();
    let mut memory = EvmByteArray::memory();
    memory.length = EvmValue::constant(U256::from(32));
    memory.default = EvmValue::unknown_byte();
    memory.root = root;
    memory.operands.insert(expression, value);
    let frozen = memory.clone();
    let offset = EvmValue::<Heavy>::constant(U256::ZERO);
    let full_read_work = memory.word_numeric_work_with(&offset, domain);

    for cancel in [false, true] {
        let token = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(full_read_work + copy_work);
        assert!(budget.charge(full_read_work));
        {
            let payer = conditional::BindingBudget::new(&mut budget);
            let context = conditional::Context::prepaid(
                &memory.operands,
                Some(&memory.conditional),
                domain,
                &payer,
            );
            assert!(
                context.demand_usage().is_none(),
                "exercise the no-Demand fallback"
            );
            HEAVY_CLONES.replace(0);
            CANCEL_HEAVY_CLONE.replace(cancel);
            let read = analysis_control::with_cancellation(&token, || {
                super::super::word_plan::evaluate(
                    &memory.root,
                    None,
                    &ExprId::constant(U256::ZERO),
                    &context,
                    domain,
                )
            });
            assert_eq!(
                HEAVY_CLONES.get(),
                1,
                "cancel in the final native payload copy"
            );
            assert!(!CANCEL_HEAVY_CLONE.get());
            assert_eq!(token.is_cancelled(), cancel);
            if cancel {
                assert_eq!(read, Err(ExprError::Cancelled.into()));
                // The same payer retains cancellation after the dynamic token
                // scope ends; a retry cannot convert it into ordinary Work.
                assert_eq!(context.check(), Err(ExprError::Cancelled));
            } else {
                let value = read.unwrap().unwrap();
                assert!(value.numeric().word.odd);
                assert!(!value.contains(U256::from(42)));
                assert!(context.check().is_ok());
            }
            assert!(!context.work_exhausted());
        }
        assert_eq!(budget.used(), full_read_work + copy_work);
        assert!(!budget.exhausted());
        assert_eq!(memory, frozen);
    }
}
