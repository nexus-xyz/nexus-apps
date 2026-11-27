//! CPU frames: the pixels of a page's frame go from the host to the app through a memory file both map.
//!
//! The host keeps one mapped file per page and frame size (a "generation"): a new size gets a new name, because a file
//! that shrinks under the app's mapping would crash the app (SIGBUS) where it reads past the new end. The file always
//! holds the whole latest frame the host sent. Each frame writes only the rects Chromium says changed, and the event
//! names them ([`Event::Frame`](super::protocol::Event::Frame)'s `dirty`).
//!
//! The app reads a frame, then acknowledges it. Until then the host writes nothing into the file: a frame painted in
//! that time goes into the host's own copy (`mirror`), and its rects into `owed`. The acknowledgement copies the owed
//! rects into the file and sends them. So the app never reads a frame half written.
//!
//! The file: `/dev/shm` on Linux, a POSIX shared memory object on macOS (`shm_open`), and a temporary file the system
//! keeps in memory where it can on Windows.

use std::fs::File;
use std::io;

use memmap2::{Mmap, MmapMut, MmapOptions};

/// A rect of device pixels: x, y, width, height.
pub(crate) type Rect = [u32; 4];

/// The most rects a frame names; more become the one rect around them all.
const MAX_RECTS: usize = 8;

/// Rects inside a `width` by `height` frame, from Chromium's (`x, y, width, height`, maybe out of bounds). None given
/// means the whole frame. Empty rects drop out, so a frame with nothing new has none.
pub(super) fn clamp(dirty: &[[i32; 4]], width: u32, height: u32) -> Vec<Rect> {
    if dirty.is_empty() {
        return vec![[0, 0, width, height]];
    }
    dirty
        .iter()
        .filter_map(|&[x, y, w, h]| {
            let (x0, y0) = (x.max(0) as i64, y.max(0) as i64);
            let x1 = (x as i64 + w.max(0) as i64).min(width as i64);
            let y1 = (y as i64 + h.max(0) as i64).min(height as i64);
            (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
        })
        .collect()
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    outer[0] <= inner[0]
        && outer[1] <= inner[1]
        && outer[0] + outer[2] >= inner[0] + inner[2]
        && outer[1] + outer[3] >= inner[1] + inner[3]
}

/// Drop rects inside another, then, past [`MAX_RECTS`], put the one rect around them all in their place. Returns
/// whether it did that: the rect then covers pixels no rect named.
pub(super) fn coalesce(rects: &mut Vec<Rect>) -> bool {
    let mut kept: Vec<Rect> = Vec::with_capacity(rects.len());
    for rect in rects.drain(..) {
        if kept.iter().any(|other| contains(other, &rect)) {
            continue;
        }
        kept.retain(|other| !contains(&rect, other));
        kept.push(rect);
    }
    *rects = kept;
    if rects.len() <= MAX_RECTS {
        return false;
    }
    let x0 = rects.iter().map(|r| r[0]).min().unwrap_or(0);
    let y0 = rects.iter().map(|r| r[1]).min().unwrap_or(0);
    let x1 = rects.iter().map(|r| r[0] + r[2]).max().unwrap_or(0);
    let y1 = rects.iter().map(|r| r[1] + r[3]).max().unwrap_or(0);
    *rects = vec![[x0, y0, x1 - x0, y1 - y0]];
    true
}

/// Copy `rects` of a `width`-pixel-wide BGRA frame from `from` to `to`, row by row. Rects past either buffer are cut.
pub(super) fn copy_rects(to: &mut [u8], from: &[u8], width: u32, rects: &[Rect]) {
    let stride = width as usize * 4;
    if stride == 0 {
        return;
    }
    let rows = to.len().min(from.len()) / stride;
    for &[x, y, w, h] in rects {
        let x = (x as usize).min(width as usize);
        let w = (w as usize).min(width as usize - x);
        let (start, end) = ((y as usize).min(rows), (y as usize + h as usize).min(rows));
        for row in start..end {
            let at = row * stride + x * 4;
            to[at..at + w * 4].copy_from_slice(&from[at..at + w * 4]);
        }
    }
}

/// Put `rect`'s rows of the `width`-wide frame `pixels` into `out`, tightly packed. False: the rect is not inside.
pub(super) fn rect_rows(pixels: &[u8], width: u32, [x, y, w, h]: Rect, out: &mut Vec<u8>) -> bool {
    out.clear();
    if x.checked_add(w).is_none_or(|end| end > width) {
        return false;
    }
    let stride = width as usize * 4;
    for row in y as usize..y as usize + h as usize {
        let at = row * stride + x as usize * 4;
        let Some(line) = pixels.get(at..at + w as usize * 4) else {
            return false;
        };
        out.extend_from_slice(line);
    }
    true
}

fn area(rects: &[Rect]) -> u64 {
    rects.iter().map(|r| r[2] as u64 * r[3] as u64).sum()
}

/// A frame the host sent: what [`Event::Frame`](super::protocol::Event::Frame) says, and the pixels it wrote.
#[derive(Debug, PartialEq)]
pub(super) struct Sent {
    pub(super) path: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) seq: u64,
    /// Empty: the whole frame.
    pub(super) dirty: Vec<Rect>,
    pub(super) pixels: u64,
}

