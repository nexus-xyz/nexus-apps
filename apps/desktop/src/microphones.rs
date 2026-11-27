//! The microphones dictation can use: the list in the composer's microphone menu (`workspace/mic.rs`), and the
//! system's sound settings.
//!
//! On macOS and Windows they are cpal's input devices, with the names the OS gives them. On Linux they are the sound
//! server's sources, as `pactl` lists them (PipeWire or PulseAudio), with their descriptions. cpal's ALSA list there
//! names sound cards, not microphones, and misses Bluetooth headsets; [`crate::dictation`] opens a chosen source
//! through PipeWire's ALSA device instead.

/// One microphone in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    /// What dictation opens it by: the device name (cpal), or the source name (Linux).
    pub id: String,
    /// What the menu shows.
    pub name: String,
    pub kind: Kind,
}

/// What a microphone is, for its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    BuiltIn,
    Headset,
    Usb,
    Other,
}

/// The system's microphones, read once per menu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Microphones {
    /// The name of the microphone the system default is now, when the system says.
    pub default: Option<String>,
    pub list: Vec<Microphone>,
}

impl Microphones {
    pub fn find(&self, id: &str) -> Option<&Microphone> {
        self.list.iter().find(|microphone| microphone.id == id)
    }
}

/// The microphone the person chose in the menu, as saved (`paths::dictation_microphone`). None is the system default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    /// Kept with the id, so a note can name a microphone that is no longer connected.
    pub name: String,
}

/// Read the system's microphones. Blocks for a moment (a device scan, or `pactl`): call it off the UI thread.
pub fn list() -> Microphones {
    platform::list()
}

/// Open the system's sound settings, on its input page where the system has one. False when nothing opened.
pub fn open_sound_settings() -> bool {
    platform::open_sound_settings()
}

/// Open the system's microphone privacy settings (macOS and Windows), or its sound settings (Linux, which has none).
pub fn open_privacy_settings() -> bool {
    platform::open_privacy_settings()
}

/// A device's kind from its name, where the OS gives nothing else (macOS and Windows).
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn kind_of_name(name: &str) -> Kind {
    let name = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if has(&["airpods", "headset", "headphone", "hands-free", "bluetooth", "buds"]) {
        Kind::Headset
    } else if has(&["usb", "wave:", "yeti", "rode", "shure", "blue "]) {
        Kind::Usb
    } else if has(&["built-in", "macbook", "internal", "array", "realtek"]) {
        Kind::BuiltIn
    } else {
        Kind::Other
    }
}

