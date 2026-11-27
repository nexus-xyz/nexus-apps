//! The voice mark's motion, as plain functions: the web kit's `voice-motion.ts`, line for line. The mark
//! (`mark.rs`) draws with them on each frame, and the tests run them as they are. No twist: the layers never turn
//! against each other, and only Thinking turns the whole mark.

use std::f32::consts::PI;

use super::call::VoiceState;

/// The mark's eleven layers, rim (0) to eye (10).
pub const LAYERS: usize = 11;
/// The voice ring: 64 ticks on a ring 1.7 times the mark.
pub const TICKS: usize = 64;
/// The room the element keeps for the ring, as a share of the mark.
pub const RING_ROOM: f32 = 1.7;
/// A frame at most every 33 ms (30 fps).
pub const FRAME_MS: f32 = 33.;
/// Changes of state cross-fade over 240 ms (`duration.base`).
pub const CROSS_FADE_MS: f32 = 240.;
/// Thinking: the mark's breath, 2.4 s, each layer 70 ms after the one inside it; it turns once in 14 s.
pub const THINK_BREATH: f32 = 2400.;
pub const THINK_STAGGER: f32 = 70.;
pub const THINK_LOW: f32 = 0.35;
pub const THINK_TURN: f32 = 14_000.;
/// The thickest hairline the mark draws, in px, so a large mark never gets heavy.
pub const MAX_STROKE_PX: f32 = 1.6;

fn clamp(value: f32) -> f32 {
    value.clamp(0., 1.)
}

/// The level smoothed toward `target` over `dt` ms: attack 40 ms, release 180 ms. Under reduced motion, over 1 s.
pub fn follow(level: f32, target: f32, dt: f32, reduce: bool) -> f32 {
    let tau = if reduce {
        1000.
    } else if target > level {
        40.
    } else {
        180.
    };
    level + (target - level) * (1. - (-dt / tau).exp())
}

/// A spoken word's pulse fades, so the level follows the words and falls between them.
pub fn fade(target: f32, dt: f32) -> f32 {
    target * (-dt / 240.).exp()
}

/// The mark's light, 0 to 1, for a state: `level` is the smoothed voice, `since` the ms in the state, `clock` the ms.
pub fn light(state: VoiceState, level: f32, since: f32, clock: f32, reduce: bool) -> f32 {
    match state {
        VoiceState::Listening => 0.22 + 0.68 * level,
        VoiceState::Speaking => 0.3 + 0.6 * level,
        // You spoke over it: the light drops from 0.5 to 0.22 over 600 ms.
        VoiceState::Interrupted => 0.5 - 0.28 * (since / 600.).min(1.),
        // At rest: a slow 5.2 s breath. Still under reduced motion. Ended rests too, still.
        VoiceState::Ready if reduce => 0.35,
        VoiceState::Ready => 0.35 + (0.15 * (1. - (2. * PI * clock / 5200.).cos())) / 2.,
        VoiceState::Ended => 0.35,
        // Needs you, muted, can't hear you and a poor connection: still, the light held.
        _ => 0.5,
    }
}

/// Layer `i`'s opacity: the light reaches the eye first; Thinking breathes eye to rim.
pub fn layer_opacity(state: VoiceState, i: usize, level: f32, since: f32, clock: f32, reduce: bool) -> f32 {
    if state == VoiceState::Thinking {
        if reduce {
            return 0.6;
        }
        let phase = (clock - (LAYERS - 1 - i) as f32 * THINK_STAGGER) / THINK_BREATH;
        return 1. - ((1. - THINK_LOW) * (1. - (2. * PI * phase).cos())) / 2.;
    }
    let u = i as f32 / (LAYERS - 1) as f32;
    0.82 + 0.18 * clamp((light(state, level, since, clock, reduce) - 0.35 * (1. - u)) / 0.65)
}

/// Layer `i`'s scale about the centre: the eye draws in with your voice and breathes with Nexus's. The rim moves
/// at most 1 % and the eye at most 4 %, so the silhouette stays a true hexagon. Interrupted: the eye settles 2.5 % and
/// back over 600 ms. Under reduced motion the geometry is still.
pub fn layer_scale(state: VoiceState, i: usize, level: f32, since: f32, clock: f32, reduce: bool) -> f32 {
    if reduce {
        return 1.;
    }
    let u = i as f32 / (LAYERS - 1) as f32;
    let a = clamp(level);
    match state {
        VoiceState::Listening => 1. - a * (0.004 + 0.032 * u),
        VoiceState::Speaking => {
            1. + a * (0.006 + 0.03 * u) * (2. * PI * 1.5 * (clock / 1000.) - 0.55 * (LAYERS - 1 - i) as f32).sin()
        }
        VoiceState::Interrupted => 1. - 0.025 * u * (PI * (since / 600.).min(1.)).sin(),
        _ => 1.,
    }
}

/// The whole mark's opacity: muted is still at 40 %; can't hear you and a poor connection at 38 %.
pub fn mark_opacity(state: VoiceState) -> f32 {
    match state {
        VoiceState::Muted => 0.4,
        VoiceState::CantHear | VoiceState::PoorConnection => 0.38,
        _ => 1.,
    }
}

/// The voice ring's opacity: 45 % while Nexus speaks, 28 % while it hears you, none otherwise.
pub fn ring_opacity(state: VoiceState) -> f32 {
    match state {
        VoiceState::Speaking => 0.45,
        VoiceState::Listening => 0.28,
        _ => 0.,
    }
}

/// One tick of the ring, centred on 0,0: from its outer end to its inner one.
pub type Tick = [(f32, f32); 2];

