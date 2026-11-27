//! Copy a dma-buf (a GPU buffer another process shares by file descriptor) into a wgpu texture, on the Vulkan device
//! GPUI draws with. Based on the `cef` crate's `osr_texture_import` (156.3.0+156.0.15,
//! <https://github.com/tauri-apps/cef-rs>, Apache-2.0 OR MIT, used here under MIT), with two differences:
//!
//! - A buffer without a modifier (`MOD_INVALID`: the layout the allocating driver chose, kept in the buffer's own
//!   metadata) is imported with optimal tiling and a dedicated allocation. Mesa's drivers then read that metadata.
//!   Chromium's capture buffers come this way on X11 with ANGLE on Vulkan (AMD, 2026-11-05); `cef`'s importer passes
//!   the value on as an explicit modifier, and reads garbage.
//! - The copy takes only the visible part of the buffer, which Chromium may pad, into a texture the caller can keep
//!   and pass back for the next frame of the same size.
//! - [`import`] and [`copy`] are apart: Chromium paints into a few buffers in turn, so the caller imports each once
//!   and copies from it every time it comes back. Importing is the costly part (a new image and memory each time).
//! - A buffer of the web host's frame ring (`ring.rs`) is not copied: [`acquire`] takes it from the host, the renderer
//!   draws it, and [`settle`] readies it for the host's next write.
//!
//! The copy is finished when [`copy`] returns, so the producer may reuse the buffer then.

use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};

use ash::vk;
use gpui_wgpu::wgpu;
use wgpu::hal::api::Vulkan;

/// DRM's "no modifier": the layout is the allocating driver's, recorded in the buffer.
pub(super) const MOD_INVALID: u64 = 0x00FF_FFFF_FFFF_FFFF;

/// wgpu's Vulkan backend keeps an image with these read usages in `GENERAL`, the layout the producer hands over.
const EXTERNAL_STATE: wgpu::TextureUses = wgpu::TextureUses::COPY_SRC.union(wgpu::TextureUses::RESOURCE);

/// A buffer to copy: its size, format and layout, and the part of it that shows.
pub(super) struct Buffer<'a> {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub modifier: u64,
    /// Each plane's stride and offset, in bytes. All planes are in the first descriptor's allocation.
    pub planes: &'a [(u32, u64)],
    pub fd: &'a OwnedFd,
}

/// A buffer imported on GPUI's device, kept to copy from each time the producer passes it again. Holding it keeps the
/// buffer's memory alive, so its identity (`frames/linux.rs` keys it by inode) is never another buffer's.
pub(super) struct Imported {
    source: wgpu::Texture,
    raw_device: ash::Device,
    image: vk::Image,
    local_family: u32,
    external_family: u32,
    /// What it was imported as: a buffer passed again with other values is imported again.
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub modifier: u64,
    pub planes: Vec<(u32, u64)>,
}

impl Imported {
    /// Whether `buffer` is laid out as this was imported.
    pub(super) fn fits(&self, buffer: &Buffer) -> bool {
        self.width == buffer.width
            && self.height == buffer.height
            && self.format == buffer.format
            && self.modifier == buffer.modifier
            && self.planes == buffer.planes
    }
}

/// Import `buffer` on `device`, to [`copy`] from.
///
/// # Safety
///
/// `buffer`'s descriptor must name a dma-buf laid out as `buffer` says.
pub(super) unsafe fn import(device: &wgpu::Device, buffer: &Buffer) -> Result<Imported, String> {
    let limit = device.limits().max_texture_dimension_2d;
    if buffer.width > limit || buffer.height > limit {
        return Err(format!(
            "{}x{} is over the device's limit of {limit}",
            buffer.width, buffer.height
        ));
    }
    let (source, raw_device, image, local_family, external_family) = unsafe { import_raw(device, buffer) }?;
    Ok(Imported {
        source,
        raw_device,
        image,
        local_family,
        external_family,
        width: buffer.width,
        height: buffer.height,
        format: buffer.format,
        modifier: buffer.modifier,
        planes: buffer.planes.to_vec(),
    })
}