/// The host's side of one page's CPU frames.
pub(super) struct Painter {
    id: u32,
    /// The generation the next new file gets, and the file the app reads now.
    generation: u32,
    file: Option<Mapping>,
    /// The newest frame painted while the app read the last, correct inside the `owed` rects: what the file still
    /// lacks.
    mirror: Vec<u8>,
    mirror_size: (u32, u32),
    owed: Vec<Rect>,
    /// A frame the app has not acknowledged yet.
    in_flight: bool,
    seq: u64,
}

impl Painter {
    pub(super) fn new(id: u32) -> Painter {
        Painter {
            id,
            generation: 0,
            file: None,
            mirror: Vec::new(),
            mirror_size: (0, 0),
            owed: Vec::new(),
            in_flight: false,
            seq: 0,
        }
    }

    fn file_size(&self) -> Option<(u32, u32)> {
        self.file.as_ref().map(|file| (file.width, file.height))
    }

    /// Chromium painted `pixels`, the whole `width` by `height` frame, of which `dirty` changed (none: all of it).
    /// Returns the frame to send now, if any: none while the app reads the last one.
    pub(super) fn paint(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        dirty: &[[i32; 4]],
    ) -> io::Result<Option<Sent>> {
        if width == 0 || height == 0 || pixels.len() < width as usize * height as usize * 4 {
            return Ok(None);
        }
        let mut rects = clamp(dirty, width, height);
        let new_size = self.file_size() != Some((width, height));
        if new_size {
            rects = vec![[0, 0, width, height]];
        }
        if rects.is_empty() {
            return Ok(None);
        }
        if !self.in_flight {
            if new_size {
                self.next_file(width, height)?;
            }
            let file = self.file.as_mut().expect("a file of this size");
            coalesce(&mut rects);
            copy_rects(&mut file.map, pixels, width, &rects);
            return Ok(Some(self.sent(rects)));
        }
        // The app reads the file: this frame waits in the mirror.
        if self.mirror_size != (width, height) {
            // Chromium's rects are against the frame before, which waits here at another size: all of it then.
            if !self.owed.is_empty() {
                rects = vec![[0, 0, width, height]];
            }
            self.mirror.clear();
            self.mirror.resize(width as usize * height as usize * 4, 0);
            self.mirror_size = (width, height);
            self.owed.clear();
        }
        self.owed.extend_from_slice(&rects);
        // The rect around them all covers pixels the mirror may not have: they come from this whole frame.
        if coalesce(&mut self.owed) {
            rects = self.owed.clone();
        }
        copy_rects(&mut self.mirror, pixels, width, &rects);
        Ok(None)
    }

    /// The app read the last frame: send what was painted since, if anything was.
    pub(super) fn ack(&mut self) -> io::Result<Option<Sent>> {
        self.in_flight = false;
        if self.owed.is_empty() {
            return Ok(None);
        }
        let (width, height) = self.mirror_size;
        let mut owed = std::mem::take(&mut self.owed);
        if self.file_size() != Some((width, height)) {
            // Owed is the whole frame already (`paint`).
            self.next_file(width, height)?;
            owed = vec![[0, 0, width, height]];
        }
        let file = self.file.as_mut().expect("a file of this size");
        copy_rects(&mut file.map, &self.mirror, width, &owed);
        Ok(Some(self.sent(owed)))
    }

    fn next_file(&mut self, width: u32, height: u32) -> io::Result<()> {
        // The old file goes first: the app has read its last frame.
        self.file = None;
        let generation = self.generation;
        self.generation = self.generation.wrapping_add(1);
        self.file = Some(Mapping::create(self.id, generation, width, height)?);
        Ok(())
    }