/// The voice ring: 64 ticks that grow inward from `radius`, each 3 to 12 units with the level, on its own slow
/// oscillator, with no rotation and no sweep. Under reduced motion, a still, faint ring.
pub fn ring_ticks(radius: f32, level: f32, clock: f32, reduce: bool) -> [Tick; TICKS] {
    std::array::from_fn(|k| {
        let angle = 2. * PI * k as f32 / TICKS as f32;
        let wobble = if reduce {
            0.
        } else {
            0.55 + 0.45 * (clock * (0.002 + 0.0007 * (k % 5) as f32) + k as f32 * 2.4).sin()
        };
        let inner = radius - (3. + 9. * clamp(level) * wobble);
        let (x, y) = (angle.cos(), angle.sin());
        [(x * radius, y * radius), (x * inner, y * inner)]
    })
}

/// Where the turning mark comes to rest: forward to the next sixth of a turn, where it looks as drawn.
pub fn rest_angle(angle: f32) -> f32 {
    (angle / 60.).ceil() * 60.
}

/// How far a change of state has cross-faded, 0 to 1, `elapsed` ms after it: eased in and out, never a jump.
pub fn cross_fade(elapsed: f32) -> f32 {
    let t = clamp(elapsed / CROSS_FADE_MS);
    t * t * (3. - 2. * t)
}

/// `from` toward `to` by `t`.
pub fn mix(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// A layer's hairline in the mark's own units, capped at `MAX_STROKE_PX` when one unit is `px_per_unit` px.
pub fn stroke_width(width: f32, px_per_unit: f32) -> f32 {
    width.min(MAX_STROKE_PX / px_per_unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn the_level_is_smoothed_attack_40_ms_release_180_ms_and_1_s_under_reduced_motion() {
        assert!(close(follow(0., 1., 40., false), 1. - (-1f32).exp()));
        assert!(close(follow(1., 0., 180., false), (-1f32).exp()));
        assert!(follow(0., 1., 40., true) < 0.05);
    }

    #[test]
    fn the_light_reaches_the_eye_first() {
        let rim = layer_opacity(VoiceState::Listening, 0, 0.2, 0., 0., false);
        let eye = layer_opacity(VoiceState::Listening, LAYERS - 1, 0.2, 0., 0., false);
        assert!(eye > rim);
        for i in 0..LAYERS {
            assert!(layer_opacity(VoiceState::Speaking, i, 1., 0., 0., false) <= 1.);
        }
    }

    #[test]
    fn the_rim_moves_at_most_1_percent_and_the_eye_4_and_nothing_under_reduced_motion() {
        for clock in [0., 120., 333., 900.] {
            assert!((1. - layer_scale(VoiceState::Speaking, 0, 1., 0., clock, false)).abs() <= 0.01);
            assert!((1. - layer_scale(VoiceState::Speaking, LAYERS - 1, 1., 0., clock, false)).abs() <= 0.04);
        }
        assert!((1. - layer_scale(VoiceState::Listening, LAYERS - 1, 1., 0., 0., false)).abs() <= 0.04);
        assert_eq!(layer_scale(VoiceState::Speaking, 5, 1., 0., 200., true), 1.);
    }

    #[test]
    fn muted_dims_to_40_percent_and_cant_hear_and_a_poor_connection_to_38() {
        assert_eq!(mark_opacity(VoiceState::Muted), 0.4);
        assert_eq!(mark_opacity(VoiceState::CantHear), 0.38);
        assert_eq!(mark_opacity(VoiceState::PoorConnection), 0.38);
        assert_eq!(mark_opacity(VoiceState::Speaking), 1.);
    }

    #[test]
    fn the_ring_shows_at_45_percent_speaking_and_28_listening_as_64_ticks() {
        assert_eq!(ring_opacity(VoiceState::Speaking), 0.45);
        assert_eq!(ring_opacity(VoiceState::Listening), 0.28);
        assert_eq!(ring_opacity(VoiceState::Thinking), 0.);
        let ticks = ring_ticks(85., 0.5, 100., false);
        assert_eq!(ticks.len(), TICKS);
        for [(ox, oy), (ix, iy)] in ticks {
            let (outer, inner) = ((ox * ox + oy * oy).sqrt(), (ix * ix + iy * iy).sqrt());
            assert!(close(outer, 85.));
            let length = outer - inner;
            assert!((3. - 1e-3..=12. + 1e-3).contains(&length), "a tick of {length}");
        }
        // Under reduced motion every tick is the still, short one.
        for [(ox, oy), (ix, iy)] in ring_ticks(85., 1., 100., true) {
            assert!(close((ox * ox + oy * oy).sqrt() - (ix * ix + iy * iy).sqrt(), 3.));
        }
    }

    #[test]
    fn a_change_of_state_cross_fades_over_240_ms_never_a_jump() {
        assert_eq!(cross_fade(0.), 0.);
        assert!(close(cross_fade(CROSS_FADE_MS / 2.), 0.5));
        assert_eq!(cross_fade(CROSS_FADE_MS), 1.);
        assert_eq!(cross_fade(10_000.), 1.);
    }

    #[test]
    fn a_hairline_is_capped_at_1_6_px() {
        // The web's 220-unit mark at 120 px.
        let px_per_unit = 120. / 220.;
        assert!(close(stroke_width(3.83, px_per_unit), 1.6 * 220. / 120.));
        assert_eq!(stroke_width(0.39, px_per_unit), 0.39);
    }

    #[test]
    fn interrupted_drops_the_light_from_half_to_022_over_600_ms() {
        assert!(close(light(VoiceState::Interrupted, 0., 0., 0., false), 0.5));
        assert!(close(light(VoiceState::Interrupted, 0., 600., 0., false), 0.22));
        assert!(close(light(VoiceState::Ready, 0., 0., 1234., true), 0.35));
        assert_eq!(rest_angle(61.), 120.);
    }
}
