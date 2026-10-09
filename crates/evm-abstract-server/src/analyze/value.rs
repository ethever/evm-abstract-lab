//! Lossless typed projection of scalar abstract components and symbolic identity.
use evm_abstract::{
    U256,
    domain::{
        AbstractValue,
        identity::Symbol,
        provenance::Origin,
        symbolic::{ExprId, ExprKind},
    },
    world::AddressInput,
};
use evm_abstract_protocol as api;

pub(super) fn word(value: U256) -> String {
    format!("0x{value:x}")
}
pub(super) fn symbol(source: Symbol) -> api::InputSymbol {
    use api::InputSymbolKind as K;
    let (kind, index) = match source {
        Symbol::To => (K::To, None),
        Symbol::Caller => (K::Caller, None),
        Symbol::Origin => (K::Origin, None),
        Symbol::Coinbase => (K::Coinbase, None),
        Symbol::CallValue => (K::CallValue, None),
        Symbol::GasPrice => (K::GasPrice, None),
        Symbol::Timestamp => (K::Timestamp, None),
        Symbol::Number => (K::Number, None),
        Symbol::Prevrandao => (K::Prevrandao, None),
        Symbol::GasLimit => (K::GasLimit, None),
        Symbol::ChainId => (K::ChainId, None),
        Symbol::BaseFee => (K::BaseFee, None),
        Symbol::BlobBaseFee => (K::BlobBaseFee, None),
        Symbol::CalldataLength => (K::CalldataLength, None),
        Symbol::CalldataWord(index) => (K::CalldataWord, Some(word(index))),
        Symbol::BlockHash(index) => (K::BlockHash, Some(word(index))),
        Symbol::BlobHash(index) => (K::BlobHash, Some(word(index))),
    };
    api::InputSymbol { kind, index }
}
pub(super) fn address(source: AddressInput) -> api::AddressValue {
    match source {
        AddressInput::Concrete(address) => api::AddressValue::Concrete(address.to_string()),
        AddressInput::Symbolic(name) => api::AddressValue::Symbolic(symbol(name)),
    }
}
fn expression(source: &ExprId) -> api::ExpressionGraph {
    fn intern(
        source: &ExprId,
        nodes: &mut Vec<api::Expression>,
        indices: &mut std::collections::BTreeMap<ExprId, usize>,
    ) -> usize {
        if let Some(index) = indices.get(source) {
            return *index;
        }
        let node = match source.kind() {
            ExprKind::Constant(value) => api::Expression::Constant(word(*value)),
            ExprKind::Input(input) => api::Expression::Input(api::InputIdentity {
                scope: input.scope,
                symbol: symbol(input.symbol),
            }),
            ExprKind::Fresh(id) => api::Expression::Fresh(*id),
            ExprKind::Operation { opcode, args } => {
                api::Expression::Operation(api::ExpressionOperation {
                    opcode: *opcode,
                    arguments: args
                        .iter()
                        .map(|child| intern(child, nodes, indices))
                        .collect(),
                })
            }
        };
        let index = nodes.len();
        nodes.push(node);
        indices.insert(source.clone(), index);
        index
    }
    let mut nodes = Vec::new();
    let root = intern(source, &mut nodes, &mut std::collections::BTreeMap::new());
    api::ExpressionGraph { nodes, root }
}
fn origin(source: Origin) -> api::ValueOrigin {
    use api::ValueOrigin as O;
    match source {
        Origin::Constant => O::Constant,
        Origin::Calldata => O::Calldata,
        Origin::Memory => O::Memory,
        Origin::Storage => O::Storage,
        Origin::TransientStorage => O::TransientStorage,
        Origin::Environment => O::Environment,
        Origin::Address => O::Address,
        Origin::CodeAddress => O::CodeAddress,
        Origin::CallValue => O::CallValue,
        Origin::Returndata => O::Returndata,
        Origin::Arithmetic => O::Arithmetic,
        Origin::Balance => O::Balance,
        Origin::Nonce => O::Nonce,
    }
}
pub(super) fn info(source: &AbstractValue) -> api::ValueInfo {
    let (lower, upper) = source.interval().unsigned_bounds();
    let unsigned = api::WordBounds {
        lower: word(lower),
        upper: word(upper),
    };
    let (lower, upper) = source.interval().signed_bounds();
    let sign = U256::ONE << 255;
    let signed = api::WordBounds {
        lower: word(lower ^ sign),
        upper: word(upper ^ sign),
    };
    let congruence = if source.congruence().is_top() {
        api::CongruenceValue::Any
    } else if let Some(exact) = source.congruence().singleton() {
        api::CongruenceValue::Exact(word(exact))
    } else {
        let (modulus, residue) = source
            .congruence()
            .modulus_residue()
            .expect("non-exact congruence has a modulus");
        api::CongruenceValue::Modulo(api::CongruenceClass {
            modulus: word(modulus),
            residue: word(residue),
        })
    };
    api::ValueInfo {
        summary: source.to_string(),
        constants: source
            .constants()
            .map(|values| values.iter().copied().map(word).collect()),
        known_zero: word(source.known_bits().zero()),
        known_one: word(source.known_bits().one()),
        unsigned,
        signed,
        congruence,
        origins: source
            .provenance()
            .origins()
            .sources()
            .map(|sources| sources.iter().copied().map(origin).collect()),
        code_address_role: source.provenance().is_code_address(),
        identity: source
            .identity()
            .input()
            .map(|identity| api::InputIdentity {
                scope: identity.scope(),
                symbol: symbol(identity.symbol()),
            }),
        expression: source.expression().map(expression),
        symbolic_limit: source.symbolic_limit_reached(),
    }
}
