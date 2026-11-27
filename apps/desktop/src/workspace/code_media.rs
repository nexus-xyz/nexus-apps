//! A Code session's images and videos, drawn as the Nexus app draws them. A tool's images show as thumbnails under
//! its row, whatever its text shows (`tool-images.tsx`). An answer's linked screenshots and videos show under the
//! answer's text (`inline-media.tsx`): an image fitted to the column, a video as a card that opens it in the video
//! player, since GPUI has no player. A click on an image opens it full size (`image_viewer.rs`).

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::inline_media::{self, MediaKind, MediaLink};

use super::code_dock::text_button;
use super::{View, Workspace, glyph};
use crate::code::Block;
use crate::code_media::{self, Media, Source, ToolImage};
use crate::palette::Palette;

/// An answer's image: at most this wide and high (`.wb-media`, `.wb-media-frame img`).
const MEDIA_WIDTH: f32 = 560.;
const MEDIA_HEIGHT: f32 = 360.;

/// Each message keeps its image handles across redraws and transcript refreshes.
#[derive(Default)]
pub(super) struct SentImages(std::collections::HashMap<i64, Arc<Vec<ToolImage>>>);

impl SentImages {
    pub(super) fn keep(&mut self, seq: i64, images: Arc<Vec<ToolImage>>) {
        self.0.insert(seq, images);
    }

    fn get(&mut self, seq: i64, images: &[nexus_workbench::event::Image]) -> Arc<Vec<ToolImage>> {
        self.0
            .entry(seq)
            .or_insert_with(|| Arc::new(code_media::tool_images(images.to_vec())))
            .clone()
    }
}

impl Workspace {
    /// What block `ix` shows under itself: a tool's images, or the media its answer links to. None for the rest.
    pub(super) fn code_item_media(
        &mut self,
        ix: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let View::Code(view) = &mut self.view else {
            return None;
        };
        match view.blocks.get(ix)?.inner() {
            Block::User {
                seq,
                image_data,
                from: Some(_),
                ..
            } if !image_data.is_empty() => {
                let images = view.user_images.get(*seq, image_data);
                Some(tool_images(ix, images.as_ref().clone(), palette, cx).into_any_element())
            }
            Block::Tool { images, .. } if !images.is_empty() => {
                Some(tool_images(ix, images.clone(), palette, cx).into_any_element())
            }
            Block::Text(markdown) => {
                let links = inline_media::media_links(markdown);
                if links.is_empty() {
                    return None;
                }
                self.answer_media(ix, links, palette, cx)
            }
            _ => None,
        }
    }

    /// Keep image handles across redraws so GPUI's asynchronous image load can finish.
    pub(super) fn code_user_images(
        &mut self,
        ix: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let View::Code(view) = &mut self.view else { return None };
        let Block::User {
            seq,
            image_data,
            from: None,
            ..
        } = view.blocks.get(ix)?.inner()
        else {
            return None;
        };
        if image_data.is_empty() {
            return None;
        }
        let seq = *seq;
        let images = view.user_images.get(seq, image_data);
        Some(self.sent_images(format!("user-images-{seq}"), images, palette, cx))
    }

    pub(super) fn sent_images(
        &self,
        id: String,
        images: Arc<Vec<ToolImage>>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = images.clone();
        let workspace = cx.weak_entity();
        user_images(
            id,
            images,
            palette,
            Rc::new(move |index, _, cx| {
                let _ = workspace.update(cx, |this, cx| {
                    let attachments: Vec<_> = open.iter().map(ToolImage::attachment).collect();
                    this.open_image(&attachments, index, cx);
                });
            }),
        )
        .into_any_element()
    }

    /// Where the open session's files are read: its folder here, or its server when the host reads media. None
    /// leaves the answer's links as links, as the Nexus app does without the host's `inline-media`.
    fn media_source(&self, id: &str) -> Option<Source> {
        let machines = crate::machines::get();
        if let Some(machine) = machines.machine_of(id) {
            let server = crate::side::server::ServerPlace {
                machines,
                machine,
                thread: Some(id.to_string()),
                project: None,
            };
            return server.shows_media().then_some(Source::Server(server));
        }
        let folder = self.code.as_ref()?.folder(id).ok()?;
        Some(Source::Here(folder.path))
    }

