//! Linux matches a window to its launcher by this identity, including the launcher's icon.

pub const DEFAULT: &str = "nexus-desktop-gpui";

pub fn get() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(id) = std::env::var("NEXUS_DESKTOP_APP_ID")
        && valid(&id)
    {
        return id;
    }
    DEFAULT.to_string()
}

/// A desktop entry's filename without `.desktop`. Keep it a single filename and a safe Exec argument.
#[cfg(any(target_os = "linux", test))]
fn valid(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        && id != "."
        && id != ".."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_and_custom_launcher_names_are_valid() {
        assert!(valid(DEFAULT));
        assert!(valid("nexus-desktop-gpui-dev"));
        assert!(valid("xyz.nexus.desktop-gpui"));
    }

    #[test]
    fn empty_names_paths_and_exec_arguments_are_invalid() {
        for id in [
            "",
            ".",
            "..",
            "../other",
            "/tmp/other",
            "other app",
            "other;app",
            "other\napp",
        ] {
            assert!(!valid(id), "{id:?}");
        }
    }
}
