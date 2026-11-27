//! Who may fix what: the person's permissions in an organization and the owners and admins who can act for them. The
//! workspace keeps it current (`workspace/setup_fix.rs`); a setup line reads it to choose between the fix and Ask.

use std::collections::HashMap;

use gpui_kit::{App, Global};
use serde_json::Value;

/// An owner or an admin of the organization (`GET /v1/orgs/members`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manager {
    pub user_id: String,
    /// The name a line shows: the first word of their name, else their email's local part.
    pub name: String,
    pub owner: bool,
}

/// One organization's view, for the person signed in to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrgAccess {
    /// The person's own user id: they are never asked.
    pub me: Option<String>,
    /// What their role allows (`/v1/tenants/me`). `None` when Gateway listed none: the fix shows, and Gateway's refusal
    /// tells (as `pages/providers.rs` does).
    pub permissions: Option<Vec<String>>,
    /// Owners first, then admins, each by name.
    pub managers: Vec<Manager>,
}

/// Every signed-in organization's [`OrgAccess`], by its URL segment (`Account::org`).
#[derive(Default)]
pub struct Access(HashMap<String, OrgAccess>);

impl Global for Access {}

impl Access {
    /// The person's access in `org`, empty when unknown.
    pub fn of(org: &str, cx: &App) -> OrgAccess {
        cx.try_global::<Access>()
            .and_then(|access| access.0.get(org).cloned())
            .unwrap_or_default()
    }

    /// The account was read: its permissions, before the members list arrives.
    pub fn set_account(org: &str, me: Option<String>, permissions: Option<Vec<String>>, cx: &mut App) {
        let entry = cx.default_global::<Access>().0.entry(org.to_string()).or_default();
        entry.me = me;
        entry.permissions = permissions;
    }

    pub fn set_managers(org: &str, managers: Vec<Manager>, cx: &mut App) {
        cx.default_global::<Access>()
            .0
            .entry(org.to_string())
            .or_default()
            .managers = managers;
    }

    /// The person left `org`, or signed out.
    pub fn forget(org: &str, cx: &mut App) {
        cx.default_global::<Access>().0.remove(org);
    }
}

/// Permissions only owners hold (`OWNER` in `rs/crates/nexus-base/src/permissions.rs`). Every other one a line asks
/// for is an owner's and an admin's (`MANAGERS`).
const OWNER_ONLY: &[&str] = &[
    "org.transfer",
    "org.delete",
    "org.billing",
    "budgets.delegate",
    "wiki.run",
    "review.configure",
];

fn owner_only(permission: &str) -> bool {
    OWNER_ONLY.contains(&permission)
}

/// What a line offers this person for a fix that needs `needs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// They can: the fix itself.
    Fix,
    /// They can't: Ask, naming up to two people who can (none: "an admin" or "an owner").
    Ask { who: Vec<String> },
}

/// The person holds `needs` (or it needs nothing, or Gateway did not say): the fix; else Ask the people who can.
pub fn decide(needs: Option<&str>, access: &OrgAccess) -> Decision {
    let Some(permission) = needs else {
        return Decision::Fix;
    };
    match &access.permissions {
        Some(held) if !held.iter().any(|each| each == permission) => Decision::Ask {
            who: who_can(permission, access),
        },
        _ => Decision::Fix,
    }
}

/// Up to two people who hold `permission`, owners first, never the person themself.
pub fn who_can(permission: &str, access: &OrgAccess) -> Vec<String> {
    let only_owners = owner_only(permission);
    access
        .managers
        .iter()
        .filter(|manager| manager.owner || !only_owners)
        .filter(|manager| access.me.as_deref() != Some(manager.user_id.as_str()))
        .map(|manager| manager.name.clone())
        .take(2)
        .collect()
}

/// "an owner" or "an admin": who can, when the app knows nobody by name.
pub fn someone(needs: Option<&str>) -> &'static str {
    if needs.is_some_and(owner_only) {
        "an owner"
    } else {
        "an admin"
    }
}

/// "Ana", "Ana and Dev", or [`someone`].
pub fn names(who: &[String], needs: Option<&str>) -> String {
    match who {
        [] => someone(needs).to_string(),
        [one] => one.clone(),
        [first, second, ..] => format!("{first} and {second}"),
    }
}

/// The Ask button: "Ask Ana", "Ask an admin". It names the first person only: the message goes to them.
pub fn ask_label(who: &[String], needs: Option<&str>) -> String {
    format!("Ask {}", who.first().map_or(someone(needs), String::as_str))
}

/// The line's second sentence: "Ana and Dev can add one." `doing` is what they can do ("add one"); empty says "can."
pub fn can_line(who: &[String], needs: Option<&str>, doing: &str) -> String {
    let names = names(who, needs);
    let mut chars = names.chars();
    let names = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => names,
    };
    if doing.is_empty() {
        format!("{names} can.")
    } else {
        format!("{names} can {doing}.")
    }
}

