"""Summarize delivered reports without inferring coverage from speed or counts."""

from collections import Counter
import json

from web_test_hybrid import job_result_report, source_block, source_program


WIRE_VERSION = 22
NATIVE_VERSION = 16
# D5 application default; only omission in a request/CLI profile applies it.
DEFAULT_DETAIL_REGIONS = 1


def strict_json(raw):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON field")
            result[key] = value
        return result

    def nonfinite(_):
        raise ValueError("nonfinite JSON number")

    return json.loads(raw, object_pairs_hook=unique, parse_constant=nonfinite)


def graph_sizes(report):
    pending = [report]
    roots, nodes, largest = 0, 0, 0
    while pending:
        value = pending.pop()
        if isinstance(value, dict):
            if "identity_scope" in value and isinstance(value.get("nodes"), list):
                size = len(value["nodes"])
                roots += 1
                nodes += size
                largest = max(largest, size)
            pending.extend(value.values())
        elif isinstance(value, list):
            pending.extend(value)
    return {"serialized_roots": roots, "serialized_nodes": nodes,
            "largest_serialized_graph": largest,
            "scope": "Repeated serialized observations, not unique live DAG nodes or SMT formulas"}


def procedure_kind(procedure, mode):
    process = procedure["frame"].get("process") if mode == "offline" else procedure.get("process")
    if process is not None:
        source = process["source"]
        if isinstance(source, dict):
            (source,) = source
        # Process coordinates intentionally have no native mode or cursor.
        # Keep interpretation distinct from known runtime/constructor identity.
        return f"Process:{procedure['semantics']}:{source}"
    if mode != "offline":
        return procedure["kind"]
    frame = procedure["frame"]
    kind = frame["mode"]
    if isinstance(kind, dict):
        (kind,) = kind
    # Match the Web CodeKind labels; tagged Precompile modes share one kind.
    return {"InitCode": "Initcode", "Unknown": "AbstractEnvironment"}.get(kind, kind)


def graph_metrics(report, mode):
    """Graph-only adapter for the separately versioned no-SMT scale profiles.

    Its caller admits the profile's declared native schema before this step.
    Current benchmark entrypoints below never guess a historical report shape.
    """
    frontiers = report["frontiers"]
    families = report.get("abstract_summaries", report.get("procedure_summaries", report.get("unknown_summaries", [])))
    procedures = [procedure for family in families for procedure in family.get("procedures", [])]
    metadata = report.get("metadata", {})
    output = {
        "status": report["status"], "frontier_count": len(frontiers), "frontiers": frontiers,
        "frontier_kind_counts": dict(Counter(str(f.get("kind", f.get("reason"))) for f in frontiers)),
        "diagnostics": report.get("diagnostics", []), "states": len(report["states"]),
        "transfers": report["transfers"], "outcomes": len(report["outcomes"]),
        "summary_requests": len(families),
        "summary_kinds": dict(Counter(str(family.get("kind")) for family in families)),
        "summary_closed": [family.get("closed") for family in families],
        "all_reported_summaries_closed": bool(families) and all(family.get("closed") is True for family in families),
        "procedure_kinds": dict(Counter(str(procedure_kind(p, mode)) for p in procedures)),
        "relation_states": sum(f.get("state_count", 0) for f in families),
        "propagations": sum(f.get("propagations", 0) for f in families),
        "reused": sum(f.get("reused", 0) for f in families),
        "graph_sizes": graph_sizes(report),
    }
    if mode == "offline":
        output.update(work=report["work"], config=report["config"],
                      summary_stats=report.get("summary_stats"),
                      ordinary_summaries=len(report.get("summaries", [])),
                      environment=report.get("entry", {}).get("environment"))
    else:
        output.update(work=metadata.get("work"), policy=metadata.get("limits"),
                      snapshot=metadata.get("snapshot"), environment=metadata.get("environment"),
                      timings=metadata.get("timings"), acquisition=report.get("acquisition"),
                      export_limits=metadata.get("export_limits"), export_usage=metadata.get("export_usage"),
                      cfg_states=len(report.get("cfg", [])), ssa_complete=report.get("ssa", {}).get("complete"),
                      symbolic_nodes=len(report.get("symbolic", {}).get("nodes", [])),
                      projected_byte_arrays=len(report.get("byte_arrays", [])),
                      projected_stores=len(report.get("stores", [])))
    return output


