//! Exact typed node projection after dependencies have been interned. The
//! shared structural map owns field reconstruction; these handlers own payload
//! conversion, identity numbering, and the original charging boundaries.
use super::{Pool, Root, value};
use alloy_primitives::{U256, hex};
use evm_abstract::domain::symbolic::{
    ByteStateId, ByteView, ExprId, InputAtom, SlotStateId, SourceId, StateBinding,
    structure::{Borrowed, Mapper},
};
use evm_abstract_protocol as api;
use std::sync::Arc;

impl Pool<'_> {
    pub(super) fn node(&mut self, root: &Root) -> Result<api::Expression, api::ApiError> {
        match root {
            Root::Native(value) => value.structure().try_map(self).map(Into::into),
            Root::Path(path) => self.path_node(path),
        }
    }
}
impl<'a> Mapper<Borrowed<'a>> for Pool<'_> {
    type Output = api::structure::Wire;
    type Error = api::ApiError;
    fn word(&mut self, value: &&ExprId) -> Result<api::ExpressionRef, api::ApiError> {
        Ok(self.e(value))
    }
    fn bytes(&mut self, value: &&ByteStateId) -> Result<api::ByteStateRef, api::ApiError> {
        Ok(self.b(value))
    }
    fn slots(&mut self, value: &&SlotStateId) -> Result<api::SlotStateRef, api::ApiError> {
        Ok(self.s(value))
    }
    fn view(&mut self, value: &&ByteView) -> Result<api::ByteViewRef, api::ApiError> {
        Ok(self.v(value))
    }
    fn constant(&mut self, value: &&U256) -> Result<String, api::ApiError> {
        Ok(value::word(**value))
    }
    fn input(&mut self, value: &&InputAtom) -> Result<api::InputIdentity, api::ApiError> {
        Ok(api::InputIdentity {
            scope: api::DecimalU64(value.scope),
            symbol: value::symbol(value.symbol),
        })
    }
    fn fresh(&mut self, value: u64) -> Result<u64, api::ApiError> {
        let next = self.fresh.len() as u64;
        Ok(*self.fresh.entry(value).or_insert(next))
    }
    fn exact(&mut self, value: &&Arc<[u8]>) -> Result<String, api::ApiError> {
        // Only real byte payloads incur encoding work; lazy ranges stay compact.
        self.payload(value.len())?;
        Ok(hex::encode_prefixed(value))
    }
    fn source(&mut self, value: &&SourceId) -> Result<api::SymbolicSource, api::ApiError> {
        Ok(api::SymbolicSource {
            token: self.e(value.token()),
            namespace: value::word(value.namespace()),
        })
    }
    fn binding(&mut self, value: &&StateBinding) -> Result<api::SymbolicBinding, api::ApiError> {
        self.charge(1 + value.numeric.constants().map_or(0, |v| v.len()))?;
        Ok(api::SymbolicBinding {
            index: value::word(value.index),
            value: self.e(&value.value),
            numeric: value::observation(&value.numeric),
        })
    }
}
