//! Web pages' frames on the GPU (Linux). Chromium paints a frame into a GPU buffer (a dma-buf) in the web host
//! process, and the app draws it with GPUI (`SharedTexture`, in `vendor/gpui-pre-wgpu`). No pixel goes through the CPU.
//! Two ways, over one socket:
//!
//! - The ring (`ring.rs`), first choice. The host copies Chromium's buffer into one of a few buffers it shares with
//!   the app, on a GPU device of its own, and sends that buffer's descriptor once the copy is done, from a thread of
//!   its own. Chromium does not wait for the app, nor for the copy. The
//!   app draws the buffer as it is, and hands it back (a release) when no frame draws it any more (`Lease`).
//! - The copy. The host passes Chromium's buffer itself, and waits for the app to copy it into a texture of its own
//!   (an ack). The app's copy runs behind its own drawing on its GPU queue, so Chromium waits up to a screen refresh
//!   per frame. Used when the ring can't start here, or fails.
//!
//! The CPU path (`host.rs`'s memory file) stays: where the app's GPU can't import a buffer (not Vulkan, an old
//! driver), the first failure turns this off for the app's life, and the page opens again with CPU frames.
//! Off by default: `NEXUS_WEB_GPU=1` turns it on.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use gpui_wgpu::{SharedTexture, wgpu};

use super::super::client::Pages;
use super::super::{dmabuf, ring};
use super::{ON, turn_off};

/// The environment variable that gives the host its end of the socket, by descriptor number.
pub(in crate::web) const FD_ENV: &str = "NEXUS_WEB_GPU_FD";
/// The environment variable that names the app's GPU (its Vulkan UUID, in hex), for the host's ring device. Unset:
/// no ring. `NEXUS_WEB_RING=0` in the app's environment leaves it unset.
pub(in crate::web) const UUID_ENV: &str = "NEXUS_WEB_GPU_UUID";
/// How long the host waits for the app to copy a frame. A frame not copied by then is dropped, and the next is sent.
const ACK_WAIT: Duration = Duration::from_millis(100);
/// A header's slot when the frame is Chromium's own buffer, not a ring's.
const NO_SLOT: u64 = u64::MAX;
/// Chromium's buffers have at most four planes.
const MAX_PLANES: usize = 4;
/// A frame's header on the socket: 64-bit words, the planes last. Its descriptors travel beside it (`SCM_RIGHTS`).
const HEADER_WORDS: usize = 10 + 3 * MAX_PLANES;
/// The app's answers: a word (the frame's number for an ack, the slot for a release), the page, whether it copied the
/// frame, and the kind.
const ANSWER_BYTES: usize = 24;

/// Set at the first frame from the host's ring, to say so once in the log.
static RING_ON: AtomicBool = AtomicBool::new(false);
/// Set when the ring's thread could not send a frame: the app's end closed.
static APP_GONE: AtomicBool = AtomicBool::new(false);

/// A frame as it crosses the socket, without its descriptors.
#[derive(Debug, PartialEq)]
struct Header {
    seq: u64,
    id: u32,
    width: u32,
    height: u32,
    /// CEF's `cef_color_type_t`: 0 RGBA, 1 BGRA.
    format: u32,
    modifier: u64,
    /// The part of the buffer that shows: x, y, width, height. Chromium may pad the buffer past it.
    visible: (u32, u32, u32, u32),
    /// The ring's slot the frame is in (`ring.rs`), or [`NO_SLOT`].
    slot: u64,
    /// Each plane's stride, offset and size, in bytes.
    planes: Vec<(u32, u64, u64)>,
}