def counter(value):
    assert type(value) is int and 0 <= value < 2**64
    return value


def effective_request_limits(limits):
    """Preserve the raw map; D5 defaults exactly one omitted input field."""
    assert type(limits) is dict
    effective, applied = dict(limits), {}
    if "max_detail_regions" not in limits:
        effective["max_detail_regions"] = DEFAULT_DETAIL_REGIONS
        applied["max_detail_regions"] = DEFAULT_DETAIL_REGIONS
    counter(effective["max_detail_regions"])
    return effective, applied


def cli_detail_region_request(arguments):
    values = []
    for index, argument in enumerate(arguments):
        if argument == "--max-detail-regions":
            if index + 1 == len(arguments):
                raise ValueError("--max-detail-regions requires an unsigned integer")
            values.append(arguments[index + 1])
        elif argument.startswith("--max-detail-regions="):
            values.append(argument.split("=", 1)[1])
    if not values:
        return {"max_detail_regions": DEFAULT_DETAIL_REGIONS, "default_applied": True}
    if len(values) != 1:
        raise ValueError("expected at most one --max-detail-regions option")
    decimal(values[0])
    return {"max_detail_regions": int(values[0]), "default_applied": False}


def detail_region_policy(hybrid, *, native_present):
    """Requested cap and actual selected sources are separate observations."""
    policies = {}
    for name in ("coverage", "precision"):
        coverage = hybrid[name]
        if coverage is None:
            policies[name] = None
            continue
        requested = counter(coverage["requested_detail_regions"])
        selected = coverage["detail_regions"]
        assert type(selected) is list and all(type(region) is dict for region in selected)
        assert len(selected) <= requested
        if requested == 0:
            assert not native_present and selected == []
            assert coverage["detail"] == "Disabled" or (
                coverage["baseline_current"] is False and coverage["detail"] == "BaselinePending")
        policies[name] = {"requested_detail_regions": requested, "selected_detail_regions": selected,
                          "selected_detail_region_count": len(selected), "detail_state": coverage["detail"]}
    assert policies["coverage"] is not None
    if policies["precision"] is not None:
        assert policies["precision"]["requested_detail_regions"] == policies["coverage"]["requested_detail_regions"]
    return policies


def decimal(value, *, positive=False):
    assert type(value) is str and value.isascii() and value.isdecimal()
    assert str(int(value)) == value and int(value) < 2**64
    assert not positive or int(value) > 0
    return value


def publication_identity(value):
    assert type(value) is dict and set(value) == {
        "version", "minimum_version", "global_version", "native_version", "model_closed"}
    decimal(value["version"], positive=True)
    for name in ("minimum_version", "global_version", "native_version"):
        if value[name] is not None:
            decimal(value[name], positive=True)
            assert int(value[name]) <= int(value["version"])
    assert type(value["model_closed"]) is bool
    return value


def result_value(envelope):
    """Only the current externally tagged Rust Result grammar is accepted."""
    assert type(envelope) is dict and set(envelope) == {"result"}
    result = envelope["result"]
    assert type(result) is dict and set(result) in ({"Ok"}, {"Err"})
    if "Ok" in result:
        return result["Ok"], None
    error = result["Err"]
    assert type(error) is dict and set(error) == {"code", "message", "details"}
    assert type(error["code"]) is str and type(error["message"]) is str
    return None, error


def job_snapshot(envelope, job_id=None):
    snapshot, error = result_value(envelope)
    if error is not None:
        return None, error
    assert type(snapshot) is dict and set(snapshot) == {"id", "state", "progress", "timings", "publication"}
    counter(snapshot["id"])
    assert job_id is None or snapshot["id"] == job_id
    state = snapshot["state"]
    if type(state) is dict:
        assert set(state) == {"Failed"}
        result_value({"result": {"Err": state["Failed"]}})
    else:
        assert state in {"Queued", "Running", "Completed", "Cancelling", "Cancelled"}
    assert type(snapshot["progress"]) is dict and type(snapshot["timings"]) is dict
    if snapshot["publication"] is not None:
        publication_identity(snapshot["publication"])
    return snapshot, None


