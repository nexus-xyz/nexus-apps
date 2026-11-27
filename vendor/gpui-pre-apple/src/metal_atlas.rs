use anyhow::{Context as _, Result, ensure};
use etagere::BucketedAtlasAllocator;
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTextureList,
    AtlasTile, Bounds, DevicePixels, PlatformAtlas, Point, Size,
};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLResourceOptions, MTLSize, MTLStorageMode,
    MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use parking_lot::Mutex;
use std::{borrow::Cow, ptr::NonNull};

pub struct MetalAtlas(Mutex<AtlasState<MetalAtlasTextures>>);

impl MetalAtlas {
    pub(crate) fn new(
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        supports_shared_storage: bool,
    ) -> Self {
        MetalAtlas(Mutex::new(AtlasState::new(MetalAtlasTextures {
            device,
            supports_shared_storage,
            monochrome_textures: AtlasTextureList::default(),
            polychrome_textures: AtlasTextureList::default(),
            pending_updates: Vec::new(),
        })))
    }

    /// Nexus: blits staged inserts and updates in a command buffer of their own on `command_queue`, committed at
    /// once. Metal runs one queue's command buffers in commit order and tracks the atlas textures' hazards, so a
    /// frame already committed reads the old pixels and the next one the new. The buffer is not the frame's: a frame
    /// that fails after this call and is never committed does not take the updates with it. Called by the renderer
    /// before each frame it draws. On an error before the commit the updates stay queued for the next frame.
    pub(crate) fn commit_updates(
        &self,
        command_queue: &ProtocolObject<dyn MTLCommandQueue>,
    ) -> Result<()> {
        let mut lock = self.0.lock();
        let textures = &mut lock.backend;
        if textures.pending_updates.is_empty() {
            return Ok(());
        }
        let command_buffer = command_queue
            .commandBuffer()
            .context("creating the atlas updates' command buffer")?;
        let blit = command_buffer
            .blitCommandEncoder()
            .context("creating the atlas updates' blit encoder")?;
        for update in textures.pending_updates.drain(..) {
            let id = update.id;
            let texture = match id.kind {
                AtlasTextureKind::Monochrome => &textures.monochrome_textures,
                AtlasTextureKind::Polychrome => &textures.polychrome_textures,
                AtlasTextureKind::Subpixel => unreachable!(),
            }
            .textures
            .get(id.index as usize)
            .and_then(Option::as_ref);
            // `remove` drops a gone tile's updates, so this is only a guard.
            let Some(texture) = texture else {
                continue;
            };
            let (width, height) = (
                update.bounds.size.width.0 as usize,
                update.bounds.size.height.0 as usize,
            );
            // SAFETY: insert validates the full image and allocates its tile. `AtlasState::update` validates
            // partial updates against that tile. Both buffers cover `bytes_per_row * height` inside the texture.
            // `commandBuffer` retains the buffer and texture until it completes.
            unsafe {
                blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &update.buffer,
                    0,
                    update.bytes_per_row,
                    update.bytes_per_row * height,
                    MTLSize {
                        width,
                        height,
                        depth: 1,
                    },
                    &texture.metal_texture,
                    0,
                    0,
                    MTLOrigin {
                        x: update.bounds.origin.x.0 as usize,
                        y: update.bounds.origin.y.0 as usize,
                        z: 0,
                    },
                );
            }
        }
        blit.endEncoding();
        command_buffer.commit();
        Ok(())
    }

    /// Returns the GPU texture backing `id`, or `None` once every tile in it
    /// has been removed. A scene can still reference such a texture when a
    /// cached view replays a paint from before the image was dropped, so
    /// callers must skip those sprites rather than assume the texture exists.
    pub(crate) fn metal_texture(
        &self,
        id: AtlasTextureId,
    ) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        Some(self.0.lock().backend.texture(id)?.metal_texture.clone())
    }
}

struct MetalAtlasTextures {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    supports_shared_storage: bool,
    monochrome_textures: AtlasTextureList<MetalAtlasTexture>,
    polychrome_textures: AtlasTextureList<MetalAtlasTexture>,
    /// Nexus: staged inserts and updates, oldest first, for `MetalAtlas::commit_updates` to blit.
    pending_updates: Vec<PendingUpdate>,
}

/// Nexus: an update's pixels in a buffer, for a blit to `bounds` of texture `id`.
struct PendingUpdate {
    id: AtlasTextureId,
    bounds: Bounds<DevicePixels>,
    bytes_per_row: usize,
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
}

