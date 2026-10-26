"""Qualified graph selection for real default Hybrid browser reports.

The complete retained task reply is saved before selecting a display graph.
Graph IDs and pools are never combined across namespaces.
"""


def graph(report, namespace):
    """Return the actual fixed graph slot; Baseline is the Global wire namespace."""
    return report[{"Baseline": "global", "Native": "native"}[namespace]]


def namespace_available(report, namespace):
    if namespace == "Baseline" and report["minimum"] is not None:
        return report["minimum"]["namespace"] == "Baseline"
    return graph(report, namespace) is not None


def source_program(report, program_id):
    matches = [item for item in report["programs"] if item["id"] == program_id]
    assert len(matches) == 1, (program_id, len(matches))
    return matches[0]


def _assert_program_source(program, source):
    (kind, identity), = source.items()
    assert kind in {"InputProgram", "Runtime", "InitCode"}, kind
    address = identity["source"] if kind == "Runtime" else None
    code_kind = "Initcode" if kind == "InitCode" else "Runtime"
    assert (program["code_address"], program["code_hash"], program["fork"], program["kind"]) == (
        address, identity["hash"], identity["fork"], code_kind)


def assert_root_source(report, metadata):
    """Qualify the original source, independently of other shared catalog rows."""
    if metadata["root_program"] is not None:
        program = source_program(report, metadata["root_program"])
        _assert_program_source(program, metadata["declared_root_source"])


def process_control_relation(report, key, region):
    """Resolve an actual MAY receipt; no location becomes a Native PC or ID."""
    index = region["relation"]
    assert type(index) is int and 0 <= index < len(report["process_controls"]), index
    relation = report["process_controls"][index]
    assert type(relation["id"]) is int and relation["id"] == index
    assert (relation["epoch"], relation["source"], region["entry"]) == (
        key["epoch"], key["source"], key["location"])
    for point in [region["entry"], *region["through"], *region["boundaries"]]:
        if type(point) is dict:
            assert set(point) == {"Block"}, point
            assert type(point["Block"]) is int and 0 <= point["Block"] < len(relation["blocks"]), point
        else:
            assert point in {"JumpTargets", "End"}, point
    if relation["program"] is not None:
        program = source_program(report, relation["program"])
        _assert_program_source(program, relation["source"])
        assert len(relation["blocks"]) == len(program["blocks"])
    else:
        assert "InitCode" in relation["source"]
    return relation


def source_block(report, node):
    """Resolve decoded source without claiming that its instructions executed."""
    if node["program"] is None or node["basic_block"] is None:
        return None
    program = source_program(report, node["program"])
    matches = [item for item in program["blocks"] if item["id"] == node["basic_block"]]
    assert len(matches) == 1, (node, len(matches))
    return matches[0]


def source_instructions(report, node):
    block = source_block(report, node)
    return [] if block is None else block["instructions"]


def oracle_graph(report, namespace):
    """Borrow a graph and shared source directory for graph-level semantic oracles.

    Graph pools and IDs stay in their namespace. Schema and shared source/control
    directories accompany it; publication timing/work/acquisition stay on report.
    Mutations of a wire reply must use graph(), which returns the original slot.
    """
    selected = graph(report, namespace)
    if selected is None:
        return None
    root = selected["metadata"]["root_program"]
    blocks = [] if root is None else source_program(report, root)["blocks"]
    return {**selected, "schema_version": report["schema_version"],
            "programs": report["programs"], "process_controls": report["process_controls"],
            "disassembly": blocks}


def displayed_graph(report):
    return oracle_graph(report, report["selected_view"])