def _source_directory(report):
    """Check references by actual IDs, including unselected graph/minimum rows.

    This is consumer admission, not a duplicate of Rust's semantic validator.
    Preserve every source/epoch/owner/control field in the captured raw reply.
    """
    programs = report["programs"]
    assert type(programs) is list
    ids = [counter(program["id"]) for program in programs]
    assert len(set(ids)) == len(ids)
    sources = [(program["code_address"], program["code_hash"], program["kind"], program["fork"]) for program in programs]
    assert len(set(sources)) == len(sources)
    for program in programs:
        blocks = program["blocks"]
        assert len({counter(block["id"]) for block in blocks}) == len(blocks)
        pcs = [counter(instruction["pc"]) for block in blocks for instruction in block["instructions"]]
        assert len(set(pcs)) == len(pcs)
        assert all(block["instructions"] and block["instructions"][0]["pc"] == block["start_pc"] for block in blocks)

    def references(value):
        if type(value) is dict:
            for name in ("program", "root_program"):
                if name in value and value[name] is not None:
                    program_id = counter(value[name])
                    source_program(report, program_id)
            if "program" in value and value.get("basic_block") is not None:
                assert value["program"] is not None
                block = source_block(report, value)
                if "start_pc" in value:
                    assert value["start_pc"] == block["start_pc"]
            for child in value.values():
                references(child)
        elif type(value) is list:
            for child in value:
                references(child)

    for slot in ("minimum", "global", "native"):
        references(report[slot])
    references(report["hybrid"])
    references(report["process_controls"])


def _qualification(model):
    return {field: model[field] for field in (
        "source_snapshot", "revision", "root_binding", "declared_root_source",
        "entry_address", "fingerprint", "environment")}


def _declared_source(report, model, fork):
    assert all(program["fork"] == fork for program in report["programs"])
    declared = model["declared_root_source"]
    assert type(declared) is dict and len(declared) == 1
    kind, source = next(iter(declared.items()))
    assert source["fork"] == fork
    if kind in ("Runtime", "InputProgram"):
        matches = [program for program in report["programs"] if program["kind"] == "Runtime"
                   and program["code_hash"] == source["hash"]
                   and program["code_address"] == (source["source"] if kind == "Runtime" else None)]
        assert len(matches) == 1
        if "root_program" in model:
            assert model["root_program"] == matches[0]["id"]


