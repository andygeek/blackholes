//! Blackholes terminal geometry built from Unicode character semantics.
//!
//! A compile-time tile union produces disjoint rectangles. Painting only maps
//! their coordinates to a cell: no heap allocation, path tessellation, or
//! overlapping alpha coverage. See docs/TERMINAL-GLYPHS.md for provenance.

use super::unicode_strokes::SOLID_STROKES;

#[derive(Clone, Copy)]
struct Tile {
    left: u8,
    top: u8,
    right: u8,
    bottom: u8,
}

#[derive(Clone, Copy)]
struct Mesh {
    tiles: [Tile; 16],
    count: usize,
}

/// Merge identical runs on consecutive rows. All operations happen at compile
/// time; the stored rectangles partition the occupied area without overlaps.
const fn tessellate(rows: [u8; 8]) -> Mesh {
    let mut mesh = Mesh {
        tiles: [Tile {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }; 16],
        count: 0,
    };
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            if rows[y] & (1 << x) == 0 {
                x += 1;
                continue;
            }
            let left = x;
            while x < 8 && rows[y] & (1 << x) != 0 {
                x += 1;
            }
            let mut i = 0;
            while i < mesh.count {
                let tile = mesh.tiles[i];
                if tile.left == left && tile.right == x && tile.bottom == y as u8 {
                    break;
                }
                i += 1;
            }
            if i == mesh.count {
                // A malformed future definition must fail compilation rather
                // than silently discard geometry.
                assert!(mesh.count < mesh.tiles.len());
                mesh.tiles[i] = Tile {
                    left,
                    right: x,
                    top: y as u8,
                    bottom: y as u8,
                };
                mesh.count += 1;
            }
            mesh.tiles[i].bottom += 1;
        }
        y += 1;
    }
    mesh
}

const fn maximum(a: u8, b: u8) -> u8 {
    if a > b { a } else { b }
}

/// Six coordinate intervals: outside, heavy-only, light, light,
/// heavy-only, outside. The center is a boundary so half-strokes end exactly
/// there. Perpendicular strokes extend into one another before taking a union.
const fn stroke_rows(packed: u8) -> [u8; 8] {
    let left = packed & 3;
    let right = (packed >> 2) & 3;
    let up = (packed >> 4) & 3;
    let down = (packed >> 6) & 3;
    let horizontal = maximum(left, right);
    let vertical = maximum(up, down);
    let mut rows = [0; 8];
    let mut y = 0u8;
    while y < 6 {
        let mut x = 0u8;
        while x < 6 {
            let occupied = (left > 0 && x < 3 + vertical && y >= 3 - left && y < 3 + left)
                || (right > 0 && x >= 3 - vertical && y >= 3 - right && y < 3 + right)
                || (up > 0 && y < 3 + horizontal && x >= 3 - up && x < 3 + up)
                || (down > 0 && y >= 3 - horizontal && x >= 3 - down && x < 3 + down);
            if occupied {
                rows[y as usize] |= 1 << x;
            }
            x += 1;
        }
        y += 1;
    }
    rows
}

/// U+2580..U+259F: eight equal subdivisions per axis. Quadrant bits are
/// top-left, top-right, bottom-left, bottom-right in that order.
const fn block_rows(index: usize) -> [u8; 8] {
    let mut rows = [0; 8];
    let quadrants = match index {
        22 => 0b0100,
        23 => 0b1000,
        24 => 0b0001,
        25 => 0b1101,
        26 => 0b1001,
        27 => 0b0111,
        28 => 0b1011,
        29 => 0b0010,
        30 => 0b0110,
        31 => 0b1110,
        _ => 0,
    };
    let mut y = 0;
    while y < 8 {
        rows[y] = match index {
            0 => {
                if y < 4 {
                    255
                } else {
                    0
                }
            }
            1..=8 => {
                if y >= 8 - index {
                    255
                } else {
                    0
                }
            }
            9..=15 => ((1u16 << (16 - index)) - 1) as u8,
            16 => 0xf0,
            17..=19 => 255, // Shade opacity is applied once to the full cell.
            20 => {
                if y == 0 {
                    255
                } else {
                    0
                }
            }
            21 => 0x80,
            _ => {
                let half = if y < 4 { quadrants & 3 } else { quadrants >> 2 };
                (if half & 1 != 0 { 0x0f } else { 0 }) | (if half & 2 != 0 { 0xf0 } else { 0 })
            }
        };
        y += 1;
    }
    rows
}

