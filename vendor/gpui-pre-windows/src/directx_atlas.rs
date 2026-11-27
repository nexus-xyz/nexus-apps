use etagere::BucketedAtlasAllocator;
use parking_lot::Mutex;
use windows::Win32::Graphics::{
    Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
    },
    Dxgi::Common::*,
};

use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTextureList,
    AtlasTile, Bounds, DevicePixels, PlatformAtlas, Point, Size,
};

pub(crate) struct DirectXAtlas(Mutex<AtlasState<DirectXAtlasTextures>>);

struct DirectXAtlasTextures {
    device: ID3D11Device,
    device_context: ID3D11DeviceContext,
    monochrome_textures: AtlasTextureList<DirectXAtlasTexture>,
    polychrome_textures: AtlasTextureList<DirectXAtlasTexture>,
    subpixel_textures: AtlasTextureList<DirectXAtlasTexture>,
}

struct DirectXAtlasTexture {
    id: AtlasTextureId,
    bytes_per_pixel: u32,
    allocator: BucketedAtlasAllocator,
    texture: ID3D11Texture2D,
    view: [Option<ID3D11ShaderResourceView>; 1],
    live_atlas_keys: u32,
}

impl DirectXAtlas {
    pub(crate) fn new(device: &ID3D11Device, device_context: &ID3D11DeviceContext) -> Self {
        DirectXAtlas(Mutex::new(AtlasState::new(DirectXAtlasTextures {
            device: device.clone(),
            device_context: device_context.clone(),
            monochrome_textures: Default::default(),
            polychrome_textures: Default::default(),
            subpixel_textures: Default::default(),
        })))
    }

    /// Returns the view backing `id`, or `None` once every tile in it has been
    /// removed. A scene can still reference such a texture when a cached view
    /// replays a paint from before the image was dropped, so callers must skip
    /// those sprites rather than assume the texture exists.
    pub(crate) fn get_texture_view(
        &self,
        id: AtlasTextureId,
    ) -> Option<[Option<ID3D11ShaderResourceView>; 1]> {
        let lock = self.0.lock();
        let texture = lock.backend.texture(id)?;
        Some(texture.view.clone())
    }

    pub(crate) fn handle_device_lost(
        &self,
        device: &ID3D11Device,
        device_context: &ID3D11DeviceContext,
    ) {
        let mut lock = self.0.lock();
        lock.clear(|textures| {
            textures.device = device.clone();
            textures.device_context = device_context.clone();
            textures.monochrome_textures = AtlasTextureList::default();
            textures.polychrome_textures = AtlasTextureList::default();
            textures.subpixel_textures = AtlasTextureList::default();
        });
    }
}

impl PlatformAtlas for DirectXAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<
            Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
        >,
    ) -> anyhow::Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }

    // Nexus: see `PlatformAtlas::update`. `AtlasState` finds the tile and checks the rect and length.
    fn update(
        &self,
        key: &AtlasKey,
        rect: Bounds<DevicePixels>,
        bytes: &[u8],
    ) -> anyhow::Result<bool> {
        self.0.lock().update(key, rect, bytes)
    }
}

impl AtlasBackend for DirectXAtlasTextures {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> anyhow::Result<AtlasTile> {
        let tile = self
            .allocate(size, kind)
            .ok_or_else(|| anyhow::anyhow!("failed to allocate"))?;
        let texture = self
            .texture(tile.texture_id)
            .ok_or_else(|| anyhow::anyhow!("allocated tile refers to a missing texture"))?;
        texture.upload(&self.device_context, tile.bounds, bytes);
        Ok(tile)
    }

    fn remove(&mut self, tile: AtlasTile) {
        let id = tile.texture_id;

        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
        };

