//! Images for a recording: RGB pixels, a box-filter resize, the contact sheet (every keyframe on one image, each
//! labelled with its number and time, so an agent sees the whole recording in one look) and PNG encoding. Pure Rust,
//! so it is tested here; decoding frames from the video is src/media.rs.

/// 8-bit RGB pixels, row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// The sheet's background (Nexus UI's metal white) and the labels' colours.
const BACKGROUND: [u8; 3] = [244, 244, 245];
const LABEL: [u8; 3] = [24, 24, 27];
const INK: [u8; 3] = [255, 255, 255];
const PAD: usize = 16;
/// About as wide as an agent's image input keeps (larger images are scaled down before a model sees them).
const SHEET_WIDTH: usize = 1984;

impl Rgb {
    pub fn new(width: usize, height: usize, color: [u8; 3]) -> Rgb {
        Rgb {
            width,
            height,
            pixels: color.repeat(width * height),
        }
    }

    /// Scaled to `width` × `height` by averaging the source pixels each one covers (good for shrinking).
    pub fn resize(&self, width: usize, height: usize) -> Rgb {
        let mut out = Rgb::new(width.max(1), height.max(1), [0, 0, 0]);
        if self.width == 0 || self.height == 0 {
            return out;
        }
        for y in 0..out.height {
            let y0 = y * self.height / out.height;
            let y1 = ((y + 1) * self.height).div_ceil(out.height).clamp(y0 + 1, self.height);
            for x in 0..out.width {
                let x0 = x * self.width / out.width;
                let x1 = ((x + 1) * self.width).div_ceil(out.width).clamp(x0 + 1, self.width);
                let mut sum = [0u64; 3];
                for sy in y0..y1 {
                    let row = sy * self.width * 3;
                    for sx in x0..x1 {
                        let at = row + sx * 3;
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += u64::from(self.pixels[at + channel]);
                        }
                    }
                }
                let count = ((y1 - y0) * (x1 - x0)) as u64;
                let at = (y * out.width + x) * 3;
                for (channel, total) in sum.iter().enumerate() {
                    out.pixels[at + channel] = (total / count) as u8;
                }
            }
        }
        out
    }

    pub fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 3]) {
        for row in y..(y + height).min(self.height) {
            for column in x..(x + width).min(self.width) {
                let at = (row * self.width + column) * 3;
                self.pixels[at..at + 3].copy_from_slice(&color);
            }
        }
    }

    /// Copy `image` in with its top-left corner at (`x`, `y`), cut at the edges.
    pub fn blit(&mut self, image: &Rgb, x: usize, y: usize) {
        let columns = image.width.min(self.width.saturating_sub(x));
        for row in 0..image.height.min(self.height.saturating_sub(y)) {
            let from = row * image.width * 3;
            let to = ((y + row) * self.width + x) * 3;
            self.pixels[to..to + columns * 3].copy_from_slice(&image.pixels[from..from + columns * 3]);
        }
    }

    /// Text in the label font (digits, `:` and spaces), `scale` pixels per font pixel; returns the width drawn.
    pub fn text(&mut self, text: &str, x: usize, y: usize, scale: usize, color: [u8; 3]) -> usize {
        let mut pen = x;
        for glyph in text.chars().map(glyph) {
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..GLYPH_WIDTH {
                    if bits & (1 << (GLYPH_WIDTH - 1 - column)) != 0 {
                        self.fill(pen + column * scale, y + row * scale, scale, scale, color);
                    }
                }
            }
            pen += (GLYPH_WIDTH + 1) * scale;
        }
        pen - x
    }

    pub fn png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
        Ok(out)
    }
}

const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;

/// A 5 × 7 font for the labels: the digits, `:` and a space.
fn glyph(c: char) -> [u8; GLYPH_HEIGHT] {
    match c {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        _ => [0; GLYPH_HEIGHT],
    }
}