    fn sent(&mut self, mut rects: Vec<Rect>) -> Sent {
        let file = self.file.as_ref().expect("a file to send");
        let pixels = area(&rects);
        if rects.contains(&[0, 0, file.width, file.height]) {
            rects.clear();
        }
        self.in_flight = true;
        self.seq += 1;
        Sent {
            path: file.name.clone(),
            width: file.width,
            height: file.height,
            seq: self.seq,
            dirty: rects,
            pixels,
        }
    }
}

/// The name of page `id`'s file of generation `generation`. On macOS, a shared memory object's: at most 31 bytes.
pub(super) fn file_name(id: u32, generation: u32) -> String {
    let pid = std::process::id();
    if cfg!(target_os = "macos") {
        format!("/nxw{pid:x}-{id:x}-{generation:x}")
    } else {
        frames_dir()
            .join(format!("nexus-web-{pid}-{id}-{generation}"))
            .to_string_lossy()
            .into_owned()
    }
}

/// Remove the frame files of hosts that are gone: a host killed before it could remove its own. Linux only: macOS can't
/// list shared memory objects.
#[cfg(target_os = "linux")]
pub(super) fn sweep() {
    let Ok(entries) = std::fs::read_dir(frames_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(host_of) else {
            continue;
        };
        // SAFETY: signal 0 sends nothing; it only checks that the process exists.
        let gone = unsafe { libc::kill(pid, 0) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The host's process id in a frame file's name (`nexus-web-<pid>-<id>-<generation>`), if it is one.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn host_of(name: &str) -> Option<i32> {
    let parts: Vec<&str> = name.strip_prefix("nexus-web-")?.split('-').collect();
    let numbers = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    numbers.then(|| parts[0].parse().ok()).flatten()
}

/// Where frame files go: in memory (`/dev/shm`) where the system has it.
fn frames_dir() -> std::path::PathBuf {
    let shm = std::path::PathBuf::from("/dev/shm");
    if shm.is_dir() { shm } else { std::env::temp_dir() }
}

/// One generation of a page's frame file, mapped by the host. Dropping it removes the file; the app's mapping stays
/// valid until the app lets it go.
struct Mapping {
    name: String,
    width: u32,
    height: u32,
    map: MmapMut,
    /// macOS: a shared memory object's name, not a path.
    #[cfg(target_os = "macos")]
    shm: bool,
    /// Windows: the host's handle, opened delete-on-close (`private_file`). Windows does not delete a mapped file, so
    /// the file goes when this handle and the last mapping of it (the host's, then the app's) are gone.
    #[cfg(windows)]
    _file: File,
}

impl Mapping {
    fn create(id: u32, generation: u32, width: u32, height: u32) -> io::Result<Mapping> {
        let len = width as usize * height as usize * 4;
        #[cfg(target_os = "macos")]
        let (name, file, shm) = match mac_shm::create(&file_name(id, generation), len) {
            Ok(file) => (file_name(id, generation), file, true),
            Err(error) => {
                // No shared memory here (a sandbox, say): a file in the temporary folder instead.
                eprintln!("nexus web host: shared memory for page {id}: {error}; a file instead");
                let name = std::env::temp_dir()
                    .join(format!("nexus-web-{}-{id}-{generation}", std::process::id()))
                    .to_string_lossy()
                    .into_owned();
                let file = private_file(&name, len)?;
                (name, file, false)
            }
        };
        #[cfg(not(target_os = "macos"))]
        let (name, file) = {
            let name = file_name(id, generation);
            let file = private_file(&name, len)?;
            (name, file)
        };
        // SAFETY: a file only this process and the app map; the app reads it only between a frame event and its
        // acknowledgement, while this process does not write it, and nothing ever makes it shorter.
        let map = unsafe { MmapOptions::new().len(len).map_mut(&file)? };
        Ok(Mapping {
            name,
            width,
            height,
            map,
            #[cfg(target_os = "macos")]
            shm,
            #[cfg(windows)]
            _file: file,
        })
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if self.shm {
            mac_shm::unlink(&self.name);
            return;
        }
        // Windows deletes the file itself (`_file`).
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&self.name);
    }
}

/// A new frame file of `len` bytes only this user can read: it holds what the page shows. A file left by an earlier
/// host with this process's id goes first.
fn private_file(path: &str, len: usize) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // Windows keeps a temporary file's pages in memory while it has room, instead of writing them to disk. It deletes
    // the file once no handle and no mapping holds it, also when the host is killed. The app's `File::open` shares
    // delete access, as Rust's default share mode does, so it can still open the file.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.attributes(0x100 /* FILE_ATTRIBUTE_TEMPORARY */);
        options.custom_flags(0x0400_0000 /* FILE_FLAG_DELETE_ON_CLOSE */);
    }
    let file = match options.open(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            options.open(path)?
        }
        result => result?,
    };
    file.set_len(len as u64)?;
    Ok(file)
}

