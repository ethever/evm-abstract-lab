//! Native operands for CALL-family roles, with no global residual input symbol.

use super::{ProcessAction, ProcessOwner, ProcessOwnerError};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement, summary},
    resource::WorkBudget,
};
use alloy_primitives::{Address, U256};

/// Logical target value, deliberately independent of its physical owner class.
pub enum ProcessTargetAddress<'a, N: WordElement> {
    /// An established real EVM address, not a storage namespace placeholder.
    Exact(Address),
    /// An existing logical operand, including a symbolic root's ADDRESS.
    Operand(&'a EvmValue<N>),
    /// An independent unknown target with no trusted global input identity.
    Independent,
}

/// Which existing invocation value supplies an implicit address operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressRole {
    /// The parent's actual logical ADDRESS operand.
    ParentAddress,
    /// The parent's inherited CALLER operand.
    ParentCaller,
    /// The selected target; residual targets carry no must-alias identity.
    Target,
}

/// CALLVALUE role independent of executable source and physical owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallValueRole {
    /// DELEGATECALL inherits its parent's actual value operand.
    Parent,
    /// Static execution admits only zero transfer value.
    Zero,
    /// Arbitrary nonstatic invocation value, conservatively Top.
    Any,
}

/// EVM child roles. Physical owner inheritance is explicit, not a logical test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallRoles {
    /// Child's logical ADDRESS source.
    pub address: AddressRole,
    /// Child's logical CALLER source.
    pub caller: AddressRole,
    /// Child's CALLVALUE source.
    pub value: CallValueRole,
    /// DELEGATECALL/CALLCODE inherit the parent's physical owner class.
    pub inherits_owner: bool,
    /// Staticness propagates through every kind of nested call.
    pub is_static: bool,
}

/// Materialized operands; none is a physical representative storage address.
#[derive(Debug)]
pub struct ProcessCallOperands<N: WordElement> {
    /// Child ADDRESS, supplied as an explicit native operand override.
    pub address: EvmValue<N>,
    /// Child CALLER, supplied as an explicit native operand override.
    pub caller: EvmValue<N>,
    /// Child CALLVALUE.
    pub value: EvmValue<N>,
}

impl CallRoles {
    /// Construct roles for one CALL-family action; creation stays explicit.
    pub fn for_action(
        action: ProcessAction,
        parent_static: bool,
    ) -> Result<Self, ProcessOwnerError> {
        let is_static = parent_static || action == ProcessAction::StaticCall;
        let value = if is_static {
            CallValueRole::Zero
        } else {
            CallValueRole::Any
        };
        let (address, caller, value, inherits_owner) = match action {
            ProcessAction::Call | ProcessAction::StaticCall => (
                AddressRole::Target,
                AddressRole::ParentAddress,
                value,
                false,
            ),
            ProcessAction::DelegateCall => (
                AddressRole::ParentAddress,
                AddressRole::ParentCaller,
                CallValueRole::Parent,
                true,
            ),
            ProcessAction::CallCode => (
                AddressRole::ParentAddress,
                AddressRole::ParentAddress,
                value,
                true,
            ),
            ProcessAction::Create | ProcessAction::Create2 => {
                return Err(ProcessOwnerError::Allocation);
            }
        };
        Ok(Self {
            address,
            caller,
            value,
            inherits_owner,
            is_static,
        })
    }

    /// Physical owner is inherited only for DELEGATECALL and CALLCODE.
    pub fn child_owner(self, parent: ProcessOwner, target: ProcessOwner) -> ProcessOwner {
        if self.inherits_owner { parent } else { target }
    }

    /// Bind actual parent operands, charging each native constructor or copy.
    /// Independent residual targets are anonymous numerical values: equality
    /// and inequality both remain possible. Reusing Input(To) would be unsound.
    pub fn bind<D: EvmNumericAlgebra>(
        self,
        target: ProcessTargetAddress<'_, D::Element>,
        parent: &ProcessCallOperands<D::Element>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<ProcessCallOperands<D::Element>, ProcessOwnerError> {
        let mut address = |role| match role {
            AddressRole::ParentAddress => summary::clone_value(&parent.address, budget),
            AddressRole::ParentCaller => summary::clone_value(&parent.caller, budget),
            AddressRole::Target => match target {
                ProcessTargetAddress::Exact(address) => {
                    summary::constant(U256::from_be_slice(address.as_slice()), domain, budget)
                }
                ProcessTargetAddress::Operand(value) => summary::clone_value(value, budget),
                ProcessTargetAddress::Independent => {
                    summary::unsigned_range(U256::ZERO, U256::MAX >> 96usize, domain, budget)
                }
            },
        };
        let child_address = address(self.address)?;
        let caller = address(self.caller)?;
        let value = match self.value {
            CallValueRole::Parent => summary::clone_value(&parent.value, budget)?,
            CallValueRole::Zero => summary::constant(U256::ZERO, domain, budget)?,
            CallValueRole::Any => summary::top(domain, budget)?,
        };
        Ok(ProcessCallOperands {
            address: child_address,
            caller,
            value,
        })
    }
}
