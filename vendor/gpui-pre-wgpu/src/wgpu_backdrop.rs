//! Nexus (new file): backdrop blur in the wgpu renderer (`Window::paint_backdrop_blur`). Ported from zui
//! (<https://github.com/zeronsh/zui>, Apache-2.0, Copyright Zed Industries), `crates/gpui_wgpu/src/wgpu_renderer.rs`
//! (`process_backdrop_blur`, `draw_backdrop_composite`, the scratch textures and the copyable frame). The math is
//! in `gpui::backdrop_blur`, shared with the Metal and D3D11 renderers.
//!
//! A blur ends the main pass, copies the padded region under it out of the frame (the snapshot), blurs that in two
//! separable passes into scratch textures, and draws the result back inside the blur's rounded bounds in the
//! reopened main pass. A surface that can't be copied from (GL, some Wayland compositors) gets a copyable texture
//! for the frames that blur, blitted to the surface at the end. Frames without a blur draw as before.

use super::*;
use gpui::BackdropBlur;
use gpui::backdrop_blur as math;

/// One separable pass's uniform (`BlurPassParams` in shaders.wgsl). `sigma` and `stride` are in source texels: the
/// horizontal pass reads the full-size snapshot with stride = downsample, the vertical one its output with stride 1.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurPassUniform {
    direction: [f32; 2],
    sigma: f32,
    stride: f32,
    source_texel_size: [f32; 2],
    _padding: [f32; 2],
    weights: [[f32; 4]; math::KERNEL_VECTORS],
}

/// The composite's uniform (`BackdropBlurParams` in shaders.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CompositeUniform {
    bounds: PodBounds,
    corner_radii: [f32; 4],
    clip_bounds: PodBounds,
    /// The snapshot's `[x, y, width, height]` in the frame.
    source_rect: [f32; 4],
}

#[derive(Clone)]
struct Texture2d {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Texture2d {
    fn new(
        device: &wgpu::Device,
        label: &str,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

/// The two pass outputs for one downsample factor, the snapshot's size divided by it.
struct BlurPair {
    size: [u32; 2],
    horizontal: Texture2d,
    vertical: Texture2d,
    idle_frames: u32,
}

/// A copyable texture the frame draws into, for a surface that can't be copied from.
struct CopyableFrame {
    target: Texture2d,
    blitter: wgpu::util::TextureBlitter,
}

/// Made at the first blur and dropped after `SCRATCH_IDLE_FRAMES` frames without one.
pub(super) struct BackdropState {
    params: wgpu::Buffer,
    slot_stride: u64,
    snapshot: Option<(Texture2d, [u32; 2])>,
    /// By downsample factor, 1 to 4. Each is dropped after `SCRATCH_IDLE_FRAMES` frames unused.
    pairs: [Option<BlurPair>; 4],
    frame: Option<CopyableFrame>,
    idle_frames: u32,
}

impl BackdropState {
    fn new(device: &wgpu::Device) -> Self {
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let slot_stride = (std::mem::size_of::<BlurPassUniform>()
            .max(std::mem::size_of::<CompositeUniform>()) as u64)
            .next_multiple_of(alignment);
        // Three slots a blur (two passes and the composite), all written before the frame's submit.
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop_blur_params"),
            size: slot_stride * 3 * math::MAX_BLURS_PER_FRAME as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            params,
            slot_stride,
            snapshot: None,
            pairs: Default::default(),
            frame: None,
            idle_frames: 0,
        }
    }
}

/// Where a blurred frame draws: the surface's texture, or the copyable one standing in for it.
pub(super) struct FrameTarget<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
    /// The texture can be a copy source (`COPY_SRC`).
    pub copyable: bool,
}

impl WgpuRendererCore {
    /// Ages the backdrop scratch and says whether this frame draws a blur. Frames without one only count.
    pub(super) fn begin_backdrop_frame(&mut self, scene: &Scene, drawable: [u32; 2]) -> bool {
        let blurs = scene
            .backdrop_blurs
            .iter()
            .take(math::MAX_BLURS_PER_FRAME)
            .any(|blur| math::plan(blur, drawable).is_some());
        let Some(state) = self.backdrop.as_mut() else {
            return blurs;
        };
        if !blurs {
            state.idle_frames += 1;
            if state.idle_frames >= math::SCRATCH_IDLE_FRAMES {
                self.backdrop = None;
            }
            return false;
        }
        state.idle_frames = 0;
        for pair in &mut state.pairs {
            if let Some(blurs) = pair {
                blurs.idle_frames += 1;
                if blurs.idle_frames > math::SCRATCH_IDLE_FRAMES {
                    *pair = None;
                }
            }
        }
        true
    }

