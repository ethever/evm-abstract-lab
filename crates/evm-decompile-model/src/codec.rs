//! Named enum payloads use the same strict object decoder as ordinary records.
//! Public enums retain their derived serializer and Rust variant shapes; these
//! typed newtype adapters only constrain deserialization of named payloads.
use crate::{
    DispatchCase, EntityId, ExitKind, IndirectArm, StructuredKind, StructuredNode, Terminator,
    U256, ValueKind,
};
use serde::{Deserialize, Deserializer};

strict_record! { pub struct Operation { pub operation: EntityId, pub index: usize } }
strict_record! { pub struct Parameter { pub block: EntityId, pub index: usize } }
strict_record! { pub struct OptionalOperation { pub operation: Option<EntityId> } }
strict_record! { pub struct Block { pub block: EntityId } }
strict_record! { pub struct Edge { pub edge: EntityId } }
strict_record! { pub struct Jump {
    pub target: EntityId, pub edges: Vec<EntityId>, pub unresolved: bool, pub invalid_targets: Vec<U256>,
} }
strict_record! { pub struct Branch {
    pub condition: EntityId, pub target: EntityId, pub then_edges: Vec<EntityId>,
    pub else_edge: Option<EntityId>, pub unresolved: bool, pub invalid_targets: Vec<U256>,
} }
strict_record! { pub struct Exit { pub kind: ExitKind, pub values: Vec<EntityId> } }
strict_record! { pub struct Trap { pub reason: String } }
strict_record! { pub struct If<N = StructuredNode> {
    pub condition: EntityId, pub then_branch: Box<N>, pub else_branch: Box<N>,
} }
strict_record! { pub struct While<N = StructuredNode> {
    pub header: Box<N>, pub condition: Option<EntityId>, pub condition_nonzero: bool,
    pub body: Box<N>, pub exit_edge: Option<EntityId>,
} }
strict_record! { pub struct Indirect<N = StructuredNode> {
    pub block: EntityId, pub arms: Vec<IndirectArm<N>>, pub unresolved: bool,
} }
strict_record! { pub struct Dispatcher { pub entry: EntityId, pub cases: Vec<DispatchCase> } }

// A newtype variant delegates its entire payload to deserialize_map through the
// strict record. Deriving a struct variant would also admit positional arrays.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValueKindWire {
    Constant(U256),
    Operation(Operation),
    Parameter(Parameter),
    Memory(OptionalOperation),
    MemoryParameter(Block),
    Effect(OptionalOperation),
    EffectParameter(Block),
}
impl<'de> Deserialize<'de> for ValueKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match ValueKindWire::deserialize(deserializer)? {
            ValueKindWire::Constant(word) => Self::Constant(word),
            ValueKindWire::Operation(Operation { operation, index }) => {
                Self::Operation { operation, index }
            }
            ValueKindWire::Parameter(Parameter { block, index }) => {
                Self::Parameter { block, index }
            }
            ValueKindWire::Memory(OptionalOperation { operation }) => Self::Memory { operation },
            ValueKindWire::MemoryParameter(Block { block }) => Self::MemoryParameter { block },
            ValueKindWire::Effect(OptionalOperation { operation }) => Self::Effect { operation },
            ValueKindWire::EffectParameter(Block { block }) => Self::EffectParameter { block },
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminatorWire {
    Pending,
    Fallthrough(Edge),
    Jump(Jump),
    Branch(Branch),
    Exit(Exit),
    Trap(Trap),
}
impl<'de> Deserialize<'de> for Terminator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match TerminatorWire::deserialize(deserializer)? {
            TerminatorWire::Pending => Self::Pending,
            TerminatorWire::Fallthrough(Edge { edge }) => Self::Fallthrough { edge },
            TerminatorWire::Jump(Jump {
                target,
                edges,
                unresolved,
                invalid_targets,
            }) => Self::Jump {
                target,
                edges,
                unresolved,
                invalid_targets,
            },
            TerminatorWire::Branch(Branch {
                condition,
                target,
                then_edges,
                else_edge,
                unresolved,
                invalid_targets,
            }) => Self::Branch {
                condition,
                target,
                then_edges,
                else_edge,
                unresolved,
                invalid_targets,
            },
            TerminatorWire::Exit(Exit { kind, values }) => Self::Exit { kind, values },
            TerminatorWire::Trap(Trap { reason }) => Self::Trap { reason },
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StructuredKindWire<N> {
    Sequence(Vec<N>),
    Block(Block),
    Transfer(Edge),
    If(If<N>),
    While(While<N>),
    Break,
    Continue,
    Exit(Block),
    PartialExit(Block),
    Indirect(Indirect<N>),
    Dispatcher(Dispatcher),
}
impl<'de, N: Deserialize<'de>> Deserialize<'de> for StructuredKind<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match StructuredKindWire::<N>::deserialize(deserializer)? {
            StructuredKindWire::Sequence(nodes) => Self::Sequence(nodes),
            StructuredKindWire::Block(Block { block }) => Self::Block { block },
            StructuredKindWire::Transfer(Edge { edge }) => Self::Transfer { edge },
            StructuredKindWire::If(If {
                condition,
                then_branch,
                else_branch,
            }) => Self::If {
                condition,
                then_branch,
                else_branch,
            },
            StructuredKindWire::While(While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            }) => Self::While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            },
            StructuredKindWire::Break => Self::Break,
            StructuredKindWire::Continue => Self::Continue,
            StructuredKindWire::Exit(Block { block }) => Self::Exit { block },
            StructuredKindWire::PartialExit(Block { block }) => Self::PartialExit { block },
            StructuredKindWire::Indirect(Indirect {
                block,
                arms,
                unresolved,
            }) => Self::Indirect {
                block,
                arms,
                unresolved,
            },
            StructuredKindWire::Dispatcher(Dispatcher { entry, cases }) => {
                Self::Dispatcher { entry, cases }
            }
        })
    }
}
