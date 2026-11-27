//! The session's terminal: raw mode while it runs, the terminal's background colour (for the shade), and drawing a
//! frame in place. As the picker (`picker.rs`) does, it draws inline, never on the alternate screen: what the session
//! printed stays in the scrollback after it ends.

use std::io::Write as _;

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::app::Frame;
use super::view::display_width;

/// Raw mode, pasted text as one event, and ⇧⏎ told apart from ⏎ where the terminal can; all undone on drop, a panic
/// included.
pub struct Raw;

impl Raw {
    pub fn enter() -> std::io::Result<Raw> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        // A terminal without these ignores them (Windows' console refuses the keyboard flags: also fine).
        let _ = crossterm::execute!(stdout, EnableBracketedPaste);
        let _ = crossterm::execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        Ok(Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = crossterm::execute!(stdout, PopKeyboardEnhancementFlags);
        let _ = crossterm::execute!(stdout, DisableBracketedPaste);
        let _ = write!(stdout, "\x1b[?25h");
        let _ = stdout.flush();
        let _ = disable_raw_mode();
    }
}

/// A Braille dot's height over its width on this terminal, from its size in cells and in pixels (most terminals
/// report both); the mark's default when it does not say.
pub fn aspect() -> f64 {
    match crossterm::terminal::window_size() {
        Ok(size) if size.width > 0 && size.height > 0 && size.columns > 0 && size.rows > 0 => {
            let cell_width = f64::from(size.width) / f64::from(size.columns);
            let cell_height = f64::from(size.height) / f64::from(size.rows);
            // A cell holds 2 × 4 dots.
            ((cell_height / 4.0) / (cell_width / 2.0)).clamp(0.5, 2.0)
        }
        _ => super::logo::ASPECT,
    }
}

/// The terminal's background colour, asked with OSC 11 (most terminals answer within a few milliseconds). `None` when
/// it does not answer in time, or off Unix; the session then draws without a shade. Call it in raw mode.
#[cfg(unix)]
pub fn background() -> Option<(u8, u8, u8)> {
    use std::os::fd::AsFd as _;
    use std::time::{Duration, Instant};

    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

    let mut stdout = std::io::stdout();
    write!(stdout, "\x1b]11;?\x1b\\").ok()?;
    stdout.flush().ok()?;
    let stdin = std::io::stdin();
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut reply = Vec::new();
    while !reply.contains(&0x07) && !reply.ends_with(b"\x1b\\") {
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = PollTimeout::from(left.as_millis().min(100) as u16);
        let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        if left.is_zero() || !matches!(poll(&mut fds, timeout), Ok(ready) if ready > 0) {
            break;
        }
        let mut buffer = [0u8; 64];
        match nix::unistd::read(stdin.as_fd(), &mut buffer) {
            Ok(read) if read > 0 => reply.extend_from_slice(&buffer[..read]),
            _ => break,
        }
    }
    parse_background(&reply)
}

#[cfg(not(unix))]
pub fn background() -> Option<(u8, u8, u8)> {
    None
}

/// `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` (each part 1 to 4 hex digits) as 8-bit channels.
pub fn parse_background(reply: &[u8]) -> Option<(u8, u8, u8)> {
    let text = String::from_utf8_lossy(reply);
    let rgb = text.split("rgb:").nth(1)?;
    let rgb = rgb.split(['\x07', '\x1b']).next()?;
    let mut channels = rgb.split('/').map(|part| {
        let value = u32::from_str_radix(part, 16).ok()?;
        let max = (1u32 << (4 * part.len().clamp(1, 4))) - 1;
        Some((value * 255 / max) as u8)
    });
    Some((channels.next()??, channels.next()??, channels.next()??))
}

/// Draws frames in place: each one moves back to where the last live part began, clears below, prints the new
/// scrollback lines, then the live part padded down to the terminal's last row, and puts the cursor in the composer.
#[derive(Debug, Default)]
pub struct Screen {
    /// The cursor's row in the live part drawn last; `None` before the first frame.
    cursor_row: Option<usize>,
    /// The terminal row the live part starts on. Taken as 0 at first and after a resize: too small a guess only fills
    /// more rows than are free, the terminal scrolls, and the composer still ends on the last row.
    top: usize,
}

