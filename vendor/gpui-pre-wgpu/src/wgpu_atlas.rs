// Modified for gpui-pre (snapshot of zed@f816e1f): adapt device queues and sRGB detection to wgpu 30.0.1.
use anyhow::{Context as _, Result};
use etagere::{BucketedAtlasAllocator, size2};
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTextureList,
    AtlasTile, Bounds, DevicePixels, PlatformAtlas, Point, Size,
};
use parking_lot::Mutex;
use std::{borrow::Cow, ops, sync::Arc};

use crate::WgpuContext;

fn device_size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    size2(size.width.0, size.height.0)
}

fn etagere_point_to_device(point: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels(point.x),
        y: DevicePixels(point.y),
    }
}

pub struct WgpuAtlas(Mutex<AtlasState<WgpuAtlasTextures>>);

struct PendingUpload {
    id: AtlasTextureId,
    bounds: Bounds<DevicePixels>,
    data: Vec<u8>,
}

struct WgpuAtlasTextures {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    max_texture_size: u32,
    color_texture_format: wgpu::TextureFormat,
    storage: WgpuAtlasStorage,
    pending_uploads: Vec<PendingUpload>,
    next_texture_generation: u64,
}

pub struct WgpuTextureInfo {
    pub view: wgpu::TextureView,
    /// Distinguishes new textures that reuse an [`AtlasTextureId`].
    pub generation: u64,
}

impl WgpuAtlas {
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        color_texture_format: wgpu::TextureFormat,
    ) -> Self {
        let max_texture_size = device.limits().max_texture_dimension_2d;
        WgpuAtlas(Mutex::new(AtlasState::new(WgpuAtlasTextures {
            device,
            queue,
            max_texture_size,
            color_texture_format,
            storage: WgpuAtlasStorage::default(),
            pending_uploads: Vec::new(),
            next_texture_generation: 0,
        })))
    }

    pub fn from_context(context: &WgpuContext) -> Self {
        Self::new(
            context.device.clone(),
            context.queue.clone(),
            context.color_texture_format(),
        )
    }

    pub fn before_frame(&self) {
        let mut lock = self.0.lock();
        lock.backend.flush_uploads();
    }

    /// Returns the view backing `id`, or `None` once every tile in it has been
    /// removed. A scene can still reference such a texture when a cached view
    /// replays a paint from before the image was dropped, so callers must skip
    /// those sprites rather than assume the texture exists.
    pub fn get_texture_info(&self, id: AtlasTextureId) -> Option<WgpuTextureInfo> {
        let lock = self.0.lock();
        let texture = lock.backend.storage.get(id)?;
        Some(WgpuTextureInfo {
            view: texture.view.clone(),
            generation: texture.generation,
        })
    }

    /// Clears all cached textures and tiles, forcing them to be recreated.
    /// Use this for incremental recovery when the device is still valid.
    pub fn clear(&self) {
        self.0.lock().clear(|textures| {
            textures.storage = WgpuAtlasStorage::default();
            textures.pending_uploads.clear();
        });
    }

    /// Handles device lost by clearing all textures and cached tiles.
    /// The atlas will lazily recreate textures as needed on subsequent frames.
    pub fn handle_device_lost(&self, context: &WgpuContext) {
        self.0.lock().clear(|textures| {
            textures.device = context.device.clone();
            textures.queue = context.queue.clone();
            textures.color_texture_format = context.color_texture_format();
            textures.storage = WgpuAtlasStorage::default();
            textures.pending_uploads.clear();
        });
    }
}

impl PlatformAtlas for WgpuAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }

    // Nexus: see `PlatformAtlas::update`. `AtlasState` finds the tile and checks the rect and length.
    fn update(&self, key: &AtlasKey, rect: Bounds<DevicePixels>, bytes: &[u8]) -> Result<bool> {
        self.0.lock().update(key, rect, bytes)
    }
}

