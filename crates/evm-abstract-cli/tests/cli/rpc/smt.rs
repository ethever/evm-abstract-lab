//! RPC acquisition and the explain adapter retain the selected SMT policy.

use super::{BLOCK, ENTRY, Fixture, RpcServer, assert_pinned, execute, result};
use crate::run_concrete;
use std::thread;

#[test]
fn rpc_analyze_and_explain_keep_the_selected_provider_and_allowance() {
    thread::scope(|scope| {
        for provider in ["z3", "bitwuzla", "cvc5"] {
            let fixture = Fixture::new(&[(ENTRY, "00")]);
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let policy = ["--smt.provider", provider, "--smt.rlimit", "200000"];
            let analysis = result(execute(&server, &policy), 0);
            let reported = &analysis["config"]["analysis"]["relations"];
            assert_eq!(reported["provider"], provider);
            assert_eq!(reported["rlimit"], 200_000);

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
                assert!(text.contains(&format!("SMT=in-process {provider} | rlimit=200000")));
            }
            let requests = server.finish();
            assert_pinned(&requests);
        }
    });
}
