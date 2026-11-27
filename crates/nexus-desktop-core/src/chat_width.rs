//! How wide the conversation and the composer are: a share of the room beside the sidebar and the side
//! pane, held between a least and a most line length in characters. Nexus GPUI calls
//! [`widths`].
//!
//! The default is [`Preset::Standard`], a fixed 720 px lane (45rem). It does not grow with
//! the window. This is a product design decision: do not change the default without a design
//! review. The other presets grow from Standard's lane, for people who
//! choose them.
//!
//! ```text
//! room     = window − sidebar − open side pane
//! column   = clamp(720, share × room, max_ch × ch), and never more than room − 48 (the 24 px gutters)
//! wide     = min(room − 48, 1.5 × column)      code blocks and tables may grow this wide
//! composer = max(column, min(560, room − 48))  a floor so its chips never wrap
//! ```
//!
//! Standard is a lane instead: column = composer = wide = min(720, room − 48).
//!
//! `ch` is [`CH_PX`] in both apps, so a choice draws at the same pixels in each. Every length is in logical pixels:
//! both apps zoom the whole window, so the room is the window's width divided by the zoom. Custom's least is
//! [`MIN_CH`], so it can still draw the earlier fixed 680 px column.

use serde::{Deserialize, Serialize};

/// The advance of "0" in DM Sans at 14 px, when the app cannot measure it.
pub const CH_PX: f32 = 7.7;
/// The gutters either side of the column, together.
pub const GUTTERS: f32 = 48.;
/// The composer's least width, so its chips stay on one line.
pub const COMPOSER_FLOOR: f32 = 560.;
/// How much wider than the column a code block or a table may grow.
pub const WIDE_FACTOR: f32 = 1.5;
/// Custom's least line length: the earlier fixed 680 px column.
pub const MIN_CH: f32 = 88.;
/// Standard's lane: 720 px (45rem), the home composer's width; T3 Code's app draws 736 px, Claude's
/// 768 px. In pixels, not characters, so
/// both apps draw it the same whatever the font measures.
pub const STANDARD_PX: f32 = 720.;
/// Custom's share of the room, least and most.
pub const CUSTOM_SHARE: (f32, f32) = (0.5, 1.);
/// Custom's most characters a line, least and most.
pub const CUSTOM_MAX_CH: (f32, f32) = (MIN_CH, 240.);
/// What Custom starts from.
pub const CUSTOM_DEFAULT: (f32, f32) = (0.8, 140.);

/// The presets in Settings › Appearance, in their order there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    /// A fixed 720 px lane, with code blocks in it: the default (a design decision). It took the place of Narrow,
    /// a fixed 680 px column, so a saved Narrow reads as Standard.
    #[default]
    #[serde(alias = "narrow")]
    Standard,
    /// 85% of the room, up to 160 characters. It took the place of Comfortable, so a saved Comfortable reads as Wide.
    #[serde(alias = "comfortable")]
    Wide,
    /// The whole room, less the gutters.
    FitWindow,
    /// The person's share and most characters.
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Standard, Preset::Wide, Preset::FitWindow, Preset::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Standard => "Standard",
            Preset::Wide => "Wide",
            Preset::FitWindow => "Fit window",
            Preset::Custom => "Custom",
        }
    }
}

/// `settings.json`'s `chatWidth`: `{"preset": "standard"}`, or `{"preset": "custom", "share": 0.8, "maxCh": 140}`.
/// `share` and `maxCh` count only for Custom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatWidth {
    #[serde(default)]
    pub preset: Preset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ch: Option<f32>,
}

/// A preset as numbers: the share of the room, and the least and most characters a line (None: no most). A `lane`
/// in pixels overrides them all: the column, the composer and wide blocks are that wide, or the room less its gutters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub share: f32,
    pub min_ch: f32,
    pub max_ch: Option<f32>,
    pub lane: Option<f32>,
}

impl ChatWidth {
    pub fn preset(preset: Preset) -> ChatWidth {
        ChatWidth {
            preset,
            ..ChatWidth::default()
        }
    }

    /// Custom with this share and most, each kept inside its range.
    pub fn custom(share: f32, max_ch: f32) -> ChatWidth {
        ChatWidth {
            preset: Preset::Custom,
            share: Some(within(share, CUSTOM_SHARE, CUSTOM_DEFAULT.0)),
            max_ch: Some(within(max_ch, CUSTOM_MAX_CH, CUSTOM_DEFAULT.1).round()),
        }
    }

