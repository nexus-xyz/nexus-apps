//! A picture of a page as it last looked, shown while the page opens again. The web engine takes about a second to
//! start and load a page; the picture fills that time, as a phone shows an app's last screen while the app starts.
//! [`WebView::keep_snapshot`](super::WebView::keep_snapshot) takes it over the DevTools Protocol after the page
//! loads, keeps it as a JPEG in the app's cache, and draws it only until the page itself has drawn.
//!
//! A picture holds what the page showed, so it stays private to this computer account (owner-only files) and goes
//! with the sign-in: [`clear`] removes them all when the person signs out or switches organization.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::RenderImage;
use smallvec::SmallVec;

/// How many pictures the cache keeps. The oldest go first.
const KEEP: usize = 50;
/// The picture is taken this long after the page loaded, so its fonts and first animations are in place.
pub(super) const TAKE_AFTER: Duration = Duration::from_millis(1500);
/// The longest a picture shows: a page that never finishes loading shows itself after this.
pub(super) const SHOW_AT_MOST: Duration = Duration::from_secs(8);
/// The JPEG quality. The picture shows for about a second, and the page then covers it.
pub(super) const QUALITY: u8 = 70;
/// A picture taken at another width is not drawn: the page lays out again at the new width, and the jump would show.
const WIDTH_SLACK: f32 = 0.15;

/// The file that keeps the picture of `key`: a name for one page at one version.
pub(super) fn file_for(key: &str) -> PathBuf {
    file_in(&folder(), key)
}

fn file_in(folder: &Path, key: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    let name: String = digest[..12].iter().map(|byte| format!("{byte:02x}")).collect();
    folder.join(format!("{name}.jpg"))
}

fn folder() -> PathBuf {
    crate::paths::cache_dir().join("page-snapshots")
}

/// Remove every picture: they show one sign-in's pages, and the next one must not find them. Runs in the background.
pub(super) fn clear() {
    let folder = folder();
    let _ = std::thread::Builder::new()
        .name("page-snapshots-clear".into())
        .spawn(move || clear_in(&folder));
}

fn clear_in(folder: &Path) {
    if let Err(error) = std::fs::remove_dir_all(folder)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("nexus-desktop-gpui: page snapshots: {error}");
    }
}

/// Whether the page is still at the address it opened at. The picture is of that page: one taken after a click on a
/// link, a redirect to a sign-in screen or an error page would show that other state at the next open.
pub(super) fn at_address(address: Option<&str>, opened: &str) -> bool {
    fn same(url: &str) -> Option<String> {
        let mut url = url::Url::parse(url).ok()?;
        url.set_fragment(None);
        Some(url.to_string())
    }
    match (address.and_then(same), same(opened)) {
        (Some(now), Some(then)) => now == then,
        _ => false,
    }
}

/// Whether the navigation's HTTP status (`performance.getEntriesByType('navigation')[0].responseStatus`, as the
/// DevTools Protocol answers `Runtime.evaluate`) is a success. A page that answered an error, or a status the engine
/// did not give, gets no picture.
pub(super) fn loaded_ok(result: &serde_json::Value) -> bool {
    result["result"]["value"]
        .as_u64()
        .is_some_and(|status| (200..300).contains(&status))
}

/// The expression that reads the navigation's status.
pub(super) const STATUS: &str = "(performance.getEntriesByType('navigation')[0] || {}).responseStatus";

/// The picture at `file`, as GPUI draws it (BGRA), or none when there is none or it does not decode.
pub(super) fn load(file: &Path) -> Option<Arc<RenderImage>> {
    let bytes = std::fs::read(file).ok()?;
    let mut picture = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .ok()?
        .into_rgba8();
    for pixel in picture.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(picture),
        1,
    ))))
}

/// Keep `data` (a JPEG in base64, as the DevTools Protocol sends it) at `file`, then drop the oldest past [`KEEP`].
pub(super) fn save(file: &Path, data: &str) -> std::io::Result<()> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(std::io::Error::other)?;
    let folder = file.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    private_folder(folder)?;
    // A whole file or none: a picture read while it is written would not decode.
    let partial = file.with_extension("part");
    private_file(&partial)?.write_all(&bytes)?;
    std::fs::rename(&partial, file)?;
    prune(folder, KEEP);
    Ok(())
}

/// The folder, readable by this user only.
fn private_folder(folder: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)
}

/// A new file, readable by this user only.
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Remove all but the `keep` newest files in `folder`.
fn prune(folder: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then_some((metadata.modified().ok()?, entry.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, file) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(file);
    }
}

