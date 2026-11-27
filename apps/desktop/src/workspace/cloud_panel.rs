//! The Code top bar's Cloud button and its panel, on a session that runs on a cloud machine, as the Nexus app draws
//! them (`workbench/cloud-actions.tsx`): the ports something listens on there, each with Preview (forwarded to this
//! computer over SSH) and a share link (the provider's link with its token, copied), and Watch desktop (the machine's
//! live screen). While the machine does not run, the panel says so; a sleeping one wakes when the person sends a
//! message.
//!
//! Preview shows the forwarded address in the session's Browser tab, or in the system browser when the session is no
//! longer on screen. Watch desktop opens in the system browser: the stream is the provider's address. The calls are
//! `nexus_cloud::desktop`'s (`Cloud::ports`, `preview`, `share` and `desktop`).

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::time::Duration;

use futures::channel::oneshot;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_remote::machine::Status;

use super::TopPanel;
use super::git_panel::popover;
use crate::cloud::words;
use crate::cloud::{Cloud, CloudPort};
use crate::machines::{CloudView, NOT_HERE};
use crate::palette::Palette;
use crate::workspace::code_dock::menu_box;
use crate::workspace::{View, Workspace, bar_button, glyph};

/// How long "Link copied." shows (`useCopied`).
const COPIED_FOR: Duration = Duration::from_millis(1500);

/// The open Cloud panel of session `id`, whose machine is `machine`.
pub(in crate::workspace) struct CloudPanel {
    pub id: String,
    machine: String,
    /// The machine's ports, once asked; cleared while it does not run, so they are asked again once it does.
    ports: Option<Vec<CloudPort>>,
    checking: bool,
    /// The port whose share link was copied last.
    shared: Option<u16>,
    /// "Link copied." shows; `copies` counts the copies, so only the latest one's timer hides it.
    copied: bool,
    copies: u64,
    problem: Option<SharedString>,
}

impl CloudPanel {
    fn new(id: &str, machine: &str) -> CloudPanel {
        CloudPanel {
            id: id.to_string(),
            machine: machine.to_string(),
            ports: None,
            checking: false,
            shared: None,
            copied: false,
            copies: 0,
            problem: None,
        }
    }
}

/// Whether the machine runs and its host answers (`status.state === "connected"`): its ports can be read.
fn running(machine: &CloudView) -> bool {
    matches!(machine.status, Status::Connected { .. })
}

/// Runs `work` with the cloud machines on their runtime, and hands its answer to `then` on the UI thread. While the
/// Nexus app runs them, the answer is [`NOT_HERE`].
fn on_cloud<T, F>(
    cx: &mut Context<Workspace>,
    work: impl FnOnce(Cloud) -> F,
    then: impl FnOnce(&mut Workspace, Result<T, String>, &mut Context<Workspace>) + 'static,
) where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    let (answer, answered) = oneshot::channel();
    match crate::cloud::get() {
        Some(cloud) => {
            let work = work(cloud);
            crate::code_run::runtime().spawn(async move {
                let _ = answer.send(work.await);
            });
        }
        None => {
            let _ = answer.send(Err(NOT_HERE.to_string()));
        }
    }
    cx.spawn(async move |this, cx| {
        if let Ok(value) = answered.await {
            let _ = this.update(cx, |this, cx| {
                then(this, value, cx);
                cx.notify();
            });
        }
    })
    .detach();
}

impl Workspace {
    /// The cloud machine session `id` runs on, if it runs on one.
    fn cloud_machine_of(&self, id: &str) -> Option<&CloudView> {
        let machine = self.session_by_id(id)?.machine.as_deref()?;
        self.cloud_machines.iter().find(|candidate| candidate.id == machine)
    }

    /// The open Cloud panel, when it is session `id`'s.
    fn cloud_panel(&mut self, id: &str) -> Option<&mut CloudPanel> {
        match self.overlay.code_top_mut() {
            Some(TopPanel::Cloud(panel)) if panel.id == id => Some(panel),
            _ => None,
        }
    }

