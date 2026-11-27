//! Images and videos in a Code session's transcript, as the Nexus app shows them: the images a tool returned stay
//! under its row (`tool-images.tsx`), and the screenshots and videos an answer links to load from the session's
//! machine (`inline-media.tsx`, `wb_file_media`). Which links are media, and how a file's pieces join, is
//! `nexus_workbench::inline_media`; this reads them and makes what GPUI draws.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui_kit::{Image, ImageFormat};
use nexus_workbench::inline_media::{self, Loaded, MediaKind};
use nexus_workbench::output_media;

use crate::attachments::Attachment;
use crate::side::server::ServerPlace;

/// An image a tool returned, decoded once as the transcript is read.
#[derive(Clone)]
pub struct ToolImage {
    pub name: String,
    pub media_type: &'static str,
    pub image: Arc<Image>,
}

impl PartialEq for ToolImage {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.media_type == other.media_type && self.image.id() == other.image.id()
    }
}

impl std::fmt::Debug for ToolImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolImage")
            .field("name", &self.name)
            .field("media_type", &self.media_type)
            .field("bytes", &self.image.bytes.len())
            .finish()
    }
}

impl Media {
    /// A loaded image as the full-size viewer takes it (`image_viewer.rs`).
    pub fn attachment(&self) -> Option<Attachment> {
        match self {
            Media::Image {
                name,
                media_type,
                image,
                ..
            } => Some(crate::attachments::image_attachment(
                name.clone(),
                media_type,
                Arc::new(image.bytes.clone()),
            )),
            _ => None,
        }
    }
}

impl ToolImage {
    /// The image as the full-size viewer takes it (`image_viewer.rs`).
    pub fn attachment(&self) -> Attachment {
        crate::attachments::image_attachment(self.name.clone(), self.media_type, Arc::new(self.image.bytes.clone()))
    }
}

/// A raster type the viewer knows, as a static name; PNG for any other.
fn known_type(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => "image/png",
    }
}

fn format_of(media_type: &str) -> ImageFormat {
    match media_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

/// A tool result's images, ready to draw: "Tool image 2" when one has no name (`tool-images.tsx`). One whose data is
/// not base64 is left out.
pub fn tool_images(images: Vec<nexus_workbench::event::Image>) -> Vec<ToolImage> {
    images
        .into_iter()
        .enumerate()
        .filter_map(|(index, image)| {
            let bytes = STANDARD.decode(image.data.as_bytes()).ok()?;
            let media_type = known_type(&image.media_type);
            Some(ToolImage {
                name: image
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| format!("Tool image {}", index + 1)),
                media_type,
                image: Arc::new(Image::from_bytes(format_of(media_type), bytes)),
            })
        })
        .collect()
}

/// What an answer's linked image or video shows.
#[derive(Clone)]
pub enum Media {
    Loading,
    /// The image, read in full: its name for the viewer, its size in pixels when they read, and in bytes.
    Image {
        name: String,
        media_type: &'static str,
        image: Arc<Image>,
        pixels: Option<(u32, u32)>,
        size: u64,
    },
    /// A video: GPUI has no player, so it shows as a card. `file`: where it is on this computer, for the video player
    /// to open; None on a server. `size`: None when the server was not asked.
    Video {
        size: Option<u64>,
        file: Option<PathBuf>,
    },
    /// Over the cap of its kind (`inline_media::too_large` says so).
    TooLarge(u64),
    Failed(String),
}

/// Where a session's media is read: its folder on this computer, or its server.
#[derive(Clone)]
pub enum Source {
    Here(PathBuf),
    Server(ServerPlace),
}

/// Read `path` (relative to the session's folder, or absolute) for an answer (slow: run it off the UI thread).
pub async fn load(source: Source, path: String, kind: MediaKind) -> Media {
    let name = file_name(&path);
    match (source, kind) {
        (Source::Here(root), MediaKind::Video) => local_video(&root, &path),
        (Source::Server(_), MediaKind::Video) => Media::Video { size: None, file: None },
        (Source::Here(root), MediaKind::Image) => {
            let read = inline_media::read_all(&path, |offset| {
                let root = root.clone();
                let path = path.clone();
                async move { output_media::read(&root, &path, offset) }
            })
            .await;
            image_of(name, read)
        }
        (Source::Server(server), MediaKind::Image) => {
            let read = inline_media::read_all(&path, |offset| {
                let server = server.clone();
                let path = path.clone();
                async move { server.media(&path, offset).await }
            })
            .await;
            image_of(name, read)
        }
    }
}

