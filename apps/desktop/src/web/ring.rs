//! The web host's frame ring (Linux): Chromium's frames copied, in the host, into a few GPU buffers the app draws
//! from directly.
//!
//! Chromium lends its frame buffer only for the paint callback. Copying it in the app made Chromium wait for the app's
//! GPU queue, which runs behind the app's own drawing: about one screen refresh per frame. Here the host copies it on a
//! Vulkan device of its own, on the app's GPU, into one of [`SLOTS`] buffers it shares with the app. The copy is done
//! in well under a millisecond, and Chromium goes on. The app imports each slot once and draws it as it is, with no
//! copy of its own; it hands a slot back when no frame draws it any more (`frames/linux.rs`'s `Lease`).
//!
//! A slot is written only while the app does not hold it. With none free (the app is behind), the frame is dropped,
//! and Chromium paints the page again when the app hands a slot back (`frames/linux.rs`). The slots are linear buffers: any GPU can share them, and a single frame's sampling does not
//! need a tiled layout to be fast.
//!
//! The paint callback does not wait for the copy. Chromium hands its buffer over before its own GPU work on it is done
//! (the copy waits for that work), and the callback runs on CEF's main thread, which also starts each frame. Waiting
//! there held the next frame back whenever the GPU was busy. The callback queues the copy and returns; a thread waits
//! for it and sends the frame. The copy's fence goes on Chromium's buffer, so Chromium's next drawing into it waits
//! for the copy, on drivers that order shared buffers. Where no fence can go on it (no exportable semaphore, a kernel
//! before 6.0), the callback waits for the copy, as the old path did.

use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ash::vk;

/// Buffers per page: one the app shows, one on its way, and room for Chromium to paint ahead.
pub(super) const SLOTS: usize = 4;
/// DRM's linear layout.
const MOD_LINEAR: u64 = 0;
/// DRM's "no modifier": the layout is the allocating driver's, recorded in the buffer.
const MOD_INVALID: u64 = 0x00FF_FFFF_FFFF_FFFF;
/// Imports of Chromium's buffers kept per page: more than the buffers it paints into in turn.
const IMPORTS: usize = 6;
/// A copy slower than this is logged, and the paint callback drops its frame rather than wait for a free submission.
/// A busy GPU or a wake from suspend can take this long.
const COPY_WAIT: Duration = Duration::from_millis(200);
/// A copy slower than this means a hung GPU: the ring stops. A lost device stops it at once.
const COPY_GIVE_UP: Duration = Duration::from_secs(10);
/// Copies queued at once: a few frames, when the GPU is busy.
const IN_FLIGHT: usize = 6;
/// `DMA_BUF_IOCTL_IMPORT_SYNC_FILE`, and its flag for a reader's fence: writers wait for it.
const IMPORT_SYNC_FILE: libc::c_ulong = 0x4008_6203;
const DMA_BUF_SYNC_READ: u32 = 1;

/// A frame copied into a slot: what the app needs to import and draw it.
pub(super) struct Copied {
    /// The slot, with its ring's generation (`slot_word`).
    pub slot: u64,
    pub fd: Arc<OwnedFd>,
    pub width: u32,
    pub height: u32,
    /// CEF's colour type, as Chromium's frame had it.
    pub color: u32,
    pub stride: u32,
    pub offset: u64,
    pub size: u64,
}

/// Why a frame was not copied.
pub(super) enum Miss {
    /// Every slot is the app's: drop the frame.
    Full,
    /// The ring can't work here (no device, an import or a copy failed): use the old path from now on.
    Broken(String),
}

/// The host's GPU device, and each page's ring.
pub(super) struct Rings {
    /// Before `gpu`: fields drop in order, and the pages' images must go before their device.
    pages: HashMap<u32, Page>,
    /// Pages that closed. Chromium may paint a closing page once more; that frame is dropped, not given a new ring.
    closed: HashSet<u32>,
    gpu: Gpu,
}

impl Rings {
    /// A device on the GPU whose Vulkan device UUID is `uuid`: the app's.
    pub(super) fn new(uuid: [u8; 16]) -> Result<Rings, String> {
        Ok(Rings {
            pages: HashMap::new(),
            closed: HashSet::new(),
            gpu: Gpu::new(uuid)?,
        })
    }