def wire_report(envelope, *, job_id, advertised=None):
    result, error = result_value(envelope)
    assert error is None
    report = job_result_report(envelope, job_id=job_id)
    assert type(report["schema_version"]) is int and report["schema_version"] == WIRE_VERSION
    # Final D4 uses this required shared directory, including an explicit empty
    # list for primitive bodies. The shared helper owns its typed control laws.
    assert type(report["process_controls"]) is list
    identity = publication_identity(result["publication"])
    if advertised is not None:
        publication_identity(advertised)
        assert int(identity["version"]) >= int(advertised["version"])
        if identity["version"] == advertised["version"]:
            assert identity == advertised
    for field in ("work", "transfers", "postprocessing_work"):
        counter(report[field])
    _source_directory(report)
    minimum, hybrid = report["minimum"], report["hybrid"]
    coverage = None if hybrid is None else hybrid["coverage"]
    if hybrid is not None:
        detail_region_policy(hybrid, native_present=report["native"] is not None)
    if coverage is not None:
        assert type(coverage["baseline_current"]) is bool
        assert report["status"] == ("Converged" if coverage["baseline_current"] else "Incomplete")
    if minimum is not None:
        _qualification(minimum)
        _declared_source(report, minimum, minimum["fork"])
        decimal(minimum["revision"])
        assert minimum["namespace"] == "Baseline"
        if coverage is not None:
            assert minimum["source_snapshot"] == coverage["snapshot"]
            assert minimum["revision"] == coverage["baseline_revision"]
            assert minimum["config"] == coverage["baseline_config"]
            assert minimum["current"] == coverage["baseline_current"]
        assert counter(minimum["export_usage"]["work"]) <= counter(minimum["postprocessing_max_work"])
        assert report["postprocessing_work"] <= minimum["postprocessing_max_work"]
        assert report["postprocessing_work"] >= minimum["export_usage"]["work"]
        closed_root = False
        for summary in minimum["summaries"]:
            relation, bounds = summary["relation"], summary["bounds"]
            assert len(relation["procedures"]) == len(bounds)
            root = relation["root_procedure"]
            if root is not None:
                assert counter(root) < len(bounds)
            if relation["closed"]:
                assert counter(summary["version"]["generation"]) > 0 and root is not None
            closed_root |= (relation["closed"] and relation["source_current"]
                            and root is not None and bool(relation["regional_cfg"]["nodes"]))
            for procedure, row in zip(relation["procedures"], bounds):
                decimal(row["version"]["output_revision"])
                if procedure["process"] is not None:
                    assert row["version"]["source_epoch"] == procedure["process"]["epoch"]
                assert type(row["input"]) is dict and type(row["outputs"]) is list
        assert not minimum["current"] or closed_root
    for namespace, slot in (("Baseline", "global"), ("Native", "native")):
        graph = report[slot]
        if graph is None:
            continue
        metadata = graph["metadata"]
        _qualification(metadata)
        _declared_source(report, metadata, graph["fork"])
        decimal(metadata["revision"])
        if minimum is not None:
            assert graph["fork"] == minimum["fork"]
        if coverage is not None:
            qualifier = coverage if namespace == "Baseline" else hybrid["precision"] or coverage
            assert metadata["source_snapshot"] == qualifier["snapshot"]
            assert metadata["revision"] == qualifier["baseline_revision" if namespace == "Baseline" else "detail_revision"]
        if namespace == "Baseline" and minimum is not None:
            for field in ("source_snapshot", "revision", "root_binding", "declared_root_source", "entry_address", "fingerprint"):
                assert metadata[field] == minimum[field]
    assert report["rich_export"] != "Complete" or report["global"] is not None or report["native"] is not None
    return result


def _wire_graph_metrics(graph):
    if graph is None:
        return None
    families = graph["abstract_summaries"]
    return {
        "status": graph["status"], "states": len(graph["states"]),
        "cfg_states": len(graph["cfg"]), "transfers": counter(graph["transfers"]),
        "frontiers": graph["frontiers"], "frontier_count": len(graph["frontiers"]),
        "outcomes": len(graph["outcomes"]), "diagnostics": graph["diagnostics"],
        "summary_requests": len(families), "summary_closed": [family["closed"] for family in families],
        "procedure_kinds": dict(Counter(procedure_kind(p, "rpc") for family in families for p in family["procedures"])),
        "ssa_complete": graph["ssa"]["complete"], "symbolic_nodes": len(graph["symbolic"]["nodes"]),
        "projected_byte_arrays": len(graph["byte_arrays"]), "projected_stores": len(graph["stores"]),
        "metadata": graph["metadata"],
    }


def wire_metrics(report):
    assert type(report["schema_version"]) is int and report["schema_version"] == WIRE_VERSION
    minimum = report["minimum"]
    # A missing rich graph has no measured states/frontiers/SSA/pool counts.
    # The qualified minimum and its joint numerical rows remain independent.
    return {
        "format": "web-analysis-report", "schema_version": report["schema_version"],
        "status": report["status"], "work": report["work"], "transfers": report["transfers"],
        "selected_view": report["selected_view"], "rich_export": report["rich_export"],
        "postprocessing_work": report["postprocessing_work"], "timings": report["timings"],
        "acquisition": report["acquisition"], "hybrid": report["hybrid"],
        "precision_policy": None if report["hybrid"] is None else detail_region_policy(
            report["hybrid"], native_present=report["native"] is not None),
        "minimum": None if minimum is None else {
            **_qualification(minimum), "namespace": minimum["namespace"], "scope": minimum["scope"],
            "fork": minimum["fork"], "status": minimum["status"], "current": minimum["current"],
            "config": minimum["config"], "numeric_model": minimum["numeric_model"],
            "domain_spec": minimum["domain_spec"], "summary_count": len(minimum["summaries"]),
            "summary_versions": [summary["version"] for summary in minimum["summaries"]],
            "postprocessing_max_work": minimum["postprocessing_max_work"],
            "export_limits": minimum["export_limits"], "export_usage": minimum["export_usage"],
        },
        "graphs": {"Baseline": _wire_graph_metrics(report["global"]),
                   "Native": _wire_graph_metrics(report["native"])},
    }


