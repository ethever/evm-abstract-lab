#!/usr/bin/env python3
"""Check Native carriers and default Hybrid coverage of the six examples."""

import argparse
from copy import deepcopy
from dataclasses import dataclass
import json


WORD_MAX = (1 << 256) - 1
SIGN = 1 << 255
# The private-carrier oracle needs at most 19 nodes, 64 bytes, one read and three choices,
# plus one runtime word whose explicit numeric certificate is a singleton.
MAX_NODES, MAX_BYTES, MAX_CHOICES = 19, 64, 3
MAX_RUNTIME_WORDS = 1
NUMERIC_FIELDS = {"constants", "known_bits", "interval", "congruence", "nonzero"}
OBSERVATION_FIELDS = {"Constants", "known_bits", "interval", "congruence", "nonzero"}
OWNER_A = "0x0000000000000000000000000000000000000101"
OWNER_C = "0x0000000000000000000000000000000000000301"
TRUE = {"constraints": [], "bottom": False, "path": {"root": 0, "nodes": ["True"]}}
PRIVATE_CALLEE = {"external-change", "outer-revert"}
MAX_WORK = 2_000_000_000


def number(value):
    assert type(value) in (str, int), ("invalid word", value)
    if isinstance(value, str):
        assert len(value) <= 78, "oversized word"
        value = int(value, 0)
    assert 0 <= value <= WORD_MAX, ("invalid Word256", value)
    return value


def numeric(value):
    """Native sparse components; absent components have their documented Top."""
    assert isinstance(value, dict) and value.keys() <= NUMERIC_FIELDS, value
    return numeric_components(value, value.get("constants"))


def observation(value):
    """The representation-independent WordObservation stored in a source DAG."""
    assert isinstance(value, dict) and value.keys() == OBSERVATION_FIELDS, value
    return numeric_components(value, value["Constants"])


def evm_numeric(value):
    """Return the required native payload; all common metadata may be absent."""
    assert isinstance(value, dict) and "numeric" in value, value
    assert value.keys() <= {"numeric", "provenance", "identity", "expression", "symbolic_limit"}, value
    payload = value["numeric"]
    assert isinstance(payload, dict) and payload.keys() <= NUMERIC_FIELDS, payload
    return payload


def numeric_components(value, constants):
    """Intersect every numeric component; no interval/Top fallback for reads."""
    assert isinstance(constants, list) and 1 <= len(constants) <= MAX_CHOICES, value
    constants = tuple(number(item) for item in constants)
    assert constants == tuple(sorted(set(constants))), value
    bits = value.get("known_bits", {"zero": "0x0", "one": "0x0"})
    interval = value.get("interval", {"unsigned_lo": "0x0", "unsigned_hi": hex(WORD_MAX),
                                     "signed_lo": "0x0", "signed_hi": hex(WORD_MAX)})
    assert bits.keys() == {"zero", "one"}, bits
    assert interval.keys() == {"unsigned_lo", "unsigned_hi", "signed_lo", "signed_hi"}, interval
    zero, one = number(bits["zero"]), number(bits["one"])
    lo, hi = number(interval["unsigned_lo"]), number(interval["unsigned_hi"])
    slo, shi = number(interval["signed_lo"]), number(interval["signed_hi"])
    assert not zero & one and lo <= hi and slo <= shi, value
    relation = value.get("congruence", "Top")
    if relation == "Top":
        congruence = ("Top",)
    elif isinstance(relation, dict) and relation.keys() == {"Exact"}:
        congruence = ("Exact", number(relation["Exact"]))
    else:
        assert isinstance(relation, dict) and relation.keys() == {"Modulo"}, relation
        modulo = relation["Modulo"]
        assert modulo.keys() == {"modulus", "residue"}, modulo
        modulus, residue = number(modulo["modulus"]), number(modulo["residue"])
        assert modulus > 0 and residue < modulus, modulo
        congruence = ("Modulo", modulus, residue)
    nonzero = value.get("nonzero", False)
    assert type(nonzero) is bool, value
    # Native signed bounds use the order-preserving sign-bit bias (Interval).
    values = frozenset(item for item in constants
                       if not item & zero and item & one == one
                       and lo <= item <= hi and slo <= item ^ SIGN <= shi
                       and (not nonzero or item != 0)
                       and (congruence[0] == "Top"
                            or congruence[0] == "Exact" and item == congruence[1]
                            or congruence[0] == "Modulo" and item % modulus == residue))
    assert values, ("inconsistent numeric", value)
    return (constants, zero, one, lo, hi, slo, shi, congruence, nonzero), values


@dataclass(frozen=True)
class Read:
    # Keep the original source/read and complete binding, even for singleton facts.
    source: tuple
    certificate: tuple
    choices: frozenset


