//! Shared source geometry and actual MAY receipt coordinates are distinct.
variant! {
    /// A conservative coordinate in an exact source/epoch, without a PC claim.
    #[derive(Copy, PartialOrd, Ord)]
    pub enum ProcessLocation {
        /// Unobserved code's arbitrary repeating actions.
        Any,
        /// Dense decoded block index in the captured source program.
        Block(usize),
        /// One set-valued union of all legal decoded JUMPDEST blocks.
        JumpTargets,
        /// Successful code fallthrough beyond the final source instruction.
        End,
    }
}
record! {
    /// A source relation edge, independent of admitted table or execution IDs.
    pub struct ProcessControlEdge {
        /// MAY source coordinate under the same source and epoch.
        pub to: ProcessLocation,
        /// Covering local transfer, without an executed-instruction claim.
        pub kind: crate::EdgeKind,
    }
}
record! {
    /// One dense decoded block's covering local relation.
    pub struct ProcessControlBlock {
        /// At most two normal local successors in original interpreter order.
        pub successors: [Option<ProcessControlEdge>; 2],
        /// Potential return coordinate; actual delivery is a regional receipt.
        pub continuation: Option<ProcessLocation>,
        /// Effects/calls/creation/exits require an actual numerical Body transfer.
        pub boundary: bool,
    }
}
record! {
    /// Immutable source/epoch directory row; presence never proves reachability.
    pub struct ProcessControlRelation {
        /// Report-local append-only directory identity.
        pub id: usize,
        /// Exact frozen source vocabulary, independent of publication versions.
        pub epoch: super::ProcessDigest,
        /// Complete source/fork/hash identity, independent of physical owner.
        pub source: super::ProcessSource,
        /// Matching captured source in the instruction catalog, when available.
        /// None qualifies unavailable constructor bytes, without claiming
        /// that the current producer supports captured constructor admission.
        pub program: Option<usize>,
        /// Dense Program.blocks order; no instruction payload is repeated.
        pub blocks: Vec<ProcessControlBlock>,
        /// All legal decoded JUMPDEST indices, excluding PUSH payload bytes.
        pub jump_targets: Vec<usize>,
    }
}
record! {
    /// Actual retained pure composition for one regional Body input.
    pub struct ProcessControlRegion {
        /// Index in AnalysisReport.process_controls, shared across contexts.
        pub relation: usize,
        /// The actual Body's ProcessFrameKey.location.
        pub entry: ProcessLocation,
        /// Pure MAY locations composed with the unchanged Store/invocation.
        pub through: Vec<ProcessLocation>,
        /// Actual emitted boundary obligations; their effects need Body receipts.
        pub boundaries: Vec<ProcessLocation>,
    }
}

impl super::ProcessSource {
    /// Match captured bytes by their complete role/address/hash/fork identity.
    pub fn matches_program(&self, program: &crate::ProgramInfo) -> bool {
        if self.fork() != program.fork {
            return false;
        }
        match self {
            Self::InputProgram(source) => {
                program.code_address.is_none()
                    && program.kind == crate::CodeKind::Runtime
                    && program.code_hash == source.hash.to_string()
            }
            Self::Runtime(source) => {
                program.code_address.as_ref() == Some(&source.source)
                    && program.kind == crate::CodeKind::Runtime
                    && program.code_hash == source.hash.to_string()
            }
            Self::InitCode(source) => {
                program.code_address.is_none()
                    && program.kind == crate::CodeKind::Initcode
                    && program.code_hash == source.hash.to_string()
            }
            _ => false,
        }
    }
}

impl crate::AnalysisReport {
    /// Resolve MAY source bytes without interpreting a location as a visit.
    pub fn process_program(&self, source: &super::ProcessSource) -> Option<&crate::ProgramInfo> {
        self.programs
            .iter()
            .map(AsRef::as_ref)
            .find(|program| source.matches_program(program))
    }

    /// Navigate captured MAY source bytes; this never supplies executed PCs.
    pub fn process_source_block(
        &self,
        key: &super::ProcessFrameKey,
    ) -> Option<crate::SourceBlockRef> {
        let ProcessLocation::Block(index) = key.location else {
            return None;
        };
        let program = self.process_program(&key.source)?;
        Some(crate::SourceBlockRef {
            program: program.id,
            basic_block: program.blocks.get(index)?.id,
        })
    }
}