impl Header {
    fn encode(&self) -> [u8; HEADER_WORDS * 8] {
        let mut words = [0u64; HEADER_WORDS];
        let (x, y, width, height) = self.visible;
        words[..9].copy_from_slice(&[
            self.seq,
            self.id.into(),
            self.width.into(),
            self.height.into(),
            self.format.into(),
            self.modifier,
            self.planes.len() as u64,
            u64::from(x) | u64::from(y) << 32,
            u64::from(width) | u64::from(height) << 32,
        ]);
        words[9] = self.slot;
        for (index, (stride, offset, size)) in self.planes.iter().take(MAX_PLANES).enumerate() {
            words[10 + index * 3..13 + index * 3].copy_from_slice(&[(*stride).into(), *offset, *size]);
        }
        let mut bytes = [0u8; HEADER_WORDS * 8];
        for (chunk, word) in bytes.chunks_exact_mut(8).zip(words) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Header> {
        if bytes.len() != HEADER_WORDS * 8 {
            return None;
        }
        let word = |index: usize| u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().expect("8 bytes"));
        let count = word(6) as usize;
        if count == 0 || count > MAX_PLANES {
            return None;
        }
        Some(Header {
            seq: word(0),
            id: word(1) as u32,
            width: word(2) as u32,
            height: word(3) as u32,
            format: word(4) as u32,
            modifier: word(5),
            visible: (
                word(7) as u32,
                (word(7) >> 32) as u32,
                word(8) as u32,
                (word(8) >> 32) as u32,
            ),
            slot: word(9),
            planes: (0..count)
                .map(|index| (word(10 + index * 3) as u32, word(11 + index * 3), word(12 + index * 3)))
                .collect(),
        })
    }
}

/// What the app says back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Frame `seq` (Chromium's own buffer) was copied, or `ok: false`, could not be.
    Ack { seq: u64, ok: bool },
    /// Page `id`'s ring slot `slot` is the host's again.
    Release { id: u32, slot: u64 },
    /// The app can't draw the ring's buffers (page `id`'s frame failed): send Chromium's own from now on.
    NoRing { id: u32 },
}

