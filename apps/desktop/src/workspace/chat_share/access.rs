//! The external version's readers and access requests, as Nexus UI's AccessPanel.

use super::*;
use crate::palette::Palette;

impl ChatShare {
    pub(super) fn load_access(&mut self, cx: &mut Context<Self>) {
        self.access = None;
        let current = &self.detail["sharing"];
        if !Audience::from_key(&word(&current["audience"], "kind")).outside() {
            return;
        }
        let id = word(current, "artifactId");
        let path = format!(
            "/v1/artifacts/{}/access",
            url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
        );
        let request = self.gateway.request(Verb::Get, path, None);
        fetch(cx, request, move |this, answer, _| {
            if word(&this.detail["sharing"], "artifactId") == id {
                this.access = Some(answer);
            }
        });
    }

    fn change_access(&mut self, action: &str, email: &str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.problem = None;
        let id = word(&self.detail["sharing"], "artifactId");
        let path = format!(
            "/v1/artifacts/{}/access",
            url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
        );
        let mut body = json!({});
        body[action] = json!([email]);
        let request = self.gateway.request(Verb::Post, path, Some(body));
        fetch(cx, request, |this, answer, _| {
            this.busy = false;
            match answer {
                Ok(access) => this.access = Some(Ok(access)),
                Err(error) => this.problem = Some(error),
            }
        });
    }

    /// Approve, Deny or Remove on a History row: primary for Approve, `control` for the others.
    pub(super) fn access_button(
        &self,
        action: &'static str,
        label: &'static str,
        primary: bool,
        email: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let email = email.to_string();
        div()
            .id(SharedString::from(format!("share-access-{action}-{email}")))
            .flex_none()
            .h(px(28.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(if primary { palette.primary } else { palette.menu_hover })
            .text_color(if primary {
                palette.primary_foreground
            } else {
                palette.foreground
            })
            .when(primary, |button| button.font_weight(FontWeight::MEDIUM))
            .child(label)
            .when(self.busy, |button| button.opacity(0.4))
            .when(!self.busy, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.change_access(action, &email, cx);
                    }))
            })
    }
}