impl AtlasBackend for WgpuAtlasTextures {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        let tile = self.allocate(size, kind).context("failed to allocate")?;
        self.upload_texture(tile.texture_id, tile.bounds, bytes);
        Ok(tile)
    }

    fn remove(&mut self, tile: AtlasTile) {
        let id = tile.texture_id;
        // Nexus: nothing queued for this tile will be seen again. The caller that got `Ok(false)` from `update`
        // because the queue was full drops its image next, and this is what empties the queue.
        self.pending_uploads
            .retain(|upload| upload.id != id || !bounds_contain(tile.bounds, upload.bounds));
        let Some(texture_slot) = self.storage[id.kind].textures.get_mut(id.index as usize) else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.allocator.deallocate(tile.tile_id.into());
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                self.pending_uploads
                    .retain(|upload| upload.id != texture.id);
                self.storage[id.kind]
                    .free_list
                    .push(texture.id.index as usize);
            } else {
                *texture_slot = Some(texture);
            }
        }
    }

    /// Nexus: queued as `insert`'s upload is, with the same swizzle to the texture's format, so `before_frame`
    /// writes it with `queue.write_texture` before the next frame's draws, after the uploads queued before it.
    fn update(
        &mut self,
        tile: AtlasTile,
        bounds: Bounds<DevicePixels>,
        bytes: &[u8],
    ) -> Result<bool> {
        if self.storage.get(tile.texture_id).is_none() {
            return Ok(false);
        }
        // A queued upload inside `bounds` would be overwritten by this one, so it goes.
        self.pending_uploads.retain(|upload| {
            upload.id != tile.texture_id || !bounds_contain(bounds, upload.bounds)
        });
        // Uploads that only part-overlap stay, in order. While the window draws no frames (hidden) and its image
        // keeps changing (a marquee, a growing progress bar), they would pile up. The atlas keeps no copy of a
        // tile's pixels, so it can't merge them into one upload. Past a bound it refuses, and the caller makes a
        // new image and drops this one, which empties the queue for the tile (`remove`).
        let bytes_per_pixel = self
            .storage
            .get(tile.texture_id)
            .map_or(4, |texture| usize::from(texture.bytes_per_pixel()));
        let tile_bytes = tile.bounds.size.width.0.max(0) as usize
            * tile.bounds.size.height.0.max(0) as usize
            * bytes_per_pixel;
        let (queued, queued_bytes) = self
            .pending_uploads
            .iter()
            .filter(|upload| {
                upload.id == tile.texture_id && bounds_contain(tile.bounds, upload.bounds)
            })
            .fold((0, 0), |(count, total), upload| {
                (count + 1, total + upload.data.len())
            });
        if queued >= MAX_QUEUED_UPLOADS_PER_TILE
            || queued_bytes.saturating_add(bytes.len()) > 2 * tile_bytes
        {
            return Ok(false);
        }
        self.upload_texture(tile.texture_id, bounds, bytes);
        Ok(true)
    }
}

/// Nexus: the most uploads that wait in the queue for one tile before `update` refuses, and (twice the tile's bytes)
/// the most bytes. The tile's first upload and one full rewrite fit in the bytes. This is a bound on memory while no
/// frame draws: a whole tile of 1-pixel uploads is far more than this in headers.
const MAX_QUEUED_UPLOADS_PER_TILE: usize = 64;

/// Nexus: whether `inner` lies inside `outer`. (`Bounds::is_contained_within` says no for equal bounds.)
fn bounds_contain(outer: Bounds<DevicePixels>, inner: Bounds<DevicePixels>) -> bool {
    let (outer_end, inner_end) = (outer.bottom_right(), inner.bottom_right());
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner_end.x <= outer_end.x
        && inner_end.y <= outer_end.y
}

