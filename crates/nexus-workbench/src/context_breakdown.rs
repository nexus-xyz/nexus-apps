//! Estimated context shares, matching `workbench/context-breakdown.ts`.
//! The agent's total stays authoritative; unseen system context gets the remainder.

use crate::event::WbEvent;

pub const IMAGE_TOKENS: u64 = 1_600;

/// Compaction notice emitted by Claude Code (`claude.rs`).
pub(crate) const CLAUDE_COMPACTION_NOTICE: &str = "Claude Code compacted the conversation to fit its context window.";
/// Compaction notice emitted by Codex (`codex.rs`).
pub(crate) const CODEX_COMPACTION_NOTICE: &str = "Codex compacted the conversation to fit its context window.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Estimate {
    pub messages: u64,
    pub tools: u64,
    pub thinking: u64,
    pub mcp: Vec<(String, u64)>,
    pub since_compaction: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub label: String,
    pub tokens: u64,
}

/// JavaScript counts UTF-16 code units, including two for non-BMP characters.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.encode_utf16().count() as u64).div_ceil(4)
}

pub fn mcp_server_name(tool: &str) -> Option<String> {
    let raw = tool.strip_prefix("mcp__")?.split_once("__")?.0;
    if raw.is_empty() {
        return None;
    }
    let name = raw.strip_prefix("claude_ai_").unwrap_or(raw);
    let name = name
        .strip_prefix("plugin_")
        .and_then(|name| name.split_once('_').map(|(_, rest)| rest))
        .unwrap_or(name);
    let words: Vec<String> = name
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter_map(|word| {
            let mut chars = word.chars();
            let first = chars.next()?;
            Some(first.to_uppercase().chain(chars).collect())
        })
        .collect();
    Some(if words.is_empty() { raw.into() } else { words.join(" ") })
}

