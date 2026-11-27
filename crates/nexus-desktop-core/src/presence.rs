//! Team presence words and ordering, as Nexus UI shows them.
use serde::Deserialize;

pub const POLL_SECONDS: u64 = 30;
pub const TEAM_SHOWN: usize = 8;
pub const STACK_SHOWN: usize = 4;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub online: bool,
    pub activity: Option<Activity>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Activity {
    Chatting,
    Page {
        title: Option<String>,
    },
    Agents {
        count: u32,
    },
    Idle,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub members: Vec<Member>,
    pub show_online: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub show_online: bool,
}

pub fn eligible(signed_in: bool, personal: bool, visible: bool) -> bool {
    signed_in && !personal && visible
}

pub fn teammates(mut members: Vec<Member>, self_id: Option<&str>, self_email: &str) -> Vec<Member> {
    members.retain(|member| {
        if let Some(id) = self_id {
            member.user_id != id
        } else {
            member
                .email
                .as_deref()
                .is_none_or(|email| !email.eq_ignore_ascii_case(self_email))
        }
    });
    members.sort_by_key(|member| !member.online);
    members
}

pub fn display_name(member: &Member) -> &str {
    member
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            member
                .email
                .as_deref()
                .and_then(|email| email.split('@').next())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or("Someone")
}

pub fn initials(member: &Member) -> String {
    let source = member
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| member.email.as_deref().and_then(|email| email.split('@').next()))
        .unwrap_or("");
    let letters: String = source
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '.' | '_' | '-'))
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() { "?".into() } else { letters }
}

pub fn hue(user_id: &str) -> usize {
    user_id
        .chars()
        .fold(0_u32, |hash, ch| hash.wrapping_mul(31).wrapping_add(ch as u32)) as usize
        % 4
}

pub fn activity_label(member: &Member) -> Option<String> {
    if !member.online {
        return None;
    }
    match member.activity.as_ref()? {
        Activity::Chatting => Some("chatting".into()),
        Activity::Agents { count } if *count > 0 => Some(format!(
            "running {} {}",
            if *count > 9 { "9+".into() } else { count.to_string() },
            if *count == 1 { "agent" } else { "agents" }
        )),
        Activity::Page { title } => Some(title.as_deref().filter(|title| !title.trim().is_empty()).map_or_else(
            || "editing a page".into(),
            |title| format!("editing {}", short_title(title)),
        )),
        _ => None,
    }
}

pub fn short_title(title: &str) -> String {
    let text = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= 40 {
        return text;
    }
    let cut: String = text.chars().take(39).collect();
    let end = cut
        .rfind(' ')
        .filter(|at| cut[..*at].chars().count() > 24)
        .unwrap_or(cut.len());
    format!("{}…", cut[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn member(id: &str, name: &str, online: bool) -> Member {
        Member {
            user_id: id.into(),
            name: Some(name.into()),
            email: Some(format!("{id}@example.com")),
            online,
            activity: None,
        }
    }
    #[test]
    fn personal_signed_out_and_hidden_windows_never_send_presence() {
        assert!(eligible(true, false, true));
        assert!(!eligible(false, false, true));
        assert!(!eligible(true, true, true));
        assert!(!eligible(true, false, false));
    }
    #[test]
    fn exclude_yourself_and_keep_the_gateways_order_with_online_first() {
        let members = vec![
            member("self", "You", true),
            member("a", "Ada", false),
            member("z", "Zoe", true),
            member("b", "Bob", true),
        ];
        let team = teammates(members, Some("self"), "");
        assert_eq!(
            team.iter().map(|m| m.user_id.as_str()).collect::<Vec<_>>(),
            ["z", "b", "a"]
        );
        assert!(teammates(vec![member("self", "You", true)], None, "SELF@example.com").is_empty());
        assert_eq!((STACK_SHOWN, TEAM_SHOWN), (4, 8));
    }
    #[test]
    fn names_initials_and_tints_match_the_sidebar_rules() {
        let mut m = member("u", "Ana Souza", true);
        assert_eq!(display_name(&m), "Ana Souza");
        assert_eq!(initials(&m), "AS");
        m.name = Some(" ".into());
        m.email = Some("bruno.lima@example.com".into());
        assert_eq!(display_name(&m), "bruno.lima");
        assert_eq!(initials(&m), "BL");
        m.email = None;
        assert_eq!(display_name(&m), "Someone");
        assert_eq!(initials(&m), "?");
        assert_eq!(hue("u"), hue("u"));
        assert!(hue("another") < 4);
    }
    #[test]
    fn offline_and_idle_have_no_activity_and_agent_counts_are_bounded() {
        let mut m = member("u", "Ada", true);
        m.activity = Some(Activity::Agents { count: 1 });
        assert_eq!(activity_label(&m).as_deref(), Some("running 1 agent"));
        m.activity = Some(Activity::Agents { count: 12 });
        assert_eq!(activity_label(&m).as_deref(), Some("running 9+ agents"));
        m.online = false;
        assert_eq!(activity_label(&m), None);
        m.online = true;
        m.activity = Some(Activity::Idle);
        assert_eq!(activity_label(&m), None);
    }
    #[test]
    fn page_activity_uses_only_gateway_visible_titles() {
        let mut m = member("u", "Ada", true);
        m.activity = Some(Activity::Page { title: None });
        assert_eq!(activity_label(&m).as_deref(), Some("editing a page"));
        m.activity = Some(Activity::Page {
            title: Some("  Pricing   v2 ".into()),
        });
        assert_eq!(activity_label(&m).as_deref(), Some("editing Pricing v2"));
        assert!(
            short_title("A very long page title that should end near a word boundary and stay short")
                .chars()
                .count()
                <= 40
        );
    }
    #[test]
    fn unknown_future_activity_does_not_hide_a_teammate() {
        let m: Member = serde_json::from_value(
            serde_json::json!({"userId":"u","name":"Ada","email":null,"online":true,"activity":{"kind":"future"}}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(activity_label(&m), None);
    }
}