        let Some(texture_slot) = textures.textures.get_mut(id.index as usize) else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.allocator.deallocate(tile.tile_id.into());
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                textures.free_list.push(texture.id.index as usize);
            } else {
                *texture_slot = Some(texture);
            }
        }
    }

    /// Nexus: written with `UpdateSubresource` on the immediate context, as `insert`'s upload is, and in the same
    /// format (an image's bytes are BGRA, as the polychrome texture is). The context runs its commands in order, so
    /// the next frame's draws read the new pixels, and draws already queued read the old ones: Direct3D copies the
    /// bytes at the call when the GPU still reads the texture. Nothing waits in the atlas, so unlike the wgpu atlas
    /// there is no queue to bound. The driver flushes its command buffer when it fills, a window that draws no
    /// frames too.
    fn update(
        &mut self,
        tile: AtlasTile,
        bounds: Bounds<DevicePixels>,
        bytes: &[u8],
    ) -> anyhow::Result<bool> {
        let Some(texture) = self.texture(tile.texture_id) else {
            return Ok(false);
        };
        texture.upload(&self.device_context, bounds, bytes);
        Ok(true)
    }
}

impl DirectXAtlasTextures {
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let textures = match texture_kind {
                AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
                AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
                AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
            };

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Some(tile);
            }
        }

        let texture = self.push_texture(size, texture_kind)?;
        texture.allocate(size)
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> Option<&mut DirectXAtlasTexture> {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        // Max texture size for DirectX. See:
        // https://learn.microsoft.com/en-us/windows/win32/direct3d11/overviews-direct3d-11-resources-limits
        const MAX_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(16384),
            height: DevicePixels(16384),
        };
        let size = min_size.min(&MAX_ATLAS_SIZE).max(&DEFAULT_ATLAS_SIZE);
        let pixel_format;
        let bind_flag;
        let bytes_per_pixel;
        match kind {
            AtlasTextureKind::Monochrome => {
                pixel_format = DXGI_FORMAT_R8_UNORM;
                bind_flag = D3D11_BIND_SHADER_RESOURCE;
                bytes_per_pixel = 1;
            }
            AtlasTextureKind::Polychrome => {
                pixel_format = DXGI_FORMAT_B8G8R8A8_UNORM;
                bind_flag = D3D11_BIND_SHADER_RESOURCE;
                bytes_per_pixel = 4;
            }
            AtlasTextureKind::Subpixel => {
                pixel_format = DXGI_FORMAT_R8G8B8A8_UNORM;
                bind_flag = D3D11_BIND_SHADER_RESOURCE;
                bytes_per_pixel = 4;
            }
        }
        let texture_desc = D3D11_TEXTURE2D_DESC {
            Width: size.width.0 as u32,
            Height: size.height.0 as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: pixel_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flag.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            // This only returns None if the device is lost, which we will recreate later.
            // So it's ok to return None here.
            self.device
                .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                .ok()?;
        }
        let texture = texture.unwrap();

        let texture_list = match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
        };
        let index = texture_list.free_list.pop();
        let view = unsafe {
            let mut view = None;
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut view))
                .ok()?;
            [view]
        };
        let atlas_texture = DirectXAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            bytes_per_pixel,
            allocator: etagere::BucketedAtlasAllocator::new(device_size_to_etagere(size)),
            texture,
            view,
            live_atlas_keys: 0,
        };
        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list.textures.get_mut(ix).unwrap().as_mut()
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list.textures.last_mut().unwrap().as_mut()
        }
    }

    fn texture(&self, id: AtlasTextureId) -> Option<&DirectXAtlasTexture> {
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
            AtlasTextureKind::Subpixel => &self.subpixel_textures,
        };
        textures.textures.get(id.index as usize)?.as_ref()
    }
}

