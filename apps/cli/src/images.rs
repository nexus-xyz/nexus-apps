//! `nexus chat sessions images` and `nexus artifacts images` (`images.ts`): the images attached to a conversation's
//! questions, saved to files. Every image of every question, or the ones named by id. Each file takes the image's own
//! name, without directories and with an extension that matches its type, else `<id>.<ext>`; two images with one name
//! get `-2`, `-3`. A file that is already there stops the command before anything is written, unless `--force`.

use std::collections::HashSet;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::error::{CliError, Result, usage};
use crate::json::{a, s};

/// The longest name kept, in characters; a longer one gives way to `<id>.<ext>`.
const MAX_NAME: usize = 100;

/// `1600 × 1000, 312 KB` (`imageDetail` in images.ts): an image's pixels, when known, and its size.
pub fn image_detail(image: &Value) -> String {
    let pixels = match (
        image.get("width").and_then(Value::as_i64),
        image.get("height").and_then(Value::as_i64),
    ) {
        (Some(width), Some(height)) if width > 0 && height > 0 => format!("{width} × {height}, "),
        _ => String::new(),
    };
    let bytes = image.get("bytes").and_then(Value::as_f64).unwrap_or(0.0);
    let size = if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / (1024.0 * 1024.0))
    } else {
        format!("{} KB", (bytes / 1024.0).round().max(1.0))
    };
    format!("{pixels}{size}")
}

/// The extension of a type Gateway takes (PNG, JPEG, GIF, WebP).
fn extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// A name's last path segment, with the characters a file system refuses replaced and no dot or space at either end.
fn clean(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let replaced: String = last
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    replaced.trim().trim_matches('.').trim().to_string()
}

/// The file an image is saved as (`fileName` in images.ts): its own name, cleaned, else `<id>.<ext>`; with `.<ext>`
/// added when its extension is not its type's.
pub fn file_name(image: &Value) -> String {
    let ext = extension(s(image, "mediaType").unwrap_or(""));
    let own = clean(s(image, "name").unwrap_or(""));
    let name = if own.is_empty() || own.chars().count() > MAX_NAME {
        match clean(s(image, "id").unwrap_or("")) {
            id if id.is_empty() => "image".to_string(),
            id => id,
        }
    } else {
        own
    };
    let lower = name.to_lowercase();
    let typed = lower.ends_with(&format!(".{ext}")) || (ext == "jpg" && lower.ends_with(".jpeg"));
    if typed { name } else { format!("{name}.{ext}") }
}

/// `name`, or `name-2.ext`, `name-3.ext`… when an earlier image took it (case aside, as macOS compares names).
pub fn unique(name: &str, taken: &mut HashSet<String>) -> String {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let mut candidate = name.to_string();
    let mut next = 2;
    while !taken.insert(candidate.to_lowercase()) {
        candidate = format!("{stem}-{next}.{ext}");
        next += 1;
    }
    candidate
}