/// Copy `visible` (x, y, width, height) of `imported` into a texture: `reuse` when it has that size and format, else a
/// new one.
///
/// # Safety
///
/// The producer must not write the buffer until this returns, and `queue` must be `device`'s.
pub(super) unsafe fn copy(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    imported: &Imported,
    visible: (u32, u32, u32, u32),
    reuse: Option<wgpu::Texture>,
) -> Result<wgpu::Texture, String> {
    let (x, y, width, height) = visible;
    if width == 0 || height == 0 || x + width > imported.width || y + height > imported.height {
        return Err(format!(
            "visible part {visible:?} outside a {}x{} buffer",
            imported.width, imported.height
        ));
    }
    let Imported {
        source,
        raw_device,
        image,
        local_family,
        external_family,
        format,
        ..
    } = imported;
    let (raw_device, image, local_family, external_family, format) =
        (raw_device, *image, *local_family, *external_family, *format);

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let destination = reuse
        .filter(|texture| texture.size() == size && texture.format() == format)
        .unwrap_or_else(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("web page frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        });

    // wgpu wants raw commands (the ownership barriers) and its own in separate encoders.
    let acquire = ownership(device, raw_device, image, external_family, local_family, true)?;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("web page frame copy"),
    });
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        destination.as_image_copy(),
        size,
    );
    encoder.transition_resources(
        std::iter::empty(),
        std::iter::once(wgpu::TextureTransition {
            texture: source,
            selector: None,
            state: EXTERNAL_STATE,
        }),
    );
    let release = ownership(device, raw_device, image, local_family, external_family, false)?;
    let submission = queue.submit([acquire, encoder.finish(), release]);
    loop {
        match device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission.clone()),
            timeout: None,
        }) {
            Ok(_) => break,
            Err(wgpu::PollError::Timeout) => continue,
            Err(error) => return Err(format!("waiting for the copy: {error}")),
        }
    }
    Ok(destination)
}

/// Take a frame the web host wrote into `imported`, a buffer of its ring (`ring.rs`), for the renderer to draw. The
/// renderer's commands, submitted after this on the same queue, see the host's writes.
///
/// # Safety
///
/// The host must have finished its write and released the buffer to `QUEUE_FAMILY_EXTERNAL` in `GENERAL`, and the
/// buffer must be in wgpu's `GENERAL` ([`settle`], or fresh from [`import`]). `queue` must be `device`'s.
pub(super) unsafe fn acquire(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    imported: &Imported,
) -> Result<wgpu::Texture, String> {
    let command = ownership(
        device,
        &imported.raw_device,
        imported.image,
        vk::QUEUE_FAMILY_EXTERNAL,
        imported.local_family,
        true,
    )?;
    queue.submit([command]);
    Ok(imported.source.clone())
}

/// Return ring buffers no frame draws any more to wgpu's `GENERAL`, where [`acquire`] expects them. The host may write
/// them once the returned submission is done: the renderer's reads were submitted before it.
pub(super) fn settle(device: &wgpu::Device, queue: &wgpu::Queue, textures: &[wgpu::Texture]) -> wgpu::SubmissionIndex {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("web page frames handed back"),
    });
    encoder.transition_resources(
        std::iter::empty(),
        textures.iter().map(|texture| wgpu::TextureTransition {
            texture,
            selector: None,
            state: EXTERNAL_STATE,
        }),
    );
    queue.submit([encoder.finish()])
}

/// The Vulkan UUID of `device`'s GPU: the web host makes its own device on the same one (`ring.rs`).
pub(super) fn device_uuid(device: &wgpu::Device) -> Option<[u8; 16]> {
    // SAFETY: reads a property of the device's own physical device.
    unsafe {
        let hal = device.as_hal::<Vulkan>()?;
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        hal.shared_instance()
            .raw_instance()
            .get_physical_device_properties2(hal.raw_physical_device(), &mut properties);
        Some(id.device_uuid)
    }
}

