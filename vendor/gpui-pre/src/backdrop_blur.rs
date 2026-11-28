// Ported from zui (https://github.com/zeronsh/zui), Copyright 2022 - 2025 Zed Industries, Inc.,
// licensed under the Apache License, Version 2.0. Modified by Nexus Laboratories, Inc. See NOTICE.
//! Nexus (new file): the math every renderer's backdrop blur shares (`Window::paint_backdrop_blur`). Ported from zui
//! (<https://github.com/zeronsh/zui>, Apache-2.0, Copyright Zed Industries): its wgpu `blur_kernel.rs` and the
//! region and scratch logic of its wgpu, Metal and D3D11 renderers, in one place so the backends agree and it is
//! tested once.
//!
//! A blur's frame: the renderer ends its pass at the blur's order, copies the padded region under the blur out of
//! the frame (the snapshot), blurs it in two separable Gaussian passes into scratch textures (at a lower resolution
//! for big radii), and draws the result back inside the blur's rounded bounds before the next batch.

use crate::BackdropBlur;

/// Blurs drawn per frame; later ones are skipped (the app's tint still draws).
pub const MAX_BLURS_PER_FRAME: usize = 32;
/// The largest sigma drawn, in device pixels. Bigger radii are clamped, which bounds each pass's taps.
pub const MAX_SIGMA: f32 = 64.;
/// The smallest sigma drawn, in device pixels.
pub const MIN_SIGMA: f32 = 1.;
/// Frames without a blur after which a renderer releases its scratch textures.
pub const SCRATCH_IDLE_FRAMES: u32 = 30;
/// Scratch sizes round up to this many pixels, so a panel that grows or animates keeps its textures.
pub const SCRATCH_QUANTUM: u32 = 64;
/// The largest kernel radius (in pass texels) with precomputed weights.
pub const MAX_KERNEL_RADIUS: usize = 128;
/// `vec4`s of weights a pass's uniform carries (`[0]` is the center tap).
pub const KERNEL_VECTORS: usize = (MAX_KERNEL_RADIUS + 1).div_ceil(4);

/// Where one blur reads and how hard it blurs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurPlan {
    /// The Gaussian's sigma in device pixels, clamped to `MIN_SIGMA..=MAX_SIGMA`.
    pub sigma: f32,
    /// The scratch textures are this many times smaller than the snapshot on each axis (1 to 4).
    pub downsample: u32,
    /// The blur's visible part, `[x, y, width, height]` in device pixels: its bounds inside its content mask and the
    /// drawable.
    pub visible: [f32; 4],
    /// The padded region the passes read, `[x0, y0, x1, y1]` in drawable pixels: the visible part and 3 sigma (plus
    /// 2 pixels) around it, inside the drawable. Never empty.
    pub region: [u32; 4],
}

impl BlurPlan {
    /// The region's size, the smallest snapshot that holds it.
    pub fn needed(&self) -> [u32; 2] {
        [
            self.region[2] - self.region[0],
            self.region[3] - self.region[1],
        ]
    }

    /// Where a snapshot of `scratch` size is copied from: over the region, and inside the drawable, so every texel
    /// holds real pixels and the Gaussian's clamp-to-edge matches the window's edge (no dark vignette).
    pub fn copy_origin(&self, scratch: [u32; 2], drawable: [u32; 2]) -> [u32; 2] {
        [
            self.region[0].min(drawable[0].saturating_sub(scratch[0])),
            self.region[1].min(drawable[1].saturating_sub(scratch[1])),
        ]
    }
}

