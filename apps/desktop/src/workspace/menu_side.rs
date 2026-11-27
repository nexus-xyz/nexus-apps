//! Which side of its chip a where-line menu opens on (the project, where it runs and the workspace).
//!
//! A menu opens under its chip when it fits there. Otherwise it opens on the side with more room, and its height is
//! capped to that room, so it never covers the chips. `anchored`'s own snapping would move it up over them.
//!
//! The chip's place and the menu's height are read from the frame before, so the first frame of a menu opens below.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::new_code::LaunchMenu;

/// The space between a chip and its menu.
pub(super) const GAP: f32 = 8.;
/// The space a menu keeps from the window's edge.
pub(super) const MARGIN: f32 = 12.;
/// A capped menu is never shorter than this, even in a tiny window.
const MIN_HEIGHT: f32 = 120.;

/// Where a menu opens, and the most height it may take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Side {
    pub above: bool,
    pub max_height: Option<f32>,
}

impl Side {
    /// Under the chip, uncapped: the first frame, before anything is measured.
    pub const BELOW: Side = Side {
        above: false,
        max_height: None,
    };
}

/// What the last frame measured.
#[derive(Debug, Default, Clone, Copy)]
struct Seen {
    /// The open chip's top and bottom, and the window's height.
    chip: Option<(f32, f32, f32)>,
    /// The menu's full height, or a height it is known not to fit in (it was capped there).
    menu: Option<f32>,
    /// The side picked once the menu was measured. It holds while the menu is open, so a search that shortens the
    /// list does not move the menu across its chip.
    above: Option<bool>,
    /// The menu open at the last render.
    open: Option<LaunchMenu>,
}

/// The open menu's measurements. One menu is open at a time; each opening forgets the last one's height and side.
#[derive(Clone, Default)]
pub(super) struct MenuSide(Rc<Cell<Seen>>);

impl MenuSide {
    /// Call on each render with the open menu. An opening forgets the last menu's height and side; a closing keeps
    /// them, so the menu fades out where it was.
    pub fn track(&self, open: Option<LaunchMenu>) {
        let seen = self.0.get();
        if seen.open == open {
            return;
        }
        self.0.set(match open {
            Some(_) => Seen {
                menu: None,
                above: None,
                open,
                ..seen
            },
            None => Seen { open, ..seen },
        });
    }

    /// The side the open menu takes now.
    pub fn side(&self) -> Side {
        let mut seen = self.0.get();
        let Some((top, bottom, window)) = seen.chip else {
            return Side::BELOW;
        };
        let side = match seen.above {
            Some(above) => capped(top, bottom, window, above),
            None => pick(top, bottom, window, seen.menu),
        };
        if seen.above.is_none() && seen.menu.is_some() {
            seen.above = Some(side.above);
            self.0.set(seen);
        }
        side
    }

    /// Measures the open chip. Put it in the chip's frame (`relative`, the chip's size).
    pub fn measure_chip(&self) -> impl IntoElement {
        let seen = self.0.clone();
        div().absolute().inset_0().child(
            canvas(
                move |bounds, window, _| {
                    let chip = Some((
                        f32::from(bounds.top()),
                        f32::from(bounds.bottom()),
                        f32::from(window.viewport_size().height),
                    ));
                    let last = seen.get();
                    if last.chip != chip {
                        seen.set(Seen { chip, ..last });
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
    }

    /// The menu in a column capped to `side`, measured as it is drawn. A menu box scrolls (`menu_box`), so the cap
    /// shrinks it, and a list that scrolls inside it shrinks first.
    pub fn fit(&self, side: Side, menu: AnyElement) -> impl IntoElement {
        let seen = self.0.clone();
        let cap = side.max_height;
        v_flex()
            .relative()
            .when_some(cap, |column, cap| column.max_h(px(cap)))
            .child(menu)
            .child(
                div().absolute().inset_0().child(
                    canvas(
                        move |bounds, window, _| {
                            let height = f32::from(bounds.size.height);
                            let last = seen.get();
                            let menu = Some(learn(last.menu, height, cap));
                            if last.menu != menu {
                                seen.set(Seen { menu, ..last });
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                ),
            )
    }
}

/// What a drawn menu says about its full height. Under its cap, `height` is the full height. At its cap, the menu
/// does not fit there: remember a height just over the cap, unless a taller one is already known.
fn learn(known: Option<f32>, height: f32, cap: Option<f32>) -> f32 {
    match cap {
        Some(cap) if height >= cap - 0.5 => known.unwrap_or(0.).max(cap + 1.),
        _ => height,
    }
}

/// Under the chip when the menu fits there or there is no more room above; else above. Capped to the room it has.
fn pick(chip_top: f32, chip_bottom: f32, window: f32, menu: Option<f32>) -> Side {
    let below = window - chip_bottom - GAP - MARGIN;
    let above = chip_top - GAP - MARGIN;
    let fits_below = menu.is_none_or(|height| height <= below);
    capped(chip_top, chip_bottom, window, !(fits_below || below >= above))
}

/// One side of the chip, capped to the room there.
fn capped(chip_top: f32, chip_bottom: f32, window: f32, above: bool) -> Side {
    let room = if above {
        chip_top - GAP - MARGIN
    } else {
        window - chip_bottom - GAP - MARGIN
    };
    Side {
        above,
        max_height: Some(room.max(MIN_HEIGHT)),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` macro in over the standard one.
    use super::{GAP, MARGIN, Side, learn, pick};

    #[test]
    fn opens_below_when_it_fits() {
        assert_eq!(
            pick(400., 428., 1000., Some(460.)),
            Side {
                above: false,
                max_height: Some(1000. - 428. - GAP - MARGIN)
            }
        );
    }

    #[test]
    fn opens_above_when_only_the_top_has_room() {
        // The screenshot's case: a 460px menu under a chip 30px from the window's bottom.
        let side = pick(500., 528., 560., Some(460.));
        assert!(side.above);
        assert_eq!(side.max_height, Some(500. - GAP - MARGIN));
    }

    #[test]
    fn takes_the_roomier_side_capped_when_neither_fits() {
        let side = pick(300., 328., 600., Some(460.));
        assert!(side.above);
        assert_eq!(side.max_height, Some(300. - GAP - MARGIN));
        let side = pick(200., 228., 600., Some(460.));
        assert!(!side.above);
        assert_eq!(side.max_height, Some(600. - 228. - GAP - MARGIN));
    }

    #[test]
    fn an_unmeasured_menu_opens_below() {
        assert!(!pick(500., 528., 560., None).above);
    }

    #[test]
    fn a_capped_menu_is_known_not_to_fit() {
        assert_eq!(learn(None, 200., Some(200.)), 201.);
        assert_eq!(learn(Some(460.), 200., Some(200.)), 460.);
        assert_eq!(learn(Some(460.), 150., Some(200.)), 150.);
        assert_eq!(learn(None, 150., None), 150.);
    }
}
