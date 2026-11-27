//! A `<select>`'s open list, and Chromium's other popup widgets (a date picker, a color picker). Off screen, Chromium
//! paints a popup apart from the page (`PaintElementType::POPUP`), at the rect `on_popup_size` names. A browser window
//! shows it over the page; here the host draws it into the page's frame before the frame goes to the app. Clicks need
//! nothing: CEF sends a click inside the popup's rect to the popup.
//!
//! CPU frames only. A page with GPU frames (Linux, `frames/linux.rs`) still shows no popup.

/// A BGRA image: its pixels, width and height in device pixels.
#[derive(Default)]
struct Image {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

impl Image {
    fn keep(&mut self, pixels: &[u8], width: u32, height: u32) {
        let len = width as usize * height as usize * 4;
        self.pixels.clear();
        self.pixels.extend_from_slice(&pixels[..len.min(pixels.len())]);
        self.width = width;
        self.height = height;
    }

    fn whole(&self) -> bool {
        self.width > 0 && self.height > 0 && self.pixels.len() == self.width as usize * self.height as usize * 4
    }
}

/// A page's popup, while one is open.
#[derive(Default)]
pub(super) struct Popup {
    shown: bool,
    /// Where the popup is, in the view's logical pixels: x, y, width, height.
    rect: Option<[i32; 4]>,
    /// Its last paint.
    popup: Image,
    /// The page's last frame while the popup shows, to draw the popup over again when either changes.
    page: Image,
}

/// A frame to send: the page with the popup over it, and the rects (device pixels) that changed.
pub(super) struct Composed {
    pub(super) pixels: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) dirty: Vec<[i32; 4]>,
}

impl Popup {
    pub(super) fn shown(&self) -> bool {
        self.shown
    }

    /// The popup opened. The host then asks Chromium for the whole page again: the page under it must be kept.
    pub(super) fn show(&mut self) {
        *self = Popup {
            shown: true,
            ..Popup::default()
        };
    }

    /// The popup closed: the page as it was under it, to send again where the popup was. None when nothing was drawn.
    pub(super) fn hide(&mut self, scale: f32) -> Option<Composed> {
        let restored = self
            .popup_rect(scale)
            .filter(|_| self.page.whole())
            .map(|rect| Composed {
                pixels: std::mem::take(&mut self.page.pixels),
                width: self.page.width,
                height: self.page.height,
                dirty: vec![rect],
            });
        *self = Popup::default();
        restored
    }

    /// The popup moved or changed size: the frame with it in its new place, which repaints its old one too.
    pub(super) fn place(&mut self, rect: [i32; 4], scale: f32) -> Option<Composed> {
        let old = self.popup_rect(scale);
        self.rect = Some(rect);
        let mut composed = self.compose(scale)?;
        composed.dirty.extend(old);
        Some(composed)
    }

    /// Chromium painted the popup.
    pub(super) fn paint_popup(&mut self, pixels: &[u8], width: u32, height: u32, scale: f32) -> Option<Composed> {
        self.popup.keep(pixels, width, height);
        self.compose(scale)
    }

    /// Chromium painted the page, of which `dirty` changed: the frame with the popup over it.
    pub(super) fn paint_page(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        dirty: &[[i32; 4]],
        scale: f32,
    ) -> Option<Composed> {
        self.page.keep(pixels, width, height);
        let mut composed = self.compose(scale)?;
        if dirty.is_empty() {
            composed.dirty.clear();
        } else {
            composed.dirty.extend_from_slice(dirty);
        }
        Some(composed)
    }

    /// The popup's rect in the frame, device pixels: where `on_popup_size` put it, as large as its paint.
    fn popup_rect(&self, scale: f32) -> Option<[i32; 4]> {
        let [x, y, ..] = self.rect?;
        self.popup.whole().then(|| {
            [
                (x as f32 * scale).round() as i32,
                (y as f32 * scale).round() as i32,
                self.popup.width as i32,
                self.popup.height as i32,
            ]
        })
    }

    /// The kept page with the popup over it, once both are painted.
    fn compose(&self, scale: f32) -> Option<Composed> {
        let rect = self.popup_rect(scale)?;
        if !self.shown || !self.page.whole() {
            return None;
        }
        let mut pixels = self.page.pixels.clone();
        blit(
            &mut pixels,
            self.page.width,
            self.page.height,
            &self.popup,
            rect[0],
            rect[1],
        );
        Some(Composed {
            pixels,
            width: self.page.width,
            height: self.page.height,
            dirty: vec![rect],
        })
    }
}