/// Nexus: the most updates that wait for one tile before `update` refuses, and (twice the tile's bytes) the most
/// bytes, as in the wgpu atlas. This bounds memory while the window draws no frames.
const MAX_QUEUED_UPDATES_PER_TILE: usize = 64;

/// Nexus: large color images use a GPU blit instead of blocking the main thread in `replaceRegion`.
/// Small images and glyphs keep the direct copy to avoid a staging allocation for each glyph.
const MIN_STAGED_IMAGE_BYTES: usize = 256 * 1024;

/// Nexus: whether `inner` lies inside `outer`. (`Bounds::is_contained_within` says no for equal bounds.)
fn bounds_contain(outer: Bounds<DevicePixels>, inner: Bounds<DevicePixels>) -> bool {
    let (outer_end, inner_end) = (outer.bottom_right(), inner.bottom_right());
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner_end.x <= outer_end.x
        && inner_end.y <= outer_end.y
}

impl PlatformAtlas for MetalAtlas {
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

impl AtlasBackend for MetalAtlasTextures {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        let upload = AtlasUpload::new(kind, size, bytes)?;
        let tile = self.allocate(size, kind)?;
        let result = if kind == AtlasTextureKind::Polychrome && upload.bytes.len() >= MIN_STAGED_IMAGE_BYTES {
            // Nexus: queue the complete new tile before any later partial updates. The renderer commits
            // these blits before drawing, and Metal retains their buffers until the GPU finishes.
            self.update(tile, tile.bounds, upload.bytes).and_then(|accepted| {
                ensure!(accepted, "staging a new atlas tile was refused");
                Ok(())
            })
        } else {
            self.texture(tile.texture_id)
                .context("looking up allocated atlas texture")
                .and_then(|texture| texture.upload(tile.bounds.origin, &upload))
        };
        if let Err(error) = result {
            self.remove(tile);
            return Err(error);
        }
        Ok(tile)
    }

    fn remove(&mut self, tile: AtlasTile) {
        let id = tile.texture_id;
        // Nexus: nothing queued for this tile will be seen again, and its space may go to another image before the
        // next frame. The caller that got `Ok(false)` from `update` because the queue was full drops its image next,
        // and this is what empties the queue.
        self.pending_updates
            .retain(|update| update.id != id || !bounds_contain(tile.bounds, update.bounds));

        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            AtlasTextureKind::Subpixel => unreachable!(),
        };

        let Some(texture_slot) = textures
            .textures
            .iter_mut()
            .find(|texture| texture.as_ref().is_some_and(|v| v.id == id))
        else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.allocator.deallocate(tile.tile_id.into());
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                textures.free_list.push(id.index as usize);
                // Nexus: the index may go to a new texture.
                self.pending_updates.retain(|update| update.id != id);
            } else {
                *texture_slot = Some(texture);
            }
        }
    }

    /// Nexus: not written with `replaceRegion`. That writes from the CPU at once, while the GPU may still draw
    /// an earlier frame from this tile, which would show a torn frame. The bytes go to a buffer,
    /// and `MetalAtlas::commit_updates` blits them in order with the frames' draws. A queued update that a newer one
    /// covers is dropped. Updates that only part-overlap stay, so the queue for a tile is bounded.
    fn update(
        &mut self,
        tile: AtlasTile,
        bounds: Bounds<DevicePixels>,
        bytes: &[u8],
    ) -> Result<bool> {
        if self.texture(tile.texture_id).is_none() {
            return Ok(false);
        }
        let bytes_per_pixel = match tile.texture_id.kind {
            AtlasTextureKind::Monochrome => 1,
            AtlasTextureKind::Polychrome => 4,
            AtlasTextureKind::Subpixel => unreachable!(),
        };
        let bytes_per_row = bounds.size.width.0 as usize * bytes_per_pixel;
        self.pending_updates.retain(|update| {
            update.id != tile.texture_id || !bounds_contain(bounds, update.bounds)
        });
        let tile_bytes = tile.bounds.size.width.0.max(0) as usize
            * tile.bounds.size.height.0.max(0) as usize
            * bytes_per_pixel;
        let (queued, queued_bytes) = self
            .pending_updates
            .iter()
            .filter(|update| {
                update.id == tile.texture_id && bounds_contain(tile.bounds, update.bounds)
            })
            .fold((0, 0), |(count, total), update| {
                (count + 1, total + update.buffer.length())
            });
        if queued >= MAX_QUEUED_UPDATES_PER_TILE
            || queued_bytes.saturating_add(bytes.len()) > 2 * tile_bytes
        {
            return Ok(false);
        }
        // SAFETY: Metal copies `bytes.len()` bytes from the live slice into the new buffer.
        let buffer = unsafe {
            self.device.newBufferWithBytes_length_options(
                NonNull::from(bytes).cast(),
                bytes.len(),
                MTLResourceOptions::StorageModeShared,
            )
        }
        .context("allocating an atlas update's buffer")?;
        self.pending_updates.push(PendingUpdate {
            id: tile.texture_id,
            bounds,
            bytes_per_row,
            buffer,
        });
        Ok(true)
    }
}

