//! Inline images returned by tools, shared by both agent adapters.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

use crate::event::Image;
use crate::{IMAGE_TYPES, MAX_IMAGE_BYTES, MAX_IMAGES, decoded_len};

/// Keep raster images within the same limits as message attachments. Never fetch image URLs or read local paths.
pub(crate) fn extract(content: Option<&Value>) -> Vec<Image> {
    let mut images = Vec::new();
    let mut bytes = 0;
    for block in content.and_then(Value::as_array).into_iter().flatten() {
        if images.len() == MAX_IMAGES {
            break;
        }
        let Some((media_type, data)) = inline(block) else {
            continue;
        };
        let size = decoded_len(data);
        if !IMAGE_TYPES.contains(&media_type) || size == 0 || size > MAX_IMAGE_BYTES - bytes {
            continue;
        }
        if STANDARD.decode(data).is_err() {
            continue;
        }
        bytes += size;
        images.push(Image {
            media_type: media_type.into(),
            data: data.into(),
            name: None,
        });
    }
    images
}

fn inline(block: &Value) -> Option<(&str, &str)> {
    match block.get("type")?.as_str()? {
        "image" => {
            if let Some(source) = block.get("source") {
                if source.get("type")?.as_str()? != "base64" {
                    return None;
                }
                Some((source.get("media_type")?.as_str()?, source.get("data")?.as_str()?))
            } else {
                Some((block.get("mimeType")?.as_str()?, block.get("data")?.as_str()?))
            }
        }
        "inputImage" => {
            let url = block.get("imageUrl")?.as_str()?.strip_prefix("data:")?;
            url.split_once(";base64,")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_inline_agent_and_mcp_images_only() {
        let images = extract(Some(&json!([
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGk="}},
            {"type":"image","mimeType":"image/jpeg","data":"aGk="},
            {"type":"inputImage","imageUrl":"data:image/webp;base64,aGk="},
            {"type":"inputImage","imageUrl":"https://example.com/screenshot.png"},
            {"type":"inputImage","imageUrl":"file:///tmp/screenshot.png"},
            {"type":"image","source":{"type":"url","url":"https://example.com/a.png"}},
            {"type":"image","mimeType":"image/svg+xml","data":"aGk="},
            {"type":"image","mimeType":"image/png","data":"invalid"},
            {"type":"image","mimeType":"image/png","data":""}
        ])));
        assert_eq!(
            images.iter().map(|image| image.media_type.as_str()).collect::<Vec<_>>(),
            ["image/png", "image/jpeg", "image/webp"]
        );
        assert!(images.iter().all(|image| image.data == "aGk="));
    }

    #[test]
    fn bounds_the_count_and_total_bytes() {
        let block = json!({"type":"image","mimeType":"image/png","data":"aGk="});
        assert_eq!(extract(Some(&json!(vec![block; MAX_IMAGES + 1]))).len(), MAX_IMAGES);
        let data = STANDARD.encode(vec![0; MAX_IMAGE_BYTES / 2 + 1]);
        let block = json!({"type":"image","mimeType":"image/png","data":data});
        assert_eq!(extract(Some(&json!([block, block]))).len(), 1);
    }
}
