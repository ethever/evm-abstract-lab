//! Compare complete state and attempted host requests, including the first Err.
use evm_decompile::{
    Artifact, ExecutionError, ExecutionLimits, ExecutionOutcome, Input, MemoryHost, Primitive,
    SemanticHost, execute_bytecode, execute_ir, execute_structured,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub opcode: u8,
    pub inputs: Vec<evm_decompile::U256>,
    pub output_count: usize,
    pub memory_limit: usize,
    pub result: Result<Vec<evm_decompile::U256>, ExecutionError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RecordingHost {
    pub state: MemoryHost,
    pub invocations: Vec<Invocation>,
}

impl RecordingHost {
    pub fn new(state: MemoryHost) -> Self {
        Self {
            state,
            invocations: Vec::new(),
        }
    }
}

impl SemanticHost for RecordingHost {
    fn execute(
        &mut self,
        primitive: Primitive<'_>,
    ) -> Result<Vec<evm_decompile::U256>, ExecutionError> {
        let mut invocation = Invocation {
            opcode: primitive.opcode,
            inputs: primitive.inputs.to_vec(),
            output_count: primitive.output_count,
            memory_limit: primitive.memory_limit,
            result: Ok(Vec::new()),
        };
        let result = self.state.execute(primitive);
        invocation.result = result.clone();
        self.invocations.push(invocation);
        result
    }
}

pub(super) fn source_execution(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
    limits: &ExecutionLimits,
) -> (
    Result<ExecutionOutcome, ExecutionError>,
    RecordingHost,
    Option<evm_decompile::Execution>,
) {
    let mut raw_host = RecordingHost::new(host.clone());
    let raw = execute_bytecode(input, &mut raw_host, limits);
    let outcome = raw
        .as_ref()
        .map(|execution| execution.outcome.clone())
        .map_err(Clone::clone);
    let stage = artifact.current().unwrap();
    let mut graph_host = RecordingHost::new(host.clone());
    let graph = execute_ir(stage, &mut graph_host, limits);
    let mut structured_host = RecordingHost::new(host);
    let structured = execute_structured(stage, &mut structured_host, limits);
    for (label, execution, actual_host) in [
        ("IR", graph, graph_host),
        ("Structured", structured, structured_host),
    ] {
        assert_eq!(
            execution.as_ref().map(|execution| &execution.outcome),
            outcome.as_ref(),
            "{label} outcome"
        );
        assert_eq!(actual_host, raw_host, "{label} complete state and attempts");
        if let (Ok(raw), Ok(execution)) = (&raw, execution) {
            assert_eq!(
                raw.observations(),
                execution.observations(),
                "{label} trace"
            );
        }
    }
    (outcome, raw_host, raw.ok())
}
