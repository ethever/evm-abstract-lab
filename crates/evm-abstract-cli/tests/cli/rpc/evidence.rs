//! Native relative block receipts stay distinct from private PC inventories.

use serde_json::Value as Json;

fn same_frame(left: &Json, right: &Json) -> bool {
    [
        "code_address",
        "code_hash",
        "mode",
        "address",
        "caller",
        "is_static",
    ]
    .iter()
    .all(|field| !left[field].is_null() && left[field] == right[field])
}

pub(super) fn relative_procedure<'a>(
    analysis: &'a Json,
    code: &str,
    owner: &str,
    hash: &Json,
) -> Option<&'a Json> {
    analysis["procedure_summaries"]
        .as_array()?
        .iter()
        .find_map(|record| {
            if record["kind"] != "KnownCall"
                || record["closed"] != true
                || record["source_current"] != true
            {
                return None;
            }
            let procedures = record["procedures"].as_array()?;
            let root = procedures.get(record["root_procedure"].as_u64()? as usize)?;
            let source = analysis["states"]
                .as_array()?
                .iter()
                .find(|state| state["id"] == record["source_state"])?;
            if !same_frame(source["key"]["frames"].as_array()?.last()?, &root["frame"]) {
                return None;
            }
            procedures
                .iter()
                .enumerate()
                .find_map(|(index, procedure)| {
                    let frame = &procedure["frame"];
                    if procedure["semantics"] != "Native"
                        || !procedure["process_input"].is_null()
                        || frame["mode"] != "Runtime"
                        || frame["code_address"] != code
                        || frame["address"] != owner
                        || frame["code_hash"] != *hash
                    {
                        return None;
                    }
                    let covered = procedure["covered_blocks"].as_array()?;
                    record["regional_cfg"]["nodes"]
                        .as_array()?
                        .iter()
                        .any(|node| {
                            let transfer = &node["transfer"];
                            node["procedure"] == index
                                && node["excluded"] == false
                                && same_frame(&node["frame"], frame)
                                && covered.contains(&node["frame"]["basic_block_index"])
                                && transfer["semantics"] == "Native"
                                && transfer["process"].is_null()
                                && transfer["input_revision"] == node["input_revision"]
                                && transfer["current"] == true
                                && transfer["complete"] == true
                                && transfer["visited_block"] == true
                        })
                        .then_some(procedure)
                })
        })
}

pub(super) fn assert_policy(report: &Json, requested: u64, maximum: u64) {
    let hybrid = &report["hybrid"];
    let analysis = report.get("analysis").unwrap_or(report);
    let coverage = hybrid
        .get("precision")
        .filter(|value| !value.is_null())
        .unwrap_or(&hybrid["coverage"]);
    assert_eq!(coverage["requested_detail_regions"], requested);
    assert_eq!(coverage["baseline_current"], true);
    assert_eq!(coverage["detail_current"], true);
    assert_eq!(analysis["config"]["max_work"], maximum);
    let work = &coverage["work"];
    assert_eq!(work["total"], analysis["work"]);
    if let Some(rpc) = hybrid.get("rpc").filter(|value| !value.is_null()) {
        assert_eq!(work["total"], rpc["work"]);
    }
    let sum: u64 = ["prior", "baseline", "preparation", "detail"]
        .iter()
        .map(|field| work[field].as_u64().unwrap())
        .sum();
    assert_eq!(work["total"], sum);
    assert!(sum <= maximum);
}

#[test]
fn relative_evidence_rejects_foreign_stale_and_unexecuted_receipts() {
    use super::{CALLEE, ENTRY, Fixture, RpcServer, call, execute, result};
    use alloy_primitives::{hex, keccak256};
    use serde_json::json;

    std::thread::scope(|scope| {
        let root = format!("{}5000", call(0xfa, 0x200, 0));
        let fixture = Fixture::new(&[(ENTRY, &root), (CALLEE, "602a5f5260205ff3")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &[]), 0);
        let hash = json!(keccak256(hex::decode("602a5f5260205ff3").unwrap()));
        assert!(relative_procedure(&analysis, CALLEE, CALLEE, &hash).is_some());
        assert!(relative_procedure(&analysis, CALLEE, ENTRY, &hash).is_none());
        assert!(relative_procedure(&analysis, CALLEE, CALLEE, &json!(keccak256([]))).is_none());
        for (path, value) in [
            ("/source_current", json!(false)),
            ("/closed", json!(false)),
            ("/procedures/0/semantics", json!("Process")),
            ("/procedures/0/frame/code_hash", json!(keccak256([]))),
            ("/regional_cfg/nodes/0/frame/address", json!(ENTRY)),
            ("/regional_cfg/nodes/0/frame/is_static", json!(false)),
            ("/regional_cfg/nodes/0/transfer/semantics", json!("Process")),
            ("/regional_cfg/nodes/0/transfer/current", json!(false)),
            ("/regional_cfg/nodes/0/transfer/complete", json!(false)),
            ("/regional_cfg/nodes/0/transfer/visited_block", json!(false)),
            ("/regional_cfg/nodes/0/transfer/input_revision", json!(99)),
            (
                "/regional_cfg/nodes/0/frame/code_hash",
                json!(keccak256([])),
            ),
            ("/source_state", json!(u64::MAX)),
        ] {
            let mut changed = analysis.clone();
            for record in changed["procedure_summaries"].as_array_mut().unwrap() {
                *record.pointer_mut(path).unwrap() = value.clone();
            }
            assert!(
                relative_procedure(&changed, CALLEE, CALLEE, &hash).is_none(),
                "{path}"
            );
        }
        super::assert_pinned(&server.finish());
    });
}