/// The buffer as a wgpu texture (wgpu owns the image and its memory from here), with what the barriers need.
unsafe fn import_raw(
    device: &wgpu::Device,
    buffer: &Buffer,
) -> Result<(wgpu::Texture, ash::Device, vk::Image, u32, u32), String> {
    let vk_format = match buffer.format {
        wgpu::TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        wgpu::TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        other => return Err(format!("unsupported format {other:?}")),
    };
    let size = wgpu::Extent3d {
        width: buffer.width,
        height: buffer.height,
        depth_or_array_layers: 1,
    };
    let (hal_texture, raw_device, image, local_family, external_family) = {
        let hal_device = unsafe { device.as_hal::<Vulkan>() }.ok_or("the device is not Vulkan")?;
        let raw = hal_device.raw_device().clone();
        let external_family = if hal_device
            .enabled_device_extensions()
            .contains(&ash::ext::queue_family_foreign::NAME)
        {
            vk::QUEUE_FAMILY_FOREIGN_EXT
        } else {
            vk::QUEUE_FAMILY_EXTERNAL
        };

        let layouts: Vec<_> = buffer
            .planes
            .iter()
            .map(|(stride, offset)| vk::SubresourceLayout {
                offset: *offset,
                row_pitch: u64::from(*stride),
                ..Default::default()
            })
            .collect();
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(buffer.modifier)
            .plane_layouts(&layouts);
        let mut external =
            vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width: buffer.width,
                height: buffer.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut external);
        info = if buffer.modifier == MOD_INVALID {
            // The driver takes the layout from the buffer's metadata when the memory is imported for this image.
            info.tiling(vk::ImageTiling::OPTIMAL)
        } else {
            info.tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .push_next(&mut explicit)
        };
        let image = unsafe { raw.create_image(&info, None) }.map_err(|error| format!("create image: {error}"))?;
        let mut owned = Owned {
            device: raw.clone(),
            image,
            memory: vk::DeviceMemory::null(),
        };

        let requirements = unsafe { raw.get_image_memory_requirements(image) };
        let fd_api = ash::khr::external_memory_fd::Device::new(hal_device.shared_instance().raw_instance(), &raw);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        unsafe {
            fd_api.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                buffer.fd.as_raw_fd(),
                &mut fd_properties,
            )
        }
        .map_err(|error| format!("dma-buf properties: {error}"))?;
        let memory_properties = unsafe {
            hal_device
                .shared_instance()
                .raw_instance()
                .get_physical_device_memory_properties(hal_device.raw_physical_device())
        };
        let allowed = requirements.memory_type_bits & fd_properties.memory_type_bits;
        let memory_type = (0..memory_properties.memory_type_count)
            .find(|index| allowed & (1 << index) != 0)
            .ok_or("no memory type fits the dma-buf")?;

        // Vulkan owns the descriptor only once the allocation succeeds: import a copy.
        let fd = unsafe { BorrowedFd::borrow_raw(buffer.fd.as_raw_fd()) }
            .try_clone_to_owned()
            .map_err(|error| format!("duplicate the descriptor: {error}"))?;
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd.as_raw_fd());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        owned.memory =
            unsafe { raw.allocate_memory(&allocate, None) }.map_err(|error| format!("import memory: {error}"))?;
        let _ = fd.into_raw_fd();
        unsafe { raw.bind_image_memory(image, owned.memory, 0) }.map_err(|error| format!("bind memory: {error}"))?;

        let texture = unsafe {
            hal_device.texture_from_raw(
                image,
                &wgpu::hal::TextureDescriptor {
                    label: Some("shared web frame"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: buffer.format,
                    usage: wgpu::TextureUses::COPY_SRC | wgpu::TextureUses::RESOURCE,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: vec![],
                },
                None,
                wgpu::hal::vulkan::TextureMemory::Dedicated(owned.memory),
            )
        };
        // wgpu-hal owns the image and its memory now.
        owned.image = vk::Image::null();
        owned.memory = vk::DeviceMemory::null();
        (texture, raw, image, hal_device.queue_family_index(), external_family)
    };
    let source = unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("shared web frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: buffer.format,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            EXTERNAL_STATE,
        )
    };
    Ok((source, raw_device, image, local_family, external_family))
}

/// Move the image between the producer's queue family and ours, leaving it in `GENERAL`, as wgpu tracks it.
fn ownership(
    device: &wgpu::Device,
    raw: &ash::Device,
    image: vk::Image,
    from: u32,
    to: u32,
    acquire: bool,
) -> Result<wgpu::CommandBuffer, String> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("web page frame ownership"),
    });
    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_queue_family_index(from)
        .dst_queue_family_index(to)
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_access_mask(if acquire {
            vk::AccessFlags::empty()
        } else {
            vk::AccessFlags::MEMORY_READ
        })
        .dst_access_mask(if acquire {
            vk::AccessFlags::MEMORY_READ
        } else {
            vk::AccessFlags::empty()
        });
    let (from_stage, to_stage) = if acquire {
        (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::ALL_COMMANDS,
        )
    } else {
        (
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
        )
    };
    unsafe {
        encoder.as_hal_mut::<Vulkan, _, _>(|hal| {
            let hal = hal.ok_or("no Vulkan encoder")?;
            raw.cmd_pipeline_barrier(
                hal.raw_handle(),
                from_stage,
                to_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            Ok::<_, String>(())
        })?;
    }
    Ok(encoder.finish())
}

/// An image and memory not yet handed to wgpu, destroyed if the import fails part way.
struct Owned {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}
