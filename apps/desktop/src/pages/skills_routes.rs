//! Skills-only web route intent. IDs are decoded once; API adapters encode the resulting opaque ID again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Detail(String),
    Requests { of: Option<String>, state: Option<String> },
}
impl Target {
    pub(crate) fn web_path(&self) -> String {
        match self {
            Self::Detail(id) => format!("skills/promotions/{}", nexus_client::url::enc(id)),
            Self::Requests { of, state } => {
                let mut url = url::Url::parse("http://skills.invalid/skills").expect("fixed URL");
                {
                    let mut query = url.query_pairs_mut();
                    query.append_pair("view", "requests");
                    if let Some(of) = of {
                        query.append_pair("of", of);
                    }
                    if let Some(state) = state {
                        query.append_pair("state", state);
                    }
                }
                format!("skills?{}", url.query().unwrap_or_default())
            }
        }
    }

    pub(crate) fn parse(path: &str) -> Option<Self> {
        let url = url::Url::parse(&format!("http://skills.invalid/{}", path.trim_start_matches('/'))).ok()?;
        let path = url.path().trim_start_matches('/');
        if path == "skills"
            && url
                .query_pairs()
                .find(|(k, _)| k == "view")
                .is_some_and(|(_, v)| v == "requests")
        {
            let get = |name| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
            return Some(Self::Requests {
                of: get("of"),
                state: get("state"),
            });
        }
        let id = path.strip_prefix("skills/promotions/")?;
        if id.is_empty() || id.contains('/') {
            return None;
        }
        let bytes = id.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                let hex = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
                decoded.push(hex(*bytes.get(i + 1)?)? * 16 + hex(*bytes.get(i + 2)?)?);
                i += 3;
            } else {
                decoded.push(bytes[i]);
                i += 1;
            }
        }
        let id = String::from_utf8(decoded).ok()?;
        (!id.is_empty()).then_some(Self::Detail(id))
    }
}
/// Web requestsFilters: an approver defaults to reach/held; mine defaults to all, including explicit mine.
pub(super) fn filters(approver: bool, of: Option<&str>, state: Option<&str>) -> (bool, &'static str) {
    let mine = !approver || of == Some("mine");
    let state = match state {
        Some("held") => "held",
        Some("approved") => "approved",
        Some("refused") => "refused",
        Some("withdrawn") => "withdrawn",
        Some("all") => "all",
        _ if mine => "all",
        _ => "held",
    };
    (mine, state)
}