    /// Queue the copy of Chromium's frame for page `id` into a free slot. `deliver` gets the frame, on another thread,
    /// once the copy is done.
    pub(super) fn copy(
        &mut self,
        id: u32,
        info: &cef::AcceleratedPaintInfo,
        visible: (u32, u32, u32, u32),
        deliver: impl FnOnce(Copied) + Send + 'static,
    ) -> Result<(), Miss> {
        if self.closed.contains(&id) {
            return Err(Miss::Full);
        }
        let count = (info.plane_count.max(0) as usize).min(4);
        if count == 0 {
            return Err(Miss::Broken("a frame with no planes".into()));
        }
        let color = *info.format.as_ref() as u32;
        let format = match color {
            0 => vk::Format::R8G8B8A8_UNORM,
            1 => vk::Format::B8G8R8A8_UNORM,
            other => return Err(Miss::Broken(format!("unknown colour type {other}"))),
        };
        let source = Source {
            width: info.extra.coded_size.width.max(0) as u32,
            height: info.extra.coded_size.height.max(0) as u32,
            format,
            modifier: info.modifier,
            planes: info.planes[..count]
                .iter()
                .map(|plane| (plane.stride, plane.offset))
                .collect(),
        };
        let (_, _, width, height) = visible;
        if width == 0 || height == 0 {
            return Err(Miss::Broken("an empty frame".into()));
        }
        // SAFETY: CEF lends this descriptor for the callback, which outlives this call.
        let fd = unsafe { BorrowedFd::borrow_raw(info.planes[0].fd) };
        let gpu = &self.gpu;
        let page = self.pages.entry(id).or_default();
        if page.ring.as_ref().is_none_or(|ring| !ring.fits(width, height, format)) {
            // The old slots go: no copy may still write them.
            gpu.settle().map_err(Miss::Broken)?;
            // The app keeps its imports of the old slots alive until it lets them go; their releases are ignored.
            let generation = page.ring.as_ref().map_or(0, |ring| ring.generation + 1);
            page.ring = Some(Ring::new(gpu, width, height, format, color, generation).map_err(Miss::Broken)?);
        }
        let ring = page.ring.as_mut().expect("made above");
        let Some(index) = ring.slots.iter().position(|slot| !slot.lent) else {
            return Err(Miss::Full);
        };
        let key = buffer_key(fd).map_err(Miss::Broken)?;
        let image = page.imports.get(gpu, key, fd, &source).map_err(Miss::Broken)?;
        let ring = page.ring.as_mut().expect("made above");
        let slot = &ring.slots[index];
        let copied = Copied {
            slot: slot_word(ring.generation, index),
            fd: slot.fd.clone(),
            width,
            height,
            color: ring.color,
            stride: slot.stride,
            offset: slot.offset,
            size: slot.size,
        };
        gpu.copy(image, slot.image(), visible, fd, Box::new(move || deliver(copied)))?;
        ring.slots[index].lent = true;
        Ok(())
    }

    /// The app let go of `slot` of page `id`'s ring.
    pub(super) fn release(&mut self, id: u32, slot: u64) {
        let Some(ring) = self.pages.get_mut(&id).and_then(|page| page.ring.as_mut()) else {
            return;
        };
        if let Some(slot) = slot_index(ring.generation, slot).and_then(|index| ring.slots.get_mut(index)) {
            slot.lent = false;
        }
    }

    /// Page `id` closed: free its ring and imports, once no copy uses them.
    pub(super) fn forget(&mut self, id: u32) {
        if self.pages.contains_key(&id) {
            let _ = self.gpu.settle();
        }
        self.pages.remove(&id);
        self.closed.insert(id);
    }
}

impl Drop for Rings {
    fn drop(&mut self) {
        // The pages' images go first (field order): no copy may still use them.
        let _ = self.gpu.settle();
    }
}

/// A slot's number on the socket: its ring's generation above, its index in the low byte ([`SLOTS`] fits in it).
fn slot_word(generation: u64, index: usize) -> u64 {
    generation << 8 | index as u64
}

/// The slot a [`slot_word`] names in the ring of `generation`. `None` for a slot of an older ring: the app may hand
/// one back after a resize, and it must not free the new ring's slot of the same index.
fn slot_index(generation: u64, word: u64) -> Option<usize> {
    (word >> 8 == generation).then_some((word & 0xFF) as usize)
}

/// A buffer's identity: its file's device and inode. Each dma-buf has an inode of its own.
fn buffer_key(fd: BorrowedFd) -> Result<(u64, u64), String> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a valid descriptor, and room for one `stat`.
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(format!("stat the buffer: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: `fstat` filled it.
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_dev as u64, stat.st_ino as u64))
}

/// Chromium's buffer, as its paint callback describes it.
struct Source {
    width: u32,
    height: u32,
    format: vk::Format,
    modifier: u64,
    /// Each plane's stride and offset.
    planes: Vec<(u32, u64)>,
}

#[derive(Default)]
struct Page {
    ring: Option<Ring>,
    imports: Imports,
}

/// A page's slots, all of one size and format.
struct Ring {
    width: u32,
    height: u32,
    format: vk::Format,
    color: u32,
    generation: u64,
    slots: Vec<Slot>,
}

impl Ring {
    fn new(
        gpu: &Gpu,
        width: u32,
        height: u32,
        format: vk::Format,
        color: u32,
        generation: u64,
    ) -> Result<Ring, String> {
        let slots = (0..SLOTS)
            .map(|_| Slot::new(gpu, width, height, format))
            .collect::<Result<_, _>>()?;
        Ok(Ring {
            width,
            height,
            format,
            color,
            generation,
            slots,
        })
    }

    fn fits(&self, width: u32, height: u32, format: vk::Format) -> bool {
        self.width == width && self.height == height && self.format == format
    }
}