/// The blur's plan in a `drawable`-sized frame, or `None` when there is nothing to draw: clipped out, off the
/// drawable, or values that are not finite (or negative corner radii).
pub fn plan(blur: &BackdropBlur, drawable: [u32; 2]) -> Option<BlurPlan> {
    let bounds = blur.bounds;
    let clip = blur.content_mask.bounds;
    let radii = blur.corner_radii;
    let finite = [
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
        clip.origin.x.0,
        clip.origin.y.0,
        clip.size.width.0,
        clip.size.height.0,
        blur.blur_radius.0,
    ]
    .iter()
    .all(|value| value.is_finite());
    let radii_valid = [
        radii.top_left.0,
        radii.top_right.0,
        radii.bottom_right.0,
        radii.bottom_left.0,
    ]
    .iter()
    .all(|value| value.is_finite() && *value >= 0.);
    if !finite || !radii_valid || drawable[0] == 0 || drawable[1] == 0 {
        return None;
    }
    let left = bounds.origin.x.0.max(clip.origin.x.0).max(0.);
    let top = bounds.origin.y.0.max(clip.origin.y.0).max(0.);
    let right = (bounds.origin.x.0 + bounds.size.width.0)
        .min(clip.origin.x.0 + clip.size.width.0)
        .min(drawable[0] as f32);
    let bottom = (bounds.origin.y.0 + bounds.size.height.0)
        .min(clip.origin.y.0 + clip.size.height.0)
        .min(drawable[1] as f32);
    if right <= left || bottom <= top {
        return None;
    }
    let sigma = blur.blur_radius.0.clamp(MIN_SIGMA, MAX_SIGMA);
    let padding = (sigma * 3.).ceil() + 2.;
    let x0 = (left - padding).floor().max(0.) as u32;
    let y0 = (top - padding).floor().max(0.) as u32;
    let x1 = ((right + padding).ceil() as u32).min(drawable[0]);
    let y1 = ((bottom + padding).ceil() as u32).min(drawable[1]);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(BlurPlan {
        sigma,
        // The passes' tap count follows the visual radius rather than the device-pixel radius.
        downsample: ((sigma / 8.) as u32).clamp(1, 4),
        visible: [left, top, right - left, bottom - top],
        region: [x0, y0, x1, y1],
    })
}

/// The scratch size a blur needs, or `None` when `current` (the scratch a renderer holds) can be used as it is: it
/// holds `needed` and still fits the drawable, which can shrink. A new size covers both, rounded up to
/// `SCRATCH_QUANTUM` and clamped to the drawable.
pub fn scratch_size(
    current: Option<[u32; 2]>,
    needed: [u32; 2],
    drawable: [u32; 2],
) -> Option<[u32; 2]> {
    let fits = |size: [u32; 2]| {
        (0..2).all(|axis| size[axis] >= needed[axis] && size[axis] <= drawable[axis])
    };
    if current.is_some_and(fits) {
        return None;
    }
    let keep = current.unwrap_or([0; 2]);
    Some(std::array::from_fn(|axis| {
        needed[axis]
            .max(keep[axis])
            .div_ceil(SCRATCH_QUANTUM)
            .saturating_mul(SCRATCH_QUANTUM)
            .min(drawable[axis])
            .max(1)
    }))
}

/// The scratch textures' size for a snapshot of `snapshot` size.
pub fn downsampled(snapshot: [u32; 2], downsample: u32) -> [u32; 2] {
    snapshot.map(|n| n.div_ceil(downsample).max(1))
}

/// Positive half of the symmetric, normalized kernel: `weights[k / 4][k % 4]` is tap `k`'s. All zero when the radius
/// is over `MAX_KERNEL_RADIUS`; the shaders then compute the weights themselves.
pub fn gaussian_weights(sigma: f32) -> [[f32; 4]; KERNEL_VECTORS] {
    let mut weights = [[0.0; 4]; KERNEL_VECTORS];
    let sigma = sigma.max(0.5);
    let radius = (sigma * 3.0).ceil() as usize;
    if radius > MAX_KERNEL_RADIUS {
        return weights;
    }
    let mut total = 0.0;
    for k in -(radius as i32)..=radius as i32 {
        let weight = (-(k as f32) * k as f32 / (2.0 * sigma * sigma)).exp();
        total += weight;
        let k = k.unsigned_abs() as usize;
        weights[k / 4][k % 4] = weight;
    }
    for weight in weights.iter_mut().flatten() {
        *weight /= total;
    }
    weights
}

