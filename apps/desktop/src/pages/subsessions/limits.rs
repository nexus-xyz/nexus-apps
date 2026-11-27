use std::num::IntErrorKind;

use nexus_workbench::linked::MAX_DEPTH;

use nexus_desktop_core::settings::OrchestrationLimits;

/// The slider is a shortcut for common values. The number field can hold any value the setting represents.
#[derive(Clone, Copy)]
pub(super) enum Limit {
    Workers,
    Sessions,
    Worktrees,
    Starts,
    AutomaticTurns,
    EmptyWaits,
    Depth,
    Spend,
}

impl Limit {
    pub(super) const ALL: [Self; 8] = [
        Self::Workers,
        Self::Sessions,
        Self::Worktrees,
        Self::Starts,
        Self::AutomaticTurns,
        Self::EmptyWaits,
        Self::Depth,
        Self::Spend,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Workers => "Workers per session",
            Self::Sessions => "Sessions on this computer",
            Self::Worktrees => "Child worktrees",
            Self::Starts => "Starts per tree",
            Self::AutomaticTurns => "Automatic turns per tree",
            Self::EmptyWaits => "Empty waits",
            Self::Depth => "Nesting depth",
            Self::Spend => "Spend per tree",
        }
    }

    pub(super) fn detail(self) -> String {
        match self {
            Self::Workers => "Live subsessions from one session.",
            Self::Sessions => "Live subsessions on this computer.",
            Self::Worktrees => "Worktrees that live subsessions can use.",
            Self::Starts => "Subsessions a tree can start.",
            Self::AutomaticTurns => "Turns a tree can take on its own.",
            Self::EmptyWaits => "Waits that return no result.",
            Self::Depth => return format!("Levels below the top session. The maximum is {MAX_DEPTH}."),
            Self::Spend => "Estimated cost at list price, in US dollars.",
        }
        .into()
    }

    pub(super) fn zero_detail(self) -> String {
        match self {
            Self::Workers | Self::Sessions | Self::Starts => "Blocks new subsessions.",
            Self::Worktrees => "Blocks new child worktrees.",
            Self::AutomaticTurns => "Allows no automatic turns.",
            Self::EmptyWaits => "Pauses after the first empty wait.",
            Self::Depth => return format!("Blocks new subsessions. The maximum is {MAX_DEPTH}."),
            Self::Spend => "Pauses at the first usage report.",
        }
        .into()
    }

    pub(super) fn slider_max(self) -> f32 {
        match self {
            Self::Workers | Self::EmptyWaits => 10.,
            Self::Depth => MAX_DEPTH as f32,
            Self::Sessions | Self::Worktrees => 20.,
            Self::Starts | Self::AutomaticTurns => 100.,
            Self::Spend => 200.,
        }
    }

    pub(super) fn slider_value(self, value: Option<f64>) -> f32 {
        value.unwrap_or_default().min(f64::from(self.slider_max())) as f32
    }

    pub(super) fn parse(self, text: &str) -> Result<Option<f64>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        if matches!(self, Self::Spend) {
            return text
                .parse::<f64>()
                .ok()
                .filter(|amount| amount.is_finite() && *amount >= 0.)
                .map(|amount| Some(amount.max(0.)))
                .ok_or_else(|| "Enter a dollar amount of zero or more.".into());
        }
        let count = text.parse::<u32>().map_err(|error| match error.kind() {
            IntErrorKind::PosOverflow => format!("Enter a whole number up to {}.", self.max_count()),
            _ => "Enter a whole number of zero or more.".into(),
        })?;
        if count > self.max_count() {
            return Err(format!("Enter a nesting depth up to {MAX_DEPTH}."));
        }
        Ok(Some(f64::from(count)))
    }

    fn max_count(self) -> u32 {
        if matches!(self, Self::Depth) {
            MAX_DEPTH
        } else {
            u32::MAX
        }
    }

    pub(super) fn get(self, limits: &OrchestrationLimits) -> Option<f64> {
        let count = match self {
            Self::Workers => limits.max_live_children,
            Self::Sessions => limits.max_live_linked,
            Self::Worktrees => limits.max_child_worktrees,
            Self::Starts => limits.max_starts,
            Self::AutomaticTurns => limits.max_auto_turns,
            Self::EmptyWaits => limits.max_idle_waits,
            Self::Depth => limits.max_depth,
            Self::Spend => return limits.crew_spend_usd,
        };
        count.map(f64::from)
    }

    pub(super) fn set(self, limits: &mut OrchestrationLimits, value: Option<f64>) {
        let count = value.map(|value| value as u32);
        match self {
            Self::Workers => limits.max_live_children = count,
            Self::Sessions => limits.max_live_linked = count,
            Self::Worktrees => limits.max_child_worktrees = count,
            Self::Starts => limits.max_starts = count,
            Self::AutomaticTurns => limits.max_auto_turns = count,
            Self::EmptyWaits => limits.max_idle_waits = count,
            Self::Depth => limits.max_depth = count,
            Self::Spend => limits.crew_spend_usd = value,
        }
    }
}