    fn backdrop_state(&mut self) -> &mut BackdropState {
        let device = &self.resources.device;
        self.backdrop
            .get_or_insert_with(|| BackdropState::new(device))
    }

    /// The copyable texture a blurred frame draws into when its target can't be copied from.
    pub(super) fn copyable_frame(&mut self, size: [u32; 2]) -> (wgpu::Texture, wgpu::TextureView) {
        let format = self.target_format;
        let device = Arc::clone(&self.resources.device);
        let state = self.backdrop_state();
        if state.frame.as_ref().is_none_or(|frame| {
            [frame.target.texture.width(), frame.target.texture.height()] != size
        }) {
            state.frame = Some(CopyableFrame {
                target: Texture2d::new(
                    &device,
                    "backdrop_copyable_frame",
                    size,
                    format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                ),
                blitter: wgpu::util::TextureBlitter::new(&device, format),
            });
        }
        let target = &state.frame.as_ref().unwrap().target;
        (target.texture.clone(), target.view.clone())
    }

    /// Draws the copyable frame onto the real target.
    pub(super) fn blit_copyable_frame(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        if let Some(frame) = self
            .backdrop
            .as_ref()
            .and_then(|state| state.frame.as_ref())
        {
            frame
                .blitter
                .copy(&self.resources.device, encoder, &frame.target.view, target);
        }
    }

    /// Copies and blurs what `frame` holds under `blur`, with the main pass ended. Returns the composite's bind
    /// group to draw in the reopened pass, or `None` when the blur draws nothing (clipped out, off the frame).
    pub(super) fn encode_backdrop_blur(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        blur: &BackdropBlur,
        slot: usize,
    ) -> Option<wgpu::BindGroup> {
        let drawable = [frame.width(), frame.height()];
        let plan = math::plan(blur, drawable)?;
        let format = self.target_format;
        let device = Arc::clone(&self.resources.device);
        let queue = Arc::clone(&self.resources.queue);
        let state = self.backdrop_state();

        let current = state.snapshot.as_ref().map(|(_, size)| *size);
        if let Some(size) = math::scratch_size(current, plan.needed(), drawable) {
            let snapshot = Texture2d::new(
                &device,
                "backdrop_snapshot",
                size,
                format,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            state.snapshot = Some((snapshot, size));
        }
        let (snapshot, snapshot_size) = state.snapshot.clone()?;
        let downsample = plan.downsample;
        let blur_size = math::downsampled(snapshot_size, downsample);
        let pair = &mut state.pairs[downsample as usize - 1];
        if pair.as_ref().is_none_or(|pair| pair.size != blur_size) {
            let usage =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            *pair = Some(BlurPair {
                size: blur_size,
                horizontal: Texture2d::new(&device, "backdrop_blur_h", blur_size, format, usage),
                vertical: Texture2d::new(&device, "backdrop_blur_v", blur_size, format, usage),
                idle_frames: 0,
            });
        }
        let pair = pair.as_mut().unwrap();
        pair.idle_frames = 0;
        let (horizontal, vertical) = (pair.horizontal.clone(), pair.vertical.clone());

        // The copy fills the whole snapshot from inside the frame, so its edges hold real pixels and the passes'
        // clamp-to-edge matches the window's edge.
        let origin = plan.copy_origin(snapshot_size, drawable);
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: frame,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &snapshot.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: snapshot_size[0],
                height: snapshot_size[1],
                depth_or_array_layers: 1,
            },
        );

