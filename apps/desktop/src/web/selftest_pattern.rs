//! The picture the web self-tests draw in place of a page: `selftest.rs` on macOS, `d3d11_selftest.rs` on Windows.
//! One pattern on both, so one script judges a screenshot of either: `web-tests/selftest/check.py`.
//!
//! - Top half: colour bars [`BAR`] device pixels wide, moving left. Tearing or a stutter shows here.
//! - Next quarter, the check band: black and white squares, 1 device pixel on the left half and 2 on the right.
//! - Bottom quarter: [`SWATCHES`], fixed, to read colours from a screenshot.
//!
//! What a tester sees: on a high-density screen (Retina, 150% and up), 1 pixel squares are too small for the eye. A
//! frame drawn pixel for pixel makes the left half an even, flat grey. A scaled frame, or one between pixels, makes
//! wavy bands, stripes or blotches there. The right half shows a fine checker either way. The eye cannot tell "sharp
//! grey" from "blurred grey", so the script reads a full-resolution screenshot and says PASS or FAIL.

/// The width of a moving bar, in device pixels.
pub(super) const BAR: usize = 64;
/// Colours as red, green, blue: the moving bars, then the fixed swatches at the bottom. `check.py` has the swatches.
const BARS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 255],
    [0, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [0, 0, 255],
    [0, 0, 0],
];
pub(super) const SWATCHES: [[u8; 3]; 8] = [
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [128, 128, 128],
    [0x33, 0x66, 0x99],
    [0xF0, 0x80, 0x20],
    [255, 255, 255],
    [0, 0, 0],
];

/// The pattern into `bytes`: BGRA rows `stride` bytes apart, `width` by `height` pixels, the bars moved `offset`
/// pixels left.
pub(super) fn fill(bytes: &mut [u8], stride: usize, width: usize, height: usize, offset: usize) {
    let mut row = vec![0; width * 4];
    // A row is made again only when it differs from the one above: in the check band, every row.
    let mut made = None;
    for y in 0..height {
        let band = (y * 4 / height).clamp(1, 3);
        let key = (band, if band == 2 { y % 4 } else { 0 });
        if made != Some(key) {
            made = Some(key);
            for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                let [red, green, blue] = match band {
                    1 => BARS[(x + offset) / BAR % BARS.len()],
                    2 => {
                        // 1 pixel squares on the left half, 2 on the right.
                        let size = if x < width / 2 { 1 } else { 2 };
                        if (x / size + y / size) % 2 == 0 {
                            [0; 3]
                        } else {
                            [255; 3]
                        }
                    }
                    _ => SWATCHES[(x * SWATCHES.len() / width).min(SWATCHES.len() - 1)],
                };
                pixel.copy_from_slice(&[blue, green, red, 255]);
            }
        }
        bytes[y * stride..y * stride + width * 4].copy_from_slice(&row);
    }
}

#[cfg(test)]
mod tests {
    use super::{BAR, SWATCHES, fill};

    const W: usize = 256;
    const H: usize = 64;

    fn pixel(bytes: &[u8], x: usize, y: usize) -> [u8; 3] {
        let at = (y * W + x) * 4;
        [bytes[at + 2], bytes[at + 1], bytes[at]]
    }

    #[test]
    fn checks_alternate_on_both_axes() {
        let mut bytes = vec![0; W * H * 4];
        fill(&mut bytes, W * 4, W, H, 0);
        let band = H / 2..H * 3 / 4;
        for y in band.clone() {
            for x in 0..W / 2 - 1 {
                // 1 pixel squares: each pixel differs from its right and lower neighbours.
                assert_ne!(pixel(&bytes, x, y), pixel(&bytes, x + 1, y), "at {x},{y}");
                if band.contains(&(y + 1)) {
                    assert_ne!(pixel(&bytes, x, y), pixel(&bytes, x, y + 1), "at {x},{y}");
                }
            }
            for x in W / 2..W - 2 {
                // 2 pixel squares: differs two pixels on, matches four on.
                assert_ne!(pixel(&bytes, x, y), pixel(&bytes, x + 2, y), "at {x},{y}");
                if x + 4 < W {
                    assert_eq!(pixel(&bytes, x, y), pixel(&bytes, x + 4, y), "at {x},{y}");
                }
                // Vertically too: differs two rows down, matches four down (the `y % 4` row key in `fill`).
                if band.contains(&(y + 2)) {
                    assert_ne!(pixel(&bytes, x, y), pixel(&bytes, x, y + 2), "at {x},{y}");
                }
                if band.contains(&(y + 4)) {
                    assert_eq!(pixel(&bytes, x, y), pixel(&bytes, x, y + 4), "at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn bars_move_left_and_swatches_stay() {
        let mut still = vec![0; W * H * 4];
        let mut moved = vec![0; W * H * 4];
        fill(&mut still, W * 4, W, H, 0);
        fill(&mut moved, W * 4, W, H, 1);
        // The first bar edge, one pixel further left.
        assert_ne!(pixel(&still, BAR - 1, 0), pixel(&still, BAR, 0));
        assert_eq!(pixel(&moved, BAR - 1, 0), pixel(&still, BAR, 0));
        let bottom = H - 1;
        assert_eq!(pixel(&moved, 0, bottom), SWATCHES[0]);
        assert_eq!(pixel(&moved, W - 1, bottom), SWATCHES[7]);
        assert_eq!(&still[bottom * W * 4..], &moved[bottom * W * 4..]);
    }

    #[test]
    fn rows_past_the_width_are_left_alone() {
        let stride = W * 4 + 16;
        let mut bytes = vec![7; stride * H];
        fill(&mut bytes, stride, W, H, 0);
        assert!(bytes[W * 4..stride].iter().all(|&byte| byte == 7));
    }
}
