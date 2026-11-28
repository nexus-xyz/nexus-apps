//! Full-size images, as the Nexus app shows them (`Viewer` in `ui/src/components/nexus-ui/chat/images.tsx`): click an
//! image chip in the composer, or an image sent in the chat, and it opens over the window as large as fits. A bar under
//! it gives its name, "2 of 3", its size in pixels and bytes, and Previous, Next, Download and Close. ← → step through
//! the images it opened among; Esc, Close or a click outside the image closes it.
//!
//! Download opens the system's save dialog in the Downloads folder with the image's name, as the app's other downloads
//! do, and writes the image where the person picks. A failed write is logged.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::attachments::{self, Attachment};
use crate::motion;
use crate::palette::Palette;

/// The share of the window the image may take, across and down; the bar sits under it.
const ACROSS: f32 = 0.9;
const DOWN: f32 = 0.8;
/// The bar under the image.
const BAR: f32 = 44.;

/// One image, ready to draw full size.
struct Shown {
    /// What Download saves: the image's own bytes and name.
    attachment: Attachment,
    name: String,
    detail: String,
    image: Arc<gpui_kit::Image>,
    /// Its size in pixels, when it could be read.
    pixels: Option<(u32, u32)>,
}

/// The open viewer: the images it opened among, and which shows.
pub(super) struct ImageViewer {
    images: Vec<Shown>,
    index: usize,
}

#[cfg(test)]
impl ImageViewer {
    /// A viewer with no images, for the overlay tests.
    pub(super) fn sample() -> Self {
        ImageViewer {
            images: Vec::new(),
            index: 0,
        }
    }
}

/// The index `by` steps from `index` among `len` images, round the ends (`step` in `images.tsx`).
fn step(index: usize, by: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (index as isize + by).rem_euclid(len as isize) as usize
}

/// `width` × `height` drawn as large as fits in `room`, never larger than it is.
fn fit(width: f32, height: f32, room: (f32, f32)) -> (f32, f32) {
    if width <= 0. || height <= 0. {
        return room;
    }
    let scale = (room.0 / width).min(room.1 / height).min(1.);
    (width * scale, height * scale)
}

impl Workspace {
    /// Open image `index` of `images` full size. Files that are not images are left out.
    pub(super) fn open_image(&mut self, images: &[Attachment], index: usize, cx: &mut Context<Self>) {
        let mut at = 0;
        let mut shown = Vec::new();
        for (position, attachment) in images.iter().enumerate() {
            let Some(full) = attachments::full_size(attachment) else {
                continue;
            };
            if position == index {
                at = shown.len();
            }
            shown.push(Shown {
                attachment: attachment.clone(),
                name: attachment.name.clone(),
                detail: attachments::image_detail(full.pixels, attachment.size()),
                image: full.image,
                pixels: full.pixels,
            });
        }
        if shown.is_empty() {
            return;
        }
        self.open_overlay(Overlay::ImageViewer(ImageViewer {
            images: shown,
            index: at,
        }));
        cx.notify();
    }

    pub(super) fn close_image(&mut self, cx: &mut Context<Self>) {
        self.overlay.close_image_viewer();
        cx.notify();
    }

    fn step_image(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(viewer) = self.overlay.image_viewer_mut() {
            viewer.index = step(viewer.index, by, viewer.images.len());
            cx.notify();
        }
    }

