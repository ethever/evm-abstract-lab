//! Finite process identity contains no native adapter address or instruction cursor.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

/// Full 256-bit catalogue or source digest; never a truncated generation counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessDigest(pub [u8; 32]);

impl Serialize for ProcessDigest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl std::fmt::Display for ProcessDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("0x")?;
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ProcessDigest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let bytes = value.as_bytes();
        if bytes.len() != 66 || !bytes.starts_with(b"0x") {
            return Err(D::Error::custom(
                "expected full lowercase 0x-prefixed 256-bit digest",
            ));
        }
        let digit = |byte| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let mut result = [0u8; 32];
        for (out, pair) in result.iter_mut().zip(bytes[2..].as_chunks::<2>().0) {
            let high =
                digit(pair[0]).ok_or_else(|| D::Error::custom("invalid canonical digest"))?;
            let low = digit(pair[1]).ok_or_else(|| D::Error::custom("invalid canonical digest"))?;
            *out = high * 16 + low;
        }
        Ok(Self(result))
    }
}

record! {
    /// Supplied root bytes with no observed physical code account.
    pub struct ProcessInputProgramSource {
        /// Complete supplied-program digest.
        pub hash: ProcessDigest,
        /// Frozen execution rules.
        pub fork: crate::Fork,
    }
}
record! {
    /// Supplied marker whose physical executable remains unobserved.
    pub struct ProcessInputDelegationSource {
        /// Physical pointer encoded in the marker.
        pub target: String,
        /// Original marker digest, not the unknown executable's code hash.
        pub hash: ProcessDigest,
        /// Frozen code-resolution rules.
        pub fork: crate::Fork,
    }
}
record! {
    /// Captured runtime or invalid-delegation source identity, without a byte claim.
    pub struct ProcessCapturedSource {
        /// Actual code account before interpretation.
        pub source: String,
        /// Full captured source hash.
        pub hash: ProcessDigest,
        /// Frozen decoder/execution fork.
        pub fork: crate::Fork,
    }
}
record! {
    /// Captured constructor identity, with no deployed runtime account claim.
    pub struct ProcessInitCodeSource {
        /// Actual initcode hash.
        pub hash: ProcessDigest,
        /// Frozen decoder/execution fork.
        pub fork: crate::Fork,
    }
}
record! {
    /// Unknown or empty executable class under a frozen fork.
    pub struct ProcessForkSource {
        /// Execution rules, independently of source capture.
        pub fork: crate::Fork,
    }
}
record! {
    /// Native interpretation has no fabricated captured program or hash.
    pub struct ProcessPrecompileSource {
        /// Actual native precompile identity.
        pub address: String,
        /// Frozen native execution rules.
        pub fork: crate::Fork,
    }
}
variant! {
    /// Executable origin independently of physical state owner and logical ADDRESS.
    pub enum ProcessSource {
        /// Supplied original root program; no physical source account is asserted.
        InputProgram(ProcessInputProgramSource),
        /// Supplied marker selecting an unobserved physical executable.
        InputDelegation(ProcessInputDelegationSource),
        /// Original captured runtime code.
        Runtime(ProcessCapturedSource),
        /// Captured constructor bytes before deployment.
        InitCode(ProcessInitCodeSource),
        /// Unobserved or future runtime code.
        AnyRuntime(ProcessForkSource),
        /// Arbitrary constructor bytes, without invented hash or Program.
        AnyInitCode(ProcessForkSource),
        /// Actual empty-account execution.
        Empty(ProcessForkSource),
        /// Actual native precompile, without captured-byte evidence.
        Precompile(ProcessPrecompileSource),
        /// Invalid second delegation marker, retaining source identity.
        InvalidDelegation(ProcessCapturedSource),
    }
}
record! {
    /// A finite process key, with no concrete residual owner or private cursor.
    pub struct ProcessFrameKey {
        /// Full owner/catalogue epoch, separate from receipt input revisions.
        pub epoch: ProcessDigest,
        /// Physical state class; residual does not imply equality between instances.
        pub owner: super::ProcessOwner,
        /// Honest captured, unknown, constructor or native source identity.
        pub source: ProcessSource,
        /// Propagated read-only restriction.
        pub is_static: bool,
        /// MAY source coordinate, never a Native executed block or PC.
        pub location: super::ProcessLocation,
    }
}

