//! Pure activity reduction and navigation transitions. No execution or permission operations.
use crate::workspace::side_pane::Tool;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum Target {
    Browser(String),
    Terminal(String),
    Changes(String),
    File(String, Option<u32>),
    Page(String),
    Preview(String),
    Agents,
}
impl Target {
    pub(in crate::workspace) fn tool(&self) -> Tool {
        match self {
            Self::Browser(_) | Self::Preview(_) => Tool::Browser,
            Self::Terminal(_) => Tool::Terminal,
            Self::Changes(_) => Tool::Changes,
            Self::File(..) => Tool::Files,
            Self::Page(_) => Tool::Artifacts,
            Self::Agents => Tool::Agents,
        }
    }
    pub(in crate::workspace) fn label(&self) -> String {
        match self {
            Self::Browser(id) | Self::Terminal(id) | Self::Changes(id) | Self::Page(id) | Self::Preview(id) => {
                format!("{} · {id}", self.tool().label())
            }
            Self::File(path, _) => format!("Files · {path}"),
            Self::Agents => "Background tasks".into(),
        }
    }
}

pub(in crate::workspace) struct Navigation {
    pub manual: bool,
    pub latest: Option<Target>,
    pub unread: Vec<Target>,
    pub revision: u64,
    pending: Option<(Target, u64, Instant)>,
    pub baseline: Option<i64>,
    pub seen: HashSet<String>,
    pub native: HashMap<String, u64>,
    calls: HashMap<String, u64>,
    pub(super) scanned: Option<(i64, u64)>,
}
impl Navigation {
    pub fn new(manual: bool) -> Self {
        Self {
            manual,
            latest: None,
            unread: vec![],
            revision: 0,
            pending: None,
            baseline: None,
            seen: HashSet::new(),
            native: HashMap::new(),
            calls: HashMap::new(),
            scanned: None,
        }
    }
    pub(super) fn collect(&mut self, blocks: &[crate::code::Block]) -> Vec<(Target, u64)> {
        let mut targets = Vec::new();
        for (index, block) in blocks.iter().enumerate() {
            match block {
                crate::code::Block::Tool {
                    call,
                    name,
                    edit,
                    error: None,
                    ..
                } => {
                    let revision = *self.calls.entry(call.id.clone()).or_insert(self.revision);
                    let Some(output) = call.output.as_ref().filter(|output| !output.error) else {
                        continue;
                    };
                    if self.seen.contains(&result_key(&call.id, &output.text)) {
                        continue;
                    }
                    if let Some((seq, path)) = edit {
                        let target = Target::Changes(path.clone());
                        if self.seen.insert(format!("edit:{}", seq)) {
                            targets.push((target, revision));
                        }
                    }
                    for path in patch_paths(name, &call.input) {
                        if self.seen.insert(format!("patch:{}:{path}", call.id)) {
                            targets.push((Target::Changes(path), revision));
                        }
                    }
                    // Whole tab IDs only: `tab-x-1` is a prefix of `tab-x-12`.
                    let tabs = browser_ids(&output.text);
                    for (tab, revision) in &self.native {
                        if tabs.contains(tab) && self.seen.insert(format!("browser:{}:{}", call.id, tab)) {
                            targets.push((Target::Browser(tab.clone()), *revision));
                        }
                    }
                    // Exact IDs are checked against the actual command registry below.
                    for id in execution_ids(&output.text) {
                        targets.push((Target::Terminal(id), revision));
                    }
                }
                crate::code::Block::Text(text) => {
                    let tabs = browser_ids(text);
                    for (tab, revision) in &self.native {
                        if tabs.contains(tab) && self.seen.insert(format!("browser-text:{index}:{tab}")) {
                            targets.push((Target::Browser(tab.clone()), *revision));
                        }
                    }
                    for id in execution_ids(text) {
                        if !self.seen.contains(&format!("history-execution:{index}:{id}")) {
                            targets.push((Target::Terminal(id), self.revision));
                        }
                    }
                    for reference in file_links(text) {
                        if self
                            .seen
                            .insert(format!("file:{index}:{}:{:?}", reference.path, reference.line))
                        {
                            targets.push((Target::File(reference.path, reference.line), self.revision));
                        }
                    }
                }
                crate::code::Block::Agent(agent) => {
                    if self.seen.insert(format!("agent:{}:{}", agent.call_id, agent.steps)) {
                        targets.push((Target::Agents, self.revision));
                    }
                }
                crate::code::Block::Inside { .. } => {}
                _ => {}
            }
        }
        targets
    }
    pub fn sync_mode(&mut self, manual: bool) {
        if manual != self.manual {
            if manual {
                self.interact();
            } else {
                self.set_manual(false);
                self.pending = None;
            }
        }
    }
    /// The one place `manual` changes.
    fn set_manual(&mut self, manual: bool) {
        self.manual = manual;
    }
    pub fn reset_history(&mut self) {
        self.baseline = None;
        self.pending = None;
        self.seen.clear();
        self.native.clear();
        self.calls.clear();
        self.scanned = None;
    }
    pub fn interact(&mut self) {
        self.set_manual(true);
        self.revision += 1;
        self.pending = None;
    }
    pub fn offer(&mut self, target: Target, ready: bool, revision: u64) {
        self.offer_filtered(target, ready, revision, true);
    }
    pub fn offer_filtered(&mut self, target: Target, ready: bool, revision: u64, automatic: bool) {
        self.unread.retain(|known| known != &target);
        self.unread.push(target.clone());
        if !ready {
            return;
        }
        self.latest = Some(target.clone());
        if automatic && !self.manual && revision == self.revision {
            self.pending = Some((target, revision, Instant::now()));
        }
    }
    pub fn follow(&mut self) {
        self.set_manual(false);
        self.pending = self
            .latest
            .clone()
            .map(|target| (target, self.revision, Instant::now()));
    }
    pub fn take(&mut self, hidden: bool, active: bool, now: Instant) -> Option<Target> {
        let (target, revision, at) = self.pending.as_ref()?;
        if now.duration_since(*at) < Duration::from_millis(120) {
            return None;
        }
        let target = target.clone();
        let revision = *revision;
        self.pending = None;
        if hidden || !active || self.manual || revision != self.revision {
            return None;
        }
        self.unread.clear();
        Some(target)
    }
}