impl WgpuAtlasTextures {
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let textures = &mut self.storage[texture_kind];

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Some(tile);
            }
        }

        let texture = self.push_texture(size, texture_kind);
        texture.allocate(size)
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> &mut WgpuAtlasTexture {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        let max_texture_size = self.max_texture_size as i32;
        let max_atlas_size = Size {
            width: DevicePixels(max_texture_size),
            height: DevicePixels(max_texture_size),
        };

        let size = min_size.min(&max_atlas_size).max(&DEFAULT_ATLAS_SIZE);
        let format = match kind {
            AtlasTextureKind::Monochrome => wgpu::TextureFormat::R8Unorm,
            AtlasTextureKind::Subpixel | AtlasTextureKind::Polychrome => self.color_texture_format,
        };

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: size.width.0 as u32,
                height: size.height.0 as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // Nexus: tests read the texture back (`update_reaches_the_texture`).
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | if cfg!(test) {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let texture_list = &mut self.storage[kind];
        let index = texture_list.free_list.pop();
        let generation = self.next_texture_generation;
        self.next_texture_generation = self.next_texture_generation.wrapping_add(1);

        let atlas_texture = WgpuAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            generation,
            allocator: BucketedAtlasAllocator::new(device_size_to_etagere(size)),
            format,
            texture,
            view,
            live_atlas_keys: 0,
        };

        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list
                .textures
                .get_mut(ix)
                .and_then(|t| t.as_mut())
                .expect("texture must exist")
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list
                .textures
                .last_mut()
                .and_then(|t| t.as_mut())
                .expect("texture must exist")
        }
    }

    fn upload_texture(&mut self, id: AtlasTextureId, bounds: Bounds<DevicePixels>, bytes: &[u8]) {
        let data = self
            .storage
            .get(id)
            .map(|texture| swizzle_upload_data(bytes, texture.format))
            .unwrap_or_else(|| bytes.to_vec());

        self.pending_uploads
            .push(PendingUpload { id, bounds, data });
    }

    fn flush_uploads(&mut self) {
        for upload in self.pending_uploads.drain(..) {
            let Some(texture) = self.storage.get(upload.id) else {
                continue;
            };
            let bytes_per_pixel = texture.bytes_per_pixel();

            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: upload.bounds.origin.x.0 as u32,
                        y: upload.bounds.origin.y.0 as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.bounds.size.width.0 as u32 * bytes_per_pixel as u32),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: upload.bounds.size.width.0 as u32,
                    height: upload.bounds.size.height.0 as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
}

#[derive(Default)]
struct WgpuAtlasStorage {
    monochrome_textures: AtlasTextureList<WgpuAtlasTexture>,
    subpixel_textures: AtlasTextureList<WgpuAtlasTexture>,
    polychrome_textures: AtlasTextureList<WgpuAtlasTexture>,
}

impl ops::Index<AtlasTextureKind> for WgpuAtlasStorage {
    type Output = AtlasTextureList<WgpuAtlasTexture>;
    fn index(&self, kind: AtlasTextureKind) -> &Self::Output {
        match kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Subpixel => &self.subpixel_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
        }
    }
}

impl ops::IndexMut<AtlasTextureKind> for WgpuAtlasStorage {
    fn index_mut(&mut self, kind: AtlasTextureKind) -> &mut Self::Output {
        match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
        }
    }
}

impl WgpuAtlasStorage {
    fn get(&self, id: AtlasTextureId) -> Option<&WgpuAtlasTexture> {
        self[id.kind]
            .textures
            .get(id.index as usize)
            .and_then(|t| t.as_ref())
    }
}

struct WgpuAtlasTexture {
    id: AtlasTextureId,
    generation: u64,
    allocator: BucketedAtlasAllocator,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    live_atlas_keys: u32,
}

impl WgpuAtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(device_size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            padding: 0,
            bounds: Bounds {
                origin: etagere_point_to_device(allocation.rectangle.min),
                size,
            },
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn bytes_per_pixel(&self) -> u8 {
        match self.format {
            wgpu::TextureFormat::R8Unorm => 1,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm => 4,
            _ => 4,
        }
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&self) -> bool {
        self.live_atlas_keys == 0
    }
}