impl ProcessFrameKey {
    /// One procedure context may contain multiple admitted Body locations.
    pub fn same_context(&self, other: &Self) -> bool {
        self.epoch == other.epoch
            && self.owner == other.owner
            && self.source == other.source
            && self.is_static == other.is_static
    }
}
record! {
    /// Actual abstract invocation operands, never reconstructed from adapter anchors.
    pub struct ProcessOperands {
        /// Logical ADDRESS value.
        pub address: crate::ValueInfo,
        /// Actual CALLER value.
        pub caller: crate::ValueInfo,
        /// Actual CALLVALUE.
        pub value: crate::ValueInfo,
    }
}
record! {
    /// Constructor continuation coordinates; nonce/endowment remain joined facts.
    pub struct ProcessCreation {
        /// CREATE or CREATE2 action.
        pub action: super::ProcessAction,
        /// Active execution owner, independently of delegated code source.
        pub creator: super::ProcessOwner,
        /// Actual named or residual destination class.
        pub destination: super::ProcessOwner,
        /// Full catalogue currency for this constructor dependency.
        pub epoch: ProcessDigest,
    }
}
record! {
    /// Ordinary native deployment retains its concrete destination.
    pub struct NativeCreation {
        /// Actual native account being created.
        pub destination: String,
    }
}
variant! {
    /// Native deployment addresses and process destination classes stay distinct.
    pub enum CreationContinuation {
        /// Ordinary native initcode deployment.
        Native(NativeCreation),
        /// Class-valued process construction before deployment.
        Process(ProcessCreation),
    }
}

impl ProcessSource {
    /// Frozen execution rules for captured, unknown and native sources alike.
    pub fn fork(&self) -> crate::Fork {
        match self {
            Self::InputProgram(source) => source.fork,
            Self::InputDelegation(source) => source.fork,
            Self::Runtime(source) | Self::InvalidDelegation(source) => source.fork,
            Self::InitCode(source) => source.fork,
            Self::AnyRuntime(source) | Self::AnyInitCode(source) | Self::Empty(source) => {
                source.fork
            }
            Self::Precompile(source) => source.fork,
        }
    }

    /// Executable kind from the public source, without consulting a native anchor.
    pub fn kind(&self) -> crate::CodeKind {
        match self {
            Self::InputProgram(_) | Self::Runtime(_) => crate::CodeKind::Runtime,
            Self::InputDelegation(_) => crate::CodeKind::AbstractEnvironment,
            Self::InitCode(_) | Self::AnyInitCode(_) => crate::CodeKind::Initcode,
            Self::AnyRuntime(_) => crate::CodeKind::AbstractEnvironment,
            Self::Empty(_) => crate::CodeKind::Empty,
            Self::Precompile(_) => crate::CodeKind::Precompile,
            Self::InvalidDelegation(_) => crate::CodeKind::InvalidDelegation,
        }
    }

    /// Interpretation required by this executable source.
    pub fn semantics(&self) -> super::TransferSemantics {
        match self {
            Self::InputProgram(_) | Self::Runtime(_) => super::TransferSemantics::KnownProcess,
            Self::InputDelegation(_) => super::TransferSemantics::Environment,
            Self::InitCode(_) | Self::AnyInitCode(_) => {
                super::TransferSemantics::ConstructorProcess
            }
            Self::AnyRuntime(_) => super::TransferSemantics::Environment,
            Self::Empty(_) | Self::Precompile(_) | Self::InvalidDelegation(_) => {
                super::TransferSemantics::BoundaryProcess
            }
        }
    }
}

choice! {
    /// Invalid process representation, without interpreting semantic closure.
    pub enum ProcessValidationKind {
        /// Source, staticness and interpretation disagree.
        Identity,
        /// A process exposes private native anchor/cursor/array fields.
        PrivateState,
        /// Actual logical operands are absent or inconsistent.
        Operands,
        /// Program identity is inconsistent with its captured source.
        Source,
        /// Constructor descriptor or delivered result is structurally inconsistent.
        Creation,
        /// Action scope is inconsistent with its primitive action.
        Scope,
        /// A MAY directory, location or actual macro receipt is inconsistent.
        Control,
    }
}
