# Terminal glyph rendering

`src/ui/terminal/native_glyph.rs` constructs geometry from character semantics:
four directional stroke weights for solid box drawing, and an eight-by-eight
occupancy grid for block elements. The geometry is developed in this repository;
it is not a port of another terminal renderer. This is not a claim of a formal
clean-room process: the old renderer was inspected to identify its interface
and supported character set before replacement.

## Character coverage

- 80 solid light/heavy box characters: U+2500–U+2503, U+250C–U+254B,
  and U+2574–U+257F. This includes all 42 previously native box characters,
  plus 38 additional mixed-weight junctions.
- All 32 block elements U+2580–U+259F, including eighths, shades and quadrants.
- Dashed, double-line, rounded and diagonal boxes continue through font shaping.
  Combining sequences also continue through the existing font path.

The directional data is generated from Unicode 17.0.0 character names with
`node scripts/generate-terminal-glyph-data.mjs`. The command writes Rust source
to stdout only. It does not read another renderer or extract font outlines.
Unicode data keeps its own notice in `licenses/UNICODE.txt`.

## Geometry and cost

At compile time, occupied tiles are merged into disjoint rectangles and stored
in a static lookup table. No geometry construction or heap allocation occurs
during painting. Cell-relative coordinates are mapped to absolute physical
pixel boundaries using the window scale factor. Adjacent rectangles share the
same boundaries, including block fractions, and opacity is applied once per
covered area. Light and heavy strokes share a center and have a 1:2 nominal
thickness ratio, clamped for very small cells.

This bounds the rendering work; it is not a measured performance comparison.
Visual parity and performance must be checked in the application before claiming
pixel-identical output or a measured speedup. Build success alone does not
establish either result.

## Manual acceptance (not run automatically)

Check light/heavy corners, T junctions, crosses and half-lines; adjacent upper
and lower blocks; all quadrant combinations; and shade opacity over both light
and dark backgrounds. Repeat at normal and small font sizes on 1x and Retina
displays, with fractional cell positions after resizing. Confirm there are no
seams, clipped corners or darker overlaps. Check selection and cursor colors,
and ensure double/rounded/diagonal lines still use font shaping.

Regression tests in `native_glyph.rs` cover coverage, geometric partitioning,
block areas and physical-pixel alignment. They are opt-in and are not executed
by the release build.
