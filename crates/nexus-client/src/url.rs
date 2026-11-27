//! Paths and query strings as the browser builds them, so every client sends the same bytes as `kit`'s TypeScript.

/// `encodeURIComponent`.
pub fn enc(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A query string (`?a=1&b=2`, form-encoded as `URLSearchParams`) from the set, non-empty values.
pub fn search(query: &[(&str, Option<String>)]) -> String {
    let Ok(mut url) = reqwest::Url::parse("http://query.invalid/") else {
        return String::new();
    };
    let mut any = false;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                pairs.append_pair(key, value);
                any = true;
            }
        }
    }
    if any {
        format!("?{}", url.query().unwrap_or(""))
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_paths_and_queries_as_the_browser_does() {
        assert_eq!(enc("a b/c@d"), "a%20b%2Fc%40d");
        assert_eq!(enc("it's(ok)"), "it's(ok)");
        assert_eq!(
            search(&[("a", Some("x y".into())), ("b", None), ("c", Some(String::new()))]),
            "?a=x+y"
        );
        assert_eq!(search(&[("b", None)]), "");
    }
}
