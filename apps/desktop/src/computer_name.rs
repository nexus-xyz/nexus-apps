//! This computer's name, as the person knows it: `DESKTOP-7Q2K1AB` on Windows, `Ada-MacBook-Pro` on a Mac. The
//! Devices page offers it as the device name, and the sign-in names this computer by it, so two computers of one
//! person read apart. Read without starting a process.

/// The name this computer registers under on the Devices page, until the person types another.
pub fn desktop_label() -> String {
    label(system_name().as_deref())
}

/// The computer's name, or the system's name when the computer has none: the sign-in's "Nexus for desktop on <this>".
pub fn name_or_os() -> String {
    system_name().unwrap_or_else(|| os().into())
}

fn label(name: Option<&str>) -> String {
    name.map(str::to_string)
        .unwrap_or_else(|| format!("Nexus for desktop on {}", os()))
}

fn os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    }
}

/// A host name as the system gives it, without the `.local` that macOS adds. None when nothing is left.
fn tidy(raw: &str) -> Option<String> {
    let name = raw.trim().trim_end_matches('\0');
    let name = match name.len().checked_sub(".local".len()) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".local") => &name[..cut],
        _ => name,
    };
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(windows)]
fn system_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().and_then(|name| tidy(&name))
}

#[cfg(unix)]
fn system_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its whole length, and gethostname writes at most that many bytes.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return None;
    }
    let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    tidy(&String::from_utf8_lossy(&buffer[..end]))
}

#[cfg(not(any(unix, windows)))]
fn system_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_name_is_the_computers_own() {
        assert_eq!(label(Some("DESKTOP-7Q2K1AB")), "DESKTOP-7Q2K1AB");
    }

    #[test]
    fn a_computer_with_no_name_falls_back_to_the_system() {
        assert_eq!(label(None), format!("Nexus for desktop on {}", os()));
    }

    #[test]
    fn the_name_drops_macos_local_suffix_and_spaces() {
        assert_eq!(tidy(" Ada-MacBook-Pro.local\n").as_deref(), Some("Ada-MacBook-Pro"));
        assert_eq!(tidy("studio.LOCAL").as_deref(), Some("studio"));
        assert_eq!(tidy("build.corp.example").as_deref(), Some("build.corp.example"));
    }

    #[test]
    fn an_empty_name_reads_as_none() {
        assert_eq!(tidy(""), None);
        assert_eq!(tidy("  "), None);
        assert_eq!(tidy(".local"), None);
    }

    #[test]
    fn this_computer_has_a_name() {
        assert!(system_name().is_some_and(|name| !name.is_empty()));
    }
}
