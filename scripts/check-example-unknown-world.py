#!/usr/bin/env python3
"""Check numerical witnesses of one installed unknown-world example report."""

import json, sys
name, path = sys.argv[1:]
analysis = json.load(open(path))["analysis"]

def number(value):
    return int(value, 0) if isinstance(value, str) else value

def allowed_values(value, allowed):
    constants = value.get("Constants")
    if constants is not None:
        values = {number(item) for item in constants}
    else:
        interval = value["interval"]
        lo, hi = number(interval["unsigned_lo"]), number(interval["unsigned_hi"])
        assert 0 <= hi - lo <= 256, (name, "unbounded numerical result", value)
        bits = value["known_bits"]
        zero, one = number(bits["zero"]), number(bits["one"])
        values = {item for item in range(lo, hi + 1)
                  if item & zero == 0 and item & one == one}
        congruence = value["congruence"]
        if isinstance(congruence, dict) and "Exact" in congruence:
            values &= {number(congruence["Exact"])}
        elif isinstance(congruence, dict) and "Modulo" in congruence:
            relation = congruence["Modulo"]
            values = {item for item in values
                      if item % number(relation["modulus"]) == number(relation["residue"])}
    assert values and values <= set(allowed), (name, values, allowed)
    return values

def word_values(data, offset, allowed):
    for index in range(offset, offset + 31):
        allowed_values(data["bytes"][str(index)], [0])
    return allowed_values(data["bytes"][str(offset + 31)], allowed)

def slot_value(store, owner, slot):
    plane = store["persistent"]
    for item in plane["slots"]:
        if item["address"] == owner and number(item["slot"]) == slot:
            return item["value"]
    # An unchanged zero slot can be absent from the numerical cache.
    # Read its actual root; unsupported symbolic nodes fail the gate.
    root = plane["roots"][owner]
    nodes = root["nodes"]
    def read_node(index):
        node = nodes[index]
        if node == "SlotsZero":
            return 0
        assert isinstance(node, dict) and "Store" in node, (name, "unsupported slot root", node)
        write = node["Store"]
        key = nodes[write["key"]]
        assert isinstance(key, dict) and "Constant" in key, (name, "symbolic slot key", key)
        if number(key["Constant"]) != slot:
            return read_node(write["previous"])
        value = nodes[write["value"]]
        assert isinstance(value, dict) and "Constant" in value, (name, "symbolic stored value", value)
        return number(value["Constant"])
    return {"Constants": [hex(read_node(root["root"]))]}

outcomes = analysis["outcomes"]
owner_a = "0x0000000000000000000000000000000000000101"
owner_c = "0x0000000000000000000000000000000000000301"
if name.startswith("busy-x"):
    returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
    assert returns, name
    possible = set()
    for outcome in returns:
        possible |= word_values(outcome["data"], 0, [0, 1, 2])
        word_values(outcome["data"], 32, [1])
        allowed_values(slot_value(outcome["store"], owner_a, 0), [0])
    assert possible == {0, 1, 2}, (name, possible)
elif name == "outer-revert":
    assert outcomes and all(outcome["kind"] != "Return" for outcome in outcomes)
    reverted_values = set()
    for outcome in outcomes:
        allowed_values(slot_value(outcome["store"], owner_c, 0), [5])
        if outcome["kind"] == "Revert":
            lengths = allowed_values(outcome["data"]["length"], [0, 64])
            if lengths == {0}:
                continue
            assert lengths == {64}, (name, "ambiguous revert length", lengths)
            word_values(outcome["data"], 0, [5])
            reverted_values |= word_values(outcome["data"], 32, [5, 9])
    assert reverted_values == {5, 9}, (name, "missing old/new revert data", reverted_values)
elif name == "static-reuse":
    returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
    assert returns, name
    for outcome in returns:
        word_values(outcome["data"], 0, [42])
elif name in {"external-guard", "external-change"}:
    returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
    assert returns, name
    possible = set()
    for outcome in returns:
        word_values(outcome["data"], 0, [5])
        possible |= word_values(outcome["data"], 32, [5] if name == "external-guard" else [5, 9])
        allowed_values(slot_value(outcome["store"], owner_c, 1), [0] if name == "external-guard" else [1])
    assert possible == ({5} if name == "external-guard" else {5, 9}), (name, possible)
else:
    raise AssertionError((name, "missing unknown-world numerical checks"))
