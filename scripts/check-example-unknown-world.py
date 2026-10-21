#!/usr/bin/env python3
"""Check the bounded immutable carriers of the six unknown-world examples."""

from dataclasses import dataclass
import json
import sys


WORD_MAX = (1 << 256) - 1
SIGN = 1 << 255
# These captures need at most 19 nodes, 64 bytes, one read and three choices,
# plus one runtime word whose explicit numeric certificate is a singleton.
MAX_NODES, MAX_BYTES, MAX_CHOICES = 19, 64, 3
MAX_RUNTIME_WORDS = 1
NUMERIC_FIELDS = {"Constants", "known_bits", "interval", "congruence", "nonzero"}
OWNER_A = "0x0000000000000000000000000000000000000101"
OWNER_C = "0x0000000000000000000000000000000000000301"
TRUE = {"constraints": [], "bottom": False, "path": {"root": 0, "nodes": ["True"]}}


def number(value):
    assert type(value) in (str, int), ("invalid word", value)
    if isinstance(value, str):
        assert len(value) <= 78, "oversized word"
        value = int(value, 0)
    assert 0 <= value <= WORD_MAX, ("invalid Word256", value)
    return value


def numeric(value):
    """Intersect every numeric component; no interval/Top fallback for reads."""
    required = NUMERIC_FIELDS
    assert isinstance(value, dict) and required <= value.keys(), ("incomplete numeric", value)
    assert value.keys() <= required | {"provenance", "expression"}, value
    constants = value["Constants"]
    assert isinstance(constants, list) and 1 <= len(constants) <= MAX_CHOICES, value
    constants = tuple(number(item) for item in constants)
    assert constants == tuple(sorted(set(constants))), value
    bits, interval = value["known_bits"], value["interval"]
    assert bits.keys() == {"zero", "one"}, bits
    assert interval.keys() == {"unsigned_lo", "unsigned_hi", "signed_lo", "signed_hi"}, interval
    zero, one = number(bits["zero"]), number(bits["one"])
    lo, hi = number(interval["unsigned_lo"]), number(interval["unsigned_hi"])
    slo, shi = number(interval["signed_lo"]), number(interval["signed_hi"])
    assert not zero & one and lo <= hi and slo <= shi, value
    relation = value["congruence"]
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
    nonzero = value["nonzero"]
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
                        assert binding["numeric"].keys() == NUMERIC_FIELDS, "binding numeric fields"
                        numeric(binding["numeric"])
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
        certificate, choices = numeric(binding["numeric"])
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
            _, values = numeric(value)
            if isinstance(term, (Read, RuntimeWord)) or "expression" in value:
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
        _, values = numeric(data["length"])
        assert len(values) == 1 and values <= set(lengths), ("exact data length", values)
        length = next(iter(values))
        # Native empty buffers carry the exact numeric length without an ExprId.
        if "expression" in data["length"]:
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


if __name__ == "__main__":
    name, path = sys.argv[1:]
    with open(path) as report:
        check(name, json.load(report)["analysis"])
