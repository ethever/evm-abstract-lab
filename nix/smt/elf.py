"""Repair pinned ELF metadata and hide each solver's private CaDiCaL copy.

Reserved indices: LSB Core 3.1, 11.7.2 (LOCAL=0, GLOBAL=1):
https://refspecs.linuxfoundation.org/LSB_3.1.1/LSB-Core-generic/LSB-Core-generic/symversion.html
LLD rejects a defined global with LOCAL, preserving LOCAL on undefined symbols:
https://github.com/llvm/llvm-project/blob/llvmorg-21.1.2/lld/ELF/InputFiles.cpp#L1504-L1537
"""

import hashlib
import json
import struct
import sys
from pathlib import Path


class Elf:
    def __init__(self, data):
        self.data = data
        if data[:7] != b"\x7fELF\x02\x01\x01":
            raise ValueError("expected ELF64 little-endian version 1")
        header = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
        if header[1] not in (62, 183):
            raise ValueError("expected x86_64 or AArch64")
        if header[10] != 64 or not header[11]:
            raise ValueError("unexpected section headers")
        offset, size, count = header[5], header[10], header[11]
        if offset + size * count > len(data):
            raise ValueError("section headers exceed the file")
        self.sections = [
            struct.unpack_from("<IIQQQQIIQQ", data, offset + i * size)
            for i in range(count)
        ]
        strings = self.section_data(self.sections[header[12]])
        self.named = {
            self.string(strings, section[0]): section for section in self.sections
        }
        self.dynamic = self.named.get(b".dynsym")
        self.versions = self.named.get(b".gnu.version")
        if self.dynamic is not None:
            if self.dynamic[9] != 24 or self.dynamic[5] % 24:
                raise ValueError("unexpected dynamic symbol entries")
            if self.versions is not None and self.versions[5] != self.dynamic[5] // 12:
                raise ValueError("version table and dynamic symbol counts differ")
            self.symbol_strings = self.section_data(self.sections[self.dynamic[6]])

    def section_data(self, section):
        start, size = section[4], section[5]
        if start + size > len(self.data):
            raise ValueError("section exceeds the file")
        return self.data[start : start + size]

    @staticmethod
    def string(strings, start):
        return bytes(strings[start : strings.index(0, start)])

    def symbols(self):
        if self.dynamic is None:
            return
        for index in range(self.dynamic[5] // 24):
            offset = self.dynamic[4] + index * 24
            name, info, other, section, value, size = struct.unpack_from(
                "<IBBHQQ", self.data, offset
            )
            version = (
                struct.unpack_from("<H", self.data, self.versions[4] + index * 2)[0]
                if self.versions is not None
                else 1
            )
            yield {
                "name": self.string(self.symbol_strings, name),
                "binding": info >> 4,
                "visibility": other & 3,
                "section": section,
                "version": version,
                "offset": offset,
                "index": index,
            }

    def imports(self):
        return sorted(
            (s["name"], s["binding"], s["version"])
            for s in self.symbols()
            if not s["section"]
        )

    def code(self):
        return {
            name: hashlib.sha256(self.section_data(section)).hexdigest()
            for name, section in self.named.items()
            if section[1] == 1 and section[2] & 4
        }


def repair(path, original):
    data = bytearray(path.read_bytes())
    before = Elf(original.read_bytes())
    elf = Elf(data)
    if elf.imports() != before.imports() or elf.code() != before.code():
        raise ValueError(f"relocation changed imports or executable bytes: {path}")
    normalized = hidden = 0
    for symbol in elf.symbols():
        if not symbol["section"]:
            if b"CaDiCaL" in symbol["name"]:
                raise ValueError(f"external CaDiCaL dependency cannot be hidden: {path}")
            continue
        if symbol["binding"] not in (1, 2, 10):
            continue
        if elf.versions is not None and symbol["version"] & 0x7FFF == 0:
            if symbol["version"] != 0:
                raise ValueError(f"unexpected hidden LOCAL version: {path}")
            # ELF's reserved version 0 denotes LOCAL; version 1 denotes an
            # unversioned GLOBAL. Do not change imported version requirements.
            struct.pack_into(
                "<H", data, elf.versions[4] + symbol["index"] * 2, 1
            )
            normalized += 1
        if b"CaDiCaL" in symbol["name"]:
            # STV_HIDDEN makes the owning DSO's references resolve internally,
            # independently of which solver was loaded first.
            position = symbol["offset"] + 5
            data[position] = (data[position] & ~3) | 2
            hidden += 1
    after = Elf(data)
    if after.imports() != before.imports() or after.code() != before.code():
        raise ValueError(f"repair changed imports or executable bytes: {path}")
    for symbol in after.symbols():
        if symbol["section"] and symbol["binding"] in (1, 2, 10):
            if symbol["version"] & 0x7FFF == 0:
                raise ValueError(f"export still has reserved LOCAL version: {path}")
            if b"CaDiCaL" in symbol["name"] and symbol["visibility"] != 2:
                raise ValueError(f"CaDiCaL definition remains externally visible: {path}")
    # Every non-CaDiCaL dynamic symbol retains its binding and visibility.
    public_before = sorted(
        (s["name"], s["binding"], s["visibility"])
        for s in before.symbols()
        if b"CaDiCaL" not in s["name"]
    )
    public_after = sorted(
        (s["name"], s["binding"], s["visibility"])
        for s in after.symbols()
        if b"CaDiCaL" not in s["name"]
    )
    if public_before != public_after:
        raise ValueError(f"public native interface changed: {path}")
    path.write_bytes(data)
    return {"file": str(path), "normalized_versions": normalized, "hidden_cadical": hidden}


def main():
    output, original, provider = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    reports = []
    definitions = set()
    for path in output.rglob("*"):
        if not path.is_file() or path.is_symlink():
            continue
        with path.open("rb") as source:
            magic = source.read(4)
        if magic == b"\x7fELF":
            reports.append(repair(path, original / path.relative_to(output)))
            definitions.update(
                symbol["name"]
                for symbol in Elf(path.read_bytes()).symbols()
                if symbol["section"]
            )
    if not reports or not any(report["hidden_cadical"] for report in reports):
        raise ValueError(f"expected embedded CaDiCaL implementation in {provider}")
    expected = (
        {b"cvc5_check_sat", b"cvc5_mk_term", b"cvc5_set_option"}
        if provider == "cvc5"
        else {b"bitwuzla_check_sat", b"bitwuzla_mk_term", b"bitwuzla_new"}
    )
    expected.add(b"_ZN7CaDiCaL6Solver5solveEv")
    if not expected <= definitions:
        raise ValueError(f"expected pinned native interface is missing: {expected - definitions}")
    print(json.dumps(reports, indent=2))


if __name__ == "__main__":
    main()
