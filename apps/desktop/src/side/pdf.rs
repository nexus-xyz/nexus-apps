//! The Files tool's PDF preview, as the Nexus app's (`pdf-preview.tsx`): one page at a time,
//! fitted to the pane's width, with Previous, "Page N of M" and Next. The Nexus app draws with PDF.js; this build
//! draws with hayro, a PDF rasterizer in Rust on the CPU. Neither runs a PDF's scripts. A page draws off the UI
//! thread, at the scale `nexus_workbench::output_files::pdf_scale` gives. The page's text goes to screen readers, as
//! PDF.js's `getTextContent` does there: each text run, joined by spaces.

use std::io::Cursor;
use std::sync::Arc;

use gpui_kit::{Image, ImageFormat};
use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::kurbo::{Affine, BezPath, Rect};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image as PdfImage, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use nexus_workbench::output_files::pdf_scale;

/// A page drawn: the image, its size in logical pixels, the document's page count, and the page's text.
pub struct Drawn {
    pub image: Arc<Image>,
    pub size: (f32, f32),
    pub pages: usize,
    pub text: String,
}

/// Draw page `page` (from 1) of the PDF in `bytes`, `width` logical pixels across on a screen of `ratio` (slow: run
/// it off the UI thread). The error is the reason, for "This PDF could not be displayed."
pub fn draw(bytes: Arc<Vec<u8>>, page: usize, width: f32, ratio: f32) -> Result<Drawn, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw_inner(bytes, page, width, ratio)))
        .unwrap_or_else(|_| Err("It could not be read.".into()))
}

fn draw_inner(bytes: Arc<Vec<u8>>, page: usize, width: f32, ratio: f32) -> Result<Drawn, String> {
    let pdf = Pdf::new(bytes).map_err(|error| match error {
        hayro::hayro_syntax::LoadPdfError::Decryption(_) => "It is encrypted.".to_string(),
        hayro::hayro_syntax::LoadPdfError::Invalid => "It is not a valid PDF.".to_string(),
    })?;
    let pages = pdf.pages();
    let count = pages.len();
    let sheet = pages
        .get(page.saturating_sub(1))
        .ok_or_else(|| format!("It has no page {page}."))?;
    let (base_width, base_height) = sheet.render_dimensions();
    let ratio = ratio.clamp(1., 2.);
    let scale = pdf_scale(base_width, base_height, width, ratio);
    let pixels = scale * ratio;
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        sheet,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: pixels,
            y_scale: pixels,
            bg_color: WHITE,
        },
    );
    let (wide, high) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    // Opaque on white, so its premultiplied pixels are its plain ones.
    let rgba =
        ::image::RgbaImage::from_raw(wide, high, pixmap.data_as_u8_slice().to_vec()).ok_or("The page did not draw.")?;
    let mut png = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut png), ::image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(Drawn {
        image: Arc::new(Image::from_bytes(ImageFormat::Png, png)),
        size: (base_width * scale, base_height * scale),
        pages: count,
        text: text_of(sheet),
    })
}

/// The page's text for a screen reader: its text runs (visible or not), joined by spaces.
fn text_of(sheet: &hayro::hayro_syntax::page::Page<'_>) -> String {
    let cache = InterpreterCache::new();
    let mut context = Context::new(
        Affine::IDENTITY,
        Rect::new(f64::MIN, f64::MIN, f64::MAX, f64::MAX),
        &cache,
        sheet.xref(),
        InterpreterSettings::default(),
    );
    let mut text = TextRuns::default();
    interpret_page(sheet, &mut context, &mut text);
    text.runs.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A hayro device that keeps only each glyph run's Unicode text.
#[derive(Default)]
struct TextRuns {
    runs: Vec<String>,
    /// The last run was a fill: text both filled and stroked comes as a fill, then the same run as a stroke.
    filled: bool,
}

impl<'a> Device<'a> for TextRuns {
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, _: DrawProps<'a>, mode: &DrawMode) {
        let mut text = String::new();
        for glyph in run.glyphs() {
            match glyph.as_unicode() {
                Some(BfString::Char(char)) => text.push(char),
                Some(BfString::String(string)) => text.push_str(&string),
                None => {}
            }
        }
        let stroke = matches!(mode, DrawMode::Stroke(_));
        let again = stroke && self.filled && self.runs.last() == Some(&text);
        self.filled = matches!(mode, DrawMode::Fill(_));
        if !again {
            self.runs.push(text);
        }
    }
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: PdfImage<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
pub(crate) mod tests {
    use core::prelude::v1::test;

    use super::*;

    /// A PDF of `pages` Letter pages, each a black square, with a correct cross-reference table.
    pub(crate) fn pdf(pages: usize) -> Vec<u8> {
        pdf_of(pages, "0 0 0 rg 100 100 200 200 re f")
    }

    /// A PDF of `pages` Letter pages, each drawn by `stream`, with Helvetica as `/F1`.
    fn pdf_of(pages: usize, stream: &str) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {pages} >>",
                (0..pages)
                    .map(|n| format!("{} 0 R", 3 + 2 * n))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ];
        for n in 0..pages {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>",
                4 + 2 * n
            ));
            objects.push(format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (n, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", n + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn a_page_draws_fitted_to_the_width_and_counts_the_pages() {
        let drawn = draw(Arc::new(pdf(3)), 2, 306., 2.).expect("the page draws");
        assert_eq!(drawn.pages, 3);
        assert!((drawn.size.0 - 306.).abs() < 0.01 && (drawn.size.1 - 396.).abs() < 0.01);
        let png = ::image::load_from_memory(&drawn.image.bytes).expect("a PNG");
        // Twice the logical size on a 2x screen.
        assert_eq!((png.width(), png.height()), (612, 792));
        let png = png.to_rgba8();
        // The square is black and the rest white (PDF's origin is at the bottom).
        assert_eq!(png.get_pixel(200, 792 - 200).0, [0, 0, 0, 255]);
        assert_eq!(png.get_pixel(10, 10).0, [255, 255, 255, 255]);
    }

    #[test]
    fn a_page_gives_its_text_runs_joined_by_spaces_and_a_page_without_text_none() {
        let stream = "BT /F1 18 Tf 72 700 Td (Quarterly report) Tj 0 -24 Td [(Rev) 120 (enue grew)] TJ ET \
                      BT /F1 12 Tf 3 Tr 72 600 Td (Hidden note) Tj ET \
                      BT /F1 12 Tf 2 Tr 72 500 Td (Outlined) Tj ET";
        let drawn = draw(Arc::new(pdf_of(1, stream)), 1, 306., 1.).expect("the page draws");
        assert_eq!(drawn.text, "Quarterly report Revenue grew Hidden note Outlined");
        assert_eq!(draw(Arc::new(pdf(1)), 1, 306., 1.).expect("the page draws").text, "");
    }

    #[test]
    fn a_bad_file_or_a_missing_page_says_why() {
        assert_eq!(
            draw(Arc::new(b"not a pdf".to_vec()), 1, 300., 1.).err().as_deref(),
            Some("It is not a valid PDF.")
        );
        assert_eq!(
            draw(Arc::new(pdf(1)), 2, 300., 1.).err().as_deref(),
            Some("It has no page 2.")
        );
    }
}