impl Answer {
    fn encode(self) -> [u8; ANSWER_BYTES] {
        let (word, id, ok, kind) = match self {
            Answer::Ack { seq, ok } => (seq, 0, ok, 0),
            Answer::Release { id, slot } => (slot, id, false, 1),
            Answer::NoRing { id } => (0, id, false, 2),
        };
        let mut bytes = [0u8; ANSWER_BYTES];
        bytes[..8].copy_from_slice(&word.to_le_bytes());
        bytes[8..12].copy_from_slice(&id.to_le_bytes());
        bytes[16] = ok as u8;
        bytes[17] = kind;
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Answer> {
        if bytes.len() != ANSWER_BYTES {
            return None;
        }
        let word = u64::from_le_bytes(bytes[..8].try_into().expect("8 bytes"));
        let id = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes"));
        match bytes[17] {
            0 => Some(Answer::Ack {
                seq: word,
                ok: bytes[16] != 0,
            }),
            1 => Some(Answer::Release { id, slot: word }),
            2 => Some(Answer::NoRing { id }),
            _ => None,
        }
    }
}

/// A connected pair of message sockets (each send is one message), closed on exec.
pub(in crate::web) fn pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: `fds` has room for the two descriptors; each is then owned exactly once.
    unsafe {
        if libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        ) < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

/// Room for `count` descriptors in a control message, in words so the header inside is aligned.
fn control_buffer(count: usize) -> (Vec<u64>, usize) {
    // SAFETY: a size computation only.
    let space = unsafe { libc::CMSG_SPACE((count * size_of::<RawFd>()) as u32) } as usize;
    (vec![0u64; space.div_ceil(8)], space)
}

fn send_with_fds(socket: &OwnedFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    let (mut control, space) = control_buffer(fds.len());
    // SAFETY: the message points at `iov` and `control`, which outlive the call; the control message is written
    // inside `control`, whose size `CMSG_SPACE` gave.
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if !fds.is_empty() {
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = space as _;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN((fds.len() * size_of::<RawFd>()) as u32) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(header).cast::<RawFd>(), fds.len());
        }
        loop {
            if libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) >= 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// How long the app's socket may be quiet before closed pages' imports are let go.
const IDLE_PRUNE: Duration = Duration::from_secs(1);

/// Whether `socket` has something to read (or has closed) within `wait`.
fn readable_within(socket: &OwnedFd, wait: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid `pollfd`, and a count of one.
    let ready = unsafe { libc::poll(&mut poll, 1, wait.as_millis() as libc::c_int) };
    // An error other than a signal is left for the read to report.
    ready != 0 && !(ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted)
}

/// One message and the descriptors that came with it. Zero bytes: the other end closed.
fn recv_with_fds(socket: &OwnedFd, buffer: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let (mut control, space) = control_buffer(MAX_PLANES);
    // SAFETY: as in `send_with_fds`; each received descriptor is owned exactly once.
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = space as _;
        let read = loop {
            let read = libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC);
            if read >= 0 {
                break read as usize;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        let mut fds = Vec::new();
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let count = ((*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<RawFd>();
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                for index in 0..count {
                    fds.push(OwnedFd::from_raw_fd(data.add(index).read_unaligned()));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
        Ok((read, fds))
    }
}

// The host's side.

/// The host's end of the socket, when the app gave it one. The host's main thread uses it: CEF paints there.
static HOST_END: Mutex<Option<HostEnd>> = Mutex::new(None);
/// Ring slots the app handed back, and pages owed a frame, from the thread that reads the app's answers. Apart from
/// [`HOST_END`] so that thread never waits on a paint.
static RETURNS: Mutex<Returns> = Mutex::new(Returns {
    released: Vec::new(),
    owed: Vec::new(),
    no_ring: false,
});

struct HostEnd {
    socket: Arc<OwnedFd>,
    seq: u64,
    /// The app's acks, by frame number, from the answer thread.
    acks: mpsc::Receiver<u64>,
    ring: Ring,
}

enum Ring {
    /// Its device is being made, off the main thread: frames go the old way meanwhile.
    Starting,
    On(ring::Rings),
    Off,
}

struct Returns {
    released: Vec<(u32, u64)>,
    /// Pages whose last frame was dropped for want of a free slot: painted again when one comes back.
    owed: Vec<u32>,
    no_ring: bool,
}

/// Take the socket the app passed down, if it did, and start the ring's device. Called once, at the host's start.
pub(in crate::web) fn host_start() {
    let Some(fd) = std::env::var(FD_ENV).ok().and_then(|fd| fd.parse::<RawFd>().ok()) else {
        return;
    };
    // SAFETY: the app passed this descriptor to this process for this use alone. Chromium's helper processes must not
    // inherit it: close it on exec.
    let socket = unsafe {
        if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return;
        }
        Arc::new(OwnedFd::from_raw_fd(fd))
    };
    let (ack, acks) = mpsc::channel();
    let reader = socket.clone();
    if std::thread::Builder::new()
        .name("web-gpu-answers".into())
        .spawn(move || read_answers(&reader, &ack))
        .is_err()
    {
        return;
    }
    let uuid = std::env::var(UUID_ENV).ok().and_then(|hex| parse_uuid(&hex));
    *HOST_END.lock().expect("gpu frames") = Some(HostEnd {
        socket,
        seq: 0,
        acks,
        ring: if uuid.is_some() { Ring::Starting } else { Ring::Off },
    });
    let Some(uuid) = uuid else {
        return;
    };
    // Making a Vulkan device takes tens of milliseconds: not on the way to the first page.
    let _ = std::thread::Builder::new().name("web-gpu-ring".into()).spawn(move || {
        let rings = ring::Rings::new(uuid);
        let mut end = HOST_END.lock().expect("gpu frames");
        let Some(end) = end.as_mut() else {
            return;
        };
        end.ring = match rings {
            Ok(rings) => Ring::On(rings),
            Err(error) => {
                eprintln!("nexus web host: no frame ring, frames go the old way: {error}");
                Ring::Off
            }
        };
    });
}

/// The app's GPU, for [`UUID_ENV`]: `None` when the ring is turned off (`NEXUS_WEB_RING=0`) or the device is not
/// Vulkan.
pub(in crate::web) fn ring_gpu() -> Option<String> {
    if std::env::var("NEXUS_WEB_RING").is_ok_and(|value| value == "0") {
        return None;
    }
    let (device, _) = gpui_wgpu::shared_device()?;
    dmabuf::device_uuid(&device).map(|uuid| format_uuid(&uuid))
}

fn parse_uuid(hex: &str) -> Option<[u8; 16]> {
    let mut uuid = [0u8; 16];
    if hex.len() != 32 {
        return None;
    }
    for (index, byte) in uuid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(uuid)
}

fn format_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Read the app's answers until it closes the socket: acks to the painting thread, releases into [`RETURNS`], and a
/// repaint for a page owed a frame.
fn read_answers(socket: &OwnedFd, acks: &mpsc::Sender<u64>) {
    let mut buffer = [0u8; ANSWER_BYTES];
    loop {
        let read = match recv_with_fds(socket, &mut buffer) {
            Ok((0, _)) | Err(_) => return,
            Ok((read, _)) => read,
        };
        match Answer::decode(&buffer[..read]) {
            Some(Answer::Ack { seq, .. }) => {
                let _ = acks.send(seq);
            }
            Some(Answer::Release { id, slot }) => {
                let owed = {
                    let mut returns = RETURNS.lock().expect("gpu frame returns");
                    returns.released.push((id, slot));
                    let owed = returns.owed.contains(&id);
                    returns.owed.retain(|other| *other != id);
                    owed
                };
                if owed {
                    super::super::host::repaint(id);
                }
            }
            Some(Answer::NoRing { id }) => {
                RETURNS.lock().expect("gpu frame returns").no_ring = true;
                // Its frame is lost: paint it again, the old way.
                super::super::host::repaint(id);
            }
            None => {}
        }
    }
}

/// Whether pages can paint GPU frames: the app passed the socket, and it is still open.
pub(in crate::web) fn host_ready() -> bool {
    HOST_END.lock().expect("gpu frames").is_some()
}

/// Page `id` closed: free its ring.
pub(in crate::web) fn forget(id: u32) {
    if let Some(HostEnd {
        ring: Ring::On(rings), ..
    }) = HOST_END.lock().expect("gpu frames").as_mut()
    {
        rings.forget(id);
    }
}

/// Chromium painted page `id`'s frame into a GPU buffer, valid for this call: pass it to the app.
pub(in crate::web) fn paint(id: u32, info: &cef::AcceleratedPaintInfo) {
    let mut end = HOST_END.lock().expect("gpu frames");
    let Some(host_end) = end.as_mut() else {
        return;
    };
    if let Err(error) = host_end.paint(id, info) {
        eprintln!("nexus web host: GPU frame for page {id}: {error}");
        *end = None;
    }
}

/// The part of the frame that shows, inside the buffer. CEF leaves it empty on some paths: then all of it.
fn visible(info: &cef::AcceleratedPaintInfo) -> (u32, u32, u32, u32) {
    let (width, height) = (info.extra.coded_size.width.max(0), info.extra.coded_size.height.max(0));
    let rect = &info.extra.visible_rect;
    let inside = rect.x >= 0
        && rect.y >= 0
        && rect.width > 0
        && rect.height > 0
        && rect.x + rect.width <= width
        && rect.y + rect.height <= height;
    if inside {
        (rect.x as u32, rect.y as u32, rect.width as u32, rect.height as u32)
    } else {
        (0, 0, width as u32, height as u32)
    }
}

impl HostEnd {
    /// Send the frame through the ring when it is on, else Chromium's own buffer. An error: the app's end closed.
    fn paint(&mut self, id: u32, info: &cef::AcceleratedPaintInfo) -> io::Result<()> {
        if APP_GONE.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let count = (info.plane_count.max(0) as usize).min(MAX_PLANES);
        if count == 0 {
            return Ok(());
        }
        let visible = visible(info);
        let mut broken = None;
        if let Ring::On(rings) = &mut self.ring {
            // Lock order: `HOST_END`, then `RETURNS`. The answer thread takes `RETURNS` alone.
            let mut returns = RETURNS.lock().expect("gpu frame returns");
            for (page, slot) in returns.released.drain(..) {
                rings.release(page, slot);
            }
            if returns.no_ring {
                broken = Some("the app can't draw the ring's buffers".to_string());
            } else {
                self.seq += 1;
                let (seq, socket) = (self.seq, self.socket.clone());
                // Runs on the ring's thread once the copy is done.
                let deliver = move |copied: ring::Copied| {
                    let header = Header {
                        seq,
                        id,
                        width: copied.width,
                        height: copied.height,
                        format: copied.color,
                        modifier: 0,
                        visible: (0, 0, copied.width, copied.height),
                        slot: copied.slot,
                        planes: vec![(copied.stride, copied.offset, copied.size)],
                    };
                    if send_with_fds(&socket, &header.encode(), &[copied.fd.as_raw_fd()]).is_err() {
                        APP_GONE.store(true, Ordering::Relaxed);
                    }
                };
                match rings.copy(id, info, visible, deliver) {
                    Ok(()) => return Ok(()),
                    Err(ring::Miss::Full) => {
                        if !returns.owed.contains(&id) {
                            returns.owed.push(id);
                        }
                        return Ok(());
                    }
                    Err(ring::Miss::Broken(why)) => broken = Some(why),
                }
            }
        }
        if let Some(why) = broken {
            eprintln!("nexus web host: frame ring off, frames go the old way: {why}");
            self.ring = Ring::Off;
            // Pages waiting for a slot will not get one: paint them now.
            for page in std::mem::take(&mut RETURNS.lock().expect("gpu frame returns").owed) {
                super::super::host::repaint(page);
            }
        }
        let planes = &info.planes[..count];
        self.seq += 1;
        let header = Header {
            seq: self.seq,
            id,
            width: info.extra.coded_size.width.max(0) as u32,
            height: info.extra.coded_size.height.max(0) as u32,
            format: *info.format.as_ref() as u32,
            modifier: info.modifier,
            visible,
            slot: NO_SLOT,
            planes: planes
                .iter()
                .map(|plane| (plane.stride, plane.offset, plane.size))
                .collect(),
        };
        let fds: Vec<RawFd> = planes.iter().map(|plane| plane.fd).collect();
        send_with_fds(&self.socket, &header.encode(), &fds)?;
        self.wait_ack(header.seq)
    }

    /// Wait for the app's ack of frame `seq`, at most [`ACK_WAIT`]. An error: the app's end closed.
    fn wait_ack(&self, seq: u64) -> io::Result<()> {
        let deadline = Instant::now() + ACK_WAIT;
        loop {
            match self
                .acks
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(acked) if acked == seq => return Ok(()),
                // An ack of an earlier frame, which timed out, is skipped.
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(io::ErrorKind::BrokenPipe.into()),
            }
        }
    }
}

// The app's side.

/// Read frames from the host until it exits: take each (a ring's buffer, or a copy of Chromium's own), answer, and
/// give it to its page.
pub(in crate::web) fn read_frames(socket: OwnedFd, pages: Pages) {
    let socket = Arc::new(socket);
    let (release, returned) = mpsc::channel();
    let releaser = socket.clone();
    if std::thread::Builder::new()
        .name("web-gpu-releases".into())
        .spawn(move || release_slots(&releaser, &returned))
        .is_err()
    {
        return;
    }
    let mut buffer = [0u8; HEADER_WORDS * 8];
    // Each page's last copied texture, copied into again when the next frame has its size. The renderer's reads of the
    // old frame are queued before the copy, so it never shows half of one.
    let mut last: HashMap<u32, wgpu::Texture> = HashMap::new();
    let mut imports = Imports::default();
    loop {
        // A quiet socket (the last page closed, so no frame comes) still lets go of the closed pages' imports.
        if !readable_within(&socket, IDLE_PRUNE) {
            let pages = pages.lock().expect("web host pages");
            imports.forget_closed(|id| pages.contains_key(&id));
            last.retain(|id, _| pages.contains_key(id));
            continue;
        }
        let (read, fds) = match recv_with_fds(&socket, &mut buffer) {
            Ok((0, _)) | Err(_) => return,
            Ok(message) => message,
        };
        let Some(header) = Header::decode(&buffer[..read]) else {
            continue;
        };
        let ring = header.slot != NO_SLOT;
        let taken = if ring {
            last.remove(&header.id);
            take(&header, &fds, &mut imports, &release)
        } else {
            import(&header, &fds, &mut imports, last.remove(&header.id)).map(|texture| (texture, None))
        };
        let answer = if ring {
            taken.is_err().then_some(Answer::NoRing { id: header.id })
        } else {
            if let Ok((texture, _)) = &taken {
                last.insert(header.id, texture.clone());
            }
            Some(Answer::Ack {
                seq: header.seq,
                ok: taken.is_ok(),
            })
        };
        if let Some(answer) = answer
            && send_with_fds(&socket, &answer.encode(), &[]).is_err()
        {
            return;
        }
        // Copied or imported: the buffer's descriptors close here.
        drop(fds);
        let shared = {
            let pages = pages.lock().expect("web host pages");
            imports.forget_closed(|id| pages.contains_key(&id));
            last.retain(|id, _| pages.contains_key(id));
            pages.get(&header.id).cloned()
        };
        // A closed page's frame: its lease, if any, drops here and hands the slot back.
        let Some(shared) = shared else {
            continue;
        };
        let mut shared = shared.lock().expect("web view state");
        match taken {
            Ok((texture, lease)) => {
                if !ON.swap(true, Ordering::Relaxed) {
                    log::info!("web pages draw GPU frames");
                }
                if ring && !RING_ON.swap(true, Ordering::Relaxed) {
                    log::info!("web pages draw the host's frame ring, with no copy");
                }
                let (device, _) = gpui_wgpu::shared_device().expect("imported on it");
                shared.texture = Some(Arc::new(match lease {
                    Some(lease) => SharedTexture::with_owner(texture, device, lease),
                    None => SharedTexture::new(texture, device),
                }));
                shared.gpu_framed = true;
            }
            // The host sends this page's frame again, its own buffer (`Answer::NoRing`).
            Err(error) if ring => log::warn!("frame ring off: {error}"),
            Err(error) => turn_off(&mut shared, &error),
        }
        shared.changed();
    }
}

/// The app's hold on a slot of the host's ring (`ring.rs`), dropped with the last frame that draws it. The slot goes
/// back to the host only once the renderer's reads of it are done (`release_slots`).
struct Lease {
    returned: Option<Returned>,
    release: mpsc::Sender<Returned>,
}

struct Returned {
    id: u32,
    slot: u64,
    texture: wgpu::Texture,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(returned) = self.returned.take() {
            // The reader ended: the host is gone too.
            let _ = self.release.send(returned);
        }
    }
}

/// Hand slots back to the host as their leases drop: ready each for the host's write, wait until the GPU is past every
/// frame that drew it, then tell the host.
fn release_slots(socket: &OwnedFd, returned: &mpsc::Receiver<Returned>) {
    while let Ok(first) = returned.recv() {
        let mut batch = vec![first];
        batch.extend(returned.try_iter());
        // One submission per device. There is one, unless a lost device was replaced while a frame was out.
        while !batch.is_empty() {
            let device = batch[0].device.clone();
            let queue = batch[0].queue.clone();
            let (same, rest): (Vec<_>, Vec<_>) = batch
                .into_iter()
                .partition(|returned| Arc::ptr_eq(&returned.device, &device));
            batch = rest;
            let textures: Vec<_> = same.iter().map(|returned| returned.texture.clone()).collect();
            let submission = dmabuf::settle(&device, &queue, &textures);
            loop {
                match device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission.clone()),
                    timeout: None,
                }) {
                    Err(wgpu::PollError::Timeout) => continue,
                    // Done, or the device is lost and reads nothing.
                    _ => break,
                }
            }
            for returned in same {
                let answer = Answer::Release {
                    id: returned.id,
                    slot: returned.slot,
                };
                if send_with_fds(socket, &answer.encode(), &[]).is_err() {
                    return;
                }
            }
        }
    }
}

