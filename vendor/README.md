# Patched GPUI crates

These are copies of six of GPUI's crates from crates.io, at the version gpui-kit 0.9.0 pins (0.3.11), and of
`global-hotkey` 0.8.0. `Cargo.toml` uses them in place of the published ones (`[patch.crates-io]`). They add six
things. A GPU texture made outside GPUI's renderer, drawn as a surface on Linux and Windows. On Linux the app draws
web pages' frames this way (`src/web/frames/linux.rs`), with no copy through the CPU. On Windows only a self-test uses it so far
(`src/web/d3d11_selftest.rs`). A BGRA IOSurface drawn as a surface on macOS, for the same frames later. And X11 loops
that sleep while nothing changes. And a zoom for the whole window (`src/zoom.rs`). And a backdrop blur (frosted
glass) on Linux, Windows and macOS. And a mouse wheel's notch that glides as in Chromium.

The copies leave out the crates' examples, benches and tests, and the `[[example]]`, `[[bench]]` and `[[test]]`
entries that name them. Every other change has a comment that starts with "Nexus":

- `gpui-pre/src/elements/animation.rs`: `animate_with_reduced_motion()` lets the sidebar pulse its active marks in place.
- `gpui-pre-apple/src/text_system.rs`: query CoreText's family names directly, and start the first query before AppKit
  finishes launching. Later queries still see installed fonts and fonts registered in memory.
- `gpui-pre/src/scene.rs`: `PaintSurface` has a `texture` field on platforms other than macOS and iOS. Its type is
  the renderer's, unknown to this crate.
- `gpui-pre/src/window.rs`: `Window::paint_surface(bounds, texture)` on those platforms.
- `gpui-pre/src/window.rs`: `before_next_native_draw` runs queued work before an already requested native draw.
  It creates no frame demand. Re-arming it leaves idle windows asleep.
  `app/test_context.rs` exposes `test_window` so downstream tests inspect native frame demand.
  `defer_draws_to_platform` disables synchronous test draws so tests exercise native frame delivery.
- `gpui-pre/src/scene.rs`: on macOS and iOS, `PaintSurface` has an `owner` field, a value the renderer keeps until
  the GPU has finished the frame. `gpui-pre/src/window.rs`: `Window::paint_surface_with_owner(bounds, image_buffer,
  owner)` sets it. When the owner drops, the buffer's producer may write the buffer again.
- `gpui-pre/src/window.rs`: `ui_zoom()` and `set_ui_zoom()`, the app's zoom (Settings › Appearance › Text size). A
  window lays out in its platform size divided by the zoom and draws at its scale factor times the zoom. Input
  positions, and a pixel scroll's delta, come in divided by it.
- `gpui-pre/src/platform.rs`: the text input's bounds go to the platform times the zoom, and a platform point comes in
  divided by it, so the IME's window sits at the caret.
- `gpui-pre/src/interactive.rs`: `scroll_event_time()`, when the device sensed the scroll event being handled
  (`set_scroll_event_time`, `monotonic_ms_instant`). The app resamples touchpad scrolling by it (`src/web/resample.rs`),
  as Chromium does; the time the event reached the app is up to a few milliseconds off.
- `gpui-pre-linux/src/linux/wayland/client.rs`: scroll events carry the axis events' time. `wl_pointer.axis_stop`
  (fingers off the touchpad), which upstream drops, comes as a zero-delta scroll with `TouchPhase::Ended`: the app
  starts a fling there.