impl Screen {
    pub fn draw(&mut self, frame: &Frame, width: usize, height: usize) -> std::io::Result<()> {
        if frame.clear {
            self.top = 0;
        }
        let placed = place(frame, self.top, width, height);
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        if frame.clear {
            // The screen, the scrollback, and home: the frame's history starts the screen again.
            out.push_str("\x1b[2J\x1b[3J\x1b[H");
        } else {
            out.push_str(&self.back());
        }
        for line in &frame.history {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str(&placed.live.join("\r\n"));
        let below = placed.live.len().saturating_sub(1).saturating_sub(placed.cursor_row);
        if below > 0 {
            out.push_str(&format!("\x1b[{below}A"));
        }
        out.push('\r');
        if frame.cursor.1 > 0 {
            out.push_str(&format!("\x1b[{}C", frame.cursor.1));
        }
        out.push_str("\x1b[?25h\x1b[?2026l");
        self.cursor_row = Some(placed.cursor_row);
        self.top = placed.top;
        write_all(&out)
    }

    /// After a resize the rows have moved: guess again (see `top`).
    pub fn resized(&mut self) {
        self.top = 0;
    }

    /// Clear the live part and leave `lines` under the scrollback.
    pub fn finish(&mut self, lines: &[String]) -> std::io::Result<()> {
        let mut out = self.back();
        for line in lines {
            out.push_str(line);
            out.push_str("\r\n");
        }
        self.cursor_row = None;
        write_all(&out)
    }

    /// To the start of the live part drawn last, clearing it.
    fn back(&self) -> String {
        match self.cursor_row {
            Some(0) => "\r\x1b[J".into(),
            Some(row) => format!("\x1b[{row}A\r\x1b[J"),
            None => String::new(),
        }
    }
}

/// The live part as drawn, the cursor's row in it, and the row it will start on.
#[derive(Debug, PartialEq, Eq)]
struct Placed {
    live: Vec<String>,
    cursor_row: usize,
    top: usize,
}

/// Pad the frame's live part so it ends on the terminal's last row, when the live part starts on row `top` and the
/// terminal is `width` × `height`. A scrollback line wider than the terminal takes more than one row.
fn place(frame: &Frame, top: usize, width: usize, height: usize) -> Placed {
    let history: usize = frame
        .history
        .iter()
        .map(|line| display_width(line).div_ceil(width.max(1)).max(1))
        .sum();
    let free = height.saturating_sub(top + history);
    let spare = free.saturating_sub(frame.live.len());
    let above = if frame.center { spare / 2 } else { 0 };
    let split = frame.split.min(frame.live.len());
    let mut live = vec![String::new(); above];
    live.extend(frame.live[..split].iter().cloned());
    live.extend(std::iter::repeat_n(String::new(), spare - above));
    live.extend(frame.live[split..].iter().cloned());
    let cursor_row = frame.cursor.0 + if frame.cursor.0 >= split { spare } else { above };
    // Past the last row, the terminal scrolls: the live part then ends on the last row.
    let start = top + history;
    let top = if start + live.len() > height {
        height.saturating_sub(live.len())
    } else {
        start
    };
    Placed { live, cursor_row, top }
}

fn write_all(text: &str) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(live: usize, split: usize, center: bool) -> Frame {
        Frame {
            clear: false,
            history: vec!["header".into(), "x".repeat(25)],
            live: (0..live).map(|row| format!("row {row}")).collect(),
            cursor: (live - 2, 2),
            split,
            center,
        }
    }

    #[test]
    fn the_composer_ends_on_the_last_row() {
        // 2 history lines, the second 25 wide on a 20-column terminal: 3 rows. 24 - 3 = 21 rows free for 5 lines.
        let placed = place(&frame(5, 0, false), 0, 20, 24);
        assert_eq!(placed.live.len(), 21);
        assert_eq!(placed.live[20], "row 4");
        assert_eq!(placed.cursor_row, 19);
        assert_eq!(placed.top, 3);
    }

    #[test]
    fn the_mark_sits_in_the_middle_of_the_free_rows() {
        let placed = place(&frame(6, 3, true), 0, 80, 20);
        // 20 - 2 = 18 rows free for 6 lines: 12 spare, 6 above and 6 between the mark and the composer.
        assert_eq!(placed.live.len(), 18);
        assert_eq!(placed.live[6], "row 0");
        assert_eq!(placed.live[15], "row 3");
        assert_eq!(placed.cursor_row, 4 + 12);
    }

    #[test]
    fn a_full_screen_scrolls() {
        let placed = place(&frame(5, 0, false), 18, 80, 20);
        assert_eq!(placed.live.len(), 5, "no rows to spare");
        assert_eq!(
            placed.top, 15,
            "the terminal scrolled: the live part ends on the last row"
        );
    }

    #[test]
    fn reads_the_background_reply() {
        assert_eq!(
            parse_background(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"),
            Some((30, 30, 46))
        );
        assert_eq!(parse_background(b"\x1b]11;rgb:ff/80/00\x07"), Some((255, 128, 0)));
        assert_eq!(parse_background(b""), None);
        assert_eq!(parse_background(b"\x1b]11;rgb:zz/00/00\x07"), None);
    }
}
