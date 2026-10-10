# Native index glyphs

These two small font subsets let egui keep canonical Unicode text such as
`σ₁₂₈` and `σᵖ₀` while laying out ordinary digit and `p` outlines at a smaller
size and the chosen baseline. Their Unicode character maps alias `₀–₉` to the
original `0–9` glyphs and `ᵖ` to the original `p` glyph. Each face contains only
those eleven glyphs and `.notdef`; the original glyph outlines, hinting,
advance widths, units-per-em and vertical metrics are preserved. The UI owns
size, baseline and the original face's egui font tweak.

The assets derive from `epaint_default_fonts` **0.36.2**, the version locked in
`Cargo.lock`. [provenance.json](provenance.json) records the exact source-font,
licence and output SHA-256 hashes, mapping, names and generator version. The
upstream source font bytes are not modified. These subsets belong only to the
named mathematical index font families; ordinary UI text continues using its
existing fonts.

| Asset | Source | Internal family | Licence |
| --- | --- | --- | --- |
| `math-indices-hack.ttf` | `Hack-Regular.ttf` 3.003 | MathIndices Hack | [Hack MIT and Bitstream Vera notices](Hack-Regular.txt) |
| `math-indices-sans.ttf` | `Ubuntu-Light.ttf` 0.83 | MathIndices Sans | [Ubuntu Font Licence 1.0](UFL.txt) |

The Hack derivative's font names contain neither reserved name `Bitstream`
nor `Vera`. The restricted character coverage and repurposed Unicode mappings
make the sans subset a substantially changed font; its names avoid the original
font name under UFL clause 2(b). Copyright and licence notices are retained both
here and in each font's name-table metadata, including the full licence in
name ID 13. The font assets retain their original licences independently of the
Rust application's licence.

To regenerate, use Python 3 with **fontTools 4.57.0** and pass the `fonts`
directory of the already cached/Nix-provided `epaint_default_fonts-0.36.2`:

```sh
python3 crates/evm-abstract-web/assets/fonts/generate.py /path/to/epaint_default_fonts-0.36.2/fonts
```

The script performs no downloads, rejects source hashes that differ from the
locked assets, validates aliases and retained metrics, and writes deterministic
TTFs and provenance. This is a maintenance tool; fontTools is not an application
or build dependency. Other fontTools versions may produce different bytes and
must be reviewed before regenerating checked-in assets.
