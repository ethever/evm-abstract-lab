//! RPC acquisition and explain preserve local reasoning and representation bounds.

use super::{BLOCK, ENTRY, Fixture, RpcServer, assert_pinned, execute, result};
use crate::run_concrete;
use std::thread;

#[test]
fn rpc_analyze_and_explain_keep_local_relation_bounds() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "00")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let policy = [
            "--max-symbolic-nodes",
            "4096",
            "--max-symbolic-depth",
            "128",
            "--max-relations",
            "1024",
        ];
        let analysis = result(execute(&server, &policy), 0);
        assert_eq!(
            analysis["config"]["analysis"]["relations"],
            serde_json::json!({
                "enabled": true, "max_nodes": 4096, "max_depth": 128, "max_constraints": 1024,
            })
        );
        for verbose in [false, true] {
            let mut args = vec![
                "explain",
                "--rpc",
                &server.endpoint,
                "--block-hash",
                BLOCK,
                "--evm.to",
                ENTRY,
            ];
            args.extend_from_slice(&policy);
            if verbose {
                args.push("--verbose");
            }
            let output = run_concrete(&args);
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(
                text.contains(
                    "relations=true | local propagation | expression nodes=4096 | depth=128 | constraints=1024"
                )
            );
            assert!(!text.contains("SMT=") && !text.contains("rlimit="));
        }
        let requests = server.finish();
        assert_pinned(&requests);
    });
}
