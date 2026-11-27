//! An organization's logo, drawn crisp at a small size. GPUI scales an image on the GPU without mipmaps, so a 512px
//! logo shrunk to 22px came out jagged. Here GPUI still fetches and decodes it; we shrink it once, with a Lanczos
//! filter, to the pixels it covers on this screen, and keep that.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::*;
use image::imageops::{FilterType, resize};
use smallvec::SmallVec;

/// The shrunk logos, by the original's id and the size in device pixels.
fn cache() -> &'static Mutex<HashMap<(ImageId, u32), Arc<RenderImage>>> {
    static CACHE: OnceLock<Mutex<HashMap<(ImageId, u32), Arc<RenderImage>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The logo at `url`, `size` logical pixels square (contained), sharp at the window's scale.
pub(super) fn logo(url: impl Into<SharedString>, size: f32) -> Img {
    let uri = SharedUri::from(url.into().to_string());
    img(ImageSource::Custom(Arc::new(move |window, cx| {
        let original = match window.use_asset::<ImgResourceLoader>(&Resource::Uri(uri.clone()), cx)? {
            Ok(original) => original,
            Err(error) => return Some(Err(error)),
        };
        let target = (size * window.scale_factor()).round().max(1.) as u32;
        let key = (original.id, target);
        let mut cache = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(shrunk) = cache.get(&key) {
            return Some(Ok(shrunk.clone()));
        }
        let shrunk = shrink(&original, target).unwrap_or(original);
        cache.insert(key, shrunk.clone());
        Some(Ok(shrunk))
    })))
    .size(px(size))
    .flex_none()
    .object_fit(ObjectFit::Contain)
}

/// The image's first frame, its longer side `target` pixels; None when it is already that small.
fn shrink(image: &RenderImage, target: u32) -> Option<Arc<RenderImage>> {
    let dimensions = image.size(0);
    let (width, height) = (dimensions.width.0 as u32, dimensions.height.0 as u32);
    let longer = width.max(height);
    if longer <= target || width == 0 || height == 0 {
        return None;
    }
    let scaled = |side: u32| ((side as f32 * target as f32 / longer as f32).round() as u32).max(1);
    // BGRA: the filter treats each channel alike, so the order needs no swap.
    let buffer = image::RgbaImage::from_raw(width, height, image.as_bytes(0)?.to_vec())?;
    let small = resize(&buffer, scaled(width), scaled(height), FilterType::Lanczos3);
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(small),
        1,
    ))))
}