impl MetalAtlasTextures {
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Result<AtlasTile> {
        {
            let textures = match texture_kind {
                AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
                AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
                AtlasTextureKind::Subpixel => unreachable!(),
            };

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Ok(tile);
            }
        }

        let texture = self.push_texture(size, texture_kind)?;
        texture.allocate(size).context("allocating atlas tile")
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> Result<&mut MetalAtlasTexture> {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        // Max texture size on all modern Apple GPUs. Anything bigger than that crashes in validateWithDevice.
        const MAX_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(16384),
            height: DevicePixels(16384),
        };
        let size = min_size.min(&MAX_ATLAS_SIZE).max(&DEFAULT_ATLAS_SIZE);
        let texture_descriptor = MTLTextureDescriptor::new();
        // SAFETY: Both dimensions are clamped to the existing 1024..=16384 atlas size limits.
        unsafe {
            texture_descriptor.setWidth(size.width.0 as usize);
            texture_descriptor.setHeight(size.height.0 as usize);
        }
        let pixel_format;
        let usage;
        match kind {
            AtlasTextureKind::Monochrome => {
                pixel_format = MTLPixelFormat::A8Unorm;
                usage = MTLTextureUsage::ShaderRead;
            }
            AtlasTextureKind::Polychrome => {
                pixel_format = MTLPixelFormat::BGRA8Unorm;
                usage = MTLTextureUsage::ShaderRead;
            }
            AtlasTextureKind::Subpixel => unreachable!(),
        }
        texture_descriptor.setPixelFormat(pixel_format);
        texture_descriptor.setUsage(usage);
        // Shared memory mode can be used only on Apple GPU families
        // https://developer.apple.com/documentation/metal/mtlresourceoptions/storagemodeshared
        texture_descriptor.setStorageMode(if self.supports_shared_storage {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        });
        let metal_texture = self
            .device
            .newTextureWithDescriptor(&texture_descriptor)
            .context("allocating atlas texture")?;

        let texture_list = match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            AtlasTextureKind::Subpixel => unreachable!(),
        };

        let index = texture_list.free_list.pop().unwrap_or_else(|| {
            let index = texture_list.textures.len();
            texture_list.textures.push(None);
            index
        });

        let atlas_texture = MetalAtlasTexture {
            id: AtlasTextureId {
                index: index as u32,
                kind,
            },
            allocator: etagere::BucketedAtlasAllocator::new(size_to_etagere(size)),
            metal_texture,
            live_atlas_keys: 0,
        };

        Ok(texture_list.textures[index].insert(atlas_texture))
    }

    fn texture(&self, id: AtlasTextureId) -> Option<&MetalAtlasTexture> {
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
            AtlasTextureKind::Subpixel => unreachable!(),
        };
        textures.textures.get(id.index as usize)?.as_ref()
    }
}

struct MetalAtlasTexture {
    id: AtlasTextureId,
    allocator: BucketedAtlasAllocator,
    metal_texture: Retained<ProtocolObject<dyn MTLTexture>>,
    live_atlas_keys: u32,
}

