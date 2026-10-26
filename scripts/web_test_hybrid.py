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
    assert type(report["programs"]) is list
    assert type(program_id) is int and program_id >= 0, program_id
    for item in report["programs"]:
        assert type(item) is dict and type(item["id"]) is int
    matches = [item for item in report["programs"] if item["id"] == program_id]
    assert len(matches) == 1, (program_id, len(matches))
    return matches[0]


def native_entry_context(report):
    """Resolve the actual root invocation for an explicit Native-context pick.

    This is a test consumer of retained frame evidence. A source row, a MAY
    receipt or an arbitrary state ordinal cannot supply the missing context.
    """
    native = graph(report, "Native")
    assert native is not None, "no Native publication"
    metadata = native["metadata"]
    root = metadata["root_program"]
    program = None if root is None else source_program(report, root)
    first = None if program is None or not program["blocks"] else program["blocks"][0]
    assert all(type(state["state"]) is int and state["state"] >= 0 for state in native["states"])
    snapshots = {state["state"]: state for state in native["states"]}
    assert len(snapshots) == len(native["states"]), "duplicate Native snapshots"
    candidates = []
    for node in native["cfg"]:
        assert type(node["id"]) is int and node["id"] >= 0, node
        if (node["process"] is not None or node["program"] != root
                or node["frame_depth"] != 1 or node["context"] != [] or node["entry_stack"] != []
                or node["basic_block"] != (None if first is None else first["id"])
                or node["start_pc"] != (None if first is None else first["start_pc"])):
            continue
        snapshot = snapshots.get(node["id"])
        assert snapshot is not None, ("Native root has no retained frame", node)
        frames = snapshot["entry"]["frames"]
        if len(frames) != 1:
            continue
        frame = frames[0]
        if (frame["process"] is not None or frame["index"] != 0 or frame["program"] != root
                or frame["continuation"] is not None or frame["context"] != [] or frame["stack"] != []):
            continue
        assert frame["basic_block"] == node["basic_block"], (node, frame)
        assert frame["storage_address"] == node["storage_address"] == metadata["entry_address"], (node, frame)
        assert frame["code_address"] == node["code_address"], (node, frame)
        assert type(frame["is_static"]) is bool and type(metadata["environment"]["is_static"]) is bool, frame
        assert frame["is_static"] == metadata["environment"]["is_static"], frame
        assert frame["caller"] == metadata["environment"]["caller"], frame
        assert frame["address_value"] == metadata["environment"]["to"], frame
        if program is not None:
            assert frame["code_hash"] == program["code_hash"] and frame["kind"] == program["kind"], frame
            assert frame["code_address"] == program["code_address"], frame
        candidates.append(node)
    assert len(candidates) == 1, f"expected one qualified Native root invocation: {candidates}"
    return candidates[0]


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
    assert type(key) is dict and type(region) is dict
    assert type(report["process_controls"]) is list
    index = region["relation"]
    assert type(index) is int and 0 <= index < len(report["process_controls"]), index
    relation = report["process_controls"][index]
    assert type(relation) is dict
    assert type(relation["id"]) is int and relation["id"] == index
    assert type(relation["blocks"]) is list
    assert type(region["through"]) is list and type(region["boundaries"]) is list
    # Validate both sides before equality: Python equates Block(False)/Block(0).
    for point in [key["location"], region["entry"], *region["through"], *region["boundaries"]]:
        if type(point) is dict:
            assert set(point) == {"Block"}, point
            assert type(point["Block"]) is int and 0 <= point["Block"] < len(relation["blocks"]), point
        else:
            assert type(point) is str and point in {"JumpTargets", "End"}, point
    assert (relation["epoch"], relation["source"], region["entry"]) == (
        key["epoch"], key["source"], key["location"])
    if relation["program"] is not None:
        program = source_program(report, relation["program"])
        _assert_program_source(program, relation["source"])
        assert type(program["blocks"]) is list
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


def _assert_summary_control_receipts(report, summary):
    """Qualify present receipts only; Rust owns topology and closure validation."""
    assert type(summary) is dict and type(summary["regional_cfg"]) is dict
    nodes = summary["regional_cfg"]["nodes"]
    assert type(nodes) is list
    for node in nodes:
        assert type(node) is dict
        transfer = node["transfer"]
        if transfer is None:
            continue
        assert type(transfer) is dict
        process = transfer["process"]
        if process is None:
            continue
        assert type(process) is dict
        control = process["control"]
        if control is not None:
            assert type(node["frame"]) is dict
            process_control_relation(report, node["frame"]["process"], control)


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
    assert type(report["programs"]) is list
    minimum = report["minimum"]
    assert (identity["minimum_version"] is not None) == (minimum is not None), identity
    if minimum is not None:
        assert type(minimum) is dict
        assert minimum["namespace"] == "Baseline", minimum["namespace"]
        assert type(minimum["current"]) is bool
        assert minimum["status"] in {"Converged", "Incomplete"}
        assert type(minimum["summaries"]) is list
        for summary in minimum["summaries"]:
            assert type(summary) is dict
            _assert_summary_control_receipts(report, summary["relation"])
    assert (identity["global_version"] is not None) == (report["global"] is not None), identity
    assert (identity["native_version"] is not None) == (report["native"] is not None), identity
    for namespace in ["global", "native"]:
        selected = report[namespace]
        if selected is not None:
            assert type(selected) is dict and type(selected["abstract_summaries"]) is list
            for summary in selected["abstract_summaries"]:
                _assert_summary_control_receipts(report, summary)
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


def assert_detail_policy(report, requested):
    """Qualify the applied D5 request; selected sources belong to Native coverage."""
    assert type(requested) is int and 0 <= requested < 2**64, requested
    hybrid = report["hybrid"]
    coverage = hybrid["coverage"]
    precision = hybrid["precision"]
    if precision is None:
        precision = coverage
    for stage in [coverage, precision]:
        assert type(stage) is dict and "requested_detail_regions" in stage
        assert type(stage["requested_detail_regions"]) is int
        assert stage["requested_detail_regions"] == requested
        assert type(stage["detail_regions"]) is list
        assert len(stage["detail_regions"]) <= requested
    for namespace in ["Baseline", "Native"]:
        selected = graph(report, namespace)
        if selected is not None:
            applied = selected["metadata"]["limits"]["max_detail_regions"]
            assert type(applied) is int and applied == requested, (namespace, applied, requested)
    if requested == 0:
        assert graph(report, "Native") is None and hybrid["local_prefixes"] == []
        for stage in [coverage, precision]:
            assert stage["detail"] == "Disabled" or (
                stage["baseline_current"] is False and stage["detail"] == "BaselinePending")
    return precision


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
    precision = assert_detail_policy(report, coverage["requested_detail_regions"])
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
