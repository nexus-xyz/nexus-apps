//! Nexus (new file): backdrop blur (frosted glass) for the D3D11 renderer, `Window::paint_backdrop_blur`. Ported from
//! zui (<https://github.com/zeronsh/zui>, Apache-2.0, Copyright Zed Industries),
//! `crates/gpui_windows/src/directx_renderer/backdrop.rs`. The region, scratch and kernel math is GPUI's shared
//! `gpui::backdrop_blur`, so the D3D11, wgpu and Metal renderers agree. zui's own copies of it (constants, region,
//! scratch growth, weights) are gone.
//!
//! A blur: copy the padded region under it out of the render target (the snapshot), blur the snapshot horizontally
//! at full resolution into a texture downsampled on x, then vertically into one downsampled on both axes, and draw
//! that back inside the blur's rounded bounds. Unlike zui the composite antialiases its rounded edge: it mixes the
//! sharp snapshot and the blur by the pixel's coverage (blending stays off, the snapshot is the frame there).
//!
//! All GPU state is lazy and device-local: nothing is made until a frame draws a visible blur, and it is all dropped
//! after `SCRATCH_IDLE_FRAMES` frames without one, on resize, and with the device.
use super::*;
use gpui::backdrop_blur::{self, BlurPlan, KERNEL_VECTORS};

// b2 in shaders.hlsl (zui used b1, which is this renderer's batch params). Every field starts on a 16-byte HLSL
// register boundary.
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    bounds: [f32; 4],
    corners: [f32; 4],
    clip: [f32; 4],
    // xy = the snapshot's copy origin in the render target (whole pixels), zw = its size.
    source: [f32; 4],
    // xy = native-source UV step, z = sigma, w = 1 only for texel-aligned paired taps.
    kernel: [f32; 4],
    weights: [[f32; 4]; KERNEL_VECTORS],
}

const _: () = assert!(std::mem::size_of::<Params>().is_multiple_of(16));

/// The constant buffer slot the passes read `Params` from (`BackdropParams` in shaders.hlsl).
const PARAMS_SLOT: u32 = 2;

struct Texture {
    texture: ID3D11Texture2D,
    srv: Option<ID3D11ShaderResourceView>,
    rtv: Option<ID3D11RenderTargetView>,
}

impl Texture {
    fn new(device: &ID3D11Device, [width, height]: [u32; 2], renderable: bool) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: RENDER_TARGET_FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                | if renderable {
                    D3D11_BIND_RENDER_TARGET.0
                } else {
                    0
                }) as u32,
            ..Default::default()
        };
        unsafe {
            let mut texture = None;
            device.CreateTexture2D(&desc, None, Some(&mut texture))?;
            let texture = texture.context("creating backdrop texture")?;
            let mut srv = None;
            device.CreateShaderResourceView(&texture, None, Some(&mut srv))?;
            let mut rtv = None;
            if renderable {
                device.CreateRenderTargetView(&texture, None, Some(&mut rtv))?;
            }
            Ok(Self { texture, srv, rtv })
        }
    }
}

/// The horizontal pass's output (downsampled on x) and the vertical pass's (on both axes).
struct PassTextures {
    downsample: u32,
    horizontal: Texture,
    vertical: Texture,
}

impl PassTextures {
    fn new(device: &ID3D11Device, snapshot: [u32; 2], downsample: u32) -> Result<Self> {
        let [width, height] = backdrop_blur::downsampled(snapshot, downsample);
        Ok(Self {
            downsample,
            horizontal: Texture::new(device, [width, snapshot[1]], true)?,
            vertical: Texture::new(device, [width, height], true)?,
        })
    }
}

struct Scratch {
    size: [u32; 2],
    snapshot: Texture,
    passes: PassTextures,
}

struct Shaders {
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
}

impl Shaders {
    fn new(device: &ID3D11Device, module: ShaderModule) -> Result<Self> {
        Ok(Self {
            vertex: create_vertex_shader(
                device,
                RawShaderBytes::new(module, ShaderTarget::Vertex)?.as_bytes(),
            )?,
            fragment: create_fragment_shader(
                device,
                RawShaderBytes::new(module, ShaderTarget::Fragment)?.as_bytes(),
            )?,
        })
    }
}

pub(super) struct BackdropResources {
    pass: Shaders,
    composite: Shaders,
    params: Option<ID3D11Buffer>,
    sampler: Option<ID3D11SamplerState>,
    scratch: Option<Scratch>,
    idle_frames: u32,
}

impl BackdropResources {
    #[cfg(test)]
    pub(super) fn snapshot(&self) -> &ID3D11Texture2D {
        &self.scratch.as_ref().unwrap().snapshot.texture
    }