@dataclass(frozen=True)
class RuntimeWord:
    # Fresh's payload is an identity, not the word's numeric value.
    scope: str
    token: int


class Graph:
    """Typed backward DAGs with distinct source-token and runtime-word roles."""

    rules = {
        "SlotRead": ("word", {"state": "slots", "key": "word"}),
        "Store": ("slots", {"previous": "slots", "key": "word", "value": "word"}),
        "WriteWord": ("bytes", {"previous": "bytes", "offset": "word", "value": "word"}),
        "View": ("view", {"state": "bytes", "offset": "word", "length": "word"}),
        "Slice": ("bytes", {"source": "view"}),
        "Copy": ("bytes", {"previous": "bytes", "destination": "word",
                           "source": "view", "length": "word"}),
    }

    def __init__(self, graph, scope):
        assert graph.keys() == {"identity_scope", "root", "nodes"}, graph
        assert isinstance(scope, str) and scope and graph["identity_scope"] == scope, "producer scope"
        self.scope, self.nodes, self.root = scope, graph["nodes"], graph["root"]
        assert isinstance(self.nodes, list) and 1 <= len(self.nodes) <= MAX_NODES, "graph size"
        assert type(self.root) is int and self.root == len(self.nodes) - 1, "graph root"
        self.roles = {}
        sorts, edges = [], []
        for index, node in enumerate(self.nodes):
            refs = []

            def ref(child, sort, token=False):
                assert type(child) is int and 0 <= child < index, ("backward reference", child)
                assert sorts[child] == sort, ("reference sort", child, sort)
                fresh = isinstance(self.nodes[child], dict) and "Fresh" in self.nodes[child]
                if token:
                    assert fresh, "source identity must be Fresh"
                if fresh:
                    assert sort == "word", "Fresh word reference"
                    self.fresh_role(child, "source" if token else "runtime")
                refs.append(child)

            if node in ("BytesZero", "SlotsZero"):
                sort = "bytes" if node == "BytesZero" else "slots"
            else:
                assert isinstance(node, dict) and len(node) == 1, ("unsupported node", node)
                kind, payload = next(iter(node.items()))
                if kind == "Constant":
                    number(payload)
                    sort = "word"
                elif kind == "Fresh":
                    assert type(payload) is int and 0 < payload < 1 << 64, "Fresh identity"
                    sort = "word"
                elif kind == "BytesExact":
                    assert isinstance(payload, list) and len(payload) <= MAX_BYTES, "exact bytes size"
                    assert all(type(byte) is int and 0 <= byte <= 255 for byte in payload), payload
                    sort = "bytes"
                elif kind == "SlotsInput":
                    assert payload.keys() == {"source", "bindings"}, payload
                    assert payload["source"].keys() == {"token", "namespace"}, payload
                    ref(payload["source"]["token"], "word", token=True)
                    number(payload["source"]["namespace"])
                    bindings = payload["bindings"]
                    assert isinstance(bindings, list) and len(bindings) <= 1, "one observed cell"
                    for binding in bindings:
                        assert binding.keys() == {"index", "value", "numeric"}, binding
                        number(binding["index"])
                        ref(binding["value"], "word")
                        observation(binding["numeric"])
                    sort = "slots"
                else:
                    assert kind in self.rules, ("unsupported node", kind)
                    sort, fields = self.rules[kind]
                    assert payload.keys() == fields.keys(), (kind, payload)
                    for field, child_sort in fields.items():
                        ref(payload[field], child_sort)
            sorts.append(sort)
            edges.append(refs)
        if isinstance(self.nodes[self.root], dict) and "Fresh" in self.nodes[self.root]:
            self.fresh_role(self.root, "runtime")
        reached = {self.root}
        for index in range(self.root, -1, -1):
            if index in reached:
                reached.update(edges[index])
        assert len(reached) == len(self.nodes), "orphan nodes"
        for node in self.nodes:
            if not isinstance(node, dict):
                continue
            kind, payload = next(iter(node.items()))
            if kind in {"WriteWord", "View", "Copy"}:
                offset = self.coordinate(payload["destination" if kind == "Copy" else "offset"])
                length = 32 if kind == "WriteWord" else self.coordinate(payload["length"])
                assert offset + length <= MAX_BYTES, "byte range"
            elif kind in {"Store", "SlotRead"}:
                self.literal(payload["key"])
            elif kind == "SlotsInput":
                self.source(payload)

    def fresh_role(self, index, role):
        token = self.node(index, "Fresh")
        assert self.roles.get(token, role) == role, "Fresh source/runtime role collision"
        self.roles[token] = role

    def node(self, index, kind):
        node = self.nodes[index]
        assert isinstance(node, dict) and node.keys() == {kind}, ("expected", kind, node)
        return node[kind]

    def literal(self, index):
        return number(self.node(index, "Constant"))

    def source(self, state):
        source = state["source"]
        return (self.scope, ("Fresh", self.node(source["token"], "Fresh")),
                number(source["namespace"]))

    def word(self, index):
        if isinstance(self.nodes[index], dict) and "Constant" in self.nodes[index]:
            return self.literal(index)
        if isinstance(self.nodes[index], dict) and "Fresh" in self.nodes[index]:
            token = self.node(index, "Fresh")
            assert self.roles.get(token) == "runtime", "source token is not a runtime word"
            return RuntimeWord(self.scope, token)
        read = self.node(index, "SlotRead")
        key = self.literal(read["key"])
        state = self.node(read["state"], "SlotsInput")
        assert len(state["bindings"]) == 1, "unbound read"
        binding = state["bindings"][0]
        assert number(binding["index"]) == key, "unobserved slot"
        original = self.node(binding["value"], "SlotRead")
        raw = self.node(original["state"], "SlotsInput")
        assert raw["bindings"] == [] and self.source(raw) == self.source(state), "original source"
        assert self.literal(original["key"]) == key, "original key"
        certificate, choices = observation(binding["numeric"])
        return Read(self.source(state) + (key,), certificate, choices)

    def coordinate(self, index):
        value = self.literal(index)
        assert value % 32 == 0 and value <= MAX_BYTES, ("whole-word coordinate", value)
        return value

    def words(self):
        terms = set()
        for index, node in enumerate(self.nodes):
            if isinstance(node, dict) and "Constant" in node:
                terms.add(self.literal(index))
            elif isinstance(node, dict) and "Fresh" in node and self.roles.get(node["Fresh"]) == "runtime":
                terms.add(self.word(index))
            elif isinstance(node, dict) and "SlotRead" in node:
                state = self.node(node["SlotRead"]["state"], "SlotsInput")
                if state["bindings"]:
                    terms.add(self.word(index))
        return terms

    def window(self, index, start):
        assert start % 32 == 0 and 0 <= start <= MAX_BYTES - 32, "word window"
        node = self.nodes[index]
        if node == "BytesZero":
            return 0
        assert isinstance(node, dict), ("unsupported byte root", node)
        kind, payload = next(iter(node.items()))
        if kind == "BytesExact":
            return int.from_bytes(bytes(payload[start:start + 32]).ljust(32, b"\0"), "big")
        if kind == "Slice":
            return self.view(payload["source"], start)
        if kind == "WriteWord":
            offset, length = self.coordinate(payload["offset"]), 32
        else:
            assert kind == "Copy", ("unsupported byte root", kind)
            offset = self.coordinate(payload["destination"])
            length = self.coordinate(payload["length"])
        assert offset + length <= MAX_BYTES, "write window"
        if start + 32 <= offset or offset + length <= start:
            return self.window(payload["previous"], start)
        assert offset <= start and start + 32 <= offset + length, "partial word coverage"
        if kind == "WriteWord":
            return self.word(payload["value"])
        return self.view(payload["source"], start - offset)

    def view(self, index, start):
        view = self.node(index, "View")
        offset, length = self.coordinate(view["offset"]), self.coordinate(view["length"])
        assert offset + length <= MAX_BYTES and start + 32 <= length, "view window"
        return self.window(view["state"], offset + start)

    def slot(self, index, key):
        if self.nodes[index] == "SlotsZero":
            return 0
        write = self.node(index, "Store")
        if self.literal(write["key"]) == key:
            return self.word(write["value"])
        return self.slot(write["previous"], key)


