//! The one menu, popover or dialog open over the window. Opening one closes the one before it, and Esc
//! closes whichever is open before it reaches a running turn's Stop or an approval's Deny (`escape_tests.rs`).
//!
//! Not here, because each can stay open beside a menu or is not one: a title being renamed in its row or the top bar,
//! the Add project dialog with its clone form (`clone_form` is also that dialog's Clone step), Rewind's editor, the
//! share sheets, a Code session's pull request form (`pr_form`: its typed title and description and a create in flight
//! outlive a popup opened over it; a top bar panel still replaces it, and Esc closes it after any overlay), the crash
//! prompt (it holds the crash until answered, and stays while it sends), a server's folder field, the focus a session
//! menu returns to (`session_anchor`, it outlives the menu), the custom snooze field (a page of the session menu,
//! cleared with it), and a Code session's dock menus and the git panel's editor menu (kept with the session and the
//! panel that hold them).

use super::Workspace;
use super::chat_menu::ChatMenu;
use super::chat_rows::RowMenu;
use super::code_title::TopPanel;
use super::image_viewer::ImageViewer;
use super::link_menu::LinkPopup;
use super::new_code::LaunchMenu;
use super::org_menu::OrgMenu;
use super::session_menu::SessionMenu;
use gpui_kit::Context;

// One per window, set on a click or a key, so its size is not worth a `Box` on the big variants.
#[allow(clippy::large_enum_variant)]
pub(super) enum Overlay {
    /// The sidebar's person menu.
    PersonMenu,
    /// The organization switcher.
    OrgMenu(OrgMenu),
    /// The panel's ⋮ menu (New chat, Settings) (`panel_view.rs`).
    PanelMenu,
    /// A sidebar project's menu (Pin, Archive), by the project's id.
    ProjectMenu(String),
    /// A Code session's menu, from its row or the top bar.
    SessionMenu(SessionMenu),
    /// The open chat's "More actions" menu.
    ChatMenu(ChatMenu),
    /// A chat row's menu.
    RowMenu(RowMenu),
    /// One of the new session's menus (project, workspace, machine, model, background).
    LaunchMenu(LaunchMenu),
    /// A link's right-click menu (`link_menu.rs`).
    LinkPopup(LinkPopup),
    /// The chat composer's model menu.
    ModelMenu,
    /// The composer's "+" menu.
    AttachMenu,
    /// The microphone menu (`mic.rs`).
    MicMenu,
    /// The chat composer's team menu (`chat_choices.rs`).
    TeamMenu,
    /// The sidebar's "+" menu over the groups (`sidebar_groups.rs`).
    AddGroupMenu,
    /// The prompt stash's menu (`stash.rs`).
    StashMenu,
    /// The usage popover over the composer's usage gauge (`usage_meter.rs`).
    UsageMeter,
    /// What teammates see, from the sidebar Team head's info icon (`team_presence.rs`).
    TeamSharing,
    /// The open Code session's top bar panel: its meter, commands, git panel, pull request card, Cloud. Opening one
    /// also closes the pull request form (`pr_form`), as one top bar panel replaced another before.
    CodeTop(TopPanel),
    /// The search palette, with the organizations its `@` scope lists once they are read.
    Palette(Option<OrgMenu>),
    /// An image shown full size over the window.
    ImageViewer(ImageViewer),
    /// Send Feedback… (`feedback_dialog.rs`).
    Feedback,
    /// The Assistant's ⋯ in the title bar: New conversation and its views (A24·c, `assistant_title.rs`).
    AssistantMenu,
}

impl Workspace {
    /// Opens `overlay`, and so closes any other one.
    pub(super) fn open_overlay(&mut self, overlay: Overlay) {
        self.close_overlay();
        if matches!(overlay, Overlay::CodeTop(_)) {
            self.pr_form = None;
        }
        self.overlay = Some(overlay);
    }

    /// Closes whichever overlay is open, and returns it.
    pub(super) fn close_overlay(&mut self) -> Option<Overlay> {
        let closed = self.overlay.take();
        if matches!(closed, Some(Overlay::SessionMenu(_))) {
            self.custom_snooze = None;
        }
        closed
    }

    /// Closes `overlay` if one of its kind is open, else opens it.
    pub(super) fn toggle_overlay(&mut self, overlay: Overlay) {
        let same = self
            .overlay
            .as_ref()
            .is_some_and(|open| std::mem::discriminant(open) == std::mem::discriminant(&overlay));
        if same {
            self.close_overlay();
        } else {
            self.open_overlay(overlay);
        }
    }

