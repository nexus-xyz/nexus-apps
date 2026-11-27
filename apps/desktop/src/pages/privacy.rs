//! Privacy gathers existing controls without reading external conversation history, in three cards: conversation
//! history and outside access, kept on this computer, and links to the account's own privacy pages. Each card's head
//! names where its settings are kept, so the page has no page-wide place. Account links keep their existing native
//! views, browser fallbacks and permission checks.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::{DeviceCtx, Load, local_sessions, outside_access, palette, read, ui};

pub const NATIVE: bool = true;

pub fn build(ctx: DeviceCtx, account: Option<super::Ctx>, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let gateway = account.map(|account| account.gateway);
    Some(
        cx.new(|cx| {
            let history = local_sessions::controls(ctx.clone(), cx);
            let page = PrivacyPage {
                ctx,
                history,
                account: gateway.as_ref().map(|_| Load::Loading),
                outside_problem: None,
            };
            if let Some(gateway) = gateway {
                read(
                    cx,
                    gateway.query("/v1/tenants/me".into()),
                    |this: &mut PrivacyPage, answer, _| {
                        this.account = Some(match answer {
                            Ok(answer) => account_access(&answer)
                                .map(Load::Ready)
                                .unwrap_or_else(|| Load::Failed("Gateway sent no organization.".into())),
                            Err(error) => Load::Failed(error.into()),
                        });
                    },
                );
            }
            page
        })
        .into(),
    )
}

struct PrivacyPage {
    ctx: DeviceCtx,
    history: AnyView,
    account: Option<Load<AccountAccess>>,
    outside_problem: Option<SharedString>,
}

#[derive(Debug, PartialEq)]
struct AccountAccess {
    personal: bool,
    settings: bool,
}

fn account_access(answer: &Value) -> Option<AccountAccess> {
    let tenant = answer.get("tenant")?.as_object()?;
    Some(AccountAccess {
        personal: tenant.get("kind").and_then(Value::as_str) == Some("personal"),
        settings: answer
            .get("permissions")
            .and_then(Value::as_array)
            .is_some_and(|permissions| {
                permissions
                    .iter()
                    .any(|permission| permission.as_str() == Some("org.settings"))
            }),
    })
}

struct Link {
    icon: Lucide,
    label: &'static str,
    note: &'static str,
    path: &'static str,
}

fn opens_in_browser(path: &str) -> bool {
    !crate::workspace::settings_path_is_native(path)
}

fn links(account: &AccountAccess) -> Vec<Link> {
    let mut links = Vec::new();
    if !account.personal {
        links.push(Link {
            icon: Lucide::Eye,
            label: "Online status",
            note: "Whether teammates see your online status and activity. Never your content.",
            path: "/settings/preferences#show-online",
        });
    }
    links.push(Link {
        icon: Lucide::Smartphone,
        label: "Trusted devices",
        note: "Devices that can read your synced sessions. Revoke one here.",
        path: "/settings/devices",
    });
    if account.settings && !account.personal {
        links.push(Link {
            icon: Lucide::RefreshCw,
            label: "Session sync",
            note: "Your organization's rules for synced coding sessions.",
            path: "/settings/general#synced-sessions",
        });
    }
    links.push(Link {
        icon: Lucide::Link,
        label: "Sharing and retention",
        note: "Shared links and chat retention.",
        path: "/settings/data",
    });
    links
}

impl Render for PrivacyPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let outside = outside_access::render(
            (self.ctx.code)(cx).outside,
            self.outside_problem.clone(),
            |this: &mut PrivacyPage, problem| this.outside_problem = problem,
            palette,
            cx,
        );
        let here = || ui::scope_tag(Lucide::Laptop, "This computer", palette);
        let mut page = v_flex()
            .w_full()
            .child(ui::lead(
                "Choose what Nexus reads on this computer and who sees your activity.",
                palette,
            ))
            .children(super::desktop_app::read_only_note(palette))
            .child(ui::group_head("Conversation history", palette).child(here()))
            .child(self.history.clone())
            .child(ui::footnote(
                "Nexus reads titles and folders only. Nothing leaves this computer.",
                palette,
            ));
        page = match outside {
            Some(outside) => page.child(outside),
            None => page
                .child(ui::group_head("Outside access", palette).child(here()))
                .child(ui::setting_detail(
                    "Outside access controls are available while this app owns Code sessions.",
                    palette,
                )),
        };
        page = page.child(ui::group_head("Your account", palette).child(ui::scope_tag(
            Lucide::CircleUser,
            "Synced to your account",
            palette,
        )));
        match &self.account {
            None if self.ctx.local_workspace => page.child(ui::group(
                links(&AccountAccess {
                    personal: false,
                    settings: true,
                })
                .into_iter()
                .map(|link| Some(ui::requires_sign_in_row(link.label, link.note, palette).into_any_element())),
                palette,
            )),
            None | Some(Load::Loading) => page.child(ui::skeleton(3, palette)),
            Some(Load::Failed(error)) => page.child(ui::failed(error.clone(), palette)),
            Some(Load::Ready(account)) => {
                let rows = links(account).into_iter().map(|link| {
                    let path = link.path.to_string();
                    let in_browser = opens_in_browser(link.path);
                    Some(
                        ui::nav_row(
                            SharedString::from(format!("privacy-{}", link.label)),
                            link.icon,
                            link.label,
                            link.note,
                            in_browser,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            (this.ctx.navigate_settings)(path.clone(), window, cx)
                        }))
                        .into_any_element(),
                    )
                });
                page.child(ui::group(rows, palette))
                    .child(ui::footnote("Your organization's permissions still apply.", palette))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use serde_json::json;

    #[test]
    fn account_link_labels_follow_the_same_native_resolver_as_navigation() {
        for link in links(&AccountAccess {
            personal: false,
            settings: true,
        }) {
            assert!(
                crate::workspace::settings_path_is_native(link.path),
                "{} must open its native settings page",
                link.label,
            );
            assert!(
                !opens_in_browser(link.path),
                "{} must use the native navigation row",
                link.label
            );
        }
    }

    #[test]
    fn unsupported_account_settings_keep_the_browser_navigation_indicator() {
        let path = "/settings/not-a-native-page";
        assert!(!crate::workspace::settings_path_is_native(path));
        assert!(opens_in_browser(path));
    }

    #[test]
    fn account_links_follow_personal_status_and_real_permissions() {
        let paths = |answer: Value| {
            links(&account_access(&answer).expect("account"))
                .into_iter()
                .map(|link| link.path)
                .collect::<Vec<_>>()
        };
        let member = paths(json!({"tenant":{"kind":"organization"},"permissions":["org.read"]}));
        assert!(member.contains(&"/settings/preferences#show-online"));
        assert!(!member.contains(&"/settings/general#synced-sessions"));
        let admin = paths(json!({"tenant":{"kind":"organization"},"permissions":["org.settings"]}));
        assert!(admin.contains(&"/settings/general#synced-sessions"));
        let personal = paths(json!({"tenant":{"kind":"personal"},"permissions":["org.settings"]}));
        assert!(!personal.contains(&"/settings/preferences#show-online"));
        assert!(!personal.contains(&"/settings/general#synced-sessions"));
        assert!(personal.contains(&"/settings/devices"));
        assert!(personal.contains(&"/settings/data"));
        assert!(account_access(&json!({"permissions":["org.settings"]})).is_none());
    }
}
