"""Summarize delivered reports without inferring coverage from speed or counts."""

from collections import Counter


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
    if mode != "offline":
        return procedure["kind"]
    kind = procedure["frame"]["mode"]
    if isinstance(kind, dict):
        (kind,) = kind
    # Match the Web CodeKind labels; tagged Precompile modes share one kind.
    return {"InitCode": "Initcode", "Unknown": "AbstractEnvironment"}.get(kind, kind)


def metrics(report, mode):
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