impl MetalAtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            bounds: Bounds {
                origin: point_from_etagere(allocation.rectangle.min),
                size,
            },
            padding: 0,
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn upload(&self, origin: Point<DevicePixels>, upload: &AtlasUpload<'_>) -> Result<()> {
        ensure!(
            self.id.kind == upload.kind,
            "atlas upload kind does not match texture"
        );
        ensure!(
            origin.x.0 >= 0
                && origin.y.0 >= 0
                && (origin.x.0 as usize)
                    .checked_add(upload.size.width.0 as usize)
                    .is_some_and(|end| end <= self.metal_texture.width())
                && (origin.y.0 as usize)
                    .checked_add(upload.size.height.0 as usize)
                    .is_some_and(|end| end <= self.metal_texture.height()),
            "atlas upload is outside texture bounds"
        );
        let region = MTLRegion {
            origin: MTLOrigin {
                x: origin.x.0 as usize,
                y: origin.y.0 as usize,
                z: 0,
            },
            size: MTLSize {
                width: upload.size.width.0 as usize,
                height: upload.size.height.0 as usize,
                depth: 1,
            },
        };
        // SAFETY: Validated bytes cover every row of the checked region in this CPU-accessible texture.
        // The borrowed bytes remain live for the copy; GPU ordering is a separate caller requirement.
        unsafe {
            self.metal_texture
                .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                    region,
                    0,
                    NonNull::from(upload.bytes).cast(),
                    upload.bytes_per_row,
                );
        }
        Ok(())
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&mut self) -> bool {
        self.live_atlas_keys == 0
    }
}

struct AtlasUpload<'a> {
    kind: AtlasTextureKind,
    size: Size<DevicePixels>,
    bytes: &'a [u8],
    bytes_per_row: usize,
}

impl<'a> AtlasUpload<'a> {
    fn new(kind: AtlasTextureKind, size: Size<DevicePixels>, bytes: &'a [u8]) -> Result<Self> {
        ensure!(
            size.width.0 > 0 && size.height.0 > 0,
            "atlas dimensions must be positive"
        );
        let bytes_per_pixel = match kind {
            AtlasTextureKind::Monochrome => 1,
            AtlasTextureKind::Polychrome => 4,
            AtlasTextureKind::Subpixel => unreachable!(),
        };
        let bytes_per_row = (size.width.0 as usize)
            .checked_mul(bytes_per_pixel)
            .context("calculating atlas row byte count")?;
        let byte_count = bytes_per_row
            .checked_mul(size.height.0 as usize)
            .context("calculating atlas byte count")?;
        ensure!(
            bytes.len() >= byte_count,
            "atlas upload byte length is {}, expected at least {byte_count}",
            bytes.len()
        );
        Ok(Self {
            kind,
            size,
            bytes: &bytes[..byte_count],
            bytes_per_row,
        })
    }
}

fn size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    etagere::Size::new(size.width.into(), size.height.into())
}

