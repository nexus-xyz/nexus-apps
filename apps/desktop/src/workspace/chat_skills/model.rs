//! Chat skill names are resolved by Gateway. Never infer byte equality from matching names.
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub duplicate_name: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub query: String,
}

pub fn token(text: &str, caret: usize) -> Option<Token> {
    if !text.is_char_boundary(caret) {
        return None;
    }
    let start = text[..caret]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + text[i..].chars().next().map(char::len_utf8).unwrap_or(0));
    if text.as_bytes().get(start) != Some(&b'/') || caret <= start {
        return None;
    }
    let end = text[caret..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| caret + i);
    if !text[start + 1..end]
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some(Token {
        start,
        end,
        query: text[start + 1..caret].to_lowercase(),
    })
}

pub fn catalog(value: &Value) -> Result<Vec<Skill>, String> {
    fn list(value: &Value) -> Result<Vec<Skill>, String> {
        let value = value
            .get("skills")
            .filter(|v| v.is_array())
            .ok_or("The skill list is malformed.")?;
        serde_json::from_value(value.clone()).map_err(|e| e.to_string())
    }
    let own = list(&value["own"])?;
    let library = value.get("library").ok_or("The library response is missing.")?;
    let library = if library.is_null() {
        vec![]
    } else {
        list(&value["library"])?
    };
    Ok(merge(own, library))
}

pub fn merge(own: Vec<Skill>, mut library: Vec<Skill>) -> Vec<Skill> {
    let mut flagged = HashMap::new();
    for skill in &library {
        *flagged.entry(skill.name.clone()).or_insert(false) |= skill.duplicate_name;
    }
    for mut skill in own {
        match flagged.get(&skill.name) {
            Some(false) => continue,
            Some(true) => skill.duplicate_name = true,
            None => {}
        }
        library.push(skill);
    }
    library
}

pub fn matches<'a>(skills: &'a [Skill], query: &str) -> Vec<&'a Skill> {
    let query = query.to_lowercase();
    let mut rows: Vec<_> = skills
        .iter()
        .filter(|s| s.name.to_lowercase().contains(&query) || s.description.to_lowercase().contains(&query))
        .collect();
    rows.sort_by_key(|s| {
        if s.name.to_lowercase().starts_with(&query) {
            0
        } else if s.name.to_lowercase().contains(&query) {
            1
        } else {
            2
        }
    });
    rows
}

pub fn choose(text: &str, token: &Token, name: &str) -> (String, usize) {
    let suffix = &text[token.end..];
    let space = if suffix.starts_with(' ') { "" } else { " " };
    let prefix = format!("{}/{name}{space}", &text[..token.start]);
    let caret = prefix.len() + usize::from(suffix.starts_with(' '));
    (format!("{prefix}{suffix}"), caret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tokens_use_the_caret_and_leave_paths_urls_and_unicode_intact() {
        assert_eq!(token("é /rev later", 7).unwrap().query, "rev");
        for text in ["a/b", "./x", "http://x", "/usr/bin", "/bad_name"] {
            assert_eq!(token(text, text.len()), None);
        }
        assert_eq!(token("é", 1), None);
        let text = "é /re old";
        let t = token(text, 6).unwrap();
        assert_eq!(choose(text, &t, "review"), ("é /review old".into(), 11));
    }
    #[test]
    fn server_collision_flags_block_own_and_library_copies() {
        let value = json!({"own":{"skills":[{"name":"review","description":"mine"},{"name":"other"}]},"library":{"skills":[{"name":"review","scope":"org","duplicateName":true}]}});
        let skills = catalog(&value).unwrap();
        assert_eq!(skills.iter().filter(|s| s.duplicate_name).count(), 2);
        assert_eq!(
            skills
                .iter()
                .filter(|s| !s.duplicate_name)
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["other"]
        );
    }
    #[test]
    fn identical_library_copy_serves_and_missing_library_falls_back() {
        let own = json!({"skills":[{"name":"review"}]});
        let both = json!({"own":own,"library":{"skills":[{"name":"review","scope":"org"}]}});
        assert_eq!(catalog(&both).unwrap().len(), 1);
        assert_eq!(catalog(&json!({"own":own,"library":null})).unwrap().len(), 1);
        assert!(catalog(&json!({"own":own,"library":{}})).is_err());
    }
    #[test]
    fn filtering_keeps_name_prefixes_before_description_matches() {
        let skills=catalog(&json!({"own":{"skills":[{"name":"other","description":"review"},{"name":"review"},{"name":"code-review"}]},"library":null})).unwrap();
        assert_eq!(
            matches(&skills, "REV")
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["review", "code-review", "other"]
        );
    }
}
