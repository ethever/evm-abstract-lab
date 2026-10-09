//! Both CLI RPC modes inherit the pinned header while retaining explicit inputs.

use super::{ENTRY, Fixture, Reply, RpcServer, execute, result};
use serde_json::json;
use std::thread;

#[test]
fn rpc_header_environment_reaches_discovery_and_preloaded_cli_execution() {
    thread::scope(|scope| {
        for no_discovery in [false, true] {
            for overrides in [false, true] {
                let fixture = Fixture::new(&[(ENTRY, "4342414445484a4600")]);
                let server = RpcServer::new(scope, move |request| {
                    if request["method"] == "eth_feeHistory" {
                        assert_eq!(request["params"], json!(["0x1", "0x10", []]));
                        return Reply::Json(
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"oldestBlock":"0x10","baseFeePerBlobGas":["0x123","0x124"]}}),
                        );
                    }
                    let Reply::Json(mut reply) = fixture.reply(request) else {
                        unreachable!()
                    };
                    if matches!(
                        request["method"].as_str(),
                        Some("eth_getBlockByHash" | "eth_getBlockByNumber")
                    ) {
                        reply["result"]["excessBlobGas"] = json!("0x42");
                        reply["result"]["blobGasUsed"] = json!("0x20000");
                    }
                    Reply::Json(reply)
                });
                let mut args = Vec::new();
                if no_discovery {
                    args.push("--no-rpc-discovery");
                }
                if overrides {
                    args.extend([
                        "--evm.number",
                        "99",
                        "--evm.timestamp",
                        "88",
                        "--evm.basefee",
                        "77",
                        "--evm.blob-basefee",
                        "66",
                        "--evm.chain-id",
                        "55",
                    ]);
                }
                let analysis = result(execute(&server, &args), 0);
                let stack: Vec<_> = analysis["states"][0]["exit_stack"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value["Constants"].clone())
                    .collect();
                assert_eq!(stack[0], json!([if overrides { "0x63" } else { "0x10" }]));
                assert_eq!(stack[1], json!([if overrides { "0x58" } else { "0x1234" }]));
                assert_eq!(stack[2], json!(["0x33"]));
                assert_eq!(stack[3], json!([super::BLOCK]));
                assert_eq!(stack[4], json!(["0x1c9c380"]));
                assert_eq!(stack[5], json!([if overrides { "0x4d" } else { "0x7" }]));
                assert_eq!(stack[6], json!([if overrides { "0x42" } else { "0x123" }]));
                assert_eq!(stack[7], json!([if overrides { "0x37" } else { "0x1" }]));
                assert_eq!(analysis["world"]["snapshot_environment"]["number"], "0x10");
                assert_eq!(
                    analysis["entry"]["environment"]["caller"],
                    json!({"Concrete":"0x0000000000000000000000000000000000001000"})
                );
                let requests = server.finish();
                assert_eq!(
                    requests
                        .iter()
                        .filter(|request| request["method"] == "eth_feeHistory")
                        .count(),
                    1
                );
            }
        }
    });
}
