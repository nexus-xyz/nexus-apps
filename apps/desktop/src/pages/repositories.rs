//! Settings › Repositories lists the organization's linked GitHub repositories.
//! Owners and admins connect repositories on GitHub. Review setup opens in the browser.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::{Ctx, Load, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: Ctx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| RepositoriesPage::new(ctx, cx));
    Some(view.into())
}

const DESCRIPTION: &str =
    "Repositories your organization linked through the Nexus GitHub App (read-only contents and metadata).";

// ------------------------------------------------------------------ What Gateway answers

/// The person's repository permissions from `GET /v1/tenants/me`.
#[derive(Debug, Clone, PartialEq, Default)]
struct Me {
    permissions: Vec<String>,
}

impl Me {
    fn can(&self, permission: &str) -> bool {
        self.permissions.iter().any(|held| held == permission)
    }
}

fn parse_me(answer: &Value) -> Me {
    Me {
        permissions: answer
            .get("permissions")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default(),
    }
}

/// `GET /v1/github/app`.
#[derive(Debug, Clone, PartialEq)]
struct GithubApp {
    configured: bool,
    install_url: Option<String>,
}

fn parse_app(answer: &Value) -> GithubApp {
    GithubApp {
        configured: answer.get("configured").and_then(Value::as_bool).unwrap_or(false),
        install_url: answer
            .get("installUrl")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .map(String::from),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Repository {
    id: String,
    full_name: String,
    url: String,
    default_branch: String,
    private: bool,
    linked_at: String,
}

/// `GET /v1/github/repositories`.
#[derive(Debug, Clone, PartialEq)]
struct Linked {
    accounts: Vec<String>,
    repositories: Vec<Repository>,
}

fn parse_linked(answer: &Value) -> Option<Linked> {
    let repositories = answer.get("repositories")?.as_array()?;
    Some(Linked {
        accounts: answer
            .get("installations")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|installation| ui::text(installation, "account").to_string())
                    .filter(|account| !account.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        repositories: repositories
            .iter()
            .filter_map(|repo| {
                Some(Repository {
                    id: repo.get("id")?.as_str()?.to_string(),
                    full_name: ui::text(repo, "fullName").to_string(),
                    url: ui::text(repo, "url").to_string(),
                    default_branch: ui::text(repo, "defaultBranch").to_string(),
                    private: repo.get("private").and_then(Value::as_bool).unwrap_or(false),
                    linked_at: ui::text(repo, "linkedAt").to_string(),
                })
            })
            .collect(),
    })
}

/// A query value for a URL.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct RepositoriesPage {
    ctx: Ctx,
    me: Load<Me>,
    app: Load<GithubApp>,
    linked: Load<Linked>,
}

impl RepositoriesPage {
    fn new(ctx: Ctx, cx: &mut Context<Self>) -> RepositoriesPage {
        let mut page = RepositoriesPage {
            ctx,
            me: Load::Loading,
            app: Load::Loading,
            linked: Load::Loading,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.me = Load::Loading;
        self.app = Load::Loading;
        self.linked = Load::Loading;
        let gateway = self.ctx.gateway.clone();
        super::read(cx, gateway.query("/v1/tenants/me".into()), |this, answer, _| {
            this.me = match answer {
                Ok(answer) => Load::Ready(parse_me(&answer)),
                Err(error) => Load::Failed(error.into()),
            };
        });
        super::read(cx, gateway.query("/v1/github/app".into()), |this, answer, _| {
            this.app = match answer {
                Ok(answer) => Load::Ready(parse_app(&answer)),
                Err(error) => Load::Failed(error.into()),
            };
        });
        self.read_linked(cx);
    }

    fn read_linked(&mut self, cx: &mut Context<Self>) {
        super::read(
            cx,
            self.ctx.gateway.query("/v1/github/repositories".into()),
            |this, answer, _| {
                this.linked = match answer {
                    Ok(answer) => parse_linked(&answer)
                        .map(Load::Ready)
                        .unwrap_or_else(|| Load::Failed("Gateway sent repositories this app cannot read.".into())),
                    Err(error) => Load::Failed(error.into()),
                };
            },
        );
    }

    fn render_list(&self, me: &Me, app: &GithubApp, linked: &Linked, palette: Palette, cx: &mut Context<Self>) -> Div {
        let manage = me.can("repositories.manage");
        let review = me.can("review.configure");
        let accounts = if linked.accounts.is_empty() {
            "None yet".to_string()
        } else {
            linked
                .accounts
                .iter()
                .map(|account| format!("@{account}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut column = v_flex().child(
            h_flex()
                .justify_between()
                .child(ui::section_head("Linked repositories", palette))
                .child(ui::note(accounts, palette).mt(px(14.))),
        );
        if let (true, Some(url)) = (manage, app.install_url.clone()) {
            column = column.child(
                h_flex()
                    .gap(px(10.))
                    .mb(px(12.))
                    .child(connect_button(palette, true).on_click(move |_, _, cx| crate::external_link::open(&url, cx)))
                    .child(ui::note("Choose them on GitHub; change them there any time.", palette)),
            );
        }
        if linked.repositories.is_empty() {
            let detail = if manage {
                "Connect repositories on GitHub above."
            } else {
                "An owner or admin connects repositories."
            };
            return column.child(ui::empty("No repositories linked", detail, palette));
        }
        column = column.child(
            ui::head_row(palette)
                .child(div().flex_1().child("Repository"))
                .child(div().w(px(110.)).child("Branch"))
                .when(review, |head| {
                    head.child(div().w(px(200.)).flex().justify_end().child("Actions"))
                }),
        );
        for repo in &linked.repositories {
            let url = repo.url.clone();
            let name = repo.full_name.clone();
            let row = ui::row(palette)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(
                            div()
                                .id(SharedString::from(format!("repo-open-{}", repo.id)))
                                .truncate()
                                .text_size(px(13.))
                                .text_color(palette.strong)
                                .cursor_pointer()
                                .hover(|style| style.underline())
                                .child(repo.full_name.clone())
                                .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child(format!(
                                    "{} · linked {}",
                                    if repo.private { "Private" } else { "Public" },
                                    ui::day(&repo.linked_at)
                                )),
                        ),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .truncate()
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .text_color(palette.foreground)
                        .child(if repo.default_branch.is_empty() {
                            "–".to_string()
                        } else {
                            repo.default_branch.clone()
                        }),
                )
                .when(review, |row| {
                    row.child(
                        h_flex().w(px(200.)).flex_none().justify_end().child(
                            ui::button(
                                SharedString::from(format!("repo-review-{}", repo.id)),
                                "Set up Review",
                                palette,
                            )
                            .text_size(px(12.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ctx
                                    .open_web(&format!("repositories/review?repo={}", encode(&name)), cx)
                            })),
                        ),
                    )
                });
            column = column.child(row);
        }
        column
    }
}

fn connect_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::primary_button("repo-connect", "Connect repositories on GitHub", palette)
    } else {
        ui::disabled_button("repo-connect", "Connect repositories on GitHub", palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .child(ui::section_head("Linked repositories", palette))
            .child(connect_button(palette, false))
            .into_any_element()
    })
}