def native_metrics(report):
    """Current `analyze --world --format json`: native16 fixed Hybrid envelope."""
    assert type(report) is dict and type(report["schema_version"]) is int
    assert report["schema_version"] == NATIVE_VERSION
    assert report["selected_view"] in {"Baseline", "Native"}
    hybrid = report["hybrid"]
    precision_policy = detail_region_policy(hybrid, native_present=report["native"] is not None)
    assert type(hybrid["coverage"]["baseline_current"]) is bool
    assert report["status"] == ("Converged" if hybrid["coverage"]["baseline_current"] else "Incomplete")
    graphs = {}
    for namespace, slot in (("Baseline", "global"), ("Native", "native")):
        wrapper = report[slot]
        if wrapper is None:
            assert namespace == "Native"
            graphs[namespace] = None
            continue
        assert type(wrapper) is dict and set(wrapper) in ({"analysis"}, {"analysis", "ssa"}, {"analysis", "partial_ssa"})
        graph = wrapper["analysis"]
        assert type(graph["schema_version"]) is int and graph["schema_version"] == NATIVE_VERSION
        assert graph["status"] in {"Converged", "Incomplete"}
        for field in ("states", "frontiers", "outcomes", "procedure_summaries"):
            assert type(graph[field]) is list and all(type(row) is dict for row in graph[field])
        assert "abstract_summaries" not in graph and "unknown_summaries" not in graph
        for field in ("world", "entry", "config"):
            assert type(graph[field]) is dict
        for field in ("work", "transfers"):
            counter(graph[field])
        graphs[namespace] = {**graph_metrics(graph, "offline"), "schema_version": graph["schema_version"],
                             "domain_spec": graph["domain_spec"]}
    assert graphs[report["selected_view"]] is not None
    assert counter(report["postprocessing_work"]) <= counter(report["postprocessing_max_work"])
    return {"format": "native-hybrid-report", "schema_version": NATIVE_VERSION,
            "status": report["status"], "work": counter(report["work"]),
            "transfers": counter(report["transfers"]), "selected_view": report["selected_view"],
            "graphs": graphs, "hybrid": hybrid, "precision_policy": precision_policy,
            "numeric_model": report["numeric_model"],
            "postprocessing_max_work": report["postprocessing_max_work"],
            "postprocessing_work": report["postprocessing_work"]}


def metrics(report, mode):
    if mode == "rpc":
        return wire_metrics(report)
    if mode == "offline":
        return native_metrics(report)
    raise ValueError("expected an explicit current native or web format")


def _address(actual, expected):
    if expected == "Unknown":
        assert type(actual) is dict and set(actual) == {"Symbolic"}
    else:
        assert set(actual) == set(expected) == {"Concrete"}
        assert actual["Concrete"].lower() == expected["Concrete"].lower()


def _word(actual, expected):
    if expected == "Unknown":
        assert actual["constants"] is None
        assert int(actual["known_zero"], 16) == int(actual["known_one"], 16) == 0
        assert actual["nonzero"] is False and actual["congruence"] == "Any"
        assert int(actual["unsigned"]["lower"], 16) == 0
        assert int(actual["unsigned"]["upper"], 16) == 2**256 - 1
    else:
        assert len(actual["constants"]) == 1
        assert int(actual["constants"][0], 16) == int(expected["Concrete"], 16)