/// `pactl --format=json list sources`'s microphones: every source but the outputs' monitors, in its order.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_sources(json: &str) -> Vec<Microphone> {
    let Ok(serde_json::Value::Array(sources)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    sources
        .iter()
        .filter_map(|source| {
            let id = source.get("name")?.as_str()?;
            let properties = source.get("properties");
            let property = |key: &str| {
                properties
                    .and_then(|p| p.get(key))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            };
            if id.ends_with(".monitor") || property("device.class") == "monitor" {
                return None;
            }
            let name = source
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(id);
            let kind = match (property("device.form_factor"), property("device.bus")) {
                ("headset" | "headphone" | "hands-free", _) | (_, "bluetooth") => Kind::Headset,
                ("internal", _) | (_, "pci") => Kind::BuiltIn,
                (_, "usb") => Kind::Usb,
                _ => Kind::Other,
            };
            Some(Microphone {
                id: id.to_string(),
                name: name.to_string(),
                kind,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
mod platform {
    use std::process::{Command, Stdio};

    use super::{Microphones, parse_sources};

    fn pactl(args: &[&str]) -> Option<String> {
        let output = Command::new("pactl")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn list() -> Microphones {
        let list = pactl(&["--format=json", "list", "sources"])
            .map(|json| parse_sources(&json))
            .unwrap_or_default();
        let default = pactl(&["get-default-source"]).and_then(|id| {
            let id = id.trim();
            list.iter()
                .find(|microphone| microphone.id == id)
                .map(|microphone| microphone.name.clone())
        });
        Microphones { default, list }
    }

    /// The desktop's own sound page first, then the mixers most systems have.
    pub fn open_sound_settings() -> bool {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        let mut tries: Vec<(&str, &[&str])> = Vec::new();
        if desktop.contains("kde") {
            tries.push(("systemsettings", &["kcm_pulseaudio"]));
        }
        if desktop.contains("gnome") || desktop.contains("unity") || desktop.contains("cinnamon") {
            tries.push(("gnome-control-center", &["sound"]));
        }
        tries.extend([
            ("pavucontrol", &["--tab=4"][..]),
            ("systemsettings", &["kcm_pulseaudio"][..]),
            ("gnome-control-center", &["sound"][..]),
        ]);
        tries.into_iter().any(|(program, args)| {
            Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
        })
    }

    pub fn open_privacy_settings() -> bool {
        open_sound_settings()
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use cpal::traits::{DeviceTrait as _, HostTrait as _};

    use super::{Microphone, Microphones, kind_of_name};

    pub fn list() -> Microphones {
        let host = cpal::default_host();
        let default = host.default_input_device().and_then(|device| device.name().ok());
        let mut list: Vec<Microphone> = Vec::new();
        for device in host.input_devices().into_iter().flatten() {
            let Ok(name) = device.name() else { continue };
            if list.iter().any(|microphone| microphone.id == name) {
                continue;
            }
            list.push(Microphone {
                kind: kind_of_name(&name),
                id: name.clone(),
                name,
            });
        }
        Microphones { default, list }
    }

    fn open(url: &str) -> bool {
        #[cfg(target_os = "macos")]
        let opened = std::process::Command::new("open").arg(url).spawn();
        #[cfg(windows)]
        let opened = std::process::Command::new("explorer").arg(url).spawn();
        #[cfg(not(any(target_os = "macos", windows)))]
        let opened: std::io::Result<()> = Err(std::io::ErrorKind::Unsupported.into());
        opened.is_ok()
    }

    pub fn open_sound_settings() -> bool {
        if cfg!(target_os = "macos") {
            open("x-apple.systempreferences:com.apple.preference.sound?input")
        } else {
            open("ms-settings:sound")
        }
    }

    pub fn open_privacy_settings() -> bool {
        if cfg!(target_os = "macos") {
            open("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        } else {
            open("ms-settings:privacy-microphone")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pactl_sources_drop_monitors_and_read_kinds() {
        let json = r#"[
          {"name":"alsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink.monitor","description":"Monitor of Speaker",
           "properties":{"device.class":"monitor","device.bus":"pci"}},
          {"name":"alsa_input.pci-0000_00_1f.3.HiFi__Mic1__source","description":"HD Audio Digital Microphone",
           "properties":{"device.class":"sound","device.bus":"pci"}},
          {"name":"alsa_input.usb-Generic_AB13X-00.analog-stereo","description":"AB13X USB Audio Analog Stereo",
           "properties":{"device.class":"sound","device.bus":"usb"}},
          {"name":"bluez_input.AA:BB:CC:DD:EE:FF","description":"AirPods Pro",
           "properties":{"device.bus":"bluetooth"}},
          {"name":"virtual_source","description":"","properties":{}}
        ]"#;
        let list = parse_sources(json);
        let seen: Vec<(&str, Kind)> = list.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(
            seen,
            vec![
                ("HD Audio Digital Microphone", Kind::BuiltIn),
                ("AB13X USB Audio Analog Stereo", Kind::Usb),
                ("AirPods Pro", Kind::Headset),
                ("virtual_source", Kind::Other),
            ]
        );
        assert!(parse_sources("not json").is_empty());
    }

    #[test]
    fn device_names_read_as_kinds() {
        assert_eq!(kind_of_name("AirPods Pro"), Kind::Headset);
        assert_eq!(kind_of_name("Elgato Wave:3"), Kind::Usb);
        assert_eq!(kind_of_name("MacBook Pro Microphone"), Kind::BuiltIn);
        assert_eq!(kind_of_name("Studio Display"), Kind::Other);
    }
}