impl DirectXAtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(device_size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            bounds: Bounds {
                origin: etagere_point_to_device(allocation.rectangle.min),
                size,
            },
            padding: 0,
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn upload(
        &self,
        device_context: &ID3D11DeviceContext,
        bounds: Bounds<DevicePixels>,
        bytes: &[u8],
    ) {
        // `UpdateSubresource` reads `row_pitch * height` bytes from `bytes` based on the
        // `D3D11_BOX` below. If the caller hands us a slice shorter than that, the driver would
        // over-read past the end of the source buffer (potentially by multiple megabytes), so bail
        // out instead. This is a first-insert path rather than a per-frame one, so the check is
        // effectively free.
        let row_bytes = bounds.size.width.to_bytes(self.bytes_per_pixel as u8) as usize;
        let expected = row_bytes * bounds.size.height.0.max(0) as usize;
        if bytes.len() < expected {
            log::error!(
                "DirectXAtlasTexture::upload: source slice is {} bytes but the {}x{} region \
                 requires {} bytes; skipping upload to avoid a driver over-read",
                bytes.len(),
                bounds.size.width.0,
                bounds.size.height.0,
                expected,
            );
            return;
        }
        unsafe {
            device_context.UpdateSubresource(
                &self.texture,
                0,
                Some(&D3D11_BOX {
                    left: bounds.left().0 as u32,
                    top: bounds.top().0 as u32,
                    front: 0,
                    right: bounds.right().0 as u32,
                    bottom: bounds.bottom().0 as u32,
                    back: 1,
                }),
                bytes.as_ptr() as _,
                bounds.size.width.to_bytes(self.bytes_per_pixel as u8),
                0,
            );
        }
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&mut self) -> bool {
        self.live_atlas_keys == 0
    }
}

fn device_size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    etagere::Size::new(size.width.into(), size.height.into())
}