/// Copy `image` into the `width` by `height` frame `to` at `x`, `y`, cut to the frame.
fn blit(to: &mut [u8], width: u32, height: u32, image: &Image, x: i32, y: i32) {
    let (width, height) = (width as i64, height as i64);
    let x0 = (x as i64).clamp(0, width);
    let x1 = (x as i64 + image.width as i64).clamp(0, width);
    if x1 <= x0 {
        return;
    }
    let from_x = (x0 - x as i64) as usize;
    let len = (x1 - x0) as usize * 4;
    for row in 0..image.height as i64 {
        let frame_row = y as i64 + row;
        if !(0..height).contains(&frame_row) {
            continue;
        }
        let at = (frame_row * width + x0) as usize * 4;
        let from = (row as usize * image.width as usize + from_x) * 4;
        to[at..at + len].copy_from_slice(&image.pixels[from..from + len]);
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn image(width: u32, height: u32, value: u8) -> Vec<u8> {
        vec![value; width as usize * height as usize * 4]
    }

    fn pixel(frame: &Composed, x: u32, y: u32) -> u8 {
        frame.pixels[((y * frame.width + x) * 4) as usize]
    }

    #[test]
    fn the_list_draws_over_the_page_once_both_painted() {
        let mut popup = Popup::default();
        popup.show();
        // At 2x: the list is at (1, 1) logical, (2, 2) in the frame.
        assert!(popup.place([1, 1, 2, 1], 2.).is_none());
        assert!(popup.paint_popup(&image(4, 2, 9), 4, 2, 2.).is_none(), "no page yet");
        let frame = popup.paint_page(&image(8, 6, 1), 8, 6, &[[0, 0, 1, 1]], 2.).unwrap();
        assert_eq!(
            (
                pixel(&frame, 2, 2),
                pixel(&frame, 5, 3),
                pixel(&frame, 6, 3),
                pixel(&frame, 1, 2)
            ),
            (9, 9, 1, 1)
        );
        assert_eq!(frame.dirty, vec![[2, 2, 4, 2], [0, 0, 1, 1]]);
        // A new paint of the list: only its rect.
        let frame = popup.paint_popup(&image(4, 2, 7), 4, 2, 2.).unwrap();
        assert_eq!((pixel(&frame, 3, 3), frame.dirty.clone()), (7, vec![[2, 2, 4, 2]]));
    }

    #[test]
    fn a_whole_page_paint_stays_whole() {
        let mut popup = Popup::default();
        popup.show();
        popup.place([0, 0, 1, 1], 1.);
        popup.paint_popup(&image(1, 1, 9), 1, 1, 1.);
        let frame = popup.paint_page(&image(3, 3, 1), 3, 3, &[], 1.).unwrap();
        assert!(frame.dirty.is_empty());
    }

    #[test]
    fn closing_the_list_puts_the_page_back_where_it_was() {
        let mut popup = Popup::default();
        assert!(popup.hide(1.).is_none(), "nothing open");
        popup.show();
        popup.place([1, 0, 2, 2], 1.);
        popup.paint_popup(&image(2, 2, 9), 2, 2, 1.);
        popup.paint_page(&image(4, 4, 1), 4, 4, &[], 1.);
        let page = popup.hide(1.).unwrap();
        assert_eq!(page.dirty, vec![[1, 0, 2, 2]]);
        assert!(page.pixels.iter().all(|&value| value == 1));
        assert!(!popup.shown());
    }

    #[test]
    fn a_moved_list_repaints_its_old_place_too() {
        let mut popup = Popup::default();
        popup.show();
        popup.place([0, 0, 1, 1], 1.);
        popup.paint_popup(&image(1, 1, 9), 1, 1, 1.);
        popup.paint_page(&image(4, 4, 1), 4, 4, &[], 1.);
        let frame = popup.place([2, 2, 1, 1], 1.).unwrap();
        assert_eq!(frame.dirty, vec![[2, 2, 1, 1], [0, 0, 1, 1]]);
        assert_eq!((pixel(&frame, 0, 0), pixel(&frame, 2, 2)), (1, 9));
    }

    #[test]
    fn a_list_past_the_frame_is_cut() {
        let mut popup = Popup::default();
        popup.show();
        popup.place([-1, 2, 3, 3], 1.);
        popup.paint_popup(&image(3, 3, 9), 3, 3, 1.);
        let frame = popup.paint_page(&image(3, 3, 1), 3, 3, &[], 1.).unwrap();
        // Columns -1 to 1 of the frame: 0 and 1 show it. Rows 2 to 4: only 2.
        assert_eq!(
            (
                pixel(&frame, 0, 2),
                pixel(&frame, 1, 2),
                pixel(&frame, 2, 2),
                pixel(&frame, 0, 1)
            ),
            (9, 9, 1, 1)
        );
    }
}