    /// The choice as it is saved: Custom's numbers inside their ranges, and none for a preset.
    pub fn normalized(self) -> ChatWidth {
        match self.preset {
            Preset::Custom => {
                let (share, max_ch) = self.custom_values();
                ChatWidth::custom(share, max_ch)
            }
            preset => ChatWidth::preset(preset),
        }
    }

    /// Custom's share and most, as saved or its defaults, each inside its range.
    pub fn custom_values(&self) -> (f32, f32) {
        (
            within(self.share.unwrap_or(CUSTOM_DEFAULT.0), CUSTOM_SHARE, CUSTOM_DEFAULT.0),
            within(self.max_ch.unwrap_or(CUSTOM_DEFAULT.1), CUSTOM_MAX_CH, CUSTOM_DEFAULT.1),
        )
    }

    pub fn shape(&self) -> Shape {
        // The presets that grow start from Standard's lane, so none is narrower than the default.
        let grows = |share, max_ch| Shape {
            share,
            min_ch: STANDARD_PX / CH_PX,
            max_ch,
            lane: None,
        };
        match self.preset {
            Preset::Standard => Shape {
                lane: Some(STANDARD_PX),
                ..grows(1., None)
            },
            Preset::Wide => grows(0.85, Some(160.)),
            Preset::FitWindow => grows(1., None),
            Preset::Custom => {
                let (share, max_ch) = self.custom_values();
                Shape {
                    min_ch: MIN_CH,
                    ..grows(share, Some(max_ch))
                }
            }
        }
    }
}

/// `value` inside `(least, most)`; `fallback` when it is not a number.
fn within(value: f32, (least, most): (f32, f32), fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(least, most)
    } else {
        fallback
    }
}

/// The widths in a room, in logical pixels, without the gutters. The default is no room at all.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Widths {
    /// The conversation's text.
    pub column: f32,
    /// What code blocks and tables may grow to, centred on the column.
    pub wide: f32,
    pub composer: f32,
}

impl Widths {
    /// How far a wide block reaches past the column on each side.
    pub fn bleed(&self) -> f32 {
        ((self.wide - self.column) / 2.).max(0.)
    }
}