/// Scissor rectangles `[x, y, width, height]` for the horizontal and vertical passes, in `output`-sized (downsampled)
/// texels. They keep a texel of halo for the composite's bilinear samples and the vertical kernel's whole reach;
/// only fragments nothing reads are skipped. `visible` is in drawable pixels, `source` is the snapshot's
/// `[x, y, width, height]` in the drawable, `sigma` is in output texels.
pub fn blur_regions(
    visible: [f32; 4],
    source: [u32; 4],
    output: [u32; 2],
    sigma: f32,
) -> ([u32; 4], [u32; 4]) {
    let mut vertical = [0; 4];
    for axis in 0..2 {
        let scale = output[axis] as f32 / source[axis + 2] as f32;
        let start = ((visible[axis] - source[axis] as f32) * scale).floor() as i64 - 1;
        let end =
            ((visible[axis] + visible[axis + 2] - source[axis] as f32) * scale).ceil() as i64 + 1;
        let start = start.clamp(0, output[axis] as i64) as u32;
        let end = end.clamp(start as i64, output[axis] as i64) as u32;
        vertical[axis] = start;
        vertical[axis + 2] = end - start;
    }
    let mut horizontal = vertical;
    for axis in 0..2 {
        let halo = if axis == 0 {
            1
        } else {
            (3.0 * sigma).ceil() as u32 + 1
        };
        let start = vertical[axis].saturating_sub(halo);
        let end = vertical[axis]
            .saturating_add(vertical[axis + 2])
            .saturating_add(halo)
            .min(output[axis]);
        horizontal[axis] = start;
        horizontal[axis + 2] = end - start;
    }
    (horizontal, vertical)
}

