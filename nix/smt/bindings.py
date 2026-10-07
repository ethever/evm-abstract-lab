"""Verify the actual CaDiCaL PLT bindings in both native DSO load orders."""

import ctypes
import json
import os
import subprocess
import sys
from pathlib import Path


def readback(paths, order):
    handles = [
        ctypes.CDLL(str(paths[name]), mode=os.RTLD_NOW | os.RTLD_GLOBAL)
        for name in order
    ]
    mappings = Path("/proc/self/maps").read_text().splitlines()
    results = []
    symbol = "_ZN7CaDiCaL6Solver5solveEv"
    for name, path in paths.items():
        real = str(path.resolve())
        rows = [row.split() for row in mappings if row.endswith(real)]
        if not rows:
            raise ValueError(f"native DSO was not mapped: {real}")
        base = min(int(row[0].split("-")[0], 16) - int(row[2], 16) for row in rows)
        symbols = subprocess.check_output(["readelf", "--dyn-syms", "-W", str(path)], text=True)
        definition = int(
            next(line.split()[1] for line in symbols.splitlines() if line.split() and line.split()[-1] == symbol),
            16,
        )
        relocations = subprocess.check_output(["readelf", "-rW", str(path)], text=True)
        slot = int(next(line.split()[0] for line in relocations.splitlines() if symbol in line), 16)
        actual = ctypes.c_void_p.from_address(base + slot).value
        expected = base + definition
        if actual != expected:
            raise ValueError(f"{name} called another DSO's CaDiCaL: {actual:#x} != {expected:#x}")
        results.append({"consumer": name, "self_binding": True})
    # Retain every dlopen handle through the readback.
    assert len(handles) == 2
    print(json.dumps({"load_order": order, "bindings": results}))


if __name__ == "__main__":
    readback(
        {"cvc5": Path(sys.argv[1]), "bitwuzla": Path(sys.argv[2])},
        sys.argv[3:],
    )