fn result_key(id: &str, text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    format!("result:{id}:{}", hash.finish())
}
pub(super) fn seed(blocks: &[crate::code::Block], seen: &mut HashSet<String>) {
    for (index, block) in blocks.iter().enumerate() {
        match block {
            crate::code::Block::Tool { call, .. } => {
                if let Some(output) = &call.output {
                    seen.insert(result_key(&call.id, &output.text));
                }
            }
            crate::code::Block::Text(text) => {
                for id in browser_ids(text) {
                    seen.insert(format!("browser-text:{index}:{id}"));
                }
                for id in execution_ids(text) {
                    seen.insert(format!("history-execution:{index}:{id}"));
                }
                for reference in file_links(text) {
                    seen.insert(format!("file:{index}:{}:{:?}", reference.path, reference.line));
                }
            }
            crate::code::Block::Agent(agent) => {
                seen.insert(format!("agent:{}:{}", agent.call_id, agent.steps));
            }
            _ => {}
        }
    }
}
/// Read patch syntax, never execute or guess what an arbitrary shell command changed.
fn patch_paths(name: &str, input: &serde_json::Value) -> Vec<String> {
    let text = input
        .as_str()
        .or_else(|| ["patch", "input", "code"].iter().find_map(|key| input[*key].as_str()));
    let Some(text) = text else { return vec![] };
    let patch = if name.ends_with("apply_patch") {
        text.to_string()
    } else if name == "functions.exec" || name == "exec" {
        static CALL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r#"tools\.apply_patch\(\s*("(?:\\.|[^"\\])*")"#).expect("literal patch call")
        });
        let Some(capture) = CALL.captures(text) else {
            return vec![];
        };
        let Ok(patch) = serde_json::from_str::<String>(&capture[1]) else {
            return vec![];
        };
        patch
    } else {
        return vec![];
    };
    patch
        .lines()
        .filter_map(|line| {
            [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}
pub(super) fn scoped_path(path: &str, folder: &std::path::Path) -> Option<String> {
    let path = std::path::Path::new(path);
    let path = if path.is_absolute() {
        path.strip_prefix(folder).ok()?
    } else {
        path
    };
    if path.components().any(|part| {
        matches!(
            part,
            std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    let path = path.strip_prefix(".").unwrap_or(path);
    (!path.as_os_str().is_empty()).then(|| path.to_string_lossy().into_owned())
}
fn browser_ids(text: &str) -> Vec<String> {
    static IDS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\btab-[A-Za-z0-9-]+\b").expect("browser tab ID"));
    IDS.find_iter(text).map(|found| found.as_str().into()).collect()
}
fn execution_ids(text: &str) -> Vec<String> {
    static IDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").expect("execution ID")
    });
    IDS.find_iter(text).map(|found| found.as_str().into()).collect()
}
/// The files an answer links to, outside code: a link quoted as an example names no file to open.
fn file_links(text: &str) -> Vec<nexus_workbench::inline_media::FileReference> {
    nexus_workbench::inline_media::file_links(text)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
