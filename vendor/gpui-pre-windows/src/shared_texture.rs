//! Textures made outside the renderer that a window draws with `Window::paint_surface`: a web page's frame from
//! another process, for one. Added for Nexus (`desktop-gpui/vendor/README.md`); not in upstream gpui-pre-windows.
//!
//! Threads: GPUI draws each window on the thread that runs its window procedure, the main thread, and only that thread
//! touches the device's immediate context (draws, `End`, `GetData`). Making a [`SharedTexture`] uses the device only
//! (`CreateShaderResourceView`), which Direct3D 11 lets any thread call.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_BIND_SHADER_RESOURCE, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11Query,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter1;
use windows::core::{IUnknown, Interface};

/// The device windows draw with, and its adapter's LUID.
static CURRENT: Mutex<Option<(ID3D11Device, LUID)>> = Mutex::new(None);

/// The device windows draw with, and the LUID of its adapter (the GPU): make or open a [`SharedTexture`]'s texture on
/// it. `None` before GPUI made its devices, and while the app runs with no window system. It changes when the device
/// is lost and GPUI makes a new one: a texture on the old one is then skipped.
pub fn shared_device() -> Option<(ID3D11Device, LUID)> {
    CURRENT.lock().ok()?.clone()
}

/// Called when GPUI makes its devices, again after a lost device, and with `None` when it lets them go.
pub(crate) fn publish(devices: Option<(&ID3D11Device, &IDXGIAdapter1)>) {
    let current = devices.map(|(device, adapter)| {
        let luid = unsafe { adapter.GetDesc1() }
            .map(|desc| desc.AdapterLuid)
            .unwrap_or_default();
        (device.clone(), luid)
    });
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = current;
    }
}

/// A texture to draw as a surface: a 2D texture on [`shared_device`]'s device with `D3D11_BIND_SHADER_RESOURCE`, in
/// a colour format the renderer can sample (`DXGI_FORMAT_B8G8R8A8_UNORM`, as Chromium's frames). Alpha is straight,
/// as for GPUI's own images on this renderer. It is not a keyed-mutex texture
/// (`D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`): nothing here calls `AcquireSync`/`ReleaseSync`, so
/// [`SharedTexture::new`] refuses one. A window whose device is no longer that one skips it rather than fail.
///
/// The renderer keeps the texture, and so its owner ([`SharedTexture::with_owner`]), until the GPU finished the
/// frames that drew it. Drop of the owner is the sign that the texture is free to write again.
pub struct SharedTexture {
    pub(crate) srv: ID3D11ShaderResourceView,
    pub(crate) device: ID3D11Device,
    size: (u32, u32),
    _owner: Option<Box<dyn Any + Send + Sync>>,
}

impl SharedTexture {
    /// A surface of `texture`, viewed on the texture's own device. Open a shared texture on [`shared_device`]'s
    /// device first (`ID3D11Device1::OpenSharedResource1`); a texture on another device is skipped when drawn.
    pub fn new(texture: &ID3D11Texture2D) -> Result<Self> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        anyhow::ensure!(
            desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 != 0,
            "a surface's texture needs D3D11_BIND_SHADER_RESOURCE"
        );
        // Nexus: a keyed-mutex texture must be taken with `AcquireSync` and given back with `ReleaseSync` around each
        // read. This renderer never calls them (the owner's drop is the only sign), so it would sample a texture the
        // producer may be writing. Refuse it here, loudly, rather than draw torn frames.
        anyhow::ensure!(
            desc.MiscFlags & D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32 == 0,
            "a surface's texture cannot have D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX: the renderer never calls \
             AcquireSync or ReleaseSync, so share it with D3D11_RESOURCE_MISC_SHARED (and _NTHANDLE) instead"
        );
        let device = unsafe { texture.GetDevice() }.context("the texture's device")?;
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(texture, None, Some(&mut srv)) }
            .context("a shader resource view of the texture")?;
        Ok(Self {
            srv: srv.context("no shader resource view")?,
            device,
            size: (desc.Width, desc.Height),
            _owner: None,
        })
    }

    /// The same, with `owner` kept until the GPU no longer reads the texture: a lease on a slot of a ring, for one.
    pub fn with_owner(mut self, owner: impl Any + Send + Sync) -> Self {
        self._owner = Some(Box::new(owner));
        self
    }

    /// Its size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Whether it was made on `device`. Compared as COM objects, which two interface pointers of one device are.
    pub(crate) fn is_on(&self, device: &ID3D11Device) -> bool {
        self.device == *device || self.device.cast::<IUnknown>().ok() == device.cast::<IUnknown>().ok()
    }
}