- `gpui-pre-linux/src/linux/x11/client.rs`: scroll events carry the XI2 event's time.
- Smooth wheel scrolling. Upstream applies a wheel's notch (60px in a list) in one frame.
  - `gpui-pre/src/wheel_scroll.rs` (new): `WheelScroll` glides a notch along Chromium's wheel curve
    (`cc::ScrollOffsetAnimationCurve`): ease-in-out over 200ms, down to 100ms for long moves, timed on the notch's
    lines at 40px a line as Chromium does. A notch during a glide starts a new leg at the glide's speed. A notch the
    other way drops what was left on its axis and starts from rest. Steps add up to exactly the notches. Only whole
    `ScrollDelta::Lines` glide; pixel deltas (touchpads), fractional lines outside macOS (high-resolution wheels),
    notches with Ctrl, Alt, Cmd or Fn, and reduced motion (`App::reduce_motion`) apply at once, as before.
  - `gpui-pre/src/elements/list.rs` and `div.rs`: the two scroll containers step the glide in `on_next_frame` and
    ask for a frame only while it moves. A scroll by code (`scroll_to`, `scroll_to_end`, `set_offset`, the
    scrollbar, follow mode), a scroll that does not glide, and a mouse press end it. So does reaching an end, or a
    change to a div's offset since the last step.
  - `gpui-pre/src/gestures.rs`: `OngoingScroll` holds a div's `WheelScroll`, so a `ScrollHandle` reaches it.
  - `gpui-pre-linux/src/linux/x11/client.rs`: X11 gives a touchpad's scrolls in lines too. A device with a touchpad
    driver's property (`libinput Tapping Enabled`, `Synaptics Off`) or a dependent touch class is a touchpad, as in
    GTK. Its scrolls go out flagged (`gpui::scroll_event_continuous`), so they are not animated.
- `gpui-pre/src/elements/div.rs`: a scroll box measures its absolutely positioned children apart from its flow
  (`flow_extent`, `absolute_overflow`). Upstream adds the padding to every child's extent, so a background layer
  (`absolute().inset_0()`) counted the padding twice, and a menu over the art scrolled past its end.
  `gpui-pre/src/taffy.rs` and `window.rs`: `is_absolute` and `Window::layout_is_absolute` tell which children those are.
- `gpui-pre-wgpu/src/shared_texture.rs` (new): `SharedTexture` and `shared_device()`, the device and queue to make
  one on. `SharedTexture::with_owner` drops a value with the texture: the app's lease on a frame buffer the web host
  writes (`src/web/frames/linux.rs`), handed back when no frame draws it.
- `gpui-pre-wgpu/src/wgpu_context.rs`: on Linux, the device asks for `VULKAN_EXTERNAL_MEMORY_DMA_BUF` when the GPU
  has it. Each new context publishes its device for `shared_device()`.
- `gpui-pre-wgpu/src/wgpu_renderer.rs`: a surface batch draws each `SharedTexture` with the polychrome sprite
  pipeline, as a sprite that covers the whole texture.
- `gpui-pre/src/platform.rs`: `PlatformAtlas::update(key, rect, bytes)` writes new pixels over part of a tile, in
  place, so an image that changes every frame (a web page's frame) keeps one texture. The default answers
  `Ok(false)` ("can't"), so a backend without it builds unchanged. `AtlasState::update` finds the tile, checks that
  the rect is inside it and that the bytes are its rows (4 bytes a pixel for images), and calls the new
  `AtlasBackend::update`. A tile that was updated and then made again from the image's first pixels (the atlas was
  cleared after GPU errors) answers `Ok(false)` until its key is removed. `HeadlessAtlas` accepts updates, as it
  accepts inserts.
- `gpui-pre/src/window.rs`: `Window::update_image(image, frame_index, rect, bytes)`, the same for an image that
  `paint_image` put in the atlas. `Ok(false)` tells the caller to make a new image. The `RenderImage` keeps the
  pixels it was made with.
- `gpui-pre-wgpu/src/wgpu_atlas.rs`: `update` queues the bytes as an insert's upload is (same swizzle to the
  texture's format), so `before_frame` writes them with `queue.write_texture` before the next frame's draws. A
  queued upload that a newer one covers is dropped. Uploads that only part-overlap stay, but the queue for one tile is
  bounded (64 uploads, twice the tile's bytes): past that `update` answers `Ok(false)` and the caller makes a new
  image, and removing a tile drops its queued uploads. Under `cfg(test)` the atlas textures can be copied from, so tests
  read them back.
- `gpui-pre-linux/src/linux/wayland/window.rs`: layer-shell surfaces stay frameless when decorations are requested.
  Client frame shadows otherwise shrink their layer size during resize, clipping the browser activity indicator.
- `gpui-pre-linux/src/linux/x11/window.rs` and `client.rs`: the X11 refresh timer stops when a frame request had
  nothing to draw, and GPUI's frame waker (`PlatformWindow::frame_waker`, which upstream implements on macOS only)
  restarts it. Upstream requests a frame every vsync for as long as a window is visible, about 60 main-thread
  wakeups a second on a window where nothing changes. `FrameDemand` in `window.rs` has the details.
- `gpui-pre-linux/src/linux/x11/display.rs`: `refresh_interval` gives the RandR mode rate of the screen's first lit
  CRTC, the one the X11 refresh timer draws at. Upstream returns `None` (an X screen spans every monitor). Web pages
  paint at the display's rate (`src/web/mod.rs`'s `frame_rate`), so on X11 they had 60 whatever the monitor's rate.