/// A buffer the host writes and the app reads: a linear image in exported memory.
struct Slot {
    owned: Owned,
    /// The memory as a dma-buf, sent with each frame in it.
    fd: Arc<OwnedFd>,
    stride: u32,
    offset: u64,
    size: u64,
    /// The app holds it: a frame in it is shown or on its way.
    lent: bool,
}

impl Slot {
    fn new(gpu: &Gpu, width: u32, height: u32, format: vk::Format) -> Result<Slot, String> {
        let device = &gpu.device;
        let modifiers = [MOD_LINEAR];
        let mut list = vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
        let mut external =
            vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external)
            .push_next(&mut list);
        // SAFETY: a valid device and create info.
        let image = unsafe { device.create_image(&info, None) }.map_err(|error| format!("slot image: {error}"))?;
        // Destroys the image, and the memory once there is some, if a step below fails.
        let mut owned = Owned {
            device: device.clone(),
            image,
            memory: vk::DeviceMemory::null(),
        };
        // SAFETY: `image` is this device's.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let memory_type = gpu
            .memory_type(requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .ok_or("no memory type for a slot")?;
        let mut export =
            vk::ExportMemoryAllocateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut export)
            .push_next(&mut dedicated);
        // SAFETY: a valid allocate info for this device.
        owned.memory =
            unsafe { device.allocate_memory(&allocate, None) }.map_err(|error| format!("slot memory: {error}"))?;
        // SAFETY: fresh image and memory, of this device.
        unsafe { device.bind_image_memory(image, owned.memory, 0) }.map_err(|error| format!("bind slot: {error}"))?;
        let get = vk::MemoryGetFdInfoKHR::default()
            .memory(owned.memory)
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        // SAFETY: the memory was allocated exportable as a dma-buf.
        let fd = unsafe { gpu.fd_api.get_memory_fd(&get) }.map_err(|error| format!("export slot: {error}"))?;
        // SAFETY: Vulkan gave this descriptor to the caller.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: a linear-modifier image has one memory plane.
        let layout = unsafe {
            device.get_image_subresource_layout(
                image,
                vk::ImageSubresource {
                    aspect_mask: vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
                    mip_level: 0,
                    array_layer: 0,
                },
            )
        };
        Ok(Slot {
            owned,
            fd: Arc::new(fd),
            stride: layout.row_pitch as u32,
            offset: layout.offset,
            size: layout.size,
            lent: false,
        })
    }

    fn image(&self) -> vk::Image {
        self.owned.image
    }
}