        let sigma = (plan.sigma / downsample as f32).max(0.5);
        let weights = math::gaussian_weights(sigma);
        let (horizontal_region, vertical_region) = math::blur_regions(
            plan.visible,
            [origin[0], origin[1], snapshot_size[0], snapshot_size[1]],
            blur_size,
            sigma,
        );
        // Each blur has its own three slots: queue writes all land before the frame's commands run.
        let base = state.slot_stride * 3 * slot as u64;
        let offsets = [base, base + state.slot_stride, base + 2 * state.slot_stride];
        queue.write_buffer(
            &state.params,
            offsets[0],
            bytemuck::bytes_of(&BlurPassUniform {
                direction: [1., 0.],
                sigma,
                stride: downsample as f32,
                source_texel_size: [1. / snapshot_size[0] as f32, 1. / snapshot_size[1] as f32],
                _padding: [0.; 2],
                weights,
            }),
        );
        queue.write_buffer(
            &state.params,
            offsets[1],
            bytemuck::bytes_of(&BlurPassUniform {
                direction: [0., 1.],
                sigma,
                stride: 1.,
                source_texel_size: [1. / blur_size[0] as f32, 1. / blur_size[1] as f32],
                _padding: [0.; 2],
                weights,
            }),
        );
        let radii = blur.corner_radii;
        queue.write_buffer(
            &state.params,
            offsets[2],
            bytemuck::bytes_of(&CompositeUniform {
                bounds: blur.bounds.into(),
                corner_radii: [
                    radii.top_left.0,
                    radii.top_right.0,
                    radii.bottom_right.0,
                    radii.bottom_left.0,
                ],
                clip_bounds: blur.content_mask.bounds.into(),
                source_rect: [
                    origin[0] as f32,
                    origin[1] as f32,
                    snapshot_size[0] as f32,
                    snapshot_size[1] as f32,
                ],
            }),
        );

        let resources = &self.resources;
        let state = self.backdrop.as_ref().unwrap();
        let group = |offset: u64, size: usize, source: &wgpu::TextureView| {
            resources
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("backdrop_bind_group"),
                    layout: &resources.bind_group_layouts.backdrop,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &state.params,
                                offset,
                                size: NonZeroU64::new(size as u64),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&resources.atlas_sampler),
                        },
                        // The sharp snapshot, for the composite's antialiased edge.
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&snapshot.view),
                        },
                    ],
                })
        };
        let pass_size = std::mem::size_of::<BlurPassUniform>();
        let passes = [
            (
                "backdrop_blur_h",
                group(offsets[0], pass_size, &snapshot.view),
                &horizontal.view,
                horizontal_region,
            ),
            (
                "backdrop_blur_v",
                group(offsets[1], pass_size, &horizontal.view),
                &vertical.view,
                vertical_region,
            ),
        ];
        for (label, bind_group, target, [x, y, width, height]) in &passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            if *width > 0 && *height > 0 {
                pass.set_pipeline(&resources.pipelines.backdrop_blur_pass);
                pass.set_bind_group(0, &resources.globals_bind_group, &[]);
                pass.set_bind_group(1, bind_group, &[]);
                pass.set_scissor_rect(*x, *y, *width, *height);
                pass.draw(0..4, 0..1);
            }
        }
        Some(group(
            offsets[2],
            std::mem::size_of::<CompositeUniform>(),
            &vertical.view,
        ))
    }

    /// Draws a blur's result over its rounded bounds in the reopened main pass. No blending: inside, the blur
    /// replaces the frame; on the one-pixel edge the shader mixes it with the sharp snapshot.
    pub(super) fn draw_backdrop_composite(
        &self,
        bind_group: &wgpu::BindGroup,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        pass.set_pipeline(&self.resources.pipelines.backdrop_composite);
        pass.set_bind_group(0, &self.resources.globals_bind_group, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.draw(0..4, 0..1);
    }
}

/// The backdrop bind group layout: a uniform, the texture a pass or the composite reads, its sampler, and the sharp
/// snapshot.
pub(super) fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("backdrop_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            texture(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture(3),
        ],
    })
}

/// Reopens the main pass after a blur, keeping what it drew.
pub(super) fn continue_main_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) -> wgpu::RenderPass<'encoder> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("main_pass_after_backdrop_blur"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        ..Default::default()
    })
}