- `gpui-pre-linux/src/linux/xdg_desktop_portal.rs`, `wayland/client.rs` and `x11/client.rs`: the client waits up to
  250 ms (`INITIAL_APPEARANCE_WAIT`) for the desktop portal's first color scheme before it opens windows. Upstream
  starts at Light and learns the scheme later, so on a dark desktop the first frames drew light.
- `gpui-pre-linux/src/linux/platform.rs`, `linux.rs` and `gpui_linux.rs`: `set_fallback_appearance`, the appearance
  windows start in until the portal answers, in place of Light. The app passes the last one it saw
  (`src/appearance.rs`), so a slow or missing portal does not open a dark desktop's windows light.
- `gpui-pre-linux/src/linux/platform.rs`, `linux.rs`, `gpui_linux.rs` and `wayland/client.rs`:
  `window_blur_available()` says whether a `Blurred` window blurs here. It is true on Wayland when the compositor
  offers `ext-background-effect-v1` or KDE's blur, and false on X11. The app keeps its window opaque when it is false
  (`src/frost.rs`), so text does not sit on a clear, unblurred desktop.
- `gpui-pre-wgpu/src/wgpu_renderer.rs`: text gamma 1.2 and grayscale contrast 0.2 by default (upstream 1.8 and 1.0).
  Upstream drew DM Sans about 20% heavier than Chromium at 12 to 14px, light and dark. These values match the Frost
  board's weight within 8%. `ZED_FONTS_GAMMA` and `ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST` still override them.
- `global-hotkey/src/platform_impl/x11/mod.rs`: the thread that reads the global shortcut's key presses waits in
  `poll` on the X connection and a socket the manager writes to with each message. Upstream checked both every 50 ms,
  20 wakeups a second for the life of the app. The crate's `Cargo.toml` adds `libc` for `poll`.
- `gpui-pre-macos/src/window.rs`: three calls to `Option::map_or_default` become `.map(f).unwrap_or_default()`.
  `map_or_default` is unstable on the pinned Rust (1.97), so the macOS build failed without this.
- `gpui-pre-apple/src/shaders.metal` and `metal_renderer.rs`: a surface can also be a one-plane BGRA pixel buffer
  (`kCVPixelFormatType_32BGRA`), a web page's frame. Its own pipeline (`surface_bgra_fragment`) samples it as
  `BGRA8Unorm`, as an image's bytes. A surface the renderer can't draw is skipped, logged once per format. Upstream
  ended the frame's encoding there, so the window showed nothing. Each frame keeps its surfaces' owners until the
  command buffer completes, as it keeps the CoreVideo textures. `NEXUS_WEB_SELFTEST=surface` checks this on a Mac
  (`src/web/selftest.rs`).