fn point_from_etagere(value: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels::from(value.x),
        y: DevicePixels::from(value.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{PlatformAtlas, point, size};
    use objc2_metal::{MTLCreateSystemDefaultDevice, MTLGPUFamily};
    use std::borrow::Cow;

    #[test]
    fn test_remove_clears_stale_keys_from_tiles_by_key() {
        let Some(atlas) = create_atlas() else {
            return;
        };

        let small = Size {
            width: DevicePixels(64),
            height: DevicePixels(64),
        };

        let key_a = make_image_key(1, 0);
        let key_b = make_image_key(2, 0);
        let key_c = make_image_key(3, 0);

        let tile_a = insert_tile(&atlas, key_a.clone(), small);
        let tile_b = insert_tile(&atlas, key_b.clone(), small);
        let tile_c = insert_tile(&atlas, key_c.clone(), small);

        assert_eq!(tile_a.texture_id, tile_b.texture_id);
        assert_eq!(tile_b.texture_id, tile_c.texture_id);

        // Remove A: texture still has B and C, so it stays.
        // The key for A must be removed from tiles_by_key.
        atlas.remove(&key_a);

        // Remove B: texture still has C.
        atlas.remove(&key_b);

        // Remove C: texture becomes unreferenced and is deleted.
        atlas.remove(&key_c);

        // Re-inserting A must allocate a fresh tile on a new texture,
        // NOT return a stale tile referencing the deleted texture.
        let tile_a2 = insert_tile(&atlas, key_a, small);
        assert_eq!(tile_a2.texture_id, tile_a.texture_id);

        // The texture must actually exist — this would panic before the fix.
        assert!(atlas.metal_texture(tile_a2.texture_id).is_some());
    }

    #[test]
    fn test_metal_texture_is_none_after_last_tile_removed() {
        let Some(atlas) = create_atlas() else {
            return;
        };

        let key = make_image_key(1, 0);
        let tile = insert_tile(
            &atlas,
            key.clone(),
            Size {
                width: DevicePixels(64),
                height: DevicePixels(64),
            },
        );
        assert!(atlas.metal_texture(tile.texture_id).is_some());

        // A scene built before the removal may still carry `tile`; looking its
        // texture up must report the gap instead of panicking.
        atlas.remove(&key);
        assert!(atlas.metal_texture(tile.texture_id).is_none());
    }

    #[test]
    fn test_remove_deallocates_tile_space_for_reuse() {
        let Some(atlas) = create_atlas() else {
            return;
        };

        let small = Size {
            width: DevicePixels(64),
            height: DevicePixels(64),
        };
        let big = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };

        let keeper_key = make_image_key(1, 0);
        let big_key_a = make_image_key(2, 0);
        let big_key_b = make_image_key(3, 0);

        let keeper_tile = insert_tile(&atlas, keeper_key, small);
        let tile_a = insert_tile(&atlas, big_key_a.clone(), big);
        assert_eq!(keeper_tile.texture_id, tile_a.texture_id);

        atlas.remove(&big_key_a);
        let tile_b = insert_tile(&atlas, big_key_b, big);
        assert_eq!(tile_b.texture_id, keeper_tile.texture_id);
    }

    #[test]
    fn test_remove_nonexistent_key_is_noop() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        let key = make_image_key(999, 0);
        atlas.remove(&key);
    }

    #[test]
    fn test_invalid_upload_does_not_allocate() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        let mut state = atlas.0.lock();
        for (kind, short_bytes, short_error) in [
            (
                AtlasTextureKind::Monochrome,
                &[][..],
                "atlas upload byte length is 0, expected at least 1",
            ),
            (
                AtlasTextureKind::Polychrome,
                &[0][..],
                "atlas upload byte length is 1, expected at least 4",
            ),
        ] {
            for (width, height, bytes, expected_error) in [
                (1, 1, short_bytes, short_error),
                (0, 1, &[][..], "atlas dimensions must be positive"),
                (1, 0, &[][..], "atlas dimensions must be positive"),
                (i32::MIN, 1, &[][..], "atlas dimensions must be positive"),
                (1, i32::MIN, &[][..], "atlas dimensions must be positive"),
            ] {
                assert_eq!(
                    state
                        .backend
                        .insert(kind, size(DevicePixels(width), DevicePixels(height)), bytes)
                        .err()
                        .map(|error| error.to_string()),
                    Some(expected_error.to_owned())
                );
                for textures in [
                    &state.backend.monochrome_textures,
                    &state.backend.polychrome_textures,
                ] {
                    assert_eq!((textures.textures.len(), textures.free_list.len()), (0, 0));
                }
            }
        }
    }

    #[test]
    fn test_texture_upload_preserves_pixel_layout() -> Result<()> {
        let Some(atlas) = create_atlas() else {
            return Ok(());
        };
        let mut state = atlas.0.lock();
        for (kind, bytes_per_pixel) in [
            (AtlasTextureKind::Monochrome, 1),
            (AtlasTextureKind::Polychrome, 4),
        ] {
            let texture = state
                .backend
                .push_texture(size(DevicePixels(8), DevicePixels(8)), kind)?;
            texture.upload(
                Point::default(),
                &AtlasUpload::new(
                    kind,
                    size(DevicePixels(8), DevicePixels(8)),
                    &vec![0; 8 * 8 * bytes_per_pixel],
                )?,
            )?;
            let pixels = (1..=3 * 2 * bytes_per_pixel)
                .map(|value| value as u8)
                .collect::<Vec<_>>();
            texture.upload(
                point(DevicePixels(2), DevicePixels(3)),
                &AtlasUpload::new(kind, size(DevicePixels(3), DevicePixels(2)), &pixels)?,
            )?;
            let mut actual = vec![0; 8 * 8 * bytes_per_pixel];
            unsafe {
                texture
                    .metal_texture
                    .getBytes_bytesPerRow_fromRegion_mipmapLevel(
                        NonNull::from(actual.as_mut_slice()).cast(),
                        8 * bytes_per_pixel,
                        MTLRegion {
                            origin: MTLOrigin { x: 0, y: 0, z: 0 },
                            size: MTLSize {
                                width: 8,
                                height: 8,
                                depth: 1,
                            },
                        },
                        0,
                    );
            }
            let mut expected = vec![0; actual.len()];
            for row in 0..2 {
                let start = ((row + 3) * 8 + 2) * bytes_per_pixel;
                let source = row * 3 * bytes_per_pixel;
                expected[start..start + 3 * bytes_per_pixel]
                    .copy_from_slice(&pixels[source..source + 3 * bytes_per_pixel]);
            }
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    // Nexus: large inserts use the update queue, with the same ordering and removal rules.
    #[test]
    fn a_large_insert_is_ready_before_the_frame() -> Result<()> {
        let atlas = create_atlas().context("native Metal device for the upload test")?;
        insert_image(
            &atlas,
            make_image_key(1, 0),
            size(DevicePixels(3), DevicePixels(2)),
            &pixels(0, 6),
        );
        // Odd-width rows exercise the staging buffer's pitch. The keeper offsets the destination.
        let dimensions = size(DevicePixels(257), DevicePixels(257));
        let bytes = (0..257 * 257 * 4).map(|index| (index % 251) as u8).collect::<Vec<_>>();
        let expected = bytes.clone();
        let tile = insert_image(&atlas, make_image_key(2, 0), dimensions, &bytes);
        drop(bytes);
        assert_ne!(tile.bounds.origin, Point::default());
        assert_eq!(queued(&atlas), 1);
        let queue = new_queue(&atlas)?;
        atlas.commit_updates(&queue)?;
        assert_eq!(queued(&atlas), 0);
        // The upload owns its resources even if the following frame is abandoned.
        drop(queue.commandBuffer().context("abandoned frame")?);
        finish_frame(&atlas, &queue)?;
        assert_eq!(read_tile(&atlas, tile), expected);
        Ok(())
    }

    #[test]
    fn a_patch_before_the_first_frame_preserves_the_new_image() -> Result<()> {
        let atlas = create_atlas().context("native Metal device for the upload test")?;
        let key = make_image_key(1, 0);
        let mut expected = vec![0x43; 257 * 257 * 4];
        let mut source = expected.clone();
        // AtlasUpload accepts a slice longer than the image. Only the image's bytes are staged.
        source.extend_from_slice(&[0xA5; 32]);
        let tile = insert_image(&atlas, key.clone(), size(DevicePixels(257), DevicePixels(257)), &source);
        let patch = pixels(0x10, 3 * 2);
        assert!(atlas.update(&key, rect(11, 13, 3, 2), &patch)?);
        assert_eq!(queued(&atlas), 2);
        for (patch_row, image_row) in patch.chunks_exact(3 * 4).zip(13..15) {
            let start = (image_row * 257 + 11) * 4;
            expected[start..start + 3 * 4].copy_from_slice(patch_row);
        }
        run_frame(&atlas)?;
        assert_eq!(read_tile(&atlas, tile), expected);
        Ok(())
    }

    #[test]
    fn removing_a_staged_insert_cancels_its_upload() -> Result<()> {
        let atlas = create_atlas().context("native Metal device for the upload test")?;
        let key = make_image_key(1, 0);
        let dimensions = size(DevicePixels(257), DevicePixels(257));
        let tile = insert_image(&atlas, key.clone(), dimensions, &vec![0x32; 257 * 257 * 4]);
        assert_eq!(queued(&atlas), 1);
        atlas.remove(&key);
        assert_eq!(queued(&atlas), 0);
        assert!(atlas.metal_texture(tile.texture_id).is_none());
        let replacement = vec![0x91; 257 * 257 * 4];
        let tile = insert_image(&atlas, make_image_key(2, 0), dimensions, &replacement);
        assert_eq!(queued(&atlas), 1);
        run_frame(&atlas)?;
        assert_eq!(read_tile(&atlas, tile), replacement);
        Ok(())
    }

    // Nexus: `update`'s tests.

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Bounds<DevicePixels> {
        Bounds {
            origin: point(DevicePixels(x), DevicePixels(y)),
            size: size(DevicePixels(width), DevicePixels(height)),
        }
    }

    /// BGRA pixels, each different from the others and from `seed`'s other values.
    fn pixels(seed: u8, count: usize) -> Vec<u8> {
        (0..count as u8)
            .flat_map(|i| [seed.wrapping_add(i), 0x40 + i, 0x80 + i, 0xF0])
            .collect()
    }

    fn insert_image(
        atlas: &MetalAtlas,
        key: AtlasKey,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> AtlasTile {
        atlas
            .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.to_vec())))))
            .expect("inserting atlas tile")
            .expect("retrieving inserted atlas tile")
    }

    /// A frame with no draws: the queued updates' blits, then (managed storage) a sync for the CPU to read.
    fn run_frame(atlas: &MetalAtlas) -> Result<()> {
        let queue = new_queue(atlas)?;
        atlas.commit_updates(&queue)?;
        finish_frame(atlas, &queue)
    }

    fn new_queue(atlas: &MetalAtlas) -> Result<Retained<ProtocolObject<dyn MTLCommandQueue>>> {
        let device = atlas.0.lock().backend.device.clone();
        device.newCommandQueue().context("command queue")
    }

    /// A committed frame after the updates on `queue`: (managed storage) a sync for the CPU to read, then a wait.
    /// The queue runs it after the updates' buffer, so its completion means theirs too.
    fn finish_frame(atlas: &MetalAtlas, queue: &ProtocolObject<dyn MTLCommandQueue>) -> Result<()> {
        use objc2_metal::MTLCommandBufferStatus;
        let command_buffer = queue.commandBuffer().context("command buffer")?;
        if !atlas.0.lock().backend.supports_shared_storage {
            let blit = command_buffer
                .blitCommandEncoder()
                .context("blit encoder")?;
            for texture in atlas
                .0
                .lock()
                .backend
                .polychrome_textures
                .textures
                .iter()
                .flatten()
            {
                blit.synchronizeResource(ProtocolObject::from_ref(&*texture.metal_texture));
            }
            blit.endEncoding();
        }
        command_buffer.commit();
        command_buffer.waitUntilCompleted();
        ensure!(
            command_buffer.status() == MTLCommandBufferStatus::Completed,
            "frame failed"
        );
        Ok(())
    }

    /// The tile's pixels as the texture holds them, rows tightly packed.
    fn read_tile(atlas: &MetalAtlas, tile: AtlasTile) -> Vec<u8> {
        let texture = atlas
            .metal_texture(tile.texture_id)
            .expect("tile's texture");
        let (width, height) = (
            tile.bounds.size.width.0 as usize,
            tile.bounds.size.height.0 as usize,
        );
        let mut out = vec![0; width * height * 4];
        unsafe {
            texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::from(out.as_mut_slice()).cast(),
                width * 4,
                MTLRegion {
                    origin: MTLOrigin {
                        x: tile.bounds.origin.x.0 as usize,
                        y: tile.bounds.origin.y.0 as usize,
                        z: 0,
                    },
                    size: MTLSize {
                        width,
                        height,
                        depth: 1,
                    },
                },
                0,
            );
        }
        out
    }

    fn queued(atlas: &MetalAtlas) -> usize {
        atlas.0.lock().backend.pending_updates.len()
    }

    #[test]
    fn update_waits_for_a_frame_then_reaches_the_texture() -> Result<()> {
        let Some(atlas) = create_atlas() else {
            return Ok(());
        };
        // A keeper first, so the image's tile is not at (0, 0) of its texture.
        insert_image(
            &atlas,
            make_image_key(1, 0),
            size(DevicePixels(3), DevicePixels(2)),
            &pixels(0, 6),
        );
        let key = make_image_key(2, 0);
        let mut image = pixels(0x10, 4 * 3);
        let tile = insert_image(
            &atlas,
            key.clone(),
            size(DevicePixels(4), DevicePixels(3)),
            &image,
        );
        assert_ne!(tile.bounds.origin, Point::default());

        let patch = pixels(0xA0, 2 * 2);
        assert!(atlas.update(&key, rect(1, 1, 2, 2), &patch)?);
        // Not written from the CPU: a frame the GPU still draws would read it.
        assert_eq!(read_tile(&atlas, tile), image);
        run_frame(&atlas)?;
        assert_eq!(queued(&atlas), 0);

        for (patch_row, image_row) in patch.chunks_exact(2 * 4).zip(1..3) {
            let start = (image_row * 4 + 1) * 4;
            image[start..start + 2 * 4].copy_from_slice(patch_row);
        }
        assert_eq!(read_tile(&atlas, tile), image);
        Ok(())
    }

    #[test]
    fn a_failed_frame_keeps_its_updates() -> Result<()> {
        let Some(atlas) = create_atlas() else {
            return Ok(());
        };
        let key = make_image_key(1, 0);
        let tile = insert_image(
            &atlas,
            key.clone(),
            size(DevicePixels(2), DevicePixels(2)),
            &pixels(0, 4),
        );
        let patch = pixels(0xB0, 4);
        assert!(atlas.update(&key, rect(0, 0, 2, 2), &patch)?);
        let queue = new_queue(&atlas)?;
        atlas.commit_updates(&queue)?;
        // The renderer's frame after the updates fails and is dropped without a commit.
        drop(queue.commandBuffer().context("command buffer")?);
        assert_eq!(queued(&atlas), 0);
        // The next frame sends no updates (the caller got `Ok(true)`), and the pixels are there.
        finish_frame(&atlas, &queue)?;
        assert_eq!(read_tile(&atlas, tile), patch);
        Ok(())
    }

    #[test]
    fn covered_updates_leave_the_queue() -> Result<()> {
        let Some(atlas) = create_atlas() else {
            return Ok(());
        };
        let key = make_image_key(1, 0);
        insert_image(
            &atlas,
            key.clone(),
            size(DevicePixels(4), DevicePixels(4)),
            &pixels(0, 16),
        );
        for _ in 0..100 {
            assert!(atlas.update(&key, rect(0, 0, 4, 4), &pixels(0x20, 16))?);
            assert!(atlas.update(&key, rect(1, 1, 1, 1), &pixels(0x30, 1))?);
        }
        // The last full update and the small one after it.
        assert_eq!(queued(&atlas), 2);
        Ok(())
    }

    /// Updates that only part-overlap stay queued, so while no frame draws the queue for a tile would grow. It stops:
    /// `update` says `Ok(false)`, and dropping the image empties the queue.
    #[test]
    fn the_queue_for_a_tile_has_a_bound() -> Result<()> {
        let Some(atlas) = create_atlas() else {
            return Ok(());
        };
        let key = make_image_key(1, 0);
        insert_image(
            &atlas,
            key.clone(),
            size(DevicePixels(32), DevicePixels(32)),
            &vec![0; 32 * 32 * 4],
        );
        let other = make_image_key(2, 0);
        insert_image(
            &atlas,
            other.clone(),
            size(DevicePixels(2), DevicePixels(2)),
            &pixels(0, 4),
        );
        assert!(atlas.update(&other, rect(0, 0, 1, 1), &pixels(0, 1))?);

        let one_pixel =
            |step: i32| atlas.update(&key, rect(step % 32, step / 32, 1, 1), &pixels(0x20, 1));
        let mut accepted = 0;
        while accepted < 200 && one_pixel(accepted)? {
            accepted += 1;
        }
        assert_eq!(accepted, MAX_QUEUED_UPDATES_PER_TILE as i32);
        for step in 0..100 {
            assert!(!one_pixel(300 + step)?);
        }
        assert_eq!(queued(&atlas), MAX_QUEUED_UPDATES_PER_TILE + 1);

        // The caller of a refused update drops the image: the tile's queue goes, another tile's does not.
        atlas.remove(&key);
        assert_eq!(queued(&atlas), 1);
        Ok(())
    }

    #[test]
    fn update_refuses_a_removed_image() -> Result<()> {
        let Some(atlas) = create_atlas() else {
            return Ok(());
        };
        let key = make_image_key(1, 0);
        insert_image(
            &atlas,
            key.clone(),
            size(DevicePixels(2), DevicePixels(2)),
            &pixels(0, 4),
        );
        assert!(atlas.update(&key, rect(1, 1, 2, 1), &pixels(0, 2)).is_err());
        atlas.remove(&key);
        assert!(!atlas.update(&key, rect(0, 0, 1, 1), &pixels(0, 1))?);
        Ok(())
    }

    fn create_atlas() -> Option<MetalAtlas> {
        let device = MTLCreateSystemDefaultDevice()?;
        let supports_shared_storage =
            cfg!(target_os = "ios") || device.supportsFamily(MTLGPUFamily::Apple1);
        Some(MetalAtlas::new(device, supports_shared_storage))
    }

    fn make_image_key(image_id: usize, frame_index: usize) -> AtlasKey {
        AtlasKey::Image(gpui::RenderImageParams {
            image_id: gpui::ImageId(image_id),
            frame_index,
        })
    }

    fn insert_tile(atlas: &MetalAtlas, key: AtlasKey, size: Size<DevicePixels>) -> AtlasTile {
        atlas
            .get_or_insert_with(key, &mut || {
                let byte_count = (size.width.0 as usize) * (size.height.0 as usize) * 4;
                Ok(Some((size, Cow::Owned(vec![0u8; byte_count]))))
            })
            .expect("inserting atlas tile")
            .expect("retrieving inserted atlas tile")
    }
}
