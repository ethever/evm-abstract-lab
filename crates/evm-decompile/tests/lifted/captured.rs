use super::oracle::certify;
use super::support::checked;
use super::{assert_unchanged_witness, lifted, structured};
use evm_bytecode::Program;
use evm_decompile::{Artifact, ExternalResponse, Fork, Input, MemoryHost, U256};

const USDC: &str =
    include_str!("../../../../examples/known-recursion/captured/usdc-26180958-observed-world.json");
const ADMIN_SLOT: &str = "10d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b";

fn word(hex: &str) -> U256 {
    U256::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap()
}

fn proxy() -> Input {
    let capture: serde_json::Value = serde_json::from_str(USDC).unwrap();
    let decoded = Program::from_hex_with_fork(
        capture["accounts"][1]["code"].as_str().unwrap(),
        Fork::Osaka,
    )
    .unwrap();
    Input::new(decoded.bytes().to_vec(), Fork::Osaka)
}

fn host(input: &Input, data: Vec<u8>, admin: bool) -> MemoryHost {
    let capture: serde_json::Value = serde_json::from_str(USDC).unwrap();
    let account = &capture["accounts"][1];
    let mut host = MemoryHost::new(input, data);
    host.storage = account["storage"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (word(key), word(value.as_str().unwrap())))
        .collect();
    let caller = if admin {
        host.storage[&word(ADMIN_SLOT)]
    } else {
        U256::from(0xcafe)
    };
    host.context
        .insert(0x30, word(account["address"].as_str().unwrap()));
    host.context.insert(0x33, caller);
    host.context.insert(0x3b, U256::from(1));
    host.context.insert(0x5a, U256::from(1_000_000));
    host
}

#[test]
fn captured_usdc_pop_and_return_address_tuples_are_cleaned_in_high_layer() {
    let input = proxy();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let prior = structured(&artifact);
    // PC 0x303 is the upgrade-and-call return shim: three POPs followed by the
    // known return JUMP at 0x307. Find its actual context, never an allocated ID.
    let shims: Vec<_> = prior
        .ir
        .blocks
        .iter()
        .filter(|block| block.source.is_some_and(|source| source.pc == 0x303))
        .collect();
    assert!(!shims.is_empty(), "captured return shim missing");
    for block in shims {
        eprintln!(
            "USDC return shim: source PC 0x303, context block {:?}, primitive parameters {:?}, high parameters {:?}",
            block.id,
            block.parameters,
            program.block(block.id).unwrap().parameters
        );
        let pops: Vec<_> = block
            .operations
            .iter()
            .filter(|op| op.opcode == 0x50)
            .collect();
        assert_eq!(pops.len(), 3);
        assert!(!block.parameters.is_empty());
        assert!(
            pops.iter().all(|pop| pop
                .inputs
                .iter()
                .any(|input| block.parameters.contains(input))),
            "Structured must retain physical POP uses"
        );
        let evm_decompile::Terminator::Jump {
            target,
            edges,
            unresolved: false,
            ..
        } = &block.terminator
        else {
            panic!("resolved return context");
        };
        assert_eq!(edges.len(), 1);
        assert_eq!(
            prior.ir.value(*target).unwrap().fact.singleton(),
            Some(U256::from(0x106))
        );
        let high = program.block(block.id).unwrap();
        assert!(
            high.parameters.is_empty(),
            "POP/return-only tuple survives high code at PC 0x303: {:?}",
            high.parameters
        );
        assert!(high.target.is_none());
        assert!(high.statements.is_empty());
        for incoming in stage.ir.edges.iter().filter(|edge| edge.to == block.id) {
            assert!(program.transfer(incoming.id).unwrap().arguments.is_empty());
            assert!(!prior.ir.edge(incoming.id).unwrap().arguments.is_empty());
        }
    }
    // Other helper POP-only slots may disappear, but slots feeding real memory,
    // storage, calls and control must still be checked by the primitive oracle.
    for (data, admin) in [
        (vec![0xf8, 0x51, 0xa4, 0x40], true),
        (vec![0x5c, 0x60, 0xda, 0x1b], true),
    ] {
        certify(&input, &artifact, host(&input, data, admin));
    }
    let implementation = U256::from(0x1234);
    let payload = [0xde, 0xad, 0xbe, 0xef, 0x42];
    let mut data = vec![0x4f, 0x1e, 0xf2, 0x86];
    for argument in [implementation, U256::from(64), U256::from(payload.len())] {
        data.extend(argument.to_be_bytes::<32>());
    }
    data.extend(payload);
    data.resize(4 + 96 + payload.len().div_ceil(32) * 32, 0);
    for success in [false, true] {
        let mut caller = host(&input, data.clone(), true);
        caller.context.insert(0x34, U256::from(7));
        caller.responses.push_back(ExternalResponse {
            result: U256::from(u64::from(success)),
            return_data: vec![0x99, 0x88],
            ..ExternalResponse::default()
        });
        let (raw, observed) = certify(&input, &artifact, caller);
        assert_eq!(
            raw.trace.iter().filter(|step| step.opcode == 0xf1).count(),
            1
        );
        assert!(observed.responses.is_empty());
        assert_eq!(raw.trace.iter().any(|step| step.pc == Some(0x307)), success);
        let mut fallback = host(&input, vec![0x12, 0x34, 0x56, 0x78, 9], false);
        fallback.responses.push_back(ExternalResponse {
            result: U256::from(u64::from(success)),
            return_data: vec![0xc0, 0xff, 0xee],
            ..ExternalResponse::default()
        });
        certify(&input, &artifact, fallback);
    }
    assert_unchanged_witness(&artifact);
    let restored: Artifact =
        serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    assert_eq!(artifact, restored);
}