class Outcome:
    """One shared finite source assignment for data, effects and checkpoints."""

    def __init__(self, outcome, states):
        assert type(outcome["state"]) is int, "source state id"
        state = states[outcome["state"]]
        self.payload = state["exit"]
        for payload in (state["entry"], self.payload):
            relations = payload["relations"]
            assert relations == TRUE and relations["bottom"] is False, "nontrivial F"
            assert type(relations["path"]["root"]) is int, "path root"
        stack = self.payload["call_stack"]
        self.frame = stack["root"]["state"]
        assert self.frame["key"]["address"] == OWNER_A and self.frame["key"]["code_address"] == OWNER_A, "root owner"
        self.scope = self.frame["memory"]["root"]["identity_scope"]
        if outcome["kind"] in {"Return", "Revert"}:
            assert stack["children"] == [] and len(state["key"]["frames"]) == 1, "root completion"
            pc = state["executed_pcs"][-1]
            opcodes = [ins["opcode"] for block in self.frame["program"]["blocks"]
                       for ins in block["instructions"] if ins["pc"] == pc]
            assert opcodes == [0xf3 if outcome["kind"] == "Return" else 0xfd], "source terminal"
        self.domains = {}
        self.runtime_values, self.roles = {}, {}

    def graph(self, graph):
        carrier = Graph(graph, self.scope)
        for token, role in carrier.roles.items():
            assert self.roles.get(token, role) == role, "Fresh source/runtime role collision"
            self.roles[token] = role
        return carrier

    def constrain(self, term, values):
        if isinstance(term, Read):
            values = term.choices & values & self.domains.get(term.source, term.choices)
            assert values, "inconsistent source facts"
            self.domains[term.source] = values
            assert len(self.domains) <= 1, "only one explicit finite source read is supported"
        elif isinstance(term, RuntimeWord):
            assert len(values) == 1, "runtime word needs an exact numeric certificate"
            value, = values
            assert self.runtime_values.get(term, value) == value, "inconsistent runtime word facts"
            self.runtime_values[term] = value
            assert len(self.runtime_values) <= MAX_RUNTIME_WORDS, "runtime word count"
        else:
            assert term in values, ("literal fact disagrees", term, values)

    def facts(self, operands, carrier):
        assert isinstance(operands, list) and len(operands) <= MAX_CHOICES + MAX_RUNTIME_WORDS, "operand count"
        terms = carrier.words()
        facts = set()
        for fact in operands:
            assert fact.keys() == {"expression", "value"}, fact
            graph = self.graph(fact["expression"])
            term = graph.word(graph.root)
            assert term in terms, "operand absent from carrier"
            value = fact["value"]
            _, values = numeric(evm_numeric(value))
            if isinstance(term, (Read, RuntimeWord)) or value.get("expression") is not None:
                original = self.graph(value["expression"])
                assert original.word(original.root) == term, "operand original expression"
            assert term not in facts, "duplicate operand"
            self.constrain(term, values)
            facts.add(term)
        assert sum(not isinstance(term, RuntimeWord) for term in facts) <= MAX_CHOICES, "operand count"
        assert all(term in facts for term in terms if isinstance(term, (Read, RuntimeWord))), "missing original word fact"
        return facts

    def certified(self, term, facts):
        if isinstance(term, Read):
            assert term in facts, "missing original read fact"
            self.constrain(term, term.choices)
        elif isinstance(term, RuntimeWord):
            assert term in facts, "missing original runtime word fact"
            # Resolve only the proven numeric singleton, never the Fresh token.
            return self.runtime_values[term]
        return term

    def data(self, data, lengths):
        assert data["failure"] is None and data["memory"] is False, "unsuccessful data"
        assert data["conditional"] == [], "conditional data facts"
        _, values = numeric(evm_numeric(data["length"]))
        assert len(values) == 1 and values <= set(lengths), ("exact data length", values)
        length = next(iter(values))
        # Native empty buffers carry the exact numeric length without an ExprId.
        if data["length"].get("expression") is not None:
            expression = self.graph(data["length"]["expression"])
            assert expression.word(expression.root) == length, "length expression"
        graph = self.graph(data["root"])
        facts = self.facts(data["operands"], graph)
        root = graph.nodes[graph.root]
        if isinstance(root, dict) and "Slice" in root:
            view = graph.node(root["Slice"]["source"], "View")
            assert graph.coordinate(view["length"]) == length, "data view length"
        elif isinstance(root, dict) and "BytesExact" in root:
            assert len(root["BytesExact"]) == length, "exact byte length"
        else:
            assert length > 0, "unsupported empty data root"
        # Whole 32-byte windows cover every byte. Sparse display caches do not
        # establish values and are deliberately not consulted, even if present.
        return tuple(self.certified(graph.window(graph.root, offset), facts)
                     for offset in range(0, length, 32))

    def slots(self, store, owner, keys):
        plane = store["persistent"]
        assert plane["failure"] is None, "unsuccessful store"
        graph = self.graph(plane["roots"][owner])
        facts = self.facts(plane["operands"], graph)
        return tuple(self.certified(graph.slot(graph.root, key), facts) for key in keys)

    def rows(self, terms):
        assert not any(isinstance(term, RuntimeWord) for term in terms), "uncertified runtime word"
        if not self.domains:
            return {terms}
        source, choices = next(iter(self.domains.items()))
        return {tuple(choice if isinstance(term, Read) and term.source == source else term
                      for term in terms) for choice in choices}