/// The buffers each page's frames came in, imported once each. Chromium paints into a few buffers in turn; a buffer
/// is known again by its inode, which stays its own while it is imported here (the import keeps it alive).
#[derive(Default)]
struct Imports {
    /// Each page's imports, by buffer (device and inode), with the frame each was last used for.
    pages: HashMap<u32, Vec<((u64, u64), dmabuf::Imported, u64)>>,
    /// Frames of all pages so far, not of one page.
    frame: u64,
}

impl Imports {
    /// A page keeps at most this many imports: more than Chromium's buffers in turn, fewer than a leak.
    const PER_PAGE: usize = 6;
    /// An import not used for this many frames of all pages is let go: about two seconds at 120 a second with one
    /// page painting, less with more. A page that loses an import too early only imports its buffer again.
    const UNUSED: u64 = 240;

    /// Page `id`'s import of the buffer `fd` names, laid out as `buffer`: the one kept, or a new one.
    fn get(
        &mut self,
        id: u32,
        fd: &OwnedFd,
        buffer: &dmabuf::Buffer,
        device: &wgpu::Device,
    ) -> Result<&dmabuf::Imported, String> {
        self.frame += 1;
        let frame = self.frame;
        let key = buffer_key(fd)?;
        let kept = self.pages.entry(id).or_default();
        // A frame of another size means the page was resized: its old buffers will not come back.
        kept.retain(|(other, imported, used)| {
            imported.width == buffer.width
                && imported.height == buffer.height
                && frame - used < Self::UNUSED
                && (*other != key || imported.fits(buffer))
        });
        let index = match kept.iter().position(|(other, ..)| *other == key) {
            Some(index) => index,
            None => {
                // SAFETY: the host passed this descriptor for a dma-buf laid out as the header says.
                let imported = unsafe { dmabuf::import(device, buffer) }?;
                if kept.len() >= Self::PER_PAGE
                    && let Some(oldest) = (0..kept.len()).min_by_key(|index| kept[*index].2)
                {
                    kept.swap_remove(oldest);
                }
                kept.push((key, imported, frame));
                kept.len() - 1
            }
        };
        kept[index].2 = frame;
        Ok(&kept[index].1)
    }

