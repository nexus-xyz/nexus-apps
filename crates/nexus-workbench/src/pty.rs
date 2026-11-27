//! Settings every pseudo-terminal needs before its program starts: the runner's commands (`terminal/process.rs`)
//! and Nexus GPUI's terminal.

use portable_pty::MasterPty;

/// Make the terminal's line editing count UTF-8 characters, not bytes (`IUTF8`), as a terminal emulator does. A new
/// pseudo-terminal starts without it. Then Backspace in a program that reads a whole line, such as `read`, erases one
/// byte of a two-byte letter but a whole column on screen: the screen walks back into the prompt while the bytes left
/// reach the program. Call it before the program starts. A no-op where the flag does not exist (Windows, the BSDs).
pub fn utf8_line_editing(master: &dyn MasterPty) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        use nix::fcntl::OFlag;
        use nix::sys::termios::{InputFlags, SetArg, tcgetattr, tcsetattr};
        use std::os::unix::fs::OpenOptionsExt;
        // The terminal's own end, by name: the settings belong to the pair. Not as this process's controlling
        // terminal, which a session leader would otherwise take.
        let path = master
            .tty_name()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "the terminal has no name"))?;
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlag::O_NOCTTY.bits())
            .open(path)?;
        let mut settings = tcgetattr(&tty)?;
        settings.input_flags.insert(InputFlags::IUTF8);
        tcsetattr(&tty, SetArg::TCSANOW, &settings)?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    let _ = master;
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests {
    use super::*;
    use nix::sys::termios::{InputFlags, tcgetattr};
    use portable_pty::{PtySize, native_pty_system};

    #[test]
    fn sets_utf8_line_editing_on_a_new_terminal() {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("open a pseudo-terminal");
        utf8_line_editing(&*pair.master).expect("set it");
        let tty = std::fs::File::open(pair.master.tty_name().expect("its name")).expect("open its end");
        let settings = tcgetattr(&tty).expect("read the settings");
        assert!(settings.input_flags.contains(InputFlags::IUTF8));
    }
}
