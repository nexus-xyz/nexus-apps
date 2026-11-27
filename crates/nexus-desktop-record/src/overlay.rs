//! The ink overlay: a clear, borderless window over the whole recorded display, above every other window and on every
//! Space, that never takes the mouse (clicks go through to the app under it). It draws what src/gesture.rs
//! decides: red strokes (#ff3b30 on a thin white edge, so they show on dark and light screens) that fade a few seconds
//! after Shift comes up, and numbered pins. `screencapture` records it with the screen, so the ink is in the video and
//! in every keyframe. Core Animation layers, no webview. Main thread only (the callers make sure); macOS only.
//!
//! Unsafe: making the window (AppKit's initialiser and `setReleasedWhenClosed`), reading AppKit's and Core
//! Animation's exported constants, and building Core Graphics paths with no transform.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreenSaverWindowLevel, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath, CGPath};
use objc2_foundation::NSString;
use objc2_quartz_core::{
    CALayer, CAShapeLayer, CATextLayer, CATransaction, kCAAlignmentCenter, kCALineCapRound, kCALineJoinRound,
};

use super::gesture::Draw;
use super::model::Point;
use super::screen::Screen;

const RED: (CGFloat, CGFloat, CGFloat) = (1.0, 59.0 / 255.0, 48.0 / 255.0);
const WIDTH: CGFloat = 4.0;
const EDGE: CGFloat = 2.0;
const PIN: CGFloat = 26.0;

struct Stroke {
    path: CFRetained<CGMutablePath>,
    layers: [Retained<CAShapeLayer>; 2],
}

struct Overlay {
    window: Retained<NSWindow>,
    root: Retained<CALayer>,
    height: CGFloat,
    scale: CGFloat,
    strokes: HashMap<u32, Stroke>,
    pins: HashMap<u32, Retained<CALayer>>,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

fn color(rgb: (CGFloat, CGFloat, CGFloat), alpha: CGFloat) -> CFRetained<CGColor> {
    CGColor::new_srgb(rgb.0, rgb.1, rgb.2, alpha)
}

/// Without Core Animation's implicit animations: ink follows the pointer at once.
fn instantly(change: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    change();
    CATransaction::commit();
}

/// Show the overlay over `screen`.
pub fn show(screen: Screen) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    hide();
    let bounds = screen.bounds;
    // AppKit counts from the main display's bottom-left corner, y up.
    let frame = CGRect::new(
        CGPoint::new(
            bounds.origin.x,
            screen.main_height - bounds.origin.y - bounds.size.height,
        ),
        bounds.size,
    );
    // SAFETY: AppKit's initialiser with a borderless style and buffered backing; the window is ours (not released on
    // close, so `Retained` owns it).
    let window = unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(false);
    window.setIgnoresMouseEvents(true);
    window.setLevel(NSScreenSaverWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    let view = NSView::initWithFrame(NSView::alloc(mtm), CGRect::new(CGPoint::new(0.0, 0.0), bounds.size));
    view.setWantsLayer(true);
    window.setContentView(Some(&view));
    let Some(root) = view.layer() else {
        return;
    };
    let scale = window.backingScaleFactor();
    window.orderFrontRegardless();
    OVERLAY.with(|overlay| {
        *overlay.borrow_mut() = Some(Overlay {
            window,
            root,
            height: bounds.size.height,
            scale,
            strokes: HashMap::new(),
            pins: HashMap::new(),
        });
    });
}

pub fn hide() {
    OVERLAY.with(|overlay| {
        if let Some(overlay) = overlay.borrow_mut().take() {
            overlay.window.orderOut(None);
            overlay.window.close();
        }
    });
}

impl Overlay {
    /// The display's points (y down) to the layer's (y up).
    fn layer_point(&self, at: Point) -> CGPoint {
        CGPoint::new(at[0], self.height - at[1])
    }

    fn stroke_layer(&self, rgb: (CGFloat, CGFloat, CGFloat), alpha: CGFloat, width: CGFloat) -> Retained<CAShapeLayer> {
        let layer = CAShapeLayer::new();
        layer.setFillColor(None);
        layer.setStrokeColor(Some(&color(rgb, alpha)));
        layer.setLineWidth(width);
        // SAFETY: constants Core Animation exports.
        unsafe {
            layer.setLineCap(kCALineCapRound);
            layer.setLineJoin(kCALineJoinRound);
        }
        layer.setContentsScale(self.scale);
        self.root.addSublayer(&layer);
        layer
    }