/// The widths in a room `room` px wide, with `ch` px per character (the module's formula).
pub fn widths(shape: Shape, room: f32, ch: f32) -> Widths {
    let ch = if ch.is_finite() && ch > 0. { ch } else { CH_PX };
    let fits = (room - GUTTERS).max(0.);
    if let Some(lane) = shape.lane {
        let column = lane.min(fits);
        return Widths {
            column,
            wide: column,
            composer: column,
        };
    }
    let least = shape.min_ch * ch;
    let most = shape.max_ch.map_or(f32::INFINITY, |max_ch| max_ch * ch).max(least);
    let column = (shape.share * room).clamp(least, most).min(fits);
    Widths {
        column,
        wide: fits.min(WIDE_FACTOR * column),
        composer: column.max(COMPOSER_FLOOR.min(fits)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDEBAR: f32 = 260.;
    const PANE: f32 = 480.;

    /// The column in a window `window` px wide at `zoom`, with or without the side pane.
    fn column(choice: ChatWidth, window: f32, zoom: f32, pane: bool) -> f32 {
        let room = window / zoom - SIDEBAR - if pane { PANE } else { 0. };
        widths(choice.shape(), room, CH_PX).column
    }

    fn near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1., "{actual} is not about {expected}");
    }

    /// A design decision: by default the conversation is a fixed 720 px lane, and code blocks stay in it.
    /// Change it only after a design review.
    #[test]
    fn the_default_is_a_fixed_720_px_lane_as_the_board_draws_it() {
        let standard = ChatWidth::default();
        assert_eq!(standard.preset, Preset::Standard);
        for (window, zoom) in [(1440., 1.), (1920., 1.), (2560., 1.), (2560., 1.5), (3840., 1.)] {
            // Whatever a character measures: the GPUI app measures DM Sans's "0" wider than CH_PX.
            for ch in [CH_PX, 9.2] {
                let lane = widths(standard.shape(), window / zoom - SIDEBAR, ch);
                assert_eq!((lane.column, lane.wide, lane.composer), (720., 720., 720.));
            }
        }
        // With little room, the room less its gutters, as every preset.
        near(column(standard, 1440., 1., true), 652.);
    }

    #[test]
    fn a_saved_narrow_reads_as_standard_and_a_saved_comfortable_as_wide() {
        let read = |json: &str| serde_json::from_str::<ChatWidth>(json).expect("reads");
        assert_eq!(read(r#"{"preset":"narrow"}"#), ChatWidth::default());
        assert_eq!(read(r#"{"preset":"comfortable"}"#).preset, Preset::Wide);
        assert_eq!(Preset::ALL[0], Preset::Standard);
    }

    #[test]
    fn wide_and_fit_window_grow_further() {
        let wide = ChatWidth::preset(Preset::Wide);
        near(column(wide, 1440., 1., false), 1003.);
        near(column(wide, 2560., 1., false), 1232.);
        // 840 px of room: 85% falls under Standard's lane, so the lane.
        near(column(wide, 1100., 1., false), 720.);
        let fit = ChatWidth::preset(Preset::FitWindow);
        near(column(fit, 1440., 1., false), 1132.);
        near(column(fit, 2560., 1., false), 2252.);
        near(column(fit, 2560., 1.5, true), 919.);
    }

    #[test]
    fn custom_keeps_its_numbers_inside_their_ranges() {
        let custom = ChatWidth::custom(0.9, 140.);
        near(column(custom, 1440., 1., false), 1062.);
        near(column(custom, 2560., 1., false), 1078.);
        assert_eq!(ChatWidth::custom(3., 9999.).custom_values(), (1., 240.));
        assert_eq!(ChatWidth::custom(f32::NAN, 10.).custom_values(), (0.8, 88.));
        // A hand-edited file out of range is read inside it.
        let edited = ChatWidth {
            preset: Preset::Custom,
            share: Some(0.1),
            max_ch: None,
        };
        assert_eq!(edited.shape().share, 0.5);
        assert_eq!(edited.shape().max_ch, Some(140.));
        assert_eq!(edited.normalized(), ChatWidth::custom(0.5, 140.));
        // A preset keeps no numbers.
        let wide = ChatWidth {
            preset: Preset::Wide,
            share: Some(0.9),
            max_ch: Some(100.),
        };
        assert_eq!(wide.normalized(), ChatWidth::preset(Preset::Wide));
    }

    #[test]
    fn wide_blocks_and_the_composer_follow_the_column() {
        let wide = ChatWidth::preset(Preset::Wide).shape();
        // Plenty of room: one and a half columns.
        let roomy = widths(wide, 2300., CH_PX);
        near(roomy.column, 1232.);
        near(roomy.wide, 1848.);
        near(roomy.bleed(), 308.);
        assert_eq!(roomy.composer, roomy.column);
        // Little room: the room less its gutters, for all three.
        let tight = widths(wide, 700., CH_PX);
        assert_eq!((tight.column, tight.wide, tight.composer), (652., 652., 652.));
        // No room at all: nothing negative.
        let none = widths(wide, 20., CH_PX);
        assert_eq!((none.column, none.wide, none.composer), (0., 0., 0.));
        // A composer floor above a narrow column: a share small enough to drop under 560 px.
        let small = Shape {
            share: 0.3,
            min_ch: 40.,
            max_ch: Some(60.),
            lane: None,
        };
        let floored = widths(small, 1000., CH_PX);
        near(floored.column, 308.);
        assert_eq!(floored.composer, COMPOSER_FLOOR);
    }

    #[test]
    fn measures_with_the_given_character_and_falls_back_when_it_is_not_one() {
        let wide = ChatWidth::preset(Preset::Wide).shape();
        near(widths(wide, 2300., 8.).column, 1280.);
        assert_eq!(widths(wide, 2300., 0.), widths(wide, 2300., CH_PX));
        assert_eq!(widths(wide, 2300., f32::NAN), widths(wide, 2300., CH_PX));
    }

    #[test]
    fn reads_and_writes_the_settings_shape() {
        let read = |json: &str| serde_json::from_str::<ChatWidth>(json).expect("reads");
        assert_eq!(read(r#"{"preset":"standard"}"#), ChatWidth::default());
        assert_eq!(read("{}"), ChatWidth::default());
        assert_eq!(read(r#"{"preset":"fitWindow"}"#).preset, Preset::FitWindow);
        assert_eq!(
            read(r#"{"preset":"custom","share":0.8,"maxCh":140}"#),
            ChatWidth::custom(0.8, 140.)
        );
        let written = serde_json::to_string(&ChatWidth::preset(Preset::Wide)).expect("writes");
        assert_eq!(written, r#"{"preset":"wide"}"#);
        let written = serde_json::to_string(&ChatWidth::custom(0.8, 140.)).expect("writes");
        assert_eq!(written, r#"{"preset":"custom","share":0.8,"maxCh":140.0}"#);
    }
}
