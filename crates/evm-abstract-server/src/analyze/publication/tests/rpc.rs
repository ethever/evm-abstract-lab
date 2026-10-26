//! Real fixed-snapshot restrictions must survive an optional presentation refusal.

use super::*;
use crate::analyze::{input, report};
use axum::{Json, Router, routing::post};
use evm_abstract::{Address, analysis, world::EvmEntry};
use std::{sync::Arc, thread};

struct RpcServer {
    endpoint: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl RpcServer {
    fn new<'scope>(scope: &'scope thread::Scope<'scope, '_>) -> Self {
        let (endpoint, receive) = std::sync::mpsc::sync_channel(1);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        scope.spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                endpoint
                    .send(format!("http://{}", listener.local_addr().unwrap()))
                    .unwrap();
                axum::serve(listener, Router::new().route("/", post(reply)))
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
        });
        Self {
            endpoint: receive.recv().unwrap(),
            stop: Some(stop),
        }
    }
}

impl Drop for RpcServer {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
    }
}

async fn reply(Json(request): Json<serde_json::Value>) -> Json<serde_json::Value> {
    let result = match request["method"].as_str().unwrap() {
        "eth_chainId" => serde_json::json!("0x1"),
        "eth_getBlockByNumber" | "eth_getBlockByHash" => serde_json::json!({
            "hash": format!("0x{}", "11".repeat(32)),
            "parentHash": format!("0x{}", "10".repeat(32)),
            "number": "0x2a", "timestamp": "0x1234",
            "miner": format!("0x{}", "33".repeat(20)),
            "mixHash": format!("0x{}", "44".repeat(32)),
            "gasLimit": "0x1c9c380", "baseFeePerGas": "0x7",
            "sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
            "stateRoot": format!("0x{}", "00".repeat(32)),
            "transactionsRoot": format!("0x{}", "00".repeat(32)),
            "receiptsRoot": format!("0x{}", "00".repeat(32)),
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "difficulty": "0x0", "gasUsed": "0x0", "extraData": "0x",
            "nonce": "0x0000000000000000", "transactions": [], "uncles": []
        }),
        method => {
            let params = request["params"].as_array().unwrap();
            let pin = params.last().unwrap();
            assert_eq!(pin["requireCanonical"], true);
            assert_eq!(pin["blockHash"], format!("0x{}", "11".repeat(32)));
            match method {
                "eth_getCode" => serde_json::json!("0x5f54565b5b00"),
                "eth_getBalance" | "eth_getTransactionCount" => serde_json::json!("0x0"),
                "eth_getStorageAt" => serde_json::json!(format!("0x{:064x}", 4)),
                other => panic!("unexpected RPC request {other}"),
            }
        }
    };
    Json(serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))
}

struct Projected {
    early: Arc<api::AnalysisReport>,
    result: Result<api::AnalysisReport, api::ApiError>,
    spent: usize,
}

fn project(endpoint: &str, limit: usize) -> Projected {
    let address = Address::from_word(evm_abstract::U256::from(0x101).into());
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Rpc(api::RpcInput {
            provider_id: "fixture".into(),
            address: address.to_string(),
            block: api::BlockSelector::Latest,
            accounts: Vec::new(),
        }),
        ..Default::default()
    };
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
        input::spec(&request.limits).unwrap(),
    ));
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let mut environment = input::environment(&request.environment).unwrap();
    environment.to = address.into();
    let mut budget = WorkBudget::new(limit);
    let api::AnalysisInput::Rpc(rpc) = &request.input else {
        unreachable!()
    };
    let rpc = input::rpc(rpc, endpoint, input::fork(request.fork), &request.limits).unwrap();
    let mut staged = Projection::new(
        &request,
        api::AnalysisScope::RpcWorld,
        &algebra,
        api::NumericModel::Product,
        &control,
        None,
        &mut budget,
    );
    let source = analysis::analyze_rpc_hybrid_with_publisher(
        AnalysisEngine::new(&algebra, &policies, &mut extension),
        &rpc,
        EvmEntry {
            address,
            environment,
        },
        input::config(&request.limits).unwrap(),
        HybridConfig::default(),
        &control,
        &mut staged,
    )
    .map_err(crate::analyze::rpc_error);
    let source = staged.resolve(source).unwrap();
    let early = staged.finish(source.baseline()).unwrap().unwrap();
    assert!(early.hybrid.as_ref().unwrap().rpc.is_none());
    let actual = source.coverage();
    assert_ne!(
        actual.baseline_revision,
        actual.current_revision,
        "{actual:?}; latest={:?}",
        source.latest().coverage()
    );
    assert_eq!(
        actual.baseline_relation,
        analysis::RpcBaselineRelation::InitialFactRestriction
    );
    assert!(!actual.restrictions.is_empty());
    let revision = actual.current_revision.value();
    let restrictions = actual.restrictions.len();
    let acquisition = actual.acquisition.requests;
    let work = source.work() as u64;
    let result = report::hybrid::rpc::project_with_retained_global(
        source,
        &algebra,
        &control,
        &mut budget,
        report::ProjectionInput {
            request: &request,
            scope: api::AnalysisScope::RpcWorld,
            numeric_model: api::NumericModel::Product,
        },
        Some(&early),
    );
    if let Ok(report) = &result {
        let rpc = report.hybrid.as_ref().unwrap().rpc.as_ref().unwrap();
        assert_eq!(rpc.current_revision, api::DecimalU64(revision));
        assert_eq!(rpc.restrictions.len(), restrictions);
        assert_eq!(rpc.acquisition.requests, acquisition);
        assert_eq!(rpc.work, work);
        assert_eq!(report.work, work);
        assert_eq!(report.acquisition.as_ref().unwrap(), &rpc.acquisition);
        assert_eq!(
            report.minimum.as_ref().unwrap().revision,
            rpc.baseline_revision
        );
        assert!(Arc::ptr_eq(
            early.minimum.as_ref().unwrap(),
            report.minimum.as_ref().unwrap()
        ));
        report.validate().unwrap();
    }
    Projected {
        early,
        result,
        spent: budget.used(),
    }
}

#[test]
fn final_rpc_refusal_keeps_actual_restrictions_and_required_metadata_failure_stays_error() {
    // The scope joins the server and propagates a panic after this guard sends
    // shutdown, without exposing a dynamically typed panic payload to the test.
    thread::scope(|scope| rpc_refusal(scope));
}

fn rpc_refusal<'scope>(scope: &'scope thread::Scope<'scope, '_>) {
    let server = RpcServer::new(scope);
    let measured = project(&server.endpoint, 2_000_000);
    let full = measured.result.unwrap();
    assert!(matches!(full.rich_export, api::RichExportOutcome::Complete));
    let minimum_work = measured.early.postprocessing_work as usize;
    let full_work = full.postprocessing_work as usize;
    assert!(full_work > minimum_work + 1);
    let limit = minimum_work + (full_work - minimum_work) / 2;
    let refused = project(&server.endpoint, limit);
    let final_report = refused.result.unwrap();
    assert!(matches!(
        final_report.rich_export,
        api::RichExportOutcome::Refused(_)
    ));
    assert!(final_report.global.is_none() && final_report.native.is_none());
    assert!(refused.spent <= limit);
    let required = project(&server.endpoint, minimum_work);
    assert_eq!(required.result.unwrap_err().code, api::ApiErrorCode::Export);
    assert_eq!(required.spent, minimum_work);
    assert!(required.early.minimum.as_ref().unwrap().current);
    required.early.validate().unwrap();
}
