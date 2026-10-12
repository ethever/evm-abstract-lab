//! Bounded extraction of bytes known from immutable concrete observations.
//! This never picks a solver model, and symbolic offsets/lengths remain unknown.

use super::{ByteStateId, ByteStateKind, ByteView, ExprError, ExprLimits};
use alloy_primitives::U256;
use std::collections::BTreeMap;

struct Known {
    memo: BTreeMap<(ByteStateId, U256), Option<u8>>,
    limits: ExprLimits,
    work: usize,
}
impl Known {
    fn charge(&mut self) -> Result<(), ExprError> {
        self.work = self.work.saturating_add(1);
        super::state::check(self.work, 1, self.limits.into()).map_err(Into::into)
    }
    fn byte(&mut self, state: &ByteStateId, index: U256) -> Result<Option<u8>, ExprError> {
        let key = (state.clone(), index);
        if let Some(value) = self.memo.get(&key) {
            return Ok(*value);
        }
        self.charge()?;
        super::state::check(state.nodes(), state.depth(), self.limits.into())?;
        let value = match state.kind() {
            ByteStateKind::Zero => Some(0),
            ByteStateKind::Exact(bytes) => Some(
                usize::try_from(index)
                    .ok()
                    .and_then(|i| bytes.get(i))
                    .copied()
                    .unwrap_or(0),
            ),
            ByteStateKind::Input {
                length, bindings, ..
            } => {
                let observed = bindings
                    .iter()
                    .filter(|b| b.index == index)
                    .find_map(|b| b.numeric.singleton().or_else(|| b.value.as_constant()))
                    .and_then(|v| u8::try_from(v).ok());
                match length.as_constant() {
                    Some(len) if index >= len => Some(0),
                    Some(_) => observed,
                    None if observed == Some(0) => Some(0),
                    None => None,
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => match offset.as_constant() {
                Some(at) if index == at => value
                    .as_constant()
                    .map(|v| (v & U256::from(255)).to::<u8>()),
                Some(_) => self.byte(previous, index)?,
                None => None,
            },
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => match offset.as_constant() {
                Some(at) if index >= at && index - at < U256::from(32) => value
                    .as_constant()
                    .map(|v| v.to_be_bytes::<32>()[(index - at).to::<usize>()]),
                Some(_) => self.byte(previous, index)?,
                None => None,
            },
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => match (destination.as_constant(), length.as_constant()) {
                (Some(dst), Some(len)) if index >= dst && index - dst < len => {
                    self.view(source, index - dst)?
                }
                (Some(_), Some(_)) => self.byte(previous, index)?,
                _ => None,
            },
            ByteStateKind::Slice { source } => self.view(source, index)?,
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => match condition.as_constant() {
                Some(v) => self.byte(
                    if v == U256::ZERO {
                        else_state
                    } else {
                        then_state
                    },
                    index,
                )?,
                None => {
                    let yes = self.byte(then_state, index)?;
                    let no = self.byte(else_state, index)?;
                    if yes == no { yes } else { None }
                }
            },
        };
        self.memo.insert(key, value);
        Ok(value)
    }
    fn view(&mut self, view: &ByteView, index: U256) -> Result<Option<u8>, ExprError> {
        self.charge()?;
        if let Some(len) = view.length.as_constant() {
            if index >= len {
                return Ok(Some(0));
            }
        } else {
            return Ok(None);
        }
        let Some(offset) = view.offset.as_constant() else {
            return Ok(None);
        };
        match offset.checked_add(index) {
            Some(backing) => self.byte(&view.state, backing),
            None => Ok(Some(0)),
        }
    }
}
impl ByteView {
    /// Extract only bytes proved by exact source data and initial numeric facts.
    /// Returns None for symbolic content/length, and a typed resource/cancellation
    /// error if the attempted bounded extraction cannot be completed.
    pub fn known_bytes(&self, limits: ExprLimits) -> Result<Option<Vec<u8>>, ExprError> {
        let Some(len) = self
            .length
            .as_constant()
            .and_then(|v| usize::try_from(v).ok())
        else {
            return Ok(None);
        };
        super::state::check(len.max(self.nodes()), self.depth(), limits.into())?;
        let mut known = Known {
            memo: BTreeMap::new(),
            limits,
            work: 0,
        };
        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            let Some(byte) = known.view(self, U256::from(i))? else {
                return Ok(None);
            };
            bytes.push(byte);
        }
        Ok(Some(bytes))
    }
}
