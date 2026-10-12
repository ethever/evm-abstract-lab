#!/usr/bin/env python3
"""Compare complete native reports up to producer-namespace alpha-renaming.

Only native SymbolicGraph.identity_scope is renamed. One map spans the complete
report; Fresh/Input/source identities, graph contents and ordering stay intact.
Numbers retain their JSON lexemes, avoiding float conversion even for u256.
"""

import json
import re
import sys


class _Number(str):
    """A validated JSON numeric lexeme, distinct from a JSON string."""


_GRAPH_FIELDS = {"identity_scope", "root", "nodes"}
# Native domain/symbolic/graph.rs's externally tagged SymbolicGraphNode enum.
_UNIT_NODES = {"BytesZero", "SlotsZero"}
_TAGGED_NODES = {
    "Constant", "Input", "Fresh", "Operation", "Ite", "ByteRead", "WordRead",
    "SlotRead", "Keccak", "BytesInput", "BytesExact", "WriteByte", "WriteWord",
    "Copy", "Slice", "BytesIte", "SlotsInput", "Store", "SlotsIte", "View",
}


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON object key: {key!r}")
        result[key] = value
    return result


def _reject_constant(value):
    raise ValueError(f"invalid JSON numeric constant: {value}")


def _validate_graph(graph):
    scope, root, nodes = (graph[key] for key in ("identity_scope", "root", "nodes"))
    if type(scope) is not str or not scope:
        raise ValueError("native SymbolicGraph.identity_scope must be a nonempty string")
    if not isinstance(nodes, list) or not nodes:
        raise ValueError("native SymbolicGraph.nodes must be a nonempty array")
    if (not isinstance(root, _Number) or not re.fullmatch(r"0|[1-9][0-9]*", root)
            or int(root) >= len(nodes)):
        raise ValueError("native SymbolicGraph.root must be an in-range integer index")
    for node in nodes:
        if type(node) is str and node in _UNIT_NODES:
            continue
        if isinstance(node, dict) and len(node) == 1 and next(iter(node)) in _TAGGED_NODES:
            continue
        raise ValueError("native SymbolicGraph.nodes contains a non-native node variant")


def _rename(value, scopes):
    if isinstance(value, dict):
        graph = value.keys() == _GRAPH_FIELDS
        if graph:
            _validate_graph(value)
        for key, child in value.items():
            if graph and key == "identity_scope":
                if child not in scopes:
                    scopes[child] = f"producer-{len(scopes)}"
                value[key] = scopes[child]
            else:
                _rename(child, scopes)
    elif isinstance(value, list):
        for child in value:
            _rename(child, scopes)


def _ordered_json(value):
    if isinstance(value, _Number):
        return str(value)
    if isinstance(value, dict):
        return "{" + ",".join(
            json.dumps(key, ensure_ascii=False) + ":" + _ordered_json(child)
            for key, child in value.items()
        ) + "}"
    if isinstance(value, list):
        return "[" + ",".join(_ordered_json(child) for child in value) + "]"
    return json.dumps(value, ensure_ascii=False)


def canonical_json(report_json: str) -> str:
    """Return compact ordered JSON for ONE COMPLETE report, with a fresh scope map.

    Objects matching the native graph's exact three-field shape must have a valid
    scope, root and node variants. Unrelated identity_scope fields are untouched.
    Raises ValueError for malformed JSON/graphs, duplicate keys and NaN/Infinity.
    """
    report = json.loads(report_json, parse_int=_Number, parse_float=_Number,
                        parse_constant=_reject_constant, object_pairs_hook=_object)
    _rename(report, {})
    return _ordered_json(report)


if __name__ == "__main__":
    try:
        print(canonical_json(sys.stdin.read()))
    except ValueError as error:
        sys.exit(f"identity-scope normalization: {error}")