    /// Let go of the imports of pages that closed.
    fn forget_closed(&mut self, open: impl Fn(u32) -> bool) {
        self.pages.retain(|id, _| open(*id));
    }
}

/// A buffer's identity: its file's device and inode. Each dma-buf has an inode of its own.
fn buffer_key(fd: &OwnedFd) -> Result<(u64, u64), String> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a valid descriptor, and a buffer for one `stat`.
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(format!("stat the buffer: {}", io::Error::last_os_error()));
    }
    // SAFETY: `fstat` filled it.
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_dev as u64, stat.st_ino as u64))
}

/// The colour format of CEF's colour type.
fn texture_format(color: u32) -> Result<wgpu::TextureFormat, String> {
    match color {
        0 => Ok(wgpu::TextureFormat::Rgba8Unorm),
        1 => Ok(wgpu::TextureFormat::Bgra8Unorm),
        other => Err(format!("unknown colour type {other}")),
    }
}

/// Take the ring's buffer a frame came in, imported once per slot, to draw as it is: its texture and the lease that
/// hands it back.
fn take(
    header: &Header,
    fds: &[OwnedFd],
    imports: &mut Imports,
    release: &mpsc::Sender<Returned>,
) -> Result<(wgpu::Texture, Option<Lease>), String> {
    let (device, queue) = gpui_wgpu::shared_device().ok_or("no GPU device yet")?;
    let ([fd], [(stride, offset, _)]) = (fds, header.planes.as_slice()) else {
        return Err(format!("{} descriptors for {} planes", fds.len(), header.planes.len()));
    };
    let planes = [(*stride, *offset)];
    let buffer = dmabuf::Buffer {
        width: header.width,
        height: header.height,
        format: texture_format(header.format)?,
        modifier: header.modifier,
        planes: &planes,
        fd,
    };
    let imported = imports.get(header.id, fd, &buffer, &device)?;
    // SAFETY: the host finished its copy into the slot and released it before it sent the frame, and the slot was in
    // wgpu's `GENERAL` when its last lease was let go (`release_slots`); `queue` is `device`'s.
    let texture = unsafe { dmabuf::acquire(&device, &queue, imported) }?;
    let lease = Lease {
        returned: Some(Returned {
            id: header.id,
            slot: header.slot,
            texture: texture.clone(),
            device,
            queue,
        }),
        release: release.clone(),
    };
    Ok((texture, Some(lease)))
}