    /// Ask where to save the image shown, with the system's save dialog opened in Downloads, then write it there.
    fn download_image(&self, cx: &mut Context<Self>) {
        let Some(shown) = self
            .overlay
            .image_viewer()
            .as_ref()
            .and_then(|viewer| viewer.images.get(viewer.index))
        else {
            return;
        };
        let bytes = shown.attachment.bytes.clone();
        let picked = cx.prompt_for_new_path(
            &attachments::downloads_dir(),
            Some(&attachments::save_name(&shown.attachment)),
        );
        cx.spawn(async move |_, cx| {
            let path = match picked.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    log::warn!("download dialog: {error}");
                    return;
                }
            };
            let written = cx
                .background_spawn(async move { std::fs::write(&path, bytes.as_slice()).map_err(|error| (path, error)) })
                .await;
            if let Err((path, error)) = written {
                log::warn!("download {}: {error}", path.display());
            }
        })
        .detach();
    }

    /// Esc closes the viewer and ← → step; it takes every other key too, as a dialog does. False when it is closed.
    pub(super) fn image_viewer_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.overlay.image_viewer().is_none() {
            return false;
        }
        match key {
            "escape" => self.close_image(cx),
            "left" => self.step_image(-1, cx),
            "right" => self.step_image(1, cx),
            _ => {}
        }
        true
    }

    pub(super) fn render_image_viewer(
        &self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let viewer = self.overlay.image_viewer()?;
        let shown = viewer.images.get(viewer.index)?;
        let many = viewer.images.len() > 1;
        let viewport = window.viewport_size();
        let room = (
            f32::from(viewport.width) * ACROSS,
            f32::from(viewport.height) * DOWN - BAR,
        );
        let (width, height) = shown
            .pixels
            .map_or(room, |(width, height)| fit(width as f32, height as f32, room));
        let button = |id: &'static str, icon: Lucide, label: &'static str| {
            div()
                .id(id)
                .size(px(28.))
                .flex_none()
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
                .child(Icon::new(icon).size(px(16.)))
        };
        let detail = if many {
            format!("{} of {} · {}", viewer.index + 1, viewer.images.len(), shown.detail)
        } else {
            shown.detail.clone()
        };
        let bar = h_flex()
            .id("image-viewer-bar")
            .h(px(BAR))
            .w(px(width.max(320.)))
            .px(px(12.))
            .gap(px(8.))
            .rounded(px(10.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .text_size(px(13.))
            // A click on the bar is not a click outside.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().min_w(px(0.)).truncate().child(shown.name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(palette.muted)
                    .child(detail),
            )
            .when(many, |bar| {
                bar.child(
                    button("image-previous", Lucide::ChevronLeft, "Previous (←)")
                        .on_click(cx.listener(|this, _, _, cx| this.step_image(-1, cx))),
                )
                .child(
                    button("image-next", Lucide::ChevronRight, "Next (→)")
                        .on_click(cx.listener(|this, _, _, cx| this.step_image(1, cx))),
                )
            })
            .child(
                button("image-download", Lucide::Download, "Download")
                    .on_click(cx.listener(|this, _, _, cx| this.download_image(cx))),
            )
            .child(
                button("image-close", Lucide::X, "Close (Esc)")
                    .on_click(cx.listener(|this, _, _, cx| this.close_image(cx))),
            );
        let image = div()
            .id("image-viewer-stage")
            .w(px(width))
            .h(px(height))
            // A click on the image is not a click outside.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(img(shown.image.clone()).size_full().object_fit(ObjectFit::Contain));
        Some(
            div()
                .id("image-viewer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .bg(palette.scrim)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_image(cx)))
                .child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap(px(12.))
                        .child(image)
                        .child(bar),
                )
                .with_animation(
                    "image-viewer-in",
                    Animation::new(motion::UI).with_easing(motion::ease_out),
                    |overlay, delta| overlay.opacity(delta),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{fit, step};

    #[test]
    fn stepping_goes_round_the_ends() {
        assert_eq!(step(0, -1, 3), 2);
        assert_eq!(step(2, 1, 3), 0);
        assert_eq!(step(1, 1, 3), 2);
        assert_eq!(step(0, 1, 1), 0);
        assert_eq!(step(0, 1, 0), 0);
    }

    #[test]
    fn an_image_fits_the_room_and_is_never_drawn_larger() {
        assert_eq!(fit(4000., 2000., (1000., 800.)), (1000., 500.));
        assert_eq!(fit(1000., 4000., (1000., 800.)), (200., 800.));
        assert_eq!(fit(300., 200., (1000., 800.)), (300., 200.));
        assert_eq!(fit(0., 0., (1000., 800.)), (1000., 800.));
    }
}
