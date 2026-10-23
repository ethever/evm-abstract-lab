//! Guarded CREATE state relations on named owners and a weak outside class.
//!
//! Balance Top covers checked affordable endowment updates and prefunding; it
//! proves no funding guard. The adapter retains rejecting guards separately.
//! Only Named can receive an account reset. Residual already has independent
//! Top bounds and is never strongly updated through an invented address.

use super::metered::pay_structure;
use super::{Code, EvmStoreSummary, Existence, InitialIndependence, OrderedMap, SummaryPlane};
use crate::{
    analysis::ProcessOwner,
    domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement, summary as scalar},
    resource::{CloneError, WorkBudget, clone_payment::checkpoint},
};
use alloy_primitives::{Address, U256};

impl<N: WordElement> SummaryPlane<N> {
    fn process_zero_owner<D: EvmNumericAlgebra<Element = N>>(
        &self,
        owner: Address,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_entries(), budget)?;
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: scalar::clone_value(&self.global_default, budget)?,
            initial_independence: InitialIndependence::new(
                self.initial_independence.global_default,
            ),
        };
        for (address, value) in &self.defaults {
            if *address != owner {
                out.defaults
                    .insert(*address, scalar::clone_value(value, budget)?);
            }
        }
        out.defaults
            .insert(owner, scalar::constant(U256::ZERO, domain, budget)?);
        for ((address, slot), value) in &self.slots {
            if *address != owner {
                out.slots
                    .insert((*address, *slot), scalar::clone_value(value, budget)?);
            }
        }
        for (address, value) in &self.initial_independence.defaults {
            if *address != owner {
                out.initial_independence.defaults.insert(*address, *value);
            }
        }
        out.initial_independence.defaults.insert(owner, false);
        for ((address, slot), value) in &self.initial_independence.slots {
            if *address != owner {
                out.initial_independence
                    .slots
                    .insert((*address, *slot), *value);
            }
        }
        checkpoint(budget)?;
        Ok(out)
    }
}

fn replace_numeric<N: WordElement>(
    map: &OrderedMap<Address, EvmValue<N>>,
    owner: Address,
    value: EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<OrderedMap<Address, EvmValue<N>>, CloneError> {
    pay_structure(map.len(), budget)?;
    let mut out = OrderedMap::new();
    for (address, old) in map {
        if *address != owner {
            out.insert(*address, scalar::clone_value(old, budget)?);
        }
    }
    out.insert(owner, value);
    checkpoint(budget)?;
    Ok(out)
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Current named nonce; None includes every protocol nonce. No getter clone
    /// or default Top factory is invoked to discover this observation.
    pub(crate) fn process_nonce(
        &self,
        owner: ProcessOwner,
        budget: &mut WorkBudget,
    ) -> Result<Option<u64>, CloneError> {
        let ProcessOwner::Named(owner) = owner else {
            return Ok(None);
        };
        self.nonces.get(&owner).map_or(Ok(None), |value| {
            Ok(scalar::singleton(value, budget)?.and_then(|value| u64::try_from(value).ok()))
        })
    }

    /// Nonempty raw code alone is a collision proof. Unknown stays unknown;
    /// empty code additionally requires an actually observed zero nonce.
    pub(crate) fn process_collision(
        &self,
        owner: ProcessOwner,
        budget: &mut WorkBudget,
    ) -> Result<crate::analysis::process_creation::Collision, CloneError> {
        use crate::analysis::process_creation::Collision;
        let ProcessOwner::Named(owner) = owner else {
            return Ok(Collision::Unknown);
        };
        if matches!(self.codes.get(&owner), Some(Code::Runtime(program)) if program.byte_len()!=0)
            || matches!(self.codes.get(&owner), Some(Code::Delegation(_)))
        {
            return Ok(Collision::NonEmptyCode);
        }
        if self
            .created
            .get(&owner)
            .copied()
            .unwrap_or(self.created_default)
            == Some(true)
        {
            return Ok(Collision::NonZeroNonce);
        }
        match self.process_nonce(ProcessOwner::Named(owner), budget)? {
            Some(0) if matches!(self.codes.get(&owner), Some(Code::Empty)) => {
                Ok(Collision::Available)
            }
            Some(1..=u64::MAX) => Ok(Collision::NonZeroNonce),
            _ => Ok(Collision::Unknown),
        }
    }

    /// Successful nonce guard excludes u64::MAX. Its increment precedes the
    /// constructor checkpoint and survives child failure. Outside remains weak.
    pub(crate) fn process_increment_nonce<D: EvmNumericAlgebra<Element = N>>(
        &self,
        creator: ProcessOwner,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_prefix_work(), budget)?;
        let nonce = self.process_nonce(creator, budget)?;
        let mut out = self.clone_with_payment(budget)?;
        if let ProcessOwner::Named(owner) = creator {
            let incremented = match nonce.and_then(|value| value.checked_add(1)) {
                Some(next) => scalar::constant(U256::from(next), domain, budget)?,
                None => scalar::unsigned_range(U256::ONE, U256::from(u64::MAX), domain, budget)?,
            };
            out.nonces = replace_numeric(&self.nonces, owner, incremented, budget)?;
        }
        checkpoint(budget)?;
        Ok(out)
    }

    /// Enter only under the adapter's collision-free/affordable guard, after
    /// nonce increment and checkpoint. Active initcode is not account runtime.
    pub(crate) fn process_begin_constructor<D: EvmNumericAlgebra<Element = N>>(
        &self,
        creator: ProcessOwner,
        destination: ProcessOwner,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_prefix_work().saturating_mul(2), budget)?;
        let mut out = self.clone_with_payment(budget)?;
        if let ProcessOwner::Named(owner) = creator {
            out.balances =
                replace_numeric(&self.balances, owner, scalar::top(domain, budget)?, budget)?;
        }
        if let ProcessOwner::Named(owner) = destination {
            out.persistent = self.persistent.process_zero_owner(owner, domain, budget)?;
            out.transient = self.transient.process_zero_owner(owner, domain, budget)?;
            out.nonces = replace_numeric(
                &self.nonces,
                owner,
                scalar::constant(U256::ONE, domain, budget)?,
                budget,
            )?;
            // Rebuild from the actual result of the creator update. Each paid
            // copy sees any newly produced Top and its actual native tariff.
            out.balances =
                replace_numeric(&out.balances, owner, scalar::top(domain, budget)?, budget)?;
            out.codes.insert(owner, Code::Empty);
            out.existence.insert(owner, Existence::Present);
            out.created.insert(owner, Some(true));
            out.pending_destruction.insert(owner, Some(false));
            out.touched.insert(owner, Some(true));
        }
        checkpoint(budget)?;
        Ok(out)
    }

    /// The successful deployment guard contains every valid runtime, including
    /// empty. Unknown is an account-code bound, never a fake captured Program.
    pub(crate) fn process_deploy<D: EvmNumericAlgebra<Element = N>>(
        &self,
        destination: ProcessOwner,
        _domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_prefix_work(), budget)?;
        let mut out = self.clone_with_payment(budget)?;
        if let ProcessOwner::Named(owner) = destination {
            out.codes.insert(owner, Code::Unknown);
        }
        checkpoint(budget)?;
        Ok(out)
    }
}