def check(name, analysis):
    assert all(type(state["id"]) is int and state["id"] >= 0 for state in analysis["states"]), "state ids"
    states = {state["id"]: state for state in analysis["states"]}
    assert len(states) == len(analysis["states"]), "duplicate source state"
    outcomes = analysis["outcomes"]
    if name == "outer-revert":
        assert outcomes and all(outcome["kind"] in {"Revert", "Failure"} for outcome in outcomes)
        possible = set()
        for outcome in outcomes:
            reader = Outcome(outcome, states)
            final, = reader.slots(outcome["store"], OWNER_C, [0])
            saved, = reader.slots(reader.frame["saved_store"], OWNER_C, [0])
            assert reader.rows((final, saved)) == {(5, 5)}, "rollback checkpoint"
            if outcome["kind"] == "Revert":
                words = reader.data(outcome["data"], [0, 64])
                if not words:
                    continue
                current, = reader.slots(reader.payload["store"], OWNER_C, [0])
                assert words[1] == current, "revert data/pre-rollback source identity"
                rows = reader.rows(words + (final, current, saved))
                assert rows <= {(5, value, 5, value, 5) for value in [5, 9]}, rows
                possible |= {row[1] for row in rows}
        assert possible == {5, 9}, (name, "missing old/new revert data", possible)
        return
    assert name in {"busy-x", "busy-x-nested", "static-reuse", "external-guard", "external-change"}, name
    returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
    assert returns, name
    possible = set()
    for outcome in returns:
        reader = Outcome(outcome, states)
        words = reader.data(outcome["data"], [32] if name == "static-reuse" else [64])
        if name.startswith("busy-x"):
            slots = reader.slots(outcome["store"], OWNER_A, [0, 1])
            assert slots == reader.slots(reader.payload["store"], OWNER_A, [0, 1]), "return source store"
            assert words[0] == slots[1], "busy data/ownerA.slot1 source identity"
            rows = reader.rows(words + slots)
            assert rows <= {(value, 1, 0, value) for value in [0, 1, 2]}, rows
            possible |= {row[0] for row in rows}
        elif name == "static-reuse":
            assert reader.rows(words) == {(42,)}, words
        else:
            slots = reader.slots(outcome["store"], OWNER_C, [0, 1])
            assert slots == reader.slots(reader.payload["store"], OWNER_C, [0, 1]), "return source store"
            assert words[1] == slots[0], "return data/ownerC.slot0 source identity"
            guard = int(name == "external-change")
            rows = reader.rows(words + slots)
            assert rows <= {(5, value, value, guard) for value in ([5, 9] if guard else [5])}, rows
            possible |= {row[1] for row in rows}
    expected = {0, 1, 2} if name.startswith("busy-x") else {5, 9} if name == "external-change" else {5}
    if name != "static-reuse":
        assert possible == expected, (name, possible)


