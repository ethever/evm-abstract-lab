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


def _buffer(report, index, memory, length):
    assert type(index) is int and 0 <= index < len(report["byte_arrays"])
    data = report["byte_arrays"][index]
    assert data["memory"] is memory and data["length"]["constants"] == [hex(length)]
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
        assert value["constants"] is None or byte in [int(word, 16) for word in value["constants"]]
        assert int(value["unsigned"]["lower"], 16) <= byte <= int(value["unsigned"]["upper"], 16)
        assert byte & int(value["known_zero"], 16) == 0
        assert byte & int(value["known_one"], 16) == int(value["known_one"], 16)


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
