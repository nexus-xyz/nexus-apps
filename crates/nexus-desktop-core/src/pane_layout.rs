//! Narrow Workbench layout rules.

#[derive(Debug, Default)]
pub struct Visibility {
    pub drawer: bool,
    docked_open: bool,
}

impl Visibility {
    /// Read the persisted docked choice instead of the transient narrow-window choice.
    pub fn docked_choice(&self, shown: bool) -> bool {
        if self.drawer { self.docked_open } else { shown }
    }

    /// A different screen keeps its own docked choice; its narrow drawer starts closed.
    pub fn switch_context(&mut self, docked_open: bool) -> bool {
        self.docked_open = docked_open;
        !self.drawer && docked_open
    }

    /// A breakpoint change closes the transient drawer and restores the docked choice.
    pub fn resize(&mut self, window_width: f32, shown: bool) -> bool {
        let drawer = window_width <= 1099.;
        if drawer == self.drawer {
            return shown;
        }
        self.drawer = drawer;
        if drawer {
            self.docked_open = shown;
            false
        } else {
            self.docked_open
        }
    }
}

#[derive(Clone, Copy)]
pub enum Content {
    Default,
    Terminal,
    Artifact,
}

pub fn drawer_width(available: f32, content: Content) -> f32 {
    let available = available.max(0.);
    match content {
        Content::Default => 380_f32.min(available * 0.92),
        Content::Terminal => available,
        Content::Artifact => 600_f32.min(available),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawer_visibility_never_overwrites_the_docked_choice() {
        let mut state = Visibility::default();
        assert!(state.resize(1200., true));
        assert!(!state.resize(1099., true));
        assert!(state.drawer);
        assert!(state.resize(900., true));
        assert!(state.resize(1200., false));
        assert!(!state.drawer);
        assert!(!state.resize(1000., false));
        assert!(!state.resize(1200., true));
    }

    #[test]
    fn home_switches_preserve_each_docked_choice_while_the_drawer_is_transient() {
        let mut layout = Visibility::default();
        let mut home = crate::home_bench::Visibility::default();
        assert!(!layout.resize(900., true));
        let next = home.switch(true, layout.docked_choice(false));
        assert!(!layout.switch_context(next));
        assert!(!layout.resize(1200., true));
        let next = home.switch(false, layout.docked_choice(false));
        assert!(layout.switch_context(next));
        assert!(!layout.resize(900., true));
        let next = home.switch(true, layout.docked_choice(true));
        assert!(!layout.switch_context(next));
        assert!(!layout.resize(1200., false));
    }

    #[test]
    fn drawer_sizes_follow_the_content_without_squeezing_the_conversation() {
        assert_eq!(drawer_width(800., Content::Default), 380.);
        assert!((drawer_width(320., Content::Default) - 294.4).abs() < 0.01);
        assert_eq!(drawer_width(800., Content::Artifact), 600.);
        assert_eq!(drawer_width(320., Content::Artifact), 320.);
        assert_eq!(drawer_width(800., Content::Terminal), 800.);
    }
}