    /// The cards for `links`, loading each file the first time it shows.
    fn answer_media(
        &mut self,
        ix: usize,
        links: Vec<MediaLink>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = match &self.view {
            View::Code(view) => view.id.clone(),
            _ => return None,
        };
        let source = self.media_source(&id)?;
        let width = (self.column_width() - 48.).clamp(120., MEDIA_WIDTH);
        let View::Code(view) = &mut self.view else {
            return None;
        };
        let mut cards = Vec::with_capacity(links.len());
        for (n, link) in links.into_iter().enumerate() {
            let state = view.media.entry(link.path.clone()).or_insert_with(|| {
                load(id.clone(), source.clone(), link.path.clone(), link.kind, cx);
                Media::Loading
            });
            cards.push(media_card(
                SharedString::from(format!("media-{ix}-{n}")),
                &link,
                state.clone(),
                width,
                palette,
                cx,
            ));
        }
        Some(
            v_flex()
                .w_full()
                .gap(px(8.))
                .pt(px(8.))
                .children(cards)
                .into_any_element(),
        )
    }

    /// Read `path` again after a failure ("Try again").
    fn retry_media(&mut self, path: &str, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view {
            remeasure(view, path);
            view.media.remove(path);
            cx.notify();
        }
    }
}

/// Sent image previews sit above the person's bubble and wrap within its column.
fn user_images(
    id: String,
    images: Arc<Vec<ToolImage>>,
    palette: Palette,
    open: Rc<dyn Fn(usize, &mut Window, &mut App)>,
) -> impl IntoElement {
    h_flex()
        .id(SharedString::from(id.clone()))
        .w_full()
        .justify_end()
        .flex_wrap()
        .gap(px(8.))
        .children(images.iter().enumerate().map(|(index, image)| {
            let open = open.clone();
            div()
                .id(SharedString::from(format!("{id}-{index}")))
                .debug_selector(move || format!("user-image-{index}"))
                .w(px(160.))
                .max_w_full()
                .h(px(120.))
                .flex_shrink_0()
                .overflow_hidden()
                .rounded(px(12.))
                .border_1()
                .border_color(palette.border)
                .bg(palette.bubble)
                .cursor_pointer()
                .hover(|style| style.border_color(palette.faint))
                .tooltip(|window, cx| Tooltip::new("Open full size").build(window, cx))
                .child(
                    img(ImageSource::Image(image.image.clone()))
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .on_click(move |_, window, cx| open(index, window, cx))
        }))
}

/// Read `path` off the UI thread; the answer shows it once read, if session `id` is still open.
fn load(id: String, source: Source, path: String, kind: MediaKind, cx: &mut Context<Workspace>) {
    let read = cx
        .background_executor()
        .spawn(code_media::load(source, path.clone(), kind));
    cx.spawn(async move |this, cx| {
        let media = read.await;
        let _ = this.update(cx, |this, cx| {
            if let View::Code(view) = &mut this.view
                && view.id == id
            {
                remeasure(view, &path);
                view.media.insert(path, media);
                cx.notify();
            }
        });
    })
    .detach();
}

/// The answers that link to `path` change height as it loads: the list measures them again.
fn remeasure(view: &super::code_view::CodeView, path: &str) {
    for (ix, block) in view.blocks.iter().enumerate() {
        if let Block::Text(markdown) = block
            && inline_media::media_links(markdown).iter().any(|link| link.path == path)
        {
            view.list.remeasure_items(ix..ix + 1);
        }
    }
}

/// A tool's images as 96 × 72 thumbnails; a click opens them full size, among the others (`tool-images.css`).
fn tool_images(ix: usize, images: Vec<ToolImage>, palette: Palette, cx: &mut Context<Workspace>) -> Div {
    let shared: Arc<Vec<ToolImage>> = Arc::new(images);
    h_flex()
        .flex_wrap()
        .gap(px(8.))
        .pt(px(8.))
        .pb(px(4.))
        .pl(px(30.))
        .children(shared.iter().enumerate().map(|(n, image)| {
            let all = shared.clone();
            div()
                .id(SharedString::from(format!("tool-image-{ix}-{n}")))
                .debug_selector(move || format!("tool-image-{ix}-{n}"))
                .flex_none()
                .overflow_hidden()
                .rounded(px(8.))
                .border_1()
                .border_color(palette.border)
                .cursor_pointer()
                .hover(|style| style.border_color(palette.faint))
                .child(
                    img(ImageSource::Image(image.image.clone()))
                        .w(px(96.))
                        .h(px(72.))
                        .object_fit(ObjectFit::Cover),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let attachments: Vec<_> = all.iter().map(ToolImage::attachment).collect();
                    this.open_image(&attachments, n, cx);
                }))
        }))
}