fn image_of(name: String, read: Result<Loaded, String>) -> Media {
    match read {
        Err(error) => Media::Failed(error),
        Ok(Loaded::TooLarge { size }) => Media::TooLarge(size),
        Ok(Loaded::Ready {
            bytes,
            media_type,
            size,
        }) => {
            let media_type = known_type(&media_type);
            let pixels = ::image::ImageReader::new(std::io::Cursor::new(bytes.as_slice()))
                .with_guessed_format()
                .ok()
                .and_then(|reader| reader.into_dimensions().ok());
            Media::Image {
                name,
                media_type,
                image: Arc::new(Image::from_bytes(format_of(media_type), bytes)),
                pixels,
                size,
            }
        }
    }
}

/// A video in the session's folder on this computer: its size and full path, refused as `wb_file_media` refuses it
/// (outside the project, not a regular file, over the cap).
fn local_video(root: &Path, path: &str) -> Media {
    let checked = (|| {
        let full = nexus_workbench::output_files::resolve(root, path)?;
        output_media::media_type(&full).ok_or("Only images and videos show inline.")?;
        let inside = root.canonicalize().is_ok_and(|root| full.starts_with(root));
        if !inside {
            return Err("Only files inside the project show inline. Save the file under .nexus/artifacts/.".into());
        }
        let size = full.metadata().map_err(|error| error.to_string())?.len();
        Ok::<_, String>((full, size))
    })();
    match checked {
        Err(error) => Media::Failed(error),
        Ok((_, size)) if size > MediaKind::Video.limit() => Media::TooLarge(size),
        Ok((full, size)) => Media::Video {
            size: Some(size),
            file: Some(full),
        },
    }
}

/// The last part of a path, as the card names a file with no label (`fileName`).
pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXEL: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn wb_image(name: Option<&str>, data: &str) -> nexus_workbench::event::Image {
        nexus_workbench::event::Image {
            media_type: "image/png".into(),
            data: data.into(),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn a_tools_images_keep_their_names_or_take_one_and_bad_data_is_left_out() {
        let images = tool_images(vec![
            wb_image(Some("login.png"), PIXEL),
            wb_image(None, "not base64!"),
            wb_image(None, PIXEL),
        ]);
        let names: Vec<_> = images.iter().map(|image| image.name.as_str()).collect();
        assert_eq!(names, ["login.png", "Tool image 3"]);
        assert_eq!(images[0].media_type, "image/png");
        assert_eq!(images[0].attachment().media_type, Some("image/png"));
        // The same image read again is the same block, so the transcript keeps its row.
        assert_eq!(tool_images(vec![wb_image(Some("login.png"), PIXEL)])[0], images[0]);
    }

    #[test]
    fn names_a_file_by_its_last_part() {
        assert_eq!(file_name(".nexus/artifacts/login.png"), "login.png");
        assert_eq!(file_name("C:\\work\\shot.png"), "shot.png");
        assert_eq!(file_name("shot.png"), "shot.png");
    }

    #[test]
    fn a_local_video_inside_the_project_opens_and_one_outside_is_refused() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-media-{}", std::process::id()));
        let root = dir.join("project");
        std::fs::create_dir_all(root.join(".nexus/artifacts")).expect("make the project");
        std::fs::write(root.join(".nexus/artifacts/flow.webm"), [1, 2, 3]).expect("write the video");
        std::fs::write(dir.join("outside.webm"), [1]).expect("write the other video");
        match local_video(&root, ".nexus/artifacts/flow.webm") {
            Media::Video { size, file } => {
                assert_eq!(size, Some(3));
                assert!(file.is_some_and(|file| file.ends_with("flow.webm")));
            }
            _ => panic!("the video shows"),
        }
        assert!(
            matches!(local_video(&root, "../outside.webm"), Media::Failed(error) if error.contains("inside the project"))
        );
        assert!(matches!(local_video(&root, "missing.webm"), Media::Failed(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_answers_image_reads_from_the_session_folder() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("make the folder");
        std::fs::write(dir.join("shot.png"), STANDARD.decode(PIXEL).expect("pixel")).expect("write the image");
        let media = futures::executor::block_on(load(Source::Here(dir.clone()), "shot.png".into(), MediaKind::Image));
        match media {
            media @ Media::Image { .. } => {
                let Media::Image { pixels, size, .. } = &media else {
                    unreachable!()
                };
                assert_eq!(*pixels, Some((1, 1)));
                let attachment = media.attachment().expect("an image");
                assert_eq!(attachment.name, "shot.png");
                assert_eq!(*size, attachment.size() as u64);
            }
            _ => panic!("the image loads"),
        }
        let missing = futures::executor::block_on(load(Source::Here(dir.clone()), "gone.png".into(), MediaKind::Image));
        assert!(matches!(missing, Media::Failed(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
