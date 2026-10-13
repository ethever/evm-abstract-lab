//! Exact typed node projection after dependencies have been interned.
use super::{Pool, Root, value};
use alloy_primitives::hex;
use evm_abstract::domain::{
    AbstractValue,
    symbolic::{ByteStateKind, ExprKind, SlotStateKind, SourceId, StateBinding},
};
use evm_abstract_protocol as api;

impl Pool<'_> {
    fn source(&self, source: &SourceId) -> api::SymbolicSource {
        api::SymbolicSource {
            token: self.e(source.token()),
            namespace: value::word(source.namespace()),
        }
    }
    fn bindings(
        &mut self,
        source: &[StateBinding],
    ) -> Result<Vec<api::SymbolicBinding>, api::ApiError> {
        source
            .iter()
            .map(|cell| {
                self.charge(1 + cell.numeric.constants().map_or(0, |v| v.len()))?;
                Ok(api::SymbolicBinding {
                    index: value::word(cell.index),
                    value: self.e(&cell.value),
                    numeric: value::info(&AbstractValue::from_numeric(cell.numeric.clone())),
                })
            })
            .collect()
    }
    pub(super) fn node(&mut self, root: &Root) -> Result<api::Expression, api::ApiError> {
        use api::Expression as N;
        Ok(match root {
            Root::Expression(expr) => match expr.kind() {
                ExprKind::Constant(v) => N::Constant(value::word(*v)),
                ExprKind::Input(v) => N::Input(api::InputIdentity {
                    scope: v.scope,
                    symbol: value::symbol(v.symbol),
                }),
                ExprKind::Fresh(id) => {
                    let next = self.fresh.len() as u64;
                    N::Fresh(*self.fresh.entry(*id).or_insert(next))
                }
                ExprKind::Operation { opcode, args } => N::Operation(api::ExpressionOperation {
                    opcode: *opcode,
                    arguments: args.iter().map(|v| self.e(v)).collect(),
                }),
                ExprKind::Ite {
                    condition,
                    then_value,
                    else_value,
                } => N::Ite(api::ExpressionIte {
                    condition: self.e(condition),
                    then_value: self.e(then_value),
                    else_value: self.e(else_value),
                }),
                ExprKind::ByteRead { state, index } => N::ByteRead(api::ByteRead {
                    state: self.b(state),
                    index: self.e(index),
                }),
                ExprKind::WordRead { view, offset } => N::WordRead(api::WordRead {
                    view: self.v(view),
                    offset: self.e(offset),
                }),
                ExprKind::SlotRead { state, key } => N::SlotRead(api::SlotRead {
                    state: self.s(state),
                    key: self.e(key),
                }),
                ExprKind::Keccak { input } => N::Keccak(self.v(input)),
            },
            Root::Bytes(bytes) => match bytes.kind() {
                ByteStateKind::Zero => N::BytesZero,
                ByteStateKind::Input {
                    source,
                    length,
                    bindings,
                } => N::BytesInput(api::BytesInput {
                    source: self.source(source),
                    length: self.e(length),
                    bindings: self.bindings(bindings)?,
                }),
                ByteStateKind::Exact(bytes) => {
                    // Encoding charges real payload work; lazy logical ranges stay compact.
                    self.payload(bytes.len())?;
                    N::BytesExact(hex::encode_prefixed(bytes))
                }
                ByteStateKind::WriteByte {
                    previous,
                    offset,
                    value,
                } => N::WriteByte(api::ByteWrite {
                    previous: self.b(previous),
                    offset: self.e(offset),
                    value: self.e(value),
                }),
                ByteStateKind::WriteWord {
                    previous,
                    offset,
                    value,
                } => N::WriteWord(api::ByteWrite {
                    previous: self.b(previous),
                    offset: self.e(offset),
                    value: self.e(value),
                }),
                ByteStateKind::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => N::Copy(api::ByteCopy {
                    previous: self.b(previous),
                    destination: self.e(destination),
                    source: self.v(source),
                    length: self.e(length),
                }),
                ByteStateKind::Slice { source } => N::Slice(self.v(source)),
                ByteStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => N::BytesIte(api::BytesIte {
                    condition: self.e(condition),
                    then_state: self.b(then_state),
                    else_state: self.b(else_state),
                }),
            },
            Root::Slots(slots) => match slots.kind() {
                SlotStateKind::Zero => N::SlotsZero,
                SlotStateKind::Input { source, bindings } => N::SlotsInput(api::SlotsInput {
                    source: self.source(source),
                    bindings: self.bindings(bindings)?,
                }),
                SlotStateKind::Store {
                    previous,
                    key,
                    value,
                } => N::Store(api::SlotWrite {
                    previous: self.s(previous),
                    key: self.e(key),
                    value: self.e(value),
                }),
                SlotStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => N::SlotsIte(api::SlotsIte {
                    condition: self.e(condition),
                    then_state: self.s(then_state),
                    else_state: self.s(else_state),
                }),
            },
            Root::View(view) => N::View(api::ByteView {
                state: self.b(&view.state),
                offset: self.e(&view.offset),
                length: self.e(&view.length),
            }),
            Root::Path(path) => self.path_node(path)?,
        })
    }
}