/// `width` × `height` fitted to `room`, never larger than it is.
fn fit(pixels: Option<(u32, u32)>, room: (f32, f32)) -> (f32, f32) {
    let Some((width, height)) = pixels.filter(|(w, h)| *w > 0 && *h > 0) else {
        return (room.0, room.1);
    };
    let (width, height) = (width as f32, height as f32);
    let scale = (room.0 / width).min(room.1 / height).min(1.);
    (width * scale, height * scale)
}

/// One linked file as its state shows it (`MediaCard`).
fn media_card(
    id: SharedString,
    link: &MediaLink,
    state: Media,
    width: f32,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let name: SharedString = if link.label.trim().is_empty() {
        code_media::file_name(&link.path).into()
    } else {
        link.label.clone().into()
    };
    let icon = match link.kind {
        MediaKind::Image => Lucide::Image,
        MediaKind::Video => Lucide::Film,
    };
    let noun = link.kind.noun();
    if let Media::Image {
        image, pixels, size, ..
    } = &state
    {
        let (w, h) = fit(*pixels, (width, MEDIA_HEIGHT));
        let open = state.clone();
        return v_flex()
            .gap(px(4.))
            .max_w(px(width))
            .child(
                div()
                    .id(id)
                    .w(px(w + 2.))
                    .overflow_hidden()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.hover)
                    .cursor_pointer()
                    .child(img(ImageSource::Image(image.clone())).w(px(w)).h(px(h)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(attachment) = open.attachment() {
                            this.open_image(&[attachment], 0, cx);
                        }
                    })),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .min_w(px(0.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(glyph(icon, palette.muted).size(px(13.)))
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_color(palette.foreground)
                            .child(name),
                    )
                    .child(div().flex_none().child(inline_media::size_label(*size))),
            )
            .into_any_element();
    }
    let (message, retry, open) = match state {
        Media::Loading => (format!("Loading {noun}…"), false, None),
        Media::TooLarge(size) => (inline_media::too_large(link.kind, size), false, None),
        Media::Failed(error) => (error, true, None),
        Media::Video { size, file } => (
            match size {
                Some(size) => format!("Video · {}", inline_media::size_label(size)),
                None => "Video".to_string(),
            },
            false,
            file,
        ),
        Media::Image { .. } => unreachable!("drawn above"),
    };
    let path = link.path.clone();
    h_flex()
        .max_w(px(width))
        .gap(px(8.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(8.))
        .border_1()
        .border_color(palette.border)
        .child(glyph(icon, palette.muted).size(px(16.)))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.foreground)
                        .child(name),
                )
                .child(div().text_size(px(12.)).text_color(palette.muted).child(message)),
        )
        .when(retry, |card| {
            card.child(
                text_button(SharedString::from(format!("{id}-retry")), "Try again", false, palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.retry_media(&path, cx))),
            )
        })
        .when_some(open, |card, file| {
            card.child(
                text_button(SharedString::from(format!("{id}-open")), "Open", false, palette)
                    .on_click(move |_, _, cx| cx.open_with_system(&file)),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use core::prelude::v1::test;
    use std::cell::Cell;

    struct Prompt {
        width: f32,
        text: String,
        source: Vec<nexus_workbench::event::Image>,
        images: SentImages,
        opened: Rc<Cell<Option<usize>>>,
    }

    impl Render for Prompt {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
            let opened = self.opened.clone();
            let images = user_images(
                "prompt".into(),
                self.images.get(7, &self.source),
                palette,
                Rc::new(move |index, _, _| opened.set(Some(index))),
            )
            .into_any_element();
            div().w(px(self.width)).child(super::super::code_view::user_prompt(
                self.text.clone(),
                Vec::new(),
                Some(images),
                palette,
                None,
            ))
        }
    }

    fn prompt() -> Prompt {
        let mut png = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::new_rgb8(32, 20)
            .write_to(&mut png, ::image::ImageFormat::Png)
            .unwrap();
        let source = (0..2)
            .map(|index| nexus_workbench::event::Image {
                media_type: "image/png".into(),
                data: STANDARD.encode(png.get_ref()),
                name: Some(format!("Screenshot {index}.png")),
            })
            .collect();
        Prompt {
            width: 600.,
            text: "Use these screenshots".into(),
            source,
            images: SentImages::default(),
            opened: Rc::new(Cell::new(None)),
        }
    }

    #[gpui_kit::test]
    fn sent_images_stay_above_the_bubble_and_wrap_inside_narrow_columns(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (prompt, cx) = cx.add_window_view(|_, _| prompt());
        for width in [600., 240., 120.] {
            prompt.update(cx, |prompt, cx| {
                prompt.width = width;
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let first = cx.debug_bounds("user-image-0").expect("first image is visible");
            let last = cx.debug_bounds("user-image-1").expect("second image is visible");
            let bubble = cx.debug_bounds("user-bubble").expect("prompt is visible");
            assert!(last.bottom() <= bubble.top(), "images stay above their message");
            assert_eq!(last.right(), bubble.right(), "images align with the person's bubble");
            assert!(
                first.left() >= px(0.) && last.right() <= px(width),
                "previews fit the column"
            );
            if width < 328. {
                assert!(first.bottom() < last.top(), "previews wrap on a narrow column");
            }
            for (index, bounds) in [(0, first), (1, last)] {
                prompt.read_with(cx, |prompt, _| prompt.opened.set(None));
                cx.simulate_click(bounds.center(), Modifiers::none());
                assert_eq!(prompt.read_with(cx, |prompt, _| prompt.opened.get()), Some(index));
            }
        }
        prompt.update(cx, |prompt, cx| {
            prompt.text.clear();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("user-image-0").is_some(),
            "image-only messages stay visible"
        );
        assert!(
            cx.debug_bounds("user-bubble").is_none(),
            "image-only messages have no empty bubble"
        );
    }

    #[test]
    fn redraws_keep_the_same_image_handles_and_each_message_has_its_own() {
        let mut prompt = prompt();
        let first = prompt.images.get(7, &prompt.source);
        let redrawn = prompt.images.get(7, &prompt.source);
        assert!(Arc::ptr_eq(&first, &redrawn));
        assert_eq!(first[0].image.id(), redrawn[0].image.id());
        assert!(!Arc::ptr_eq(&first, &prompt.images.get(8, &prompt.source)));
        let pending = Arc::new(code_media::tool_images(prompt.source.clone()));
        prompt.images.keep(9, pending.clone());
        assert!(
            Arc::ptr_eq(&pending, &prompt.images.get(9, &prompt.source)),
            "the stored message keeps its opening preview"
        );
    }

    #[test]
    fn an_answers_image_fits_the_column_and_never_grows() {
        assert_eq!(fit(Some((1120, 720)), (560., 360.)), (560., 360.));
        assert_eq!(fit(Some((2000, 400)), (560., 360.)), (560., 112.));
        assert_eq!(fit(Some((200, 100)), (560., 360.)), (200., 100.));
        assert_eq!(fit(None, (560., 360.)), (560., 360.));
    }
}