/// Save the images of a conversation's questions to files. `read` answers the conversation (a session or an artifact,
/// with its `id` and `messages`), `fetch` one image with its bytes by its id, and `wanted` narrows the images to these
/// ids. `named` is what the command was given, for a conversation that does not say its id.
pub fn save_images(
    cmd: &Cmd,
    named: &str,
    wanted: &[String],
    read: impl FnOnce() -> Result<Value>,
    fetch: impl Fn(&str) -> Result<Value>,
) -> Result<i32> {
    let style = cmd.style;
    if cmd.has("out") {
        return usage("--out needs a directory.", None);
    }
    let conversation = read()?;
    let id = s(&conversation, "id").unwrap_or(named);
    let images: Vec<&Value> = a(&conversation, "messages")
        .iter()
        .flat_map(|message| a(message, "attachments"))
        .filter(|image| wanted.is_empty() || wanted.iter().any(|want| s(image, "id") == Some(want.as_str())))
        .collect();
    if let Some(missing) = wanted
        .iter()
        .find(|want| !images.iter().any(|image| s(image, "id") == Some(want.as_str())))
    {
        return Err(CliError::hint(
            format!("No image {missing} in {id}."),
            "Leave out the image ids to save every image; with --json, each one's id is listed.",
        )
        .into());
    }
    if images.is_empty() {
        if cmd.json {
            return Ok(cmd.print(&json!({ "id": id, "images": [] })));
        }
        cmd.out(format!("No images in {id}."));
        return Ok(0);
    }

    let out = cmd.args.flag_set("out");
    let mut taken = HashSet::new();
    let targets: Vec<String> = images
        .iter()
        .map(|image| {
            let name = unique(&file_name(image), &mut taken);
            match out {
                Some(dir) => format!("{}/{name}", dir.trim_end_matches('/')),
                None => name,
            }
        })
        .collect();
    // Anything there, a dangling link too: checked for every image before the first is written.
    if !cmd.has("force")
        && let Some(existing) = targets
            .iter()
            .find(|target| std::fs::symlink_metadata(cmd.ctx.cwd.join(target)).is_ok())
    {
        return Err(CliError::hint(
            format!("{existing} already exists."),
            "--force overwrites it; --out saves the images in another directory.",
        )
        .into());
    }
    if let Some(dir) = out {
        std::fs::create_dir_all(cmd.ctx.cwd.join(dir))
            .map_err(|error| CliError::new(format!("Could not make {dir}: {error}")))?;
    }

    let mut written = Vec::with_capacity(images.len());
    for (image, target) in images.iter().zip(&targets) {
        let image_id = s(image, "id").unwrap_or("");
        let answer = fetch(image_id)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s(&answer, "data").unwrap_or(""))
            .map_err(|_| CliError::new(format!("Gateway sent image {image_id} in a form this CLI cannot read.")))?;
        std::fs::write(cmd.ctx.cwd.join(target), &bytes)
            .map_err(|error| CliError::new(format!("Could not write {target}: {error}")))?;
        if !cmd.json {
            cmd.out(format!("{} Wrote {target} ({}).", style.ok, image_detail(image)));
        }
        written.push(json!({
            "id": image_id,
            "messageId": s(&answer, "messageId").unwrap_or(""),
            "path": target,
            "mediaType": s(image, "mediaType").unwrap_or(""),
            "bytes": bytes.len(),
        }));
    }
    if cmd.json {
        return Ok(cmd.print(&json!({ "id": id, "images": written })));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_file_by_the_images_own_name_made_safe() {
        let named =
            |name: &str, media_type: &str| file_name(&json!({ "id": "cha_1", "mediaType": media_type, "name": name }));
        assert_eq!(named("shot.png", "image/png"), "shot.png");
        assert_eq!(named("Photo.JPEG", "image/jpeg"), "Photo.JPEG");
        // No directories, no hidden file, nothing a file system refuses.
        assert_eq!(named("../../.ssh/authorized_keys", "image/png"), "authorized_keys.png");
        assert_eq!(named("C:\\Users\\ada\\shot.png", "image/png"), "shot.png");
        assert_eq!(named(".zshrc", "image/gif"), "zshrc.gif");
        assert_eq!(named("what? <now>.webp", "image/webp"), "what_ _now_.webp");
        // An extension that is not the type's gets the type's.
        assert_eq!(named("shot", "image/png"), "shot.png");
        assert_eq!(named("photo.heic", "image/jpeg"), "photo.heic.jpg");
        // No usable name: the id.
        assert_eq!(named("..", "image/png"), "cha_1.png");
        assert_eq!(named(&"x".repeat(MAX_NAME + 1), "image/png"), "cha_1.png");
        assert_eq!(
            file_name(&json!({ "id": "cha_2", "mediaType": "image/jpeg" })),
            "cha_2.jpg"
        );
    }

    #[test]
    fn a_name_taken_earlier_gets_a_number() {
        let mut taken = HashSet::new();
        assert_eq!(unique("shot.png", &mut taken), "shot.png");
        assert_eq!(unique("Shot.png", &mut taken), "Shot-2.png");
        assert_eq!(unique("shot.png", &mut taken), "shot-3.png");
        assert_eq!(unique("cha_1.png", &mut taken), "cha_1.png");
    }

    #[test]
    fn describes_an_image_by_its_pixels_and_size() {
        assert_eq!(
            image_detail(&json!({ "bytes": 319_488, "width": 1600, "height": 1000 })),
            "1600 × 1000, 312 KB"
        );
        assert_eq!(image_detail(&json!({ "bytes": 3_145_728 })), "3.0 MB");
        assert_eq!(image_detail(&json!({ "bytes": 10 })), "1 KB");
    }
}