/// An image and its memory on the host's device, destroyed with it.
struct Owned {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: images are dropped only once the copies are done: `Gpu::settle` returns only when the device is
        // idle, whether it succeeds or fails, and every drop of images follows it.
        unsafe {
            self.device.destroy_image(self.image, None);
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

/// A page's imports of Chromium's buffers, by buffer, with the frame each was last used for.
#[derive(Default)]
struct Imports {
    kept: Vec<((u64, u64), Import, u64)>,
    frame: u64,
}

impl Imports {
    fn get(&mut self, gpu: &Gpu, key: (u64, u64), fd: BorrowedFd, source: &Source) -> Result<vk::Image, String> {
        self.frame += 1;
        let frame = self.frame;
        let stale = |(other, import, _): &((u64, u64), Import, u64)| {
            (*other == key && !import.fits(source)) || import.width != source.width || import.height != source.height
        };
        let known = self.kept.iter().any(|(other, ..)| *other == key);
        if self.kept.iter().any(stale) || (!known && self.kept.len() >= IMPORTS) {
            // Imports go below: no copy may still read them.
            gpu.settle()?;
        }
        // A buffer of another size or layout is the page's past: it will not come back.
        self.kept
            .retain(|(other, import, _)| *other != key || import.fits(source));
        self.kept
            .retain(|(_, import, _)| import.width == source.width && import.height == source.height);
        if let Some(entry) = self.kept.iter_mut().find(|(other, ..)| *other == key) {
            entry.2 = frame;
            return Ok(entry.1.owned.image);
        }
        let import = Import::new(gpu, fd, source)?;
        if self.kept.len() >= IMPORTS
            && let Some(oldest) = (0..self.kept.len()).min_by_key(|index| self.kept[*index].2)
        {
            self.kept.swap_remove(oldest);
        }
        let image = import.owned.image;
        self.kept.push((key, import, frame));
        Ok(image)
    }
}

/// Chromium's buffer imported on the host's device. Holding it keeps the buffer alive, so its inode stays its own.
struct Import {
    owned: Owned,
    width: u32,
    height: u32,
    format: vk::Format,
    modifier: u64,
    planes: Vec<(u32, u64)>,
}

impl Import {
    fn fits(&self, source: &Source) -> bool {
        self.width == source.width
            && self.height == source.height
            && self.format == source.format
            && self.modifier == source.modifier
            && self.planes == source.planes
    }

    fn new(gpu: &Gpu, fd: BorrowedFd, source: &Source) -> Result<Import, String> {
        let device = &gpu.device;
        let layouts: Vec<_> = source
            .planes
            .iter()
            .map(|(stride, offset)| vk::SubresourceLayout {
                offset: *offset,
                row_pitch: u64::from(*stride),
                ..Default::default()
            })
            .collect();
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(source.modifier)
            .plane_layouts(&layouts);
        let mut external =
            vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(source.format)
            .extent(vk::Extent3D {
                width: source.width,
                height: source.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut external);
        info = if source.modifier == MOD_INVALID {
            // The driver takes the layout from the buffer's own metadata when the memory is bound (`dmabuf.rs`).
            info.tiling(vk::ImageTiling::OPTIMAL)
        } else {
            info.tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .push_next(&mut explicit)
        };
        // SAFETY: a valid create info for this device.
        let image = unsafe { device.create_image(&info, None) }.map_err(|error| format!("import image: {error}"))?;
        let mut import = Import {
            owned: Owned {
                device: device.clone(),
                image,
                memory: vk::DeviceMemory::null(),
            },
            width: source.width,
            height: source.height,
            format: source.format,
            modifier: source.modifier,
            planes: source.planes.clone(),
        };
        // SAFETY: `image` is this device's.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let mut properties = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: a valid dma-buf descriptor.
        unsafe {
            gpu.fd_api.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut properties,
            )
        }
        .map_err(|error| format!("dma-buf properties: {error}"))?;
        let memory_type = gpu
            .memory_type(
                requirements.memory_type_bits & properties.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
            .ok_or("no memory type fits Chromium's buffer")?;
        // Vulkan owns the descriptor once the import succeeds: give it a copy.
        let owned = fd
            .try_clone_to_owned()
            .map_err(|error| format!("duplicate the descriptor: {error}"))?;
        let mut import_info = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(owned.as_raw_fd());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut import_info)
            .push_next(&mut dedicated);
        // SAFETY: a valid allocate info importing a dma-buf.
        import.owned.memory =
            unsafe { device.allocate_memory(&allocate, None) }.map_err(|error| format!("import memory: {error}"))?;
        // Vulkan owns it now.
        std::mem::forget(owned);
        // SAFETY: fresh image and memory, of this device.
        unsafe { device.bind_image_memory(image, import.owned.memory, 0) }
            .map_err(|error| format!("bind import: {error}"))?;
        Ok(import)
    }
}

/// The host's Vulkan device: one queue, and [`IN_FLIGHT`] command buffers, each with its fence and semaphore. A thread
/// (`wait_copies`) waits for each copy's fence, sends its frame, and hands the command buffer back.
///
/// The module's header says why a device of its own: its queue is not behind the app's drawing.
struct Gpu {
    _entry: ash::Entry,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    fd_api: ash::khr::external_memory_fd::Device,
    queue: vk::Queue,
    family: u32,
    /// The queue family Chromium's buffers come from and go back to.
    foreign: u32,
    pool: vk::CommandPool,
    submissions: Vec<Submission>,
    /// Submissions not in use, and those the waiting thread handed back.
    free: std::cell::RefCell<Vec<usize>>,
    done: mpsc::Receiver<usize>,
    /// Copies for the waiting thread. Taken at drop, to end it.
    jobs: Option<mpsc::Sender<Job>>,
    waiter: Option<std::thread::JoinHandle<()>>,
    /// Exports a copy's semaphore as a sync file, for Chromium's buffer. `None`: the driver can't.
    semaphore_api: Option<ash::khr::external_semaphore_fd::Device>,
    /// Cleared when an export fails, or the kernel can't put its sync file on a buffer.
    semaphores_work: std::cell::Cell<bool>,
    /// A copy with no reader fence was waited for, and said so once.
    told_unfenced: std::cell::Cell<bool>,
    /// Why the GPU stopped, from the waiting thread.
    broken: Arc<Mutex<Option<String>>>,
}

/// A command buffer, the fence the waiting thread waits on, and the semaphore whose sync file goes on Chromium's buffer.
struct Submission {
    command: vk::CommandBuffer,
    fence: vk::Fence,
    semaphore: vk::Semaphore,
}

/// A copy for the waiting thread: its submission, and what sends its frame.
struct Job {
    index: usize,
    fence: vk::Fence,
    deliver: Box<dyn FnOnce() + Send>,
}

/// The waiting thread: wait for each copy in turn (the queue runs them in order), send its frame, and hand its
/// submission back. A lost device, or a copy not done in [`COPY_GIVE_UP`], stops the ring (`broken`). A copy slower
/// than [`COPY_WAIT`] is only logged.
fn wait_copies(
    device: ash::Device,
    jobs: mpsc::Receiver<Job>,
    done: mpsc::Sender<usize>,
    broken: Arc<Mutex<Option<String>>>,
) {
    for job in jobs {
        let started = Instant::now();
        let mut slow = false;
        let waited = loop {
            // SAFETY: the fence is this device's, and only this thread uses it until it hands the submission back.
            match unsafe { device.wait_for_fences(&[job.fence], true, COPY_WAIT.as_nanos() as u64) } {
                Err(vk::Result::TIMEOUT) if started.elapsed() < COPY_GIVE_UP => {
                    if !slow {
                        slow = true;
                        eprintln!(
                            "nexus web host: a frame copy takes over {}ms (a busy GPU, or a wake from sleep); waiting",
                            COPY_WAIT.as_millis()
                        );
                    }
                }
                Err(vk::Result::TIMEOUT) => break Err(format!("a copy took over {}s", COPY_GIVE_UP.as_secs())),
                other => break other.map_err(|error| format!("the copy: {error}")),
            }
        };
        if let Err(why) = waited {
            *broken.lock().expect("ring broken") = Some(why);
            return;
        }
        // SAFETY: as above; the fence signalled.
        if let Err(error) = unsafe { device.reset_fences(&[job.fence]) } {
            *broken.lock().expect("ring broken") = Some(format!("reset fence: {error}"));
            return;
        }
        (job.deliver)();
        if done.send(job.index).is_err() {
            return;
        }
    }
}

impl Gpu {
    fn new(uuid: [u8; 16]) -> Result<Gpu, String> {
        // SAFETY: loads the system's Vulkan loader.
        let entry = unsafe { ash::Entry::load() }.map_err(|error| format!("no Vulkan: {error}"))?;
        let app = vk::ApplicationInfo::default()
            .application_name(c"nexus-web-host")
            .api_version(vk::API_VERSION_1_2);
        // SAFETY: a valid create info.
        let instance =
            unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None) }
                .map_err(|error| format!("Vulkan instance: {error}"))?;
        // From here, a failure destroys the instance.
        let destroy = |instance: &ash::Instance| unsafe { instance.destroy_instance(None) };
        let made = (|| {
            // SAFETY: a live instance.
            let physical = unsafe { instance.enumerate_physical_devices() }
                .map_err(|error| format!("GPUs: {error}"))?
                .into_iter()
                .find(|physical| {
                    let mut id = vk::PhysicalDeviceIDProperties::default();
                    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
                    // SAFETY: a physical device of this instance.
                    unsafe { instance.get_physical_device_properties2(*physical, &mut properties) };
                    id.device_uuid == uuid
                })
                .ok_or("the app's GPU is not among Vulkan's")?;
            // SAFETY: a physical device of this instance.
            let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
            // A graphics family: on an AMD laptop (2026-11-08) its copies waited 1.0ms against 1.2ms on a compute-only
            // family. `NEXUS_WEB_RING_QUEUE=compute` asks for a compute-only one (AMD's async compute), to measure on
            // another GPU. A transfer-only family (a DMA engine) is not used: some drivers' can't read compressed
            // images.
            let compute = std::env::var("NEXUS_WEB_RING_QUEUE").is_ok_and(|queue| queue == "compute");
            let family = families
                .iter()
                .position(|family| {
                    compute
                        && family.queue_flags.contains(vk::QueueFlags::COMPUTE)
                        && !family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                })
                .or_else(|| {
                    families
                        .iter()
                        .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                })
                .ok_or("no queue that copies")? as u32;
            // SAFETY: a physical device of this instance.
            let available = unsafe { instance.enumerate_device_extension_properties(physical) }
                .map_err(|error| format!("extensions: {error}"))?;
            let has = |name: &CStr| {
                available
                    .iter()
                    .any(|extension| extension.extension_name_as_c_str().is_ok_and(|other| other == name))
            };
            let mut extensions = vec![
                ash::khr::external_memory_fd::NAME,
                ash::ext::external_memory_dma_buf::NAME,
                ash::ext::image_drm_format_modifier::NAME,
            ];
            if let Some(missing) = extensions.iter().find(|name| !has(name)) {
                return Err(format!("the GPU lacks {}", missing.to_string_lossy()));
            }
            let foreign_family = has(ash::ext::queue_family_foreign::NAME);
            if foreign_family {
                extensions.push(ash::ext::queue_family_foreign::NAME);
            }
            // A copy's semaphore, exported as a sync file, goes on Chromium's buffer. Without it the kernel's own
            // ordering of shared buffers (amdgpu, i915) still holds.
            let semaphores = has(ash::khr::external_semaphore_fd::NAME) && {
                let mut properties = vk::ExternalSemaphoreProperties::default();
                let info = vk::PhysicalDeviceExternalSemaphoreInfo::default()
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
                // SAFETY: a physical device of this instance.
                unsafe { instance.get_physical_device_external_semaphore_properties(physical, &info, &mut properties) };
                properties
                    .external_semaphore_features
                    .contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE)
            };
            if semaphores {
                extensions.push(ash::khr::external_semaphore_fd::NAME);
            }
            let names: Vec<_> = extensions.iter().map(|name| name.as_ptr()).collect();
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&priorities)];
            let info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&names);
            // SAFETY: a valid create info for this physical device.
            let device = unsafe { instance.create_device(physical, &info, None) }
                .map_err(|error| format!("Vulkan device: {error}"))?;
            let made = (|| {
                // SAFETY: the queue asked for above.
                let queue = unsafe { device.get_device_queue(family, 0) };
                let pool_info = vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(family);
                // SAFETY: valid create infos for this device.
                let pool = unsafe { device.create_command_pool(&pool_info, None) }
                    .map_err(|error| format!("command pool: {error}"))?;
                // From here, a failure destroys what was made (the pool frees its command buffers).
                let mut submissions = Vec::new();
                let made = (|| {
                    let commands = unsafe {
                        device.allocate_command_buffers(
                            &vk::CommandBufferAllocateInfo::default()
                                .command_pool(pool)
                                .level(vk::CommandBufferLevel::PRIMARY)
                                .command_buffer_count(IN_FLIGHT as u32),
                        )
                    }
                    .map_err(|error| format!("command buffers: {error}"))?;
                    for command in commands {
                        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
                            .map_err(|error| format!("fence: {error}"))?;
                        let mut submission = Submission {
                            command,
                            fence,
                            semaphore: vk::Semaphore::null(),
                        };
                        if semaphores {
                            let mut export = vk::ExportSemaphoreCreateInfo::default()
                                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
                            let info = vk::SemaphoreCreateInfo::default().push_next(&mut export);
                            match unsafe { device.create_semaphore(&info, None) } {
                                Ok(semaphore) => submission.semaphore = semaphore,
                                Err(error) => {
                                    submissions.push(submission);
                                    return Err(format!("semaphore: {error}"));
                                }
                            }
                        }
                        submissions.push(submission);
                    }
                    Ok::<_, String>(())
                })();
                if let Err(error) = made {
                    // SAFETY: none of these is in use.
                    unsafe {
                        for submission in &submissions {
                            device.destroy_fence(submission.fence, None);
                            if submission.semaphore != vk::Semaphore::null() {
                                device.destroy_semaphore(submission.semaphore, None);
                            }
                        }
                        device.destroy_command_pool(pool, None);
                    }
                    return Err(error);
                }
                Ok::<_, String>((queue, pool, submissions))
            })();
            match made {
                Ok((queue, pool, submissions)) => Ok((
                    physical,
                    device,
                    family,
                    foreign_family,
                    semaphores,
                    queue,
                    pool,
                    submissions,
                )),
                Err(error) => {
                    // SAFETY: nothing of the device is in use.
                    unsafe { device.destroy_device(None) };
                    Err(error)
                }
            }
        })();
        let (physical, device, family, foreign_family, semaphores, queue, pool, submissions) = match made {
            Ok(made) => made,
            Err(error) => {
                destroy(&instance);
                return Err(error);
            }
        };
        let fd_api = ash::khr::external_memory_fd::Device::new(&instance, &device);
        let semaphore_api = semaphores.then(|| ash::khr::external_semaphore_fd::Device::new(&instance, &device));
        let (jobs, queued) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let broken = Arc::new(Mutex::new(None));
        let waiter = {
            let (device, broken) = (device.clone(), broken.clone());
            std::thread::Builder::new()
                .name("web-ring-copies".into())
                .spawn(move || wait_copies(device, queued, finished, broken))
        };
        let waiter = match waiter {
            Ok(waiter) => waiter,
            Err(error) => {
                // SAFETY: nothing of the device is in use.
                unsafe {
                    for submission in &submissions {
                        device.destroy_fence(submission.fence, None);
                        if submission.semaphore != vk::Semaphore::null() {
                            device.destroy_semaphore(submission.semaphore, None);
                        }
                    }
                    device.destroy_command_pool(pool, None);
                    device.destroy_device(None);
                }
                destroy(&instance);
                return Err(format!("the copies' thread: {error}"));
            }
        };
        Ok(Gpu {
            _entry: entry,
            instance,
            physical,
            device,
            fd_api,
            queue,
            family,
            foreign: if foreign_family {
                vk::QUEUE_FAMILY_FOREIGN_EXT
            } else {
                vk::QUEUE_FAMILY_EXTERNAL
            },
            pool,
            free: std::cell::RefCell::new((0..submissions.len()).collect()),
            submissions,
            done,
            jobs: Some(jobs),
            waiter: Some(waiter),
            semaphore_api,
            semaphores_work: std::cell::Cell::new(true),
            told_unfenced: std::cell::Cell::new(false),
            broken,
        })
    }

    /// Wait until no copy is queued: before images a copy may use are destroyed.
    ///
    /// When the ring broke this still returns only once the device is idle (an `Err` after), so every caller may drop
    /// images right after it, whether or not it looks at the result.
    fn settle(&self) -> Result<(), String> {
        let mut free = self.free.borrow_mut();
        while free.len() < self.submissions.len() {
            // The waiting thread hands each submission back, or ends (a lost device, a hung copy) within its limit.
            match self.done.recv_timeout(COPY_GIVE_UP) {
                Ok(index) => free.push(index),
                Err(_) => {
                    // SAFETY: the device is this one's. The queue is only submitted to from the thread that calls
                    // this, so nothing is submitted while it waits. A lost device returns at once.
                    unsafe {
                        let _ = self.device.device_wait_idle();
                    }
                    return Err(self.why_broken());
                }
            }
        }
        Ok(())
    }

    fn why_broken(&self) -> String {
        self.broken
            .lock()
            .expect("ring broken")
            .clone()
            .unwrap_or_else(|| format!("a copy took over {}s", COPY_GIVE_UP.as_secs()))
    }

    /// A submission to record a copy into: a free one, else the next the waiting thread hands back. With none back
    /// in [`COPY_WAIT`] (the GPU is slow) the frame is dropped ([`Miss::Full`]), and Chromium paints again when the
    /// app hands a slot back. The ring stops only when the waiting thread ends.
    fn submission(&self) -> Result<usize, Miss> {
        let mut free = self.free.borrow_mut();
        free.extend(self.done.try_iter());
        if let Some(index) = free.pop() {
            return Ok(index);
        }
        drop(free);
        match self.done.recv_timeout(COPY_WAIT) {
            Ok(index) => Ok(index),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Miss::Full),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Miss::Broken(self.why_broken())),
        }
    }

    /// A memory type among `allowed` with `wanted`, or any of `allowed` when none has it.
    fn memory_type(&self, allowed: u32, wanted: vk::MemoryPropertyFlags) -> Option<u32> {
        // SAFETY: a physical device of this instance.
        let properties = unsafe { self.instance.get_physical_device_memory_properties(self.physical) };
        let types = &properties.memory_types[..properties.memory_type_count as usize];
        let fits = |index: usize| allowed & (1 << index) != 0;
        (0..types.len())
            .find(|index| fits(*index) && types[*index].property_flags.contains(wanted))
            .or_else(|| (0..types.len()).find(|index| fits(*index)))
            .map(|index| index as u32)
    }

    /// Queue the copy of `visible` of Chromium's `source` into `slot`; `deliver` runs once it is done. Chromium's
    /// buffer goes back to its queue family, and the slot to the app's (`QUEUE_FAMILY_EXTERNAL`: another Vulkan
    /// instance), as the copy leaves them. The copy's semaphore goes on Chromium's buffer (`source_fd`), as a reader's
    /// fence. With no fence on it, this waits for the copy: Chromium may draw into its buffer once the paint callback
    /// returns.
    fn copy(
        &self,
        source: vk::Image,
        slot: vk::Image,
        visible: (u32, u32, u32, u32),
        source_fd: BorrowedFd,
        deliver: Box<dyn FnOnce() + Send>,
    ) -> Result<(), Miss> {
        if let Some(why) = self.broken.lock().expect("ring broken").clone() {
            return Err(Miss::Broken(why));
        }
        let index = self.submission()?;
        let Submission {
            command,
            fence,
            semaphore,
        } = self.submissions[index];
        // Handed back unless queued below.
        let unused = |error: String| {
            self.free.borrow_mut().push(index);
            Miss::Broken(error)
        };
        let device = &self.device;
        let (x, y, width, height) = visible;
        let color = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let layers = vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        };
        let before = [
            // Chromium's buffer, from its queue family to ours, as it left it (`GENERAL`).
            vk::ImageMemoryBarrier::default()
                .image(source)
                .subresource_range(color)
                .src_queue_family_index(self.foreign)
                .dst_queue_family_index(self.family)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
            // The slot: its last frame is not needed, so no transfer back from the app, and any layout will do.
            vk::ImageMemoryBarrier::default()
                .image(slot)
                .subresource_range(color)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE),
        ];
        let after = [
            vk::ImageMemoryBarrier::default()
                .image(source)
                .subresource_range(color)
                .src_queue_family_index(self.family)
                .dst_queue_family_index(self.foreign)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::empty()),
            // To the app, in `GENERAL`, where it takes it (`dmabuf::acquire`).
            vk::ImageMemoryBarrier::default()
                .image(slot)
                .subresource_range(color)
                .src_queue_family_index(self.family)
                .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::empty()),
        ];
        let region = vk::ImageCopy {
            src_subresource: layers,
            src_offset: vk::Offset3D {
                x: x as i32,
                y: y as i32,
                z: 0,
            },
            dst_subresource: layers,
            dst_offset: vk::Offset3D::default(),
            extent: vk::Extent3D {
                width,
                height,
                depth: 1,
            },
        };
        let commands = [command];
        let signals = [semaphore];
        let mut submit = vk::SubmitInfo::default().command_buffers(&commands);
        let api = self.semaphore_api.as_ref().filter(|_| self.semaphores_work.get());
        if api.is_some() {
            submit = submit.signal_semaphores(&signals);
        }
        // SAFETY: the submission is free: its last copy was waited for, and its fence reset (`wait_copies`).
        unsafe {
            device
                .reset_command_buffer(command, vk::CommandBufferResetFlags::empty())
                .map_err(|error| unused(format!("reset: {error}")))?;
            device
                .begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| unused(format!("begin: {error}")))?;
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &before,
            );
            device.cmd_copy_image(
                command,
                source,
                vk::ImageLayout::GENERAL,
                slot,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &after,
            );
            device
                .end_command_buffer(command)
                .map_err(|error| unused(format!("end: {error}")))?;
            device
                .queue_submit(self.queue, &[submit], fence)
                .map_err(|error| unused(format!("submit: {error}")))?;
        }
        // Why no reader fence went on Chromium's buffer, if none did.
        let unfenced = match api {
            None if self.semaphore_api.is_none() => Some("the driver can't export a copy's semaphore".to_string()),
            None => Some("an earlier fence failed".to_string()),
            Some(api) => {
                let get = vk::SemaphoreGetFdInfoKHR::default()
                    .semaphore(semaphore)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
                // SAFETY: the semaphore has a signal pending, from the submit above. Exporting it as a sync file waits
                // on it.
                match unsafe { api.get_semaphore_fd(&get) } {
                    // A sync file of -1 means already signalled: the copy is done, nothing to wait for.
                    Ok(file) if file < 0 => None,
                    Ok(file) => {
                        // SAFETY: Vulkan gave this descriptor to the caller.
                        let file = unsafe { OwnedFd::from_raw_fd(file) };
                        // A kernel without the call fails it every time: stop exporting.
                        put_reader_fence(source_fd, &file).err().map(|error| {
                            self.semaphores_work.set(false);
                            format!("the kernel can't put a fence on a buffer ({error})")
                        })
                    }
                    // The semaphore stays signalled, so it can't be signalled again: no more of them.
                    Err(error) => {
                        self.semaphores_work.set(false);
                        Some(format!("a semaphore's export failed ({error})"))
                    }
                }
            }
        };
        // With no fence, the waiting thread says when the copy is done.
        let (deliver, copied): (Box<dyn FnOnce() + Send>, _) = if unfenced.is_some() {
            let (done, copied) = mpsc::sync_channel(1);
            let deliver = Box::new(move || {
                deliver();
                let _ = done.send(());
            });
            (deliver, Some(copied))
        } else {
            (deliver, None)
        };
        let job = Job { index, fence, deliver };
        // The thread ends at drop, or if it panicked: then no frame would be sent and this submission would never
        // come back, so say so and let the caller take the old path.
        if let Some(jobs) = &self.jobs {
            jobs.send(job)
                .map_err(|_| Miss::Broken("the copies' thread ended".to_string()))?;
        }
        if let (Some(why), Some(copied)) = (unfenced, copied) {
            if !self.told_unfenced.replace(true) {
                eprintln!("nexus web host: frame ring: no fence on Chromium's buffer ({why}); each copy is waited for");
            }
            // An error: the waiting thread ended, a lost device or a hung copy. The next copy reports it.
            let _ = copied.recv();
        }
        Ok(())
    }
}