    /// Open session `id`'s Cloud panel, or close it when it is open. A running machine's ports are asked for now.
    fn toggle_cloud_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_panel(id).is_some() {
            self.overlay.close_code_top();
            cx.notify();
            return;
        }
        let Some(machine) = self.cloud_machine_of(id).map(|machine| machine.id.clone()) else {
            return;
        };
        self.open_overlay(Overlay::CodeTop(TopPanel::Cloud(CloudPanel::new(id, &machine))));
        self.cloud_panel_follow(cx);
        cx.notify();
    }

    /// The machines changed (or the panel opened): ports are asked for once the panel's machine runs, and forgotten
    /// while it does not.
    pub(in crate::workspace) fn cloud_panel_follow(&mut self, cx: &mut Context<Self>) {
        let Some(TopPanel::Cloud(panel)) = self.overlay.code_top() else {
            return;
        };
        let runs = self
            .cloud_machines
            .iter()
            .find(|machine| machine.id == panel.machine)
            .is_some_and(running);
        let id = panel.id.clone();
        if !runs {
            if let Some(panel) = self.cloud_panel(&id) {
                panel.ports = None;
            }
        } else if panel.ports.is_none() && !panel.checking {
            self.check_ports(&id, cx);
        }
    }

    /// Ask the machine which ports something listens on.
    fn check_ports(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(panel) = self.cloud_panel(id) else {
            return;
        };
        panel.checking = true;
        panel.problem = None;
        let machine = panel.machine.clone();
        let id = id.to_string();
        on_cloud(
            cx,
            move |cloud| async move { cloud.ports(&machine).await },
            move |this, answer, _| {
                let Some(panel) = this.cloud_panel(&id) else {
                    return;
                };
                panel.checking = false;
                match answer {
                    Ok(ports) => panel.ports = Some(ports),
                    Err(error) => panel.problem = Some(error.into()),
                }
            },
        );
        cx.notify();
    }

    /// Preview: forward `port` to this computer and show it in the session's Browser tab, or in the system browser
    /// once the session is no longer on screen.
    fn cloud_preview(&mut self, id: &str, port: u16, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.cloud_panel(id) else {
            return;
        };
        panel.problem = None;
        let machine = panel.machine.clone();
        let id = id.to_string();
        let Some(cloud) = crate::cloud::get() else {
            if let Some(panel) = self.cloud_panel(&id) {
                panel.problem = Some(NOT_HERE.into());
            }
            cx.notify();
            return;
        };
        super::super::cloud_view::on_runtime(
            window,
            cx,
            async move { cloud.preview(&machine, port).await },
            move |this, answer, window, cx| match answer {
                Ok(url) => {
                    if matches!(&this.view, View::Code(view) if view.id == id) {
                        this.overlay.close_code_top();
                        this.show_in_side_browser(&url, window, cx);
                    } else {
                        crate::external_link::open(&url, cx);
                    }
                }
                Err(error) => {
                    if let Some(panel) = this.cloud_panel(&id) {
                        panel.problem = Some(error.into());
                    }
                }
            },
        );
        cx.notify();
    }

    /// Copy the provider's link to `port`: anyone holding it gets in.
    fn cloud_share(&mut self, id: &str, port: u16, cx: &mut Context<Self>) {
        let Some(panel) = self.cloud_panel(id) else {
            return;
        };
        panel.problem = None;
        let machine = panel.machine.clone();
        let id = id.to_string();
        on_cloud(
            cx,
            move |cloud| async move { cloud.share(&machine, port).await },
            move |this, answer, cx| match answer {
                Ok(url) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(url));
                    let Some(panel) = this.cloud_panel(&id) else {
                        return;
                    };
                    panel.shared = Some(port);
                    panel.copied = true;
                    panel.copies += 1;
                    let copy = panel.copies;
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(COPIED_FOR).await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(panel) = this.cloud_panel(&id)
                                && panel.copies == copy
                            {
                                panel.copied = false;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
                Err(error) => {
                    if let Some(panel) = this.cloud_panel(&id) {
                        panel.problem = Some(error.into());
                    }
                }
            },
        );
        cx.notify();
    }

    /// Watch desktop: the machine's live screen, in the system browser.
    fn cloud_desktop(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(panel) = self.cloud_panel(id) else {
            return;
        };
        panel.problem = None;
        let machine = panel.machine.clone();
        let id = id.to_string();
        on_cloud(
            cx,
            move |cloud| async move { cloud.desktop(&machine).await },
            move |this, answer, cx| match answer {
                Ok(url) => crate::external_link::open(&url, cx),
                Err(error) => {
                    if let Some(panel) = this.cloud_panel(&id) {
                        panel.problem = Some(error.into());
                    }
                }
            },
        );
        cx.notify();
    }

    /// The Cloud button, with its panel under it while open. None when the session runs elsewhere.
    pub(super) fn render_cloud_button(&self, id: &str, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let machine = self.cloud_machine_of(id)?;
        let label = words::cloud_view(&machine.info, &machine.status, crate::code::now_ms()).label;
        let open = matches!(self.overlay.code_top(), Some(TopPanel::Cloud(panel)) if panel.id == id);
        let tip: SharedString = words::panel_tip(label).into();
        let toggle = id.to_string();
        let button = bar_button("code-cloud", Lucide::Cloud, palette)
            .when(open, |button| button.bg(palette.selected))
            .when(!open, |button| {
                button.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .menu_trigger(
                open,
                cx,
                {
                    let id = id.to_string();
                    move |this| matches!(this.overlay.code_top(), Some(TopPanel::Cloud(panel)) if panel.id == id)
                },
                move |this, _, cx| this.toggle_cloud_panel(&toggle, cx),
            );
        let panel = match self.overlay.code_top() {
            Some(TopPanel::Cloud(panel)) if open => Some(self.render_cloud_panel(panel, machine, palette, cx)),
            _ => None,
        };
        Some(div().relative().child(button).children(panel).into_any_element())
    }

    fn render_cloud_panel(
        &self,
        panel: &CloudPanel,
        machine: &CloudView,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = panel.id.clone();
        let title = |text: SharedString| {
            div()
                .text_color(palette.foreground)
                .font_weight(FontWeight::MEDIUM)
                .child(text)
        };
        let note = |text: SharedString| div().text_size(px(12.)).text_color(palette.muted).child(text);
        let mut card = menu_box("cloud-panel", 320., palette)
            .p(px(12.))
            .gap(px(8.))
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if matches!(this.overlay.code_top(), Some(TopPanel::Cloud(_))) {
                    this.overlay.close_code_top();
                    cx.notify();
                }
            }));

        if !running(machine) {
            let label = words::cloud_view(&machine.info, &machine.status, crate::code::now_ms()).label;
            card = card
                .child(title(words::panel_waiting(label).into()))
                .child(note(words::PANEL_WAITING_NOTE.into()));
        } else {
            let again = id.clone();
            card = card.child(
                h_flex().justify_between().child(title("Open ports".into())).child(
                    div()
                        .id("cloud-ports-again")
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .when(panel.checking, |button| button.opacity(0.45))
                        .when(!panel.checking, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| menu_item_hover(style, palette))
                                .on_click(cx.listener(move |this, _, _, cx| this.check_ports(&again, cx)))
                        })
                        .tooltip(|window, cx| Tooltip::new("Check again").build(window, cx))
                        .child(glyph(Lucide::RefreshCw, palette.icon).size(px(14.))),
                ),
            );
            match &panel.ports {
                None if panel.checking => card = card.child(note(words::PANEL_CHECKING.into())),
                None => {}
                Some(ports) if ports.is_empty() => card = card.child(note(words::PANEL_NO_PORTS.into())),
                Some(ports) => {
                    card = card.child(
                        v_flex()
                            .mx(px(-4.))
                            .children(ports.iter().map(|entry| self.render_port(&id, entry, palette, cx))),
                    );
                }
            }
            if let Some(port) = panel.shared {
                card = card.child(note(words::shared_line(port, panel.copied).into()));
            }
            let desktop = id.clone();
            card = card.child(
                h_flex()
                    .id("cloud-desktop")
                    .mx(px(-8.))
                    .h(px(32.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|style| menu_item_hover(style, palette))
                    .on_click(cx.listener(move |this, _, _, cx| this.cloud_desktop(&desktop, cx)))
                    .child(glyph(Lucide::Monitor, palette.icon).size(px(14.)))
                    .child(div().flex_1().child("Watch desktop"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child("Opens in your browser"),
                    ),
            );
        }
        if let Some(problem) = &panel.problem {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.destructive)
                    .child(problem.clone()),
            );
        }
        popover(card, palette)
    }

    /// One port: its number and process, Preview and the share link.
    fn render_port(&self, id: &str, entry: &CloudPort, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let port = entry.port;
        let preview = id.to_string();
        let share = id.to_string();
        let preview_tip: SharedString = format!("Open port {port} on this computer").into();
        h_flex()
            .id(("cloud-port", usize::from(port)))
            .h(px(32.))
            .pl(px(8.))
            .pr(px(4.))
            .gap(px(2.))
            .rounded(px(8.))
            .hover(|style| menu_item_hover(style, palette))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_none()
                            .font_family("monospace")
                            .text_color(palette.foreground)
                            .child(format!(":{port}")),
                    )
                    .children(entry.process.clone().map(|process| {
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(process)
                    })),
            )
            .child(
                div()
                    .id(("cloud-port-preview", usize::from(port)))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.selected))
                    .tooltip(move |window, cx| Tooltip::new(preview_tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.cloud_preview(&preview, port, window, cx)))
                    .child("Preview"),
            )
            .child(
                div()
                    .id(("cloud-port-share", usize::from(port)))
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.selected))
                    .tooltip(|window, cx| Tooltip::new("Copy a share link").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.cloud_share(&share, port, cx)))
                    .child(glyph(Lucide::Link2, palette.icon).size(px(14.))),
            )
            .into_any_element()
    }
}
