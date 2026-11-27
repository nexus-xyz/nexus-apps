//! Screenshots and videos that an agent names in its reply, read on the session's machine so the reply can show them
//! inline. The reply loads them without a click, so this read is narrower than [`crate::output_files`]:
//! only image and video files, only inside the session folder (its `.nexus/artifacts/` folder included), and only up
//! to a size cap. The bytes come in pieces of [`CHUNK`], so that a 50 MB video never becomes one message on a remote
//! host's connection (a WebSocket frame takes at most 16 MiB).
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

/// A host's hello names this when it answers `wb_file_media`.
pub const FEATURE: &str = "inline-media";
/// The largest image a reply shows inline.
pub const MAX_IMAGE: u64 = 10 * 1024 * 1024;
/// The largest video a reply shows inline.
pub const MAX_VIDEO: u64 = 50 * 1024 * 1024;
/// The bytes in one answer. Base64 makes them 5.3 MiB, well under a WebSocket frame.
pub const CHUNK: u64 = 4 * 1024 * 1024;
/// The folder where the agents keep their proof, inside the project. The agent instructions name it too.
pub const ARTIFACTS_DIR: &str = ".nexus/artifacts";

/// One piece of a media file. `status` is `ready` (here are `data`, from `offset`), or `tooLarge` (no data: `size` is
/// over the cap of its kind). Written by the session's machine, read by the app that shows it
/// ([`crate::inline_media::read_all`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaChunk {
    pub path: String,
    pub media_type: String,
    pub size: u64,
    pub offset: u64,
    pub data: Option<String>,
    pub status: String,
}

/// The media type of an image or video path, from its extension. None for every other file.
pub fn media_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

/// The cap for a media type.
pub fn limit(media_type: &str) -> u64 {
    if media_type.starts_with("video/") {
        MAX_VIDEO
    } else {
        MAX_IMAGE
    }
}