/// Put `file` on Chromium's buffer as a reader's fence: Chromium's next drawing into it waits for the copy. Fails on a
/// kernel before 6.0, which lacks the call.
fn put_reader_fence(buffer: BorrowedFd, file: &OwnedFd) -> std::io::Result<()> {
    #[repr(C)]
    struct Import {
        flags: u32,
        fd: i32,
    }
    let mut import = Import {
        flags: DMA_BUF_SYNC_READ,
        fd: file.as_raw_fd(),
    };
    // SAFETY: `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` on a dma-buf, with its struct. The kernel takes its own reference.
    if unsafe { libc::ioctl(buffer.as_raw_fd(), IMPORT_SYNC_FILE, &mut import) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // End the waiting thread: it finishes the copies queued.
        self.jobs = None;
        if let Some(waiter) = self.waiter.take() {
            let _ = waiter.join();
        }
        // SAFETY: the waiting thread ended; the rings and imports, made on this device, are dropped first (`Rings`'
        // drop and field order).
        unsafe {
            let _ = self.device.device_wait_idle();
            for submission in &self.submissions {
                self.device.destroy_fence(submission.fence, None);
                if submission.semaphore != vk::Semaphore::null() {
                    self.device.destroy_semaphore(submission.semaphore, None);
                }
            }
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use super::*;

    #[test]
    fn a_slot_word_names_its_slot_in_its_own_ring_only() {
        for generation in [0, 1, 7, u64::MAX >> 8] {
            for index in 0..SLOTS {
                assert_eq!(slot_index(generation, slot_word(generation, index)), Some(index));
            }
        }
        // A slot of the ring before a resize, handed back late: the new ring's slot 2 stays lent.
        assert_eq!(slot_index(4, slot_word(3, 2)), None);
        assert_eq!(slot_index(3, slot_word(4, 2)), None);
    }

    #[test]
    fn a_fence_that_does_not_go_on_is_an_error() {
        // Not a dma-buf: the kernel refuses the call, as a kernel before 6.0 does for any buffer.
        let buffer = std::fs::File::open("/dev/null").expect("/dev/null");
        let file = OwnedFd::from(std::fs::File::open("/dev/null").expect("/dev/null"));
        assert!(put_reader_fence(buffer.as_fd(), &file).is_err());
    }
}
