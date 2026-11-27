const PREFIX: &str = "<!-- nexus-shared-memory:";

fn marker(key: &str, edge: &str) -> String {
    if key.is_empty() {
        format!("{PREFIX}{edge} -->")
    } else {
        format!("{PREFIX}{key}:{edge} -->")
    }
}

fn edge<'a>(line: &'a str, kind: &str) -> Option<&'a str> {
    let value = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix(PREFIX)?
        .strip_suffix(" -->")?;
    if value == kind {
        return Some("");
    }
    let (key, edge) = value.rsplit_once(':')?;
    (edge == kind).then_some(key)
}

/// Keep unmatched markers and human-authored text when removing completed export blocks.
pub(super) fn without(text: &str, remove: impl Fn(&str) -> bool) -> String {
    let mut kept = String::new();
    let mut block = String::new();
    let mut key = None;
    for line in text.split_inclusive('\n') {
        if let Some(start) = edge(line, "begin") {
            kept.push_str(&block);
            block.clear();
            block.push_str(line);
            key = Some(start);
        } else if let Some(start) = key {
            block.push_str(line);
            if edge(line, "end") == Some(start) {
                if remove(start) {
                    if kept.ends_with("\r\n\r\n") {
                        kept.truncate(kept.len() - 2);
                    } else if kept.ends_with("\n\n") {
                        kept.pop();
                    }
                } else {
                    kept.push_str(&block);
                }
                block.clear();
                key = None;
            }
        } else {
            kept.push_str(line);
        }
    }
    kept.push_str(&block);
    kept
}

pub(super) fn replace(text: &str, key: &str, content: &str) -> String {
    let retained = without(text, |previous| previous == key);
    let mut output = retained.trim_end_matches(['\r', '\n']).to_owned();
    if !content.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&marker(key, "begin"));
        output.push('\n');
        for line in content.lines() {
            if edge(line, "begin").is_some() || edge(line, "end").is_some() {
                output.push_str("    ");
            }
            output.push_str(line);
            output.push('\n');
        }
        output.push_str(&marker(key, "end"));
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output
}