/// The composite's coverage of a pixel whose center is `sdf` device pixels outside the rounded bounds (negative
/// inside): a one-pixel antialiased edge. The composite mixes the sharp snapshot and the blur by it.
pub fn edge_coverage(sdf: f32) -> f32 {
    (0.5 - sdf).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, ContentMask, Corners, ScaledPixels, point, size};

    fn blur(x: f32, y: f32, width: f32, height: f32, sigma: f32) -> BackdropBlur {
        let bounds = Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(width), ScaledPixels(height)),
        );
        BackdropBlur {
            order: 0,
            blur_radius: ScaledPixels(sigma),
            bounds,
            content_mask: ContentMask { bounds },
            corner_radii: Corners::all(ScaledPixels(12.)),
        }
    }

    #[test]
    fn plan_pads_by_three_sigma_inside_the_drawable() {
        let plan = plan(&blur(100., 50., 200., 80., 10.), [1000, 800]).unwrap();
        assert_eq!(plan.sigma, 10.);
        assert_eq!(plan.downsample, 1);
        assert_eq!(plan.visible, [100., 50., 200., 80.]);
        assert_eq!(plan.region, [68, 18, 332, 162]);
        assert_eq!(plan.needed(), [264, 144]);

        // At the window's corner the region stops at the drawable.
        let plan = super::plan(&blur(-20., -20., 60., 60., 10.), [1000, 800]).unwrap();
        assert_eq!(plan.visible, [0., 0., 40., 40.]);
        assert_eq!(plan.region, [0, 0, 72, 72]);
    }

    #[test]
    fn plan_clamps_sigma_and_downsamples_big_radii() {
        for (requested, sigma, downsample) in [
            (0.0, 1.0, 1),
            (0.25, 1.0, 1),
            (8.0, 8.0, 1),
            (16.0, 16.0, 2),
            (24.0, 24.0, 3),
            (40.0, 40.0, 4),
            (1.0e6, MAX_SIGMA, 4),
            (f32::INFINITY, 0.0, 0),
        ] {
            let plan = plan(&blur(0., 0., 100., 100., requested), [500, 500]);
            if requested.is_finite() {
                let plan = plan.unwrap();
                assert_eq!((plan.sigma, plan.downsample), (sigma, downsample));
                // The passes' kernels always fit the precomputed weights.
                let radius = (3. * plan.sigma / plan.downsample as f32).ceil() as usize;
                assert!(radius <= MAX_KERNEL_RADIUS);
            } else {
                assert!(plan.is_none());
            }
        }
    }

    #[test]
    fn plan_skips_what_draws_nothing() {
        let drawable = [400, 300];
        // Off the drawable, empty, clipped out.
        assert!(plan(&blur(500., 0., 50., 50., 8.), drawable).is_none());
        assert!(plan(&blur(10., 10., 0., 50., 8.), drawable).is_none());
        let mut clipped = blur(10., 10., 50., 50., 8.);
        clipped.content_mask.bounds.origin = point(ScaledPixels(200.), ScaledPixels(200.));
        assert!(plan(&clipped, drawable).is_none());
        // Values the shaders can't use.
        let mut nan = blur(10., 10., 50., 50., 8.);
        nan.bounds.origin.x = ScaledPixels(f32::NAN);
        assert!(plan(&nan, drawable).is_none());
        let mut negative = blur(10., 10., 50., 50., 8.);
        negative.corner_radii.top_left = ScaledPixels(-1.);
        assert!(plan(&negative, drawable).is_none());
        assert!(plan(&blur(10., 10., 50., 50., 8.), [0, 300]).is_none());
    }

    #[test]
    fn copy_origin_keeps_the_snapshot_inside_the_drawable() {
        let drawable = [1000, 800];
        let plan = plan(&blur(900., 700., 90., 90., 4.), drawable).unwrap();
        let scratch = scratch_size(None, plan.needed(), drawable).unwrap();
        let origin = plan.copy_origin(scratch, drawable);
        for axis in 0..2 {
            assert!(origin[axis] + scratch[axis] <= drawable[axis]);
            assert!(origin[axis] <= plan.region[axis]);
            assert!(origin[axis] + scratch[axis] >= plan.region[axis + 2]);
        }
    }

    #[test]
    fn scratch_is_quantized_reused_and_grown() {
        let drawable = [1000, 800];
        assert_eq!(scratch_size(None, [100, 30], drawable), Some([128, 64]));
        // Smaller and equal regions reuse it.
        assert_eq!(scratch_size(Some([128, 64]), [128, 10], drawable), None);
        // A bigger region grows it on the axis it needs, keeping the other.
        assert_eq!(
            scratch_size(Some([128, 64]), [100, 70], drawable),
            Some([128, 128])
        );
        // Never bigger than the drawable, which may have shrunk since.
        assert_eq!(scratch_size(None, [990, 790], drawable), Some([1000, 800]));
        assert_eq!(
            scratch_size(Some([1000, 800]), [100, 100], [640, 480]),
            Some([640, 480])
        );
        assert_eq!(downsampled([129, 64], 4), [33, 16]);
        assert_eq!(downsampled([1, 1], 4), [1, 1]);
    }

    #[test]
    fn edge_coverage_is_a_one_pixel_ramp() {
        assert_eq!(edge_coverage(-3.), 1.);
        assert_eq!(edge_coverage(-0.5), 1.);
        assert_eq!(edge_coverage(0.), 0.5);
        assert_eq!(edge_coverage(0.5), 0.);
        assert_eq!(edge_coverage(2.), 0.);
    }

    // zui's kernel tests, unchanged.
    #[test]
    fn regions_cover_composite_samples_and_every_vertical_tap() {
        for source_size in [[1024, 256], [1279, 799], [17, 13]] {
            for downsample in 1..=4 {
                let output = source_size.map(|n: u32| n.div_ceil(downsample));
                for sigma in [0.5, 8.0, 16.0, 100.0] {
                    for origin in [[0.0, 0.0], [31.25, 17.75], [-20.0, -10.0]] {
                        let visible = [origin[0], origin[1], 736.5, 48.5];
                        let (h, v) = blur_regions(
                            visible,
                            [0, 0, source_size[0], source_size[1]],
                            output,
                            sigma,
                        );
                        let inside = |rect: [u32; 4], axis: usize, pixel: i64| {
                            let pixel = pixel.clamp(0, output[axis] as i64 - 1) as u32;
                            pixel >= rect[axis] && pixel < rect[axis] + rect[axis + 2]
                        };
                        for axis in 0..2 {
                            for sample in 0..=100 {
                                let point =
                                    visible[axis] + visible[axis + 2] * sample as f32 / 100.0;
                                if point < 0.0 || point >= source_size[axis] as f32 {
                                    continue;
                                }
                                let texel =
                                    point * output[axis] as f32 / source_size[axis] as f32 - 0.5;
                                assert!(inside(v, axis, texel.floor() as i64));
                                assert!(inside(v, axis, texel.floor() as i64 + 1));
                            }
                            let radius = if axis == 0 {
                                0
                            } else {
                                (3.0 * sigma).ceil() as i64
                            };
                            for pixel in v[axis]..v[axis] + v[axis + 2] {
                                for offset in -radius..=radius {
                                    assert!(inside(h, axis, pixel as i64 + offset));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn regions_are_invariant_under_snapshot_translation() {
        for offset in [[0, 0], [123, 456], [2000, 1000]] {
            assert_eq!(
                blur_regions(
                    [31.25, 17.75, 736.5, 48.5],
                    [0, 0, 1024, 256],
                    [512, 128],
                    8.0
                ),
                blur_regions(
                    [
                        31.25 + offset[0] as f32,
                        17.75 + offset[1] as f32,
                        736.5,
                        48.5
                    ],
                    [offset[0], offset[1], 1024, 256],
                    [512, 128],
                    8.0
                ),
            );
        }
    }

    #[test]
    fn cached_kernel_matches_original_convolution() {
        for sigma in [0.5, 0.75, 1.0, 3.2, 8.0, 11.0, 16.0, 32.0, 42.5] {
            let radius = (sigma * 3.0_f32).ceil() as i32;
            let weights = gaussian_weights(sigma);
            for seed in 0..100 {
                let mut expected = 0.0;
                let mut total = 0.0;
                let mut actual = 0.0;
                for k in -radius..=radius {
                    let sample = ((k * 73 + seed * 37).rem_euclid(256)) as f32 / 255.0;
                    let weight = (-(k as f32) * k as f32 / (2.0 * sigma * sigma)).exp();
                    expected += sample * weight;
                    total += weight;
                    let ix = k.unsigned_abs() as usize;
                    actual += sample * weights[ix / 4][ix % 4];
                }
                assert!((expected / total - actual).abs() < 0.000002);
            }
        }
    }

    #[test]
    fn kernel_weights_sum_to_one() {
        for sigma in [0.5, 1.0, 4.0, 16.0, 42.5] {
            let weights = gaussian_weights(sigma);
            let radius = (sigma * 3.0_f32).ceil() as usize;
            let weight = |k: usize| weights[k / 4][k % 4];
            let sum: f32 = weight(0) + 2.0 * (1..=radius).map(weight).sum::<f32>();
            assert!((sum - 1.0).abs() < 1e-5, "sigma {sigma}: {sum}");
            assert!((1..=radius).all(|k| weight(k) <= weight(k - 1)));
        }
    }

    #[test]
    fn oversized_kernel_preserves_shader_fallback() {
        assert_eq!(gaussian_weights(100.0), [[0.0; 4]; KERNEL_VECTORS]);
    }

    #[test]
    fn paired_linear_samples_preserve_kernel_at_texture_edges() {
        let texels: Vec<f32> = (0..64).map(|x| ((x * 73) % 256) as f32 / 255.0).collect();
        let sample = |x: f32| {
            let x = x.clamp(0.0, (texels.len() - 1) as f32);
            let i = x.floor() as usize;
            let a = texels[i];
            let b = texels[(i + 1).min(texels.len() - 1)];
            a + (b - a) * x.fract()
        };
        for sigma in [0.5, 0.75, 1.0, 3.2, 8.0, 11.0, 16.0, 32.0, 42.5] {
            let radius = (sigma * 3.0_f32).ceil() as i32;
            let weights = gaussian_weights(sigma);
            let weight = |k: usize| weights[k / 4][k % 4];
            for center in 0..64 {
                let center = center as f32;
                let expected: f32 = (-radius..=radius)
                    .map(|k| sample(center + k as f32) * weight(k.unsigned_abs() as usize))
                    .sum();
                let mut actual = sample(center) * weight(0);
                for k in (1..=radius as usize).step_by(2) {
                    let combined = weight(k) + weight(k + 1);
                    let offset = k as f32 + weight(k + 1) / combined;
                    actual += (sample(center + offset) + sample(center - offset)) * combined;
                }
                assert!(
                    (expected - actual).abs() < 0.000002,
                    "sigma={sigma}, center={center}: {expected} vs {actual}"
                );
            }
        }
    }
}