fn swizzle_upload_data(bytes: &[u8], format: wgpu::TextureFormat) -> Vec<u8> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => {
            let mut data = bytes.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            data
        }
        _ => bytes.to_vec(),
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use gpui::block_on;
    use gpui::{ImageId, RenderImageParams};
    use std::sync::Arc;

    fn test_device_and_queue() -> anyhow::Result<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
        block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                flags: wgpu::InstanceFlags::default(),
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: None,
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .map_err(|error| anyhow::anyhow!("failed to request adapter: {error}"))?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("wgpu_atlas_test_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits())
                        .using_alignment(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                })
                .await
                .map_err(|error| anyhow::anyhow!("failed to request device: {error}"))?;
            Ok((Arc::new(device), Arc::new(queue)))
        })
    }

    #[test]
    fn before_frame_skips_uploads_for_removed_texture() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;

        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1),
            frame_index: 0,
        });
        let size = Size {
            width: DevicePixels(1),
            height: DevicePixels(1),
        };
        let mut build = || Ok(Some((size, Cow::Owned(vec![0, 0, 0, 255]))));

        // Regression test: before the fix, this panicked in flush_uploads
        atlas
            .get_or_insert_with(key.clone(), &mut build)?
            .expect("tile should be created");
        atlas.remove(&key);
        atlas.before_frame();
        Ok(())
    }

    #[test]
    fn remove_deallocates_tile_space_for_reuse() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);

        let small = Size {
            width: DevicePixels(64),
            height: DevicePixels(64),
        };
        let big = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };

        let make_key = |image_id: usize| {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(image_id),
                frame_index: 0,
            })
        };
        let insert = |key: AtlasKey, size: Size<DevicePixels>| {
            let byte_count = (size.width.0 as usize) * (size.height.0 as usize) * 4;
            atlas
                .get_or_insert_with(key, &mut || {
                    Ok(Some((size, Cow::Owned(vec![0u8; byte_count]))))
                })
                .expect("allocation should succeed")
                .expect("callback returns Some")
        };

        let keeper_key = make_key(1);
        let big_key_a = make_key(2);
        let big_key_b = make_key(3);

        let keeper_tile = insert(keeper_key, small);
        let tile_a = insert(big_key_a.clone(), big);
        assert_eq!(keeper_tile.texture_id, tile_a.texture_id);

        atlas.remove(&big_key_a);
        let tile_b = insert(big_key_b, big);
        assert_eq!(tile_b.texture_id, keeper_tile.texture_id);
        Ok(())
    }

    #[test]
    fn reused_texture_id_has_new_generation() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let size = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };
        let make_key = |image_id| {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(image_id),
                frame_index: 0,
            })
        };
        let insert = |key: AtlasKey| {
            atlas
                .get_or_insert_with(key, &mut || {
                    Ok(Some((
                        size,
                        Cow::Owned(vec![0; size.width.0 as usize * size.height.0 as usize * 4]),
                    )))
                })
                .expect("allocation should succeed")
                .expect("callback returns Some")
        };

        let first_key = make_key(1);
        let first_tile = insert(first_key.clone());
        let first_generation = atlas
            .get_texture_info(first_tile.texture_id)
            .context("first texture should exist")?
            .generation;
        atlas.remove(&first_key);
        assert!(atlas.get_texture_info(first_tile.texture_id).is_none());

        let second_tile = insert(make_key(2));
        let second_generation = atlas
            .get_texture_info(second_tile.texture_id)
            .context("second texture should exist")?
            .generation;

        assert_eq!(second_tile.texture_id, first_tile.texture_id);
        assert_ne!(second_generation, first_generation);
        Ok(())
    }

    // Nexus: `update`'s tests.

    fn image_key(image_id: usize) -> AtlasKey {
        AtlasKey::Image(RenderImageParams {
            image_id: ImageId(image_id),
            frame_index: 0,
        })
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Bounds<DevicePixels> {
        Bounds {
            origin: Point {
                x: DevicePixels(x),
                y: DevicePixels(y),
            },
            size: Size {
                width: DevicePixels(width),
                height: DevicePixels(height),
            },
        }
    }

    /// BGRA pixels, each different from the others and from `seed`'s other values.
    fn pixels(seed: u8, count: usize) -> Vec<u8> {
        (0..count as u8)
            .flat_map(|i| [seed.wrapping_add(i), 0x40 + i, 0x80 + i, 0xF0])
            .collect()
    }

    fn insert_image(
        atlas: &WgpuAtlas,
        key: AtlasKey,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> anyhow::Result<AtlasTile> {
        atlas
            .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.to_vec())))))?
            .context("callback returns Some")
    }

    /// The tile's pixels as the texture holds them, rows tightly packed.
    fn read_tile(
        atlas: &WgpuAtlas,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tile: AtlasTile,
    ) -> anyhow::Result<Vec<u8>> {
        let width = tile.bounds.size.width.0 as u32;
        let height = tile.bounds.size.height.0 as u32;
        let row = width * 4;
        let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atlas_test_readback"),
            size: u64::from(padded_row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let lock = atlas.0.lock();
            let texture = lock
                .backend
                .storage
                .get(tile.texture_id)
                .context("tile's texture")?;
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: tile.bounds.origin.x.0 as u32,
                        y: tile.bounds.origin.y.0 as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let submission = queue.submit(std::iter::once(encoder.finish()));
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .map_err(|error| anyhow::anyhow!("failed to wait for the readback: {error}"))?;
        receiver.recv_timeout(std::time::Duration::from_secs(30))??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        Ok(mapped
            .chunks_exact(padded_row as usize)
            .flat_map(|padded| &padded[..row as usize])
            .copied()
            .collect())
    }

    #[test]
    fn update_reaches_the_texture() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        for format in [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ] {
            let atlas = WgpuAtlas::new(device.clone(), queue.clone(), format);
            // A keeper first, so the image's tile is not at (0, 0) of its texture.
            insert_image(&atlas, image_key(1), rect(0, 0, 3, 2).size, &pixels(0, 6))?;
            let key = image_key(2);
            let mut image = pixels(0x10, 4 * 3);
            let tile = insert_image(&atlas, key.clone(), rect(0, 0, 4, 3).size, &image)?;
            assert_ne!(tile.bounds.origin, Point::default());
            atlas.before_frame();

            let patch = pixels(0xA0, 2 * 2);
            assert!(atlas.update(&key, rect(1, 1, 2, 2), &patch)?);
            atlas.before_frame();

            for (patch_row, image_row) in patch.chunks_exact(2 * 4).zip(1..3) {
                let start = (image_row * 4 + 1) * 4;
                image[start..start + 2 * 4].copy_from_slice(patch_row);
            }
            assert_eq!(
                read_tile(&atlas, &device, &queue, tile)?,
                swizzle_upload_data(&image, format),
                "{format:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn update_before_the_first_flush_lands_after_the_insert() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Bgra8Unorm,
        );
        let key = image_key(1);
        let tile = insert_image(&atlas, key.clone(), rect(0, 0, 2, 1).size, &pixels(0, 2))?;
        let mut image = pixels(0, 2);
        let patch = pixels(0xA0, 1);
        assert!(atlas.update(&key, rect(1, 0, 1, 1), &patch)?);
        atlas.before_frame();
        image[4..].copy_from_slice(&patch);
        assert_eq!(read_tile(&atlas, &device, &queue, tile)?, image);
        Ok(())
    }

    #[test]
    fn covered_uploads_leave_the_queue() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);
        insert_image(&atlas, image_key(2), rect(0, 0, 4, 4).size, &pixels(0, 16))?;
        insert_image(&atlas, key.clone(), rect(0, 0, 4, 4).size, &pixels(0, 16))?;
        // Frames that never draw: each full update replaces the queued ones for this tile.
        for _ in 0..100 {
            assert!(atlas.update(&key, rect(0, 0, 4, 4), &pixels(0x20, 16))?);
            assert!(atlas.update(&key, rect(1, 1, 1, 1), &pixels(0x30, 1))?);
        }
        // The other tile's upload, the last full update and the small one after it.
        assert_eq!(atlas.0.lock().backend.pending_uploads.len(), 3);
        Ok(())
    }

    /// Uploads waiting for `key`'s tile.
    fn queued_for(atlas: &WgpuAtlas, tile: &AtlasTile) -> usize {
        atlas
            .0
            .lock()
            .backend
            .pending_uploads
            .iter()
            .filter(|upload| {
                upload.id == tile.texture_id && bounds_contain(tile.bounds, upload.bounds)
            })
            .count()
    }

    /// 1 x 1 updates at 200 different pixels of a 32 x 32 tile: none lies inside another, so none drops another.
    fn update_one_pixel_at(atlas: &WgpuAtlas, key: &AtlasKey, step: i32) -> anyhow::Result<bool> {
        atlas.update(key, rect(step % 32, step / 32, 1, 1), &pixels(0x20, 1))
    }

    /// Updates that only part-overlap stay queued, so while no frame draws the queue for a tile would grow. It stops:
    /// `update` says `Ok(false)`, and dropping the image empties the queue.
    #[test]
    fn the_queue_for_a_tile_has_a_bound() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);
        let tile = insert_image(
            &atlas,
            key.clone(),
            rect(0, 0, 32, 32).size,
            &vec![0; 32 * 32 * 4],
        )?;
        atlas.before_frame();
        // Frames that never draw from here on. Another tile's upload waits in the queue too.
        insert_image(&atlas, image_key(2), rect(0, 0, 2, 2).size, &pixels(0, 4))?;
        let others = atlas.0.lock().backend.pending_uploads.len();
        assert_eq!(others, 1);

        let mut accepted = 0;
        while accepted < 200 && update_one_pixel_at(&atlas, &key, accepted)? {
            accepted += 1;
        }
        assert_eq!(accepted, MAX_QUEUED_UPLOADS_PER_TILE as i32);
        assert_eq!(queued_for(&atlas, &tile), MAX_QUEUED_UPLOADS_PER_TILE);
        // It stays refused, and the queue stays where it is, however often it is asked.
        for step in 0..100 {
            assert!(!update_one_pixel_at(&atlas, &key, 300 + step)?);
        }
        assert_eq!(queued_for(&atlas, &tile), MAX_QUEUED_UPLOADS_PER_TILE);

        // The caller of a refused update drops the image: the tile's queue goes, another tile's does not.
        atlas.remove(&key);
        assert_eq!(queued_for(&atlas, &tile), 0);
        assert_eq!(atlas.0.lock().backend.pending_uploads.len(), others);
        Ok(())
    }

    /// A frame writes the queue, and the tile takes updates again.
    #[test]
    fn a_frame_empties_the_queue_for_a_tile() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);
        let tile = insert_image(
            &atlas,
            key.clone(),
            rect(0, 0, 32, 32).size,
            &vec![0; 32 * 32 * 4],
        )?;
        atlas.before_frame();
        for step in 0..MAX_QUEUED_UPLOADS_PER_TILE as i32 {
            assert!(update_one_pixel_at(&atlas, &key, step)?);
        }
        assert!(!update_one_pixel_at(&atlas, &key, 100)?);
        atlas.before_frame();
        assert_eq!(queued_for(&atlas, &tile), 0);
        assert!(update_one_pixel_at(&atlas, &key, 100)?);
        Ok(())
    }

    /// The bytes bound: large part-overlapping updates reach twice the tile's bytes long before the count.
    #[test]
    fn the_queue_for_a_tile_has_a_byte_bound() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);
        let tile = insert_image(
            &atlas,
            key.clone(),
            rect(0, 0, 8, 8).size,
            &vec![0; 8 * 8 * 4],
        )?;
        atlas.before_frame();
        // The tile is 256 bytes. Each update is 7 x 7 = 196 bytes and moves by one pixel, so none covers another.
        let patch = pixels(0x40, 49);
        assert!(atlas.update(&key, rect(0, 0, 7, 7), &patch)?);
        assert!(atlas.update(&key, rect(1, 0, 7, 7), &patch)?);
        // 3 x 196 is past 512.
        assert!(!atlas.update(&key, rect(0, 1, 7, 7), &patch)?);
        assert_eq!(queued_for(&atlas, &tile), 2);
        Ok(())
    }

    #[test]
    fn update_needs_a_tile_and_a_rect_inside_it() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);
        assert!(!atlas.update(&key, rect(0, 0, 1, 1), &pixels(0, 1))?);

        insert_image(&atlas, key.clone(), rect(0, 0, 2, 2).size, &pixels(0, 4))?;
        atlas
            .update(&key, rect(1, 1, 2, 1), &pixels(0, 2))
            .expect_err("rect past the tile's edge");
        atlas
            .update(&key, rect(0, 0, 1, 1), &pixels(0, 2))
            .expect_err("bytes for two pixels");
        assert!(atlas.update(&key, rect(1, 1, 1, 1), &pixels(0, 1))?);

        // After a clear the tile comes back with the image's first pixels: the caller must make a new image.
        atlas.clear();
        assert!(!atlas.update(&key, rect(0, 0, 1, 1), &pixels(0, 1))?);
        insert_image(&atlas, key.clone(), rect(0, 0, 2, 2).size, &pixels(0, 4))?;
        assert!(!atlas.update(&key, rect(0, 0, 1, 1), &pixels(0, 1))?);
        atlas.remove(&key);
        assert!(!atlas.update(&key, rect(0, 0, 1, 1), &pixels(0, 1))?);
        Ok(())
    }

    #[test]
    fn swizzle_upload_data_preserves_bgra_uploads() {
        let input = vec![0x10, 0x20, 0x30, 0x40];
        assert_eq!(
            swizzle_upload_data(&input, wgpu::TextureFormat::Bgra8Unorm),
            input
        );
    }

    #[test]
    fn swizzle_upload_data_converts_bgra_to_rgba() {
        let input = vec![0x10, 0x20, 0x30, 0x40, 0xAA, 0xBB, 0xCC, 0xDD];
        assert_eq!(
            swizzle_upload_data(&input, wgpu::TextureFormat::Rgba8Unorm),
            vec![0x30, 0x20, 0x10, 0x40, 0xCC, 0xBB, 0xAA, 0xDD]
        );
    }
}