def job_result_report(envelope, *, job_id=None, publication=None):
    """Decode JobResultReply from /api/tasks/{id}/result, never AnalyzeReply."""
    assert type(envelope) is dict
    assert set(envelope) == {"result"}, envelope.keys()
    variant = envelope["result"]
    assert type(variant) is dict
    assert set(variant) == {"Ok"}, variant.keys()
    result = variant["Ok"]
    assert type(result) is dict
    assert set(result) == {"id", "publication", "report"}, result.keys()
    assert type(result["id"]) is int and 0 <= result["id"] < 2**64, result["id"]
    if job_id is not None:
        assert result["id"] == job_id, (result["id"], job_id)
    identity = result["publication"]
    assert type(identity) is dict
    assert set(identity) == {"version", "minimum_version", "global_version", "native_version", "model_closed"}, identity
    for name in ["version", "minimum_version", "global_version", "native_version"]:
        value = identity[name]
        if name != "version" and value is None:
            continue
        assert type(value) is str and value.isascii() and value.isdecimal(), (name, value)
        assert str(int(value)) == value and 0 < int(value) < 2**64, (name, value)
        assert int(value) <= int(identity["version"]), identity
    if publication is not None:
        assert identity == publication, (identity, publication)
    report = result["report"]
    assert type(report) is dict
    assert type(report["schema_version"]) is int and report["schema_version"] == 21
    assert "process_controls" in report
    assert type(report["process_controls"]) is list
    minimum = report["minimum"]
    assert (identity["minimum_version"] is not None) == (minimum is not None), identity
    if minimum is not None:
        assert type(minimum) is dict
        assert minimum["namespace"] == "Baseline", minimum["namespace"]
        assert type(minimum["current"]) is bool
        assert minimum["status"] in {"Converged", "Incomplete"}
    assert (identity["global_version"] is not None) == (report["global"] is not None), identity
    assert (identity["native_version"] is not None) == (report["native"] is not None), identity
    rich_export = report["rich_export"]
    if type(rich_export) is str:
        assert rich_export in {"NotRequested", "Complete", "Cancelled"}, rich_export
    else:
        assert type(rich_export) is dict and set(rich_export) == {"Refused"}
        error = rich_export["Refused"]
        assert type(error) is dict and set(error) == {"code", "message", "details"}
        assert type(error["code"]) is str and type(error["message"]) is str
    assert type(identity["model_closed"]) is bool
    closed = report["status"] == "Converged" and (report["hybrid"] is None
                                                 or report["hybrid"]["coverage"]["baseline_current"])
    closed = closed and (minimum is None or (minimum["current"] and minimum["status"] == "Converged"))
    assert identity["model_closed"] == closed, identity
    assert report["selected_view"] in {"Baseline", "Native"}, report["selected_view"]
    assert namespace_available(report, report["selected_view"]), report["selected_view"]
    return report


def assert_hybrid(report):
    assert report["schema_version"] == 21
    hybrid = report["hybrid"]
    assert isinstance(hybrid, dict)
    coverage = hybrid["coverage"]
    assert report["status"] == ("Converged" if coverage["baseline_current"] else "Incomplete")
    assert coverage["baseline_config"]["root_terminal_coverage"] is False
    baseline = graph(report, "Baseline")
    assert baseline is not None
    native = graph(report, "Native")
    precision = hybrid["precision"] or coverage
    assert baseline["metadata"]["source_snapshot"] == coverage["snapshot"]
    assert baseline["metadata"]["revision"] == coverage["baseline_revision"]
    if native is not None:
        assert native["metadata"]["source_snapshot"] == precision["snapshot"]
        assert native["metadata"]["revision"] == precision["detail_revision"]
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
    if "Namespace" in baseline["metadata"]["root_binding"]:
        coordinate = baseline["metadata"]["root_binding"]["Namespace"]["coordinate"]
        assert baseline["metadata"]["environment"]["to"].get("Symbolic")
        assert_root_source(report, baseline["metadata"])
        assert coverage["chosen"] != "Native"
        if native is not None:
            condition = precision["detail_binding"]["SymbolicOwnerNamespace"]["namespace"]
            assert native["metadata"]["entry_address"] == condition
            assert native["metadata"]["environment"]["to"] == {"Concrete": condition}
