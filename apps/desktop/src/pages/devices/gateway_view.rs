//! This device when Gateway vouches for the person's devices: no code, QR code or approval to show. The computer
//! registers itself (`sign_in.rs`, and the page's `register_if_gateway`), and this card says where it stands.

use super::view::device_tile;
use super::*;

impl DevicesPage {
    pub(super) fn render_this_device_by_account(&self, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let Some(me) = self.me() else {
            let name = self.name.read(cx).value().trim().to_string();
            return self.render_connecting(name, cx);
        };
        let row = |detail: String| {
            ui::card_row()
                .child(device_tile(&me.name, p))
                .child(ui::setting_lines(me.name.clone(), detail, p).flex_1())
        };
        let new_key = |id: &'static str, label: &'static str, busy_label: &'static str, cx: &mut Context<Self>| {
            if self.busy {
                ui::disabled_button(id, busy_label, p).into_any_element()
            } else {
                ui::button(id, label, p)
                    .debug_selector(move || id.into())
                    .on_click(cx.listener(|this, _, _, cx| this.new_key(cx)))
                    .into_any_element()
            }
        };
        let card = match status(me, now_ms()) {
            Status::Trusted => row(format!(
                "Connected through your account · Added {}",
                ui::day(&me.created_at)
            ))
            .child(ui::status("Trusted", Palette::success(), p)),
            Status::Pending => return self.render_connecting(me.name.clone(), cx),
            // Gateway answers an old pairing's key with its expired row: only a new key connects this computer.
            Status::Expired => row("This computer's earlier setup expired. Set it up again to connect it.".into())
                .child(new_key("devices-again", "Set up again", "Setting up…", cx)),
            Status::Revoked => row(format!(
                "Revoked {}. It gets no new keys. To use it again, make a new key.",
                ui::day(me.revoked_at.as_deref().unwrap_or(&me.created_at))
            ))
            .child(new_key("devices-new-key", "Make a new key", "Making a new key…", cx)),
        };
        ui::card_frame(p).child(card)
    }

    /// This computer before Gateway lists it as trusted: it is connecting, or its registration failed.
    fn render_connecting(&self, name: String, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let failed = !self.busy && matches!(self.outcome, Some((Part::ThisDevice, _, true)));
        let detail = if failed {
            "This computer is not connected to your account yet."
        } else {
            "Connecting this computer to your account…"
        };
        let control = if failed {
            ui::button("devices-connect-retry", "Try again", p)
                .on_click(cx.listener(|this, _, _, cx| this.register(Some(crate::computer_name::desktop_label()), cx)))
                .into_any_element()
        } else {
            ui::status("Connecting", p.waiting(), p).into_any_element()
        };
        ui::card_frame(p).child(
            ui::card_row()
                .child(device_tile(&name, p))
                .child(ui::setting_lines(name, detail, p).flex_1())
                .child(control),
        )
    }
}
