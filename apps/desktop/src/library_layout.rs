//! The Library's layout, list or gallery, as Nexus UI's (`ui/src/components/nexus-ui/library/view.ts`). The
//! list is the default. The choice is this device's: `nexus-desktop-gpui/library-layout` in this build's data folder,
//! apart from the browser's. Also the gallery's column count and a card's color, as `gallery.css` and `presence.ts`.
//! Plain data, no UI, so the tests below check it directly.

use std::path::{Path, PathBuf};

/// Rows, or cards with a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    List,
    Gallery,
}

impl Layout {
    /// The switch's order, as the web's: Gallery, then List.
    pub const ALL: [Layout; 2] = [Layout::Gallery, Layout::List];

    pub fn label(self) -> &'static str {
        match self {
            Layout::List => "List",
            Layout::Gallery => "Gallery",
        }
    }

    /// The saved word, as the web's localStorage value.
    fn key(self) -> &'static str {
        match self {
            Layout::List => "list",
            Layout::Gallery => "gallery",
        }
    }

    /// A saved word; anything else is the default.
    fn parse(text: &str) -> Layout {
        match text.trim() {
            "gallery" => Layout::Gallery,
            _ => Layout::List,
        }
    }

    /// The saved layout; the list when none is saved or the file does not read.
    pub fn load() -> Layout {
        file().map(|file| load_at(&file)).unwrap_or_default()
    }

    /// Saves the layout. A failure keeps the choice on screen until the app closes.
    pub fn save(self) -> Result<(), String> {
        save_at(&file().ok_or("No data folder")?, self)
    }
}

fn file() -> Option<PathBuf> {
    crate::paths::own_data_dir().map(|dir| dir.join("library-layout"))
}

fn load_at(file: &Path) -> Layout {
    std::fs::read_to_string(file)
        .map(|text| Layout::parse(&text))
        .unwrap_or_default()
}

fn save_at(file: &Path, layout: Layout) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    nexus_desktop_core::atomic_file::write_atomic(file, layout.key().as_bytes()).map_err(|error| error.to_string())
}

/// A card's narrowest, as `.library-grid`'s `minmax(236px, 1fr)`.
pub const CARD_MIN: f32 = 236.;
/// The space between cards, as `.library-grid`'s gap.
pub const CARD_GAP: f32 = 12.;

/// How many cards fit in a line `width` wide, as `repeat(auto-fill, minmax(236px, 1fr))`: at least one.
pub fn columns(width: f32) -> usize {
    (((width + CARD_GAP) / (CARD_MIN + CARD_GAP)).floor() as usize).max(1)
}

/// The owner's color slot, as `hueOf` in `presence.ts`: one of `--nx-route-1`, 2, 4 and 5, stable for `key`.
pub fn hue_slot(key: &str) -> u8 {
    const HUES: [u8; 4] = [1, 2, 4, 5];
    // `for (const char of key)` with `charCodeAt(0)`: a code point's first UTF-16 unit.
    let hash = key.chars().fold(0u32, |hash, char| {
        let unit = char.encode_utf16(&mut [0; 2])[0];
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    });
    HUES[(hash % HUES.len() as u32) as usize]
}

#[cfg(test)]
mod tests {
    use super::{Layout, columns, hue_slot, load_at, save_at};

    #[test]
    fn the_list_is_the_default() {
        assert_eq!(Layout::default(), Layout::List);
        assert_eq!(Layout::parse("gallery\n"), Layout::Gallery);
        assert_eq!(Layout::parse("list"), Layout::List);
        assert_eq!(Layout::parse("grid"), Layout::List);
    }

    #[test]
    fn a_saved_layout_reads_back() {
        let dir = std::env::temp_dir().join(format!("library-layout-{}", std::process::id()));
        let file = dir.join("nested").join("library-layout");
        assert_eq!(load_at(&file), Layout::List);
        save_at(&file, Layout::Gallery).unwrap();
        assert_eq!(load_at(&file), Layout::Gallery);
        save_at(&file, Layout::List).unwrap();
        assert_eq!(load_at(&file), Layout::List);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn columns_fill_the_line_as_auto_fill() {
        assert_eq!(columns(0.), 1);
        assert_eq!(columns(236.), 1);
        assert_eq!(columns(483.), 1);
        assert_eq!(columns(484.), 2);
        assert_eq!(columns(979.), 3);
        // The widest column, 1080 less 48 on each side.
        assert_eq!(columns(984.), 4);
    }

    #[test]
    fn hues_match_the_web() {
        // hueOf("you") in presence.ts: hash 119839, 119839 % 4 = 3, slot 5.
        assert_eq!(hue_slot("you"), 5);
        assert_eq!(hue_slot(""), 1);
        assert_eq!(hue_slot("ada@example.com"), hue_slot("ada@example.com"));
        assert!([1, 2, 4, 5].contains(&hue_slot("🙂 someone")));
    }
}