/// Reads the piece of `path` (relative to `root`, or absolute) that starts at `offset`. Refused: other file types, files
/// outside `root` (also through a symlink), folders and devices, and an offset past the end.
pub fn read(root: &Path, path: &str, offset: u64) -> Result<MediaChunk, String> {
    let full = crate::output_files::resolve(root, path)?;
    let media =
        media_type(&full).ok_or("Only images (PNG, JPEG, GIF, WebP) and videos (MP4, WebM, MOV) show inline.")?;
    let inside = root.canonicalize().is_ok_and(|root| full.starts_with(root));
    if !inside {
        return Err("Only files inside the project show inline. Save the file under .nexus/artifacts/.".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // A file replaced with a FIFO between resolution and open must not block a worker forever.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let mut file = options.open(&full).map_err(read_error)?;
    let metadata = file.metadata().map_err(read_error)?;
    if !metadata.is_file() {
        return Err("Only regular files show inline.".into());
    }
    let size = metadata.len();
    let mut chunk = MediaChunk {
        path: full.to_string_lossy().into_owned(),
        media_type: media.into(),
        size,
        offset,
        data: None,
        status: "ready".into(),
    };
    if size > limit(media) {
        chunk.status = "tooLarge".into();
        return Ok(chunk);
    }
    if offset > size {
        return Err("The file changed while it loaded. Load it again.".into());
    }
    file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
    let mut bytes = Vec::new();
    file.take(CHUNK.min(size - offset))
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    chunk.data = Some(STANDARD.encode(bytes));
    Ok(chunk)
}

fn read_error(error: io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => crate::output_files::MISSING.into(),
        io::ErrorKind::PermissionDenied => {
            "The operating system denied access to this file. Check its permissions and try again.".into()
        }
        _ => format!("Could not read this file: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("nexus-media-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("project").join(ARTIFACTS_DIR)).expect("create the fixture project");
            Self(dir)
        }
        fn root(&self) -> std::path::PathBuf {
            self.0.join("project")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn decode(chunk: &MediaChunk) -> Vec<u8> {
        STANDARD
            .decode(chunk.data.as_deref().expect("the chunk has data"))
            .expect("decode the chunk")
    }

    #[test]
    fn reads_artifacts_in_pieces() {
        let f = Fixture::new();
        let bytes: Vec<u8> = (0..CHUNK + 10).map(|n| (n % 251) as u8).collect();
        std::fs::write(f.root().join(ARTIFACTS_DIR).join("login.webm"), &bytes).expect("write the video");
        let first = read(&f.root(), ".nexus/artifacts/login.webm", 0).expect("read the first piece");
        assert_eq!(first.media_type, "video/webm");
        assert_eq!(first.size, CHUNK + 10);
        assert_eq!(first.status, "ready");
        assert_eq!(decode(&first), bytes[..CHUNK as usize]);
        let last = read(&f.root(), ".nexus/artifacts/login.webm", CHUNK).expect("read the last piece");
        assert_eq!(last.offset, CHUNK);
        assert_eq!(decode(&last), bytes[CHUNK as usize..]);
        let end = read(&f.root(), ".nexus/artifacts/login.webm", CHUNK + 10).expect("read at the end");
        assert_eq!(decode(&end), b"");
        assert!(read(&f.root(), ".nexus/artifacts/login.webm", CHUNK + 11).is_err());
        let absolute = f.root().join(ARTIFACTS_DIR).join("login.webm");
        let by_path = read(&f.root(), &absolute.to_string_lossy(), 0).expect("read by absolute path");
        assert_eq!(by_path.size, CHUNK + 10);
    }

    #[test]
    fn caps_each_kind() {
        let f = Fixture::new();
        let image = f.root().join("big.png");
        std::fs::File::create(&image)
            .and_then(|file| file.set_len(MAX_IMAGE + 1))
            .expect("make a large image");
        let big = read(&f.root(), "big.png", 0).expect("report the large image");
        assert_eq!(big.status, "tooLarge");
        assert!(big.data.is_none());
        let video = f.root().join("fits.mp4");
        std::fs::File::create(&video)
            .and_then(|file| file.set_len(MAX_IMAGE + 1))
            .expect("make a video");
        assert_eq!(read(&f.root(), "fits.mp4", 0).expect("read the video").status, "ready");
        std::fs::File::create(&video)
            .and_then(|file| file.set_len(MAX_VIDEO + 1))
            .expect("make a large video");
        let big = read(&f.root(), "fits.mp4", 0).expect("report the large video");
        assert_eq!(big.status, "tooLarge");
        assert_eq!(big.size, MAX_VIDEO + 1);
        assert!(big.data.is_none());
    }

    #[test]
    fn refuses_outside_files_other_types_and_devices() {
        let f = Fixture::new();
        std::fs::write(f.0.join("outside.png"), b"png").expect("write the outside image");
        let outside = read(&f.root(), "../outside.png", 0).expect_err("refuse a file outside the project");
        assert!(outside.contains("inside the project"));
        let absolute = f.0.join("outside.png");
        assert!(read(&f.root(), &absolute.to_string_lossy(), 0).is_err());
        std::fs::write(f.root().join("notes.md"), b"# notes").expect("write a text file");
        assert!(
            read(&f.root(), "notes.md", 0)
                .expect_err("refuse text")
                .contains("Only images")
        );
        assert!(
            read(&f.root(), "missing.png", 0)
                .expect_err("report a missing file")
                .contains("missing")
        );
        std::fs::create_dir_all(f.root().join("folder.png")).expect("make a folder named like an image");
        assert!(read(&f.root(), "folder.png", 0).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&absolute, f.root().join("link.png")).expect("link the outside image");
            assert!(
                read(&f.root(), "link.png", 0)
                    .expect_err("refuse a link out")
                    .contains("inside the project")
            );
        }
    }

    #[test]
    fn names_media_types() {
        for (name, media) in [
            ("a.PNG", Some("image/png")),
            ("a.jpeg", Some("image/jpeg")),
            ("a.gif", Some("image/gif")),
            ("a.webp", Some("image/webp")),
            ("a.mp4", Some("video/mp4")),
            ("a.webm", Some("video/webm")),
            ("a.mov", Some("video/quicktime")),
            ("a.svg", None),
            ("a", None),
        ] {
            assert_eq!(media_type(Path::new(name)), media, "{name}");
        }
    }
}
