//! The page's width, as Cursor's browser: Desktop fits the pane, and Phone draws the page at a phone's width,
//! centred, and never wider than the pane (`preview.css`). There is no tablet width: two choices cover the checks
//! people make, and a third button only crowds the bar.

use gpui_kit::assets::IconName as Lucide;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Width {
    #[default]
    Fit,
    Phone,
}

impl Width {
    /// The choices, in the bar's order.
    pub const ALL: [Width; 2] = [Width::Fit, Width::Phone];

    /// The button's name, as the Nexus app's.
    pub fn label(self) -> &'static str {
        match self {
            Width::Fit => "Desktop: fit the pane",
            Width::Phone => "Phone width",
        }
    }

    pub fn icon(self) -> Lucide {
        match self {
            Width::Fit => Lucide::Monitor,
            Width::Phone => Lucide::Smartphone,
        }
    }

    /// The page's width in pixels, or `None` to fill the pane.
    pub fn pixels(self) -> Option<f32> {
        match self {
            Width::Fit => None,
            Width::Phone => Some(375.),
        }
    }

    /// The id of its button in the bar.
    pub fn id(self) -> &'static str {
        match self {
            Width::Fit => "browser-width-fit",
            Width::Phone => "browser-width-phone",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_widths_are_desktop_and_phone() {
        assert_eq!(Width::default(), Width::Fit);
        assert_eq!(Width::Fit.pixels(), None);
        // `preview.css`: `.wb-native-surface[data-device=phone]`.
        assert_eq!(Width::Phone.pixels(), Some(375.));
        let labels: Vec<_> = Width::ALL.iter().map(|width| width.label()).collect();
        assert_eq!(labels, ["Desktop: fit the pane", "Phone width"]);
    }
}
