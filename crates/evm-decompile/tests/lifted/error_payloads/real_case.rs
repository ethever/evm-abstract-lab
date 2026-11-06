use super::*;

fn router_bytes() -> Vec<u8> {
    let hex = include_str!("../../fixtures/error-payloads/ethereum-uniswap-v2-router02.hex").trim();
    evm_bytecode::parse_hex(hex).unwrap()
}

#[test]
fn pinned_router_fixture_contains_the_actual_twenty_plus_twelve_mstore_word() {
    let bytes = router_bytes();
    assert_eq!(bytes.len(), 21_943);
    assert_eq!(
        format!("{:x}", alloy_primitives::keccak256(&bytes)),
        "a324bc7db3d091b6f1a2d526e48a9c7039e03b3cc35f7d44b15ac7a1544c11d2"
    );
    assert_eq!(&bytes[..5], &[0x60, 0x80, 0x60, 0x40, 0x52]);
    assert_eq!(bytes[21_064], 0x7f);
    assert_eq!(&bytes[21_065..21_085], DS_MATH);
    assert_eq!(&bytes[21_085..21_097], &[0_u8; 12]);
    assert_eq!(bytes[15_559], 0x7f);
    assert_eq!(&bytes[15_560..15_591], TRANSFER);
    assert_eq!(bytes[15_591], 0);
}

#[test]
fn actual_router_constructor_slices_execute_and_certify_with_the_actual_initial_prefix() {
    let bytes = router_bytes();
    for (start, end, message) in [(21_011, 21_113, DS_MATH), (15_506, 15_608, TRANSFER)] {
        let wrapped = evm_bytecode::parse_hex(if start == 21_011 {
            include_str!("../../fixtures/error-payloads/router-ds-math-guard.hex")
        } else {
            include_str!("../../fixtures/error-payloads/router-transfer-guard.hex")
        })
        .unwrap();
        assert_eq!(&wrapped[..5], &bytes[..5]);
        assert_eq!(
            &wrapped[12..114],
            &bytes[start..end],
            "CLI / HTTP wrapper changed the actual constructor bytes"
        );
        // These exact source instruction slices have no relocatable control
        // operands. This checks the actual compiler's constructor, while the
        // whole-router coverage probe separately checks reaching prefix paths.
        let input = evm_decompile::Input::new(
            [&bytes[..5], &bytes[start..end]].concat(),
            evm_decompile::Fork::Osaka,
        );
        let artifact = checked(&input);
        assert_message(&artifact, message);
        let (outcome, host, capsules) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, Vec::new()),
            1_048_576,
        );
        assert_eq!(
            oracle::reverted(&outcome.unwrap(), &host),
            Some(oracle::abi(message))
        );
        assert_eq!(capsules, 1);
    }
}
