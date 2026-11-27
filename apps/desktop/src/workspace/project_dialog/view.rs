//! How Add a project looks: one title, a Folder | Clone switch, the computer as a chip with its menu,
//! the mode's body, and a footer that is always there. It fades in and rises 8 px as it opens (`motion::arrive`).
use super::*;
use crate::machine_view::{Action, Tone, status_view};
use crate::motion::{self, presence};
use crate::pages::PageId;
use crate::palette::Palette;
use crate::side::icon_button_when;
use crate::workspace::clone_form::{clone_progress, ready};
use crate::workspace::code_dock::MenuTrigger as _;
use crate::workspace::glass::FloatingGround;
use crate::workspace::{SettingsSection, glyph};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};

impl Workspace {
    pub(in crate::workspace) fn render_project_dialog(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.project_dialog.as_ref()?;
        let remote = dialog.machine.as_deref();
        let server = remote.and_then(|id| self.servers.iter().find(|server| server.id == id));
        let name = server
            .map(|server| server.name.as_str())
            .unwrap_or(if remote.is_some() {
                "Unavailable computer"
            } else {
                crate::code::THIS_COMPUTER
            });
        let connected = self.project_target_ready(remote, false);
        let busy = self.project_dialog_busy();
        let body = if !connected {
            self.render_project_unreachable(name, palette, cx)
        } else {
            match dialog.mode {
                Mode::Folder => self.render_project_folder(name, palette, cx),
                Mode::Clone => self.render_project_clone(name, palette, cx),
            }
        };
        let panel = v_flex()
            .id("project-dialog")
            .debug_selector(|| "project-dialog".into())
            .occlude()
            .relative()
            .w(px(520.))
            .max_w(relative(0.94))
            .max_h(relative(0.92))
            .rounded(px(16.))
            .floating_ground(16., palette.pop, false, &palette)
            .shadow(palette.pop_shadow(palette.border))
            .text_size(px(13.5))
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .px(px(24.))
                    .pt(px(20.))
                    .pb(px(12.))
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Add a project"),
                    )
                    .child(
                        icon_button_when("project-close", Lucide::X, !busy, palette)
                            .debug_selector(|| "project-close".into())
                            .aria_label("Close")
                            .track_focus(&dialog.controls[CLOSE])
                            .border_1()
                            .border_color(transparent_black())
                            .focus(|style| style.border_color(palette.muted))
                            .tooltip(|window, cx| Tooltip::new("Close  Esc").build(window, cx))
                            .when(!busy, |button| {
                                button
                                    .on_click(cx.listener(|this, _, window, cx| this.close_project_dialog(window, cx)))
                            }),
                    ),
            )
            .child(self.render_project_toolbar(name, palette, cx))
            .child(
                div()
                    .id("project-dialog-scroll")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h(px(0.))
                    .px(px(24.))
                    .pb(px(20.))
                    .child(body),
            )
            .child(self.render_project_footer(palette, cx))
            .children(self.render_project_computers(palette, cx));
        Some(
            deferred(motion::fade_in(
                div()
                    .id("project-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    // A fixed top, not centred: the title and the field stay put while the list grows and shrinks.
                    .flex()
                    .items_start()
                    .justify_center()
                    .pt(relative(0.12))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_project_dialog(window, cx)),
                    )
                    .child(motion::arrive(panel, "project-dialog-arrive", 8.)),
                "project-overlay-fade",
            ))
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// Folder | Clone, and "on <computer>" as a chip that opens the computer menu.
    fn render_project_toolbar(&self, name: &str, palette: Palette, cx: &mut Context<Self>) -> Div {
        let dialog = self.project_dialog.as_ref().unwrap();
        let busy = self.project_dialog_busy();
        let tab = |control: usize, mode: Mode, icon: Lucide, label: &'static str, cx: &mut Context<Self>| {
            let on = dialog.mode == mode;
            h_flex()
                .id(("project-mode", control))
                .debug_selector(move || format!("project-mode-{label}"))
                .track_focus(&dialog.controls[control])
                .h(px(28.))
                .px(px(12.))
                .gap(px(8.))
                .rounded(px(8.))
                .font_weight(FontWeight::MEDIUM)
                .border_1()
                .border_color(transparent_black())
                .focus(|style| style.border_color(palette.muted))
                .text_color(if busy && !on {
                    palette.faint
                } else if on {
                    palette.foreground
                } else {
                    palette.muted
                })
                .when(on, |tab| tab.bg(palette.selected))
                .child(glyph(icon, if on { palette.foreground } else { palette.icon }).size(px(13.)))
                .child(label)
                .when(!busy && !on, |tab| {
                    tab.cursor_pointer()
                        .hover(|style| style.text_color(palette.foreground))
                        .on_click(cx.listener(move |this, _, window, cx| this.project_mode(mode, window, cx)))
                })
        };
        let server = dialog
            .machine
            .as_deref()
            .and_then(|id| self.servers.iter().find(|server| server.id == id));
        let dot = match server.map(|server| status_view(&server.status).tone) {
            Some(Tone::Ok) | None => palette.muted,
            Some(Tone::Busy) => palette.waiting(),
            Some(Tone::Warn | Tone::Off) => palette.destructive,
        };
        let open = dialog.computers;
        let chip = h_flex()
            .id("project-computer")
            .debug_selector(|| "project-computer".into())
            .track_focus(&dialog.controls[COMPUTER])
            .h(px(28.))
            .pl(px(8.))
            .pr(px(6.))
            .gap(px(8.))
            .rounded_full()
            .border_1()
            .border_color(palette.border)
            .focus(|style| style.border_color(palette.muted))
            .bg(if open { palette.selected } else { palette.hover })
            .font_weight(FontWeight::MEDIUM)
            .child(
                glyph(
                    if dialog.machine.is_some() {
                        Lucide::Server
                    } else {
                        Lucide::Laptop
                    },
                    palette.icon,
                )
                .size(px(13.)),
            )
            .child(div().max_w(px(160.)).truncate().child(name.to_string()))
            .child(div().size(px(7.)).rounded_full().bg(dot))
            .when(!busy, |chip| {
                chip.cursor_pointer()
                    .child(glyph(Lucide::ChevronDown, palette.icon).size(px(12.)))
                    .menu_trigger(
                        open,
                        cx,
                        |this: &Workspace| this.project_dialog.as_ref().is_some_and(|dialog| dialog.computers),
                        |this, _, cx| this.toggle_project_computers(cx),
                    )
            });
        h_flex()
            .px(px(24.))
            .pb(px(16.))
            .gap(px(8.))
            .child(
                h_flex()
                    .p(px(4.))
                    .gap(px(4.))
                    .rounded(px(10.))
                    .bg(palette.background)
                    .border_1()
                    .border_color(palette.border)
                    .child(tab(FOLDER_TAB, Mode::Folder, Lucide::Folder, "Folder", cx))
                    .child(tab(CLONE_TAB, Mode::Clone, Lucide::GitBranch, "Clone", cx)),
            )
            .child(div().flex_1())
            .child(div().text_color(palette.faint).child("on"))
            .child(chip)
    }

    /// The computer menu under the chip: each computer with its status. It opens and closes as menus do.
    fn render_project_computers(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.project_dialog.as_ref()?;
        let phase = presence("project-computers", dialog.computers, cx)?;
        let mut list = v_flex()
            .id("project-computer-list")
            .max_h(px(280.))
            .overflow_y_scroll()
            .track_scroll(&dialog.scroll)
            .gap(px(2.));
        for index in 0..=self.servers.len() {
            let server = index.checked_sub(1).and_then(|index| self.servers.get(index));
            let machine = server.map(|server| server.id.clone());
            let title = server
                .map(|server| server.name.as_str())
                .unwrap_or(crate::code::THIS_COMPUTER);
            let view = server.map(|server| status_view(&server.status));
            let (status, dot) = match &view {
                None => ("Local files and Git", None),
                Some(view) => (
                    view.label,
                    Some(match view.tone {
                        Tone::Ok => palette.muted,
                        Tone::Busy => palette.waiting(),
                        Tone::Warn | Tone::Off => palette.destructive,
                    }),
                ),
            };
            list = list.child(
                h_flex()
                    .id(("project-machine", index))
                    .min_h(px(40.))
                    .px(px(8.))
                    .py(px(4.))
                    .gap(px(12.))
                    .rounded(px(8.))
                    .when(index == dialog.cursor, |row| row.bg(palette.selected))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .child(glyph(
                        if server.is_some() {
                            Lucide::Server
                        } else {
                            Lucide::Laptop
                        },
                        palette.icon,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().truncate().child(title.to_string()))
                            .child(
                                h_flex()
                                    .gap(px(6.))
                                    .text_size(px(12.))
                                    .text_color(palette.faint)
                                    .children(dot.map(|dot| div().size(px(6.)).rounded_full().bg(dot)))
                                    .child(status),
                            ),
                    )
                    .when(machine == dialog.machine, |row| {
                        row.child(glyph(Lucide::Check, palette.muted))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_project_computer(machine.clone(), window, cx)
                    })),
            );
        }
        let manage = h_flex()
            .id("project-manage-computers")
            .h(px(32.))
            .px(px(8.))
            .rounded(px(8.))
            .text_color(palette.muted)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .child("Manage computers…")
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(dialog) = &mut this.project_dialog {
                    dialog.computers = false;
                }
                this.close_project_dialog(window, cx);
                this.open_settings(SettingsSection::Page(PageId::Servers), window, cx);
            }));
        let menu = v_flex()
            .id("project-computer-menu")
            .debug_selector(|| "project-computer-menu".into())
            .occlude()
            .w(px(280.))
            .p(px(4.))
            .rounded(px(14.))
            .floating_ground(14., palette.pop, false, &palette)
            .shadow(palette.pop_shadow(palette.border))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if let Some(dialog) = &mut this.project_dialog {
                    dialog.computers = false;
                    cx.notify();
                }
            }))
            .child(list)
            .child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(palette.border))
            .child(manage);
        Some(
            div()
                .absolute()
                .top(px(100.))
                .right(px(24.))
                .child(phase.menu("project-computers", -4., menu))
                .into_any_element(),
        )
    }

    /// A computer that cannot take a project now: why, and the one thing to do.
    fn render_project_unreachable(&self, name: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let dialog = self.project_dialog.as_ref().unwrap();
        let remote = dialog.machine.clone();
        let view = remote
            .as_deref()
            .and_then(|id| self.servers.iter().find(|server| server.id == id))
            .map(|server| status_view(&server.status));
        let status = view.as_ref().map_or(
            if remote.is_some() {
                "not configured"
            } else {
                "unavailable"
            },
            |view| view.label,
        );
        let action = view.as_ref().and_then(|view| view.action);
        empty_state(
            if remote.is_some() {
                Lucide::Server
            } else {
                Lucide::Laptop
            },
            format!("{name} is {}.", status.trim_end_matches('…').to_lowercase()),
            if remote.is_some() {
                "Connect it to browse its folders. What you typed stays."
            } else {
                "This window cannot add projects on this computer."
            },
            palette,
        )
        .when_some(action, |body, action| {
            let id = remote.unwrap_or_default();
            body.child(
                project_button(
                    "project-reconnect",
                    if action == Action::Retry {
                        "Reconnect"
                    } else {
                        "Open computer settings"
                    },
                    false,
                    true,
                    palette,
                )
                .track_focus(&dialog.controls[RECOVER])
                .on_click(cx.listener(move |this, _, window, cx| {
                    if action == Action::Retry {
                        if let Err(error) = crate::machines::get().reconnect(&id) {
                            eprintln!("nexus-desktop-gpui: reconnect {id}: {error}");
                        }
                    } else {
                        this.close_project_dialog(window, cx);
                        this.open_settings(SettingsSection::Page(PageId::Servers), window, cx);
                    }
                    cx.notify();
                })),
            )
        })
        .into_any_element()
    }

    /// Folder mode: the folder field with its list (`browser.rs`), and Browse… on this computer.
    fn render_project_folder(&self, name: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let dialog = self.project_dialog.as_ref().unwrap();
        let Some(browser) = dialog.browser.clone() else {
            return div().into_any_element();
        };
        let local = dialog.machine.is_none();
        let busy = self.project_dialog_busy();
        v_flex()
            .gap(px(8.))
            .child(
                h_flex()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(div().flex_1().child(format!("Folder on {name}")))
                    .when(local, |row| {
                        row.child(
                            div()
                                .id("project-browse")
                                .debug_selector(|| "project-browse".into())
                                .track_focus(&dialog.controls[BROWSE])
                                .px(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(transparent_black())
                                .focus(|style| style.border_color(palette.muted))
                                .text_color(if busy { palette.faint } else { palette.muted })
                                .when(!busy, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(palette.foreground))
                                        .on_click(
                                            cx.listener(|this, _, window, cx| this.browse_project_folder(window, cx)),
                                        )
                                })
                                .child("Browse…"),
                        )
                    }),
            )
            .child(browser)
            .into_any_element()
    }

    /// Clone mode: Repository, Clone into, where it lands, and git's progress while it runs.
    fn render_project_clone(&self, name: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let dialog = self.project_dialog.as_ref().unwrap();
        let remote = dialog.machine.as_deref();
        let busy = self.project_dialog_busy();
        let clones = self.project_target_ready(remote, true);
        if !clones {
            return empty_state(
                Lucide::GitBranch,
                format!("{name}'s Nexus host cannot clone yet."),
                "Update it in computer settings, or add a folder that is already on it.",
                palette,
            )
            .child(
                project_button("project-update-host", "Open computer settings", false, true, palette)
                    .track_focus(&dialog.controls[RECOVER])
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_project_dialog(window, cx);
                        this.open_settings(SettingsSection::Page(PageId::Servers), window, cx);
                    })),
            )
            .into_any_element();
        }
        let form = self
            .clone_form
            .as_ref()
            .map(|form| (&form.source, &form.parent, &form.progress, &form.problem))
            .or_else(|| {
                self.server_folder
                    .as_ref()
                    .and_then(|form| form.clone.as_ref())
                    .map(|form| (&form.source, &form.parent, &form.progress, &form.problem))
            });
        let Some((source, parent, progress, error)) = form else {
            return div().into_any_element();
        };
        let mut body = v_flex()
            .gap(px(8.))
            .child(project_field("Repository", source, busy, Lucide::GitBranch, palette))
            .child(hint("A repository URL, or owner/name on GitHub.", palette))
            .child(
                h_flex()
                    .pt(px(8.))
                    .gap(px(8.))
                    .items_end()
                    .child(div().flex_1().min_w(px(0.)).child(project_field(
                        "Clone into",
                        parent,
                        busy,
                        Lucide::Folder,
                        palette,
                    )))
                    .when(remote.is_none(), |row| {
                        row.child(
                            project_button("project-choose-folder", "Choose…", false, !busy, palette)
                                .track_focus(&dialog.controls[CHOOSE])
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.project_dialog_busy() {
                                        this.choose_clone_parent(window, cx);
                                    }
                                })),
                        )
                    }),
            );
        if let Some(destination) =
            destination_preview(source.read(cx).value().as_ref(), parent.read(cx).value().as_ref())
        {
            body = body.child(hint(
                format!("Clones into {destination} on {name}, with its Git sign-in."),
                palette,
            ));
        }
        if busy {
            body = body.child(div().pt(px(8.)).child(clone_progress(progress.as_ref(), palette)));
        }
        body.when_some(error.clone(), |body, error| body.child(problem(&error, palette)))
            .into_any_element()
    }

    /// Always there, so the main action never moves: a keyboard hint, Cancel, and Add or Clone.
    fn render_project_footer(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let dialog = self.project_dialog.as_ref().unwrap();
        let remote = dialog.machine.as_deref();
        let busy = self.project_dialog_busy();
        let clone = dialog.mode == Mode::Clone;
        let uncertain = self
            .server_folder
            .as_ref()
            .and_then(|form| form.clone.as_ref())
            .is_some_and(|clone_form| clone && clone_form.uncertain);
        let browser = dialog.browser.as_ref().filter(|_| !clone);
        let target = browser
            .filter(|browser| !browser.read(cx).blocked(cx))
            .and_then(|browser| browser.read(cx).target(cx));
        let reachable = self.project_target_ready(remote, clone);
        let opens = target.as_ref().is_some_and(|target| target.project.is_some());
        let can_submit = !busy
            && !uncertain
            && self.project_target_ready(remote, clone)
            && if clone {
                match self.project_inputs(cx).as_slice() {
                    [source, parent] => ready(source.read(cx).value().as_ref(), parent.read(cx).value().as_ref()),
                    _ => false,
                }
            } else {
                target.is_some()
            };
        let label = match (busy, clone, opens) {
            (true, true, _) => "Cloning…",
            (true, false, _) => "Adding…",
            (false, true, _) => "Clone",
            (false, false, true) => "Open project",
            (false, false, false) => "Add project",
        };
        h_flex()
            .px(px(24.))
            .py(px(16.))
            .gap(px(8.))
            .border_t_1()
            .border_color(palette.border)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(if busy {
                        "Git and files stay on the computer you chose."
                    } else if !reachable {
                        ""
                    } else if clone {
                        "Enter clones"
                    } else {
                        "↑ ↓ choose · Tab completes · Enter adds"
                    }),
            )
            .child(
                project_button("project-cancel", "Cancel", false, !busy, palette)
                    .track_focus(&dialog.controls[CANCEL])
                    .on_click(cx.listener(|this, _, window, cx| this.close_project_dialog(window, cx))),
            )
            .child(
                project_button("project-submit", label, true, can_submit, palette)
                    .track_focus(&dialog.controls[SUBMIT])
                    .when(can_submit, |button| {
                        button.on_click(cx.listener(|this, _, window, cx| this.submit_project(window, cx)))
                    }),
            )
    }
}