def native_analysis(name, report, regions):
    """Select the Native namespace even when Global is the chosen model."""
    assert report["schema_version"] == 16 and report["status"] == "Converged", "Hybrid schema/status"
    assert report["selected_view"] == "Baseline", "selected Global view"
    assert report["numeric_model"] == "product", "numeric model"
    assert report["postprocessing_max_work"] == MAX_WORK, "postprocess allowance"
    assert 0 <= report["postprocessing_work"] <= MAX_WORK, "postprocess work"
    coverage = report["hybrid"]["coverage"]
    assert coverage["chosen"] == "ProcessWithDetail", "Global control with Native detail"
    assert coverage["baseline_current"] is True and coverage["detail_current"] is True, "current graphs"
    assert coverage["detail"] == "Current" and coverage["detail_binding"] == "OriginalOwner", "Native binding"
    assert type(coverage["requested_detail_regions"]) is int and coverage["requested_detail_regions"] == regions, "detail region request"
    assert coverage["requested_detail_work"] == (1 << 64) - 1, "shared detail allowance"
    assert coverage["requested_native_terminal_coverage"] is True, "terminal coverage policy"
    assert isinstance(report.get("native"), dict), "missing Native graph"
    graph, global_graph = report["native"], report["global"]
    analysis, global_analysis = graph["analysis"], global_graph["analysis"]
    config = {"analysis": {"context_depth": 128, "max_states": 100000, "max_transfers": 10000000},
              "max_work": MAX_WORK, "max_call_depth": 1025, "max_memory_bytes": 67108864,
              "use_summaries": True, "root_terminal_coverage": True}
    assert analysis["config"] == config, "original execution policy"
    assert global_analysis["config"] == dict(config, root_terminal_coverage=False), "Global policy"
    for key in ("world", "entry", "domain_spec", "snapshot", "revision"):
        assert analysis[key] == global_analysis[key], ("graph input identity", key)
    for current in (analysis, global_analysis):
        assert current["schema_version"] == 16 and current["status"] == "Converged", "graph schema/status"
        assert current["frontiers"] == [], "graph frontiers"
        summaries = current["procedure_summaries"]
        assert summaries and all(s["closed"] is True and s["procedures"] for s in summaries), "closed procedures"
    assert "ssa" not in global_graph and "partial_ssa" not in global_graph, "Global has no Native SSA"
    assert all(s["executed_pcs"] == [] for s in global_analysis["states"]), "Global MAY is not executed PCs"
    world, entry = analysis["world"], analysis["entry"]
    assert world["fork"] == "osaka" and world["provenance"] == f"offline:unknown-world:{name}:v1", "fixture world"
    assert entry["binding"] == {"Physical": {"address": OWNER_A}} and entry["address"] == OWNER_A, "entry owner"
    environment = entry["environment"]
    assert environment["to"] == {"Concrete": OWNER_A}, "entry target"
    assert environment["caller"] == {"Concrete": "0x0000000000000000000000000000000000000900"}, "entry caller"
    assert environment["is_static"] is False and numeric(evm_numeric(environment["value"]))[1] == {0}, "entry value/static"
    calldata = environment["calldata"]
    assert numeric(evm_numeric(calldata["length"]))[1] == {0}, "entry calldata length"
    assert calldata["root"]["nodes"] == [{"BytesExact": []}] and calldata["root"]["root"] == 0, "entry calldata"
    expected = [OWNER_A] + ([OWNER_C] if regions == 2 else [])
    assert coverage["detail_regions"] == [
        {"source": {"Runtime": {"source": owner, "hash": world["code_hashes"][owner], "fork": world["fork"]}},
         "code_hash": world["code_hashes"][owner]} for owner in expected
    ], "selected source regions"
    work = coverage["work"]
    assert all(type(v) is int and v >= 0 for v in work.values()), "work counters"
    assert work["prior"] == 0 and work["baseline"] == global_analysis["work"], "original payer"
    assert work["detail_allowance"] == MAX_WORK - work["baseline"], "remaining shared allowance"
    assert work["total"] == sum(work[k] for k in ("prior", "baseline", "preparation", "detail")), "cumulative work"
    assert report["work"] == analysis["work"] == work["total"] <= MAX_WORK, "execution work"
    assert global_analysis["transfers"] <= report["transfers"] == analysis["transfers"] <= config["analysis"]["max_transfers"], "cumulative transfers"
    states = {state["id"]: state for state in analysis["states"]}
    assert states and len(states) == len(analysis["states"]), "Native states"
    ssa = graph.get("ssa")
    assert isinstance(ssa, dict) and ssa["complete"] is True and ssa["status"] == "Converged", "Native SSA"
    assert ssa["frontiers"] == [] and ssa["deferred_edges"] == [], "closed Native SSA"
    blocks = {block["state"]: block for block in ssa["blocks"]}
    assert len(blocks) == len(ssa["blocks"]) and blocks.keys() == states.keys(), "Native SSA states"
    executed_sources = set()
    for state_id, state in states.items():
        assert type(state_id) is int and state_id >= 0, "Native state id"
        stack = state["entry"]["call_stack"]
        frames = [stack["root"]["state"]] + [child["state"] for child in stack["children"]]
        assert [frame["key"] for frame in frames] == state["key"]["frames"], "Native source frames"
        frame, pcs, instructions = frames[-1], state["executed_pcs"], blocks[state_id]["instructions"]
        assert [instruction["pc"] for instruction in instructions] == pcs, "Native SSA PCs"
        if pcs:
            key = frame["key"]
            assert key["code_hash"] == world["code_hashes"][key["code_address"]], "Native source hash"
            block, = [block for block in frame["program"]["blocks"] if block["id"] == key["basic_block_index"]]
            original = block["instructions"][:len(pcs)]
            assert [(i["pc"], i["opcode"]) for i in instructions] == [(i["pc"], i["opcode"]) for i in original], "Native instruction source"
            executed_sources.add(key["code_address"])
    assert executed_sources == set(expected), "actual Native source coverage"
    return analysis