- `gpui-pre-apple/src/metal_atlas.rs` and `metal_renderer.rs`: `update` copies the bytes into a buffer and queues
  it. Before each frame, the queue is blitted into the atlas in a command buffer of its own, committed before the
  frame's, so a frame the GPU still draws reads the old pixels and a frame that fails does not lose the updates. An
  insert uses this queue for color images of at least 256 KiB. Smaller inserts keep the direct CPU copy.
  The GPU receives the new pixels before drawing them. A failed upload commit stops that frame.
  The queue is bounded as in the wgpu atlas (64 updates, twice the tile's bytes).
  Removing a tile drops its uploads.
- `gpui-pre-windows/src/shared_texture.rs` (new): `SharedTexture`, a D3D11 texture to draw as a surface, with an
  owner that drops once the GPU drew it. `shared_device()`: the device to make it on, and its adapter's LUID.
  `SurfaceFrames`: keeps each frame's surface textures until the frame's event query is done. `SharedTexture::new`
  rejects a keyed-mutex texture (`D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`): nothing calls `AcquireSync`/`ReleaseSync`.
- `gpui-pre-windows/src/directx_devices.rs` and `platform.rs`: each new set of devices publishes its device for
  `shared_device()`, after a lost device too. Releasing the devices clears it.
- `gpui-pre-windows/src/directx_renderer.rs`: a surface batch draws each `SharedTexture` on the current device with
  the polychrome sprite shaders, as a sprite that covers the whole texture, from a buffer of its own. A frame that drew
  surfaces ends with an event query, just before `Present`. The next frames drop its textures once `GetData` says done.
  A lost device drops them all.
- `gpui-pre-windows/src/directx_atlas.rs`: `update` writes the bytes with `UpdateSubresource`, as an insert's upload
  is. The immediate context keeps the order with the frames' draws and copies the bytes at the call, so nothing waits
  in the atlas and there is no queue to bound. Tests on WARP read the tile back.
- `gpui-pre/src/layout_dump.rs` (new): the layout dump for the polish tools (`polish/README.md`). When the app turns it
  on (`NEXUS_LAYOUT_DUMP`), `Interactivity::paint` (`elements/div.rs`), `Svg`, `Img`, `ShapedLine` and `WrappedLine`
  record each painted box and text line with its bounds, style and source line; `window.rs` brackets each frame, and
  `view.rs` skips the view cache so every element paints. Off, each hook is one atomic load.
- Backdrop blur, ported from zui (Zeron's GPUI, <https://github.com/zeronsh/zui>, Apache-2.0, as GPUI is). The
  ported code keeps a comment that names zui.
  - `gpui-pre/src/scene.rs`: `BackdropBlur` and the scene's `backdrop_blurs`. Batches split at each blur's order, so
    a renderer blurs what was drawn before it. zui's scene tests come with it.
  - `gpui-pre/src/window.rs`: `Window::paint_backdrop_blur(bounds, corner_radii, blur_radius)`, as in zui. Paint it
    first inside `paint_layer`, then draw the tint over it. A blur inside the layer of another blurs that glass too.
  - `gpui-pre/src/backdrop_blur.rs` (new): the math the three renderers share. It clips each blur to its content
    mask and the frame, and clamps the radius (Gaussian sigma) to 64 device pixels. It downsamples big blurs and
    sizes the scratch textures. It also gives the kernel weights and the edge coverage. A frame draws 32 blurs at
    most. Scratch textures go after 30 frames without a blur.
  - `gpui-pre-wgpu/src/wgpu_backdrop.rs` (new), `wgpu_renderer.rs` and `shaders.wgsl`: each blur ends the render
    pass and copies the region under it. Two Gaussian passes blur it, and a composite draws it back. The composite
    antialiases the rounded corners by mixing the blurred and sharp pixels. zui's corners are hard. When the surface
    can't be a copy source, a frame that blurs draws into a texture that can. Then it is blitted to the surface. A
    frame without a blur draws to the surface directly, as before.
  - `gpui-pre-windows/src/directx_renderer/backdrop.rs` (new), `directx_renderer.rs`, `shaders.hlsl` and `build.rs`:
    the same on D3D11, from zui's D3D11 port. `backdrop_tests.rs` holds zui's WARP pixel tests.
  - `gpui-pre-apple/src/metal_renderer.rs`, `shaders.metal` and `build.rs`: the same on Metal, with two MSL passes
    in place of zui's `MPSImageGaussianBlur`. A blit needs a drawable that is not framebuffer-only. The layer turns
    `framebufferOnly` off when a frame blurs. It turns it back on after 30 frames without a blur.
- `gpui-pre-wgpu/src/shaders.wgsl`, `gpui-pre-windows/src/shaders.hlsl` and `gpui-pre-apple/src/shaders.metal`: a drop
  shadow paints only outside its element, as CSS `box-shadow` does. Upstream paints it under the whole element. A
  see-through element (a frosted dialog) then showed its own shadow, and its backdrop blur smeared that shadow in.
  `wgpu_renderer.rs` has a pixel test.
- `gpui-pre-wgpu/src/shaders.wgsl`: shadows and the backdrop blur's composite are dithered (`dither_over`,
  `dither_noise`). A dark shadow over a dark window fades through only a few 8-bit levels and showed flat bands; a
  noise of up to one level lets each pixel round its own way. `wgpu_renderer.rs` has a pixel test. Metal and D3D11
  are not dithered yet.

To update GPUI or global-hotkey, copy the new versions from `~/.cargo/registry/src/*/` and apply the changes again.
Remove this folder when gpui-kit's GPUI draws external textures itself, or move the changes to a fork.
