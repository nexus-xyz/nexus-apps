//! A shell's latest output, by byte cursor: what a read after a cursor gets, and whether older bytes were dropped.

use std::collections::VecDeque;

/// The newest `cap` bytes a shell wrote. A byte's cursor is its place in everything the shell wrote, so a client that
/// reconnects reads on from its cursor without loss, as long as the bytes are still kept.
#[derive(Debug)]
pub(crate) struct Output {
    bytes: VecDeque<u8>,
    /// The cursor of `bytes[0]`.
    start: u64,
    cap: usize,
}

/// What a read gets: the bytes, the cursor after them, and whether bytes before them were dropped.
#[derive(Debug, PartialEq)]
pub(crate) struct Slice {
    pub bytes: Vec<u8>,
    pub next: u64,
    pub truncated: bool,
}

impl Output {
    pub(crate) fn new(cap: usize) -> Output {
        Output {
            bytes: VecDeque::new(),
            start: 0,
            cap: cap.max(1),
        }
    }

    /// Keeps `data`, dropping the oldest bytes past the cap.
    pub(crate) fn push(&mut self, data: &[u8]) {
        if data.len() >= self.cap {
            self.start += (self.bytes.len() + data.len() - self.cap) as u64;
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.cap..]);
            return;
        }
        self.bytes.extend(data);
        let over = self.bytes.len().saturating_sub(self.cap);
        if over > 0 {
            self.bytes.drain(..over);
            self.start += over as u64;
        }
    }

    /// The cursor after the last byte.
    pub(crate) fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    /// At most `max` bytes after cursor `after`. A cursor before the kept bytes starts at the oldest kept one
    /// (`truncated`); one past the end reads nothing.
    pub(crate) fn read(&self, after: u64, max: usize) -> Slice {
        let truncated = after < self.start;
        let from = after.clamp(self.start, self.end());
        let skip = (from - self.start) as usize;
        let take = max.min(self.bytes.len() - skip);
        Slice {
            bytes: self.bytes.range(skip..skip + take).copied().collect(),
            next: from + take as u64,
            truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_on_from_a_cursor_and_says_when_bytes_were_dropped() {
        let mut output = Output::new(8);
        output.push(b"hello");
        assert_eq!(
            output.read(0, 100),
            Slice {
                bytes: b"hello".to_vec(),
                next: 5,
                truncated: false
            }
        );
        assert_eq!(output.read(2, 2).bytes, b"ll");
        output.push(b" world");
        // 11 bytes written, 8 kept: cursor 3 onwards.
        assert_eq!(output.end(), 11);
        let late = output.read(0, 100);
        assert_eq!(late.bytes, b"lo world");
        assert!(late.truncated);
        assert_eq!(late.next, 11);
        assert!(!output.read(5, 100).truncated);
        assert_eq!(output.read(11, 100).bytes, b"");
        assert_eq!(output.read(99, 100).next, 11);
    }

    #[test]
    fn a_write_larger_than_the_cap_keeps_its_tail() {
        let mut output = Output::new(4);
        output.push(b"ab");
        output.push(b"0123456789");
        assert_eq!(output.end(), 12);
        let slice = output.read(0, 100);
        assert_eq!(slice.bytes, b"6789");
        assert!(slice.truncated);
    }
}