def physical_input(analysis):
    """Only the two input ExprArena scope labels are fresh across CLI runs."""
    result = deepcopy({key: analysis[key] for key in
                       ("world", "entry", "config", "domain_spec", "snapshot", "revision")})
    source = result["world"]["storage_source"]
    calldata = result["entry"]["environment"]["calldata"]["root"]
    assert isinstance(source["identity_scope"], str) and source["identity_scope"], "input expression scope"
    assert source["identity_scope"] == calldata["identity_scope"], "shared input expression scope"
    source["identity_scope"] = calldata["identity_scope"] = "input scope"
    return result


def check_relative_callee(name, analysis):
    """Default1 retains relative C receipts; its coarse data is not a joint oracle."""
    states = {state["id"]: state for state in analysis["states"]}
    outcomes = analysis["outcomes"]
    assert outcomes and any(o["kind"] == ("Revert" if name == "outer-revert" else "Return") for o in outcomes), "root terminal outcomes"
    for outcome in outcomes:
        reader = Outcome(outcome, states)
        if name == "outer-revert":
            assert outcome["kind"] in {"Revert", "Failure"}, "outer revert outcomes"
            final, = reader.slots(outcome["store"], OWNER_C, [0])
            saved, = reader.slots(reader.frame["saved_store"], OWNER_C, [0])
            assert reader.rows((final, saved)) == {(5, 5)}, "rollback checkpoint"
        if outcome["kind"] in {"Return", "Revert"}:
            data = outcome["data"]
            assert data["failure"] is None and data["memory"] is False and data["conditional"] == [], "relative root data"
            _, lengths = numeric(evm_numeric(data["length"]))
            allowed_lengths = {0, 64} if name == "outer-revert" else {64} if outcome["kind"] == "Return" else {0}
            assert len(lengths) == 1 and lengths <= allowed_lengths, "relative root length"
            if lengths == {64}:
                graph = data["root"]
                assert graph["identity_scope"] == reader.scope and graph["root"] == len(graph["nodes"]) - 1, "relative data source"
                nodes = graph["nodes"]
                view = nodes[nodes[-1]["Slice"]["source"]]["View"]
                write = nodes[view["state"]]["WriteWord"]
                assert nodes[view["offset"]] == {"Constant": "0x0"} and nodes[view["length"]] == {"Constant": "0x40"}, "relative return view"
                assert nodes[write["offset"]] == {"Constant": "0x0"} and nodes[write["value"]] == {"Constant": "0x5"}, "original first word"
    summaries = [s for s in analysis["procedure_summaries"] if s["source_current"] and s["kind"] == "KnownCall"]
    assert summaries, "current relative callee"
    for summary in summaries:
        state = states[summary["source_state"]]
        procedure = summary["procedures"][summary["root_procedure"]]
        frame = procedure["frame"]
        assert procedure["semantics"] == "Native" and frame == state["key"]["frames"][-1], "relative source frame"
        assert frame["code_address"] == frame["address"] == OWNER_C and frame["caller"] == {"Concrete": OWNER_A}, "relative callee owner"
        assert frame["code_hash"] == analysis["world"]["code_hashes"][OWNER_C] and frame["is_static"] is False, "relative source/static"
        assert state["executed_pcs"] == [], "relative receipt is not World execution"
        nodes = [n for n in summary["regional_cfg"]["nodes"] if n["procedure"] == summary["root_procedure"] and not n["excluded"]]
        source_fields = ("code_address", "code_hash", "mode", "address", "address_value", "caller", "is_static")
        assert all(all(n["frame"][key] == frame[key] for key in source_fields) for n in nodes), "relative control source/owner"
        assert nodes and all(n["transfer"]["semantics"] == "Native" and n["transfer"]["current"] is True
                             and n["transfer"]["complete"] is True and n["transfer"]["visited_block"] is True for n in nodes), "current relative control"
        assert any(e["from"] == state["id"] and e["kind"] == {"Summary": "Return"}
                   and states[e["to"]]["key"]["frames"][-1]["code_address"] == OWNER_A
                   for e in analysis["edges"]), "relative call return"
        outputs = procedure["outputs"]
        assert {o["kind"] for o in outputs} == {"Return", "Failure"}, "relative callee outputs"
        for output in outputs:
            data = output["data"]
            expected_length = 32 if output["kind"] == "Return" else 0
            assert data.keys() == {"length", "memory", "content", "words"} and data["words"] == {}, "relative data payload"
            assert data["memory"] is False and numeric(evm_numeric(data["length"]))[1] == {expected_length}, "relative callee data"
            allowed = ("Zero",) if expected_length == 0 else ({"Cells": {}}, {"Exact": [0] * 31 + [5]})
            assert data["content"] in allowed, "relative callee content"
        for store in [procedure["input"]] + [o["store"] for o in outputs]:
            cells = [s for s in store["persistent"]["slots"] if s["address"] == OWNER_C]
            slots = {number(s["slot"]): s["value"] for s in cells}
            assert len(cells) == len(slots) == 2 and slots.keys() == {0, 1}, "relative callee Store"
            evm_numeric(slots[0])  # Top is honest here; the region2 check proves the joint finite facts.
            assert numeric(evm_numeric(slots[1]))[1] == {1}, "relative callee guard"