/// What the Ask button becomes once pressed: a Gateway request lands in an inbox; anything else was copied.
pub fn asked_label(who: &[String], needs: Option<&str>, sent: bool) -> String {
    match (sent, who) {
        (true, [one]) => format!("Asked · in {one}'s inbox"),
        (true, _) => "Asked · in the owners' inboxes".into(),
        (false, _) => format!(
            "Copied · send it to {}",
            who.first().map_or(someone(needs), String::as_str)
        ),
    }
}

/// The name a line shows for a member: their first name, else their email's local part.
fn short_name(name: &str, email: &str) -> Option<String> {
    let first = name.split_whitespace().next().filter(|word| !word.contains('@'));
    first.map(str::to_string).or_else(|| {
        email
            .split('@')
            .next()
            .filter(|part| !part.is_empty())
            .map(str::to_string)
    })
}

/// The owners and admins in `GET /v1/orgs/members`, owners first, then by name.
pub fn parse_managers(value: &Value) -> Vec<Manager> {
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let mut managers: Vec<Manager> = value
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let role = text(row, "role");
            if role != "owner" && role != "admin" {
                return None;
            }
            let user_id = text(row, "userId");
            let name = short_name(&text(row, "name"), &text(row, "email"))?;
            (!user_id.is_empty()).then_some(Manager {
                user_id,
                name,
                owner: role == "owner",
            })
        })
        .collect();
    managers.sort_by(|a, b| {
        b.owner
            .cmp(&a.owner)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    managers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn acme(permissions: Option<&[&str]>) -> OrgAccess {
        OrgAccess {
            me: Some("u_me".into()),
            permissions: permissions.map(|list| list.iter().map(|each| each.to_string()).collect()),
            managers: parse_managers(&json!({"members": [
                {"userId": "u_dev", "name": "Dev Patel", "role": "admin"},
                {"userId": "u_me", "name": "Maya Chen", "role": "member"},
                {"userId": "u_ana", "name": "Ana Souza", "role": "owner"},
                {"userId": "u_li", "name": "", "email": "li@acme.dev", "role": "admin"},
            ]})),
        }
    }

    #[test]
    fn whoever_holds_the_permission_sees_the_fix() {
        let access = acme(Some(&["provider_keys.manage"]));
        assert_eq!(decide(Some("provider_keys.manage"), &access), Decision::Fix);
        assert_eq!(
            decide(None, &access),
            Decision::Fix,
            "a fix that needs nothing shows to everyone"
        );
    }

    #[test]
    fn anyone_else_asks_two_people_who_can_owners_first() {
        let access = acme(Some(&["agents.use"]));
        assert_eq!(
            decide(Some("provider_keys.manage"), &access),
            Decision::Ask {
                who: vec!["Ana".into(), "Dev".into()]
            }
        );
    }

    #[test]
    fn an_owner_only_permission_asks_owners_only() {
        let access = acme(Some(&[]));
        assert_eq!(
            decide(Some("org.billing"), &access),
            Decision::Ask {
                who: vec!["Ana".into()]
            }
        );
    }

    #[test]
    fn permissions_gateway_did_not_list_show_the_fix() {
        assert_eq!(decide(Some("policy.manage"), &acme(None)), Decision::Fix);
    }

    #[test]
    fn the_person_is_never_asked() {
        let mut access = acme(Some(&[]));
        access.me = Some("u_ana".into());
        assert_eq!(
            who_can("policy.manage", &access),
            vec!["Dev".to_string(), "li".to_string()]
        );
    }

    #[test]
    fn with_nobody_known_it_asks_an_admin_or_an_owner() {
        let access = OrgAccess {
            permissions: Some(Vec::new()),
            ..OrgAccess::default()
        };
        let Decision::Ask { who } = decide(Some("policy.manage"), &access) else {
            panic!("a member without the permission asks");
        };
        assert!(who.is_empty());
        assert_eq!(ask_label(&who, Some("policy.manage")), "Ask an admin");
        assert_eq!(ask_label(&who, Some("org.billing")), "Ask an owner");
        assert_eq!(can_line(&who, Some("policy.manage"), ""), "An admin can.");
    }

    #[test]
    fn the_words_name_the_people() {
        let two = vec!["Ana".to_string(), "Dev".to_string()];
        assert_eq!(ask_label(&two, None), "Ask Ana");
        assert_eq!(can_line(&two, None, "add one"), "Ana and Dev can add one.");
        assert_eq!(asked_label(&two[..1], None, true), "Asked · in Ana's inbox");
        assert_eq!(asked_label(&two, None, true), "Asked · in the owners' inboxes");
        assert_eq!(asked_label(&two, None, false), "Copied · send it to Ana");
        assert_eq!(
            asked_label(&[], Some("org.billing"), false),
            "Copied · send it to an owner"
        );
    }
}
