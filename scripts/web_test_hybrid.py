"""Qualified graph selection for real default Hybrid browser reports.

The complete retained task reply is saved before selecting a display graph.
Graph IDs and pools are never combined across namespaces.
"""


def graph(report, namespace):
    hybrid = report.get("hybrid")
    if hybrid is None:
        return report if namespace == "Native" else None
    primary = "Native" if hybrid["coverage"]["chosen"] == "Native" else "Baseline"
    return report if namespace == primary else hybrid["baseline" if namespace == "Baseline" else "detail"]


def displayed_graph(report):
    hybrid = report.get("hybrid")
    if hybrid is None:
        return report
    namespace = "Native" if hybrid["local_prefixes"] or hybrid["coverage"]["chosen"] == "Native" else "Baseline"
    return graph(report, namespace)


def assert_hybrid(report):
    assert report["schema_version"] == 20
    hybrid = report["hybrid"]
    assert isinstance(hybrid, dict)
    coverage = hybrid["coverage"]
    assert report["status"] == ("Converged" if coverage["baseline_current"] else "Incomplete")
    assert coverage["baseline_config"]["root_terminal_coverage"] is False
    native = graph(report, "Native")
    precision = hybrid["precision"] or coverage
    assert len(hybrid["local_prefixes"]) == precision["local_detail_sites"]
    for prefix in hybrid["local_prefixes"]:
        assert prefix["namespace"] == "Native"
        assert prefix["binding"] == precision["detail_binding"]
        assert prefix["snapshot"] == precision["snapshot"]
        assert prefix["revision"] == precision["detail_revision"]
        state = next(state for state in native["states"] if state["state"] == prefix["state"])
        assert prefix["frame"]["process"] is None and state["entry"]["frames"][-1]["process"] is None
        block = next(block for block in native["cfg"] if block["id"] == prefix["state"])
        assert [instruction["pc"] for instruction in prefix["instructions"]] == block["executed_pcs"]
        assert all(instruction["phase"] in {"Started", "OperandsConsumed", "Completed", "Dispatched", "Faulted"}
                   for instruction in prefix["instructions"])
        assert prefix["frontiers"] == [index for index, frontier in enumerate(native["frontiers"])
                                        if frontier["from"] == prefix["state"]]
    if "Namespace" in report["metadata"]["root_binding"]:
        coordinate = report["metadata"]["root_binding"]["Namespace"]["coordinate"]
        assert report["metadata"]["environment"]["to"].get("Symbolic")
        assert all(program["code_address"] != coordinate for program in report["programs"])
        assert coverage["chosen"] != "Native"
        if native is not None:
            condition = precision["detail_binding"]["SymbolicOwnerNamespace"]["namespace"]
            assert native["metadata"]["entry_address"] == condition
            assert native["metadata"]["environment"]["to"] == {"Concrete": condition}
