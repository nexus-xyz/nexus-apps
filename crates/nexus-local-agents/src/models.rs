//! Models and reasoning efforts reported by installed coding agents.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub efforts: Vec<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_id: Option<String>,
    /// The agent reports a fast speed for it: Codex's `priority` service tier. Claude Code reports none; nexus-workbench's
    /// `claude::supports_fast` lists its fast models.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fast: bool,
}

/// The agent's reasoning effort. None on a session is the model's own default.
/// Ordered as declared, lowest first: a linked session runs at the lower of its own and its parent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    #[serde(rename = "none")]
    Disabled,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Disabled => "none",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
            Effort::Ultra => "ultra",
        }
    }

    pub fn parse(value: &str) -> Option<Effort> {
        match value {
            "none" => Some(Effort::Disabled),
            "minimal" => Some(Effort::Minimal),
            "low" => Some(Effort::Low),
            "medium" => Some(Effort::Medium),
            "high" => Some(Effort::High),
            "xhigh" => Some(Effort::Xhigh),
            "max" => Some(Effort::Max),
            "ultra" => Some(Effort::Ultra),
            _ => None,
        }
    }
}