impl Estimate {
    /// Events must be in log order. Child calls and answers belong to another context.
    pub fn of<'a>(events: impl IntoIterator<Item = &'a WbEvent>) -> Self {
        let mut estimate = Self::default();
        let mut tools = std::collections::HashMap::new();
        let mut outputs = std::collections::HashMap::new();
        for event in events {
            let clear = match event {
                WbEvent::Notice { text } => {
                    matches!(text.as_str(), CLAUDE_COMPACTION_NOTICE | CODEX_COMPACTION_NOTICE)
                }
                WbEvent::User { text, .. } => text
                    .trim()
                    .strip_prefix("/clear")
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace)),
                _ => false,
            };
            if clear {
                estimate = Self {
                    since_compaction: true,
                    ..Self::default()
                };
                tools.clear();
                outputs.clear();
                continue;
            }
            match event {
                WbEvent::User { text, images, .. } => {
                    estimate.messages += estimate_tokens(text) + images.len() as u64 * IMAGE_TOKENS
                }
                WbEvent::Text { text, parent: None, .. } => estimate.messages += estimate_tokens(text),
                WbEvent::Thinking { text, parent: None, .. } => estimate.thinking += estimate_tokens(text),
                WbEvent::Tool {
                    id,
                    name,
                    input,
                    parent: None,
                } => {
                    let input = input.as_str().map(str::to_owned).unwrap_or_else(|| input.to_string());
                    let server = mcp_server_name(name);
                    estimate.add_tool(server.as_deref(), estimate_tokens(name) + estimate_tokens(&input));
                    tools.insert(id.as_str(), server);
                }
                WbEvent::ToolResult { id, output, .. } => {
                    if let Some(server) = tools.get(id.as_str()) {
                        let tokens = estimate_tokens(output);
                        let previous = outputs.insert(id.as_str(), tokens).unwrap_or(0);
                        let target = estimate.tool_total(server.as_deref());
                        *target = target.saturating_sub(previous) + tokens;
                    }
                }
                _ => {}
            }
        }
        estimate
    }

    fn tool_total(&mut self, server: Option<&str>) -> &mut u64 {
        match server {
            None => &mut self.tools,
            Some(server) => {
                let index = self.mcp.iter().position(|(name, _)| name == server).unwrap_or_else(|| {
                    self.mcp.push((server.into(), 0));
                    self.mcp.len() - 1
                });
                &mut self.mcp[index].1
            }
        }
    }

    fn add_tool(&mut self, server: Option<&str>, tokens: u64) {
        *self.tool_total(server) += tokens;
    }

    /// Largest-remainder scaling keeps the parts equal to the reported total.
    pub fn parts(&self, total: u64) -> Vec<Part> {
        let mut parts = vec![Part {
            label: "Messages".into(),
            tokens: self.messages,
        }];
        if self.tools > 0 {
            parts.push(Part {
                label: "Tool calls and results".into(),
                tokens: self.tools,
            });
        }
        parts.extend(self.mcp.iter().map(|(label, tokens)| Part {
            label: label.clone(),
            tokens: *tokens,
        }));
        if self.thinking > 0 {
            parts.push(Part {
                label: "Thinking".into(),
                tokens: self.thinking,
            });
        }
        let sum: u128 = parts.iter().map(|part| u128::from(part.tokens)).sum();
        if sum > u128::from(total) {
            let mut remainders = Vec::new();
            for (index, part) in parts.iter_mut().enumerate() {
                let product = u128::from(part.tokens) * u128::from(total);
                part.tokens = (product / sum) as u64;
                remainders.push((index, product % sum));
            }
            remainders.sort_by_key(|entry| std::cmp::Reverse(entry.1));
            let short = total - parts.iter().map(|part| part.tokens).sum::<u64>();
            for (index, _) in remainders.into_iter().take(short as usize) {
                parts[index].tokens += 1;
            }
        }
        let used: u64 = parts.iter().map(|part| part.tokens).sum();
        parts.push(Part {
            label: if self.since_compaction {
                "System prompt, tools, memory and summary"
            } else {
                "System prompt, tools and memory"
            }
            .into(),
            tokens: total.saturating_sub(used),
        });
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn estimate(events: Value) -> Estimate {
        let events: Vec<WbEvent> = serde_json::from_value(events).expect("valid event fixtures");
        Estimate::of(&events)
    }

    #[test]
    fn names_each_mcp_server_from_its_tool_names() {
        for name in [
            "mcp__claude_ai_Linear__list",
            "mcp__linear__get",
            "mcp__plugin_linear_linear__get",
        ] {
            assert_eq!(mcp_server_name(name).as_deref(), Some("Linear"));
        }
        assert_eq!(
            mcp_server_name("mcp__claude_ai_Google_Calendar__list").as_deref(),
            Some("Google Calendar")
        );
        assert_eq!(
            mcp_server_name("mcp__chrome-devtools__click").as_deref(),
            Some("Chrome Devtools")
        );
        assert_eq!(mcp_server_name("Read"), None);
        assert_eq!(mcp_server_name("mcp_linear"), None);
        assert_eq!(estimate_tokens("😀😀😀"), 2);
    }

    #[test]
    fn counts_messages_thinking_tools_and_each_server_but_not_children() {
        let result = estimate(json!([
            {"kind":"user","text":"x".repeat(40)},
            {"kind":"text","id":"t","text":"x".repeat(20),"parent":null},
            {"kind":"thinking","id":"th","text":"x".repeat(80),"parent":null},
            {"kind":"tool","id":"a","name":"Read","input":{"p":"x".repeat(10)},"parent":null},
            {"kind":"toolResult","id":"a","output":"x".repeat(400),"isError":false,"truncated":false},
            {"kind":"tool","id":"b","name":"mcp__linear__get","input":{},"parent":null},
            {"kind":"toolResult","id":"b","output":"x".repeat(40),"isError":false,"truncated":false},
            {"kind":"tool","id":"child","name":"Read","input":{},"parent":"task"},
            {"kind":"toolResult","id":"child","output":"x".repeat(4000),"isError":false,"truncated":false},
            {"kind":"text","id":"ct","text":"x".repeat(400),"parent":"task"}
        ]));
        assert_eq!(result.messages, 15);
        assert_eq!(result.thinking, 20);
        assert_eq!(result.tools, 106);
        assert_eq!(result.mcp, vec![("Linear".into(), 15)]);
    }

    #[test]
    fn compaction_and_clear_reset_context() {
        for boundary in [
            json!({"kind":"notice","text": CLAUDE_COMPACTION_NOTICE}),
            json!({"kind":"notice","text": CODEX_COMPACTION_NOTICE}),
            json!({"kind":"user","text":" /clear now"}),
        ] {
            let result = estimate(
                json!([{"kind":"user","text":"x".repeat(4000)}, boundary, {"kind":"user","text":"x".repeat(40)}]),
            );
            assert_eq!(result.messages, 10);
            assert!(result.since_compaction);
            assert!(
                result
                    .parts(5000)
                    .last()
                    .expect("the system context row")
                    .label
                    .ends_with("summary")
            );
        }
        assert!(!estimate(json!([{"kind":"user","text":"/clearly not"}])).since_compaction);
    }

    #[test]
    fn steer_notice_mentioning_compaction_does_not_reset_context() {
        let steer = crate::linked::steer::steer_text(
            "Coordinator",
            "t_parent",
            "Claude compacted the conversation, so re-read X.",
        );
        let result = estimate(json!([
            {"kind":"user","text":"x".repeat(4000)},
            {"kind":"notice","text": steer},
            {"kind":"user","text":"x".repeat(40)}
        ]));
        assert!(!result.since_compaction);
        assert_eq!(result.messages, 1010);
    }

    #[test]
    fn counts_images_and_the_total() {
        let image = json!({"mediaType":"image/png","data":"abc","name":null});
        assert_eq!(
            estimate(json!([{"kind":"user","text":"abcd","images":[image]}])).messages,
            1601
        );
        let result = Estimate {
            messages: 1000,
            tools: 1003,
            thinking: 1000,
            ..Estimate::default()
        };
        for total in [0, 1, 1001, 84000, u64::MAX] {
            let parts = result.parts(total);
            assert_eq!(parts.iter().map(|part| part.tokens).sum::<u64>(), total);
        }
        assert_eq!(
            result.parts(1001).iter().map(|part| part.tokens).collect::<Vec<_>>(),
            vec![334, 334, 333, 0]
        );
        assert_eq!(Estimate::default().parts(0).len(), 2);
    }
}
