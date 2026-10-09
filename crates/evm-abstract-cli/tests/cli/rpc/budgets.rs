//! Default response allocation reaches the actual RPC client, across both commands.
use super::{ENTRY, Fixture, Reply, RpcServer, run_concrete};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

#[test]
fn analyze_and_explain_accept_responses_above_the_old_limit_and_honor_explicit_old_limits() {
    for command in ["analyze", "explain"] {
        for explicit_old_limit in [false, true] {
            thread::scope(|scope| {
                let fixture = Fixture::new(&[(ENTRY, "00")]);
                let padded = AtomicBool::new(false);
                let server = RpcServer::new(scope, move |request| {
                    let Reply::Json(reply) = fixture.reply(request) else {
                        unreachable!()
                    };
                    if request["method"] == "eth_chainId" && !padded.swap(true, Ordering::Relaxed) {
                        let mut body = serde_json::to_vec(&reply).unwrap();
                        body.resize(4 * 1024 * 1024 + 128, b' ');
                        Reply::BytesWithPeerAbort(body)
                    } else {
                        Reply::Json(reply)
                    }
                });
                let mut args = vec![command, "--rpc", &server.endpoint, "--evm.to", ENTRY];
                if explicit_old_limit {
                    args.extend(["--max-rpc-response-bytes", "4194304"]);
                }
                let output = run_concrete(&args);
                if explicit_old_limit {
                    assert_eq!(output.status.code(), Some(1));
                    assert!(output.stdout.is_empty());
                    let error = String::from_utf8(output.stderr).unwrap();
                    assert!(
                        error.contains("4194304") && error.contains("eth_chainId"),
                        "{error}"
                    );
                } else {
                    assert!(
                        output.status.success(),
                        "{command}: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert!(
                        String::from_utf8(output.stdout)
                            .unwrap()
                            .contains("Converged")
                    );
                }
                let requests = server.finish();
                if explicit_old_limit {
                    assert_eq!(
                        requests.len(),
                        1,
                        "oversized initial response must stop before account reads"
                    );
                } else {
                    assert!(
                        requests
                            .iter()
                            .any(|request| request["method"] == "eth_getCode")
                    );
                }
            });
        }
    }
}