    fn new(device: &ID3D11Device) -> Result<Self> {
        let pass = Shaders::new(device, ShaderModule::BackdropPass)?;
        let composite = Shaders::new(device, ShaderModule::BackdropComposite)?;
        let params = create_constant_buffer::<Params>(device)?;
        unsafe {
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: D3D11_FLOAT32_MAX,
                    MaxAnisotropy: 1,
                    ComparisonFunc: D3D11_COMPARISON_ALWAYS,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            Ok(Self {
                pass,
                composite,
                params,
                sampler,
                scratch: None,
                idle_frames: 0,
            })
        }
    }

    /// Ages the resources by a frame; true once they went `SCRATCH_IDLE_FRAMES` frames without a drawn blur.
    pub(super) fn expire(&mut self) -> bool {
        self.idle_frames += 1;
        self.idle_frames >= backdrop_blur::SCRATCH_IDLE_FRAMES
    }

    /// Scratch textures that hold `plan`'s region at its downsample. They grow (quantized, clamped to the drawable)
    /// and are kept while they fit, so a panel that animates does not allocate every frame.
    fn ensure_scratch(
        &mut self,
        device: &ID3D11Device,
        plan: &BlurPlan,
        drawable: [u32; 2],
    ) -> Result<()> {
        self.idle_frames = 0;
        let current = self.scratch.as_ref().map(|scratch| scratch.size);
        if let Some(size) = backdrop_blur::scratch_size(current, plan.needed(), drawable) {
            // Release the old textures before making the bigger ones.
            self.scratch = None;
            self.scratch = Some(Scratch {
                size,
                snapshot: Texture::new(device, size, false)?,
                passes: PassTextures::new(device, size, plan.downsample)?,
            });
        } else if let Some(scratch) = self.scratch.as_mut()
            && scratch.passes.downsample != plan.downsample
        {
            // zui made the whole scratch again; the snapshot does not depend on the downsample, so it stays.
            scratch.passes = PassTextures::new(device, scratch.size, plan.downsample)?;
        }
        Ok(())
    }

