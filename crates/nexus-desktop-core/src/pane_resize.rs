//! Workbench pane width rules.

pub const MIN: f32 = 360.;
pub const DEFAULT: f32 = 440.;
const CONVERSATION_MIN: f32 = 360.;

pub fn maximum(available: f32, window: f32) -> f32 {
    (available - CONVERSATION_MIN).min(window * 0.6).floor().max(MIN)
}

pub fn clamp(width: f32, maximum: f32) -> f32 {
    let width = if width.is_finite() { width } else { DEFAULT };
    width.round().max(MIN).min(maximum.max(MIN))
}

/// The left edge moves left to widen the pane, and right to narrow it.
pub fn key_width(width: f32, maximum: f32, key: &str) -> Option<f32> {
    let width = clamp(width, maximum);
    let next = match key {
        "left" => width + 16.,
        "right" => width - 16.,
        "home" => MIN,
        "end" => maximum,
        _ => return None,
    };
    Some(clamp(next, maximum))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_and_respects_available_workspace_width() {
        assert_eq!(clamp(10., f32::INFINITY), MIN);
        assert_eq!(clamp(400.6, f32::INFINITY), 401.);
        assert_eq!(clamp(1000., maximum(1200., f32::INFINITY)), 840.);
        assert_eq!(clamp(1000., maximum(1800., f32::INFINITY)), 1000.);
        assert_eq!(maximum(500., f32::INFINITY), MIN);
        assert_eq!(maximum(1500., 1500.), 900.);
    }

    #[test]
    fn keys_step_in_the_drag_direction_and_stop_at_the_limits() {
        assert_eq!(key_width(DEFAULT, 840., "left"), Some(456.));
        assert_eq!(key_width(DEFAULT, 840., "right"), Some(424.));
        assert_eq!(key_width(DEFAULT, 840., "home"), Some(MIN));
        assert_eq!(key_width(DEFAULT, 840., "end"), Some(840.));
        assert_eq!(key_width(MIN, 840., "right"), Some(MIN));
        assert_eq!(key_width(840., 840., "left"), Some(840.));
        assert_eq!(key_width(DEFAULT, 840., "enter"), None);
        assert_eq!(clamp(f32::NAN, 840.), DEFAULT);
    }
}
