"""Fixed wire-graph assertions for the word42 and RPC browser fixtures.

The producer exports stored numeric upper bounds separately from the complete
ByteView root. Match only these fixtures' literal windows and ordered writes;
unsupported constructors fail closed. This does not evaluate symbolic graphs.
"""


class _Graph:
    def __init__(self, report):
        self.nodes = report["symbolic"]["nodes"]

    def node(self, ref, before, kind=None, fields=None):
        assert isinstance(ref, dict) and set(ref) == {"node"}, ref
        index = ref["node"]
        assert type(index) is int and 0 <= index < before <= len(self.nodes), ref
        value = self.nodes[index]
        if kind is not None:
            assert isinstance(value, dict) and set(value) == {kind}, value
            value = value[kind]
            if fields is not None:
                assert isinstance(value, dict) and set(value) == set(fields), value
        return index, value

    def view(self, ref, before, length):
        index, value = self.node(ref, before, "View", ("state", "offset", "length"))
        assert self.node(value["offset"], index, "Constant")[1] == "0x0"
        assert self.node(value["length"], index, "Constant")[1] == hex(length)
        return index, value

    def background(self, ref, before):
        # Every accepted write covers the entire checked window. Its earlier
        # background need not be zero, but must still be a valid byte leaf.
        _, value = self.node(ref, before)
        if value != "BytesZero":
            assert isinstance(value, dict) and set(value) == {"BytesExact"}
            payload = value["BytesExact"]
            assert isinstance(payload, str) and payload.startswith("0x") and len(payload) % 2 == 0
            assert all(char in "0123456789abcdef" for char in payload[2:])

    def zero_window(self, ref, before, start, end):
        """Prove an untouched window of a literal zero or exact byte leaf."""
        self.background(ref, before)
        _, value = self.node(ref, before)
        if value != "BytesZero":
            # BytesExact has zero padding beyond its complete content.
            assert not any(bytes.fromhex(value["BytesExact"][2:])[start:end])


def _numeric_contains(value, actual):
    """Check one fixed Word256 against every stored numeric component."""
    limit = 1 << 256
    sign = 1 << 255

    def word(encoded):
        assert isinstance(encoded, str) and encoded.startswith("0x") and 2 < len(encoded) <= 66
        assert all(char in "0123456789abcdef" for char in encoded[2:])
        parsed = int(encoded, 16)
        assert 0 <= parsed < limit
        return parsed

    assert type(actual) is int and 0 <= actual < limit
    constants = value["constants"]
    assert constants is None or isinstance(constants, list)
    finite = constants is None or actual in [word(item) for item in constants]
    zero, one = word(value["known_zero"]), word(value["known_one"])
    unsigned, signed = value["unsigned"], value["signed"]
    unsigned_lo, unsigned_hi = word(unsigned["lower"]), word(unsigned["upper"])
    # The server removes the internal sign bias for two-complement wire words.
    signed_lo, signed_hi = word(signed["lower"]) ^ sign, word(signed["upper"]) ^ sign
    assert type(value["nonzero"]) is bool
    congruence = value["congruence"]
    if congruence == "Any":
        congruent = True
    else:
        assert isinstance(congruence, dict) and len(congruence) == 1
        if "Exact" in congruence:
            congruent = actual == word(congruence["Exact"])
        else:
            assert set(congruence) == {"Modulo"}
            modulo = congruence["Modulo"]
            assert isinstance(modulo, dict) and set(modulo) == {"modulus", "residue"}
            modulus, residue = word(modulo["modulus"]), word(modulo["residue"])
            assert 0 < modulus and residue < modulus
            congruent = actual % modulus == residue
    return (finite and actual & zero == 0 and actual & one == one
            and unsigned_lo <= actual <= unsigned_hi
            and signed_lo <= actual ^ sign <= signed_hi
            and (not value["nonzero"] or actual != 0) and congruent)


def _buffer(report, index, memory, length):
    assert type(index) is int and 0 <= index < len(report["byte_arrays"])
    data = report["byte_arrays"][index]
    assert data["memory"] is memory and data["length"]["constants"] == [hex(length)]
    assert _numeric_contains(data["length"], length)
    return data


def _stored_upper_bounds(data, expected):
    # Current stored facts are only a consistency check after the root proof.
    # They never supply an exact witness or replace a missing graph operand.
    cells = {}
    for cell in data["cells"]:
        offset, value = cell["offset"], cell["value"]
        assert type(offset) is int and 0 <= offset < len(expected) and offset not in cells
        assert type(value) is int and 0 <= value < len(data["values"])
        cells[offset] = data["values"][value]
    for offset, byte in enumerate(expected):
        value = cells.get(offset, data["default"])
        assert _numeric_contains(value, byte)


def assert_empty_return_data(report, index):
    """Prove the zero-byte view; backing byte states and pool IDs may differ."""
    data = _buffer(report, index, False, 0)
    assert data["cells"] == []
    graph = _Graph(report)
    view_id, view = graph.view(data["symbolic"], len(graph.nodes), 0)
    _, state = graph.node(view["state"], view_id)
    if isinstance(state, str):
        assert state == "BytesZero"
    else:
        assert isinstance(state, dict) and len(state) == 1
        assert next(iter(state)) in {"BytesInput", "BytesExact", "WriteByte", "WriteWord", "Copy", "Slice", "BytesIte"}