#[cfg(target_os = "macos")]
mod mac_shm {
    use std::ffi::CString;
    use std::fs::File;
    use std::io;
    use std::os::fd::FromRawFd;

    fn c_name(name: &str) -> io::Result<CString> {
        CString::new(name).map_err(|_| io::ErrorKind::InvalidInput.into())
    }

    /// A new shared memory object of `len` bytes, only this user's. One left by an earlier host goes first.
    pub(super) fn create(name: &str, len: usize) -> io::Result<File> {
        let c = c_name(name)?;
        let flags = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL;
        // SAFETY: a valid C string; the descriptor is checked and then owned by the `File`.
        let file = unsafe {
            let mut fd = libc::shm_open(c.as_ptr(), flags, 0o600 as libc::c_uint);
            if fd < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                libc::shm_unlink(c.as_ptr());
                fd = libc::shm_open(c.as_ptr(), flags, 0o600 as libc::c_uint);
            }
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            File::from_raw_fd(fd)
        };
        // macOS sets a shared memory object's size once, when it is new.
        file.set_len(len as u64)?;
        Ok(file)
    }

    pub(super) fn open(name: &str) -> io::Result<File> {
        let c = c_name(name)?;
        // SAFETY: as in `create`.
        unsafe {
            let fd = libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0 as libc::c_uint);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(File::from_raw_fd(fd))
        }
    }

    pub(super) fn unlink(name: &str) {
        if let Ok(c) = c_name(name) {
            // SAFETY: a valid C string.
            unsafe { libc::shm_unlink(c.as_ptr()) };
        }
    }

    /// Whether `path` names a shared memory object (`/nxw…`) rather than a file.
    pub(super) fn is_shm(path: &str) -> bool {
        path.starts_with('/') && !path[1..].contains('/')
    }
}

/// The app's side: each page's frame file, mapped once per generation.
#[derive(Default)]
pub(super) struct Views(std::collections::HashMap<u32, View>);

struct View {
    path: String,
    map: Mmap,
}

impl View {
    fn open(path: &str, len: usize) -> io::Result<View> {
        #[cfg(target_os = "macos")]
        let file = if mac_shm::is_shm(path) {
            mac_shm::open(path)?
        } else {
            File::open(path)?
        };
        #[cfg(not(target_os = "macos"))]
        let file = File::open(path)?;
        // A shared memory object's size is rounded up to whole pages on macOS.
        if (file.metadata()?.len() as usize) < len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        // SAFETY: the host never makes the file shorter (a new size is a new file), and writes it only after this
        // side acknowledged the frame it read.
        let map = unsafe { MmapOptions::new().len(len).map(&file)? };
        Ok(View {
            path: path.to_string(),
            map,
        })
    }
}

impl Views {
    /// Read frame `seq` of page `id` from the file at `path`, before acknowledging it. `pending` is the page's last
    /// frame if its view has not taken it yet: when it is frame `seq - 1`, only the dirty rects are copied into it, and
    /// they join its `changed`. Otherwise the whole frame is, into `pending`'s buffer when its size fits, and `changed`
    /// names the rects against frame `seq - 1`. On an error `pending` stays.
    pub(super) fn read(
        &mut self,
        id: u32,
        path: &str,
        (width, height): (u32, u32),
        seq: u64,
        dirty: &[Rect],
        pending: &mut Option<super::Frame>,
    ) -> io::Result<super::Frame> {
        let len = width as usize * height as usize * 4;
        // Full-tile reuse is measured on Metal only. Other renderers keep replacing full images:
        // after GPU loss, their atlas can otherwise rebuild an updated image from its stale CPU pixels.
        #[cfg(target_os = "macos")]
        let full = [[0, 0, width, height]];
        #[cfg(target_os = "macos")]
        let dirty = if dirty.is_empty() { full.as_slice() } else { dirty };
        let view = match self.0.remove(&id) {
            Some(view) if view.path == path && view.map.len() == len => view,
            _ => View::open(path, len)?,
        };
        let view = self.0.entry(id).or_insert(view);
        let mut pending = pending.take();
        if let Some(mut frame) = pending {
            if !dirty.is_empty()
                && seq > 0
                && frame.seq + 1 == seq
                && (frame.width, frame.height) == (width, height)
                && frame.pixels.len() == len
            {
                copy_rects(&mut frame.pixels, &view.map, width, dirty);
                frame.seq = seq;
                frame.changed = frame.changed.take().map(|(since, mut rects)| {
                    rects.extend_from_slice(dirty);
                    coalesce(&mut rects);
                    (since, rects)
                });
                return Ok(frame);
            }
            pending = Some(frame);
        }
        let mut pixels = pending
            .map(|frame| frame.pixels)
            .filter(|pixels| pixels.capacity() >= len)
            .unwrap_or_default();
        pixels.clear();
        pixels.extend_from_slice(&view.map[..len]);
        Ok(super::Frame {
            pixels,
            width,
            height,
            seq,
            // The rects are against frame `seq - 1`: a view that shows it writes only those.
            changed: (seq > 1 && !dirty.is_empty()).then(|| (seq - 1, dirty.to_vec())),
        })
    }

