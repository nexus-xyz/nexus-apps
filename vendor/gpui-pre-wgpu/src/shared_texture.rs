//! Textures made outside the renderer that a window draws with `Window::paint_surface`: a web page's frame imported
//! from another process, for one. Added for Nexus (`desktop-gpui/vendor/README.md`); not in upstream gpui-pre.

use std::any::Any;
use std::fmt;
use std::sync::{Arc, Mutex};

/// The device and queue of the newest renderer context, for code that makes textures off the main thread.
static CURRENT: Mutex<Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)>> = Mutex::new(None);

/// The device windows draw with, and its queue: make a [`SharedTexture`]'s texture on it. `None` before the first
/// window, and on a backend other than native Vulkan, GL or Metal.
pub fn shared_device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    CURRENT.lock().ok()?.clone()
}

/// Called when a context is made, again after a lost device is replaced.
pub(crate) fn publish(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) {
    if let Ok(mut current) = CURRENT.lock() {
        *current = Some((device.clone(), queue.clone()));
    }
}

/// A texture to draw as a surface. It must be made on [`shared_device`]'s device, sampleable
/// (`TEXTURE_BINDING`), in a filterable colour format with premultiplied alpha. A window whose device is no longer
/// that one skips it rather than fail.
pub struct SharedTexture {
    pub(crate) texture: wgpu::Texture,
    pub(crate) device: Arc<wgpu::Device>,
    /// Dropped with the texture, after the last frame that draws it is built: a lease on memory another process
    /// writes, say, which hands it back on drop.
    _owner: Option<Box<dyn Any + Send + Sync>>,
}

impl SharedTexture {
    pub fn new(texture: wgpu::Texture, device: Arc<wgpu::Device>) -> Self {
        Self {
            texture,
            device,
            _owner: None,
        }
    }

    /// As [`SharedTexture::new`], with `owner` dropped when this is. The renderer may still be drawing with the
    /// texture then: an owner that lets other code write the texture's memory must wait for the device's queue first.
    pub fn with_owner(texture: wgpu::Texture, device: Arc<wgpu::Device>, owner: impl Any + Send + Sync) -> Self {
        Self {
            texture,
            device,
            _owner: Some(Box::new(owner)),
        }
    }

    /// Its size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}

impl fmt::Debug for SharedTexture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedTexture")
            .field("texture", &self.texture)
            .field("owned", &self._owner.is_some())
            .finish()
    }
}