def assert_return_word42(report, outcome):
    """One root-owned Return must jointly contain all 32 bytes of word 42."""
    assert outcome["kind"] == "Return"
    owner = next(block for block in report["cfg"] if block["id"] == outcome["state"])
    assert owner["frame_depth"] == 1 and owner["program"] == report["metadata"]["root_program"]
    data = _buffer(report, outcome["data"], False, 32)
    graph = _Graph(report)
    outer_id, outer = graph.view(data["symbolic"], len(graph.nodes), 32)
    slice_id, source = graph.node(outer["state"], outer_id, "Slice")
    inner_id, inner = graph.view(source, slice_id, 32)
    write_id, write = graph.node(inner["state"], inner_id, "WriteWord", ("previous", "offset", "value"))
    assert graph.node(write["offset"], write_id, "Constant")[1] == "0x0"
    assert graph.node(write["value"], write_id, "Constant")[1] == "0x2a"
    graph.background(write["previous"], write_id)
    _stored_upper_bounds(data, (42).to_bytes(32, "big"))


def assert_child_bytes(report, frame):
    """The same child's one-byte input and its complete padded MSTORE."""
    calldata = _buffer(report, frame["calldata"], False, 1)
    memory = _buffer(report, frame["memory"], True, 32)
    graph = _Graph(report)
    input_id, input_view = graph.view(calldata["symbolic"], len(graph.nodes), 1)
    slice_id, source = graph.node(input_view["state"], input_id, "Slice")
    source_id, source_view = graph.view(source, slice_id, 1)
    byte_id, write_byte = graph.node(source_view["state"], source_id, "WriteByte",
                                     ("previous", "offset", "value"))
    assert graph.node(write_byte["offset"], byte_id, "Constant")[1] == "0x0"
    assert graph.node(write_byte["value"], byte_id, "Constant")[1] == "0xab"
    graph.background(write_byte["previous"], byte_id)

    memory_id, memory_view = graph.view(memory["symbolic"], len(graph.nodes), 32)
    word_id, write_word = graph.node(memory_view["state"], memory_id, "WriteWord",
                                     ("previous", "offset", "value"))
    assert graph.node(write_word["offset"], word_id, "Constant")[1] == "0x0"
    graph.background(write_word["previous"], word_id)
    read_id, read = graph.node(write_word["value"], word_id, "WordRead", ("view", "offset"))
    assert graph.node(read["offset"], read_id, "Constant")[1] == "0x0"
    read_view_id, read_view = graph.view(read["view"], read_id, 32)
    graph.node(read_view["state"], read_view_id, "Slice")
    # Identity ties the copy to this frame's input and its one-byte Slice
    # boundary. All other 31 bytes of the captured word must be zero padding.
    assert read_view["state"] == input_view["state"]
    _stored_upper_bounds(calldata, bytes([0xab]))
    _stored_upper_bounds(memory, bytes([0xab]) + bytes(31))


def assert_summary_child_bytes(report, child, caller_before, caller_after):
    """Match this fixture's relative return and the same caller's output copy."""
    graph = _Graph(report)
    calldata = _buffer(report, child["calldata"], False, 1)
    input_id, input_view = graph.view(calldata["symbolic"], len(graph.nodes), 1)
    slice_id, source = graph.node(input_view["state"], input_id, "Slice")
    source_id, source_view = graph.view(source, slice_id, 1)
    byte_id, write_byte = graph.node(source_view["state"], source_id, "WriteByte",
                                     ("previous", "offset", "value"))
    assert graph.node(write_byte["offset"], byte_id, "Constant")[1] == "0x0"
    assert graph.node(write_byte["value"], byte_id, "Constant")[1] == "0xab"
    graph.zero_window(write_byte["previous"], byte_id, 1, 64)
    # The relative delivery instantiates the complete fixed return window as
    # exact bytes with zero padding; it is not a fabricated child memory frame.
    returned = _buffer(report, caller_after["returndata"], False, 32)
    return_id, return_view = graph.view(returned["symbolic"], len(graph.nodes), 32)
    assert graph.node(return_view["state"], return_id, "BytesExact")[1] == "0xab"
    before = _buffer(report, caller_before["memory"], True, 64)
    after = _buffer(report, caller_after["memory"], True, 64)
    before_id, before_view = graph.view(before["symbolic"], len(graph.nodes), 64)
    assert before_view["state"] == source_view["state"]
    after_id, after_view = graph.view(after["symbolic"], len(graph.nodes), 64)
    copy_id, copy = graph.node(after_view["state"], after_id, "Copy",
                               ("previous", "destination", "source", "length"))
    assert copy["previous"] == before_view["state"]
    assert copy["source"] == returned["symbolic"]
    assert graph.node(copy["destination"], copy_id, "Constant")[1] == "0x20"
    assert graph.node(copy["length"], copy_id, "Constant")[1] == "0x20"
    _stored_upper_bounds(calldata, bytes([0xab]))
    _stored_upper_bounds(returned, bytes([0xab]) + bytes(31))
    _stored_upper_bounds(before, bytes([0xab]) + bytes(63))
    _stored_upper_bounds(after, (bytes([0xab]) + bytes(31)) * 2)