fn hint(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .px(px(4.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(text.into())
}

/// A body that has nothing to fill in: an icon, what is wrong, what to do, and room for its one button.
fn empty_state(icon: Lucide, title: String, detail: &'static str, palette: Palette) -> Div {
    v_flex()
        .items_start()
        .gap(px(12.))
        .pt(px(8.))
        .child(glyph(icon, palette.icon).size(px(20.)))
        .child(div().text_size(px(15.)).font_weight(FontWeight::MEDIUM).child(title))
        .child(div().text_color(palette.muted).child(detail))
}

fn problem(error: &str, palette: Palette) -> Div {
    h_flex()
        .px(px(4.))
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(palette.destructive)
        .child(glyph(Lucide::TriangleAlert, palette.destructive))
        .child(div().flex_1().min_w(px(0.)).child(error.to_string()))
}

fn project_field(
    label: &'static str,
    input: &Entity<InputState>,
    disabled: bool,
    icon: Lucide,
    palette: Palette,
) -> Stateful<Div> {
    v_flex()
        .id(SharedString::from(format!("project-field-{label}")))
        .debug_selector(move || format!("project-field-{label}"))
        .gap(px(8.))
        .child(div().text_size(px(13.)).text_color(palette.muted).child(label))
        .child(
            h_flex()
                .min_w(px(0.))
                .h(px(40.))
                .px(px(12.))
                .gap(px(8.))
                .rounded(px(12.))
                .border_1()
                .border_color(palette.border)
                .bg(if disabled { palette.sidebar } else { palette.background })
                .child(glyph(icon, palette.icon))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .font_family(crate::pages::ui::MONO)
                        .text_size(px(13.))
                        .child(Input::new(input).appearance(false).disabled(disabled)),
                ),
        )
}

fn project_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    enabled: bool,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .h(px(36.))
        .px(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .font_weight(FontWeight::MEDIUM)
        .text_color(if primary && enabled {
            palette.primary_foreground
        } else if enabled {
            palette.foreground
        } else {
            palette.faint
        })
        .bg(if primary && enabled {
            palette.primary
        } else {
            palette.selected
        })
        .border_1()
        .border_color(transparent_black())
        .focus(|style| style.border_color(palette.muted))
        .when(enabled, |button| button.cursor_pointer())
        .child(label)
}

fn destination_preview(source: &str, parent: &str) -> Option<String> {
    nexus_workbench::clone::parse(source).ok()?;
    if parent.trim().is_empty() {
        return None;
    }
    let name = nexus_workbench::clone::name_of(source.trim())?;
    Some(format!("{}/{name}", parent.trim().trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::destination_preview;
    use core::prelude::v1::test;
    #[test]
    fn the_preview_uses_the_targets_path_for_github_and_ssh_sources() {
        assert_eq!(
            destination_preview("owner/repo", "~/projects"),
            Some("~/projects/repo".into())
        );
        assert_eq!(
            destination_preview("git@example.com:org/repo.git", "/srv/code/"),
            Some("/srv/code/repo".into())
        );
        assert_eq!(destination_preview("invalid source", "/srv/code"), None);
        assert_eq!(destination_preview("owner/repo", ""), None);
    }
}