fn etagere_point_to_device(value: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels::from(value.x),
        y: DevicePixels::from(value.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ImageId, RenderImageParams};
    use std::borrow::Cow;
    use windows::Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_WARP,
            Direct3D11::{D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice},
        },
    };

    fn create_atlas() -> Option<DirectXAtlas> {
        let mut device: Option<ID3D11Device> = None;
        let mut device_context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut device_context),
            )
        }
        .ok()?;
        Some(DirectXAtlas::new(&device?, &device_context?))
    }

    fn make_image_key(image_id: usize) -> AtlasKey {
        AtlasKey::Image(RenderImageParams {
            image_id: ImageId(image_id),
            frame_index: 0,
        })
    }

    fn insert_tile(atlas: &DirectXAtlas, key: AtlasKey, size: Size<DevicePixels>) -> AtlasTile {
        atlas
            .get_or_insert_with(key, &mut || {
                let byte_count = (size.width.0 as usize) * (size.height.0 as usize) * 4;
                Ok(Some((size, Cow::Owned(vec![0u8; byte_count]))))
            })
            .expect("allocation should succeed")
            .expect("callback returns Some")
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

        let keeper_key = make_image_key(1);
        let big_key_a = make_image_key(2);
        let big_key_b = make_image_key(3);

        let keeper_tile = insert_tile(&atlas, keeper_key, small);
        let tile_a = insert_tile(&atlas, big_key_a.clone(), big);
        assert_eq!(keeper_tile.texture_id, tile_a.texture_id);

        atlas.remove(&big_key_a);

        let tile_b = insert_tile(&atlas, big_key_b, big);
        assert_eq!(tile_b.texture_id, keeper_tile.texture_id);
    }

    // Nexus: `update`'s tests. They run on WARP, Direct3D's software device, so any Windows machine runs them.

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
        atlas: &DirectXAtlas,
        key: AtlasKey,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> AtlasTile {
        atlas
            .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.to_vec())))))
            .expect("allocation should succeed")
            .expect("callback returns Some")
    }

    /// The tile's pixels as the texture holds them, rows tightly packed: a copy to a staging texture, mapped.
    fn read_tile(atlas: &DirectXAtlas, tile: AtlasTile) -> Vec<u8> {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_USAGE_STAGING,
        };
        let lock = atlas.0.lock();
        let textures = &lock.backend;
        let texture = textures.texture(tile.texture_id).expect("tile's texture");
        let (width, height) = (
            tile.bounds.size.width.0 as u32,
            tile.bounds.size.height.0 as u32,
        );
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.texture.GetDesc(&mut desc) };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            textures
                .device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        }
        .expect("staging texture");
        let staging = staging.expect("staging texture");
        let context = &textures.device_context;
        unsafe {
            context.CopySubresourceRegion(
                &staging,
                0,
                0,
                0,
                0,
                &texture.texture,
                0,
                Some(&D3D11_BOX {
                    left: tile.bounds.left().0 as u32,
                    top: tile.bounds.top().0 as u32,
                    front: 0,
                    right: tile.bounds.right().0 as u32,
                    bottom: tile.bounds.bottom().0 as u32,
                    back: 1,
                }),
            )
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.expect("map");
        let row = width as usize * 4;
        let mut out = vec![0u8; row * height as usize];
        // SAFETY: `Map` succeeded, so `pData` holds `RowPitch * height` bytes, and `RowPitch >= row`.
        unsafe {
            let src = mapped.pData as *const u8;
            for y in 0..height as usize {
                std::ptr::copy_nonoverlapping(
                    src.add(y * mapped.RowPitch as usize),
                    out.as_mut_ptr().add(y * row),
                    row,
                );
            }
            context.Unmap(&staging, 0);
        }
        out
    }

    #[test]
    fn update_reaches_the_texture() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        // A keeper first, so the image's tile is not at (0, 0) of its texture.
        insert_image(
            &atlas,
            make_image_key(1),
            rect(0, 0, 3, 2).size,
            &pixels(0, 6),
        );
        let key = make_image_key(2);
        let mut image = pixels(0x10, 4 * 3);
        let tile = insert_image(&atlas, key.clone(), rect(0, 0, 4, 3).size, &image);
        assert_ne!(tile.bounds.origin, Point::default());

        let patch = pixels(0xA0, 2 * 2);
        assert!(
            atlas
                .update(&key, rect(1, 1, 2, 2), &patch)
                .expect("update")
        );

        for (patch_row, image_row) in patch.chunks_exact(2 * 4).zip(1..3) {
            let start = (image_row * 4 + 1) * 4;
            image[start..start + 2 * 4].copy_from_slice(patch_row);
        }
        assert_eq!(read_tile(&atlas, tile), image);
    }

    #[test]
    fn update_refuses_a_removed_image_and_a_wrong_rect() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        let key = make_image_key(1);
        insert_image(&atlas, key.clone(), rect(0, 0, 2, 2).size, &pixels(0, 4));
        assert!(atlas.update(&key, rect(1, 1, 2, 1), &pixels(0, 2)).is_err());
        assert!(atlas.update(&key, rect(0, 0, 1, 1), &pixels(0, 2)).is_err());
        atlas.remove(&key);
        assert!(
            !atlas
                .update(&key, rect(0, 0, 1, 1), &pixels(0, 1))
                .expect("update")
        );
    }

    /// A lost device clears the atlas. An image updated before is made again from its first pixels, so `update`
    /// refuses it until the caller makes a new image.
    #[test]
    fn an_updated_image_is_refused_after_a_lost_device() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        let key = make_image_key(1);
        let image = pixels(0, 4);
        insert_image(&atlas, key.clone(), rect(0, 0, 2, 2).size, &image);
        assert!(
            atlas
                .update(&key, rect(0, 0, 1, 1), &pixels(0x80, 1))
                .expect("update")
        );
        let (device, context) = {
            let lock = atlas.0.lock();
            (
                lock.backend.device.clone(),
                lock.backend.device_context.clone(),
            )
        };
        atlas.handle_device_lost(&device, &context);
        insert_image(&atlas, key.clone(), rect(0, 0, 2, 2).size, &image);
        assert!(
            !atlas
                .update(&key, rect(0, 0, 1, 1), &pixels(0x80, 1))
                .expect("update")
        );
    }
}