static MESHES: [Mesh; 160] = {
    let mut meshes = [tessellate([0; 8]); 160];
    let mut i = 0;
    while i < 160 {
        meshes[i] = tessellate(if i < 128 {
            stroke_rows(SOLID_STROKES[i])
        } else {
            block_rows(i - 128)
        });
        i += 1;
    }
    meshes
};

pub(super) fn is_supported(character: char) -> bool {
    let code = character as usize;
    (0x2580..=0x259f).contains(&code)
        || ((0x2500..=0x257f).contains(&code) && SOLID_STROKES[code - 0x2500] != 0)
}

fn stroke_coordinates(length: f32, thin: f32, thick: f32) -> [f32; 9] {
    let center = length / 2.0;
    [
        0.0,
        center - thick / 2.0,
        center - thin / 2.0,
        center,
        center + thin / 2.0,
        center + thick / 2.0,
        length,
        length,
        length,
    ]
}

/// Rectangle endpoints are absolute logical coordinates, snapped to physical
/// pixels. Shared cell edges use the same expression, so fractional cell sizes
/// do not create seams at Retina or non-integer scaling factors.
pub(super) fn paint_rects(
    character: char,
    origin: [f32; 2],
    size: [f32; 2],
    scale: f32,
    mut paint: impl FnMut(f32, f32, f32, f32, f32),
) {
    let [width, height] = size;
    if !is_supported(character)
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !scale.is_finite()
        || scale <= 0.0
        || origin.iter().any(|value| !value.is_finite())
    {
        return;
    }
    let index = character as usize - 0x2500;
    let (mut xs, mut ys) = if index < 128 {
        // Approximately one physical pixel per 16 physical pixels of line
        // height. Clamp to the cell for very small terminal zoom levels.
        let thin = ((height * scale / 16.0).round().max(1.0) / scale).min(width.min(height));
        let thick = (thin * 2.0).min(width.min(height));
        (
            stroke_coordinates(width, thin, thick),
            stroke_coordinates(height, thin, thick),
        )
    } else {
        (
            std::array::from_fn(|i| width * (i as f32 / 8.0)),
            std::array::from_fn(|i| height * (i as f32 / 8.0)),
        )
    };
    for x in &mut xs {
        *x = ((origin[0] + *x) * scale).round() / scale;
    }
    for y in &mut ys {
        *y = ((origin[1] + *y) * scale).round() / scale;
    }
    let opacity = match character {
        '░' => 0.25,
        '▒' => 0.5,
        '▓' => 0.75,
        _ => 1.0,
    };
    let mesh = &MESHES[index];
    for tile in &mesh.tiles[..mesh.count] {
        let x = xs[tile.left as usize];
        let y = ys[tile.top as usize];
        let w = xs[tile.right as usize] - x;
        let h = ys[tile.bottom as usize] - y;
        if w > 0.0 && h > 0.0 {
            paint(x, y, w, h, opacity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangles(c: char, origin: [f32; 2], size: [f32; 2], scale: f32) -> Vec<[f32; 5]> {
        let mut output = Vec::new();
        paint_rects(c, origin, size, scale, |x, y, w, h, a| {
            output.push([x, y, w, h, a])
        });
        output
    }

    #[test]
    fn repertoire_includes_all_solid_boxes_and_blocks_but_preserves_font_fallback() {
        let count = (0x2500..=0x259f)
            .filter(|&code| is_supported(char::from_u32(code).unwrap()))
            .count();
        assert_eq!(count, 112);
        for c in "─━│┃┌┍┎┏┐┑┒┓└┕┖┗┘┙┚┛├┣┤┫┬┳┴┻┼╋╴╵╶╷╸╹╺╻╼╽╾╿".chars()
        {
            assert!(is_supported(c), "previously supported {c}");
        }
        for c in "┝┞┟┠┡┢┥┦┧┨┩┪┭┮┯┰┱┲┵┶┷┸┹┺┽┾┿╀╁╂╃╄╅╆╇╈╉╊".chars()
        {
            assert!(is_supported(c));
        }
        for c in "Aé┄┅═║╔╭╮╱╲╳".chars() {
            assert!(!is_supported(c));
        }
    }

    #[test]
    fn rectangles_stay_on_pixel_grid_without_overlap_at_fractional_cell_sizes() {
        for (size, origin, scale) in [
            ([8.0, 16.0], [0.0, 0.0], 1.0),
            ([9.35, 18.7], [17.3, 31.9], 2.0),
            ([7.1, 13.6], [0.3, 4.2], 1.25),
            ([0.4, 0.8], [0.25, 0.25], 2.0),
        ] {
            let snap = |v: f32| (v * scale).round() / scale;
            for code in 0x2500..=0x259f {
                let c = char::from_u32(code).unwrap();
                let rects = rectangles(c, origin, size, scale);
                for (i, &[x, y, w, h, a]) in rects.iter().enumerate() {
                    assert!(w > 0.0 && h > 0.0 && a > 0.0 && a <= 1.0);
                    assert!(x >= snap(origin[0]) - 0.001 && y >= snap(origin[1]) - 0.001);
                    assert!(x + w <= snap(origin[0] + size[0]) + 0.001);
                    assert!(y + h <= snap(origin[1] + size[1]) + 0.001);
                    for edge in [x, y, x + w, y + h] {
                        assert!((edge * scale - (edge * scale).round()).abs() < 0.001);
                    }
                    for &[xx, yy, ww, hh, _] in &rects[i + 1..] {
                        let overlap_x = (x + w).min(xx + ww) - x.max(xx);
                        let overlap_y = (y + h).min(yy + hh) - y.max(yy);
                        assert!(overlap_x < 0.001 || overlap_y < 0.001, "overlap in {c}");
                    }
                }
            }
        }
    }

    #[test]
    fn blocks_and_shades_cover_the_unicode_fraction_exactly() {
        for (c, fraction) in [
            ('█', 1.0),
            ('▀', 0.5),
            ('▄', 0.5),
            ('▌', 0.5),
            ('▐', 0.5),
            ('▁', 0.125),
            ('▔', 0.125),
            ('▏', 0.125),
            ('▕', 0.125),
            ('▖', 0.25),
            ('▗', 0.25),
            ('▘', 0.25),
            ('▝', 0.25),
            ('▚', 0.5),
            ('▞', 0.5),
            ('▙', 0.75),
            ('▛', 0.75),
            ('▜', 0.75),
            ('▟', 0.75),
            ('░', 0.25),
            ('▒', 0.5),
            ('▓', 0.75),
        ] {
            let area: f32 = rectangles(c, [0.0, 0.0], [16.0, 32.0], 2.0)
                .iter()
                .map(|r| r[2] * r[3] * r[4])
                .sum();
            assert_eq!(area, 512.0 * fraction, "incorrect area for {c}");
        }
    }

    #[test]
    fn invalid_geometry_does_not_emit_gpu_primitives() {
        for size in [
            [0.0, 16.0],
            [-1.0, 16.0],
            [8.0, f32::NAN],
            [f32::INFINITY, 16.0],
        ] {
            assert!(rectangles('█', [0.0, 0.0], size, 2.0).is_empty());
        }
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(rectangles('█', [0.0, 0.0], [8.0, 16.0], scale).is_empty());
        }
    }
}