def check_report(name, report, detail_report=None):
    analysis = native_analysis(name, report, 1)
    if name in PRIVATE_CALLEE:
        # The old shared-source proof needs C's selected private carriers. Keep
        # default1 as a separate checked request, with no execution-budget reset.
        assert detail_report is not None, "additional region2 private oracle required"
        check_relative_callee(name, analysis)
        detail = native_analysis(name, detail_report, 2)
        assert physical_input(analysis) == physical_input(detail), "same original input and budget"
        check(name, detail)
    else:
        assert detail_report is None, "unexpected additional detail report"
        check(name, analysis)


def selftest(name, report, detail_report):
    """Mutate real fixture envelopes; none of these checks reruns execution."""
    check_report(name, report, detail_report)

    def rejected(change, message):
        changed, detail = deepcopy(report), deepcopy(detail_report)
        change(changed, detail)
        try:
            check_report(name, changed, detail)
        except AssertionError:
            return
        raise AssertionError(message)

    rejected(lambda r, _: r.update(native=None), "accepted missing Native")
    rejected(lambda r, _: r["global"]["analysis"]["states"][0].update(executed_pcs=[0]), "accepted Global executed PCs")
    rejected(lambda r, _: r["native"].update(ssa=None), "accepted missing Native SSA")
    rejected(lambda r, _: r["hybrid"]["coverage"].update(requested_detail_regions=2), "accepted replaced default1")
    rejected(lambda r, _: r["hybrid"]["coverage"]["work"].update(total=r["work"] + 1), "accepted fresh execution payer")
    rejected(lambda r, _: r["native"]["ssa"]["blocks"][0]["instructions"][0].update(opcode=0xfe), "accepted wrong SSA source")

    def larger_budget(r, _):
        for namespace in ("global", "native"):
            r[namespace]["analysis"]["config"]["max_work"] += 1

    rejected(larger_budget, "accepted raised original budget")

    def substitute_global(r, _):
        config = r["native"]["analysis"]["config"]
        r["native"]["analysis"] = deepcopy(r["global"]["analysis"])
        r["native"]["analysis"]["config"] = config

    rejected(substitute_global, "accepted Global as Native")
    if detail_report is not None:
        def different_input(_, detail):
            for namespace in ("global", "native"):
                detail[namespace]["analysis"]["world"]["fingerprint"] = "different"

        rejected(different_input, "accepted changed detail input")
        rejected(lambda _, d: d["hybrid"]["coverage"].update(requested_detail_regions=1), "accepted unlabeled detail request")
        rejected(lambda r, _: r["native"]["analysis"]["procedure_summaries"].clear(), "accepted empty relative evidence")

        def foreign_source(r, _):
            summaries = r["native"]["analysis"]["procedure_summaries"]
            summary = next(s for s in summaries if s["source_current"] and s["kind"] == "KnownCall")
            node = next(n for n in summary["regional_cfg"]["nodes"] if n["procedure"] == summary["root_procedure"] and not n["excluded"])
            node["frame"]["code_hash"] = "0x" + "00" * 32

        rejected(foreign_source, "accepted foreign relative control source")

        if name == "external-change":
            def empty_return(r, _):
                outcome = next(o for o in r["native"]["analysis"]["outcomes"] if o["kind"] == "Return")
                outcome["data"]["length"] = {"numeric": {"constants": ["0x0"]}}

            rejected(empty_return, "accepted empty root Return")

        def empty_relative(r, _, field):
            summaries = r["native"]["analysis"]["procedure_summaries"]
            summary = next(s for s in summaries if s["source_current"] and s["kind"] == "KnownCall")
            procedure = summary["procedures"][summary["root_procedure"]]
            procedure["outputs"][0][field] = {}

        # Missing canonical payloads must fail closed before any private proof.
        for field in ("store", "data"):
            changed = deepcopy(report)
            empty_relative(changed, None, field)
            try:
                check_report(name, changed, detail_report)
            except (AssertionError, KeyError):
                pass
            else:
                raise AssertionError("accepted empty relative " + field)

        def separate_source(_, detail):
            # Change only the data source, retaining all numeric certificates.
            def fresh(value):
                if isinstance(value, dict):
                    if "Fresh" in value:
                        value["Fresh"] += 1000000
                    else:
                        for child in value.values():
                            fresh(child)
                elif isinstance(value, list):
                    for child in value:
                        fresh(child)

            outcomes = detail["native"]["analysis"]["outcomes"]
            outcome = next(o for o in outcomes if any(isinstance(n, dict) and "SlotsInput" in n for n in o["data"]["root"]["nodes"]))
            fresh(outcome["data"])

        rejected(separate_source, "accepted independent data/Store source certificates")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("name")
    parser.add_argument("report")
    parser.add_argument("--detail-report", help="same input with explicit max-detail-regions2 for the private C oracle")
    parser.add_argument("--selftest", action="store_true", help="also reject corrupted copies of this fixture report")
    args = parser.parse_args()
    with open(args.report) as source:
        report = json.load(source)
    detail = None
    if args.detail_report:
        with open(args.detail_report) as source:
            detail = json.load(source)
    if args.selftest:
        selftest(args.name, report, detail)
    else:
        check_report(args.name, report, detail)