/// Copy the host's buffer into a texture on the device GPUI draws with (`reuse`, if it fits). The copy is finished
/// on return.
fn import(
    header: &Header,
    fds: &[OwnedFd],
    imports: &mut Imports,
    reuse: Option<wgpu::Texture>,
) -> Result<wgpu::Texture, String> {
    let (device, queue) = gpui_wgpu::shared_device().ok_or("no GPU device yet")?;
    if fds.len() != header.planes.len() {
        return Err(format!("{} descriptors for {} planes", fds.len(), header.planes.len()));
    }
    let format = texture_format(header.format)?;
    let planes: Vec<(u32, u64)> = header
        .planes
        .iter()
        .map(|(stride, offset, _)| (*stride, *offset))
        .collect();
    let buffer = dmabuf::Buffer {
        width: header.width,
        height: header.height,
        format,
        modifier: header.modifier,
        planes: &planes,
        fd: &fds[0],
    };
    let imported = imports.get(header.id, &fds[0], &buffer, &device)?;
    // SAFETY: the host holds the buffer unchanged until this process answers, after this returns; `queue` is
    // `device`'s, the device GPUI draws with.
    unsafe { dmabuf::copy(&device, &queue, imported, header.visible, reuse) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_survives_the_socket() {
        let header = Header {
            seq: 7,
            id: 3,
            width: 1600,
            height: 900,
            format: 1,
            modifier: 0x0100_0000_0000_0002,
            visible: (0, 0, 1600, 900),
            slot: 2 << 8 | 3,
            planes: vec![(6400, 0, 5_760_000), (64, 5_760_000, 4096)],
        };
        assert_eq!(Header::decode(&header.encode()), Some(header));
    }

    #[test]
    fn answers_survive_the_socket() {
        for answer in [
            Answer::Ack { seq: 9, ok: true },
            Answer::Ack { seq: 10, ok: false },
            Answer::Release {
                id: 4,
                slot: 7 << 8 | 1,
            },
            Answer::NoRing { id: 5 },
        ] {
            assert_eq!(Answer::decode(&answer.encode()), Some(answer));
        }
    }

    #[test]
    fn a_gpu_uuid_crosses_the_environment() {
        let uuid = [0xab, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 0xff];
        assert_eq!(parse_uuid(&format_uuid(&uuid)), Some(uuid));
        assert_eq!(parse_uuid("abc"), None);
    }

    #[test]
    fn descriptors_cross_with_their_message() {
        let (app, host) = pair().expect("socket pair");
        let (file, _) = pair().expect("something to pass");
        send_with_fds(&host, b"frame", &[file.as_raw_fd()]).expect("sent");
        let mut buffer = [0u8; 16];
        let (read, fds) = recv_with_fds(&app, &mut buffer).expect("received");
        assert_eq!(&buffer[..read], b"frame");
        assert_eq!(fds.len(), 1);
        assert_ne!(fds[0].as_raw_fd(), file.as_raw_fd());
    }
}