/// Whether a picture `taken` device pixels wide fits a page `width` device pixels wide.
pub(super) fn fits(taken: f32, width: f32) -> bool {
    width > 0. && (taken - width).abs() / width <= WIDTH_SLACK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_only_about_the_width_it_was_taken_at() {
        assert!(fits(1000., 1000.));
        assert!(fits(900., 1000.));
        assert!(!fits(700., 1000.));
        assert!(!fits(1400., 1000.));
        assert!(!fits(1000., 0.));
    }

    #[test]
    fn each_page_version_has_its_own_file() {
        assert_eq!(file_for("pg_1@2"), file_for("pg_1@2"));
        assert_ne!(file_for("pg_1@2"), file_for("pg_1@3"));
    }

    #[test]
    fn the_cache_keeps_only_the_newest() {
        let folder = std::env::temp_dir().join(format!("nexus-snapshots-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for index in 0..4 {
            let file = folder.join(format!("{index}.jpg"));
            std::fs::write(&file, b"x").unwrap();
            let at = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + index);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        prune(&folder, 2);
        let mut left: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["2.jpg", "3.jpg"]);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// A folder of its own for one test, empty.
    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("nexus-snapshots-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A `width` by `height` JPEG of one color, in base64 as the DevTools Protocol sends it.
    fn jpeg(width: u32, height: u32, color: [u8; 3]) -> String {
        use base64::Engine as _;
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(width, height, image::Rgb(color))
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    }

    #[test]
    fn a_saved_picture_loads_back_in_gpui_color_order() {
        let folder = scratch("round-trip");
        let file = file_in(&folder, "pg_1@1");
        save(&file, &jpeg(40, 20, [255, 0, 0])).unwrap();
        let picture = load(&file).expect("the picture loads");
        let size = picture.size(0);
        assert_eq!((size.width.0, size.height.0), (40, 20));
        // GPUI draws BGRA: a red picture has its red byte last.
        let pixel = &picture.as_bytes(0).unwrap()[..4];
        assert!(pixel[0] < 40 && pixel[1] < 40 && pixel[2] > 215, "{pixel:?}");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_missing_or_broken_picture_loads_as_none() {
        let folder = scratch("broken");
        assert!(load(&file_in(&folder, "pg_1@1")).is_none());
        let file = file_in(&folder, "pg_1@2");
        std::fs::write(&file, b"not a jpeg").unwrap();
        assert!(load(&file).is_none());
        // Bad base64 from the engine keeps nothing.
        assert!(save(&file_in(&folder, "pg_1@3"), "***").is_err());
        assert!(!file_in(&folder, "pg_1@3").exists());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn clearing_removes_every_picture_and_the_folder() {
        let folder = scratch("clear");
        save(&file_in(&folder, "pg_1@1"), &jpeg(8, 8, [0, 0, 255])).unwrap();
        save(&file_in(&folder, "pg_2@1"), &jpeg(8, 8, [0, 255, 0])).unwrap();
        clear_in(&folder);
        assert!(!folder.exists());
        // Nothing to clear is not a problem.
        clear_in(&folder);
    }

    #[cfg(unix)]
    #[test]
    fn pictures_and_their_folder_are_for_this_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let folder = scratch("private").join("page-snapshots");
        let file = file_in(&folder, "pg_1@1");
        save(&file, &jpeg(8, 8, [0, 0, 255])).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&folder), 0o700);
        assert_eq!(mode(&file), 0o600);
        std::fs::remove_dir_all(folder.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_picture_is_taken_only_while_the_page_is_at_its_own_address() {
        let opened = "https://frames.example/p/abc?sig=1";
        assert!(at_address(Some(opened), opened));
        // A fragment is the same page; a link, a sign-in redirect or the engine's error page is not.
        assert!(at_address(Some("https://frames.example/p/abc?sig=1#top"), opened));
        assert!(!at_address(Some("https://frames.example/p/other?sig=1"), opened));
        assert!(!at_address(Some("https://login.example/"), opened));
        assert!(!at_address(Some("chrome-error://chromewebdata/"), opened));
        assert!(!at_address(None, opened));
    }

    #[test]
    fn a_picture_is_taken_only_of_a_page_that_answered_with_success() {
        let status = |value: serde_json::Value| serde_json::json!({ "result": { "type": "number", "value": value } });
        assert!(loaded_ok(&status(200.into())));
        assert!(!loaded_ok(&status(403.into())));
        assert!(!loaded_ok(&status(500.into())));
        assert!(!loaded_ok(&status(0.into())));
        // No status (the engine did not say): no picture.
        assert!(!loaded_ok(&serde_json::json!({ "result": { "type": "undefined" } })));
    }

    #[test]
    fn a_new_version_has_no_picture_of_the_old_one() {
        let folder = scratch("versions");
        save(&file_in(&folder, "pg_1@1"), &jpeg(8, 8, [0, 0, 255])).unwrap();
        assert!(load(&file_in(&folder, "pg_1@1")).is_some());
        assert!(load(&file_in(&folder, "pg_1@2")).is_none());
        assert!(load(&file_in(&folder, "pg_2@1")).is_none());
        // Keys that join differently never meet: `pg_1@12` is not `pg_1@1` and `2`.
        assert!(load(&file_in(&folder, "pg_1@12")).is_none());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn saving_past_the_limit_drops_the_oldest_and_keeps_the_new_one() {
        let folder = scratch("limit");
        for index in 0..KEEP as u64 {
            let file = folder.join(format!("old-{index}.jpg"));
            std::fs::write(&file, b"x").unwrap();
            let at = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + index);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        let new = file_in(&folder, "pg_9@1");
        save(&new, &jpeg(8, 8, [0, 255, 0])).unwrap();
        let count = std::fs::read_dir(&folder).unwrap().count();
        assert_eq!(count, KEEP);
        assert!(new.exists());
        assert!(!folder.join("old-0.jpg").exists());
        assert!(folder.join("old-1.jpg").exists());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