    fn draw_pass(
        &self,
        context: &ID3D11DeviceContext,
        shaders: &Shaders,
        sources: &[Option<ID3D11ShaderResourceView>],
        target: &Option<ID3D11RenderTargetView>,
        viewport: D3D11_VIEWPORT,
        params: Params,
    ) -> Result<()> {
        // WRITE_DISCARD lets the immediate context rename storage between draws;
        // previously submitted passes keep their own constants, including nested blurs.
        update_buffer(context, self.params.as_ref().unwrap(), &[params])?;
        unsafe {
            // Unbind both slots first: the target may be the previous pass's source.
            context.PSSetShaderResources(0, Some(&[None, None]));
            context.OMSetRenderTargets(Some(slice::from_ref(target)), None);
            context.PSSetShaderResources(0, Some(sources));
            context.VSSetConstantBuffers(PARAMS_SLOT, Some(slice::from_ref(&self.params)));
            context.PSSetConstantBuffers(PARAMS_SLOT, Some(slice::from_ref(&self.params)));
            context.PSSetSamplers(0, Some(slice::from_ref(&self.sampler)));
            context.RSSetViewports(Some(&[viewport]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.VSSetShader(&shaders.vertex, None);
            context.PSSetShader(&shaders.fragment, None);
            // Default D3D11 blend state disables blending: snapshot RGBA is
            // already premultiplied and must replace, not source-over, itself.
            context.OMSetBlendState(None, None, u32::MAX);
            context.Draw(4, 0);
        }
        Ok(())
    }
}

impl DirectXRenderer {
    /// Nexus: the scene's blurs that come before `order` (all that are left when `None`), each until the frame has
    /// drawn `MAX_BLURS_PER_FRAME`. Later ones are skipped; their tint still draws.
    pub(super) fn draw_backdrop_blurs_before<'a>(
        &mut self,
        blurs: &mut std::iter::Peekable<std::slice::Iter<'a, BackdropBlur>>,
        order: Option<DrawOrder>,
        drawn: &mut usize,
    ) -> Result<()> {
        while let Some(blur) = blurs.next_if(|blur| order.is_none_or(|order| blur.order <= order)) {
            if *drawn >= backdrop_blur::MAX_BLURS_PER_FRAME {
                continue;
            }
            if self.draw_backdrop_blur(blur)? {
                *drawn += 1;
            }
        }
        Ok(())
    }

    /// Nexus: draws one blur over what the render target holds now. False when it draws nothing (clipped out, off
    /// the drawable, values that are not finite); then nothing is allocated either.
    pub(super) fn draw_backdrop_blur(&mut self, blur: &BackdropBlur) -> Result<bool> {
        let drawable = [self.width, self.height];
        let Some(plan) = backdrop_blur::plan(blur, drawable) else {
            return Ok(false);
        };
        let devices = self.devices.as_ref().context("devices missing")?;
        let resources = self.resources.as_mut().context("resources missing")?;
        // A failed allocation skips only this blur, as Metal does (`draw_backdrop_blur` in `metal_renderer.rs`):
        // failing the frame would skip its present, and the composer blurs on every frame.
        if resources.backdrop.is_none() {
            match BackdropResources::new(&devices.device) {
                Ok(backdrop) => resources.backdrop = Some(backdrop),
                Err(error) => {
                    log::error!("skipped a backdrop blur: {error:#}");
                    return Ok(false);
                }
            }
        }
        let backdrop = resources.backdrop.as_mut().unwrap();
        if let Err(error) = backdrop.ensure_scratch(&devices.device, &plan, drawable) {
            log::error!("skipped a backdrop blur: {error:#}");
            return Ok(false);
        }
        let backdrop = &*backdrop;
        let scratch = backdrop.scratch.as_ref().unwrap();
        let [copy_x, copy_y] = plan.copy_origin(scratch.size, drawable);
        let [width, height] = scratch.size;
        let downsample = plan.downsample;
        let context = &devices.device_context;
        unsafe {
            // A blur can be the first (or only) operation of the frame.
            context
                .VSSetConstantBuffers(0, Some(slice::from_ref(&self.globals.global_params_buffer)));
            context
                .PSSetConstantBuffers(0, Some(slice::from_ref(&self.globals.global_params_buffer)));
            // Unbind before copying or changing a texture from RTV to SRV.
            // D3D11 otherwise silently nulls conflicting SRV bindings.
            context.VSSetShaderResources(0, Some(&[None]));
            context.PSSetShaderResources(0, Some(&[None, None]));
            context.OMSetRenderTargets(None, None);
            // zui could copy from an overlay plane (`backdrop_source`); this renderer has none, so always the
            // render target.
            context.CopySubresourceRegion(
                &scratch.snapshot.texture,
                0,
                0,
                0,
                0,
                resources
                    .render_target
                    .as_ref()
                    .context("missing render target")?,
                0,
                Some(&D3D11_BOX {
                    left: copy_x,
                    top: copy_y,
                    front: 0,
                    right: copy_x + width,
                    bottom: copy_y + height,
                    back: 1,
                }),
            );
        }
        // Filter native pixels before reducing either axis to avoid aliasing fine detail.
        let sigma_texels = plan.sigma;
        let mut params = Params {
            bounds: rect(blur.bounds),
            corners: [
                blur.corner_radii.top_left.0,
                blur.corner_radii.top_right.0,
                blur.corner_radii.bottom_right.0,
                blur.corner_radii.bottom_left.0,
            ],
            clip: rect(blur.content_mask.bounds),
            source: [copy_x as f32, copy_y as f32, width as f32, height as f32],
            kernel: [1. / width as f32, 0., sigma_texels, downsample as f32],
            weights: backdrop_blur::gaussian_weights(sigma_texels),
        };
        let [output_width, output_height] = backdrop_blur::downsampled(scratch.size, downsample);
        let viewport = D3D11_VIEWPORT {
            Width: output_width as f32,
            Height: height as f32,
            MaxDepth: 1.,
            ..Default::default()
        };
        // Always restore the main target and unbind owned resources, also on
        // Map failure. The context must not retain cache resources after expiry.
        let result = (|| {
            backdrop.draw_pass(
                context,
                &backdrop.pass,
                slice::from_ref(&scratch.snapshot.srv),
                &scratch.passes.horizontal.rtv,
                viewport,
                params,
            )?;
            params.kernel = [0., 1. / height as f32, sigma_texels, downsample as f32];
            let viewport = D3D11_VIEWPORT {
                Height: output_height as f32,
                ..viewport
            };
            backdrop.draw_pass(
                context,
                &backdrop.pass,
                slice::from_ref(&scratch.passes.horizontal.srv),
                &scratch.passes.vertical.rtv,
                viewport,
                params,
            )?;
            // Nexus: the composite also reads the sharp snapshot (t1) for its antialiased edge.
            backdrop.draw_pass(
                context,
                &backdrop.composite,
                &[
                    scratch.passes.vertical.srv.clone(),
                    scratch.snapshot.srv.clone(),
                ],
                &resources.render_target_view,
                resources.viewport,
                params,
            )
        })();
        unsafe {
            context.PSSetShaderResources(0, Some(&[None, None]));
            context.VSSetConstantBuffers(PARAMS_SLOT, Some(&[None]));
            context.PSSetConstantBuffers(PARAMS_SLOT, Some(&[None]));
            context.PSSetSamplers(0, Some(&[None]));
            context.VSSetShader(None, None);
            context.PSSetShader(None, None);
            context.OMSetRenderTargets(Some(slice::from_ref(&resources.render_target_view)), None);
            context.RSSetViewports(Some(&[resources.viewport]));
        }
        result.context("Drawing backdrop blur")?;
        Ok(true)
    }
}

fn rect(bounds: Bounds<ScaledPixels>) -> [f32; 4] {
    [
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
    ]
}