    fn apply(&mut self, draw: Draw) {
        match draw {
            Draw::Begin { id, at } => {
                let path = CGMutablePath::new();
                let point = self.layer_point(at);
                // SAFETY: a path made here, no transform.
                unsafe { CGMutablePath::move_to_point(Some(&path), std::ptr::null(), point.x, point.y) };
                let edge = self.stroke_layer((1.0, 1.0, 1.0), 0.9, WIDTH + EDGE * 2.0);
                let ink = self.stroke_layer(RED, 1.0, WIDTH);
                self.strokes.insert(
                    id,
                    Stroke {
                        path,
                        layers: [edge, ink],
                    },
                );
            }
            Draw::Extend { id, at } => {
                let point = self.layer_point(at);
                let Some(stroke) = self.strokes.get(&id) else {
                    return;
                };
                // SAFETY: as above.
                unsafe { CGMutablePath::add_line_to_point(Some(&stroke.path), std::ptr::null(), point.x, point.y) };
                // The layers keep a copy: a path changed after it is set would not be redrawn.
                let copy = CGPath::new_copy(Some(&stroke.path));
                for layer in &stroke.layers {
                    layer.setPath(copy.as_deref());
                }
            }
            Draw::End { .. } => {}
            Draw::Discard { id } => {
                if let Some(stroke) = self.strokes.remove(&id) {
                    for layer in &stroke.layers {
                        layer.removeFromSuperlayer();
                    }
                }
            }
            Draw::Pin { n, at } => {
                let center = self.layer_point(at);
                let pin = CALayer::new();
                pin.setFrame(CGRect::new(
                    CGPoint::new(center.x - PIN / 2.0, center.y - PIN / 2.0),
                    CGSize::new(PIN, PIN),
                ));
                pin.setCornerRadius(PIN / 2.0);
                pin.setBackgroundColor(Some(&color(RED, 1.0)));
                pin.setBorderColor(Some(&color((1.0, 1.0, 1.0), 1.0)));
                pin.setBorderWidth(EDGE);
                pin.setContentsScale(self.scale);
                let label = CATextLayer::new();
                label.setFrame(CGRect::new(CGPoint::new(0.0, 4.0), CGSize::new(PIN, PIN - 8.0)));
                label.setFontSize(14.0);
                label.setForegroundColor(Some(&color((1.0, 1.0, 1.0), 1.0)));
                label.setContentsScale(self.scale);
                // SAFETY: an NSString is a valid `string` for a text layer; the alignment is Core Animation's constant.
                unsafe {
                    label.setString(Some(&NSString::from_str(&n.to_string())));
                    label.setAlignmentMode(kCAAlignmentCenter);
                }
                pin.addSublayer(&label);
                self.root.addSublayer(&pin);
                self.pins.insert(n, pin);
            }
        }
    }

    /// Fade a stroke or a pin out (Core Animation's implicit opacity animation), then remove it.
    fn fade(&mut self, target: Faded) {
        let layers: Vec<Retained<CALayer>> = match target {
            Faded::Stroke(id) => self
                .strokes
                .get(&id)
                .map(|stroke| {
                    stroke
                        .layers
                        .iter()
                        .map(|layer| Retained::into_super(layer.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            Faded::Pin(n) => self.pins.get(&n).cloned().into_iter().collect(),
        };
        CATransaction::begin();
        CATransaction::setAnimationDuration(0.6);
        for layer in &layers {
            layer.setOpacity(0.0);
        }
        CATransaction::commit();
    }

    fn remove(&mut self, target: Faded) {
        match target {
            Faded::Stroke(id) => {
                if let Some(stroke) = self.strokes.remove(&id) {
                    for layer in &stroke.layers {
                        layer.removeFromSuperlayer();
                    }
                }
            }
            Faded::Pin(n) => {
                if let Some(pin) = self.pins.remove(&n) {
                    pin.removeFromSuperlayer();
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Faded {
    Stroke(u32),
    Pin(u32),
}

pub fn apply(draws: Vec<Draw>) {
    OVERLAY.with(|overlay| {
        if let Some(overlay) = overlay.borrow_mut().as_mut() {
            instantly(|| {
                for draw in draws {
                    overlay.apply(draw);
                }
            });
        }
    });
}

pub fn fade(target: Faded) {
    OVERLAY.with(|overlay| {
        if let Some(overlay) = overlay.borrow_mut().as_mut() {
            overlay.fade(target);
        }
    });
}

pub fn remove(target: Faded) {
    OVERLAY.with(|overlay| {
        if let Some(overlay) = overlay.borrow_mut().as_mut() {
            instantly(|| overlay.remove(target));
        }
    });
}