impl std::fmt::Debug for SharedTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedTexture").field("size", &self.size).finish()
    }
}

type Kept = Arc<dyn Any + Send + Sync>;

/// Frames in flight past this, the renderer waits for the oldest. The GPU runs a frame or two behind; more means a
/// stall, and owners must not pile up behind it.
const MAX_IN_FLIGHT: usize = 8;
/// The longest that wait lasts. Past it the owners go anyway: the worst case is a frame drawn from a slot already
/// written again, never a freed texture (Direct3D keeps every resource a queued draw reads).
const MAX_WAIT: Duration = Duration::from_millis(100);

/// The surfaces' textures a renderer drew, kept until the GPU finished the frame that drew them: an event query ends
/// each frame that drew any, and a frame's textures go when its query is done.
#[derive(Default)]
pub(crate) struct SurfaceFrames {
    /// Drawn in the frame being made.
    drawn: Vec<Kept>,
    /// Each presented frame's query and textures, oldest first.
    in_flight: VecDeque<(ID3D11Query, Vec<Kept>)>,
    /// Queries done, to use again.
    spare: Vec<ID3D11Query>,
}

impl SurfaceFrames {
    /// `texture` was drawn in the frame being made.
    pub(crate) fn keep(&mut self, texture: Kept) {
        self.drawn.push(texture);
    }

    /// Before a frame: let go of the textures of each frame the GPU finished. Does not flush.
    pub(crate) fn release_finished(&mut self, context: &ID3D11DeviceContext) {
        while let Some((query, _)) = self.in_flight.front() {
            if !query_done(context, query, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32) {
                break;
            }
            self.pop_with(true);
        }
        if let Some((query, _)) = self.in_flight.front()
            && self.in_flight.len() > MAX_IN_FLIGHT
        {
            let started = Instant::now();
            let mut done = query_done(context, query, 0);
            while !done && started.elapsed() < MAX_WAIT {
                std::thread::yield_now();
                done = query_done(context, query, 0);
            }
            if !done {
                log::warn!("surface textures: the GPU is {} frames behind", self.in_flight.len());
            }
            self.pop_with(done);
        }
    }

    /// At the end of a frame's commands, before `Present` flushes them: the frame's textures go when the GPU passes
    /// this point. A frame that drew no surface adds nothing.
    pub(crate) fn end_frame(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        if self.drawn.is_empty() {
            return;
        }
        let query = self.spare.pop().or_else(|| {
            let desc = D3D11_QUERY_DESC {
                Query: D3D11_QUERY_EVENT,
                MiscFlags: 0,
            };
            let mut query = None;
            unsafe { device.CreateQuery(&desc, Some(&mut query)) }
                .map_err(|error| log::error!("surface textures: no event query: {error}"))
                .ok()
                .and(query)
        });
        // With no query, the textures stay in `drawn` and go with the next frame's.
        let Some(query) = query else {
            return;
        };
        unsafe { context.End(&query) };
        self.in_flight.push_back((query, std::mem::take(&mut self.drawn)));
    }

    /// The device was lost: its work is gone, and its queries with it.
    pub(crate) fn clear(&mut self) {
        self.drawn.clear();
        self.in_flight.clear();
        self.spare.clear();
    }

    /// Let go of the oldest frame's textures. Its query is used again only if done: ending a query still in flight
    /// would move its mark.
    fn pop_with(&mut self, done: bool) {
        if let Some((query, _textures)) = self.in_flight.pop_front()
            && done
        {
            self.spare.push(query);
        }
    }
}

/// Whether the GPU passed `query`. An error (a removed device) counts as done: nothing runs on that device any more.
fn query_done(context: &ID3D11DeviceContext, query: &ID3D11Query, flags: u32) -> bool {
    // An event query's data is a BOOL, TRUE once done. The call's S_FALSE ("not yet") is a success code, which
    // `windows` turns into `Ok`, so the data says which.
    let mut done = 0i32;
    let result = unsafe {
        context.GetData(
            query,
            Some(&mut done as *mut i32 as *mut _),
            size_of::<i32>() as u32,
            flags,
        )
    };
    result.is_err() || done != 0
}