    /// Let go of the files of pages that are gone.
    pub(super) fn retain(&mut self, open: impl Fn(u32) -> bool) {
        self.0.retain(|id, _| open(*id));
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    /// A `width` by `height` frame whose every pixel is `value`.
    fn frame(width: u32, height: u32, value: u8) -> Vec<u8> {
        vec![value; width as usize * height as usize * 4]
    }

    /// Paint `rect` of `pixels` with `value`.
    fn fill(pixels: &mut [u8], width: u32, [x, y, w, h]: Rect, value: u8) {
        for row in y..y + h {
            let at = (row * width + x) as usize * 4;
            pixels[at..at + w as usize * 4].fill(value);
        }
    }

    fn mapped(painter: &Painter) -> Vec<u8> {
        painter.file.as_ref().expect("a file").map.to_vec()
    }

    #[test]
    fn rects_are_cut_to_the_frame_and_none_means_all_of_it() {
        assert_eq!(clamp(&[], 10, 5), vec![[0, 0, 10, 5]]);
        assert_eq!(clamp(&[[-2, 3, 5, 9]], 10, 5), vec![[0, 3, 3, 2]]);
        assert_eq!(clamp(&[[10, 0, 4, 4], [0, 0, 0, 3]], 10, 5), Vec::<Rect>::new());
    }

    #[test]
    fn rects_inside_others_drop_and_too_many_become_one() {
        let mut rects = vec![[0, 0, 10, 10], [2, 2, 3, 3], [0, 0, 10, 10]];
        assert!(!coalesce(&mut rects));
        assert_eq!(rects, vec![[0, 0, 10, 10]]);
        let mut rects: Vec<Rect> = (0..9).map(|i| [i * 10, i * 2, 4, 4]).collect();
        assert!(!coalesce(&mut rects[..8].to_vec()));
        assert!(coalesce(&mut rects));
        assert_eq!(rects, vec![[0, 0, 84, 20]]);
    }

    #[test]
    fn each_size_gets_a_new_file_name() {
        let names: Vec<String> = (0..3).map(|generation| file_name(7, generation)).collect();
        assert_ne!(names[0], names[1]);
        assert_ne!(names[1], names[2]);
        assert_eq!(
            host_of(&format!("nexus-web-{}-7-2", std::process::id())),
            Some(std::process::id() as i32)
        );
        assert_eq!(host_of("nexus-web-12-7"), None);
        assert_eq!(host_of("nexus-web-12-7-x"), None);
        if cfg!(target_os = "macos") {
            assert!(names.iter().all(|name| name.len() <= 31 && name.starts_with("/nxw")));
            assert!(file_name(u32::MAX, u32::MAX).len() <= 31);
        } else {
            assert!(names[2].ends_with(&format!("nexus-web-{}-7-2", std::process::id())));
        }
    }

    #[test]
    fn a_new_size_moves_to_a_new_file_and_removes_the_old() {
        let mut painter = Painter::new(9001);
        let first = painter.paint(&frame(4, 2, 1), 4, 2, &[]).unwrap().unwrap();
        assert!(std::path::Path::new(&first.path).exists() || cfg!(target_os = "macos"));
        painter.ack().unwrap();
        // A small change, at the same size: the same file, one rect.
        let mut pixels = frame(4, 2, 1);
        fill(&mut pixels, 4, [1, 0, 1, 1], 2);
        let same = painter.paint(&pixels, 4, 2, &[[1, 0, 1, 1]]).unwrap().unwrap();
        assert_eq!(
            (same.path.as_str(), same.dirty.as_slice(), same.seq),
            (first.path.as_str(), &[[1, 0, 1, 1]][..], 2)
        );
        assert_eq!(mapped(&painter), pixels);
        painter.ack().unwrap();
        // Larger: a new file, the whole frame, even if Chromium names one rect.
        let grown = painter.paint(&frame(6, 3, 5), 6, 3, &[[0, 0, 1, 1]]).unwrap().unwrap();
        assert_ne!(grown.path, first.path);
        assert!(grown.dirty.is_empty());
        assert_eq!(grown.pixels, 18);
        assert!(!std::path::Path::new(&first.path).exists());
        let path = grown.path.clone();
        drop(painter);
        assert!(!std::path::Path::new(&path).exists());
    }

    #[test]
    fn frames_painted_while_the_app_reads_wait_and_go_out_together() {
        let mut painter = Painter::new(9002);
        let mut latest = frame(8, 4, 0);
        painter.paint(&latest, 8, 4, &[]).unwrap().unwrap();
        // In flight: two paints, nothing sent, the file untouched.
        fill(&mut latest, 8, [0, 0, 2, 2], 1);
        assert_eq!(painter.paint(&latest, 8, 4, &[[0, 0, 2, 2]]).unwrap(), None);
        fill(&mut latest, 8, [6, 3, 2, 1], 2);
        assert_eq!(painter.paint(&latest, 8, 4, &[[6, 3, 2, 1]]).unwrap(), None);
        assert_eq!(mapped(&painter), frame(8, 4, 0));
        // The acknowledgement sends both rects, and the file then holds the latest frame.
        let sent = painter.ack().unwrap().unwrap();
        assert_eq!(sent.dirty, vec![[0, 0, 2, 2], [6, 3, 2, 1]]);
        assert_eq!((sent.seq, sent.pixels), (2, 6));
        assert_eq!(mapped(&painter), latest);
        // Nothing painted since: nothing to send.
        assert_eq!(painter.ack().unwrap(), None);
        assert!(painter.paint(&latest, 8, 4, &[[0, 0, 1, 1]]).unwrap().is_some());
    }

    #[test]
    fn many_waiting_rects_become_one_from_the_latest_frame() {
        let mut painter = Painter::new(9003);
        // The mirror starts empty (zeros); the frames are 5s: the rect around the rects must come from the frame.
        let mut latest = frame(32, 4, 5);
        painter.paint(&latest, 32, 4, &[]).unwrap().unwrap();
        for i in 0..9u32 {
            fill(&mut latest, 32, [i * 3, 0, 1, 1], 3);
            assert_eq!(
                painter.paint(&latest, 32, 4, &[[(i * 3) as i32, 0, 1, 1]]).unwrap(),
                None
            );
        }
        let sent = painter.ack().unwrap().unwrap();
        assert_eq!(sent.dirty, vec![[0, 0, 25, 1]]);
        assert_eq!(mapped(&painter), latest);
    }

    #[test]
    fn a_resize_while_the_app_reads_sends_the_whole_new_frame() {
        let mut painter = Painter::new(9004);
        let first = painter.paint(&frame(4, 4, 1), 4, 4, &[]).unwrap().unwrap();
        // Two sizes while in flight: only the last counts.
        painter.paint(&frame(5, 5, 2), 5, 5, &[[0, 0, 1, 1]]).unwrap();
        painter.paint(&frame(6, 2, 3), 6, 2, &[[0, 0, 1, 1]]).unwrap();
        let sent = painter.ack().unwrap().unwrap();
        assert_ne!(sent.path, first.path);
        assert_eq!((sent.width, sent.height, sent.dirty.len()), (6, 2, 0));
        assert_eq!(mapped(&painter), frame(6, 2, 3));
        // The mirror's size again while the file has another: the whole frame, not the one rect.
        painter.paint(&frame(5, 5, 4), 5, 5, &[[0, 0, 1, 1]]).unwrap();
        painter.ack().unwrap().unwrap();
        assert_eq!(painter.ack().unwrap(), None);
        painter.paint(&frame(6, 2, 5), 6, 2, &[]).unwrap().unwrap();
        painter.paint(&frame(5, 5, 6), 5, 5, &[[0, 0, 1, 1]]).unwrap();
        let sent = painter.ack().unwrap().unwrap();
        assert_eq!((sent.width, sent.dirty.len()), (5, 0));
        assert_eq!(mapped(&painter), frame(5, 5, 6));
        // Another size and back while the app reads: the rects are against the other size, so all of it.
        painter.paint(&frame(7, 7, 7), 7, 7, &[]).unwrap();
        painter.paint(&frame(5, 5, 8), 5, 5, &[[0, 0, 1, 1]]).unwrap();
        let sent = painter.ack().unwrap().unwrap();
        assert_eq!((sent.width, sent.dirty.len()), (5, 0));
        assert_eq!(mapped(&painter), frame(5, 5, 8));
    }

    #[test]
    fn the_app_patches_its_pending_frame_or_reads_the_whole_one() {
        let mut painter = Painter::new(9005);
        let mut latest = frame(4, 4, 1);
        let sent = painter.paint(&latest, 4, 4, &[]).unwrap().unwrap();
        let mut views = Views::default();
        let read = |views: &mut Views, sent: &Sent, mut pending| {
            views
                .read(
                    9005,
                    &sent.path,
                    (sent.width, sent.height),
                    sent.seq,
                    &sent.dirty,
                    &mut pending,
                )
                .unwrap()
        };
        let first = read(&mut views, &sent, None);
        assert_eq!(first.pixels, latest);
        painter.ack().unwrap();
        fill(&mut latest, 4, [1, 1, 2, 2], 9);
        let sent = painter.paint(&latest, 4, 4, &[[1, 1, 2, 2]]).unwrap().unwrap();
        // The view did not take frame 1: frame 2's rect goes into it.
        let second = read(&mut views, &sent, Some(first));
        assert_eq!((second.seq, &second.pixels), (2, &latest));
        painter.ack().unwrap();
        // A pending frame that is not the one before: the whole frame.
        fill(&mut latest, 4, [0, 0, 1, 1], 4);
        let sent = painter.paint(&latest, 4, 4, &[[0, 0, 1, 1]]).unwrap().unwrap();
        let stale = super::super::Frame {
            pixels: frame(4, 4, 0),
            width: 4,
            height: 4,
            seq: 1,
            changed: None,
        };
        assert_eq!(read(&mut views, &sent, Some(stale)).pixels, latest);
        views.retain(|_| false);
        assert!(views.0.is_empty());
    }

    /// The rects a frame carries are against the frame its view shows: a view can write only those into its image.
    #[test]
    fn a_frame_names_what_changed_since_the_one_shown() {
        let mut painter = Painter::new(9006);
        let mut latest = frame(4, 4, 1);
        let mut views = Views::default();
        let read = |views: &mut Views, sent: &Sent, mut pending| {
            views
                .read(
                    9006,
                    &sent.path,
                    (sent.width, sent.height),
                    sent.seq,
                    &sent.dirty,
                    &mut pending,
                )
                .unwrap()
        };
        // The first frame: all of it.
        let sent = painter.paint(&latest, 4, 4, &[]).unwrap().unwrap();
        let first = read(&mut views, &sent, None);
        assert_eq!(first.changed, None);
        painter.ack().unwrap();

        // The view took frame 1: the whole frame is read, and the rect is against frame 1.
        fill(&mut latest, 4, [1, 1, 2, 2], 9);
        let sent = painter.paint(&latest, 4, 4, &[[1, 1, 2, 2]]).unwrap().unwrap();
        let second = read(&mut views, &sent, None);
        assert_eq!(
            (&second.pixels, &second.changed),
            (&latest, &Some((1, vec![[1, 1, 2, 2]])))
        );
        painter.ack().unwrap();

        // The view took frame 2 too.
        fill(&mut latest, 4, [0, 0, 1, 1], 5);
        let sent = painter.paint(&latest, 4, 4, &[[0, 0, 1, 1]]).unwrap().unwrap();
        let third = read(&mut views, &sent, None);
        assert_eq!(
            (&third.pixels, &third.changed),
            (&latest, &Some((2, vec![[0, 0, 1, 1]])))
        );
        painter.ack().unwrap();

        // The view has not taken frame 3: frame 4's rect joins it, still against frame 2.
        fill(&mut latest, 4, [3, 3, 1, 1], 6);
        let sent = painter.paint(&latest, 4, 4, &[[3, 3, 1, 1]]).unwrap().unwrap();
        let fourth = read(&mut views, &sent, Some(third));
        assert_eq!(fourth.pixels, latest);
        assert_eq!(fourth.changed, Some((2, vec![[0, 0, 1, 1], [3, 3, 1, 1]])));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn full_repaints_keep_the_image_tile_and_merge_pending_changes() {
        let mut painter = Painter::new(9010);
        let mut views = Views::default();
        let mut latest = frame(4, 4, 1);
        let read = |views: &mut Views, sent: &Sent, mut pending| {
            views
                .read(
                    9010,
                    &sent.path,
                    (sent.width, sent.height),
                    sent.seq,
                    &sent.dirty,
                    &mut pending,
                )
                .unwrap()
        };
        let sent = painter.paint(&latest, 4, 4, &[]).unwrap().unwrap();
        let first = read(&mut views, &sent, None);
        assert!(first.changed.is_none(), "the first frame creates its image");
        painter.ack().unwrap();

        latest.fill(2);
        let sent = painter.paint(&latest, 4, 4, &[]).unwrap().unwrap();
        assert!(sent.dirty.is_empty(), "the wire format stays unchanged");
        let second = read(&mut views, &sent, None);
        let full = vec![[0, 0, 4, 4]];
        assert_eq!(second.pixels, latest);
        assert_eq!(second.changed, Some((1, full.clone())));
        assert_eq!(super::super::changes_onto(1, (4, 4), &second), Some(full.as_slice()));
        assert_eq!(super::super::changes_onto(0, (4, 4), &second), None);
        assert_eq!(super::super::changes_onto(1, (5, 4), &second), None);
        painter.ack().unwrap();

        // A full repaint joins the pending partial update, still against the image on screen.
        fill(&mut latest, 4, [0, 0, 1, 1], 3);
        let sent = painter.paint(&latest, 4, 4, &[[0, 0, 1, 1]]).unwrap().unwrap();
        let third = read(&mut views, &sent, None);
        painter.ack().unwrap();
        latest.fill(4);
        let sent = painter.paint(&latest, 4, 4, &[]).unwrap().unwrap();
        let fourth = read(&mut views, &sent, Some(third));
        assert_eq!(fourth.pixels, latest);
        assert_eq!(fourth.changed, Some((2, full.clone())));
        assert_eq!(super::super::changes_onto(2, (4, 4), &fourth), Some(full.as_slice()));
        assert_eq!(super::super::changes_onto(1, (4, 4), &fourth), None);
        painter.ack().unwrap();

        // A following partial repaint remains covered by the pending full repaint.
        fill(&mut latest, 4, [3, 3, 1, 1], 5);
        let sent = painter.paint(&latest, 4, 4, &[[3, 3, 1, 1]]).unwrap().unwrap();
        let fifth = read(&mut views, &sent, Some(fourth));
        assert_eq!(fifth.pixels, latest);
        assert_eq!(fifth.changed, Some((2, full)));
        painter.ack().unwrap();

        // Resizing still makes a new image, even with complete repaint metadata.
        let resized = frame(5, 4, 6);
        let sent = painter.paint(&resized, 5, 4, &[]).unwrap().unwrap();
        let sixth = read(&mut views, &sent, Some(fifth));
        assert_eq!(sixth.pixels, resized);
        assert_eq!(super::super::changes_onto(5, (4, 4), &sixth), None);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn full_repaints_keep_replacing_images_outside_metal() {
        let mut painter = Painter::new(9011);
        let mut views = Views::default();
        let read = |views: &mut Views, sent: &Sent, mut pending| {
            views
                .read(
                    9011,
                    &sent.path,
                    (sent.width, sent.height),
                    sent.seq,
                    &sent.dirty,
                    &mut pending,
                )
                .unwrap()
        };
        let sent = painter.paint(&frame(4, 4, 1), 4, 4, &[]).unwrap().unwrap();
        let first = read(&mut views, &sent, None);
        assert!(first.changed.is_none());
        painter.ack().unwrap();
        let latest = frame(4, 4, 2);
        let sent = painter.paint(&latest, 4, 4, &[]).unwrap().unwrap();
        let second = read(&mut views, &sent, Some(first));
        assert_eq!(second.pixels, latest);
        assert!(
            second.changed.is_none(),
            "full repaints must refresh the CPU image backing"
        );
    }

    #[test]
    fn rect_rows_packs_a_rect_or_refuses_one_outside() {
        let pixels: Vec<u8> = (0..4 * 3 * 4).map(|value| value as u8).collect();
        let mut out = Vec::new();
        assert!(rect_rows(&pixels, 4, [1, 1, 2, 2], &mut out));
        assert_eq!(out, [20, 21, 22, 23, 24, 25, 26, 27, 36, 37, 38, 39, 40, 41, 42, 43]);
        assert!(!rect_rows(&pixels, 4, [3, 0, 2, 1], &mut out));
        assert!(!rect_rows(&pixels, 4, [0, 2, 1, 2], &mut out));
        assert!(!rect_rows(&pixels, 4, [u32::MAX, 0, 2, 1], &mut out));
    }
}