impl Render for RepositoriesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let failure = [&self.me.as_failure(), &self.app.as_failure(), &self.linked.as_failure()]
            .into_iter()
            .find_map(|failure| failure.clone());
        let body: AnyElement = if let Some(error) = failure {
            v_flex()
                .items_center()
                .child(ui::failed(error, palette))
                .child(
                    ui::button("repo-retry", "Try again", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                )
                .into_any_element()
        } else {
            match (&self.me, &self.app, &self.linked) {
                (Load::Ready(me), Load::Ready(app), _) if !app.configured => {
                    let manage = me.can("repositories.manage");
                    v_flex()
                        .items_center()
                        .child(ui::empty(
                            "The Nexus GitHub App is not set up on this Gateway yet",
                            if manage {
                                "Create it once from GitHub's App manifest, then connect repositories."
                            } else {
                                "Ask your organization's owner or an admin to set it up."
                            },
                            palette,
                        ))
                        .when(manage, |column| {
                            column.child(
                                ui::web_link("repo-setup-app", "Set up the GitHub App in the browser", palette)
                                    .on_click(cx.listener(|this, _, _, cx| this.ctx.open_web("settings/github", cx))),
                            )
                        })
                        .into_any_element()
                }
                (Load::Ready(me), Load::Ready(app), Load::Ready(linked)) => {
                    let (me, app, linked) = (me.clone(), app.clone(), linked.clone());
                    self.render_list(&me, &app, &linked, palette, cx).into_any_element()
                }
                _ => ui::skeleton(4, palette).into_any_element(),
            }
        };
        v_flex().w_full().child(ui::lead(DESCRIPTION, palette))
            .child(ui::settings_notice("Repository access", "Repository authorization and Review setup continue in your browser. Existing repository controls remain available below.", palette).mb(px(12.)))
            .child(body)
    }
}

/// The failure of a read, if it failed.
trait Failure {
    fn as_failure(&self) -> Option<SharedString>;
}

impl<T> Failure for Load<T> {
    fn as_failure(&self) -> Option<SharedString> {
        match self {
            Load::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob import of gpui_kit brings its own `test` macro: name the built-in one.
    use core::prelude::v1::test;

    /// A sample answer, written as Gateway sends it.
    fn value(text: &str) -> Value {
        serde_json::from_str(text).expect("JSON")
    }

    #[test]
    fn reads_the_app_and_the_linked_repositories() {
        let app = parse_app(&value(
            r#"{ "configured": true, "slug": "nexus-app", "installUrl": "https://github.com/apps/nexus-app/installations/new?state=x" }"#,
        ));
        assert!(app.configured && app.install_url.is_some());
        assert_eq!(parse_app(&value(r#"{ "configured": false }"#)).install_url, None);

        let linked = parse_linked(&value(r#"{
            "installations": [{ "id": "inst_1", "account": "acme", "accountType": "Organization", "repositorySelection": "selected", "createdAt": "2026-10-01T00:00:00Z" }],
            "repositories": [
                {
                    "id": "repo_1", "installationId": "inst_1", "fullName": "acme/web", "url": "https://github.com/acme/web",
                    "defaultBranch": "main", "private": true, "linkedAt": "2026-10-02T10:00:00Z"
                },
                { "id": "repo_2", "installationId": "inst_1", "fullName": "acme/api", "url": "https://github.com/acme/api", "private": false, "linkedAt": "2026-10-03T10:00:00Z" }
            ]
        }"#))
        .unwrap();
        assert_eq!(linked.accounts, vec!["acme"]);
        assert_eq!(linked.repositories.len(), 2);
        assert!(linked.repositories[1].default_branch.is_empty());
    }

    #[test]
    fn reads_who_may_manage_repositories() {
        let me = parse_me(&value(
            r#"{
            "tenant": { "id": "ten_1", "name": "Acme" },
            "permissions": ["repositories.manage"]
        }"#,
        ));
        assert!(me.can("repositories.manage"));
        assert!(!me.can("review.configure"));
        assert_eq!(encode("acme/web"), "acme%2Fweb");
    }
}