    /// Esc: closes the open overlay, if there is one, with what its own close does. False when none is open.
    pub(super) fn escape_overlay(&mut self, window: &mut gpui_kit::Window, cx: &mut Context<Self>) -> bool {
        match &self.overlay {
            None => return false,
            Some(Overlay::SessionMenu(_)) => {
                self.renaming = None;
                self.restore_session_anchor(window, cx);
            }
            Some(Overlay::MicMenu) => self.close_mic_menu(),
            Some(Overlay::Palette(_)) => {
                self.close_palette(window, cx);
                return true;
            }
            _ => {}
        }
        self.close_overlay();
        cx.notify();
        true
    }
}

/// Reads and closes one kind of overlay. A close is a no-op while another kind is open, so a late close (a menu's
/// mouse down outside, an answer that arrives after) never shuts the overlay that replaced it.
macro_rules! slots {
    (
        unit { $($unit:ident: $is:ident, $close_unit:ident;)* }
        copy { $($copy:ident($copy_ty:ty): $get_copy:ident, $close_copy:ident;)* }
        by_ref { $($by_ref:ident($ref_ty:ty): $get:ident, $get_mut:ident, $close:ident;)* }
    ) => {
        // Every kind gets each accessor, used or not yet.
        #[allow(dead_code)]
        pub(super) trait OverlaySlot {
            $(
                fn $is(&self) -> bool;
                fn $close_unit(&mut self) -> bool;
            )*
            $(
                fn $get_copy(&self) -> Option<$copy_ty>;
                fn $close_copy(&mut self) -> Option<$copy_ty>;
            )*
            $(
                fn $get(&self) -> Option<&$ref_ty>;
                fn $get_mut(&mut self) -> Option<&mut $ref_ty>;
                fn $close(&mut self) -> Option<$ref_ty>;
            )*
        }

        impl OverlaySlot for Option<Overlay> {
            $(
                fn $is(&self) -> bool {
                    matches!(self, Some(Overlay::$unit))
                }
                fn $close_unit(&mut self) -> bool {
                    let open = self.$is();
                    if open {
                        *self = None;
                    }
                    open
                }
            )*
            $(
                fn $get_copy(&self) -> Option<$copy_ty> {
                    match self {
                        Some(Overlay::$copy(open)) => Some(*open),
                        _ => None,
                    }
                }
                fn $close_copy(&mut self) -> Option<$copy_ty> {
                    let open = self.$get_copy();
                    if open.is_some() {
                        *self = None;
                    }
                    open
                }
            )*
            $(
                fn $get(&self) -> Option<&$ref_ty> {
                    match self {
                        Some(Overlay::$by_ref(open)) => Some(open),
                        _ => None,
                    }
                }
                fn $get_mut(&mut self) -> Option<&mut $ref_ty> {
                    match self {
                        Some(Overlay::$by_ref(open)) => Some(open),
                        _ => None,
                    }
                }
                fn $close(&mut self) -> Option<$ref_ty> {
                    match self.take() {
                        Some(Overlay::$by_ref(open)) => Some(open),
                        other => {
                            *self = other;
                            None
                        }
                    }
                }
            )*
        }
    };
}

slots! {
    unit {
        PersonMenu: person_menu, close_person_menu;
        PanelMenu: panel_menu, close_panel_menu;
        ModelMenu: model_menu, close_model_menu;
        AttachMenu: attach_menu, close_attach_menu;
        MicMenu: mic_menu, close_mic_menu;
        TeamMenu: team_menu, close_team_menu;
        AddGroupMenu: add_group_menu, close_add_group_menu;
        StashMenu: stash_menu, close_stash_menu;
        UsageMeter: usage_meter, close_usage_meter;
        TeamSharing: team_sharing, close_team_sharing;
        Feedback: feedback, close_feedback;
        AssistantMenu: assistant_menu, close_assistant_menu;
    }
    copy {
        LaunchMenu(LaunchMenu): launch_menu, close_launch_menu;
        ChatMenu(ChatMenu): chat_menu, close_chat_menu;
    }
    by_ref {
        OrgMenu(OrgMenu): org_menu, org_menu_mut, close_org_menu;
        ProjectMenu(String): project_menu, project_menu_mut, close_project_menu;
        SessionMenu(SessionMenu): session_menu, session_menu_mut, close_session_menu;
        RowMenu(RowMenu): row_menu, row_menu_mut, close_row_menu;
        LinkPopup(LinkPopup): link_popup, link_popup_mut, close_link_popup;
        ImageViewer(ImageViewer): image_viewer, image_viewer_mut, close_image_viewer;
        CodeTop(TopPanel): code_top, code_top_mut, close_code_top;
        Palette(Option<OrgMenu>): palette, palette_mut, close_palette;
    }
}