def request_qualification(report, request):
    """Admit this fixed RPC input without rewriting bytes, policy or Unknowns.

    Minimum-only replies expose execution/numerical policy, not RPC transport
    limits. Preserve the full exact request alongside those observable checks.
    """
    assert set(request["input"]) == {"Rpc"}
    rpc, supplied = request["input"]["Rpc"], request["environment"]
    limits, applied_defaults = effective_request_limits(request["limits"])
    policies = detail_region_policy(report["hybrid"], native_present=report["native"] is not None)
    assert policies["coverage"]["requested_detail_regions"] == limits["max_detail_regions"]
    assert set(rpc["block"]) == {"Hash"}
    minimum = report["minimum"]
    sources = []
    if minimum is not None:
        config = minimum["config"]
        assert config == {key: False if key == "root_terminal_coverage" else limits[key] for key in config}
        assert minimum["numeric_model"] == limits["numeric_model"]
        domain = minimum["domain_spec"]
        for field, key in (("capacity", "max_constants"), ("reduction_rounds", "reduction_rounds"), ("fact_limit", "max_facts")):
            assert domain[field] == limits[key]
        for field, key in (("enabled", "relations_enabled"), ("max_constraints", "max_constraints"),
                           ("max_nodes", "max_expression_nodes"), ("max_depth", "max_expression_depth")):
            assert domain["relations"][field] == limits[key]
        sources.append((minimum, minimum["environment"]["calldata"]))
    for slot in ("global", "native"):
        graph = report[slot]
        if graph is not None:
            echoed = graph["metadata"]["limits"]
            assert type(echoed) is dict and set(echoed) == set(limits)
            assert all(type(echoed[key]) is type(value) and echoed[key] == value for key, value in limits.items())
            env = graph["metadata"]["environment"]
            calldata = graph["byte_arrays"][counter(env["calldata"])]
            sources.append(({**graph["metadata"], "fork": graph["fork"], "scope": graph["scope"]}, calldata))
    assert sources
    for source, calldata in sources:
        assert source["fork"] == request["fork"] and source["scope"] == "RpcWorld"
        snapshot = source["source_snapshot"]
        assert set(snapshot) == {"Chain"}
        assert snapshot["Chain"]["block_hash"].lower() == rpc["block"]["Hash"].lower()
        binding = source["root_binding"]
        assert set(binding) == {"Physical"}
        assert binding["Physical"]["address"].lower() == source["entry_address"].lower() == rpc["address"].lower()
        env = source["environment"]
        _address(env["to"], {"Concrete": rpc["address"]})
        _address(env["caller"], supplied["caller"])
        if supplied["origin"] is None:
            assert env["origin"] == env["caller"]
        else:
            _address(env["origin"], supplied["origin"])
        for field in ("call_value", "gas_price", "blob_count"):
            _word(env[field], supplied[field])
        for field in ("is_static", "gas_upper_bound", "block_hashes", "blob_hashes"):
            assert env[field] == supplied[field]
        for field in ("timestamp", "number", "prevrandao", "gas_limit", "chain_id", "base_fee", "blob_base_fee"):
            if supplied[field] is not None:
                _word(env[field], {"Concrete": supplied[field]})
        if supplied["coinbase"] is not None:
            _address(env["coinbase"], {"Concrete": supplied["coinbase"]})
        if supplied["calldata"] == "Unknown":
            _word(calldata["length"], "Unknown")
            if source is minimum:
                assert calldata["content"] == {"Cells": []} and calldata["words"] == []
            else:
                assert calldata["cells"] == []
        else:
            exact = supplied["calldata"]["Exact"].removeprefix("0x")
            _word(calldata["length"], {"Concrete": hex(len(bytes.fromhex(exact)))})
    return {"effective_limits": limits, "applied_defaults": applied_defaults,
            "precision_policy": policies,
            "execution_policy_checked": True, "original_environment_checked": True,
            "rich_request_policy_checked": any(report[slot] is not None for slot in ("global", "native")),
            "rpc_transport_policy": "Exact submitted bytes; minimum has no echoed transport policy"}
