//! ⌃v: the image on the clipboard, as PNG bytes, read with the platform's own tools: `wl-paste` (Wayland) or `xclip`
//! (X11) on Linux, `pngpaste` or AppleScript on macOS, PowerShell on Windows. A pasted path to an image file is taken
//! as that image too (`image_file`).

use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine as _;
use nexus_client::base::images::MAX_IMAGE_BYTES;
use serde_json::{Value, json};

/// The image on the clipboard as `{data, name}` (base64), as chat and the agent take it, or why there is none.
pub fn image() -> Result<Value, String> {
    let bytes = read_png().ok_or("There is no image on the clipboard.")?;
    as_attachment(&bytes, "pasted.png")
}

/// A pasted path to a PNG, JPEG, GIF or WebP file, as that image. `None` when the text is not one.
pub fn image_file(text: &str) -> Option<Result<Value, String>> {
    let text = text.trim().trim_matches(|c| c == '\'' || c == '"');
    let path = Path::new(text.strip_prefix("file://").unwrap_or(text));
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") || !path.is_file() {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    Some(
        std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| as_attachment(&bytes, &name)),
    )
}

fn as_attachment(bytes: &[u8], name: &str) -> Result<Value, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "The image is {:.1} MB; an image may be at most {} MB.",
            bytes.len() as f64 / 1_048_576.0,
            MAX_IMAGE_BYTES >> 20
        ));
    }
    Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(bytes), "name": name }))
}

/// The first tool that answers with PNG bytes.
fn read_png() -> Option<Vec<u8>> {
    let tries: &[&[&str]] = if cfg!(target_os = "macos") {
        &[
            &["pngpaste", "-"],
            &[
                "osascript",
                "-e",
                "set p to (POSIX path of (path to temporary items)) & \"nexus-paste.png\"",
                "-e",
                "set f to open for access (POSIX file p) with write permission",
                "-e",
                "set eof f to 0",
                "-e",
                "write (the clipboard as «class PNGf») to f",
                "-e",
                "close access f",
                "-e",
                "do shell script \"cat \" & quoted form of p",
            ],
        ]
    } else if cfg!(windows) {
        &[&[
            "powershell",
            "-NoProfile",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; $i=[Windows.Forms.Clipboard]::GetImage(); if($i){$m=New-Object IO.MemoryStream; $i.Save($m,[Drawing.Imaging.ImageFormat]::Png); [Console]::OpenStandardOutput().Write($m.ToArray(),0,$m.Length)}",
        ]]
    } else {
        &[
            &["wl-paste", "--no-newline", "--type", "image/png"],
            &["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
        ]
    };
    tries.iter().find_map(|args| {
        let output = Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        // A PNG starts with its signature; anything else (text, an error) is not an image.
        (output.status.success() && output.stdout.starts_with(b"\x89PNG")).then_some(output.stdout)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_image_path_is_that_image() {
        let dir = std::env::temp_dir().join(format!("nexus-clipboard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("folder");
        let path = dir.join("shot.png");
        std::fs::write(&path, b"\x89PNG fake").expect("file");
        let pasted = image_file(&format!("'{}'", path.display())).and_then(Result::ok);
        assert_eq!(
            pasted.as_ref().and_then(|image| image["name"].as_str()),
            Some("shot.png")
        );
        assert!(image_file("just some text").is_none());
        assert!(image_file(&dir.join("missing.png").display().to_string()).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