/// The contact sheet: `frames` (each with its label, e.g. `01  0:04`) in a grid, three to a row (two for four or
/// fewer, one alone), on Nexus UI's metal white, each with its label in a dark tab at its top-left corner.
pub fn contact_sheet(frames: &[(&Rgb, String)]) -> Rgb {
    let columns = match frames.len() {
        0 | 1 => 1,
        2..=4 => 2,
        _ => 3,
    };
    // Never larger than the frames themselves.
    let widest = frames.iter().map(|(frame, _)| frame.width).max().unwrap_or(1).max(1);
    let cell_width = ((SHEET_WIDTH - PAD * (columns + 1)) / columns).min(widest);
    let cell_height = frames
        .iter()
        .map(|(frame, _)| frame.height * cell_width / frame.width.max(1))
        .max()
        .unwrap_or(0);
    let rows = frames.len().div_ceil(columns).max(1);
    let width = PAD + columns * (cell_width + PAD);
    let mut sheet = Rgb::new(width, PAD + rows * (cell_height + PAD), BACKGROUND);
    let scale = 3;
    for (index, (frame, label)) in frames.iter().enumerate() {
        let x = PAD + (index % columns) * (cell_width + PAD);
        let y = PAD + (index / columns) * (cell_height + PAD);
        let height = frame.height * cell_width / frame.width.max(1);
        sheet.blit(&frame.resize(cell_width, height), x, y);
        let text_width = label.chars().count() * (GLYPH_WIDTH + 1) * scale - scale;
        let tab = 8;
        sheet.fill(x, y, text_width + tab * 2, GLYPH_HEIGHT * scale + tab * 2, LABEL);
        sheet.text(label, x + tab, y + tab, scale, INK);
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_averages_the_pixels_it_covers() {
        let mut image = Rgb::new(4, 2, [0, 0, 0]);
        image.fill(0, 0, 2, 2, [200, 100, 0]);
        let small = image.resize(2, 1);
        assert_eq!(small.pixels, vec![200, 100, 0, 0, 0, 0]);
        let half = Rgb::new(3, 3, [90, 90, 90]).resize(2, 2);
        assert_eq!(half.pixels, vec![90; 12]);
    }

    #[test]
    fn the_sheet_lays_frames_out_in_a_grid_with_labels() {
        let frame = Rgb::new(1600, 1000, [10, 120, 200]);
        let frames: Vec<(&Rgb, String)> = (1..=5).map(|n| (&frame, format!("0{n}  0:0{n}"))).collect();
        let sheet = contact_sheet(&frames);
        // Three to a row, two rows.
        let cell = (SHEET_WIDTH - PAD * 4) / 3;
        assert_eq!(sheet.width, PAD + 3 * (cell + PAD));
        assert_eq!(sheet.height, PAD + 2 * (cell * 1000 / 1600 + PAD));
        // A lone small frame stays its size.
        let small = Rgb::new(240, 160, [0, 0, 0]);
        assert_eq!(contact_sheet(&[(&small, "01".into())]).width, PAD * 2 + 240);
        let pixel =
            |x: usize, y: usize| sheet.pixels[(y * sheet.width + x) * 3..(y * sheet.width + x) * 3 + 3].to_vec();
        // The background, a frame's middle, its label tab.
        assert_eq!(pixel(2, 2), BACKGROUND.to_vec());
        assert_eq!(pixel(PAD + cell / 2, PAD + 200), vec![10, 120, 200]);
        assert_eq!(pixel(PAD + 2, PAD + 2), LABEL.to_vec());
        // The sixth cell is empty.
        assert_eq!(
            pixel(PAD + 2 * (cell + PAD) + cell / 2, sheet.height - PAD - 10),
            BACKGROUND.to_vec()
        );
    }

    #[test]
    fn png_round_trips_through_a_decoder() {
        let mut image = Rgb::new(7, 3, [1, 2, 3]);
        image.text("0:1", 0, 0, 1, [255, 255, 255]);
        let bytes = image.png().expect("png");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("info");
        let mut buffer = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buffer).expect("frame");
        assert_eq!((info.width, info.height), (7, 3));
        assert_eq!(&buffer[..info.buffer_size()], image.pixels.as_slice());
    }
}
