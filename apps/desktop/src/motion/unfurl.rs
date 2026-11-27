//! The F5 mark's layer step. In the drawn mark each layer sits 10.18° past the one outside it (`app-mark.svg`); the
//! field behind the sign-in (`field.rs`) turns by the same step. The sign-in's arrival used to unfurl the layers by it;
//! the welcome mark now spins in instead (`spin.rs`).

/// How far each layer sits past the one outside it, in degrees.
pub(crate) const STEP: f32 = 10.18;
