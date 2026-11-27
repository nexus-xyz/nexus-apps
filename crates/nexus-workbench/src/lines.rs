//! An agent's output, one line at a time, none longer than a cap: a line that never ends must not take all memory.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// The longest line kept from an agent: 64 MiB. Claude Code writes a tool's whole result on one line, images as
/// base64 too, so lines of tens of megabytes are real; anything longer is dropped rather than held.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

/// Reads lines of at most `max` bytes (the newline included) from `reader`. A longer line is dropped as it arrives,
/// with a warning, and reading goes on with the next.
pub struct Lines<R> {
    reader: R,
    max: usize,
    /// Whose output this is, for the warning.
    label: String,
    /// In a line over `max`: its bytes are dropped up to its newline.
    skipping: bool,
}

impl<R: AsyncBufRead + Unpin> Lines<R> {
    pub fn new(reader: R, max: usize, label: impl Into<String>) -> Lines<R> {
        Lines {
            reader,
            max,
            label: label.into(),
            skipping: false,
        }
    }

    /// Reads the next line into `buffer`, its `\n` included, as `read_until` does: the line's length, or 0 at the end.
    /// Cancel-safe, like `read_until`: it only waits in `fill_buf`, and a line cut off stays in `buffer` (or, when
    /// being dropped, in `skipping`), so the next call goes on with it.
    pub async fn next(&mut self, buffer: &mut Vec<u8>) -> std::io::Result<usize> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                // The end. A last line with no newline still counts, unless it was being dropped.
                if std::mem::take(&mut self.skipping) {
                    buffer.clear();
                }
                return Ok(buffer.len());
            }
            let (used, ends) = match available.iter().position(|&byte| byte == b'\n') {
                Some(newline) => (newline + 1, true),
                None => (available.len(), false),
            };
            if !self.skipping {
                if buffer.len() + used > self.max {
                    eprintln!(
                        "nexus-workbench: {}: dropped an output line longer than {} bytes",
                        self.label, self.max
                    );
                    self.skipping = true;
                    // Gives the memory back, not just the length.
                    *buffer = Vec::new();
                } else {
                    buffer.extend_from_slice(&available[..used]);
                }
            }
            self.reader.consume(used);
            if ends {
                if std::mem::take(&mut self.skipping) {
                    continue;
                }
                return Ok(buffer.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    /// Every line `text` reads as, read 4 bytes at a time so lines span reads.
    async fn read_all(text: &[u8], max: usize) -> Vec<String> {
        let mut lines = Lines::new(BufReader::with_capacity(4, text), max, "test");
        let mut buffer = Vec::new();
        let mut out = Vec::new();
        while lines.next(&mut buffer).await.unwrap_or(0) > 0 {
            out.push(String::from_utf8_lossy(&buffer).into_owned());
            buffer.clear();
        }
        out
    }

    #[tokio::test]
    async fn reads_lines_and_a_last_one_with_no_newline() {
        assert_eq!(read_all(b"one\ntwo\r\nlast", 64).await, ["one\n", "two\r\n", "last"]);
        assert!(read_all(b"", 64).await.is_empty());
    }

    #[tokio::test]
    async fn drops_a_line_over_the_cap_and_goes_on_with_the_next() {
        let long = "x".repeat(50);
        let text = format!("a\n{long}\nb\n{long}");
        // A line of exactly the cap is kept; one byte more is dropped, a last one with no newline too.
        assert_eq!(read_all(text.as_bytes(), 10).await, ["a\n", "b\n"]);
        assert_eq!(
            read_all(b"123456789\n1234567890\nc\n", 10).await,
            ["123456789\n", "c\n"]
        );
    }

    #[tokio::test]
    async fn holds_no_more_than_the_cap_while_a_line_never_ends() {
        let endless = vec![b'x'; 1 << 20];
        let mut lines = Lines::new(BufReader::with_capacity(4096, endless.as_slice()), 1024, "test");
        let mut buffer = Vec::new();
        assert_eq!(lines.next(&mut buffer).await.unwrap_or(1), 0);
        assert!(buffer.capacity() <= 1024, "{}", buffer.capacity());
    }
}
