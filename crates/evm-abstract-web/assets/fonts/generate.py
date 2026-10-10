#!/usr/bin/env python3
"""Build indexed glyph aliases from the exact locked egui font assets.

Maintenance tool only: Python/fontTools is not a runtime/build dependency.
Pass the cached epaint_default_fonts-0.36.2/fonts directory as the sole argument.
"""

import argparse
import hashlib
import json
from pathlib import Path

import fontTools
from fontTools import subset
from fontTools.ttLib import TTFont

SOURCES = (
    (
        "Hack-Regular.ttf",
        "15f55cc0c85a2988d2b4b3a8cdb5d77fdfbaf319e1bb5309d725db9818fb7125",
        "math-indices-hack.ttf",
        "MathIndices Hack",
        "MathIndicesHack-Regular",
        "Hack-Regular.txt",
    ),
    (
        "Ubuntu-Light.ttf",
        "80307b8da7649aa4ee4d484b232140e3ce1ec0ca093073d3c53c8f5a5ced7a70",
        "math-indices-sans.ttf",
        "MathIndices Sans",
        "MathIndicesSans-Light",
        "UFL.txt",
    ),
)
ASCII = tuple(map(ord, "0123456789p"))
ALIASES = {0x2080 + digit: ord(str(digit)) for digit in range(10)} | {0x1D56: ord("p")}
OUTPUT = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build(source_dir, source_name, expected, target_name, family, postscript, license_name):
    source = source_dir / source_name
    if digest(source) != expected:
        raise ValueError(f"unexpected font bytes: {source}; requires locked epaint_default_fonts 0.36.2")
    original = TTFont(source, recalcTimestamp=False)
    source_map = original.getBestCmap()
    metrics = {code: original["hmtx"][source_map[code]] for code in ASCII}
    face_metrics = {
        table: {field: getattr(original[table], field) for field in fields}
        for table, fields in {
            "head": ("unitsPerEm", "created", "modified"),
            "hhea": ("ascent", "descent", "lineGap"),
            "OS/2": ("sTypoAscender", "sTypoDescender", "sTypoLineGap", "usWinAscent", "usWinDescent"),
        }.items()
    }
    options = subset.Options()
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.name_legacy = True
    options.glyph_names = True
    options.notdef_glyph = True
    options.notdef_outline = True
    options.layout_features = []
    options.drop_tables.append("TTFA")
    options.recalc_bounds = False
    options.recalc_timestamp = False
    options.prune_unicode_ranges = False
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(unicodes=ASCII)
    subsetter.subset(original)
    for table in original["cmap"].tables:
        if table.isUnicode():
            for alias, code in ALIASES.items():
                table.cmap[alias] = table.cmap[code]
    names = {
        1: family,
        2: "Regular" if "Hack" in family else "Light",
        3: f"evm-abstract-lab:{postscript}:1",
        4: family + (" Regular" if "Hack" in family else " Light"),
        6: postscript,
        16: family,
        17: "Regular" if "Hack" in family else "Light",
    }
    for record in original["name"].names:
        if record.nameID in names:
            record.string = names[record.nameID].encode(record.getEncoding())
    # Add preferred-family records when the source did not have them.
    for name_id, value in names.items():
        original["name"].setName(value, name_id, 3, 1, 0x409)
    # Include the full font licence in metadata, including embedded/Wasm copies.
    original["name"].setName((source_dir / license_name).read_text(), 13, 3, 1, 0x409)
    if license_name == "UFL.txt":
        original["name"].setName("https://ubuntu.com/legal/font-licence", 14, 3, 1, 0x409)
    target = OUTPUT / target_name
    original.save(target, reorderTables=True)
    derived = TTFont(target, recalcTimestamp=False)
    cmap = derived.getBestCmap()
    assert set(cmap) == set(ASCII) | set(ALIASES)
    for code in ASCII:
        assert derived["hmtx"][cmap[code]] == metrics[code]
    for alias, code in ALIASES.items():
        assert cmap[alias] == cmap[code]
        assert derived["hmtx"][cmap[alias]] == metrics[code]
    for table, fields in face_metrics.items():
        assert {field: getattr(derived[table], field) for field in fields} == fields
    return {
        "source": source_name,
        "source_sha256": expected,
        "output": target_name,
        "output_sha256": digest(target),
        "bytes": target.stat().st_size,
        "family": family,
        "license": license_name,
        "license_sha256": digest(source_dir / license_name),
        "glyph_count": len(derived.getGlyphOrder()),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_dir", type=Path)
    arguments = parser.parse_args()
    results = [build(arguments.source_dir, *source) for source in SOURCES]
    manifest = {
        "source_package": "epaint_default_fonts",
        "source_version": "0.36.2",
        "generator": "generate.py",
        "fonttools_version": fontTools.__version__,
        "aliases": {f"U+{alias:04X}": chr(code) for alias, code in ALIASES.items()},
        "fonts": results,
    }
    (OUTPUT / "provenance.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
